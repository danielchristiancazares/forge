# Rust 1.92.0 and Edition 2024 Reference

This project uses **Rust 1.92.0** with **Edition 2024**. This document covers key differences from earlier Rust versions that may affect code review.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-48 | Lifetime Capture, Temporary Scope Changes in impl Trait and if-let |
| 49-92 | Unsafe Changes: extern blocks, unsafe attributes, static mut, match ergonomics |
| 93-138 | Macro Changes, Reserved Syntax, Prelude Additions, Box IntoIterator |
| 139-157 | Unsafe Environment Functions, Cargo Resolver 3 |

## Edition 2024 Key Changes (from 2021)

### Lifetime Capture in `impl Trait`

Edition 2024 **inverts** the default lifetime capture for return-position `impl Trait`. Now captures all lifetimes by default.

```rust
// Edition 2024 - compiles without explicit bounds
fn numbers(nums: &[i32]) -> impl Iterator<Item=i32> {
    nums.iter().copied()
}

// To explicitly capture nothing (2021 behavior):
fn display(label: &str, ret: impl Sized) -> impl Sized + use<> {
    println!("{label}");
    ret
}
```

### Temporary Scope Changes

**`if let` temporaries**: Now drop at branch end, not after entire if/else.

```rust
// Valid in 2024 - lock drops before else branch
fn get_cached(cache: &RefCell<Option<String>>) -> String {
    if let Some(value) = cache.borrow().as_ref() {
        value.clone()
    } else {
        *cache.borrow_mut() = Some(gen_value()); // No panic - borrow dropped
        cache.borrow().clone().unwrap()
    }
}
```

**Tail expression temporaries**: Drop before local variables.

```rust
// Valid in 2024
fn f() -> usize {
    let c = RefCell::new("..");
    c.borrow().len() // Works - temp drops before c
}
```

### Unsafe Changes

**`extern` blocks require `unsafe`:**
```rust
// Edition 2024 syntax
unsafe extern "C" {
    fn c_function();
}
```

**Unsafe attributes require `unsafe()` wrapper:**
```rust
#[unsafe(no_mangle)]
fn my_function() {}

#[unsafe(export_name = "exported")]
fn another() {}
```

**Static mut references are errors:**
```rust
static mut GLOBAL: i32 = 0;
// let x = &GLOBAL;  // ERROR in 2024 - use raw pointers instead
let x = unsafe { std::ptr::addr_of!(GLOBAL) };
```

### Match Ergonomics Restrictions

Mixing match ergonomics with explicit `ref`/`mut` modifiers is now an error:

```rust
fn f(opt: &mut Option<i32>) {
    // Valid - explicit pattern
    if let &mut Some(ref mut val) = opt { }

    // Valid - pure match ergonomics
    if let Some(val) = opt { }

    // ERROR in 2024 - mixing ergonomics with modifier
    // if let Some(mut val) = opt { }
}
```

### Macro Changes

**`:expr` fragment now matches `const` and `_`:**
```rust
macro_rules! m {
    ($e:expr) => { $e };
}
m!(const { 1 + 1 });  // Valid in 2024
m!(_);                 // Valid in 2024

// Use :expr_2021 for old behavior
```

**Fragment specifiers required:**
```rust
// ERROR in 2024 - missing specifier
macro_rules! broken {
    ($x) => { };  // Must be ($x:expr) or similar
}
```

### Reserved Syntax

- `gen` keyword reserved (use `r#gen` if needed)
- `#"string"#` syntax reserved for future guarded strings
- Multiple `##` without whitespace reserved

### Prelude Additions

`Future` and `IntoFuture` added to prelude:
```rust
// No longer need: use std::future::Future;
async fn example() -> impl Future<Output = i32> {
    async { 42 }
}
```

### `Box<[T]>` IntoIterator

`Box<[T]>` now implements `IntoIterator` yielding owned values:
```rust
let b: Box<[i32]> = vec![1, 2, 3].into_boxed_slice();
for x in b {  // x: i32 (owned), not &i32
    println!("{x}");
}
```

### Unsafe Environment Functions

These are now `unsafe fn`:
- `std::env::set_var`
- `std::env::remove_var`
- `std::os::unix::process::CommandExt::before_exec`

### Cargo Resolver 3

Edition 2024 uses `resolver = "3"` which is MSRV-aware:
```toml
[workspace]
resolver = "3"  # Considers rust-version when resolving deps
```

---

## Rust 1.92.0 Specific Features (December 2025)

### Language

- `#[track_caller]` and `#[no_mangle]` can be combined
- `&raw [mut | const]` allowed for union fields in safe code
- Multiple bounds for same associated item (except trait objects)
- Never type lints (`never_type_fallback_flowing_into_unsafe`) are deny-by-default
- `unused_must_use` doesn't warn on `Result<(), !>`

### Standard Library

New stabilized APIs:
- `NonZero<u{N}>::div_ceil`
- `Location::file_as_c_str`
- `RwLockWriteGuard::downgrade`
- `Box::new_zeroed`, `new_zeroed_slice`
- `Rc::new_zeroed`, `new_zeroed_slice`
- `Arc::new_zeroed`, `new_zeroed_slice`
- `btree_map::Entry::insert_entry`
- Slice rotation methods now const-stable

### Compiler

- Unwind tables emitted by default with `-Cpanic=abort` (backtraces work)
- Minimum external LLVM version: 20
- `iter::Repeat::last` and `count` now panic instead of infinite loop

---

## Project-Specific Notes

This project (`forge`) uses:
- `edition = "2024"`
- `rust-version = "1.92.0"`
- `resolver = "3"` (workspace)

When reviewing code, keep in mind:
1. Lifetime capture in `impl Trait` returns is implicit
2. Temporaries drop earlier in `if let` and tail expressions
3. `extern` blocks should have `unsafe` keyword
4. Match ergonomics cannot mix with explicit `ref`/`mut`

---

## Sources

- [Rust 1.92.0 Release Notes](https://releases.rs/docs/1.92.0/)
- [Rust 2024 Edition Guide](https://doc.rust-lang.org/edition-guide/rust-2024/index.html)
- [Rust 2024 Annotated](https://bertptrs.nl/2025/02/23/rust-edition-2024-annotated.html)
- [Announcing Rust 1.85.0 and Rust 2024](https://blog.rust-lang.org/2025/02/20/Rust-1.85.0/)
