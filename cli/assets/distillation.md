You are compressing a conversation into a handoff document. A new LLM session will receive ONLY this document and access to the repo. No other context.

Fill in the template below. Leave any section empty with "None." if it doesn't apply. Do not add sections, commentary, or preamble.

GOAL
[What is being built or solved, what "done" looks like, and any governing spec/doc by path. 1-3 sentences.]

STATE
Repo: [name or path]
Branch: [branch + short commit, or UNKNOWN]
Build/test: [exact commands to build, test, run — include env vars if needed]
Status: [what passes, what fails, exact failing test names or error strings]

Changed files (ONLY files with edits in this conversation — omit files that were read for reference):
- `[path]`: [role in the system]. Changed: [symbol names — functions, types, constants]. Working: [observable behavior]. Broken: [observable behavior + verbatim error text, or "N/A"].
[Repeat per file.]

DECISIONS
- [What was chosen] over [what was rejected], because [why, 1-2 sentences]. [Failed attempts and what went wrong, if any.]
[Repeat per decision. Omit if no meaningful decisions were made.]

BLOCKERS
- [Symptom + verbatim error text]. Repro: [command/steps]. Suspect: [path + symbol]. Next probe: [concrete experiment].
[Repeat per blocker. "None." if no blockers.]

VERBATIM USER CONSTRAINTS
[Copy user messages that define requirements, constraints, or corrections. Paste exactly as written. Summarize any message over 400 words. "None." if the conversation was purely exploratory.]

NEXT
Stopped at: [what was in progress when the conversation ended]
Resume by: [ONE concrete action — e.g. "edit path::symbol to do X, then run Y"]
Then: [follow-on steps, if any]

Rules:
- NEVER quote code. Reference by path, line number, or symbol name.
- Only list files that were EDITED, not files that were read. If unsure, omit.
- Preserve edge cases, error paths, and non-obvious constraints. Mark uncertainty with UNKNOWN.
- No pronouns without a referent in the same sentence.
- Omit anything re-derivable from the repo in under a minute.
- You MUST stay under 1200 words or less.
