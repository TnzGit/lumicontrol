# LumiControl V2 Software Architecture

Status: Implemented V2 release-candidate architecture

## 1. Product model

LumiControl ships one Windows software package for all supported Lumi hardware.
The connected device declares its model and capabilities during discovery. The
software never relies on a user-selected SKU or a hard-coded COM port.

Initial hardware profiles:

| Profile | Required capabilities |
| --- | --- |
| Lumi Sensor | `ambient_lux` |
| Lumi Sensor + Relay | `ambient_lux`, `relay` |

Unsupported controls and rule actions are omitted from the normal UI. Raw GPIO,
relay polarity, and NO/NC wiring belong in installation diagnostics, not the
dashboard.

## 2. Goals

- Continue brightness control when the settings UI is closed or crashes.
- Discover and reconnect supported USB devices without a configured COM port.
- Support both hardware profiles through one capability-driven code path.
- Keep idle CPU, memory, wakeups, and DDC/CI traffic low and measurable.
- Survive display hotplug, USB reconnect, lock, sleep, and resume.
- Give every persisted format and IPC/device contract an explicit version.
- Make the control engine deterministic and testable without physical hardware.

## 3. Non-goals for the first production release

- Cloud accounts or a required online service.
- Remote control over the public internet.
- User-authored scripts or arbitrary code in automation rules.
- Cross-platform support. V2 targets supported Windows 10 and Windows 11 builds.
- Automatic support for every monitor. DDC/CI compatibility remains a qualified
  hardware requirement.

## 4. Process model

### 4.1 Lumi Agent

`lumi-agent.exe` is a single-instance, per-user background process started at
login. It owns all mutable product state and all hardware handles:

- USB serial discovery and the persistent device connection
- sensor filtering and health evaluation
- monitor enumeration and DDC/CI operations
- brightness transitions and manual override handling
- relay rules and logical light state
- configuration, migrations, logs, and diagnostics
- local IPC server

The Agent is not a Windows Service. Monitor control and tray interaction belong
to the logged-in interactive session. A same-user named mutex enforces one Agent
per session.

### 4.2 Lumi UI

`lumi-ui.exe` is launched on first run, from the tray, or from a desktop/start
menu shortcut. It is an IPC client and owns no serial or monitor handles. Closing
the window terminates the UI process; the Agent continues running.

The production UI is an on-demand Tauri application with a TypeScript frontend.
The current egui UI remains an engineering fallback during migration only.

### 4.3 Tray host

The tray icon is owned by the Agent or by a minimal native companion that has no
GPU-backed window. Tray actions call Agent commands and launch the UI only when
needed.

## 5. Component boundaries

| Component | Responsibility | Must not own |
| --- | --- | --- |
| `lumi-core` | curves, filtering, target calculation, rules, transitions | Windows APIs, serial ports, files |
| `lumi-protocol` | typed USB and IPC messages, version negotiation | device discovery, UI state |
| `lumi-device` | discovery, persistent connection, reconnect state machine | monitor control, rules |
| `lumi-monitor-windows` | stable monitor identity, DDC/CI capabilities and writes | sensor policy, UI |
| `lumi-store` | schema migration, validation, atomic persistence | hardware I/O |
| `lumi-agent` | orchestration and authoritative runtime state | rendered UI |
| `lumi-ui` | presentation, onboarding, settings editing | hardware handles, policy decisions |

All time-dependent core logic receives a clock abstraction. Weather and solar
data are optional providers. The sensor remains the primary control source.

## 6. Runtime data flow

1. Device Manager emits a validated lux sample.
2. Sensor Pipeline rejects invalid values and filters lux in log space.
3. Control Engine evaluates the calibration curve and deadband.
4. Rule Engine evaluates enabled rules in priority order.
5. Transition Scheduler retargets each monitor without blocking other work.
6. Monitor Manager performs bounded DDC/CI writes.
7. Agent publishes one immutable state snapshot to connected UI clients.

The system is event-driven. Fixed GUI repaint timers and repeated monitor scans
are not part of the control loop.

## 7. Monitor model

- A monitor key is derived from EDID identity plus the Windows display path. An
  enumeration ordinal such as `monitor-1` is never persisted.
- DDC/CI raw minimum/current/maximum values are normalized to logical percent.
- A display-change event invalidates cached physical handles and triggers one
  capability refresh.
- One slow or faulty monitor cannot block device reads, relay actions, or other
  monitors indefinitely.
- Transitions are time-based, cancelable, and retargetable. The scheduler limits
  DDC writes and does not sleep inside the central Agent command loop.
- A detected user brightness change starts a configurable manual-override grace
  period before automatic control resumes.

## 8. Persistence

Product data lives under `%LOCALAPPDATA%\LumiControl` rather than the process
working directory.

| File | Purpose |
| --- | --- |
| `settings.json` | user-editable settings and automation rules |
| `state.json` | last device/monitor association and transient recovery state |
| `logs\` | bounded rotating diagnostic logs |
| `backups\` | the last known-good migrated settings |

Every document contains `schema_version`. Writes use a temporary file, flush,
and atomic replacement. Invalid settings are quarantined and reported; they are
never silently replaced with defaults. V1 `config.json` is imported once and
left untouched.

## 9. Local IPC

- Transport: same-user Windows named pipe.
- Framing: length-prefixed UTF-8 JSON for inspectability during V2 development.
- Security: pipe ACL limited to the current user; no TCP listener.
- Contract: request ID, API version, typed command, typed result or error.
- State: UI receives an initial snapshot followed by versioned state-change
  events. Reconnection always starts from a fresh full snapshot.

The UI may edit a draft configuration, but only the Agent validates and commits
it. Unsupported relay actions are rejected when the connected device lacks the
`relay` capability.

## 10. Resource and reliability budgets

Measured on a release build after a five-minute warm-up:

| Scenario | Acceptance target |
| --- | --- |
| UI closed, device connected | Agent average CPU below 0.2% of one logical core |
| UI closed, device absent | Agent average CPU below 0.1% of one logical core |
| Agent steady-state memory | working set below 35 MB |
| UI closed | no WebView or GPU-backed UI process remains |
| USB reconnect | healthy state restored within 5 seconds in the normal case |
| Resume from sleep | first valid control decision within 10 seconds |
| Relay command | result or actionable timeout within 2 seconds |
| Configuration save | previous valid file survives interruption |

CPU, memory, handle count, thread count, reconnect count, malformed frame count,
sensor age, and DDC error count are included in diagnostic snapshots.

## 11. User experience boundaries

The first-run flow performs device discovery, monitor qualification, comfort
calibration, optional relay verification, and startup preference selection.

The dashboard answers only:

- Is automatic control working?
- What is the current ambient light and brightness?
- Does anything need attention?

Curve editing, rules, monitor details, installation wiring, firmware, and logs
live in dedicated views. Advanced rule building provides presets, a current
match explanation, and a test mode with a supplied time/lux/weather context.

## 12. Release and support requirements

- Code-signed installer and executables
- semantic application, firmware, and protocol versions
- signed update metadata and rollback support
- explicit application/firmware compatibility matrix
- single-instance behavior and clean uninstall
- exportable support bundle with redaction of user-sensitive data
- opt-in crash reporting and analytics; core control remains fully local
