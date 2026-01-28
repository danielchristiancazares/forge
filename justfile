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

# Run tests
test:
    cargo test

# Run clippy lints
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Format code
fmt:
    cargo fmt

# Check formatting without modifying
fmt-check:
    cargo fmt -- --check

# Coverage report (requires cargo-llvm-cov)
cov:
    cargo cov

# Run all checks before committing
verify: fmt-check lint test

# Create source zip for bug analysis (excludes build artifacts)
[windows]
zip:
    Compress-Archive -Path (Get-ChildItem -Path . -Exclude 'target','docs','scripts','*.zip','lcov.info','coverage','sha256.txt' | Where-Object { -not $_.Name.StartsWith('.') }) -DestinationPath forge-source.zip -Force
    Get-FileHash -Algorithm SHA256 forge-source.zip | ForEach-Object { "{0}  {1}" -f $_.Hash, $_.Path } | Set-Content -NoNewline sha256.txt

[unix]
zip:
    zip -r forge-source.zip . -x 'target/*' -x 'docs/*' -x 'scripts/*' -x '.*' -x '.*/*' -x '*.zip' -x 'lcov.info' -x 'coverage/*' -x 'sha256.txt'
    sha256sum forge-source.zip > sha256.txt || shasum -a 256 forge-source.zip > sha256.txt

# Clean build artifacts
clean:
    cargo clean

# Flatten source files for external review (path-prefixed filenames)
[windows]
flatten:
    if (Test-Path gemini-review) { Remove-Item -Recurse -Force gemini-review }
    New-Item -ItemType Directory -Path gemini-review | Out-Null
    Get-ChildItem -Path . -Include *.rs -Recurse | Where-Object { $_.FullName -notmatch '\\target\\' } | ForEach-Object { $newName = ($_.FullName.Substring((Get-Location).Path.Length + 1) -replace '[\\/]', '-'); Copy-Item $_.FullName -Destination "gemini-review/$newName" }
    Get-ChildItem -Path . -Include README.md,CLAUDE.md,DESIGN.md,ARCHITECTURE.md -Recurse | Where-Object { $_.FullName -notmatch '\\target\\' } | ForEach-Object { $newName = ($_.FullName.Substring((Get-Location).Path.Length + 1) -replace '[\\/]', '-'); Copy-Item $_.FullName -Destination "gemini-review/$newName" }
    Write-Host "Flattened $(Get-ChildItem gemini-review | Measure-Object | Select-Object -ExpandProperty Count) files to gemini-review/"

[unix]
flatten:
    rm -rf gemini-review && mkdir -p gemini-review
    find . -name '*.rs' -not -path './target/*' | while read f; do cp "$f" "gemini-review/$(echo "${f#./}" | tr '/' '-')"; done
    find . \( -name 'README.md' -o -name 'CLAUDE.md' -o -name 'DESIGN.md' -o -name 'ARCHITECTURE.md' \) -not -path './target/*' | while read f; do cp "$f" "gemini-review/$(echo "${f#./}" | tr '/' '-')"; done
    echo "Flattened $(ls gemini-review | wc -l) files to gemini-review/"

# Update TOC with current line numbers (uses cached descriptions)
toc file="README.md":
    cargo run --manifest-path scripts/toc-gen/Cargo.toml -- update {{file}}

# Generate descriptions for new sections via LLM
toc-generate file="README.md":
    cargo run --manifest-path scripts/toc-gen/Cargo.toml --features generate -- update {{file}} --generate

# Check if TOC is current (exit 1 if stale)
toc-check file="README.md":
    cargo run --manifest-path scripts/toc-gen/Cargo.toml -- check {{file}}

# Update all known TOC files
toc-all:
    just toc README.md
    just toc context/README.md
