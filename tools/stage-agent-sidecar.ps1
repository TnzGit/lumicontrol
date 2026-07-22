param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$cargoArgs = @("build", "-p", "lumi-agent")
if ($Profile -eq "release") {
    $cargoArgs += "--release"
}

Push-Location $repoRoot
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    $hostLine = (& rustc -vV | Select-String "^host:").Line
    if (-not $hostLine) {
        throw "Could not determine the Rust host target"
    }
    $hostTarget = $hostLine.Substring(5).Trim()
    if ($hostTarget -notlike "*-windows-*") {
        throw "LumiControl V2 packaging currently supports Windows only"
    }

    $source = Join-Path $repoRoot "target\$Profile\lumi-agent.exe"
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Agent binary was not produced at $source"
    }
    $destinationDirectory = Join-Path $repoRoot "apps\lumi-ui\src-tauri\binaries"
    New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null
    $destination = Join-Path $destinationDirectory "lumi-agent-$hostTarget.exe"
    Copy-Item -LiteralPath $source -Destination $destination -Force
    Write-Host "Staged $destination"
}
finally {
    Pop-Location
}
