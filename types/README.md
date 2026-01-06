# forge-types

Core domain types for Forge - a vim-modal TUI for LLMs.

This crate contains pure domain types with **no IO, no async, and minimal dependencies**. Every type here can be safely used from any layer of the application without pulling in runtime complexity.

## Design Philosophy

This crate follows **type-driven design** principles where invalid states are unrepresentable:

- **Invariants at Construction**: Types like `NonEmptyString` and `OutputLimits` enforce their constraints when created, not when used.
- **Provider Scoping**: `ModelName` and `ApiKey` are bound to their provider, preventing cross-provider mixing at compile time.
- **Sum Types over Tags**: `Message` is a proper enum with role-specific data, not a role tag with optional fields.

## Dependencies

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
thiserror = "2.0"
```

---

## Public API Reference

### NonEmpty String Types

#### `NonEmptyString`

A string guaranteed to be non-empty after trimming whitespace.

```rust
use forge_types::{NonEmptyString, EmptyStringError};

// Construction (fallible)
let s = NonEmptyString::new("hello")?;          // Ok
let s = NonEmptyString::new("")?;               // Err(EmptyStringError)
let s = NonEmptyString::new("   ")?;            // Err(EmptyStringError)

// From traits
let s: NonEmptyString = "hello".try_into()?;
let s: NonEmptyString = String::from("hello").try_into()?;

// Access
assert_eq!(s.as_str(), "hello");
assert_eq!(&*s, "hello");                       // via Deref

// Mutation
let s = s.append(" world");                     // Consumes and returns new instance
assert_eq!(s.as_str(), "hello world");

// Conversion back to String
let raw: String = s.into_inner();
let raw: String = s.into();                     // via From trait
```

**Serde Support**: Serializes as a plain string. Deserialization validates non-emptiness and fails with an error if the string is empty or whitespace-only.

#### `NonEmptyStaticStr`

A compile-time checked non-empty static string. Panics at compile time if empty.

```rust
use forge_types::NonEmptyStaticStr;

// Compile-time validation
const GREETING: NonEmptyStaticStr = NonEmptyStaticStr::new("Hello");

// This would fail to compile:
// const BAD: NonEmptyStaticStr = NonEmptyStaticStr::new("");

// Access (all const)
let s: &'static str = GREETING.as_str();

// Convert to runtime NonEmptyString
let runtime: NonEmptyString = GREETING.into();
```

#### `EmptyStringError`

Error type returned when attempting to create a `NonEmptyString` from an empty or whitespace-only input.

```rust
use forge_types::EmptyStringError;

// Error message
let err = EmptyStringError;
assert_eq!(err.to_string(), "message content must not be empty");
```

---

### Provider & Model Types

#### `Provider`

Enumeration of supported LLM providers.

```rust
use forge_types::Provider;

// Variants
let claude = Provider::Claude;   // Default
let openai = Provider::OpenAI;

// String representations
assert_eq!(claude.as_str(), "claude");
assert_eq!(claude.display_name(), "Claude");
assert_eq!(openai.display_name(), "GPT");

// Environment variable names
assert_eq!(claude.env_var(), "ANTHROPIC_API_KEY");
assert_eq!(openai.env_var(), "OPENAI_API_KEY");

// Default models
let model = Provider::Claude.default_model();
assert_eq!(model.as_str(), "claude-sonnet-4-5-20250929");

// Available models
let models: &[&str] = Provider::Claude.available_models();
// ["claude-sonnet-4-5-20250929", "claude-haiku-4-5-20251001", "claude-opus-4-5-20251101"]

let models: &[&str] = Provider::OpenAI.available_models();
// ["gpt-5.2", "gpt-5.2-2025-12-11", "gpt-5.2-chat-latest"]

// Parsing from user input (case-insensitive, multiple aliases)
assert_eq!(Provider::from_str("claude"), Some(Provider::Claude));
assert_eq!(Provider::from_str("Anthropic"), Some(Provider::Claude));
assert_eq!(Provider::from_str("openai"), Some(Provider::OpenAI));
assert_eq!(Provider::from_str("gpt"), Some(Provider::OpenAI));
assert_eq!(Provider::from_str("chatgpt"), Some(Provider::OpenAI));
assert_eq!(Provider::from_str("unknown"), None);

// Enumerate all providers
for provider in Provider::all() {
    println!("{}", provider.display_name());
}
```

#### `ModelName`

A provider-scoped model name that prevents mixing models across providers.

```rust
use forge_types::{Provider, ModelName, ModelNameKind, ModelParseError};

// Parse from user input (validates and normalizes)
let model = Provider::Claude.parse_model("claude-sonnet-4-5-20250929")?;
assert_eq!(model.provider(), Provider::Claude);
assert_eq!(model.as_str(), "claude-sonnet-4-5-20250929");
assert_eq!(model.kind(), ModelNameKind::Known);

// Unknown models are accepted but marked as Unverified
let model = Provider::Claude.parse_model("claude-future-model")?;
assert_eq!(model.kind(), ModelNameKind::Unverified);

// OpenAI models must start with "gpt-5"
let result = Provider::OpenAI.parse_model("gpt-4o");
assert!(matches!(result, Err(ModelParseError::OpenAIMinimum(_))));

let result = Provider::OpenAI.parse_model("");
assert!(matches!(result, Err(ModelParseError::Empty)));

// Create known models directly (for internal/const use)
const SONNET: ModelName = ModelName::known(Provider::Claude, "claude-sonnet-4-5-20250929");

// Display
println!("Using model: {}", model);  // Prints just the model name
```

#### `ModelNameKind`

Indicates whether a model name is verified against the known model list or user-supplied.

| Variant | Description |
|---------|-------------|
| `Known` | Model exists in `Provider::available_models()` |
| `Unverified` | User-supplied model name not in known list (default) |

#### `ModelParseError`

Errors that can occur when parsing a model name.

| Variant | Condition |
|---------|-----------|
| `Empty` | Model name is empty or whitespace-only |
| `OpenAIMinimum(String)` | OpenAI model does not start with `gpt-5` |

---

### API Key Types

#### `ApiKey`

A provider-scoped API key that prevents the invalid state "OpenAI key used with Claude" from being representable.

```rust
use forge_types::{ApiKey, Provider};

// Create provider-specific keys
let claude_key = ApiKey::Claude("sk-ant-...".into());
let openai_key = ApiKey::OpenAI("sk-...".into());

// Access
assert_eq!(claude_key.provider(), Provider::Claude);
assert_eq!(claude_key.as_str(), "sk-ant-...");
```

---

### OpenAI Request Options

These types configure OpenAI-specific request parameters.

#### `OpenAIReasoningEffort`

Controls how much reasoning the model should perform.

```rust
use forge_types::OpenAIReasoningEffort;

// Variants: None, Low, Medium, High (default), XHigh

// Parse from string (case-insensitive)
assert_eq!(OpenAIReasoningEffort::parse("high"), Some(OpenAIReasoningEffort::High));
assert_eq!(OpenAIReasoningEffort::parse("xhigh"), Some(OpenAIReasoningEffort::XHigh));
assert_eq!(OpenAIReasoningEffort::parse("x-high"), Some(OpenAIReasoningEffort::XHigh));

// Convert to API string
assert_eq!(OpenAIReasoningEffort::High.as_str(), "high");
```

#### `OpenAITextVerbosity`

Controls response verbosity level.

```rust
use forge_types::OpenAITextVerbosity;

// Variants: Low, Medium, High (default)

let verbosity = OpenAITextVerbosity::parse("medium").unwrap();
assert_eq!(verbosity.as_str(), "medium");
```

#### `OpenAITruncation`

Controls whether long contexts are automatically truncated.

```rust
use forge_types::OpenAITruncation;

// Variants: Auto (default), Disabled

let truncation = OpenAITruncation::parse("disabled").unwrap();
assert_eq!(truncation.as_str(), "disabled");
```

#### `OpenAIRequestOptions`

Combines all OpenAI-specific request configuration.

```rust
use forge_types::{OpenAIRequestOptions, OpenAIReasoningEffort, OpenAITextVerbosity, OpenAITruncation};

// Create with specific options
let opts = OpenAIRequestOptions::new(
    OpenAIReasoningEffort::High,
    OpenAITextVerbosity::Medium,
    OpenAITruncation::Auto,
);

// Access individual settings
let effort = opts.reasoning_effort();
let verbosity = opts.verbosity();
let truncation = opts.truncation();

// Default configuration (High reasoning, High verbosity, Auto truncation)
let default_opts = OpenAIRequestOptions::default();
```

---

### Caching & Output Limits

#### `CacheHint`

A hint for whether content should be cached by the provider.

```rust
use forge_types::CacheHint;

// Variants
let none = CacheHint::None;           // No caching preference (default)
let ephemeral = CacheHint::Ephemeral; // Content should be cached if supported
```

**Provider Behavior**:
- **Claude**: Explicit `cache_control: { type: "ephemeral" }` markers are added
- **OpenAI**: Automatic server-side prefix caching (hints are ignored)

#### `OutputLimits`

Validated output configuration that guarantees invariants by construction.

**Invariants enforced**:
- If thinking is enabled: `thinking_budget >= 1024`
- If thinking is enabled: `thinking_budget < max_output_tokens`

```rust
use forge_types::{OutputLimits, OutputLimitsError};

// Without thinking (always succeeds)
let limits = OutputLimits::new(4096);
assert_eq!(limits.max_output_tokens(), 4096);
assert_eq!(limits.thinking_budget(), None);
assert!(!limits.has_thinking());

// With thinking (validated)
let limits = OutputLimits::with_thinking(8192, 4096)?;
assert_eq!(limits.max_output_tokens(), 8192);
assert_eq!(limits.thinking_budget(), Some(4096));
assert!(limits.has_thinking());

// Validation errors
let result = OutputLimits::with_thinking(4096, 512);
assert!(matches!(result, Err(OutputLimitsError::ThinkingBudgetTooSmall)));

let result = OutputLimits::with_thinking(4096, 5000);
assert!(matches!(result, Err(OutputLimitsError::ThinkingBudgetTooLarge { .. })));
```

---

### Streaming Events

#### `StreamEvent`

Events emitted during streaming responses.

```rust
use forge_types::StreamEvent;

match event {
    StreamEvent::TextDelta(text) => {
        // Append text to response
        response.push_str(&text);
    }
    StreamEvent::ThinkingDelta(thought) => {
        // Claude extended thinking content
        thinking.push_str(&thought);
    }
    StreamEvent::Done => {
        // Stream completed successfully
    }
    StreamEvent::Error(msg) => {
        // Error occurred during streaming
        eprintln!("Stream error: {}", msg);
    }
}
```

#### `StreamFinishReason`

Reason why a stream finished.

```rust
use forge_types::StreamFinishReason;

match reason {
    StreamFinishReason::Done => println!("Completed successfully"),
    StreamFinishReason::Error(msg) => eprintln!("Failed: {}", msg),
}
```

---

### Message Types

#### `SystemMessage`

A system prompt message with content and timestamp.

```rust
use forge_types::{SystemMessage, NonEmptyString};

let content = NonEmptyString::new("You are a helpful assistant.")?;
let msg = SystemMessage::new(content);
assert_eq!(msg.content(), "You are a helpful assistant.");
```

#### `UserMessage`

A user input message with content and timestamp.

```rust
use forge_types::{UserMessage, NonEmptyString};

let content = NonEmptyString::new("Hello, world!")?;
let msg = UserMessage::new(content);
assert_eq!(msg.content(), "Hello, world!");
```

#### `AssistantMessage`

An assistant response with content, timestamp, and the model that generated it.

```rust
use forge_types::{AssistantMessage, ModelName, Provider, NonEmptyString};

let model = Provider::Claude.default_model();
let content = NonEmptyString::new("Hello! How can I help you today?")?;
let msg = AssistantMessage::new(model, content);

assert_eq!(msg.content(), "Hello! How can I help you today?");
assert_eq!(msg.provider(), Provider::Claude);
assert_eq!(msg.model().as_str(), "claude-sonnet-4-5-20250929");
```

#### `Message`

A sum type representing any message in a conversation.

```rust
use forge_types::{Message, NonEmptyString, Provider};

// Create messages
let system = Message::system(NonEmptyString::new("You are helpful.")?);
let user = Message::user(NonEmptyString::new("Hi!")?);
let assistant = Message::assistant(
    Provider::Claude.default_model(),
    NonEmptyString::new("Hello!")?
);

// Convenience constructor with validation
let user = Message::try_user("Hello")?;  // Returns Result

// Access role
assert_eq!(system.role_str(), "system");
assert_eq!(user.role_str(), "user");
assert_eq!(assistant.role_str(), "assistant");

// Access content
assert_eq!(user.content(), "Hello");

// Pattern match for role-specific data
match &message {
    Message::System(m) => println!("System: {}", m.content()),
    Message::User(m) => println!("User: {}", m.content()),
    Message::Assistant(m) => {
        println!("Assistant ({:?}): {}", m.provider(), m.content());
    }
}
```

**Serde Support**: Messages serialize with their role and content. `AssistantMessage` includes the model information via `#[serde(flatten)]`.

#### `CacheableMessage`

A message paired with a cache hint for API serialization.

```rust
use forge_types::{CacheableMessage, Message, CacheHint, NonEmptyString};

let msg = Message::system(NonEmptyString::new("You are helpful.")?);

// No caching
let plain = CacheableMessage::plain(msg.clone());
assert_eq!(plain.cache_hint, CacheHint::None);

// With caching hint
let cached = CacheableMessage::cached(msg.clone());
assert_eq!(cached.cache_hint, CacheHint::Ephemeral);

// Explicit construction
let explicit = CacheableMessage::new(msg, CacheHint::Ephemeral);
```

---

## Architecture & Design Patterns

### 1. Invariants at Construction

Invalid values cannot be created. Validation happens once at construction, not repeatedly at usage sites.

```rust
// NonEmptyString: Cannot be empty
let s = NonEmptyString::new("")?;  // Fails here, not later

// OutputLimits: thinking_budget constraints enforced
let limits = OutputLimits::with_thinking(4096, 5000)?;  // Fails: budget >= max

// ModelName: OpenAI prefix validated
let model = Provider::OpenAI.parse_model("gpt-4o")?;  // Fails: must be gpt-5+
```

### 2. Provider Scoping

Types are bound to their provider to prevent mixing at compile time.

```rust
// ModelName is scoped to a provider
let claude_model = Provider::Claude.parse_model("claude-sonnet-4-5-20250929")?;
assert_eq!(claude_model.provider(), Provider::Claude);

// ApiKey variants are provider-specific
let key = ApiKey::Claude("...".into());
assert_eq!(key.provider(), Provider::Claude);
```

### 3. True Sum Types

`Message` is a proper enum where each variant has role-specific data, rather than a role tag with optional fields.

```rust
// Each variant has exactly the fields it needs
pub enum Message {
    System(SystemMessage),      // Has: content, timestamp
    User(UserMessage),          // Has: content, timestamp  
    Assistant(AssistantMessage), // Has: content, timestamp, model
}
```

### 4. Compile-Time vs Runtime Validation

- `NonEmptyStaticStr`: Validates at compile time via `const fn` panic
- `NonEmptyString`: Validates at runtime, returns `Result`

### 5. Zero-Cost Abstractions

- `Cow<'static, str>` in `ModelName` avoids allocations for known models
- `Deref` and `AsRef` implementations allow seamless string access
- No heap allocations for static data

---

## Type Relationship Diagram

```
                    NonEmptyString <------- NonEmptyStaticStr
                          |                     (compile-time)
                          v
    +---------------------+----------------------+
    |                     |                      |
SystemMessage        UserMessage          AssistantMessage
    |                     |                      |
    +---------------------+----------------------+
                          |
                          v
                      Message  <------>  CacheableMessage
                                              |
                                              v
                                          CacheHint

Provider -----> ModelName (scoped)
    |               |
    |               +---> ModelNameKind
    |
    +-----> ApiKey (scoped)
    |
    +-----> OpenAIRequestOptions
                |
                +---> OpenAIReasoningEffort
                +---> OpenAITextVerbosity
                +---> OpenAITruncation

StreamEvent -----> StreamFinishReason

OutputLimits (validated invariants)
```

---

## Error Types Summary

| Error Type | Used By | Condition |
|------------|---------|-----------|
| `EmptyStringError` | `NonEmptyString::new()` | Empty or whitespace-only input |
| `ModelParseError::Empty` | `ModelName::parse()` | Empty model name |
| `ModelParseError::OpenAIMinimum` | `ModelName::parse()` | OpenAI model without `gpt-5` prefix |
| `OutputLimitsError::ThinkingBudgetTooSmall` | `OutputLimits::with_thinking()` | `thinking_budget < 1024` |
| `OutputLimitsError::ThinkingBudgetTooLarge` | `OutputLimits::with_thinking()` | `thinking_budget >= max_output_tokens` |

---

## Testing

Run the crate's tests:

```bash
cargo test -p forge-types
```

The test suite verifies:
- `NonEmptyString` rejects empty and whitespace-only strings
- `Provider::from_str()` handles all aliases correctly
- `ModelName::parse()` validates OpenAI prefix requirements
- `OutputLimits::with_thinking()` enforces budget constraints

---

## License

See the repository root for license information.
