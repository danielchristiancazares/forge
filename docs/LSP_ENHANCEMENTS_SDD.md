# LSP Auto-Detection — Software Design Document

## 1. Overview

### 1.1 Purpose

This document specifies the design for automatic language server detection in Forge's LSP subsystem. The goal is to eliminate manual `[lsp.servers.*]` configuration for common language servers when `enabled = true` is set.

### 1.2 Scope

- Auto-detection of known language servers based on workspace root markers and PATH availability
- Programmatic `ServerConfig` construction for detected servers
- Integration into the existing lazy-start lifecycle
- Documentation of canonical server names, behavior, and limitations

### 1.3 Out of Scope

- Per-extension `language_id` routing (future improvement)
- Ancestor directory walking for nested projects
- `node_modules/.bin` or non-PATH binary resolution
- Runtime server capability negotiation

## 2. Background

### 2.1 Current State

LSP is configured via `~/.forge/config.toml`:

```toml
[lsp]
enabled = true
[lsp.servers.rust]
command = "rust-analyzer"
language_id = "rust"
file_extensions = ["rs"]
root_markers = ["Cargo.toml"]
```

Users must specify `command`, `language_id`, `file_extensions`, and optionally `root_markers` and `args` for every server. This requires knowledge of LSP protocol internals.

### 2.2 Problem Statement

The configuration burden prevents adoption. A user who has `rust-analyzer` installed and is working in a Rust project should get diagnostics with `enabled = true` and nothing else.

### 2.3 Desired Behavior

| Config | Behavior |
|--------|----------|
| `[lsp]` absent or `enabled = false` | No LSP. No change. |
| `enabled = true`, no `[lsp.servers.*]` | Auto-detect known servers from workspace + PATH |
| `enabled = true`, explicit `[lsp.servers.*]` | Use explicit config only. No auto-detection. |

## 3. Design

### 3.1 Known Server Registry

A compile-time catalog of well-known language servers, defined in `lsp/src/detection.rs`:

| Canonical Name | Command | Args | Language ID | File Extensions | Root Markers |
|----------------|---------|------|-------------|-----------------|--------------|
| `rust` | `rust-analyzer` | `[]` | `rust` | `rs` | `Cargo.toml` |
| `python` | `pyright-langserver` | `["--stdio"]` | `python` | `py`, `pyi` | `pyproject.toml`, `setup.py`, `requirements.txt` |
| `typescript` | `typescript-language-server` | `["--stdio"]` | `typescript` | `ts`, `tsx`, `js`, `jsx` | `package.json`, `tsconfig.json` |
| `go` | `gopls` | `[]` | `go` | `go` | `go.mod` |
| `c_cpp` | `clangd` | `[]` | `cpp` | `c`, `cpp`, `cc`, `h`, `hpp` | `CMakeLists.txt`, `compile_commands.json` |

**Design notes:**

- `pyright-langserver` is the LSP entrypoint from the `pyright` npm package (not `pyright` itself, which is the CLI checker).
- `typescript-language-server` handles both TypeScript and JavaScript despite sending `languageId: "typescript"` for all files. This is a known limitation of the per-server language_id model (§3.5).
- `clangd` sends `languageId: "cpp"` for C files. This works in practice; clangd infers the actual language from file extension regardless.
- `compile_commands.json` often lives under `build/` rather than project root; `CMakeLists.txt` is the more reliable root marker.

### 3.2 Detection Algorithm

```
detect_servers(workspace_root, binary_exists_probe):
    results = {}
    for server in KNOWN_SERVERS:
        has_marker = any(workspace_root.join(marker).exists() for marker in server.root_markers)
        has_binary = binary_exists_probe(server.command)
        if has_marker and has_binary:
            results[server.name] = ServerConfig::new(...)
    return results
```

The `binary_exists_probe` parameter enables testing without depending on the host machine's PATH. The production implementation uses `which::which()`.

### 3.3 Integration Point

Detection runs at lazy-start time in `engine/src/lsp_integration.rs`, immediately before `LspManager::start()`. This is the earliest point where `workspace_root` is available:

```rust
// lsp_integration.rs:103 — inside tokio::spawn block
if let Some(config) = config {
    let config = config.with_auto_detected(&workspace_root);
    let mgr = forge_lsp::LspManager::start(config, &workspace_root).await;
    // ...
}
```

`LspConfig::with_auto_detected()` only runs detection when `self.servers.is_empty()`. If the user has any explicit server entries, detection is skipped entirely.

### 3.4 ServerConfig Programmatic Construction

Currently `ServerConfig` can only be created via serde deserialization (`TryFrom<RawServerConfig>`). A programmatic constructor is needed for auto-detected configs.

To avoid duplicating invariant checks (IFA §7), validation is extracted into a shared function:

```rust
fn build_validated(
    command: String, args: Vec<String>, language_id: String,
    file_extensions: Vec<String>, root_markers: Vec<String>,
) -> Result<ServerConfig, ServerConfigError>
```

Both `TryFrom<RawServerConfig>` and `ServerConfig::new()` delegate to this function. Builder methods (`with_args`, `with_file_extensions`, `with_root_markers`) allow setting optional fields after construction.

### 3.5 Known Limitations

| Limitation | Impact | Mitigation |
|------------|--------|------------|
| **PATH-only detection** | Node-installed servers in `node_modules/.bin` not found | User adds to PATH or uses explicit config |
| **Workspace root only** | Nested projects (monorepo subdirs) not detected | Acceptable for v1; ancestor walking is future work |
| **One language_id per server** | `typescript-language-server` sends "typescript" for .js files; `clangd` sends "cpp" for .c files | Servers handle this correctly in practice; per-extension language_id routing is future work |
| **Windows .cmd shims** | Node servers install as `.cmd` on Windows | `which::which()` resolves `.cmd` extensions; `tokio::process::Command` executes them |

## 4. Detailed Changes

### 4.1 File: `Cargo.toml` (workspace root)

Add `which = "7"` to `[workspace.dependencies]`.

### 4.2 File: `lsp/Cargo.toml`

Add `which.workspace = true` to `[dependencies]`. Add `tempfile.workspace = true` to `[dev-dependencies]`.

### 4.3 File: `engine/Cargo.toml`

Change `which = "7"` to `which.workspace = true`.

### 4.4 File: `lsp/src/types.rs`

- Extract `build_validated()` from `TryFrom<RawServerConfig>::try_from()`
- Add `ServerConfig::new(command, language_id) -> Result<Self, ServerConfigError>`
- Add builder methods: `with_args()`, `with_file_extensions()`, `with_root_markers()`
- Add `LspConfig::with_auto_detected(self, workspace_root: &Path) -> Self`
- New tests: constructor validation, builder methods, auto-detection skip when servers present

### 4.5 File: `lsp/src/detection.rs` (new)

- `KnownServer` struct with static string slices
- `KNOWN_SERVERS` const slice
- `detect_servers(workspace_root) -> HashMap<String, ServerConfig>` (production wrapper)
- `detect_servers_with(workspace_root, probe)` (testable core)
- Tests using tempdir + injectable probe

### 4.6 File: `lsp/src/lib.rs`

Add `mod detection;`.

### 4.7 File: `engine/src/lsp_integration.rs`

One-line addition at line 104: `let config = config.with_auto_detected(&workspace_root);`

### 4.8 File: `CLAUDE.md`

Update LSP configuration section with:
- Auto-detection behavior
- Canonical server names for override
- Known limitations

## 5. Testing Strategy

### 5.1 Unit Tests — `lsp/src/detection.rs`

| Test | Setup | Expected |
|------|-------|----------|
| `detect_finds_server_when_marker_and_binary_present` | tempdir with `Cargo.toml`; probe true for `rust-analyzer` | Map contains `"rust"` entry |
| `detect_skips_when_no_markers` | empty tempdir; probe always true | Empty map |
| `detect_skips_when_binary_missing` | tempdir with `Cargo.toml`; probe always false | Empty map |
| `detect_multiple_servers` | tempdir with `Cargo.toml` + `go.mod`; probe true for both | Map contains `"rust"` and `"go"` |

### 5.2 Unit Tests — `lsp/src/types.rs`

| Test | Expected |
|------|----------|
| `server_config_new_valid` | Succeeds with correct fields |
| `server_config_new_rejects_empty_command` | `Err(EmptyCommand)` |
| `server_config_new_rejects_empty_language_id` | `Err(EmptyLanguageId)` |
| `builder_methods` | Fields set correctly |
| `with_auto_detected_skips_when_servers_present` | Explicit servers unchanged |
| `with_auto_detected_safe_on_disabled_config` | No panic, no servers added |

### 5.3 Edge Cases

- `enabled = true`, no servers detected (no markers, no binaries): `LspManager::start()` with empty servers → `has_running_servers()` returns false → deferred diag check bails at `lsp_integration.rs:43`. Already handled by existing code.

## 6. Verification

```bash
just verify  # cargo fmt + clippy --workspace -D warnings + cargo test
```
