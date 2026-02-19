# IFA Deterministic Rulebook for Rust

Machine-executable rules for writing and reviewing Rust in an Invariant-First Architecture codebase.
Derived strictly from `INVARIANT_FIRST_ARCHITECTURE.md`. No escape hatches, no carve-outs not present
in the spec. Where the spec is absolutist, these rules are absolutist.

---

## 0. Classification Rules

**IFA-R0: Every module and function is either Core or Boundary. No third category.**

Classify as **Boundary** if the code touches any of: I/O, network, filesystem, clocks, randomness,
async/concurrency primitives, FFI, logging/telemetry, panic/termination reporting, environment
variables, config files, external services, OS APIs.

Classify as **Core** if none of the above apply.

**Non-acceptable pushback:** "It's mostly Core but it needs one async call." No. That makes it
Boundary. Move the async portion into a Boundary module and expose a synchronous owned result to Core.

---

**IFA-R1: Core is synchronous and single-threaded.**

Core MUST NOT use `async`, `.await`, `tokio`, `std::thread`, `Arc`, `Mutex`, `RwLock`, channels,
or atomics. Any of these in Core code is a misclassification. Move to Boundary and expose an owned
synchronous result.

---

**IFA-R2: Authority Boundaries are Rust access control. Nothing else qualifies.**

A proof type's constructors MUST be restricted by `private` fields and `pub(crate)` or narrower
visibility. "Same repository," "same crate," or "I trust this caller" are not Authority Boundaries.
Only Rust privacy counts. If it's `pub`, it's not controlled.

---

## 1. Proof Objects and Forgeability

**IFA-R3: Core MUST accept only proof types for its invariants.**

If Core correctness depends on predicate `P(x)`, the parameter type MUST be a proof type
`XWhereP`, not `String`, `i64`, `Vec<T>`, `Option<T>`, `Result<T, _>`, or any raw primitive.

---

**IFA-R4: "Maybe-valid" types MUST NOT cross into Core.**

Core interfaces MUST NOT accept:

- `Option<T>`
- nullable pointers (`*const T`, `*mut T`)
- sentinel values (`-1`, `0`, `""`, `INVALID_ID`, etc. meaning "absent" or "unknown")
- out-parameters that may not be written

If the value can be absent, the type is wrong. Restructure. See IFA-R40 for the ban and section 11.2
of the spec for the reasoning.

**Non-acceptable pushback:** "The domain allows absence." The domain is wrong. Restructure (IFA section 18).

---

**IFA-R5: Proof types MUST NOT be forgeable through the safe surface.**

Outside the Authority Boundary, no safe Rust code may construct an invalid proof value. This means:
no public fields, no public unchecked constructors, no `pub From<Raw>` that cannot fail, no
public tuple struct fields (`pub struct Foo(pub String)` is non-conforming).

---

**IFA-R6: Unchecked constructors MUST be `unsafe` and as narrow as possible.**

The narrowest permissible visibility is `unsafe fn new_unchecked` restricted to the Authority
Boundary (fully private, or `pub(super)` at widest before `pub(crate)`). `pub(crate)` is a ceiling,
not a default. A safe `new_unchecked` is unconditionally non-conforming.

---

**IFA-R7: Proof types MUST NOT derive constructors that bypass validation.**

For proof objects, the following derives are banned unless the implementation guarantees they
enforce invariants:

- `#[derive(Default)]` -- default value likely violates invariant
- `#[derive(Deserialize)]` -- serde constructs values without calling your validator

Action: implement `TryFrom<RawType>` + `#[serde(try_from = "RawType")]`, or a custom
`Deserialize` impl that calls the canonical validator inside the Authority Boundary.

---

**IFA-R8: If it type-checks and an invalid state is still reachable through safe APIs, the interface is wrong.**

Successful compilation is only evidence of invariant enforcement when the interface makes invalid
states uncallable. If a core function compiles but can receive invalid inputs via safe code,
redesign the types. The assertion "it compiles" is meaningless until IFA-R5 holds.

---

## 2. Boundary Conversion Discipline

**IFA-R9: Boundary converts once. Core assumes always.**

Raw/untrusted values (from parsing, JSON, CLI args, DB rows, HTTP params, IPC) MUST be converted
into canonical proof objects at the Boundary before Core is called. Core does not validate. Core
does not check. Core assumes it received valid input because the type guarantees it.

---

**IFA-R10: Boundary conversion failure MUST be explicit and typed.**

Conversions that can fail MUST return `Result<ProofType, BoundaryError>` or a discriminated union
that forces the caller to handle the failure case. Silent swallowing, logging-and-continuing, and
returning a fallback through the same type are all non-conforming (section 8 of spec).

---

**IFA-R11: Core MUST NOT contain validation that Boundary could have performed.**

If Core contains any of the following, it is non-conforming:

- `if x.is_empty()`
- `if x < 0`
- `is_valid_*` calls
- range checks on parameters
- shape checks on parameters
- assertions that restate parameter validity

Replace the raw parameter with a proof type and delete the check.

---

## 3. Single Point of Encoding

**IFA-R12: Each invariant has exactly one canonical proof-carrying representation.**

If two types both claim to represent "validated email," one of them is non-conforming. Choose one
canonical proof type. All others MUST wrap or compose it, never re-validate independently.

**Non-acceptable pushback:** "They have slightly different constraints." Then they are different
invariants and need different names.

---

**IFA-R13: Exactly one Authority Boundary establishes each proof.**

The same invariant predicate MUST NOT be implemented in multiple modules or crates. Delete
duplicates. Route all construction through the canonical factory in its Authority Boundary.
Multiple ingestion points (formats, protocols) are permissible; each MUST delegate to the single
canonical validator.

---

**IFA-R14: Shared invariants MUST be composed, not copied.**

If `EmailAddress` implies `NonEmptyString`, then `EmailAddress` wraps `NonEmptyString` and
construction delegates to `NonEmptyString::try_new`. The non-empty predicate is never
re-implemented inside `EmailAddress`.

---

**IFA-R15: Derived data MUST NOT be stored independently of its source. No exceptions.**

If a value is computable from existing state, it MUST be computed on access. Storing it creates a
synchronization obligation between source and cache (sections 6.1 and 7.6 of spec).

Non-conforming: `struct Foo { items: Vec<T>, count: usize }` -- `count` must equal `items.len()`
but the type permits them to diverge.

Conforming: `fn count(&self) -> usize { self.items.len() }`

This ban has no "Core only" qualifier. Denormalized data is a coordination defect in any context.

---

## 4. Typestate and State-as-Location

**IFA-R16: Lifecycle states that change valid operations or valid data MUST be distinct types or data-carrying enum variants.**

If a state change affects (a) which operations are callable, or (b) which fields are meaningful,
that state MUST be encoded as a distinct type or as a variant in a discriminated union where each
variant owns its data exclusively.

---

**IFA-R17: Tag fields MUST NOT encode lifecycle state.**

`struct X { state: StateTag, field_a: ..., field_b: ... }` where some fields are meaningless
depending on `state` is non-conforming. Replace with:

```rust
enum X {
    StateA { field_a: ... },
    StateB { field_b: ... },
}
```

---

**IFA-R18: State transitions MUST move between types or variants. No in-place flag toggling.**

A transition function consumes state A (by value) and returns state B. There is no setter, no
`self.state = NewState`, no mutable flag flip. The type is the state. You know what you hold by
what you're holding.

---

**IFA-R19: Transitions MUST be non-forking.**

After A -> B, the old A MUST NOT be usable for A-privileged operations. In Rust, the transition
consumes `self`. Do not implement `Clone` or `Copy` on state handles that represent exclusive
rights or lifecycle positions.

---

**IFA-R20: Zombie-state rule -- moved-from values must be provably inert.**

If a design relies on "move makes it unusable," that claim MUST be constructively verifiable: the
moved-from value must not retain any usable capabilities (tokens, handles, IDs granting rights).
Use non-`Clone`, non-`Copy` types and consumption APIs. If the moved-from state is still
capability-bearing, the transition is not non-forking and the design is non-conforming.

---

**IFA-R21: Container types do not confer state-as-location.**

`Vec<Deck<Shuffled>>` is convention, not enforcement. Nothing prevents inserting a `Deck<Shuffled>`
constructed by bypassing the shuffle transition. State-as-location means the type system rejects
misuse at the call site, not that you labelled your Vec carefully. If insertion of an invalid
element is possible through safe code, it's herding, not enforcement (section 15.5 of spec).

---

**IFA-R22: Domain enumeration test -- every variant MUST name a domain state.**

To validate a discriminated union:

1. List all states the domain entity occupies using only domain language.
2. Every variant in code MUST map to exactly one domain state from step 1.

Any variant that exists because the code needed it but the domain doesn't name it -- `None`,
`Empty`, `Unknown`, `Default` -- is encoding absence or incomplete resolution, not a domain state.
It is non-conforming.

---

## 5. Booleans

**IFA-R23: Booleans are unconditionally non-conforming for lifecycle state.**

A boolean is `if self.ready { ... }` without the type system. A boolean flag that changes which
operations are valid, which fields are meaningful, or which code paths execute is a lifecycle state
encoded as a primitive. It is an if-statement in a trenchcoat. Replace it with a typestate type
or data-carrying enum variant.

The only test (section 14.1 of spec): is changing this boolean value valid for the same type, leaving
all fields valid and all operations applicable? If changing it requires that any field be
reinterpreted or any method call become invalid, the boolean is lifecycle state and is
non-conforming.

**Non-acceptable pushback:** "It's just a configuration flag." Apply the test. If the flag changes
what the type can do or what its fields mean, configuration is not a valid defense.

---

## 6. Capability Tokens

**IFA-R24: Phase-conditional operations MUST require a capability token.**

If an operation is only valid during a specific phase (render pass, transaction, initialization
scope), the operation MUST require a token that cannot be obtained outside that phase and cannot
outlive it.

`assert!(is_rendering)` is non-conforming as a substitute for a missing token. The assertion
detects an invalid call after the architecture already permitted it.

---

**IFA-R25: Token constructors MUST be inaccessible outside the phase Authority Boundary.**

Token construction is private to the boundary that owns the phase. No public constructor, no
`Default`, no derived `From`.

---

**IFA-R26: Token lifetimes MUST prevent outliving the phase.**

The token MUST carry a lifetime bound to the phase scope (typically a borrow of a private phase
guard). A token with lifetime `'static` cannot prove phase constraint and is non-conforming.

---

**IFA-R27: Tokens MUST NOT be duplicable.**

No `Clone`, no `Copy`. A cloned capability token can be held past the phase transition it was
issued for. There is no "explicitly safe duplication" exception -- if the token can be duplicated,
it is not a proof of exclusive phase constraint. Full stop.

---

## 7. Parametricity

**IFA-R28: Generic functions MUST NOT inspect `T` without explicit bounds granting that inspection.**

If the signature is `fn f<T>(x: T)`, the body MUST NOT: log `x`, format `x`, serialize `x`,
branch on `x`'s contents, call domain methods on `x`, or downcast `x`. If inspection is required,
it MUST be declared at the interface via explicit bounds (`T: Debug`, `T: Serialize`,
`T: SomeTrait`).

---

**IFA-R29: No implicit type-based branching in parametric code.**

Inside a function claiming parametric behavior, the following are unconditionally non-conforming:

- `Any` downcasts (`&dyn Any`, `downcast_ref`, `TypeId` comparisons)
- Specialization or sealed-impl tricks that change semantics by type
- `if constexpr`-equivalent branching on type properties

If the behavior must differ by type, that MUST be a different interface with a different contract.

---

## 8. Ownership and Construction

**IFA-R30: Mutable resources have exactly one authoritative owner in Core.**

Shared mutable state patterns -- `Rc<RefCell<_>>`, `Arc<Mutex<_>>`, global singletons, interior
mutability shared across components -- are unconditionally non-conforming in Core. Exactly one
component owns mutation. Multi-owner coordination, if required, belongs in Boundary.

---

**IFA-R31: Two-phase initialization is non-conforming.**

`Foo::new()` followed by a required `foo.init()` means `Foo` can exist in an invalid state. The
constructor MUST return either a fully valid `Foo` or `Result<Foo, E>` where `Foo` does not exist
on error. No partial objects. No `init()` methods required for correctness.

---

**IFA-R32: Core MUST NOT accept partially-owned constructions or out-params.**

APIs where one party owns storage and another owns initialization, signaled by boolean return codes
or sentinel values, are non-conforming. Return an owned value or an explicit typed error.

---

**IFA-R33: Compensatory retries are non-conforming in Core.**

A loop in Core is a prohibited compensatory retry if it re-attempts an operation because you failed
to ensure its success. This includes: allocation retries, lock retries, "try again" polling,
resource exhaustion that could have been bounded, lock contention you could have eliminated through
ownership design. Move such loops to Boundary under explicit policy control.

**The spec's single exception (section 6.3):** Deterministic algorithms that iterate toward a result
determined by their inputs -- numeric methods, search, parsing -- are not retries because their
termination depends on input structure, not on external conditions changing. This exception is
narrow. If the loop's termination depends on anything other than the values passed in, it is a
compensatory retry and belongs in Boundary.

---

## 9. Mechanism vs Policy

**IFA-R34: Mechanisms MUST report facts. They MUST NOT silently choose fallbacks.**

A data provider that can return real data or a fallback MUST distinguish these outcomes
structurally. The call site MUST be forced to handle both cases explicitly. A fallback returned
through the same type as real data is non-conforming.

---

**IFA-R35: Absence MUST be represented as absence.**

Returning `0`, `""`, an empty vec, or any value of the same type as the real result to signal
"not found" is non-conforming. The type returned when the value is absent MUST be structurally
different from the type returned when it is present, so the policy choice is mandatory.

---

## 10. Failure Surfaces

**IFA-R36: Failures observable outside the system MUST be produced by Boundary as boundary-defined representations.**

Core errors MUST NOT leak raw into any external observation surface (API response, UI message,
log, telemetry). Boundary code maps internal failures to boundary-defined error representations.
Internal type names, file paths, stack traces, memory addresses, module structure MUST NOT appear
in externally observable failure paths.

---

**IFA-R37: Core MUST NOT parse diagnostic strings or stack traces as domain data.**

Matching on error message text, stack trace strings, debug output, or any substrate diagnostic
as a branching condition is non-conforming. Replace with typed errors.

---

## 11. Assertions

**IFA-R38: An assertion in Core on a representable invalid state is a design defect. Full stop.**

If Core contains `assert!(x > 0)` where `x: i32` is a parameter, the design is non-conforming
regardless of whether the assertion panics on violation. The assertion's existence proves the
invariant is not encoded in the type. Fix: replace the parameter with a proof type that can only
exist if the predicate holds. Delete the assertion.

A fail-stopping assertion does not satisfy IFA. Fail-stop is the correct behavior upon detecting
the defect at runtime; it is not a substitute for encoding the invariant.

---

**IFA-R39: If Core reaches a state that contradicts required invariants, it MUST fail-stop.**

If a state contradiction is detected -- meaning the design has a gap and an invalid state reached
Core despite best efforts -- the implementation MUST terminate. It MUST NOT continue executing Core
logic, attempt recovery inside Core, retry, or silently continue. Recovery and retry belong in
Boundary under explicit policy.

---

**IFA-R40: Unencodable invariants produce non-conformant components. Isolation does not change this.**

If an invariant cannot be fully encoded in the type system, the architecture MUST (section 16.2 of spec):

1. Encode as much as the language permits.
2. Make violation require explicit circumvention (unsafe, casts, ignoring warnings).

If the invariant cannot be encoded at all, that component is non-conformant. Declaring it a
"designated Boundary component" or "isolated module" does not make the system IFA-conformant. It
names the non-conformance. The section 17 Operational Definitions Checklist must document it as such.
There is no exception register.

---

## 12. Deterministic Bans

**IFA-R41: `Option<T>` is banned in Core interfaces and Core domain structs.**

No exceptions for "semantically optional" fields. If the field can be absent, the type is wrong.
Restructure into distinct types. The discomfort of splitting is not an argument (section 11.2 of spec).

---

**IFA-R42: `None`, `Empty`, `Unknown`, `Default` enum variants are banned in Core domain enums.**

Apply the domain enumeration test (IFA-R22). If the variant has no name in the domain language,
it is encoding absence or incomplete resolution. Remove it by restructuring.

---

**IFA-R43: Sentinel values are banned as absence markers.**

`-1`, `0`, `""`, `INVALID_ID`, any in-band "absent" value -- replace with proof types, enums,
or typestates that cannot represent the absent condition.

---

**IFA-R44: `is_valid_*` functions called in Core are design defects.**

Their presence proves validity is not encoded in the parameter type. Move validity into type
construction and require the proof type.

---

**IFA-R45: Any Core function that can "fail because input was invalid" is non-conforming.**

If Core can be invoked with valid-compiling but semantically invalid inputs, the interface is
wrong. Redesign types so invalid inputs cannot be passed, making the Core operation infallible
with respect to its inputs.

---

## 13. Mandatory Process Artifacts (section 17 of Spec)

**IFA-R46: Conformance claims require all five section 17 operational definitions. No exceptions.**

A codebase cannot claim IFA conformance without maintaining:

1. **Invariant Registry** -- a declared list of all invariants and their encoding mechanism.
2. **Authority Boundary Map** -- for each proof object or controlled type, which code constitutes
   its Authority Boundary.
3. **Parametricity Rules** -- explicit bans on specialization and type-trait branching.
4. **Move Semantics Rules** -- what "unusable after transition" means for each state-bearing type.
5. **DRY Proof Map** -- for each invariant, one canonical proof type and one Authority Boundary.

Code-level rule compliance without these artifacts is not conformance. It is incomplete conformance.

---

## 14. Non-Acceptable Objections Reference

These objections are rejected by the spec (section 18). When encountered in review, name the section and move on.

| Objection | Response |
|---|---|
| "It's simpler this way." | IFA optimizes for simplicity of reasoning, not ease of writing. |
| "We'll be careful." | Discipline is not architecture. |
| "We'll document it." | Documentation is not enforcement. If the invalid state compiles, it ships. |
| "We'll assert it." | Assertions detect invalid states after the architecture permitted them. This is IFA-R38. |
| "We'll handle it later in Core." | If Core rejects invalid states, those states were representable at the interface. Move rejection to Boundary. |
| "The language can't enforce it." | Make violation require circumvention. If you can't, document the non-conformance (IFA-R40). This is not permission to leave invalid states easy to reach. |
| "The domain allows absence." | The domain is wrong. Restructure. |
| "This boolean is just configuration." | Apply IFA-R23's litmus test. If the flag changes valid operations or valid fields, it's lifecycle state. |

---

## 15. Canonical Rust Construction Patterns

The following patterns satisfy the corresponding rules. Use these exactly when a rule says
"make it a proof type / typestate / token."

**Proof type (value invariant):**

```rust
// Authority Boundary module
pub struct NonEmptyString(String); // private field -- IFA-R5

impl NonEmptyString {
    pub fn try_new(s: String) -> Result<Self, EmptyStringError> {
        if s.is_empty() { return Err(EmptyStringError); }
        Ok(Self(s))
    }
    // No Default, no unchecked safe constructor, no derived Deserialize -- IFA-R7
}
```

**Typestate (temporal invariant):**

```rust
struct Unshuffled;
struct Shuffled;
struct Deck<S> { cards: Vec<Card>, _state: PhantomData<S> }

impl Deck<Unshuffled> {
    pub fn shuffle(self, rng: &mut impl Rng) -> Deck<Shuffled> { // consumes self -- IFA-R19
        // ...
    }
}

fn deal(deck: Deck<Shuffled>) { /* cannot be called with Deck<Unshuffled> */ }
```

**State-as-location (lifecycle):**

```rust
// Each variant owns its own data -- IFA-R17
enum Connection {
    Connecting { attempt: u32 },
    Connected { stream: TcpStream, established_at: Instant },
    Draining { stream: TcpStream, deadline: Instant },
}
// No separate `state: StateTag` field. No shared fields across variants.
```

**Capability token:**

```rust
mod render_boundary {
    pub struct RenderPass<'a> {
        _guard: &'a mut RenderGuard, // scope-bound -- IFA-R26
    }
    // No Clone, no Copy -- IFA-R27
    // Constructor is private to this module -- IFA-R25

    pub fn with_render_pass<F: FnOnce(RenderPass<'_>)>(guard: &mut RenderGuard, f: F) {
        f(RenderPass { _guard: guard });
    }
}

pub fn draw_widget(pass: RenderPass<'_>, widget: &Widget) {
    // callable only while holding a RenderPass -- IFA-R24
}
```

---

## 16. LLM Review Procedure

Execute this sequence in order for every Rust change. Do not skip steps.

1. **Classify** -- Apply IFA-R0/R1 to every touched module and function. Label each Core or
   Boundary before proceeding.

2. **List invariants** -- Enumerate every predicate that Core correctness depends on, derived from
   types, docs, assertions, validation functions, and error paths.

3. **Map to canonical proof types** -- For each invariant, identify or create the canonical proof
   type and its Authority Boundary (IFA-R12/R13). Verify there is exactly one.

4. **Verify unforgeability** -- Confirm that outside the Authority Boundary, no safe code can
   construct an invalid proof value (IFA-R5, R6, R7, R8).

5. **Verify Boundary conversion** -- Confirm all messy inputs are converted at Boundary into proof
   types before Core is called (IFA-R9, R10, R11).

6. **Eliminate maybe** -- Check for `Option`, sentinels, tag fields, two-phase init, compensatory
   retries (excluding the deterministic-algorithm exception), and booleans governing lifecycle.
   Eliminate all of them (IFA-R4, R16-R33, R41-R45).

7. **Apply domain enumeration test** -- Apply IFA-R22 to every enum in Core. Reject any variant
   with no domain name.

8. **Apply the boolean litmus test** -- Apply IFA-R23 to every `bool` field in Core structs.
   If changing the value alters valid operations or valid fields, replace with typestate.

9. **Enforce parametricity** -- Confirm generic code does not inspect `T` without declared bounds,
   no downcasts, no type branching (IFA-R28/R29).

10. **Lock failure surfaces** -- Confirm all externally observable failures are shaped by Boundary,
    no raw internal representations leak (IFA-R36/R37).

11. **Treat assertions as evidence** -- Every `assert!` / `debug_assert!` in Core on a
    representable condition is a design defect. Redesign types until the assertion is unnecessary
    (IFA-R38).

12. **Check section 17 artifacts** -- Confirm the invariant registry, authority boundary map,
    parametricity rules, move semantics rules, and DRY proof map exist and reflect this change
    (IFA-R46).
