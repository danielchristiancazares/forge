# Forge development commands

set windows-shell := ["pwsh", "-NoProfile", "-Command"]

# Default recipe: list available commands
default:
    @just --list

# Fast type-check (use during development)
check:
    cargo check

# Debug build
build:
    cargo build -j 24

# Release build
release:
    cargo build --release

test:
   cargo -q test 2>&1

# Run clippy lints (silent on pass, errors only on fail)
lint:
    cargo clippy -q --workspace --all-targets -- -D warnings

# Format code
fmt:
    cargo fmt --all

# Check formatting without modifying
fmt-check:
    cargo fmt -- --check

# Coverage report (requires cargo-llvm-cov)
cov:
    cargo cov

# Audit dependencies (advisories, licenses, bans, sources)
deny:
    cargo deny check

# Run all checks and print a one-line summary
[windows]
verify:
    @pwsh -NoProfile -File scripts/verify-summary.ps1

[unix]
verify:
    @bash scripts/verify-summary.sh

[windows]
ifa-check:
    python scripts/ifa_conformance_check.py

[unix]
ifa-check:
    py=$(command -v python3 >/dev/null 2>&1 && echo python3 || echo python); $py scripts/ifa_conformance_check.py

# Generate CONTEXT.md (file tree, state transitions, cargo metadata, features)
[windows]
context:
    cargo metadata --format-version 1 --no-deps | python scripts/gen_context.py > CONTEXT.md

[unix]
context:
    py=$(command -v python3 >/dev/null 2>&1 && echo python3 || echo python); cargo metadata --format-version 1 --no-deps | $py scripts/gen_context.py > CONTEXT.md

# Create source zip for GPT analysis (updates CONTEXT.md, DIGEST.md, TOC, normalizes LF; excludes build artifacts)
[windows]
zip: context digest fix toc-all
    Compress-Archive -Path (Get-ChildItem -Path . -Exclude 'target','scripts','gemini-review','*.zip','lcov.info','coverage','sha256.txt' | Where-Object { -not $_.Name.StartsWith('.') }) -DestinationPath forge-source.zip -Force
    Get-FileHash -Algorithm SHA256 forge-source.zip | ForEach-Object { "{0}  {1}" -f $_.Hash, $_.Path } | Set-Content -NoNewline sha256.txt

[unix]
zip: context digest fix toc-all
    zip -r forge-source.zip . -x 'target/*' -x 'scripts/*' -x 'gemini-review/*' -x '.*' -x '.*/*' -x '*.zip' -x 'lcov.info' -x 'coverage/*' -x 'sha256.txt'
    sha256sum forge-source.zip > sha256.txt || shasum -a 256 forge-source.zip > sha256.txt

# Install forge binary to ~/.cargo/bin
install:
    cargo install --path cli

# Clean build artifacts
clean:
    cargo clean

# Update TOC with current line numbers (uses cached descriptions)
toc file="README.md":
    cargo run --manifest-path scripts/toc-gen/Cargo.toml -- update "{{file}}"

# Generate descriptions for new sections via LLM
toc-generate file="README.md":
    cargo run --manifest-path scripts/toc-gen/Cargo.toml --features generate -- update "{{file}}" --generate

# Check if TOC is current (exit 1 if stale)
toc-check file="README.md":
    cargo run --manifest-path scripts/toc-gen/Cargo.toml -- check "{{file}}"

# Generate API digest from rustdoc JSON (requires nightly)
[windows]
digest:
    cargo +nightly rustdoc -p forge-types -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-utils -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-config -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-providers -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-context -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-core -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-engine -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tui -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tools -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-lsp -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge -- -Z unstable-options --output-format json
    python scripts/rustdoc_digest.py target/doc/forge_types.json target/doc/forge_utils.json target/doc/forge_config.json target/doc/forge_providers.json target/doc/forge_context.json target/doc/forge_core.json target/doc/forge_engine.json target/doc/forge_tui.json target/doc/forge_tools.json target/doc/forge_lsp.json target/doc/forge.json > DIGEST.md

[unix]
digest:
    cargo +nightly rustdoc -p forge-types -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-utils -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-config -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-providers -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-context -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-core -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-engine -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tui -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tools -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-lsp -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge -- -Z unstable-options --output-format json
    py=$(command -v python3 >/dev/null 2>&1 && echo python3 || echo python); $py scripts/rustdoc_digest.py target/doc/forge_types.json target/doc/forge_utils.json target/doc/forge_config.json target/doc/forge_providers.json target/doc/forge_context.json target/doc/forge_core.json target/doc/forge_engine.json target/doc/forge_tui.json target/doc/forge_tools.json target/doc/forge_lsp.json target/doc/forge.json > DIGEST.md

# Update all known TOC files
toc-all:
    just toc README.md
    just toc context/README.md

# Normalize line endings to LF for source and doc files
[windows]
fix:
  cargo clippy -q --fix --workspace --all-targets --allow-dirty --allow-staged -- -W clippy::collapsible_if -W clippy::redundant_closure -W clippy::redundant_closure_for_method_calls -W clippy::needless_return -W clippy::let_and_return -W clippy::needless_borrow -W clippy::needless_borrows_for_generic_args -W clippy::clone_on_copy -W clippy::unnecessary_cast -W clippy::needless_bool -W clippy::needless_bool_assign -W unused_imports -W unused_mut -W unused_parens
  [IO.Directory]::EnumerateFiles($PWD, "*", 1) | Where-Object { $_ -match '\.(rs|md)$' -and $_ -notmatch '\\(target|gemini-review|\.git)\\' } | ForEach-Object { $b = [IO.File]::ReadAllBytes($_); if ($b -contains 13) { [IO.File]::WriteAllText($_, ([Text.Encoding]::UTF8.GetString($b) -replace "\r", "")) } }

[unix]
fix:
    cargo clippy --fix --workspace --all-targets --allow-dirty --allow-staged -- -W 'clippy::collapsible_if' -W 'clippy::redundant_closure' -W 'clippy::redundant_closure_for_method_calls' -W 'clippy::needless_return' -W 'clippy::let_and_return' -W 'clippy::needless_borrow' -W 'clippy::needless_borrows_for_generic_args' -W 'clippy::clone_on_copy' -W 'clippy::unnecessary_cast' -W 'clippy::needless_bool' -W 'clippy::needless_bool_assign' -W 'unused_imports' -W 'unused_mut' -W 'unused_parens'
