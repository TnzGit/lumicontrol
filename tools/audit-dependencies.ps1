$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$uiRoot = Join-Path $repoRoot "apps\lumi-ui"
$windowsTarget = "x86_64-pc-windows-msvc"
$excludedQuickXml = "quick-xml@0.39.4"
$excludedGlib = "glib@0.18.5"
$excludedAdvisories = @(
    "RUSTSEC-2024-0429", # Optional Linux/GTK metadata; excluded from the Windows tree below.
    "RUSTSEC-2026-0194",
    "RUSTSEC-2026-0195"
)

function Assert-NotInWindowsDependencyTree([string]$Package, [string]$VersionPattern) {
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $treeOutput = (& cargo tree --locked --target $windowsTarget -i $Package 2>&1 | Out-String)
        $treeExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }

    if ($treeExitCode -eq 0) {
        if ($treeOutput -match $VersionPattern) {
            throw "$Package is reachable in the Windows release dependency tree"
        }
        return
    }

    if ($treeOutput -notmatch "did not match any packages") {
        throw "Unable to verify the Windows dependency tree for $Package`n$treeOutput"
    }
}

Push-Location $repoRoot
try {
    Assert-NotInWindowsDependencyTree $excludedQuickXml "(?m)^quick-xml v0\.39\.4(?:\s|$)"
    Assert-NotInWindowsDependencyTree $excludedGlib "(?m)^glib v0\.18\.5(?:\s|$)"

    $auditArgs = @(
        "audit",
        "--target-os", "windows",
        "--target-arch", "x86_64"
    )
    foreach ($advisory in $excludedAdvisories) {
        $auditArgs += @("--ignore", $advisory)
    }
    try {
        $ErrorActionPreference = "Continue"
        & cargo @auditArgs
        $auditExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($auditExitCode -ne 0) {
        throw "Rust dependency audit failed with exit code $auditExitCode"
    }
}
finally {
    Pop-Location
}

Push-Location $uiRoot
try {
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        & npm audit --audit-level=high
        $npmExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($npmExitCode -ne 0) {
        throw "npm dependency audit failed with exit code $npmExitCode"
    }
}
finally {
    Pop-Location
}

Write-Host "Dependency audit passed for the LumiControl Windows release."
