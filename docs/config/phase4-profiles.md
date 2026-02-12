# Phase 4: Profiles + Quick Switch

**Goal**: Introduce named configuration bundles and fast mode switching.

## Scope

- Profile CRUD (create, duplicate, edit, delete)
- Quick-switcher (`Ctrl+p`)
- Elevated-permission activation ritual for risky profiles
- Persist active profile selection

## Behavior

- Profile activation updates defaults immediately
- Active in-flight turn remains immutable
- Next turn resolves against the newly active profile

## Profile Storage

- Profiles live in `~/.forge/profiles/*.toml`
- Active profile metadata lives in `~/.forge/state.toml`
- Global config remains in `~/.forge/config.toml`

## UI Mockups

### Profile List

```
┌─ Settings › Profiles ─────────────────────────────────────────────┐
│  ● deep-work                                           active     │
│  ○ quick                                                          │
│  ○ research                                                ⚠      │
│  ○ yolo                                                   ⚠⚠      │
│                                                                   │
│  Enter activate   e edit   d duplicate   n new   x delete         │
└───────────────────────────────────────────────────────────────────┘
```

### Elevated Activation Ritual

```
┌─ Confirm Activate: yolo ──────────────────────────────────────────┐
│  ⚠ This profile has elevated permissions                          │
│  Type YOLO to activate:                                           │
│  > _                                                              │
└───────────────────────────────────────────────────────────────────┘
```

## Verification

1. `just verify` passes
2. Create and activate a non-elevated profile
3. Confirm next turn resolves to that profile
4. Activate an elevated profile and verify ritual gate
5. Restart Forge and verify active profile persists

