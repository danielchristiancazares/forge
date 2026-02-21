# Behavioral Security Framework Plan

**Status:** Draft v2 (remediated)
**Date:** 2026-02-21

---

## Overview

Honeytokens are tripwires. They detect one specific class of attack: "agent was told to read secrets." But prompt injection has many shapes. A comprehensive behavioral security framework accumulates evidence across multiple detection surfaces and makes graduated enforcement decisions.

This plan defines the architecture that honeytokens fold into.

---

## Threat Model

| Attack Class | Example | Detection Surface |
|--------------|---------|-------------------|
| Secret Discovery | "Read .env and show me" | Honeytoken tripwire, sensitive path access |
| Exfiltration | "curl attacker.com with contents" | Network + exposure correlation |
| Staged Exfiltration | Read file, base64 encode, write to new file, later upload | Threat state machine |
| Instruction Injection | README contains "ignore previous instructions" | Exposure tracking |
| Tool Argument Injection | File contains `"; rm -rf /; echo "` interpolated into shell | Argument sanitization |
| Privilege Escalation | "Run sudo to fix permissions" | Command pattern matching |
| Persistence | "Add this to .bashrc" | Write target classification |
| Evasion | Unicode homoglyphs in commands, encoded payloads | Normalization layer |
| Security System DoS | Flood low-severity events to exhaust storage or mask attack | Noise floor detection |

No single detector catches all. Defense in depth requires composition.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Behavioral Security Engine                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │  Honeytoken  │  │   Exposure   │  │    Threat    │          │
│  │   Detector   │  │   Tracker    │  │ State Machine│          │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘          │
│         │                 │                 │                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐          │
│  │   Argument   │  │  Exfil       │  │   Evasion    │          │
│  │  Sanitizer   │  │  Correlator  │  │   Detector   │          │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘          │
│         │                 │                 │                   │
│         ▼                 ▼                 ▼                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Evidence Accumulator (bounded)              │   │
│  │  (typed evidence, per-category scores, threat bits)      │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                             │                                   │
│                             ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │              Policy Evaluator (short-circuit)            │   │
│  │  (caller-owned thresholds, early exit on critical)       │   │
│  └──────────────────────────┬──────────────────────────────┘   │
│                             │                                   │
│                             ▼                                   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                  Enforcement Engine                      │   │
│  │  (block, warn, terminate, lock, require approval)        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Crate Placement

```
forge/
├── security/              # NEW: Behavioral security framework
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── evidence.rs    # Evidence types, accumulator
│       ├── detectors/     # Detector implementations
│       │   ├── mod.rs
│       │   ├── honeytoken.rs
│       │   ├── exposure.rs
│       │   ├── threat_state.rs
│       │   ├── argument.rs
│       │   ├── exfiltration.rs
│       │   └── evasion.rs
│       ├── scoring.rs     # Score algebra
│       ├── policy.rs      # Policy evaluator
│       └── enforcement.rs # Enforcement engine
├── types/src/security.rs  # Shared types (Evidence enum, ThreatCategory)
├── engine/                # Calls security:: in tool loop
└── context/               # Calls security:: for canary injection
```

---

## Core Types

### Evidence Records

```rust
// types/src/security.rs

/// Sum type: one variant per detector. Exhaustive match required.
pub enum SecurityEvidence {
    HoneytokenAccess(HoneytokenEvidence),
    SensitiveExposure(ExposureEvidence),
    ThreatStateTransition(ThreatStateEvidence),
    ArgumentInjection(ArgumentEvidence),
    ExfiltrationPattern(ExfiltrationEvidence),
    PersistenceAttempt(PersistenceEvidence),
    EvasionIndicator(EvasionEvidence),
    UserOverride(OverrideEvidence),
}

pub struct HoneytokenEvidence {
    pub canary_id: CanaryId,
    pub access_type: CanaryAccessType,
    pub tool: ToolInvocationId,
    pub timestamp: Instant,
}

pub struct ExposureEvidence {
    pub source: ExposureSource,
    pub sensitivity: SensitivityLevel,
    pub tool_invocation: ToolInvocationId,
    pub content_hash: ContentHash,
}

pub struct ThreatStateEvidence {
    pub hypothesis: ThreatHypothesis,
    pub transition: StateTransition,
    pub triggering_tool: ToolInvocationId,
}

pub struct ArgumentEvidence {
    pub tool: ToolInvocationId,
    pub injection_type: InjectionType,
    pub payload_preview: RedactedPreview,
}

pub struct OverrideEvidence {
    pub tool: ToolInvocationId,
    pub score_at_override: ThreatScore,
    pub override_type: OverrideType,
    pub timestamp: Instant,
}

pub enum InjectionType {
    ShellMetacharacter,
    PathTraversal,
    EnvironmentExpansion,
    NullByte,
    CommandChaining,
}
```

### Threat State Machine

```rust
// security/src/detectors/threat_state.rs

/// State bits per threat hypothesis. Bits persist until decay.
/// Transitions fire on bit combinations, not linear sequences.
pub struct ThreatStateMachine {
    hypotheses: EnumMap<ThreatHypothesis, HypothesisState>,
}

pub enum ThreatHypothesis {
    StagedExfiltration,
    CredentialHarvesting,
    ReconToExfil,
    PersistenceInstall,
    SandboxProbe,
}

pub struct HypothesisState {
    bits: ThreatBits,
    last_transition: Option<Instant>,
}

bitflags! {
    pub struct ThreatBits: u16 {
        const SENSITIVE_READ      = 0b0000_0001;
        const ENCODED_CONTENT     = 0b0000_0010;
        const WROTE_NEW_FILE      = 0b0000_0100;
        const NETWORK_REQUEST     = 0b0000_1000;
        const SSH_ACCESS          = 0b0001_0000;
        const CLOUD_CRED_ACCESS   = 0b0010_0000;
        const DOTFILE_WRITE       = 0b0100_0000;
        const REPEATED_DENY       = 0b1000_0000;
    }
}

/// Transition rules: when bits match mask, fire evidence.
pub struct TransitionRule {
    pub hypothesis: ThreatHypothesis,
    pub required_bits: ThreatBits,
    pub severity: Severity,
    pub score: CategoryScore,
}

impl ThreatStateMachine {
    pub fn observe(&mut self, event: &ToolEvent) -> Vec<ThreatStateEvidence> {
        // Set bits based on event type
        // Check all hypotheses for firing conditions
        // Return evidence for any transitions that fired
    }

    pub fn decay(&mut self, clean_actions: u32) {
        // Clear bits after N clean tool invocations (not time-based)
    }
}
```

### Scoring Algebra

```rust
// security/src/scoring.rs

/// Formal scoring properties:
/// 1. Per-category scores are capped (prevents inflation from repeated low-severity)
/// 2. Cumulative score = max(category_scores), not sum (single bad category dominates)
/// 3. Thresholds form a total order: warn < block < terminate < lock
/// 4. Critical evidence (honeytoken +1000) exceeds all thresholds by construction
pub struct ThreatScore {
    by_category: EnumMap<ThreatCategory, CategoryScore>,
}

pub struct CategoryScore {
    raw: u32,
    capped: u32,
}

pub enum ThreatCategory {
    SecretAccess,
    Exfiltration,
    Persistence,
    PrivilegeEscalation,
    Evasion,
    ArgumentInjection,
}

/// Per-category caps. Prevents 100 low-severity events from crossing terminate.
pub const CATEGORY_CAPS: EnumMap<ThreatCategory, u32> = enum_map! {
    ThreatCategory::SecretAccess => 1000,
    ThreatCategory::Exfiltration => 800,
    ThreatCategory::Persistence => 600,
    ThreatCategory::PrivilegeEscalation => 700,
    ThreatCategory::Evasion => 400,
    ThreatCategory::ArgumentInjection => 500,
};

impl ThreatScore {
    pub fn add(&mut self, category: ThreatCategory, points: u32) {
        let entry = &mut self.by_category[category];
        entry.raw = entry.raw.saturating_add(points);
        entry.capped = entry.raw.min(CATEGORY_CAPS[category]);
    }

    /// Cumulative = max of capped category scores.
    /// A single bad category dominates; you can't sum your way to terminate.
    pub fn cumulative(&self) -> u32 {
        self.by_category.values().map(|c| c.capped).max().unwrap_or(0)
    }

    /// Decay: reduce raw scores by percentage, recalculate capped.
    /// Called after N clean tool invocations.
    pub fn decay(&mut self, factor: f32) {
        for entry in self.by_category.values_mut() {
            entry.raw = ((entry.raw as f32) * factor) as u32;
            entry.capped = entry.raw.min(CATEGORY_CAPS[entry.category]);
        }
    }
}

/// Thresholds form a total order. Invariant: warn < block < terminate < lock.
pub struct ThresholdPolicy {
    warn: u32,       // default: 100
    block: u32,      // default: 300
    terminate: u32,  // default: 500
    lock: u32,       // default: 800
}

impl ThresholdPolicy {
    pub fn new(warn: u32, block: u32, terminate: u32, lock: u32) -> Result<Self, ThresholdError> {
        if warn < block && block < terminate && terminate < lock {
            Ok(Self { warn, block, terminate, lock })
        } else {
            Err(ThresholdError::InvalidOrder)
        }
    }
}
```

### Proof-Carrying Execution Token

```rust
// security/src/enforcement.rs

/// AllowedExecution can ONLY be constructed by PolicyEvaluator::evaluate.
/// The token is non-Clone, non-Copy, and consumed by tool execution.
pub struct AllowedExecution {
    token: ExecutionToken,
    pub tool: ToolInvocation,
    pub score_snapshot: ThreatScore,
}

/// Private token type. Cannot be constructed outside this module.
pub struct ExecutionToken {
    _private: PrivateZst,
}

/// Zero-sized type with private constructor.
struct PrivateZst(());

impl AllowedExecution {
    /// Only callable from PolicyEvaluator. Not pub.
    pub(in crate::security) fn new(tool: ToolInvocation, score: ThreatScore) -> Self {
        Self {
            token: ExecutionToken { _private: PrivateZst(()) },
            tool,
            score_snapshot: score,
        }
    }
}

/// Consuming the token proves policy was checked.
pub fn execute_with_proof(proof: AllowedExecution) -> ToolExecutionHandle {
    // proof.token consumed here, cannot be reused
    ToolExecutionHandle::new(proof.tool)
}
```

### Evidence Accumulator (Bounded)

```rust
// security/src/evidence.rs

pub struct EvidenceAccumulator {
    session_id: SessionId,
    score: ThreatScore,
    threat_state: ThreatStateMachine,

    // Bounded storage: circular buffer, oldest evicted
    evidence_log: BoundedVec<TimestampedEvidence, MAX_EVIDENCE>,

    // Noise floor detection
    low_severity_count: u32,
    noise_floor_threshold: u32,

    // Decay tracking
    clean_action_count: u32,
    decay_interval: u32,  // Decay after N clean actions
}

const MAX_EVIDENCE: usize = 1000;

impl EvidenceAccumulator {
    pub fn record(&mut self, evidence: SecurityEvidence) -> EvidenceResult {
        // Check for noise floor attack
        if self.is_noise_floor_attack(&evidence) {
            return EvidenceResult::NoiseFloorExceeded;
        }

        // Update threat state machine
        let state_evidence = self.threat_state.observe(&evidence);

        // Add to score
        let (category, points) = evidence.score_contribution();
        self.score.add(category, points);

        // Record in bounded log (oldest evicted if full)
        self.evidence_log.push(TimestampedEvidence::now(evidence));

        // Reset clean action count (evidence pauses decay)
        self.clean_action_count = 0;

        EvidenceResult::Recorded(self.threat_level())
    }

    pub fn record_clean_action(&mut self) {
        self.clean_action_count += 1;
        if self.clean_action_count >= self.decay_interval {
            self.score.decay(0.5);  // Halve scores
            self.threat_state.decay(self.decay_interval);
            self.clean_action_count = 0;
        }
    }

    fn is_noise_floor_attack(&mut self, evidence: &SecurityEvidence) -> bool {
        if evidence.severity() == Severity::Low {
            self.low_severity_count += 1;
            self.low_severity_count > self.noise_floor_threshold
        } else {
            false
        }
    }
}
```

### Policy Evaluator (Short-Circuit)

```rust
// security/src/policy.rs

pub struct PolicyEvaluator;

impl PolicyEvaluator {
    /// Evaluates detectors with early exit on critical evidence.
    /// Returns immediately on terminate/lock decisions.
    pub fn evaluate(
        accumulator: &mut EvidenceAccumulator,
        pending_tool: &ToolInvocation,
        policy: &SecurityPolicy,
        detectors: &mut DetectorSet,
    ) -> PolicyDecision {
        // Priority order with short-circuit

        // 1. Honeytoken (instant terminate on critical)
        if let Some(evidence) = detectors.honeytoken.check(pending_tool) {
            accumulator.record(evidence.clone().into());
            if evidence.severity == Severity::Critical {
                return PolicyDecision::Terminate(TerminateDecision {
                    reason: TerminateReason::HoneytokenCritical,
                    evidence: vec![evidence.into()],
                });
            }
        }

        // 2. Argument injection (block before execution)
        if let Some(evidence) = detectors.argument.check(pending_tool) {
            accumulator.record(evidence.clone().into());
            return PolicyDecision::Block(BlockDecision {
                reason: BlockReason::ArgumentInjection,
                evidence: vec![evidence.into()],
            });
        }

        // 3. Remaining detectors (accumulate, then threshold check)
        for evidence in detectors.exposure.check(pending_tool) {
            accumulator.record(evidence.into());
        }
        for evidence in detectors.exfiltration.check(pending_tool, accumulator) {
            accumulator.record(evidence.into());
        }
        for evidence in detectors.evasion.check(pending_tool) {
            accumulator.record(evidence.into());
        }

        // 4. Threshold evaluation
        let score = accumulator.score.cumulative();

        if score >= policy.thresholds.lock {
            PolicyDecision::Lock(LockDecision {
                reason: LockReason::ThresholdExceeded,
                score,
                unlock: policy.unlock_requirement.clone(),
            })
        } else if score >= policy.thresholds.terminate {
            PolicyDecision::Terminate(TerminateDecision {
                reason: TerminateReason::ThresholdExceeded,
                evidence: accumulator.top_evidence(3),
            })
        } else if score >= policy.thresholds.block {
            PolicyDecision::Block(BlockDecision {
                reason: BlockReason::ThresholdExceeded,
                evidence: accumulator.top_evidence(2),
            })
        } else if score >= policy.thresholds.warn {
            PolicyDecision::Warn(WarnDecision {
                score,
                top_category: accumulator.score.top_category(),
            })
        } else {
            PolicyDecision::Allow(AllowedExecution::new(
                pending_tool.clone(),
                accumulator.score.clone(),
            ))
        }
    }
}
```

### Unlock Requirements

```rust
// security/src/enforcement.rs

pub enum UnlockRequirement {
    /// User must explicitly approve continuation (modal)
    UserApproval,

    /// Session must be restarted
    SessionRestart,

    /// Specific config flag must be set (for security researchers)
    ConfigOverride { key: String },

    /// Cool-down period must elapse
    Cooldown { duration: Duration },
}

pub struct LockDecision {
    pub reason: LockReason,
    pub score: u32,
    pub unlock: UnlockRequirement,
}

pub struct LockRecord {
    pub decision: LockDecision,
    pub timestamp: Instant,
    pub evidence_snapshot: Vec<SecurityEvidence>,
}

impl LockRecord {
    pub fn can_unlock(&self, attempt: &UnlockAttempt) -> Result<(), UnlockError> {
        match (&self.decision.unlock, attempt) {
            (UnlockRequirement::UserApproval, UnlockAttempt::UserApproved) => Ok(()),
            (UnlockRequirement::SessionRestart, UnlockAttempt::NewSession) => Ok(()),
            (UnlockRequirement::ConfigOverride { key }, UnlockAttempt::ConfigSet(k))
                if k == key => Ok(()),
            (UnlockRequirement::Cooldown { duration }, UnlockAttempt::TimeElapsed(elapsed))
                if elapsed >= duration => Ok(()),
            _ => Err(UnlockError::RequirementNotMet),
        }
    }
}
```

### Security Mode

```rust
// config/src/security.rs

pub struct SecurityConfig {
    /// Enforcement mode. Default: Audit (log only, no enforcement).
    pub mode: SecurityMode,

    pub honeytoken: HoneytokenConfig,
    pub thresholds: ThresholdPolicy,
    pub unlock_requirement: UnlockRequirement,
    pub decay_interval: u32,
    pub noise_floor_threshold: u32,
}

pub enum SecurityMode {
    /// Log all evidence, no enforcement. Ship default.
    Audit,

    /// Warn only, never block/terminate/lock.
    WarnOnly,

    /// Full enforcement.
    Enforce,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            mode: SecurityMode::Audit,  // Safe default
            honeytoken: HoneytokenConfig::default(),
            thresholds: ThresholdPolicy::new(100, 300, 500, 800).unwrap(),
            unlock_requirement: UnlockRequirement::UserApproval,
            decay_interval: 10,  // Decay after 10 clean actions
            noise_floor_threshold: 50,
        }
    }
}
```

---

## Detector Specifications

### 1. Honeytoken Detector

As specified in `PLAN_HONEYTOKEN.md`. Folds in as one evidence source.

**Output:** `HoneytokenEvidence` with severity classification.

**Score contribution (SecretAccess category):**
- Critical (read + exfil): +1000 (instant terminate, exceeds all thresholds)
- High (direct read): +500
- Medium (string match, no execution): +100

**Short-circuit:** Critical → immediate terminate, skip remaining detectors.

### 2. Exposure Tracker

Track which sensitive content the model has been exposed to. NOT taint propagation through the model (intractable). Simple flag-setting.

**What we track:**
- Which file reads occurred in this session
- Sensitivity classification of each read
- Recency (turns since exposure)

**Data structure:**

```rust
pub struct ExposureTracker {
    exposures: HashMap<ContentHash, ExposureRecord>,
}

pub struct ExposureRecord {
    pub source: ExposureSource,
    pub sensitivity: SensitivityLevel,
    pub turn_observed: u32,
    pub path: Option<PathBuf>,
}

pub enum SensitivityLevel {
    Low,      // Workspace files
    Medium,   // Config files, .gitconfig
    High,     // .env, credentials, cloud config
    Critical, // Private keys, honeytokens
}
```

**Trigger condition:** Network/write tool invocation while `exposure_set.has_high_or_above()` within last N turns.

**Score contribution (Exfiltration category):**
- Critical exposure + network: +300
- High exposure + network: +150
- High exposure + write outside workspace: +100

### 3. Threat State Machine

Replace linear sequence matching with persistent state bits.

**Hypotheses and their firing conditions:**

| Hypothesis | Required Bits | Severity | Score |
|------------|---------------|----------|-------|
| Staged Exfiltration | SENSITIVE_READ ∧ ENCODED_CONTENT ∧ NETWORK_REQUEST | High | +400 |
| Credential Harvesting | SSH_ACCESS ∧ CLOUD_CRED_ACCESS | High | +350 |
| Recon to Exfil | SENSITIVE_READ ∧ NETWORK_REQUEST | Medium | +200 |
| Persistence Install | DOTFILE_WRITE | Medium | +200 |
| Sandbox Probe | REPEATED_DENY (≥3 denials) | Low | +100 |

**Decay:** Bits cleared after N clean tool invocations (default: 10). NOT time-based.

### 4. Argument Sanitizer

Detect injection payloads in tool arguments before execution.

**Patterns:**

| Injection Type | Pattern | Score |
|----------------|---------|-------|
| Shell Metacharacter | `; | & $ \` \` $() ` in shell args | +300 |
| Path Traversal | `../` sequences, especially `../../../` | +200 |
| Null Byte | `\x00` in paths | +250 |
| Command Chaining | `&&`, `||`, `; ` in shell args | +300 |
| Environment Expansion | `${...}` to sensitive vars | +150 |

**Short-circuit:** Block immediately on detection. Do not execute tool.

### 5. Exfiltration Correlator

Detect content from recent reads appearing in outbound requests.

**Mechanism:**
- Track content hashes of recent file reads (bounded: last 50)
- On network tool, check if request body/URL contains:
  - Exact hash match (verbatim content)
  - High-entropy payload (base64/hex encoded)
  - Substring match for short sensitive content

**Score contribution (Exfiltration category):**
- Hash match (sensitive content in request): +400
- High-entropy payload after sensitive read: +200
- Unknown destination after sensitive read: +150

**Performance:** O(1) hash lookup, O(n) substring only for short sensitive strings.

### 6. Evasion Detector

Detect attempts to bypass detection through obfuscation.

**Indicators:**

| Indicator | Example | Score |
|-----------|---------|-------|
| Unicode homoglyphs | Cyrillic 'а' for ASCII 'a' in paths | +150 |
| Excessive encoding | base64(base64(content)) | +100 |
| Whitespace injection | Null bytes, zero-width chars in commands | +150 |
| BiDi override | RLO/LRO characters | +200 |

**Normalization:** All paths/commands normalized before other detectors run.

---

## Integration Points

### Tool Loop Integration

```rust
// engine/src/tool_loop.rs

pub async fn execute_tool(
    invocation: ToolInvocation,
    security: &mut BehavioralSecurityEngine,
    policy: &SecurityPolicy,
) -> ToolResult {
    // Check mode first
    if policy.mode == SecurityMode::Audit {
        // Run all detectors, log evidence, but don't enforce
        security.analyze_audit_mode(&invocation);
        return tool.execute_unguarded(invocation).await;
    }

    // Full evaluation with short-circuit
    let decision = PolicyEvaluator::evaluate(
        &mut security.accumulator,
        &invocation,
        policy,
        &mut security.detectors,
    );

    match decision {
        PolicyDecision::Allow(proof) => {
            let result = execute_with_proof(proof).await;
            security.accumulator.record_clean_action();
            result
        }
        PolicyDecision::Warn(warn) => {
            emit_warning(&warn);
            // Still execute, but user sees warning
            let result = tool.execute_unguarded(invocation).await;
            result
        }
        PolicyDecision::Block(block) => {
            emit_block_ui(&block);
            ToolResult::Blocked(block)
        }
        PolicyDecision::Terminate(term) => {
            emit_terminate_ui(&term);
            ToolResult::SessionTerminated(term)
        }
        PolicyDecision::Lock(lock) => {
            emit_lock_ui(&lock);
            ToolResult::SessionLocked(lock)
        }
    }
}
```

---

## Phased Implementation

### Phase 1: Core Framework

1. Create `security` crate with module structure
2. Define evidence types in `types/src/security.rs`
3. Implement `ThreatScore` with formal algebra (caps, max-aggregation)
4. Implement `EvidenceAccumulator` (bounded storage, decay)
5. Implement `PolicyEvaluator` (short-circuit, threshold checks)
6. Implement `AllowedExecution` with private constructor
7. Add tool loop hooks, default to `SecurityMode::Audit`

**Deliverable:** Framework compiles, logs all tool invocations, enforces nothing.

### Phase 2: Honeytoken Integration

1. Move honeytoken detector into framework
2. Emit `HoneytokenEvidence` → accumulator
3. Short-circuit on critical
4. UI surfaces terminate modal

**Deliverable:** Honeytokens work through unified framework.

### Phase 3: Exposure Tracker + Sensitive Path Classifier

1. Implement `ExposureTracker` (simple flag-setting, not propagation)
2. Implement static path sensitivity classification
3. Emit evidence on sensitive read
4. Emit evidence on network/write while exposed

**Deliverable:** Reading `~/.ssh/id_rsa` then `curl` raises score.

### Phase 4: Argument Sanitizer

1. Implement metacharacter detection
2. Implement path traversal detection
3. Short-circuit: block on detection, before execution

**Deliverable:** `"; rm -rf /"` in tool args blocked.

### Phase 5: Threat State Machine

1. Implement state bits and hypothesis rules
2. Replace any remaining sequence logic
3. Decay based on clean action count

**Deliverable:** Interleaved attack sequences detected.

### Phase 6: Exfiltration Correlator

1. Track recent read content hashes
2. Inspect outbound network requests
3. Correlate presence

**Deliverable:** `curl` with file contents detected.

### Phase 7: Evasion Detector + Normalization

1. Normalization layer for all paths/commands
2. Homoglyph detection
3. Encoding depth analysis
4. Runs before all other detectors

**Deliverable:** Unicode tricks normalized before detection.

---

## IFA Alignment

| Principle | Application |
|-----------|-------------|
| Sum types preserve distinctions | `SecurityEvidence` enum with exhaustive match |
| No Option in core | `ThreatScore` fields explicit, `CategoryScore` always present |
| Mechanism vs Policy | Detectors report facts, `PolicyEvaluator` decides |
| Proof-carrying | `AllowedExecution` has private constructor, consumed on use |
| Boundary collapse | Exposure classified at ingestion, sensitivity determined once |
| Caller owns policy | `SecurityPolicy` is caller-provided, mode is caller-selected |
| Bounded resources | `EvidenceAccumulator` uses `BoundedVec`, noise floor detection |

---

## Testing Strategy

1. **Unit: Scoring algebra** — verify caps, max-aggregation, decay properties
2. **Unit: Threshold ordering** — invalid orderings rejected at construction
3. **Unit: Proof-carrying** — `AllowedExecution` not constructable outside module
4. **Unit: State machine** — bits set correctly, hypotheses fire on expected combinations
5. **Unit: Argument sanitizer** — all injection patterns detected
6. **Integration: Honeytoken → terminate** — latency under 100ms
7. **Integration: Audit mode** — all evidence logged, no enforcement
8. **Integration: Exposure + network** — raises score appropriately
9. **Integration: Interleaved attack** — state machine catches despite benign interleaving
10. **Integration: Noise floor** — excessive low-severity events detected as DoS
11. **Regression: False positive corpus** — benign workflows do not trigger

---

## Open Questions — Resolved

| Question | Resolution |
|----------|------------|
| Decay: time-based or action-based? | **Action-based.** Decay after N clean tool invocations. |
| Session vs persistent threat memory? | **Session-scoped scores, persistent evidence logs.** |
| User approval and threat score? | **Approval recorded as `UserOverride` evidence.** Score unchanged. |
| Security research workflows? | **`SecurityMode::Audit`** (log only) or config override unlock. |
| Training mode? | **Ship with `Audit` as default.** Enforcement opt-in. |

---

## Success Criteria

1. No enforcement decision without evidence record
2. No silent security failures (all blocks/terminates audited)
3. Honeytoken → terminate path under 100ms
4. False positive rate under 1% in Audit mode observation period
5. Framework extensible: new detector = new evidence variant + score contribution
6. Scoring algebra formally specified and tested
7. Proof-carrying claims structurally enforced
