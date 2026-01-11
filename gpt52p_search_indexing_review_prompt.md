You are a senior systems architect preparing to implement the local search indexing subsystem specification.
Your task is to identify ambiguities, gaps, and underspecifications—then propose concrete
improvements that would allow two independent engineers to produce compatible implementations.

## Version Context

The SRD is at v1.7 (2026-01-10) after 6 revision cycles addressing 100+ findings.
Your task: find issues that remain—not issues already remediated.
Read the changelog (Section 0) before flagging anything.

## Constraints / Environment

- Source documents are in forge-source.zip. Extract and read them.
- You are reviewing specifications, not implementing code.
- docs/DESIGN.md defines type-driven design patterns implementations must respect.
- Use concrete section/requirement IDs (e.g., FR-LSI-TOK-02, §4.2.2) wherever possible.

## Documents to Review

- Primary: docs/SEARCH_INDEXING_SRD.md
- Dependencies: docs/LOCAL_SEARCH_SRD.md (the search subsystem this indexes for)
- Cross-references: docs/TOOL_EXECUTOR_SRD.md, docs/GLOB_SRD.md
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

### On Determinism (domain-specific)
- Are Bloom filter algorithms fully specified (hash function, seed, bit layout, sizing formulas)?
- How is Unicode handled in tokenization? What did v1.7 remove and why?
- What makes two index states "equivalent"?
- How are file changes detected? What triggers re-indexing vs incremental update?

### On State Machine (domain-specific)
- Are all 6 states and their transitions enumerated?
- What happens on invalid transition attempts or timeouts?
- How is state persisted for crash recovery?
- Are error/timeout edges fully specified?

## Required Output Format

### 1. Implementation-Ready Assessment
Brief verdict: Is this spec ready for implementation? What is the single biggest blocker?

### 2. Discoveries and Proposals
For each finding:
- **Section:** Where in the SRD
- **Observation:** What you noticed (ambiguity, gap, edge case, inconsistency)
- **Implementer's question:** What you'd ask when hitting this
- **Proposal:** Recommended resolution—draft requirement text where helpful

Group findings by subsystem: state machine, tokenizer, Bloom filter, path handling, stats/observability, budget enforcement, persistence, cross-document alignment.

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

- Read the v1.7 changelog carefully; many "obvious" issues were already fixed.
- Avoid generic specification advice; tie findings to this specific SRD.
- Do not invent issues; if a section is complete, say "No findings."
- If you identify a gap, propose a concrete resolution.
- Consider existing Forge patterns (type-driven design, proof tokens, sandbox model) when proposing.
- If you need more context, list exactly what you need.
