use lumi_protocol::{
    decode_frame, encode_frame, Capability, DeviceInfo, DeviceStatus, EventEnvelope, Message,
    ProtocolError, RelaySetParams, RelayStatus, RequestEnvelope, ResponseEnvelope, SensorSample,
    StreamConfigureParams, WireError, COMMAND_DEVICE_GET_STATUS, COMMAND_DEVICE_HELLO,
    COMMAND_RELAY_SET, COMMAND_STREAM_CONFIGURE, EVENT_DEVICE_STATUS, EVENT_SENSOR_SAMPLE,
    MAX_FRAME_BYTES,
};
use serialport::{ClearBuffer, SerialPort, SerialPortInfo, SerialPortType};
use std::collections::VecDeque;
use std::fmt;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

pub const DEFAULT_BAUD_RATE: u32 = 115_200;
pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbId {
    pub vid: u16,
    pub pid: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortKind {
    Usb {
        id: UsbId,
        serial_number: Option<String>,
        manufacturer: Option<String>,
        product: Option<String>,
    },
    Bluetooth,
    Pci,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortCandidate {
    pub name: String,
    pub kind: PortKind,
}

impl From<SerialPortInfo> for PortCandidate {
    fn from(info: SerialPortInfo) -> Self {
        let kind = match info.port_type {
            SerialPortType::UsbPort(usb) => PortKind::Usb {
                id: UsbId {
                    vid: usb.vid,
                    pid: usb.pid,
                },
                serial_number: usb.serial_number,
                manufacturer: usb.manufacturer,
                product: usb.product,
            },
            SerialPortType::BluetoothPort => PortKind::Bluetooth,
            SerialPortType::PciPort => PortKind::Pci,
            SerialPortType::Unknown => PortKind::Unknown,
        };
        Self {
            name: info.port_name,
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryPolicy {
    pub allowed_usb_ids: Vec<UsbId>,
    pub preferred_port: Option<String>,
    pub probe_all_usb_when_allowlist_empty: bool,
    pub probe_non_usb_ports: bool,
}

impl Default for DiscoveryPolicy {
    fn default() -> Self {
        Self {
            allowed_usb_ids: Vec::new(),
            preferred_port: None,
            probe_all_usb_when_allowlist_empty: true,
            probe_non_usb_ports: false,
        }
    }
}

impl DiscoveryPolicy {
    pub fn accepts(&self, candidate: &PortCandidate) -> bool {
        if self
            .preferred_port
            .as_deref()
            .is_some_and(|port| port.eq_ignore_ascii_case(&candidate.name))
        {
            return true;
        }
        match candidate.kind {
            PortKind::Usb { id, .. } => {
                (self.allowed_usb_ids.is_empty() && self.probe_all_usb_when_allowlist_empty)
                    || self.allowed_usb_ids.contains(&id)
            }
            _ => self.probe_non_usb_ports,
        }
    }

    pub fn sort_candidates(&self, candidates: &mut [PortCandidate]) {
        candidates.sort_by_key(|candidate| {
            let preferred = self
                .preferred_port
                .as_deref()
                .is_some_and(|port| port.eq_ignore_ascii_case(&candidate.name));
            let usb = matches!(candidate.kind, PortKind::Usb { .. });
            (!preferred, !usb, candidate.name.clone())
        });
    }
}

pub trait DevicePort: Send {
    fn name(&self) -> &str;
    fn prepare_session(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }
    fn write_frame(&mut self, frame: &[u8]) -> Result<(), DeviceError>;
    fn read_frame(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError>;
}

pub trait DevicePortProvider: Send + Sync {
    fn candidates(&self) -> Result<Vec<PortCandidate>, DeviceError>;
    fn open(&self, candidate: &PortCandidate) -> Result<Box<dyn DevicePort>, DeviceError>;
}

#[derive(Clone, Debug)]
pub struct SerialPortProvider {
    pub baud_rate: u32,
}

impl Default for SerialPortProvider {
    fn default() -> Self {
        Self {
            baud_rate: DEFAULT_BAUD_RATE,
        }
    }
}

impl DevicePortProvider for SerialPortProvider {
    fn candidates(&self) -> Result<Vec<PortCandidate>, DeviceError> {
        Ok(serialport::available_ports()
            .map_err(DeviceError::Serial)?
            .into_iter()
            .map(PortCandidate::from)
            .collect())
    }

    fn open(&self, candidate: &PortCandidate) -> Result<Box<dyn DevicePort>, DeviceError> {
        let port = serialport::new(&candidate.name, self.baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(DeviceError::Serial)?;
        Ok(Box::new(SerialDevicePort::new(
            candidate.name.clone(),
            port,
        )))
    }
}

struct SerialDevicePort {
    name: String,
    port: Box<dyn SerialPort>,
    framer: LineFramer,
}

impl SerialDevicePort {
    fn new(name: String, port: Box<dyn SerialPort>) -> Self {
        Self {
            name,
            port,
            framer: LineFramer::new(),
        }
    }
}

impl DevicePort for SerialDevicePort {
    fn name(&self) -> &str {
        &self.name
    }

    fn prepare_session(&mut self) -> Result<(), DeviceError> {
        self.port
            .clear(ClearBuffer::Input)
            .map_err(DeviceError::Serial)?;
        std::thread::sleep(Duration::from_millis(25));
        self.port
            .clear(ClearBuffer::Input)
            .map_err(DeviceError::Serial)?;
        self.framer.buffer.clear();
        Ok(())
    }

    fn write_frame(&mut self, frame: &[u8]) -> Result<(), DeviceError> {
        self.port.write_all(frame)?;
        self.port.flush()?;
        Ok(())
    }

    fn read_frame(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
        if let Some(frame) = self.framer.take_frame() {
            return Ok(Some(frame));
        }
        let deadline = Instant::now() + timeout;
        let mut buffer = [0u8; 256];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.port
                .set_timeout(
                    deadline
                        .saturating_duration_since(now)
                        .min(Duration::from_millis(100)),
                )
                .map_err(DeviceError::Serial)?;
            match self.port.read(&mut buffer) {
                Ok(0) => continue,
                Ok(count) => {
                    self.framer.push(&buffer[..count])?;
                    if let Some(frame) = self.framer.take_frame() {
                        return Ok(Some(frame));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => return Err(DeviceError::Io(error)),
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct LineFramer {
    buffer: Vec<u8>,
}

impl LineFramer {
    fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(MAX_FRAME_BYTES),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        self.buffer.extend_from_slice(bytes);
        if self.buffer.len() > MAX_FRAME_BYTES && !self.buffer.contains(&b'\n') {
            let actual = self.buffer.len();
            self.buffer.clear();
            return Err(DeviceError::Protocol(ProtocolError::FrameTooLarge {
                actual,
                maximum: MAX_FRAME_BYTES,
            }));
        }
        Ok(())
    }

    fn take_frame(&mut self) -> Option<Vec<u8>> {
        let newline = self.buffer.iter().position(|byte| *byte == b'\n')?;
        let mut frame = self.buffer.drain(..=newline).collect::<Vec<_>>();
        while frame.last().is_some_and(u8::is_ascii_whitespace) {
            frame.pop();
        }
        Some(frame)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeviceEvent {
    SensorSample {
        seq: u32,
        uptime_ms: u64,
        sample: SensorSample,
        sequence_gap: Option<SequenceGap>,
    },
    Status {
        seq: u32,
        uptime_ms: u64,
        status: DeviceStatus,
        sequence_gap: Option<SequenceGap>,
    },
    Unknown(EventEnvelope),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceGap {
    pub expected: u32,
    pub received: u32,
}

#[derive(Clone, Debug, Default)]
struct SequenceTracker {
    last: Option<u32>,
}

impl SequenceTracker {
    fn observe(&mut self, sequence: u32) -> Option<SequenceGap> {
        let gap = self.last.and_then(|last| {
            let expected = last.wrapping_add(1);
            (sequence != expected).then_some(SequenceGap {
                expected,
                received: sequence,
            })
        });
        self.last = Some(sequence);
        gap
    }
}

pub struct ConnectedDevice {
    port: Box<dyn DevicePort>,
    info: DeviceInfo,
    next_request_id: u32,
    queued_events: VecDeque<DeviceEvent>,
    sequence: SequenceTracker,
}

impl ConnectedDevice {
    pub fn connect(mut port: Box<dyn DevicePort>, timeout: Duration) -> Result<Self, DeviceError> {
        port.prepare_session()?;
        let mut device = Self {
            port,
            info: DeviceInfo {
                product_id: String::new(),
                serial_number: String::new(),
                hardware_version: String::new(),
                firmware_version: String::new(),
                bootloader_version: String::new(),
                protocol_min: 0,
                protocol_max: 0,
                capabilities: Vec::new(),
            },
            next_request_id: 1,
            queued_events: VecDeque::new(),
            sequence: SequenceTracker::default(),
        };
        let response = device.transact_hello(timeout)?;
        let info: DeviceInfo = response.parse_result()?;
        info.validate()?;
        device.info = info;
        Ok(device)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn port_name(&self) -> &str {
        self.port.name()
    }

    pub fn configure_stream(
        &mut self,
        params: StreamConfigureParams,
    ) -> Result<StreamConfigureParams, DeviceError> {
        params.validate()?;
        let response =
            self.transact_raw(COMMAND_STREAM_CONFIGURE, &params, DEFAULT_COMMAND_TIMEOUT)?;
        Ok(response.parse_result()?)
    }

    pub fn get_status(&mut self) -> Result<DeviceStatus, DeviceError> {
        let response =
            self.transact_raw(COMMAND_DEVICE_GET_STATUS, &(), DEFAULT_COMMAND_TIMEOUT)?;
        let status: DeviceStatus = response.parse_result()?;
        status.relay.validate()?;
        Ok(status)
    }

    pub fn set_relay(&mut self, energized: bool) -> Result<RelayStatus, DeviceError> {
        if !self.info.supports(Capability::Relay) {
            return Err(DeviceError::MissingCapability(Capability::Relay));
        }
        let response = self.transact_raw(
            COMMAND_RELAY_SET,
            &RelaySetParams { energized },
            DEFAULT_COMMAND_TIMEOUT,
        )?;
        let status: RelayStatus = response.parse_result()?;
        status.validate()?;
        Ok(status)
    }

    pub fn poll(&mut self, timeout: Duration) -> Result<Option<DeviceEvent>, DeviceError> {
        if let Some(event) = self.queued_events.pop_front() {
            return Ok(Some(event));
        }
        let Some(frame) = self.port.read_frame(timeout)? else {
            return Ok(None);
        };
        match decode_frame(&frame)? {
            Message::Event(event) => Ok(Some(self.convert_event(event)?)),
            Message::Response(_) => Ok(None),
            Message::Request(request) => Err(DeviceError::UnexpectedRequest(request.command)),
        }
    }

    fn transact_raw<T: serde::Serialize>(
        &mut self,
        command: &str,
        params: &T,
        timeout: Duration,
    ) -> Result<ResponseEnvelope, DeviceError> {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let request = RequestEnvelope::new(id, command, params)?;
        let frame = encode_frame(&Message::Request(request))?;
        self.port.write_frame(&frame)?;
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(DeviceError::Timeout {
                    command: command.to_string(),
                    timeout,
                });
            }
            let remaining = deadline.saturating_duration_since(now);
            let Some(frame) = self
                .port
                .read_frame(remaining.min(Duration::from_millis(100)))?
            else {
                continue;
            };
            match decode_frame(&frame)? {
                Message::Response(response) if response.id == id => {
                    response.validate()?;
                    if response.ok {
                        return Ok(response);
                    }
                    return Err(DeviceError::Remote(
                        response
                            .error
                            .expect("validated error response has an error"),
                    ));
                }
                Message::Response(_) => continue,
                Message::Event(event) => {
                    let event = self.convert_event(event)?;
                    self.queued_events.push_back(event);
                }
                Message::Request(request) => {
                    return Err(DeviceError::UnexpectedRequest(request.command));
                }
            }
        }
    }

    fn transact_hello(&mut self, timeout: Duration) -> Result<ResponseEnvelope, DeviceError> {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let request = RequestEnvelope::new(id, COMMAND_DEVICE_HELLO, &())?;
        let frame = encode_frame(&Message::Request(request))?;
        self.port.write_frame(&frame)?;
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(DeviceError::Timeout {
                    command: COMMAND_DEVICE_HELLO.to_string(),
                    timeout,
                });
            }
            let remaining = deadline.saturating_duration_since(now);
            let Some(frame) = self
                .port
                .read_frame(remaining.min(Duration::from_millis(100)))?
            else {
                continue;
            };
            let message = match decode_frame(&frame) {
                Ok(message) => message,
                Err(_) => continue,
            };
            match message {
                Message::Response(response) if response.id == id => {
                    response.validate()?;
                    if response.ok {
                        return Ok(response);
                    }
                    return Err(DeviceError::Remote(
                        response
                            .error
                            .expect("validated error response has an error"),
                    ));
                }
                Message::Response(_) | Message::Request(_) => continue,
                Message::Event(event) => {
                    if let Ok(event) = self.convert_event(event) {
                        self.queued_events.push_back(event);
                    }
                }
            }
        }
    }

    fn convert_event(&mut self, event: EventEnvelope) -> Result<DeviceEvent, DeviceError> {
        let gap = self.sequence.observe(event.seq);
        match event.event.as_str() {
            EVENT_SENSOR_SAMPLE => {
                let sample: SensorSample = event.parse_data()?;
                sample.validate()?;
                Ok(DeviceEvent::SensorSample {
                    seq: event.seq,
                    uptime_ms: event.uptime_ms,
                    sample,
                    sequence_gap: gap,
                })
            }
            EVENT_DEVICE_STATUS => {
                let status: DeviceStatus = event.parse_data()?;
                status.relay.validate()?;
                Ok(DeviceEvent::Status {
                    seq: event.seq,
                    uptime_ms: event.uptime_ms,
                    status,
                    sequence_gap: gap,
                })
            }
            _ => Ok(DeviceEvent::Unknown(event)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryFailure {
    pub port_name: String,
    pub error: String,
}

pub struct DiscoveryOutcome {
    pub device: ConnectedDevice,
    pub candidate: PortCandidate,
    pub previous_failures: Vec<DiscoveryFailure>,
}

pub fn discover_device(
    provider: &dyn DevicePortProvider,
    policy: &DiscoveryPolicy,
    handshake_timeout: Duration,
) -> Result<DiscoveryOutcome, DeviceError> {
    let mut candidates = provider.candidates()?;
    policy.sort_candidates(&mut candidates);
    let mut failures = Vec::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| policy.accepts(candidate))
    {
        let result = provider
            .open(&candidate)
            .and_then(|port| ConnectedDevice::connect(port, handshake_timeout));
        match result {
            Ok(device) => {
                return Ok(DiscoveryOutcome {
                    device,
                    candidate,
                    previous_failures: failures,
                })
            }
            Err(error) => failures.push(DiscoveryFailure {
                port_name: candidate.name,
                error: error.to_string(),
            }),
        }
    }
    Err(DeviceError::DiscoveryFailed(failures))
}

#[derive(Clone, Debug)]
pub struct ReconnectBackoff {
    initial: Duration,
    maximum: Duration,
    next: Duration,
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new(Duration::from_millis(250), Duration::from_secs(5))
    }
}

impl ReconnectBackoff {
    pub fn new(initial: Duration, maximum: Duration) -> Self {
        let initial = initial.min(maximum);
        Self {
            initial,
            maximum,
            next: initial,
        }
    }

    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = self.next.saturating_mul(2).min(self.maximum);
        delay
    }

    pub fn reset(&mut self) {
        self.next = self.initial;
    }
}

#[derive(Debug)]
pub enum DeviceError {
    Io(std::io::Error),
    Serial(serialport::Error),
    Protocol(ProtocolError),
    Remote(WireError),
    MissingCapability(Capability),
    Timeout { command: String, timeout: Duration },
    UnexpectedResponse(u32),
    UnexpectedRequest(String),
    DiscoveryFailed(Vec<DiscoveryFailure>),
}

impl fmt::Display for DeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceError::Io(error) => write!(formatter, "device I/O failed: {error}"),
            DeviceError::Serial(error) => write!(formatter, "serial port failed: {error}"),
            DeviceError::Protocol(error) => write!(formatter, "device protocol failed: {error}"),
            DeviceError::Remote(error) => {
                write!(
                    formatter,
                    "device returned {:?}: {}",
                    error.code, error.message
                )
            }
            DeviceError::MissingCapability(capability) => {
                write!(formatter, "device does not support {capability:?}")
            }
            DeviceError::Timeout { command, timeout } => {
                write!(
                    formatter,
                    "{command} timed out after {} ms",
                    timeout.as_millis()
                )
            }
            DeviceError::UnexpectedResponse(id) => {
                write!(formatter, "received unexpected response ID {id}")
            }
            DeviceError::UnexpectedRequest(command) => {
                write!(formatter, "device sent unexpected request {command}")
            }
            DeviceError::DiscoveryFailed(failures) => {
                if failures.is_empty() {
                    write!(formatter, "no eligible serial ports were found")
                } else {
                    write!(
                        formatter,
                        "no Lumi device responded: {}",
                        failures
                            .iter()
                            .map(|failure| format!("{} ({})", failure.port_name, failure.error))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        }
    }
}

impl std::error::Error for DeviceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeviceError::Io(error) => Some(error),
            DeviceError::Serial(error) => Some(error),
            DeviceError::Protocol(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DeviceError {
    fn from(error: std::io::Error) -> Self {
        DeviceError::Io(error)
    }
}

impl From<ProtocolError> for DeviceError {
    fn from(error: ProtocolError) -> Self {
        DeviceError::Protocol(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumi_device_simulator::{SimulatedProfile, Simulator};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct SimulatorPort {
        name: String,
        simulator: Simulator,
        incoming: VecDeque<Vec<u8>>,
        inject_stale_response: bool,
    }

    impl SimulatorPort {
        fn new(profile: SimulatedProfile) -> Self {
            Self {
                name: "COM-SIM".to_string(),
                simulator: Simulator::new(profile, "SIM-0001"),
                incoming: VecDeque::new(),
                inject_stale_response: false,
            }
        }
    }

    impl DevicePort for SimulatorPort {
        fn name(&self) -> &str {
            &self.name
        }

        fn write_frame(&mut self, frame: &[u8]) -> Result<(), DeviceError> {
            let Message::Request(request) = decode_frame(frame)? else {
                return Err(DeviceError::UnexpectedRequest("non-request".to_string()));
            };
            if self.inject_stale_response && request.command != COMMAND_DEVICE_HELLO {
                self.inject_stale_response = false;
                let stale = ResponseEnvelope::success(request.id.wrapping_sub(1), &true)?;
                self.incoming
                    .push_back(encode_frame(&Message::Response(stale))?);
            }
            let response = self.simulator.response_message(request);
            self.incoming.push_back(encode_frame(&response)?);
            Ok(())
        }

        fn read_frame(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
            if let Some(frame) = self.incoming.pop_front() {
                Ok(Some(frame))
            } else {
                std::thread::sleep(timeout.min(Duration::from_millis(1)));
                Ok(None)
            }
        }
    }

    struct FakeProvider {
        profile: SimulatedProfile,
        opens: AtomicUsize,
        candidates: Mutex<Vec<PortCandidate>>,
    }

    impl DevicePortProvider for FakeProvider {
        fn candidates(&self) -> Result<Vec<PortCandidate>, DeviceError> {
            Ok(self.candidates.lock().unwrap().clone())
        }

        fn open(&self, candidate: &PortCandidate) -> Result<Box<dyn DevicePort>, DeviceError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if candidate.name == "COM-BAD" {
                return Err(DeviceError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "simulated missing port",
                )));
            }
            Ok(Box::new(SimulatorPort::new(self.profile)))
        }
    }

    fn usb_candidate(name: &str) -> PortCandidate {
        PortCandidate {
            name: name.to_string(),
            kind: PortKind::Usb {
                id: UsbId {
                    vid: 0x303a,
                    pid: 0x1001,
                },
                serial_number: Some("SIM-0001".to_string()),
                manufacturer: Some("Lumi".to_string()),
                product: Some("Lumi Sensor".to_string()),
            },
        }
    }

    #[test]
    fn persistent_connection_handles_multiple_commands_on_one_open_port() {
        let mut device = ConnectedDevice::connect(
            Box::new(SimulatorPort::new(SimulatedProfile::SensorRelay)),
            Duration::from_millis(50),
        )
        .unwrap();
        assert_eq!(device.info().serial_number, "SIM-0001");
        assert!(device.set_relay(true).unwrap().energized.unwrap());
        assert!(device.get_status().unwrap().relay.energized.unwrap());
    }

    #[test]
    fn sensor_only_device_fails_relay_locally_without_sending_command() {
        let mut device = ConnectedDevice::connect(
            Box::new(SimulatorPort::new(SimulatedProfile::Sensor)),
            Duration::from_millis(50),
        )
        .unwrap();
        assert!(matches!(
            device.set_relay(true),
            Err(DeviceError::MissingCapability(Capability::Relay))
        ));
    }

    #[test]
    fn handshake_discards_a_partial_old_frame_and_queues_valid_old_events() {
        let mut port = SimulatorPort::new(SimulatedProfile::Sensor);
        port.incoming.push_back(b"sample\":42}".to_vec());
        let event = Message::Event(port.simulator.sample_event());
        port.incoming.push_back(encode_frame(&event).unwrap());

        let mut device = ConnectedDevice::connect(Box::new(port), Duration::from_millis(50))
            .expect("hello response after stale frames should be accepted");
        assert_eq!(device.info().serial_number, "SIM-0001");
        assert!(matches!(
            device.poll(Duration::ZERO).unwrap(),
            Some(DeviceEvent::SensorSample { .. })
        ));
    }

    #[test]
    fn persistent_commands_ignore_a_late_response_from_an_older_request() {
        let mut port = SimulatorPort::new(SimulatedProfile::SensorRelay);
        port.inject_stale_response = true;
        let mut device = ConnectedDevice::connect(Box::new(port), Duration::from_millis(50))
            .expect("handshake should succeed");
        assert_eq!(
            device
                .configure_stream(StreamConfigureParams {
                    ambient_lux_interval_ms: 500,
                    include_status_every: 4,
                })
                .unwrap()
                .ambient_lux_interval_ms,
            500
        );
    }

    #[test]
    fn discovery_filters_non_usb_and_returns_valid_handshake() {
        let provider = FakeProvider {
            profile: SimulatedProfile::Sensor,
            opens: AtomicUsize::new(0),
            candidates: Mutex::new(vec![
                PortCandidate {
                    name: "COM1".to_string(),
                    kind: PortKind::Unknown,
                },
                usb_candidate("COM-SIM"),
            ]),
        };
        let outcome = discover_device(
            &provider,
            &DiscoveryPolicy::default(),
            Duration::from_millis(50),
        )
        .unwrap();
        assert_eq!(outcome.device.info().product_id, "lumi-sensor");
        assert_eq!(provider.opens.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sequence_tracker_reports_gaps_and_accepts_wraparound() {
        let mut tracker = SequenceTracker::default();
        assert_eq!(tracker.observe(u32::MAX), None);
        assert_eq!(tracker.observe(0), None);
        assert_eq!(
            tracker.observe(2),
            Some(SequenceGap {
                expected: 1,
                received: 2
            })
        );
    }

    #[test]
    fn line_framer_handles_fragmentation_and_multiple_frames() {
        let mut framer = LineFramer::new();
        framer.push(b"{\"a\":").unwrap();
        assert!(framer.take_frame().is_none());
        framer.push(b"1}\n{\"b\":2}\n").unwrap();
        assert_eq!(framer.take_frame().unwrap(), b"{\"a\":1}");
        assert_eq!(framer.take_frame().unwrap(), b"{\"b\":2}");
    }

    #[test]
    fn reconnect_backoff_caps_and_resets() {
        let mut backoff = ReconnectBackoff::default();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
        for _ in 0..10 {
            backoff.next_delay();
        }
        assert_eq!(backoff.next_delay(), Duration::from_secs(5));
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
    }
}
