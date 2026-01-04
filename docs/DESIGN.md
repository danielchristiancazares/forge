# Type-Driven Design: The Philosophy

## How to Read This Document

**Absorb the principle, not the syntax.**

The examples exist to illustrate structural concepts, not to be copied outright. The principle behind every pattern is **Correctness by Construction**. If you understand this, you can derive the correct implementation for any situation. If you are looking for escape hatches in the examples, you are missing the point.

## The Core Idea

**Make invalid states unrepresentable.**

We do not write code to "handle" invalid states; we design types so that invalid states cannot mathematically exist. Do not rely on programmer discipline, runtime checks, or documentation.

If invalid code compiles, the type definitions do not accurately reflect the domain. Successful compilation must be sufficient proof of structural validity.

---

## Typestate: The ShuffledDeck Pattern

**The Problem:** Temporal coupling hidden in side effects.

Games require a shuffled deck. A `Deck` object that *might* be shuffled is a lie. Nothing prevents `startGame(unshuffledDeck)`.

**The Semantic Fix:**

The `shuffle` function is not an action; it is a **transition**. It consumes the precursor state and emits the valid state.

```cpp
class Deck { ... }; // Represents a raw deck

class ShuffledDeck {
    friend ShuffledDeck shuffle(Deck&&); // Consumption
    ShuffledDeck() = default; 
};

// The Transition: Consumes 'Deck', proves 'ShuffledDeck' exists
[[nodiscard]] ShuffledDeck shuffle(Deck&& d);

// The Consumer: Physically impossible to call without the proof
void startGame(ShuffledDeck d);
```

**The Reality:** The types now mirror the physics of the domain. `Deck` and `ShuffledDeck` are not just different classes; they are different universes of validity. Move semantics prevent "use-after-transition," ensuring the timeline allows no forks.

The Return Type is the Proof of History.

---

## Parametricity (Enforced Agnosticism)

**The Problem:** Inspection breeds coupling.

A function taking `const vector<Card>&` has too much power. It can inspect cards, log values, or branch logic based on specific suits.

**The Semantic Fix:**

Polymorphism is not just code reuse; it is Structural Blindness.

```cpp
template <typename T>
T get_first(const std::vector<T>& list);
```

**The Semantic Guarantee:**

By quantifying over `T`, we strip the implementation of the ability to inspect the data. The signature `forall A. [A] -> A` constrains the implementation to pure data movement. It operates on the structure of the container, never the nature of the content.

The constraint forces the correct implementation to be the path of least resistance.

---

## Constrained Generics (Call-Site Rejection)

Parametricity blinds the implementation (what it can do). Constraints filter the call site (what it accepts).

```cpp
// Unconstrained: Accepts garbage, fails deep inside logic
template <typename T>
void serialize(const T& obj);

// Constrained: Rejects garbage at the boundary
template <typename T>
    requires Serializable<T>
void serialize(const T& obj);
```

SFINAE (and Concepts) moves the rejection from inside the template instantiation to the signature. The function literally does not exist for types that cannot satisfy it.

**The Boundary Principle:** Invalid instantiations must not propagate past the signature.

---

## The Shared Foundation

| Pattern | The Semantic Guarantee |
|---|---|
| Typestate | Proof of History. (`B` cannot exist unless `A` happened first). |
| Parametricity | Implementation Agnosticism. (Logic cannot depend on data values). |
| Constraints | Call-Site Rejection. (Invalid inputs are structurally incompatible). |
| Move Semantics | Linearity. (Resource is at exactly one place at one time). |
| State as Location | Synchronization. (Flag and map cannot disagree). |
| Capability Tokens | Phase Validity. (Code cannot run outside its time slot). |

---

## Ownership is Coordination

**Make coordination unnecessary by making ownership complete.**

If two pieces of code must "agree" on the state of a resource, the architecture is flawed. One object should own the resource.

* **Fragmented Ownership:** Function takes an ID and a pointer, tries to fill the pointer. Caller owns memory, Callee owns logic. Result: Boolean return codes and undefined states.
* **Complete Ownership:** Function takes an ID, returns a fully constructed object or a boundary error. The core never receives a "maybe."

**Corollary: The Retry Fallacy.**

Retry loops inside the core are consensus failures—logic attempting to synchronize with itself. If you have to ask "did that work?" and try again, two components have different views of reality. Fix the ownership scope so disagreement is impossible.

(Retries at the boundary are adaptation to an unreliable world. Retries inside are architecture rot.)

---

## Data Providers Don't Decide (Mechanism vs. Policy)

**Providers expose Mechanism (facts); Callers decide Policy (actions).**

When a data provider returns a fallback value (e.g., a default texture) because an ID was missing, it is usurping the caller's agency.

* **Wrong:** `TextureManager::get(id)` returns a default texture. The Manager decides "The game must go on."
* **Right:** `TextureManager::available()` returns a read-only view of what exists. The Caller decides "I need this texture; if missing, I will trigger a loading screen."

The texture manager herds cats to food—it makes textures available. It doesn't need to know if each cat is hungry. Cats (callers) can see what's available and decide for themselves.

**Semantic Rule:** If a function returns "Fallback OR Real Data" without the type system distinguishing them, you have hidden a decision inside a data access.

---

## State as Location

**Existence is state.**

Do not use `enum State` or `bool is_active` to track lifecycle. An object's state is defined by where it is, not what it contains.

* **Wrong:** A list of objects with status flags (`Pending`, `Resident`). You must filter the list to find valid ones.
* **Right:** Two containers: `pending_uploads` and `resident_textures`.

To change state, you move the object from one container to another.

**Proof:** If an object is in `m_resident`, it is resident. There is no flag to de-sync. The data structure *is* the state machine.

Topology is Truth.

---

## Capability Tokens

**Capability tokens are "Tickets to Ride."**

If an operation is only valid during a specific phase (e.g., Rendering), require a token that only exists during that phase.

```cpp
void submitDrawCall(const RenderPass& proof, Mesh m);
```

You do not need `assert(is_rendering)`. The code physically cannot be called unless you possess a `RenderPass` object. If the token exists on the stack, the phase is active.

---

## Boundaries vs. Internals

**Parse, don't validate.**

A system has a Crust (Boundary) and a Core (Internal).

* **The Crust:** Accepts messy, optional input. It parses (converts) this into strict Types.
* **The Core:** Operates on strict Types. No `optional`, no `nullptr` checking.

**The Disease:** Passing `std::optional` or `ptr*` deep into the core logic. This forces every internal function to handle the "missing" case, spreading the complexity of the boundary throughout the entire application.

* **Domain-Optional:** A legitimate absence (e.g., User has no middle name).
* **State-Optional:** A leaky abstraction (e.g., Object exists but `init()` hasn't been called). Eliminate state-optionality.

---

## On Assertions

> "An assertion is a confession: you let the wrong world exist, and now you're policing its borders."

Guards don't prevent invalid states. They catch invalid states that your types already permitted. The types are the border. If you're writing `assert`, you've already let the enemy inside the walls.

**Don't build a border patrol. Build a world with no illegal crossings.**

---

## The Death List

When reviewing code, interpret these patterns as structural failures:

| Pattern | The Semantic Flaw |
|---|---|
| State-Dependent Validity | Fake Sum Type. An enum tag that implies which fields are inhabited is a sum type implemented unsafely. Use distinct types or `variant`. |
| Inhabitant Branching | Lying Signature. Checks like `if (ptr)` deep in logic mean the function requested a `T` but accepted a `Maybe T`. |
| Sentinel Values | In-Band Signaling. `-1` or `""` masquerading as valid types. |
| Init/Update Methods | Step-Coupling. Constructors must complete initialization. |
| Retry Loops (in core) | Consensus Failure. Logic attempting to synchronize with itself. |
| Protective Guards | Normalization of Deviance. `assert(impossible)` protects invalid state from being noticed. |
| GetOrDefault | Policy Leak. Data provider making decisions for the consumer. |

---

## The Litmus Test: Memory Layout

Not all enums are bad. Apply the **Memory Layout Test** to distinguish "Behavior" from "State."

* **Behavioral Configuration (Valid):** `enum AI { Aggressive, Defensive }`
    * Does changing this enum invalidate any member variables? **No.**
    * The object structure remains identical; only the algorithm changes.

* **Structural State (Invalid):** `enum Connection { Disconnected, Connected }`
    * Does changing this enum invalidate `m_socketHandle`? **Yes.**
    * This is a Sum Type masquerading as a Product Type. You are manually managing a relationship the compiler doesn't see.

**Rule:** If the validity of data depends on the value of a flag, that flag must not exist. The data structure itself must change (via `variant` or move to a new type).

---

## Approximations in C++

C++ lacks dependent types, has weak parametricity, and permits escape hatches. We approximate the ideal:

| Ideal | C++ Approximation |
|---|---|
| Typestate | Move-only types, private constructors, friend factories. |
| Linear Types | `= delete` copy, move-only, `[[nodiscard]]`. |
| Parametricity | Templates with minimal constraints. |
| Bounded Polymorphism | SFINAE, `requires` clauses, Concepts. |
| Capability Tokens | Structs with private constructors, `friend` access. |
| State as Location | Separate containers per state. |

**Lock the door anyway.**

"C++ can't prevent X" is not an excuse to leave X easy. If you can't eliminate an invalid state, make it awkward to reach:

* **The Zombie State:** C++ moves are non-destructive (source remains). Use `unique_ptr` to force emptiness, or destructive `consume() &&` methods.
* **Explicit Intent:** Use rvalue-qualified methods to force "use-once" semantics where possible.

Saying "the language can't enforce it perfectly" is like saying "my car door doesn't prevent all theft." True—but you still lock it.

---

## The Test

When reviewing a design, do not ask:

> "What happens if someone passes the wrong data?"

Ask:

> "How do I make passing the wrong data a compile error?"

We do not want to detect invalid state at runtime. We want to define invalid state out of existence.

---

## Not Acceptable

Objections based on "simplicity" usually conflate ease of writing with simplicity of reasoning.

* Writing a global boolean is "easy."
* Debugging a race condition caused by that boolean is "complex."

We optimize for the latter. The runtime cost of a container move is negligible compared to the cost of debugging a state mismatch that only manifests in production.

---

The goal is a codebase where "It compiles" is a high-confidence statement of structural integrity.
