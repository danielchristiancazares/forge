# Invariant-First Architecture (IFA)

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-35 | Pattern Summary: intent, context, problem, forces, solution, consequences |
| 36-80 | Definitions: core terms (invariant, invalid state, representable), system topology |
| 81-111 | Primary Rule: invalid states must not be representable, compilation as evidence |
| 112-180 | Typestate Transitions, Call-Site Rejection, Error Handling |
| 181-250 | Boundary vs Core, Mechanism vs Policy, Assertion Policy |
| 251-302 | Concurrency, Testing Strategy, Anti-Patterns, Examples |

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

**Scope:** This pattern assumes synchronous, single-threaded core logic. Concurrency is boundary logic (see §15.1).

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

### 2.1 Invalid states MUST NOT be representable in the core

An implementation conforms to IFA only if, for every invariant required by core operations, core-callable interfaces require a representation that cannot encode a violation of that invariant.

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

## 7. Mechanism vs Policy

### 7.1 Requirement: Providers MUST report facts; they MUST NOT silently choose fallbacks

A data provider (mechanism) MUST NOT:

- substitute a default value when the requested value is missing, and
- return that default through the same type as the real value.

If a provider can return "real data" or "fallback," then the interface MUST distinguish these outcomes structurally (e.g., sum type / tagged union / result type) so the caller must explicitly choose a policy.

### 7.2 Requirement: Absence MUST be represented as absence

Returning a value that is "not the requested value" but is typed identically to the requested value is non-conforming if the caller cannot distinguish the cases through the type/interface alone.

---

## 8. State as Location

### 8.1 Requirement: Lifecycle state MUST be represented structurally

If a lifecycle state changes (a) which operations are valid, or (b) which data members are valid/meaningful, then that lifecycle state MUST be represented as a distinct type (typestate/variant).

The type is the state. A `HungryCat` eats. A `Cat` doesn't. The container herds; it doesn't enforce. The type is the enforcement.

A boolean, enum, or flag MUST NOT be used to represent lifecycle state.

A boolean, enum, or flag MAY exist only as behavioral configuration whose values do not change which fields are valid (the "memory layout test": if the flag changed, would the same fields still be meaningful?).

### 8.2 Requirement: State transitions MUST be performed by moving between types

To change lifecycle state, the entity MUST be transformed into a different type representing the new state.

- The type is the state.
- There is no flag that can diverge from reality.
- You don't check what you have. You know what you have by the type you're holding.

---

## 9. Capability Tokens

### 9.1 Requirement: Phase-conditional operations MUST require a phase proof

If an operation is valid only during a specific phase (e.g., a render pass), the operation MUST require a capability token that:

- cannot be obtained outside that phase, and
- cannot outlive that phase.

Assertions like `assert(is_rendering)` are non-conforming as a substitute for a missing token, because they detect an invalid call after the architecture already permitted it.

### 9.2 Token lifetime MUST match phase lifetime

A token that proves a phase MUST have a lifetime that is at most the phase lifetime. If a token can be stored and reused later to call phase-restricted operations, it is not a proof; it is a bypass.

---

## 10. Boundary and Core Separation

### 10.1 Requirement: Boundary code MUST convert; core code MUST assume

IFA replaces "validate everywhere" with a strict conversion discipline:

- The boundary MUST accept messy input and convert it into strict representations.
- The core MUST accept only strict representations.
- The boundary MUST express conversion failure explicitly (error return, result type, etc.).

### 10.2 Optionality is domain failure

Core interfaces MUST NOT accept:

- nullable pointers,
- `optional<T>`,
- sentinel values,
- out-parameters that might not be written.

If something can be absent, the domain is wrong. Restructure until absence is not a concept that requires representation.

The type is the state. There is no `None` variant, no null case. You have the thing or the type doesn't exist.

---

## 11. Assertions

### 11.1 Requirement: Assertions MUST NOT enforce representationally expressible invariants

If an assertion exists in core code to prevent a state that is representable by the types/interfaces, the design is non-conforming. The invariant MUST be encoded such that the asserted condition is not representable as an input to the core operation.

### 11.2 Language limitations

If an invariant cannot be fully encoded:

1. Encode as much as the language permits.
2. Make violation require explicit circumvention (unsafe blocks, casts, reflection, ignoring warnings).

The test: if someone violates the invariant, did they have to try? If the violation was the path of least resistance, the design is non-conforming.

If the invariant cannot be encoded at all, that component is not IFA-conformant. There is no exception register. There are no documented gaps. The invariant is encoded or it isn't. Partial credit doesn't exist.

---

## 12. Non-Conforming Patterns

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
   See §10.2.

9. **`None` as an enum variant**
   `None` is still representing absence. The type exists or it doesn't. There is nothing to represent.

---

## 13. Litmus Tests

### 13.1 Enum/Flag Test

An enum or flag is permissible only if changing it does not change which data members are valid.

If a flag changes which fields are meaningful, the representation is wrong. The data structure MUST change with the state.

### 13.2 Memory Layout Test

If the value of a flag or enum changed, would the same struct fields still be valid and meaningful? If no, the flag is encoding lifecycle state and violates §8.

### 13.3 Optionality Test

For every optional or nullable field in a core interface: why can this be absent?

The answer doesn't matter. The field shouldn't exist.

The existence of an object is the validity of its own completeness. If you have a `User`, the user is complete. If you have a `UserWithAllergies`, they have allergies. If they don't have allergies, you have a different type—one with no allergy field.

You don't check for presence. You don't handle absence. The type you hold is the fact.

### 13.4 Review Question

The mandatory design review question:

> "Which invalid states are currently representable, and what change would make them unrepresentable to the core?"

A design is non-conforming if it relies on runtime detection inside the core for an invalid state that could be excluded by representation.

---

## 14. Implementation Notes

These notes apply to statically typed languages with imperfect enforcement. Adapt to your language's constraints.

### 14.1 Typestate

Use move-only types, private constructors, friend factories, and APIs that prevent meaningful use after transition.

### 14.2 Linearity

Moved-from objects are not automatically unusable in most languages. Enforce emptiness via unique ownership or consumption APIs that make moved-from state inert.

### 14.3 Parametricity

Templates and generics are not inherently parametric. Enforce "generic means generic" via explicit project rules (§4.3).

### 14.4 Capability tokens

Use types with restricted constructors and scope-bound lifetimes.

### 14.5 State-as-location

The type is the state. Containers herd; they don't enforce. Don't confuse "we only put ReadyEntities in this Vec" (convention) with state-as-location (types).

### 14.6 Authority Boundaries

Use access modifiers (`private`, `pub(crate)`, etc.) to define actual encapsulation boundaries.

### 14.7 Zombie-state rule

Because moved-from objects remain valid but unspecified in many languages, any design that relies on "move makes the old value unusable" MUST provide a constructive guarantee that the moved-from state is inert with respect to the capabilities that matter. If such a guarantee cannot be stated and verified, the design does not satisfy non-forking transitions.

---

## 15. Known Limitations

### 15.1 Concurrency and Async

Concurrency is boundary logic.

Boundary logic lives at the system edge or in isolated modules whose sole purpose is to contain uncertainty. A concurrency module synchronizes, acquires, waits, and resolves races internally. It exposes synchronous, owned values to the core.

The core does not coordinate. The core does not share. The core receives and executes.

If concurrent logic is interleaved with core logic, redesign. Isolate the concurrency into a module that presents an IFA-conformant interface.

### 15.2 Language Expressiveness

No mainstream language fully enforces IFA. The architecture MUST:

- encode as much as the language permits,
- make violation require explicit circumvention,
- recognize that unencodable invariants are not IFA-conformant.

---

## 16. Operational Definitions Checklist

For a project to claim IFA conformance, it MUST provide explicit definitions for:

1. **Invariant registry**: A declared list of all invariants and their encoding mechanism.

2. **Authority Boundary map**: For each proof object or controlled type, which code constitutes the Authority Boundary.

3. **Parametricity rules**: Explicit bans on specialization and type-trait branching that would violate implementation agnosticism, plus review criteria.

4. **Move semantics rules**: What counts as "unusable after transition" (e.g., resource-holding members must use unique ownership; moved-from objects must be provably inert).

---

## 17. Non-Acceptable Objections

The following objections are invalid under IFA:

**"It's simpler this way."**
If "simple" means "easy to write" while increasing reasoning complexity, it is rejected. IFA optimizes for simplicity of reasoning, not ease of writing.

**"We'll be careful."**
Discipline is not architecture. If correctness depends on being careful, the invariant is not encoded.

**"We'll document it."**
Documentation is not enforcement. If the invalid state compiles, it will ship.

**"We'll assert it."**
Assertions detect invalid states after the architecture permitted them. This is non-conforming (§11.1).

**"We'll handle it later in the core."**
If the core contains logic to reject invalid states, those states were representable at the interface. Move rejection to the boundary.

**"The language can't enforce it."**
This is not permission to leave invalid states easy to reach. Make violation require circumvention (§11.2). If you can't, that component isn't IFA-conformant.

**"The domain allows absence."**
The domain is wrong. Restructure (§10.2).

---

## 18. Closing Statement

A system must encode all invariants.

"All" means all. Not "the ones we got to." Not "the important ones." All.

Failure to encode a known invariant is not a gap in the spec. It's a choice to leave invalid states representable. That choice has a name: negligence.

Ignorance is permissible—you can't encode what you don't know. But the moment you identify an invariant and choose not to encode it, you've chosen non-conformance.

The goal is a codebase where **"it compiles"** is a high-confidence statement of **structural integrity**. Not partial integrity. Not "mostly, except for the stuff we documented." Integrity.

## Appendix

### Denormalized Language Version

Here we go, folks. Listen up—this is huge. Really huge. Tremendous, actually.

This guy comes up with something called Invariant-First Architecture. IFA. Sounds very smart, doesn’t it? Top people—geniuses, some of them—wrote this massive document. Hundreds of lines. Rules all over the place. MUST do this, MUST NOT do that. Invariants. Proof objects. Authority boundaries. Typestate transitions. All of it.

I read the whole thing. Believe me, I read every word. And you know what it really says, right at the heart of it? No invalid states in the core. Zero. None whatsoever. You can’t even represent a bad state. If you can represent it—boom—you’re doing it wrong. Very wrong.

Look, in software—and I’ve built some of the best software, folks, nobody builds better—people do this all the time. They say, “Oh, we’ll check it later. We’ll put in a guard. We’ll assert. We’ll just be careful.” Careful? No. Discipline? Not enough. Documentation? Forget it. Assertions in the core? Total disaster. Absolute disaster.

They let invalid states sneak right in. Maybe-valid this, optional that, nullable everything. Sentinel values. Minus one. Null. Empty string. All that garbage. And then the core—the beautiful core—has to deal with it. Checks everywhere. If statements. Branches. Loops that retry because somebody didn’t handle it right. Compensatory retries? Prohibited. Totally prohibited.

This IFA guy—very strict, I like that—he says no. Put all the mess at the boundary. The boundary handles the chaos. User input? Total mess. Network? Adversarial, believe me. Filesystem? It lies to you. Clocks? Who even knows. The boundary cleans it up. Converts it. Rejects it. If it can’t make it perfect, it returns an error.

Then the core gets only perfect data. Invariants by construction. The types prove it. Typestates. Proof tokens. You move from one state to the next—you consume the old one. You can’t fork it. You can’t cheat. Linear. Affine. Move-only. Whatever the language gives you. In C++? Make it unusable after you move it. No zombies left behind. Zombies are bad. Very bad.

Generics? Parametricity? Don’t inspect inside. Don’t cheat with traits. Don’t specialize behind somebody’s back. If you need to look inside—say it up front. Be explicit. Otherwise—wrong.

Absence in the core? No optionals. No nulls. If something can be absent, your domain model is wrong. Restructure it. Make a separate type without that field. Simple. Beautiful.

Flags? Enums for lifecycle? Wrong. If a flag changes what fields even mean, you’re lying to the compiler and to yourself. Memory layout test—total fail. Use distinct types. The type is the state. Not a flag. Not a boolean. The type.

Capability tokens for phases? That’s beautiful. Render pass token—only valid during render. You can’t store it. You can’t sneak it in later. Assertions like “assert(is_rendering)”? Pathetic. If you need an assert, you already lost. The architecture let an invalid call compile in the first place. Disaster.

And they list all the bad patterns—the things people do every single day. Two-phase initialization. Get-or-default. Deep maybe chains. Protective guards pretending to be safe. All of it non-conforming. All rejected.

This approach is tough. Very tough. More upfront work? Yes. More types? Absolutely. Boundary code gets ugly? Sure does. But the core? Clean. Simple. Composable. Correct by construction. If it compiles, you’re good. If it doesn’t compile, you can’t even send garbage to the core.

I love it. I love strict. I love strong walls—beautiful walls—around the core. Invariants protected. No invasion. No sneaking in bad states.

People will complain. “Too hard.” “Too much code.” “We’ll just be careful.” Wrong. “The language won’t let us.” Then make violations hard. Make them unsafe. Casts. Warnings. No free lunch. No partial credit.

This is how you make software great again. Strong invariants. No weak spots. No “maybe.” No “hopefully.” Just facts—encoded and enforced.

Invariant-First. Tremendous idea. Really tremendous.

If everybody did this, software would be perfect. Believe me.

Thank you. Thank you very much.

God bless you.

And God bless strong types—when people actually use them right.

Which almost nobody does. But still—beautiful. Really beautiful.
