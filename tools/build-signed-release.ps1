$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$uiRoot = Join-Path $repoRoot "apps\lumi-ui"
$generatedConfig = Join-Path $uiRoot "src-tauri\tauri.release.generated.json"

& (Join-Path $PSScriptRoot "audit-dependencies.ps1")
if ($LASTEXITCODE -ne 0) {
    throw "Dependency audit failed with exit code $LASTEXITCODE"
}

function Require-EnvironmentValue([string]$Name) {
    $value = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($value)) {
        throw "Required release environment variable is missing: $Name"
    }
    return $value
}

function Require-HttpsUrl([string]$Name) {
    $value = Require-EnvironmentValue $Name
    $uri = [Uri]$value
    if (-not $uri.IsAbsoluteUri -or $uri.Scheme -ne "https") {
        throw "$Name must be an absolute HTTPS URL"
    }
    return $value
}

function Find-SignTool {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path -LiteralPath $kitsRoot -PathType Container) {
        $candidate = Get-ChildItem -LiteralPath $kitsRoot -Directory |
            Sort-Object Name -Descending |
            ForEach-Object { Join-Path $_.FullName "x64\signtool.exe" } |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            Select-Object -First 1
        if ($candidate) {
            return $candidate
        }
    }
    throw "signtool.exe was not found; install the Windows SDK signing tools"
}

$null = Require-HttpsUrl "LUMICONTROL_UPDATE_ENDPOINT"
$null = Require-EnvironmentValue "LUMICONTROL_UPDATE_PUBKEY"
$null = Require-HttpsUrl "LUMICONTROL_WEATHER_ENDPOINT"
$null = Require-EnvironmentValue "TAURI_SIGNING_PRIVATE_KEY"
$certificateThumbprint = Require-EnvironmentValue "LUMICONTROL_WINDOWS_CERTIFICATE_THUMBPRINT"
$timestampUrl = Require-EnvironmentValue "LUMICONTROL_WINDOWS_TIMESTAMP_URL"

$releaseConfig = @{
    bundle = @{
        createUpdaterArtifacts = $true
        externalBin = @("binaries/lumi-agent")
        windows = @{
            certificateThumbprint = $certificateThumbprint
            digestAlgorithm = "sha256"
            timestampUrl = $timestampUrl
            webviewInstallMode = @{
                type = "offlineInstaller"
            }
        }
    }
}
$releaseConfig | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $generatedConfig -Encoding UTF8

& (Join-Path $PSScriptRoot "stage-agent-sidecar.ps1") -Profile release
if ($LASTEXITCODE -ne 0) {
    throw "Agent staging failed with exit code $LASTEXITCODE"
}

$stagedAgent = Get-ChildItem -LiteralPath (Join-Path $uiRoot "src-tauri\binaries") -Filter "lumi-agent-*.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $stagedAgent) {
    throw "The staged Lumi Agent sidecar was not found"
}
$signTool = Find-SignTool
& $signTool sign /sha1 $certificateThumbprint /fd SHA256 /tr $timestampUrl /td SHA256 $stagedAgent.FullName
if ($LASTEXITCODE -ne 0) {
    throw "Lumi Agent signing failed with exit code $LASTEXITCODE"
}
$agentSignature = Get-AuthenticodeSignature -LiteralPath $stagedAgent.FullName
if ($agentSignature.Status -ne "Valid") {
    throw "Lumi Agent Authenticode signature is not valid: $($agentSignature.Status)"
}

Push-Location $uiRoot
try {
    & npx tauri build --config src-tauri/tauri.release.generated.json
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri signed build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$bundleRoot = Join-Path $repoRoot "target\release\bundle"
$desktopApp = Get-Item -LiteralPath (Join-Path $repoRoot "target\release\LumiControl.exe")
$desktopSignature = Get-AuthenticodeSignature -LiteralPath $desktopApp.FullName
if ($desktopSignature.Status -ne "Valid") {
    throw "Desktop app Authenticode signature is not valid: $($desktopSignature.Status)"
}
$installer = Get-ChildItem -LiteralPath (Join-Path $bundleRoot "nsis") -Filter "*-setup.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $installer) {
    throw "NSIS installer was not produced"
}
$authenticode = Get-AuthenticodeSignature -LiteralPath $installer.FullName
if ($authenticode.Status -ne "Valid") {
    throw "Installer Authenticode signature is not valid: $($authenticode.Status)"
}
$updateArtifact = Get-ChildItem -LiteralPath (Join-Path $bundleRoot "nsis") -Filter "*.nsis.zip" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $updateArtifact) {
    throw "The signed NSIS updater archive was not produced"
}
$updateSignature = Get-Item -LiteralPath ($updateArtifact.FullName + ".sig") -ErrorAction Stop
$version = (Get-Content -LiteralPath (Join-Path $uiRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version

$manifest = @{
    version = $version
    generatedAtUtc = [DateTime]::UtcNow.ToString("o")
    desktopApp = @{
        name = $desktopApp.Name
        sha256 = (Get-FileHash -LiteralPath $desktopApp.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    agent = @{
        name = $stagedAgent.Name
        sha256 = (Get-FileHash -LiteralPath $stagedAgent.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    installer = @{
        name = $installer.Name
        sha256 = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    updater = @{
        name = $updateArtifact.Name
        sha256 = (Get-FileHash -LiteralPath $updateArtifact.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        signature = $updateSignature.Name
        signatureSha256 = (Get-FileHash -LiteralPath $updateSignature.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$manifestPath = Join-Path $bundleRoot "release-manifest.json"
$manifest | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
Write-Host "Signed release verified: $($installer.FullName)"
Write-Host "Release manifest: $manifestPath"
