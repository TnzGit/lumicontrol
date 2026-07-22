use lumi_protocol::{
    Capability, DeviceInfo, DeviceStatus, ErrorCode, EventEnvelope, Message, RelaySetParams,
    RelayStatus, RequestEnvelope, ResponseEnvelope, SampleQuality, SensorSample, SensorStatus,
    StreamConfigureParams, COMMAND_DEVICE_GET_STATUS, COMMAND_DEVICE_HELLO, COMMAND_DEVICE_REBOOT,
    COMMAND_RELAY_SET, COMMAND_STREAM_CONFIGURE, EVENT_DEVICE_STATUS, EVENT_SENSOR_SAMPLE,
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulatedProfile {
    Sensor,
    SensorRelay,
}

impl SimulatedProfile {
    pub fn product_id(self) -> &'static str {
        match self {
            SimulatedProfile::Sensor => "lumi-sensor",
            SimulatedProfile::SensorRelay => "lumi-sensor-relay",
        }
    }

    pub fn capabilities(self) -> Vec<Capability> {
        let mut capabilities = vec![Capability::AmbientLux];
        if self == SimulatedProfile::SensorRelay {
            capabilities.push(Capability::Relay);
        }
        capabilities
    }
}

#[derive(Clone, Debug)]
pub struct SimulatorFaults {
    pub response_delay: Duration,
    pub sensor_error: bool,
    pub malformed_after_messages: Option<u64>,
    pub disconnect_after_messages: Option<u64>,
}

impl Default for SimulatorFaults {
    fn default() -> Self {
        Self {
            response_delay: Duration::ZERO,
            sensor_error: false,
            malformed_after_messages: None,
            disconnect_after_messages: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Simulator {
    info: DeviceInfo,
    relay_energized: bool,
    stream: StreamConfigureParams,
    lux: f64,
    seq: u32,
    uptime_ms: u64,
    malformed_frames: u32,
    handled_messages: u64,
    faults: SimulatorFaults,
}

impl Simulator {
    pub fn new(profile: SimulatedProfile, serial_number: impl Into<String>) -> Self {
        Self {
            info: DeviceInfo {
                product_id: profile.product_id().to_string(),
                serial_number: serial_number.into(),
                hardware_version: "sim-1".to_string(),
                firmware_version: "2.0.0-sim".to_string(),
                bootloader_version: "1.0.0-sim".to_string(),
                protocol_min: 2,
                protocol_max: 2,
                capabilities: profile.capabilities(),
            },
            relay_energized: false,
            stream: StreamConfigureParams {
                ambient_lux_interval_ms: 1000,
                include_status_every: 30,
            },
            lux: 67.5,
            seq: 0,
            uptime_ms: 0,
            malformed_frames: 0,
            handled_messages: 0,
            faults: SimulatorFaults::default(),
        }
    }

    pub fn with_faults(mut self, faults: SimulatorFaults) -> Self {
        self.faults = faults;
        self
    }

    pub fn set_lux(&mut self, lux: f64) {
        self.lux = lux;
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn faults(&self) -> &SimulatorFaults {
        &self.faults
    }

    pub fn should_disconnect(&self) -> bool {
        self.faults
            .disconnect_after_messages
            .is_some_and(|limit| self.handled_messages >= limit)
    }

    pub fn should_emit_malformed(&self) -> bool {
        self.faults
            .malformed_after_messages
            .is_some_and(|limit| self.handled_messages >= limit)
    }

    pub fn handle(&mut self, request: RequestEnvelope) -> ResponseEnvelope {
        self.handled_messages = self.handled_messages.saturating_add(1);
        if request.protocol != 2 {
            return ResponseEnvelope::failure(
                request.id,
                ErrorCode::UnsupportedProtocol,
                "simulator supports protocol 2",
            );
        }
        match request.command.as_str() {
            COMMAND_DEVICE_HELLO => ResponseEnvelope::success(request.id, &self.info)
                .expect("device info always serializes"),
            COMMAND_DEVICE_GET_STATUS => ResponseEnvelope::success(request.id, &self.status())
                .expect("device status always serializes"),
            COMMAND_STREAM_CONFIGURE => {
                let params = match request.parse_params::<StreamConfigureParams>() {
                    Ok(params) => params,
                    Err(error) => {
                        return ResponseEnvelope::failure(
                            request.id,
                            ErrorCode::InvalidRequest,
                            error.to_string(),
                        )
                    }
                };
                if let Err(error) = params.validate() {
                    return ResponseEnvelope::failure(
                        request.id,
                        ErrorCode::InvalidParameter,
                        error.to_string(),
                    );
                }
                self.stream = params.clone();
                ResponseEnvelope::success(request.id, &params)
                    .expect("stream configuration always serializes")
            }
            COMMAND_RELAY_SET => {
                if !self.info.supports(Capability::Relay) {
                    return ResponseEnvelope::failure(
                        request.id,
                        ErrorCode::UnsupportedCapability,
                        "relay is not installed",
                    );
                }
                let params = match request.parse_params::<RelaySetParams>() {
                    Ok(params) => params,
                    Err(error) => {
                        return ResponseEnvelope::failure(
                            request.id,
                            ErrorCode::InvalidRequest,
                            error.to_string(),
                        )
                    }
                };
                self.relay_energized = params.energized;
                ResponseEnvelope::success(
                    request.id,
                    &RelayStatus {
                        available: true,
                        energized: Some(self.relay_energized),
                    },
                )
                .expect("relay status always serializes")
            }
            COMMAND_DEVICE_REBOOT => ResponseEnvelope::success(
                request.id,
                &serde_json::json!({
                    "rebooting": true
                }),
            )
            .expect("reboot response always serializes"),
            _ => ResponseEnvelope::failure(
                request.id,
                ErrorCode::UnsupportedCommand,
                "command is not supported",
            ),
        }
    }

    pub fn sample_event(&mut self) -> EventEnvelope {
        self.seq = self.seq.wrapping_add(1);
        self.uptime_ms = self
            .uptime_ms
            .saturating_add(self.stream.ambient_lux_interval_ms as u64);
        let sample = if self.faults.sensor_error {
            SensorSample {
                lux: None,
                quality: SampleQuality::ReadError,
            }
        } else {
            SensorSample {
                lux: Some(self.lux),
                quality: SampleQuality::Valid,
            }
        };
        EventEnvelope::new(EVENT_SENSOR_SAMPLE, self.seq, self.uptime_ms, &sample)
            .expect("sensor sample always serializes")
    }

    pub fn status_event(&mut self) -> EventEnvelope {
        self.seq = self.seq.wrapping_add(1);
        EventEnvelope::new(
            EVENT_DEVICE_STATUS,
            self.seq,
            self.uptime_ms,
            &self.status(),
        )
        .expect("device status always serializes")
    }

    pub fn status(&self) -> DeviceStatus {
        DeviceStatus {
            sensor: SensorStatus {
                healthy: !self.faults.sensor_error,
                lux: (!self.faults.sensor_error).then_some(self.lux),
                sample_age_ms: Some(0),
            },
            relay: if self.info.supports(Capability::Relay) {
                RelayStatus {
                    available: true,
                    energized: Some(self.relay_energized),
                }
            } else {
                RelayStatus {
                    available: false,
                    energized: None,
                }
            },
            uptime_ms: self.uptime_ms,
            reset_reason: "simulated".to_string(),
            malformed_frames: self.malformed_frames,
        }
    }

    pub fn response_message(&mut self, request: RequestEnvelope) -> Message {
        Message::Response(self.handle(request))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumi_protocol::{ErrorCode, Message};

    #[test]
    fn sensor_profile_rejects_relay_command_by_capability() {
        let mut simulator = Simulator::new(SimulatedProfile::Sensor, "SIM-SENSOR");
        let request =
            RequestEnvelope::new(8, COMMAND_RELAY_SET, &RelaySetParams { energized: true })
                .unwrap();
        let response = simulator.handle(request);
        assert!(!response.ok);
        assert_eq!(
            response.error.unwrap().code,
            ErrorCode::UnsupportedCapability
        );
    }

    #[test]
    fn relay_profile_reports_observed_energized_state() {
        let mut simulator = Simulator::new(SimulatedProfile::SensorRelay, "SIM-RELAY");
        let request =
            RequestEnvelope::new(9, COMMAND_RELAY_SET, &RelaySetParams { energized: true })
                .unwrap();
        let response = simulator.handle(request);
        let status: RelayStatus = response.parse_result().unwrap();
        assert_eq!(status.energized, Some(true));
    }

    #[test]
    fn simulated_sample_is_a_valid_protocol_event() {
        let mut simulator = Simulator::new(SimulatedProfile::Sensor, "SIM-SENSOR");
        let message = Message::Event(simulator.sample_event());
        message.validate().unwrap();
        let Message::Event(event) = message else {
            unreachable!();
        };
        let sample: SensorSample = event.parse_data().unwrap();
        sample.validate().unwrap();
        assert_eq!(sample.lux, Some(67.5));
    }
}
