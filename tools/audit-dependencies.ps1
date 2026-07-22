$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$uiRoot = Join-Path $repoRoot "apps\lumi-ui"
$windowsTarget = "x86_64-pc-windows-msvc"
$excludedQuickXml = "quick-xml@0.39.4"
$excludedAdvisories = @("RUSTSEC-2026-0194", "RUSTSEC-2026-0195")

Push-Location $repoRoot
try {
    $previousPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = "Continue"
        $treeOutput = (& cargo tree --locked --target $windowsTarget -i $excludedQuickXml 2>&1 | Out-String)
        $treeExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($treeExitCode -ne 0) {
        throw "Unable to verify the Windows dependency tree for $excludedQuickXml`n$treeOutput"
    }
    if ($treeOutput -match "(?m)^quick-xml v0\.39\.4(?:\s|$)") {
        throw "$excludedQuickXml is reachable in the Windows release dependency tree"
    }

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
