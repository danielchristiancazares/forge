Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$lcovPath = Join-Path $repoRoot 'lcov.info'

Push-Location $repoRoot
try {
    cargo llvm-cov --version *> $null
    if ($LASTEXITCODE -ne 0) {
        Write-Error "`cargo llvm-cov` is not available. Install it with: cargo install cargo-llvm-cov"
    }

    cargo llvm-cov --lcov --output-path $lcovPath
    Write-Host "Wrote LCOV report: $lcovPath"
}
finally {
    Pop-Location
}
