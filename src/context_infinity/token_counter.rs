//! Token counting using tiktoken.
//!
//! This module provides accurate token counting compatible with OpenAI and
//! Anthropic models using the cl100k_base encoding (used by GPT-4, Claude, etc.).

use std::sync::OnceLock;
use tiktoken_rs::{CoreBPE, cl100k_base};

use crate::message::Message;

/// Singleton encoder instance.
///
/// The tiktoken encoder is expensive to initialize (loads vocabulary data),
/// so we create it once and reuse it across all `TokenCounter` instances.
static ENCODER: OnceLock<CoreBPE> = OnceLock::new();

/// Returns a reference to the shared encoder instance.
///
/// Initializes the encoder on first call using `cl100k_base` encoding.
fn get_encoder() -> &'static CoreBPE {
    ENCODER
        .get_or_init(|| cl100k_base().expect("Failed to initialize tiktoken cl100k_base encoder"))
}

/// Thread-safe token counter using tiktoken's cl100k_base encoding.
///
/// This counter is compatible with GPT-4, GPT-3.5-turbo, and Claude models.
/// It uses a singleton encoder instance for efficiency.
///
/// # Token Counting Overhead
///
/// When counting tokens for chat messages, this counter adds a ~4 token
/// overhead per message to account for:
/// - Role markers (e.g., "user", "assistant")
/// - Message formatting/delimiters
///
/// # Example
///
/// ```
/// use forge::TokenCounter;
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
    encoder: &'static CoreBPE,
}

impl std::fmt::Debug for TokenCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenCounter")
            .field("encoder", &"<CoreBPE>")
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
        Self {
            encoder: get_encoder(),
        }
    }

    /// Counts the number of tokens in a string.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to count tokens for
    ///
    /// # Returns
    ///
    /// The number of tokens in the text.
    ///
    /// # Example
    ///
    /// ```
    /// use forge::TokenCounter;
    ///
    /// let counter = TokenCounter::new();
    /// let tokens = counter.count_str("Hello, world!");
    /// println!("Token count: {tokens}");
    /// ```
    #[must_use]
    pub fn count_str(&self, text: &str) -> u32 {
        self.encoder.encode_ordinary(text).len() as u32
    }

    /// Counts tokens for a single message, including role overhead.
    ///
    /// Each message has approximately 4 tokens of overhead for:
    /// - The role name (e.g., "user", "assistant")
    /// - Message structure/delimiters
    ///
    /// # Arguments
    ///
    /// * `msg` - The message to count tokens for
    ///
    /// # Returns
    ///
    /// The total number of tokens including content and overhead.
    ///
    /// # Example
    ///
    /// ```
    /// use forge::TokenCounter;
    /// use forge::message::Message;
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
    /// # Arguments
    ///
    /// * `messages` - The messages to count tokens for
    ///
    /// # Returns
    ///
    /// The total number of tokens across all messages.
    ///
    /// # Example
    ///
    /// ```
    /// use forge::context_infinity::TokenCounter;
    /// use forge::message::Message;
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
        // Just verify it doesn't panic
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

        // "Hello" is typically 1 token
        let tokens = counter.count_str("Hello");
        assert!(tokens >= 1);

        // "Hello, world!" is typically 4 tokens
        let tokens = counter.count_str("Hello, world!");
        assert!(tokens >= 1);
    }

    #[test]
    fn count_str_longer_text() {
        let counter = TokenCounter::new();

        let text = "The quick brown fox jumps over the lazy dog.";
        let tokens = counter.count_str(text);

        // Should be roughly 10 tokens
        assert!(tokens >= 5);
        assert!(tokens <= 20);
    }

    #[test]
    fn count_str_unicode() {
        let counter = TokenCounter::new();

        // Unicode characters may take multiple tokens
        let tokens = counter.count_str("Hello, world!");
        let tokens_cn = counter.count_str("Hello, world! :)");

        // Both should produce valid token counts
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

        // Code should tokenize reasonably
        assert!(tokens >= 5);
    }

    #[test]
    fn count_message_user() {
        let counter = TokenCounter::new();
        let msg = Message::try_user("Hello!").expect("non-empty test message");

        let tokens = counter.count_message(&msg);

        // Should include content + role + overhead
        let content_tokens = counter.count_str("Hello!");
        let role_tokens = counter.count_str("user");
        let expected_min = content_tokens + role_tokens + 4; // 4 = MESSAGE_OVERHEAD

        assert_eq!(tokens, expected_min);
    }

    #[test]
    fn count_message_includes_overhead() {
        let counter = TokenCounter::new();
        let msg = Message::try_user("Hi").expect("non-empty test message");

        let content_tokens = counter.count_str("Hi");
        let message_tokens = counter.count_message(&msg);

        // Message tokens should be greater than just content
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
        let cloned = counter.clone();

        // All should work identically
        assert_eq!(counter.count_str("test"), copied.count_str("test"));
        assert_eq!(counter.count_str("test"), cloned.count_str("test"));
    }

    #[test]
    fn multiple_counters_share_encoder() {
        // Creating multiple counters should be cheap
        let counter1 = TokenCounter::new();
        let counter2 = TokenCounter::new();
        let counter3 = TokenCounter::default();

        // All should produce the same results
        let text = "The quick brown fox";
        assert_eq!(counter1.count_str(text), counter2.count_str(text));
        assert_eq!(counter2.count_str(text), counter3.count_str(text));
    }

    #[test]
    fn consistent_token_counts() {
        let counter = TokenCounter::new();
        let text = "This is a test sentence for token counting.";

        // Multiple calls should return the same result
        let count1 = counter.count_str(text);
        let count2 = counter.count_str(text);
        let count3 = counter.count_str(text);

        assert_eq!(count1, count2);
        assert_eq!(count2, count3);
    }
}
