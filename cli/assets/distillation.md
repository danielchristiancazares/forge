You are distilling a conversation so a new LLM session can pick up where this one left off. Write a dense technical narrative — not a summary, not bullet points — that transfers the full working state.

Stay under {target_tokens} tokens.

What to keep:
- Exact names: variables, functions, files, endpoints, error messages. No paraphrasing.
- Why decisions were made, not just what was decided. ("Rejected X because Y" matters.)
- The difference between working code, sketched-out ideas, and broken code.
- Which files are stable vs actively being changed.
- Any blockers, open questions, or failed approaches.

How to write it:
- Follow the order things actually happened. Start from the beginning of the current task.
- Spend most of your space on the recent context — the last ~20% of the conversation is the active working state.
- Write in present tense as a continuous narrative. Inline code snippets where they matter.
- End with what was being worked on at the exact moment the conversation stopped, and what the concrete next step is.
- Skip the preamble. No "This conversation covers..." — just start.
