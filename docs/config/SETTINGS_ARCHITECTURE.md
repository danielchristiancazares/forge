# Forge Settings Architecture v2

---

## Top-Level Navigation

```
┌─ Settings ────────────────────────────────────────────────────────┐
│                                                                   │
│  Type to filter...                                                │
│                                                                   │
│  ▸ Providers                                    2 verified        │
│    Models                                       4 usable          │
│    Model Overrides                              chat/code split   │
│    Context                                      128k default      │
│    Tools                                        12 enabled        │
│    Keybindings                                  vim               │
│    Profiles                                     4 saved           │
│    History                                      30 days           │
│    Appearance                                   tokyo-night       │
│                                                                   │
│  ─────────────────────────────────────────────────────────────    │
│    Validation                                   1 error, 2 warns  │
│    Resolution                                   show cascade      │
│                                                                   │
│  Enter select   / filter   q quit                                 │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Providers

```
┌─ Settings › Providers ────────────────────────────────────────────┐
│                                                                   │
│  ANTHROPIC                                          ● Verified    │
│    API Key                                    sk-ant-***wxyz      │
│    Base URL                           https://api.anthropic.com   │
│    Rate Limit Mode                            Respect Headers     │
│    Last Verified                                        2h ago    │
│                                                                   │
│  OPENAI                                            ● Configured   │
│    API Key                                       sk-***abcd       │
│    Base URL                             https://api.openai.com    │
│    Org ID                                                unset    │
│    Rate Limit Mode                            Respect Headers     │
│    Last Verified                                         never    │
│                                                                   │
│  GOOGLE                                              ○ Not Set    │
│    API Key                                    Press e to add      │
│    Region                                        us-central1      │
│                                                                   │
│  Provider Enforcement:                                            │
│    (•) Explicit Model Must Match Provider                         │
│    ( ) Allow Cross-Provider Alias                                 │
│                                                                   │
│  e edit   t test now   n new provider   Esc back                  │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Models

```
┌─ Settings › Models ───────────────────────────────────────────────┐
│                                                                   │
│  Type to filter...                                                │
│                                                                   │
│  ANTHROPIC                                          ● Verified    │
│  ─────────────────────────────────────────────────────────────    │
│  ▸ claude-opus-4-5-20251101      ✓ ★chat   256k       $5/$25     │
│    claude-opus-4-6               ✓         1M         $5/$25      │
│    claude-sonnet-4-5-20250929    ✓         200k       $3/$15      │
│    claude-haiku-4-5              ✓         200k    $0.25/$1.25    │
│                                                                   │
│  OPENAI                                            ● Configured   │
│  ─────────────────────────────────────────────────────────────    │
│    gpt-5.3-codex                 ⊘ ★code   256k       $6/$18      │
│    gpt-5.2-pro                   ⊘         128k       $2/$10      │
│    gpt-5.2                       ⊘         128k    $0.50/$1.50    │
│                       ⊘ Provider not verified - run test first    │
│                                                                   │
│  GOOGLE                                              ○ Not Set    │
│  ─────────────────────────────────────────────────────────────    │
│    gemini-3-pro                  ✗         1M        $1.25/$5     │
│    gemini-3-flash                ✗         1M     $0.075/$0.30    │
│                       ✗ Provider not configured                   │
│                                                                   │
│  ✓ usable   ⊘ degraded   ✗ blocked                                │
│                                                                   │
│  Enter info   c set chat default   d set code default   Esc back  │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Model Overrides

```
┌─ Settings › Model Overrides ──────────────────────────────────────┐
│                                                                   │
│  Mode-Specific Models                                             │
│  ─────────────────────────────────────────────────────────────    │
│  Chat Model                         claude-opus-4-5-20251101      │
│  Code Model                         gpt-5.3-codex                 │
│                                                                   │
│  Inference Parameters                                             │
│  ─────────────────────────────────────────────────────────────    │
│  Temperature (Chat)                 0.2                           │
│  Temperature (Code)                 0.0                           │
│  Max Tokens Per Response            4096                          │
│                                                                   │
│  Large File Strategy (>20k LOC)                                   │
│  ─────────────────────────────────────────────────────────────    │
│    (•) Switch to High-Context Model                               │
│    ( ) Chunk + Summarize                                          │
│    ( ) Abort                                                      │
│                                                                   │
│  ─────────────────────────────────────────────────────────────    │
│  Effective Resolution                                             │
│    Chat → claude-opus-4-5-20251101       (profile: deep-work)     │
│    Code → gpt-5.3-codex                  (profile: deep-work)     │
│                                                                   │
│  ↑↓ navigate   Enter edit   D show full cascade   Esc back        │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Profile    Layer: Settings    Dirty: no                   │
└───────────────────────────────────────────────────────────────────┘
```

---

## Resolution Cascade (`/resolve` or `D` from Model Overrides)

```
┌─ Resolution Cascade ──────────────────────────────────────────────┐
│                                                                   │
│  Chat Model                                                       │
│  ─────────────────────────────────────────────────────────────    │
│    Global              gpt-5.2-pro                                │
│    Project             unset                                      │
│    Profile             claude-opus-4-5-20251101    ← winner       │
│    Session             unset                                      │
│                                                                   │
│  Code Model                                                       │
│  ─────────────────────────────────────────────────────────────    │
│    Global              gpt-5.2-pro                                │
│    Project             unset                                      │
│    Profile             gpt-5.3-codex               ← winner       │
│    Session             unset                                      │
│                                                                   │
│  Temperature (Chat)                                               │
│  ─────────────────────────────────────────────────────────────    │
│    Global              0.7                                        │
│    Project             unset                                      │
│    Profile             0.2                         ← winner       │
│    Session             unset                                      │
│                                                                   │
│  Temperature (Code)                                               │
│  ─────────────────────────────────────────────────────────────    │
│    Global              0.0                         ← winner       │
│    Project             unset                                      │
│    Profile             unset                                      │
│    Session             unset                                      │
│                                                                   │
│  Context Limit                                                    │
│  ─────────────────────────────────────────────────────────────    │
│    Global              128k                                       │
│    Project             256k                        ← winner       │
│    Profile             unset                                      │
│    Session             unset                                      │
│                                                                   │
│  j/k scroll   Enter jump to setting   Esc back                    │
├───────────────────────────────────────────────────────────────────┤
│  Session Config Hash: 8f3a21c                                     │
└───────────────────────────────────────────────────────────────────┘
```

---

## Context

```
┌─ Settings › Context ──────────────────────────────────────────────┐
│                                                                   │
│  Context Management                                               │
│  ─────────────────────────────────────────────────────────────    │
│  Default Limit                              128k tokens           │
│  Distill Threshold                          80% capacity          │
│  Distill Strategy                           ▸ summarize           │
│                                               truncate_old        │
│                                               hybrid              │
│                                                                   │
│  Auto-Attach Rules                                                │
│  ─────────────────────────────────────────────────────────────    │
│  INVARIANT_FIRST_ARCHITECTURE.md    ✓       always                │
│  CLAUDE.md                          ✓       always                │
│  AGENTS.md                          ✓       on_tag:@agents        │
│  RUST_PATTERNS.md                   ✓       on_path:*.rs          │
│  DEBUG.md                           ✓       on_tool:bash          │
│  REVIEW.md                          ✓       on_mode:chat          │
│                                                                   │
│  + Add rule...                                                    │
│                                                                   │
│  Rule types: always | never | on_tag:<t> | on_path:<glob>         │
│              on_tool:<tool> | on_mode:chat|code                   │
│                                                                   │
│  ↑↓ navigate   Enter edit rule   a add   x delete   Esc back      │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Tools

```
┌─ Settings › Tools ────────────────────────────────────────────────┐
│                                                                   │
│  Permission Levels: disabled < project_only < allowlist <         │
│                     workspace < unrestricted                      │
│                                                                   │
│  Code Execution                                                   │
│  ─────────────────────────────────────────────────────────────    │
│  ✓ bash              Run shell commands              workspace    │
│  ✓ python            Execute Python scripts          workspace    │
│  ✓ node              Execute JavaScript              workspace    │
│                                                                   │
│  File Operations                                                  │
│  ─────────────────────────────────────────────────────────────    │
│  ✓ read              Read file contents           unrestricted    │
│  ✓ write             Create/modify files          project_only    │
│  ✓ glob              Search file patterns         unrestricted    │
│    delete            Remove files                     disabled    │
│                                                                   │
│  Network                                                          │
│  ─────────────────────────────────────────────────────────────    │
│  ✓ fetch             HTTP requests                   allowlist    │
│    browser           Full browser control            disabled     │
│                                                                   │
│  Paths:                                                           │
│    Workspace: ~/projects    Allowlist: ~/.forge/net.allow         │
│                                                                   │
│  space toggle level   e edit allowlist   Esc back                 │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Keybindings

```
┌─ Settings › Keybindings ──────────────────────────────────────────┐
│                                                                   │
│  Preset                                         ▸ vim             │
│                                                   emacs           │
│                                                   minimal         │
│                                                   custom          │
│                                                                   │
│  Navigation                                                       │
│  ─────────────────────────────────────────────────────────────    │
│  Insert mode                                    i                 │
│  Normal mode                                    Esc               │
│  Command palette                                /                 │
│  File picker                                    @                 │
│  Model picker                                   Ctrl+m            │
│  Profile switcher                               Ctrl+p            │
│                                                                   │
│  Actions                                                          │
│  ─────────────────────────────────────────────────────────────    │
│  Send message                                   Enter             │
│  Cancel generation                              Ctrl+c            │
│  Rewind last                                    u                 │
│  Accept tool call                               y                 │
│  Reject tool call                               n                 │
│                                                                   │
│  Enter rebind   r reset preset   Esc back                         │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Profiles

```
┌─ Settings › Profiles ─────────────────────────────────────────────┐
│                                                                   │
│  ● deep-work                                           active     │
│    Chat: claude-opus-4-5    Code: gpt-5.3-codex                   │
│    Context: 256k · distill at 80%                                 │
│    Auto-attach: IFA.md, CLAUDE.md (always)                        │
│    Tools: workspace permissions                                   │
│                                                                   │
│  ○ quick                                                          │
│    Chat: claude-haiku-4-5   Code: claude-haiku-4-5                │
│    Context: 32k · truncate old                                    │
│    Auto-attach: none                                              │
│    Tools: project_only                                            │
│                                                                   │
│  ○ research                                                       │
│    Chat: gpt-5.2-pro        Code: gpt-5.2-pro                     │
│    Context: 128k · summarize                                      │
│    Auto-attach: AGENTS.md (on_tag)                                │
│    Tools: fetch allowlist, browser enabled                ⚠       │
│                                                                   │
│  ○ yolo                                                   ⚠⚠      │
│    Chat: claude-opus-4-5    Code: claude-opus-4-5                 │
│    Context: 256k                                                  │
│    Tools: unrestricted                                            │
│                                                                   │
│  Enter activate   e edit   d duplicate   n new   Esc back         │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Profile Activation Ritual (for ⚠ profiles)

```
┌─ Confirm Activate: yolo ──────────────────────────────────────────┐
│                                                                   │
│  ⚠ This profile has elevated permissions:                         │
│                                                                   │
│    • Tools set to unrestricted                                    │
│    • Browser tool enabled                                         │
│                                                                   │
│  Type YOLO to activate:                                           │
│                                                                   │
│  > _                                                              │
│                                                                   │
│  Esc cancel                                                       │
└───────────────────────────────────────────────────────────────────┘
```

---

## Profile Quick-Switcher (Ctrl+p)

```
┌─ Switch Profile ──────────────────────────────────────────────────┐
│                                                                   │
│  1.  ● deep-work       opus-4-5/codex · 256k                      │
│  2.  ○ quick           haiku/haiku · 32k                          │
│  3.  ○ research        gpt-5.2-pro · 128k · ⚠ browser             │
│  4.  ○ yolo            opus-4-5 · 256k · ⚠⚠ unrestricted          │
│                                                                   │
│  1-4 quick pick   Enter confirm   Esc cancel                      │
└───────────────────────────────────────────────────────────────────┘
```

---

## Validation Dashboard

```
┌─ Validation ──────────────────────────────────────────────────────┐
│                                                                   │
│  Errors (1)                                                       │
│  ─────────────────────────────────────────────────────────────    │
│  ✗ Google API key missing                                         │
│      Required by: gemini-3-pro, gemini-3-flash                    │
│      Fix: Settings › Providers › Google                           │
│                                                                   │
│  Warnings (2)                                                     │
│  ─────────────────────────────────────────────────────────────    │
│  ⚠ OpenAI provider not verified                                   │
│      Key configured but never tested                              │
│      Fix: Settings › Providers › OpenAI › t test                  │
│                                                                   │
│  ⚠ Browser tool enabled in profile "research"                     │
│      Elevated permission active                                   │
│      Info: Settings › Profiles › research                         │
│                                                                   │
│  Healthy (14)                                                     │
│  ─────────────────────────────────────────────────────────────    │
│  ✓ Anthropic provider verified                                    │
│  ✓ Chat model usable                                              │
│  ✓ Code model usable (degraded - provider unverified)             │
│  ✓ Context settings valid                                         │
│  ... 10 more                                                      │
│                                                                   │
│  Enter jump to fix   h toggle healthy   Esc back                  │
├───────────────────────────────────────────────────────────────────┤
│  Last validated: just now    Auto-refresh: on settings change     │
└───────────────────────────────────────────────────────────────────┘
```

---

## History

```
┌─ Settings › History ──────────────────────────────────────────────┐
│                                                                   │
│  Retention                                                        │
│  ─────────────────────────────────────────────────────────────    │
│  Keep conversations for                         30 days           │
│  Keep journal entries for                       7 days            │
│  Max storage                                    500 MB            │
│                                                                   │
│  Privacy                                                          │
│  ─────────────────────────────────────────────────────────────    │
│  Store API keys in                              system keyring    │
│  Log tool executions                            ✓                 │
│  Log API requests                               ✓ headers only    │
│  Include thinking blocks in export              ✓           ⚠     │
│                                                                   │
│  Storage: ~/.forge/history    Using: 127 MB of 500 MB    [██░░░]  │
│                                                                   │
│  c clear all history   x export   Esc back                        │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Appearance

```
┌─ Settings › Appearance ───────────────────────────────────────────┐
│                                                                   │
│  Theme                                                            │
│  ─────────────────────────────────────────────────────────────    │
│  Color Scheme                                   ▸ tokyo-night     │
│                                                   dracula         │
│                                                   catppuccin      │
│                                                   gruvbox         │
│                                                   solarized       │
│                                                   monochrome      │
│                                                                   │
│  Layout                                                           │
│  ─────────────────────────────────────────────────────────────    │
│  Density                                        comfortable       │
│  Show thinking blocks                           ✓ collapsed       │
│  Show token counts                              ✓                 │
│  Show timestamps                                                  │
│  Context meter                                  bottom-right      │
│  Footer compass                                 ✓                 │
│                                                                   │
│  Markdown                                                         │
│  ─────────────────────────────────────────────────────────────    │
│  Render markdown                                ✓                 │
│  Syntax highlighting                            ✓                 │
│  Code block theme                               match scheme      │
│                                                                   │
│  ↑↓ navigate   Enter toggle/select   p preview   Esc back         │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Global    Layer: Settings    Dirty: no                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Runtime (`/runtime`)

```
┌─ Runtime ─────────────────────────────────────────────────────────┐
│                                                                   │
│  Session                                                          │
│  ─────────────────────────────────────────────────────────────    │
│  Active Profile                     deep-work                     │
│  Session Config Hash                8f3a21c                       │
│  Started                            14 minutes ago                │
│                                                                   │
│  Current Mode: code                                               │
│  ─────────────────────────────────────────────────────────────    │
│  Active Model                       gpt-5.3-codex                 │
│  Provider                           OpenAI (configured, unverified)│
│  Temperature                        0.0                           │
│                                                                   │
│  Context                                                          │
│  ─────────────────────────────────────────────────────────────    │
│  Usage                              38% (28k / 200k)   [████░░░░] │
│  Distill Threshold                  80% (160k)                    │
│  Auto-Attached                      IFA.md, CLAUDE.md             │
│                                                                   │
│  Health                                                           │
│  ─────────────────────────────────────────────────────────────    │
│  Rate Limit State                   Healthy                       │
│  Last API Call                      12s ago (success)             │
│  Last Error                         None                          │
│                                                                   │
│  Session Overrides                                                │
│  ─────────────────────────────────────────────────────────────    │
│    (none - using profile defaults)                                │
│                                                                   │
│  r refresh   D show resolution cascade   Esc close                │
├───────────────────────────────────────────────────────────────────┤
│  Scope: Session    Layer: Runtime    Read-only                    │
└───────────────────────────────────────────────────────────────────┘
```

---

## Data Model

```rust
// Config layers (mutable)
struct GlobalConfig { ... }
struct ProjectConfig { ... }  // .forge/project.toml
struct Profile { ... }
struct SessionOverrides { ... }

// Resolution (immutable snapshot)
struct ResolvedConfig {
    chat_model: ModelSpec,
    code_model: ModelSpec,
    temperature_chat: f32,
    temperature_code: f32,
    context_limit: usize,
    // ... all resolved values
}

impl ResolvedConfig {
    fn resolve(
        global: &GlobalConfig,
        project: Option<&ProjectConfig>,
        profile: &Profile,
        session: &SessionOverrides,
    ) -> Self { ... }
    
    fn hash(&self) -> ConfigHash { ... }
}

// Runtime (read-only view for the active turn)
struct RuntimeState {
    config: ResolvedConfigSnapshot,  // immutable for active turn; edits apply next turn
    context_used: usize,
    rate_limit_state: RateLimitState,
    last_error: Option<Error>,
    // ... live telemetry
}

// Validation
struct ValidationReport {
    errors: Vec<ValidationError>,
    warnings: Vec<ValidationWarning>,
    healthy: Vec<ValidationCheck>,
}

fn validate(
    resolved: &ResolvedConfig,
    providers: &ProviderRegistry,
) -> ValidationReport { ... }
```

---

## Command Summary

| Command | Action |
|---------|--------|
| `/settings` | Open settings root |
| `/runtime` | Show live session state |
| `/resolve` | Show full resolution cascade |
| `/validate` | Show validation dashboard |
| `Ctrl+p` | Quick profile switcher |
| `Ctrl+m` | Quick model picker |

---

## Design Principles

1. **Configured ≠ Verified** — Show what's tested, not just what's present
2. **Usable ≠ Listed** — Glyph shows what you can actually use right now
3. **Effective Resolution Visible** — Show what won and which layer
4. **Validation First-Class** — All errors in one place, with fix paths
5. **Immutable Turn Snapshots** — Runtime reads frozen config per active turn
6. **Explicit Elevated Permissions** — Ritual required for dangerous profiles
7. **Footer Compass** — Always know scope, layer, dirty state
8. **Esc = back, q = quit** — Consistent navigation

---

This is the complete v2. Ready for implementation.
