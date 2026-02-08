# forge-lsp

LSP client for consuming language server diagnostics. It manages the lifecycle
of language server child processes, communicates via JSON-RPC over stdin/stdout,
and accumulates `textDocument/publishDiagnostics` notifications into a
queryable diagnostics store. The engine uses it to surface compiler errors and
warnings to the LLM agent after tool batches modify files.

## LLM-TOC
<!-- Section map for LLM context (approximate line ranges) -->
| Lines | Section |
|-------|---------|
| 1-36 | Header, Intro, LLM-TOC, Table of Contents |
| 38-49 | Architecture |
| 51-145 | Key Types |
| 146-171 | Public API |
| 173-233 | How It Works |
| 235-278 | Configuration |
| 280-345 | Usage Examples |
| 347-350 | Integration with Other Crates |
| 352-367 | Testing |
| 369-402 | Design Principles |

## Table of Contents

1. [Architecture](#architecture)
2. [Key Types](#key-types)
3. [Public API](#public-api)
4. [How It Works](#how-it-works)
5. [Configuration](#configuration)
6. [Usage Examples](#usage-examples)
7. [Integration with Other Crates](#integration-with-other-crates)
8. [Testing](#testing)
9. [Design Principles](#design-principles)

---

## Architecture

The crate is organized into these modules:

| Module | Visibility | Purpose |
|--------|------------|---------|
| `types` | `pub` | Public domain types: config, diagnostics, events, severity |
| `codec` | `pub` | JSON-RPC framing codec (Content-Length header + JSON body) |
| `protocol` | `pub(crate)` | LSP message serde types, initialize/didOpen/didChange params, URI helpers |
| `server` | `pub(crate)` | Server handle: child process lifecycle, request/response routing, file notifications |
| `diagnostics` | `pub(crate)` | Per-file diagnostics accumulator with sorted snapshot generation |
| `manager` | private | Public facade (`LspManager`) that orchestrates servers and diagnostics |

## Key Types

### Configuration Types

**`LspConfig`**: Top-level LSP configuration, deserialized from `[lsp]` in Forge config.

```rust
pub struct LspConfig {
    enabled: bool,                          // Master switch for LSP integration
    servers: HashMap<String, ServerConfig>, // Named server configurations
}
```

**`ServerConfig`**: Configuration for a single language server. Validated at deserialization
time via `TryFrom<RawServerConfig>` -- rejects empty `command` or `language_id` fields.

```rust
pub struct ServerConfig {
    command: String,            // Executable name (e.g. "rust-analyzer")
    args: Vec<String>,          // Command-line arguments
    language_id: String,        // LSP language identifier (e.g. "rust")
    file_extensions: Vec<String>, // Extensions this server handles (e.g. ["rs"])
    root_markers: Vec<String>,  // Project root markers (e.g. ["Cargo.toml"])
}
```

### Diagnostic Types

**`DiagnosticSeverity`**: LSP severity levels with numeric correspondence.

| Variant | LSP Value | Label |
|---------|-----------|-------|
| `Error` | 1 | `"error"` |
| `Warning` | 2 | `"warning"` |
| `Information` | 3 | `"info"` |
| `Hint` | 4 | `"hint"` |

**`ForgeDiagnostic`**: A single diagnostic finding, converted from the LSP wire format.

```rust
pub struct ForgeDiagnostic {
    severity: DiagnosticSeverity,
    message: String,     // Human-readable description
    line: u32,           // 0-indexed line number
    col: u32,            // 0-indexed column
    source: String,      // Origin tool (e.g. "rustc", "clippy")
}
```

Key methods:
- `display_with_path(&self, path: &Path) -> String` -- formats as `path:line:col: severity: [source] message` with 1-indexed line/col for human display
- `severity()`, `message()`, `line()`, `col()`, `source()` -- field accessors

**`DiagnosticsSnapshot`**: Immutable point-in-time view of all diagnostics, sorted with
error-containing files first, then alphabetically by path.

```rust
pub struct DiagnosticsSnapshot {
    files: Vec<(PathBuf, Vec<ForgeDiagnostic>)>,
}
```

Key methods:
- `files()` -- per-file diagnostics in sorted order
- `is_empty()` -- whether there are any diagnostics
- `error_count()`, `warning_count()`, `info_count()`, `hint_count()` -- counts by severity
- `total_count()` -- total across all files and severities
- `status_string()` -- compact display like `"E:3 W:5"` (empty string when no diagnostics)

### Event Types

**`LspEvent`**: Events emitted by server reader tasks, consumed by the manager.

| Variant | Description |
|---------|-------------|
| `ServerStopped { server, reason }` | Server exited or crashed; manager removes it from the active set |
| `Diagnostics { path, items }` | New diagnostics published for a file path |

**`ServerStopReason`**: Why a server stopped.

| Variant | Description |
|---------|-------------|
| `Exited` | Clean shutdown (EOF on stdout) |
| `Failed(String)` | Crash or I/O error with message |

### Codec Types

**`FrameReader<R>`**: Async reader that parses LSP's `Content-Length` header framing.
Reads headers (case-insensitive `Content-Length` lookup), validates the length against
a 4 MiB maximum (`MAX_FRAME_BYTES`), reads the body, and deserializes into
`serde_json::Value`. Returns `Ok(None)` on clean EOF, errors on truncated frames.

**`FrameWriter<W>`**: Async writer that serializes a `serde_json::Value` to JSON,
prepends the `Content-Length: N\r\n\r\n` header, and flushes.

## Public API

The crate exports these items from the root:

| Export | Kind | Purpose |
|--------|------|---------|
| `LspManager` | struct | Primary facade for starting, driving, and shutting down LSP servers |
| `LspConfig` | struct | Deserialized configuration |
| `ServerConfig` | struct | Per-server configuration |
| `DiagnosticsSnapshot` | struct | Immutable diagnostics view |
| `ForgeDiagnostic` | struct | Single diagnostic item |
| `DiagnosticSeverity` | enum | Error / Warning / Information / Hint |
| `LspEvent` | enum | Server-stopped and diagnostics events |
| `ServerStopReason` | enum | Exited or Failed |

### LspManager Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `start` | `async fn start(config: LspConfig, workspace_root: &Path) -> Self` | Spawns all configured servers, performs initialize handshake for each |
| `on_file_changed` | `async fn on_file_changed(&mut self, path: &Path, text: &str)` | Routes `didOpen`/`didChange` to the server registered for the file's extension |
| `poll_events` | `fn poll_events(&mut self, budget: usize) -> usize` | Non-blocking drain of up to `budget` events from the channel; returns count processed |
| `snapshot` | `fn snapshot(&self) -> DiagnosticsSnapshot` | Returns current sorted diagnostics snapshot |
| `errors_for_files` | `fn errors_for_files(&self, paths: &[PathBuf]) -> Vec<(PathBuf, Vec<ForgeDiagnostic>)>` | Returns only error-severity diagnostics for the given file paths |
| `has_running_servers` | `fn has_running_servers(&self) -> bool` | Whether any server processes are still active |
| `shutdown` | `async fn shutdown(&mut self)` | Gracefully shuts down all servers (shutdown request, exit notification, kill timeout) |

## How It Works

### Server Lifecycle

1. **Spawn**: `RunningServer::start()` spawns the server command as a child process
   with stdin/stdout piped and stderr discarded. `kill_on_drop(true)` ensures cleanup.

2. **Writer task**: A dedicated tokio task reads `WriterCommand` messages from an
   `mpsc` channel and writes JSON-RPC frames to the child's stdin via `FrameWriter`.
   Channel capacity is 64.

3. **Reader task**: A dedicated tokio task reads JSON-RPC frames from the child's
   stdout via `FrameReader` and dispatches each frame:
   - **Responses** (has `id`, has `result`/`error`): Routed to the pending `oneshot`
     sender registered by `send_request()`.
   - **Server requests** (has `id` and `method`): Replied with a JSON-RPC
     "Method not found" error (-32601) so the server does not block.
   - **Notifications** (no `id`, has `method`): Only `textDocument/publishDiagnostics`
     is processed; all others are silently ignored.

4. **Initialize handshake**: After spawn, the server performs the LSP initialize
   sequence: sends an `initialize` request (with process ID, root URI, minimal
   capabilities advertising `publishDiagnostics` support), waits up to 30 seconds
   for the response, then sends an `initialized` notification.

5. **Shutdown**: `shutdown()` sends a `shutdown` request, follows with an `exit`
   notification on success, signals the writer task to stop, then waits up to
   2 seconds for the process to exit before killing it.

### File Change Notifications

When `on_file_changed(path, text)` is called on the manager:

1. The file extension is extracted and looked up in the extension map to find the
   responsible server name.
2. If the file has not been opened before on that server, a `textDocument/didOpen`
   notification is sent with `version: 1`.
3. On subsequent changes, `textDocument/didChange` is sent with a monotonically
   increasing version counter. Full document sync is used (the entire file text
   is sent each time).

### Diagnostics Flow

1. The language server publishes `textDocument/publishDiagnostics` notifications.
2. The reader task deserializes `PublishDiagnosticsParams`, converts the file URI
   to a local path, maps each `LspDiagnostic` to a `ForgeDiagnostic` (defaulting
   missing severity to `Warning`, missing source to `"unknown"`), and sends an
   `LspEvent::Diagnostics` through the event channel.
3. `poll_events()` drains the channel up to the budget, feeding events into
   `DiagnosticsStore::update()`.
4. `DiagnosticsStore` replaces all diagnostics for a given path on each update.
   An empty diagnostics list removes the file entry entirely.
5. `snapshot()` produces a sorted `DiagnosticsSnapshot` (files with errors first,
   then alphabetically).

### Extension Map

The extension map is built at startup from all `ServerConfig` entries. Server names
are sorted alphabetically before iteration, so when multiple servers claim the same
file extension, the alphabetically first server wins deterministically. A warning is
logged for conflicts.

## Configuration

`LspConfig` maps to the `[lsp]` section in Forge's `config.toml`:

```toml
[lsp]
enabled = true

[lsp.servers.rust]
command = "rust-analyzer"
language_id = "rust"
file_extensions = ["rs"]
root_markers = ["Cargo.toml"]

[lsp.servers.python]
command = "pyright-langserver"
args = ["--stdio"]
language_id = "python"
file_extensions = ["py", "pyi"]
root_markers = ["pyproject.toml", "setup.py"]

[lsp.servers.typescript]
command = "typescript-language-server"
args = ["--stdio"]
language_id = "typescript"
file_extensions = ["ts", "tsx"]
root_markers = ["tsconfig.json", "package.json"]
```

### ServerConfig Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `command` | Yes | -- | Server executable (must not be empty/whitespace) |
| `args` | No | `[]` | Command-line arguments |
| `language_id` | Yes | -- | LSP language identifier (must not be empty/whitespace) |
| `file_extensions` | No | `[]` | File extensions this server handles |
| `root_markers` | No | `[]` | Files/dirs that indicate project root |

### Validation

`ServerConfig` rejects invalid configurations at deserialization time:
- Empty or whitespace-only `command` produces `ServerConfigError::EmptyCommand`
- Empty or whitespace-only `language_id` produces `ServerConfigError::EmptyLanguageId`

## Usage Examples

### Starting the Manager

```rust
use forge_lsp::{LspConfig, LspManager};
use std::path::Path;

async fn start_lsp(config: LspConfig, workspace: &Path) -> LspManager {
    LspManager::start(config, workspace).await
}
```

### Notifying File Changes

```rust
use std::path::Path;
use forge_lsp::LspManager;

async fn after_tool_writes(manager: &mut LspManager) {
    let path = Path::new("/workspace/src/main.rs");
    let text = tokio::fs::read_to_string(path).await.unwrap();
    manager.on_file_changed(path, &text).await;
}
```

### Polling and Reading Diagnostics

```rust
use forge_lsp::LspManager;

fn on_tick(manager: &mut LspManager) {
    let processed = manager.poll_events(32);
    if processed > 0 {
        let snap = manager.snapshot();
        if !snap.is_empty() {
            println!("{}", snap.status_string()); // "E:3 W:5"
            for (path, diags) in snap.files() {
                for diag in diags {
                    println!("{}", diag.display_with_path(path));
                }
            }
        }
    }
}
```

### Querying Errors for Specific Files

```rust
use std::path::PathBuf;
use forge_lsp::LspManager;

fn check_errors(manager: &LspManager) {
    let paths = vec![
        PathBuf::from("src/main.rs"),
        PathBuf::from("src/lib.rs"),
    ];
    let errors = manager.errors_for_files(&paths);
    for (path, diags) in &errors {
        for diag in diags {
            eprintln!("{}", diag.display_with_path(path));
        }
    }
}
```

## Integration with Other Crates

- **`forge-engine`**: The engine holds an `Arc<Mutex<Option<LspManager>>>` and lazily starts it on the first tool batch that modifies files. `lsp_integration.rs` in the engine polls events each tick, forwards file changes after tool execution, and injects diagnostic errors as agent feedback after a 3-second delay.
- **`forge-engine` config**: `ForgeConfig` includes an optional `LspConfig` under `[lsp]` that is passed through to the manager on first use.

## Testing

```sh
cargo test -p forge-lsp
```

The crate has comprehensive unit tests across all modules:

- **`types`**: Severity conversion, diagnostic formatting, snapshot counting, config deserialization and validation
- **`protocol`**: JSON-RPC request/notification serialization, diagnostic wire-format conversion, URI roundtripping
- **`codec`**: Frame read/write roundtrips, multi-frame sequences, EOF handling, missing/invalid headers, oversized frames, case-insensitive headers, multibyte UTF-8
- **`server`**: Frame dispatch routing (responses to pending map, diagnostics to event channel, server requests get method-not-found replies, unknown notifications ignored)
- **`diagnostics`**: Store CRUD, empty-clears-file, error-first sorting, errors-for-files filtering, status string formatting
- **`manager`**: Extension map construction, overlap determinism, event polling with budget, deferred diagnostics clearing, server-stopped cleanup

No external servers are needed for tests. Server tests use channel-based mocks rather than spawning real language servers.

## Design Principles

### Lazy Initialization

The `LspManager` is not created at application startup. It is constructed on first
use (when a tool batch modifies files), avoiding the cost of spawning language servers
for sessions that never edit code.

### Non-Blocking Event Polling

`poll_events()` uses `try_recv()` with a budget parameter, making it safe to call
from the main TUI tick loop without blocking the render thread. The event channel
capacity is 256, providing backpressure if events arrive faster than they are polled.

### Graceful Degradation

Server startup failures are logged as warnings but do not abort the manager. If a
server crashes mid-session, a `ServerStopped` event removes it from the active map
without affecting other running servers. The engine checks `has_running_servers()`
before waiting for deferred diagnostics.

### Invariant-First Boundaries

`ServerConfig` validates at the deserialization boundary (via `TryFrom`) rather than
at use sites. The `source` field on `ForgeDiagnostic` is always a concrete `String`
(defaulting `None` to `"unknown"`) to eliminate `Option` handling in the core --
following IFA section 11.2 as noted in the source.

### Deterministic Conflict Resolution

When multiple servers claim the same file extension, the alphabetically first server
name wins. This is achieved by sorting server names before building the extension map,
ensuring reproducible behavior regardless of `HashMap` iteration order.
