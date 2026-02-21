# Behavioral Security Framework — Plan Review

**Reviewed:** 2026-02-21
**Source:** `docs/PLAN_BEHAVIORAL_SECURITY.md` (draft v1)

---

## Structural Issues

### 1. Taint tracker is architecturally aspirational, not tractable

Taint propagation through LLM-generated content is an unsolved research problem. The model doesn't copy verbatim — it paraphrases, restructures, and intermixes. The propagation rules say "content copied verbatim inherits source taint" and "summarized/transformed reduces taint by one level," but there is no mechanism to distinguish these cases. The LLM is a black box between tool output and tool input.

What *can* be tracked: which tool outputs the model has seen before it requests a tool invocation. That's a "has seen sensitive content" flag, not taint propagation. Call it what it is — **exposure tracking** — and the implementation becomes tractable. Track which file reads/web fetches occurred in the session, classify their sensitivity, and flag when a tool invocation follows exposure to sensitive content within N turns.

### 2. Sequence analyzer pattern matching is brittle against reordering

The patterns assume linear sequences (`read → encode → write → network`). An adversarial model will interleave benign operations, split the chain across conversation turns, or use indirect paths. A sliding window of 20 tool invocations with strict sequence matching will miss these.

Better: model it as a **state machine per threat hypothesis**, not a sequence matcher. "Has read sensitive file" is a state bit. "Has encoded content" is a state bit. "Network request while sensitive-read bit is set" fires regardless of how many benign operations happened in between. State bits decay on the accumulator's schedule, not the window boundary.

### 3. Score arithmetic needs formal properties

Score contributions (+100, +200, +500) and thresholds (warn, block, terminate, lock) are defined but nothing about:
- Are scores additive? Can 5 medium events cross a terminate threshold designed for 1 critical event?
- Is there a cap per evidence type to prevent score inflation from repeated low-severity events?
- What's the relationship between `lock_threshold` and `terminate_threshold`? Can you be locked without being terminated first?

Define the scoring algebra explicitly. Suggestion: per-category caps, with the cumulative score being `max(category_scores)` or a weighted sum with diminishing returns, not raw addition.

### 4. `AllowedExecution` proof is structurally empty

```rust
pub struct AllowedExecution {
    _proof: (),  // This proves nothing
    tool: ToolInvocation,
    score_snapshot: ThreatScore,
}
```

The `_proof: ()` field is a naming convention, not a proof. Any code can construct this. If proof-carrying tokens are desired, make `AllowedExecution` constructable only via `PolicyEvaluator::evaluate` — private field with a `pub(crate)` constructor, or a newtype wrapping a non-`Clone` token. Otherwise drop the "proof-carrying" language from the IFA alignment section; it's aspirational, not structural.

---

## Design Gaps

### 5. No detector priority or short-circuit

If the honeytoken detector fires at +1000 (instant terminate), the system still runs taint tracking, sequence analysis, exfiltration correlation, and evasion detection before enforcing. Define an evaluation order with early exit: critical evidence → terminate immediately, skip remaining detectors.

### 6. Decay model is underspecified

The `DecayPolicy` has `half_life`, `floor`, and `pause_on_evidence`, but:
- Does decay apply per-category or to cumulative score?
- If per-category, a slow drip across 5 categories never decays any single one but accumulates cumulatively.
- If cumulative, a single HIGH event decays away even though the evidence record persists.

Suggestion: decay applies to cumulative score, per-category scores are monotonic (evidence is permanent), and the policy evaluator considers both the decayed cumulative and the raw per-category maxima.

### 7. User approval interaction is missing from the type system

`RequireApproval(ApprovalContext)` is a policy decision, but there's no type for "user approved this specific tool invocation despite threat score." If user approval doesn't lower the score, a `UserOverride` evidence type is needed that the accumulator records, so the audit trail shows *why* a high-score execution was allowed.

### 8. No false-positive recovery path

If the sequence analyzer misclassifies a legitimate workflow and the score crosses `lock_threshold`, the plan says `Lock(LockReason, UnlockRequirement)` but never defines `UnlockRequirement`. Is it user password? Session restart? Admin override? This is critical for the security research workflow question.

---

## Threat Model Gaps

### 9. Missing attack class: tool argument injection

The model can be tricked into passing crafted arguments to tools. Example: a file contains `"; rm -rf /; echo "` and the model interpolates it into a shell command. This isn't evasion (the model doesn't know it's being tricked) and isn't exfiltration. It's a distinct class that needs its own detector — argument sanitization validation pre-execution.

### 10. Missing attack class: denial of service against the security system itself

An adversary could flood the accumulator with low-severity evidence to trigger decay resets, exhaust memory with evidence logs, or generate enough noise to mask a real attack. Consider bounded evidence storage and a "noise floor" detector.

---

## Implementation Concerns

### 11. Phase 4 (Taint Tracker) blocks on tractability

Reframe taint tracking as exposure tracking and it becomes a phase 3 addition (simple flag-setting on file reads) rather than a standalone phase requiring propagation analysis.

### 12. Honeytoken → terminate latency SLA

"Honeytoken → terminate path under 100ms" is a performance SLA on the hot path of every tool invocation. The pre-execution analysis runs all detectors synchronously before executing the tool. With 6 detectors, ensure they're all O(1) or O(log n) against accumulated state, not O(n) against evidence history. The sequence analyzer's sliding window is fine; the exfiltration correlator's content hash comparison could be expensive if hashing large file reads.

### 13. Crate placement

The plan doesn't say where this lives. Given the crate structure, this probably belongs as a new `security` crate that `engine` depends on, with types in `types/src/security.rs`. The tool loop integration means `engine` calls into `security`, and `context` calls into it for canary injection. Don't put it in `core` — it has too much domain logic.

---

## Open Questions — Recommendations

1. **Decay: time-based, action-based, or both?** Action-based. Time decay penalizes slow users. Decay per N clean tool invocations is behavior-meaningful.
2. **Session vs persistent threat memory?** Session-scoped scores, persistent evidence logs. A new session starts clean but the audit trail survives.
3. **User approval and threat score?** Approval should not lower the score. It should be recorded as a `UserOverride` evidence type. The score reflects observed behavior, not user intent.
4. **Security research workflows?** A config-level `security.mode = "audit"` that logs everything but enforces nothing.
5. **Training mode?** Yes. Ship with it as default. Enforcement mode is opt-in until false positive rates are validated.

---

## Summary

The architecture is sound. The layered detector → accumulator → policy → enforcement pipeline is the right shape. The main risks are:

- Taint tracking as described is intractable (reframe as exposure tracking)
- Score arithmetic needs formal bounds
- The proof-carrying claim needs structural backing
- Phasing should reorder around tractability

The threat model should add tool argument injection and anti-DoS for the security system itself.
