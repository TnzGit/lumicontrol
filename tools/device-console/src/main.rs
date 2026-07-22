use lumi_device::{discover_device, DeviceEvent, DiscoveryPolicy, SerialPortProvider};
use lumi_protocol::{Capability, SampleQuality, StreamConfigureParams};
use std::error::Error;
use std::time::{Duration, Instant};

struct Arguments {
    port: Option<String>,
    samples: usize,
    relay: Option<bool>,
}

fn parse_arguments() -> Result<Arguments, String> {
    let mut port = None;
    let mut samples = 3usize;
    let mut relay = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--port" => port = Some(args.next().ok_or("--port requires a value")?),
            "--samples" => {
                samples = args
                    .next()
                    .ok_or("--samples requires a value")?
                    .parse()
                    .map_err(|_| "--samples must be a positive integer")?;
                if samples == 0 {
                    return Err("--samples must be positive".to_string());
                }
            }
            "--relay" => {
                relay = Some(match args.next().as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("--relay must be on or off".to_string()),
                });
            }
            "--help" | "-h" => {
                println!("Usage: lumi-device-console [--port COM3] [--samples 3] [--relay on|off]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Arguments {
        port,
        samples,
        relay,
    })
}

fn main() {
    if let Err(error) = run() {
        eprintln!("device check failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = parse_arguments()?;
    let provider = SerialPortProvider::default();
    let policy = DiscoveryPolicy {
        preferred_port: arguments.port,
        ..DiscoveryPolicy::default()
    };
    let mut outcome = discover_device(&provider, &policy, Duration::from_millis(1500))?;
    println!(
        "device={} port={}\n{}",
        outcome.device.info().serial_number,
        outcome.device.port_name(),
        serde_json::to_string_pretty(outcome.device.info())?
    );

    outcome.device.configure_stream(StreamConfigureParams {
        ambient_lux_interval_ms: 500,
        include_status_every: 4,
    })?;
    let status = outcome.device.get_status()?;
    println!("status={}", serde_json::to_string(&status)?);

    if let Some(energized) = arguments.relay {
        if !outcome.device.info().supports(Capability::Relay) {
            return Err("requested relay test on sensor-only hardware".into());
        }
        let observed = outcome.device.set_relay(energized)?;
        println!("relay={}", serde_json::to_string(&observed)?);
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut valid_samples = 0usize;
    while valid_samples < arguments.samples && Instant::now() < deadline {
        match outcome.device.poll(Duration::from_millis(750))? {
            Some(DeviceEvent::SensorSample {
                seq,
                uptime_ms,
                sample,
                sequence_gap,
            }) => {
                println!(
                    "sample seq={seq} uptime_ms={uptime_ms} quality={:?} lux={:?} gap={sequence_gap:?}",
                    sample.quality, sample.lux
                );
                if sample.quality == SampleQuality::Valid {
                    valid_samples += 1;
                }
            }
            Some(DeviceEvent::Status {
                seq,
                status,
                sequence_gap,
                ..
            }) => println!(
                "status-event seq={seq} gap={sequence_gap:?} data={}",
                serde_json::to_string(&status)?
            ),
            Some(DeviceEvent::Unknown(event)) => println!("unknown-event={}", event.event),
            None => {}
        }
    }
    if valid_samples != arguments.samples {
        return Err(format!(
            "received {valid_samples}/{} valid samples before timeout",
            arguments.samples
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_request_a_short_non_destructive_probe() {
        let arguments = Arguments {
            port: None,
            samples: 3,
            relay: None,
        };
        assert_eq!(arguments.samples, 3);
        assert!(arguments.relay.is_none());
    }
}
