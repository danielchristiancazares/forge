# GPT-5.2 Pro Specification Review Template

A Socratic template for reviewing specifications before implementation.
Core question: "Can two engineers implement this independently and get compatible results?"

---

## Placeholders

| Placeholder | Description | Example |
| :--- | :--- | :--- |
| `{{SPEC_NAME}}` | Specification filename | `WEBFETCH_SRD.md` |
| `{{SPEC_DESCRIPTION}}` | One-line description | `WebFetch tool for URL fetching with browser fallback` |
| `{{VERSION_CONTEXT}}` | Version info (optional, for mature specs) | See below |
| `{{RELATED_SPECS}}` | Dependencies and cross-references | `TOOL_EXECUTOR_SRD.md, GLOB_SRD.md` |
| `{{KEY_FILES}}` | Implementation-relevant source files | `engine/src/tools/mod.rs` |
| `{{SUBSYSTEMS}}` | Logical groupings for findings | `URL validation, caching, errors` |
| `{{DOMAIN_QUESTIONS}}` | Domain-specific Socratic questions | See examples below |

### Version Context (include for mature specs, omit for v1.0)

```markdown
## Version Context
The SRD is at v1.7 (2026-01-10) after 6 revision cycles addressing 100+ findings.
Your task: find issues that remain—not issues already remediated.
Read the changelog (Section 0) before flagging anything.
```

---

## Prompt Template

```markdown
You are a senior engineer preparing to implement the {{SPEC_DESCRIPTION}} specification.
Your task is to make the specification implementation ready and airtight.

{{VERSION_CONTEXT}}

## Constraints / Environment

- Source documents are in forge-source.zip. Extract and read them.
- You are reviewing specifications, not implementing code.
- docs/DESIGN.md defines type-driven design patterns implementations must respect.
- Use concrete section/requirement IDs wherever possible. If you infer, label it "inferred."

## Documents to Review

- Primary: docs/{{SPEC_NAME}}
- Related: docs/{{RELATED_SPECS}}
- Implementation context: {{KEY_FILES}}
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

{{DOMAIN_QUESTIONS}}

## Required Output Format

### 1. Implementation-Ready Assessment
Brief verdict: Is this spec ready for implementation? What is the single biggest blocker?

### 2. Discoveries and Proposals
For each finding:
- **Section:** Where in the SRD
- **Observation:** What you noticed (ambiguity, gap, edge case, inconsistency)
- **Implementer's question:** What you'd ask when hitting this
- **Proposal:** Recommended resolution—draft requirement text where helpful

Group findings by subsystem: {{SUBSYSTEMS}}

### 3. Architecture Considerations
Structural improvements: missing appendices, flow diagrams, algorithm specs, cross-references.

### 4. Prioritized Recommendations
- **Blocking:** Must fix before implementation begins
- **Important:** Should fix early in implementation
- **Refinement:** Can address iteratively

### 5. Summary Table
| Section | Issue | Severity | Proposal Summary |
|---------|-------|----------|------------------|

## Additional Instructions

- Avoid generic advice; tie everything to this specification.
- Do not invent issues; if a section is complete, say "No findings."
- Prefer adding precision over adding complexity.
- Consider existing Forge patterns (type-driven design, proof tokens, sandbox model) when proposing.
- If you need more context, list exactly what you need.
```

---

## Domain-Specific Questions

Add these under `{{DOMAIN_QUESTIONS}}` based on what the spec covers.

### Network / HTTP

```markdown
### On Network Behavior
- Are timeout values specified for each phase (DNS, connect, TLS, first byte, total)?
- How should redirect chains be handled? Limits? Cross-origin?
- Are SSRF protections complete? DNS rebinding, TOCTOU, IPv6, redirects?
- Could cache keys enable path traversal or poisoning?
```

### File System

```markdown
### On Path Handling
- How are relative vs. absolute paths resolved?
- What symlink behavior is expected? Following? Detection? Cycles?
- How are platform differences (Windows vs. Unix) handled?
- Which operations must be atomic? What does "atomic" mean here?
```

### Search / Indexing

```markdown
### On Determinism
- Are algorithms fully specified (hash functions, seeds, bit ordering)?
- How is Unicode handled (normalization, case folding)?
- What makes two index states "equivalent"?
- How are file changes detected? What triggers re-indexing?
```

### State Machines

```markdown
### On Transitions
- Are all valid state transitions enumerated?
- What happens on invalid transition attempts?
- Are there timeout transitions? What triggers them?
- How is state persisted for crash recovery?
```

### Protocols / APIs

```markdown
### On Contracts
- Are request/response schemas complete (all fields, types, constraints)?
- What retry semantics apply? Idempotency guarantees?
- How is versioning handled? Backward compatibility?
- Are error codes exhaustive for all failure modes?
```

---

## Example: Filled Template

```markdown
You are a senior engineer preparing to implement the WebFetch tool specification...

## Version Context
The SRD is at v1.3 (2026-01-09) after 3 revision cycles.
Read the changelog before flagging anything.

## Documents to Review
- Primary: docs/WEBFETCH_SRD.md
- Related: docs/TOOL_EXECUTOR_SRD.md
- Implementation context: engine/src/tools/mod.rs, engine/src/tools/builtins.rs, engine/src/tools/webfetch.rs
- Patterns: docs/DESIGN.md

## Guiding Questions
[standard questions...]

### On Network Behavior
- Are SSRF protections complete? Consider DNS rebinding, TOCTOU gaps, IPv6 edge cases.
- Are robots.txt edge cases covered? Wildcards, crawl-delay, malformed files.
- Could cache keys enable path traversal or poisoning?

## Required Output Format
[standard format...]

Group findings by subsystem: URL validation, robots.txt, rendering, chunking, caching, errors, config, tests.
```

---

## Anti-patterns

| Anti-pattern | Problem | Fix |
| :--- | :--- | :--- |
| "Review this spec" | Too vague | Add guiding questions, subsystems |
| 15+ domain questions | Dilutes focus | Pick 3-5 highest-risk |
| No "implementer's question" | Loses the voice | Always ask "what would I ask?" |
| Everything is "Blocking" | Cries wolf | Reserve for true blockers |
| Skipping version context | Re-discovers fixed issues | Add for specs v1.2+ |
