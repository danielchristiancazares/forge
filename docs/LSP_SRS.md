## Forge Consumes LSP Diagnostics (MVP) via New forge-lsp Crate

### Summary

Add a new workspace crate, `lsp/` (`forge-lsp`), that implements a minimal LSP client focused on `publishDiagnostics`. `forge-engine` depends on `forge-lsp` and delegates all protocol/process concerns to it, keeping engine code focused on orchestration and UI state.

MVP target: Rust via `rust-analyzer`, with config hooks to add more servers later.

- Enables isolated tests: unit tests for framing/routing + integration tests with a fake LSP server binary, runnable as `cargo test -p forge-lsp`.
- Keeps the engine as orchestration + UI state, not protocol plumbing.

---

## Workspace / Crate Structure

### Add new member

- `lsp/Cargo.toml` with package name `forge-lsp`
- Add `"lsp"` to root `Cargo.toml` `[workspace].members`
- Add `forge-lsp = { path = "lsp" }` to `[workspace.dependencies]`

### forge-lsp dependencies (minimal)

- `tokio` (process + IO + sync)
- `serde`, `serde_json`
- `tracing`
- `url` (already a workspace dependency — reference via `url.workspace = true`)

---

## Public Interface: Engine Talks to forge-lsp (No Engine Types Leaking In)

### Goals

- Engine supplies: workspace root, server command, and file contents after edits.
- forge-lsp supplies: diagnostics snapshots + status/events.
- No filesystem reads inside forge-lsp for MVP (engine already has limits/sandbox-aware semantics).

### Types (forge-lsp)

```
LspConfig
    servers: Vec<ServerConfig>

ServerConfig
    name: String              (e.g. "rust-analyzer")
    language_id: String       (e.g. "rust")
    command: String
    args: Vec<String>
    file_extensions: Vec<String>  (e.g. ["rs"])
    root_markers: Vec<String>     (e.g. ["Cargo.toml", ".git"])

LspState
    Starting        — spawn + initialize in progress
    Running         — initialized notification sent, server ready
    Stopped(String) — exited or failed; String is human-readable reason

LspEvent
    Status { server: String, state: LspState }
    Diagnostics { path: PathBuf, items: Vec<ForgeDiagnostic> }

ForgeDiagnostic
    severity: DiagnosticSeverity   (Error | Warning | Information | Hint)
    message: String
    range: DiagnosticRange         (start_line, start_col, end_line, end_col — 0-based)
    source: Option<String>         (e.g. "rustc", "clippy")
    code: Option<String>           (e.g. "E0308")

DiagnosticSeverity
    Error | Warning | Information | Hint

DiagnosticRange
    start_line: u32
    start_col: u32
    end_line: u32
    end_col: u32

DiagnosticsSnapshot
    total_errors: usize
    total_warnings: usize
    by_file: BTreeMap<PathBuf, Vec<ForgeDiagnostic>>   (errors sorted first within each file)
```

### Methods

- `LspManager::new(config: LspConfig) -> LspManager`
- `LspManager::ensure_started(workspace_root: &Path) -> Result<()>`
  - Idempotent. Spawns servers that aren't already running. If a server failed and the cooldown (30 s) has elapsed, retries once.
- `LspManager::notify_file_changed(path: &Path, text_utf8: &str) -> Result<()>`
  - Internally tracks open documents: first call for a path sends `textDocument/didOpen`; subsequent calls send `textDocument/didChange` (full content).
- `LspManager::notify_file_saved(path: &Path) -> Result<()>`
  - Sends `textDocument/didSave`. Many servers refresh diagnostics on save, not change.
- `LspManager::notify_file_closed(path: &Path) -> Result<()>`
  - Sends `textDocument/didClose` and removes the path from the open-document set.
- `LspManager::poll_events(&mut self, budget: usize) -> Vec<LspEvent>`
  - Non-blocking; called from `App::tick()` like other event drains. Drains at most `budget` events from the internal channel.
- `LspManager::snapshot(&self) -> DiagnosticsSnapshot`
  - Clones current diagnostics state for UI rendering.
- `LspManager::shutdown(&mut self)`
  - Sends LSP `shutdown` request to each running server, awaits response (with 5 s timeout), then sends `exit` notification. Kills the child process if the server doesn't exit within the timeout.

---

## JSON-RPC / LSP Implementation Details (Inside forge-lsp)

### Framing (Codec)

Bidirectional, applied to both stdin (write) and stdout (read):

- **Write:** `Content-Length: N\r\n\r\n{json}`
- **Read:** Parse `Content-Length` header from byte stream → read exactly N bytes → deserialize JSON. Must handle partial reads (header split across buffers) and back-to-back frames in a single read.

### Async Topology

Per server, two long-lived tasks:

1. **Reader task** — owns the child's stdout `BufReader`. Runs the frame codec read loop, dispatches:
   - Responses (have `id`): resolve the matching `oneshot::Sender` in the pending map.
   - Notifications (no `id`): if `textDocument/publishDiagnostics`, update `DiagnosticsStore` and push an `LspEvent::Diagnostics` to the event channel.
   - Unknown methods: log at `debug`, discard.
2. **Writer task** — owns the child's stdin. Receives outbound messages (`Request` | `Notification`) from an `mpsc` channel and writes framed JSON-RPC.

Event channel from reader → `poll_events`: bounded `mpsc` (capacity 256). If full, oldest events are dropped (diagnostics are last-write-wins per file anyway).

### Request/Response Correlation

- `next_request_id: u64` (monotonic)
- `pending: HashMap<u64, oneshot::Sender<serde_json::Value>>`
- Reader task resolves or drops senders; callers await with a timeout.

### Routing

- `textDocument/publishDiagnostics` → update `DiagnosticsStore`, emit `LspEvent::Diagnostics`
- `window/logMessage`, `window/showMessage` → log at appropriate tracing level, discard
- All other notifications → `trace!`, discard

### Initialization Sequence

1. Spawn server via `tokio::process::Command` with stdin/stdout piped, stderr inherited (or logged).
2. Send `initialize` request:
   - `rootUri` and `workspaceFolders` (single root)
   - `capabilities.textDocumentSync`: request full sync (`TextDocumentSyncKind::Full`)
   - `capabilities.textDocument.publishDiagnostics`: `{ relatedInformation: false }`
3. Await `initialize` response (10 s timeout). Verify server did not return an error.
4. Send `initialized` notification (`{}`).
5. Transition server state to `LspState::Running`, emit `LspEvent::Status`.

### Document Lifecycle

forge-lsp maintains `open_documents: HashSet<Url>` per server.

| Engine calls               | LSP notification sent        | State change                         |
|----------------------------|------------------------------|--------------------------------------|
| `notify_file_changed` (first time for path) | `textDocument/didOpen`  | Insert into `open_documents`  |
| `notify_file_changed` (already open)        | `textDocument/didChange` (full) | No set change           |
| `notify_file_saved`                         | `textDocument/didSave`         | No set change            |
| `notify_file_closed`                        | `textDocument/didClose`        | Remove from `open_documents` |

### Shutdown Sequence

1. Send `shutdown` request (id = next).
2. Await response with 5 s timeout.
3. Send `exit` notification.
4. `child.wait()` with 2 s timeout.
5. If still alive, `child.kill()`.

On unexpected EOF / broken pipe during normal operation: mark server as `Stopped("server process exited unexpectedly")`, emit status event. Do not auto-restart; let `ensure_started` handle it on the next call if the cooldown has elapsed.

### Server Crash / Failure Recovery

- **Spawn failure** (binary not found): emit `LspEvent::Status` with `Stopped`, log actionable install hint. Do not retry until user restarts or `ensure_started` is called after cooldown.
- **Initialize timeout** (10 s): kill child, mark `Stopped("initialization timed out")`.
- **Mid-session crash** (EOF on stdout): mark `Stopped`, clear `open_documents`. Next `ensure_started` call after 30 s cooldown re-spawns.
- **Malformed JSON-RPC**: log at `warn`, skip the frame. Do not kill the server over a single bad frame.

### Windows Correctness

Central helper: `path_to_file_uri(Path) -> Url` that:
- Normalizes separators
- Percent-encodes spaces
- Produces `file:///C:/...` correctly

---

## Engine Wiring (Where It Hooks In)

### Trigger point: tool-driven file changes

Forge already records per-turn created/modified files via `ChangeRecorder` (shared `Arc<Mutex<TurnChangeLog>>`). The recorder is accessible from tool executors throughout the turn — it does not require consuming `TurnContext`.

Implementation wiring plan (engine-side):

1. Add `forge-lsp` dependency in `engine/Cargo.toml`.
2. Add `App` field: `lsp: Option<LspManager>` plus UI-facing cached `DiagnosticsSnapshot`.
3. In tool executors that write files (write, apply-patch): after the file write succeeds and the change is recorded via `ChangeRecorder`, call `lsp.notify_file_changed(path, text)` and `lsp.notify_file_saved(path)` inline. The recorder already holds the paths and the executor already has the content bytes. This avoids the need to re-read files or wait for batch/turn boundaries.
4. At turn end (`TurnContext::finish`): for any files in the `created`/`modified` sets that were not yet opened with the LSP (belt-and-suspenders), send `notify_file_changed` + `notify_file_saved`.
5. Before auto-resuming to the next model step (after tool batch completes), wait briefly for diagnostics to settle:
   - Goal: ensure the next LLM prompt includes any newly introduced errors.
   - Wait condition: either (a) at least one `publishDiagnostics` received for each just-edited file, or (b) timeout.
   - Timeout: small + bounded (configurable; default 1 s), then proceed with best-effort snapshot.
6. Each `tick()`, after `poll_distillation()` and `poll_tool_loop()`:
   - `events = lsp.poll_events(budget)`
   - Update engine's cached `DiagnosticsSnapshot` for UI.
   - If error count changed, push a debounced system notification.
7. On `App` drop / quit: call `lsp.shutdown()`.

### Agent Feedback Integration (MVP Value-Add)

Goal: code changed → new errors introduced → alert the model with bounded, actionable diagnostics.

After each tool batch, once diagnostics have settled (step 5 above):

- Only report **new** errors in the just-edited files (delta vs the previous diagnostics snapshot).
  - Delta key: `(source, code, severity, message)`. Range is for display only (line/col), not identity.
  - If there is no baseline snapshot for a file yet (never seen diagnostics for it): treat all current errors as "new (no baseline)" for MVP. Suppressing until a baseline exists is a future config option.
- Cap model feedback:
  - Max files: 5 (prioritized by error count descending)
  - Max errors per file: 5
  - Truncate each diagnostic message: 200 chars
  - Format: `path:line:col: severity: message` (one per line), prefixed with a brief system note
- If only warnings/info: omit by default (unless later you add a command toggle)

---

## Optimizations (MVP-Friendly, Not Fancy)

- Keep servers alive for the session (no restart cost).
- **Debounce** (inside `LspManager`): coalesce multiple `notify_file_changed` calls per file within 200 ms before sending `didChange` to the server. The debounce timer resets on each call. The caller (engine) is fire-and-forget; `LspManager` buffers internally.
- Only sync files Forge touched (don't open the whole workspace).
- Full-sync `didChange` (simpler, reliable); incremental only if perf forces it.
- **Hard caps are enforced by the engine**, not forge-lsp (engine already has size/binary policies):
  - Skip binary detection (NUL in first 8 KB)
  - Skip very large files (configurable; default ~1–2 MB)
  - forge-lsp trusts that `text_utf8` passed to it is valid, bounded text.
- UI polling budget (avoid stalling render loop): drain max N LSP events per tick.

---

## UI

- Problems panel:
  - Grouped by file, errors first
  - Entries show `path:line:col severity message`
- Commands (minimal):
  - `:problems` open/close
  - `:next_error`, `:prev_error`

### Failure UX

- If server missing at startup: one notification with actionable install hint, then silence for that session.
- If server crashes mid-session: one notification with reason, then silence until cooldown triggers a restart attempt.

---

## Config Surface (Minimal)

```toml
[lsp]
enabled = true

[lsp.servers.rust]
command = "rust-analyzer"
args = []
language_id = "rust"
file_extensions = ["rs"]
root_markers = ["Cargo.toml", ".git"]
```

Defaults if `[lsp]` absent:

- `enabled = false` (defaulting off to avoid surprise background processes)
- A built-in `rust-analyzer` preset is used if enabled and `Cargo.toml` exists and `rust-analyzer` is on `PATH`.

---

## Testing Strategy (Isolated in forge-lsp)

### Unit Tests

- **Frame codec:** split headers/bodies across reads; multiple frames in one buffer; partial header reads; zero-length edge case
- **JSON-RPC routing:** request/response correlation; response to unknown id logged and dropped; unknown notification methods ignored safely
- **Document lifecycle:** didOpen on first change, didChange on subsequent, didClose removes from set

### Integration Tests (no real rust-analyzer required)

- `forge-lsp/tests/fake_server.rs`: small test binary implementing:
  - `initialize` → responds with capabilities
  - `initialized` → accepted (no response)
  - `textDocument/didOpen`, `textDocument/didChange`, `textDocument/didSave` → accepted
  - Emits deterministic `publishDiagnostics` after receiving `didOpen` or `didChange`
  - `shutdown` → responds with `null`
  - `exit` → process exits cleanly
- Tests assert:
  - Manager receives diagnostics events with correct severity/range/message
  - Snapshot grouping/counts correct
  - Shutdown completes without hanging (timeout-guarded)
  - Server crash (fake server exits unexpectedly) produces `Stopped` status event

---

## Assumptions / Defaults

- MVP language support: Rust only (`rust-analyzer`).
- Only tool-driven edits trigger LSP sync (Forge is not a keystroke editor).
- Engine reads file contents and enforces size/binary limits; forge-lsp stays protocol/process only.
- Diagnostics are used for: UI + bounded model feedback; no automatic code actions in MVP.
- `textDocument/didSave` is sent because `rust-analyzer` and many servers use save events as a diagnostic refresh trigger.
- forge-lsp does not parse server capabilities beyond verifying `initialize` succeeded. Pull-model diagnostics (LSP 3.17+) are out of scope for MVP.
