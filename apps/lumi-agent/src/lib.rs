mod support;

pub use support::StartupRegistration;

use lumi_core::{
    evaluate_rules, map_normalized_lux_to_brightness, ConditionExpression, LightAction,
    LightCondition, LogLuxFilter, ManualOverrideGuard, RuleContext, TargetStabilizer,
};
use lumi_device::{
    discover_device, DeviceEvent, DevicePortProvider, DiscoveryPolicy, ReconnectBackoff,
    SerialPortProvider, UsbId,
};
use lumi_environment::{
    current_solar_context, fetch_open_meteo, SolarContext, WeatherObservation, WeatherRequest,
};
use lumi_ipc::{
    default_pipe_name, unix_millis, AgentCommand, AgentSnapshot, DeviceConnectionState,
    EnvironmentSnapshot, HealthLevel, IpcError, IpcErrorCode, IpcRequest, IpcResponse,
    IpcWireError, MonitorSnapshot, NamedPipeServer, RequestHandler, ResponsePayload,
};
use lumi_monitor_windows::{
    MonitorBackend, MonitorDescriptor, SchedulerEvent, TransitionScheduler, WindowsMonitorBackend,
};
use lumi_protocol::{Capability, DeviceInfo, DeviceStatus, RelayStatus, SampleQuality};
use lumi_store::{ProductPaths, SettingsDocument, SettingsStore, StoreError};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use support::{
    export_diagnostics, install_crash_hook, production_startup_registration,
    sample_process_resources, EventLogger,
};

const SENSOR_STREAM_INTERVAL_MS: u32 = 1_000;
const SENSOR_STALE_AFTER: Duration = Duration::from_secs(5);
const MONITOR_PROBE_INTERVAL: Duration = Duration::from_secs(30);
const AGENT_COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const WEATHER_RETRY_AFTER: Duration = Duration::from_secs(60);
const SOLAR_CACHE_FOR: Duration = Duration::from_secs(30);
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(60);
const ESP32_C3_USB_ID: UsbId = UsbId {
    vid: 0x303a,
    pid: 0x1001,
};

type CommandResult = Result<ResponsePayload, IpcWireError>;

pub struct AgentOptions {
    pub store: SettingsStore,
    pub legacy_config_path: PathBuf,
    pub monitor_backend: Arc<dyn MonitorBackend>,
    pub device_provider: Arc<dyn DevicePortProvider>,
    pub pipe_name: String,
    pub startup_registration: Arc<dyn StartupRegistration>,
    pub install_crash_hook: bool,
}

impl AgentOptions {
    pub fn production() -> Result<Self, AgentError> {
        let paths = ProductPaths::from_environment()?;
        let legacy_config_path = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("config.json")))
            .unwrap_or_else(|| PathBuf::from("config.json"));
        Ok(Self {
            store: SettingsStore::new(paths),
            legacy_config_path,
            monitor_backend: Arc::new(WindowsMonitorBackend),
            device_provider: Arc::new(SerialPortProvider::default()),
            pipe_name: default_pipe_name()?,
            startup_registration: production_startup_registration().map_err(AgentError::Startup)?,
            install_crash_hook: true,
        })
    }
}

#[derive(Clone)]
pub struct AgentHandle {
    tx: Sender<RuntimeMessage>,
    snapshots: Arc<SnapshotStore>,
    settings: Arc<RwLock<SettingsDocument>>,
    shutdown: Arc<ShutdownSignal>,
}

impl AgentHandle {
    pub fn snapshot(&self) -> AgentSnapshot {
        self.snapshots.get()
    }

    pub fn settings(&self) -> SettingsDocument {
        self.settings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn execute(&self, command: AgentCommand) -> CommandResult {
        match command {
            AgentCommand::Ping => Ok(ResponsePayload::Pong {
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            AgentCommand::GetSnapshot => Ok(ResponsePayload::Snapshot(Box::new(self.snapshot()))),
            AgentCommand::WaitForSnapshot {
                after_revision,
                timeout_ms,
            } => Ok(ResponsePayload::Snapshot(Box::new(
                self.snapshots.wait_for_revision(
                    after_revision,
                    Duration::from_millis(u64::from(timeout_ms.min(30_000))),
                ),
            ))),
            AgentCommand::GetSettings => Ok(ResponsePayload::Settings(Box::new(self.settings()))),
            AgentCommand::OpenUi => {
                launch_ui_process()?;
                Ok(ResponsePayload::Acknowledged)
            }
            command => {
                let timeout = if matches!(&command, AgentCommand::ExportDiagnostics) {
                    Duration::from_secs(15)
                } else {
                    AGENT_COMMAND_TIMEOUT
                };
                let (reply_tx, reply_rx) = mpsc::channel();
                self.tx
                    .send(RuntimeMessage::Command {
                        command,
                        reply: reply_tx,
                    })
                    .map_err(|_| wire_error(IpcErrorCode::Internal, "Agent has stopped"))?;
                reply_rx.recv_timeout(timeout).map_err(|error| {
                    let message = match error {
                        RecvTimeoutError::Timeout => "Agent command timed out",
                        RecvTimeoutError::Disconnected => "Agent command channel closed",
                    };
                    wire_error(IpcErrorCode::Timeout, message)
                })?
            }
        }
    }

    pub fn wait_for_shutdown(&self) {
        self.shutdown.wait();
    }

    fn request_shutdown(&self) {
        let _ = self.execute(AgentCommand::Shutdown);
    }
}

pub struct AgentProcess {
    handle: AgentHandle,
    pipe_server: Option<NamedPipeServer>,
    runtime_join: Option<JoinHandle<()>>,
}

impl AgentProcess {
    pub fn start(options: AgentOptions) -> Result<Self, AgentError> {
        if options.install_crash_hook {
            install_crash_hook(&options.store.paths);
        }
        let logger = EventLogger::best_effort(&options.store.paths);
        logger.info("agent_starting", "Agent process is starting");
        let load_outcome = options
            .store
            .load_or_import_v1_with_recovery(&options.legacy_config_path)?;
        let mut document = load_outcome.document;
        document.normalize();
        document.validate()?;

        let snapshots = Arc::new(SnapshotStore::new(document.settings.paused));
        if let Some(warning) = load_outcome.warning {
            logger.warn(
                "settings_recovered",
                "Settings were recovered from the last valid backup",
            );
            snapshots.update(|snapshot| snapshot.configuration_warning = Some(warning));
        }
        if let Err(error) = options
            .startup_registration
            .set_enabled(document.settings.start_at_login)
        {
            logger.warn("startup_registration_failed", &error);
            snapshots.update(|snapshot| {
                let message = format!("Start at login could not be applied: {error}");
                snapshot.configuration_warning =
                    Some(match snapshot.configuration_warning.take() {
                        Some(existing) => format!("{existing}; {message}"),
                        None => message,
                    });
            });
        }
        let settings = Arc::new(RwLock::new(document.clone()));
        let shutdown = Arc::new(ShutdownSignal::default());
        let (runtime_tx, runtime_rx) = mpsc::channel();
        let system_watcher = SystemWatcher::spawn(runtime_tx.clone())?;

        let (monitor_event_tx, monitor_event_rx) = mpsc::channel();
        let scheduler = TransitionScheduler::new(options.monitor_backend, monitor_event_tx);
        let monitor_runtime_tx = runtime_tx.clone();
        let monitor_forward_join = thread::Builder::new()
            .name("lumi-monitor-events".to_string())
            .spawn(move || {
                while let Ok(event) = monitor_event_rx.recv() {
                    if monitor_runtime_tx
                        .send(RuntimeMessage::Monitor(event))
                        .is_err()
                    {
                        break;
                    }
                }
            })?;

        let (device_command_tx, device_command_rx) = mpsc::channel();
        let device_runtime_tx = runtime_tx.clone();
        let device_join = spawn_device_worker(
            options.device_provider,
            device_command_rx,
            device_runtime_tx,
        )?;

        let thread_snapshots = Arc::clone(&snapshots);
        let thread_settings = Arc::clone(&settings);
        let thread_shutdown = Arc::clone(&shutdown);
        let store = options.store;
        let startup_registration = options.startup_registration;
        let runtime_logger = logger.clone();
        let core_runtime_tx = runtime_tx.clone();
        let runtime_join = thread::Builder::new()
            .name("lumi-agent-core".to_string())
            .spawn(move || {
                let services = RuntimeServices {
                    store,
                    startup_registration,
                    runtime_logger,
                    scheduler,
                    device_command_tx,
                    core_runtime_tx,
                };
                let shared = RuntimeSharedState {
                    thread_snapshots,
                    thread_settings,
                    thread_shutdown,
                };
                let runtime = AgentRuntime::new(document, services, shared);
                runtime.run(
                    runtime_rx,
                    device_join,
                    monitor_forward_join,
                    system_watcher,
                );
            })?;

        let handle = AgentHandle {
            tx: runtime_tx,
            snapshots,
            settings,
            shutdown,
        };
        let ipc_handle = handle.clone();
        let handler: Arc<RequestHandler> = Arc::new(move |request: IpcRequest| {
            let id = request.id;
            match ipc_handle.execute(request.command) {
                Ok(payload) => IpcResponse::success(id, payload),
                Err(error) => IpcResponse::failure(id, error.code, error.message),
            }
        });
        let pipe_server = match NamedPipeServer::bind(options.pipe_name, handler) {
            Ok(server) => server,
            Err(error) => {
                handle.request_shutdown();
                let _ = runtime_join.join();
                return Err(AgentError::Ipc(error));
            }
        };

        Ok(Self {
            handle,
            pipe_server: Some(pipe_server),
            runtime_join: Some(runtime_join),
        })
    }

    pub fn handle(&self) -> AgentHandle {
        self.handle.clone()
    }

    pub fn pipe_name(&self) -> Option<&str> {
        self.pipe_server.as_ref().map(NamedPipeServer::pipe_name)
    }

    pub fn wait(&self) {
        self.handle.wait_for_shutdown();
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.pipe_server.take();
        if !self.handle.shutdown.is_set() {
            self.handle.request_shutdown();
        }
        if let Some(join) = self.runtime_join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Default)]
struct ShutdownSignal {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl ShutdownSignal {
    fn signal(&self) {
        *self
            .stopped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.wake.notify_all();
    }

    fn wait(&self) {
        let mut stopped = self
            .stopped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !*stopped {
            stopped = self
                .wake
                .wait(stopped)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn is_set(&self) -> bool {
        *self
            .stopped
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

struct SnapshotState {
    snapshot: AgentSnapshot,
    last_sensor_unix_ms: Option<u64>,
    last_resource_sample: Instant,
}

struct SnapshotStore {
    state: Mutex<SnapshotState>,
    changed: Condvar,
    started: Instant,
}

impl SnapshotStore {
    fn new(paused: bool) -> Self {
        let started = Instant::now();
        let mut snapshot = AgentSnapshot {
            paused,
            ..AgentSnapshot::default()
        };
        snapshot.resources = sample_process_resources();
        Self {
            state: Mutex::new(SnapshotState {
                snapshot,
                last_sensor_unix_ms: None,
                last_resource_sample: started,
            }),
            changed: Condvar::new(),
            started,
        }
    }

    fn get(&self) -> AgentSnapshot {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.materialize(&mut state)
    }

    fn update(&self, update: impl FnOnce(&mut AgentSnapshot)) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut state.snapshot);
        state.snapshot.revision = state.snapshot.revision.wrapping_add(1);
        state.snapshot.generated_at_unix_ms = unix_millis();
        state.snapshot.resources.uptime_seconds = self.started.elapsed().as_secs();
        recompute_health(&mut state.snapshot);
        self.changed.notify_all();
    }

    fn update_if(&self, update: impl FnOnce(&mut AgentSnapshot) -> bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !update(&mut state.snapshot) {
            return;
        }
        state.snapshot.revision = state.snapshot.revision.wrapping_add(1);
        state.snapshot.generated_at_unix_ms = unix_millis();
        state.snapshot.resources.uptime_seconds = self.started.elapsed().as_secs();
        recompute_health(&mut state.snapshot);
        self.changed.notify_all();
    }

    fn record_sensor_sample(&self, update: impl FnOnce(&mut AgentSnapshot)) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        update(&mut state.snapshot);
        state.last_sensor_unix_ms = Some(unix_millis());
        state.snapshot.sensor.sample_age_ms = Some(0);
        state.snapshot.revision = state.snapshot.revision.wrapping_add(1);
        state.snapshot.generated_at_unix_ms = unix_millis();
        state.snapshot.resources.uptime_seconds = self.started.elapsed().as_secs();
        recompute_health(&mut state.snapshot);
        self.changed.notify_all();
    }

    fn wait_for_revision(&self, after_revision: u64, timeout: Duration) -> AgentSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (mut state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.snapshot.revision <= after_revision
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.materialize(&mut state)
    }

    fn materialize(&self, state: &mut SnapshotState) -> AgentSnapshot {
        if state.last_resource_sample.elapsed() >= RESOURCE_SAMPLE_INTERVAL {
            let elapsed_ms = state.last_resource_sample.elapsed().as_millis() as u64;
            let previous_cpu_ms = state.snapshot.resources.cpu_time_ms;
            let mut resources = sample_process_resources();
            if let (Some(previous), Some(current)) = (previous_cpu_ms, resources.cpu_time_ms) {
                resources.cpu_usage_basis_points =
                    Some(cpu_usage_basis_points(previous, current, elapsed_ms));
            }
            resources.uptime_seconds = self.started.elapsed().as_secs();
            state.snapshot.resources = resources;
            state.last_resource_sample = Instant::now();
        }
        let mut snapshot = state.snapshot.clone();
        snapshot.generated_at_unix_ms = unix_millis();
        snapshot.resources.uptime_seconds = self.started.elapsed().as_secs();
        snapshot.sensor.sample_age_ms = state
            .last_sensor_unix_ms
            .map(|sampled| unix_millis().saturating_sub(sampled));
        snapshot
    }
}

fn cpu_usage_basis_points(previous_cpu_ms: u64, current_cpu_ms: u64, elapsed_ms: u64) -> u32 {
    current_cpu_ms
        .saturating_sub(previous_cpu_ms)
        .saturating_mul(10_000)
        .checked_div(elapsed_ms.max(1))
        .unwrap_or(0)
        .min(u64::from(u32::MAX)) as u32
}

fn recompute_health(snapshot: &mut AgentSnapshot) {
    let qualified_monitor = snapshot.monitors.iter().any(|monitor| monitor.qualified);
    if snapshot.paused {
        snapshot.health = HealthLevel::Healthy;
        snapshot.status_message = "Automatic control paused".to_string();
    } else if !qualified_monitor {
        snapshot.health = HealthLevel::Fault;
        snapshot.status_message = "No compatible DDC/CI monitor".to_string();
    } else if snapshot.device.state != DeviceConnectionState::Connected {
        snapshot.health = HealthLevel::Degraded;
        snapshot.status_message = "Looking for Lumi sensor".to_string();
    } else if !snapshot.sensor.valid {
        snapshot.health = HealthLevel::Degraded;
        snapshot.status_message = "Waiting for a valid light reading".to_string();
    } else if snapshot
        .monitors
        .iter()
        .any(|monitor| monitor.last_error.is_some())
    {
        snapshot.health = HealthLevel::Degraded;
        snapshot.status_message = "A monitor needs attention".to_string();
    } else if snapshot.environment.configured && snapshot.environment.last_error.is_some() {
        snapshot.health = HealthLevel::Degraded;
        snapshot.status_message = "Weather data is temporarily unavailable".to_string();
    } else {
        snapshot.health = HealthLevel::Healthy;
        snapshot.status_message = "Automatic control is working".to_string();
    }
}

enum RuntimeMessage {
    Command {
        command: AgentCommand,
        reply: Sender<CommandResult>,
    },
    Device(Box<DeviceWorkerEvent>),
    Environment(EnvironmentWorkerEvent),
    Monitor(SchedulerEvent),
    System(SystemEvent),
}

struct EnvironmentWorkerEvent {
    generation: u64,
    result: Result<WeatherObservation, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SystemEvent {
    DisplayChanged,
    Suspend,
    Resume,
}

enum DeviceCommand {
    SetRelay {
        energized: bool,
        light_on: bool,
        reply: Sender<CommandResult>,
    },
    Suspend,
    Refresh,
    Shutdown,
}

enum DeviceWorkerEvent {
    Discovering,
    Connected {
        info: DeviceInfo,
        port_name: String,
        status: Option<DeviceStatus>,
    },
    Sample(DeviceEvent),
    Status(DeviceStatus),
    BackingOff {
        delay: Duration,
        error: String,
    },
    RelayResult {
        light_on: bool,
        result: Result<RelayStatus, String>,
        reply: Sender<CommandResult>,
    },
}

fn spawn_device_worker(
    provider: Arc<dyn DevicePortProvider>,
    commands: Receiver<DeviceCommand>,
    runtime_tx: Sender<RuntimeMessage>,
) -> Result<JoinHandle<()>, std::io::Error> {
    thread::Builder::new()
        .name("lumi-usb-device".to_string())
        .spawn(move || run_device_worker(provider, commands, runtime_tx))
}

fn run_device_worker(
    provider: Arc<dyn DevicePortProvider>,
    commands: Receiver<DeviceCommand>,
    runtime_tx: Sender<RuntimeMessage>,
) {
    let mut backoff = ReconnectBackoff::default();
    let policy = agent_discovery_policy();
    let mut suspended = false;
    'outer: loop {
        if suspended {
            match wait_suspended_command(&commands) {
                DeviceWaitOutcome::Retry => {
                    suspended = false;
                    backoff.reset();
                }
                DeviceWaitOutcome::Suspended => continue,
                DeviceWaitOutcome::Shutdown => break,
            }
        }
        if runtime_tx
            .send(RuntimeMessage::Device(Box::new(
                DeviceWorkerEvent::Discovering,
            )))
            .is_err()
        {
            break;
        }
        match discover_device(provider.as_ref(), &policy, Duration::from_millis(750)) {
            Ok(mut outcome) => {
                backoff.reset();
                let configured =
                    outcome
                        .device
                        .configure_stream(lumi_protocol::StreamConfigureParams {
                            ambient_lux_interval_ms: SENSOR_STREAM_INTERVAL_MS,
                            include_status_every: 30,
                        });
                if let Err(error) = configured {
                    let delay = backoff.next_delay();
                    let _ = runtime_tx.send(RuntimeMessage::Device(Box::new(
                        DeviceWorkerEvent::BackingOff {
                            delay,
                            error: error.to_string(),
                        },
                    )));
                    match wait_disconnected_command(&commands, delay) {
                        DeviceWaitOutcome::Retry => {}
                        DeviceWaitOutcome::Suspended => suspended = true,
                        DeviceWaitOutcome::Shutdown => break,
                    }
                    continue;
                }
                let info = outcome.device.info().clone();
                let port_name = outcome.device.port_name().to_string();
                let status = outcome.device.get_status().ok();
                if runtime_tx
                    .send(RuntimeMessage::Device(Box::new(
                        DeviceWorkerEvent::Connected {
                            info,
                            port_name,
                            status,
                        },
                    )))
                    .is_err()
                {
                    break;
                }

                loop {
                    while let Ok(command) = commands.try_recv() {
                        match command {
                            DeviceCommand::SetRelay {
                                energized,
                                light_on,
                                reply,
                            } => {
                                let result = outcome
                                    .device
                                    .set_relay(energized)
                                    .map_err(|error| error.to_string());
                                let _ = runtime_tx.send(RuntimeMessage::Device(Box::new(
                                    DeviceWorkerEvent::RelayResult {
                                        light_on,
                                        result,
                                        reply,
                                    },
                                )));
                            }
                            DeviceCommand::Suspend => {
                                suspended = true;
                                continue 'outer;
                            }
                            DeviceCommand::Refresh => continue 'outer,
                            DeviceCommand::Shutdown => break 'outer,
                        }
                    }
                    match outcome.device.poll(Duration::from_millis(250)) {
                        Ok(Some(event @ DeviceEvent::SensorSample { .. })) => {
                            if runtime_tx
                                .send(RuntimeMessage::Device(Box::new(DeviceWorkerEvent::Sample(
                                    event,
                                ))))
                                .is_err()
                            {
                                break 'outer;
                            }
                        }
                        Ok(Some(DeviceEvent::Status { status, .. })) => {
                            let _ = runtime_tx.send(RuntimeMessage::Device(Box::new(
                                DeviceWorkerEvent::Status(status),
                            )));
                        }
                        Ok(Some(DeviceEvent::Unknown(_))) | Ok(None) => {}
                        Err(error) => {
                            let delay = backoff.next_delay();
                            let _ = runtime_tx.send(RuntimeMessage::Device(Box::new(
                                DeviceWorkerEvent::BackingOff {
                                    delay,
                                    error: error.to_string(),
                                },
                            )));
                            match wait_disconnected_command(&commands, delay) {
                                DeviceWaitOutcome::Retry => {}
                                DeviceWaitOutcome::Suspended => suspended = true,
                                DeviceWaitOutcome::Shutdown => break 'outer,
                            }
                            continue 'outer;
                        }
                    }
                }
            }
            Err(error) => {
                let delay = backoff.next_delay();
                let _ = runtime_tx.send(RuntimeMessage::Device(Box::new(
                    DeviceWorkerEvent::BackingOff {
                        delay,
                        error: error.to_string(),
                    },
                )));
                match wait_disconnected_command(&commands, delay) {
                    DeviceWaitOutcome::Retry => {}
                    DeviceWaitOutcome::Suspended => suspended = true,
                    DeviceWaitOutcome::Shutdown => break,
                }
            }
        }
    }
}

fn agent_discovery_policy() -> DiscoveryPolicy {
    DiscoveryPolicy {
        allowed_usb_ids: vec![ESP32_C3_USB_ID],
        probe_all_usb_when_allowlist_empty: false,
        ..DiscoveryPolicy::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceWaitOutcome {
    Retry,
    Suspended,
    Shutdown,
}

fn wait_disconnected_command(
    commands: &Receiver<DeviceCommand>,
    delay: Duration,
) -> DeviceWaitOutcome {
    match commands.recv_timeout(delay) {
        Ok(DeviceCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
            DeviceWaitOutcome::Shutdown
        }
        Ok(DeviceCommand::SetRelay { reply, .. }) => {
            let _ = reply.send(Err(wire_error(
                IpcErrorCode::HardwareUnavailable,
                "Lumi device is disconnected",
            )));
            DeviceWaitOutcome::Retry
        }
        Ok(DeviceCommand::Suspend) => DeviceWaitOutcome::Suspended,
        Ok(DeviceCommand::Refresh) | Err(RecvTimeoutError::Timeout) => DeviceWaitOutcome::Retry,
    }
}

fn wait_suspended_command(commands: &Receiver<DeviceCommand>) -> DeviceWaitOutcome {
    match commands.recv() {
        Ok(DeviceCommand::Shutdown) | Err(_) => DeviceWaitOutcome::Shutdown,
        Ok(DeviceCommand::SetRelay { reply, .. }) => {
            let _ = reply.send(Err(wire_error(
                IpcErrorCode::HardwareUnavailable,
                "Lumi device is suspended",
            )));
            DeviceWaitOutcome::Suspended
        }
        Ok(DeviceCommand::Suspend) => DeviceWaitOutcome::Suspended,
        Ok(DeviceCommand::Refresh) => DeviceWaitOutcome::Retry,
    }
}

struct MonitorControlState {
    guard: ManualOverrideGuard,
    current: Option<i32>,
    target: Option<i32>,
    transition_active: bool,
}

struct SolarCache {
    latitude_bits: u64,
    longitude_bits: u64,
    timezone: String,
    computed_at: Instant,
    context: SolarContext,
}

struct RuntimeServices {
    store: SettingsStore,
    startup_registration: Arc<dyn StartupRegistration>,
    runtime_logger: EventLogger,
    scheduler: TransitionScheduler,
    device_command_tx: Sender<DeviceCommand>,
    core_runtime_tx: Sender<RuntimeMessage>,
}

struct RuntimeSharedState {
    thread_snapshots: Arc<SnapshotStore>,
    thread_settings: Arc<RwLock<SettingsDocument>>,
    thread_shutdown: Arc<ShutdownSignal>,
}

struct AgentRuntime {
    document: SettingsDocument,
    store: SettingsStore,
    startup_registration: Arc<dyn StartupRegistration>,
    logger: EventLogger,
    scheduler: TransitionScheduler,
    device_tx: Sender<DeviceCommand>,
    runtime_tx: Sender<RuntimeMessage>,
    snapshots: Arc<SnapshotStore>,
    shared_settings: Arc<RwLock<SettingsDocument>>,
    shutdown: Arc<ShutdownSignal>,
    filter: LogLuxFilter,
    stabilizer: TargetStabilizer,
    monitor_state: BTreeMap<String, MonitorControlState>,
    capabilities: Vec<Capability>,
    last_sensor: Option<Instant>,
    last_target: Option<i32>,
    last_probe: Instant,
    origin: Instant,
    reconnect_count: u64,
    solar_cache: Option<SolarCache>,
    weather_observation: Option<WeatherObservation>,
    weather_observed_at_unix_ms: Option<u64>,
    solar_error: Option<String>,
    weather_error: Option<String>,
    weather_fetch_in_flight: bool,
    weather_next_refresh: Instant,
    weather_generation: u64,
    suspended: bool,
}

impl AgentRuntime {
    fn new(
        document: SettingsDocument,
        services: RuntimeServices,
        shared: RuntimeSharedState,
    ) -> Self {
        let filter = LogLuxFilter::new(document.settings.control.filter)
            .expect("validated settings contain a valid filter");
        let stabilizer = TargetStabilizer::new(document.settings.control.target_deadband);
        Self {
            document,
            store: services.store,
            startup_registration: services.startup_registration,
            logger: services.runtime_logger,
            scheduler: services.scheduler,
            device_tx: services.device_command_tx,
            runtime_tx: services.core_runtime_tx,
            snapshots: shared.thread_snapshots,
            shared_settings: shared.thread_settings,
            shutdown: shared.thread_shutdown,
            filter,
            stabilizer,
            monitor_state: BTreeMap::new(),
            capabilities: Vec::new(),
            last_sensor: None,
            last_target: None,
            last_probe: Instant::now(),
            origin: Instant::now(),
            reconnect_count: 0,
            solar_cache: None,
            weather_observation: None,
            weather_observed_at_unix_ms: None,
            solar_error: None,
            weather_error: None,
            weather_fetch_in_flight: false,
            weather_next_refresh: Instant::now(),
            weather_generation: 0,
            suspended: false,
        }
    }

    fn run(
        mut self,
        rx: Receiver<RuntimeMessage>,
        device_join: JoinHandle<()>,
        monitor_forward_join: JoinHandle<()>,
        system_watcher: SystemWatcher,
    ) {
        self.logger.info("agent_ready", "Agent runtime is ready");
        self.snapshots.update(|snapshot| {
            snapshot.environment.configured = self.document.settings.weather.enabled;
        });
        self.refresh_monitors();
        let mut running = true;
        while running {
            let timeout = self
                .next_deadline()
                .saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(RuntimeMessage::Command { command, reply }) => {
                    running = self.handle_command(command, reply);
                }
                Ok(RuntimeMessage::Device(event)) => self.handle_device_event(*event),
                Ok(RuntimeMessage::Environment(event)) => {
                    self.handle_environment_event(event);
                }
                Ok(RuntimeMessage::Monitor(event)) => self.handle_monitor_event(event),
                Ok(RuntimeMessage::System(event)) => self.handle_system_event(event),
                Err(RecvTimeoutError::Timeout) => self.handle_deadlines(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = self.device_tx.send(DeviceCommand::Shutdown);
        let _ = device_join.join();
        drop(self.scheduler);
        let _ = monitor_forward_join.join();
        drop(system_watcher);
        self.logger
            .info("agent_stopped", "Agent runtime stopped cleanly");
        self.shutdown.signal();
    }

    fn next_deadline(&self) -> Instant {
        if self.suspended {
            return Instant::now() + Duration::from_secs(60);
        }
        let probe = self.last_probe + MONITOR_PROBE_INTERVAL;
        let sensor_or_probe = self
            .last_sensor
            .map(|sample| sample + SENSOR_STALE_AFTER)
            .map_or(probe, |stale| stale.min(probe));
        if self.weather_is_needed() && !self.weather_fetch_in_flight {
            sensor_or_probe.min(self.weather_next_refresh)
        } else {
            sensor_or_probe
        }
    }

    fn handle_deadlines(&mut self) {
        if self.suspended {
            return;
        }
        let now = Instant::now();
        if self
            .last_sensor
            .is_some_and(|sample| now.duration_since(sample) >= SENSOR_STALE_AFTER)
        {
            self.last_sensor = None;
            self.snapshots
                .update(|snapshot| snapshot.sensor.valid = false);
        }
        if now.duration_since(self.last_probe) >= MONITOR_PROBE_INTERVAL {
            self.last_probe = now;
            for descriptor in self.scheduler.descriptors() {
                let _ = self.scheduler.read_now(&descriptor.id);
            }
        }
        self.maybe_schedule_weather();
    }

    fn handle_system_event(&mut self, event: SystemEvent) {
        match event {
            SystemEvent::DisplayChanged => {
                if self.suspended {
                    return;
                }
                self.logger
                    .info("display_changed", "Display topology changed");
                self.refresh_monitors();
            }
            SystemEvent::Suspend => {
                if self.suspended {
                    return;
                }
                self.logger.info("system_suspend", "System is suspending");
                self.suspended = true;
                self.last_sensor = None;
                let _ = self.device_tx.send(DeviceCommand::Suspend);
                self.snapshots.update(|snapshot| {
                    snapshot.sensor.valid = false;
                    snapshot.target_percent = None;
                    snapshot.device.state = DeviceConnectionState::Disconnected;
                    snapshot.relay.available = false;
                    snapshot.relay.energized = None;
                    snapshot.relay.light_on = None;
                    for monitor in &mut snapshot.monitors {
                        monitor.transition_active = false;
                    }
                });
            }
            SystemEvent::Resume => {
                if !self.suspended {
                    return;
                }
                self.logger.info("system_resume", "System resumed");
                self.suspended = false;
                self.last_sensor = None;
                self.filter.reset();
                self.stabilizer.reset();
                self.last_target = None;
                self.snapshots.update(|snapshot| {
                    snapshot.sensor.valid = false;
                    snapshot.target_percent = None;
                    snapshot.device.state = DeviceConnectionState::Discovering;
                });
                self.refresh_monitors();
                let _ = self.device_tx.send(DeviceCommand::Refresh);
            }
        }
    }

    fn handle_command(&mut self, command: AgentCommand, reply: Sender<CommandResult>) -> bool {
        match command {
            AgentCommand::SaveSettings { document } => {
                let mut document = *document;
                document.normalize();
                let result = self.replace_settings(document);
                let _ = reply.send(result.map(|_| ResponsePayload::Acknowledged));
            }
            AgentCommand::SetPaused { paused } => {
                let mut document = self.document.clone();
                document.settings.paused = paused;
                let result = self.replace_settings(document);
                let _ = reply.send(result.map(|_| ResponsePayload::Acknowledged));
            }
            AgentCommand::RunNow => {
                self.apply_last_target();
                self.evaluate_relay_rules();
                let _ = reply.send(Ok(ResponsePayload::Acknowledged));
            }
            AgentCommand::RefreshHardware => {
                if self.suspended {
                    let _ = reply.send(Err(wire_error(
                        IpcErrorCode::HardwareUnavailable,
                        "Hardware refresh is unavailable while the system is suspended",
                    )));
                } else {
                    self.refresh_monitors();
                    let _ = self.device_tx.send(DeviceCommand::Refresh);
                    let _ = reply.send(Ok(ResponsePayload::Acknowledged));
                }
            }
            AgentCommand::SetLight { light_on } => self.request_light(light_on, reply),
            AgentCommand::ClearManualOverride { monitor_id } => {
                let now_ms = self.now_ms();
                for (id, state) in &mut self.monitor_state {
                    if monitor_id.as_ref().is_none_or(|selected| selected == id) {
                        state.guard.clear();
                    }
                }
                self.update_override_snapshots(now_ms);
                self.apply_last_target();
                let _ = reply.send(Ok(ResponsePayload::Acknowledged));
            }
            AgentCommand::ExportDiagnostics => {
                let paths = self.store.paths.clone();
                let snapshot = self.snapshots.get();
                let document = self.document.clone();
                let logger = self.logger.clone();
                let worker_reply = reply.clone();
                let spawn = thread::Builder::new()
                    .name("lumi-diagnostics-export".to_string())
                    .spawn(move || {
                        let result = match export_diagnostics(&paths, &snapshot, &document, &logger)
                        {
                            Ok(path) => {
                                logger.info(
                                    "diagnostics_exported",
                                    "A diagnostic package was exported",
                                );
                                Ok(ResponsePayload::DiagnosticsExported {
                                    path: path.display().to_string(),
                                })
                            }
                            Err(error) => {
                                logger.error(
                                    "diagnostics_export_failed",
                                    "A diagnostic package could not be exported",
                                );
                                Err(wire_error(IpcErrorCode::Internal, error))
                            }
                        };
                        let _ = worker_reply.send(result);
                    });
                if let Err(error) = spawn {
                    let _ = reply.send(Err(wire_error(
                        IpcErrorCode::Internal,
                        format!("Could not start diagnostic export: {error}"),
                    )));
                }
            }
            AgentCommand::OpenUi => {
                let _ = reply.send(Ok(ResponsePayload::Acknowledged));
            }
            AgentCommand::Shutdown => {
                let _ = reply.send(Ok(ResponsePayload::Acknowledged));
                return false;
            }
            AgentCommand::Ping
            | AgentCommand::GetSnapshot
            | AgentCommand::WaitForSnapshot { .. }
            | AgentCommand::GetSettings => {
                let _ = reply.send(Err(wire_error(
                    IpcErrorCode::InvalidRequest,
                    "command should have been handled by the IPC facade",
                )));
            }
        }
        true
    }

    fn replace_settings(&mut self, document: SettingsDocument) -> Result<(), IpcWireError> {
        document
            .validate()
            .map_err(|error| wire_error(IpcErrorCode::InvalidSettings, error.to_string()))?;
        if document.settings.weather.enabled {
            current_solar_context(
                document.settings.weather.latitude,
                document.settings.weather.longitude,
                &document.settings.weather.timezone,
            )
            .map_err(|error| {
                wire_error(
                    IpcErrorCode::InvalidSettings,
                    format!("Invalid weather and solar settings: {error}"),
                )
            })?;
        }
        if !self.capabilities.is_empty()
            && !self.capabilities.contains(&Capability::Relay)
            && document.settings.relay.rules_enabled
            && document
                .settings
                .relay
                .rules
                .iter()
                .any(|rule| rule.enabled && rule.then != LightAction::Keep)
        {
            return Err(wire_error(
                IpcErrorCode::UnsupportedCapability,
                "Connected Lumi Sensor has no relay capability",
            ));
        }
        let weather_changed = self.document.settings.weather != document.settings.weather;
        let previous_start_at_login = self.document.settings.start_at_login;
        let startup_changed = previous_start_at_login != document.settings.start_at_login;
        if startup_changed {
            self.startup_registration
                .set_enabled(document.settings.start_at_login)
                .map_err(|error| {
                    wire_error(
                        IpcErrorCode::Internal,
                        format!("Could not update start at login: {error}"),
                    )
                })?;
        }
        if let Err(error) = self.store.save_settings(&document) {
            let mut message = error.to_string();
            if startup_changed {
                if let Err(rollback) = self
                    .startup_registration
                    .set_enabled(previous_start_at_login)
                {
                    message.push_str(&format!("; startup rollback also failed: {rollback}"));
                    self.logger.error("startup_rollback_failed", &rollback);
                }
            }
            return Err(wire_error(IpcErrorCode::Internal, message));
        }
        self.filter = LogLuxFilter::new(document.settings.control.filter)
            .map_err(|error| wire_error(IpcErrorCode::InvalidSettings, error.to_string()))?;
        self.stabilizer = TargetStabilizer::new(document.settings.control.target_deadband);
        let override_config = document.settings.control.manual_override;
        for state in self.monitor_state.values_mut() {
            state.guard = ManualOverrideGuard::new(override_config);
        }
        if weather_changed {
            self.weather_generation = self.weather_generation.wrapping_add(1);
            self.weather_fetch_in_flight = false;
            self.weather_observation = None;
            self.weather_observed_at_unix_ms = None;
            self.weather_error = None;
            self.solar_error = None;
            self.solar_cache = None;
        }
        if self.weather_observation.is_none() {
            self.weather_next_refresh = Instant::now();
        }
        self.document = document.clone();
        *self
            .shared_settings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = document;
        self.snapshots.update(|snapshot| {
            snapshot.paused = self.document.settings.paused;
            snapshot.relay.rules_enabled = self.document.settings.relay.rules_enabled;
            snapshot.environment.configured = self.document.settings.weather.enabled;
            if weather_changed {
                snapshot.environment = EnvironmentSnapshot {
                    configured: self.document.settings.weather.enabled,
                    ..EnvironmentSnapshot::default()
                };
            }
        });
        self.apply_last_target();
        self.evaluate_relay_rules();
        self.logger.info("settings_saved", "Settings were updated");
        Ok(())
    }

    fn handle_device_event(&mut self, event: DeviceWorkerEvent) {
        if self.suspended {
            if let DeviceWorkerEvent::RelayResult { reply, .. } = event {
                let _ = reply.send(Err(wire_error(
                    IpcErrorCode::HardwareUnavailable,
                    "Lumi device is suspended",
                )));
            }
            return;
        }
        match event {
            DeviceWorkerEvent::Discovering => {
                self.snapshots.update(|snapshot| {
                    snapshot.device.state = DeviceConnectionState::Discovering;
                });
            }
            DeviceWorkerEvent::Connected {
                info,
                port_name,
                status,
            } => {
                self.reconnect_count = self.reconnect_count.saturating_add(1);
                self.capabilities = info.capabilities.clone();
                let negotiated_protocol = info.negotiated_protocol();
                self.logger.info(
                    "device_connected",
                    format!(
                        "Device connected with {} capabilities; reconnect count {}",
                        info.capabilities.len(),
                        self.reconnect_count
                    ),
                );
                self.snapshots.update(|snapshot| {
                    snapshot.device.state = DeviceConnectionState::Connected;
                    snapshot.device.product_id = Some(info.product_id);
                    snapshot.device.serial_number = Some(info.serial_number);
                    snapshot.device.hardware_version = Some(info.hardware_version);
                    snapshot.device.firmware_version = Some(info.firmware_version);
                    snapshot.device.bootloader_version = Some(info.bootloader_version);
                    snapshot.device.protocol_min = Some(info.protocol_min);
                    snapshot.device.protocol_max = Some(info.protocol_max);
                    snapshot.device.negotiated_protocol = negotiated_protocol;
                    snapshot.device.port_name = Some(port_name);
                    snapshot.device.capabilities = info.capabilities;
                    snapshot.device.reconnect_count = self.reconnect_count;
                    snapshot.device.last_error = None;
                    snapshot.relay.available =
                        snapshot.device.capabilities.contains(&Capability::Relay);
                });
                if let Some(status) = status {
                    self.apply_device_status(status);
                }
            }
            DeviceWorkerEvent::Sample(DeviceEvent::SensorSample {
                sample,
                sequence_gap,
                ..
            }) => {
                if sequence_gap.is_some() {
                    self.snapshots.update(|snapshot| {
                        snapshot.sensor.sequence_gaps =
                            snapshot.sensor.sequence_gaps.saturating_add(1);
                    });
                }
                match (sample.quality, sample.lux) {
                    (SampleQuality::Valid, Some(lux)) => self.apply_lux_sample(lux),
                    _ => self.snapshots.update(|snapshot| {
                        snapshot.sensor.raw_lux = None;
                        snapshot.sensor.valid = false;
                    }),
                }
            }
            DeviceWorkerEvent::Sample(_) => {}
            DeviceWorkerEvent::Status(status) => self.apply_device_status(status),
            DeviceWorkerEvent::BackingOff { delay, error } => {
                self.capabilities.clear();
                self.logger.warn(
                    "device_reconnect_wait",
                    format!("Device unavailable; retrying in {} ms", delay.as_millis()),
                );
                self.snapshots.update(|snapshot| {
                    snapshot.device.state = DeviceConnectionState::BackingOff;
                    snapshot.device.last_error =
                        Some(format!("{error}; retrying in {} ms", delay.as_millis()));
                    snapshot.relay.available = false;
                });
            }
            DeviceWorkerEvent::RelayResult {
                light_on,
                result,
                reply,
            } => match result {
                Ok(status) => {
                    self.snapshots.update(|snapshot| {
                        snapshot.relay.available = status.available;
                        snapshot.relay.energized = status.energized;
                        snapshot.relay.light_on = Some(light_on);
                        snapshot.relay.last_error = None;
                    });
                    let _ = reply.send(Ok(ResponsePayload::Acknowledged));
                }
                Err(error) => {
                    self.snapshots.update(|snapshot| {
                        snapshot.relay.last_error = Some(error.clone());
                    });
                    let _ = reply.send(Err(wire_error(IpcErrorCode::HardwareUnavailable, error)));
                }
            },
        }
    }

    fn apply_lux_sample(&mut self, lux: f64) {
        let filtered = match self.filter.push(lux) {
            Ok(value) => value,
            Err(_) => return,
        };
        self.last_sensor = Some(Instant::now());
        let candidate = map_normalized_lux_to_brightness(
            filtered,
            &self.document.settings.control.sensor_curve,
        );
        let target = self.stabilizer.update(candidate);
        let changed = self.last_target != Some(target);
        self.last_target = Some(target);
        self.snapshots.record_sensor_sample(|snapshot| {
            snapshot.sensor.raw_lux = Some(lux);
            snapshot.sensor.filtered_lux = Some(filtered);
            snapshot.sensor.valid = true;
            snapshot.target_percent = Some(target);
        });
        if changed {
            self.apply_last_target();
        }
        self.evaluate_relay_rules();
    }

    fn apply_device_status(&mut self, status: DeviceStatus) {
        let malformed = u64::from(status.malformed_frames);
        let contact_mode = self.document.settings.relay.contact_mode;
        self.snapshots.update(|snapshot| {
            snapshot.sensor.malformed_frames = malformed;
            snapshot.relay.available = status.relay.available;
            snapshot.relay.energized = status.relay.energized;
            snapshot.relay.light_on = status
                .relay
                .energized
                .map(|energized| contact_mode.light_on(energized));
        });
    }

    fn request_light(&mut self, light_on: bool, reply: Sender<CommandResult>) {
        if self.suspended {
            let _ = reply.send(Err(wire_error(
                IpcErrorCode::HardwareUnavailable,
                "Lumi device is suspended",
            )));
            return;
        }
        if !self.capabilities.contains(&Capability::Relay) {
            let _ = reply.send(Err(wire_error(
                IpcErrorCode::UnsupportedCapability,
                "Connected hardware has no relay",
            )));
            return;
        }
        let energized = self
            .document
            .settings
            .relay
            .contact_mode
            .energized_for_light(light_on);
        if self
            .device_tx
            .send(DeviceCommand::SetRelay {
                energized,
                light_on,
                reply: reply.clone(),
            })
            .is_err()
        {
            let _ = reply.send(Err(wire_error(
                IpcErrorCode::HardwareUnavailable,
                "Device worker stopped",
            )));
        }
    }

    fn evaluate_relay_rules(&mut self) {
        if self.suspended
            || !self.document.settings.relay.rules_enabled
            || !self.capabilities.contains(&Capability::Relay)
        {
            self.snapshots.update_if(|snapshot| {
                let changed = snapshot.relay.matched_rule_id.is_some()
                    || snapshot.relay.matched_rule_name.is_some()
                    || snapshot.environment.configured != self.document.settings.weather.enabled;
                snapshot.relay.matched_rule_id = None;
                snapshot.relay.matched_rule_name = None;
                snapshot.environment.configured = self.document.settings.weather.enabled;
                changed
            });
            return;
        }
        self.maybe_schedule_weather();
        let snapshot = self.snapshots.get();
        let current_brightness = mean_brightness(&snapshot.monitors);
        let environment = self.environment_snapshot();
        let context = RuleContext {
            now_minutes: environment.now_minutes,
            sunrise_minutes: environment.sunrise_minutes,
            sunset_minutes: environment.sunset_minutes,
            weather: environment.weather,
            lux: snapshot.sensor.filtered_lux,
            current_brightness,
            target_brightness: self.last_target,
        };
        let decision = evaluate_rules(
            &self.document.settings.relay.rules,
            self.document.settings.relay.fallback_action,
            &context,
        );
        self.snapshots.update_if(|snapshot| {
            let changed = snapshot.relay.matched_rule_id != decision.matched_rule_id
                || snapshot.relay.matched_rule_name != decision.matched_rule_name
                || snapshot.environment != environment;
            snapshot.relay.matched_rule_id = decision.matched_rule_id.clone();
            snapshot.relay.matched_rule_name = decision.matched_rule_name.clone();
            snapshot.environment = environment;
            changed
        });
        let Some(light_on) = decision.action.light_on() else {
            return;
        };
        if snapshot.relay.light_on == Some(light_on) {
            return;
        }
        let (discard_tx, _discard_rx) = mpsc::channel();
        self.request_light(light_on, discard_tx);
    }

    fn environment_snapshot(&mut self) -> EnvironmentSnapshot {
        let settings = &self.document.settings.weather;
        if !settings.enabled {
            return EnvironmentSnapshot {
                now_minutes: local_minutes(),
                ..EnvironmentSnapshot::default()
            };
        }

        let now = Instant::now();
        let cache_matches = self.solar_cache.as_ref().is_some_and(|cache| {
            cache.latitude_bits == settings.latitude.to_bits()
                && cache.longitude_bits == settings.longitude.to_bits()
                && cache.timezone == settings.timezone
                && now.duration_since(cache.computed_at) < SOLAR_CACHE_FOR
        });
        if !cache_matches {
            match current_solar_context(settings.latitude, settings.longitude, &settings.timezone) {
                Ok(context) => {
                    self.solar_error = None;
                    self.solar_cache = Some(SolarCache {
                        latitude_bits: settings.latitude.to_bits(),
                        longitude_bits: settings.longitude.to_bits(),
                        timezone: settings.timezone.clone(),
                        computed_at: now,
                        context,
                    });
                }
                Err(error) => {
                    self.solar_error = Some(error.to_string());
                    self.solar_cache = None;
                }
            }
        }
        let solar = self.solar_cache.as_ref().map(|cache| &cache.context);
        let weather_error = self
            .weather_is_needed()
            .then(|| self.weather_error.clone())
            .flatten();
        EnvironmentSnapshot {
            configured: true,
            now_minutes: solar.map_or_else(local_minutes, |context| context.now_minutes),
            sunrise_minutes: solar.and_then(|context| context.sunrise_minutes),
            sunset_minutes: solar.and_then(|context| context.sunset_minutes),
            timezone: solar
                .map(|context| context.timezone.clone())
                .or_else(|| Some(settings.timezone.clone())),
            weather: self.weather_observation.map(|observation| observation.kind),
            weather_observed_at_unix_ms: self.weather_observed_at_unix_ms,
            last_error: self.solar_error.clone().or(weather_error),
        }
    }

    fn weather_is_needed(&self) -> bool {
        self.document.settings.weather.enabled
            && self.document.settings.relay.rules_enabled
            && self.capabilities.contains(&Capability::Relay)
            && self
                .document
                .settings
                .relay
                .rules
                .iter()
                .any(|rule| rule.enabled && expression_uses_weather(&rule.when))
    }

    fn maybe_schedule_weather(&mut self) {
        if !self.weather_is_needed()
            || self.weather_fetch_in_flight
            || Instant::now() < self.weather_next_refresh
        {
            return;
        }
        let settings = &self.document.settings.weather;
        let request = WeatherRequest::new(
            settings.latitude,
            settings.longitude,
            settings.timezone.clone(),
        );
        let generation = self.weather_generation;
        let runtime_tx = self.runtime_tx.clone();
        self.weather_fetch_in_flight = true;
        self.weather_next_refresh =
            Instant::now() + Duration::from_secs(settings.refresh_seconds.max(60));
        let spawn = thread::Builder::new()
            .name("lumi-weather-request".to_string())
            .spawn(move || {
                let result = fetch_open_meteo(&request).map_err(|error| error.to_string());
                let _ = runtime_tx.send(RuntimeMessage::Environment(EnvironmentWorkerEvent {
                    generation,
                    result,
                }));
            });
        if let Err(error) = spawn {
            self.weather_fetch_in_flight = false;
            self.weather_error = Some(format!("Could not start weather request: {error}"));
            self.weather_next_refresh = Instant::now() + WEATHER_RETRY_AFTER;
        }
    }

    fn handle_environment_event(&mut self, event: EnvironmentWorkerEvent) {
        if event.generation != self.weather_generation {
            return;
        }
        self.weather_fetch_in_flight = false;
        match event.result {
            Ok(observation) => {
                self.logger
                    .info("weather_updated", "Weather context was updated");
                self.weather_observation = Some(observation);
                self.weather_observed_at_unix_ms = Some(unix_millis());
                self.weather_error = None;
                self.weather_next_refresh = Instant::now()
                    + Duration::from_secs(self.document.settings.weather.refresh_seconds.max(60));
            }
            Err(error) => {
                self.logger.warn(
                    "weather_unavailable",
                    "Weather context is temporarily unavailable",
                );
                self.weather_error = Some(error);
                self.weather_next_refresh = Instant::now() + WEATHER_RETRY_AFTER;
            }
        }
        self.evaluate_relay_rules();
    }

    fn refresh_monitors(&mut self) {
        if !self.scheduler.request_refresh() && self.scheduler.descriptors().is_empty() {
            self.snapshots.update(|snapshot| {
                snapshot.status_message = "Monitor discovery is already running".to_string();
            });
        }
    }

    fn replace_monitor_descriptors(&mut self, descriptors: Vec<MonitorDescriptor>) {
        let ids = descriptors
            .iter()
            .map(|descriptor| descriptor.id.clone())
            .collect::<Vec<_>>();
        self.monitor_state.retain(|id, _| ids.contains(id));
        for descriptor in &descriptors {
            self.monitor_state
                .entry(descriptor.id.clone())
                .or_insert_with(|| MonitorControlState {
                    guard: ManualOverrideGuard::new(self.document.settings.control.manual_override),
                    current: descriptor.current_percent(),
                    target: self.last_target,
                    transition_active: false,
                });
        }
        self.snapshots.update(|snapshot| {
            snapshot.monitors = descriptors
                .into_iter()
                .map(|descriptor| {
                    let current_percent = descriptor.current_percent();
                    MonitorSnapshot {
                        id: descriptor.id,
                        display_name: descriptor.display_name,
                        display_path: descriptor.display_path,
                        qualified: descriptor.brightness.is_some(),
                        current_percent,
                        target_percent: self.last_target,
                        transition_active: false,
                        manual_override_remaining_ms: None,
                        ddc_error_count: 0,
                        last_error: descriptor.qualification_error,
                    }
                })
                .collect();
        });
        self.apply_last_target();
    }

    fn handle_monitor_event(&mut self, event: SchedulerEvent) {
        if self.suspended {
            return;
        }
        match event {
            SchedulerEvent::MonitorOnline {
                monitor_id,
                current_percent,
            } => {
                if let Some(state) = self.monitor_state.get_mut(&monitor_id) {
                    state.current = Some(current_percent);
                }
                self.update_monitor_snapshot(&monitor_id, |monitor| {
                    monitor.current_percent = Some(current_percent);
                    monitor.last_error = None;
                });
            }
            SchedulerEvent::BrightnessApplied {
                monitor_id,
                percent,
            } => {
                if let Some(state) = self.monitor_state.get_mut(&monitor_id) {
                    state.current = Some(percent);
                }
                self.update_monitor_snapshot(&monitor_id, |monitor| {
                    monitor.current_percent = Some(percent);
                    monitor.last_error = None;
                });
            }
            SchedulerEvent::BrightnessObserved {
                monitor_id,
                percent,
            } => {
                let now_ms = self.now_ms();
                if let Some(state) = self.monitor_state.get_mut(&monitor_id) {
                    let expected = state.current.or(state.target).unwrap_or(percent);
                    state
                        .guard
                        .observe(now_ms, expected, percent, state.transition_active);
                    state.current = Some(percent);
                    let remaining = state.guard.remaining_ms(now_ms);
                    self.update_monitor_snapshot(&monitor_id, |monitor| {
                        monitor.current_percent = Some(percent);
                        monitor.manual_override_remaining_ms = remaining;
                    });
                }
            }
            SchedulerEvent::TransitionComplete {
                monitor_id,
                percent,
            } => {
                if let Some(state) = self.monitor_state.get_mut(&monitor_id) {
                    state.current = Some(percent);
                    state.transition_active = false;
                }
                self.update_monitor_snapshot(&monitor_id, |monitor| {
                    monitor.current_percent = Some(percent);
                    monitor.transition_active = false;
                });
            }
            SchedulerEvent::MonitorError {
                monitor_id,
                message,
            } => {
                self.logger
                    .warn("monitor_error", "A monitor operation failed");
                if let Some(state) = self.monitor_state.get_mut(&monitor_id) {
                    state.transition_active = false;
                }
                self.update_monitor_snapshot(&monitor_id, |monitor| {
                    monitor.ddc_error_count = monitor.ddc_error_count.saturating_add(1);
                    monitor.last_error = Some(message);
                    monitor.transition_active = false;
                });
            }
            SchedulerEvent::MonitorRemoved { monitor_id } => {
                self.monitor_state.remove(&monitor_id);
                self.snapshots.update(|snapshot| {
                    snapshot.monitors.retain(|monitor| monitor.id != monitor_id);
                });
            }
            SchedulerEvent::RefreshReady { descriptors } => {
                self.logger.info(
                    "monitor_refresh",
                    format!("Monitor discovery found {} displays", descriptors.len()),
                );
                self.scheduler.install_descriptors(descriptors.clone());
                self.replace_monitor_descriptors(descriptors);
            }
            SchedulerEvent::RefreshFailed { message } => {
                self.logger
                    .warn("monitor_refresh_failed", "Monitor discovery failed");
                self.snapshots.update(|snapshot| {
                    snapshot.status_message = message;
                });
            }
        }
    }

    fn apply_last_target(&mut self) {
        if self.document.settings.paused {
            return;
        }
        let Some(target) = self.last_target else {
            return;
        };
        let now_ms = self.now_ms();
        let ids = self
            .monitor_state
            .iter()
            .filter(|(id, state)| {
                let enabled = self
                    .document
                    .settings
                    .monitors
                    .get(*id)
                    .is_none_or(|profile| profile.enabled);
                enabled && !state.guard.is_suppressed(now_ms)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            match self
                .scheduler
                .retarget(&id, target, self.document.settings.control.transition)
            {
                Ok(()) => {
                    if let Some(state) = self.monitor_state.get_mut(&id) {
                        state.target = Some(target);
                        state.transition_active = true;
                    }
                    self.update_monitor_snapshot(&id, |monitor| {
                        monitor.target_percent = Some(target);
                        monitor.transition_active = true;
                        monitor.manual_override_remaining_ms = None;
                    });
                }
                Err(error) => self.update_monitor_snapshot(&id, |monitor| {
                    monitor.last_error = Some(error.to_string());
                }),
            }
        }
        self.update_override_snapshots(now_ms);
    }

    fn update_override_snapshots(&self, now_ms: u64) {
        let remaining = self
            .monitor_state
            .iter()
            .map(|(id, state)| (id.clone(), state.guard.remaining_ms(now_ms)))
            .collect::<BTreeMap<_, _>>();
        self.snapshots.update(|snapshot| {
            for monitor in &mut snapshot.monitors {
                monitor.manual_override_remaining_ms =
                    remaining.get(&monitor.id).copied().flatten();
            }
        });
    }

    fn update_monitor_snapshot(&self, monitor_id: &str, update: impl FnOnce(&mut MonitorSnapshot)) {
        self.snapshots.update(|snapshot| {
            if let Some(monitor) = snapshot
                .monitors
                .iter_mut()
                .find(|monitor| monitor.id == monitor_id)
            {
                update(monitor);
            }
        });
    }

    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
}

fn mean_brightness(monitors: &[MonitorSnapshot]) -> Option<i32> {
    let values = monitors
        .iter()
        .filter_map(|monitor| monitor.current_percent)
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<i32>() / values.len() as i32)
}

fn expression_uses_weather(expression: &ConditionExpression) -> bool {
    match expression {
        ConditionExpression::Condition { condition } => {
            matches!(condition, LightCondition::WeatherIs { .. })
        }
        ConditionExpression::And { conditions } | ConditionExpression::Or { conditions } => {
            conditions.iter().any(expression_uses_weather)
        }
    }
}

#[cfg(windows)]
fn local_minutes() -> i32 {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetLocalTime(system_time: *mut SystemTime);
    }
    let mut time = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    unsafe { GetLocalTime(&mut time) };
    i32::from(time.hour) * 60 + i32::from(time.minute)
}

#[cfg(not(windows))]
fn local_minutes() -> i32 {
    ((unix_millis() / 60_000) % (24 * 60)) as i32
}

fn wire_error(code: IpcErrorCode, message: impl Into<String>) -> IpcWireError {
    IpcWireError {
        code,
        message: message.into(),
    }
}

fn launch_ui_process() -> Result<(), IpcWireError> {
    let executable = std::env::current_exe()
        .map_err(|error| wire_error(IpcErrorCode::Internal, error.to_string()))?;
    let directory = executable.parent().ok_or_else(|| {
        wire_error(
            IpcErrorCode::Internal,
            "Agent executable has no parent directory",
        )
    })?;
    let ui = ["LumiControl.exe", "lumi-ui.exe"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file());
    let Some(ui) = ui else {
        return Err(wire_error(
            IpcErrorCode::Internal,
            format!("Lumi UI is not installed next to {}", executable.display()),
        ));
    };
    std::process::Command::new(ui)
        .spawn()
        .map_err(|error| wire_error(IpcErrorCode::Internal, error.to_string()))?;
    Ok(())
}

#[cfg(windows)]
struct SystemWatcher {
    window: isize,
    join: Option<JoinHandle<()>>,
}

#[cfg(windows)]
impl SystemWatcher {
    fn spawn(runtime_tx: Sender<RuntimeMessage>) -> Result<Self, std::io::Error> {
        use std::cell::RefCell;
        use std::ffi::c_void;
        use std::ptr::{null, null_mut};

        type Bool = i32;
        type Dword = u32;
        type Handle = *mut c_void;
        type Hwnd = Handle;
        type Hinstance = Handle;
        type Lparam = isize;
        type Lresult = isize;
        type Uint = u32;
        type Wparam = usize;

        const ERROR_CLASS_ALREADY_EXISTS: Dword = 1410;
        const WM_CLOSE: Uint = 0x0010;
        const WM_DESTROY: Uint = 0x0002;
        const WM_DISPLAYCHANGE: Uint = 0x007e;
        const WM_POWERBROADCAST: Uint = 0x0218;
        const PBT_APMSUSPEND: Wparam = 0x0004;
        const PBT_APMRESUMECRITICAL: Wparam = 0x0006;
        const PBT_APMRESUMESUSPEND: Wparam = 0x0007;
        const PBT_APMRESUMEAUTOMATIC: Wparam = 0x0012;

        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }

        #[repr(C)]
        struct Message {
            window: Hwnd,
            message: Uint,
            wparam: Wparam,
            lparam: Lparam,
            time: Dword,
            point: Point,
            private: Dword,
        }

        type WindowProcedure =
            Option<unsafe extern "system" fn(Hwnd, Uint, Wparam, Lparam) -> Lresult>;

        #[repr(C)]
        struct WindowClass {
            style: Uint,
            procedure: WindowProcedure,
            class_extra: i32,
            window_extra: i32,
            instance: Hinstance,
            icon: Handle,
            cursor: Handle,
            background: Handle,
            menu_name: *const u16,
            class_name: *const u16,
        }

        #[link(name = "Kernel32")]
        extern "system" {
            fn GetModuleHandleW(module_name: *const u16) -> Hinstance;
            fn GetLastError() -> Dword;
        }

        #[link(name = "User32")]
        extern "system" {
            fn RegisterClassW(window_class: *const WindowClass) -> u16;
            fn UnregisterClassW(class_name: *const u16, instance: Hinstance) -> Bool;
            fn CreateWindowExW(
                extended_style: Dword,
                class_name: *const u16,
                window_name: *const u16,
                style: Dword,
                x: i32,
                y: i32,
                width: i32,
                height: i32,
                parent: Hwnd,
                menu: Handle,
                instance: Hinstance,
                parameter: *mut c_void,
            ) -> Hwnd;
            fn DestroyWindow(window: Hwnd) -> Bool;
            fn DefWindowProcW(
                window: Hwnd,
                message: Uint,
                wparam: Wparam,
                lparam: Lparam,
            ) -> Lresult;
            fn GetMessageW(
                message: *mut Message,
                window: Hwnd,
                minimum: Uint,
                maximum: Uint,
            ) -> Bool;
            fn TranslateMessage(message: *const Message) -> Bool;
            fn DispatchMessageW(message: *const Message) -> Lresult;
            fn PostQuitMessage(exit_code: i32);
        }

        thread_local! {
            static SYSTEM_EVENT_TX: RefCell<Option<Sender<RuntimeMessage>>> = const { RefCell::new(None) };
        }

        unsafe extern "system" fn window_proc(
            window: Hwnd,
            message: Uint,
            wparam: Wparam,
            lparam: Lparam,
        ) -> Lresult {
            let event = match (message, wparam) {
                (WM_DISPLAYCHANGE, _) => Some(SystemEvent::DisplayChanged),
                (WM_POWERBROADCAST, PBT_APMSUSPEND) => Some(SystemEvent::Suspend),
                (
                    WM_POWERBROADCAST,
                    PBT_APMRESUMECRITICAL | PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC,
                ) => Some(SystemEvent::Resume),
                _ => None,
            };
            if let Some(event) = event {
                SYSTEM_EVENT_TX.with(|sender| {
                    if let Some(sender) = sender.borrow().as_ref() {
                        let _ = sender.send(RuntimeMessage::System(event));
                    }
                });
                return if message == WM_POWERBROADCAST { 1 } else { 0 };
            }
            match message {
                WM_CLOSE => {
                    DestroyWindow(window);
                    0
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    0
                }
                _ => DefWindowProcW(window, message, wparam, lparam),
            }
        }

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("lumi-system-events".to_string())
            .spawn(move || {
                SYSTEM_EVENT_TX.with(|sender| *sender.borrow_mut() = Some(runtime_tx));
                let class_name =
                    wide_null(&format!("LumiControl.SystemEvents.{}", std::process::id()));
                let instance = unsafe { GetModuleHandleW(null()) };
                if instance.is_null() {
                    let _ = ready_tx.send(Err(std::io::Error::last_os_error()));
                    return;
                }
                let window_class = WindowClass {
                    style: 0,
                    procedure: Some(window_proc),
                    class_extra: 0,
                    window_extra: 0,
                    instance,
                    icon: null_mut(),
                    cursor: null_mut(),
                    background: null_mut(),
                    menu_name: null(),
                    class_name: class_name.as_ptr(),
                };
                let atom = unsafe { RegisterClassW(&window_class) };
                if atom == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
                    let _ = ready_tx.send(Err(std::io::Error::last_os_error()));
                    return;
                }
                let window = unsafe {
                    CreateWindowExW(
                        0,
                        class_name.as_ptr(),
                        class_name.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        null_mut(),
                        null_mut(),
                        instance,
                        null_mut(),
                    )
                };
                if window.is_null() {
                    let _ = ready_tx.send(Err(std::io::Error::last_os_error()));
                    return;
                }
                if ready_tx.send(Ok(window as isize)).is_err() {
                    unsafe { DestroyWindow(window) };
                    return;
                }
                let mut message = Message {
                    window: null_mut(),
                    message: 0,
                    wparam: 0,
                    lparam: 0,
                    time: 0,
                    point: Point { x: 0, y: 0 },
                    private: 0,
                };
                while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                SYSTEM_EVENT_TX.with(|sender| *sender.borrow_mut() = None);
                unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
            })?;
        let window = ready_rx
            .recv()
            .map_err(|_| std::io::Error::other("system event watcher stopped during startup"))??;
        Ok(Self {
            window,
            join: Some(join),
        })
    }
}

#[cfg(windows)]
impl Drop for SystemWatcher {
    fn drop(&mut self) {
        use std::ffi::c_void;

        type Hwnd = *mut c_void;
        const WM_CLOSE: u32 = 0x0010;
        #[link(name = "User32")]
        extern "system" {
            fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
        }
        unsafe {
            PostMessageW(self.window as Hwnd, WM_CLOSE, 0, 0);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(not(windows))]
struct SystemWatcher;

#[cfg(not(windows))]
impl SystemWatcher {
    fn spawn(_runtime_tx: Sender<RuntimeMessage>) -> Result<Self, std::io::Error> {
        Ok(Self)
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[derive(Debug)]
pub enum AgentError {
    Store(StoreError),
    Ipc(IpcError),
    Thread(std::io::Error),
    Startup(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Store(error) => write!(formatter, "settings failed: {error}"),
            AgentError::Ipc(error) => write!(formatter, "IPC failed: {error}"),
            AgentError::Thread(error) => write!(formatter, "thread creation failed: {error}"),
            AgentError::Startup(error) => write!(formatter, "startup registration failed: {error}"),
        }
    }
}

impl std::error::Error for AgentError {}

impl From<StoreError> for AgentError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<IpcError> for AgentError {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

impl From<std::io::Error> for AgentError {
    fn from(error: std::io::Error) -> Self {
        Self::Thread(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumi_core::{LightRule, WeatherKind};
    use lumi_device::{DeviceError, DevicePort, PortCandidate, PortKind, UsbId};
    use lumi_device_simulator::{SimulatedProfile, Simulator};
    use lumi_monitor_windows::{BrightnessRange, MonitorError, MonitorSession};
    use lumi_protocol::{decode_frame, encode_frame, Message};
    use std::collections::VecDeque;
    use std::io;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static NEXT_TEST_ID: AtomicUsize = AtomicUsize::new(1);

    #[test]
    fn cpu_usage_is_reported_as_percent_of_one_logical_core() {
        assert_eq!(cpu_usage_basis_points(100, 101, 1_000), 10);
        assert_eq!(cpu_usage_basis_points(100, 200, 1_000), 1_000);
        assert_eq!(cpu_usage_basis_points(100, 2_100, 1_000), 20_000);
    }

    #[test]
    fn agent_discovery_does_not_probe_unrelated_usb_serial_devices() {
        let policy = agent_discovery_policy();
        let unrelated = PortCandidate {
            name: "COM9".to_string(),
            kind: PortKind::Usb {
                id: UsbId {
                    vid: 0x1a86,
                    pid: 0x7523,
                },
                serial_number: None,
                manufacturer: None,
                product: None,
            },
        };
        assert!(!policy.accepts(&unrelated));
    }

    #[cfg(windows)]
    #[test]
    fn system_watcher_uses_a_hidden_top_level_window_and_forwards_power_events() {
        use std::ffi::c_void;

        type Hwnd = *mut c_void;
        const WM_POWERBROADCAST: u32 = 0x0218;
        const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;

        #[link(name = "User32")]
        extern "system" {
            fn GetParent(window: Hwnd) -> Hwnd;
            fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
        }

        let (tx, rx) = mpsc::channel();
        let watcher = SystemWatcher::spawn(tx).unwrap();
        let window = watcher.window as Hwnd;
        assert!(unsafe { GetParent(window) }.is_null());
        assert_ne!(
            unsafe { PostMessageW(window, WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC, 0) },
            0
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            RuntimeMessage::System(SystemEvent::Resume)
        ));
        drop(watcher);
    }

    struct SimPort {
        simulator: Simulator,
        incoming: VecDeque<Vec<u8>>,
        reads: usize,
    }

    impl DevicePort for SimPort {
        fn name(&self) -> &str {
            "COM-SIM"
        }

        fn write_frame(&mut self, frame: &[u8]) -> Result<(), DeviceError> {
            let Message::Request(request) = decode_frame(frame)? else {
                return Err(DeviceError::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "expected request",
                )));
            };
            self.incoming
                .push_back(encode_frame(&self.simulator.response_message(request))?);
            Ok(())
        }

        fn read_frame(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, DeviceError> {
            if let Some(frame) = self.incoming.pop_front() {
                return Ok(Some(frame));
            }
            self.reads += 1;
            if self.reads.is_multiple_of(2) {
                return Ok(Some(encode_frame(&Message::Event(
                    self.simulator.sample_event(),
                ))?));
            }
            thread::sleep(timeout.min(Duration::from_millis(5)));
            Ok(None)
        }
    }

    struct SimProvider {
        opens: AtomicUsize,
        profile: SimulatedProfile,
    }

    impl DevicePortProvider for SimProvider {
        fn candidates(&self) -> Result<Vec<PortCandidate>, DeviceError> {
            Ok(vec![PortCandidate {
                name: "COM-SIM".to_string(),
                kind: PortKind::Usb {
                    id: UsbId {
                        vid: 0x303a,
                        pid: 0x1001,
                    },
                    serial_number: Some("SIM-1".to_string()),
                    manufacturer: Some("Lumi".to_string()),
                    product: Some("Sensor".to_string()),
                },
            }])
        }

        fn open(&self, _candidate: &PortCandidate) -> Result<Box<dyn DevicePort>, DeviceError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(SimPort {
                simulator: Simulator::new(self.profile, "SIM-1"),
                incoming: VecDeque::new(),
                reads: 0,
            }))
        }
    }

    #[derive(Clone)]
    struct FakeMonitorBackend {
        state: Arc<Mutex<i32>>,
    }

    struct FakeMonitorSession {
        state: Arc<Mutex<i32>>,
    }

    impl MonitorSession for FakeMonitorSession {
        fn id(&self) -> &str {
            "monitor-test"
        }

        fn read_brightness(&mut self) -> Result<BrightnessRange, MonitorError> {
            Ok(BrightnessRange {
                minimum_raw: 0,
                current_raw: *self.state.lock().unwrap() as u32,
                maximum_raw: 100,
            })
        }

        fn set_brightness_percent(&mut self, percent: i32) -> Result<(), MonitorError> {
            *self.state.lock().unwrap() = percent;
            Ok(())
        }
    }

    impl MonitorBackend for FakeMonitorBackend {
        fn enumerate(&self) -> Result<Vec<MonitorDescriptor>, MonitorError> {
            Ok(vec![MonitorDescriptor {
                id: "monitor-test".to_string(),
                display_name: "Test Monitor".to_string(),
                display_path: "DISPLAY1".to_string(),
                device_id: "TEST".to_string(),
                brightness: Some(BrightnessRange {
                    minimum_raw: 0,
                    current_raw: *self.state.lock().unwrap() as u32,
                    maximum_raw: 100,
                }),
                qualification_error: None,
            }])
        }

        fn open(&self, monitor_id: &str) -> Result<Box<dyn MonitorSession>, MonitorError> {
            if monitor_id != "monitor-test" {
                return Err(MonitorError::NotFound(monitor_id.to_string()));
            }
            Ok(Box::new(FakeMonitorSession {
                state: Arc::clone(&self.state),
            }))
        }
    }

    struct TestStartupRegistration;

    impl StartupRegistration for TestStartupRegistration {
        fn set_enabled(&self, _enabled: bool) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingStartupRegistration {
        values: Mutex<Vec<bool>>,
        fail_next: AtomicBool,
    }

    impl StartupRegistration for RecordingStartupRegistration {
        fn set_enabled(&self, enabled: bool) -> Result<(), String> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err("simulated registry failure".to_string());
            }
            self.values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(enabled);
            Ok(())
        }
    }

    fn start_test_agent_with_startup(
        profile: SimulatedProfile,
        startup_registration: Arc<dyn StartupRegistration>,
    ) -> (AgentProcess, Arc<Mutex<i32>>) {
        let test_id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "lumi-agent-test-{}-{}-{}",
            std::process::id(),
            unix_millis(),
            test_id
        ));
        let state = Arc::new(Mutex::new(25));
        let pipe = format!(
            r"\\.\pipe\LumiControl.AgentTest.{}.{}.{}",
            std::process::id(),
            unix_millis(),
            test_id
        );
        let process = AgentProcess::start(AgentOptions {
            store: SettingsStore::new(ProductPaths::under(root)),
            legacy_config_path: PathBuf::from("missing-v1-config.json"),
            monitor_backend: Arc::new(FakeMonitorBackend {
                state: Arc::clone(&state),
            }),
            device_provider: Arc::new(SimProvider {
                opens: AtomicUsize::new(0),
                profile,
            }),
            pipe_name: pipe,
            startup_registration,
            install_crash_hook: false,
        })
        .unwrap();
        (process, state)
    }

    fn start_test_agent(profile: SimulatedProfile) -> (AgentProcess, Arc<Mutex<i32>>) {
        start_test_agent_with_startup(profile, Arc::new(TestStartupRegistration))
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(predicate(), "condition did not become true before timeout");
    }

    #[test]
    fn sensor_events_continue_without_any_ui_client() {
        let (process, monitor) = start_test_agent(SimulatedProfile::Sensor);
        let handle = process.handle();
        wait_until(Duration::from_secs(2), || {
            let snapshot = handle.snapshot();
            snapshot.sensor.valid && snapshot.target_percent.is_some()
        });
        wait_until(Duration::from_secs(2), || *monitor.lock().unwrap() != 25);
        process.shutdown();
    }

    #[test]
    fn suspend_holds_device_offline_until_resume() {
        let (process, _) = start_test_agent(SimulatedProfile::SensorRelay);
        let handle = process.handle();
        wait_until(Duration::from_secs(2), || {
            let snapshot = handle.snapshot();
            snapshot.device.state == DeviceConnectionState::Connected && snapshot.sensor.valid
        });

        handle
            .tx
            .send(RuntimeMessage::System(SystemEvent::Suspend))
            .unwrap();
        wait_until(Duration::from_secs(1), || {
            let snapshot = handle.snapshot();
            snapshot.device.state == DeviceConnectionState::Disconnected
                && !snapshot.sensor.valid
                && !snapshot.relay.available
        });
        thread::sleep(Duration::from_millis(100));
        let suspended = handle.snapshot();
        assert_eq!(suspended.device.state, DeviceConnectionState::Disconnected);
        assert!(!suspended.sensor.valid);
        let error = handle
            .execute(AgentCommand::SetLight { light_on: true })
            .unwrap_err();
        assert_eq!(error.code, IpcErrorCode::HardwareUnavailable);
        let error = handle.execute(AgentCommand::RefreshHardware).unwrap_err();
        assert_eq!(error.code, IpcErrorCode::HardwareUnavailable);

        handle
            .tx
            .send(RuntimeMessage::System(SystemEvent::Resume))
            .unwrap();
        wait_until(Duration::from_secs(2), || {
            let snapshot = handle.snapshot();
            snapshot.device.state == DeviceConnectionState::Connected
                && snapshot.sensor.valid
                && snapshot.relay.available
        });
        process.shutdown();
    }

    #[test]
    fn sensor_only_profile_rejects_relay_commands_locally() {
        let (process, _) = start_test_agent(SimulatedProfile::Sensor);
        let handle = process.handle();
        wait_until(Duration::from_secs(2), || {
            handle.snapshot().device.state == DeviceConnectionState::Connected
        });
        let error = handle
            .execute(AgentCommand::SetLight { light_on: true })
            .unwrap_err();
        assert_eq!(error.code, IpcErrorCode::UnsupportedCapability);
        process.shutdown();
    }

    #[test]
    fn relay_profile_reports_observed_state_after_command() {
        let (process, _) = start_test_agent(SimulatedProfile::SensorRelay);
        let handle = process.handle();
        wait_until(Duration::from_secs(2), || handle.snapshot().relay.available);
        handle
            .execute(AgentCommand::SetLight { light_on: true })
            .unwrap();
        assert_eq!(handle.snapshot().relay.light_on, Some(true));
        process.shutdown();
    }

    #[test]
    fn solar_rule_context_is_available_without_a_weather_request() {
        let (process, _) = start_test_agent(SimulatedProfile::SensorRelay);
        let handle = process.handle();
        wait_until(Duration::from_secs(2), || handle.snapshot().relay.available);
        let mut document = handle.settings();
        document.settings.weather.enabled = true;
        document.settings.weather.latitude = 31.2304;
        document.settings.weather.longitude = 121.4737;
        document.settings.weather.timezone = "Asia/Shanghai".to_string();
        document.settings.relay.rules_enabled = true;
        document.settings.relay.rules = vec![LightRule {
            id: "solar-test".to_string(),
            name: "Solar test".to_string(),
            enabled: true,
            when: ConditionExpression::condition(LightCondition::AfterSunrise {
                offset_minutes: 0,
            }),
            then: LightAction::Keep,
        }];
        handle
            .execute(AgentCommand::SaveSettings {
                document: Box::new(document),
            })
            .unwrap();
        let environment = handle.snapshot().environment;
        assert!(environment.configured);
        assert!(environment.sunrise_minutes.is_some());
        assert!(environment.sunset_minutes.is_some());
        assert_eq!(environment.timezone.as_deref(), Some("Asia/Shanghai"));
        assert_eq!(environment.weather, None);
        process.shutdown();
    }

    #[test]
    fn nested_weather_condition_is_detected() {
        let expression = ConditionExpression::And {
            conditions: vec![ConditionExpression::Or {
                conditions: vec![ConditionExpression::condition(LightCondition::WeatherIs {
                    weather: WeatherKind::Rain,
                })],
            }],
        };
        assert!(expression_uses_weather(&expression));
    }

    #[test]
    fn start_at_login_is_applied_transactionally() {
        let startup = Arc::new(RecordingStartupRegistration::default());
        let (process, _) = start_test_agent_with_startup(SimulatedProfile::Sensor, startup.clone());
        let handle = process.handle();
        let mut document = handle.settings();
        document.settings.start_at_login = true;
        handle
            .execute(AgentCommand::SaveSettings {
                document: Box::new(document),
            })
            .unwrap();
        assert!(handle.settings().settings.start_at_login);
        assert_eq!(
            *startup
                .values
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec![false, true]
        );

        startup.fail_next.store(true, Ordering::SeqCst);
        let mut document = handle.settings();
        document.settings.start_at_login = false;
        let error = handle
            .execute(AgentCommand::SaveSettings {
                document: Box::new(document),
            })
            .unwrap_err();
        assert_eq!(error.code, IpcErrorCode::Internal);
        assert!(handle.settings().settings.start_at_login);
        process.shutdown();
    }
}
