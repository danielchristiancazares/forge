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
    cargo build

# Release build
release:
    cargo build --release

# Run tests (concise: one-line summary on pass, full output on fail)
[windows]
test:
    @& { $r = cargo test 2>&1; if ($LASTEXITCODE -eq 0) { $p=0; $f=0; $i=0; $r | Select-String 'test result:' | ForEach-Object { if ($_ -match '(\d+) passed; (\d+) failed; (\d+) ignored') { $p+=[int]$Matches[1]; $f+=[int]$Matches[2]; $i+=[int]$Matches[3] } }; "ok: $p passed, $f failed, $i ignored" } else { $r | ForEach-Object { "$_" }; exit 1 } }

[unix]
test:
    @r=$(cargo test 2>&1); rc=$?; if [ $rc -eq 0 ]; then echo "$r" | grep 'test result:' | awk '{p+=$4; f+=$6; i+=$8} END {printf "ok: %d passed, %d failed, %d ignored\n", p, f, i}'; else echo "$r"; exit $rc; fi

# Run clippy lints (silent on pass, errors only on fail)
[windows]
lint:
    @& { $r = cargo clippy --workspace --all-targets -- -D warnings 2>&1; if ($LASTEXITCODE -ne 0) { $r | Where-Object { $_ -notmatch '^\s*(Checking|Compiling|Finished|Downloading|Downloaded|warning: build failed)' } | ForEach-Object { "$_" }; exit 1 } }

[unix]
lint:
    @r=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1); rc=$?; if [ $rc -ne 0 ]; then echo "$r" | grep -Ev '^\s*(Checking|Compiling|Finished|Downloading|Downloaded|warning: build failed)'; exit $rc; fi

# Format code
fmt:
    @cargo fmt --all

# Check formatting without modifying
fmt-check:
    @cargo fmt -- --check

# Coverage report (requires cargo-llvm-cov)
cov:
    cargo cov

# Run all checks before committing (includes auto-formatting)
verify: fmt fmt-check lint test

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

# Flatten source files for external review (path-prefixed filenames)
[windows]
flatten:
    if (Test-Path gemini-review) { Remove-Item -Recurse -Force gemini-review }
    New-Item -ItemType Directory -Path gemini-review | Out-Null
    Get-ChildItem -Path . -Include *.rs -Recurse | Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' -and $_.FullName -notmatch '[\\/]gemini-review[\\/]' } | ForEach-Object { $newName = ($_.FullName.Substring((Get-Location).Path.Length + 1) -replace '[\\/]', '-'); Copy-Item $_.FullName -Destination "gemini-review/$newName" }
    Get-ChildItem -Path . -Include README.md,CLAUDE.md,DESIGN.md,ARCHITECTURE.md -Recurse | Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' -and $_.FullName -notmatch '[\\/]gemini-review[\\/]' } | ForEach-Object { $newName = ($_.FullName.Substring((Get-Location).Path.Length + 1) -replace '[\\/]', '-'); Copy-Item $_.FullName -Destination "gemini-review/$newName" }
    Write-Host "Flattened $(Get-ChildItem gemini-review | Measure-Object | Select-Object -ExpandProperty Count) files to gemini-review/"

[unix]
flatten:
    rm -rf gemini-review && mkdir -p gemini-review
    find . -name '*.rs' -not -path './target/*' -not -path './gemini-review/*' | while IFS= read -r f; do cp "$f" "gemini-review/$(echo "${f#./}" | tr '/' '-')"; done
    find . \( -name 'README.md' -o -name 'CLAUDE.md' -o -name 'DESIGN.md' -o -name 'ARCHITECTURE.md' \) -not -path './target/*' -not -path './gemini-review/*' | while IFS= read -r f; do cp "$f" "gemini-review/$(echo "${f#./}" | tr '/' '-')"; done
    echo "Flattened $(ls gemini-review | wc -l) files to gemini-review/"

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
    cargo +nightly rustdoc -p forge-providers -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-context -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-engine -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tui -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tools -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-lsp -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge -- -Z unstable-options --output-format json
    python scripts/rustdoc_digest.py target/doc/forge_types.json target/doc/forge_providers.json target/doc/forge_context.json target/doc/forge_engine.json target/doc/forge_tui.json target/doc/forge_tools.json target/doc/forge_lsp.json target/doc/forge.json > DIGEST.md

[unix]
digest:
    cargo +nightly rustdoc -p forge-types -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-providers -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-context -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-engine -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tui -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-tools -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge-lsp -- -Z unstable-options --output-format json
    cargo +nightly rustdoc -p forge -- -Z unstable-options --output-format json
    py=$(command -v python3 >/dev/null 2>&1 && echo python3 || echo python); $py scripts/rustdoc_digest.py target/doc/forge_types.json target/doc/forge_providers.json target/doc/forge_context.json target/doc/forge_engine.json target/doc/forge_tui.json target/doc/forge_tools.json target/doc/forge_lsp.json target/doc/forge.json > DIGEST.md

# Update all known TOC files
toc-all:
    just toc README.md
    just toc context/README.md

# Normalize line endings to LF for source and doc files
[windows]
fix:
    Get-ChildItem -Path . -Include *.rs,*.md -Recurse | Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' -and $_.FullName -notmatch '[\\/]gemini-review[\\/]' } | ForEach-Object { $c = [System.IO.File]::ReadAllText($_.FullName); if ($c -match "`r`n") { [System.IO.File]::WriteAllText($_.FullName, ($c -replace "`r`n", "`n")); Write-Host "fixed: $($_.FullName)" } }

[unix]
fix:
    find . \( -name '*.rs' -o -name '*.md' \) -not -path './target/*' -not -path './gemini-review/*' -exec perl -pi -e 's/\r$//' {} +
