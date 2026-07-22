# Protocol V2 Firmware Validation - 2026-07-17

Hardware under test:

- ESP32-C3 SuperMini, revision 0.4, 4 MB flash
- device identifiers omitted from this public report
- BH1750/GY-302 on GPIO4/GPIO5
- active-low relay module on GPIO10

Build environment:

- Arduino ESP32 core 3.3.10
- ArduinoJson 7.4.3, pinned by compile-time check
- FQBN `esp32:esp32:nologo_esp32c3_super_mini:FlashMode=dio`

Both compile-time profiles built successfully from the same source:

| Profile | Product ID | Capabilities | Sketch bytes | Static RAM |
| --- | --- | --- | ---: | ---: |
| sensor | `lumi-sensor` | `ambient_lux` | 330,972 | 16,476 |
| sensor-relay | `lumi-sensor-relay` | `ambient_lux`, `relay` | 331,524 | 16,476 |

Hardware contract results:

- `device.hello`, `stream.configure`, `device.get_status`, and telemetry passed.
- Sensor profile reported `relay.available=false`; relay testing was rejected by
  the host before a command was sent.
- Relay profile reported observed energized state for both `true` and `false`.
- Valid BH1750 samples arrived at the configured 500 ms interval.
- A periodic `device.status` event arrived after four samples.
- Ten consecutive close/reopen cycles all completed hello, status, and sample.
- Event sequence and device uptime increased monotonically across reconnects.
- A 12-second host absence, over twice the 5-second firmware watchdog period,
  caused no reset.
- Malformed-frame count remained zero throughout the final reconnect test.
- Flash upload verification passed for every written segment.

Issues found and corrected during HIL:

1. A new host could begin reading in the middle of telemetry left by a previous
   session. The host now clears input on open and tolerates pre-handshake frames.
2. Late responses from old request IDs caused a reconnect failure. Transactions
   now accept only their matching response and discard stale responses.
3. Continuous CDC telemetry under host backpressure caused a watchdog reset.
   Firmware now bounds TX waits, uses a 512-byte queue, and drops telemetry when
   a complete frame cannot be queued. Sequence gaps expose any dropped event.

Final board state: sensor-relay profile, firmware 2.0.0, relay de-energized.
