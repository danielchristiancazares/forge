use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

use crate::message::Message;

/// Supported LLM providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Provider {
    #[default]
    Claude,
    OpenAI,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::OpenAI => "openai",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::OpenAI => "GPT",
        }
    }

    pub fn env_var(&self) -> &'static str {
        match self {
            Provider::Claude => "ANTHROPIC_API_KEY",
            Provider::OpenAI => "OPENAI_API_KEY",
        }
    }

    pub fn default_model(&self) -> ModelName {
        match self {
            Provider::Claude => ModelName::known(*self, "claude-sonnet-4-5-20250929"),
            Provider::OpenAI => ModelName::known(*self, "gpt-4o"),
        }
    }

    /// All available models for this provider.
    pub fn available_models(&self) -> &'static [&'static str] {
        match self {
            Provider::Claude => &[
                "claude-sonnet-4-5-20250929",
                "claude-haiku-4-5-20251001",
                "claude-opus-4-5-20251101",
            ],
            Provider::OpenAI => &["gpt-4o", "gpt-4o-mini"],
        }
    }

    /// Parse a model name for this provider.
    pub fn parse_model(&self, raw: &str) -> Result<ModelName, ModelParseError> {
        ModelName::parse(*self, raw)
    }

    /// Parse provider from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "claude" | "anthropic" => Some(Provider::Claude),
            "openai" | "gpt" | "chatgpt" => Some(Provider::OpenAI),
            _ => None,
        }
    }

    /// Get all available providers
    pub fn all() -> &'static [Provider] {
        &[Provider::Claude, Provider::OpenAI]
    }
}

/// Whether a model name is verified/known or user-supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ModelNameKind {
    Known,
    #[default]
    Unverified,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelParseError {
    #[error("model name cannot be empty")]
    Empty,
}

/// Provider-scoped model name.
///
/// This prevents mixing model names across providers and makes unknown names explicit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelName {
    provider: Provider,
    #[serde(rename = "model")]
    name: Cow<'static, str>,
    #[serde(default)]
    kind: ModelNameKind,
}

impl ModelName {
    pub fn parse(provider: Provider, raw: &str) -> Result<Self, ModelParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(ModelParseError::Empty);
        }

        if let Some(known) = provider
            .available_models()
            .iter()
            .find(|model| model.eq_ignore_ascii_case(trimmed))
        {
            return Ok(Self {
                provider,
                name: Cow::Borrowed(*known),
                kind: ModelNameKind::Known,
            });
        }

        Ok(Self {
            provider,
            name: Cow::Owned(trimmed.to_string()),
            kind: ModelNameKind::Unverified,
        })
    }

    pub const fn known(provider: Provider, name: &'static str) -> Self {
        Self {
            provider,
            name: Cow::Borrowed(name),
            kind: ModelNameKind::Known,
        }
    }

    pub const fn provider(&self) -> Provider {
        self.provider
    }

    pub fn as_str(&self) -> &str {
        self.name.as_ref()
    }

    pub const fn kind(&self) -> ModelNameKind {
        self.kind
    }
}

impl std::fmt::Display for ModelName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.name.fmt(f)
    }
}

/// Provider-scoped API key.
///
/// This prevents the invalid state "OpenAI key used with Claude" from being representable.
#[derive(Debug, Clone)]
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
}

impl ApiKey {
    pub fn provider(&self) -> Provider {
        match self {
            ApiKey::Claude(_) => Provider::Claude,
            ApiKey::OpenAI(_) => Provider::OpenAI,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            ApiKey::Claude(key) | ApiKey::OpenAI(key) => key,
        }
    }
}

/// Configuration for API requests
#[derive(Debug, Clone)]
pub struct ApiConfig {
    api_key: ApiKey,
    model: ModelName,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiConfigError {
    #[error("API key provider {key:?} does not match model provider {model:?}")]
    ProviderMismatch { key: Provider, model: Provider },
}

impl ApiConfig {
    pub fn new(api_key: ApiKey, model: ModelName) -> Result<Self, ApiConfigError> {
        let key_provider = api_key.provider();
        let model_provider = model.provider();
        if key_provider != model_provider {
            return Err(ApiConfigError::ProviderMismatch {
                key: key_provider,
                model: model_provider,
            });
        }

        Ok(Self { api_key, model })
    }

    pub fn provider(&self) -> Provider {
        self.api_key.provider()
    }

    pub fn api_key(&self) -> &str {
        self.api_key.as_str()
    }

    pub fn api_key_owned(&self) -> ApiKey {
        self.api_key.clone()
    }

    pub fn model(&self) -> &ModelName {
        &self.model
    }
}

/// Streaming event from the API
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta
    TextDelta(String),
    /// Stream completed
    Done,
    /// Error occurred
    Error(String),
}

/// Send a chat request and stream the response
pub async fn send_message(
    config: &ApiConfig,
    messages: &[Message],
    max_output_tokens: u32,
    on_event: impl Fn(StreamEvent) + Send + 'static,
) -> Result<()> {
    match config.provider() {
        Provider::Claude => {
            claude::send_message(config, messages, max_output_tokens, on_event).await
        }
        Provider::OpenAI => {
            openai::send_message(config, messages, max_output_tokens, on_event).await
        }
    }
}

/// Claude/Anthropic API implementation
pub mod claude {
    use super::*;
    use reqwest::Client;
    use serde_json::json;

    const API_URL: &str = "https://api.anthropic.com/v1/messages";

    pub async fn send_message(
        config: &ApiConfig,
        messages: &[Message],
        max_output_tokens: u32,
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        let client = Client::new();

        // Convert messages to Claude format, handling system messages separately.
        // Claude's Messages API accepts a top-level "system" field; message roles are user/assistant.
        let mut system_parts: Vec<String> = Vec::new();
        let mut api_messages: Vec<serde_json::Value> = Vec::new();

        for msg in messages {
            match msg {
                Message::System(_) => {
                    system_parts.push(msg.content().to_string());
                }
                Message::User(_) => {
                    api_messages.push(json!({
                        "role": "user",
                        "content": msg.content(),
                    }));
                }
                Message::Assistant(_) => {
                    api_messages.push(json!({
                        "role": "assistant",
                        "content": msg.content(),
                    }));
                }
            }
        }

        let system = system_parts.join("\n\n");
        let body = if system.is_empty() {
            json!({
                "model": config.model().as_str(),
                "max_tokens": max_output_tokens,
                "stream": true,
                "messages": api_messages
            })
        } else {
            json!({
                "model": config.model().as_str(),
                "max_tokens": max_output_tokens,
                "stream": true,
                "system": system,
                "messages": api_messages
            })
        };

        let response = client
            .post(API_URL)
            .header("x-api-key", config.api_key())
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = match response.text().await {
                Ok(text) => text,
                Err(e) => format!("<failed to read error body: {e}>"),
            };
            on_event(StreamEvent::Error(format!(
                "API error {}: {}",
                status, error_text
            )));
            return Ok(());
        }

        // Process SSE stream
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;

        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);
            buffer.push_str(&text);

            // Process complete SSE events
            while let Some(pos) = buffer.find("\n\n") {
                let event = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                // SSE can have event: and data: lines
                let mut data_line = None;
                for line in event.lines() {
                    if let Some(d) = line.strip_prefix("data: ") {
                        data_line = Some(d);
                    }
                }

                if let Some(data) = data_line {
                    if data == "[DONE]" {
                        on_event(StreamEvent::Done);
                        return Ok(());
                    }

                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                        // Handle content_block_delta events
                        if json["type"] == "content_block_delta"
                            && let Some(text) = json["delta"]["text"].as_str()
                        {
                            on_event(StreamEvent::TextDelta(text.to_string()));
                        }
                        // Handle message_stop event
                        if json["type"] == "message_stop" {
                            on_event(StreamEvent::Done);
                            return Ok(());
                        }
                    }
                }
            }
        }

        on_event(StreamEvent::Done);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_parses_known_names_and_aliases() {
        assert_eq!(Provider::from_str("claude"), Some(Provider::Claude));
        assert_eq!(Provider::from_str("Anthropic"), Some(Provider::Claude));
        assert_eq!(Provider::from_str("openai"), Some(Provider::OpenAI));
        assert_eq!(Provider::from_str("gpt"), Some(Provider::OpenAI));
        assert_eq!(Provider::from_str("chatgpt"), Some(Provider::OpenAI));
        assert_eq!(Provider::from_str("unknown"), None);
    }

    #[test]
    fn provider_metadata_is_consistent() {
        for provider in Provider::all() {
            assert!(!provider.as_str().is_empty());
            assert!(!provider.display_name().is_empty());
            assert!(!provider.env_var().is_empty());
            assert!(!provider.default_model().as_str().is_empty());
        }
    }
}

/// OpenAI API implementation
pub mod openai {
    use super::*;
    use reqwest::Client;
    use serde_json::json;

    const API_URL: &str = "https://api.openai.com/v1/chat/completions";

    pub async fn send_message(
        config: &ApiConfig,
        messages: &[Message],
        max_output_tokens: u32,
        on_event: impl Fn(StreamEvent) + Send + 'static,
    ) -> Result<()> {
        let client = Client::new();

        // Convert messages to OpenAI format
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": m.role_str(),
                    "content": m.content(),
                })
            })
            .collect();

        let body = json!({
            "model": config.model().as_str(),
            "messages": api_messages,
            "max_tokens": max_output_tokens,
            "stream": true
        });

        let response = client
            .post(API_URL)
            .header("Authorization", format!("Bearer {}", config.api_key()))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = match response.text().await {
                Ok(text) => text,
                Err(e) => format!("<failed to read error body: {e}>"),
            };
            on_event(StreamEvent::Error(format!(
                "API error {}: {}",
                status, error_text
            )));
            return Ok(());
        }

        // Process SSE stream
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;

        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);
            buffer.push_str(&text);

            // Process complete SSE events
            while let Some(pos) = buffer.find("\n\n") {
                let event = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                for line in event.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            on_event(StreamEvent::Done);
                            return Ok(());
                        }

                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            // Extract content from choices[0].delta.content
                            if let Some(content) = json["choices"][0]["delta"]["content"].as_str()
                                && !content.is_empty()
                            {
                                on_event(StreamEvent::TextDelta(content.to_string()));
                            }
                        }
                    }
                }
            }
        }

        on_event(StreamEvent::Done);
        Ok(())
    }
}
