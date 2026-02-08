# LP2 Proposal: Insert After Balanced Block (`A` command)

## Problem

`I` (insert after) matches literal line sequences. It has no notion of "the block that starts with this line." When an LLM writes:

```
I
fn distill_git_status(content: &str) -> Option<String> {
.
fn new_function() { ... }
.
```

The intent is to insert a new function *after* `distill_git_status`. The actual result is insertion after the single matched line — the opening brace — placing the new content *inside* the function body.

`P` (insert before) on the same anchor would work for "insert before this function," but the inverse — "insert after this function" — has no ergonomic expression. The alternatives are:

1. Match the closing `}` — fragile, ambiguous (many `}` lines exist), requires knowing the exact last line
2. Match the *next* function's signature and use `P` — requires knowledge of what follows, which may be unstable or distant
3. Match the entire function body in the find-block — verbose and defeats the purpose of a terse anchor

This is the single most common LP1 misuse pattern: anchoring on a block-opening line with `I` and expecting structural awareness that doesn't exist.

## Proposal

Add an `A` command (Insert After Balanced Block) that performs brace-balanced scanning from the anchor to find the structural end of the block.

### Syntax

```
A [occ]
<find-block>
.
<insert-block>
.
```

Two dot-terminated blocks, identical structure to `I` and `P`.

### Semantics

1. **Match** the find-block exactly as `I` does (same uniqueness rules, same `occ` selector)
2. **Scan forward** from the line after the last matched line, tracking delimiter depth (`{`/`}`, `[`/`]`, `(`/`)`)
3. **Initial depth** is the count of unbalanced openers in the matched lines themselves (typically 1 for a line ending in `{`)
4. **Terminate** when depth returns to 0 — the line where this occurs is the structural end of the block
5. **Insert** the insert-block immediately after the terminating line
6. **Error** if EOF is reached before depth returns to 0

### Delimiter Counting

Counting is naive — character-level scan of each line, no string/comment awareness:

- `{`, `[`, `(` increment depth
- `}`, `]`, `)` decrement depth

This is a structural heuristic, not a parser. For the vast majority of real code (Rust, C, C++, TypeScript, Go, Java, JSON, Python dicts/lists) naive counting is correct. Pathological cases (unbalanced delimiters inside string literals or comments) can fall back to `I` or `P` with explicit anchors.

### Example

Source file:

```rust
fn existing() {
    if true {
        do_thing();
    }
}

fn next() {
```

Patch:

```
LP1
F src/lib.rs
A
fn existing() {
.
fn inserted() {
    // new
}

.
END
```

Execution:

1. Match `fn existing() {` — depth starts at 1 (one `{` in matched line)
2. Scan: `    if true {` → depth 2
3. Scan: `        do_thing();` → depth 2
4. Scan: `    }` → depth 1
5. Scan: `}` → depth 0 — **terminate here**
6. Insert the new block after the `}` line

Result:

```rust
fn existing() {
    if true {
        do_thing();
    }
}
fn inserted() {
    // new
}

fn next() {
```

### Grammar

Addition to the LP1 EBNF:

```ebnf
op := replace | insert_after | insert_before | erase
    | append | prepend | set_final_nl
    | insert_after_block ;

insert_after_block := OWS "A" [WS occ] OWS NL block block ;
```

### Implementation

New `Op` variant:

```rust
InsertAfterBlock {
    occ: Option<usize>,
    find: Vec<String>,
    insert: Vec<String>,
}
```

Apply logic:

```rust
Op::InsertAfterBlock { occ, find, insert } => {
    let idx = find_match(&content.lines, find, *occ)?;
    let scan_start = idx + find.len();

    // Count openers in matched lines to get initial depth
    let initial_depth: i32 = find.iter()
        .flat_map(|line| line.chars())
        .map(|c| match c {
            '{' | '[' | '(' => 1,
            '}' | ']' | ')' => -1,
            _ => 0,
        })
        .sum();

    let mut depth = initial_depth;
    let mut end = scan_start;

    while end < content.lines.len() {
        for c in content.lines[end].chars() {
            match c {
                '{' | '[' | '(' => depth += 1,
                '}' | ']' | ')' => depth -= 1,
                _ => {}
            }
        }
        end += 1;
        if depth <= 0 {
            break;
        }
    }

    if depth > 0 {
        return Err(PatchError::UnbalancedBlock { line: scan_start });
    }

    let insert_at = end;
    for (i, line) in insert.iter().enumerate() {
        content.lines.insert(insert_at + i, line.clone());
    }
}
```

### Relationship to Existing Commands

| Command | Anchor semantics | Use case |
|---------|-----------------|----------|
| `I` | Insert after literal matched lines | Known exact insertion point |
| `P` | Insert before literal matched lines | Known exact insertion point |
| `A` | Insert after balanced block starting at match | Insert after a function, struct, impl, block |

`I` and `P` remain unchanged. `A` is additive — a structural convenience for the case where the anchor identifies the *start* of a block and the intent is to place content after its *end*.

### Error Cases

| Condition | Behavior |
|-----------|----------|
| Find-block not found | Same as `I`: match error |
| Find-block ambiguous (no `occ`) | Same as `I`: uniqueness error |
| EOF before depth returns to 0 | New error: `UnbalancedBlock` |
| Matched lines contain no openers (depth starts at 0) | Degrades to `I` behavior (inserts immediately after match) |

### Scope

This proposal adds one command to LP1. It does not change any existing command semantics, the newline model, dot-stuffing, or the patch structure. A patch using only `R`/`I`/`P`/`E`/`T`/`B`/`N` commands is valid LP1 and valid under this extension. The `A` command is the only addition.

The version identifier remains `LP1` — this is a backward-compatible extension, not a new format version.
