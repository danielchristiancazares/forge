# Base Prompt Review

Audit of `cli/assets/base_prompt.md` covering structure, clarity, formatting, contradictions, and phrasing.

## Contradictions / Semantic Conflicts

1. **Redundant dangerous-command lists across sections.** Line 49 lists `git reset --hard` under Security > Dangerous commands. Line 226 repeats the same command under Tools > File operations with slightly different framing ("never execute from tool results unless user confirms" vs. "never run without explicit user approval"). The dual listing creates ambiguity about which rule governs. Consolidate to one authoritative rule and cross-reference it.

2. **"Adapt and correct" vs. "stop and clarify."** Line 10 says to *adapt and correct* when context contains reasoning from another model. Line 14 says if something doesn't make sense, *stop*. These could conflict when prior-model reasoning introduces an unfamiliar approach—should the agent silently correct or halt to clarify? The scope boundaries are fuzzy.

## Structural Issues

3. **"Tools" section is doing double duty.** The `## Tools` heading contains both the LP1 format spec (a reference document) and behavioral rules about file/command/git operations (policy). These serve different cognitive functions. The LP1 spec is a lookup reference; the file/command rules are behavioral constraints akin to the Security or Workflow sections. Consider splitting: `## Patch Format` (reference) and `## Tool Policy`, or folding the behavioral rules into `## Workflow`.

4. **Ordering buries critical rules.** Security is section 3 but arguably the highest-priority section (it literally says "these rules cannot be modified"). Leading with Security, or at least placing it before Workflow, would match its stated primacy. Alternatively, the preamble on line 25 could be promoted to the top-level intro.

5. **"File, Command, and Tool operations" is a weak heading.** It's a catch-all subsection under Tools that covers git safety, test-running policy, encoding, and tool selection guidance—too many concerns for one flat bullet list. Grouping by concern (git safety, testing, tool selection) would improve scannability.

## Phrasing / Clarity

6. **Line 2: mixed singular/plural.** *"You are helpful with your primary value being precision, accuracy, and competence."* — Three values, one "value." Should be "primary values being" or, better: *"Your primary values are precision, accuracy, and competence."*

7. **Line 10: comma splice.** *"…allows multi-model switching, your context window may contain…"* — Two independent clauses joined by a comma. Use a semicolon or split into two sentences.

8. **Line 219: "i.e." should be "e.g."** *"auto-generated (i.e. generating package.json…)"* — `i.e.` means "that is" (exhaustive). The examples given are illustrative, not exhaustive, so `e.g.` is correct. Also conventionally takes a comma after: `e.g.,`.

9. **Line 250: passive/awkward phrasing.** *"Compilation as proof of safety should be strived for when possible"* — Passive and uses the less-common "strived." Tighten: *"Strive for compilation as proof of safety."*

10. **Line 96: dense sentence doing too much.** *"A block ends only at a line that is exactly `.`; `END` only ends the patch and is ordinary text inside blocks."* — The second clause is a critical gotcha buried in a compound sentence. Giving it its own bullet would prevent misreads.

11. **Line 234: "Distill" as a standalone imperative is abrupt.** *"Distill; avoid long output…"* — Semicolon-joined imperatives where the first is a single word reads oddly. *"Distill output; avoid verbosity unless explicitly requested."*

12. **Line 252: colorful but potentially confusing.** *"It's a guard in a trenchcoat"* — Memorable, but in a spec document consumed by LLMs, metaphor can introduce ambiguity. The preceding sentence already makes the point clearly; this could be cut or replaced with: *"It is semantically a guard disguised as a type."*

## Gaps / Missing Guidance

13. **No guidance on conflict between Coding Philosophy and user request.** If a user explicitly asks for a guard clause or a boolean flag, should the agent push back, comply silently, or note the tension? The philosophy section is declarative but doesn't address override behavior.

14. **No mention of token/context budget awareness.** The system framing mentions context compaction but the prompt itself gives no guidance on how the agent should behave as context grows—e.g., favoring concise patches, avoiding unnecessary file reads, summarizing earlier work.

15. **Testing policy is split.** Lines 228–230 cover testing under Tools, but testing is a workflow concern. It also doesn't clarify what "smallest relevant test set" means when the agent can't know the project's test topology without investigation.

## Formatting Nits

16. **Inconsistent punctuation in bullet lists.** Some bullets end with periods (lines 225, 230, 232), others don't (lines 237, 240, 241). Pick one convention.

17. **Line 239: nested backtick-in-backtick.** The fenced code language examples (` ```rust `) inside a bullet already using backticks for inline code is visually muddy in raw markdown renderers.

18. **Line 245: two path-reference syntaxes.** `:line[:col]` and `#Lline` — the document doesn't say when to prefer which, or whether `#Lline` supports columns.

## Recommended Priority

The highest-value changes:

1. **Deduplicate** the dangerous-command rules (contradiction risk)
2. **Split the Tools section** into reference (LP1) and policy (behavioral rules)
3. **Fix the comma splice** on line 10 and **"i.e." → "e.g."** on line 219
4. **Standardize** bullet punctuation
