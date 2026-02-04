//! TOC Generator for Forge README files
//!
//! Generates and maintains table of contents with auto-computed line numbers
//! and cached section descriptions.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

const TOC_START_MARKER: &str = "<!-- toc:start -->";
const TOC_END_MARKER: &str = "<!-- toc:end -->";

#[derive(Parser)]
#[command(name = "toc-gen")]
#[command(about = "Generate and maintain table of contents for markdown files")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Update TOC with current line numbers
    Update {
        /// Markdown file to update
        file: PathBuf,
        /// Generate descriptions for new sections via LLM
        #[arg(long)]
        generate: bool,
    },
    /// Verify TOC is current (exit 1 if stale)
    Check {
        /// Markdown file to check
        file: PathBuf,
    },
    /// Add TOC markers and initial entries
    Init {
        /// Markdown file to initialize
        file: PathBuf,
    },
}

/// A section parsed from markdown
#[derive(Debug)]
struct Section {
    /// Normalized key for TOML lookup (lowercase, spaces to dashes)
    key: String,
    /// Original heading text
    heading: String,
    /// 1-based line number where section starts
    start_line: usize,
    /// 1-based line number where section ends (inclusive)
    end_line: usize,
}

/// Stored descriptions keyed by file
#[derive(Debug, Default, Serialize, Deserialize)]
struct TocDescriptions {
    #[serde(default)]
    sections: HashMap<String, HashMap<String, String>>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Update { file, generate } => update_toc(&file, generate),
        Commands::Check { file } => check_toc(&file),
        Commands::Init { file } => init_toc(&file),
    }
}

/// Normalize heading text to TOML key
fn normalize_key(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse markdown file to extract ## sections with line numbers
fn parse_sections(content: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Find all ## headings with their line numbers
    let mut heading_lines: Vec<(usize, String)> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Match ## but not ### or more
        if trimmed.starts_with("## ") && !trimmed.starts_with("### ") {
            let heading = trimmed[3..].trim().to_string();
            heading_lines.push((idx + 1, heading)); // 1-based line numbers
        }
    }

    // Compute sections with end lines
    for (i, (start_line, heading)) in heading_lines.iter().enumerate() {
        let end_line = if i + 1 < heading_lines.len() {
            heading_lines[i + 1].0 - 1 // Line before next heading
        } else {
            total_lines // Last section goes to end of file
        };

        sections.push(Section {
            key: normalize_key(heading),
            heading: heading.clone(),
            start_line: *start_line,
            end_line,
        });
    }

    sections
}

/// Load descriptions from TOML file
fn load_descriptions(workspace_root: &Path) -> Result<TocDescriptions> {
    let toml_path = workspace_root.join(".toc-descriptions.toml");
    if toml_path.exists() {
        let content = fs::read_to_string(&toml_path)
            .with_context(|| format!("Failed to read {}", toml_path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", toml_path.display()))
    } else {
        Ok(TocDescriptions::default())
    }
}

/// Save descriptions to TOML file, preserving formatting where possible
fn save_descriptions(workspace_root: &Path, descriptions: &TocDescriptions) -> Result<()> {
    let toml_path = workspace_root.join(".toc-descriptions.toml");

    // Use toml_edit to preserve formatting if file exists
    let mut doc = if toml_path.exists() {
        let content = fs::read_to_string(&toml_path)?;
        content.parse::<toml_edit::DocumentMut>().unwrap_or_default()
    } else {
        toml_edit::DocumentMut::new()
    };

    // Add header comment if new file
    if doc.as_table().is_empty() {
        doc.insert(
            "# TOC section descriptions",
            toml_edit::Item::None,
        );
    }

    // Ensure sections table exists
    if !doc.contains_key("sections") {
        doc["sections"] = toml_edit::Item::Table(toml_edit::Table::new());
    }

    // Update sections
    for (file_key, section_map) in &descriptions.sections {
        let sections_table = doc["sections"].as_table_mut().unwrap();
        if !sections_table.contains_key(file_key) {
            sections_table[file_key] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let file_table = sections_table[file_key].as_table_mut().unwrap();

        for (section_key, description) in section_map {
            file_table[section_key] = toml_edit::value(description.as_str());
        }
    }

    fs::write(&toml_path, doc.to_string())
        .with_context(|| format!("Failed to write {}", toml_path.display()))?;

    Ok(())
}

/// Get file key for TOML lookup (relative path from workspace root, normalized)
/// e.g., "README.md" -> "readme", "context/README.md" -> "context-readme"
fn file_key(path: &Path, workspace_root: &Path) -> String {
    let relative = path.strip_prefix(workspace_root).unwrap_or(path);

    relative
        .with_extension("")
        .to_string_lossy()
        .to_lowercase()
        .replace(['/', '\\'], "-")
}

/// Generate TOC table markdown
fn generate_toc_table(sections: &[Section], descriptions: &HashMap<String, String>) -> String {
    let mut lines = vec![
        "| Lines | Section |".to_string(),
        "| --- | --- |".to_string(),
    ];

    for section in sections {
        let description = descriptions
            .get(&section.key)
            .cloned()
            .unwrap_or_else(|| section.heading.clone());

        lines.push(format!(
            "| {}-{} | {} |",
            section.start_line, section.end_line, description
        ));
    }

    lines.join("\n")
}

/// Find workspace root by looking for Cargo.toml with [workspace]
fn find_workspace_root(start: &Path) -> Result<PathBuf> {
    let mut current = start.canonicalize()?;
    loop {
        let cargo_toml = current.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = fs::read_to_string(&cargo_toml)?;
            if content.contains("[workspace]") {
                return Ok(current);
            }
        }
        if !current.pop() {
            bail!("Could not find workspace root (Cargo.toml with [workspace])");
        }
    }
}

/// Update TOC in markdown file
fn update_toc(file: &Path, generate: bool) -> Result<()> {
    let file = file.canonicalize().with_context(|| format!("File not found: {}", file.display()))?;
    let workspace_root = find_workspace_root(&file)?;

    let content = fs::read_to_string(&file)
        .with_context(|| format!("Failed to read {}", file.display()))?;

    // Check for TOC markers
    let start_idx = content.find(TOC_START_MARKER);
    let end_idx = content.find(TOC_END_MARKER);

    let (start_idx, end_idx) = match (start_idx, end_idx) {
        (Some(s), Some(e)) if s < e => (s, e),
        _ => bail!(
            "TOC markers not found or in wrong order. Run `toc-gen init {}` first.",
            file.display()
        ),
    };

    // Parse sections
    let sections = parse_sections(&content);
    if sections.is_empty() {
        println!("No ## sections found in {}", file.display());
        return Ok(());
    }

    // Load and update descriptions
    let mut descriptions = load_descriptions(&workspace_root)?;
    let fkey = file_key(&file, &workspace_root);

    // Ensure file entry exists
    descriptions.sections.entry(fkey.clone()).or_default();

    // Find sections needing descriptions
    let missing: Vec<&Section> = sections
        .iter()
        .filter(|s| !descriptions.sections[&fkey].contains_key(&s.key))
        .collect();

    if !missing.is_empty() {
        let file_descriptions = descriptions.sections.get_mut(&fkey).unwrap();
        if generate {
            #[cfg(feature = "generate")]
            {
                generate_descriptions(&file, &content, &missing, file_descriptions)?;
            }
            #[cfg(not(feature = "generate"))]
            {
                eprintln!(
                    "Warning: --generate flag requires 'generate' feature. Using heading text as placeholder."
                );
                for section in &missing {
                    file_descriptions.insert(section.key.clone(), section.heading.clone());
                }
            }
        } else {
            eprintln!("Warning: {} section(s) missing descriptions:", missing.len());
            for section in &missing {
                eprintln!("  - {} (key: {})", section.heading, section.key);
                // Use heading as placeholder
                file_descriptions.insert(section.key.clone(), section.heading.clone());
            }
            eprintln!("Run with --generate to auto-generate descriptions via LLM.");
        }
    }

    // Save updated descriptions
    save_descriptions(&workspace_root, &descriptions)?;

    // Generate new TOC (clone the file descriptions to avoid borrow issues)
    let file_descriptions = descriptions.sections.get(&fkey).unwrap().clone();
    let toc_table = generate_toc_table(&sections, &file_descriptions);

    // Replace content between markers
    let before_toc = &content[..start_idx + TOC_START_MARKER.len()];
    let after_toc = &content[end_idx..];
    let new_content = format!("{}\n{}\n{}", before_toc, toc_table, after_toc);

    // Write if changed
    if new_content != content {
        fs::write(&file, &new_content)
            .with_context(|| format!("Failed to write {}", file.display()))?;
        println!("Updated TOC in {}", file.display());
    } else {
        println!("TOC already up to date in {}", file.display());
    }

    Ok(())
}

/// Check if TOC is current
fn check_toc(file: &Path) -> Result<()> {
    let file = file.canonicalize().with_context(|| format!("File not found: {}", file.display()))?;
    let workspace_root = find_workspace_root(&file)?;

    let content = fs::read_to_string(&file)
        .with_context(|| format!("Failed to read {}", file.display()))?;

    // Check for TOC markers
    let start_idx = content.find(TOC_START_MARKER);
    let end_idx = content.find(TOC_END_MARKER);

    let (start_idx, end_idx) = match (start_idx, end_idx) {
        (Some(s), Some(e)) if s < e => (s, e),
        _ => bail!("TOC markers not found in {}", file.display()),
    };

    // Parse sections
    let sections = parse_sections(&content);

    // Load descriptions
    let descriptions = load_descriptions(&workspace_root)?;
    let fkey = file_key(&file, &workspace_root);
    let file_descriptions = descriptions.sections.get(&fkey).cloned().unwrap_or_default();

    // Generate expected TOC
    let expected_toc = generate_toc_table(&sections, &file_descriptions);

    // Extract current TOC
    let current_start = start_idx + TOC_START_MARKER.len();
    let current_toc = content[current_start..end_idx].trim();

    if current_toc != expected_toc {
        eprintln!("TOC is stale in {}", file.display());
        eprintln!("Run `just toc {}` to update.", file.display());
        std::process::exit(1);
    }

    println!("TOC is current in {}", file.display());
    Ok(())
}

/// Initialize TOC markers in a file
fn init_toc(file: &Path) -> Result<()> {
    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read {}", file.display()))?;

    // Check if markers already exist
    if content.contains(TOC_START_MARKER) {
        bail!("TOC markers already exist in {}. Use `toc-gen update` instead.", file.display());
    }

    // Find the ## LLM-TOC heading or first ## heading to insert after
    let lines: Vec<&str> = content.lines().collect();
    let mut insert_after_line = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "## LLM-TOC" {
            insert_after_line = Some(idx);
            break;
        }
    }

    // If no ## LLM-TOC found, look for first ## heading
    if insert_after_line.is_none() {
        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("## ") && !trimmed.starts_with("### ") {
                insert_after_line = Some(idx);
                break;
            }
        }
    }

    let insert_after_line = insert_after_line.unwrap_or(0);

    // Build new content
    let mut new_lines: Vec<String> = Vec::new();

    // Add lines up to and including insertion point
    for line in &lines[..=insert_after_line] {
        new_lines.push(line.to_string());
    }

    // Add TOC markers with placeholder
    new_lines.push(TOC_START_MARKER.to_string());
    new_lines.push("| Lines | Section |".to_string());
    new_lines.push("| --- | --- |".to_string());
    new_lines.push(TOC_END_MARKER.to_string());

    // Add remaining lines
    for line in &lines[insert_after_line + 1..] {
        new_lines.push(line.to_string());
    }

    let new_content = new_lines.join("\n");

    // Preserve trailing newline if original had one
    let new_content = if content.ends_with('\n') && !new_content.ends_with('\n') {
        format!("{}\n", new_content)
    } else {
        new_content
    };

    fs::write(file, &new_content)
        .with_context(|| format!("Failed to write {}", file.display()))?;

    println!("Initialized TOC markers in {}. Run `toc-gen update` to populate.", file.display());
    Ok(())
}

/// Generate descriptions via LLM (only compiled with 'generate' feature)
#[cfg(feature = "generate")]
fn generate_descriptions(
    file: &Path,
    content: &str,
    missing: &[&Section],
    file_descriptions: &mut HashMap<String, String>,
) -> Result<()> {
    use tokio::runtime::Runtime;

    let rt = Runtime::new()?;
    rt.block_on(async {
        // Try Anthropic first, then OpenAI
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .with_context(|| "Set ANTHROPIC_API_KEY or OPENAI_API_KEY for --generate")?;

        let is_anthropic = std::env::var("ANTHROPIC_API_KEY").is_ok();

        let client = reqwest::Client::new();

        for section in missing {
            let section_content = extract_section_content(content, section);
            let description = if is_anthropic {
                call_anthropic(&client, &api_key, &section.heading, &section_content).await?
            } else {
                call_openai(&client, &api_key, &section.heading, &section_content).await?
            };

            println!("  Generated: {} -> {}", section.heading, description);
            file_descriptions.insert(section.key.clone(), description);
        }

        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}

#[cfg(feature = "generate")]
fn extract_section_content(content: &str, section: &Section) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = section.start_line.saturating_sub(1);
    let end = section.end_line.min(lines.len());
    lines[start..end].join("\n")
}

#[cfg(feature = "generate")]
async fn call_anthropic(
    client: &reqwest::Client,
    api_key: &str,
    heading: &str,
    section_content: &str,
) -> Result<String> {
    let prompt = format!(
        "Distill this README section into 3-8 words for a table of contents entry. \
        Return ONLY the summary text, no quotes or explanation.\n\n\
        Section heading: {}\n\nContent:\n{}",
        heading,
        &section_content[..section_content.len().min(4000)]
    );

    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "model": "claude-haiku-4-5",
            "max_tokens": 50,
            "messages": [{"role": "user", "content": prompt}]
        }))
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;
    let text = body["content"][0]["text"]
        .as_str()
        .unwrap_or(heading)
        .trim()
        .to_string();

    Ok(text)
}

#[cfg(feature = "generate")]
async fn call_openai(
    client: &reqwest::Client,
    api_key: &str,
    heading: &str,
    section_content: &str,
) -> Result<String> {
    let prompt = format!(
        "Distill this README section into 3-8 words for a table of contents entry. \
        Return ONLY the summary text, no quotes or explanation.\n\n\
        Section heading: {}\n\nContent:\n{}",
        heading,
        &section_content[..section_content.len().min(4000)]
    );

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("content-type", "application/json")
        .json(&serde_json::json!({
            "model": "gpt-4o-mini",
            "max_tokens": 50,
            "messages": [{"role": "user", "content": prompt}]
        }))
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;
    let text = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or(heading)
        .trim()
        .to_string();

    Ok(text)
}

