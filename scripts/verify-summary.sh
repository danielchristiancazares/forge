#!/usr/bin/env bash
# verify-summary.sh — runs all verification steps and prints a one-line summary
# like cargo deny: "ifa ok, fmt ok, lint ok, test ok, advisories ok, bans ok, licenses ok, sources ok"

set -uo pipefail

VERBOSE="${VERBOSE:-}"
PARTS=()
FAILURES=()

run_step() {
    local name="$1"
    shift
    [[ -n "$VERBOSE" ]] && printf '  running %s...\n' "$name" >&2
    local output
    if output=$("$@" 2>&1); then
        PARTS+=("$name ok")
    else
        PARTS+=("$name FAIL")
        FAILURES+=("--- $name ---"$'\n'"$output")
    fi
}

py=$(command -v python3 >/dev/null 2>&1 && echo python3 || echo python)

# 1. IFA conformance
run_step ifa $py scripts/ifa_conformance_check.py

# 2. Auto-fix (clippy --fix + CRLF normalization via just fix)
run_step fix cargo clippy --fix --workspace --all-targets --allow-dirty --allow-staged \
    -- -W 'clippy::collapsible_if' -W 'clippy::redundant_closure' \
    -W 'clippy::redundant_closure_for_method_calls' -W 'clippy::needless_return' \
    -W 'clippy::let_and_return' -W 'clippy::needless_borrow' \
    -W 'clippy::needless_borrows_for_generic_args' -W 'clippy::clone_on_copy' \
    -W 'clippy::unnecessary_cast' -W 'clippy::needless_bool' \
    -W 'clippy::needless_bool_assign' -W 'unused_imports' -W 'unused_mut' -W 'unused_parens'

# 3. Format
run_step fmt bash -c 'cargo fmt --all && cargo fmt -- --check'

# 4. Lint
run_step lint cargo clippy -q --workspace --all-targets -- -D warnings

# 5. Test
run_step test cargo -q test

# 6. Deny (split for granular output)
for check in advisories bans licenses sources; do
    run_step "$check" cargo deny check "$check"
done

# Build summary
summary=$(IFS=', '; echo "${PARTS[*]}")

if [[ ${#FAILURES[@]} -eq 0 ]]; then
    printf '\033[32m%s\033[0m\n' "$summary"
else
    printf '\033[31m%s\033[0m\n' "$summary"
    echo ""
    for f in "${FAILURES[@]}"; do
        printf '\033[33m%s\033[0m\n' "$f"
    done
    exit 1
fi
