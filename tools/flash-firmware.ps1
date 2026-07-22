param(
    [ValidateSet("sensor", "sensor-relay")]
    [string]$Profile = "sensor-relay",
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Port,
    [string]$ArduinoConfig = $env:ARDUINO_CLI_CONFIG_FILE,
    [string]$Fqbn = "esp32:esp32:nologo_esp32c3_super_mini:FlashMode=dio"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$sketch = Join-Path $repo "firmware\lumi-device\lumi_device"
$output = Join-Path $repo "firmware\lumi-device\build\$Profile"

& (Join-Path $PSScriptRoot "build-firmware.ps1") `
    -Profile $Profile `
    -ArduinoConfig $ArduinoConfig `
    -Fqbn $Fqbn

$arduinoArgs = @()
if ($ArduinoConfig) {
    $arduinoArgs += @("--config-file", $ArduinoConfig)
}

& arduino-cli @arduinoArgs upload `
    --fqbn $Fqbn `
    --port $Port `
    --input-dir $output `
    $sketch

if ($LASTEXITCODE -ne 0) {
    throw "Firmware upload failed for $Profile on $Port"
}
