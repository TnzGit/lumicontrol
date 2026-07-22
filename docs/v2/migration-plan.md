# LumiControl V2 Migration Plan

Status: V2 release candidate implemented on 2026-07-17

## 1. Migration principle

Build V2 beside the current application and migrate verified behavior through
typed boundaries. Do not perform a single big-bang rewrite. The current Rust app
remains the reference implementation and engineering fallback until V2 reaches
the release gates below.

## 2. Proposed workspace

```text
apps/
  lumi-agent/
  lumi-ui/
crates/
  lumi-core/
  lumi-device/
  lumi-ipc/
  lumi-monitor-windows/
  lumi-protocol/
  lumi-store/
tools/
  device-simulator/
  resource-probe/
firmware/
  lumi-device/
```

The existing `src/` and prototype firmware folders stay in place during the
migration. New crates must not import production logic from `src/main.rs` by file
path; behavior is moved with tests into an owned crate.

## 3. Phase 0: stabilize and measure the current build

Deliverables:

- remove fixed GUI repaint timers that keep a hidden viewport polling
- record five-minute CPU, memory, handle, and thread baselines
- define repeatable visible, tray-hidden, disconnected-device, and resume tests
- capture current configuration and protocol fixtures before refactoring

Exit criteria:

- hidden release process averages below 0.2% of one logical core
- all current automated tests pass
- release build succeeds without warnings introduced by the change
- current supported user workflow remains functional

## 4. Phase 1: protocol and simulation

Deliverables:

- `lumi-protocol` request/response/event types
- V2 firmware handshake, capability reporting, and telemetry
- sensor-only and sensor-plus-relay firmware profiles
- deterministic device simulator with disconnect, malformed frame, delay, and
  hardware-error injection
- compatibility tests using captured JSON fixtures

Exit criteria:

- Agent-side protocol tests require no physical hardware
- both profiles pass the same contract suite
- unsupported relay commands fail with `unsupported_capability`
- reconnect and sequence-gap behavior is deterministic under simulation

## 5. Phase 2: Agent foundation

Deliverables:

- per-user single-instance Agent and minimal tray host
- automatic USB discovery and persistent serial connection
- named-pipe IPC with current-user ACL
- versioned settings under `%LOCALAPPDATA%\LumiControl`
- atomic save, migration, backup, and V1 import
- structured rotating logs and support snapshot

Exit criteria:

- closing or crashing a test UI does not interrupt sensor processing
- unplug/replug restores the same device identity without user selection
- corrupt settings do not erase the last valid configuration
- idle resource budgets in the architecture specification pass

## 6. Phase 3: control and monitor migration

Deliverables:

- move curves, rules, and target calculation into `lumi-core`
- log-lux filtering, stale-sensor policy, and hysteresis
- stable EDID/display-path monitor identity
- normalized DDC/CI ranges and capability qualification
- asynchronous, cancelable, retargetable transition scheduler
- display hotplug, user manual override, sleep, and resume handling

Exit criteria:

- unit tests use fake clock, device, weather, and monitor ports
- monitor order changes do not move calibration to another display
- one failing monitor does not delay relay or healthy-monitor control
- no central Agent loop sleeps while applying a transition
- suspend/resume and hotplug test matrix passes

## 7. Phase 4: production UI

Deliverables:

- on-demand Tauri UI connected only through Agent IPC
- first-run device, monitor, calibration, relay, and startup flow
- compact status dashboard
- calibration curve editor with history and explicit reset
- priority rule builder with presets, test context, and match explanation
- capability-driven navigation and controls
- localization-ready copy and accessible keyboard navigation

Exit criteria:

- UI process exits when its window closes
- Agent behavior is unchanged when no UI client is connected
- sensor-only hardware never presents functional relay actions
- all normal settings are editable without opening JSON
- critical onboarding and rules flows pass end-to-end tests

## 8. Phase 5: distribution and private beta

Deliverables:

- signed installer and executables
- startup registration, single-instance launch routing, and clean uninstall
- signed application update channel with staged rollout and rollback
- firmware compatibility check and guided recovery path
- privacy disclosure, opt-in crash reporting, and diagnostics export
- hardware-in-the-loop release rig

Exit criteria:

- clean install, upgrade, downgrade-recovery, and uninstall tests pass
- application and firmware compatibility failures are actionable
- seven-day soak test has no resource growth or missed reconnect
- 100 suspend/resume cycles pass on the release hardware matrix
- support can diagnose device, sensor, monitor, rule, and update state from one
  exported bundle

## 9. Required automated test layers

| Layer | Coverage |
| --- | --- |
| Core unit | filtering, curves, rules, transitions, manual override |
| Protocol | golden frames, malformed input, version negotiation, fuzz targets |
| Device integration | discovery, reconnect, timeout, sequence gaps, two SKUs |
| Monitor integration | stable IDs, raw range normalization, partial failures |
| Store | migrations, interrupted writes, corrupt files, V1 import |
| IPC | authorization, reconnect snapshot, incompatible API version |
| UI end-to-end | onboarding, calibration, rules, update/error states |
| Hardware-in-loop | USB loss, sensor fault, relay, multi-monitor, sleep/resume |
| Resource regression | idle CPU, memory, handles, threads, wakeups |

Source-text assertions may remain temporarily for legacy regressions, but V2
release gates require behavior tests through public component boundaries.

## 10. Implementation status

| Phase | Status | Evidence |
| --- | --- | --- |
| Phase 0 | Complete | Five-minute live V2 resource baseline and legacy regression suite |
| Phase 1 | Complete | Protocol V2, two firmware profiles, simulator, malformed/reconnect tests |
| Phase 2 | Complete | Headless Agent, discovery, same-user IPC, migration, logs, diagnostics |
| Phase 3 | Complete | Core extraction, DDC workers, smooth retargeting, hotplug and suspend/resume |
| Phase 4 | Complete | On-demand Tauri UI, onboarding, calibration history, rules, support views |
| Phase 5 engineering | Complete | NSIS bundle, startup, single instance, signed-update and signing pipeline |
| Phase 5 operations | External gate | Production certificates, hosted HTTPS update feed, soak and hardware matrix |

The legacy `src/` application remains only as a migration fallback and test
oracle. New product work belongs in the V2 workspace boundaries above.
