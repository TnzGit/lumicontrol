# Lumi Device Protocol V2

Status: Implemented for the V2 engineering release candidate

## 1. Purpose

Protocol V2 provides one stable USB contract for the sensor-only and
sensor-plus-relay hardware profiles. It replaces COM-port assumptions and
unversioned ad hoc command parsing with discovery, identity, capabilities, typed
responses, and continuous telemetry.

## 2. Transport and framing

- USB CDC serial at 115200 baud for initial hardware.
- UTF-8 JSON, one object per line, terminated by LF.
- Maximum encoded frame size: 1024 bytes.
- Host and device ignore blank lines.
- Malformed or oversized frames are rejected and counted without rebooting.
- The Agent maintains one persistent connection. Opening a port for every sample
  or relay command is not supported behavior.

JSON lines remain intentionally inspectable for manufacturing and support. A
binary transport can be introduced only under a later protocol major version.

## 3. Message envelope

Host request:

```json
{"type":"request","protocol":2,"id":17,"command":"device.hello","params":{}}
```

Successful response:

```json
{"type":"response","protocol":2,"id":17,"ok":true,"result":{}}
```

Error response:

```json
{"type":"response","protocol":2,"id":17,"ok":false,"error":{"code":"unsupported_command","message":"command is not supported"}}
```

Device event:

```json
{"type":"event","protocol":2,"event":"sensor.sample","seq":341,"uptime_ms":912340,"data":{}}
```

Rules:

- Request IDs are unsigned 32-bit values selected by the host.
- A response repeats the request ID exactly.
- Sequence numbers increase for each event and wrap as unsigned 32-bit values.
- Unknown fields are ignored within a compatible protocol major version.
- Unknown commands receive `unsupported_command`; they are never silently
  ignored.

## 4. Discovery and handshake

The Agent enumerates likely USB serial ports using the product VID/PID and USB
serial number. It then sends `device.hello`. A port is accepted only after a
valid Lumi response. The engineering prototype allowlist is Espressif
`303A:1001`; production hardware must use the assigned LumiControl identity and
update the allowlist before customer release. Unrelated serial devices are never
probed.

Example result:

```json
{
  "product_id":"lumi-sensor-relay",
  "serial_number":"LC24000123",
  "hardware_version":"1.1",
  "firmware_version":"2.0.0",
  "bootloader_version":"1.0.0",
  "protocol_min":2,
  "protocol_max":2,
  "capabilities":["ambient_lux","relay"]
}
```

Required fields are immutable for the connection. `serial_number` is the stable
device key used for preferences, diagnostics, firmware compatibility, and
support. A COM port name is runtime metadata only and is never persisted as the
device identity.

## 5. Capabilities

Initial capability identifiers:

| Capability | Meaning |
| --- | --- |
| `ambient_lux` | device can emit validated ambient-light samples |
| `relay` | device can report and set relay coil energization |

The Agent must not infer `relay` from a GPIO field or product name. A command
that requires a missing capability returns `unsupported_capability`.

## 6. Commands

### 6.1 `device.hello`

Returns identity, versions, and capabilities. Safe to retry.

### 6.2 `device.get_status`

Returns current health and state:

```json
{
  "sensor":{"healthy":true,"lux":67.5,"sample_age_ms":120},
  "relay":{"available":true,"energized":false},
  "uptime_ms":912340,
  "reset_reason":"power_on",
  "malformed_frames":0
}
```

The `relay` object is present with `available:false` on sensor-only hardware so
diagnostics can distinguish an absent capability from a communication failure.

### 6.3 `stream.configure`

Parameters:

```json
{"ambient_lux_interval_ms":1000,"include_status_every":30}
```

- Allowed lux interval: 200 to 5000 ms.
- `include_status_every` is a sample count from 1 to 300.
- The configured stream applies until disconnect or reboot.
- Safe to retry with the same parameters.

### 6.4 `relay.set`

Requires `relay`.

```json
{"energized":true}
```

The response reports the observed state after applying the command. This command
controls coil energization, not the user's logical concept of light on/off. The
Agent maps the installation's NO/NC mode to `energized` and exposes logical light
state to the UI. Sending the current state is idempotent.

### 6.5 `device.reboot`

Used only by diagnostics and firmware workflows. The response is sent before
reboot. It is never issued by normal control logic.

## 7. Events

### 7.1 `sensor.sample`

```json
{
  "type":"event",
  "protocol":2,
  "event":"sensor.sample",
  "seq":341,
  "uptime_ms":912340,
  "data":{"lux":67.5,"quality":"valid"}
}
```

Allowed quality values:

- `valid`
- `saturated`
- `below_range`
- `read_error`

Invalid samples carry no fabricated lux value. The Agent retains the last valid
sample with its age and applies its own stale-data policy.

### 7.2 `device.status`

Periodic status event containing sensor health, relay energization when
available, reset reason, and diagnostic counters.

## 8. Error codes

| Code | Meaning |
| --- | --- |
| `invalid_request` | envelope or field type is invalid |
| `unsupported_protocol` | no compatible protocol major version |
| `unsupported_command` | command name is unknown |
| `unsupported_capability` | command is valid but unavailable on this SKU |
| `invalid_parameter` | parameter is outside its documented range |
| `busy` | device cannot perform the operation now; retry may succeed |
| `hardware_failure` | sensor or relay operation failed |
| `internal_error` | unexpected firmware failure |

Errors include a stable code and a concise diagnostic message. Software behavior
keys off the code, never localized message text.

## 9. Timing and reconnect behavior

- `device.hello` response target: 500 ms.
- normal command response target: 250 ms.
- Agent command timeout: 1500 ms unless command documentation states otherwise.
- The Agent retries only idempotent commands and uses a new request ID.
- Disconnect triggers exponential reconnect backoff from 250 ms to 5 seconds.
- A successful handshake resets backoff and event sequence tracking.
- DTR/RTS behavior must not cause an unintended production-device reset.

## 10. Compatibility

- Protocol major versions are breaking compatibility boundaries.
- Firmware advertises `protocol_min` and `protocol_max`.
- Agent selects the highest mutually supported major version.
- Application release metadata defines supported firmware ranges by product and
  hardware revision.
- The development Agent may include a read-only V1 adapter for current prototype
  firmware. Supported hardware requires V2.

## 11. Firmware requirements implied by V2

- Non-blocking sensor acquisition and serial command processing
- watchdog and reset-reason reporting
- stable factory-programmed product ID, hardware revision, and serial number
- relay capability configured by the manufacturing profile, not inferred at run
  time
- bounded input parser with no dynamic unbounded buffering
- firmware image version embedded in the build

Firmware update transport and image signing are specified separately. Normal V2
telemetry must continue to work without Wi-Fi or a cloud connection.
