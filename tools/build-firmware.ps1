param(
    [ValidateSet("sensor", "sensor-relay")]
    [string]$Profile = "sensor-relay",
    [string]$ArduinoConfig = $env:ARDUINO_CLI_CONFIG_FILE,
    [string]$Fqbn = "esp32:esp32:nologo_esp32c3_super_mini:FlashMode=dio"
)

$ErrorActionPreference = "Stop"
if (-not (Get-Command arduino-cli -ErrorAction SilentlyContinue)) {
    throw "arduino-cli was not found on PATH"
}
if ($ArduinoConfig -and -not (Test-Path -LiteralPath $ArduinoConfig -PathType Leaf)) {
    throw "Arduino CLI config was not found: $ArduinoConfig"
}

$repo = Split-Path -Parent $PSScriptRoot
$sketch = Join-Path $repo "firmware\lumi-device\lumi_device"
$output = Join-Path $repo "firmware\lumi-device\build\$Profile"
$buildPath = Join-Path $repo "firmware\lumi-device\.build\$Profile"
$relay = if ($Profile -eq "sensor-relay") { 1 } else { 0 }
$flags = "-DLUMI_PROFILE_RELAY=$relay"
$arduinoArgs = @()
if ($ArduinoConfig) {
    $arduinoArgs += @("--config-file", $ArduinoConfig)
}

New-Item -ItemType Directory -Force -Path $output | Out-Null
New-Item -ItemType Directory -Force -Path $buildPath | Out-Null
& arduino-cli @arduinoArgs compile `
    --fqbn $Fqbn `
    --build-property "compiler.cpp.extra_flags=$flags" `
    --build-path $buildPath `
    --output-dir $output `
    $sketch

if ($LASTEXITCODE -ne 0) {
    throw "Firmware compilation failed for profile $Profile"
}
