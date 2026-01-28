# forge-types

Core domain types for Forge with **no IO, no async, and minimal dependencies**.

This crate provides the foundational type system that enforces correctness at compile time. Every type can be safely used from any layer of the application without pulling in runtime complexity.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-46 | Header, LLM-TOC, Table of Contents |
| 47-111 | Design Philosophy |
| 112-131 | Module Structure |
| 133-241 | NonEmpty String Types |
| 243-394 | Provider and Model Types |
| 396-436 | API Key Types |
| 438-532 | OpenAI Request Options |
| 534-619 | Caching and Output Limits |
| 621-686 | Streaming Events |
| 688-794 | Tool Calling Types |
| 796-944 | Message Types |
| 946-1017 | Terminal Sanitization |
| 1019-1062 | Type Relationships |
| 1064-1078 | Error Types Summary |
| 1080-1097 | Testing |
| 1099-1131 | Extending the Crate |

## Table of Contents

- [Design Philosophy](#design-philosophy)
- [Module Structure](#module-structure)
- [NonEmpty String Types](#nonempty-string-types)
- [Provider and Model Types](#provider-and-model-types)
- [API Key Types](#api-key-types)
- [OpenAI Request Options](#openai-request-options)
- [Caching and Output Limits](#caching-and-output-limits)
- [Streaming Events](#streaming-events)
- [Tool Calling Types](#tool-calling-types)
- [Message Types](#message-types)
- [Terminal Sanitization](#terminal-sanitization)
- [Type Relationships](#type-relationships)
- [Error Types Summary](#error-types-summary)
- [Testing](#testing)

---

## Design Philosophy

This crate follows **type-driven design** principles where invalid states are unrepresentable:

### 1. Invariants at Construction

Types validate their constraints when created, not when used. Once you have a value of a type, you know it satisfies all required invariants.

```rust
// NonEmptyString: Cannot be empty - fails at construction
let s = NonEmptyString::new("")?;  // Err(EmptyStringError)

// OutputLimits: thinking_budget constraints enforced at creation
let limits = OutputLimits::with_thinking(4096, 5000)?;  // Err: budget >= max

// ModelName: Provider prefix validated during parsing
let model = Provider::OpenAI.parse_model("gpt-5.2")?;  // Ok
let model = Provider::Gemini.parse_model("gemini-1.5-pro")?;  // Ok
```

### 2. Provider Scoping

Types that belong to a provider carry that association, preventing cross-provider mixing at compile time.

```rust
// ModelName is bound to its provider
let model = Provider::Claude.parse_model("claude-opus-4-5-20251101")?;
assert_eq!(model.provider(), Provider::Claude);

// ApiKey variants are provider-specific
let key = ApiKey::Claude("sk-ant-...".into());
assert_eq!(key.provider(), Provider::Claude);
```

### 3. True Sum Types

`Message` is a proper enum where each variant contains role-specific data, rather than a role tag with optional fields that may or may not be meaningful.

```rust
pub enum Message {
    System(SystemMessage),      // content, timestamp
    User(UserMessage),          // content, timestamp  
    Assistant(AssistantMessage), // content, timestamp, model
    ToolUse(ToolCall),          // id, name, arguments, optional thought_signature
    ToolResult(ToolResult),     // tool_call_id, content, is_error
}
```

### 4. Compile-Time vs Runtime Validation

The crate provides both options depending on use case:

| Type | Validation | Use Case |
| ---- | ---------- | -------- |
| `NonEmptyStaticStr` | Compile-time (const fn panic) | Static strings, constants |
| `NonEmptyString` | Runtime (Result) | Dynamic user input |

### 5. Zero-Cost Abstractions

- `Cow<'static, str>` in `ModelName` avoids allocations for known models
- `Deref` and `AsRef` implementations allow seamless string access
- `sanitize_terminal_text` returns `Cow::Borrowed` when no changes needed

---

## Module Structure

```text
forge-types/
├── Cargo.toml
└── src/
    ├── lib.rs        # All public types and core implementations
    └── sanitize.rs   # Terminal text sanitization (security)
```

**Dependencies** (minimal by design):

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
```

---

## NonEmpty String Types

### NonEmptyString

A string guaranteed to be non-empty after trimming whitespace.

**Invariants:**

- Content is never empty after `trim()`
- Whitespace-only strings are rejected

**Construction:**

```rust
use forge_types::{NonEmptyString, EmptyStringError};

// Primary constructor (fallible)
let s = NonEmptyString::new("hello")?;          // Ok
let s = NonEmptyString::new("")?;               // Err(EmptyStringError)
let s = NonEmptyString::new("   ")?;            // Err(EmptyStringError)

// From traits
let s: NonEmptyString = "hello".try_into()?;
let s: NonEmptyString = String::from("hello").try_into()?;
```

**Access:**

```rust
// Explicit access
let content: &str = s.as_str();

// Via Deref (seamless string operations)
let len = s.len();
let upper = s.to_uppercase();
assert!(s.contains("ell"));

// Via AsRef<str>
fn accepts_str(s: impl AsRef<str>) { }
accepts_str(&s);
```

**Mutation:**

```rust
// Append consumes self and returns new instance (preserves invariant)
let s = NonEmptyString::new("hello")?;
let s = s.append(" world");
assert_eq!(s.as_str(), "hello world");
```

**Conversion:**

```rust
// Extract inner String
let raw: String = s.into_inner();
let raw: String = s.into();  // via From trait
```

**Serde Behavior:**

- Serializes as a plain JSON string
- Deserialization validates non-emptiness and fails with error if invalid

### NonEmptyStaticStr

A compile-time checked non-empty static string. Validation occurs at compile time via `const fn` panic.

**Invariants:**

- Content is never empty
- Validation guaranteed at compile time

```rust
use forge_types::NonEmptyStaticStr;

// Compile-time validation (const context)
const GREETING: NonEmptyStaticStr = NonEmptyStaticStr::new("Hello");

// This fails to compile:
// const BAD: NonEmptyStaticStr = NonEmptyStaticStr::new("");
// error: evaluation of constant panics

// Access (all methods are const)
let s: &'static str = GREETING.as_str();

// Convert to runtime NonEmptyString (fallible - whitespace-only would fail)
let runtime: NonEmptyString = NonEmptyString::try_from(GREETING)?;
```

**Use Cases:**

- System prompts
- Error messages
- Default values
- Any string constant that must be non-empty

### EmptyStringError

Error type returned when constructing `NonEmptyString` from empty or whitespace-only input.

```rust
use forge_types::EmptyStringError;

let err = EmptyStringError;
assert_eq!(err.to_string(), "message content must not be empty");
```

---

## Provider and Model Types

### Provider

Enumeration of supported LLM providers with associated metadata.

**Variants:**

| Variant | Default | Display Name | Env Var |
| ------- | ------- | ------------ | ------- |
| `Claude` | Yes | "Claude" | `ANTHROPIC_API_KEY` |
| `OpenAI` | No | "GPT" | `OPENAI_API_KEY` |
| `Gemini` | No | "Gemini" | `GEMINI_API_KEY` |

**Usage:**

```rust
use forge_types::Provider;

// Default provider
let provider = Provider::default();  // Claude

// String representations
assert_eq!(Provider::Claude.as_str(), "claude");
assert_eq!(Provider::Claude.display_name(), "Claude");
assert_eq!(Provider::OpenAI.display_name(), "GPT");

// Environment variable names
assert_eq!(Provider::Claude.env_var(), "ANTHROPIC_API_KEY");
assert_eq!(Provider::OpenAI.env_var(), "OPENAI_API_KEY");

// Parse from user input (case-insensitive, multiple aliases)
assert_eq!(Provider::parse("claude"), Some(Provider::Claude));
assert_eq!(Provider::parse("Anthropic"), Some(Provider::Claude));
assert_eq!(Provider::parse("openai"), Some(Provider::OpenAI));
assert_eq!(Provider::parse("gpt"), Some(Provider::OpenAI));
assert_eq!(Provider::parse("gemini"), Some(Provider::Gemini));
assert_eq!(Provider::parse("google"), Some(Provider::Gemini));
assert_eq!(Provider::parse("unknown"), None);

// Enumerate all providers
for provider in Provider::all() {
    println!("{}: {}", provider.as_str(), provider.display_name());
}
```

**Model Operations:**

```rust
// Get default model for provider
let model = Provider::Claude.default_model();
assert_eq!(model.as_str(), "claude-opus-4-5-20251101");

let model = Provider::Gemini.default_model();
assert_eq!(model.as_str(), "gemini-3-pro-preview");

// List available models
let models: &[&str] = Provider::Claude.available_models();
// ["claude-opus-4-5-20251101", "claude-haiku-4-5-20251001"]

let models: &[&str] = Provider::OpenAI.available_models();
// ["gpt-5.2", "gpt-5.2-pro", "gpt-5.2-2025-12-11"]

let models: &[&str] = Provider::Gemini.available_models();
// ["gemini-3-pro-preview", "gemini-3-flash-preview"]

// Parse model name with validation
let model = Provider::Claude.parse_model("claude-opus-4-5-20251101")?;
```

### ModelName

A provider-scoped model name that prevents mixing models across providers.

**Invariants:**

- Always associated with a specific `Provider`
- Claude models must start with `claude-`
- OpenAI models must start with `gpt-5`
- Gemini models must start with `gemini-`
- Empty model names are rejected

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `provider` | `Provider` | The provider this model belongs to |
| `name` | `Cow<'static, str>` | The model name string |
| `kind` | `ModelNameKind` | Whether model is known or user-supplied |

**Construction:**

```rust
use forge_types::{Provider, ModelName, ModelNameKind, ModelParseError};

// Parse from user input (validates prefix, checks known list)
let model = Provider::Claude.parse_model("claude-opus-4-5-20251101")?;
assert_eq!(model.provider(), Provider::Claude);
assert_eq!(model.as_str(), "claude-opus-4-5-20251101");
assert_eq!(model.kind(), ModelNameKind::Known);

// Unknown models are accepted but marked as Unverified
let model = Provider::Claude.parse_model("claude-future-model")?;
assert_eq!(model.kind(), ModelNameKind::Unverified);

// Validation errors
let result = Provider::OpenAI.parse_model("gpt-4o");
assert!(matches!(result, Err(ModelParseError::OpenAIMinimum(_))));

let result = Provider::Claude.parse_model("gpt-5.2");
assert!(matches!(result, Err(ModelParseError::ClaudePrefix(_))));

let result = Provider::OpenAI.parse_model("");
assert!(matches!(result, Err(ModelParseError::Empty)));

// Create known model directly (for internal/const use)
const OPUS: ModelName = ModelName::known(Provider::Claude, "claude-opus-4-5-20251101");
```

**Memory Optimization:**

Known models use `Cow::Borrowed` to avoid allocation:

```rust
// Known model - no heap allocation
let known = Provider::Claude.default_model();  // Cow::Borrowed

// User-supplied model - allocates
let custom = Provider::Claude.parse_model("claude-custom-v1")?;  // Cow::Owned
```

### ModelNameKind

Indicates whether a model name was verified against the known model list.

| Variant | Description |
| ------- | ----------- |
| `Known` | Model exists in `Provider::available_models()` |
| `Unverified` | User-supplied model name not in known list (default) |

### ModelParseError

Errors that occur when parsing a model name.

| Variant | Condition | Message |
| ------- | --------- | ------- |
| `Empty` | Model name is empty or whitespace-only | "model name cannot be empty" |
| `ClaudePrefix(String)` | Claude model missing `claude-` prefix | "Claude model must start with claude-" |
| `OpenAIMinimum(String)` | OpenAI model missing `gpt-5` prefix | "OpenAI model must start with gpt-5" |
| `GeminiPrefix(String)` | Gemini model missing `gemini-` prefix | "Gemini model must start with gemini-" |

---

## API Key Types

### ApiKey

A provider-scoped API key that prevents using a key with the wrong provider.

**Variants:**

```rust
pub enum ApiKey {
    Claude(String),
    OpenAI(String),
    Gemini(String),
}
```

**Invariant:** The key string is always associated with its correct provider.

**Usage:**

```rust
use forge_types::{ApiKey, Provider};

// Create provider-specific keys
let claude_key = ApiKey::Claude("sk-ant-api03-...".into());
let openai_key = ApiKey::OpenAI("sk-proj-...".into());

// Access provider
assert_eq!(claude_key.provider(), Provider::Claude);
assert_eq!(openai_key.provider(), Provider::OpenAI);
assert_eq!(gemini_key.provider(), Provider::Gemini);

// Access key string
assert_eq!(claude_key.as_str(), "sk-ant-api03-...");
```

**Design Rationale:**

By making `ApiKey` a sum type rather than a struct with a provider field, the compiler ensures you cannot accidentally pass a Claude key to OpenAI client code. The key and provider are inseparable.

---

## OpenAI Request Options

Configuration types for OpenAI-specific request parameters.

### OpenAIReasoningEffort

Controls how much reasoning the model should perform before responding.

| Variant | API Value | Description |
| ------- | --------- | ----------- |
| `None` | "none" | No reasoning |
| `Low` | "low" | Minimal reasoning |
| `Medium` | "medium" | Moderate reasoning |
| `High` (default) | "high" | Full reasoning |
| `XHigh` | "xhigh" | Extended reasoning |

```rust
use forge_types::OpenAIReasoningEffort;

// Parse from string (case-insensitive)
assert_eq!(OpenAIReasoningEffort::parse("high"), Some(OpenAIReasoningEffort::High));
assert_eq!(OpenAIReasoningEffort::parse("xhigh"), Some(OpenAIReasoningEffort::XHigh));
assert_eq!(OpenAIReasoningEffort::parse("x-high"), Some(OpenAIReasoningEffort::XHigh));
assert_eq!(OpenAIReasoningEffort::parse("invalid"), None);

// Convert to API string
assert_eq!(OpenAIReasoningEffort::High.as_str(), "high");
assert_eq!(OpenAIReasoningEffort::XHigh.as_str(), "xhigh");

// Default
assert_eq!(OpenAIReasoningEffort::default(), OpenAIReasoningEffort::High);
```

### OpenAIReasoningSummary

Controls whether OpenAI returns a reasoning summary (for supported models).

| Variant | API Value | Description |
| ------- | --------- | ----------- |
| `None` (default) | "none" | Do not request a reasoning summary |
| `Auto` | "auto" | Request the most detailed summary available |
| `Concise` | "concise" | Request a concise summary |
| `Detailed` | "detailed" | Request a detailed summary |

```rust
use forge_types::OpenAIReasoningSummary;

assert_eq!(OpenAIReasoningSummary::parse("auto"), Some(OpenAIReasoningSummary::Auto));
assert_eq!(OpenAIReasoningSummary::parse("CONCISE"), Some(OpenAIReasoningSummary::Concise));
assert_eq!(OpenAIReasoningSummary::parse("detailed"), Some(OpenAIReasoningSummary::Detailed));
assert_eq!(OpenAIReasoningSummary::parse("invalid"), None);

assert_eq!(OpenAIReasoningSummary::Auto.as_str(), "auto");
assert_eq!(OpenAIReasoningSummary::default(), OpenAIReasoningSummary::None);
```

### OpenAITextVerbosity

Controls response verbosity level.

| Variant | API Value | Description |
| ------- | --------- | ----------- |
| `Low` | "low" | Concise responses |
| `Medium` | "medium" | Balanced verbosity |
| `High` (default) | "high" | Detailed responses |

```rust
use forge_types::OpenAITextVerbosity;

let verbosity = OpenAITextVerbosity::parse("medium").unwrap();
assert_eq!(verbosity.as_str(), "medium");
assert_eq!(OpenAITextVerbosity::default(), OpenAITextVerbosity::High);
```

### OpenAITruncation

Controls whether long contexts are automatically truncated.

| Variant | API Value | Description |
| ------- | --------- | ----------- |
| `Auto` (default) | "auto" | Automatic truncation when needed |
| `Disabled` | "disabled" | No truncation (may error on overflow) |

```rust
use forge_types::OpenAITruncation;

let truncation = OpenAITruncation::parse("disabled").unwrap();
assert_eq!(truncation.as_str(), "disabled");
assert_eq!(OpenAITruncation::default(), OpenAITruncation::Auto);
```

### OpenAIRequestOptions

Combines all OpenAI-specific request configuration into a single type.

```rust
use forge_types::{
    OpenAIReasoningEffort, OpenAIReasoningSummary, OpenAIRequestOptions, OpenAITextVerbosity,
    OpenAITruncation,
};

// Create with specific options
let opts = OpenAIRequestOptions::new(
    OpenAIReasoningEffort::High,
    OpenAIReasoningSummary::Auto,
    OpenAITextVerbosity::Medium,
    OpenAITruncation::Auto,
);

// Access individual settings
assert_eq!(opts.reasoning_effort(), OpenAIReasoningEffort::High);
assert_eq!(opts.reasoning_summary(), OpenAIReasoningSummary::Auto);
assert_eq!(opts.verbosity(), OpenAITextVerbosity::Medium);
assert_eq!(opts.truncation(), OpenAITruncation::Auto);

// Default configuration
let default_opts = OpenAIRequestOptions::default();
assert_eq!(default_opts.reasoning_effort(), OpenAIReasoningEffort::High);
assert_eq!(default_opts.reasoning_summary(), OpenAIReasoningSummary::None);
assert_eq!(default_opts.verbosity(), OpenAITextVerbosity::High);
assert_eq!(default_opts.truncation(), OpenAITruncation::Auto);
```

---

## Caching and Output Limits

### CacheHint

A hint for whether content should be cached by the provider.

| Variant | Description |
| ------- | ----------- |
| `None` (default) | No caching preference |
| `Ephemeral` | Content should be cached if supported |

**Provider Behavior:**

| Provider | Caching Mechanism |
| -------- | ----------------- |
| Claude | Explicit `cache_control: { type: "ephemeral" }` markers |
| OpenAI | Automatic server-side prefix caching (hints ignored) |

**Naming Note:** The `Ephemeral` variant name matches Anthropic's API terminology. Despite the name suggesting temporary, it actually means "cache this content" - Anthropic uses "ephemeral" to indicate the cache entry has a limited TTL (~5 minutes) rather than permanent storage.

```rust
use forge_types::CacheHint;

let default = CacheHint::default();
assert_eq!(default, CacheHint::None);

let cached = CacheHint::Ephemeral;
```

### OutputLimits

Validated output configuration that guarantees invariants by construction.

**Invariants:**

- If thinking is enabled: `thinking_budget >= 1024`
- If thinking is enabled: `thinking_budget < max_output_tokens`

**Without Thinking:**

```rust
use forge_types::OutputLimits;

// Always succeeds (no validation needed)
let limits = OutputLimits::new(4096);
assert_eq!(limits.max_output_tokens(), 4096);
assert_eq!(limits.thinking_budget(), None);
assert!(!limits.has_thinking());
```

**With Thinking:**

```rust
use forge_types::{OutputLimits, OutputLimitsError};

// Valid configuration
let limits = OutputLimits::with_thinking(16384, 8192)?;
assert_eq!(limits.max_output_tokens(), 16384);
assert_eq!(limits.thinking_budget(), Some(8192));
assert!(limits.has_thinking());

// Minimum budget (1024)
let limits = OutputLimits::with_thinking(4096, 1024)?;

// Budget too small
let result = OutputLimits::with_thinking(4096, 512);
assert!(matches!(result, Err(OutputLimitsError::ThinkingBudgetTooSmall)));

// Budget too large (>= max_output_tokens)
let result = OutputLimits::with_thinking(4096, 4096);
assert!(matches!(result, Err(OutputLimitsError::ThinkingBudgetTooLarge { .. })));

let result = OutputLimits::with_thinking(4096, 5000);
assert!(matches!(result, Err(OutputLimitsError::ThinkingBudgetTooLarge { .. })));
```

### OutputLimitsError

Errors when constructing invalid output limits.

| Variant | Condition | Message |
| ------- | --------- | ------- |
| `ThinkingBudgetTooSmall` | `thinking_budget < 1024` | "thinking budget must be at least 1024 tokens" |
| `ThinkingBudgetTooLarge { budget, max_output }` | `thinking_budget >= max_output_tokens` | "thinking budget ({budget}) must be less than max output tokens ({max_output})" |

---

## Streaming Events

### StreamEvent

Events emitted during streaming API responses.

| Variant | Description |
| ------- | ----------- |
| `TextDelta(String)` | Incremental text content |
| `ThinkingDelta(String)` | Provider reasoning content (Claude extended thinking or OpenAI summaries) |
| `ToolCallStart { id, name, thought_signature }` | Tool use content block began |
| `ToolCallDelta { id, arguments }` | Tool call JSON arguments chunk |
| `Done` | Stream completed successfully |
| `Error(String)` | Error occurred during streaming |

```rust
use forge_types::StreamEvent;

fn handle_event(event: StreamEvent, response: &mut String, thinking: &mut String) {
    match event {
        StreamEvent::TextDelta(text) => {
            response.push_str(&text);
        }
        StreamEvent::ThinkingDelta(thought) => {
            thinking.push_str(&thought);
        }
        StreamEvent::ToolCallStart {
            id,
            name,
            thought_signature: _,
        } => {
            println!("Tool call started: {} ({})", name, id);
        }
        StreamEvent::ToolCallDelta { id, arguments } => {
            println!("Tool {} args: {}", id, arguments);
        }
        StreamEvent::Done => {
            println!("Stream completed");
        }
        StreamEvent::Error(msg) => {
            eprintln!("Stream error: {}", msg);
        }
    }
}
```

### StreamFinishReason

Reason why a stream finished.

| Variant | Description |
| ------- | ----------- |
| `Done` | Completed successfully |
| `Error(String)` | Failed with error message |

```rust
use forge_types::StreamFinishReason;

let reason = StreamFinishReason::Done;
assert_eq!(reason, StreamFinishReason::Done);

let reason = StreamFinishReason::Error("timeout".to_string());
assert_ne!(reason, StreamFinishReason::Done);
```

---

## Tool Calling Types

Types for LLM function/tool calling, following the standard schema used by Claude and OpenAI.

### ToolDefinition

Definition of a tool that can be called by the LLM.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `name` | `String` | The function name |
| `description` | `String` | What the tool does |
| `parameters` | `serde_json::Value` | JSON Schema for parameters |

```rust
use forge_types::ToolDefinition;
use serde_json::json;

let tool = ToolDefinition::new(
    "get_weather",
    "Get the current weather for a location",
    json!({
        "type": "object",
        "properties": {
            "location": {
                "type": "string",
                "description": "The city and state, e.g. San Francisco, CA"
            },
            "unit": {
                "type": "string",
                "enum": ["celsius", "fahrenheit"],
                "description": "Temperature unit"
            }
        },
        "required": ["location"]
    }),
);

assert_eq!(tool.name, "get_weather");
```

### ToolCall

A tool call requested by the LLM during a response.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `id` | `String` | Unique identifier (for matching with results) |
| `name` | `String` | The tool being called |
| `arguments` | `serde_json::Value` | Parsed JSON arguments |
| `thought_signature` | `Option<String>` | Optional thought signature (Gemini) |

```rust
use forge_types::ToolCall;
use serde_json::json;

let call = ToolCall::new(
    "call_abc123",
    "get_weather",
    json!({
        "location": "San Francisco, CA",
        "unit": "fahrenheit"
    }),
);

assert_eq!(call.id, "call_abc123");
assert_eq!(call.name, "get_weather");
```

### ToolResult

The result of executing a tool call.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `tool_call_id` | `String` | ID of the tool call this result is for |
| `tool_name` | `String` | Name of the tool (required for Gemini) |
| `content` | `String` | The result content |
| `is_error` | `bool` | Whether execution resulted in an error |

```rust
use forge_types::ToolResult;

// Successful result
let result = ToolResult::success(
    "call_abc123",
    "get_weather",
    r#"{"temperature": 72, "conditions": "sunny"}"#,
);
assert!(!result.is_error);

// Error result
let result = ToolResult::error(
    "call_abc123",
    "get_weather",
    "Location not found",
);
assert!(result.is_error);
```

---

## Message Types

### SystemMessage

A system prompt message with content and timestamp.

```rust
use forge_types::{SystemMessage, NonEmptyString};

let content = NonEmptyString::new("You are a helpful assistant.")?;
let msg = SystemMessage::new(content);
assert_eq!(msg.content(), "You are a helpful assistant.");
```

**Fields:** `content: NonEmptyString`, `timestamp: SystemTime`

### UserMessage

A user input message with content and timestamp.

```rust
use forge_types::{UserMessage, NonEmptyString};

let content = NonEmptyString::new("Hello, world!")?;
let msg = UserMessage::new(content);
assert_eq!(msg.content(), "Hello, world!");
```

**Fields:** `content: NonEmptyString`, `timestamp: SystemTime`

### AssistantMessage

An assistant response with content, timestamp, and the model that generated it.

```rust
use forge_types::{AssistantMessage, ModelName, Provider, NonEmptyString};

let model = Provider::Claude.default_model();
let content = NonEmptyString::new("Hello! How can I help you today?")?;
let msg = AssistantMessage::new(model, content);

assert_eq!(msg.content(), "Hello! How can I help you today?");
assert_eq!(msg.provider(), Provider::Claude);
assert_eq!(msg.model().as_str(), "claude-opus-4-5-20251101");
```

**Fields:** `content: NonEmptyString`, `timestamp: SystemTime`, `model: ModelName`

### Message

A sum type representing any message in a conversation.

**Variants:**

| Variant | Contains | `role_str()` |
|---------|----------|--------------|
| `System(SystemMessage)` | System prompt | "system" |
| `User(UserMessage)` | User input | "user" |
| `Assistant(AssistantMessage)` | Assistant response | "assistant" |
| `ToolUse(ToolCall)` | Tool call request | "assistant" |
| `ToolResult(ToolResult)` | Tool execution result | "user" |

**Construction:**

```rust
use forge_types::{Message, NonEmptyString, Provider, ToolCall, ToolResult};
use serde_json::json;

// Direct constructors
let system = Message::system(NonEmptyString::new("You are helpful.")?);
let user = Message::user(NonEmptyString::new("Hi!")?);
let assistant = Message::assistant(
    Provider::Claude.default_model(),
    NonEmptyString::new("Hello!")?
);

// Convenience constructor with validation
let user = Message::try_user("Hello")?;  // Returns Result

// Tool messages
let tool_use = Message::tool_use(ToolCall::new(
    "call_123",
    "get_time",
    json!({}),
));

let tool_result = Message::tool_result(ToolResult::success(
    "call_123",
    "get_time",
    "2024-01-15T10:30:00Z",
));
```

**Access:**

```rust
// Role string (for API serialization)
assert_eq!(system.role_str(), "system");
assert_eq!(user.role_str(), "user");
assert_eq!(assistant.role_str(), "assistant");
assert_eq!(tool_use.role_str(), "assistant");    // Tool use is from assistant
assert_eq!(tool_result.role_str(), "user");      // Tool result is sent as user role

// Content access
assert_eq!(user.content(), "Hello");

// Pattern matching for role-specific data
match &message {
    Message::System(m) => println!("System: {}", m.content()),
    Message::User(m) => println!("User: {}", m.content()),
    Message::Assistant(m) => {
        println!("Assistant ({:?}): {}", m.provider(), m.content());
    }
    Message::ToolUse(call) => {
        println!("Tool call: {} with {:?}", call.name, call.arguments);
    }
    Message::ToolResult(result) => {
        println!("Tool result: {} (error={})", result.content, result.is_error);
    }
}
```

**Serde Behavior:**

- `AssistantMessage` model info is flattened via `#[serde(flatten)]`
- Each variant serializes with its role and content

### CacheableMessage

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

## Terminal Sanitization

The `sanitize` module provides security-critical text sanitization for terminal display.

### Security Rationale

Terminal emulators interpret escape sequences that can:

- **Manipulate clipboard** (OSC 52)
- **Create deceptive hyperlinks** (OSC 8)
- **Rewrite displayed content** (CSI cursor movement)
- **Alter terminal state/configuration**

All text from untrusted sources (LLM output, network errors, persisted history) **MUST** be sanitized before display.

### sanitize_terminal_text

Sanitizes text for safe terminal display.

**Strips:**

- ANSI escape sequences (CSI, OSC, DCS, PM, APC)
- C0 control characters (`0x00`-`0x1F`) except `\n`, `\t`, `\r`
- C1 control characters (`0x80`-`0x9F`)
- DEL character (`0x7F`)
- Unicode bidirectional controls (Trojan Source prevention): LRM, RLM, LRE, RLE, PDF, LRO, RLO, LRI, RLI, FSI, PDI, Arabic Letter Mark

**Preserves:**

- All printable ASCII and UTF-8 characters
- Newlines, tabs, and carriage returns

**Performance:**
Returns `Cow::Borrowed` when no sanitization is needed (common case), avoiding allocation.

```rust
use forge_types::sanitize_terminal_text;
use std::borrow::Cow;

// Clean text passes through without allocation
let clean = "Hello, world!";
match sanitize_terminal_text(clean) {
    Cow::Borrowed(s) => assert_eq!(s, clean),
    Cow::Owned(_) => panic!("unexpected allocation"),
}

// Escape sequences are stripped
assert_eq!(sanitize_terminal_text("Hello\x1b[2JWorld"), "HelloWorld");

// Color codes stripped
assert_eq!(sanitize_terminal_text("\x1b[31mRed\x1b[0m"), "Red");

// Clipboard manipulation stripped (OSC 52)
assert_eq!(sanitize_terminal_text("text\x1b]52;c;SGVsbG8=\x07more"), "textmore");

// Hyperlinks stripped (OSC 8)
assert_eq!(
    sanitize_terminal_text("\x1b]8;;http://evil.com\x1b\\Click\x1b]8;;\x1b\\"),
    "Click"
);

// Unicode preserved
assert_eq!(sanitize_terminal_text("Hello World"), "Hello World");

// Newlines/tabs preserved
assert_eq!(sanitize_terminal_text("Line1\nLine2\tTabbed"), "Line1\nLine2\tTabbed");

// Control characters stripped
assert_eq!(sanitize_terminal_text("A\x00B\x01C"), "ABC");
```

---

## Type Relationships

```text
                    NonEmptyString <------- NonEmptyStaticStr
                          |                     (compile-time)
                          v
    +---------------------+----------------------+
    |                     |                      |
SystemMessage        UserMessage          AssistantMessage
    |                     |                      |
    +---------------------+-----+----------------+
                          |     |
                     +----+     +----+
                     |              |
                 ToolCall      ToolResult
                     |              |
                     +------+-------+
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

ToolDefinition (standalone - tool schemas)

StreamEvent -----> StreamFinishReason

OutputLimits (validated invariants)
```

---

## Error Types Summary

| Error Type | Source | Condition |
| ---------- | ------ | --------- |
| `EmptyStringError` | `NonEmptyString::new()` | Empty or whitespace-only input |
| `ModelParseError::Empty` | `ModelName::parse()` | Empty model name |
| `ModelParseError::ClaudePrefix` | `ModelName::parse()` | Claude model without `claude-` prefix |
| `ModelParseError::OpenAIMinimum` | `ModelName::parse()` | OpenAI model without `gpt-5` prefix |
| `ModelParseError::GeminiPrefix` | `ModelName::parse()` | Gemini model without `gemini-` prefix |
| `OutputLimitsError::ThinkingBudgetTooSmall` | `OutputLimits::with_thinking()` | `thinking_budget < 1024` |
| `OutputLimitsError::ThinkingBudgetTooLarge` | `OutputLimits::with_thinking()` | `thinking_budget >= max_output_tokens` |

All error types implement `std::error::Error` via `thiserror` and provide descriptive messages.

---

## Testing

Run the crate's tests:

```bash
cargo test -p forge-types
```

The test suite verifies:

- `NonEmptyString` rejects empty and whitespace-only strings
- `Provider::parse()` handles all aliases correctly  
- `ModelName::parse()` validates provider prefix requirements
- `OutputLimits::with_thinking()` enforces budget constraints
- `sanitize_terminal_text()` strips all dangerous escape sequences
- `sanitize_terminal_text()` preserves safe content without allocation

---

## Extending the Crate

### Adding a New Provider

1. Add variant to `Provider` enum
2. Implement all `match` arms:
   - `as_str()` - lowercase identifier
   - `display_name()` - human-readable name
   - `env_var()` - environment variable for API key
   - `default_model()` - create default `ModelName`
   - `available_models()` - list of known model names
   - `parse()` - add parsing aliases
   - `all()` - add to static slice
3. Add variant to `ApiKey` enum
4. Update `ModelName::parse()` if provider has prefix requirements

### Adding a New Message Type

1. Create struct with `content: NonEmptyString` and `timestamp: SystemTime`
2. Implement `new()` constructor and `content()` accessor
3. Add variant to `Message` enum
4. Update `Message::role_str()` and `Message::content()` match arms
5. Add convenience constructor if needed

### Adding Configuration Types

Follow the pattern of `OpenAIReasoningEffort`:

1. Define enum with `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]`
2. Implement `parse(&str) -> Option<Self>` for user input
3. Implement `as_str(self) -> &'static str` for API serialization
4. Mark default variant with `#[default]`
