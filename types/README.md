# forge-types

Core domain types for Forge with **no IO, no async, and minimal dependencies**.

This crate provides the foundational type system that enforces correctness at compile time. Every type can be safely used from any layer of the application without pulling in runtime complexity.

## Table of Contents

- [Design Philosophy](#design-philosophy)
- [Module Structure](#module-structure)
- [NonEmpty String Types](#nonempty-string-types)
- [PersistableContent](#persistablecontent)
- [Provider and Model Types](#provider-and-model-types)
- [API Key Types](#api-key-types)
- [OpenAI Request Options](#openai-request-options)
- [Caching and Output Limits](#caching-and-output-limits)
- [Thought Signatures](#thought-signatures)
- [Streaming Events](#streaming-events)
- [API Usage Tracking](#api-usage-tracking)
- [Tool Calling Types](#tool-calling-types)
- [Message Types](#message-types)
- [Terminal Sanitization](#terminal-sanitization)
- [Steganographic Sanitization](#steganographic-sanitization)
- [Homoglyph Detection](#homoglyph-detection)
- [Text Utilities](#text-utilities)
- [Type Relationships](#type-relationships)
- [Error Types Summary](#error-types-summary)
- [Testing](#testing)
- [Extending the Crate](#extending-the-crate)

---

## Design Philosophy

This crate follows **type-driven design** principles where invalid states are unrepresentable:

### 1. Invariants at Construction

Types validate their constraints when created, not when used. Once you have a value of a type, you know it satisfies all required invariants.

```rust
// NonEmptyString: Cannot be empty - fails at construction
let s = NonEmptyString::new("")?;  // Err(EmptyStringError)

// PersistableContent: Standalone \r normalized at construction
let safe = PersistableContent::new("attack\roverwrite");
assert_eq!(safe.as_str(), "attack\noverwrite");

// OutputLimits: thinking_budget constraints enforced at creation
let limits = OutputLimits::with_thinking(4096, 5000)?;  // Err: budget >= max

// ModelName: Provider prefix validated during parsing
let model = Provider::OpenAI.parse_model("gpt-5.2")?;  // Ok
let model = Provider::Gemini.parse_model("gemini-3-pro-preview")?;  // Ok
```

### 2. Provider Scoping

Types that belong to a provider carry that association, preventing cross-provider mixing at compile time.

```rust
// ModelName is bound to its provider
let model = Provider::Claude.parse_model("claude-opus-4-6")?;
assert_eq!(model.provider(), Provider::Claude);

// ApiKey variants are provider-specific
let key = ApiKey::claude("sk-ant-...");
assert_eq!(key.provider(), Provider::Claude);
```

### 3. True Sum Types

`Message` is a proper enum where each variant contains role-specific data, rather than a role tag with optional fields that may or may not be meaningful.

```rust
pub enum Message {
    System(SystemMessage),        // content, timestamp
    User(UserMessage),            // content, timestamp
    Assistant(AssistantMessage),   // content, timestamp, model
    Thinking(ThinkingMessage),     // content, signature, timestamp, model
    ToolUse(ToolCall),            // id, name, arguments, thought_signature
    ToolResult(ToolResult),       // tool_call_id, content, is_error
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
- `strip_steganographic_chars` returns `Cow::Borrowed` when no changes needed
- `PersistableContent::new` skips allocation when no normalization is needed

### 6. Opaque Secret Handling

`SecretString` and `ApiKey` prevent accidental credential disclosure:

- No `Display` impl on `SecretString` (compile error on `format!("{}", secret)`)
- `Debug` is redacted for both types
- The only way to access the raw value is via `expose_secret()`

---

## Module Structure

```text
forge-types/
├── Cargo.toml
└── src/
    ├── lib.rs           # All public types and core implementations
    ├── sanitize.rs      # Terminal text sanitization + steganographic stripping (security)
    ├── text.rs          # Pure text helpers (truncation)
    └── confusables.rs   # Homoglyph / mixed-script detection (security)
```

**Dependencies** (minimal by design):

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
unicode-script = "..."   # For mixed-script / homoglyph detection
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

**Prefixed Construction:**

```rust
use forge_types::{NonEmptyString, NonEmptyStaticStr};

// Build a non-empty string by prefixing with a known non-empty static string
const PREFIX: NonEmptyStaticStr = NonEmptyStaticStr::new("Error");
let content = NonEmptyString::new("something went wrong")?;
let message = NonEmptyString::prefixed(PREFIX, ": ", &content);
assert_eq!(message.as_str(), "Error: something went wrong");
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

## PersistableContent

A string type that normalizes standalone `\r` characters to `\n` at construction time, preventing log spoofing attacks where carriage returns overwrite preceding content in terminal display.

**Invariants:**

- No standalone `\r` exists (only `\r\n` pairs are permitted)
- Normalization: standalone `\r` becomes `\n`; `\r\n` (Windows line endings) is preserved

**Security Rationale:**

```text
Attack: "File saved\rERROR: Permission denied"
Display: "ERROR: Permission denied" (overwrites "File saved")
```

By normalizing at construction, this attack vector is eliminated in all persisted content (history, journals, logs).

**Construction:**

```rust
use forge_types::PersistableContent;

// Standalone \r normalized to \n
let safe = PersistableContent::new("File saved\rERROR: Permission denied");
assert_eq!(safe.as_str(), "File saved\nERROR: Permission denied");

// Windows line endings preserved
let safe = PersistableContent::new("Line 1\r\nLine 2");
assert_eq!(safe.as_str(), "Line 1\r\nLine 2");

// Unix line endings preserved
let safe = PersistableContent::new("Line 1\nLine 2");
assert_eq!(safe.as_str(), "Line 1\nLine 2");

// Empty strings are valid
let safe = PersistableContent::new("");
assert!(safe.is_empty());
```

**Access:**

```rust
// Explicit access
let content: &str = safe.as_str();

// Via Deref (seamless string operations)
assert!(safe.starts_with("File"));

// Via AsRef<str>
fn accepts_str(s: impl AsRef<str>) { }
accepts_str(&safe);

// Length and emptiness
assert!(!safe.is_empty());
let len = safe.len();

// Extract inner String
let raw: String = safe.into_inner();
let raw: String = safe.into();  // via From trait
```

**Performance:**

Uses a fast-path check: if no standalone `\r` is found, no allocation is performed. Only strings containing the attack vector allocate.

**Serde Behavior:**

- Uses `#[serde(transparent)]` -- serializes/deserializes as a plain JSON string
- Deserialization re-normalizes (the `new()` constructor runs on the deserialized string)

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
assert_eq!(Provider::parse("claude").unwrap(), Provider::Claude);
assert_eq!(Provider::parse("Anthropic").unwrap(), Provider::Claude);
assert_eq!(Provider::parse("openai").unwrap(), Provider::OpenAI);
assert_eq!(Provider::parse("gpt").unwrap(), Provider::OpenAI);
assert_eq!(Provider::parse("chatgpt").unwrap(), Provider::OpenAI);
assert_eq!(Provider::parse("gemini").unwrap(), Provider::Gemini);
assert_eq!(Provider::parse("google").unwrap(), Provider::Gemini);
assert!(Provider::parse("unknown").is_err());

// Infer provider from model name prefix
assert_eq!(
    Provider::from_model_name("claude-opus-4-6").unwrap(),
    Provider::Claude
);
assert_eq!(Provider::from_model_name("gpt-5.2").unwrap(), Provider::OpenAI);
assert_eq!(
    Provider::from_model_name("gemini-3-pro-preview").unwrap(),
    Provider::Gemini
);
assert!(Provider::from_model_name("unknown-model").is_err());

// Enumerate all providers
for provider in Provider::all() {
    println!("{}: {}", provider.as_str(), provider.display_name());
}
```

**Model Operations:**

```rust
// Get default model for provider
let model = Provider::Claude.default_model();
assert_eq!(model.as_str(), "claude-opus-4-6");

let model = Provider::Gemini.default_model();
assert_eq!(model.as_str(), "gemini-3-pro-preview");

// List available models
let models = Provider::Claude.available_models();
let model_ids: Vec<&'static str> = models.iter().map(|model| model.model_id()).collect();
// ["claude-opus-4-6", "claude-haiku-4-5-20251001"]

let models = Provider::OpenAI.available_models();
let model_ids: Vec<&'static str> = models.iter().map(|model| model.model_id()).collect();
// ["gpt-5.2-pro", "gpt-5.2"]

let models = Provider::Gemini.available_models();
let model_ids: Vec<&'static str> = models.iter().map(|model| model.model_id()).collect();
// ["gemini-3-pro-preview", "gemini-3-flash-preview"]

// Parse model name with validation
let model = Provider::Claude.parse_model("claude-opus-4-6")?;
```

### PredefinedModel

Enumeration of known models with associated metadata.

**Variants:**

| Variant | Model ID | Display Name | Model Name | Firm |
| ------- | -------- | ------------ | ---------- | ---- |
| `ClaudeOpus` | `claude-opus-4-6` | Anthropic Claude Opus 4.6 | Opus 4.6 | Anthropic |
| `ClaudeHaiku` | `claude-haiku-4-5-20251001` | Anthropic Claude Haiku 4.5 | Haiku 4.5 | Anthropic |
| `Gpt52Pro` | `gpt-5.2-pro` | OpenAI GPT 5.2 Pro | GPT 5.2 Pro | OpenAI |
| `Gpt52` | `gpt-5.2` | OpenAI GPT 5.2 | GPT 5.2 | OpenAI |
| `GeminiPro` | `gemini-3-pro-preview` | Google Gemini 3 Pro | Gemini 3 Pro | Google |
| `GeminiFlash` | `gemini-3-flash-preview` | Google Gemini 3 Flash | Gemini 3 Flash | Google |

**Methods:**

```rust
use forge_types::PredefinedModel;

let model = PredefinedModel::ClaudeOpus;

// Metadata accessors (all const)
assert_eq!(model.model_id(), "claude-opus-4-6");
assert_eq!(model.display_name(), "Anthropic Claude Opus 4.6");
assert_eq!(model.model_name(), "Opus 4.6");
assert_eq!(model.firm_name(), "Anthropic");
assert_eq!(model.provider(), Provider::Claude);

// Convert to ModelName
let model_name = model.to_model_name();

// Parse from model ID string (case-insensitive)
let parsed = PredefinedModel::from_model_id("gpt-5.2")?;
assert_eq!(parsed, PredefinedModel::Gpt52);

// Parse scoped to a specific provider
let parsed = PredefinedModel::from_provider_and_id(Provider::Gemini, "gemini-3-pro-preview")?;
assert_eq!(parsed, PredefinedModel::GeminiPro);

// List all predefined models
for m in PredefinedModel::all() {
    println!("{}: {} ({})", m.model_id(), m.display_name(), m.firm_name());
}
```

### ModelName

A provider-scoped model name that prevents mixing models across providers.

**Invariants:**

- Always associated with a specific `Provider`
- Claude models must start with `claude-`
- OpenAI models must start with `gpt-5`
- Gemini models must start with `gemini-`
- Model names must exist in the predefined catalog
- Empty model names are rejected

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `provider` | `Provider` | The provider this model belongs to |
| `name` | `Cow<'static, str>` | The model name string |

**Construction:**

```rust
use forge_types::{Provider, ModelName, ModelParseError, PredefinedModel};

// Parse from user input (validates prefix, checks known list)
let model = Provider::Claude.parse_model("claude-opus-4-6")?;
assert_eq!(model.provider(), Provider::Claude);
assert_eq!(model.as_str(), "claude-opus-4-6");
assert_eq!(model.predefined(), PredefinedModel::ClaudeOpus);

// Unknown models are rejected
let model = Provider::Claude.parse_model("claude-future-model");
assert!(matches!(model, Err(ModelParseError::UnknownModel(_))));

// Validation errors
let result = Provider::OpenAI.parse_model("gpt-4o");
assert!(matches!(result, Err(ModelParseError::OpenAIMinimum(_))));

let result = Provider::Claude.parse_model("gpt-5.2");
assert!(matches!(result, Err(ModelParseError::ClaudePrefix(_))));

let result = Provider::OpenAI.parse_model("");
assert!(matches!(result, Err(ModelParseError::Empty)));

// Create known model directly (for internal/const use)
const OPUS: ModelName = ModelName::from_predefined(PredefinedModel::ClaudeOpus);
```

**Memory Optimization:**

Model names always use `Cow::Borrowed` to avoid allocation:

```rust
// Known model - no heap allocation
let known = Provider::Claude.default_model();  // Cow::Borrowed
```

### EnumParseError

Structured error returned when parsing provider names, model IDs, or OpenAI option strings from user input.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `kind` | `EnumKind` | Which enum was being parsed |
| `raw` | `String` | The invalid input value |
| `expected` | `&'static [&'static str]` | Valid values |

**EnumKind Variants:**

| Variant | Description |
| ------- | ----------- |
| `Provider` | Provider name parsing |
| `PredefinedModel` | Model ID parsing |
| `OpenAIReasoningEffort` | Reasoning effort parsing |
| `OpenAIReasoningSummary` | Reasoning summary parsing |
| `OpenAITextVerbosity` | Text verbosity parsing |
| `OpenAITruncation` | Truncation parsing |

**Error message format:** `"invalid {kind} value '{raw}'; expected one of: {expected:?}"`

### ModelParseError

Errors that occur when parsing a model name via `ModelName::parse()`.

| Variant | Condition | Message |
| ------- | --------- | ------- |
| `Empty` | Model name is empty or whitespace-only | "model name cannot be empty" |
| `ClaudePrefix(String)` | Claude model missing `claude-` prefix | "Claude model must start with claude-" |
| `OpenAIMinimum(String)` | OpenAI model missing `gpt-5` prefix | "OpenAI model must start with gpt-5" |
| `GeminiPrefix(String)` | Gemini model missing `gemini-` prefix | "Gemini model must start with gemini-" |
| `UnknownModel(String)` | Model name not in the predefined catalog | "unknown model name" |

---

## API Key Types

### SecretString

An opaque wrapper for secret strings that prevents accidental disclosure in logs and error messages.

**Security Properties:**

- No `Display` impl -- `format!("{}", secret)` is a compile error
- `Debug` output is redacted: `SecretString(<redacted>)`
- The only way to access the raw value is via `expose_secret()`
- Every access point is explicitly visible and greppable

```rust
use forge_types::SecretString;

let secret = SecretString::new("sk-ant-api03-secret".to_string());

// Access the raw value (deliberately named for auditability)
assert_eq!(secret.expose_secret(), "sk-ant-api03-secret");

// Debug is redacted
assert_eq!(format!("{:?}", secret), "SecretString(<redacted>)");
```

### ApiKey

A provider-scoped API key wrapping `SecretString` to prevent using a key with the wrong provider and prevent accidental credential disclosure.

**Variants:**

```rust
pub enum ApiKey {
    Claude(SecretString),
    OpenAI(SecretString),
    Gemini(SecretString),
}
```

**Invariant:** The key string is always associated with its correct provider.

**Security:** The `Debug` implementation redacts the key value to prevent accidental credential disclosure in logs or error messages:

```rust
let key = ApiKey::claude("sk-ant-api03-secret");
// Debug output: ApiKey::Claude(<redacted>)
```

**Construction and Usage:**

```rust
use forge_types::{ApiKey, Provider};

// Create provider-specific keys via opaque constructors
let claude_key = ApiKey::claude("sk-ant-api03-...");
let openai_key = ApiKey::openai("sk-proj-...");
let gemini_key = ApiKey::gemini("AIza...");

// Access provider
assert_eq!(claude_key.provider(), Provider::Claude);
assert_eq!(openai_key.provider(), Provider::OpenAI);
assert_eq!(gemini_key.provider(), Provider::Gemini);

// Access key string (deliberately named for auditability)
assert_eq!(claude_key.expose_secret(), "sk-ant-api03-...");
```

**Design Rationale:**

By making `ApiKey` a sum type rather than a struct with a provider field, the compiler ensures you cannot accidentally pass a Claude key to OpenAI client code. The key and provider are inseparable. Wrapping in `SecretString` further prevents any accidental disclosure path.

---

## OpenAI Request Options

Configuration types for OpenAI-specific request parameters.

### OpenAIReasoningEffort

Controls how much reasoning the model should perform before responding.

| Variant | API Value | Description |
| ------- | --------- | ----------- |
| `Disabled` | "none" | No reasoning |
| `Low` | "low" | Minimal reasoning |
| `Medium` | "medium" | Moderate reasoning |
| `High` (default) | "high" | Full reasoning |
| `XHigh` | "xhigh" | Extended reasoning |

```rust
use forge_types::OpenAIReasoningEffort;

// Parse from string (case-insensitive)
assert_eq!(OpenAIReasoningEffort::parse("high").unwrap(), OpenAIReasoningEffort::High);
assert_eq!(OpenAIReasoningEffort::parse("xhigh").unwrap(), OpenAIReasoningEffort::XHigh);
assert_eq!(OpenAIReasoningEffort::parse("x-high").unwrap(), OpenAIReasoningEffort::XHigh);
assert!(OpenAIReasoningEffort::parse("invalid").is_err());

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
| `Disabled` (default) | "none" | Do not request a reasoning summary |
| `Auto` | "auto" | Request the most detailed summary available |
| `Concise` | "concise" | Request a concise summary |
| `Detailed` | "detailed" | Request a detailed summary |

```rust
use forge_types::OpenAIReasoningSummary;

assert_eq!(OpenAIReasoningSummary::parse("auto").unwrap(), OpenAIReasoningSummary::Auto);
assert_eq!(OpenAIReasoningSummary::parse("CONCISE").unwrap(), OpenAIReasoningSummary::Concise);
assert_eq!(OpenAIReasoningSummary::parse("detailed").unwrap(), OpenAIReasoningSummary::Detailed);
assert!(OpenAIReasoningSummary::parse("invalid").is_err());

assert_eq!(OpenAIReasoningSummary::Auto.as_str(), "auto");
assert_eq!(OpenAIReasoningSummary::default(), OpenAIReasoningSummary::Disabled);
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
assert_eq!(default_opts.reasoning_summary(), OpenAIReasoningSummary::Disabled);
assert_eq!(default_opts.verbosity(), OpenAITextVerbosity::High);
assert_eq!(default_opts.truncation(), OpenAITruncation::Auto);
```

---

## Caching and Output Limits

### CacheHint

A hint for whether content should be cached by the provider.

| Variant | Description |
| ------- | ----------- |
| `Default` (default) | No caching preference |
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
assert_eq!(default, CacheHint::Default);

let cached = CacheHint::Ephemeral;
```

### OutputLimits

Validated output configuration that guarantees invariants by construction.

**Invariants:**

- If thinking is enabled: `thinking_budget >= 1024`
- If thinking is enabled: `thinking_budget < max_output_tokens`

Use `ThinkingState` to inspect whether thinking is enabled, and `ThinkingBudget::as_u32()` to read the validated token budget.

**Without Thinking:**

```rust
use forge_types::{OutputLimits, ThinkingState};

// Always succeeds (no validation needed)
let limits = OutputLimits::new(4096);
assert_eq!(limits.max_output_tokens(), 4096);
assert_eq!(limits.thinking(), ThinkingState::Disabled);
assert!(!limits.has_thinking());
```

**With Thinking:**

```rust
use forge_types::{OutputLimits, OutputLimitsError, ThinkingState};

// Valid configuration
let limits = OutputLimits::with_thinking(16384, 8192)?;
assert_eq!(limits.max_output_tokens(), 16384);
assert!(matches!(limits.thinking(), ThinkingState::Enabled(_)));
assert!(limits.has_thinking());

// Minimum budget (1024)
let _limits = OutputLimits::with_thinking(4096, 1024)?;

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

## Thought Signatures

Types for tracking provider-specific thinking/reasoning signatures required for API replay.

### ThoughtSignature

An opaque wrapper for the encrypted thinking signature string that some providers (e.g., Claude) require when replaying conversations with thinking enabled.

```rust
use forge_types::ThoughtSignature;

let sig = ThoughtSignature::new("encrypted-signature-data");
assert_eq!(sig.as_str(), "encrypted-signature-data");

// Incremental building during streaming
let mut sig = ThoughtSignature::new("");
sig.push_str("partial");
sig.push_str("-data");
assert_eq!(sig.as_str(), "partial-data");

// From conversions
let sig: ThoughtSignature = "sig-abc".into();
let sig: ThoughtSignature = String::from("sig-abc").into();
```

### ThoughtSignatureState

Tracks whether a thinking block or tool call has a provider signature attached.

| Variant | Description |
| ------- | ----------- |
| `Unsigned` | No signature present |
| `Signed(ThoughtSignature)` | Signature attached for API replay |

```rust
use forge_types::{ThoughtSignatureState, ThoughtSignature};

let unsigned = ThoughtSignatureState::Unsigned;
assert!(!unsigned.is_signed());

let signed = ThoughtSignatureState::Signed(ThoughtSignature::new("sig"));
assert!(signed.is_signed());
```

**Serde:** Serializes with `#[serde(tag = "state", content = "signature", rename_all = "snake_case")]`.

---

## Streaming Events

### StreamEvent

Events emitted during streaming API responses.

| Variant | Description |
| ------- | ----------- |
| `TextDelta(String)` | Incremental text content |
| `ThinkingDelta(String)` | Provider reasoning content (Claude extended thinking or OpenAI reasoning summaries) |
| `ThinkingSignature(String)` | Encrypted thinking signature for API replay (Claude extended thinking) |
| `ToolCallStart { id, name, thought_signature }` | Tool use content block began; `thought_signature` is `ThoughtSignatureState` |
| `ToolCallDelta { id, arguments }` | Tool call JSON arguments chunk |
| `Usage(ApiUsage)` | API-reported token usage from provider |
| `Done` | Stream completed successfully |
| `Error(String)` | Error occurred during streaming |

```rust
use forge_types::{StreamEvent, ApiUsage, ThoughtSignatureState};

fn handle_event(event: StreamEvent, response: &mut String, thinking: &mut String) {
    match event {
        StreamEvent::TextDelta(text) => {
            response.push_str(&text);
        }
        StreamEvent::ThinkingDelta(thought) => {
            thinking.push_str(&thought);
        }
        StreamEvent::ThinkingSignature(sig) => {
            // Store signature for API replay
            println!("Thinking signature received: {} bytes", sig.len());
        }
        StreamEvent::ToolCallStart {
            id,
            name,
            thought_signature,
        } => {
            println!("Tool call started: {} ({})", name, id);
            if thought_signature.is_signed() {
                println!("  (has thought signature)");
            }
        }
        StreamEvent::ToolCallDelta { id, arguments } => {
            println!("Tool {} args: {}", id, arguments);
        }
        StreamEvent::Usage(usage) => {
            println!("Tokens: {} in, {} out", usage.input_tokens, usage.output_tokens);
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

## API Usage Tracking

### ApiUsage

Captures actual token counts from provider API responses for accurate cost tracking and cache hit analysis.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `input_tokens` | `u32` | Total input tokens (includes cached tokens) |
| `cache_read_tokens` | `u32` | Input tokens read from cache (cache hits) |
| `cache_creation_tokens` | `u32` | Input tokens written to cache (cache misses that were cached) |
| `output_tokens` | `u32` | Output tokens generated by the model |

**Construction:**

```rust
use forge_types::ApiUsage;

// Default is all zeros
let usage = ApiUsage::default();
assert_eq!(usage.input_tokens, 0);
assert!(!usage.has_data());

// Direct field access (public fields)
let usage = ApiUsage {
    input_tokens: 1000,
    cache_read_tokens: 800,
    cache_creation_tokens: 50,
    output_tokens: 500,
};
```

**Computed Properties:**

```rust
use forge_types::ApiUsage;

let usage = ApiUsage {
    input_tokens: 1000,
    cache_read_tokens: 800,
    cache_creation_tokens: 0,
    output_tokens: 500,
};

// Non-cached input tokens (for cost calculation)
// cost = (non_cached * input_price) + (cache_read * cached_price) + (output * output_price)
assert_eq!(usage.non_cached_input_tokens(), 200);

// Check if usage has any data
assert!(usage.has_data());

// Cache hit percentage (0-100)
let hit_rate = usage.cache_hit_percentage();
assert!((hit_rate - 80.0).abs() < 0.01);  // 80% cache hit rate
```

**Aggregation:**

```rust
use forge_types::ApiUsage;

let mut total = ApiUsage {
    input_tokens: 1000,
    cache_read_tokens: 800,
    cache_creation_tokens: 100,
    output_tokens: 500,
};

let call2 = ApiUsage {
    input_tokens: 2000,
    cache_read_tokens: 1500,
    cache_creation_tokens: 200,
    output_tokens: 1000,
};

// Merge another usage into this one (saturating arithmetic)
total.merge(&call2);

assert_eq!(total.input_tokens, 3000);
assert_eq!(total.cache_read_tokens, 2300);
assert_eq!(total.cache_creation_tokens, 300);
assert_eq!(total.output_tokens, 1500);
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
| `hidden` | `bool` | Whether this tool is hidden from UI rendering (default `false`). Hidden tools execute normally but are invisible to the user. |
| `provider` | `Option<Provider>` | If set, this tool is only included in the tool manifest for the specified provider (default `None`). |

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
assert!(!tool.hidden);           // default: visible
assert!(tool.provider.is_none()); // default: all providers
```

### ToolCall

A tool call requested by the LLM during a response.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `id` | `String` | Unique identifier (for matching with results) |
| `name` | `String` | The tool being called |
| `arguments` | `serde_json::Value` | Parsed JSON arguments |
| `thought_signature` | `ThoughtSignatureState` | Thought signature state for providers that require it (e.g., Gemini) |

```rust
use forge_types::{ToolCall, ThoughtSignature, ThoughtSignatureState};
use serde_json::json;

// Basic construction (unsigned)
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
assert!(!call.signature_state().is_signed());

// With thought signature (for providers that require it)
let call = ToolCall::new_signed(
    "call_xyz789",
    "search",
    json!({"query": "rust programming"}),
    ThoughtSignature::new("sig_abc"),
);

assert!(call.signature_state().is_signed());
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
assert_eq!(msg.model().as_str(), "claude-opus-4-6");
```

**Fields:** `content: NonEmptyString`, `timestamp: SystemTime`, `model: ModelName`

### ThinkingMessage

Provider reasoning/thinking content (Claude extended thinking, Gemini thinking, etc.).

This is separate from `AssistantMessage` because thinking is metadata about the reasoning process, not part of the actual response. It can be shown/hidden independently in the UI.

```rust
use forge_types::{ThinkingMessage, ModelName, Provider, NonEmptyString};

let model = Provider::Claude.default_model();
let content = NonEmptyString::new("Let me think about this...")?;

// Without signature
let msg = ThinkingMessage::new(model.clone(), content.clone());
assert_eq!(msg.content(), "Let me think about this...");
assert!(!msg.has_signature());
assert_eq!(msg.provider(), Provider::Claude);
assert_eq!(msg.model().as_str(), "claude-opus-4-6");

// With encrypted signature (for API replay)
let msg = ThinkingMessage::with_signature(
    model,
    content,
    "encrypted-sig-data".to_string(),
);
assert!(msg.has_signature());
assert!(msg.signature_state().is_signed());
```

**Fields:** `content: NonEmptyString`, `signature: ThoughtSignatureState`, `timestamp: SystemTime`, `model: ModelName`

### Message

A sum type representing any message in a conversation.

**Variants:**

| Variant | Contains | `role_str()` |
|---------|----------|--------------|
| `System(SystemMessage)` | System prompt | "system" |
| `User(UserMessage)` | User input | "user" |
| `Assistant(AssistantMessage)` | Assistant response | "assistant" |
| `Thinking(ThinkingMessage)` | Provider reasoning content | "assistant" |
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

// Thinking messages
let thinking = Message::thinking(
    Provider::Claude.default_model(),
    NonEmptyString::new("Let me reason...")?
);
let thinking_signed = Message::thinking_with_signature(
    Provider::Claude.default_model(),
    NonEmptyString::new("Let me reason...")?,
    "encrypted-sig".to_string(),
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
assert_eq!(thinking.role_str(), "assistant");     // Thinking is assistant-role content
assert_eq!(tool_use.role_str(), "assistant");     // Tool use is from assistant
assert_eq!(tool_result.role_str(), "user");       // Tool result is sent as user role

// Content access
assert_eq!(user.content(), "Hello");

// Pattern matching for role-specific data
match &message {
    Message::System(m) => println!("System: {}", m.content()),
    Message::User(m) => println!("User: {}", m.content()),
    Message::Assistant(m) => {
        println!("Assistant ({:?}): {}", m.provider(), m.content());
    }
    Message::Thinking(m) => {
        println!("Thinking ({:?}): {}", m.provider(), m.content());
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

- `AssistantMessage` and `ThinkingMessage` model info is flattened via `#[serde(flatten)]`
- Each variant serializes with its role and content

### CacheableMessage

A message paired with a cache hint for API serialization.

```rust
use forge_types::{CacheableMessage, Message, CacheHint, NonEmptyString};

let msg = Message::system(NonEmptyString::new("You are helpful.")?);

// No caching
let plain = CacheableMessage::plain(msg.clone());
assert_eq!(plain.cache_hint, CacheHint::Default);

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

## Steganographic Sanitization

### strip_steganographic_chars

Strips invisible Unicode characters used for steganographic prompt injection from untrusted content entering the LLM context.

**Threat Model:**

Untrusted content (web pages, file contents, command output) may contain invisible Unicode payloads that encode instructions the LLM interprets but humans cannot see. The Unicode Tags block (U+E0000-U+E007F) is the sharpest vector: each codepoint maps directly to an ASCII character, enabling plaintext instruction encoding with zero visual presence.

**Stripped Categories:**

| Category | Range | Attack Vector |
|----------|-------|---------------|
| Unicode Tags | U+E0000-U+E007F | ASCII smuggling (direct mapping) |
| Zero-width chars | U+200B-U+200F, U+2060, U+FEFF | Binary steganography |
| Bidi controls | U+202A-U+202E, U+2066-U+2069, U+061C | Visual spoofing (Trojan Source) |
| Variation selectors | U+FE00-U+FE0F, U+E0100-U+E01EF | Payload encoding |
| Invisible operators | U+2061-U+2064 | Hidden semantic content |
| Interlinear annotations | U+FFF9-U+FFFB | Hidden text layers |
| Soft hyphen | U+00AD | Token-splitting attacks |
| Combining grapheme joiner | U+034F | Token manipulation |
| Hangul fillers | U+115F, U+1160, U+3164, U+FFA0 | Invisible padding |
| Mongolian vowel separator | U+180E | Format control abuse |
| Khmer inherent vowels | U+17B4, U+17B5 | Invisible carriers |

**Scope:**

Apply to untrusted content entering the LLM context:
- Web-fetched content (webfetch extraction output)
- Tool results (file reads, command output)
- NOT user direct input (would break emoji ZWJ sequences)

**Performance:**
Returns `Cow::Borrowed` when no steganographic characters are found (common case), avoiding allocation.

**Composability:**

This function handles a different threat class than `sanitize_terminal_text`:
- `sanitize_terminal_text`: terminal escape injection (display safety)
- `strip_steganographic_chars`: invisible prompt injection (LLM context safety)

For untrusted content, apply both:

```rust
use forge_types::{sanitize_terminal_text, strip_steganographic_chars};

let raw = "Hello\x1b[31m\u{200B}\u{E0041}World\x1b[0m";
let safe = strip_steganographic_chars(&sanitize_terminal_text(raw));
assert_eq!(safe, "HelloWorld");
```

**Examples:**

```rust
use forge_types::strip_steganographic_chars;
use std::borrow::Cow;

// Clean text passes through without allocation
let clean = "Hello, world!";
match strip_steganographic_chars(clean) {
    Cow::Borrowed(s) => assert_eq!(s, clean),
    Cow::Owned(_) => panic!("unexpected allocation"),
}

// Zero-width spaces stripped
assert_eq!(strip_steganographic_chars("Hello\u{200B}World"), "HelloWorld");

// Unicode Tags block stripped (ASCII smuggling vector)
let tags = "Clean\u{E0069}\u{E0067}\u{E006E}\u{E006F}\u{E0072}\u{E0065}Text";
assert_eq!(strip_steganographic_chars(tags), "CleanText");

// Soft hyphen stripped (token-splitting attack)
assert_eq!(
    strip_steganographic_chars("ig\u{00AD}nore previous instructions"),
    "ignore previous instructions"
);
```

---

## Homoglyph Detection

### detect_mixed_script

Detects mixed-script content that could indicate homoglyph attacks, where visually-similar characters from different Unicode scripts create deceptive text (e.g., Cyrillic 'a' looks like Latin 'a').

This function is a **mechanism** (reports the fact) per IFA-8. The caller (UI) makes the **policy** decision about how to display the warning.

**Detection Logic:**

Only flags Latin mixed with Cyrillic, Greek, Armenian, or Cherokee (highest attack surface for English-language tools). Pure non-Latin scripts (legitimate non-English content) are not flagged.

**Fast Path:** ASCII-only strings return `None` immediately without character iteration.

```rust
use forge_types::detect_mixed_script;

// Cyrillic 'a' (U+0430) looks like Latin 'a'
let warning = detect_mixed_script("paypal.com", "url");
assert!(warning.is_some());

// Pure Latin is fine
assert!(detect_mixed_script("paypal.com", "url").is_none());

// Pure Cyrillic is fine (legitimate Russian content)
assert!(detect_mixed_script("hello", "text").is_none());
```

### HomoglyphWarning

Proof object that homoglyph analysis was performed and detected suspicious content.

**Fields:**

| Field | Type | Description |
| ----- | ---- | ----------- |
| `field_name` | `String` | The field name where mixed scripts were detected (e.g., "url", "command") |
| `snippet` | `String` | A truncated snippet of the suspicious content for display (max 40 chars) |
| `scripts` | `Vec<Script>` | The scripts detected in the content |

```rust
use forge_types::HomoglyphWarning;

// Assuming a warning was returned by detect_mixed_script
let warning: HomoglyphWarning = /* ... */;
println!("Mixed scripts in {}: {} ({})",
    warning.field_name,
    warning.snippet,
    warning.scripts_display()  // e.g., "Latin, Cyrillic"
);
```

---

## Text Utilities

### truncate_with_ellipsis

Truncates a string to a maximum character length, adding `...` if truncation occurs.

- Trims surrounding whitespace before truncating
- Uses `char` count (not bytes) to avoid splitting Unicode scalar values
- Enforces a minimum `max` of 3 so the ellipsis always fits

```rust
use forge_types::truncate_with_ellipsis;

// Short strings pass through (after trimming)
assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
assert_eq!(truncate_with_ellipsis("  hello  ", 10), "hello");

// Exact length unchanged
assert_eq!(truncate_with_ellipsis("hello", 5), "hello");

// Long strings truncated with ellipsis
assert_eq!(truncate_with_ellipsis("hello world", 8), "hello...");

// Minimum max is 3
assert_eq!(truncate_with_ellipsis("hello", 1), "...");
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
    +---------------------+-----+--------+-------+
                          |     |        |
                     +----+     +----+   |
                     |              |    |
                 ToolCall      ToolResult |
                     |              |    |
                     +------+-------+    |
                            |            |
                            v            |
                        Message  <-------+---- ThinkingMessage
                            |
                            v
                    CacheableMessage -------> CacheHint

Provider -----> PredefinedModel
    |               |
    |               +---> ModelName (scoped)
    |
    +-----> ApiKey (scoped) -------> SecretString
    |
    +-----> OpenAIRequestOptions
                |
                +---> OpenAIReasoningEffort
                +---> OpenAIReasoningSummary
                +---> OpenAITextVerbosity
                +---> OpenAITruncation

ThoughtSignature -----> ThoughtSignatureState
                            |
                            +---> ToolCall.thought_signature
                            +---> ThinkingMessage.signature
                            +---> StreamEvent::ToolCallStart

ToolDefinition (standalone - tool schemas, optional hidden + provider)

PersistableContent (standalone - safe-persisted strings)

StreamEvent -----> StreamFinishReason
    |
    +-----> ApiUsage (token tracking)

OutputLimits -----> ThinkingBudget -----> ThinkingState

EnumParseError -----> EnumKind

HomoglyphWarning (proof object from detect_mixed_script)
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
| `ModelParseError::UnknownModel` | `ModelName::parse()` | Model name not in the predefined catalog |
| `EnumParseError` | `Provider::parse()`, `PredefinedModel::from_model_id()`, OpenAI option `parse()` methods | Invalid string for the target enum; includes `kind`, `raw`, and `expected` values |
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
- `PersistableContent` normalizes standalone `\r` to `\n` and preserves `\r\n`
- `Provider::parse()` handles all aliases correctly
- `ModelName::parse()` validates provider prefix requirements and known model list
- `PredefinedModel::from_model_id()` and `from_provider_and_id()` resolve correctly
- `OutputLimits::with_thinking()` enforces budget constraints
- `ApiKey` opaque constructors and `expose_secret()` work correctly
- `ApiUsage` arithmetic and aggregation functions
- `sanitize_terminal_text()` strips all dangerous escape sequences
- `sanitize_terminal_text()` preserves safe content without allocation
- `strip_steganographic_chars()` strips all invisible Unicode injection vectors
- `strip_steganographic_chars()` preserves normal text without allocation
- `strip_steganographic_chars()` composes correctly with `sanitize_terminal_text()`
- `detect_mixed_script()` flags Latin mixed with Cyrillic/Greek/Armenian/Cherokee
- `detect_mixed_script()` ignores pure non-Latin scripts and ASCII-only input
- `truncate_with_ellipsis()` truncates correctly and enforces minimum length

---

## Extending the Crate

### Adding a New Provider

1. Add variant to `Provider` enum
2. Implement all `match` arms:
   - `as_str()` - lowercase identifier
   - `display_name()` - human-readable name
   - `env_var()` - environment variable for API key
   - `default_model()` - create default `ModelName`
   - `available_models()` - list of known models (`PredefinedModel`)
   - `parse()` - add parsing aliases
   - `from_model_name()` - add model id detection
   - `all()` - add to static slice
3. Add variant(s) to `PredefinedModel` with canonical model IDs and implement all metadata methods (`model_id()`, `display_name()`, `model_name()`, `firm_name()`, `provider()`)
4. Add variant to `ApiKey` enum with opaque constructor (`ApiKey::new_provider(impl Into<String>)`)
5. Update `ModelName::parse()` if provider has prefix requirements
6. Add model ID constants to the relevant `*_MODEL_IDS` array and `ALL_MODEL_IDS`

### Adding a New Message Type

1. Create struct with `content: NonEmptyString` and `timestamp: SystemTime`
2. Implement `new()` constructor and `content()` accessor
3. Add variant to `Message` enum
4. Update `Message::role_str()` and `Message::content()` match arms
5. Add convenience constructor if needed

### Adding Configuration Types

Follow the pattern of `OpenAIReasoningEffort`:

1. Define enum with `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]`
2. Add variant to `EnumKind` with display string in `as_str()`
3. Add a `const` array of valid string values
4. Implement `parse(&str) -> Result<Self, EnumParseError>` for user input
5. Implement `as_str(self) -> &'static str` for API serialization
6. Mark default variant with `#[default]`
