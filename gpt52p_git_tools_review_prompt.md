# GPT-5.2p Git Tools SRD Review Prompt

You are a senior engineer preparing to implement the Git tools specification (11 tools for repository inspection and manipulation).
Your task is to make the specification implementation ready and airtight.

## Version Context

The SRD is at v1.2 (2026-01-11) after 2 revision cycles addressing param naming mismatches.
Your task: find issues that remain—not issues already remediated.
Read the changelog (Section 0) before flagging anything.

## Constraints / Environment

- Source documents are in forge-source.zip. Extract and read them.
- You are reviewing specifications, not implementing code.
- docs/DESIGN.md defines type-driven design patterns implementations must respect.
- Use concrete section/requirement IDs wherever possible. If you infer, label it "inferred."

## Documents to Review

- Primary: docs/GIT_TOOLS_SRD.md
- Related: docs/TOOL_EXECUTOR_SRD.md
- Implementation context: engine/src/tools/git.rs
- Patterns: docs/INVARIANT_FIRST_ARCHITECTURE.md (type-driven design criteria)
- Philosophy: docs/DESIGN.md (illustrative examples)

## Guiding Questions

Rather than a checklist, consider these as you read:

### On Precision

- If you implemented exactly what's written, would two engineers produce compatible results? Where would they diverge?
- Which requirements use vague terms ("should handle," "appropriate," "reasonable") that need quantification?
- Are there implicit ordering dependencies or precedence rules that should be explicit?

### On Boundaries

- What happens at the edges? Empty inputs, enormous inputs, malformed data, unusual encodings, platform differences.
- How should partial failures behave? What if an operation fails midway?
- What resource limits exist, and what happens when they're exceeded?

### On Security

- What are the trust boundaries? What input is untrusted?
- Are there TOCTOU gaps, injection risks, or path traversal concerns?
- How should untrusted output be labeled/handled for downstream consumers?

### On Consistency

- Do configuration options map to every functional requirement that needs them?
- Do error codes cover all described failure modes?
- Do verification tests cover each MUST/SHALL requirement?
- Are normative vs. informative sections clearly distinguished?

### On Implementability

- Would a state diagram help clarify complex flows?
- Would pseudocode help for complex algorithms?
- Are there missing appendices that would aid implementation?

### On Process Execution

- How are arguments escaped/quoted for git invocation? Platform differences?
- What environment variables are passed, filtered, or sanitized?
- How is process timeout enforced? What happens to orphaned git processes?
- How are large outputs (e.g., `git log` on a huge repo) handled?

### On Path Handling

- How are relative vs. absolute paths resolved within `working_dir`?
- What symlink behavior is expected? Can `working_dir` be a symlink?
- How are Windows vs. Unix path separators handled in pathspecs?
- Are glob patterns in pathspecs expanded by git or pre-processed?

### On Git Semantics

- What happens when git operations conflict (e.g., checkout with uncommitted changes)?
- How are detached HEAD states communicated to the caller?
- What git config assumptions exist (user.name, user.email for commits)?
- How are merge conflicts or other interactive states handled?

## Required Output Format

### 1. Implementation-Ready Assessment

Brief verdict: Is this spec ready for implementation? What is the single biggest blocker?

### 2. Discoveries and Proposals

For each finding:

- **Section:** Where in the SRD
- **Observation:** What you noticed (ambiguity, gap, edge case, inconsistency)
- **Implementer's question:** What you'd ask when hitting this
- **Proposal:** Recommended resolution—draft requirement text where helpful

Group findings by subsystem: status, diff, restore, add, commit, log, branch, checkout, stash, show, blame, common behavior, security, config, tests.

### 3. Architecture Considerations

Structural improvements: missing appendices, flow diagrams, algorithm specs, cross-references.

### 4. Prioritized Recommendations

- **Blocking:** Must fix before implementation begins
- **Important:** Should fix early in implementation
- **Refinement:** Can address iteratively

### 5. Summary Table

| Section | Issue | Severity | Proposal Summary |
| :--- | :--- | :--- | :--- |

## Additional Instructions

- Avoid generic advice; tie everything to this specification.
- Do not invent issues; if a section is complete, say "No findings."
- Prefer adding precision over adding complexity.
- Consider existing Forge patterns (type-driven design, proof tokens, sandbox model) when proposing.
- If you need more context, list exactly what you need.
