# Performance Baseline: 2026-07-16

## Purpose

Record the resource regression that motivated the V2 process split and verify
the immediate fixed-repaint correction with a repeatable probe.

Probe command:

```powershell
.\tools\measure-process-resources.ps1 -ProcessId <pid> -DurationSeconds 15
```

CPU percentages are percentages of one logical core, calculated from process
CPU-time delta divided by wall-clock delta.

## Existing running release

Conditions:

- executable: `target\release\screen-brightness.exe`
- process uptime: approximately 160 hours
- GUI hidden in the tray
- build still contained a one-second `request_repaint_after` timer
- sample duration: 5 seconds

Results:

| Metric | Value |
| --- | ---: |
| Average CPU | 84.161% of one core |
| Peak CPU | 92.934% of one core |
| Average working set | 47.875 MB |
| Average private memory | 77.699 MB |
| Peak handles | 464 |
| Peak threads | 16 |

Thread sampling attributed the sustained CPU time to the GUI/event-loop thread.

## Verification release after timer removal

Conditions:

- executable: `target\verify\release\screen-brightness.exe`
- release profile with LTO
- isolated config: paused, `COM999`, rules disabled, 3600-second control tick
- no hardware or monitor writes
- root window had no visible main-window handle
- sample duration: 15 seconds

Results:

| Metric | Value |
| --- | ---: |
| Average CPU | 0.000% of one core |
| Peak CPU | 0.000% of one core |
| Average working set | 116.542 MB |
| Average private memory | 97.756 MB |
| Peak handles | 400 |
| Peak threads | 14 |

## Interpretation

Removing the fixed repaint timer eliminated the reproduced idle CPU spin. The
memory values are not a like-for-like steady-state comparison: the verification
process was a newly launched, isolated GUI build, while the old process had been
running for several days. They are retained as observations, not a memory
regression conclusion.

The result supports the V2 requirement that the production Agent must not load a
GPU-backed UI stack and that the UI process should exit when closed.

## Updated standard release with live configuration

After copying the verified binary to `target\release`, the normal application
was started with its real configuration and hardware connection, then sampled
while hidden in the tray for 35 seconds. This interval included a normal control
cycle.

| Metric | Value |
| --- | ---: |
| Average CPU | 0.176% of one core |
| Peak CPU | 4.604% of one core |
| Average working set | 81.834 MB |
| Average private memory | 62.899 MB |
| Peak handles | 381 |
| Peak threads | 15 |

This short live sample passes the provisional hidden-CPU target and confirms
that control-cycle work no longer leaves the GUI thread spinning afterward.

## V2 Agent release candidate with live hardware

Conditions:

- executable: `target\release\lumi-agent.exe`
- UI process closed for the full sample
- ESP32-C3 sensor-relay hardware connected on USB
- two qualified monitors and normal automatic control enabled
- sample duration: 302.38 seconds, 60 samples at five-second intervals

Results:

| Metric | Average | Peak |
| --- | ---: | ---: |
| CPU | 0.052% of one core | 0.622% of one core |
| Working set | 17.504 MB | 17.504 MB |
| Private memory | 2.551 MB | 2.551 MB |
| Handles | - | 192 |
| Threads | - | 9 |

The V2 Agent passes the connected-device resource budgets of less than 0.2% of
one logical core and less than 35 MB working set. Closing the Tauri UI leaves no
WebView process; control, sampling, rule evaluation, and smooth transitions stay
in this measured native Agent process.

The absent-device five-minute case remains part of the release-hardware matrix.
It is not a blocker for the V2 engineering handoff, but it must be recorded for
each production USB design before a public hardware release.

## Post-reconnect resource-growth probe

The final V2 Agent was measured before and after 30 forced USB rediscovery
cycles against the real sensor-relay board. Every cycle restored a valid sample;
the slowest recovery was 348 ms.

| Metric | Before | After | Delta |
| --- | ---: | ---: | ---: |
| Handles | 188 | 188 | 0 |
| Threads | 11 | 11 | 0 |
| Working set | 13.531 MB | 13.852 MB | 0.320 MB |
| Private memory | 2.344 MB | 2.578 MB | 0.234 MB |

Event sequence gaps and malformed frames remained zero. This short stress probe
shows no serial-handle or worker-thread leak; the seven-day production soak
remains a public-launch gate.
