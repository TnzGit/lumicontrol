use lumi_core::{
    default_sensor_curve, normalize_brightness, normalize_sensor_curve, ConditionExpression,
    LightAction, LightCondition, LightRule, LogLuxFilterConfig, ManualOverrideConfig,
    RelayContactMode, SensorCurvePoint, TransitionSpec, WeatherKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
#[cfg(not(windows))]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SETTINGS_SCHEMA_VERSION: u32 = 2;
pub const STATE_SCHEMA_VERSION: u32 = 1;
const MAX_SENSOR_CURVE_POINTS: usize = 64;
const MAX_LIGHT_RULES: usize = 64;
const MAX_RULE_CONDITIONS: usize = 32;
const MAX_RULE_DEPTH: usize = 8;
const MAX_MONITOR_PROFILES: usize = 32;
const MAX_TEXT_CHARS: usize = 128;
const MAX_WEATHER_REFRESH_SECONDS: u64 = 24 * 60 * 60;
const MAX_MANUAL_OVERRIDE_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_SETTINGS_FILE_BYTES: u64 = 1024 * 1024;
const MAX_STATE_FILE_BYTES: u64 = 256 * 1024;
const MAX_LEGACY_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SettingsDocument {
    pub schema_version: u32,
    pub settings: Settings,
    pub migration: MigrationMetadata,
}

impl Default for SettingsDocument {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            settings: Settings::default(),
            migration: MigrationMetadata::default(),
        }
    }
}

impl SettingsDocument {
    pub fn normalize(&mut self) {
        self.settings.control.sensor_curve =
            normalize_sensor_curve(&self.settings.control.sensor_curve);
        self.settings.control.daytime_peak_brightness =
            normalize_brightness(self.settings.control.daytime_peak_brightness);
        self.settings.control.night_target_brightness =
            normalize_brightness(self.settings.control.night_target_brightness);
        self.settings.control.target_deadband = self.settings.control.target_deadband.clamp(0, 20);
        self.settings.control.manual_override.detection_threshold = self
            .settings
            .control
            .manual_override
            .detection_threshold
            .clamp(1, 100);
        self.settings.control.manual_override.grace_period_ms = self
            .settings
            .control
            .manual_override
            .grace_period_ms
            .clamp(60_000, MAX_MANUAL_OVERRIDE_MS);
        self.settings.weather.refresh_seconds = self
            .settings
            .weather
            .refresh_seconds
            .clamp(60, MAX_WEATHER_REFRESH_SECONDS);
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        let mut problems = Vec::new();
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            problems.push(format!(
                "unsupported settings schema {}; expected {}",
                self.schema_version, SETTINGS_SCHEMA_VERSION
            ));
        }
        if self.settings.control.filter.validate().is_err() {
            problems.push("invalid lux filter configuration".to_string());
        }
        if self.settings.control.transition.validate().is_err() {
            problems.push("invalid transition configuration".to_string());
        }
        if self.settings.control.sensor_curve.is_empty() {
            problems.push("sensor curve must contain at least one point".to_string());
        }
        if self.settings.control.sensor_curve.len() > MAX_SENSOR_CURVE_POINTS {
            problems.push(format!(
                "sensor curve must contain at most {MAX_SENSOR_CURVE_POINTS} points"
            ));
        }
        for point in &self.settings.control.sensor_curve {
            if !point.lux.is_finite() || point.lux <= 0.0 {
                problems.push("sensor curve lux values must be finite and positive".to_string());
                break;
            }
            if !(0..=100).contains(&point.brightness) {
                problems.push("sensor curve brightness values must be in 0..=100".to_string());
                break;
            }
        }
        if !(0..=20).contains(&self.settings.control.target_deadband) {
            problems.push("target_deadband must be in 0..=20".to_string());
        }
        if !(0..=100).contains(&self.settings.control.daytime_peak_brightness)
            || !(0..=100).contains(&self.settings.control.night_target_brightness)
        {
            problems.push("day and night brightness must be in 0..=100".to_string());
        }
        let manual_override = self.settings.control.manual_override;
        if !(1..=100).contains(&manual_override.detection_threshold) {
            problems.push("manual override detection_threshold must be in 1..=100".to_string());
        }
        if !(60_000..=MAX_MANUAL_OVERRIDE_MS).contains(&manual_override.grace_period_ms) {
            problems.push(
                "manual override grace period must be between 1 minute and 24 hours".to_string(),
            );
        }
        if self.settings.weather.enabled {
            if !self.settings.weather.latitude.is_finite()
                || !(-90.0..=90.0).contains(&self.settings.weather.latitude)
            {
                problems.push("weather latitude must be in -90..=90".to_string());
            }
            if !self.settings.weather.longitude.is_finite()
                || !(-180.0..=180.0).contains(&self.settings.weather.longitude)
            {
                problems.push("weather longitude must be in -180..=180".to_string());
            }
            if self.settings.weather.timezone.trim().is_empty() {
                problems.push("weather timezone must not be empty".to_string());
            }
        }
        if !(60..=MAX_WEATHER_REFRESH_SECONDS).contains(&self.settings.weather.refresh_seconds) {
            problems.push("weather refresh_seconds must be in 60..=86400".to_string());
        }
        if self.settings.weather.location_name.chars().count() > MAX_TEXT_CHARS
            || self.settings.weather.timezone.chars().count() > MAX_TEXT_CHARS
        {
            problems.push(format!(
                "weather location and timezone must be at most {MAX_TEXT_CHARS} characters"
            ));
        }
        if self.settings.locale.trim().is_empty() {
            problems.push("locale must not be empty".to_string());
        }
        if self.settings.locale.chars().count() > 32 {
            problems.push("locale must be at most 32 characters".to_string());
        }
        if self.settings.relay.rules.len() > MAX_LIGHT_RULES {
            problems.push(format!(
                "at most {MAX_LIGHT_RULES} light rules are supported"
            ));
        }
        let mut rule_ids = BTreeSet::new();
        for rule in &self.settings.relay.rules {
            if rule.id.trim().is_empty() {
                problems.push("rule IDs must not be empty".to_string());
            } else if !rule_ids.insert(rule.id.clone()) {
                problems.push(format!("duplicate rule ID: {}", rule.id));
            }
            if rule.name.trim().is_empty() {
                problems.push(format!("rule {} must have a name", rule.id));
            }
            if rule.id.chars().count() > MAX_TEXT_CHARS
                || rule.name.chars().count() > MAX_TEXT_CHARS
            {
                problems.push(format!(
                    "rule {} ID and name must be at most {MAX_TEXT_CHARS} characters",
                    rule.id
                ));
            }
            let (condition_count, maximum_depth) = validate_expression(&rule.when, &mut problems);
            if rule.enabled && condition_count == 0 {
                problems.push(format!("enabled rule {} must contain a condition", rule.id));
            }
            if condition_count > MAX_RULE_CONDITIONS {
                problems.push(format!(
                    "rule {} must contain at most {MAX_RULE_CONDITIONS} conditions",
                    rule.id
                ));
            }
            if maximum_depth > MAX_RULE_DEPTH {
                problems.push(format!(
                    "rule {} nesting must not exceed {MAX_RULE_DEPTH} levels",
                    rule.id
                ));
            }
        }
        if self.settings.monitors.len() > MAX_MONITOR_PROFILES {
            problems.push(format!(
                "at most {MAX_MONITOR_PROFILES} monitor profiles are supported"
            ));
        }
        for (monitor_id, monitor) in &self.settings.monitors {
            if monitor_id.trim().is_empty() {
                problems.push("monitor profile ID must not be empty".to_string());
            }
            if monitor.display_name.trim().is_empty() {
                problems.push(format!("monitor {monitor_id} must have a display name"));
            }
            if monitor_id.chars().count() > MAX_TEXT_CHARS
                || monitor.display_name.chars().count() > MAX_TEXT_CHARS
            {
                problems.push(format!(
                    "monitor {monitor_id} ID and name must be at most {MAX_TEXT_CHARS} characters"
                ));
            }
            if monitor.calibration.perceptual_points.len() > MAX_SENSOR_CURVE_POINTS {
                problems.push(format!(
                    "monitor {monitor_id} calibration must contain at most {MAX_SENSOR_CURVE_POINTS} points"
                ));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(StoreError::Validation(problems))
        }
    }
}

fn validate_expression(
    expression: &ConditionExpression,
    problems: &mut Vec<String>,
) -> (usize, usize) {
    let mut condition_count = 0usize;
    let mut maximum_depth = 0usize;
    let mut pending = vec![(expression, 1usize)];
    while let Some((expression, depth)) = pending.pop() {
        maximum_depth = maximum_depth.max(depth);
        match expression {
            ConditionExpression::Condition { condition } => {
                condition_count = condition_count.saturating_add(1);
                match condition {
                    LightCondition::TimeAfter { minutes }
                    | LightCondition::TimeBefore { minutes }
                        if !(0..24 * 60).contains(minutes) =>
                    {
                        problems.push("rule clock times must be in 0..1439 minutes".to_string());
                    }
                    LightCondition::AfterSunrise { offset_minutes }
                    | LightCondition::BeforeSunset { offset_minutes }
                    | LightCondition::AfterSunset { offset_minutes }
                        if !(-24 * 60..=24 * 60).contains(offset_minutes) =>
                    {
                        problems
                            .push("rule solar offsets must be in -1440..=1440 minutes".to_string());
                    }
                    LightCondition::LuxBelow { lux } | LightCondition::LuxAbove { lux }
                        if !lux.is_finite() || *lux < 0.0 =>
                    {
                        problems.push(
                            "rule lux thresholds must be finite and non-negative".to_string(),
                        );
                    }
                    LightCondition::CurrentBrightnessBelow { brightness }
                    | LightCondition::CurrentBrightnessAbove { brightness }
                    | LightCondition::TargetBrightnessBelow { brightness }
                    | LightCondition::TargetBrightnessAbove { brightness }
                        if !(0..=100).contains(brightness) =>
                    {
                        problems.push("rule brightness thresholds must be in 0..=100".to_string());
                    }
                    _ => {}
                }
            }
            ConditionExpression::And { conditions } | ConditionExpression::Or { conditions } => {
                pending.extend(conditions.iter().map(|condition| (condition, depth + 1)));
            }
        }
    }
    (condition_count, maximum_depth)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub paused: bool,
    pub start_at_login: bool,
    pub onboarding_completed: bool,
    pub locale: String,
    pub theme: ThemeMode,
    pub control: ControlSettings,
    pub relay: RelaySettings,
    pub weather: WeatherSettings,
    pub monitors: BTreeMap<String, MonitorProfile>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            paused: false,
            start_at_login: false,
            onboarding_completed: false,
            locale: "system".to_string(),
            theme: ThemeMode::System,
            control: ControlSettings::default(),
            relay: RelaySettings::default(),
            weather: WeatherSettings::default(),
            monitors: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ControlSettings {
    pub sensor_curve: Vec<SensorCurvePoint>,
    pub filter: LogLuxFilterConfig,
    pub target_deadband: i32,
    pub transition: TransitionSpec,
    pub manual_override: ManualOverrideConfig,
    pub daytime_peak_brightness: i32,
    pub night_target_brightness: i32,
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            sensor_curve: default_sensor_curve(),
            filter: LogLuxFilterConfig::default(),
            target_deadband: 2,
            transition: TransitionSpec::default(),
            manual_override: ManualOverrideConfig::default(),
            daytime_peak_brightness: 90,
            night_target_brightness: 18,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RelaySettings {
    pub contact_mode: RelayContactMode,
    pub rules_enabled: bool,
    pub rules: Vec<LightRule>,
    pub fallback_action: LightAction,
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self {
            contact_mode: RelayContactMode::No,
            rules_enabled: false,
            rules: Vec::new(),
            fallback_action: LightAction::Keep,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WeatherSettings {
    pub enabled: bool,
    pub location_name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub timezone: String,
    pub refresh_seconds: u64,
}

impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            location_name: String::new(),
            latitude: 0.0,
            longitude: 0.0,
            timezone: "system".to_string(),
            refresh_seconds: 300,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MonitorProfile {
    pub display_name: String,
    pub enabled: bool,
    pub calibration: MonitorCalibration,
}

impl Default for MonitorProfile {
    fn default() -> Self {
        Self {
            display_name: "Unknown monitor".to_string(),
            enabled: true,
            calibration: MonitorCalibration::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MonitorCalibration {
    pub minimum_raw: Option<u32>,
    pub maximum_raw: Option<u32>,
    pub perceptual_points: Vec<SensorCurvePoint>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MigrationMetadata {
    pub imported_from_v1: bool,
    pub source_path: Option<String>,
    pub imported_at_unix_seconds: Option<u64>,
    pub legacy_device_port: Option<String>,
    pub legacy_monitor_calibrations: Option<Value>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StateDocument {
    pub schema_version: u32,
    pub last_device_serial: Option<String>,
    pub last_monitor_ids: Vec<String>,
    pub last_clean_shutdown: bool,
}

impl Default for StateDocument {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            last_device_serial: None,
            last_monitor_ids: Vec::new(),
            last_clean_shutdown: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductPaths {
    pub root: PathBuf,
    pub settings: PathBuf,
    pub state: PathBuf,
    pub logs: PathBuf,
    pub backups: PathBuf,
    pub diagnostics: PathBuf,
    pub crashes: PathBuf,
}

impl ProductPaths {
    pub fn from_environment() -> Result<Self, StoreError> {
        let local_app_data =
            std::env::var_os("LOCALAPPDATA").ok_or(StoreError::MissingLocalAppData)?;
        Ok(Self::under(
            PathBuf::from(local_app_data).join("LumiControl"),
        ))
    }

    pub fn under(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            settings: root.join("settings.json"),
            state: root.join("state.json"),
            logs: root.join("logs"),
            backups: root.join("backups"),
            diagnostics: root.join("diagnostics"),
            crashes: root.join("crashes"),
            root,
        }
    }

    pub fn ensure_directories(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.root)?;
        fs::create_dir_all(&self.logs)?;
        fs::create_dir_all(&self.backups)?;
        fs::create_dir_all(&self.diagnostics)?;
        fs::create_dir_all(&self.crashes)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct SettingsStore {
    pub paths: ProductPaths,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsLoadOutcome {
    pub document: SettingsDocument,
    pub recovered_from: Option<PathBuf>,
    pub warning: Option<String>,
}

impl SettingsStore {
    pub fn new(paths: ProductPaths) -> Self {
        Self { paths }
    }

    pub fn load_settings(&self) -> Result<SettingsDocument, StoreError> {
        if !self.paths.settings.exists() {
            return Ok(SettingsDocument::default());
        }
        let text = match read_bounded_text(&self.paths.settings, MAX_SETTINGS_FILE_BYTES) {
            Ok(text) => text,
            Err(error @ StoreError::FileTooLarge { .. }) => {
                let quarantine = self.quarantine_oversized(&self.paths.settings).ok();
                return Err(StoreError::InvalidSettings {
                    path: self.paths.settings.clone(),
                    problems: vec![error.to_string()],
                    quarantine,
                });
            }
            Err(error) => return Err(error),
        };
        let document = serde_json::from_str::<SettingsDocument>(
            text.trim_start_matches('\u{feff}'),
        )
        .map_err(|error| {
            let quarantine = self.quarantine_invalid(&self.paths.settings).ok();
            StoreError::InvalidJson {
                path: self.paths.settings.clone(),
                source: error,
                quarantine,
            }
        })?;
        if let Err(StoreError::Validation(problems)) = document.validate() {
            let quarantine = self.quarantine_invalid(&self.paths.settings).ok();
            return Err(StoreError::InvalidSettings {
                path: self.paths.settings.clone(),
                problems,
                quarantine,
            });
        }
        Ok(document)
    }

    pub fn load_settings_with_recovery(&self) -> Result<SettingsLoadOutcome, StoreError> {
        match self.load_settings() {
            Ok(document) => Ok(SettingsLoadOutcome {
                document,
                recovered_from: None,
                warning: None,
            }),
            Err(_primary @ StoreError::InvalidJson { .. })
            | Err(_primary @ StoreError::InvalidSettings { .. }) => {
                let backup = self.paths.backups.join("settings-previous.json");
                match self.read_valid_document(&backup) {
                    Ok(document) => {
                        let mut warning = format!(
                            "Current settings are invalid; recovered {}",
                            backup.display()
                        );
                        if let Err(error) = atomic_write_json(&self.paths.settings, &document) {
                            warning.push_str(&format!(
                                "; the primary settings file could not be repaired: {error}"
                            ));
                        }
                        Ok(SettingsLoadOutcome {
                            warning: Some(warning),
                            document,
                            recovered_from: Some(backup),
                        })
                    }
                    Err(_) => {
                        let document = SettingsDocument::default();
                        let mut warning = "Current settings are invalid and no valid backup exists; defaults were restored".to_string();
                        if let Err(error) = atomic_write_json(&self.paths.settings, &document) {
                            warning.push_str(&format!(
                                "; the primary settings file could not be repaired: {error}"
                            ));
                        }
                        Ok(SettingsLoadOutcome {
                            document,
                            recovered_from: None,
                            warning: Some(warning),
                        })
                    }
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn save_settings(&self, document: &SettingsDocument) -> Result<(), StoreError> {
        document.validate()?;
        self.paths.ensure_directories()?;
        if self.paths.settings.exists() {
            fs::copy(
                &self.paths.settings,
                self.paths.backups.join("settings-previous.json"),
            )?;
        }
        atomic_write_json(&self.paths.settings, document)
    }

    pub fn load_state(&self) -> Result<StateDocument, StoreError> {
        if !self.paths.state.exists() {
            return Ok(StateDocument::default());
        }
        let text = read_bounded_text(&self.paths.state, MAX_STATE_FILE_BYTES)?;
        let state: StateDocument = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
        if state.schema_version != STATE_SCHEMA_VERSION {
            return Err(StoreError::Validation(vec![format!(
                "unsupported state schema {}; expected {}",
                state.schema_version, STATE_SCHEMA_VERSION
            )]));
        }
        Ok(state)
    }

    pub fn save_state(&self, state: &StateDocument) -> Result<(), StoreError> {
        if state.schema_version != STATE_SCHEMA_VERSION {
            return Err(StoreError::Validation(vec![format!(
                "unsupported state schema {}; expected {}",
                state.schema_version, STATE_SCHEMA_VERSION
            )]));
        }
        self.paths.ensure_directories()?;
        atomic_write_json(&self.paths.state, state)
    }

    pub fn load_or_import_v1(&self, legacy_path: &Path) -> Result<SettingsDocument, StoreError> {
        Ok(self.load_or_import_v1_with_recovery(legacy_path)?.document)
    }

    pub fn load_or_import_v1_with_recovery(
        &self,
        legacy_path: &Path,
    ) -> Result<SettingsLoadOutcome, StoreError> {
        if self.paths.settings.exists() {
            return self.load_settings_with_recovery();
        }
        if !legacy_path.exists() {
            return Ok(SettingsLoadOutcome {
                document: SettingsDocument::default(),
                recovered_from: None,
                warning: None,
            });
        }
        let text = read_bounded_text(legacy_path, MAX_LEGACY_FILE_BYTES)?;
        let mut document = import_v1(&text, legacy_path)?;
        document.normalize();
        document.validate()?;
        self.save_settings(&document)?;
        Ok(SettingsLoadOutcome {
            document,
            recovered_from: None,
            warning: None,
        })
    }

    fn quarantine_invalid(&self, path: &Path) -> Result<PathBuf, StoreError> {
        self.paths.ensure_directories()?;
        let timestamp = unix_seconds();
        let quarantine = self
            .paths
            .backups
            .join(format!("settings-invalid-{timestamp}.json"));
        fs::copy(path, &quarantine)?;
        Ok(quarantine)
    }

    fn quarantine_oversized(&self, path: &Path) -> Result<PathBuf, StoreError> {
        self.paths.ensure_directories()?;
        let quarantine = self.paths.backups.join(format!(
            "settings-oversized-{}-{}.json",
            unix_seconds(),
            std::process::id()
        ));
        fs::rename(path, &quarantine)?;
        Ok(quarantine)
    }

    fn read_valid_document(&self, path: &Path) -> Result<SettingsDocument, StoreError> {
        let text = read_bounded_text(path, MAX_SETTINGS_FILE_BYTES)?;
        let document: SettingsDocument = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
        document.validate()?;
        Ok(document)
    }
}

fn read_bounded_text(path: &Path, maximum: u64) -> Result<String, StoreError> {
    let size = fs::metadata(path)?.len();
    if size > maximum {
        return Err(StoreError::FileTooLarge {
            path: path.to_path_buf(),
            size,
            maximum,
        });
    }
    Ok(fs::read_to_string(path)?)
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::InvalidPath(path.to_path_buf()))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| StoreError::InvalidPath(path.to_path_buf()))?;
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        atomic_replace(&temporary, path)?;
        sync_directory(parent)?;
        Ok::<(), StoreError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), StoreError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(StoreError::Io(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), StoreError> {
    fs::rename(source, destination)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), StoreError> {
    #[cfg(not(windows))]
    {
        File::open(path)?.sync_all()?;
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn import_v1(text: &str, source_path: &Path) -> Result<SettingsDocument, StoreError> {
    let trimmed = text.trim_start_matches('\u{feff}');
    let raw: Value = serde_json::from_str(trimmed)?;
    let legacy: LegacyConfig = serde_json::from_value(raw.clone())?;
    let mut warnings = Vec::new();
    let rules = legacy
        .light_rules
        .into_iter()
        .enumerate()
        .map(|(index, rule)| convert_legacy_rule(index, rule, &mut warnings))
        .collect();
    let theme = match legacy.theme_mode.as_str() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::System,
    };
    let monitor_calibrations = raw.get("monitor_calibrations").cloned();
    Ok(SettingsDocument {
        schema_version: SETTINGS_SCHEMA_VERSION,
        settings: Settings {
            paused: legacy.paused,
            onboarding_completed: true,
            theme,
            control: ControlSettings {
                sensor_curve: legacy.sensor_calibration_curve,
                target_deadband: legacy.brightness_deadband,
                daytime_peak_brightness: legacy.daytime_peak_brightness,
                night_target_brightness: legacy.night_target_brightness,
                ..ControlSettings::default()
            },
            relay: RelaySettings {
                contact_mode: legacy.relay_contact_mode,
                rules_enabled: legacy.light_rules_enabled,
                rules,
                fallback_action: legacy.light_rules_fallback_action,
            },
            weather: WeatherSettings {
                enabled: false,
                location_name: legacy.location_name,
                latitude: legacy.latitude,
                longitude: legacy.longitude,
                timezone: legacy.timezone_name,
                refresh_seconds: legacy.weather_refresh_seconds,
            },
            ..Settings::default()
        },
        migration: MigrationMetadata {
            imported_from_v1: true,
            source_path: Some(source_path.to_string_lossy().into_owned()),
            imported_at_unix_seconds: Some(unix_seconds()),
            legacy_device_port: Some(legacy.sensor_port),
            legacy_monitor_calibrations: monitor_calibrations,
            warnings,
        },
    })
}

fn convert_legacy_rule(
    index: usize,
    rule: LegacyLightRule,
    warnings: &mut Vec<String>,
) -> LightRule {
    let mut groups = Vec::new();
    if !rule.all.is_empty() {
        groups.push(ConditionExpression::And {
            conditions: rule
                .all
                .into_iter()
                .map(|condition| ConditionExpression::condition(condition.into()))
                .collect(),
        });
    }
    if !rule.any.is_empty() {
        groups.push(ConditionExpression::Or {
            conditions: rule
                .any
                .into_iter()
                .map(|condition| ConditionExpression::condition(condition.into()))
                .collect(),
        });
    }
    let when = match groups.len() {
        0 => {
            warnings.push(format!(
                "legacy rule '{}' had no conditions and was disabled",
                rule.name
            ));
            ConditionExpression::Or {
                conditions: Vec::new(),
            }
        }
        1 => groups.remove(0),
        _ => ConditionExpression::And { conditions: groups },
    };
    let enabled = rule.enabled && when.condition_count() > 0;
    LightRule {
        id: format!("legacy-rule-{}", index + 1),
        name: rule.name,
        enabled,
        when,
        then: rule.then_action,
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct LegacyConfig {
    location_name: String,
    latitude: f64,
    longitude: f64,
    timezone_name: String,
    weather_refresh_seconds: u64,
    brightness_deadband: i32,
    sensor_port: String,
    daytime_peak_brightness: i32,
    night_target_brightness: i32,
    theme_mode: String,
    paused: bool,
    relay_contact_mode: RelayContactMode,
    light_rules_enabled: bool,
    light_rules: Vec<LegacyLightRule>,
    light_rules_fallback_action: LightAction,
    sensor_calibration_curve: Vec<SensorCurvePoint>,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        Self {
            location_name: "Shanghai".to_string(),
            latitude: 31.2304,
            longitude: 121.4737,
            timezone_name: "Asia/Shanghai".to_string(),
            weather_refresh_seconds: 300,
            brightness_deadband: 2,
            sensor_port: "COM3".to_string(),
            daytime_peak_brightness: 90,
            night_target_brightness: 18,
            theme_mode: "dark".to_string(),
            paused: false,
            relay_contact_mode: RelayContactMode::No,
            light_rules_enabled: false,
            light_rules: Vec::new(),
            light_rules_fallback_action: LightAction::Keep,
            sensor_calibration_curve: default_sensor_curve(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct LegacyLightRule {
    name: String,
    enabled: bool,
    all: Vec<LegacyCondition>,
    any: Vec<LegacyCondition>,
    then_action: LightAction,
}

impl Default for LegacyLightRule {
    fn default() -> Self {
        Self {
            name: "Imported rule".to_string(),
            enabled: true,
            all: Vec::new(),
            any: Vec::new(),
            then_action: LightAction::Keep,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyCondition {
    TimeAfter { minutes: i32 },
    TimeBefore { minutes: i32 },
    AfterSunrise { offset_minutes: i32 },
    BeforeSunset { offset_minutes: i32 },
    AfterSunset { offset_minutes: i32 },
    LuxBelow { lux: f64 },
    LuxAbove { lux: f64 },
    CurrentBrightnessBelow { brightness: i32 },
    CurrentBrightnessAbove { brightness: i32 },
    TargetBrightnessBelow { brightness: i32 },
    TargetBrightnessAbove { brightness: i32 },
    WeatherIs { kind: WeatherKind },
}

impl From<LegacyCondition> for LightCondition {
    fn from(condition: LegacyCondition) -> Self {
        match condition {
            LegacyCondition::TimeAfter { minutes } => Self::TimeAfter { minutes },
            LegacyCondition::TimeBefore { minutes } => Self::TimeBefore { minutes },
            LegacyCondition::AfterSunrise { offset_minutes } => {
                Self::AfterSunrise { offset_minutes }
            }
            LegacyCondition::BeforeSunset { offset_minutes } => {
                Self::BeforeSunset { offset_minutes }
            }
            LegacyCondition::AfterSunset { offset_minutes } => Self::AfterSunset { offset_minutes },
            LegacyCondition::LuxBelow { lux } => Self::LuxBelow { lux },
            LegacyCondition::LuxAbove { lux } => Self::LuxAbove { lux },
            LegacyCondition::CurrentBrightnessBelow { brightness } => {
                Self::CurrentBrightnessBelow { brightness }
            }
            LegacyCondition::CurrentBrightnessAbove { brightness } => {
                Self::CurrentBrightnessAbove { brightness }
            }
            LegacyCondition::TargetBrightnessBelow { brightness } => {
                Self::TargetBrightnessBelow { brightness }
            }
            LegacyCondition::TargetBrightnessAbove { brightness } => {
                Self::TargetBrightnessAbove { brightness }
            }
            LegacyCondition::WeatherIs { kind } => Self::WeatherIs { weather: kind },
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    MissingLocalAppData,
    InvalidPath(PathBuf),
    Validation(Vec<String>),
    InvalidSettings {
        path: PathBuf,
        problems: Vec<String>,
        quarantine: Option<PathBuf>,
    },
    InvalidJson {
        path: PathBuf,
        source: serde_json::Error,
        quarantine: Option<PathBuf>,
    },
    FileTooLarge {
        path: PathBuf,
        size: u64,
        maximum: u64,
    },
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::MissingLocalAppData => write!(formatter, "LOCALAPPDATA is not available"),
            StoreError::InvalidPath(path) => {
                write!(formatter, "invalid storage path: {}", path.display())
            }
            StoreError::Validation(problems) => {
                write!(
                    formatter,
                    "settings validation failed: {}",
                    problems.join("; ")
                )
            }
            StoreError::InvalidSettings {
                path,
                problems,
                quarantine,
            } => {
                write!(
                    formatter,
                    "invalid settings in {}: {}",
                    path.display(),
                    problems.join("; ")
                )?;
                if let Some(quarantine) = quarantine {
                    write!(formatter, "; copied to {}", quarantine.display())?;
                }
                Ok(())
            }
            StoreError::InvalidJson {
                path,
                source,
                quarantine,
            } => {
                write!(formatter, "invalid JSON in {}: {source}", path.display())?;
                if let Some(quarantine) = quarantine {
                    write!(formatter, "; copied to {}", quarantine.display())?;
                }
                Ok(())
            }
            StoreError::FileTooLarge {
                path,
                size,
                maximum,
            } => write!(
                formatter,
                "storage file {} is too large ({size} bytes; maximum {maximum})",
                path.display()
            ),
            StoreError::Io(error) => write!(formatter, "storage I/O failed: {error}"),
            StoreError::Json(error) => write!(formatter, "storage JSON failed: {error}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::InvalidJson { source, .. } => Some(source),
            StoreError::Io(error) => Some(error),
            StoreError::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        StoreError::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        StoreError::Json(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lumi-store-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn settings_round_trip_through_atomic_file() {
        let root = temporary_root("round-trip");
        let store = SettingsStore::new(ProductPaths::under(&root));
        let mut settings = SettingsDocument::default();
        settings.settings.paused = true;
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings().unwrap(), settings);
        assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_settings_are_reported_and_quarantined_without_deletion() {
        let root = temporary_root("corrupt");
        let store = SettingsStore::new(ProductPaths::under(&root));
        store.paths.ensure_directories().unwrap();
        fs::write(&store.paths.settings, "{not-json").unwrap();
        let error = store.load_settings().unwrap_err();
        let quarantine = match error {
            StoreError::InvalidJson {
                quarantine: Some(path),
                ..
            } => path,
            other => panic!("unexpected error: {other}"),
        };
        assert!(store.paths.settings.exists());
        assert!(quarantine.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v1_import_preserves_curve_rules_relay_and_source_file() {
        let root = temporary_root("import");
        let legacy_path = root.join("config.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &legacy_path,
            r#"{
                "sensor_port":"COM7",
                "paused":true,
                "theme_mode":"light",
                "relay_contact_mode":"nc",
                "light_rules_enabled":true,
                "light_rules_fallback_action":"off",
                "sensor_calibration_curve":[{"lux":10.0,"brightness":20},{"lux":100.0,"brightness":70}],
                "light_rules":[{
                    "name":"Dark evening",
                    "enabled":true,
                    "all":[{"after_sunset":{"offset_minutes":0}}],
                    "any":[{"lux_below":{"lux":30.0}}],
                    "then_action":"on"
                }],
                "monitor_calibrations":{"monitor-1":{"manual_0":{"average_raw":0}}}
            }"#,
        )
        .unwrap();
        let store = SettingsStore::new(ProductPaths::under(root.join("v2")));
        let imported = store.load_or_import_v1(&legacy_path).unwrap();
        assert!(imported.settings.paused);
        assert_eq!(imported.settings.theme, ThemeMode::Light);
        assert_eq!(imported.settings.relay.contact_mode, RelayContactMode::Nc);
        assert_eq!(imported.settings.relay.rules.len(), 1);
        assert_eq!(imported.settings.relay.rules[0].when.condition_count(), 2);
        assert_eq!(
            imported.migration.legacy_device_port.as_deref(),
            Some("COM7")
        );
        assert!(imported.migration.legacy_monitor_calibrations.is_some());
        assert!(legacy_path.exists());
        assert!(store.paths.settings.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_enabled_rule_is_rejected_before_save() {
        let root = temporary_root("validation");
        let store = SettingsStore::new(ProductPaths::under(&root));
        let mut settings = SettingsDocument::default();
        settings.settings.relay.rules.push(LightRule {
            id: "empty".to_string(),
            name: "Empty".to_string(),
            enabled: true,
            when: ConditionExpression::Or {
                conditions: Vec::new(),
            },
            then: LightAction::On,
        });
        assert!(matches!(
            store.save_settings(&settings),
            Err(StoreError::Validation(_))
        ));
        assert!(!store.paths.settings.exists());
    }

    #[test]
    fn semantic_corruption_is_quarantined_and_previous_settings_are_recovered() {
        let root = temporary_root("semantic-recovery");
        let store = SettingsStore::new(ProductPaths::under(&root));
        let first = SettingsDocument::default();
        store.save_settings(&first).unwrap();
        let mut second = first.clone();
        second.settings.paused = true;
        store.save_settings(&second).unwrap();

        let mut invalid = second;
        invalid.settings.control.sensor_curve.clear();
        fs::write(
            &store.paths.settings,
            serde_json::to_vec_pretty(&invalid).unwrap(),
        )
        .unwrap();

        let outcome = store.load_settings_with_recovery().unwrap();
        assert_eq!(outcome.document, first);
        assert!(outcome.recovered_from.is_some());
        assert!(outcome.warning.is_some());
        assert_eq!(store.load_settings().unwrap(), first);
        assert!(fs::read_dir(&store.paths.backups)
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("settings-invalid-")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_settings_without_a_backup_restore_defaults_with_a_warning() {
        let root = temporary_root("default-recovery");
        let store = SettingsStore::new(ProductPaths::under(&root));
        store.paths.ensure_directories().unwrap();
        fs::write(&store.paths.settings, b"{not-json").unwrap();

        let outcome = store.load_settings_with_recovery().unwrap();
        assert_eq!(outcome.document, SettingsDocument::default());
        assert!(outcome.warning.is_some());
        assert_eq!(store.load_settings().unwrap(), SettingsDocument::default());
        assert!(fs::read_dir(&store.paths.backups)
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("settings-invalid-")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_settings_are_moved_without_copying_and_recovered() {
        let root = temporary_root("oversized-recovery");
        let store = SettingsStore::new(ProductPaths::under(&root));
        let first = SettingsDocument::default();
        store.save_settings(&first).unwrap();
        let mut second = first.clone();
        second.settings.paused = true;
        store.save_settings(&second).unwrap();
        fs::write(
            &store.paths.settings,
            vec![b'x'; (MAX_SETTINGS_FILE_BYTES + 1) as usize],
        )
        .unwrap();

        let outcome = store.load_settings_with_recovery().unwrap();
        assert_eq!(outcome.document, first);
        assert_eq!(store.load_settings().unwrap(), first);
        assert!(fs::read_dir(&store.paths.backups)
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("settings-oversized-")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn normalization_caps_settings_that_drive_runtime_work() {
        let mut document = SettingsDocument::default();
        document.settings.weather.refresh_seconds = u64::MAX;
        document.settings.control.manual_override.grace_period_ms = u64::MAX;
        document
            .settings
            .control
            .manual_override
            .detection_threshold = -1;
        document.normalize();
        assert_eq!(
            document.settings.weather.refresh_seconds,
            MAX_WEATHER_REFRESH_SECONDS
        );
        assert_eq!(
            document.settings.control.manual_override.grace_period_ms,
            MAX_MANUAL_OVERRIDE_MS
        );
        assert_eq!(
            document
                .settings
                .control
                .manual_override
                .detection_threshold,
            1
        );
        document.validate().unwrap();
    }
}
