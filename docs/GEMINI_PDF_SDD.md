# Gemini PDF Integration SDD

**Phase**: 2 of 4
**Depends on**: Phase 1 (Multimodal Attachment Foundation)
**Unlocks**: Phase 3 (Claude/OpenAI Attachments)

## 1. Goal

Enable Forge to send PDF attachments to Gemini 3 models (Pro/Flash) using the
foundational `Attachment` types from Phase 1.

## 2. Scope

### In Scope

- `MediaResolution` enum in `forge-providers`.
- `ApiConfig` field + builder/getter for `gemini_media_resolution`.
- `build_request_body` update to emit `inline_data` / `file_data` for
  `UserMessage` attachments.
- `send_message` signature threading.
- Serialization tests.

### Out of Scope

- Claude/OpenAI attachment support (Phase 3).
- UX for attaching files (Phase 4).
- `context` crate token counting for image tokens (separate SDD).

## 3. API Reference

Gemini 3 accepts PDFs via two mechanisms:

```json
// inline_data (base64, up to ~50 MB)
{ "inline_data": { "mime_type": "application/pdf", "data": "BASE64..." } }

// file_data (File API URI, up to 2 GB)
{ "file_data": { "mime_type": "application/pdf", "file_uri": "https://..." } }
```

Resolution control via `generationConfig`:

```json
{ "generationConfig": { "mediaResolution": "MEDIA_RESOLUTION_MEDIUM" } }
```

Levels: `LOW`, `MEDIUM` (recommended for documents), `HIGH`, `ULTRA_HIGH` (per-part only).

## 4. Detailed Design

*To be expanded when Phase 1 is complete and this phase begins.*
