<#
.SYNOPSIS
    Minifies Rust compiler error output for LLM consumption by deduplicating
    repeated error signatures and stripping verbose help/suggestion blocks.

.DESCRIPTION
    Rust compiler output often repeats the same diagnostic details at many call
    sites. This script groups diagnostics by (error_code, message, target_method)
    and emits one canonical entry with all affected locations.

    It supports both coded errors (`error[E####]: ...`) and non-coded/lint-style
    errors (`error: ...`). It is also ANSI-aware and strips terminal color escape
    sequences before parsing.

.PARAMETER Path
    Path to a file containing cargo build output. If omitted, reads from stdin.

.PARAMETER KeepHelp
    If set, preserves `help:` / `= help:` suggestion blocks.

.PARAMETER KeepNotes
    If set, preserves all `note:` / `= note:` lines in each canonical block.
    By default, at most one note section is emitted per group.

.EXAMPLE
    cargo build 2>&1 | pwsh -File scripts/minify-rust-errors.ps1

.EXAMPLE
    cargo clippy -- -D warnings 2>&1 | pwsh -File scripts/minify-rust-errors.ps1 -KeepHelp

.EXAMPLE
    pwsh -File scripts/minify-rust-errors.ps1 -Path build-errors.txt
#>
param(
    [string]$Path,
    [switch]$KeepHelp,
    [switch]$KeepNotes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$AnsiRegex = '\x1B\[[0-?]*[ -/]*[@-~]'

function Strip-Ansi {
    param([string]$Text)

    if ($null -eq $Text) {
        return ''
    }

    return ($Text -replace $AnsiRegex, '')
}

function Is-ErrorHeader {
    param([string]$Line)

    if (-not ($Line -match '^\s*error(?:\[(?<code>[A-Z]\d+)\])?:\s*(?<msg>.*)$')) {
        return $false
    }

    $msg = $Matches.msg.Trim()

    # Ignore cargo/clippy summary lines that are not diagnostic entries.
    if ($msg -match '^(could not compile|aborting due to|for more information about this error|recipe\s+`?.+`?\s+failed|build failed)\b') {
        return $false
    }

    return $true
}

function Is-NonErrorDiagnosticHeader {
    param([string]$Line)

    if ([string]::IsNullOrWhiteSpace($Line)) {
        return $false
    }

    # Only split on top-level diagnostics, not indented in-block detail lines.
    if ($Line -notmatch '^\S') {
        return $false
    }

    if ($Line -match '^(warning|note|help)(?:\[[^\]]+\])?:\s+') {
        return $true
    }

    if ($Line -match '^error(?:\[[A-Z]\d+\])?:\s+') {
        return $true
    }

    if ($Line -match '^(could not compile|aborting due to|for more information about this error|recipe\s+`?.+`?\s+failed|build failed)\b') {
        return $true
    }

    return $false
}

function Get-ErrorInfo {
    param([object[]]$Block)

    $header = $Block[0].Clean
    $null = $header -match '^\s*error(?:\[(?<code>[A-Z]\d+)\])?:\s*(?<msg>.*)$'

    $errorCode = if ($Matches.ContainsKey('code') -and $Matches.code) {
        "error[$($Matches.code)]"
    } else {
        'error'
    }

    $message = $Matches.msg.Trim()

    $location = ''
    foreach ($lineInfo in $Block) {
        if ($lineInfo.Clean -match '^\s*-->\s*(.+)$') {
            $location = $Matches[1].Trim()
            break
        }
    }

    $targetMethod = ''
    for ($i = 0; $i -lt $Block.Count; $i++) {
        if ($Block[$i].Clean -match '^\s*(?:=\s*)?note:\s*(?:method|function|associated function) defined here\b') {
            for ($j = $i + 1; $j -lt $Block.Count -and $j -le $i + 12; $j++) {
                $candidate = $Block[$j].Clean
                if ($candidate -match '^\s*(?:\d+\s*)?\|\s*(?<sig>.*\bfn\s+[A-Za-z_]\w*.*)$') {
                    $targetMethod = (($Matches.sig -replace '\s+', ' ').Trim())
                    break
                }
            }
            break
        }
    }

    $key = "${errorCode}|${message}|${targetMethod}"

    return [pscustomobject]@{
        Key          = $key
        ErrorCode    = $errorCode
        Message      = $message
        Location     = $location
        TargetMethod = $targetMethod
        Block        = $Block
    }
}

# --- Read input ---
if ($Path) {
    if (-not (Test-Path $Path)) {
        Write-Error "File not found: $Path"
        exit 1
    }
    $rawLines = Get-Content $Path -Encoding UTF8
} else {
    $rawLines = @($input)
    if ($rawLines.Count -eq 0) {
        Write-Error 'No input. Pipe compiler output or use -Path.'
        exit 1
    }
}

# Normalize lines and strip known host noise.
$lineInfos = [System.Collections.Generic.List[object]]::new()
foreach ($raw in $rawLines) {
    $rawString = [string]$raw
    $clean = Strip-Ansi $rawString
    if ($clean -eq 'System.Management.Automation.RemoteException') {
        continue
    }
    $lineInfos.Add([pscustomobject]@{
        Raw   = $rawString
        Clean = $clean
    })
}

# --- Parse into discrete error blocks ---
$blocks = [System.Collections.Generic.List[object[]]]::new()
$current = $null

foreach ($lineInfo in $lineInfos) {
    if (Is-ErrorHeader $lineInfo.Clean) {
        if ($null -ne $current -and $current.Count -gt 0) {
            $blocks.Add($current.ToArray())
        }
        $current = [System.Collections.Generic.List[object]]::new()
        $current.Add($lineInfo)
        continue
    }

    if ($null -ne $current -and (Is-NonErrorDiagnosticHeader $lineInfo.Clean)) {
        if ($current.Count -gt 0) {
            $blocks.Add($current.ToArray())
        }
        $current = $null
        continue
    }

    if ($null -ne $current) {
        $current.Add($lineInfo)
    }
}

if ($null -ne $current -and $current.Count -gt 0) {
    $blocks.Add($current.ToArray())
}

# --- Group by dedupe key ---
$groups = [ordered]@{}
foreach ($block in $blocks) {
    $info = Get-ErrorInfo $block
    $key = $info.Key

    if (-not $groups.Contains($key)) {
        $groups[$key] = [ordered]@{
            Info       = $info
            Locations  = [System.Collections.Generic.List[string]]::new()
            LocationSet = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
        }
    }

    if ($info.Location -and $groups[$key].LocationSet.Add($info.Location)) {
        $groups[$key].Locations.Add($info.Location)
    }
}

# --- Emit minified output ---
$totalErrors = $blocks.Count
$uniqueErrors = $groups.Count

Write-Output "# Minified Rust errors: $totalErrors total -> $uniqueErrors unique"
Write-Output ''

foreach ($entry in $groups.Values) {
    $info = $entry.Info
    $locations = $entry.Locations

    Write-Output "$($info.ErrorCode): $($info.Message)"

    if ($info.TargetMethod) {
        Write-Output "  target: $($info.TargetMethod)"
    }

    if ($locations.Count -eq 0) {
        Write-Output '  --> (no location found)'
    } elseif ($locations.Count -eq 1) {
        Write-Output "  --> $($locations[0])"
    } else {
        Write-Output "  --> $($locations.Count) call sites:"
        foreach ($loc in $locations) {
            Write-Output "      - $loc"
        }
    }

    $coreDiagnostics = [System.Collections.Generic.List[string]]::new()
    $section = ''
    $noteEmitted = $false
    $includeCurrentNoteSection = $false

    foreach ($lineInfo in $info.Block) {
        $line = $lineInfo.Clean

        if ($line -match '^\s*error(?:\[[A-Z]\d+\])?:' -or $line -match '^\s*-->\s*') {
            continue
        }

        if ($line -match '^\s*(?:=\s*)?help:\s*(.*)$') {
            $section = 'help'
            $includeCurrentNoteSection = $false
            if ($KeepHelp) {
                $coreDiagnostics.Add($line)
            }
            continue
        }

        if ($line -match '^\s*(?:=\s*)?note:\s*(.*)$') {
            $section = 'note'
            if ($KeepNotes -or -not $noteEmitted) {
                $coreDiagnostics.Add($line)
                $includeCurrentNoteSection = $true
                if (-not $KeepNotes) {
                    $noteEmitted = $true
                }
            } else {
                $includeCurrentNoteSection = $false
            }
            continue
        }

        if ([string]::IsNullOrWhiteSpace($line)) {
            $section = ''
            $includeCurrentNoteSection = $false
            continue
        }

        if ($section -eq 'help' -and -not $KeepHelp) {
            continue
        }

        if ($section -eq 'note' -and -not $KeepNotes -and -not $includeCurrentNoteSection) {
            continue
        }

        # Skip noisy suggestion diff payload lines only within help sections.
        if ($section -eq 'help' -and $line -match '^\s*(?:\d+\s*)?\|\s*[-+~]') {
            continue
        }

        # Skip empty gutter-only source frame lines.
        if ($line -match '^\s*(?:\d+\s*)?\|\s*$') {
            continue
        }

        $coreDiagnostics.Add($line)
    }

    $previous = ''
    foreach ($diag in $coreDiagnostics) {
        $normalized = $diag.TrimEnd()
        if ($normalized -ne $previous) {
            Write-Output "  $normalized"
        }
        $previous = $normalized
    }

    Write-Output ''
}

Write-Output "# Summary: $totalErrors errors collapsed to $uniqueErrors unique signatures"
