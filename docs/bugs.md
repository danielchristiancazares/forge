# Known Bugs

## Search tool `--files-from` error with path-scoped globs

**Date:** 2025-07-27

**Symptom:** The `Search` tool returns `exit_code: 2` with `stderr: "rg: unrecognized flag --files-from\n"` when using the `glob` parameter with path-scoped patterns like `["types/src/*.rs"]` or `["types/**/*.rs"]`.

**Works:** `glob: ["*.rs"]` with no `path`, or `glob: ["*.rs"]` with `path: "types/src"`.

**Fails:** `glob: ["types/src/*.rs"]` — the tool appears to translate this into a `--files-from` invocation that the installed ripgrep version does not support.

**Workaround:** Use the `path` parameter to scope the directory, and keep `glob` patterns simple (e.g., `["*.rs"]`). Alternatively, use `findstr` or `Glob` + `Read` as fallbacks.

**Environment:** Windows (PowerShell), Forge CLI.
