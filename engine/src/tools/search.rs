//! Local search tool backed by ugrep or ripgrep.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSet};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use unicode_normalization::UnicodeNormalization;

use super::{
    RiskLevel, ToolCtx, ToolError, ToolExecutor, ToolFut, redact_summary, sanitize_output,
};

const SEARCH_TOOL_NAME: &str = "Search";
const SEARCH_TOOL_ALIASES: &[&str] = &["Search", "search", "rg", "ripgrep", "ugrep", "ug"];

#[derive(Debug, Clone)]
pub struct SearchToolConfig {
    pub binary: String,
    pub fallback_binary: String,
    pub default_timeout_ms: u64,
    pub default_max_results: usize,
    pub max_matches_per_file: usize,
    pub max_files: usize,
    pub max_file_size_bytes: u64,
}

impl Default for SearchToolConfig {
    fn default() -> Self {
        Self {
            binary: "ugrep".to_string(),
            fallback_binary: "rg".to_string(),
            default_timeout_ms: 20_000,
            default_max_results: 200,
            max_matches_per_file: 50,
            max_files: 10_000,
            max_file_size_bytes: 2_000_000,
        }
    }
}

#[derive(Debug)]
pub struct SearchTool {
    name: &'static str,
    config: SearchToolConfig,
}

impl SearchTool {
    pub fn with_name(name: &'static str, config: SearchToolConfig) -> Self {
        Self { name, config }
    }

    pub fn aliases() -> &'static [&'static str] {
        SEARCH_TOOL_ALIASES
    }

    async fn select_backend(
        &self,
        need_fuzzy: bool,
        need_context: bool,
    ) -> Result<BackendInfo, ToolError> {
        let primary = probe_backend(&self.config.binary).await;
        let fallback = probe_backend(&self.config.fallback_binary).await;

        let mut candidates = Vec::new();
        if let Some(info) = primary {
            candidates.push(info);
        }
        if let Some(info) = fallback
            && !candidates.iter().any(|c| c.binary == info.binary)
        {
            candidates.push(info);
        }

        if candidates.is_empty() {
            return Err(ToolError::ExecutionFailed {
                tool: SEARCH_TOOL_NAME.to_string(),
                message: "No valid search backend found (ugrep >= 3.0 or rg >= 13.0)".to_string(),
            });
        }

        if need_fuzzy {
            return candidates
                .into_iter()
                .find(|c| matches!(c.kind, BackendKind::Ugrep))
                .ok_or_else(|| ToolError::BadArgs {
                    message: "fuzzy search requires ugrep".to_string(),
                });
        }

        if need_context {
            return candidates
                .into_iter()
                .find(|c| matches!(c.kind, BackendKind::Ripgrep))
                .ok_or_else(|| ToolError::ExecutionFailed {
                    tool: SEARCH_TOOL_NAME.to_string(),
                    message: "context search requires ripgrep".to_string(),
                });
        }

        // Prefer ugrep when possible.
        if let Some(ugrep) = candidates
            .iter()
            .find(|c| matches!(c.kind, BackendKind::Ugrep))
        {
            return Ok(ugrep.clone());
        }

        Ok(candidates
            .into_iter()
            .find(|c| matches!(c.kind, BackendKind::Ripgrep))
            .expect("at least one candidate"))
    }
}

impl ToolExecutor for SearchTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        "Search local files with ugrep or ripgrep"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "pattern": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Regex pattern to search for in file contents. Use the Glob tool to find files by name."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in. Defaults to current working directory."
                },
                "case": {
                    "type": "string",
                    "enum": ["smart", "sensitive", "insensitive"],
                    "default": "smart",
                    "description": "Case sensitivity: 'smart' (case-sensitive if pattern has uppercase), 'sensitive', or 'insensitive'."
                },
                "fixed_strings": {
                    "type": "boolean",
                    "default": false,
                    "description": "Treat pattern as literal string, not regex."
                },
                "word_regexp": {
                    "type": "boolean",
                    "default": false,
                    "description": "Match whole words only."
                },
                "include_glob": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter: only search files matching these patterns (e.g. ['*.rs', '*.toml']). Does NOT find files — use Glob tool for that."
                },
                "exclude_glob": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter: skip files matching these patterns."
                },
                "glob": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Filter: only search files matching these patterns. Alias for include_glob. To find files by name, use the Glob tool instead."
                },
                "recursive": {
                    "type": "boolean",
                    "default": true,
                    "description": "Search subdirectories recursively."
                },
                "hidden": {
                    "type": "boolean",
                    "default": false,
                    "description": "Include hidden files and directories."
                },
                "follow": {
                    "type": "boolean",
                    "default": false,
                    "description": "Follow symbolic links."
                },
                "no_ignore": {
                    "type": "boolean",
                    "default": false,
                    "description": "Don't respect .gitignore and other ignore files."
                },
                "context": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Number of lines of context to show before and after each match."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 200,
                    "description": "Maximum total matches to return."
                },
                "max_matches_per_file": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum matches per file."
                },
                "max_files": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum number of files to search."
                },
                "max_file_size_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Skip files larger than this size."
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 20000,
                    "description": "Search timeout in milliseconds."
                },
                "fuzzy": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4,
                    "description": "Enable fuzzy matching with 1-4 allowed edits (insertions, deletions, substitutions)."
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_side_effecting(&self) -> bool {
        false
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn risk_level(&self) -> RiskLevel {
        RiskLevel::Medium
    }

    fn approval_summary(&self, args: &serde_json::Value) -> Result<String, ToolError> {
        let typed: SearchArgs =
            serde_json::from_value(args.clone()).map_err(|e| ToolError::BadArgs {
                message: e.to_string(),
            })?;
        let path = typed.path.unwrap_or_else(|| ".".to_string());
        let summary = format!("Search '{}' in {}", typed.pattern, path);
        Ok(redact_summary(&summary))
    }

    fn execute<'a>(&'a self, args: serde_json::Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            ctx.allow_truncation = false;
            let typed: SearchArgs =
                serde_json::from_value(args).map_err(|e| ToolError::BadArgs {
                    message: e.to_string(),
                })?;

            let pattern = typed.pattern.trim().to_string();
            if pattern.is_empty() {
                return Err(ToolError::BadArgs {
                    message: "pattern must not be empty".to_string(),
                });
            }

            let path_raw = typed.path.unwrap_or_else(|| ".".to_string());
            let resolved = ctx.sandbox.resolve_path(&path_raw, &ctx.working_dir)?;
            let metadata =
                std::fs::metadata(&resolved).map_err(|e| ToolError::ExecutionFailed {
                    tool: SEARCH_TOOL_NAME.to_string(),
                    message: e.to_string(),
                })?;
            if !metadata.is_dir() && !metadata.is_file() {
                return Err(ToolError::ExecutionFailed {
                    tool: SEARCH_TOOL_NAME.to_string(),
                    message: "path is not a file or directory".to_string(),
                });
            }

            let case_mode = parse_case(typed.case.as_deref());
            let fixed_strings = typed.fixed_strings.unwrap_or(false);
            let word_regexp = typed.word_regexp.unwrap_or(false);
            let recursive = typed.recursive.unwrap_or(true);
            let hidden = typed.hidden.unwrap_or(false);
            let follow = typed.follow.unwrap_or(false);
            let no_ignore = typed.no_ignore.unwrap_or(false);
            let context = typed.context.unwrap_or(0) as usize;
            let max_results = typed.max_results.unwrap_or(self.config.default_max_results);
            let timeout_ms = typed.timeout_ms.unwrap_or(self.config.default_timeout_ms);
            let fuzzy = typed.fuzzy;

            if let Some(level) = fuzzy
                && !(1..=4).contains(&level)
            {
                return Err(ToolError::BadArgs {
                    message: "fuzzy must be in range 1-4".to_string(),
                });
            }
            let max_matches_per_file = typed
                .max_matches_per_file
                .unwrap_or(self.config.max_matches_per_file);
            if let Some(val) = typed.max_matches_per_file
                && val > self.config.max_matches_per_file
            {
                return Err(ToolError::BadArgs {
                    message: format!(
                        "max_matches_per_file exceeds configured cap ({})",
                        self.config.max_matches_per_file
                    ),
                });
            }

            let max_files = typed.max_files.unwrap_or(self.config.max_files);
            if let Some(val) = typed.max_files
                && val > self.config.max_files
            {
                return Err(ToolError::BadArgs {
                    message: format!(
                        "max_files exceeds configured cap ({})",
                        self.config.max_files
                    ),
                });
            }

            let max_file_size_bytes = typed
                .max_file_size_bytes
                .unwrap_or(self.config.max_file_size_bytes);
            if let Some(val) = typed.max_file_size_bytes
                && val > self.config.max_file_size_bytes
            {
                return Err(ToolError::BadArgs {
                    message: format!(
                        "max_file_size_bytes exceeds configured cap ({})",
                        self.config.max_file_size_bytes
                    ),
                });
            }

            let include_glob = resolve_include_glob(typed.include_glob, typed.glob)?;
            let exclude_glob = resolve_glob_list(typed.exclude_glob)?;

            let backend = self.select_backend(fuzzy.is_some(), context > 0).await?;

            let order_root =
                determine_order_root(&resolved, &ctx.working_dir, Path::new(&path_raw))
                    .unwrap_or_else(|| ctx.working_dir.clone());
            let search_root_dir = if metadata.is_dir() {
                resolved.clone()
            } else {
                resolved
                    .parent()
                    .map_or_else(|| resolved.clone(), std::path::Path::to_path_buf)
            };

            let deadline = Instant::now() + Duration::from_millis(timeout_ms);

            let mut errors = Vec::new();
            let mut timed_out = false;
            let mut files_scanned = 0usize;
            let mut files = Vec::new();

            if metadata.is_file() {
                if Instant::now() >= deadline {
                    timed_out = true;
                } else if let Some(rel) = relativize_path(&resolved, &search_root_dir)
                    && include_glob.as_ref().is_none_or(|set| set.is_match(&rel))
                    && !exclude_glob.as_ref().is_some_and(|set| set.is_match(&rel))
                {
                    files_scanned = 1;
                    match ctx.sandbox.ensure_path_allowed(&resolved) {
                        Ok(_canon) => {
                            let size = metadata.len();
                            if size <= max_file_size_bytes {
                                files.push(FileCandidate { rel_path: rel });
                            }
                        }
                        Err(err) => {
                            errors.push(SearchFileError::from_tool_error(
                                &resolved,
                                err,
                                &search_root_dir,
                            ));
                        }
                    }
                }
            } else {
                let include_glob = include_glob.as_ref();
                let exclude_glob = exclude_glob.as_ref();
                let mut builder = WalkBuilder::new(&resolved);
                builder.follow_links(follow);
                builder.hidden(!hidden);
                if no_ignore {
                    builder.ignore(false);
                    builder.git_ignore(false);
                    builder.git_global(false);
                    builder.git_exclude(false);
                    builder.parents(false);
                }
                if !recursive {
                    builder.max_depth(Some(1));
                }
                // Skip .git directory entirely to avoid sandbox violation noise
                builder.filter_entry(|entry| entry.file_name() != ".git");
                builder
                    .sort_by_file_path(|a, b| normalize_walk_path(a).cmp(&normalize_walk_path(b)));

                for entry in builder.build() {
                    if Instant::now() >= deadline {
                        timed_out = true;
                        break;
                    }
                    match entry {
                        Ok(dirent) => {
                            let path = dirent.path();
                            let Some(file_type) = dirent.file_type() else {
                                continue;
                            };
                            if !file_type.is_file() {
                                continue;
                            }

                            let rel = if let Some(rel) = relativize_path(path, &search_root_dir) {
                                rel
                            } else {
                                errors.push(SearchFileError {
                                    path: normalize_display_path(path),
                                    error: "path outside search root".to_string(),
                                });
                                continue;
                            };

                            if let Some(include) = include_glob
                                && !include.is_match(&rel)
                            {
                                continue;
                            }
                            if let Some(exclude) = exclude_glob
                                && exclude.is_match(&rel)
                            {
                                continue;
                            }

                            if files_scanned >= max_files {
                                break;
                            }
                            files_scanned += 1;

                            let _canonical = match ctx.sandbox.ensure_path_allowed(path) {
                                Ok(canon) => canon,
                                Err(err) => {
                                    errors.push(SearchFileError::from_tool_error(
                                        path,
                                        err,
                                        &search_root_dir,
                                    ));
                                    continue;
                                }
                            };

                            let meta = dirent.metadata().or_else(|_| std::fs::metadata(path));
                            let meta = match meta {
                                Ok(meta) => meta,
                                Err(err) => {
                                    errors.push(SearchFileError {
                                        path: rel,
                                        error: err.to_string(),
                                    });
                                    continue;
                                }
                            };

                            if meta.len() > max_file_size_bytes {
                                continue;
                            }

                            files.push(FileCandidate { rel_path: rel });
                        }
                        Err(err) => {
                            errors.push(SearchFileError {
                                path: "<unknown>".to_string(),
                                error: err.to_string(),
                            });
                        }
                    }

                    if files_scanned >= max_files {
                        break;
                    }
                }
            }

            let normalized_root = normalize_display_path(&resolved);
            if files.is_empty() || timed_out {
                let mut response = SearchResponse {
                    pattern: pattern.clone(),
                    path: normalized_root,
                    count: 0,
                    matches: Vec::new(),
                    truncated: false,
                    timed_out,
                    files_scanned,
                    errors,
                    exit_code: None,
                    stderr: None,
                    content: String::new(),
                };
                response.content =
                    render_content(&response.matches, response.truncated, response.timed_out);
                return finalize_output(response, ctx);
            }

            let mut accumulator = SearchAccumulator::new(max_matches_per_file);
            let run = match backend.kind {
                BackendKind::Ripgrep => {
                    let run = RipgrepRun {
                        base: RunBase {
                            backend: &backend,
                            pattern: &pattern,
                            files: &files,
                            search_root: &search_root_dir,
                            order_root: &order_root,
                            case_mode: &case_mode,
                            fixed_strings,
                            word_regexp,
                            deadline,
                            accumulator: &mut accumulator,
                        },
                        context,
                        no_ignore,
                        errors: &mut errors,
                    };
                    run_ripgrep(run).await?
                }
                BackendKind::Ugrep => {
                    let run = UgrepRun {
                        base: RunBase {
                            backend: &backend,
                            pattern: &pattern,
                            files: &files,
                            search_root: &search_root_dir,
                            order_root: &order_root,
                            case_mode: &case_mode,
                            fixed_strings,
                            word_regexp,
                            deadline,
                            accumulator: &mut accumulator,
                        },
                        fuzzy,
                    };
                    run_ugrep(run).await?
                }
            };

            timed_out |= run.timed_out;
            let stderr = run.stderr;
            let exit_code = run.exit_code;

            if let Some(code) = exit_code
                && code >= 2
                && let Some(stderr_text) = stderr.as_ref()
            {
                if looks_like_regex_error(stderr_text) {
                    return Err(ToolError::BadArgs {
                        message: stderr_text.trim().to_string(),
                    });
                }
                if fuzzy.is_some() && looks_like_option_error(stderr_text) {
                    return Err(ToolError::BadArgs {
                        message: stderr_text.trim().to_string(),
                    });
                }
            }

            if fuzzy.is_some() && context > 0 {
                inject_fuzzy_context(
                    &mut accumulator,
                    &search_root_dir,
                    &order_root,
                    context,
                    &mut errors,
                );
            }

            let mut events = accumulator.finish();
            let mut truncated = events.len() > max_results;
            if timed_out && events.len() >= max_results {
                truncated = true;
            }
            if events.len() > max_results {
                events.truncate(max_results);
            }

            let mut response = SearchResponse {
                pattern: pattern.clone(),
                path: normalized_root,
                count: events.len(),
                matches: events,
                truncated,
                timed_out,
                files_scanned,
                errors,
                exit_code,
                stderr,
                content: String::new(),
            };
            response.content =
                render_content(&response.matches, response.truncated, response.timed_out);

            finalize_output(response, ctx)
        })
    }
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    pattern: String,
    path: Option<String>,
    case: Option<String>,
    fixed_strings: Option<bool>,
    word_regexp: Option<bool>,
    include_glob: Option<Vec<String>>,
    exclude_glob: Option<Vec<String>>,
    glob: Option<Vec<String>>,
    recursive: Option<bool>,
    hidden: Option<bool>,
    follow: Option<bool>,
    no_ignore: Option<bool>,
    context: Option<u32>,
    max_results: Option<usize>,
    max_matches_per_file: Option<usize>,
    max_files: Option<usize>,
    max_file_size_bytes: Option<u64>,
    timeout_ms: Option<u64>,
    fuzzy: Option<u8>,
}

#[derive(Debug, Clone)]
struct FileCandidate {
    rel_path: String,
}

#[derive(Debug, Clone, Copy)]
enum CaseMode {
    Sensitive,
    Insensitive,
    Smart,
}

#[derive(Debug, Clone, Serialize)]
struct SearchResponse {
    pattern: String,
    path: String,
    count: usize,
    matches: Vec<SearchEvent>,
    truncated: bool,
    timed_out: bool,
    files_scanned: usize,
    errors: Vec<SearchFileError>,
    exit_code: Option<i32>,
    stderr: Option<String>,
    content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SearchEvent {
    Match { data: MatchData },
    Context { data: ContextData },
}

#[derive(Debug, Clone, Serialize)]
struct MatchData {
    path: TextWrapper,
    line_number: u64,
    column: u64,
    lines: TextWrapper,
    match_text: String,
}

#[derive(Debug, Clone, Serialize)]
struct ContextData {
    path: TextWrapper,
    line_number: u64,
    lines: TextWrapper,
}

#[derive(Debug, Clone, Serialize)]
struct TextWrapper {
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct SearchFileError {
    path: String,
    error: String,
}

impl SearchFileError {
    fn from_tool_error(path: &Path, err: ToolError, root: &Path) -> Self {
        let rel = relativize_path(path, root).unwrap_or_else(|| normalize_display_path(path));
        Self {
            path: rel,
            error: err.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct ParsedEvent {
    path: String,
    line_number: u64,
    sort_key: Vec<u8>,
    kind: ParsedEventKind,
    parse_index: usize,
}

#[derive(Debug, Clone)]
enum ParsedEventKind {
    Match {
        column: u64,
        line_text: String,
        match_text: String,
    },
    Context {
        line_text: String,
    },
}

#[derive(Debug, Clone)]
struct SearchAccumulator {
    max_matches_per_file: usize,
    match_counts: HashMap<String, usize>,
    closed_files: HashSet<String>,
    events: Vec<ParsedEvent>,
    parse_index: usize,
}

impl SearchAccumulator {
    fn new(max_matches_per_file: usize) -> Self {
        Self {
            max_matches_per_file,
            match_counts: HashMap::new(),
            closed_files: HashSet::new(),
            events: Vec::new(),
            parse_index: 0,
        }
    }

    fn push_match(
        &mut self,
        path: String,
        line_number: u64,
        sort_key: Vec<u8>,
        column: u64,
        line_text: String,
        match_text: String,
    ) {
        if self.closed_files.contains(&path) {
            return;
        }
        let count = self.match_counts.entry(path.clone()).or_insert(0);
        if *count >= self.max_matches_per_file {
            self.closed_files.insert(path);
            return;
        }
        *count += 1;
        let path_key = path.clone();
        let event = ParsedEvent {
            path,
            line_number,
            sort_key,
            kind: ParsedEventKind::Match {
                column,
                line_text,
                match_text,
            },
            parse_index: self.parse_index,
        };
        self.parse_index += 1;
        self.events.push(event);
        if *count >= self.max_matches_per_file {
            self.closed_files.insert(path_key);
        }
    }

    fn push_context(
        &mut self,
        path: String,
        line_number: u64,
        sort_key: Vec<u8>,
        line_text: String,
    ) {
        let event = ParsedEvent {
            path,
            line_number,
            sort_key,
            kind: ParsedEventKind::Context { line_text },
            parse_index: self.parse_index,
        };
        self.parse_index += 1;
        self.events.push(event);
    }

    fn finish(&mut self) -> Vec<SearchEvent> {
        self.events.sort_by(|a, b| {
            let by_path = a.sort_key.cmp(&b.sort_key);
            if by_path != std::cmp::Ordering::Equal {
                return by_path;
            }
            let by_line = a.line_number.cmp(&b.line_number);
            if by_line != std::cmp::Ordering::Equal {
                return by_line;
            }
            let by_type = match (&a.kind, &b.kind) {
                (ParsedEventKind::Context { .. }, ParsedEventKind::Match { .. }) => {
                    std::cmp::Ordering::Less
                }
                (ParsedEventKind::Match { .. }, ParsedEventKind::Context { .. }) => {
                    std::cmp::Ordering::Greater
                }
                _ => std::cmp::Ordering::Equal,
            };
            if by_type != std::cmp::Ordering::Equal {
                return by_type;
            }
            a.parse_index.cmp(&b.parse_index)
        });

        let mut out = Vec::with_capacity(self.events.len());
        for event in self.events.drain(..) {
            match event.kind {
                ParsedEventKind::Match {
                    column,
                    line_text,
                    match_text,
                } => out.push(SearchEvent::Match {
                    data: MatchData {
                        path: TextWrapper { text: event.path },
                        line_number: event.line_number,
                        column,
                        lines: TextWrapper { text: line_text },
                        match_text,
                    },
                }),
                ParsedEventKind::Context { line_text } => out.push(SearchEvent::Context {
                    data: ContextData {
                        path: TextWrapper { text: event.path },
                        line_number: event.line_number,
                        lines: TextWrapper { text: line_text },
                    },
                }),
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
struct BackendRun {
    timed_out: bool,
    exit_code: Option<i32>,
    stderr: Option<String>,
}

struct RunBase<'a> {
    backend: &'a BackendInfo,
    pattern: &'a str,
    files: &'a [FileCandidate],
    search_root: &'a Path,
    order_root: &'a Path,
    case_mode: &'a CaseMode,
    fixed_strings: bool,
    word_regexp: bool,
    deadline: Instant,
    accumulator: &'a mut SearchAccumulator,
}

struct RipgrepRun<'a> {
    base: RunBase<'a>,
    context: usize,
    no_ignore: bool,
    errors: &'a mut Vec<SearchFileError>,
}

struct UgrepRun<'a> {
    base: RunBase<'a>,
    fuzzy: Option<u8>,
}

#[derive(Debug, Clone)]
struct BackendInfo {
    kind: BackendKind,
    binary: String,
}

#[derive(Debug, Clone, Copy)]
enum BackendKind {
    Ripgrep,
    Ugrep,
}

async fn probe_backend(binary: &str) -> Option<BackendInfo> {
    let mut cmd = Command::new(binary);
    cmd.arg("--version");
    let output = cmd.output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or("");
    if first_line.to_ascii_lowercase().contains("ripgrep") && version_ok(first_line, 13) {
        return Some(BackendInfo {
            kind: BackendKind::Ripgrep,
            binary: binary.to_string(),
        });
    }
    if first_line.to_ascii_lowercase().contains("ugrep") && version_ok(first_line, 3) {
        return Some(BackendInfo {
            kind: BackendKind::Ugrep,
            binary: binary.to_string(),
        });
    }
    None
}

fn version_ok(line: &str, min_major: u32) -> bool {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    let ver = parts[1];
    let mut nums = ver.split('.');
    let major = nums.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
    major >= min_major
}

fn parse_case(raw: Option<&str>) -> CaseMode {
    match raw.map(|s| s.trim().to_ascii_lowercase()) {
        Some(ref s) if s == "sensitive" => CaseMode::Sensitive,
        Some(ref s) if s == "insensitive" => CaseMode::Insensitive,
        _ => CaseMode::Smart,
    }
}

fn resolve_include_glob(
    include: Option<Vec<String>>,
    legacy: Option<Vec<String>>,
) -> Result<Option<GlobSet>, ToolError> {
    if include.as_ref().is_some() {
        return resolve_glob_list(include);
    }
    resolve_glob_list(legacy)
}

fn resolve_glob_list(list: Option<Vec<String>>) -> Result<Option<GlobSet>, ToolError> {
    let Some(list) = list else {
        return Ok(None);
    };
    if list.is_empty() {
        return Ok(None);
    }
    let mut builder = globset::GlobSetBuilder::new();
    for pat in list {
        let trimmed = pat.trim();
        if trimmed.is_empty() {
            return Err(ToolError::BadArgs {
                message: "glob entries must be non-empty".to_string(),
            });
        }
        let mut glob = GlobBuilder::new(trimmed);
        if cfg!(windows) {
            glob.case_insensitive(true);
        }
        let glob = glob.build().map_err(|e| ToolError::BadArgs {
            message: format!("Invalid glob '{trimmed}': {e}"),
        })?;
        builder.add(glob);
    }
    let set = builder.build().map_err(|e| ToolError::BadArgs {
        message: format!("Invalid glob set: {e}"),
    })?;
    Ok(Some(set))
}

fn determine_order_root(resolved: &Path, working_dir: &Path, raw_path: &Path) -> Option<PathBuf> {
    let is_abs = raw_path.is_absolute();
    if is_abs {
        if resolved.is_dir() {
            return std::fs::canonicalize(resolved).ok();
        }
        return resolved
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok());
    }
    std::fs::canonicalize(working_dir).ok()
}

fn normalize_walk_path(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows)
        && let Some(colon) = s.find(':')
    {
        let (drive, rest) = s.split_at(colon);
        if drive.len() == 1 {
            s = format!("{}{}", drive.to_ascii_uppercase(), rest);
        }
    }
    s
}

fn normalize_display_path(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/");
    if s.ends_with('/') && s.len() > 1 && !(cfg!(windows) && s.ends_with(":/")) {
        s = s.trim_end_matches('/').to_string();
    }
    s
}

fn relativize_path(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_string_lossy().replace('\\', "/");
    Some(s)
}

fn path_sort_key(path: &Path, order_root: &Path) -> Vec<u8> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let rel = canonical.strip_prefix(order_root).unwrap_or(&canonical);
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if cfg!(windows)
        && let Some(colon) = s.find(':')
    {
        let (drive, rest) = s.split_at(colon);
        if drive.len() == 1 {
            s = format!("{}{}", drive.to_ascii_uppercase(), rest);
        }
    }
    let s = s.trim_end_matches('/');
    s.nfc().collect::<String>().into_bytes()
}

fn render_content(matches: &[SearchEvent], truncated: bool, timed_out: bool) -> String {
    let mut out = String::new();
    for event in matches {
        match event {
            SearchEvent::Match { data } => {
                out.push_str(&format!(
                    "{}:{}:{}: {}",
                    data.path.text, data.line_number, data.column, data.lines.text
                ));
            }
            SearchEvent::Context { data } => {
                out.push_str(&format!(
                    "{}:{}: {}",
                    data.path.text, data.line_number, data.lines.text
                ));
            }
        }
        out.push('\n');
    }
    if truncated {
        out.push_str("... [truncated]\n");
    }
    if timed_out {
        out.push_str("... [timed out]\n");
    }
    out.trim_end().to_string()
}

fn inject_fuzzy_context(
    accumulator: &mut SearchAccumulator,
    search_root: &Path,
    order_root: &Path,
    context: usize,
    errors: &mut Vec<SearchFileError>,
) {
    if context == 0 {
        return;
    }

    let mut matches_by_path: HashMap<String, Vec<u64>> = HashMap::new();
    for event in &accumulator.events {
        if matches!(event.kind, ParsedEventKind::Match { .. }) {
            matches_by_path
                .entry(event.path.clone())
                .or_default()
                .push(event.line_number);
        }
    }

    for (path, mut match_lines) in matches_by_path {
        match_lines.sort_unstable();
        match_lines.dedup();

        let mut context_lines = HashSet::new();
        for line in &match_lines {
            let start = line.saturating_sub(context as u64);
            let end = line.saturating_add(context as u64);
            for line_number in start..=end {
                if line_number == 0 || match_lines.binary_search(&line_number).is_ok() {
                    continue;
                }
                context_lines.insert(line_number);
            }
        }

        if context_lines.is_empty() {
            continue;
        }

        let abs_path = search_root.join(&path);
        let bytes = match std::fs::read(&abs_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                errors.push(SearchFileError {
                    path: path.clone(),
                    error: err.to_string(),
                });
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut line_number = 1u64;
        let sort_key = path_sort_key(&abs_path, order_root);
        for raw_line in text.split_terminator('\n') {
            if context_lines.contains(&line_number) {
                accumulator.events.push(ParsedEvent {
                    path: path.clone(),
                    line_number,
                    sort_key: sort_key.clone(),
                    kind: ParsedEventKind::Context {
                        line_text: trim_line_endings(raw_line),
                    },
                    parse_index: accumulator.parse_index,
                });
                accumulator.parse_index += 1;
            }
            line_number += 1;
        }
    }
}

fn finalize_output(response: SearchResponse, ctx: &ToolCtx) -> Result<String, ToolError> {
    let effective_max = ctx.max_output_bytes.min(ctx.available_capacity_bytes);
    let mut response = response;

    loop {
        let json = serde_json::to_string(&response).map_err(|e| ToolError::ExecutionFailed {
            tool: SEARCH_TOOL_NAME.to_string(),
            message: e.to_string(),
        })?;
        if json.len() <= effective_max || response.matches.is_empty() {
            return Ok(sanitize_output(&json));
        }

        response.truncated = true;
        response.matches.pop();
        response.count = response.matches.len();
        response.content =
            render_content(&response.matches, response.truncated, response.timed_out);
    }
}

async fn run_ripgrep(run: RipgrepRun<'_>) -> Result<BackendRun, ToolError> {
    let RipgrepRun {
        base,
        context,
        no_ignore,
        errors,
    } = run;
    let RunBase {
        backend,
        pattern,
        files,
        search_root,
        order_root,
        case_mode,
        fixed_strings,
        word_regexp,
        deadline,
        accumulator,
    } = base;
    let mut file_list = String::new();
    for file in files {
        file_list.push_str(&file.rel_path);
        file_list.push('\n');
    }

    let tmp = tempfile::NamedTempFile::new().map_err(|e| ToolError::ExecutionFailed {
        tool: SEARCH_TOOL_NAME.to_string(),
        message: e.to_string(),
    })?;
    std::fs::write(tmp.path(), file_list).map_err(|e| ToolError::ExecutionFailed {
        tool: SEARCH_TOOL_NAME.to_string(),
        message: e.to_string(),
    })?;

    let mut cmd = Command::new(&backend.binary);
    cmd.current_dir(search_root);
    cmd.arg("--json");
    cmd.arg("--files-from");
    cmd.arg(tmp.path());
    cmd.arg("--max-count");
    cmd.arg(accumulator.max_matches_per_file.to_string());
    if context > 0 {
        cmd.arg("-C");
        cmd.arg(context.to_string());
    }
    if fixed_strings {
        cmd.arg("-F");
    }
    if word_regexp {
        cmd.arg("-w");
    }
    match case_mode {
        CaseMode::Sensitive => {}
        CaseMode::Insensitive => {
            cmd.arg("-i");
            cmd.arg("--no-unicode");
        }
        CaseMode::Smart => {
            if pattern_has_ascii_uppercase(pattern) {
                // treat as sensitive
            } else {
                cmd.arg("-i");
                cmd.arg("--no-unicode");
            }
        }
    }
    if no_ignore {
        cmd.arg("--no-ignore");
    }
    cmd.arg("--");
    cmd.arg(pattern);

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ToolError::ExecutionFailed {
            tool: SEARCH_TOOL_NAME.to_string(),
            message: e.to_string(),
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: SEARCH_TOOL_NAME.to_string(),
            message: "failed to capture stdout".to_string(),
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: SEARCH_TOOL_NAME.to_string(),
            message: "failed to capture stderr".to_string(),
        })?;

    let mut stderr_reader = BufReader::new(stderr);
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr_reader.read_to_string(&mut buf).await;
        buf
    });

    let mut timed_out = false;
    let mut stdout_reader = BufReader::new(stdout).lines();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            let _ = child.kill().await;
            break;
        }
        let line = match tokio::time::timeout(remaining, stdout_reader.next_line()).await {
            Ok(Ok(line)) => line,
            Ok(Err(err)) => {
                return Err(ToolError::ExecutionFailed {
                    tool: SEARCH_TOOL_NAME.to_string(),
                    message: err.to_string(),
                });
            }
            Err(_) => {
                timed_out = true;
                let _ = child.kill().await;
                break;
            }
        };
        let Some(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| ToolError::ExecutionFailed {
                tool: SEARCH_TOOL_NAME.to_string(),
                message: format!("invalid JSON from ripgrep: {e}"),
            })?;
        let Some(kind) = value.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        match kind {
            "match" => {
                if let Some(event) = parse_rg_match(&value, order_root, search_root) {
                    accumulator.push_match(
                        event.path,
                        event.line_number,
                        event.sort_key,
                        event.column,
                        event.line_text,
                        event.match_text,
                    );
                }
            }
            "context" => {
                if let Some(event) = parse_rg_context(&value, order_root, search_root) {
                    accumulator.push_context(
                        event.path,
                        event.line_number,
                        event.sort_key,
                        event.line_text,
                    );
                }
            }
            "error" => {
                if let Some(err) = parse_rg_error(&value) {
                    errors.push(err);
                }
            }
            _ => {}
        }
    }

    let status = child.wait().await.map_err(|e| ToolError::ExecutionFailed {
        tool: SEARCH_TOOL_NAME.to_string(),
        message: e.to_string(),
    })?;

    let stderr = stderr_task.await.unwrap_or_default();
    let stderr = if stderr.trim().is_empty() {
        None
    } else {
        Some(stderr)
    };

    Ok(BackendRun {
        timed_out,
        exit_code: status.code(),
        stderr,
    })
}

async fn run_ugrep(run: UgrepRun<'_>) -> Result<BackendRun, ToolError> {
    let UgrepRun { base, fuzzy } = run;
    let RunBase {
        backend,
        pattern,
        files,
        search_root,
        order_root,
        case_mode,
        fixed_strings,
        word_regexp,
        deadline,
        accumulator,
    } = base;
    // ugrep formatted output does not include context; fuzzy context is injected separately.
    let mut timed_out = false;
    let mut exit_code = None;
    let mut stderr_out = None;

    let mut offset = 0usize;
    let batch_size = 500usize;
    while offset < files.len() {
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let end = (offset + batch_size).min(files.len());
        let batch = &files[offset..end];
        offset = end;

        let mut cmd = Command::new(&backend.binary);
        cmd.current_dir(search_root);
        // Use --format=VALUE syntax to avoid Windows command-line argument parsing issues.
        // %h yields a quoted pathname and %J yields a JSON-escaped line.
        cmd.arg(
            r#"--format={"path": %h, "line": %n, "column": %k, "size": %d, "line_text": %J}%~"#,
        );
        if fixed_strings {
            cmd.arg("-F");
        }
        if word_regexp {
            cmd.arg("-w");
        }
        match case_mode {
            CaseMode::Sensitive => {}
            CaseMode::Insensitive => {
                cmd.arg("-i");
            }
            CaseMode::Smart => {
                if pattern_has_ascii_uppercase(pattern) {
                    // sensitive
                } else {
                    cmd.arg("-i");
                }
            }
        }
        if let Some(level) = fuzzy {
            cmd.arg(format!("-Z{level}"));
        }

        cmd.arg("--");
        cmd.arg(pattern);
        for file in batch {
            cmd.arg(&file.rel_path);
        }

        let mut child = cmd
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed {
                tool: SEARCH_TOOL_NAME.to_string(),
                message: e.to_string(),
            })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed {
                tool: SEARCH_TOOL_NAME.to_string(),
                message: "failed to capture stdout".to_string(),
            })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed {
                tool: SEARCH_TOOL_NAME.to_string(),
                message: "failed to capture stderr".to_string(),
            })?;

        let mut stderr_reader = BufReader::new(stderr);
        let stderr_task = tokio::spawn(async move {
            let mut buf = String::new();
            let _ = stderr_reader.read_to_string(&mut buf).await;
            buf
        });

        let mut stdout_reader = BufReader::new(stdout).lines();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                timed_out = true;
                let _ = child.kill().await;
                break;
            }
            let line = match tokio::time::timeout(remaining, stdout_reader.next_line()).await {
                Ok(Ok(line)) => line,
                Ok(Err(err)) => {
                    return Err(ToolError::ExecutionFailed {
                        tool: SEARCH_TOOL_NAME.to_string(),
                        message: err.to_string(),
                    });
                }
                Err(_) => {
                    timed_out = true;
                    let _ = child.kill().await;
                    break;
                }
            };
            let Some(line) = line else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_str(&line).map_err(|e| ToolError::ExecutionFailed {
                    tool: SEARCH_TOOL_NAME.to_string(),
                    message: format!("invalid JSON from ugrep: {e}"),
                })?;
            if let Some(event) = parse_ugrep_match(&value, order_root, search_root) {
                accumulator.push_match(
                    event.path,
                    event.line_number,
                    event.sort_key,
                    event.column,
                    event.line_text,
                    event.match_text,
                );
            }
        }

        let status = child.wait().await.map_err(|e| ToolError::ExecutionFailed {
            tool: SEARCH_TOOL_NAME.to_string(),
            message: e.to_string(),
        })?;
        exit_code = status.code();
        let stderr = stderr_task.await.unwrap_or_default();
        if !stderr.trim().is_empty() {
            stderr_out = Some(stderr);
        }

        if timed_out {
            break;
        }
    }

    Ok(BackendRun {
        timed_out,
        exit_code,
        stderr: stderr_out,
    })
}

fn parse_rg_match(
    value: &serde_json::Value,
    order_root: &Path,
    search_root: &Path,
) -> Option<ParsedMatchEvent> {
    let data = value.get("data")?;
    let path_text = data.get("path")?.get("text")?.as_str()?;
    let line_number = data.get("line_number")?.as_u64()?;
    let lines_text = data.get("lines")?.get("text")?.as_str()?;
    let line_text = trim_line_endings(lines_text);

    let submatch = data.get("submatches")?.as_array()?.first()?;
    let start = submatch.get("start")?.as_u64()? as usize;
    let end = submatch.get("end")?.as_u64()? as usize;

    let match_text = line_text
        .as_bytes()
        .get(start..end)
        .and_then(|s| std::str::from_utf8(s).ok())
        .unwrap_or("")
        .to_string();

    let column = start as u64 + 1;
    let path = normalize_path_text(path_text);
    let abs_path = search_root.join(path_text);
    let sort_key = path_sort_key(&abs_path, order_root);

    Some(ParsedMatchEvent {
        path,
        line_number,
        sort_key,
        column,
        line_text,
        match_text,
    })
}

fn parse_rg_context(
    value: &serde_json::Value,
    order_root: &Path,
    search_root: &Path,
) -> Option<ParsedContextEvent> {
    let data = value.get("data")?;
    let path_text = data.get("path")?.get("text")?.as_str()?;
    let line_number = data.get("line_number")?.as_u64()?;
    let lines_text = data.get("lines")?.get("text")?.as_str()?;
    let line_text = trim_line_endings(lines_text);

    let path = normalize_path_text(path_text);
    let abs_path = search_root.join(path_text);
    let sort_key = path_sort_key(&abs_path, order_root);

    Some(ParsedContextEvent {
        path,
        line_number,
        sort_key,
        line_text,
    })
}

fn parse_rg_error(value: &serde_json::Value) -> Option<SearchFileError> {
    let data = value.get("data")?;
    let message = data.get("message")?.as_str()?.to_string();
    let path = data
        .get("path")
        .and_then(|p| p.get("text"))
        .and_then(|p| p.as_str())
        .map_or_else(|| "<unknown>".to_string(), normalize_path_text);
    Some(SearchFileError {
        path,
        error: message,
    })
}

fn parse_ugrep_match(
    value: &serde_json::Value,
    order_root: &Path,
    search_root: &Path,
) -> Option<ParsedMatchEvent> {
    let path_text = value.get("path")?.as_str()?;
    let line_number = value.get("line")?.as_u64()?;
    let column = value.get("column")?.as_u64()?;
    let size = value.get("size")?.as_u64()? as usize;
    let line_text_raw = value.get("line_text")?.as_str()?;
    let line_text = trim_line_endings(line_text_raw);

    let start = column.saturating_sub(1) as usize;
    let match_text = line_text
        .as_bytes()
        .get(start..start.saturating_add(size))
        .and_then(|s| std::str::from_utf8(s).ok())
        .unwrap_or("")
        .to_string();

    let path = normalize_path_text(path_text);
    let abs_path = search_root.join(path_text);
    let sort_key = path_sort_key(&abs_path, order_root);

    Some(ParsedMatchEvent {
        path,
        line_number,
        sort_key,
        column,
        line_text,
        match_text,
    })
}

#[derive(Debug, Clone)]
struct ParsedMatchEvent {
    path: String,
    line_number: u64,
    sort_key: Vec<u8>,
    column: u64,
    line_text: String,
    match_text: String,
}

#[derive(Debug, Clone)]
struct ParsedContextEvent {
    path: String,
    line_number: u64,
    sort_key: Vec<u8>,
    line_text: String,
}

fn normalize_path_text(path: &str) -> String {
    path.replace('\\', "/")
}

fn trim_line_endings(line: &str) -> String {
    line.trim_end_matches(['\n', '\r'].as_ref()).to_string()
}

fn pattern_has_ascii_uppercase(pattern: &str) -> bool {
    pattern.chars().any(|c| c.is_ascii_uppercase())
}

fn looks_like_regex_error(stderr: &str) -> bool {
    let lowered = stderr.to_ascii_lowercase();
    lowered.contains("regex")
        || lowered.contains("regular expression")
        || lowered.contains("parse error")
}

fn looks_like_option_error(stderr: &str) -> bool {
    let lowered = stderr.to_ascii_lowercase();
    lowered.contains("unknown option")
        || lowered.contains("unrecognized option")
        || lowered.contains("invalid option")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_line_endings_removes_trailing_newlines() {
        assert_eq!(trim_line_endings("hello\n"), "hello");
        assert_eq!(trim_line_endings("hello\r\n"), "hello");
        assert_eq!(trim_line_endings("hello"), "hello");
    }

    #[test]
    fn normalize_path_text_replaces_backslashes() {
        assert_eq!(normalize_path_text("a\\b\\c"), "a/b/c");
    }

    #[test]
    fn pattern_detects_ascii_uppercase() {
        assert!(pattern_has_ascii_uppercase("Foo"));
        assert!(!pattern_has_ascii_uppercase("foo"));
    }
}
