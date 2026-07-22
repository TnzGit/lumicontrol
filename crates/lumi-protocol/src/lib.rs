use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_FRAME_BYTES: usize = 1024;

pub const COMMAND_DEVICE_HELLO: &str = "device.hello";
pub const COMMAND_DEVICE_GET_STATUS: &str = "device.get_status";
pub const COMMAND_STREAM_CONFIGURE: &str = "stream.configure";
pub const COMMAND_RELAY_SET: &str = "relay.set";
pub const COMMAND_DEVICE_REBOOT: &str = "device.reboot";

pub const EVENT_SENSOR_SAMPLE: &str = "sensor.sample";
pub const EVENT_DEVICE_STATUS: &str = "device.status";
pub const PRODUCT_SENSOR: &str = "lumi-sensor";
pub const PRODUCT_SENSOR_RELAY: &str = "lumi-sensor-relay";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    AmbientLux,
    Relay,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceInfo {
    pub product_id: String,
    pub serial_number: String,
    pub hardware_version: String,
    pub firmware_version: String,
    pub bootloader_version: String,
    pub protocol_min: u16,
    pub protocol_max: u16,
    pub capabilities: Vec<Capability>,
}

impl DeviceInfo {
    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn negotiated_protocol(&self) -> Option<u16> {
        negotiate_protocol(self.protocol_min, self.protocol_max)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !matches!(
            self.product_id.as_str(),
            PRODUCT_SENSOR | PRODUCT_SENSOR_RELAY
        ) {
            return Err(ProtocolError::InvalidPayload(format!(
                "unsupported Lumi product_id: {}",
                self.product_id
            )));
        }
        if self.serial_number.trim().is_empty() || self.serial_number.chars().count() > 64 {
            return Err(ProtocolError::InvalidPayload(
                "serial_number must contain 1..=64 characters".to_string(),
            ));
        }
        for version in [
            &self.hardware_version,
            &self.firmware_version,
            &self.bootloader_version,
        ] {
            if version.trim().is_empty() || version.chars().count() > 64 {
                return Err(ProtocolError::InvalidPayload(
                    "hardware, firmware, and bootloader versions must contain 1..=64 characters"
                        .to_string(),
                ));
            }
        }
        if self.protocol_min > self.protocol_max {
            return Err(ProtocolError::InvalidPayload(
                "protocol_min must not exceed protocol_max".to_string(),
            ));
        }
        if self.negotiated_protocol().is_none() {
            return Err(ProtocolError::UnsupportedProtocol {
                minimum: self.protocol_min,
                maximum: self.protocol_max,
            });
        }
        if !self.supports(Capability::AmbientLux) {
            return Err(ProtocolError::InvalidPayload(
                "supported Lumi devices must advertise ambient_lux".to_string(),
            ));
        }
        if self
            .capabilities
            .iter()
            .enumerate()
            .any(|(index, capability)| self.capabilities[index + 1..].contains(capability))
        {
            return Err(ProtocolError::InvalidPayload(
                "capabilities must not contain duplicates".to_string(),
            ));
        }
        let relay_expected = self.product_id == PRODUCT_SENSOR_RELAY;
        if self.supports(Capability::Relay) != relay_expected {
            return Err(ProtocolError::InvalidPayload(
                "product_id and relay capability do not match".to_string(),
            ));
        }
        Ok(())
    }
}

pub fn negotiate_protocol(device_min: u16, device_max: u16) -> Option<u16> {
    (device_min..=device_max)
        .rev()
        .find(|version| *version == PROTOCOL_VERSION)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RequestEnvelope {
    #[serde(rename = "type")]
    pub message_type: RequestMessageType,
    pub protocol: u16,
    pub id: u32,
    pub command: String,
    #[serde(default)]
    pub params: Value,
}

impl RequestEnvelope {
    pub fn new<T: Serialize>(
        id: u32,
        command: impl Into<String>,
        params: &T,
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            message_type: RequestMessageType::Request,
            protocol: PROTOCOL_VERSION,
            id,
            command: command.into(),
            params: serde_json::to_value(params)?,
        })
    }

    pub fn empty(id: u32, command: impl Into<String>) -> Self {
        Self {
            message_type: RequestMessageType::Request,
            protocol: PROTOCOL_VERSION,
            id,
            command: command.into(),
            params: Value::Object(Default::default()),
        }
    }

    pub fn parse_params<T: DeserializeOwned>(&self) -> Result<T, ProtocolError> {
        Ok(serde_json::from_value(self.params.clone())?)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedProtocol {
                minimum: self.protocol,
                maximum: self.protocol,
            });
        }
        if self.command.trim().is_empty() {
            return Err(ProtocolError::InvalidPayload(
                "command must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequestMessageType {
    Request,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ResponseEnvelope {
    #[serde(rename = "type")]
    pub message_type: ResponseMessageType,
    pub protocol: u16,
    pub id: u32,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
}

impl ResponseEnvelope {
    pub fn success<T: Serialize>(id: u32, result: &T) -> Result<Self, ProtocolError> {
        Ok(Self {
            message_type: ResponseMessageType::Response,
            protocol: PROTOCOL_VERSION,
            id,
            ok: true,
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub fn failure(id: u32, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            message_type: ResponseMessageType::Response,
            protocol: PROTOCOL_VERSION,
            id,
            ok: false,
            result: None,
            error: Some(WireError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn parse_result<T: DeserializeOwned>(&self) -> Result<T, ProtocolError> {
        self.validate()?;
        let result = self
            .result
            .clone()
            .ok_or_else(|| ProtocolError::InvalidPayload("response has no result".to_string()))?;
        Ok(serde_json::from_value(result)?)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedProtocol {
                minimum: self.protocol,
                maximum: self.protocol,
            });
        }
        match (self.ok, self.result.is_some(), self.error.is_some()) {
            (true, true, false) | (false, false, true) => Ok(()),
            _ => Err(ProtocolError::InvalidPayload(
                "response must contain exactly one of result or error matching ok".to_string(),
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResponseMessageType {
    Response,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub message_type: EventMessageType,
    pub protocol: u16,
    pub event: String,
    pub seq: u32,
    pub uptime_ms: u64,
    pub data: Value,
}

impl EventEnvelope {
    pub fn new<T: Serialize>(
        event: impl Into<String>,
        seq: u32,
        uptime_ms: u64,
        data: &T,
    ) -> Result<Self, ProtocolError> {
        Ok(Self {
            message_type: EventMessageType::Event,
            protocol: PROTOCOL_VERSION,
            event: event.into(),
            seq,
            uptime_ms,
            data: serde_json::to_value(data)?,
        })
    }

    pub fn parse_data<T: DeserializeOwned>(&self) -> Result<T, ProtocolError> {
        Ok(serde_json::from_value(self.data.clone())?)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedProtocol {
                minimum: self.protocol,
                maximum: self.protocol,
            });
        }
        if self.event.trim().is_empty() {
            return Err(ProtocolError::InvalidPayload(
                "event must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventMessageType {
    Event,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    Request(RequestEnvelope),
    Response(ResponseEnvelope),
    Event(EventEnvelope),
}

impl Message {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Message::Request(message) => message.validate(),
            Message::Response(message) => message.validate(),
            Message::Event(message) => message.validate(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedProtocol,
    UnsupportedCommand,
    UnsupportedCapability,
    InvalidParameter,
    Busy,
    HardwareFailure,
    InternalError,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamConfigureParams {
    pub ambient_lux_interval_ms: u32,
    pub include_status_every: u16,
}

impl StreamConfigureParams {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !(200..=5000).contains(&self.ambient_lux_interval_ms) {
            return Err(ProtocolError::InvalidPayload(
                "ambient_lux_interval_ms must be in 200..=5000".to_string(),
            ));
        }
        if !(1..=300).contains(&self.include_status_every) {
            return Err(ProtocolError::InvalidPayload(
                "include_status_every must be in 1..=300".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelaySetParams {
    pub energized: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SampleQuality {
    Valid,
    Saturated,
    BelowRange,
    ReadError,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SensorSample {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lux: Option<f64>,
    pub quality: SampleQuality,
}

impl SensorSample {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match (self.quality, self.lux) {
            (SampleQuality::Valid, Some(lux)) if lux.is_finite() && lux >= 0.0 => Ok(()),
            (SampleQuality::Valid, _) => Err(ProtocolError::InvalidPayload(
                "valid samples require a finite non-negative lux value".to_string(),
            )),
            (_, None) => Ok(()),
            (_, Some(_)) => Err(ProtocolError::InvalidPayload(
                "invalid samples must not contain a fabricated lux value".to_string(),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SensorStatus {
    pub healthy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lux: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_age_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayStatus {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energized: Option<bool>,
}

impl RelayStatus {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match (self.available, self.energized) {
            (true, Some(_)) | (false, None) => Ok(()),
            _ => Err(ProtocolError::InvalidPayload(
                "relay energized state must match availability".to_string(),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeviceStatus {
    pub sensor: SensorStatus,
    pub relay: RelayStatus,
    pub uptime_ms: u64,
    pub reset_reason: String,
    pub malformed_frames: u32,
}

pub fn encode_frame(message: &Message) -> Result<Vec<u8>, ProtocolError> {
    message.validate()?;
    let mut encoded = match message {
        Message::Request(message) => serde_json::to_vec(message)?,
        Message::Response(message) => serde_json::to_vec(message)?,
        Message::Event(message) => serde_json::to_vec(message)?,
    };
    if encoded.len() + 1 > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            actual: encoded.len() + 1,
            maximum: MAX_FRAME_BYTES,
        });
    }
    encoded.push(b'\n');
    Ok(encoded)
}

pub fn decode_frame(frame: &[u8]) -> Result<Message, ProtocolError> {
    if frame.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge {
            actual: frame.len(),
            maximum: MAX_FRAME_BYTES,
        });
    }
    let trimmed = trim_ascii_whitespace(frame);
    if trimmed.is_empty() {
        return Err(ProtocolError::EmptyFrame);
    }
    let value: Value = serde_json::from_slice(trimmed)?;
    let message_type = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| ProtocolError::InvalidPayload("message type is required".to_string()))?;
    let message = match message_type {
        "request" => Message::Request(serde_json::from_value(value)?),
        "response" => Message::Response(serde_json::from_value(value)?),
        "event" => Message::Event(serde_json::from_value(value)?),
        other => return Err(ProtocolError::UnknownMessageType(other.to_string())),
    };
    message.validate()?;
    Ok(message)
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[derive(Debug)]
pub enum ProtocolError {
    EmptyFrame,
    FrameTooLarge { actual: usize, maximum: usize },
    UnknownMessageType(String),
    UnsupportedProtocol { minimum: u16, maximum: u16 },
    InvalidPayload(String),
    Json(serde_json::Error),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::EmptyFrame => write!(formatter, "protocol frame is empty"),
            ProtocolError::FrameTooLarge { actual, maximum } => {
                write!(
                    formatter,
                    "protocol frame is {actual} bytes; maximum is {maximum}"
                )
            }
            ProtocolError::UnknownMessageType(value) => {
                write!(formatter, "unknown protocol message type: {value}")
            }
            ProtocolError::UnsupportedProtocol { minimum, maximum } => write!(
                formatter,
                "device protocol range {minimum}..={maximum} does not include {PROTOCOL_VERSION}"
            ),
            ProtocolError::InvalidPayload(message) => write!(formatter, "{message}"),
            ProtocolError::Json(error) => write!(formatter, "invalid protocol JSON: {error}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<serde_json::Error> for ProtocolError {
    fn from(error: serde_json::Error) -> Self {
        ProtocolError::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor_device() -> DeviceInfo {
        DeviceInfo {
            product_id: "lumi-sensor".to_string(),
            serial_number: "LC24000122".to_string(),
            hardware_version: "1.0".to_string(),
            firmware_version: "2.0.0".to_string(),
            bootloader_version: "1.0.0".to_string(),
            protocol_min: 2,
            protocol_max: 2,
            capabilities: vec![Capability::AmbientLux],
        }
    }

    #[test]
    fn request_round_trips_as_one_json_line() {
        let request = RequestEnvelope::empty(17, COMMAND_DEVICE_HELLO);
        let encoded = encode_frame(&Message::Request(request.clone())).unwrap();
        assert_eq!(encoded.last(), Some(&b'\n'));
        assert_eq!(decode_frame(&encoded).unwrap(), Message::Request(request));
    }

    #[test]
    fn response_shape_requires_result_xor_error() {
        let invalid = ResponseEnvelope {
            message_type: ResponseMessageType::Response,
            protocol: 2,
            id: 4,
            ok: true,
            result: None,
            error: None,
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn both_supported_profiles_validate_through_capabilities() {
        let sensor = sensor_device();
        sensor.validate().unwrap();
        assert!(!sensor.supports(Capability::Relay));

        let mut relay = sensor;
        relay.product_id = "lumi-sensor-relay".to_string();
        relay.capabilities.push(Capability::Relay);
        relay.validate().unwrap();
        assert!(relay.supports(Capability::Relay));
    }

    #[test]
    fn product_identity_must_match_the_capability_table() {
        let mut unknown = sensor_device();
        unknown.product_id = "unrelated-device".to_string();
        assert!(unknown.validate().is_err());

        let mut inconsistent = sensor_device();
        inconsistent.capabilities.push(Capability::Relay);
        assert!(inconsistent.validate().is_err());

        let mut duplicate = sensor_device();
        duplicate.capabilities.push(Capability::AmbientLux);
        assert!(duplicate.validate().is_err());
    }

    #[test]
    fn protocol_negotiation_rejects_incompatible_firmware() {
        let mut device = sensor_device();
        device.protocol_min = 3;
        device.protocol_max = 4;
        assert!(matches!(
            device.validate(),
            Err(ProtocolError::UnsupportedProtocol { .. })
        ));
    }

    #[test]
    fn valid_sample_requires_real_lux_and_errors_do_not_fabricate_it() {
        SensorSample {
            lux: Some(67.5),
            quality: SampleQuality::Valid,
        }
        .validate()
        .unwrap();
        assert!(SensorSample {
            lux: None,
            quality: SampleQuality::Valid,
        }
        .validate()
        .is_err());
        assert!(SensorSample {
            lux: Some(0.0),
            quality: SampleQuality::ReadError,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn stream_configuration_enforces_documented_limits() {
        StreamConfigureParams {
            ambient_lux_interval_ms: 1000,
            include_status_every: 30,
        }
        .validate()
        .unwrap();
        assert!(StreamConfigureParams {
            ambient_lux_interval_ms: 199,
            include_status_every: 30,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn oversized_frames_are_rejected_before_json_parsing() {
        let frame = vec![b'x'; MAX_FRAME_BYTES + 1];
        assert!(matches!(
            decode_frame(&frame),
            Err(ProtocolError::FrameTooLarge { .. })
        ));
    }
}
