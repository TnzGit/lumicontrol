# LumiControl

LumiControl is a Windows ambient-light controller for DDC/CI monitors. A small
USB device built around an ESP32-C3 and BH1750 sensor supplies live lux readings;
the background Agent maps those readings to monitor brightness and can optionally
control a low-voltage light strip through a relay output.

> [!IMPORTANT]
> LumiControl is **source-available for noncommercial use** under the
> [PolyForm Noncommercial License 1.0.0](LICENSE). It is not OSI-approved open
> source, and commercial use is not permitted. Third-party components remain
> under their respective licenses; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Features

- always-on, low-resource Windows Agent with a separate on-demand Tauri UI
- automatic discovery of supported USB sensor and sensor-plus-relay profiles
- smooth, retargetable DDC/CI monitor brightness transitions
- draggable lux-to-brightness calibration curve with three-step history
- manual brightness override detection
- optional relay controls with NO/NC contact mapping
- prioritized light rules using time, sunrise, sunset, weather, lux, and monitor brightness
- light, dark, and system themes
- local diagnostics with sensitive identifiers redacted

## Architecture

- `apps/lumi-agent`: background tray process; owns USB, DDC/CI, rules, settings,
  diagnostics, and brightness transitions
- `apps/lumi-ui`: React and Tauri desktop interface
- `crates/lumi-*`: shared protocol, policy, storage, device, environment, IPC,
  and Windows monitor libraries
- `firmware/lumi-device`: production ESP32-C3 firmware and hardware profiles
- `tools`: firmware, device-console, simulator, audit, and release helpers
- `src`: legacy V1 implementation retained as a regression reference

The Agent and UI communicate over a named pipe restricted to the current Windows
user. Settings, backups, logs, and diagnostics are stored under
`%LOCALAPPDATA%\LumiControl` and are not part of the repository.

## Hardware

The validated ESP32-C3 SuperMini wiring is:

| Function | ESP32-C3 pin |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| Relay module input | GPIO10 |

The relay profile assumes a common active-low 5 V relay module. Use a relay
module with a suitable driver and flyback protection; never drive a bare relay
coil from a GPIO. Keep mains wiring outside the low-voltage enclosure and use an
appropriately isolated, certified power path.

## Development

Prerequisites:

- Windows 10 or 11 with WebView2
- Rust stable with `rustfmt` and `clippy`
- Node.js and npm
- DDC/CI enabled in each monitor's on-screen settings
- `cargo-audit` for the dependency audit
- Arduino CLI, Espressif Arduino core 3.3.10, and ArduinoJson 7.4.3 for firmware

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
python -m pytest -q tests/test_launchers.py
.\tools\audit-dependencies.ps1

Set-Location apps\lumi-ui
npm ci
npm run build
npm run tauri:build
```

The unsigned local installer is written to
`target\release\bundle\nsis\LumiControl_0.2.0_x64-setup.exe`. Public binary
distribution should use code signing and include all required third-party notices.

Useful hardware commands:

```powershell
cargo run -p lumi-device-console -- --port COM3 --samples 5
.\tools\build-firmware.ps1 -Profile sensor-relay
.\tools\flash-firmware.ps1 -Profile sensor-relay -Port COM3
```

Architecture, protocol, firmware validation, performance measurements, and
release-readiness notes are available under [`docs/v2`](docs/v2/).

## Contributing

Contributions are welcome for noncommercial use under the same project license.
Read [CONTRIBUTING.md](CONTRIBUTING.md) before submitting changes. Security
issues should follow [SECURITY.md](SECURITY.md), and use of the project name and
icons is governed separately by [TRADEMARKS.md](TRADEMARKS.md).
