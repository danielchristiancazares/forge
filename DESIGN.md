# Type-Driven Design: The Rust Philosophy

## How to Read This Document

Absorb the principle, not the syntax. Examples illustrate structural concepts. The principle behind every pattern is Correctness by Construction. Understand this to derive the correct implementation.

Readers seeking escape hatches miss the point. Reaching for `.unwrap()`, `.clone()`, `unsafe`, or `Arc<Mutex<T>>` wrappers indicates failure to grasp the philosophy.

This document specifies a stronger contract than raw Rust semantics. If Rust permits a construct but this contract forbids it (for example `Option` in core domain logic), the contract wins.

## The Core Idea

Make invalid states unrepresentable. Design types so invalid states cannot exist. Reject reliance on discipline, runtime checks, or documentation.

Invalid code that passes `cargo check` exposes type definitions as inadequate. Compilation proves structural validity. "It compiles" is a theorem.

## Typestate: The ShuffledDeck Pattern

Temporal coupling hides in side effects. Games require shuffled decks. A `Deck` with an `is_shuffled` flag fails architecturally. Mutation leaves the type unchanged. The compiler cannot enforce the sequence.

`shuffle` transitions states. It consumes the precursor and produces the valid state.

```rust

pub struct Deck { /* raw deck */ }

pub struct ShuffledDeck {

    _proof: (), // Private: blocks bypass

}

impl Deck {

    #[must_use]

    pub fn shuffle(self) -> ShuffledDeck {

        // shuffle logic

        ShuffledDeck { _proof: () }

    }

}

pub fn start_game(d: ShuffledDeck) { /* game logic */ }

```

Types reflect the domain's rules. `Deck` and `ShuffledDeck` represent separate stages of validity. Rust's move semantics consume the original value completely. Attempts to use the original after the transition cause compilation errors. The design ensures a single, linear progression without branches. The return type serves as evidence that the transition occurred.

## Parametricity (Enforced Agnosticism)

Functions that accept a specific type, such as `&[Card]`, can inspect elements, log values, or branch based on content. This creates tight coupling between the function and the type.

Use generics to make the function blind to the specific element type.

```rust

fn get_first<T>(list: &NonEmptyList<T>) -> &T {

    &list.inner[0]

}

```

A generic parameter `T` with no trait bounds prevents the function from using type-specific operations on `T`. The signature limits the function to operations on the container's structure, such as indexing. The function cannot access or depend on the meaning of the elements. This restriction ensures the implementation remains decoupled and generic.

## Trait Bounds (Call-Site Rejection)

Parametricity blinds implementations. Bounds reject invalid calls.

```rust

// Unconstrained: No actions possible

fn serialize_blind<T>(obj: &T) {

    // obj.serialize(); // Fails compilation

}

// Constrained: Rejects invalid, enables valid

fn serialize<T: Serialize>(obj: &T) {

    obj.serialize();

}

```

Rust enforces boundaries at signatures. Functions do not exist for unsatisfied bounds. Invalid instantiations halt at the boundary.

## The Shared Foundation

| Pattern | Contract Definition | Enforcement Mechanism (Rust Approximation) |
|---|---|---|
| Typestate | Encode discrete states as separate struct types. Transitions consume `self` and return the next type. | Rejects attempts to call State B methods while data is still in State A, and rejects reuse of State A after transition. |
| Parametricity | Write functions with unbounded generic parameters (for example `fn process<T>(item: T)`). | Prevents type-specific branching, field access, and method calls on `T` without explicit bounds. |
| Trait Bounds | Restrict generic parameters with traits (for example `fn process<T: Serialize>(item: T)`). | Rejects calls at the call site when the provided type does not satisfy required traits. |
| Affine Types (Linearity Target) | Design for exactly-once semantic use of consumed resources and capabilities. | Rust guarantees at-most-once use via move semantics; API shape plus `#[must_use]` tighten this toward exactly-once intent. |
| Algebraic Data Types | Use `enum` (sum types) for mutually exclusive variants with variant-specific data. | Forces exhaustive `match` and prevents accessing data outside the active variant. |
| Capability Tokens | Define zero-sized witness types with private constructors and require them for restricted operations. | Restricts execution to scopes where a valid token can exist and be passed. |

**Absolute Rule:** `Option` and `Result` are boundary parsing artifacts. Core domain logic accepts only strict, inhabited types.

If a domain concept is "optional," model it as separate types (for example `User` and `UserWithMiddleName`), not `Option` fields in core structs.

At boundaries, collapse `Option`/`Result` immediately where they are first observed. Do not forward them through boundary helper layers.

## Ownership is Coordination

Make coordination unnecessary by making ownership complete. If two pieces of code must "agree" on the state of a resource, the architecture is flawed. One component must own the resource.

The borrow checker proves ownership graphs.

- Fragmented Ownership: `Arc<Mutex<T>>` or `Rc<RefCell<T>>` spreads ownership and refuses hierarchy. Deadlocks and panics indicate failure.

- Complete Ownership: Functions consume IDs and produce `Result<Data, Error>`. The core never receives a "maybe."

Retry loops inside the core are consensus failures—logic attempting to synchronize with itself. If you have to ask "did that work?" and try again, two components have different views of reality. Fix the ownership scope so disagreement is impossible.

Boundary retries adapt to unreliability. Internal retries expose flaws.

## Data Providers Don't Decide (Mechanism vs. Policy)

Providers expose mechanisms. Callers enforce policies.

Fallbacks on missing IDs usurp agency.

- Wrong: `get(id)` returns defaults. Managers dictate continuation.

- Wrong: `get(id)` returns `Option<&Texture>`. You moved the policy decision; you didn't eliminate the invalid state.

- Right: Handles prove existence.

```rust

pub struct TextureHandle(());

impl TextureManager {

    pub fn load(&self, id: AssetId) -> Result<TextureHandle, LoadError>;

    pub fn get(&self, handle: &TextureHandle) -> &Texture;

}

```

The texture manager herds cats to food—it makes textures available. It doesn't need to know if each cat is hungry. Cats (callers) can see what's available and decide for themselves. Possession of a handle guarantees existence. The capability token eliminates the question.

**Semantic Rule:** If a function returns "Fallback OR Real Data" without the type system distinguishing them, you have hidden a decision inside a data access.

## Algebraic Data Types (The End of Flags)

Existence defines state. Booleans or status fields tracking lifecycles fail. Variants define states.

Product types (structs) multiply the state space. Sum types (enums) add it. Do not use multiplication when you mean addition.

- Invalid:

```rust

struct Connection {

    state: ConnectionState,

    socket: Option<TcpStream>, // Manual enforcement

}

```

You are manually managing a relationship the compiler doesn't enforce.

- Valid:

```rust

enum Connection {

    Disconnected,

    Connected(TcpStream),

}

```

Flags vanish. Variants prevent desynchronization. Matches force state proofs before data access. Variants enforce truth.

## State as Location

**Existence is the proof of validity.**

Status fields fail lifecycle tracking in collections. Locations define states.

- Wrong: Single `Vec<Asset>` with status fields. Filters expose desyncs.

- Right: Distinct `pending_uploads: Vec<PendingAsset>` and `resident_textures: Vec<ResidentAsset>`.

Transitions consume from one and produce to another. Presence in `resident_textures` proves residency. Structures enforce state machines.

## Capability Tokens

Tokens grant phase access. Operations valid in phases require phase-bound tokens. Zero-sized types enforce at zero cost.

```rust

pub struct RenderPassToken(());

pub fn submit_draw_call(proof: &RenderPassToken, m: Mesh) {

    // draw logic

}

```

Assertions vanish. Calls require possession. Scopes guarantee phases.

## Boundaries vs. Internals

Parse inputs. Boundaries convert mess to strict types. Cores demand strict types.

Boundaries must exhaust `Result` and `Option` immediately at ingestion.

Cores ban `Option` and `Result` in domain signatures and state. Constructors yield fully valid selves.

Passing optionals through multiple boundary functions spreads complexity and normalizes uncertainty. Eliminate optionality in the first boundary function that touches it.

## On Assertions

> "An assertion is a confession: you let the wrong world exist, and now you're policing its borders."

Guards do not prevent invalid states. They catch invalid states that your types already permitted. The types are the border. If you are writing `assert!`, `debug_assert!`, `unwrap()`, `expect()`, or `unreachable!()` in core logic, you already let the enemy inside the walls.

**Do not build a border patrol. Build a world with no illegal crossings.**

## The Death List

| Pattern | The Semantic Flaw |
|---|---|
| Structs with Option Fields | Banned in core. This is a fake sum type that hides lifecycle state and permits invalid combinations. |
| Inhabitant Branching | Lying Signature: Accepts `Option<T>` where the function actually requires inhabited `T`. |
| Sentinel Values | In-Band Signaling: Model domains distinctly. |
| Two-Phase Init | Step-Coupling: Constructors yield valid selves. |
| `.unwrap()` / `expect()` / `unreachable!()` (in core logic) | Normalization of Deviance: Converts compile-time obligations into runtime panics. |
| Interior Mutability (RefCell) | Borrow-Checker Defeatism: Runtime defers compile flaws. |
| Arc<Mutex<...>> Soup | Consensus Failure: Define hierarchies. |
| .clone() Driven Development | Architecture Rot: Fix geometries. |

## Approximations in Rust

Rust lacks dependent types and permits escape hatches (`unsafe`, interior mutability). We approximate the ideal:

| Ideal | Rust Approximation |
|---|---|
| Typestate | Newtypes + consuming transitions (`fn next(self) -> Next`). |
| Linear Types | Affine ownership, move semantics, `#[must_use]`. |
| Parametricity | Generics with minimal trait bounds. |
| Bounded Polymorphism | `where` clauses and trait bounds. |
| Capability Tokens | Zero-sized witness types with private constructors. |
| State as Location | Separate containers per state, or enums when co-located. |

**Lock the door anyway.**

"Rust can't prevent X" is not an excuse to leave X easy. If you can't eliminate an invalid state, make it awkward to reach:

- **The Zombie State:** `Option<T>` plus status flags recreate partial-initialization bugs. Prefer enums or ownership moves that make invalid combinations unrepresentable.
- **Explicit Intent:** Use consuming transitions (`self`), `#[must_use]`, and scoped capability tokens to enforce use-once and phase-correct behavior.

Saying "the language can't enforce it perfectly" is like saying "my car door doesn't prevent all theft." True - but you still lock it.

## The Test

When reviewing a design, do not ask:

> "What happens if someone passes the wrong data?"

Ask:

> "How do I make passing the wrong data a compile error?"

We do not want to detect invalid state at runtime. We want to define invalid state out of existence.

## Not Acceptable

"Simplicity" objections conflate typing ease with reasoning simplicity.

Clone sprays or mutex wrappers silence compilers easily. Deadlocks, panics, or desyncs in production demand complexity.

The compile-time friction of designing a rigorous typestate, modeling lifetimes, or exhausting an enum costs nothing compared to debugging a state mismatch in production. Fighting the borrow checker means the architecture needs work. Listen to the compiler.

The goal is a codebase where `cargo check` is a high-confidence statement of structural integrity.
