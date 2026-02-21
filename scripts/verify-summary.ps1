#!/usr/bin/env pwsh
# verify-summary.ps1 — runs all verification steps and prints a one-line summary
# Output: "ifa ok, fix ok, fmt ok, lint ok, test ok, advisories ok, bans ok, licenses ok, sources ok"

param([switch]$Verbose)

$ErrorActionPreference = 'Continue'

function Run-Step {
    param([string]$Name, [scriptblock]$Command)
    if ($Verbose) { Write-Host "  running $Name..." -ForegroundColor DarkGray }
    try {
        $output = & $Command 2>&1
        if ($LASTEXITCODE -ne 0) {
            return @{ Name = $Name; Ok = $false; Output = ($output | Out-String) }
        }
        return @{ Name = $Name; Ok = $true; Output = ($output | Out-String) }
    } catch {
        return @{ Name = $Name; Ok = $false; Output = $_.Exception.Message }
    }
}

$results = @()
$bail = $false

# Sequential: ifa → fix → fmt → lint → test (ordering matters — fix mutates source)
$results += Run-Step 'ifa' { python scripts/ifa_conformance_check.py }

$results += Run-Step 'fix' {
    cargo clippy -q --fix --workspace --all-targets --allow-dirty --allow-staged `
        -- -W clippy::collapsible_if -W clippy::redundant_closure `
        -W clippy::redundant_closure_for_method_calls -W clippy::needless_return `
        -W clippy::let_and_return -W clippy::needless_borrow `
        -W clippy::needless_borrows_for_generic_args -W clippy::clone_on_copy `
        -W clippy::unnecessary_cast -W clippy::needless_bool `
        -W clippy::needless_bool_assign -W unused_imports -W unused_mut -W unused_parens
    if ($LASTEXITCODE -ne 0) { return }
    [IO.Directory]::EnumerateFiles($PWD, "*", 1) |
        Where-Object { $_ -match '\.(rs|md)$' -and $_ -notmatch '\\(target|gemini-review|\.git)\\' } |
        ForEach-Object {
            $b = [IO.File]::ReadAllBytes($_)
            if ($b -contains 13) {
                [IO.File]::WriteAllText($_, ([Text.Encoding]::UTF8.GetString($b) -replace "`r", ""))
            }
        }
}

$results += Run-Step 'fmt' {
    cargo fmt --all
    if ($LASTEXITCODE -ne 0) { return }
    cargo fmt -- --check
}

$results += Run-Step 'lint' { cargo clippy -q --workspace --all-targets -- -D warnings }

$results += Run-Step 'test' { cargo -q test 2>&1 }

# Parallel: cargo deny sub-checks are read-only and independent
$denyResults = @('advisories', 'bans', 'licenses', 'sources') | ForEach-Object -Parallel {
    $name = $_
    try {
        $output = cargo deny check $name 2>&1
        @{ Name = $name; Ok = ($LASTEXITCODE -eq 0); Output = ($output | Out-String) }
    } catch {
        @{ Name = $name; Ok = $false; Output = $_.Exception.Message }
    }
} -ThrottleLimit 4

$results += $denyResults

# Summary
$parts = $results | ForEach-Object { "$($_.Name) $(if ($_.Ok) { 'ok' } else { 'FAIL' })" }
$failed = $results | Where-Object { -not $_.Ok }
$summary = $parts -join ', '

if (-not $failed -or $failed.Count -eq 0) {
    Write-Host $summary -ForegroundColor Green
} else {
    Write-Host $summary -ForegroundColor Red
    $failed | ForEach-Object {
        Write-Host "`n--- $($_.Name) ---" -ForegroundColor Yellow
        Write-Host $_.Output
    }
    exit 1
}
