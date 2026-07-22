use lumi_core::{BrightnessSource, WeatherKind};
use lumi_protocol::Capability;
use lumi_store::SettingsDocument;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const API_VERSION: u16 = 2;
pub const MAX_IPC_FRAME_BYTES: usize = 1024 * 1024;
pub const DEFAULT_PIPE_BASE: &str = "LumiControl.Agent.v2";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IpcRequest {
    pub api_version: u16,
    pub id: u64,
    pub command: AgentCommand,
}

impl IpcRequest {
    pub fn new(id: u64, command: AgentCommand) -> Self {
        Self {
            api_version: API_VERSION,
            id,
            command,
        }
    }

    pub fn validate(&self) -> Result<(), IpcError> {
        if self.api_version != API_VERSION {
            return Err(IpcError::IncompatibleApi {
                received: self.api_version,
                supported: API_VERSION,
            });
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AgentCommand {
    Ping,
    GetSnapshot,
    WaitForSnapshot {
        after_revision: u64,
        timeout_ms: u32,
    },
    GetSettings,
    SaveSettings {
        document: Box<SettingsDocument>,
    },
    SetPaused {
        paused: bool,
    },
    RunNow,
    RefreshHardware,
    SetLight {
        light_on: bool,
    },
    ClearManualOverride {
        monitor_id: Option<String>,
    },
    ExportDiagnostics,
    OpenUi,
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IpcResponse {
    pub api_version: u16,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ResponsePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcWireError>,
}

impl IpcResponse {
    pub fn success(id: u64, result: ResponsePayload) -> Self {
        Self {
            api_version: API_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: u64, code: IpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            api_version: API_VERSION,
            id,
            result: None,
            error: Some(IpcWireError {
                code,
                message: message.into(),
            }),
        }
    }

    pub fn validate(&self) -> Result<(), IpcError> {
        if self.api_version != API_VERSION {
            return Err(IpcError::IncompatibleApi {
                received: self.api_version,
                supported: API_VERSION,
            });
        }
        match (self.result.is_some(), self.error.is_some()) {
            (true, false) | (false, true) => Ok(()),
            _ => Err(IpcError::InvalidFrame(
                "response must contain exactly one of result or error".to_string(),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ResponsePayload {
    Pong { agent_version: String },
    Snapshot(Box<AgentSnapshot>),
    Settings(Box<SettingsDocument>),
    Acknowledged,
    DiagnosticsExported { path: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    InvalidRequest,
    IncompatibleApi,
    InvalidSettings,
    UnsupportedCapability,
    HardwareUnavailable,
    Timeout,
    Internal,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IpcWireError {
    pub code: IpcErrorCode,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthLevel {
    Healthy,
    Degraded,
    Fault,
    #[default]
    Starting,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceConnectionState {
    #[default]
    Discovering,
    Connected,
    BackingOff,
    Disconnected,
    Fault,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceSnapshot {
    pub state: DeviceConnectionState,
    pub product_id: Option<String>,
    pub serial_number: Option<String>,
    pub hardware_version: Option<String>,
    pub firmware_version: Option<String>,
    pub bootloader_version: Option<String>,
    pub protocol_min: Option<u16>,
    pub protocol_max: Option<u16>,
    pub negotiated_protocol: Option<u16>,
    pub port_name: Option<String>,
    pub capabilities: Vec<Capability>,
    pub reconnect_count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SensorSnapshot {
    pub raw_lux: Option<f64>,
    pub filtered_lux: Option<f64>,
    pub sample_age_ms: Option<u64>,
    pub valid: bool,
    pub sequence_gaps: u64,
    pub malformed_frames: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MonitorSnapshot {
    pub id: String,
    pub display_name: String,
    pub display_path: String,
    pub qualified: bool,
    pub current_percent: Option<i32>,
    pub target_percent: Option<i32>,
    pub transition_active: bool,
    pub manual_override_remaining_ms: Option<u64>,
    pub ddc_error_count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RelaySnapshot {
    pub available: bool,
    pub light_on: Option<bool>,
    pub energized: Option<bool>,
    pub rules_enabled: bool,
    pub matched_rule_id: Option<String>,
    pub matched_rule_name: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EnvironmentSnapshot {
    pub configured: bool,
    pub now_minutes: i32,
    pub sunrise_minutes: Option<i32>,
    pub sunset_minutes: Option<i32>,
    pub solar_elevation_degrees: Option<f64>,
    pub daylight_minutes: Option<i32>,
    pub day_of_year: Option<u32>,
    pub timezone: Option<String>,
    pub weather: Option<WeatherKind>,
    pub cloud_cover_percent: Option<i32>,
    pub precipitation_probability_percent: Option<i32>,
    pub weather_observed_at_unix_ms: Option<u64>,
    pub base_brightness_percent: Option<i32>,
    pub brightness_offset_percent: i32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub process_id: u32,
    pub uptime_seconds: u64,
    pub cpu_usage_basis_points: Option<u32>,
    pub cpu_time_ms: Option<u64>,
    pub thread_count: Option<u32>,
    pub handle_count: Option<u32>,
    pub working_set_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AgentSnapshot {
    pub api_version: u16,
    pub revision: u64,
    pub generated_at_unix_ms: u64,
    pub health: HealthLevel,
    pub status_message: String,
    pub configuration_warning: Option<String>,
    pub paused: bool,
    pub brightness_source: BrightnessSource,
    pub target_percent: Option<i32>,
    pub device: DeviceSnapshot,
    pub sensor: SensorSnapshot,
    pub monitors: Vec<MonitorSnapshot>,
    pub relay: RelaySnapshot,
    pub environment: EnvironmentSnapshot,
    pub resources: ResourceSnapshot,
}

impl Default for AgentSnapshot {
    fn default() -> Self {
        Self {
            api_version: API_VERSION,
            revision: 0,
            generated_at_unix_ms: unix_millis(),
            health: HealthLevel::Starting,
            status_message: "Starting".to_string(),
            configuration_warning: None,
            paused: false,
            brightness_source: BrightnessSource::Sensor,
            target_percent: None,
            device: DeviceSnapshot::default(),
            sensor: SensorSnapshot::default(),
            monitors: Vec::new(),
            relay: RelaySnapshot::default(),
            environment: EnvironmentSnapshot::default(),
            resources: ResourceSnapshot {
                process_id: std::process::id(),
                ..ResourceSnapshot::default()
            },
        }
    }
}

pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn encode_length_prefixed<T: Serialize>(value: &T) -> Result<Vec<u8>, IpcError> {
    let json = serde_json::to_vec(value)?;
    if json.len() > MAX_IPC_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge(json.len()));
    }
    let mut frame = Vec::with_capacity(4 + json.len());
    frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
    frame.extend_from_slice(&json);
    Ok(frame)
}

pub fn decode_length_prefixed<T: DeserializeOwned>(frame: &[u8]) -> Result<T, IpcError> {
    if frame.len() < 4 {
        return Err(IpcError::InvalidFrame(
            "frame has no length prefix".to_string(),
        ));
    }
    let length = u32::from_le_bytes(frame[..4].try_into().expect("four-byte prefix")) as usize;
    if length > MAX_IPC_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge(length));
    }
    if frame.len() != length + 4 {
        return Err(IpcError::InvalidFrame(format!(
            "length prefix says {length} bytes but frame contains {}",
            frame.len().saturating_sub(4)
        )));
    }
    Ok(serde_json::from_slice(&frame[4..])?)
}

pub type RequestHandler = dyn Fn(IpcRequest) -> IpcResponse + Send + Sync + 'static;

pub struct NamedPipeServer {
    inner: platform::Server,
}

impl NamedPipeServer {
    pub fn bind(
        pipe_name: impl Into<String>,
        handler: Arc<RequestHandler>,
    ) -> Result<Self, IpcError> {
        Ok(Self {
            inner: platform::Server::bind(pipe_name.into(), handler)?,
        })
    }

    pub fn pipe_name(&self) -> &str {
        self.inner.pipe_name()
    }
}

pub struct NamedPipeClient {
    inner: platform::Client,
    next_id: u64,
}

impl NamedPipeClient {
    pub fn connect(pipe_name: &str, timeout_ms: u32) -> Result<Self, IpcError> {
        Ok(Self {
            inner: platform::Client::connect(pipe_name, timeout_ms)?,
            next_id: 1,
        })
    }

    pub fn call(&mut self, command: AgentCommand) -> Result<IpcResponse, IpcError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let request = IpcRequest::new(id, command);
        self.inner.write_message(&request)?;
        let response: IpcResponse = self.inner.read_message()?;
        response.validate()?;
        if response.id != id {
            return Err(IpcError::InvalidFrame(format!(
                "response ID {} does not match request ID {id}",
                response.id
            )));
        }
        Ok(response)
    }
}

pub struct SingleInstanceGuard {
    inner: platform::InstanceGuard,
}

impl SingleInstanceGuard {
    pub fn acquire(name: &str) -> Result<Self, IpcError> {
        Ok(Self {
            inner: platform::InstanceGuard::acquire(name)?,
        })
    }

    pub fn name(&self) -> &str {
        self.inner.name()
    }
}

pub fn default_pipe_name() -> Result<String, IpcError> {
    let user = platform::current_user_identity()?;
    Ok(format!(
        r"\\.\pipe\{}.{}",
        DEFAULT_PIPE_BASE,
        fnv1a64(user.as_bytes())
    ))
}

pub fn default_instance_name() -> Result<String, IpcError> {
    let user = platform::current_user_identity()?;
    Ok(format!(
        r"Local\LumiControl.Agent.v2.{:016x}",
        fnv1a64(user.as_bytes())
    ))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Debug)]
pub enum IpcError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidFrame(String),
    FrameTooLarge(usize),
    IncompatibleApi { received: u16, supported: u16 },
    AlreadyRunning(String),
    UnsupportedPlatform,
    ServerStopped,
}

impl fmt::Display for IpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IpcError::Io(error) => write!(formatter, "IPC I/O error: {error}"),
            IpcError::Json(error) => write!(formatter, "IPC JSON error: {error}"),
            IpcError::InvalidFrame(message) => write!(formatter, "invalid IPC frame: {message}"),
            IpcError::FrameTooLarge(size) => write!(formatter, "IPC frame is too large: {size}"),
            IpcError::IncompatibleApi {
                received,
                supported,
            } => write!(
                formatter,
                "incompatible IPC API {received}; this Agent supports {supported}"
            ),
            IpcError::AlreadyRunning(name) => {
                write!(formatter, "instance {name} is already running")
            }
            IpcError::UnsupportedPlatform => {
                formatter.write_str("Windows named pipes are not supported on this platform")
            }
            IpcError::ServerStopped => formatter.write_str("IPC server stopped"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<std::io::Error> for IpcError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for IpcError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(windows)]
mod platform {
    use super::{IpcError, IpcRequest, IpcResponse, RequestHandler, MAX_IPC_FRAME_BYTES};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::{self, JoinHandle};

    type Bool = i32;
    type Dword = u32;
    type Handle = *mut c_void;
    type LocalHandle = *mut c_void;

    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    const ERROR_ALREADY_EXISTS: Dword = 183;
    const ERROR_PIPE_CONNECTED: Dword = 535;
    const GENERIC_READ: Dword = 0x80000000;
    const GENERIC_WRITE: Dword = 0x40000000;
    const OPEN_EXISTING: Dword = 3;
    const PIPE_ACCESS_DUPLEX: Dword = 0x00000003;
    const PIPE_TYPE_BYTE: Dword = 0;
    const PIPE_READMODE_BYTE: Dword = 0;
    const PIPE_WAIT: Dword = 0;
    const PIPE_REJECT_REMOTE_CLIENTS: Dword = 0x00000008;
    const PIPE_UNLIMITED_INSTANCES: Dword = 255;
    const TOKEN_QUERY: Dword = 0x0008;
    const TOKEN_USER_CLASS: Dword = 1;
    const SDDL_REVISION_1: Dword = 1;

    #[repr(C)]
    struct SecurityAttributes {
        length: Dword,
        security_descriptor: *mut c_void,
        inherit_handle: Bool,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: Dword,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TokenUser {
        user: SidAndAttributes,
    }

    #[link(name = "Kernel32")]
    extern "system" {
        fn CreateNamedPipeW(
            name: *const u16,
            open_mode: Dword,
            pipe_mode: Dword,
            max_instances: Dword,
            output_buffer_size: Dword,
            input_buffer_size: Dword,
            default_timeout: Dword,
            security_attributes: *mut SecurityAttributes,
        ) -> Handle;
        fn ConnectNamedPipe(pipe: Handle, overlapped: *mut c_void) -> Bool;
        fn DisconnectNamedPipe(pipe: Handle) -> Bool;
        fn WaitNamedPipeW(name: *const u16, timeout: Dword) -> Bool;
        fn CreateFileW(
            name: *const u16,
            desired_access: Dword,
            share_mode: Dword,
            security_attributes: *mut SecurityAttributes,
            creation_disposition: Dword,
            flags: Dword,
            template: Handle,
        ) -> Handle;
        fn ReadFile(
            file: Handle,
            buffer: *mut c_void,
            bytes_to_read: Dword,
            bytes_read: *mut Dword,
            overlapped: *mut c_void,
        ) -> Bool;
        fn WriteFile(
            file: Handle,
            buffer: *const c_void,
            bytes_to_write: Dword,
            bytes_written: *mut Dword,
            overlapped: *mut c_void,
        ) -> Bool;
        fn FlushFileBuffers(file: Handle) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
        fn GetLastError() -> Dword;
        fn GetCurrentProcess() -> Handle;
        fn LocalFree(memory: LocalHandle) -> LocalHandle;
        fn CreateMutexW(
            security_attributes: *mut SecurityAttributes,
            initial_owner: Bool,
            name: *const u16,
        ) -> Handle;
        fn ReleaseMutex(mutex: Handle) -> Bool;
    }

    #[link(name = "Advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, desired_access: Dword, token: *mut Handle) -> Bool;
        fn GetTokenInformation(
            token: Handle,
            information_class: Dword,
            information: *mut c_void,
            information_length: Dword,
            return_length: *mut Dword,
        ) -> Bool;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor: *const u16,
            revision: Dword,
            security_descriptor: *mut *mut c_void,
            size: *mut Dword,
        ) -> Bool;
    }

    #[link(name = "Advapi32")]
    extern "system" {
        fn ConvertSidToStringSidW(sid: *mut c_void, string_sid: *mut *mut u16) -> Bool;
    }

    struct OwnedHandle(Handle);

    unsafe impl Send for OwnedHandle {}

    impl OwnedHandle {
        fn new(handle: Handle, operation: &str) -> Result<Self, IpcError> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                Err(last_error(operation))
            } else {
                Ok(Self(handle))
            }
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    struct OwnedSecurityDescriptor(*mut c_void);

    impl Drop for OwnedSecurityDescriptor {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0) };
        }
    }

    pub(super) struct Server {
        pipe_name: String,
        stop: Arc<AtomicBool>,
        join: Option<JoinHandle<()>>,
    }

    impl Server {
        pub(super) fn bind(
            pipe_name: String,
            handler: Arc<RequestHandler>,
        ) -> Result<Self, IpcError> {
            validate_pipe_name(&pipe_name)?;
            let stop = Arc::new(AtomicBool::new(false));
            let ready = Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
            let thread_stop = Arc::clone(&stop);
            let thread_ready = Arc::clone(&ready);
            let thread_name = pipe_name.clone();
            let join = thread::Builder::new()
                .name("lumi-ipc-accept".to_string())
                .spawn(move || {
                    let first = create_server_pipe(&thread_name);
                    {
                        let (lock, wake) = &*thread_ready;
                        *lock.lock().expect("IPC ready mutex poisoned") =
                            Some(first.as_ref().map(|_| ()).map_err(ToString::to_string));
                        wake.notify_one();
                    }
                    let mut pending = match first {
                        Ok(pipe) => Some(pipe),
                        Err(_) => return,
                    };
                    while !thread_stop.load(Ordering::Acquire) {
                        let pipe = pending.take().expect("pending pipe exists");
                        let connected = unsafe { ConnectNamedPipe(pipe.0, null_mut()) } != 0
                            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
                        if !connected {
                            pending = create_server_pipe(&thread_name).ok();
                            if pending.is_none() {
                                break;
                            }
                            continue;
                        }
                        if thread_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let client_handler = Arc::clone(&handler);
                        let _ = thread::Builder::new()
                            .name("lumi-ipc-client".to_string())
                            .spawn(move || serve_client(pipe, client_handler));
                        pending = create_server_pipe(&thread_name).ok();
                        if pending.is_none() {
                            break;
                        }
                    }
                })?;
            let (lock, wake) = &*ready;
            let mut result = lock.lock().expect("IPC ready mutex poisoned");
            while result.is_none() {
                result = wake.wait(result).expect("IPC ready mutex poisoned");
            }
            if let Some(Err(message)) = result.take() {
                let _ = join.join();
                return Err(IpcError::InvalidFrame(message));
            }
            Ok(Self {
                pipe_name,
                stop,
                join: Some(join),
            })
        }

        pub(super) fn pipe_name(&self) -> &str {
            &self.pipe_name
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            let _ = Client::connect(&self.pipe_name, 200);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    pub(super) struct Client {
        handle: OwnedHandle,
    }

    impl Client {
        pub(super) fn connect(pipe_name: &str, timeout_ms: u32) -> Result<Self, IpcError> {
            validate_pipe_name(pipe_name)?;
            let name = wide_null(pipe_name);
            if unsafe { WaitNamedPipeW(name.as_ptr(), timeout_ms) } == 0 {
                return Err(last_error("WaitNamedPipeW"));
            }
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null_mut(),
                    OPEN_EXISTING,
                    0,
                    null_mut(),
                )
            };
            Ok(Self {
                handle: OwnedHandle::new(handle, "CreateFileW(named pipe)")?,
            })
        }

        pub(super) fn write_message<T: serde::Serialize>(
            &mut self,
            value: &T,
        ) -> Result<(), IpcError> {
            write_message(self.handle.0, value)
        }

        pub(super) fn read_message<T: serde::de::DeserializeOwned>(
            &mut self,
        ) -> Result<T, IpcError> {
            read_message(self.handle.0)
        }
    }

    pub(super) struct InstanceGuard {
        name: String,
        handle: OwnedHandle,
    }

    impl InstanceGuard {
        pub(super) fn acquire(name: &str) -> Result<Self, IpcError> {
            let wide = wide_null(name);
            let handle = unsafe { CreateMutexW(null_mut(), 1, wide.as_ptr()) };
            let handle = OwnedHandle::new(handle, "CreateMutexW")?;
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                return Err(IpcError::AlreadyRunning(name.to_string()));
            }
            Ok(Self {
                name: name.to_string(),
                handle,
            })
        }

        pub(super) fn name(&self) -> &str {
            &self.name
        }
    }

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            unsafe { ReleaseMutex(self.handle.0) };
        }
    }

    pub(super) fn current_user_identity() -> Result<String, IpcError> {
        current_user_sid()
    }

    fn create_server_pipe(pipe_name: &str) -> Result<OwnedHandle, IpcError> {
        let sid = current_user_sid()?;
        let sddl = wide_null(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor = null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            ));
        }
        let descriptor = OwnedSecurityDescriptor(descriptor);
        let mut attributes = SecurityAttributes {
            length: size_of::<SecurityAttributes>() as Dword,
            security_descriptor: descriptor.0,
            inherit_handle: 0,
        };
        let name = wide_null(pipe_name);
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &mut attributes,
            )
        };
        OwnedHandle::new(handle, "CreateNamedPipeW")
    }

    fn serve_client(pipe: OwnedHandle, handler: Arc<RequestHandler>) {
        while let Ok(request) = read_message::<IpcRequest>(pipe.0) {
            let response = if let Err(error) = request.validate() {
                IpcResponse::failure(
                    request.id,
                    super::IpcErrorCode::IncompatibleApi,
                    error.to_string(),
                )
            } else {
                handler(request)
            };
            if write_message(pipe.0, &response).is_err() {
                break;
            }
        }
        unsafe {
            FlushFileBuffers(pipe.0);
            DisconnectNamedPipe(pipe.0);
        }
    }

    fn write_message<T: serde::Serialize>(handle: Handle, value: &T) -> Result<(), IpcError> {
        let frame = super::encode_length_prefixed(value)?;
        write_all(handle, &frame)
    }

    fn read_message<T: serde::de::DeserializeOwned>(handle: Handle) -> Result<T, IpcError> {
        let mut prefix = [0u8; 4];
        read_exact(handle, &mut prefix)?;
        let length = u32::from_le_bytes(prefix) as usize;
        if length > MAX_IPC_FRAME_BYTES {
            return Err(IpcError::FrameTooLarge(length));
        }
        let mut data = vec![0u8; length];
        read_exact(handle, &mut data)?;
        Ok(serde_json::from_slice(&data)?)
    }

    fn write_all(handle: Handle, mut bytes: &[u8]) -> Result<(), IpcError> {
        while !bytes.is_empty() {
            let amount = bytes.len().min(u32::MAX as usize) as Dword;
            let mut written = 0;
            if unsafe {
                WriteFile(
                    handle,
                    bytes.as_ptr().cast(),
                    amount,
                    &mut written,
                    null_mut(),
                )
            } == 0
            {
                return Err(last_error("WriteFile(named pipe)"));
            }
            if written == 0 {
                return Err(IpcError::ServerStopped);
            }
            bytes = &bytes[written as usize..];
        }
        Ok(())
    }

    fn read_exact(handle: Handle, mut bytes: &mut [u8]) -> Result<(), IpcError> {
        while !bytes.is_empty() {
            let amount = bytes.len().min(u32::MAX as usize) as Dword;
            let mut read = 0;
            if unsafe {
                ReadFile(
                    handle,
                    bytes.as_mut_ptr().cast(),
                    amount,
                    &mut read,
                    null_mut(),
                )
            } == 0
            {
                return Err(last_error("ReadFile(named pipe)"));
            }
            if read == 0 {
                return Err(IpcError::ServerStopped);
            }
            let (_, remaining) = bytes.split_at_mut(read as usize);
            bytes = remaining;
        }
        Ok(())
    }

    fn current_user_sid() -> Result<String, IpcError> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        let token = OwnedHandle::new(token, "OpenProcessToken")?;
        let mut length = 0;
        unsafe {
            GetTokenInformation(token.0, TOKEN_USER_CLASS, null_mut(), 0, &mut length);
        }
        if length < size_of::<TokenUser>() as Dword {
            return Err(last_error("GetTokenInformation(size)"));
        }
        let mut buffer = vec![0u8; length as usize];
        if unsafe {
            GetTokenInformation(
                token.0,
                TOKEN_USER_CLASS,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(last_error("GetTokenInformation"));
        }
        let token_user = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TokenUser>()) };
        let mut string_sid = null_mut();
        if unsafe { ConvertSidToStringSidW(token_user.user.sid, &mut string_sid) } == 0 {
            return Err(last_error("ConvertSidToStringSidW"));
        }
        let mut length = 0usize;
        while unsafe { *string_sid.add(length) } != 0 {
            length += 1;
        }
        let sid =
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(string_sid, length) });
        unsafe { LocalFree(string_sid.cast()) };
        Ok(sid)
    }

    fn validate_pipe_name(name: &str) -> Result<(), IpcError> {
        if !name.starts_with(r"\\.\pipe\") || name.len() > 240 {
            return Err(IpcError::InvalidFrame(format!(
                "invalid named pipe path: {name}"
            )));
        }
        Ok(())
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(operation: &str) -> IpcError {
        IpcError::Io(std::io::Error::other(format!(
            "{operation} failed: {}",
            std::io::Error::last_os_error()
        )))
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{IpcError, IpcRequest, IpcResponse, RequestHandler};
    use std::sync::Arc;

    pub(super) struct Server;
    impl Server {
        pub(super) fn bind(_name: String, _handler: Arc<RequestHandler>) -> Result<Self, IpcError> {
            Err(IpcError::UnsupportedPlatform)
        }
        pub(super) fn pipe_name(&self) -> &str {
            ""
        }
    }

    pub(super) struct Client;
    impl Client {
        pub(super) fn connect(_name: &str, _timeout: u32) -> Result<Self, IpcError> {
            Err(IpcError::UnsupportedPlatform)
        }
        pub(super) fn write_message<T: serde::Serialize>(
            &mut self,
            _value: &T,
        ) -> Result<(), IpcError> {
            Err(IpcError::UnsupportedPlatform)
        }
        pub(super) fn read_message<T: serde::de::DeserializeOwned>(
            &mut self,
        ) -> Result<T, IpcError> {
            Err(IpcError::UnsupportedPlatform)
        }
    }

    pub(super) struct InstanceGuard;
    impl InstanceGuard {
        pub(super) fn acquire(_name: &str) -> Result<Self, IpcError> {
            Err(IpcError::UnsupportedPlatform)
        }
        pub(super) fn name(&self) -> &str {
            ""
        }
    }

    pub(super) fn current_user_identity() -> Result<String, IpcError> {
        Err(IpcError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_prefixed_json_round_trips() {
        let request = IpcRequest::new(7, AgentCommand::SetPaused { paused: true });
        let frame = encode_length_prefixed(&request).unwrap();
        assert_eq!(
            decode_length_prefixed::<IpcRequest>(&frame).unwrap(),
            request
        );
    }

    #[test]
    fn malformed_lengths_are_rejected() {
        let mut frame = encode_length_prefixed(&IpcRequest::new(1, AgentCommand::Ping)).unwrap();
        frame[0] = frame[0].wrapping_add(1);
        assert!(matches!(
            decode_length_prefixed::<IpcRequest>(&frame),
            Err(IpcError::InvalidFrame(_))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn same_user_named_pipe_supports_persistent_round_trips() {
        let name = format!(
            r"\\.\pipe\LumiControl.Test.{}.{}",
            std::process::id(),
            unix_millis()
        );
        let handler: Arc<RequestHandler> = Arc::new(|request| {
            IpcResponse::success(
                request.id,
                ResponsePayload::Pong {
                    agent_version: "test".to_string(),
                },
            )
        });
        let server = NamedPipeServer::bind(name.clone(), handler).unwrap();
        assert_eq!(server.pipe_name(), name);
        let mut client = NamedPipeClient::connect(&name, 1_000).unwrap();
        for _ in 0..3 {
            let response = client.call(AgentCommand::Ping).unwrap();
            assert!(matches!(
                response.result,
                Some(ResponsePayload::Pong { .. })
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn single_instance_mutex_rejects_a_second_owner() {
        let name = format!(
            r"Local\LumiControl.Test.{}.{}",
            std::process::id(),
            unix_millis()
        );
        let first = SingleInstanceGuard::acquire(&name).unwrap();
        assert_eq!(first.name(), name);
        assert!(matches!(
            SingleInstanceGuard::acquire(&name),
            Err(IpcError::AlreadyRunning(_))
        ));
    }
}
