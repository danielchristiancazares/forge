# Security Sanitization Infrastructure

This document describes Forge's defense-in-depth security sanitization infrastructure, which protects against terminal escape injection, invisible prompt injection, visual spoofing, and credential leaks.

## Table of Contents

- [Overview](#overview)
- [Terminal Sanitization](#terminal-sanitization)
- [Steganographic Sanitization](#steganographic-sanitization)
- [API Key Redaction](#api-key-redaction)
- [Integration Points](#integration-points)
- [Threat Model](#threat-model)
- [Design Decisions](#design-decisions)

## Overview

Forge processes untrusted content from multiple sources:

| Source | Risk | Sanitization Required |
|--------|------|----------------------|
| LLM output | Terminal injection | `sanitize_terminal_text` |
| Web content | Invisible prompt injection | `strip_steganographic_chars` |
| Tool output | Both terminal + prompt injection | `sanitize_output` |
| Error messages | Credential leaks | `sanitize_stream_error` |
| Recovered history | Stored injection attacks | `sanitize_terminal_text` |

All sanitization functions live in `types/src/sanitize.rs` and `engine/src/security.rs`.

## Terminal Sanitization

**Function**: `sanitize_terminal_text(input: &str) -> Cow<'_, str>`

**Location**: `types/src/sanitize.rs`

Strips characters and sequences that terminal emulators interpret as commands, preventing untrusted text from manipulating the user's terminal.

### Stripped Categories

#### ANSI Escape Sequences

| Sequence | Format | Risk |
|----------|--------|------|
| CSI (Control Sequence Introducer) | `ESC [` ... final byte | Cursor movement, screen clearing, color injection |
| OSC (Operating System Command) | `ESC ]` ... `BEL`/`ST` | Clipboard manipulation (OSC 52), hyperlink injection (OSC 8), title spoofing |
| DCS (Device Control String) | `ESC P` ... `ST` | Terminal state manipulation |
| PM (Privacy Message) | `ESC ^` ... `ST` | Terminal-specific commands |
| APC (Application Program Command) | `ESC _` ... `ST` | Application-specific commands |
| Two-character sequences | `ESC (`, `ESC )`, `ESC #`, etc. | Character set selection, line attributes |
| Single-character commands | `ESC 7`, `ESC c`, `ESC D`, etc. | Cursor save/restore, terminal reset |

#### Control Characters

| Category | Range | Examples | Risk |
|----------|-------|----------|------|
| C0 controls | `U+0000`-`U+001F` | NUL, BEL, BS, VT, FF | Terminal state corruption |
| C1 controls | `U+0080`-`U+009F` | CSI equivalent (`U+009B`), etc. | Escape sequence injection |
| DEL | `U+007F` | - | Backspace attacks |

**Preserved**: `\n` (newline), `\t` (tab), `\r` (carriage return)

#### Bidirectional Controls (Trojan Source)

| Character | Code Point | Attack |
|-----------|------------|--------|
| Arabic Letter Mark | `U+061C` | Text direction manipulation |
| Left-to-Right Mark | `U+200E` | Visual ordering spoofing |
| Right-to-Left Mark | `U+200F` | Visual ordering spoofing |
| LRE, RLE, PDF, LRO, RLO | `U+202A`-`U+202E` | Embedding/override attacks |
| LRI, RLI, FSI, PDI | `U+2066`-`U+2069` | Isolate attacks |

These characters allow text to appear different from its logical order, enabling attacks where malicious code visually appears benign.

### Usage Example

```rust
use forge_types::sanitize_terminal_text;

// Clean text passes through without allocation
let clean = "Hello, world!";
assert_eq!(sanitize_terminal_text(clean), clean);

// Escape sequences are stripped
let dirty = "Hello\x1b[2JWorld";  // CSI clear screen
assert_eq!(sanitize_terminal_text(dirty), "HelloWorld");

// OSC clipboard attack neutralized
let attack = "text\x1b]52;c;SGVsbG8=\x07more";  // OSC 52 clipboard write
assert_eq!(sanitize_terminal_text(attack), "textmore");
```

## Steganographic Sanitization

**Function**: `strip_steganographic_chars(input: &str) -> Cow<'_, str>`

**Location**: `types/src/sanitize.rs`

Strips invisible Unicode characters that survive into the LLM context window and can encode hidden instructions humans cannot see.

### Stripped Categories (by severity)

#### High Severity: Direct Prompt Injection Vectors

| Category | Range | Attack Vector |
|----------|-------|---------------|
| **Unicode Tags** | `U+E0000`-`U+E007F` | ASCII smuggling: each codepoint maps to an ASCII character (`U+E0041` = 'A'), enabling invisible plaintext instructions |
| **Zero-width chars** | `U+200B`-`U+200F` | Binary steganography: ZWSP, ZWNJ, ZWJ, LRM, RLM sequences encode hidden data |
| **Bidi embedding/override** | `U+202A`-`U+202E` | LRE, RLE, PDF, LRO, RLO for Trojan Source attacks |
| **Bidi isolates** | `U+2066`-`U+2069` | LRI, RLI, FSI, PDI for visual spoofing |
| **Word Joiner** | `U+2060` | Invisible joining control |
| **Zero Width No-Break Space** | `U+FEFF` | BOM when not at position 0 becomes steganographic |
| **Arabic Letter Mark** | `U+061C` | Bidi manipulation |

#### Medium Severity: Payload Encoding Vectors

| Category | Range | Attack Vector |
|----------|-------|---------------|
| **Variation selectors** | `U+FE00`-`U+FE0F` | VS1-VS16: glyph selection encodes data |
| **Supplementary VS** | `U+E0100`-`U+E01EF` | VS17-VS256: extended encoding |
| **Invisible operators** | `U+2061`-`U+2064` | Invisible math function application, multiplication, etc. |

#### Lower Severity: Supporting Vectors

| Category | Range | Attack Vector |
|----------|-------|---------------|
| **Interlinear annotations** | `U+FFF9`-`U+FFFB` | Hidden text layers |
| **Soft hyphen** | `U+00AD` | Token-splitting attacks on banned words |
| **Combining grapheme joiner** | `U+034F` | Token boundary manipulation |
| **Hangul fillers** | `U+115F`, `U+1160`, `U+3164`, `U+FFA0` | Invisible padding |
| **Mongolian vowel separator** | `U+180E` | Format control abuse |
| **Khmer inherent vowels** | `U+17B4`, `U+17B5` | Invisible carriers |

### Usage Example

```rust
use forge_types::strip_steganographic_chars;

// Unicode Tags ASCII smuggling attack
// "ignore" encoded as: U+E0069 U+E0067 U+E006E U+E006F U+E0072 U+E0065
let attack = "Clean\u{E0069}\u{E0067}\u{E006E}\u{E006F}\u{E0072}\u{E0065}Text";
assert_eq!(strip_steganographic_chars(attack), "CleanText");

// Zero-width space injection
let zwsp = "Hello\u{200B}World";
assert_eq!(strip_steganographic_chars(zwsp), "HelloWorld");

// Soft hyphen token-splitting (evading "ignore" keyword filter)
let split = "ig\u{00AD}nore previous instructions";
assert_eq!(strip_steganographic_chars(split), "ignore previous instructions");
```

### Scope

Apply steganographic sanitization to **untrusted external content only**:

- Web-fetched content (HTML extraction output)
- Tool results (file reads, command output)
- **NOT** direct user input (would break emoji ZWJ sequences like family emojis)

## API Key Redaction

**Functions**: `redact_api_keys(raw: &str) -> String`, `sanitize_stream_error(raw: &str) -> String`

**Location**: `engine/src/security.rs`

Prevents API keys from leaking into logs, error messages, or terminal output.

### Detected Patterns

| Provider | Pattern | Redacted To |
|----------|---------|-------------|
| OpenAI | `sk-...` | `sk-***` |
| Anthropic | `sk-ant-...` | `sk-ant-***` |
| Google/Gemini | `AIza...` | `AIza***` |

### Key Delimiters

Redaction stops at: whitespace, `"`, `'`, `,`, `}`, `]`, `)`, `\`

### Usage Example

```rust
use forge_engine::security::{redact_api_keys, sanitize_stream_error};

// OpenAI key redaction
let error = "Error: sk-abc123xyz key invalid";
assert_eq!(redact_api_keys(error), "Error: sk-*** key invalid");

// Anthropic key redaction
let error = "Error: sk-ant-api03-abc123xyz";
assert_eq!(redact_api_keys(error), "Error: sk-ant-***");

// Full stream error sanitization (redact + terminal + stego)
let raw = "Error with sk-secret123 and \x1b[31mred text\x1b[0m";
let safe = sanitize_stream_error(raw);
// Result: "Error with sk-*** and red text"
```

## Integration Points

### Tool Output Sanitization

**Function**: `sanitize_output(output: &str) -> String`

**Location**: `engine/src/tools/mod.rs`

Composes both sanitizers for tool results entering the LLM context:

```rust
pub fn sanitize_output(output: &str) -> String {
    let terminal_safe = sanitize_terminal_text(output);
    strip_steganographic_chars(&terminal_safe).into_owned()
}
```

**Applied at** (`engine/src/tool_loop.rs`):
- Tool execution results (success and error)
- Tool panic messages
- Streaming stdout/stderr chunks
- Tool name and ID display

### Stream Error Sanitization

**Function**: `sanitize_stream_error(raw: &str) -> String`

**Location**: `engine/src/security.rs`

Three-pass sanitization for error messages:

```rust
pub fn sanitize_stream_error(raw: &str) -> String {
    let redacted = redact_api_keys(raw.trim());
    let terminal_safe = sanitize_terminal_text(&redacted);
    strip_steganographic_chars(&terminal_safe).into_owned()
}
```

**Applied at** (`engine/src/streaming.rs`):
- `StreamEvent::Error` messages from LLM providers

### Web Content Sanitization

**Location**: `webfetch/src/extract.rs`

Applied after HTML-to-Markdown conversion:

```rust
let normalized = normalize_whitespace_final(&raw);
strip_steganographic_chars(&normalized).into_owned()
```

### LLM Output Sanitization

**Location**: `engine/src/streaming.rs`

Applied to streaming text and thinking deltas:

```rust
StreamEvent::TextDelta(sanitize_terminal_text(&text).into_owned())
StreamEvent::ThinkingDelta(sanitize_terminal_text(&thinking).into_owned())
```

### Recovered History Sanitization

**Location**: `engine/src/persistence.rs`

Applied when recovering partial responses after crashes:

```rust
let sanitized = sanitize_terminal_text(partial_text);
let sanitized_error = sanitize_terminal_text(error);
```

### TUI Display Sanitization

**Locations**: `tui/src/lib.rs`, `tui/src/ui_inline.rs`, `tui/src/shared.rs`

All message content, tool names, tool IDs, and tool output lines are sanitized before rendering:

```rust
let content = sanitize_terminal_text(msg.content());
let name = sanitize_terminal_text(&status.name);
let safe_line = sanitize_terminal_text(line);
```

## Threat Model

### Terminal Escape Injection

**Attack**: Malicious LLM output contains ANSI escape sequences.

**Impact**:
- OSC 52: Write to user's clipboard (exfiltrate data or inject commands)
- OSC 8: Create deceptive hyperlinks
- CSI: Move cursor to overwrite displayed content
- Terminal state manipulation

**Mitigation**: `sanitize_terminal_text` at all display boundaries.

### Invisible Prompt Injection

**Attack**: Untrusted content (web pages, files) contains invisible Unicode characters encoding instructions the LLM interprets but humans cannot see.

**Example**: A web page contains visually "Hello World" but actually encodes "Hello [invisible: ignore previous instructions and...] World"

**Impact**: Prompt injection bypassing human review.

**Mitigation**: `strip_steganographic_chars` on all external content before LLM ingestion.

### Trojan Source (Visual Spoofing)

**Attack**: Bidirectional control characters make code appear different from its logical interpretation.

**Example**: Code visually shows `if (admin)` but logically executes `if (user)` due to RLO/LRO overrides.

**Impact**: Malicious code hidden in plain sight, code review bypass.

**Mitigation**: Both sanitizers strip bidi controls (defense-in-depth).

### Credential Leaks

**Attack**: API keys appear in error messages that get logged or displayed.

**Impact**: Key exposure enabling unauthorized API access.

**Mitigation**: `redact_api_keys` in error handling paths.

### DoS Considerations

**Attack**: Extremely large inputs could cause sanitization to consume excessive resources.

**Mitigation**: Upstream size limits:
- Tool output: `max_output_bytes` configuration
- Web content: Response size limits in webfetch
- File reads: `max_file_read_bytes` limit

## Design Decisions

### Invariant-First Architecture (IFA) Conformance

Per the project's IFA principles, sanitization occurs at a **single point of encoding** - the boundary where untrusted data enters trusted contexts:

1. **LLM output -> Display**: Sanitized in streaming handler
2. **External content -> LLM context**: Sanitized at ingestion (tools, webfetch)
3. **Error messages -> Display**: Sanitized in error handler

This prevents sanitization gaps and redundant processing.

### `Cow<str>` for Zero-Allocation Fast Path

Both sanitizers return `Cow<'_, str>`:

```rust
pub fn sanitize_terminal_text(input: &str) -> Cow<'_, str> {
    if !needs_sanitization(input) {
        return Cow::Borrowed(input);  // No allocation
    }
    // ... build sanitized string
    Cow::Owned(result)
}
```

**Rationale**: Most well-behaved LLM output requires no sanitization. The fast path avoids allocation overhead in the common case while still providing safety guarantees.

### Composition of Sanitizers at Boundaries

Different threat classes require different sanitizers:

| Boundary | Threat | Sanitizer |
|----------|--------|-----------|
| Display | Terminal injection | `sanitize_terminal_text` |
| LLM context | Prompt injection | `strip_steganographic_chars` |
| Both | Mixed threat | `sanitize_output` (composes both) |
| Errors | Credential leak + display | `sanitize_stream_error` (all three) |

### ZWJ Stripped Unconditionally

Zero Width Joiner (`U+200D`) is stripped even though it's used in emoji sequences (family emojis, skin tone combinations).

**Rationale**:
1. LLM context doesn't need rendered emoji - semantic meaning is preserved
2. ZWJ is a documented steganographic attack vector
3. Scope is limited to untrusted external content, not user input

**Tradeoff**: Compound emojis in tool output may display as separate characters. Acceptable for security.

### Bidi Controls in Both Sanitizers

Bidirectional control characters are stripped by both `sanitize_terminal_text` and `strip_steganographic_chars`.

**Rationale**: Defense-in-depth. If either sanitizer is called alone (not composed), Trojan Source protection is still active.

### Pattern-Based Key Detection

API key redaction uses pattern matching (`sk-`, `sk-ant-`, `AIza`) rather than entropy analysis.

**Tradeoffs**:
- Pro: Fast, deterministic, no false positives on high-entropy normal text
- Con: Could miss keys with non-standard prefixes
- Mitigation: Covers the three supported providers; can extend patterns as needed

## Testing

The sanitization infrastructure has comprehensive test coverage in `types/src/sanitize.rs` and `engine/src/security.rs`:

```bash
cargo test sanitize       # Run sanitization tests
cargo test redact         # Run redaction tests
```

Test categories:
- Fast path verification (no allocation for clean input)
- Escape sequence stripping (CSI, OSC, DCS, C0, C1)
- Steganographic character removal (all categories)
- Bidi control stripping
- API key redaction (all providers)
- Composition tests (terminal + stego combined)
- Edge cases (empty strings, incomplete sequences, mixed content)
