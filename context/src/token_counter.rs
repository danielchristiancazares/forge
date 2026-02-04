//! Token counting using tiktoken.
//!
//! This module provides **approximate** token counting using the `o200k_base`
//! encoding from tiktoken. This encoding is accurate for OpenAI models
//! (`gpt-5.2`, `gpt-5.2-pro`) and serves as a reasonable approximation for others:
//!
//! - **Claude models**: Anthropic uses a proprietary tokenizer; counts may vary by ~5-10%
//! - **Gemini models**: Google uses a proprietary tokenizer; counts may vary
//! - **Message overhead**: The fixed 4-token overhead per message is an approximation
//!
//! The 5% safety margin in `ModelLimits::effective_input_budget()` helps account
//! for these inaccuracies. For precise token counts, use the provider's native
//! token counting endpoint when available.

use std::sync::OnceLock;
use tiktoken_rs::{CoreBPE, o200k_base};

use forge_types::Message;

/// Singleton encoder instance.
///
/// The tiktoken encoder is expensive to initialize (loads vocabulary data),
/// so we create it once and reuse it across all `TokenCounter` instances.
static ENCODER: OnceLock<Option<CoreBPE>> = OnceLock::new();

/// Returns a reference to the shared encoder instance.
///
/// Initializes the encoder on first call using `o200k_base` encoding.
fn get_encoder() -> Option<&'static CoreBPE> {
    ENCODER.get_or_init(|| o200k_base().ok()).as_ref()
}

/// Thread-safe approximate token counter using tiktoken's `o200k_base` encoding.
///
/// **Note**: Token counts are approximate. See module documentation for accuracy
/// considerations across different providers and models.
///
/// Uses a singleton encoder instance for efficiency.
///
/// # Token Counting Overhead
///
/// When counting tokens for chat messages, this counter adds a ~4 token
/// overhead per message to approximate:
/// - Role markers (e.g., "user", "assistant")
/// - Message formatting/delimiters
///
/// This overhead may vary by provider and model.
///
/// # Example
///
/// ```
/// use forge_context::TokenCounter;
///
/// let counter = TokenCounter::new();
///
/// // Count tokens in a string
/// let tokens = counter.count_str("Hello, world!");
/// assert!(tokens > 0);
///
/// // Use default() for convenience
/// let counter = TokenCounter::default();
/// ```
#[derive(Clone, Copy)]
pub struct TokenCounter {
    /// Reference to the shared encoder.
    encoder: Option<&'static CoreBPE>,
}

impl std::fmt::Debug for TokenCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenCounter")
            .field("encoder", &self.encoder.as_ref().map(|_| "<CoreBPE>"))
            .finish()
    }
}

impl TokenCounter {
    /// Creates a new token counter.
    ///
    /// This is cheap to call - the underlying encoder is a singleton
    /// that is initialized only once across all `TokenCounter` instances.
    #[must_use]
    pub fn new() -> Self {
        let encoder = get_encoder();
        if encoder.is_none() {
            tracing::error!(
                "Failed to initialize tiktoken o200k_base encoder. Falling back to byte-length estimates."
            );
        }

        Self { encoder }
    }

    /// Counts the number of tokens in a string.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use forge_context::TokenCounter;
    ///
    /// let counter = TokenCounter::new();
    /// let tokens = counter.count_str("Hello, world!");
    /// println!("Token count: {tokens}");
    /// ```
    #[must_use]
    pub fn count_str(&self, text: &str) -> u32 {
        let len = match self.encoder {
            Some(encoder) => encoder.encode_ordinary(text).len(),
            None => text.len(),
        };

        u32::try_from(len).unwrap_or(u32::MAX)
    }

    /// Counts tokens for a single message, including role overhead.
    ///
    /// Each message has approximately 4 tokens of overhead for:
    /// - The role name (e.g., "user", "assistant")
    /// - Message structure/delimiters
    ///
    /// # Example
    ///
    /// ```ignore
    /// use forge_context::TokenCounter;
    /// use forge_types::Message;
    ///
    /// let counter = TokenCounter::new();
    /// let msg = Message::try_user("What is the meaning of life?").unwrap();
    /// let tokens = counter.count_message(&msg);
    /// ```
    #[must_use]
    pub fn count_message(&self, msg: &Message) -> u32 {
        // Role overhead: ~4 tokens for role markers and message structure
        // This is an approximation based on OpenAI's token counting guidelines
        const MESSAGE_OVERHEAD: u32 = 4;

        let content_tokens = self.count_str(msg.content());
        let role_tokens = self.count_str(msg.role_str());

        content_tokens + role_tokens + MESSAGE_OVERHEAD
    }

    /// Counts total tokens for a slice of messages.
    ///
    /// This sums the token count of each message including their overhead.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use forge_context::TokenCounter;
    /// use forge_types::Message;
    ///
    /// let counter = TokenCounter::new();
    /// let messages = vec![
    ///     Message::try_user("Hello!").unwrap(),
    ///     Message::try_user("How are you?").unwrap(),
    /// ];
    /// let total = counter.count_messages(&messages);
    /// ```
    #[must_use]
    #[cfg(test)]
    pub fn count_messages(&self, messages: &[Message]) -> u32 {
        messages.iter().map(|msg| self.count_message(msg)).sum()
    }
}

impl Default for TokenCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_counter() {
        let counter = TokenCounter::new();
        let _ = counter.count_str("test");
    }

    #[test]
    fn default_creates_counter() {
        let counter = TokenCounter::default();
        let _ = counter.count_str("test");
    }

    #[test]
    fn count_str_empty_string() {
        let counter = TokenCounter::new();
        assert_eq!(counter.count_str(""), 0);
    }

    #[test]
    fn count_str_simple_text() {
        let counter = TokenCounter::new();

        let tokens = counter.count_str("Hello");
        assert!(tokens >= 1);

        let tokens = counter.count_str("Hello, world!");
        assert!(tokens >= 1);
    }

    #[test]
    fn count_str_longer_text() {
        let counter = TokenCounter::new();

        let text = "The quick brown fox jumps over the lazy dog.";
        let tokens = counter.count_str(text);

        assert!(tokens >= 5);
        assert!(tokens <= 20);
    }

    #[test]
    fn count_str_unicode() {
        let counter = TokenCounter::new();

        let tokens = counter.count_str("Hello, world!");
        let tokens_cn = counter.count_str("Hello, world! :)");

        assert!(tokens > 0);
        assert!(tokens_cn > 0);
    }

    #[test]
    fn count_str_code() {
        let counter = TokenCounter::new();

        let code = r#"fn main() {
    println!("Hello, world!");
}"#;
        let tokens = counter.count_str(code);

        assert!(tokens >= 5);
    }

    #[test]
    fn count_message_user() {
        let counter = TokenCounter::new();
        let msg = Message::try_user("Hello!").expect("non-empty test message");

        let tokens = counter.count_message(&msg);

        let content_tokens = counter.count_str("Hello!");
        let role_tokens = counter.count_str("user");
        let expected_min = content_tokens + role_tokens + 4;

        assert_eq!(tokens, expected_min);
    }

    #[test]
    fn count_message_includes_overhead() {
        let counter = TokenCounter::new();
        let msg = Message::try_user("Hi").expect("non-empty test message");

        let content_tokens = counter.count_str("Hi");
        let message_tokens = counter.count_message(&msg);

        assert!(message_tokens > content_tokens);
    }

    #[test]
    fn count_messages_empty() {
        let counter = TokenCounter::new();
        let messages: Vec<Message> = vec![];

        assert_eq!(counter.count_messages(&messages), 0);
    }

    #[test]
    fn count_messages_single() {
        let counter = TokenCounter::new();
        let messages = vec![Message::try_user("Hello!").expect("non-empty test message")];

        let total = counter.count_messages(&messages);
        let single = counter.count_message(&messages[0]);

        assert_eq!(total, single);
    }

    #[test]
    fn count_messages_multiple() {
        let counter = TokenCounter::new();
        let messages = vec![
            Message::try_user("Hello!").expect("non-empty test message"),
            Message::try_user("How are you today?").expect("non-empty test message"),
            Message::try_user("I have a question about Rust.").expect("non-empty test message"),
        ];

        let total = counter.count_messages(&messages);

        let sum: u32 = messages.iter().map(|m| counter.count_message(m)).sum();

        assert_eq!(total, sum);
    }

    #[test]
    fn counter_is_copy_and_clone() {
        let counter = TokenCounter::new();
        let copied = counter;
        let cloned = counter;

        assert_eq!(counter.count_str("test"), copied.count_str("test"));
        assert_eq!(counter.count_str("test"), cloned.count_str("test"));
    }

    #[test]
    fn multiple_counters_share_encoder() {
        let counter1 = TokenCounter::new();
        let counter2 = TokenCounter::new();
        let counter3 = TokenCounter::default();

        let text = "The quick brown fox";
        assert_eq!(counter1.count_str(text), counter2.count_str(text));
        assert_eq!(counter2.count_str(text), counter3.count_str(text));
    }

    #[test]
    fn consistent_token_counts() {
        let counter = TokenCounter::new();
        let text = "This is a test sentence for token counting.";

        let count1 = counter.count_str(text);
        let count2 = counter.count_str(text);
        let count3 = counter.count_str(text);

        assert_eq!(count1, count2);
        assert_eq!(count2, count3);
    }
}
