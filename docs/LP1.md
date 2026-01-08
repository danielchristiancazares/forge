# LP1 (Line Patch v1)

LP1 is a line-oriented patch DSL for applying structured edits across one or more files. It uses dot-terminated blocks with dot-stuffing, and applies operations sequentially within each file section.

## Patch structure

- Header: `LP1` on its own line (leading/trailing whitespace allowed).
- Footer: `END` on its own line (leading/trailing whitespace allowed).
- Comments: lines starting with `#` (after optional whitespace).
- Blank lines are allowed.
- A file section begins with `F <path>` and is followed by one or more operations.

## Commands

Within a file section (`F <path>`):

| Cmd | Meaning |
| --- | --- |
| `R [occ]` | Replace a matched block with a new block |
| `I [occ]` | Insert new block after a matched block |
| `P [occ]` | Insert new block before a matched block |
| `E [occ]` | Delete a matched block |
| `T` | Append a block to end of file |
| `B` | Prepend a block to beginning of file |
| `N +` | Ensure file ends with a newline boundary |
| `N -` | Ensure file does not end with a newline boundary |

`occ` is an optional 1-based occurrence selector.

## Blocks and dot-stuffing

Blocks are dot-terminated and use dot-stuffing to allow lines beginning with `.`:

- Terminator line: a single `.` at column 1, with optional trailing whitespace (regex: `/^\.[ \t]*$/`).
- Any content line whose first character is `.` MUST be encoded starting with `..`.
- The decoder removes exactly one leading `.` from lines that start with `..`.
- Terminator detection occurs before unstuffing.

## Normative semantics

- Operations apply sequentially within a file section to the evolving file content.
- Matching unit is whole lines (line sequences), not bytes. Matching is exact on line text after normalizing line endings per the newline model below.
- For `R`/`I`/`P`/`E`, if `occ` is omitted, the match MUST be unique (exactly one). If zero or more than one match is found, the operation errors.
- `occ` is 1-based in increasing start-position order (leftmost-first).
- `R` replaces the matched line-sequence with the new block.
- `I` inserts the new block immediately after the matched line-sequence.
- `P` inserts the new block immediately before the matched line-sequence.
- `E` deletes the matched line-sequence.
- `T` appends the block to the end of file; `B` prepends the block to the beginning of file.
- Behavior of `T`/`B` when the file does not exist is implementation-defined (backend policy).
- `N` operations apply sequentially; if multiple `N` ops appear, the last one wins (subject to empty-file rules below).

## Normative newline model (LP1)

### 1) Newline boundaries

A newline boundary is either:

- `LF = "\n"`
- `CRLF = "\r\n"`

Bare `\r` is invalid input.

### 2) Greedy scan + mixed-EOL detection (normative)

Parse the file bytes left-to-right:

- When you see `\r`:
  - If the next byte is `\n`, consume both as a CRLF boundary.
  - Otherwise error (bare `\r` invalid).
- When you see `\n` not consumed as part of `\r\n`, consume it as an LF boundary.

Mixed EOL check:

- Let `eol_kind` start as `unset`.
- On the first boundary, set `eol_kind` to that boundary's kind.
- On any later boundary, if its kind differs, error (mixed EOL).

If no boundaries exist, set `eol_kind = LF` by policy (used only if edits introduce boundaries).

### 3) Canonical in-memory representation

Represent a file as:

- `lines: Vec<String>` — each line's text excluding newline bytes
- `final_newline: bool`
- `eol_kind: LF | CRLF`

Canonical empty file:

- Empty bytes MUST be represented as `lines = []`, `final_newline = false`.

Example mapping:

- `""` -> `lines=[]`, `final_newline=false`
- `"a"` -> `["a"]`, `false`
- `"a\n"` -> `["a"]`, `true`
- `"a\nb"` -> `["a","b"]`, `false`
- `"a\nb\n"` -> `["a","b"]`, `true`
- `"\n"` -> `[""]`, `true` (newline-only file)

### 4) Emission (round-trip preserving)

To emit bytes after edits:

- Join `lines` with `eol_kind` between adjacent lines.
- If `final_newline` is true, append one `eol_kind`.

Invariant enforced by ops:

- If `lines.is_empty()`, then `final_newline` must be false.

### 5) `N` operation (trailing newline control)

`N` primarily toggles `final_newline`, with a canonical empty-file special case:

`N +`:

- If `lines.is_empty()`:
  - set `lines = [""]`
  - set `final_newline = true`
  - if `eol_kind` is unset, set it to `LF` (policy default)
- Else:
  - set `final_newline = true`

`N -`:

- If `lines == [""]` and `final_newline == true`:
  - set `lines = []`
  - set `final_newline = false`
- Else:
  - set `final_newline = false`

Preservation:

- `final_newline` is preserved from the preimage unless an `N` op is applied.

## Grammar (EBNF)

```ebnf
patch            := header { stmt } { file_section } footer ;
header           := OWS "LP1" OWS NL ;
footer           := OWS "END" OWS [NL] ;
stmt             := comment | blank ;
file_section     := file_select { file_stmt } ;
file_select      := OWS "F" WS path OWS NL ;
file_stmt        := comment | blank | op ;
op               := replace | insert_after | insert_before | erase | append | prepend | set_final_nl ;
replace          := OWS "R" [WS occ] OWS NL block block ;
insert_after     := OWS "I" [WS occ] OWS NL block block ;
insert_before    := OWS "P" [WS occ] OWS NL block block ;
erase            := OWS "E" [WS occ] OWS NL block ;
append           := OWS "T" OWS NL block ;
prepend          := OWS "B" OWS NL block ;
set_final_nl      := OWS "N" WS nlflag OWS NL ;
nlflag           := "+" | "-" ;
(* Blocks are dot-terminated with dot-stuffing:
   - Terminator line is a single '.' at column 1, with optional trailing whitespace:  /^\.[ \t]*$/
   - Any content line whose first character is '.' MUST be encoded starting with '..'
     (decoder removes exactly one leading '.' from lines beginning with '..'). *)
block            := { block_row } block_end_row ;
block_end_row    := "." OWS NL ;                     (* '.' at column 1 terminates the block *)
block_row        := stuffed_row | nondot_row | empty_row ;
empty_row        := NL ;                             (* truly empty line *)
stuffed_row      := "." "." { notNL } NL ;           (* raw begins with "..", rest may be empty *)
nondot_row       := nondot_first { notNL } NL ;      (* raw begins with any non-dot char *)
nondot_first     := ? any char except ".", "\n", "\r" ? ;
comment          := OWS "#" { notNL } NL ;
blank            := OWS NL ;
path             := bare_path | quoted_path ;
bare_path        := path_char { path_char } ;        (* 1+ *)
path_char        := ? any char except whitespace, "\n", "\r", and '"' ? ;
quoted_path      := '"' { qchar } '"' ;
qchar            := q_esc | q_raw ;
q_esc            := '\\' ('"' | '\\') ;
q_raw            := ? any char except '"', '\\', "\n", "\r" ? ;
occ              := pos_int ;
pos_int          := nonzero_digit { digit } ;
WS               := (" " | "\t") { " " | "\t" } ;
OWS              := { " " | "\t" } ;
NL               := "\n" | "\r\n" ;
digit            := "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
nonzero_digit    := "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" ;
notNL            := ? any char except "\n" and "\r" ? ;
```
