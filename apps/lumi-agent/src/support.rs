use lumi_ipc::{unix_millis, AgentSnapshot, ResourceSnapshot};
use lumi_store::{ProductPaths, SettingsDocument, StoreError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const ACTIVE_LOG: &str = "agent.log";
const MAX_LOG_BYTES: u64 = 1024 * 1024;
const LOG_GENERATIONS: usize = 3;
const DIAGNOSTIC_LOG_TAIL_BYTES: u64 = 256 * 1024;
const DIAGNOSTIC_CRASH_BYTES: u64 = 64 * 1024;

static CRASH_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn install_crash_hook(paths: &ProductPaths) {
    if CRASH_HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let directory = paths.crashes.clone();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = fs::create_dir_all(&directory);
        let message = panic_info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| {
                panic_info
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
            })
            .unwrap_or("non-string panic payload");
        let location = panic_info.location().map(|location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        });
        let payload = serde_json::to_vec_pretty(&json!({
            "timestamp_unix_ms": unix_millis(),
            "process_id": std::process::id(),
            "version": env!("CARGO_PKG_VERSION"),
            "message": truncate_utf8(message, 4096),
            "location": location,
        }));
        if let Ok(payload) = payload {
            let path = directory.join(format!(
                "agent-crash-{}-{}.json",
                unix_millis(),
                std::process::id()
            ));
            if let Ok(mut file) = OpenOptions::new().create_new(true).write(true).open(path) {
                let _ = file.write_all(&payload);
                let _ = file.flush();
            }
        }
        previous(panic_info);
    }));
}

fn truncate_utf8(value: &str, maximum_bytes: usize) -> &str {
    if value.len() <= maximum_bytes {
        return value;
    }
    let mut end = maximum_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub trait StartupRegistration: Send + Sync {
    fn set_enabled(&self, enabled: bool) -> Result<(), String>;
}

pub(crate) fn production_startup_registration() -> Result<Arc<dyn StartupRegistration>, String> {
    Ok(Arc::new(PlatformStartupRegistration::new(
        std::env::current_exe().map_err(|error| error.to_string())?,
    )))
}

#[cfg(windows)]
struct PlatformStartupRegistration {
    executable: PathBuf,
}

#[cfg(windows)]
impl PlatformStartupRegistration {
    fn new(executable: PathBuf) -> Self {
        Self { executable }
    }
}

#[cfg(windows)]
impl StartupRegistration for PlatformStartupRegistration {
    fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        use std::ptr::null_mut;
        use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegSetValueExW, HKEY_CURRENT_USER, REG_SZ,
        };

        let subkey = wide_null("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let value_name = wide_null("LumiControl Agent");
        let mut key = null_mut();
        let status = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, subkey.as_ptr(), &mut key) };
        if status != 0 {
            return Err(format!(
                "could not open the per-user startup registry key ({status})"
            ));
        }

        let operation_status = if enabled {
            let command = format!("\"{}\" --background", self.executable.display());
            let command = wide_null(&command);
            unsafe {
                RegSetValueExW(
                    key,
                    value_name.as_ptr(),
                    0,
                    REG_SZ,
                    command.as_ptr().cast(),
                    (command.len() * std::mem::size_of::<u16>()) as u32,
                )
            }
        } else {
            let status = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
            if status == ERROR_FILE_NOT_FOUND {
                0
            } else {
                status
            }
        };
        unsafe { RegCloseKey(key) };
        if operation_status == 0 {
            Ok(())
        } else {
            Err(format!(
                "could not update the per-user startup registration ({operation_status})"
            ))
        }
    }
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(not(windows))]
struct PlatformStartupRegistration;

#[cfg(not(windows))]
impl PlatformStartupRegistration {
    fn new(_executable: PathBuf) -> Self {
        Self
    }
}

#[cfg(not(windows))]
impl StartupRegistration for PlatformStartupRegistration {
    fn set_enabled(&self, _enabled: bool) -> Result<(), String> {
        Ok(())
    }
}

struct LogFile {
    file: File,
    bytes: u64,
}

#[derive(Clone)]
pub(crate) struct EventLogger {
    directory: PathBuf,
    file: Arc<Mutex<Option<LogFile>>>,
}

impl EventLogger {
    pub(crate) fn best_effort(paths: &ProductPaths) -> Self {
        let directory = paths.logs.clone();
        let file = fs::create_dir_all(&directory)
            .and_then(|()| open_log(&directory.join(ACTIVE_LOG)))
            .ok();
        Self {
            directory,
            file: Arc::new(Mutex::new(file)),
        }
    }

    pub(crate) fn info(&self, event: &str, message: impl AsRef<str>) {
        self.write("info", event, message.as_ref());
    }

    pub(crate) fn warn(&self, event: &str, message: impl AsRef<str>) {
        self.write("warn", event, message.as_ref());
    }

    pub(crate) fn error(&self, event: &str, message: impl AsRef<str>) {
        self.write("error", event, message.as_ref());
    }

    pub(crate) fn flush(&self) {
        if let Some(log) = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
        {
            let _ = log.file.flush();
        }
    }

    fn write(&self, level: &str, event: &str, message: &str) {
        let mut line = match serde_json::to_vec(&json!({
            "timestamp_unix_ms": unix_millis(),
            "level": level,
            "event": event,
            "message": message,
            "version": env!("CARGO_PKG_VERSION"),
        })) {
            Ok(line) => line,
            Err(_) => return,
        };
        line.push(b'\n');

        let mut guard = self
            .file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let should_rotate = guard
            .as_ref()
            .is_some_and(|log| log.bytes.saturating_add(line.len() as u64) > MAX_LOG_BYTES);
        if should_rotate {
            if let Some(mut log) = guard.take() {
                let _ = log.file.flush();
            }
            let _ = rotate_logs(&self.directory);
            *guard = open_log(&self.directory.join(ACTIVE_LOG)).ok();
        }
        let Some(log) = guard.as_mut() else {
            return;
        };
        if log.file.write_all(&line).is_ok() {
            log.bytes = log.bytes.saturating_add(line.len() as u64);
            let _ = log.file.flush();
        }
    }
}

fn open_log(path: &Path) -> std::io::Result<LogFile> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let bytes = file.metadata()?.len();
    Ok(LogFile { file, bytes })
}

fn rotate_logs(directory: &Path) -> std::io::Result<()> {
    for generation in (1..LOG_GENERATIONS).rev() {
        let source = directory.join(format!("agent.{generation}.log"));
        let destination = directory.join(format!("agent.{}.log", generation + 1));
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        if source.exists() {
            fs::rename(source, destination)?;
        }
    }
    let active = directory.join(ACTIVE_LOG);
    if active.exists() {
        fs::rename(active, directory.join("agent.1.log"))?;
    }
    Ok(())
}

pub(crate) fn export_diagnostics(
    paths: &ProductPaths,
    snapshot: &AgentSnapshot,
    document: &SettingsDocument,
    logger: &EventLogger,
) -> Result<PathBuf, String> {
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    paths.ensure_directories().map_err(store_error)?;
    logger.flush();
    let output = paths
        .diagnostics
        .join(format!("LumiControl-diagnostics-{}.zip", unix_millis()));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&output)
        .map_err(|error| error.to_string())?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o600);

    write_zip_entry(
        &mut archive,
        "README.txt",
        b"LumiControl diagnostic package\r\nGenerated locally after an explicit user request. Location, device serial number, port names, and monitor identifiers are redacted. API keys and environment variables are never collected. Recent local crash records may be included; nothing is uploaded automatically.\r\n",
        options,
    )?;
    let manifest = serde_json::to_vec_pretty(&json!({
        "product": "LumiControl",
        "agent_version": env!("CARGO_PKG_VERSION"),
        "generated_at_unix_ms": unix_millis(),
        "os": std::env::consts::OS,
        "architecture": std::env::consts::ARCH,
    }))
    .map_err(|error| error.to_string())?;
    write_zip_entry(&mut archive, "manifest.json", &manifest, options)?;

    let sanitized_snapshot = sanitized_snapshot(snapshot);
    let snapshot_json =
        serde_json::to_vec_pretty(&sanitized_snapshot).map_err(|error| error.to_string())?;
    write_zip_entry(
        &mut archive,
        "snapshot.sanitized.json",
        &snapshot_json,
        options,
    )?;

    let sanitized_settings = sanitized_settings(document);
    let settings_json =
        serde_json::to_vec_pretty(&sanitized_settings).map_err(|error| error.to_string())?;
    write_zip_entry(
        &mut archive,
        "settings.sanitized.json",
        &settings_json,
        options,
    )?;

    for name in [ACTIVE_LOG, "agent.1.log", "agent.2.log", "agent.3.log"] {
        let path = paths.logs.join(name);
        if !path.is_file() {
            continue;
        }
        let content =
            read_tail(&path, DIAGNOSTIC_LOG_TAIL_BYTES).map_err(|error| error.to_string())?;
        let content = sanitized_log_records(&content);
        write_zip_entry(&mut archive, &format!("logs/{name}"), &content, options)?;
    }
    let mut crash_files = fs::read_dir(&paths.crashes)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .collect::<Vec<_>>();
    crash_files.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });
    for entry in crash_files.into_iter().take(3) {
        let content =
            read_tail(&entry.path(), DIAGNOSTIC_CRASH_BYTES).map_err(|error| error.to_string())?;
        let content = sanitized_crash_record(&content);
        let name = entry.file_name().to_string_lossy().into_owned();
        write_zip_entry(&mut archive, &format!("crashes/{name}"), &content, options)?;
    }
    archive.finish().map_err(|error| error.to_string())?;
    Ok(output)
}

fn sanitized_log_records(content: &[u8]) -> Vec<u8> {
    let mut sanitized = Vec::new();
    for line in content.split(|byte| *byte == b'\n') {
        let Ok(record) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let safe = json!({
            "timestamp_unix_ms": record.get("timestamp_unix_ms").and_then(Value::as_u64),
            "level": record.get("level").and_then(Value::as_str),
            "event": record.get("event").and_then(Value::as_str),
            "version": record.get("version").and_then(Value::as_str),
            "message": "<redacted>",
        });
        if serde_json::to_writer(&mut sanitized, &safe).is_ok() {
            sanitized.push(b'\n');
        }
    }
    sanitized
}

fn sanitized_crash_record(content: &[u8]) -> Vec<u8> {
    let record = serde_json::from_slice::<Value>(content).ok();
    let timestamp = record
        .as_ref()
        .and_then(|value| value.get("timestamp_unix_ms"))
        .and_then(Value::as_u64);
    let process_id = record
        .as_ref()
        .and_then(|value| value.get("process_id"))
        .and_then(Value::as_u64);
    let sanitized = json!({
        "timestamp_unix_ms": timestamp,
        "process_id": process_id,
        "version": env!("CARGO_PKG_VERSION"),
        "message": "A panic was recorded; details remain local",
        "location": "<redacted>",
    });
    serde_json::to_vec_pretty(&sanitized)
        .unwrap_or_else(|_| b"{\"message\":\"A crash record was present\"}".to_vec())
}

fn write_zip_entry(
    archive: &mut zip::ZipWriter<File>,
    name: &str,
    content: &[u8],
    options: zip::write::SimpleFileOptions,
) -> Result<(), String> {
    archive
        .start_file(name, options)
        .map_err(|error| error.to_string())?;
    archive
        .write_all(content)
        .map_err(|error| error.to_string())
}

fn read_tail(path: &Path, maximum: u64) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let read_length = length.min(maximum);
    file.seek(SeekFrom::Start(length.saturating_sub(read_length)))?;
    let mut content = Vec::with_capacity(read_length as usize);
    file.take(read_length).read_to_end(&mut content)?;
    Ok(content)
}

fn sanitized_snapshot(snapshot: &AgentSnapshot) -> AgentSnapshot {
    let mut sanitized = snapshot.clone();
    sanitized.device.serial_number = sanitized
        .device
        .serial_number
        .as_ref()
        .map(|_| "<redacted>".to_string());
    sanitized.device.port_name = sanitized
        .device
        .port_name
        .as_ref()
        .map(|_| "<redacted>".to_string());
    sanitized.device.last_error = sanitized
        .device
        .last_error
        .as_ref()
        .map(|_| "A device error was present".to_string());
    for (index, monitor) in sanitized.monitors.iter_mut().enumerate() {
        monitor.id = format!("monitor-{}", index + 1);
        monitor.display_name = "<redacted monitor>".to_string();
        monitor.display_path = "<redacted>".to_string();
        monitor.last_error = monitor
            .last_error
            .as_ref()
            .map(|_| "A monitor error was present".to_string());
    }
    sanitized.relay.last_error = sanitized
        .relay
        .last_error
        .as_ref()
        .map(|_| "A relay error was present".to_string());
    sanitized.relay.matched_rule_id = sanitized
        .relay
        .matched_rule_id
        .as_ref()
        .map(|_| "<redacted>".to_string());
    sanitized.relay.matched_rule_name = sanitized
        .relay
        .matched_rule_name
        .as_ref()
        .map(|_| "<redacted rule>".to_string());
    sanitized.environment.timezone = sanitized
        .environment
        .timezone
        .as_ref()
        .map(|_| "<redacted>".to_string());
    sanitized.environment.last_error = sanitized
        .environment
        .last_error
        .as_ref()
        .map(|_| "An environment data error was present".to_string());
    if sanitized.configuration_warning.is_some() {
        sanitized.configuration_warning =
            Some("A settings recovery warning was present".to_string());
    }
    sanitized
}

fn sanitized_settings(document: &SettingsDocument) -> SettingsDocument {
    let mut sanitized = document.clone();
    sanitized.settings.weather.location_name =
        if sanitized.settings.weather.location_name.is_empty() {
            String::new()
        } else {
            "<redacted>".to_string()
        };
    sanitized.settings.weather.latitude = 0.0;
    sanitized.settings.weather.longitude = 0.0;
    if !sanitized.settings.weather.timezone.is_empty() {
        sanitized.settings.weather.timezone = "<redacted>".to_string();
    }
    sanitized.migration.source_path = None;
    sanitized.migration.legacy_device_port = None;
    sanitized.migration.legacy_monitor_calibrations = None;
    if !sanitized.migration.warnings.is_empty() {
        sanitized.migration.warnings = vec!["Migration warnings were present".to_string()];
    }
    let profiles = sanitized
        .settings
        .monitors
        .values()
        .cloned()
        .enumerate()
        .map(|(index, mut profile)| {
            profile.display_name = "<redacted monitor>".to_string();
            (format!("monitor-{}", index + 1), profile)
        })
        .collect::<BTreeMap<_, _>>();
    sanitized.settings.monitors = profiles;
    sanitized
}

fn store_error(error: StoreError) -> String {
    error.to_string()
}

pub(crate) fn sample_process_resources() -> ResourceSnapshot {
    let mut resources = ResourceSnapshot {
        process_id: std::process::id(),
        ..ResourceSnapshot::default()
    };
    sample_platform_resources(&mut resources);
    resources
}

#[cfg(windows)]
fn sample_platform_resources(resources: &mut ResourceSnapshot) {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessHandleCount, GetProcessTimes,
    };

    let process = unsafe { GetCurrentProcess() };
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    if unsafe {
        K32GetProcessMemoryInfo(
            process,
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    } != 0
    {
        resources.working_set_bytes = Some(counters.WorkingSetSize as u64);
    }
    let mut handles = 0;
    if unsafe { GetProcessHandleCount(process, &mut handles) } != 0 {
        resources.handle_count = Some(handles);
    }
    let mut creation: FILETIME = unsafe { zeroed() };
    let mut exit: FILETIME = unsafe { zeroed() };
    let mut kernel: FILETIME = unsafe { zeroed() };
    let mut user: FILETIME = unsafe { zeroed() };
    if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) } != 0 {
        let kernel_100ns =
            (u64::from(kernel.dwHighDateTime) << 32) | u64::from(kernel.dwLowDateTime);
        let user_100ns = (u64::from(user.dwHighDateTime) << 32) | u64::from(user.dwLowDateTime);
        resources.cpu_time_ms = Some(kernel_100ns.saturating_add(user_100ns) / 10_000);
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return;
    }
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    let mut count = 0u32;
    let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == resources.process_id {
            count = count.saturating_add(1);
        }
        has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    resources.thread_count = Some(count);
}

#[cfg(not(windows))]
fn sample_platform_resources(_resources: &mut ResourceSnapshot) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    fn test_paths() -> ProductPaths {
        ProductPaths::under(std::env::temp_dir().join(format!(
            "lumi-support-test-{}-{}-{}",
            std::process::id(),
            unix_millis(),
            NEXT_TEST.fetch_add(1, Ordering::Relaxed)
        )))
    }

    #[test]
    fn diagnostics_are_bounded_and_redacted() {
        let paths = test_paths();
        let logger = EventLogger::best_effort(&paths);
        logger.info("test", "C:\\Users\\Private diagnostic detail");
        let mut document = SettingsDocument::default();
        document.settings.weather.location_name = "Private home".to_string();
        document.settings.weather.latitude = 31.2304;
        document.settings.weather.longitude = 121.4737;
        document.settings.weather.timezone = "Private/Timezone".to_string();
        document.migration.source_path = Some("C:\\Users\\Private\\config.json".to_string());
        document.migration.warnings = vec!["Private migration detail".to_string()];
        let mut snapshot = AgentSnapshot::default();
        snapshot.device.serial_number = Some("SECRET-SERIAL".to_string());
        snapshot.device.port_name = Some("COM77".to_string());
        snapshot.relay.matched_rule_id = Some("private-rule-id".to_string());
        snapshot.relay.matched_rule_name = Some("Private bedtime".to_string());
        snapshot.environment.timezone = Some("Private/Timezone".to_string());
        fs::create_dir_all(&paths.crashes).unwrap();
        fs::write(
            paths.crashes.join("secret-crash.json"),
            br#"{"message":"C:\\Users\\Private panic","location":"C:\\Users\\Private\\source.rs:12"}"#,
        )
        .unwrap();

        let output = export_diagnostics(&paths, &snapshot, &document, &logger).unwrap();
        assert!(output.metadata().unwrap().len() < 2 * 1024 * 1024);
        let file = File::open(output).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut settings = String::new();
        archive
            .by_name("settings.sanitized.json")
            .unwrap()
            .read_to_string(&mut settings)
            .unwrap();
        assert!(!settings.contains("Private home"));
        assert!(!settings.contains("31.2304"));
        assert!(!settings.contains("Private\\\\config.json"));
        assert!(!settings.contains("Private/Timezone"));
        assert!(!settings.contains("Private migration detail"));
        let mut snapshot = String::new();
        archive
            .by_name("snapshot.sanitized.json")
            .unwrap()
            .read_to_string(&mut snapshot)
            .unwrap();
        assert!(!snapshot.contains("SECRET-SERIAL"));
        assert!(!snapshot.contains("COM77"));
        assert!(!snapshot.contains("private-rule-id"));
        assert!(!snapshot.contains("Private bedtime"));
        assert!(!snapshot.contains("Private/Timezone"));
        let mut crash = String::new();
        archive
            .by_name("crashes/secret-crash.json")
            .unwrap()
            .read_to_string(&mut crash)
            .unwrap();
        assert!(!crash.contains("Private"));
        assert!(crash.contains("details remain local"));
        let mut log = String::new();
        archive
            .by_name("logs/agent.log")
            .unwrap()
            .read_to_string(&mut log)
            .unwrap();
        assert!(!log.contains("Private"));
        assert!(log.contains("\"event\":\"test\""));
    }

    #[test]
    fn process_sampler_always_reports_the_current_process() {
        let resources = sample_process_resources();
        assert_eq!(resources.process_id, std::process::id());
        #[cfg(windows)]
        {
            assert!(resources.thread_count.is_some());
            assert!(resources.handle_count.is_some());
            assert!(resources.working_set_bytes.is_some());
            assert!(resources.cpu_time_ms.is_some());
        }
    }
}
