# IFA Plan: types/ crate optionality removal

## Goals
- Eliminate optionality in core domain types within `types/` per INVARIANT_FIRST_ARCHITECTURE (IFA) Section 10.2 and 12.9.
- Replace `Option` fields/returns and `None` enum variants with explicit state types.
- Move validation to boundary-facing constructors and parsing APIs with explicit error types.
- Preserve external serialization compatibility where feasible via dedicated serde adapters.

## Scope
- Primary: `types/src/lib.rs`.
- Secondary (expected downstream fixes, not in-scope for this doc): usage updates in `engine/`, `context/`, `providers/`, `tui/`, `cli/`.
- Non-goals: behavioral changes outside `types/` unless required by type fixes.

## IFA Principles Applied
- No optional fields in core interfaces (no `Option<T>` for core domain state).
- No `None` enum variant representing absence.
- Distinct valid states are distinct types or enum variants with semantic names (not `None`).
- Boundary parse/conversion returns explicit errors (Result-based), not `Option`.

## Inventory of Optionality in types/

### 1) Parsing APIs returning Option (boundary only today, but used widely)
- `Provider::parse(&str) -> Option<Provider>` in `types/src/lib.rs:253`.
- `Provider::from_model_name(&str) -> Option<Provider>` in `types/src/lib.rs:264`.
- `PredefinedModel::from_model_id(&str) -> Option<PredefinedModel>` in `types/src/lib.rs:371`.
- `PredefinedModel::from_provider_and_id(Provider, &str) -> Option<PredefinedModel>` in `types/src/lib.rs:383`.
- `OpenAIReasoningEffort::parse`, `OpenAIReasoningSummary::parse`, `OpenAITextVerbosity::parse`, `OpenAITruncation::parse` in `types/src/lib.rs:560`, `594`, `625`, `653`.

### 2) Option fields in core structs
- `OutputLimits { thinking_budget: Option<u32> }` in `types/src/lib.rs:752`.
- `StreamEvent::ToolCallStart { thought_signature: Option<String> }` in `types/src/lib.rs:821`.
- `ToolCall { thought_signature: Option<String> }` in `types/src/lib.rs:928`.
- `ThinkingMessage { signature: Option<String> }` in `types/src/lib.rs:1089`.

### 3) None enum variants representing absence
- `OpenAIReasoningEffort::None` in `types/src/lib.rs:550`.
- `OpenAIReasoningSummary::None` in `types/src/lib.rs:586`.
- `CacheHint::None` in `types/src/lib.rs:735`.

## Target Type Redesign (Proposed)

### A) Parse APIs: Option -> Result with explicit errors
Introduce a shared `EnumParseError` and replace `Option` with `Result<_, EnumParseError>`:
- `EnumParseError { kind: EnumKind, raw: String, expected: &'static [&'static str] }`.
- `EnumKind` enumerates each parse target (Provider, PredefinedModel, ReasoningEffort, ReasoningSummary, TextVerbosity, Truncation).

Notes:
- Hard break: remove `Option`-returning parsers instead of deprecating them.
- All boundary parsing must flow through `Result` and handle errors explicitly.

### B) Remove `None` enum variants with explicit semantic names
- `OpenAIReasoningEffort::None` -> `OpenAIReasoningEffort::Disabled`.
- `OpenAIReasoningSummary::None` -> `OpenAIReasoningSummary::Disabled`.
- `CacheHint::None` -> `CacheHint::Default` (or `ProviderDefault`).

Behavior:
- `as_str()` should still emit the provider-required string (`"none"`) for Disabled variants.
- Parse should accept `"none"` and map to `Disabled`.

### C) OutputLimits: remove optional thinking budget
Replace the optional field with distinct, fully valid states:
```
enum OutputLimits {
  Standard { max_output_tokens: u32 },
  WithThinking { max_output_tokens: u32, thinking_budget: ThinkingBudget },
}

struct ThinkingBudget(u32); // validated: >= 1024 and < max_output_tokens
```
- `ThinkingBudget` constructed only via a validator to make the invariant unrepresentable.
- `OutputLimits::new(max)` returns `Standard`.
- `OutputLimits::with_thinking(max, budget) -> Result<OutputLimits, OutputLimitsError>` returns `WithThinking`.
- Replace `thinking_budget() -> Option<u32>` with `thinking()` returning a typed view:
  - `fn thinking(&self) -> ThinkingState` where `ThinkingState::Disabled | Enabled(ThinkingBudget)`.

### D) Tool call signatures: remove optional signature fields
Represent signature presence as distinct types rather than `Option`:
```
struct UnsignedToolCall { id, name, arguments }
struct SignedToolCall { id, name, arguments, thought_signature: ThoughtSignature }

enum ToolCall { Unsigned(UnsignedToolCall), Signed(SignedToolCall) }

struct ThoughtSignature(NonEmptyString); // or validated String
```
- Same approach for `StreamEvent::ToolCallStart`:
```
enum ToolCallStart { Unsigned { id, name }, Signed { id, name, thought_signature } }
```
- For `ThinkingMessage`:
```
enum ThinkingMessage { Unsigned { content, timestamp, model }, Signed { content, timestamp, model, signature } }
```

Serialization compatibility:
- Introduce serde shim structs (eg `ToolCallSerde`) that deserialize old JSON with missing `thought_signature` into `ToolCall::Unsigned`.
- For serialization, always emit schema with a clear variant (eg tagged enum) or mimic old schema while guaranteeing valid state internally.

### E) Keep provider-specific domain facts explicit
- Ensure string values that are truly meaningful (eg `"none"` as API value) are still represented, just not via `None`.
- Maintain conversion helpers to translate to provider-specific payload shapes.

## Migration Steps (Detailed)

1) **Add parse error types and Result-returning parsers**
   - Implement `EnumParseError` + `EnumKind` in `types/src/lib.rs`.
   - Replace all `Option`-returning parse functions with `Result` versions.
   - Update docs to treat `Result` parsers as the only boundary API (no legacy wrappers).

2) **Rename None enum variants**
   - Update enum definitions and all call sites.
   - Maintain `as_str()` mapping to provider values (`"none"`).
   - Adjust tests that assert `None` variant names.

3) **Refactor OutputLimits**
   - Introduce `ThinkingBudget` newtype with validation.
   - Replace `Option`-based fields and methods with explicit enum states.
   - Update constructors and uses (downstream changes required).

4) **Refactor ToolCall, StreamEvent, ThinkingMessage**
   - Add new signed/unsigned structs and enums.
   - Provide conversion helpers for legacy builder APIs (`ToolCall::new`, `ToolCall::new_with_thought_signature`).
   - Add serde adapters to preserve on-disk or wire compatibility.
   - Update any code that previously expected `Option` signatures.

5) **Update downstream crates (expected follow-up)**
   - `engine/` and `providers/` for parse and ToolCall/Thinking usage.
   - `context/` for distillation serialization and stream events.
   - `tui/` for any display assumptions on signature options.

6) **Tests and docs**
   - Update tests in `types/` that reference `None` variants or `Option` returns.
   - Add new tests for serde shims to ensure backward compatibility.
   - Update any docs referencing `None` variants or optional fields.

## Compatibility Strategy
- No backwards compatibility guarantees for `types/` API changes in this release.
- Serialization format may change; downstream crates must migrate in lockstep.
- Remove all legacy helpers that preserve optionality.

## Risks / Considerations
- Public API churn: `types/` is shared; downstream changes are mandatory.
- Serialization format changes: consider tagged enums or legacy-compatible fields.
- Provider requirements: ensure renamed enum variants still emit correct API strings.

## Test Plan (for implementation phase)
- `cargo test -p forge-types`.
- `cargo test --workspace`.
- Focused tests: ToolCall/ThinkingMessage serde round-trips, OutputLimits invariants, parse errors.

## Decisions
- No backwards compatibility required for `types/` API or serialization changes.
- Centralize all parse errors into `EnumParseError`.
- Hard break: remove `Option`-returning parse APIs with no deprecation window.
