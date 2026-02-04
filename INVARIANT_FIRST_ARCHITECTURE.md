# Invariant-First Architecture (IFA)

## LLM-TOC
<!-- Stable section identifiers for LLM context -->
| ID | Section |
|----|---------|
| IFA-0 | Status and Scope |
| IFA-1 | Definitions |
| IFA-1.1 | Core Terms |
| IFA-1.2 | System Topology |
| IFA-1.3 | Proof and Transition |
| IFA-1.4 | Mechanism vs Policy |
| IFA-2 | Primary Rule |
| IFA-2.1 | Invalid states MUST NOT be forgeable |
| IFA-2.2 | Compilation as evidence |
| IFA-3 | Typestate Transitions |
| IFA-4 | Parametricity |
| IFA-5 | Constrained Generics and Call-Site Rejection |
| IFA-6 | Ownership as Coordination Elimination |
| IFA-7 | Single Point of Encoding (DRY as Invariant Ownership) |
| IFA-8 | Mechanism vs Policy |
| IFA-9 | State as Location |
| IFA-10 | Capability Tokens |
| IFA-11 | Boundary and Core Separation |
| IFA-12 | Assertions |
| IFA-13 | Non-Conforming Patterns |
| IFA-14 | Litmus Tests |
| IFA-15 | Implementation Notes |
| IFA-16 | Known Limitations |
| IFA-17 | Operational Definitions Checklist |
| IFA-18 | Non-Acceptable Objections |
| IFA-19 | Closing Statement |

## Pattern Summary

**Intent:** Prevent invalid states from being usable by core logic by encoding invariants and temporal constraints in representations and interfaces, such that misuse is structurally rejected.

**Context:** Systems with a boundary that ingests unreliable or adversarial inputs and a core that must remain simple, composable, and correct by construction.

**Problem:** If core functions accept "maybe-valid" representations, the core must contain pervasive checks, assertions, or conventions; this spreads boundary complexity through the system and makes correctness depend on discipline.

**Forces:**

1. We prioritize simplicity of reasoning over ease of writing ("compile-time rejection" over "debug-time discovery").
2. Boundaries are messy and must be adaptable; cores must be strict and assumption-driven.
3. The language may not perfectly enforce the ideal; the architecture must still make invalid states hard to reach and impossible to use in the core.

**Solution:** Encode invariants as types / typestates / capability tokens / container topology; enforce call-site rejection; keep retries and adaptation at the boundary; treat assertions as evidence of representable invalid states.

**Consequences:**

- More upfront type modeling and boundary conversion code.
- Fewer runtime guard paths in the core; fewer "undefined intermediate states."
- Interfaces become the primary correctness artifact.

**Scope:** This pattern assumes synchronous, single-threaded core logic. Concurrency is boundary logic (see §16.1).

---

## 0. Status and Scope

This document defines **Invariant-First Architecture (IFA)** as an architectural pattern. IFA is a set of design constraints whose purpose is to ensure that **core logic is only callable with values that satisfy explicitly stated invariants**, by construction, and without reliance on programmer discipline, runtime guard code inside the core, or informal documentation.

This document is normative where it uses "MUST" and "MUST NOT." Examples are illustrative and are not normative unless the requirement is stated independently of the example.

---

## 1. Definitions

### 1.1 Core Terms

**Invariant**
A predicate over a value, a set of values, or a system state that is required to hold whenever that value or state is observable by core code.

**Invalid state**
Any value or system state for which at least one required invariant does not hold.

**Representable**
An invalid state is representable if there exists any program expression—available to code outside the Authority Boundary—that can construct or obtain a value or state that violates an invariant and still type-checks against an interface intended for core use.

**Unrepresentable (for the core)**
An invalid state is unrepresentable for the core if no code outside the Authority Boundary can produce a value that violates the invariant and still satisfy the types (or equivalent interface constraints) required to invoke core operations.

**Safe surface**
The subset of language features and APIs intended for normal use, excluding explicit circumvention mechanisms (unsafe blocks, reflection, representation casts/transmute, raw pointer fabrication, etc.).

**Forgeable**
An invalid state is forgeable if it can be constructed outside the Authority Boundary using only the safe surface.

**Circumventable**
An invalid state is circumventable if it can be constructed only via explicit circumvention mechanisms. Circumventable states are acceptable under IFA; forgeable states are not.

### 1.2 System Topology

**Authority Boundary**
The smallest encapsulation boundary that can actually enforce construction control for a representation. A value is considered "controlled" by an Authority Boundary if and only if code outside that boundary cannot construct it without using the boundary's public API.

*Operationalization:* The Authority Boundary for a type is the set of code that can call its constructors or factory functions that are otherwise inaccessible (e.g., via `private` constructors and `friend` access in C++, `pub(crate)` in Rust). "Same repository," "same namespace," and "same header" are not Authority Boundaries unless they coincide with actual access control.

**Boundary**
The set of components that interact with non-IFA-controlled inputs or effects, including but not limited to: user input, network I/O, filesystem, external services, clocks/time, random sources, concurrency primitives, and foreign-function interfaces.

**Core**
The set of components whose correctness depends on the assumption that all invariants are satisfied. Core code MUST treat invariants as already established and MUST NOT accept "maybe-valid" representations of required invariants.

### 1.3 Proof and Transition

**Proof object**
A value whose existence is used as evidence that a specific invariant holds or that a specific event/phase has occurred. Proof objects are only meaningful if the system prevents their construction without establishing the corresponding invariant.

**Transition**
A function or operation that consumes (or otherwise makes unusable) a proof object for state A and produces a proof object for state B, thereby encoding temporal or phase ordering as a type-level (or interface-level) fact.

### 1.4 Mechanism vs Policy

**Mechanism**: A component that reports facts and constraints about the world or a subsystem.

**Policy**: A component that makes decisions about what to do given those facts.

---

## 2. Primary Rule

### 2.1 Invalid states MUST NOT be forgeable for the core

An implementation conforms to IFA only if, for every invariant required by core operations, core-callable interfaces require a representation that cannot be forged outside the Authority Boundary using the safe surface.

Consequences:

- Core functions MUST NOT accept "maybe-valid" parameters for invariants that are required for correct operation.
- Core functions MUST NOT rely on:
  - caller discipline,
  - runtime checks inside the core,
  - comments or documentation,
  - "this will never happen" assumptions that are not enforced by representation.

### 2.2 The architecture MUST encode invariants such that compilation becomes meaningful evidence

IFA requires that invariants be declared and encoded in interfaces. When this is done correctly, successful compilation demonstrates that encoded invariants are satisfied at every call site.

IFA does not claim that compilation implies absence of runtime defects, undefined behavior, or external failures. It claims only that invariants which are declared and encoded in interfaces are not optional at runtime.

If invalid behavior is still possible after successful type-checking, then either:

- the invariant was not encoded in the interface, or
- the Authority Boundary permits construction of a proof object without establishing the invariant.

Either case is a design defect under IFA.

---

## 3. Typestate Transitions

### 3.1 Problem: Temporal coupling expressed as a runtime convention

If correct behavior depends on an operation happening "earlier" (e.g., "deck must be shuffled before starting a game"), then any representation that can mean both "shuffled" and "not shuffled" without forcing the distinction at the interface level is non-conforming.

### 3.2 Requirement: Distinct states MUST be distinct types

For any lifecycle step where validity depends on prior events, the system MUST provide distinct representations for each meaningful validity state. A consumer requiring state B MUST accept only the representation for state B.

### 3.3 Requirement: Transitions MUST be non-forking

A transition from state A to state B MUST prevent continued use of the "A" representation as if it still grants rights to act as state A.

Operationalization options include:

- linear/affine types,
- move-only types paired with an API that makes moved-from values unusable,
- ownership transfer via unique handles,
- explicit consumption APIs that invalidate the prior handle.

**C++ note:** In standard C++, "move" alone does not guarantee destructive consumption (moved-from objects remain valid but unspecified). "Non-forking" MUST be enforced by ensuring that the moved-from object cannot perform meaningful actions (e.g., resource in `unique_ptr` where moved-from becomes null, or methods are rvalue-qualified consumption-only).

---

## 4. Parametricity

### 4.1 Problem: Inspection creates hidden coupling

If a function can inspect the data it is given, it can embed decisions based on that inspection. Such decisions create unintended policy inside low-level mechanisms.

### 4.2 Requirement: Generic functions MUST NOT inspect without explicit permission

A function that is declared generic over `T` and is not explicitly constrained to permit inspection of `T` MUST NOT incorporate any behavior that depends on inspecting the content of `T` values.

"Inspection" includes:

- reading fields,
- invoking domain-specific methods,
- logging/serializing values for debugging,
- branching based on value properties.

Any capability to inspect `T` MUST be made explicit at the interface boundary as a declared constraint (concept/trait/interface) that grants specific operations. Inspection MUST NOT be introduced implicitly inside the implementation.

### 4.3 Enforcement rule

A function claiming parametric behavior MUST NOT use:

- template specialization that changes semantics based on `T`,
- `if constexpr` on type traits,
- RTTI (`typeid`, `dynamic_cast`),
- reflection-like mechanisms,
- casts to obtain type- or representation-specific knowledge.

If such mechanisms are used, the function is not parametric under IFA and MUST be specified as a different interface with a different contract.

---

## 5. Constrained Generics and Call-Site Rejection

### 5.1 Requirement: Invalid instantiations MUST be rejected at the Authority Boundary

If a generic function requires a capability (e.g., "serializable"), the interface MUST express that requirement such that:

- the function is not callable (or not type-checkable) for non-conforming types, and
- failure does not occur "deep inside" instantiation or runtime logic.

Invalid inputs MUST NOT propagate into internal logic as partially-formed obligations.

---

## 6. Ownership as Coordination Elimination

### 6.1 Requirement: A mutable resource MUST have exactly one authoritative owner

If two components must "agree" on the state of a resource, the system has created distributed ownership. Under IFA, this is a design defect unless the architecture explicitly models multi-owner consensus as a first-class protocol.

### 6.2 Requirement: Core functions MUST NOT accept partially-owned constructions

A core function MUST NOT accept an interface where:

- one party owns allocation/storage and another party owns completion/initialization semantics, and
- success is signaled via boolean return codes or sentinel values while leaving the destination in an indeterminate state.

A constructor/factory boundary MUST return either:

- a fully constructed, invariant-satisfying object, or
- an explicit error value that prevents the object from existing in the core in a partial state.

### 6.3 Compensatory retries inside the core are prohibited

A loop inside the core is a prohibited **compensatory retry** if it re-attempts an operation whose success you could have ensured but didn't.

If you can manage it, you own it. Anything you can prevent but don't is a design failure, not an external event.

This includes:

- Memory allocation you could have pre-allocated or pooled,
- Lock contention you could have eliminated through ownership design,
- Resource exhaustion you could have bounded,
- Any failure mode you had the power to prevent.

Such loops MUST be located at the boundary and MUST be governed by an explicit policy component. The boundary is where you handle consequences of choices you made about what to control.

Deterministic algorithms that iterate toward a result (e.g., numeric methods, search, parsing) are not retries under this definition because their termination is determined by inputs, not by hoping conditions change.

---

## 7. Single Point of Encoding (DRY as Invariant Ownership)

### 7.1 Problem: Duplicated invariant encoding is distributed ownership of correctness

If the same invariant is encoded in two places—two types, two validation paths, a type and a runtime check—those encodings can diverge. One boundary accepts emails up to 254 characters; another accepts 320. Both produce `ValidEmail`. The invariant now has two owners, and §6.1 already prohibits this for mutable resources. The same reasoning applies to invariant definitions.

### 7.2 Requirement: Each invariant MUST have exactly one canonical proof-carrying representation

The system MUST define exactly one canonical proof-carrying representation for each invariant required by any core-callable operation (a controlled type, typestate, capability token, or equivalent).

### 7.3 Requirement: Exactly one Authority Boundary MUST establish each proof

All construction paths that claim to establish an invariant MUST converge on a single Authority Boundary. Boundary code MAY have multiple ingestion points (I/O formats, APIs, protocols), but those adapters MUST delegate to the canonical proof-producing constructor, factory, or transition. Re-implementing the same invariant predicate in multiple boundary locations is non-conforming because it creates multiple interpretations of the same invariant.

### 7.4 Requirement: Core code MUST NOT re-check or re-derive encoded invariants

If core code contains range checks, structural checks, `is_valid_*` calls, or assertions that restate an invariant already implied by the types, the invariant is not encoded as a reusable proof and the design is non-conforming (see §12).

### 7.5 Requirement: Shared invariants MUST be composed, not copied

If multiple domain-specific types share a common invariant, they MUST reuse the canonical proof by composition (wrapping or containing the proof object, or delegating construction to it) rather than copying the predicate. Distinct domain types are permitted; duplicated invariant logic is not.

### 7.6 Requirement: Derived data MUST NOT be stored independently of its source

If a value is computable from existing state, storing it as a separate field creates a synchronization obligation between source and cache. This is §6.1's coordination problem applied to data rather than resources.

**Non-conforming:** A struct storing both `items: Vec<T>` and `count: usize` where `count` must equal `items.len()`. The type permits `count = 5` with three items.

**Conforming:** Compute on access (`fn count(&self) -> usize { self.items.len() }`). If caching is required for performance, encapsulate behind an Authority Boundary that maintains the invariant internally and does not expose independent mutation of both fields.

### 7.7 Non-conforming patterns

| Pattern | Flaw |
|---|---|
| Same validation logic in two boundary modules | Distributed encoding; divergence is inevitable |
| Type + `assert` for the same constraint | Redundant or incomplete (§12.1) |
| Cached derived value with public mutation of source | Synchronization obligation without ownership (§6.1) |
| Two proof types for the same invariant | Core must choose which to trust; the other is a lie |
| Copy-pasted boundary conversion | Forked encoding with independent drift |

*Illustrative example (non-normative):*

- `NonEmptyString` is the canonical proof for "string is non-empty."
- `EmailAddress` composes `NonEmptyString` and adds stronger constraints, without duplicating "non-empty" checks in every consumer.

The core accepts `EmailAddress` (proof-carrying), never `String` + `is_valid_email(...)`.

### 7.8 Relationship to existing sections

This section does not introduce a new principle. It applies §6 (Ownership as Coordination Elimination) to invariant definitions and §2.1 (Primary Rule) to their encoding sites. If an invariant has two encodings, they will eventually disagree, and the core will accept a value that satisfies one encoding but violates the other.

An invariant with two owners is an invariant with zero guarantees.

---

## 8. Mechanism vs Policy

### 8.1 Requirement: Providers MUST report facts; they MUST NOT silently choose fallbacks

A data provider (mechanism) MUST NOT:

- substitute a default value when the requested value is missing, and
- return that default through the same type as the real value.

If a provider can return "real data" or "fallback," then the interface MUST distinguish these outcomes structurally (e.g., sum type / tagged union / result type) so the caller must explicitly choose a policy.

### 8.2 Requirement: Absence MUST be represented as absence

Returning a value that is "not the requested value" but is typed identically to the requested value is non-conforming if the caller cannot distinguish the cases through the type/interface alone.

---

## 9. State as Location

### 9.1 Requirement: Lifecycle state MUST be represented structurally

If a lifecycle state changes (a) which operations are valid, or (b) which data members are valid/meaningful, then that lifecycle state MUST be represented as a distinct type (typestate) or as a variant in a discriminated union where each variant owns its own data.

The type is the state. A `HungryCat` eats. A `Cat` doesn't. The container herds; it doesn't enforce. The type is the enforcement.

### 9.2 Tag fields vs. discriminated unions

A **tag field** is an enum or boolean stored alongside data in a product type (struct). The tag and the data are independent fields that can desynchronize. Tag fields MUST NOT represent lifecycle state.

A **discriminated union** (Rust `enum` with data variants, C++ `std::variant`) is a sum type where each variant owns exactly the data valid for that state. No desynchronization is possible because the tag and the data are the same allocation. Discriminated unions are the prescribed pattern for lifecycle state.

The distinction: if you can change the tag without changing the data, the tag is a field and the design is non-conforming. If changing the state requires constructing a different variant with different data, the union is structural and conforms.

A tag field (boolean, C-style enum, or flag) MAY exist only as behavioral configuration whose values do not change which fields are valid (the "memory layout test," §14.2).

### 9.3 The domain enumeration test

To determine whether a discriminated union is conforming:

1. Enumerate the states the domain entity can occupy using only domain language. A network socket is Connecting, Connected, or Draining. A document is Draft, Published, or Archived. The domain defines the list; the implementation encodes it. If a state has no name in the domain, it is not a state.

2. Compare the list to the variants in the code. Every variant MUST map to exactly one domain state from step 1. Any variant that does not—`None`, `Empty`, `Unknown`, `Default`—is encoding absence or incomplete resolution, not a domain state.

A variant that exists because the code needs it but the domain doesn't name it is non-conforming. The domain is the spec. The code serves the domain, not the reverse.

`Idle | Streaming(TurnState)` — both are named domain states of a turn lifecycle. Conforming.

`Some(Connection) | None` — `None` has no domain name. It is absence, not a state. Non-conforming in core.

### 9.4 Requirement: State transitions MUST be performed by moving between types

To change lifecycle state, the entity MUST be transformed into a different type or variant representing the new state.

- The type is the state.
- There is no flag that can diverge from reality.
- You don't check what you have. You know what you have by the type you're holding.

---

## 10. Capability Tokens

### 10.1 Requirement: Phase-conditional operations MUST require a phase proof

If an operation is valid only during a specific phase (e.g., a render pass), the operation MUST require a capability token that:

- cannot be obtained outside that phase, and
- cannot outlive that phase.

Assertions like `assert(is_rendering)` are non-conforming as a substitute for a missing token, because they detect an invalid call after the architecture already permitted it.

### 10.2 Token lifetime MUST match phase lifetime

A token that proves a phase MUST have a lifetime that is at most the phase lifetime. If a token can be stored and reused later to call phase-restricted operations, it is not a proof; it is a bypass.

---

## 11. Boundary and Core Separation

### 11.1 Requirement: Boundary code MUST convert; core code MUST assume

IFA replaces "validate everywhere" with a strict conversion discipline:

- The boundary MUST accept messy input and convert it into strict representations.
- The core MUST accept only strict representations.
- The boundary MUST express conversion failure explicitly (error return, result type, etc.).

### 11.2 Optionality is a representable invalid state

Core interfaces MUST NOT accept:

- nullable pointers,
- `optional<T>`,
- sentinel values (`-1`, `""`, `null`),
- out-parameters that might not be written.

If a field can be absent, the type is wrong. Restructure until absence is not representable. A `User` with an optional middle name is two types: `User` and `UserWithMiddleName`. The split may feel mechanical. That feeling is not an argument (§18).

If you can encode the invariant — if the language permits a representation where the absent state does not exist — and you choose `Option` instead, you have chosen non-conformance. §19 applies: ignorance is permissible; knowing and refusing is not.

---

## 12. Assertions

### 12.1 Requirement: Assertions MUST NOT enforce representationally expressible invariants

If an assertion exists in core code to prevent a state that is representable by the types/interfaces, the design is non-conforming. The invariant MUST be encoded such that the asserted condition is not representable as an input to the core operation.

### 12.2 Language limitations

If an invariant cannot be fully encoded:

1. Encode as much as the language permits.
2. Make violation require explicit circumvention (unsafe blocks, casts, reflection, ignoring warnings).

The test: if someone violates the invariant, did they have to try? If the violation was the path of least resistance, the design is non-conforming.

If the invariant cannot be encoded at all, that component is not IFA-conformant. There is no exception register. There are no documented gaps. The invariant is encoded or it isn't. Partial credit doesn't exist.

---

## 13. Non-Conforming Patterns

The following are structural anti-patterns under IFA. Each indicates that an invariant exists but is not encoded:

1. **State-dependent validity via flags**
   A flag determines which fields are valid, but all fields exist simultaneously. Use distinct types.

2. **Deep "maybe" checks**
   Core logic checks `if (ptr)` / `if (optional)` / `if (id != -1)`. The function accepted "maybe T" while claiming it needed "T."

3. **Sentinel values**
   Using in-band values (e.g., `-1`, `""`, `null`) to represent non-values while keeping the same type.

4. **Two-phase initialization**
   `init()` / `update()` patterns that complete construction after the constructor. Construction MUST establish invariants. Pre-init objects are invalid states.

5. **Compensatory retry loops in the core**
   See §6.3.

6. **Protective guards as policy in disguise**
   Guard code that "makes it work anyway" rather than rejecting at the boundary or refining representation.

7. **Get-or-default mechanisms**
   Mechanism choosing a fallback without structurally communicating that a fallback occurred.

8. **Optional fields in core interfaces**
   Any `optional<T>`, nullable pointer, or sentinel value in a core interface. Absence is a representable invalid state (§11.2).

9. **`None` as an enum variant**
   A `None` or empty variant encoding absence in a discriminated union. If removing the variant leaves the domain model complete, the variant was encoding absence and is non-conforming (§9.3).

---

## 14. Litmus Tests

### 14.1 Enum/Flag Test

An enum or flag is permissible only if changing it does not change which data members are valid.

If a flag changes which fields are meaningful, the representation is wrong. The data structure MUST change with the state.

### 14.2 Memory Layout Test

If the value of a flag or enum changed, would the same struct fields still be valid and meaningful? If no, the flag is encoding lifecycle state and violates §9.

### 14.3 Optionality Test

For every optional or nullable field in a core interface: why can this be absent?

The answer doesn't matter. The field shouldn't exist.

The existence of an object is the validity of its own completeness. If you have a `User`, the user is complete. If you have a `UserWithAllergies`, they have allergies. If they don't have allergies, you have a different type—one with no allergy field.

You don't check for presence. You don't handle absence. The type you hold is the fact.

### 14.4 Review Question

The mandatory design review question:

> "Which invalid states are currently representable, and what change would make them unrepresentable to the core?"

A design is non-conforming if it relies on runtime detection inside the core for an invalid state that could be excluded by representation.

---

## 15. Implementation Notes

These notes apply to statically typed languages with imperfect enforcement. Adapt to your language's constraints.

### 15.1 Typestate

Use move-only types, private constructors, friend factories, and APIs that prevent meaningful use after transition.

### 15.2 Linearity

Moved-from objects are not automatically unusable in most languages. Enforce emptiness via unique ownership or consumption APIs that make moved-from state inert.

### 15.3 Parametricity

Templates and generics are not inherently parametric. Enforce "generic means generic" via explicit project rules (§4.3).

### 15.4 Capability tokens

Use types with restricted constructors and scope-bound lifetimes.

### 15.5 State-as-location

The type is the state. Containers herd; they don't enforce. Don't confuse "we only put ReadyEntities in this Vec" (convention) with state-as-location (types).

### 15.6 Authority Boundaries

Use access modifiers (`private`, `pub(crate)`, etc.) to define actual encapsulation boundaries.

### 15.7 Zombie-state rule

Because moved-from objects remain valid but unspecified in many languages, any design that relies on "move makes the old value unusable" MUST provide a constructive guarantee that the moved-from state is inert with respect to the capabilities that matter. If such a guarantee cannot be stated and verified, the design does not satisfy non-forking transitions.

---

## 16. Known Limitations

### 16.1 Concurrency and Async

Concurrency is boundary logic.

Boundary logic lives at the system edge or in isolated modules whose sole purpose is to contain uncertainty. A concurrency module synchronizes, acquires, waits, and resolves races internally. It exposes synchronous, owned values to the core.

The core does not coordinate. The core does not share. The core receives and executes.

If concurrent logic is interleaved with core logic, redesign. Isolate the concurrency into a module that presents an IFA-conformant interface.

### 16.2 Language Expressiveness

No mainstream language fully enforces IFA. The architecture MUST:

- encode as much as the language permits,
- make violation require explicit circumvention,
- recognize that unencodable invariants are not IFA-conformant.

---

## 17. Operational Definitions Checklist

For a project to claim IFA conformance, it MUST provide explicit definitions for:

1. **Invariant registry**: A declared list of all invariants and their encoding mechanism.

2. **Authority Boundary map**: For each proof object or controlled type, which code constitutes the Authority Boundary.

3. **Parametricity rules**: Explicit bans on specialization and type-trait branching that would violate implementation agnosticism, plus review criteria.

4. **Move semantics rules**: What counts as "unusable after transition" (e.g., resource-holding members must use unique ownership; moved-from objects must be provably inert).

5. **DRY proof map**: For each invariant, one canonical proof type and one Authority Boundary (§7).

---

## 18. Non-Acceptable Objections

The following objections are invalid under IFA:

**"It's simpler this way."**
If "simple" means "easy to write" while increasing reasoning complexity, it is rejected. IFA optimizes for simplicity of reasoning, not ease of writing.

**"We'll be careful."**
Discipline is not architecture. If correctness depends on being careful, the invariant is not encoded.

**"We'll document it."**
Documentation is not enforcement. If the invalid state compiles, it will ship.

**"We'll assert it."**
Assertions detect invalid states after the architecture permitted them. This is non-conforming (§12.1).

**"We'll handle it later in the core."**
If the core contains logic to reject invalid states, those states were representable at the interface. Move rejection to the boundary.

**"The language can't enforce it."**
This is not permission to leave invalid states easy to reach. Make violation require circumvention (§12.2). If you can't, that component isn't IFA-conformant.

**"The domain allows absence."**
The domain is wrong. Restructure (§11.2).

---

## 19. Closing Statement

A system must encode all invariants.

"All" means all. Not "the ones we got to." Not "the important ones." All.

Failure to encode a known invariant is not a gap in the spec. It's a choice to leave invalid states representable. That choice has a name: negligence.

Ignorance is permissible—you can't encode what you don't know. But the moment you identify an invariant and choose not to encode it, you've chosen non-conformance.

The goal is a codebase where **"it compiles"** is a high-confidence statement of **structural integrity**. Not partial integrity. Not "mostly, except for the stuff we documented." Integrity.
