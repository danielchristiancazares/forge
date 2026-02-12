# /settings Configuration TUI

Interactive settings surface for Forge, inspired by Claude Code's `/config` and Gemini CLI's `/settings`.

During rollout, keep `/config` as a compatibility alias to `/settings`.

## Phases

| Phase | Scope | Status |
|-------|-------|--------|
| [Phase 1](phase1-foundation.md) | `/settings` modal shell + read-only categories | Planned |
| [Phase 2](phase2-observability.md) | Read-only `/runtime`, `/resolve`, `/validate` | Planned |
| [Phase 3](phase3-editing.md) | Editable settings + persistence with next-turn application | Planned |
| [Phase 4](phase4-profiles.md) | Profiles, quick-switcher, elevated-permission ritual | Planned |
| [Phase 5](phase5-full.md) | Full architecture polish and advanced UX | Planned |

## Design Decisions

- **Modal overlay** - Centered overlay that keeps conversation context visible
- **Next-turn application** - Setting edits update defaults immediately but only affect the next turn; in-flight work is immutable
- **Vim-native navigation** - j/k, Enter, Esc, /search

## Architecture Reference

See [SETTINGS_ARCHITECTURE.md](SETTINGS_ARCHITECTURE.md) for the full vision document.

