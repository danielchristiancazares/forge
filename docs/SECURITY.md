# Security Sanitization Infrastructure

This document describes Forge's defense-in-depth sanitization architecture for protecting both terminal display and LLM context integrity.

## Overview

Forge processes untrusted content from multiple sources:
- LLM responses (potentially manipulated by prior prompt injection)
- Web-fetched content (attacker-controlled HTML/Markdown)
- Tool execution results (file contents, command output)
- Error messages (may contain sensitive data or escape sequences)

All untrusted content passes through layered sanitization before reaching the terminal or LLM context.

## Sanitization Functions

### `sanitize_terminal_text`

**Location:** `types/src/sanitize.rs`

**Purpose:** Strip characters that could manipulate terminal state or display.

**Threat Model:** Terminal emulators interpret escape sequences that can:
- Manipulate clipboard (OSC 52)
- Create deceptive hyperlinks (OSC 8)
- Rewrite displayed content via cursor movement (CSI)
- Alter terminal state/configuration

**Stripped Categories:**

| Category | Description |
|----------|-------------|
| ANSI escape sequences | CSI, OSC, DCS, PM, APC sequences |
| C0 controls | 0x00–0x1F except `\n`, `\t`, `\r` |
| C1 controls | 0x80–0x9F |
| DEL character | 0x7F |
| Bidi controls | U+061C, U+200E–U+200F, U+202A–U+202E, U+2066–U+2069 |

**API:**
```rust
pub fn sanitize_terminal_text(input: &str) -> Cow<'_, str>
```

Returns `Cow::Borrowed` when no sanitization needed (zero-allocation fast path).

---

### `strip_steganographic_chars`

**Location:** `types/src/sanitize.rs`

**Purpose:** Strip invisible Unicode characters used for prompt injection attacks.

**Threat Model:** Untrusted content may contain invisible Unicode payloads that encode instructions the LLM interprets but humans cannot see. These attacks include:

| Attack | Vector | Description |
|--------|--------|-------------|
| ASCII Smuggling | Unicode Tags (U+E0000–U+E007F) | Each codepoint maps directly to ASCII character |
| Binary Steganography | Zero-width characters | Encode data in sequences of invisible chars |
| Trojan Source | Bidi controls | Make text render differently than it's parsed |
| Token Splitting | Soft hyphen (U+00AD) | Split banned words to evade keyword filters |

**Stripped Categories (by severity):**

| Severity | Category | Range | Attack Vector |
|----------|----------|-------|---------------|
| HIGH | Unicode Tags | U+E0000–U+E007F | ASCII smuggling (direct mapping) |
| HIGH | Zero-width chars | U+200B–U+200F, U+2060, U+FEFF | Binary steganography |
| HIGH | Bidi controls | U+202A–U+202E, U+2066–U+2069, U+061C | Visual spoofing (Trojan Source) |
| MEDIUM | Variation selectors | U+FE00–U+FE0F, U+E0100–U+E01EF | Payload encoding |
| MEDIUM | Invisible operators | U+2061–U+2064 | Hidden semantic content |
| LOWER | Interlinear annotations | U+FFF9–U+FFFB | Hidden text layers |
| LOWER | Soft hyphen | U+00AD | Token-splitting attacks |
| LOWER | Combining grapheme joiner | U+034F | Token boundary manipulation |
| LOWER | Filler characters | U+115F, U+1160, U+3164, U+FFA0, U+180E, U+17B4, U+17B5 | Invisible padding |

**API:**
```rust
pub fn strip_steganographic_chars(input: &str) -> Cow<'_, str>
```

Returns `Cow::Borrowed` when no steganographic characters found (zero-allocation fast path).

**Important:** ZWJ (Zero-Width Joiner, U+200D) is stripped unconditionally. This breaks emoji composition (e.g., 👨‍👩‍👧 renders as 👨👩👧) but is acceptable for LLM context where visual emoji rendering is not required.

---

### `redact_api_keys`

**Location:** `engine/src/security.rs`

**Purpose:** Remove API keys from error messages before display or logging.

**Detected Patterns:**

| Provider | Pattern | Redacted Form |
|----------|---------|---------------|
| OpenAI | `sk-...` | `sk-***` |
| Anthropic | `sk-ant-...` | `sk-ant-***` |
| Google/Gemini | `AIza...` | `AIza***` |

**API:**
```rust
pub fn redact_api_keys(raw: &str) -> String
```

## Integration Points

### Tool Output Sanitization

**Location:** `engine/src/tools/mod.rs:sanitize_output()`

All tool execution results pass through this function before entering the LLM context
(terminal + stego normalization + secret redaction):

```rust
pub fn sanitize_output(output: &str) -> String {
    crate::security::sanitize_display_text(output)
}
```

**Protects against:**
- Files containing terminal escapes or steganographic payloads
- Command output with embedded injection attempts
- Any tool that reads untrusted content

### Stream Error Sanitization

**Location:** `engine/src/security.rs:sanitize_stream_error()`

Error messages from API streams are sanitized before display. Normalization (terminal + stego)
MUST run before any redaction to prevent split-then-rejoin bypasses.

```rust
pub fn sanitize_stream_error(raw: &str) -> String {
    let trimmed = raw.trim();

    // Normalize untrusted text first
    let terminal_safe = sanitize_terminal_text(trimmed);
    let normalized = strip_steganographic_chars(terminal_safe.as_ref());

    // Then redact secrets (pattern + env-derived)
    let pattern_redacted = redact_api_keys(normalized.as_ref());
    secret_redactor().redact(&pattern_redacted).into_owned()
}
```

**Protects against:**
- API key leakage in error messages
- Malformed responses with escape sequences
- Error payloads with steganographic content

### Web Content Sanitization

**Location:** `webfetch/src/extract.rs`

Web-fetched content is sanitized after HTML-to-Markdown conversion:

```rust
let raw = convert_element_to_markdown(element, ctx);
let normalized = normalize_whitespace_final(&raw);
strip_steganographic_chars(&normalized).into_owned()
```

**Protects against:**
- Malicious websites embedding invisible instructions
- HTML that renders normally but contains hidden Unicode payloads
- Web pages designed to manipulate LLM behavior

**Note:** Web content also has upstream protection via `max_download_bytes` (default 10MB) limiting DoS potential.

## DoS Considerations

Sanitization functions process untrusted input character-by-character. To prevent them from becoming denial-of-service vectors on large malicious payloads:

1. **Upstream Size Limits:** Tools enforce strict byte limits *before* sanitization:
   - `read_file` has configurable line limits
   - Command output is truncated at tool boundaries
   - Web fetch respects `max_download_bytes`

2. **Zero-Allocation Fast Path:** The `Cow::Borrowed` optimization ensures sanitizers don't become bottlenecks for legitimate high-volume streaming output (the 99% case of clean content).

3. **Linear Time Complexity:** Both sanitizers are O(n) single-pass algorithms with no backtracking or regex engines.

## Design Decisions

### Composition at Boundaries

Forge keeps primitive, single-purpose sanitizers and composes them in a canonical
helper for untrusted external text. This avoids call-site drift (IFA §7) and
ensures ordering is consistent (normalization before redaction).

| Function | Threat Class | Scope |
|----------|--------------|-------|
| `sanitize_terminal_text` | Display safety | Terminal emulator manipulation |
| `strip_steganographic_chars` | Context safety | LLM prompt injection |
| `sanitize_display_text` | Untrusted external text | Terminal + stego + secret redaction |

Composition at boundaries stays explicit, but most call sites should use the
canonical helper:
```rust
// Untrusted external content (LLM output, tool output, provider errors):
let safe = sanitize_display_text(raw_untrusted);

// User-authored text (preserve emoji ZWJ composition):
let safe_user = sanitize_terminal_text(raw_user);
```

### Defense in Depth

Bidi controls are stripped by **both** functions. This provides defense in depth:
- If only terminal sanitization is applied, Trojan Source is still prevented
- If only steganographic stripping is applied, Trojan Source is still prevented
- Composition order doesn't matter for Bidi controls

### Zero-Allocation Fast Path

Both sanitization functions use `Cow<str>` to avoid allocation when no sanitization is needed:
- `Cow::Borrowed(input)` — no dangerous characters found, return original slice
- `Cow::Owned(result)` — allocation required for filtered output

This optimizes the common case (clean content) while handling the adversarial case.

### User Input Not Sanitized

Direct user input bypasses steganographic stripping because:
1. Users may legitimately use ZWJ for emoji composition
2. Users have agency over their own prompts
3. The threat model focuses on untrusted third-party content

User-authored text is still terminal-sanitized for display safety, but untrusted
external content additionally uses stego stripping and secret redaction.

## IFA Conformance

This architecture follows Invariant-First Architecture principles:

1. **Single Point of Encoding (§7):** `sanitize_display_text` is the canonical sanitizer for untrusted external text, and boundary helpers (`sanitize_output`, `sanitize_stream_error`) delegate to it or match its ordering.

2. **Boundary and Core Separation (§11):** Sanitization functions live in `types` (boundary module), applied at ingestion points. Core receives already-sanitized strings.

3. **Mechanism vs Policy (§8):** Pure mechanism that strips characters. No embedded policy decisions. Callers decide when to apply.

4. **Composability (§7.5):** Primitive sanitizers remain reusable for specialized boundaries; `sanitize_display_text` composes them for the common untrusted-external-text case.

## Testing

The sanitization module includes comprehensive tests covering:

- **Fast path verification:** Confirm no allocation for clean input
- **All stripped categories:** Each character class has dedicated tests
- **Attack vectors:** Real-world attack patterns (ASCII smuggling, Trojan Source, token splitting)
- **Composition:** Tests for combined terminal + steganographic sanitization
- **Edge cases:** Empty strings, strings containing only stripped characters

Run tests with:
```bash
cargo test -p forge-types
```

## References

- [Unicode Tags Block](https://en.wikipedia.org/wiki/Tags_(Unicode_block))
- [Trojan Source: Invisible Vulnerabilities](https://trojansource.codes/)
- [ASCII Smuggling via Unicode Tags](https://embracethered.com/blog/ascii-smuggler.html)
- [Hidden Layer: Invisible Unicode Text Prompt Injection](https://blog.seclify.com/the-hidden-layer/)
