# Lumi Device Firmware V2

One firmware source builds both supported hardware profiles:

- `sensor`: `lumi-sensor`, BH1750 only
- `sensor-relay`: `lumi-sensor-relay`, BH1750 plus relay coil control on GPIO10

Shared prototype wiring:

| Signal | ESP32-C3 SuperMini pin |
| --- | --- |
| BH1750 SDA | GPIO4 |
| BH1750 SCL | GPIO5 |
| Relay IN | GPIO10, relay profile only |

The relay output is active-low for the current module and starts de-energized.
Logical NO/NC light behavior is owned by the Windows Agent; firmware commands
and telemetry always describe physical coil energization.

Build from the repository root:

```powershell
arduino-cli core update-index --additional-urls https://espressif.github.io/arduino-esp32/package_esp32_index.json
arduino-cli core install esp32:esp32@3.3.10 --additional-urls https://espressif.github.io/arduino-esp32/package_esp32_index.json
arduino-cli lib install ArduinoJson@7.4.3

.\tools\build-firmware.ps1 -Profile sensor
.\tools\build-firmware.ps1 -Profile sensor-relay
```

The scripts use Arduino CLI's normal configuration by default. Set
`ARDUINO_CLI_CONFIG_FILE`, pass `-ArduinoConfig`, or pass `-Fqbn` when using an
isolated toolchain or a compatible ESP32-C3 board definition.

Flash a development board:

```powershell
.\tools\flash-firmware.ps1 -Profile sensor-relay -Port COM3
```

The validated toolchain is Espressif Arduino core 3.3.10 with ArduinoJson 7.4.3.
Production devices must receive a factory `lumi_factory/serial` NVS value.
Development boards derive a stable `DEV-...` serial from the eFuse MAC.
