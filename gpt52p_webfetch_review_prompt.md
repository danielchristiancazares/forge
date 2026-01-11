You are a senior systems architect preparing to implement the WebFetch tool specification.
Your task is to make the specification implementation ready and airtight.

## Constraints / Environment

- Source documents are in forge-source.zip. Extract and read them.
- You are reviewing specifications, not implementing code.
- docs/DESIGN.md defines type-driven design patterns implementations must respect.
- Use concrete section/requirement IDs wherever possible. If you infer, label it "inferred."

## Documents to Review

- Primary: docs/WEBFETCH_SRD.md
- Related: docs/TOOL_EXECUTOR_SRD.md
- Implementation context: engine/src/tools/mod.rs, engine/src/tools/builtins.rs, engine/src/tools/sandbox.rs
- Domain types: types/src/lib.rs, types/src/sanitize.rs
- Config: engine/src/config.rs
- Patterns: docs/INVARIANT_FIRST_ARCHITECTURE.md (type-driven design criteria)
- Philosophy: docs/DESIGN.md (illustrative examples)

## Guiding Questions

Rather than a checklist, consider these as you read:

### On Precision
- If you implemented exactly what's written, would two engineers produce compatible results? Where would they diverge?
- Which requirements use vague terms ("should handle," "appropriate," "reasonable") that need quantification?
- Are there implicit ordering dependencies or precedence rules that should be explicit?

### On Boundaries
- What happens at the edges? Malformed URLs, empty responses, enormous pages, binary content, unusual encodings, internationalized domains.
- How should partial failures behave? Timeout mid-download, browser crash, disk full, DNS failure after initial success.
- What resource limits exist, and what happens when they're exceeded?

### On Security
- Are SSRF protections complete? Consider DNS rebinding, TOCTOU gaps, IPv6 edge cases, redirect chains.
- What does "untrusted output" mean for downstream consumers? How should it be labeled/handled?
- Are robots.txt edge cases covered? Wildcards, crawl-delay, malformed files, missing vs. forbidden.
- Could cache keys enable path traversal or poisoning?

### On Consistency
- Do configuration options map to every functional requirement that needs them?
- Do error codes cover all described failure modes?
- Do verification tests cover each MUST/SHALL requirement?
- Are normative vs. informative sections clearly distinguished?

### On Implementability
- Would a state diagram help clarify the fetch/fallback/cache flow?
- Would pseudocode help for complex algorithms (chunking boundaries, SSRF validation)?
- Are there missing appendices that would aid correct implementation?

### On Network Behavior (domain-specific)
- Are timeout values specified for each phase (DNS, connect, TLS, first byte, total)?
- How should redirect chains be handled? Limits? Cross-origin behavior?
- What caching semantics apply? Cache keys? Invalidation? Staleness?
- How should binary vs. text content be distinguished and handled?

### On Content Processing (domain-specific)
- How is HTML-to-Markdown conversion specified? Edge cases?
- What encoding detection/conversion is required?
- How should enormous responses be bounded and chunked?
- What happens when the browser fallback also fails?

## Required Output Format

### 1. Implementation-Ready Assessment
Brief verdict: Is this spec ready for implementation? What is the single biggest blocker?

### 2. Discoveries and Proposals
For each finding:
- **Section:** Where in the SRD
- **Observation:** What you noticed (ambiguity, gap, edge case, inconsistency)
- **Implementer's question:** What you'd ask when hitting this
- **Proposal:** Recommended resolution—draft requirement text where helpful

Group findings by subsystem: URL validation, robots.txt, rendering, chunking, caching, errors, config, tests.

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

- Avoid generic advice; tie everything directly to this specification and the Forge architecture.
- Do not invent issues; if a section is complete, say "No findings."
- Prefer adding precision over adding complexity.
- Consider existing Forge patterns (type-driven design, proof tokens, sandbox model, tool executor framework) when proposing solutions.
- If you need more context or clarity, list exactly what you need.
