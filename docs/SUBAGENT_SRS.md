# Subagent Delegation

## Overview

Subagents are real agents with read-only tool access, spawned via a single `Delegate` tool call. They run their own tool loops, bounded by context window exhaustion rather than arbitrary limits. This keeps multi-agent concurrency at the boundary while preserving the core state machine's simplicity.

### Design Principles

1. **Subagents are real agents** — Full read-only tool access, not prompt splitters
2. **Context window is the constraint** — No arbitrary timeouts or iteration caps
3. **Transcript capture for durability** — Full conversation logged regardless of outcome
4. **Scratchpad for intentional findings** — Subagents journal key discoveries via Note tool
5. **No recursion** — Subagents cannot call `Delegate`
6. **Read-only for now** — Write access is a future capability, blocked until subagent trust is established

---

## 1. User Experience

### TUI Flow

1. User asks the active model to delegate work
2. Model issues a single `Delegate` tool call with an array of subagent tasks
3. Forge shows one approval prompt enumerating: subagent count, labels, goals (truncated), policy/risk
4. On approval, the tool execution view shows real-time progress via stdout chunks
5. On completion, Forge returns a single tool result with per-subagent status, reports, and usage
6. Parent model synthesizes a final answer, reconciling any conflicts

### Expected Parent Model Behavior

- Delegate only when beneficial: parallel exploration, independent critique, specialized viewpoints
- Provide each subagent a crisp, non-overlapping objective and required output shape
- Treat subagent output as advisory—reconcile disagreements
- Handle failures gracefully: continue and synthesize with available results
- Default to small N (2–4) unless explicitly asked for more

### Progress Rendering

```
Delegate [3 subagents]
  [security]     ✓ complete (in=45k out=2.1k)
  [correctness]  → Read src/lib.rs (in=23k out=890)
  [refactor]     ⚠ partial: context exhausted
```

---

## 2. Subagent Tool Access

### Included Tools

| Tool | Rationale |
|------|-----------|
| `Read` | File inspection |
| `Grep` | Content search |
| `Glob` | File discovery |
| `WebFetch` | External docs |
| `WebSearch` | Research |
| `Note` | Scratchpad writes (subagent-only) |

### Excluded Tools

| Tool | Rationale |
|------|-----------|
| `Delegate` | Prevents recursion |
| `Edit`, `Write` | No mutations |
| `Run` | No side effects |
| `NotebookEdit` | No mutations |

### Implementation

```rust
const SUBAGENT_ALLOWED_TOOLS: &[&str] = &[
    "Read", "Grep", "Glob", "WebFetch", "WebSearch",
];

// Build filtered tool set for subagent
let tools: Vec<ToolDefinition> = tool_registry
    .iter()
    .filter(|t| SUBAGENT_ALLOWED_TOOLS.contains(&t.name()))
    .map(|t| t.definition())
    .collect();

// Add Note tool (subagent-only)
tools.push(NoteTool::new(&scratchpad).definition());
```

---

## 3. Tool Surface

**Name**: `Delegate`

### Schema

```json
{
  "type": "object",
  "properties": {
    "tasks": {
      "type": "array",
      "minItems": 1,
      "maxItems": 8,
      "items": {
        "type": "object",
        "properties": {
          "label": {
            "type": "string",
            "minLength": 1,
            "maxLength": 32,
            "description": "Identifier for progress/transcripts. Persona activation belongs in prompt."
          },
          "prompt": { "type": "string", "minLength": 1 },
          "context": {
            "type": "array",
            "items": { "type": "string" },
            "maxItems": 10,
            "description": "File paths to prepend to prompt (reduces subagent thrashing)"
          },
          "model": {
            "type": "string",
            "description": "RESERVED: Model override for this subagent (not yet implemented)"
          },
          "max_output_tokens": {
            "type": "integer",
            "minimum": 100,
            "maximum": 16384,
            "default": 4096
          }
        },
        "required": ["label", "prompt"],
        "additionalProperties": false
      }
    },
    "concurrency": { "type": "integer", "minimum": 1, "maximum": 4, "default": 2 },
    "return": { "type": "string", "enum": ["markdown", "json"], "default": "markdown" }
  },
  "required": ["tasks"],
  "additionalProperties": false
}
```

### Tool Traits

| Method | Value |
|--------|-------|
| `is_side_effecting()` | `true` (external API call, incurs cost) |
| `requires_approval()` | `true` (model-initiated spend) |
| `risk_level()` | `RiskLevel::Medium` |

### Approval Distillate

Use existing `util::truncate_with_ellipsis` and `tools::redact_Distillate` patterns:

```
Spawn 3 subagents: [security] scan for vulnerab…; [correctness] check return val…; [refactor] propose simplific…
```

---

## 4. Context Window as Constraint

### No Arbitrary Limits

- No timeout
- No iteration cap
- No tool call cap

### Natural Termination

Subagent runs until:
1. **Success**: Produces final response with no pending tool calls
2. **Context exhaustion**: Hits context window limit → returns partial results from scratchpad

User can ctrl+c if impatient—same as parent agent.

### Why This Works

Context window directly correlates with:
- Work done (tool calls consume tokens)
- Cost incurred (tokens are the billing unit)
- Quality of output (more context = more thorough investigation)

Arbitrary caps (5 iterations, 60s timeout) are proxies for what we actually care about: token spend.

---

## 5. Transcript Capture

### Problem

If a subagent fails or exhausts context, we lose visibility into what happened.

### Solution

Every subagent's full conversation is logged to disk regardless of outcome.

### Location

```
~/.forge/subagent/{label}-{uuid}.transcript.json
```

### Contents

```json
{
  "label": "security",
  "started_at": "2025-01-31T12:34:56Z",
  "ended_at": "2025-01-31T12:35:42Z",
  "outcome": "success|context_exhausted|error",
  "messages": [
    { "role": "user", "content": "..." },
    { "role": "assistant", "content": "...", "tool_calls": [...] },
    { "role": "tool", "tool_call_id": "...", "content": "..." },
    ...
  ],
  "usage": { "input": 45000, "output": 2100 }
}
```

### Purpose

- **Crash recovery**: If Forge dies mid-subagent, transcript survives
- **Debugging**: Full visibility into what the subagent did
- **Audit trail**: Cost attribution and behavior analysis

This is the safety net. The scratchpad (via Note tool) is for *intentional* findings the subagent wants to preserve.

### Retention

Transcripts older than **7 days** are pruned on startup. No pinning mechanism — if something is important, use the appropriate persistence tool (fact store, notes, etc.).

---

## 6. Scratchpad Pattern

### Problem

If a subagent exhausts its context window mid-investigation, only the transcript remains—but transcripts are verbose and not synthesized.

### Solution

Each subagent writes incremental findings to a scratchpad file via the `Note` tool.

### Flow

```
1. Delegate creates temp file: {scratchpad_dir}/subagent-{label}.scratch.md
2. Subagent receives scratchpad path in system prompt
3. Subagent writes findings incrementally via Note tool
4. On context exhaustion → parent reads scratchpad for partial results
5. On success → final response is authoritative, scratchpad is debug artifact
```

### Note Tool (Subagent-Only)

```rust
struct NoteTool {
    scratchpad: PathBuf,
}

impl Tool for NoteTool {
    fn name(&self) -> &str { "Note" }

    fn execute(&self, args: Value, _ctx: &mut ToolCtx) -> ToolFut {
        let content: String = serde_json::from_value(args["content"].clone())?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.scratchpad)?;
        writeln!(file, "{}", content)?;
        Ok(ToolOutput::text("Noted."))
    }
}
```

### System Prompt Addition

```
You have a scratchpad for incremental notes. As you investigate, record key findings
using the Note tool. If your context window fills up, your scratchpad contents will
be returned to the parent agent. Structure your notes so partial results are useful.
```

---

## 7. Engine Integration

The tool loop already provides correct typestate transitions:

```
Streaming → ToolLoop(AwaitingApproval|Executing) → Streaming(resume with tool results)
```

`Delegate` is "just another tool"—no new `OperationState` variant required.

### File Touchpoints

| Action | File |
|--------|------|
| Add | `engine/src/tools/delegate.rs` |
| Add | `engine/src/tools/note.rs` (subagent-only tool) |
| Add | `engine/src/tools/transcript.rs` (transcript serialization) |
| Modify | `engine/src/tools/mod.rs` → add `ToolLlmCtx`, register tools |
| Modify | `engine/src/tool_loop.rs` → thread model, build filtered tools |
| Modify | `tui/src/tool_display.rs` → subagent progress rendering |

### Directories

| Path | Purpose |
|------|---------|
| `~/.forge/subagent/` | Transcript storage (auto-created) |

### No Changes Required

- `engine/src/state.rs` (no new operation state)
- `engine/src/streaming.rs` (resume-after-tools path is correct)
- `cli/src/main.rs` (tool loop already blocks and renders progress)

---

## 8. Model Provenance Fix

**Problem**: Tool execution must use `batch.model` (the model that produced the tool calls), not `App.model` at execution time.

**Solution**: Thread `batch.model` through both `spawn_tool_execution` and `start_next_tool_call`.

```rust
// engine/src/tool_loop.rs

fn spawn_tool_execution(
    &self,
    queue: Vec<ToolCall>,
    initial_capacity_bytes: usize,
    turn_recorder: ChangeRecorder,
    model: &ModelName,  // NEW
) -> ActiveToolExecution {
    let mut exec = ActiveToolExecution { /* ... */ };
    self.start_next_tool_call(&mut exec, model);
    exec
}

fn start_next_tool_call(&self, exec: &mut ActiveToolExecution, model: &ModelName) -> bool {
    // ... existing logic, now with access to model ...
}

// Update ALL call sites to pass &batch.model
```

---

## 9. LLM Capability on ToolCtx

### Type Definition

```rust
// engine/src/tools/mod.rs

pub struct ToolLlmCtx {
    pub config: ApiConfig,
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
    pub tool_executor: Arc<ToolExecutor>,
    pub scratchpad_dir: PathBuf,
    pub transcript_dir: PathBuf,  // ~/.forge/subagent/
}

pub struct ToolCtx {
    // ... existing fields ...
    pub llm: Option<ToolLlmCtx>,
}
```

### Population in start_next_tool_call

Only populate when needed (when `call.name == "Delegate"`):

```rust
// engine/src/tool_loop.rs

let llm = if call.name == "Delegate" {
    let provider = model.provider();
    self.api_keys.get(&provider).and_then(|raw_key| {
        let api_key = crate::util::wrap_api_key(provider, raw_key.clone());
        let config = ApiConfig::new(api_key, model.clone()).ok()?;
        let config = config
            .with_openai_options(self.openai_options_for_model(model))
            .with_gemini_thinking_enabled(self.gemini_thinking_enabled);
        let system_prompt = self.system_prompts.get(provider).to_string();

        // Build filtered tool set
        let tools = build_subagent_tools(&self.tool_registry);

        Some(tools::ToolLlmCtx {
            config,
            system_prompt,
            tools,
            tool_executor: self.tool_executor.clone(),
            scratchpad_dir: self.scratchpad_dir.clone(),
            transcript_dir: self.transcript_dir.clone(),
        })
    })
} else {
    None
};
```

If no API key exists, `llm` is `None` and the tool returns a clean error: "Cannot spawn subagents: no API key for provider X".

---

## 10. Subagent Execution

### Execution Loop

```rust
async fn run_subagent(
    task: &Task,
    llm_ctx: &ToolLlmCtx,
) -> SubagentOutcome {
    let run_id = Uuid::new_v4();
    let started_at = Utc::now();

    // Create scratchpad
    let scratchpad = llm_ctx.scratchpad_dir.join(format!("{}.scratch.md", task.label));
    std::fs::write(&scratchpad, "")?;

    // Transcript path for durability
    let transcript_path = llm_ctx.transcript_dir.join(format!("{}-{}.transcript.json", task.label, run_id));

    // Build initial messages
    let context_content = load_context_files(&task.context).await?;
    let prompt = format!("{}\n\n{}", context_content, task.prompt);
    let mut messages = vec![CacheableMessage::user(prompt)];

    // Add Note tool to subagent's tool set
    let mut tools = llm_ctx.tools.clone();
    tools.push(NoteTool::new(&scratchpad).definition());

    let system = format!(
        "{}\n\n{}",
        llm_ctx.system_prompt,
        SUBAGENT_SYSTEM_SUFFIX
    );

    let mut total_usage = ApiUsage::default();

    loop {
        let response = match stream_one_turn(&llm_ctx.config, &messages, &system, &tools).await {
            Ok(r) => r,
            Err(e) if e.is_context_exhausted() => {
                let scratchpad_contents = std::fs::read_to_string(&scratchpad)?;
                save_transcript(&transcript_path, &task.label, started_at, "context_exhausted", &messages, &total_usage)?;
                return SubagentOutcome::ContextExhausted {
                    scratchpad_contents,
                    usage: total_usage,
                };
            }
            Err(e) => {
                save_transcript(&transcript_path, &task.label, started_at, "error", &messages, &total_usage)?;
                return SubagentOutcome::Error { message: e.to_string() };
            }
        };

        total_usage = total_usage.merge(&response.usage);

        if response.tool_calls.is_empty() {
            save_transcript(&transcript_path, &task.label, started_at, "success", &messages, &total_usage)?;
            return SubagentOutcome::Success {
                response: response.text,
                usage: total_usage,
            };
        }

        // Execute tool calls
        let results = execute_tools(&response.tool_calls, &llm_ctx.tool_executor).await;

        // Append to conversation
        messages.push(CacheableMessage::assistant_with_tools(response.text, response.tool_calls));
        for result in results {
            messages.push(CacheableMessage::tool_result(result));
        }

        // Incremental transcript save (crash recovery)
        save_transcript(&transcript_path, &task.label, started_at, "in_progress", &messages, &total_usage)?;
    }
}
```

### Concurrency (Cancellation Safety + Stable Ordering)

Use `FuturesUnordered` with index tracking for stable ordering:

```rust
use futures_util::stream::{FuturesUnordered, StreamExt};

async fn execute_delegate(
    tasks: Vec<Task>,
    concurrency: usize,
    llm_ctx: &ToolLlmCtx,
) -> ToolResult {
    let semaphore = Arc::new(Semaphore::new(concurrency));

    let futures: FuturesUnordered<_> = tasks
        .into_iter()
        .enumerate()
        .map(|(idx, task)| {
            let sem = semaphore.clone();
            async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                let res = run_subagent(&task, llm_ctx).await;
                (idx, task.label.clone(), res)
            }
        })
        .collect();

    let mut results: Vec<_> = futures.collect().await;
    results.sort_by_key(|(idx, _, _)| *idx);  // Restore input order
    format_results(results)
}
```

### Outcome Type

```rust
enum SubagentOutcome {
    Success { response: String, usage: ApiUsage },
    ContextExhausted { scratchpad_contents: String, usage: ApiUsage },
    Error { message: String },
}
```

---

## 11. Output Format

### Markdown (default)

```markdown
## Subagents complete: 3/3

### [security] ✓
**Usage**: in=45,000 out=2,100

<report content>

### [correctness] ✓
**Usage**: in=23,000 out=890

<report content>

### [refactor] ⚠️ partial (context exhausted)
**Usage**: in=195,000 out=12,400

**Findings before exhaustion:**

<scratchpad contents>
```

### JSON (when `return="json"`)

```json
{
  "completed": 2,
  "partial": 1,
  "total": 3,
  "results": [
    { "label": "security", "status": "ok", "usage": { "input": 45000, "output": 2100 }, "report": "..." },
    { "label": "correctness", "status": "ok", "usage": { "input": 23000, "output": 890 }, "report": "..." },
    { "label": "refactor", "status": "partial", "usage": { "input": 195000, "output": 12400 }, "scratchpad": "..." }
  ]
}
```

### Usage Reporting

Per-subagent usage is included in tool result content. Do not attempt to integrate into `App.turn_usage` initially—that would require widening types across the architecture.

---

## 12. Progress Events

### Event Format

```json
{"event": "started", "label": "security", "index": 0, "total": 3}
{"event": "tool_call", "label": "security", "tool": "Read", "args": "src/auth.rs"}
{"event": "note", "label": "security", "content": "Found hardcoded secret on line 42"}
{"event": "tokens", "label": "security", "input": 12000, "output": 450}
{"event": "completed", "label": "security", "status": "ok"}
{"event": "completed", "label": "review", "status": "partial", "reason": "context_exhausted"}
```

---

## 13. Failure Behavior

| Scenario | Behavior |
|----------|----------|
| One subagent context exhausted | Return partial from scratchpad, continue others |
| One subagent errors | Mark as error, continue others |
| Provider/network failure | Return `error` for affected tasks with sanitized message |
| All fail | Return structured result so parent can respond coherently |
| No API key | Return tool error: "Cannot spawn subagents: no API key for provider X" |
| User ctrl+c | All subagents cancelled; incremental transcripts preserved |

---

## 14. Constants

```rust
// engine/src/tools/delegate.rs

const MAX_TASKS_PER_CALL: usize = 8;
const MAX_CONCURRENCY: usize = 4;
const DEFAULT_CONCURRENCY: usize = 2;
const DEFAULT_SUBAGENT_MAX_OUTPUT_TOKENS: u32 = 4096;

const SUBAGENT_ALLOWED_TOOLS: &[&str] = &[
    "Read", "Grep", "Glob", "WebFetch", "WebSearch",
];

const SUBAGENT_SYSTEM_SUFFIX: &str = "\
You are a subagent with read-only tool access (Read, Grep, Glob, WebFetch, WebSearch). \
You cannot modify files or run commands. \

Use the Note tool to record important findings as you work. If your context fills up, \
your notes will be returned to the parent agent. Structure notes so partial results are useful.

Produce a clear, concise report addressing your assigned task.";
```

---

## 15. System Prompt Guidance

Add to base prompts (`cli/assets/base_prompt_*.md`):

> **Delegate Tool**: Use `Delegate` to spawn 2–4 focused subagents for parallel analysis, independent critique, or specialized viewpoints. Each subagent has read-only tool access (Read, Grep, Glob, WebFetch, WebSearch) and runs until completion or context exhaustion. Provide each subagent a distinct, non-overlapping objective. Treat their output as advisory—reconcile disagreements in your synthesis.

---

## 16. Tests

### Schema/Arg Validation

- Empty tasks rejected
- Empty prompt rejected (use `NonEmptyString` in Serde)
- Task count > MAX_TASKS_PER_CALL rejected

### Formatting

- Stable markdown layout (ordering preserved regardless of completion order)
- Partial results include scratchpad content

### Backend Abstraction

Avoid adding `mockall` as a dependency. Use an object-safe trait with boxed futures:

```rust
// engine/src/tools/delegate.rs

trait DelegateBackend: Send + Sync {
    fn run_subagent<'a>(
        &'a self,
        task: &'a Task,
        llm_ctx: &'a ToolLlmCtx,
    ) -> Pin<Box<dyn Future<Output = SubagentOutcome> + Send + 'a>>;
}

struct RealBackend;

impl DelegateBackend for RealBackend {
    fn run_subagent<'a>(
        &'a self,
        task: &'a Task,
        llm_ctx: &'a ToolLlmCtx,
    ) -> Pin<Box<dyn Future<Output = SubagentOutcome> + Send + 'a>> {
        Box::pin(run_subagent(task, llm_ctx))
    }
}

// DelegateTool holds Arc<dyn DelegateBackend>
// Tests inject a stub implementation
```

---

## 17. Future Capabilities

The following are explicitly out of scope for initial implementation but the design should remain extensible:

| Capability | Status | Notes |
|------------|--------|-------|
| **Model override** | Schema field reserved | `model` field in task schema, not yet wired |
| **Write access** | Blocked | Safety constraint until subagent trust is established |
| **Configurable tool set** | Not implemented | Currently hardcoded allowlist |
| **Token budget caps** | Not implemented | Context window is the current constraint |
| **Subagent-to-subagent delegation** | Blocked | Recursion prevention is a hard constraint |

