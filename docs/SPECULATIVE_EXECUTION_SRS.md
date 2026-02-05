# Speculative Execution

## Software Requirements Document

**Version:** 1.1
**Date:** 2026-02-05
**Status:** Draft

## LLM-TOC
<!-- Auto-generated section map for LLM context — will be re-indexed after final edits -->
| Section | Description |
|---------|-------------|
| 0 | Change Log |
| 1 | Introduction |
| 2 | Overall Description |
| 3 | Functional Requirements |
| 4 | Non-Functional Requirements |
| 5 | Data Structures |
| 6 | Worktree Layout |
| 7 | State Transitions |
| 8 | Implementation Checklist |
| 9 | Verification Requirements |
| 10 | Future Considerations |
| A | Appendix A: Future SRS — Architecture Lint System |
| B | Appendix B: IFA Conformance Notes |

---

## 0. Change Log

### 0.1 Initial draft

* Initial requirements for speculative execution during plan review.

### 0.2 IFA conformance remediation

* **V1 — WorktreeStatus string tag → discriminated union:** Replaced `status: String` tag field + conditionally-valid `files: Vec<String>` with `WorktreeStatusKind` enum where each variant owns exactly the data valid for that state (IFA §9.2, §13.1, §14.2).
* **V2 — Speculative model resolution mechanism/policy separation:** Replaced FR-SE-30's silent default selection with `SpeculativeModelResolution` enum that structurally distinguishes user-configured, inferred-default, and unavailable outcomes. Policy decisions remain with the caller (IFA §8.1).
* **V3 — Phantom state diagram alignment:** Removed "Discarded", "Deleted", and "Promoted" as named boxes in the state diagram. These are transitions back to `Idle`, not distinct representable states. Diagram now matches the `SpeculativeState` enum exactly (IFA §9.3, §9.4).
* **W1 — SpeculativeFile.path proof-carrying type:** Replaced raw `String` with `RelativeProjectPath` newtype that validates the path is relative, normalized, and within project bounds at the boundary (IFA §11.1, §11.2).
* **W2 — SpeculativeState::Failed missing name:** Added `name: String` to `Failed` variant for consistent cleanup logging. All non-Idle variants now carry the worktree identity uniformly.
* Added IFA conformance notes appendix (Appendix B).

---

## 1. Introduction

### 1.1 Purpose

Define requirements for speculative execution: running a fast, cheap model to scaffold implementation code *while the user reviews a plan*, so approved plans have an immediate head start.

### 1.2 Scope

The Speculative Execution feature will:

* Launch a background coding pass using a fast model (Sonnet/Haiku) during plan review
* Write speculative code into an isolated worktree directory
* On plan approval, feed speculative code to the primary model as a starting point
* On plan rejection, discard the worktree with zero side effects

Out of scope:

* Automatic merging of speculative code without primary model review
* Multi-plan speculative execution (v1 supports one active speculation per session)
* Speculative execution for non-plan workflows (e.g., direct user prompts)

### 1.3 Definitions

| Term | Definition |
| --- | --- |
| Speculative pass | A background coding attempt by a fast model based on a plan |
| Worktree | An isolated directory containing speculative code output |
| Worktree name | Human-readable identifier: `<color>-<emotion>-<animal>-<timestamp>` |
| Primary model | The user's configured model (e.g., Opus 4.6) used for final execution |
| Speculative model | A fast, cheap model (e.g., Sonnet 4.5, Haiku 4.5) used for speculative coding |
| Promotion | The act of feeding speculative code to the primary model after approval |
| RelativeProjectPath | Proof-carrying newtype over `String` that guarantees the path is relative, normalized (no `..`), and within project bounds. Constructed only at the parse boundary. |
| SpeculativeModelResolution | Sum type that structurally distinguishes how the speculative model was determined: user-configured, inferred from available keys, or unavailable. |

### 1.4 References

| Document | Description |
| --- | --- |
| `engine/src/lib.rs` | App state machine, `OperationState` |
| `engine/src/streaming.rs` | Streaming API call orchestration |
| `engine/src/config.rs` | `ForgeConfig`, `AnthropicConfig` |
| `providers/src/lib.rs` | `ApiConfig`, multi-provider dispatch |
| `context/src/model_limits.rs` | Per-model token limits |
| `tui/src/lib.rs` | Full-screen rendering |
| `INVARIANT_FIRST_ARCHITECTURE.md` | IFA design constraints — conformance target for all data structures |

### 1.5 Requirement Keywords

The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** are as defined in RFC 2119.

---

## 2. Overall Description

### 2.1 Product Perspective

Speculative Execution is a Plan Mode enhancement. When the user enters plan mode and the primary model produces a plan, a secondary model begins coding speculatively in the background. The user reviews the plan as normal. If approved, the speculative output accelerates the primary model's execution pass. If rejected, nothing happened.

The key UX insight: the user perceives plan approval as instant implementation because the speculative pass ran concurrently with their review time.

### 2.2 Product Functions

| Function | Description |
| --- | --- |
| FR-SE-LAUNCH | Launch speculative pass when plan is finalized |
| FR-SE-WORKTREE | Create and manage isolated worktree directories |
| FR-SE-CODE | Run speculative model to produce semi-code from plan |
| FR-SE-PROMOTE | Feed speculative output to primary model on approval |
| FR-SE-DISCARD | Clean up worktree on rejection or session end |
| FR-SE-STATUS | Show speculative execution progress in UI |
| FR-SE-CONFIG | Configure speculative model, enable/disable feature |

### 2.3 User Characteristics

* Users who use plan mode for non-trivial changes
* Users who review plans for 30s-5min (the speculative window)
* Users who value speed over token cost

### 2.4 Constraints

* Speculative code does NOT need to compile, pass linting, or be correct
* Speculative code SHOULD capture structure, logic flow, and intent
* Speculative model MUST be cheaper than primary model (cost efficiency)
* Worktree MUST NOT modify the user's working directory or git state
* Speculative pass MUST be cancellable if user rejects plan quickly

### 2.5 Design Philosophy

The speculative pass is a **rough draft**, not a finished product. Think of it as a senior architect (Opus) handing a sketch to a junior dev (Sonnet) who codes it up quickly. The architect then reviews and cleans up. The junior's code doesn't need to be perfect — it needs to be *directionally correct* so the architect spends time editing, not writing from scratch.

---

## 3. Functional Requirements

### 3.1 Speculative Launch

**FR-SE-01:** When the user calls `ExitPlanMode` and the plan is finalized (written to plan file), the system MUST check if speculative execution is enabled.

**FR-SE-02:** If enabled, the system MUST immediately spawn a background task that:

1. Creates a worktree directory
2. Copies relevant source files from the working directory
3. Sends the plan + copied file contents to the speculative model
4. Writes the model's output into the worktree

**FR-SE-03:** The speculative task MUST run concurrently with the plan approval prompt. The user MUST NOT be blocked.

**FR-SE-04:** The speculative task MUST use a separate `ApiConfig` with the speculative model, NOT the user's primary model.

**FR-SE-05:** The speculative task SHOULD use a minimal system prompt optimized for speed:

```
You are a fast code scaffolder. Given a plan and source files, produce the modified files.
Focus on structure and logic. Do not worry about imports, formatting, or compilation.
Output each file as: === PATH ===\n<content>\n
```

**FR-SE-06:** The speculative task MUST include the full plan text and the contents of all files listed in the plan's "Files Modified" section.

### 3.2 Worktree Management

**FR-SE-07:** Worktrees MUST be created under `~/.forge/worktrees/`.

**FR-SE-08:** Worktree names MUST follow the pattern `<color>-<emotion>-<animal>-<unix_timestamp>`.

**FR-SE-09:** Name components MUST be randomly selected from curated word lists:

| Component | Examples |
| --- | --- |
| Color | amber, cobalt, crimson, jade, ivory, violet, slate, copper, teal, rust |
| Emotion | calm, bold, swift, keen, warm, fierce, gentle, sharp, bright, steady |
| Animal | falcon, orca, lynx, raven, cobra, mantis, heron, viper, condor, wolf |

**FR-SE-10:** The system MUST create the worktree directory with the structure:

```
~/.forge/worktrees/calm-violet-orca-1738762800/
  plan.md          # Copy of the plan
  status.json      # Speculative pass metadata
  files/           # Speculative file outputs (flat or mirroring project structure)
```

**FR-SE-11:** The system MUST copy source files into the worktree before sending to the speculative model. These copies serve as the baseline for diffing later.

**FR-SE-12:** The system SHOULD mirror the project's directory structure under `files/` for clarity.

### 3.3 Speculative Coding

**FR-SE-13:** The speculative model MUST receive:

* The plan text (from the plan file)
* Contents of each file referenced in the plan
* A terse system prompt (FR-SE-05)

**FR-SE-14:** The speculative model's output MUST be parsed into individual file outputs. The expected format:

```
=== src/engine/config.rs ===
<file content>

=== src/providers/lib.rs ===
<file content>
```

**FR-SE-15:** Parsed file outputs MUST be written to the worktree's `files/` directory.

**FR-SE-16:** If the speculative model's output cannot be parsed (malformed separators, empty output), the system MUST log a warning and mark the speculation as failed. This is NOT an error shown to the user.

**FR-SE-17:** The speculative pass SHOULD complete within the user's review time. A hard timeout of 120 seconds MUST be enforced.

**FR-SE-18:** The speculative pass MUST be abortable via `AbortHandle` if the user rejects the plan or exits plan mode before it completes.

### 3.4 Plan Approval (Promotion)

**FR-SE-19:** When the user approves a plan, the system MUST pattern-match on `SpeculativeState` and act according to the variant held:

| `SpeculativeState` variant | Action |
| --- | --- |
| `Ready { files, .. }` | Promote speculative output, transition to `Idle` |
| `Running { .. }` | Wait up to 10s for completion; if completes → promote, else → proceed without |
| `Failed { .. }` | Proceed without speculative output (normal execution), transition to `Idle` |
| `Idle` | Proceed without speculative output (no speculation was active) |

There is no "Discarded" variant. Abort/reject transitions `Running` → `Idle` (see §3.5). By the time the user approves, the state is one of the four variants above.

**FR-SE-20:** Promotion MUST inject the speculative output into the primary model's context as reference material. The injection format:

```
## Speculative Draft

A fast model pre-generated the following draft based on your plan.
Review and correct this code — it may have import errors, type mismatches,
or incomplete logic. Use it as a starting point, not a finished product.

### src/engine/config.rs (speculative)
\`\`\`rust
<speculative content>
\`\`\`

### src/providers/lib.rs (speculative)
\`\`\`rust
<speculative content>
\`\`\`
```

**FR-SE-21:** The primary model MUST still read the actual source files. Speculative output is advisory, not authoritative.

**FR-SE-22:** After promotion (or decision to skip), the worktree SHOULD be marked for cleanup.

### 3.5 Plan Rejection (Discard)

**FR-SE-23:** When the user rejects a plan, any active speculative pass MUST be aborted immediately.

**FR-SE-24:** The worktree MUST be deleted on rejection.

**FR-SE-25:** Aborting a speculative pass MUST NOT produce any user-visible error or notification beyond a debug-level log.

### 3.6 Cleanup

**FR-SE-26:** On application startup, the system SHOULD scan `~/.forge/worktrees/` and delete any worktrees older than 24 hours. Stale worktrees indicate crashed sessions.

**FR-SE-27:** On normal session exit, any active worktree MUST be deleted.

**FR-SE-28:** The `/clear` command MUST abort any active speculative pass and delete the worktree.

### 3.7 Configuration

**FR-SE-29:** Speculative execution MUST be configurable via `config.toml`:

```toml
[plan]
speculative_execution = true          # Enable/disable (default: false for v1)
speculative_model = "claude-sonnet-4-5-20250929"  # Model to use
```

**FR-SE-30:** Speculative model resolution MUST be performed at the init boundary and MUST produce a `SpeculativeModelResolution` value that structurally distinguishes how the model was determined:

```rust
/// Result of resolving which model to use for speculative execution.
/// Mechanism: reports facts. Policy (whether to proceed) belongs to the caller.
pub enum SpeculativeModelResolution {
    /// User explicitly configured a speculative model in config.toml.
    Configured(ModelName),
    /// No model configured; inferred a default from the available API key.
    InferredDefault {
        model: ModelName,
        /// Which provider key was detected.
        provider: Provider,
    },
    /// No suitable model could be determined (no keys, or primary == speculative).
    Unavailable {
        reason: SpeculativeUnavailableReason,
    },
}

/// Why speculative execution cannot proceed.
pub enum SpeculativeUnavailableReason {
    /// No API key available for any speculative-capable provider.
    NoApiKey,
    /// The resolved speculative model is the same as the primary model.
    SameAsPrimary,
}
```

The resolver (mechanism) MUST NOT silently fall back or disable the feature. It reports the resolution outcome. The caller (policy) decides whether to proceed, warn, or disable.

**FR-SE-30a:** The default inference table, used only when the user has not configured `speculative_model`:

| Available Keys | Default Speculative Model |
| --- | --- |
| Anthropic | `claude-sonnet-4-5-20250929` |
| OpenAI | `gpt-5.2` |
| Gemini | `gemini-3-pro-preview` |

**FR-SE-31:** If `SpeculativeModelResolution::Unavailable` is returned, the init boundary (policy) MUST set `SpeculativeState::Idle` and log the reason at `debug` level. The feature is effectively disabled for that session. No user-visible error is produced.

**FR-SE-32:** The resolver MUST return `Unavailable { reason: SameAsPrimary }` when the resolved speculative model equals the primary model. This check applies to both `Configured` and `InferredDefault` paths.

### 3.8 UI Feedback

**FR-SE-33:** While a speculative pass is running, the status bar SHOULD display a subtle indicator:

```
Context 96% left  Tokens 70k in / 291 out (94% cached)  [spec: scaffolding...]
```

**FR-SE-34:** When a speculative pass completes, the indicator SHOULD update:

```
[spec: ready]
```

**FR-SE-35:** When promoting speculative output, the system SHOULD display a notification:

```
Speculative draft from calm-violet-orca injected as reference.
```

**FR-SE-36:** The notification MUST NOT imply the speculative code is correct or final.

---

## 4. Non-Functional Requirements

### 4.1 Performance

| Requirement | Specification |
| --- | --- |
| NFR-SE-PERF-01 | Worktree creation MUST complete in <500ms |
| NFR-SE-PERF-02 | File copy into worktree MUST complete in <2s for projects up to 1000 files |
| NFR-SE-PERF-03 | Speculative pass MUST NOT degrade main thread UI responsiveness |
| NFR-SE-PERF-04 | Speculative model timeout MUST be 120s max |

### 4.2 Cost

| Requirement | Specification |
| --- | --- |
| NFR-SE-COST-01 | Speculative pass SHOULD use a model at most 1/5th the cost of the primary model |
| NFR-SE-COST-02 | Speculative pass input SHOULD be minimized (only plan + referenced files, not full context) |
| NFR-SE-COST-03 | Speculative pass SHOULD use max_tokens ≤ 16K to limit output cost |

### 4.3 Reliability

| Requirement | Specification |
| --- | --- |
| NFR-SE-REL-01 | Speculative pass failure MUST NOT affect plan mode or primary execution |
| NFR-SE-REL-02 | Worktree cleanup MUST be crash-safe (startup scan handles orphans) |
| NFR-SE-REL-03 | AbortHandle MUST reliably cancel in-flight API calls |

### 4.4 Security

| Requirement | Specification |
| --- | --- |
| NFR-SE-SEC-01 | Speculative model MUST NOT receive API keys, secrets, or .env contents |
| NFR-SE-SEC-02 | Worktree directory permissions MUST match user's umask |
| NFR-SE-SEC-03 | File contents sent to speculative model MUST pass the same secret redaction as primary model |

---

## 5. Data Structures

### 5.1 RelativeProjectPath (proof-carrying path)

```rust
/// A relative file path guaranteed to be:
/// - Non-empty
/// - Relative (no leading `/` or drive letter)
/// - Normalized (no `..` components, no `.` components)
/// - Within project bounds (does not escape the project root)
///
/// Constructed only at the parse boundary (`parse_speculative_output`).
/// Core code accepts this type, never raw `String` paths.
///
/// Authority Boundary: `RelativeProjectPath::new()` — the sole constructor.
pub struct RelativeProjectPath(String);

impl RelativeProjectPath {
    /// Validate and construct. Returns `None` if the path is empty, absolute,
    /// contains `..` components, or is otherwise invalid.
    pub fn new(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let path = std::path::Path::new(trimmed);
        if path.is_absolute() {
            return None;
        }
        // Reject path traversal
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => return None,
                std::path::Component::Prefix(_) => return None, // Windows drive letters
                _ => {}
            }
        }
        Some(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_path(&self) -> &std::path::Path {
        std::path::Path::new(&self.0)
    }
}

impl std::fmt::Display for RelativeProjectPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
```

**IFA note (§2.1, §11.1):** Core functions that consume speculative file paths accept `RelativeProjectPath`, not `String`. The boundary (`parse_speculative_output`) performs validation and produces this proof. If a path from the speculative model is invalid, it is rejected at the boundary — the core never sees it.

### 5.2 SpeculativeState

```rust
/// Active speculative execution state, held in App during plan review.
///
/// IFA §9: Each variant is a distinct domain state. Each variant owns exactly
/// the data valid for that state. There are no tag fields, no conditionally-valid
/// fields, and no phantom states. The type IS the state.
///
/// Transitions:
///   Idle → Running  (plan finalized + speculative enabled)
///   Running → Ready (model completes successfully)
///   Running → Failed (error, timeout, parse failure)
///   Running → Idle  (user rejects plan / abort)
///   Ready → Idle    (promoted or cleaned up)
///   Failed → Idle   (cleaned up)
pub enum SpeculativeState {
    /// No active speculation. Initial state and terminal state after
    /// cleanup/promotion/abort. This is the only state with no worktree.
    Idle,

    /// Speculative pass is running in a background task.
    Running {
        /// Worktree directory path.
        worktree: PathBuf,
        /// Human-readable worktree name (e.g., "calm-violet-orca").
        name: String,
        /// Handle to abort the speculative task.
        abort_handle: AbortHandle,
        /// Timestamp when speculation started.
        started_at: Instant,
    },

    /// Speculative pass completed successfully. Files are parsed and ready
    /// for promotion into the primary model's context.
    Ready {
        /// Worktree directory path.
        worktree: PathBuf,
        /// Human-readable worktree name.
        name: String,
        /// Parsed file outputs from the speculative model.
        files: Vec<SpeculativeFile>,
    },

    /// Speculative pass failed (timeout, parse error, API error).
    /// Worktree exists on disk and must be cleaned up.
    Failed {
        /// Worktree directory path (for cleanup).
        worktree: PathBuf,
        /// Human-readable worktree name (for logging during cleanup).
        name: String,
    },
}
```

**IFA §9.3 domain enumeration test:** The domain states are: *not speculating* (Idle), *speculating in progress* (Running), *speculation succeeded* (Ready), *speculation failed* (Failed). Each maps to exactly one variant. There are no variants without domain names. "Promoted", "Discarded", and "Deleted" are transitions that return the state to `Idle`, not distinct states.

### 5.3 SpeculativeFile

```rust
/// A single file output from the speculative model.
///
/// `path` is a `RelativeProjectPath` (proof-carrying), not a raw `String`.
/// Constructed at the parse boundary; core code can trust the path is valid.
pub struct SpeculativeFile {
    /// Validated relative path within the project (e.g., "src/engine/config.rs").
    pub path: RelativeProjectPath,
    /// Speculative content (may not compile, may have placeholder imports).
    pub content: String,
}
```

### 5.4 WorktreeStatus (status.json)

```rust
/// Persisted worktree metadata for crash recovery and cleanup.
///
/// IFA §9.2: Status is a discriminated union (`WorktreeStatusKind`), not a
/// string tag. Each variant owns exactly the data valid for that lifecycle
/// phase. The `files` list exists only in the `Completed` variant — it is
/// structurally impossible to have files in a "running" or "failed" status.
///
/// Common metadata is factored into `WorktreeMeta` (IFA §7.5: shared
/// invariants composed, not copied).
#[derive(Serialize, Deserialize)]
pub struct WorktreeStatus {
    /// Common metadata shared across all status phases.
    #[serde(flatten)]
    pub meta: WorktreeMeta,
    /// Current lifecycle status (discriminated union).
    pub status: WorktreeStatusKind,
}

/// Metadata common to all worktree lifecycle phases.
/// Factored out to satisfy IFA §7.2 (single canonical encoding).
#[derive(Serialize, Deserialize)]
pub struct WorktreeMeta {
    /// Worktree name (e.g., "calm-violet-orca-1738762800").
    pub name: String,
    /// Unix timestamp when created.
    pub created_at: u64,
    /// Plan file path that triggered this speculation.
    pub plan_path: String,
    /// Speculative model used.
    pub model: String,
}

/// Lifecycle status of a persisted worktree.
///
/// IFA §9.2, §14.2: Each variant owns the data valid for that phase.
/// Changing the variant changes which fields exist — this is a discriminated
/// union, not a tag field.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WorktreeStatusKind {
    /// Speculative pass is still running (or the process crashed mid-run).
    Running,
    /// Speculative pass completed. `files` lists the produced file paths.
    Completed {
        /// Relative paths of files produced by the speculative model.
        files: Vec<String>,
    },
    /// Speculative pass failed (timeout, API error, parse error).
    Failed,
}
```

**IFA §14.2 memory layout test:** If you change `WorktreeStatusKind` from `Running` to `Completed`, the struct gains a `files` field. If you change from `Completed` to `Failed`, the `files` field disappears. The data changes with the state — conforming.

**Serialization example (`status.json`):**

```json
{
    "name": "calm-violet-orca-1738762800",
    "created_at": 1738762800,
    "plan_path": "~/.claude/plans/sharded-conjuring-blanket.md",
    "model": "claude-sonnet-4-5-20250929",
    "status": {
        "kind": "completed",
        "files": ["src/engine/config.rs", "src/providers/lib.rs"]
    }
}
```

### 5.5 SpeculativeModelResolution

```rust
/// Result of resolving which model to use for speculative execution.
///
/// IFA §8.1: This is a mechanism — it reports facts about model resolution.
/// It does NOT silently choose a fallback. The caller (policy) decides
/// whether to proceed, warn, or disable based on the variant returned.
///
/// Authority Boundary: `resolve_speculative_model()` in init.rs.
pub enum SpeculativeModelResolution {
    /// User explicitly set `speculative_model` in config.toml.
    Configured(ModelName),

    /// No model configured; inferred a default from the available API key.
    InferredDefault {
        model: ModelName,
        /// Which provider key was detected.
        provider: Provider,
    },

    /// No suitable model could be determined.
    Unavailable {
        reason: SpeculativeUnavailableReason,
    },
}

/// Why speculative execution cannot proceed.
pub enum SpeculativeUnavailableReason {
    /// No API key available for any speculative-capable provider.
    NoApiKey,
    /// The resolved speculative model is the same as the primary model.
    SameAsPrimary,
}
```

### 5.6 PlanConfig

```rust
/// Plan mode configuration.
#[derive(Debug, Default, Deserialize)]
pub struct PlanConfig {
    /// Enable speculative execution during plan review. Default: false.
    #[serde(default)]
    pub speculative_execution: bool,
    /// Model to use for speculative coding. If absent, the system infers
    /// a default from available API keys (see SpeculativeModelResolution).
    pub speculative_model: Option<String>,
}
```

**IFA note:** `Option<String>` is acceptable here because `PlanConfig` is boundary code (config deserialization). The `Option` is resolved into `SpeculativeModelResolution` at the init boundary before any core code runs. Core code never sees `Option<String>` — it receives either a `ModelName` (from `Configured` or `InferredDefault`) or does not start speculation (from `Unavailable`).

---

## 6. Worktree Layout

### 6.1 Directory Structure

```
~/.forge/
  worktrees/
    calm-violet-orca-1738762800/
      plan.md                       # Copy of the approved plan
      status.json                   # Metadata (WorktreeStatus with discriminated WorktreeStatusKind)
      baseline/                     # Original file copies (for diffing)
        src/
          engine/
            config.rs
          providers/
            lib.rs
      files/                        # Speculative model output
        src/
          engine/
            config.rs               # Modified by speculative model
          providers/
            lib.rs                  # Modified by speculative model
```

### 6.2 Name Generation

```rust
fn generate_worktree_name() -> String {
    let colors = ["amber", "cobalt", "crimson", "jade", "ivory",
                   "violet", "slate", "copper", "teal", "rust"];
    let emotions = ["calm", "bold", "swift", "keen", "warm",
                     "fierce", "gentle", "sharp", "bright", "steady"];
    let animals = ["falcon", "orca", "lynx", "raven", "cobra",
                    "mantis", "heron", "viper", "condor", "wolf"];

    let mut rng = rand::rng();
    format!("{}-{}-{}-{}",
        colors.choose(&mut rng).unwrap(),
        emotions.choose(&mut rng).unwrap(),
        animals.choose(&mut rng).unwrap(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap().as_secs()
    )
}
```

---

## 7. State Transitions

```
                         ┌──────────┐
              ┌──────────│   Idle   │◄──────────────────────┐
              │          └────┬─────┘                        │
              │               │ Plan finalized +             │
              │               │ speculative enabled          │
              │               ▼                              │
              │          ┌──────────┐                        │
              │   Abort  │ Running  │  Model completes       │
              ├──────────┤          ├───────────┐            │
              │          └────┬─────┘           │            │
              │               │ Error/          │            │
              │               │ Timeout         │            │
              │               ▼                 ▼            │
              │          ┌──────────┐     ┌──────────┐      │
              │          │  Failed  │     │  Ready   │      │
              │          └────┬─────┘     └────┬─────┘      │
              │               │                │             │
              │               │ Cleanup        │ Promote     │
              │               │                │ + Cleanup   │
              │               └────────┬───────┘             │
              │                        │                     │
              └────────────────────────┴─────────────────────┘
```

**IFA §9.3 alignment:** Every box in this diagram corresponds to exactly one variant of `SpeculativeState`. There are four boxes and four variants: `Idle`, `Running`, `Ready`, `Failed`. All transitions terminate at `Idle`. There are no phantom states — "promoted", "discarded", and "deleted" are *actions* performed during transitions, not states the system occupies.

### 7.1 Trigger: Plan Finalized

When `ExitPlanMode` is called and the plan file exists:

1. Parse plan file for "Files Modified" section
2. If speculative execution enabled AND files identified:
   a. Create worktree
   b. Copy baseline files
   c. Spawn background API call to speculative model
   d. Transition: `Idle` → `Running`
3. If disabled or no files identified: remain `Idle`

### 7.2 Trigger: User Approves Plan

Pattern-match on `SpeculativeState`:

1. `Ready { files, .. }` → inject speculative output into primary model context, delete worktree, transition: `Ready` → `Idle`
2. `Running { abort_handle, .. }` → wait up to 10s; if transitions to `Ready`, promote per (1); else abort, delete worktree, transition: `Running` → `Idle`
3. `Failed { worktree, .. }` → delete worktree, transition: `Failed` → `Idle`, proceed without speculative output
4. `Idle` → proceed without speculative output (no speculation was active)

Primary execution begins (normal flow) after the match completes.

### 7.3 Trigger: User Rejects Plan / `/clear`

1. Pattern-match on `SpeculativeState`:
   - `Running { abort_handle, worktree, .. }` → abort via `abort_handle`, delete `worktree`
   - `Ready { worktree, .. }` / `Failed { worktree, .. }` → delete `worktree`
   - `Idle` → no-op
2. Transition: any variant → `Idle`

---

## 8. Implementation Checklist

### 8.1 Config (`engine/src/config.rs`)

- [ ] Add `PlanConfig` struct with `speculative_execution` and `speculative_model`
- [ ] Add `plan: Option<PlanConfig>` to `ForgeConfig`
- [ ] Add config parsing test

### 8.2 Proof-Carrying Types (new: `engine/src/speculative/types.rs`)

- [ ] `RelativeProjectPath` newtype with `new()` constructor (sole Authority Boundary)
- [ ] Reject: empty, absolute, `..` components, Windows drive prefixes
- [ ] `Display` impl for markdown rendering
- [ ] `SpeculativeFile` struct with `path: RelativeProjectPath`
- [ ] `SpeculativeModelResolution` enum (`Configured`, `InferredDefault`, `Unavailable`)
- [ ] `SpeculativeUnavailableReason` enum (`NoApiKey`, `SameAsPrimary`)
- [ ] Unit tests for `RelativeProjectPath::new()` edge cases

### 8.3 Worktree Management (new: `engine/src/speculative/worktree.rs`)

- [ ] `generate_worktree_name()` — random name generation
- [ ] `create_worktree(plan, files)` — create directory, copy baseline files, write status.json
- [ ] `delete_worktree(path)` — recursive delete
- [ ] `cleanup_stale_worktrees()` — startup scan, delete >24h old
- [ ] `WorktreeStatus`, `WorktreeMeta`, `WorktreeStatusKind` structs (discriminated union)
- [ ] Serde round-trip test for `WorktreeStatus` with each `WorktreeStatusKind` variant

### 8.4 Parse Boundary (new: `engine/src/speculative/parse.rs`)

- [ ] `parse_speculative_output(raw: &str) -> Vec<SpeculativeFile>` — boundary function
- [ ] Parse `=== PATH ===` blocks, validate each path via `RelativeProjectPath::new()`
- [ ] Reject invalid paths at the boundary (log warning, skip file, do not propagate)
- [ ] Return `Vec<SpeculativeFile>` — core-safe, every path is proof-carrying

### 8.5 Speculative Execution (`engine/src/speculative/mod.rs`)

- [ ] `SpeculativeState` enum (4 variants: `Idle`, `Running`, `Ready`, `Failed`)
- [ ] `start_speculation(plan_text, files, model: ModelName) -> SpeculativeState::Running`
- [ ] Background task: build prompt, call speculative model, parse output, write to worktree
- [ ] Abort handling via `AbortHandle`
- [ ] Timeout enforcement (120s)

### 8.6 Plan Mode Integration (`engine/src/lib.rs`, `engine/src/commands.rs`)

- [ ] Add `speculative_state: SpeculativeState` to `App`
- [ ] On `ExitPlanMode`: trigger `start_speculation` if enabled and model resolved
- [ ] On plan approval: pattern-match `SpeculativeState`, promote or skip per §7.2
- [ ] On plan rejection / `/clear`: pattern-match, abort and cleanup per §7.3
- [ ] On startup: call `cleanup_stale_worktrees()`

### 8.7 Promotion (`engine/src/speculative/promote.rs`)

- [ ] `build_promotion_context(files: &[SpeculativeFile]) -> String` — format for injection
- [ ] Uses `RelativeProjectPath::as_str()` for header labels (no raw strings in output)
- [ ] Inject into primary model's message context before execution begins
- [ ] Log promotion event with worktree name

### 8.8 UI (`tui/src/lib.rs`)

- [ ] Status bar indicator: `[spec: scaffolding...]` / `[spec: ready]`
- [ ] Notification on promotion: `"Speculative draft from <name> injected as reference."`

### 8.9 App Init (`engine/src/init.rs`)

- [ ] Read `PlanConfig` from config
- [ ] Call `resolve_speculative_model()` → `SpeculativeModelResolution`
- [ ] Pattern-match resolution:
  - `Configured(model)` / `InferredDefault { model, .. }` → store `ModelName` for speculation
  - `Unavailable { reason }` → log at `debug`, set feature disabled
- [ ] Call `cleanup_stale_worktrees()` on startup

---

## 9. Verification Requirements

### 9.1 Unit Tests — Proof-Carrying Types

| Test ID | Description |
| --- | --- |
| T-SE-PATH-01 | `RelativeProjectPath::new("src/engine/config.rs")` succeeds |
| T-SE-PATH-02 | `RelativeProjectPath::new("")` returns `None` (empty) |
| T-SE-PATH-03 | `RelativeProjectPath::new("/etc/passwd")` returns `None` (absolute) |
| T-SE-PATH-04 | `RelativeProjectPath::new("../escape/file.rs")` returns `None` (traversal) |
| T-SE-PATH-05 | `RelativeProjectPath::new("src/../escape.rs")` returns `None` (embedded traversal) |
| T-SE-PATH-06 | `RelativeProjectPath::new("C:\\Windows\\system32")` returns `None` (Windows absolute) |
| T-SE-PATH-07 | `RelativeProjectPath::new("  src/lib.rs  ")` succeeds (trimmed) |
| T-SE-PATH-08 | `RelativeProjectPath::as_str()` returns the validated path |

### 9.2 Unit Tests — Parse Boundary

| Test ID | Description |
| --- | --- |
| T-SE-PARSE-01 | `parse_speculative_output` correctly splits `=== PATH ===` blocks into `Vec<SpeculativeFile>` with `RelativeProjectPath` paths |
| T-SE-PARSE-02 | `parse_speculative_output` handles malformed input gracefully (returns empty vec) |
| T-SE-PARSE-03 | `parse_speculative_output` handles empty model output (returns empty vec) |
| T-SE-PARSE-04 | `parse_speculative_output` skips blocks with invalid paths (e.g., `=== /etc/passwd ===`), logs warning, continues parsing remaining blocks |
| T-SE-PARSE-05 | `parse_speculative_output` skips blocks with traversal paths (e.g., `=== ../../etc/passwd ===`) |

### 9.3 Unit Tests — Model Resolution

| Test ID | Description |
| --- | --- |
| T-SE-RESOLVE-01 | Configured model returns `SpeculativeModelResolution::Configured` |
| T-SE-RESOLVE-02 | No config + Anthropic key returns `InferredDefault { provider: Claude, .. }` |
| T-SE-RESOLVE-03 | No config + no keys returns `Unavailable { reason: NoApiKey }` |
| T-SE-RESOLVE-04 | Configured model == primary model returns `Unavailable { reason: SameAsPrimary }` |
| T-SE-RESOLVE-05 | Inferred model == primary model returns `Unavailable { reason: SameAsPrimary }` |

### 9.4 Unit Tests — Worktree & Config

| Test ID | Description |
| --- | --- |
| T-SE-NAME-01 | `generate_worktree_name()` produces valid `color-emotion-animal-timestamp` format |
| T-SE-CONFIG-01 | `PlanConfig` defaults to disabled |
| T-SE-CONFIG-02 | `PlanConfig` parses `speculative_model` correctly |
| T-SE-PROMO-01 | `build_promotion_context` formats files with correct markdown fences using `RelativeProjectPath::as_str()` for headers |
| T-SE-STATUS-01 | `WorktreeStatus` with `WorktreeStatusKind::Running` round-trips through serde (no `files` field in JSON) |
| T-SE-STATUS-02 | `WorktreeStatus` with `WorktreeStatusKind::Completed { files }` round-trips through serde (`files` present) |
| T-SE-STATUS-03 | `WorktreeStatus` with `WorktreeStatusKind::Failed` round-trips through serde (no `files` field) |
| T-SE-STATUS-04 | Deserializing a `Completed` status without `files` field fails (proves the field is required, not optional) |

### 9.5 Integration Tests

| Test ID | Description |
| --- | --- |
| IT-SE-WORKTREE-01 | Worktree creation and deletion round-trip |
| IT-SE-CLEANUP-01 | Stale worktree cleanup removes >24h directories |
| IT-SE-CLEANUP-02 | Fresh worktrees are NOT cleaned up on startup |
| IT-SE-ABORT-01 | Aborting a running speculation cleans up worktree |
| IT-SE-RESOLVE-01 | Full init flow: config → resolve → pattern-match → store or disable |

### 9.6 Manual Verification

| Test ID | Description |
| --- | --- |
| MV-SE-E2E-01 | Full flow: enter plan mode, finalize plan, see `[spec: scaffolding...]`, approve, see promotion notification |
| MV-SE-REJECT-01 | Reject plan during speculation, verify worktree deleted |
| MV-SE-TIMEOUT-01 | Speculative model takes >120s, verify timeout and graceful degradation |
| MV-SE-DISABLED-01 | Feature disabled in config, verify no speculative activity |
| MV-SE-NOKEY-01 | No API keys for speculative model, verify silent disable with debug log |

---

## 10. Future Considerations

### 10.1 v2 Enhancements

* **Multi-plan speculation** — generate competing plans, speculate on top 2, let user compare actual code not just plan text
* **Incremental speculation** — as the user edits the plan during review, re-run speculation on changed sections
* **Diff view** — show baseline vs speculative diff in the TUI before promotion
* **Cost tracking** — display estimated cost of speculative pass in status bar
* **Worktree persistence** — keep successful worktrees as "snapshots" for undo/compare
* **Smart file selection** — use dependency analysis (FR-BA from Blast Radius SRS) to include files not explicitly in the plan but likely affected

### 10.2 Model Routing

Future versions may route different plan steps to different models:

| Step Complexity | Model |
| --- | --- |
| Boilerplate (struct fields, imports) | Haiku 4.5 |
| Logic (algorithms, state machines) | Sonnet 4.5 |
| Architecture (trait design, error handling) | Opus 4.6 |

This maximizes speed and minimizes cost while maintaining quality where it matters.

### 10.3 Speculative Execution for Non-Plan Workflows

If the user sends a long message that will clearly require code changes, the system could speculatively begin analysis before the primary model's full response is ready. This is significantly more complex and deferred to v3+.

---

## Appendix A: Future SRS — Architecture Lint System

> **REMINDER:** Write a separate SRS for a clippy-style architecture lint system.
>
> Concept: The system tracks code patterns across tool executions. When it detects
> repeated structural duplication (e.g., "I've seen you write this 95% similar code
> block 3 times"), it surfaces a non-blocking alert suggesting refactoring.
>
> Key requirements to spec:
> - Pattern fingerprinting (AST-level or text-similarity based)
> - Threshold configuration (how many duplicates before alerting)
> - Alert UX (non-modal notification, not a blocker)
> - Scope: within-session only for v1, cross-session for v2
> - Integration with plan mode (suggest refactoring as a plan)
> - False positive suppression (user can dismiss + "don't alert for this pattern")
>
> Working title: `ARCHITECTURE_LINT_SRS.md`

---

## Appendix B: IFA Conformance Notes

This section documents how each data structure in §5 satisfies the Invariant-First Architecture requirements defined in `INVARIANT_FIRST_ARCHITECTURE.md`.

### B.1 Invariant Registry

| Invariant | Encoding | Authority Boundary | IFA Section |
| --- | --- | --- | --- |
| Speculative file path is relative and within project bounds | `RelativeProjectPath` newtype | `RelativeProjectPath::new()` | §2.1, §11.1 |
| Worktree lifecycle status determines valid fields | `WorktreeStatusKind` discriminated union | Serde deserialization + variant construction | §9.2, §14.2 |
| Speculative model resolution outcome is structurally distinguished | `SpeculativeModelResolution` sum type | `resolve_speculative_model()` in init.rs | §8.1 |
| Speculative execution state determines valid operations | `SpeculativeState` enum with per-variant data | State transition functions in `speculative/mod.rs` | §9.1, §9.4 |
| Only one speculation active per session | Single `speculative_state` field on `App` | `App` struct (one slot = one speculation) | §6.1 |

### B.2 Authority Boundary Map

| Proof Type | Authority Boundary | Safe Surface |
| --- | --- | --- |
| `RelativeProjectPath` | `RelativeProjectPath::new()` — private inner `String`, no `pub` field | Cannot construct without validation; `as_str()` is read-only |
| `SpeculativeFile` | `parse_speculative_output()` — sole producer | Fields are `pub` but path is `RelativeProjectPath` (proof-carrying) |
| `SpeculativeModelResolution` | `resolve_speculative_model()` in init.rs | Enum variants are constructible, but only init boundary calls the resolver |
| `WorktreeStatusKind` | Serde deserialization + programmatic variant construction | `#[serde(tag = "kind")]` ensures JSON ↔ Rust round-trip preserves variant |
| `SpeculativeState` | Transition functions (`start_speculation`, abort, promote, cleanup) | `App` holds the single instance; transitions consume prior state |

### B.3 Conformance Checklist

| IFA Requirement | Status | Notes |
| --- | --- | --- |
| §2.1 Invalid states MUST NOT be forgeable | **Pass** | `RelativeProjectPath` prevents invalid paths; `WorktreeStatusKind` prevents `files` on non-completed status |
| §7.2 Single canonical proof per invariant | **Pass** | One `RelativeProjectPath` type for all path validation; one `WorktreeStatusKind` for lifecycle |
| §8.1 Mechanism/policy separation | **Pass** | `SpeculativeModelResolution` reports facts; init boundary (policy) decides |
| §9.2 No tag fields for lifecycle state | **Pass** | `WorktreeStatusKind` is a discriminated union; `SpeculativeState` is a discriminated union |
| §9.3 Domain enumeration test | **Pass** | All enum variants map to named domain states; no `None`/`Unknown`/`Default` variants |
| §9.4 State transitions by moving between types | **Pass** | `SpeculativeState` transitions consume the prior variant and produce a new one |
| §11.1 Boundary converts, core assumes | **Pass** | `parse_speculative_output` (boundary) validates; core accepts `Vec<SpeculativeFile>` |
| §11.2 No optionality in core interfaces | **Pass** | `Option<String>` exists only in `PlanConfig` (boundary); resolved before core |
| §12.1 No assertions for representable invariants | **Pass** | No `assert!(path.is_relative())` in core — enforced by `RelativeProjectPath` type |
| §14.2 Memory layout test | **Pass** | Changing `WorktreeStatusKind` variant changes which fields exist |

### B.4 Non-Conforming Patterns Avoided

| Anti-Pattern (IFA §13) | How Avoided |
| --- | --- |
| State-dependent validity via flags | `WorktreeStatusKind` discriminated union, not `status: String` + conditional `files` |
| Deep "maybe" checks | Core matches on `SpeculativeState` variants, never checks `if state.is_ready()` |
| Sentinel values | No `-1`, `""`, or `null` used for absence |
| Two-phase initialization | `SpeculativeFile` is fully constructed at parse time, not built incrementally |
| Get-or-default mechanisms | `SpeculativeModelResolution::Unavailable` is structurally distinct from a resolved model |
| Optional fields in core interfaces | `RelativeProjectPath` is non-optional; `SpeculativeFile.path` is always valid |
