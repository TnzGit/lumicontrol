# LumiControl

<p align="center"><strong>Ambient-light-aware monitor brightness for Windows.</strong></p>

<p align="center">
  <a href="https://github.com/TnzGit/lumicontrol/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/TnzGit/lumicontrol/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/TnzGit/lumicontrol/releases"><img alt="Release" src="https://img.shields.io/github/v/release/TnzGit/lumicontrol?include_prereleases"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-PolyForm%20Noncommercial-4B5563"></a>
</p>

<p align="center">
  English |
  <a href="docs/readme/README.zh-CN.md">简体中文</a> |
  <a href="docs/readme/README.zh-TW.md">繁體中文</a> |
  <a href="docs/readme/README.ja.md">日本語</a> |
  <a href="docs/readme/README.ko.md">한국어</a> |
  <a href="docs/readme/README.es.md">Español</a> |
  <a href="docs/readme/README.pt-BR.md">Português</a> |
  <a href="docs/readme/README.tr.md">Türkçe</a> |
  <a href="docs/readme/README.ru.md">Русский</a> |
  <a href="docs/readme/README.uk.md">Українська</a>
</p>

LumiControl automatically adapts compatible DDC/CI monitors to the light around
you. It can use either live lux readings from an ESP32-C3 and GY-302/BH1750, or a
hardware-free recommendation model based on local weather, solar elevation,
sunrise, sunset, and seasonal daylight. A low-resource Windows Agent applies the
result with smooth, retargetable brightness transitions. An optional relay
profile can also control a low-voltage desk or monitor light strip.

## Download

Download the latest Windows x64 installer from
**[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)**.

> [!WARNING]
> Preview installers are currently not code-signed. Windows SmartScreen may show
> an unknown-publisher warning. Verify the SHA-256 checksum published with the
> Release before running the installer.

### Requirements

- Windows 10 or Windows 11, x64
- a monitor with DDC/CI enabled in its on-screen menu
- either an internet connection and location for **Weather & sun** mode, or an
  ESP32-C3 SuperMini with a GY-302/BH1750 sensor for **USB sensor** mode
- optional supported 5 V relay module for light-strip control

After installation, open LumiControl and choose an automatic brightness source.
**Weather & sun** works without LumiControl hardware and lets you add a personal
offset to the recommendation. **USB sensor** discovers supported hardware
automatically and provides a draggable lux-to-brightness calibration curve.

## Highlights

- always-on, low-resource Windows Agent with an on-demand Tauri interface
- hardware-free brightness recommendations from weather, solar elevation,
  sunrise, sunset, seasonal daylight, and a personal offset
- automatic USB discovery for sensor-only and sensor-plus-relay profiles
- smooth DDC/CI brightness transitions that can retarget while moving
- draggable lux-to-brightness calibration curve with three-step revert history
- manual brightness override detection and configurable control intervals
- per-monitor calibration and full-range brightness control
- prioritized light-strip rules using time, sunrise, sunset, weather, lux, and
  monitor brightness
- NO/NC relay contact mapping, manual relay control, and fallback actions
- light, dark, and system themes
- local diagnostics with sensitive hardware identifiers redacted

## Optional Hardware

Validated ESP32-C3 SuperMini wiring:

| Function | ESP32-C3 pin |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| Relay module input | GPIO10 |

Two firmware profiles are provided:

| Profile | Sensor | Relay |
| --- | --- | --- |
| `sensor` | BH1750 | No |
| `sensor-relay` | BH1750 | GPIO10 |

The relay profile assumes a common active-low 5 V relay module. Use a module
with a suitable driver and flyback protection; never drive a bare relay coil
from an ESP32 GPIO. Keep mains wiring outside the low-voltage enclosure and use
an appropriately isolated power path.

## Privacy And Security

The Agent and UI communicate through a Windows named pipe restricted to the
current user. Settings, backups, logs, and diagnostics remain under
`%LOCALAPPDATA%\LumiControl`. Weather requests are made only when **Weather &
sun** mode or enabled light rules need them. Requests include the configured
coordinates but no LumiControl account or hardware identifier. If weather is
unavailable, brightness control continues with the local sunlight model.

## Architecture

- `apps/lumi-agent`: tray Agent that owns USB, DDC/CI, rules, settings, and
  brightness transitions
- `apps/lumi-ui`: React and Tauri desktop interface
- `crates/lumi-*`: protocol, policy, storage, device, environment, IPC, and
  Windows monitor libraries
- `firmware/lumi-device`: ESP32-C3 firmware and hardware profiles
- `tools`: firmware, simulator, audit, packaging, and release helpers
- `src`: legacy V1 implementation retained as a regression reference

Architecture, protocol, firmware validation, performance measurements, and
release-readiness notes are available under [`docs/v2`](docs/v2/). The
hardware-free recommendation model is documented in
[`docs/v2/environment-brightness.md`](docs/v2/environment-brightness.md).

## Development

Prerequisites include Rust stable with `rustfmt` and `clippy`, Node.js, npm,
Python, and WebView2. Firmware builds additionally require Arduino CLI,
Espressif Arduino core 3.3.10, and ArduinoJson 7.4.3.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
python -m pytest -q tests/test_launchers.py
.\tools\audit-dependencies.ps1

Set-Location apps\lumi-ui
npm ci
npm run build
npm run tauri:build
```

Useful hardware commands:

```powershell
cargo run -p lumi-device-console -- --port COM3 --samples 5
.\tools\build-firmware.ps1 -Profile sensor-relay
.\tools\flash-firmware.ps1 -Profile sensor-relay -Port COM3
```

## License

LumiControl is **source-available for noncommercial use** under the
[PolyForm Noncommercial License 1.0.0](LICENSE). It is not OSI-approved open
source, and commercial use is not permitted. Third-party components retain
their own licenses; see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Contributions are welcome under the same project license. Read
[CONTRIBUTING.md](CONTRIBUTING.md), report security issues through
[SECURITY.md](SECURITY.md), and review [TRADEMARKS.md](TRADEMARKS.md) before
using the LumiControl name or icons.
