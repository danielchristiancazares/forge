use serde::Deserialize;
use std::{env, path::PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct ForgeConfig {
    pub app: Option<AppConfig>,
    pub api_keys: Option<ApiKeys>,
    pub context: Option<ContextConfig>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tui: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ApiKeys {
    pub anthropic: Option<String>,
    pub openai: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ContextConfig {
    pub infinity: Option<bool>,
}

pub fn expand_env_vars(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut i = 0;

    while i < value.len() {
        if value[i..].starts_with("${") {
            let start = i + 2;
            if let Some(end_rel) = value[start..].find('}') {
                let end = start + end_rel;
                let var = &value[start..end];
                if !var.is_empty() {
                    let replacement = env::var(var).unwrap_or_default();
                    out.push_str(&replacement);
                }
                i = end + 1;
                continue;
            }
        }

        let ch = value[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

impl ForgeConfig {
    pub fn load() -> Option<Self> {
        let path = config_path()?;
        if !path.exists() {
            return None;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                tracing::warn!("Failed to read config at {:?}: {}", path, err);
                return None;
            }
        };

        match toml::from_str(&content) {
            Ok(config) => Some(config),
            Err(err) => {
                tracing::warn!("Failed to parse config at {:?}: {}", path, err);
                None
            }
        }
    }

    pub fn path() -> Option<PathBuf> {
        config_path()
    }
}

fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".forge").join("config.toml"))
}
