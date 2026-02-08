# Multimodal Attachment Foundation SDD

**Phase**: 1 of 4 (Prep Work)
**Depends on**: Nothing
**Unlocks**: Phase 2 (Gemini PDF), Phase 3 (Claude/OpenAI Attachments), Phase 4 (UX Surface)

## 1. Goal

Add the foundational types for binary file attachments to `forge-types`, so that
`UserMessage` can carry PDFs, images, and other files alongside text. No provider
changes, no UX changes -- this phase is pure type plumbing.

## 2. Motivation

All three providers now support multimodal inputs (PDFs, images), but Forge's
`Message` type is text-only. Before any provider can serialize attachments, the
shared type layer must support them. Doing this as isolated prep work keeps the
blast radius small and lets us validate the serialization/persistence story
before touching provider code.

## 3. Current State

`UserMessage` in `types/src/lib.rs`:

```rust
pub struct UserMessage {
    content: NonEmptyString,
    timestamp: SystemTime,
}
```

Consumed by:

- `providers/src/lib.rs` -- Claude (line 627), OpenAI, Gemini (line 2553) all
  match `Message::User(_)` and call `msg.content()` to get text.
- `context/src/distillation.rs` -- matches `Message::User(_)` for role labeling.
- `engine/src/input_modes.rs` -- `QueuedUserMessage` wraps `ApiConfig` + `TurnContext`,
  not `UserMessage` directly.

## 4. Proposed Changes

### 4.1. New Types in `forge-types`

```rust
/// Validated MIME type for attachments.
///
/// Restricted to types supported by at least one LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentMime {
    Pdf,
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl AttachmentMime {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pdf => "application/pdf",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// How the attachment data is provided.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AttachmentData {
    /// Raw bytes (base64-encoded at the provider serialization boundary).
    Inline(Vec<u8>),
    /// Remote URI (File API, GCS, presigned URL).
    Uri(String),
}

/// A file attached to a user message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub mime: AttachmentMime,
    pub data: AttachmentData,
    /// Optional human-readable label (e.g. filename).
    pub label: Option<String>,
}
```

### 4.2. `UserMessage` Update

```diff
 pub struct UserMessage {
     content: NonEmptyString,
+    attachments: Vec<Attachment>,
     timestamp: SystemTime,
 }
```

**Key constraints:**

- `attachments` defaults to `Vec::new()` via `#[serde(default)]`.
- Existing constructors (`UserMessage::new`, `Message::try_user`) are unchanged
  -- they produce messages with empty attachments.
- A new constructor is added for multimodal messages:

```rust
impl UserMessage {
    pub fn with_attachments(content: NonEmptyString, attachments: Vec<Attachment>) -> Self {
        Self { content, attachments, timestamp: SystemTime::now() }
    }

    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }
}
```

### 4.3. `Message` Convenience Methods

```rust
impl Message {
    pub fn user_with_attachments(
        content: NonEmptyString,
        attachments: Vec<Attachment>,
    ) -> Self {
        Self::User(UserMessage::with_attachments(content, attachments))
    }
}
```

### 4.4 Impact on Existing Code

**No breakage expected.** All existing `Message::User(_)` match arms continue
to work. The new `attachments` field is empty by default and invisible to code
that doesn't ask for it. Specifically:

| Crate | File | Impact |
|:------|:-----|:-------|
| `providers` | `lib.rs` (Claude, OpenAI, Gemini) | None -- still calls `msg.content()`. Attachment serialization is Phase 2/3. |
| `context` | `distillation.rs` | None -- matches on variant, not fields. |
| `engine` | `input_modes.rs` | None -- `QueuedUserMessage` doesn't wrap `UserMessage`. |
| `types` | `lib.rs` | Serde: `#[serde(default)]` on `attachments` ensures backward-compatible deserialization of existing persisted messages. |

## 5. Security Considerations

- **Size validation**: `Attachment` does not enforce size limits. That is the
  responsibility of the caller (engine layer) at construction time. Phase 2+
  SDDs will specify per-provider limits.
- **MIME trust boundary**: `AttachmentMime` is a closed enum -- no arbitrary
  strings. Providers can further restrict which MIME types they accept.

## 6. Verification Plan

### Automated Tests

- `UserMessage::new()` produces empty attachments.
- `UserMessage::with_attachments()` stores and returns attachments.
- Serde round-trip: serialize a `UserMessage` with attachments, deserialize,
  verify equality.
- Backward compat: deserialize a JSON blob without `attachments` field into
  `UserMessage` (should succeed with empty vec).
- `AttachmentMime::as_str()` returns correct MIME strings.

### Build Validation

- `cargo build` -- no compile errors across workspace.
- `cargo test` -- all existing tests pass.
- `cargo clippy --workspace --all-targets -- -D warnings` -- clean.
