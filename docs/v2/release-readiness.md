# LumiControl V2 Release Readiness

Status: Engineering release candidate complete; public hardware launch gated

## What is complete

- One capability-driven Windows package for sensor-only and sensor-relay SKUs
- Protocol V2 firmware, simulator, discovery, reconnect, and compatibility checks
- Headless per-user Agent with tray, same-user IPC, atomic settings, and V1 import
- Smooth asynchronous DDC/CI control, hotplug handling, and suspend/resume recovery
- On-demand Tauri UI with onboarding, calibration, rules, diagnostics, and updates
- Current-user NSIS installer, sidecar packaging, startup, and single instance
- Fail-closed Authenticode and Tauri updater signing pipeline
- Redacted support bundle and five-minute connected-hardware resource baseline

## Local release-candidate evidence

| Check | Result |
| --- | --- |
| Rust workspace tests | Pass |
| Strict Rust clippy | Pass |
| Windows-target RustSec dependency gate | Pass |
| npm dependency audit | Pass, 0 vulnerabilities |
| TypeScript and production UI build | Pass |
| Native UI close leaves Agent running | Pass |
| Second UI launch routes to existing instance | Pass |
| Real Protocol V2 sensor/relay handshake | Pass |
| Forced USB rediscovery | 30/30 pass; 348 ms maximum recovery |
| Windows suspend/resume broadcast integration | 10/10 pass; 502 ms maximum recovery |
| Post-reconnect handles and threads | No growth after 30 reconnects |
| Silent NSIS install, installed launch, and uninstall | Pass |
| Five-minute Agent CPU budget | 0.052% average of one logical core |
| Five-minute Agent memory budget | 17.504 MB working set |
| Diagnostics redaction | Serial, COM port, user name, coordinates redacted |

## Required production inputs

The signed build script intentionally fails unless these are supplied:

| Environment variable | Purpose |
| --- | --- |
| `LUMICONTROL_WINDOWS_CERTIFICATE_THUMBPRINT` | Authenticode certificate |
| `LUMICONTROL_WINDOWS_TIMESTAMP_URL` | RFC 3161 HTTPS timestamp service |
| `TAURI_SIGNING_PRIVATE_KEY` | Offline updater artifact signing key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Updater key password when configured |
| `LUMICONTROL_UPDATE_ENDPOINT` | Hosted HTTPS update manifest |
| `LUMICONTROL_UPDATE_PUBKEY` | Public updater verification key |
| `LUMICONTROL_WEATHER_ENDPOINT` | Optional HTTPS weather proxy/provider |

Run:

```powershell
.\tools\audit-dependencies.ps1
.\tools\build-signed-release.ps1
```

The signed build runs the dependency gate, signs the Agent before bundling,
builds updater artifacts, verifies the Agent, desktop app, and installer
signatures, and writes a hashed release manifest. The signed configuration
embeds the WebView2 offline installer so first installation does not depend on a
live Microsoft download. Do not publish the unsigned development installer.

The dependency gate proves that `quick-xml 0.39.4` is absent from the shipped
Windows dependency graph before ignoring `RUSTSEC-2026-0194` and
`RUSTSEC-2026-0195`. Those crates remain in the cross-platform lockfile through
Linux-only Wayland/zbus packages. Any future Windows reachability fails the gate.

## Local unsigned builds

Unsigned local installers are development artifacts and are intentionally not
stored in this repository. Before publishing any binary, rebuild from the tagged
source, record fresh hashes and smoke-test evidence, and complete the signing and
third-party notice checks described above.

## Public-launch gates

- Assign production USB VID/PID and immutable serial identities; do not ship
  Espressif development identity as the final product identity.
- Freeze board revisions and validate sensor-only and relay SKUs across the
  supported Windows 10/11 and monitor matrix.
- Complete 100 suspend/resume cycles, USB selective-suspend testing, and a
  seven-day Agent/firmware soak with resource-growth checks.
- Test clean install, upgrade, rollback recovery, and uninstall from standard
  user accounts on clean machines.
- Validate relay isolation, power topology, enclosure, EMC/ESD, thermal, and all
  market-specific electrical certifications with qualified hardware engineers.
- Contract and capacity-plan the weather service or proxy; core sensor control
  must remain usable when the service is unavailable.
- Publish privacy, diagnostics, warranty, support, and firmware-recovery policies.
- Establish staged updater rollout, revocation, incident response, and release
  key backup procedures.
- Run accessibility and Simplified Chinese/English copy review on production UI.

## Release policy

The first customer release should be a limited hardware beta. Promote it to a
general release only after every public-launch gate above has named evidence,
an owner, and a sign-off date. Software feature completeness alone is not a
hardware-product launch approval.
