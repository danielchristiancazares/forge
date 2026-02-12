# Distillation Design

## Problem

Context window exhaustion. Occurs either mid-turn (assistant response pushed past the limit) or on prompt submission (user message won't fit).

## Goal

Summarize the chat history into a single message.

## End State

The API sees a fresh session: summary as the opening message, then any new user input. The TUI is unchanged — the user still sees their full conversation history. Compaction is transparent.

## Behavior

1. Context is full (or the next message won't fit).
2. Send the entire conversation history to a cheap/fast distiller model.
3. Distiller produces a structured handoff document.
4. Mark a compaction point in the history.
5. Inject the handoff document as the opening context message.
6. From this point forward, API calls include only the summary + messages after the compaction point.
7. If a user prompt triggered compaction, send it as the first user message after the summary.
8. The TUI display is untouched — the user sees their full conversation.

## Two views of the same history

- **TUI view**: All messages, old and new, in chronological order. No visible break.
- **API view**: Summary message, then only messages created after the compaction point.

## Invariants

- **Compaction replaces what the API sees.** Messages before the compaction point are display-only. The API never sees them again.
- **Compaction output always fits.** The distiller's max output tokens (64k Haiku, 128k GPT-5-nano, 65k Gemini) is always smaller than any main model's context window (400k–1M). The summary cannot overflow a fresh window.
- **One transition.** A session is either pre-compaction or post-compaction. There is no intermediate state where some messages are compressed and some aren't.
- **Compaction can occur more than once.** If the post-compaction session itself fills up, the entire API-visible history (previous summary + all messages since) is compacted into a new summary. Each compaction is a fresh operation on the current API view — not a retry of a previous compaction.

## Non-goals

- Partial/incremental distillation of message subsets
- Hierarchical re-distillation (distilling a distillate alongside new messages IS just another compaction)
- Target token budgeting or ratio computation
