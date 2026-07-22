use lumi_core::{normalize_brightness, TransitionPlan, TransitionSpec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorIdentity {
    pub display_path: String,
    pub device_id: String,
    pub edid: Vec<u8>,
    pub physical_index: u32,
}

impl MonitorIdentity {
    pub fn stable_id(&self) -> String {
        let mut bytes = Vec::with_capacity(
            self.display_path.len() + self.device_id.len() + self.edid.len() + 16,
        );
        bytes.extend_from_slice(self.display_path.to_ascii_lowercase().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(self.device_id.to_ascii_lowercase().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&self.edid);
        bytes.extend_from_slice(&self.physical_index.to_le_bytes());
        format!("monitor-{:016x}", fnv1a64(&bytes))
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BrightnessRange {
    pub minimum_raw: u32,
    pub current_raw: u32,
    pub maximum_raw: u32,
}

impl BrightnessRange {
    pub fn validate(self) -> Result<Self, MonitorError> {
        if self.maximum_raw <= self.minimum_raw {
            return Err(MonitorError::InvalidRange {
                minimum: self.minimum_raw,
                maximum: self.maximum_raw,
            });
        }
        Ok(self)
    }

    pub fn current_percent(self) -> Result<i32, MonitorError> {
        let range = self.validate()?;
        let position = (range.current_raw.saturating_sub(range.minimum_raw)) as f64
            / (range.maximum_raw - range.minimum_raw) as f64;
        Ok(normalize_brightness((position * 100.0).round() as i32))
    }

    pub fn raw_for_percent(self, percent: i32) -> Result<u32, MonitorError> {
        let range = self.validate()?;
        let position = normalize_brightness(percent) as f64 / 100.0;
        Ok(
            (range.minimum_raw as f64 + (range.maximum_raw - range.minimum_raw) as f64 * position)
                .round() as u32,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorDescriptor {
    pub id: String,
    pub display_name: String,
    pub display_path: String,
    pub device_id: String,
    pub brightness: Option<BrightnessRange>,
    pub qualification_error: Option<String>,
}

impl MonitorDescriptor {
    pub fn current_percent(&self) -> Option<i32> {
        self.brightness
            .and_then(|range| range.current_percent().ok())
    }
}

pub trait MonitorSession: Send {
    fn id(&self) -> &str;
    fn read_brightness(&mut self) -> Result<BrightnessRange, MonitorError>;
    fn set_brightness_percent(&mut self, percent: i32) -> Result<(), MonitorError>;
}

pub trait MonitorBackend: Send + Sync {
    fn enumerate(&self) -> Result<Vec<MonitorDescriptor>, MonitorError>;
    fn open(&self, monitor_id: &str) -> Result<Box<dyn MonitorSession>, MonitorError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsMonitorBackend;

impl MonitorBackend for WindowsMonitorBackend {
    fn enumerate(&self) -> Result<Vec<MonitorDescriptor>, MonitorError> {
        platform::enumerate()
    }

    fn open(&self, monitor_id: &str) -> Result<Box<dyn MonitorSession>, MonitorError> {
        platform::open(monitor_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchedulerEvent {
    MonitorOnline {
        monitor_id: String,
        current_percent: i32,
    },
    BrightnessApplied {
        monitor_id: String,
        percent: i32,
    },
    BrightnessObserved {
        monitor_id: String,
        percent: i32,
    },
    TransitionComplete {
        monitor_id: String,
        percent: i32,
    },
    MonitorError {
        monitor_id: String,
        message: String,
    },
    MonitorRemoved {
        monitor_id: String,
    },
    RefreshReady {
        descriptors: Vec<MonitorDescriptor>,
    },
    RefreshFailed {
        message: String,
    },
}

enum WorkerCommand {
    Retarget {
        target_percent: i32,
        spec: TransitionSpec,
    },
    ReadNow,
    Shutdown,
}

struct MonitorWorker {
    tx: Sender<WorkerCommand>,
    join: Option<JoinHandle<()>>,
}

impl MonitorWorker {
    fn stop(mut self) {
        let _ = self.tx.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    fn stop_async(mut self) {
        let _ = self.tx.send(WorkerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = thread::Builder::new()
                .name("lumi-monitor-reaper".to_string())
                .spawn(move || {
                    let _ = join.join();
                });
        }
    }
}

pub struct TransitionScheduler {
    backend: Arc<dyn MonitorBackend>,
    event_tx: Sender<SchedulerEvent>,
    descriptors: BTreeMap<String, MonitorDescriptor>,
    workers: BTreeMap<String, MonitorWorker>,
    refresh_in_progress: Arc<AtomicBool>,
}

impl TransitionScheduler {
    pub fn new(backend: Arc<dyn MonitorBackend>, event_tx: Sender<SchedulerEvent>) -> Self {
        Self {
            backend,
            event_tx,
            descriptors: BTreeMap::new(),
            workers: BTreeMap::new(),
            refresh_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn refresh(&mut self) -> Result<Vec<MonitorDescriptor>, MonitorError> {
        let descriptors = self.backend.enumerate()?;
        self.install_descriptors(descriptors.clone());
        Ok(descriptors)
    }

    pub fn request_refresh(&self) -> bool {
        if self
            .refresh_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        let backend = Arc::clone(&self.backend);
        let event_tx = self.event_tx.clone();
        let in_progress = Arc::clone(&self.refresh_in_progress);
        let spawned = thread::Builder::new()
            .name("lumi-monitor-discovery".to_string())
            .spawn(move || {
                let event = match backend.enumerate() {
                    Ok(descriptors) => SchedulerEvent::RefreshReady { descriptors },
                    Err(error) => SchedulerEvent::RefreshFailed {
                        message: error.to_string(),
                    },
                };
                let _ = event_tx.send(event);
                in_progress.store(false, Ordering::Release);
            });
        if spawned.is_err() {
            self.refresh_in_progress.store(false, Ordering::Release);
            return false;
        }
        true
    }

    pub fn install_descriptors(&mut self, descriptors: Vec<MonitorDescriptor>) {
        let old_ids = self.workers.keys().cloned().collect::<BTreeSet<_>>();
        let new_ids = descriptors
            .iter()
            .filter(|monitor| monitor.brightness.is_some())
            .map(|monitor| monitor.id.clone())
            .collect::<BTreeSet<_>>();
        let old_workers = std::mem::take(&mut self.workers);
        for (_, worker) in old_workers {
            worker.stop_async();
        }
        for id in old_ids.difference(&new_ids) {
            let _ = self.event_tx.send(SchedulerEvent::MonitorRemoved {
                monitor_id: id.clone(),
            });
        }
        for descriptor in &descriptors {
            if descriptor.brightness.is_some() {
                self.start_worker(descriptor.id.clone());
            }
        }
        self.descriptors = descriptors
            .iter()
            .cloned()
            .map(|monitor| (monitor.id.clone(), monitor))
            .collect();
    }

    pub fn descriptors(&self) -> Vec<MonitorDescriptor> {
        self.descriptors.values().cloned().collect()
    }

    pub fn retarget(
        &self,
        monitor_id: &str,
        target_percent: i32,
        spec: TransitionSpec,
    ) -> Result<(), MonitorError> {
        let worker = self
            .workers
            .get(monitor_id)
            .ok_or_else(|| MonitorError::NotFound(monitor_id.to_string()))?;
        worker
            .tx
            .send(WorkerCommand::Retarget {
                target_percent: normalize_brightness(target_percent),
                spec: spec.validate().map_err(|error| {
                    MonitorError::InvalidTransitionConfiguration(error.to_string())
                })?,
            })
            .map_err(|_| MonitorError::WorkerStopped(monitor_id.to_string()))
    }

    pub fn retarget_all(&self, target_percent: i32, spec: TransitionSpec) -> Vec<MonitorError> {
        self.workers
            .keys()
            .filter_map(|id| self.retarget(id, target_percent, spec).err())
            .collect()
    }

    pub fn read_now(&self, monitor_id: &str) -> Result<(), MonitorError> {
        let worker = self
            .workers
            .get(monitor_id)
            .ok_or_else(|| MonitorError::NotFound(monitor_id.to_string()))?;
        worker
            .tx
            .send(WorkerCommand::ReadNow)
            .map_err(|_| MonitorError::WorkerStopped(monitor_id.to_string()))
    }

    fn start_worker(&mut self, monitor_id: String) {
        let (tx, rx) = mpsc::channel();
        let backend = Arc::clone(&self.backend);
        let event_tx = self.event_tx.clone();
        let thread_id = monitor_id.clone();
        let spawned = thread::Builder::new()
            .name(format!("lumi-monitor-{}", short_id(&monitor_id)))
            .spawn(move || run_monitor_worker(backend, thread_id, rx, event_tx));
        match spawned {
            Ok(join) => {
                self.workers.insert(
                    monitor_id,
                    MonitorWorker {
                        tx,
                        join: Some(join),
                    },
                );
            }
            Err(error) => {
                let _ = self.event_tx.send(SchedulerEvent::MonitorError {
                    monitor_id,
                    message: format!("could not start monitor worker: {error}"),
                });
            }
        }
    }
}

impl Drop for TransitionScheduler {
    fn drop(&mut self) {
        let workers = std::mem::take(&mut self.workers);
        for (_, worker) in workers {
            worker.stop();
        }
    }
}

fn short_id(id: &str) -> &str {
    id.rsplit('-').next().unwrap_or(id)
}

fn run_monitor_worker(
    backend: Arc<dyn MonitorBackend>,
    monitor_id: String,
    rx: Receiver<WorkerCommand>,
    event_tx: Sender<SchedulerEvent>,
) {
    let mut session = match backend.open(&monitor_id) {
        Ok(session) => session,
        Err(error) => {
            let _ = event_tx.send(SchedulerEvent::MonitorError {
                monitor_id,
                message: error.to_string(),
            });
            return;
        }
    };
    let mut current = match session
        .read_brightness()
        .and_then(BrightnessRange::current_percent)
    {
        Ok(current) => current,
        Err(error) => {
            let _ = event_tx.send(SchedulerEvent::MonitorError {
                monitor_id,
                message: error.to_string(),
            });
            return;
        }
    };
    let _ = event_tx.send(SchedulerEvent::MonitorOnline {
        monitor_id: monitor_id.clone(),
        current_percent: current,
    });

    let origin = Instant::now();
    let mut plan: Option<TransitionPlan> = None;
    let mut next_write = Instant::now();
    loop {
        let command = if plan.is_some() {
            let timeout = next_write.saturating_duration_since(Instant::now());
            match rx.recv_timeout(timeout) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };

        if let Some(command) = command {
            match command {
                WorkerCommand::Retarget {
                    target_percent,
                    spec,
                } => {
                    let now_ms = elapsed_ms(origin);
                    let next = match plan {
                        Some(active) => active.retarget(now_ms, target_percent),
                        None => TransitionPlan::new(current, target_percent, now_ms, spec),
                    };
                    match next {
                        Ok(next) if next.target() == current => {
                            plan = None;
                            let _ = event_tx.send(SchedulerEvent::TransitionComplete {
                                monitor_id: monitor_id.clone(),
                                percent: current,
                            });
                        }
                        Ok(next) => {
                            plan = Some(next);
                            next_write = Instant::now();
                        }
                        Err(error) => {
                            let _ = event_tx.send(SchedulerEvent::MonitorError {
                                monitor_id: monitor_id.clone(),
                                message: error.to_string(),
                            });
                        }
                    }
                }
                WorkerCommand::ReadNow => match session
                    .read_brightness()
                    .and_then(BrightnessRange::current_percent)
                {
                    Ok(value) => {
                        current = value;
                        let _ = event_tx.send(SchedulerEvent::BrightnessObserved {
                            monitor_id: monitor_id.clone(),
                            percent: current,
                        });
                    }
                    Err(error) => {
                        let _ = event_tx.send(SchedulerEvent::MonitorError {
                            monitor_id: monitor_id.clone(),
                            message: error.to_string(),
                        });
                    }
                },
                WorkerCommand::Shutdown => break,
            }
            continue;
        }

        let Some(active) = plan else {
            continue;
        };
        let now_ms = elapsed_ms(origin);
        let value = active.value_at(now_ms);
        let complete = active.is_complete(now_ms);
        if value != current || complete {
            match session.set_brightness_percent(value) {
                Ok(()) => {
                    current = value;
                    let _ = event_tx.send(SchedulerEvent::BrightnessApplied {
                        monitor_id: monitor_id.clone(),
                        percent: current,
                    });
                }
                Err(error) => {
                    plan = None;
                    let _ = event_tx.send(SchedulerEvent::MonitorError {
                        monitor_id: monitor_id.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            }
        }
        if complete {
            plan = None;
            let _ = event_tx.send(SchedulerEvent::TransitionComplete {
                monitor_id: monitor_id.clone(),
                percent: current,
            });
        } else {
            next_write = Instant::now() + Duration::from_millis(active.minimum_write_interval_ms());
        }
    }
}

fn elapsed_ms(origin: Instant) -> u64 {
    origin.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Debug)]
pub enum MonitorError {
    Platform(String),
    NotFound(String),
    UnsupportedPlatform,
    UnsupportedBrightness(String),
    InvalidRange { minimum: u32, maximum: u32 },
    InvalidTransitionConfiguration(String),
    WorkerStopped(String),
}

impl fmt::Display for MonitorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MonitorError::Platform(message) => formatter.write_str(message),
            MonitorError::NotFound(id) => write!(formatter, "monitor {id} was not found"),
            MonitorError::UnsupportedPlatform => {
                formatter.write_str("monitor control is only supported on Windows")
            }
            MonitorError::UnsupportedBrightness(id) => {
                write!(formatter, "monitor {id} does not expose DDC/CI brightness")
            }
            MonitorError::InvalidRange { minimum, maximum } => {
                write!(
                    formatter,
                    "invalid monitor brightness range {minimum}..={maximum}"
                )
            }
            MonitorError::InvalidTransitionConfiguration(message) => formatter.write_str(message),
            MonitorError::WorkerStopped(id) => write!(formatter, "monitor worker {id} stopped"),
        }
    }
}

impl std::error::Error for MonitorError {}

#[cfg(windows)]
mod platform {
    use super::{
        BrightnessRange, MonitorDescriptor, MonitorError, MonitorIdentity, MonitorSession,
    };
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    type Bool = i32;
    type Dword = u32;
    type Lparam = isize;
    type Handle = *mut c_void;
    type Hmonitor = Handle;
    type Hdc = Handle;
    type Hkey = Handle;
    type Lstatus = i32;

    const EDD_GET_DEVICE_INTERFACE_NAME: Dword = 0x00000001;
    const KEY_READ: Dword = 0x00020019;
    const ERROR_SUCCESS: Lstatus = 0;
    const HKEY_LOCAL_MACHINE: Hkey = 0x80000002usize as Hkey;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct MonitorInfoExW {
        cb_size: Dword,
        monitor: Rect,
        work: Rect,
        flags: Dword,
        device: [u16; 32],
    }

    #[repr(C)]
    struct DisplayDeviceW {
        cb: Dword,
        device_name: [u16; 32],
        device_string: [u16; 128],
        state_flags: Dword,
        device_id: [u16; 128],
        device_key: [u16; 128],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PhysicalMonitor {
        handle: Handle,
        description: [u16; 128],
    }

    #[link(name = "User32")]
    extern "system" {
        fn EnumDisplayMonitors(
            hdc: Hdc,
            clip: *const Rect,
            callback: Option<unsafe extern "system" fn(Hmonitor, Hdc, *mut Rect, Lparam) -> Bool>,
            data: Lparam,
        ) -> Bool;
        fn GetMonitorInfoW(hmonitor: Hmonitor, info: *mut MonitorInfoExW) -> Bool;
        fn EnumDisplayDevicesW(
            device: *const u16,
            device_number: Dword,
            display_device: *mut DisplayDeviceW,
            flags: Dword,
        ) -> Bool;
    }

    #[link(name = "Dxva2")]
    extern "system" {
        fn GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor: Hmonitor, count: *mut Dword) -> Bool;
        fn GetPhysicalMonitorsFromHMONITOR(
            hmonitor: Hmonitor,
            array_size: Dword,
            array: *mut PhysicalMonitor,
        ) -> Bool;
        fn DestroyPhysicalMonitor(handle: Handle) -> Bool;
        fn DestroyPhysicalMonitors(array_size: Dword, array: *mut PhysicalMonitor) -> Bool;
        fn GetMonitorBrightness(
            handle: Handle,
            minimum: *mut Dword,
            current: *mut Dword,
            maximum: *mut Dword,
        ) -> Bool;
        fn SetMonitorBrightness(handle: Handle, brightness: Dword) -> Bool;
    }

    #[link(name = "Advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            key: Hkey,
            subkey: *const u16,
            options: Dword,
            access: Dword,
            result: *mut Hkey,
        ) -> Lstatus;
        fn RegQueryValueExW(
            key: Hkey,
            value_name: *const u16,
            reserved: *mut Dword,
            value_type: *mut Dword,
            data: *mut u8,
            data_size: *mut Dword,
        ) -> Lstatus;
        fn RegCloseKey(key: Hkey) -> Lstatus;
    }

    struct LogicalMonitor {
        handle: Hmonitor,
        display_path: String,
        identity_path: String,
        device_id: String,
        display_name: String,
        edid: Vec<u8>,
    }

    struct NativeSession {
        id: String,
        handle: Handle,
        range: BrightnessRange,
    }

    unsafe impl Send for NativeSession {}

    impl Drop for NativeSession {
        fn drop(&mut self) {
            unsafe {
                DestroyPhysicalMonitor(self.handle);
            }
        }
    }

    impl MonitorSession for NativeSession {
        fn id(&self) -> &str {
            &self.id
        }

        fn read_brightness(&mut self) -> Result<BrightnessRange, MonitorError> {
            self.range = read_range(self.handle)?;
            Ok(self.range)
        }

        fn set_brightness_percent(&mut self, percent: i32) -> Result<(), MonitorError> {
            let raw = self.range.raw_for_percent(percent)?;
            if unsafe { SetMonitorBrightness(self.handle, raw) } == 0 {
                return Err(last_error("SetMonitorBrightness"));
            }
            self.range.current_raw = raw;
            Ok(())
        }
    }

    pub(super) fn enumerate() -> Result<Vec<MonitorDescriptor>, MonitorError> {
        let logical = logical_monitors()?;
        let mut descriptors = Vec::new();
        for monitor in logical {
            let mut physical = physical_monitors(monitor.handle)?;
            for (index, item) in physical.iter().enumerate() {
                let identity = MonitorIdentity {
                    display_path: monitor.identity_path.clone(),
                    device_id: monitor.device_id.clone(),
                    edid: monitor.edid.clone(),
                    physical_index: index as u32,
                };
                let range = read_range(item.handle);
                descriptors.push(MonitorDescriptor {
                    id: identity.stable_id(),
                    display_name: nonempty(
                        wide_to_string(&item.description),
                        &monitor.display_name,
                    ),
                    display_path: monitor.display_path.clone(),
                    device_id: monitor.device_id.clone(),
                    brightness: range.as_ref().ok().copied(),
                    qualification_error: range.err().map(|error| error.to_string()),
                });
            }
            if !physical.is_empty() {
                unsafe {
                    DestroyPhysicalMonitors(physical.len() as Dword, physical.as_mut_ptr());
                }
            }
        }
        descriptors.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(descriptors)
    }

    pub(super) fn open(monitor_id: &str) -> Result<Box<dyn MonitorSession>, MonitorError> {
        for monitor in logical_monitors()? {
            let physical = physical_monitors(monitor.handle)?;
            let selected = physical.iter().enumerate().find_map(|(index, item)| {
                let identity = MonitorIdentity {
                    display_path: monitor.identity_path.clone(),
                    device_id: monitor.device_id.clone(),
                    edid: monitor.edid.clone(),
                    physical_index: index as u32,
                };
                let id = identity.stable_id();
                (id == monitor_id).then_some((index, id, item.handle))
            });
            if let Some((selected_index, id, handle)) = selected {
                for (index, item) in physical.iter().enumerate() {
                    if index != selected_index {
                        unsafe { DestroyPhysicalMonitor(item.handle) };
                    }
                }
                let range = match read_range(handle) {
                    Ok(range) => range,
                    Err(error) => {
                        unsafe { DestroyPhysicalMonitor(handle) };
                        return Err(error);
                    }
                };
                return Ok(Box::new(NativeSession { id, handle, range }));
            }
            for item in physical {
                unsafe {
                    DestroyPhysicalMonitor(item.handle);
                }
            }
        }
        Err(MonitorError::NotFound(monitor_id.to_string()))
    }

    fn logical_monitors() -> Result<Vec<LogicalMonitor>, MonitorError> {
        let mut handles = Vec::<Hmonitor>::new();
        if unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(collect_monitor),
                &mut handles as *mut _ as Lparam,
            )
        } == 0
        {
            return Err(last_error("EnumDisplayMonitors"));
        }
        let mut monitors = Vec::with_capacity(handles.len());
        for handle in handles {
            let mut info: MonitorInfoExW = unsafe { zeroed() };
            info.cb_size = size_of::<MonitorInfoExW>() as Dword;
            if unsafe { GetMonitorInfoW(handle, &mut info) } == 0 {
                continue;
            }
            let display_path = wide_to_string(&info.device);
            let display_path_wide = wide_null(&display_path);
            let mut device: DisplayDeviceW = unsafe { zeroed() };
            device.cb = size_of::<DisplayDeviceW>() as Dword;
            let found = unsafe {
                EnumDisplayDevicesW(
                    display_path_wide.as_ptr(),
                    0,
                    &mut device,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            } != 0;
            let device_id = if found {
                wide_to_string(&device.device_id)
            } else {
                String::new()
            };
            let device_key = if found {
                wide_to_string(&device.device_key)
            } else {
                String::new()
            };
            let display_name = found
                .then(|| wide_to_string(&device.device_string))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| display_path.clone());
            let edid = read_edid(&device_key).unwrap_or_default();
            let identity_path = if device_id.is_empty() {
                display_path.clone()
            } else {
                device_id.clone()
            };
            monitors.push(LogicalMonitor {
                handle,
                display_path,
                identity_path,
                device_id,
                display_name,
                edid,
            });
        }
        Ok(monitors)
    }

    unsafe extern "system" fn collect_monitor(
        monitor: Hmonitor,
        _dc: Hdc,
        _rect: *mut Rect,
        data: Lparam,
    ) -> Bool {
        let handles = &mut *(data as *mut Vec<Hmonitor>);
        handles.push(monitor);
        1
    }

    fn physical_monitors(handle: Hmonitor) -> Result<Vec<PhysicalMonitor>, MonitorError> {
        let mut count = 0;
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(handle, &mut count) } == 0 {
            return Err(last_error("GetNumberOfPhysicalMonitorsFromHMONITOR"));
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut monitors = vec![unsafe { zeroed() }; count as usize];
        if unsafe { GetPhysicalMonitorsFromHMONITOR(handle, count, monitors.as_mut_ptr()) } == 0 {
            return Err(last_error("GetPhysicalMonitorsFromHMONITOR"));
        }
        Ok(monitors)
    }

    fn read_range(handle: Handle) -> Result<BrightnessRange, MonitorError> {
        let (mut minimum, mut current, mut maximum) = (0, 0, 0);
        if unsafe { GetMonitorBrightness(handle, &mut minimum, &mut current, &mut maximum) } == 0 {
            return Err(last_error("GetMonitorBrightness"));
        }
        BrightnessRange {
            minimum_raw: minimum,
            current_raw: current,
            maximum_raw: maximum,
        }
        .validate()
    }

    fn read_edid(device_key: &str) -> Option<Vec<u8>> {
        let key = device_key
            .strip_prefix("\\Registry\\Machine\\")
            .or_else(|| device_key.strip_prefix("\\REGISTRY\\MACHINE\\"))?;
        let key_wide = wide_null(key);
        let mut handle = null_mut();
        if unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                key_wide.as_ptr(),
                0,
                KEY_READ,
                &mut handle,
            )
        } != ERROR_SUCCESS
        {
            return None;
        }
        let value_name = wide_null("EDID");
        let mut value_type = 0;
        let mut size = 0;
        let first = unsafe {
            RegQueryValueExW(
                handle,
                value_name.as_ptr(),
                null_mut(),
                &mut value_type,
                null_mut(),
                &mut size,
            )
        };
        if first != ERROR_SUCCESS || size == 0 || size > 4096 {
            unsafe { RegCloseKey(handle) };
            return None;
        }
        let mut data = vec![0u8; size as usize];
        let second = unsafe {
            RegQueryValueExW(
                handle,
                value_name.as_ptr(),
                null_mut(),
                &mut value_type,
                data.as_mut_ptr(),
                &mut size,
            )
        };
        unsafe { RegCloseKey(handle) };
        (second == ERROR_SUCCESS).then_some(data)
    }

    fn nonempty(primary: String, fallback: &str) -> String {
        if primary.trim().is_empty() {
            fallback.to_string()
        } else {
            primary
        }
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_to_string(value: &[u16]) -> String {
        let len = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        String::from_utf16_lossy(&value[..len]).trim().to_string()
    }

    fn last_error(operation: &str) -> MonitorError {
        MonitorError::Platform(format!(
            "{operation} failed: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{MonitorDescriptor, MonitorError, MonitorSession};

    pub(super) fn enumerate() -> Result<Vec<MonitorDescriptor>, MonitorError> {
        Err(MonitorError::UnsupportedPlatform)
    }

    pub(super) fn open(_monitor_id: &str) -> Result<Box<dyn MonitorSession>, MonitorError> {
        Err(MonitorError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn stable_identity_is_independent_of_enumeration_order() {
        let left = MonitorIdentity {
            display_path: r"\\.\DISPLAY2".to_string(),
            device_id: "MONITOR\\DEL1234\\A".to_string(),
            edid: vec![1, 2, 3, 4],
            physical_index: 0,
        };
        let mut equivalent = left.clone();
        equivalent.display_path = r"\\.\display2".to_string();
        assert_eq!(left.stable_id(), equivalent.stable_id());

        let mut other = left.clone();
        other.physical_index = 1;
        assert_ne!(left.stable_id(), other.stable_id());
    }

    #[test]
    fn brightness_ranges_are_normalized_both_directions() {
        let range = BrightnessRange {
            minimum_raw: 20,
            current_raw: 60,
            maximum_raw: 220,
        };
        assert_eq!(range.current_percent().unwrap(), 20);
        assert_eq!(range.raw_for_percent(20).unwrap(), 60);
        assert_eq!(range.raw_for_percent(150).unwrap(), 220);
    }

    #[derive(Clone)]
    struct FakeBackend {
        states: Arc<BTreeMap<String, Arc<Mutex<FakeState>>>>,
    }

    struct FakeState {
        current: i32,
        delay: Duration,
        writes: Vec<i32>,
    }

    type FakeStates = Arc<BTreeMap<String, Arc<Mutex<FakeState>>>>;

    struct FakeSession {
        id: String,
        state: Arc<Mutex<FakeState>>,
    }

    impl MonitorSession for FakeSession {
        fn id(&self) -> &str {
            &self.id
        }

        fn read_brightness(&mut self) -> Result<BrightnessRange, MonitorError> {
            let state = self.state.lock().unwrap();
            Ok(BrightnessRange {
                minimum_raw: 0,
                current_raw: state.current as u32,
                maximum_raw: 100,
            })
        }

        fn set_brightness_percent(&mut self, percent: i32) -> Result<(), MonitorError> {
            let delay = self.state.lock().unwrap().delay;
            if !delay.is_zero() {
                thread::sleep(delay);
            }
            let mut state = self.state.lock().unwrap();
            state.current = percent;
            state.writes.push(percent);
            Ok(())
        }
    }

    impl MonitorBackend for FakeBackend {
        fn enumerate(&self) -> Result<Vec<MonitorDescriptor>, MonitorError> {
            Ok(self
                .states
                .iter()
                .map(|(id, state)| MonitorDescriptor {
                    id: id.clone(),
                    display_name: id.clone(),
                    display_path: id.clone(),
                    device_id: id.clone(),
                    brightness: Some(BrightnessRange {
                        minimum_raw: 0,
                        current_raw: state.lock().unwrap().current as u32,
                        maximum_raw: 100,
                    }),
                    qualification_error: None,
                })
                .collect())
        }

        fn open(&self, monitor_id: &str) -> Result<Box<dyn MonitorSession>, MonitorError> {
            let state = self
                .states
                .get(monitor_id)
                .cloned()
                .ok_or_else(|| MonitorError::NotFound(monitor_id.to_string()))?;
            Ok(Box::new(FakeSession {
                id: monitor_id.to_string(),
                state,
            }))
        }
    }

    fn fake_backend() -> (Arc<dyn MonitorBackend>, FakeStates) {
        let states = Arc::new(BTreeMap::from([
            (
                "fast".to_string(),
                Arc::new(Mutex::new(FakeState {
                    current: 10,
                    delay: Duration::ZERO,
                    writes: Vec::new(),
                })),
            ),
            (
                "slow".to_string(),
                Arc::new(Mutex::new(FakeState {
                    current: 10,
                    delay: Duration::from_millis(250),
                    writes: Vec::new(),
                })),
            ),
        ]));
        (
            Arc::new(FakeBackend {
                states: Arc::clone(&states),
            }),
            states,
        )
    }

    #[test]
    fn a_slow_monitor_does_not_block_other_targets_or_the_caller() {
        let (backend, states) = fake_backend();
        let (event_tx, _event_rx) = mpsc::channel();
        let mut scheduler = TransitionScheduler::new(backend, event_tx);
        scheduler.refresh().unwrap();
        let spec = TransitionSpec {
            duration_ms: 100,
            max_writes_per_second: 20,
        };
        let started = Instant::now();
        scheduler.retarget("slow", 90, spec).unwrap();
        scheduler.retarget("fast", 90, spec).unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
        thread::sleep(Duration::from_millis(180));
        assert_eq!(states["fast"].lock().unwrap().current, 90);
    }

    #[test]
    fn retargeting_finishes_at_the_latest_target() {
        let (backend, states) = fake_backend();
        let (event_tx, _event_rx) = mpsc::channel();
        let mut scheduler = TransitionScheduler::new(backend, event_tx);
        scheduler.refresh().unwrap();
        let spec = TransitionSpec {
            duration_ms: 200,
            max_writes_per_second: 20,
        };
        scheduler.retarget("fast", 100, spec).unwrap();
        thread::sleep(Duration::from_millis(70));
        scheduler.retarget("fast", 30, spec).unwrap();
        thread::sleep(Duration::from_millis(280));
        assert_eq!(states["fast"].lock().unwrap().current, 30);
    }
}
