use lumi_device_simulator::{SimulatedProfile, Simulator, SimulatorFaults};
use lumi_protocol::{decode_frame, encode_frame, Message};
use std::io::{self, BufRead, Write};
use std::time::Duration;

#[derive(Clone, Debug)]
struct Options {
    profile: SimulatedProfile,
    serial: String,
    lux: f64,
    delay_ms: u64,
    sensor_error: bool,
    malformed_after: Option<u64>,
    disconnect_after: Option<u64>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            profile: SimulatedProfile::SensorRelay,
            serial: "SIM-0001".to_string(),
            lux: 67.5,
            delay_ms: 0,
            sensor_error: false,
            malformed_after: None,
            disconnect_after: None,
        }
    }
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--profile" => {
                options.profile = match args.next().as_deref() {
                    Some("sensor") => SimulatedProfile::Sensor,
                    Some("relay") => SimulatedProfile::SensorRelay,
                    _ => return Err("--profile must be sensor or relay".to_string()),
                };
            }
            "--serial" => {
                options.serial = args
                    .next()
                    .ok_or_else(|| "--serial requires a value".to_string())?;
            }
            "--lux" => {
                options.lux = args
                    .next()
                    .ok_or_else(|| "--lux requires a value".to_string())?
                    .parse()
                    .map_err(|_| "--lux requires a number".to_string())?;
            }
            "--delay-ms" => {
                options.delay_ms = args
                    .next()
                    .ok_or_else(|| "--delay-ms requires a value".to_string())?
                    .parse()
                    .map_err(|_| "--delay-ms requires an integer".to_string())?;
            }
            "--sensor-error" => options.sensor_error = true,
            "--malformed-after" => {
                options.malformed_after = Some(
                    args.next()
                        .ok_or_else(|| "--malformed-after requires a value".to_string())?
                        .parse()
                        .map_err(|_| "--malformed-after requires an integer".to_string())?,
                );
            }
            "--disconnect-after" => {
                options.disconnect_after = Some(
                    args.next()
                        .ok_or_else(|| "--disconnect-after requires a value".to_string())?
                        .parse()
                        .map_err(|_| "--disconnect-after requires an integer".to_string())?,
                );
            }
            "--help" | "-h" => {
                return Err("usage: lumi-device-simulator [--profile sensor|relay] [--serial ID] [--lux N] [--delay-ms N] [--sensor-error] [--malformed-after N] [--disconnect-after N]".to_string());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(options)
}

fn main() {
    let options = match parse_options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let faults = SimulatorFaults {
        response_delay: Duration::from_millis(options.delay_ms),
        sensor_error: options.sensor_error,
        malformed_after_messages: options.malformed_after,
        disconnect_after_messages: options.disconnect_after,
    };
    let mut simulator = Simulator::new(options.profile, options.serial).with_faults(faults);
    simulator.set_lux(options.lux);
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("stdin failed: {error}");
                std::process::exit(1);
            }
        };
        let request = match decode_frame(line.as_bytes()) {
            Ok(Message::Request(request)) => request,
            Ok(_) => continue,
            Err(error) => {
                eprintln!("invalid request: {error}");
                continue;
            }
        };
        if !simulator.faults().response_delay.is_zero() {
            std::thread::sleep(simulator.faults().response_delay);
        }
        let response = simulator.response_message(request);
        if simulator.should_emit_malformed() {
            let _ = stdout.write_all(b"{malformed\n");
        } else if let Ok(frame) = encode_frame(&response) {
            let _ = stdout.write_all(&frame);
        }
        let _ = stdout.flush();
        if simulator.should_disconnect() {
            break;
        }
    }
}
