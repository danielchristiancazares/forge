When using the `Edit` tool, emit patches in LP1 format. LP1 is a line-oriented patch DSL.

### **Structure:**
- Header: `LP1` on its own line
- File section: `F <path>` followed by operations
- Footer: `END` on its own line
- Blocks are dot-terminated; lines starting with `.` must be escaped as `..`

### **Operations:**

| Cmd | Args | Description |
|-----|------|-------------|
| `R [occ]` | find-block, replace-block | Replace matched lines |
| `I [occ]` | find-block, insert-block | Insert after matched lines |
| `P [occ]` | find-block, insert-block | Insert before matched lines |
| `E [occ]` | find-block | Erase matched lines |
| `T` | block | Append to end of file |
| `B` | block | Prepend to start of file |
| `N +` | (none) | Ensure file ends with newline |
| `N -` | (none) | Ensure file does not end with newline |

`occ` is an optional 1-based occurrence selector. If omitted, the match must be unique.

### **Semantics:**
- Matching is exact on contiguous whole lines; whitespace is significant; no regex/substring matching.
- `R`/`I`/`P` take exactly two dot-terminated blocks; `E`/`T`/`B` take exactly one; `N +/-` takes no blocks.
- A block ends only at a line that is exactly `.`; `END` only ends the patch and is ordinary text inside blocks.
- If `occ` is omitted, the find-block must match exactly once; otherwise provide `occ` or make the find-block more specific.
- `occ` is 1-based in increasing start-position order (leftmost-first).
- `I` inserts the new block immediately after the matched line-sequence. `P` inserts immediately before the matched line-sequence.
 - `I` does not insert after any structural unit (function, block, struct) that line belongs to. If the find-block matches a function signature ending in {, insertion happens inside the function body, not after it. `P` is treated simiarly for prepends.
- Operations apply sequentially within a file section (later ops see earlier edits).
- Outside blocks, only `F`, operation headers, comments/blank lines, and the final `END` are valid; never emit raw file content at top level.
- Dot-stuffing: inside blocks, any content line whose first character is `.` must be written with an extra leading dot (`..`); decoding removes exactly one dot.

**Examples:**

Replace a single line:
```
LP1
F src/config.rs
R
const MAX_SIZE: usize = 100;
.
const MAX_SIZE: usize = 200;
.
END
```

Insert a line after an existing line:
```
LP1
F src/main.rs
I
use std::io;
.
use std::fs;
.
END
```

Delete a function (multi-line match):
```
LP1
F src/utils.rs
E
fn deprecated_helper() {
    // old code
}
.
END
```

Replace second occurrence:
```
LP1
F src/lib.rs
R 2
    println!("debug");
.
    // debug removed
.
END
```

Append to file:
```
LP1
F README.md
T
## License
MIT
.
END
```

Dot-stuffing (`..env` decodes to `.env`):
```
LP1
F .gitignore
R
..env
.
..env.local
..env.production
.
END
```

Multiple operations in one file:
```
LP1
F src/lib.rs
R
use old_crate;
.
use new_crate;
.
I
fn existing() {
.
fn new_helper() {
    // added
}
.
END
```

Multiple files:
```
LP1
F src/a.rs
R
old_a
.
new_a
.
F src/b.rs
R
old_b
.
new_b
.
END
```
