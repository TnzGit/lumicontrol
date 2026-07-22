#[cfg(windows)]
mod native_gui;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
#[cfg(not(windows))]
use std::io;
use std::io::{BufRead, BufReader, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    location_name: String,
    latitude: f64,
    longitude: f64,
    timezone_name: String,
    brightness_min: i32,
    brightness_max: i32,
    weather_refresh_seconds: u64,
    control_tick_seconds: u64,
    brightness_deadband: i32,
    maximum_step_per_tick: i32,
    sensor_port: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            location_name: "Shanghai".to_string(),
            latitude: 31.2304,
            longitude: 121.4737,
            timezone_name: "Asia/Shanghai".to_string(),
            brightness_min: 0,
            brightness_max: 100,
            weather_refresh_seconds: 300,
            control_tick_seconds: 30,
            brightness_deadband: 2,
            maximum_step_per_tick: 4,
            sensor_port: default_sensor_port(),
        }
    }
}

fn default_sensor_port() -> String {
    "COM3".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
struct RuntimeConfig {
    daytime_peak_brightness: i32,
    night_target_brightness: i32,
    theme_mode: String,
    paused: bool,
    relay_contact_mode: RelayContactMode,
    light_rules_enabled: bool,
    light_rules: Vec<LightRule>,
    light_rules_fallback_action: LightRuleAction,
    sensor_calibration_curve: Vec<SensorCurvePoint>,
    monitor_calibrations: BTreeMap<String, BTreeMap<String, CalibrationPoint>>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            daytime_peak_brightness: 90,
            night_target_brightness: 18,
            theme_mode: "dark".to_string(),
            paused: false,
            relay_contact_mode: RelayContactMode::default(),
            light_rules_enabled: false,
            light_rules: Vec::new(),
            light_rules_fallback_action: LightRuleAction::Keep,
            sensor_calibration_curve: default_sensor_calibration_curve(),
            monitor_calibrations: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct SensorCurvePoint {
    lux: f64,
    brightness: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CalibrationPoint {
    label: String,
    samples: Vec<i32>,
    average_raw: i32,
    min_raw: i32,
    max_raw: i32,
    captured_at: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RelayContactMode {
    #[default]
    No,
    Nc,
}

impl RelayContactMode {
    fn label(self) -> &'static str {
        match self {
            RelayContactMode::No => "NO",
            RelayContactMode::Nc => "NC",
        }
    }

    fn relay_state_for_light(self, light_on: bool) -> RelayState {
        match (self, light_on) {
            (RelayContactMode::No, true) | (RelayContactMode::Nc, false) => RelayState::On,
            (RelayContactMode::No, false) | (RelayContactMode::Nc, true) => RelayState::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RelayState {
    On,
    Off,
}

impl RelayState {
    fn label(self) -> &'static str {
        match self {
            RelayState::On => "On",
            RelayState::Off => "Off",
        }
    }

    fn light_on(self, contact_mode: RelayContactMode) -> bool {
        match contact_mode {
            RelayContactMode::No => self == RelayState::On,
            RelayContactMode::Nc => self == RelayState::Off,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct RelayCommandResponse {
    command: String,
    action: String,
    relay: RelayState,
    relay_gpio: i32,
    #[allow(dead_code)]
    relay_active_low: bool,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum LightRuleAction {
    #[default]
    Keep,
    On,
    Off,
}

impl LightRuleAction {
    fn label(self) -> &'static str {
        match self {
            LightRuleAction::Keep => "Keep",
            LightRuleAction::On => "On",
            LightRuleAction::Off => "Off",
        }
    }

    fn light_on(self) -> Option<bool> {
        match self {
            LightRuleAction::Keep => None,
            LightRuleAction::On => Some(true),
            LightRuleAction::Off => Some(false),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum WeatherKind {
    #[default]
    Clear,
    Cloudy,
    Rain,
    Fog,
}

impl WeatherKind {
    fn label(self) -> &'static str {
        match self {
            WeatherKind::Clear => "Clear",
            WeatherKind::Cloudy => "Cloudy",
            WeatherKind::Rain => "Rain",
            WeatherKind::Fog => "Fog",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LightRuleCondition {
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
struct LightRule {
    name: String,
    enabled: bool,
    all: Vec<LightRuleCondition>,
    any: Vec<LightRuleCondition>,
    then_action: LightRuleAction,
}

impl Default for LightRule {
    fn default() -> Self {
        Self {
            name: "New rule".to_string(),
            enabled: true,
            all: Vec::new(),
            any: Vec::new(),
            then_action: LightRuleAction::On,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LightRuleContext {
    now: LocalDateTime,
    sunrise_minutes: Option<i32>,
    sunset_minutes: Option<i32>,
    weather_kind: Option<WeatherKind>,
    lux: Option<f64>,
    current_brightness: Option<i32>,
    target_brightness: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LightRuleDecision {
    action: LightRuleAction,
    matched_rule: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BrightnessPolicy {
    brightness_min: i32,
    brightness_max: i32,
    deadband: i32,
    maximum_step: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BrightnessInputs {
    daylight_factor: f64,
    cloud_cover: i32,
    visibility_km: f64,
    precipitation_probability: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WeatherSnapshot {
    cloud_cover: i32,
    visibility_km: f64,
    precipitation_probability: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SolarSnapshot {
    elevation_degrees: f64,
    daylight_factor: f64,
    is_daylight: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MonitorInfo {
    identifier: String,
    description: String,
}

#[derive(Clone, Debug)]
struct MonitorRecord {
    identifier: String,
    description: String,
    current: i32,
}

#[derive(Clone, Debug)]
struct WeatherCache {
    snapshot: Option<WeatherSnapshot>,
    fetched_at: Option<Instant>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct SensorReading {
    sensor: String,
    lux: f64,
    addr: String,
    sda: i32,
    scl: i32,
    relay: Option<RelayState>,
    relay_gpio: Option<i32>,
}

fn monitor_brightness_range() -> RangeInclusive<i32> {
    0..=100
}

fn normalize_monitor_brightness(value: i32) -> i32 {
    value.clamp(
        *monitor_brightness_range().start(),
        *monitor_brightness_range().end(),
    )
}

fn normalize_monitor_brightness_bounds(min: i32, max: i32) -> (i32, i32) {
    let min = normalize_monitor_brightness(min);
    let max = normalize_monitor_brightness(max);
    if min <= max {
        (min, max)
    } else {
        (max, min)
    }
}

fn normalize_runtime_config(runtime: &mut RuntimeConfig) {
    runtime.daytime_peak_brightness = normalize_monitor_brightness(runtime.daytime_peak_brightness);
    runtime.night_target_brightness = normalize_monitor_brightness(runtime.night_target_brightness);
    runtime.sensor_calibration_curve = normalize_sensor_curve(&runtime.sensor_calibration_curve);
    if !matches!(runtime.theme_mode.as_str(), "dark" | "light" | "system") {
        runtime.theme_mode = "dark".to_string();
    }
}

fn local_time_minutes(now: LocalDateTime) -> i32 {
    (now.hour as i32 * 60 + now.minute as i32).clamp(0, 1439)
}

fn normalize_day_minutes(minutes: i32) -> i32 {
    minutes.rem_euclid(1440)
}

fn time_is_after(now_minutes: i32, threshold_minutes: i32) -> bool {
    now_minutes >= normalize_day_minutes(threshold_minutes)
}

fn time_is_before(now_minutes: i32, threshold_minutes: i32) -> bool {
    now_minutes < normalize_day_minutes(threshold_minutes)
}

fn classify_weather(snapshot: WeatherSnapshot) -> WeatherKind {
    if snapshot.precipitation_probability >= 0.35 {
        WeatherKind::Rain
    } else if snapshot.visibility_km <= 3.0 {
        WeatherKind::Fog
    } else if snapshot.cloud_cover >= 60 {
        WeatherKind::Cloudy
    } else {
        WeatherKind::Clear
    }
}

fn light_rule_condition_matches(
    condition: &LightRuleCondition,
    context: &LightRuleContext,
) -> bool {
    let now_minutes = local_time_minutes(context.now);
    match condition {
        LightRuleCondition::TimeAfter { minutes } => time_is_after(now_minutes, *minutes),
        LightRuleCondition::TimeBefore { minutes } => time_is_before(now_minutes, *minutes),
        LightRuleCondition::AfterSunrise { offset_minutes } => context
            .sunrise_minutes
            .map(|minutes| time_is_after(now_minutes, minutes + offset_minutes))
            .unwrap_or(false),
        LightRuleCondition::BeforeSunset { offset_minutes } => context
            .sunset_minutes
            .map(|minutes| time_is_before(now_minutes, minutes + offset_minutes))
            .unwrap_or(false),
        LightRuleCondition::AfterSunset { offset_minutes } => context
            .sunset_minutes
            .map(|minutes| time_is_after(now_minutes, minutes + offset_minutes))
            .unwrap_or(false),
        LightRuleCondition::LuxBelow { lux } => {
            context.lux.map(|value| value < *lux).unwrap_or(false)
        }
        LightRuleCondition::LuxAbove { lux } => {
            context.lux.map(|value| value > *lux).unwrap_or(false)
        }
        LightRuleCondition::CurrentBrightnessBelow { brightness } => context
            .current_brightness
            .map(|value| value < *brightness)
            .unwrap_or(false),
        LightRuleCondition::CurrentBrightnessAbove { brightness } => context
            .current_brightness
            .map(|value| value > *brightness)
            .unwrap_or(false),
        LightRuleCondition::TargetBrightnessBelow { brightness } => context
            .target_brightness
            .map(|value| value < *brightness)
            .unwrap_or(false),
        LightRuleCondition::TargetBrightnessAbove { brightness } => context
            .target_brightness
            .map(|value| value > *brightness)
            .unwrap_or(false),
        LightRuleCondition::WeatherIs { kind } => context.weather_kind == Some(*kind),
    }
}

fn light_rule_condition_needs_weather(condition: &LightRuleCondition) -> bool {
    matches!(condition, LightRuleCondition::WeatherIs { .. })
}

fn light_rules_need_weather(rules: &[LightRule]) -> bool {
    rules.iter().filter(|rule| rule.enabled).any(|rule| {
        rule.all.iter().any(light_rule_condition_needs_weather)
            || rule.any.iter().any(light_rule_condition_needs_weather)
    })
}

fn light_rule_matches(rule: &LightRule, context: &LightRuleContext) -> bool {
    if !rule.enabled || (rule.all.is_empty() && rule.any.is_empty()) {
        return false;
    }
    let all_match = rule
        .all
        .iter()
        .all(|condition| light_rule_condition_matches(condition, context));
    let any_match = rule.any.is_empty()
        || rule
            .any
            .iter()
            .any(|condition| light_rule_condition_matches(condition, context));
    all_match && any_match
}

fn evaluate_light_rules(
    rules: &[LightRule],
    fallback_action: LightRuleAction,
    context: &LightRuleContext,
) -> LightRuleDecision {
    for rule in rules {
        if light_rule_matches(rule, context) {
            return LightRuleDecision {
                action: rule.then_action,
                matched_rule: Some(rule.name.clone()),
            };
        }
    }
    LightRuleDecision {
        action: fallback_action,
        matched_rule: None,
    }
}

fn default_sensor_calibration_curve() -> Vec<SensorCurvePoint> {
    vec![
        SensorCurvePoint {
            lux: 20.0,
            brightness: 40,
        },
        SensorCurvePoint {
            lux: 80.0,
            brightness: 72,
        },
        SensorCurvePoint {
            lux: 250.0,
            brightness: 88,
        },
    ]
}

fn normalize_sensor_curve(points: &[SensorCurvePoint]) -> Vec<SensorCurvePoint> {
    let mut normalized = points
        .iter()
        .filter(|point| point.lux.is_finite() && point.lux > 0.0)
        .map(|point| SensorCurvePoint {
            lux: point.lux,
            brightness: normalize_monitor_brightness(point.brightness),
        })
        .collect::<Vec<_>>();

    normalized.sort_by(|left, right| {
        left.lux
            .partial_cmp(&right.lux)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    normalized.dedup_by(|left, right| {
        if (left.lux - right.lux).abs() < f64::EPSILON {
            right.brightness = left.brightness;
            true
        } else {
            false
        }
    });

    if normalized.is_empty() {
        default_sensor_calibration_curve()
    } else {
        normalized
    }
}

impl WeatherCache {
    fn new() -> Self {
        Self {
            snapshot: None,
            fetched_at: None,
        }
    }
}

fn load_config(path: Option<&Path>) -> Result<AppConfig, Box<dyn std::error::Error>> {
    let Some(path) = path else {
        return Ok(AppConfig::default());
    };
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    Ok(serde_json::from_str(&read_json_text(path)?)?)
}

fn load_runtime_config(path: &Path) -> Result<RuntimeConfig, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(RuntimeConfig::default());
    }
    Ok(serde_json::from_str(&read_json_text(path)?)?)
}

fn read_json_text(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(fs::read_to_string(path)?
        .trim_start_matches('\u{feff}')
        .to_string())
}

fn save_runtime_config(
    path: &Path,
    runtime: &RuntimeConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(path, serde_json::to_string_pretty(runtime)?)?;
    Ok(())
}

fn compute_target_brightness(inputs: BrightnessInputs, policy: BrightnessPolicy) -> i32 {
    let daylight_factor = inputs.daylight_factor.clamp(0.0, 1.0);
    let base = policy.brightness_min as f64
        + (policy.brightness_max - policy.brightness_min) as f64 * daylight_factor;
    let cloud_penalty = 0.20 * (inputs.cloud_cover.clamp(0, 100) as f64 / 100.0);
    let visibility_penalty = if inputs.visibility_km < 10.0 {
        0.08
    } else {
        0.0
    };
    let rain_penalty = 0.06 * inputs.precipitation_probability.clamp(0.0, 1.0);
    (base * (1.0 - cloud_penalty - visibility_penalty - rain_penalty))
        .clamp(policy.brightness_min as f64, policy.brightness_max as f64)
        .round() as i32
}

fn adapt_brightness_target(target: i32, daylight_factor: f64, runtime: &RuntimeConfig) -> i32 {
    if daylight_factor >= 0.5 {
        target.min(runtime.daytime_peak_brightness)
    } else if daylight_factor <= 0.1 {
        target.max(runtime.night_target_brightness)
    } else {
        target
    }
}

fn smooth_brightness_step(current: i32, target: i32, policy: BrightnessPolicy) -> i32 {
    let delta = target - current;
    if delta.abs() < policy.deadband {
        return current.clamp(policy.brightness_min, policy.brightness_max);
    }
    target.clamp(policy.brightness_min, policy.brightness_max)
}

fn smootherstep(position: f64) -> f64 {
    let t = position.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn brightness_transition_points(current: i32, target: i32, policy: BrightnessPolicy) -> Vec<i32> {
    let current = current.clamp(policy.brightness_min, policy.brightness_max);
    let target = target.clamp(policy.brightness_min, policy.brightness_max);
    let delta = target - current;
    if delta.abs() < policy.deadband {
        return Vec::new();
    }

    let steps = (delta.abs().max(8) as usize).min(24);
    let mut points = Vec::with_capacity(steps);
    for step in 1..=steps {
        let progress = smootherstep(step as f64 / steps as f64);
        let value = current as f64 + delta as f64 * progress;
        let value = (value.round() as i32).clamp(policy.brightness_min, policy.brightness_max);
        if points.last() != Some(&value) && value != current {
            points.push(value);
        }
    }
    if points.last() != Some(&target) {
        points.push(target);
    }
    points
}

fn compute_environment_brightness(
    policy: BrightnessPolicy,
    solar: SolarSnapshot,
    weather: Option<WeatherSnapshot>,
) -> i32 {
    let weather = weather.unwrap_or(WeatherSnapshot {
        cloud_cover: 0,
        visibility_km: 20.0,
        precipitation_probability: 0.0,
    });
    compute_target_brightness(
        BrightnessInputs {
            daylight_factor: solar.daylight_factor,
            cloud_cover: weather.cloud_cover,
            visibility_km: weather.visibility_km,
            precipitation_probability: weather.precipitation_probability,
        },
        policy,
    )
}

fn compute_sensor_brightness_target(
    lux: f64,
    runtime: &RuntimeConfig,
    policy: BrightnessPolicy,
) -> i32 {
    if !lux.is_finite() || lux <= 0.0 {
        return policy.brightness_min;
    }
    let curve = normalize_sensor_curve(&runtime.sensor_calibration_curve);
    if lux <= curve[0].lux {
        return curve[0]
            .brightness
            .clamp(policy.brightness_min, policy.brightness_max);
    }
    if let Some(last) = curve.last() {
        if lux >= last.lux {
            return last
                .brightness
                .clamp(policy.brightness_min, policy.brightness_max);
        }
    }

    for pair in curve.windows(2) {
        let left = &pair[0];
        let right = &pair[1];
        if lux >= left.lux && lux <= right.lux {
            let left_lux = left.lux.ln();
            let right_lux = right.lux.ln();
            let position = if (right_lux - left_lux).abs() < f64::EPSILON {
                0.0
            } else {
                (lux.ln() - left_lux) / (right_lux - left_lux)
            };
            let target = left.brightness as f64
                + (right.brightness - left.brightness) as f64 * position.clamp(0.0, 1.0);
            return (target.round() as i32).clamp(policy.brightness_min, policy.brightness_max);
        }
    }

    policy.brightness_min
}

#[derive(Debug, Deserialize)]
struct OpenMeteoPayload {
    current: Option<OpenMeteoCurrent>,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoCurrent {
    cloud_cover: Option<i32>,
    visibility: Option<f64>,
    precipitation_probability: Option<f64>,
}

fn normalize_weather_payload(payload: OpenMeteoPayload) -> WeatherSnapshot {
    let current = payload.current.unwrap_or(OpenMeteoCurrent {
        cloud_cover: None,
        visibility: None,
        precipitation_probability: None,
    });
    WeatherSnapshot {
        cloud_cover: current.cloud_cover.unwrap_or(0).clamp(0, 100),
        visibility_km: current.visibility.unwrap_or(20_000.0).max(0.0) / 1000.0,
        precipitation_probability: (current.precipitation_probability.unwrap_or(0.0) / 100.0)
            .clamp(0.0, 1.0),
    }
}

fn fetch_weather(config: &AppConfig, cache: &mut WeatherCache) -> Option<WeatherSnapshot> {
    let now = Instant::now();
    if let (Some(snapshot), Some(fetched_at)) = (cache.snapshot, cache.fetched_at) {
        if now.duration_since(fetched_at).as_secs() < config.weather_refresh_seconds {
            return Some(snapshot);
        }
    }

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .connect_timeout(Duration::from_millis(800))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("weather client failed, using cached or solar-only mode: {error}");
            return cache.snapshot;
        }
    };

    let response = client
        .get("https://api.open-meteo.com/v1/forecast")
        .query(&[
            ("latitude", config.latitude.to_string()),
            ("longitude", config.longitude.to_string()),
            (
                "current",
                "cloud_cover,visibility,precipitation_probability".to_string(),
            ),
            ("timezone", config.timezone_name.clone()),
        ])
        .send();

    match response.and_then(|response| response.error_for_status()) {
        Ok(response) => match response.json::<OpenMeteoPayload>() {
            Ok(payload) => {
                let snapshot = normalize_weather_payload(payload);
                cache.snapshot = Some(snapshot);
                cache.fetched_at = Some(now);
                Some(snapshot)
            }
            Err(error) => {
                eprintln!("weather parse failed, using cached or solar-only mode: {error}");
                cache.snapshot
            }
        },
        Err(error) => {
            eprintln!("weather fetch failed, using cached or solar-only mode: {error}");
            cache.snapshot
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LocalDateTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    utc_offset_minutes: i32,
}

impl LocalDateTime {
    fn now_with_offset(utc_offset_minutes: i32) -> Self {
        let unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        Self::from_unix_seconds_with_offset(unix_seconds, utc_offset_minutes)
    }

    fn from_unix_seconds_with_offset(unix_seconds: i64, utc_offset_minutes: i32) -> Self {
        let local_seconds = unix_seconds + utc_offset_minutes as i64 * 60;
        let days = local_seconds.div_euclid(86_400);
        let seconds_of_day = local_seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: (seconds_of_day / 3600) as u32,
            minute: ((seconds_of_day % 3600) / 60) as u32,
            second: (seconds_of_day % 60) as u32,
            utc_offset_minutes,
        }
    }

    fn day_of_year(self) -> u32 {
        let leap = if is_leap_year(self.year) { 1 } else { 0 };
        let month_lengths = [31, 28 + leap, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        month_lengths[..self.month.saturating_sub(1) as usize]
            .iter()
            .sum::<i32>() as u32
            + self.day
    }

    fn iso8601(self) -> String {
        let sign = if self.utc_offset_minutes >= 0 {
            '+'
        } else {
            '-'
        };
        let abs_minutes = self.utc_offset_minutes.abs();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            sign,
            abs_minutes / 60,
            abs_minutes % 60
        )
    }
}

fn timezone_offset_minutes(timezone_name: &str) -> i32 {
    match timezone_name {
        "Asia/Shanghai" | "Asia/Singapore" | "+08:00" | "UTC+8" => 8 * 60,
        "UTC" | "Etc/UTC" | "Z" => 0,
        _ => 8 * 60,
    }
}

fn compute_solar_snapshot(config: &AppConfig, now: LocalDateTime) -> SolarSnapshot {
    let elevation_degrees = solar_elevation_degrees(config.latitude, config.longitude, now);
    let daylight_factor = ((elevation_degrees + 6.0) / 66.0).clamp(0.0, 1.0).powf(1.6);
    SolarSnapshot {
        elevation_degrees,
        daylight_factor,
        is_daylight: elevation_degrees > -6.0,
    }
}

fn solar_event_minutes(config: &AppConfig, now: LocalDateTime, rising: bool) -> Option<i32> {
    let mut previous_minute = 0;
    let mut previous_elevation = solar_elevation_degrees(
        config.latitude,
        config.longitude,
        LocalDateTime {
            hour: 0,
            minute: 0,
            second: 0,
            ..now
        },
    );
    for minute in (5..=1440).step_by(5) {
        let clamped_minute = minute.min(1439);
        let elevation = solar_elevation_degrees(
            config.latitude,
            config.longitude,
            LocalDateTime {
                hour: (clamped_minute / 60) as u32,
                minute: (clamped_minute % 60) as u32,
                second: 0,
                ..now
            },
        );
        let crossed_up = previous_elevation < -6.0 && elevation >= -6.0;
        let crossed_down = previous_elevation >= -6.0 && elevation < -6.0;
        if (rising && crossed_up) || (!rising && crossed_down) {
            let fraction =
                ((-6.0 - previous_elevation) / (elevation - previous_elevation)).clamp(0.0, 1.0);
            let interpolated = previous_minute as f64 + fraction * 5.0;
            return Some(interpolated.round() as i32);
        }
        previous_minute = clamped_minute;
        previous_elevation = elevation;
    }
    None
}

fn build_light_rule_context(
    config: &AppConfig,
    now: LocalDateTime,
    weather: Option<WeatherSnapshot>,
    lux: Option<f64>,
    current_brightness: Option<i32>,
    target_brightness: Option<i32>,
) -> LightRuleContext {
    LightRuleContext {
        now,
        sunrise_minutes: solar_event_minutes(config, now, true),
        sunset_minutes: solar_event_minutes(config, now, false),
        weather_kind: weather.map(classify_weather),
        lux,
        current_brightness,
        target_brightness,
    }
}

fn solar_elevation_degrees(latitude: f64, longitude: f64, now: LocalDateTime) -> f64 {
    let day_of_year = now.day_of_year() as f64;
    let local_hour = now.hour as f64 + now.minute as f64 / 60.0 + now.second as f64 / 3600.0;
    let gamma =
        2.0 * std::f64::consts::PI / 365.0 * (day_of_year - 1.0 + (local_hour - 12.0) / 24.0);
    let equation_of_time = 229.18
        * (0.000075 + 0.001868 * gamma.cos()
            - 0.032077 * gamma.sin()
            - 0.014615 * (2.0 * gamma).cos()
            - 0.040849 * (2.0 * gamma).sin());
    let declination = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin()
        - 0.006758 * (2.0 * gamma).cos()
        + 0.000907 * (2.0 * gamma).sin()
        - 0.002697 * (3.0 * gamma).cos()
        + 0.00148 * (3.0 * gamma).sin();
    let timezone_hours = now.utc_offset_minutes as f64 / 60.0;
    let true_solar_time = (local_hour * 60.0 + equation_of_time + 4.0 * longitude
        - 60.0 * timezone_hours)
        .rem_euclid(1440.0);
    let mut hour_angle = true_solar_time / 4.0 - 180.0;
    if hour_angle < -180.0 {
        hour_angle += 360.0;
    }
    let latitude_rad = latitude.to_radians();
    let cos_zenith = latitude_rad.sin() * declination.sin()
        + latitude_rad.cos() * declination.cos() * hour_angle.to_radians().cos();
    90.0 - cos_zenith.clamp(-1.0, 1.0).acos().to_degrees()
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

trait MonitorGateway {
    fn list_monitor_info(&self) -> Result<Vec<MonitorInfo>, Box<dyn std::error::Error>>;
    fn get_brightness(&self, monitor_id: &str) -> Result<i32, Box<dyn std::error::Error>>;
    fn set_brightness(
        &self,
        monitor_id: &str,
        brightness: i32,
    ) -> Result<(), Box<dyn std::error::Error>>;
    fn set_brightness_transition(
        &self,
        monitor_id: &str,
        brightness_values: &[i32],
        interval: Duration,
    ) -> BrightnessTransitionResult {
        let mut result = BrightnessTransitionResult::default();
        for (index, brightness) in brightness_values.iter().enumerate() {
            if let Err(error) = self.set_brightness(monitor_id, *brightness) {
                result.error = Some(error.to_string());
                break;
            }
            result.applied += 1;
            if index + 1 < brightness_values.len() {
                thread::sleep(interval);
            }
        }
        result
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct BrightnessTransitionResult {
    applied: usize,
    error: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Dxva2MonitorGateway;

impl Dxva2MonitorGateway {
    fn list_records(&self) -> Result<Vec<MonitorRecord>, Box<dyn std::error::Error>> {
        list_records_platform()
    }
}

impl MonitorGateway for Dxva2MonitorGateway {
    fn list_monitor_info(&self) -> Result<Vec<MonitorInfo>, Box<dyn std::error::Error>> {
        Ok(self
            .list_records()?
            .into_iter()
            .map(|record| MonitorInfo {
                identifier: record.identifier,
                description: record.description,
            })
            .collect())
    }

    fn get_brightness(&self, monitor_id: &str) -> Result<i32, Box<dyn std::error::Error>> {
        self.list_records()?
            .into_iter()
            .find(|record| record.identifier == monitor_id)
            .map(|record| record.current)
            .ok_or_else(|| format!("unknown monitor id: {monitor_id}").into())
    }

    fn set_brightness(
        &self,
        monitor_id: &str,
        brightness: i32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        set_brightness_platform(monitor_id, normalize_monitor_brightness(brightness))
    }

    fn set_brightness_transition(
        &self,
        monitor_id: &str,
        brightness_values: &[i32],
        interval: Duration,
    ) -> BrightnessTransitionResult {
        let normalized = brightness_values
            .iter()
            .map(|value| normalize_monitor_brightness(*value))
            .collect::<Vec<_>>();
        set_brightness_transition_platform(monitor_id, &normalized, interval)
    }
}

fn parse_sensor_reading_line(line: &str) -> Result<SensorReading, Box<dyn std::error::Error>> {
    let reading: SensorReading = serde_json::from_str(line.trim())?;
    if reading.sensor != "bh1750" {
        return Err(format!("unexpected sensor: {}", reading.sensor).into());
    }
    if !reading.lux.is_finite() || reading.lux < 0.0 {
        return Err(format!("invalid lux: {}", reading.lux).into());
    }
    Ok(reading)
}

fn parse_relay_response_line(
    line: &str,
) -> Result<RelayCommandResponse, Box<dyn std::error::Error>> {
    let response: RelayCommandResponse = serde_json::from_str(line.trim())?;
    if response.command != "relay" {
        return Err(format!("unexpected command response: {}", response.command).into());
    }
    Ok(response)
}

fn send_relay_command(
    port: &str,
    desired_relay_state: RelayState,
) -> Result<RelayCommandResponse, Box<dyn std::error::Error>> {
    send_relay_command_with_min_deadline(port, desired_relay_state, 4)
}

fn send_relay_command_with_min_deadline(
    port: &str,
    desired_relay_state: RelayState,
    min_deadline_seconds: u64,
) -> Result<RelayCommandResponse, Box<dyn std::error::Error>> {
    let mut serial = serialport::new(port, 115_200)
        .timeout(Duration::from_millis(1000))
        .open()?;
    serial.write_data_terminal_ready(true)?;
    serial.write_request_to_send(false)?;
    thread::sleep(Duration::from_millis(300));
    serial
        .write_all(format!("relay {}\n", desired_relay_state.label().to_lowercase()).as_bytes())?;
    serial.flush()?;

    let mut reader = BufReader::new(serial);
    let mut line = String::new();
    let deadline = Instant::now() + Duration::from_secs(min_deadline_seconds.max(1));

    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(response) = parse_relay_response_line(trimmed) {
                    if response.action == desired_relay_state.label().to_lowercase() {
                        return Ok(response);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err(format!("timed out after sending relay command to {port}").into())
}

#[cfg(not(windows))]
fn list_records_platform() -> Result<Vec<MonitorRecord>, Box<dyn std::error::Error>> {
    Err("DXVA2 monitor control is only available on Windows".into())
}

#[cfg(not(windows))]
fn set_brightness_platform(
    _monitor_id: &str,
    _brightness: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    Err("DXVA2 monitor control is only available on Windows".into())
}

#[cfg(not(windows))]
fn set_brightness_transition_platform(
    _monitor_id: &str,
    brightness_values: &[i32],
    _interval: Duration,
) -> BrightnessTransitionResult {
    BrightnessTransitionResult {
        applied: 0,
        error: (!brightness_values.is_empty())
            .then(|| "DXVA2 monitor control is only available on Windows".to_string()),
    }
}

#[cfg(windows)]
mod windows_monitor {
    use super::MonitorRecord;
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use std::thread;
    use std::time::Duration;

    type Bool = i32;
    type Dword = u32;
    type Lparam = isize;
    type Handle = *mut c_void;
    type Hmonitor = Handle;
    type Hdc = Handle;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
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
    }

    #[link(name = "Dxva2")]
    extern "system" {
        fn GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor: Hmonitor, count: *mut Dword) -> Bool;
        fn GetPhysicalMonitorsFromHMONITOR(
            hmonitor: Hmonitor,
            array_size: Dword,
            array: *mut PhysicalMonitor,
        ) -> Bool;
        fn DestroyPhysicalMonitors(array_size: Dword, array: *mut PhysicalMonitor) -> Bool;
        fn GetMonitorBrightness(
            handle: Handle,
            min: *mut Dword,
            current: *mut Dword,
            max: *mut Dword,
        ) -> Bool;
        fn SetMonitorBrightness(handle: Handle, brightness: Dword) -> Bool;
    }

    pub fn list_records() -> Result<Vec<MonitorRecord>, Box<dyn std::error::Error>> {
        let mut records = Vec::<MonitorRecord>::new();
        let ok = unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_read_proc),
                &mut records as *mut _ as Lparam,
            )
        };
        if ok == 0 {
            return Err("EnumDisplayMonitors failed".into());
        }
        for (index, record) in records.iter_mut().enumerate() {
            record.identifier = format!("monitor-{}", index + 1);
        }
        Ok(records)
    }

    unsafe extern "system" fn enum_read_proc(
        hmonitor: Hmonitor,
        _hdc: Hdc,
        _rect: *mut Rect,
        data: Lparam,
    ) -> Bool {
        let records = &mut *(data as *mut Vec<MonitorRecord>);
        let mut count: Dword = 0;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) == 0 || count == 0 {
            return 1;
        }
        let mut monitors: Vec<PhysicalMonitor> = vec![zeroed(); count as usize];
        if GetPhysicalMonitorsFromHMONITOR(hmonitor, count, monitors.as_mut_ptr()) != 0 {
            for monitor in monitors.iter() {
                let mut min: Dword = 0;
                let mut current: Dword = 0;
                let mut max: Dword = 0;
                if GetMonitorBrightness(monitor.handle, &mut min, &mut current, &mut max) != 0 {
                    records.push(MonitorRecord {
                        identifier: String::new(),
                        description: wide_to_string(&monitor.description),
                        current: current as i32,
                    });
                }
            }
            DestroyPhysicalMonitors(count, monitors.as_mut_ptr());
        }
        1
    }

    struct SetRequest {
        target_ordinal: usize,
        target_percent: i32,
        current_ordinal: usize,
        updated: bool,
        failed: bool,
    }

    pub fn set_brightness(
        monitor_id: &str,
        brightness: i32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let ordinal = monitor_id
            .rsplit('-')
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let mut request = SetRequest {
            target_ordinal: ordinal,
            target_percent: brightness,
            current_ordinal: 0,
            updated: false,
            failed: false,
        };
        let ok = unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_set_proc),
                &mut request as *mut _ as Lparam,
            )
        };
        if ok == 0 {
            return Err("EnumDisplayMonitors failed".into());
        }
        if request.failed {
            return Err(format!("SetMonitorBrightness failed for monitor-{ordinal}").into());
        }
        if !request.updated {
            return Err(format!("monitor-{ordinal} was not found").into());
        }
        Ok(())
    }

    struct TransitionRequest {
        target_ordinal: usize,
        target_percents: Vec<i32>,
        interval: Duration,
        current_ordinal: usize,
        applied: usize,
        failed: bool,
    }

    pub fn set_brightness_transition(
        monitor_id: &str,
        brightness_values: &[i32],
        interval: Duration,
    ) -> super::BrightnessTransitionResult {
        if brightness_values.is_empty() {
            return super::BrightnessTransitionResult::default();
        }
        let ordinal = monitor_id
            .rsplit('-')
            .next()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1);
        let mut request = TransitionRequest {
            target_ordinal: ordinal,
            target_percents: brightness_values.to_vec(),
            interval,
            current_ordinal: 0,
            applied: 0,
            failed: false,
        };
        let ok = unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enum_transition_proc),
                &mut request as *mut _ as Lparam,
            )
        };
        let error = if ok == 0 {
            Some("EnumDisplayMonitors failed".to_string())
        } else if request.failed {
            Some(format!("SetMonitorBrightness failed for monitor-{ordinal}"))
        } else if request.applied == 0 {
            Some(format!("monitor-{ordinal} was not found"))
        } else {
            None
        };
        super::BrightnessTransitionResult {
            applied: request.applied,
            error,
        }
    }

    unsafe extern "system" fn enum_transition_proc(
        hmonitor: Hmonitor,
        _hdc: Hdc,
        _rect: *mut Rect,
        data: Lparam,
    ) -> Bool {
        let request = &mut *(data as *mut TransitionRequest);
        let mut count: Dword = 0;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) == 0 || count == 0 {
            return 1;
        }
        let mut monitors: Vec<PhysicalMonitor> = vec![zeroed(); count as usize];
        if GetPhysicalMonitorsFromHMONITOR(hmonitor, count, monitors.as_mut_ptr()) != 0 {
            for monitor in monitors.iter() {
                let mut min: Dword = 0;
                let mut current: Dword = 0;
                let mut max: Dword = 0;
                if GetMonitorBrightness(monitor.handle, &mut min, &mut current, &mut max) != 0 {
                    request.current_ordinal += 1;
                    if request.current_ordinal == request.target_ordinal {
                        for (index, target_percent) in request.target_percents.iter().enumerate() {
                            let raw_target =
                                min as f64 + (max - min) as f64 * (*target_percent as f64 / 100.0);
                            if SetMonitorBrightness(monitor.handle, raw_target.round() as Dword)
                                == 0
                            {
                                request.failed = true;
                                break;
                            }
                            request.applied += 1;
                            if index + 1 < request.target_percents.len() {
                                thread::sleep(request.interval);
                            }
                        }
                    }
                }
            }
            DestroyPhysicalMonitors(count, monitors.as_mut_ptr());
        }
        1
    }

    unsafe extern "system" fn enum_set_proc(
        hmonitor: Hmonitor,
        _hdc: Hdc,
        _rect: *mut Rect,
        data: Lparam,
    ) -> Bool {
        let request = &mut *(data as *mut SetRequest);
        let mut count: Dword = 0;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(hmonitor, &mut count) == 0 || count == 0 {
            return 1;
        }
        let mut monitors: Vec<PhysicalMonitor> = vec![zeroed(); count as usize];
        if GetPhysicalMonitorsFromHMONITOR(hmonitor, count, monitors.as_mut_ptr()) != 0 {
            for monitor in monitors.iter() {
                let mut min: Dword = 0;
                let mut current: Dword = 0;
                let mut max: Dword = 0;
                if GetMonitorBrightness(monitor.handle, &mut min, &mut current, &mut max) != 0 {
                    request.current_ordinal += 1;
                    if request.current_ordinal == request.target_ordinal {
                        let raw_target = min as f64
                            + (max - min) as f64 * (request.target_percent as f64 / 100.0);
                        if SetMonitorBrightness(monitor.handle, raw_target.round() as Dword) == 0 {
                            request.failed = true;
                        } else {
                            request.updated = true;
                        }
                    }
                }
            }
            DestroyPhysicalMonitors(count, monitors.as_mut_ptr());
        }
        1
    }

    fn wide_to_string(raw: &[u16]) -> String {
        let len = raw
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(raw.len());
        String::from_utf16_lossy(&raw[..len]).trim().to_string()
    }
}

#[cfg(windows)]
fn list_records_platform() -> Result<Vec<MonitorRecord>, Box<dyn std::error::Error>> {
    windows_monitor::list_records()
}

#[cfg(windows)]
fn set_brightness_platform(
    monitor_id: &str,
    brightness: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    windows_monitor::set_brightness(monitor_id, brightness)
}

#[cfg(windows)]
fn set_brightness_transition_platform(
    monitor_id: &str,
    brightness_values: &[i32],
    interval: Duration,
) -> BrightnessTransitionResult {
    windows_monitor::set_brightness_transition(monitor_id, brightness_values, interval)
}

fn apply_brightness_step<G: MonitorGateway>(
    gateway: &G,
    target: i32,
    policy: BrightnessPolicy,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let monitors = gateway.list_monitor_info()?;
    let mut current_brightness = BTreeMap::new();
    for monitor in &monitors {
        match gateway.get_brightness(&monitor.identifier) {
            Ok(current) => {
                current_brightness.insert(monitor.identifier.clone(), current);
            }
            Err(error) => eprintln!("monitor={} read_failed={error}", monitor.identifier),
        }
    }
    let _ = apply_brightness_step_from_snapshot(
        gateway,
        &monitors,
        &current_brightness,
        target,
        policy,
        dry_run,
    );
    Ok(())
}

fn apply_brightness_step_from_snapshot<G: MonitorGateway>(
    gateway: &G,
    monitors: &[MonitorInfo],
    current_brightness: &BTreeMap<String, i32>,
    target: i32,
    policy: BrightnessPolicy,
    dry_run: bool,
) -> BTreeMap<String, i32> {
    let mut next_brightness = current_brightness.clone();
    for monitor in monitors {
        let Some(current) = current_brightness.get(&monitor.identifier).copied() else {
            continue;
        };
        let points = brightness_transition_points(current, target, policy);
        let next = points
            .last()
            .copied()
            .unwrap_or_else(|| current.clamp(policy.brightness_min, policy.brightness_max));
        println!(
            "monitor={} current={} target={} next={} transition_steps={} dry_run={}",
            monitor.identifier,
            current,
            target,
            next,
            points.len(),
            dry_run
        );
        if dry_run {
            continue;
        }
        let result = gateway.set_brightness_transition(
            &monitor.identifier,
            &points,
            Duration::from_millis(50),
        );
        if result.applied > 0 {
            if let Some(applied) = points.get(result.applied.min(points.len()) - 1) {
                next_brightness.insert(monitor.identifier.clone(), *applied);
            }
        }
        if let Some(error) = result.error {
            eprintln!("monitor={} update_failed={error}", monitor.identifier);
        }
    }
    next_brightness
}

fn capture_calibration_point<G: MonitorGateway>(
    gateway: &G,
    monitor_id: &str,
    label: &str,
    sample_count: usize,
) -> Result<CalibrationPoint, Box<dyn std::error::Error>> {
    let mut samples = Vec::with_capacity(sample_count.max(1));
    for _ in 0..sample_count.max(1) {
        samples.push(gateway.get_brightness(monitor_id)?);
    }
    let min_raw = *samples.iter().min().unwrap_or(&0);
    let max_raw = *samples.iter().max().unwrap_or(&0);
    let average_raw = (samples.iter().sum::<i32>() as f64 / samples.len() as f64).round() as i32;
    Ok(CalibrationPoint {
        label: label.to_string(),
        samples,
        average_raw,
        min_raw,
        max_raw,
        captured_at: LocalDateTime::now_with_offset(8 * 60).iso8601(),
    })
}

fn policy_from_config(config: &AppConfig) -> BrightnessPolicy {
    let (brightness_min, brightness_max) =
        normalize_monitor_brightness_bounds(config.brightness_min, config.brightness_max);
    BrightnessPolicy {
        brightness_min,
        brightness_max,
        deadband: config.brightness_deadband.max(0),
        maximum_step: config.maximum_step_per_tick.max(1),
    }
}

fn run_cycle<G: MonitorGateway>(
    config: &AppConfig,
    runtime: &RuntimeConfig,
    gateway: &G,
    cache: &mut WeatherCache,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = LocalDateTime::now_with_offset(timezone_offset_minutes(&config.timezone_name));
    let solar = compute_solar_snapshot(config, now);
    let weather = fetch_weather(config, cache);
    let policy = policy_from_config(config);
    let mut target = compute_environment_brightness(policy, solar, weather);
    target = adapt_brightness_target(target, solar.daylight_factor, runtime);
    println!(
        "time={} elevation={:.2} daylight_factor={:.3} weather={:?} target={} dry_run={}",
        now.iso8601(),
        solar.elevation_degrees,
        solar.daylight_factor,
        weather,
        target,
        dry_run
    );
    if dry_run || !runtime.paused {
        apply_brightness_step(gateway, target, policy, dry_run)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn gui_loop<G: MonitorGateway>(
    config: &AppConfig,
    runtime_path: &Path,
    runtime: &mut RuntimeConfig,
    gateway: &G,
    cache: &mut WeatherCache,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("Brightness Controller Rust control panel");
    println!("Commands: r=run now, p=pause/resume, s=settings, c=calibrate, i=identify, m=monitors, q=quit");
    loop {
        println!(
            "\nstate={} daytime_peak={} night_target={}",
            if runtime.paused { "paused" } else { "active" },
            runtime.daytime_peak_brightness,
            runtime.night_target_brightness
        );
        print!("command> ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        match input.trim() {
            "r" => {
                let _ = run_cycle(config, runtime, gateway, cache, false);
            }
            "p" => {
                runtime.paused = !runtime.paused;
                save_runtime_config(runtime_path, runtime)?;
                println!("paused={}", runtime.paused);
            }
            "s" => {
                runtime.daytime_peak_brightness = normalize_monitor_brightness(prompt_i32(
                    "daytime_peak_brightness",
                    runtime.daytime_peak_brightness,
                )?);
                runtime.night_target_brightness = normalize_monitor_brightness(prompt_i32(
                    "night_target_brightness",
                    runtime.night_target_brightness,
                )?);
                save_runtime_config(runtime_path, runtime)?;
            }
            "c" => {
                for monitor in gateway.list_monitor_info()? {
                    println!("{} - {}", monitor.identifier, monitor.description);
                }
                let monitor_id = prompt("monitor id")?;
                for (point_key, label) in [
                    ("manual_0", "0%"),
                    ("manual_50", "50%"),
                    ("manual_100", "100%"),
                ] {
                    let _ = prompt(&format!("manually set {label}, then press Enter"))?;
                    let point = capture_calibration_point(gateway, &monitor_id, label, 3)?;
                    runtime
                        .monitor_calibrations
                        .entry(monitor_id.clone())
                        .or_default()
                        .insert(point_key.to_string(), point);
                    save_runtime_config(runtime_path, runtime)?;
                    println!("saved {label} for {monitor_id}");
                }
            }
            "i" => {
                for monitor in gateway.list_monitor_info()? {
                    println!("{} - {}", monitor.identifier, monitor.description);
                }
                let monitor_id = prompt("monitor id")?;
                let current = gateway.get_brightness(&monitor_id)?;
                let flash = if current <= config.brightness_max - 12 {
                    current + 12
                } else {
                    (current - 12).max(config.brightness_min)
                };
                gateway.set_brightness(&monitor_id, flash)?;
                thread::sleep(Duration::from_millis(450));
                gateway.set_brightness(&monitor_id, current)?;
            }
            "m" => {
                for monitor in gateway.list_monitor_info()? {
                    println!("{} - {}", monitor.identifier, monitor.description);
                }
            }
            "q" => break,
            _ => println!("unknown command"),
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn prompt(label: &str) -> Result<String, Box<dyn std::error::Error>> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim().to_string())
}

#[cfg(not(windows))]
fn prompt_i32(label: &str, default: i32) -> Result<i32, Box<dyn std::error::Error>> {
    let value = prompt(&format!("{label} [{default}]"))?;
    Ok(if value.is_empty() {
        default
    } else {
        value.parse().unwrap_or(default)
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Oneshot,
    DryRun,
    Daemon,
    Gui,
    SensorTest { port: String, samples: usize },
    SensorDryRun { port: String, samples: usize },
    SensorOneshot { port: String, samples: usize },
}

fn parse_args() -> Result<(Mode, Option<PathBuf>), String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I>(args: I) -> Result<(Mode, Option<PathBuf>), String>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut mode = None;
    let mut config_path = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_ref() {
            "oneshot" => mode = Some(Mode::Oneshot),
            "dry-run" => mode = Some(Mode::DryRun),
            "daemon" => mode = Some(Mode::Daemon),
            "gui" => mode = Some(Mode::Gui),
            "sensor-test" | "sensor-dry-run" | "sensor-oneshot" => {
                let sensor_mode = arg.as_ref().to_string();
                let mut port = "COM3".to_string();
                let mut samples = 5usize;
                while let Some(next) = args.next() {
                    match next.as_ref() {
                        "--config" => {
                            let path = args
                                .next()
                                .ok_or_else(|| "--config requires a path".to_string())?;
                            config_path = Some(PathBuf::from(path.as_ref()));
                        }
                        "--port" => {
                            let value = args
                                .next()
                                .ok_or_else(|| "--port requires a value".to_string())?;
                            port = value.as_ref().to_string();
                        }
                        "--samples" => {
                            let value = args
                                .next()
                                .ok_or_else(|| "--samples requires a value".to_string())?;
                            samples = value
                                .as_ref()
                                .parse::<usize>()
                                .map_err(|_| "--samples requires a positive integer".to_string())?
                                .max(1);
                        }
                        other => {
                            return Err(format!(
                                "unknown sensor-test argument: {other}\n{}",
                                usage()
                            ))
                        }
                    }
                }
                mode = Some(match sensor_mode.as_str() {
                    "sensor-test" => Mode::SensorTest { port, samples },
                    "sensor-dry-run" => Mode::SensorDryRun { port, samples },
                    "sensor-oneshot" => Mode::SensorOneshot { port, samples },
                    _ => unreachable!("sensor mode is matched above"),
                });
            }
            "--config" => {
                let path = args
                    .next()
                    .ok_or_else(|| "--config requires a path".to_string())?;
                config_path = Some(PathBuf::from(path.as_ref()));
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument: {other}\n{}", usage())),
        }
    }
    Ok((mode.unwrap_or(Mode::Gui), config_path))
}

fn usage() -> String {
    "usage: screen-brightness <oneshot|dry-run|daemon|gui|sensor-test|sensor-dry-run|sensor-oneshot> [--config PATH] [--port COM3] [--samples N]".to_string()
}

fn read_sensor_samples(
    port: &str,
    samples: usize,
) -> Result<Vec<SensorReading>, Box<dyn std::error::Error>> {
    read_sensor_samples_with_min_deadline(port, samples, 15)
}

fn read_sensor_samples_with_min_deadline(
    port: &str,
    samples: usize,
    min_deadline_seconds: u64,
) -> Result<Vec<SensorReading>, Box<dyn std::error::Error>> {
    let mut serial = serialport::new(port, 115_200)
        .timeout(Duration::from_millis(1000))
        .open()?;
    serial.write_data_terminal_ready(true)?;
    serial.write_request_to_send(false)?;
    thread::sleep(Duration::from_millis(300));

    let mut reader = BufReader::new(serial);
    let mut line = String::new();
    let mut readings = Vec::with_capacity(samples);
    let deadline = Instant::now()
        + Duration::from_secs((samples as u64).saturating_mul(5).max(min_deadline_seconds));

    while readings.len() < samples && Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match parse_sensor_reading_line(trimmed) {
                    Ok(reading) => {
                        readings.push(reading);
                    }
                    Err(error) => eprintln!("ignored sensor line: {trimmed} ({error})"),
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
            Err(error) => return Err(error.into()),
        }
    }

    if readings.len() == samples {
        Ok(readings)
    } else {
        Err(format!(
            "timed out after collecting {}/{} sample(s) from {port}",
            readings.len(),
            samples
        )
        .into())
    }
}

fn run_sensor_test(port: &str, samples: usize) -> Result<(), Box<dyn std::error::Error>> {
    println!("reading {samples} BH1750 sample(s) from {port} at 115200 baud");
    for (index, reading) in read_sensor_samples(port, samples)?.iter().enumerate() {
        println!(
            "sample={} sensor={} lux={:.2} addr={} sda={} scl={} relay={} relay_gpio={}",
            index + 1,
            reading.sensor,
            reading.lux,
            reading.addr,
            reading.sda,
            reading.scl,
            reading
                .relay
                .map(|relay| relay.label())
                .unwrap_or("unknown"),
            reading
                .relay_gpio
                .map(|gpio| gpio.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
    }
    Ok(())
}

fn run_sensor_cycle<G: MonitorGateway>(
    config: &AppConfig,
    runtime: &RuntimeConfig,
    gateway: &G,
    port: &str,
    samples: usize,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let readings = read_sensor_samples(port, samples)?;
    let average_lux =
        readings.iter().map(|reading| reading.lux).sum::<f64>() / readings.len() as f64;
    let policy = policy_from_config(config);
    let target = compute_sensor_brightness_target(average_lux, runtime, policy);
    println!("sensor_lux={average_lux:.2} target={target} dry_run={dry_run}");
    apply_brightness_step(gateway, target, policy, dry_run)
}

fn main() {
    let (mode, config_path) = match parse_args() {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let config = match load_config(config_path.as_deref()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config error: {error}");
            std::process::exit(1);
        }
    };
    let runtime_path = config_path.unwrap_or_else(|| PathBuf::from("config.json"));
    let mut runtime = load_runtime_config(&runtime_path).unwrap_or_default();
    normalize_runtime_config(&mut runtime);
    let gateway = Dxva2MonitorGateway;
    let mut cache = WeatherCache::new();
    let result = match mode {
        Mode::Oneshot => run_cycle(&config, &runtime, &gateway, &mut cache, false),
        Mode::DryRun => run_cycle(&config, &runtime, &gateway, &mut cache, true),
        Mode::Daemon => loop {
            if let Err(error) = run_cycle(&config, &runtime, &gateway, &mut cache, false) {
                eprintln!("{error}");
            }
            thread::sleep(Duration::from_secs(config.control_tick_seconds));
        },
        Mode::Gui => {
            #[cfg(windows)]
            {
                native_gui::run_native_gui(
                    &config,
                    &runtime_path,
                    &mut runtime,
                    &gateway,
                    &mut cache,
                )
            }
            #[cfg(not(windows))]
            {
                gui_loop(&config, &runtime_path, &mut runtime, &gateway, &mut cache)
            }
        }
        Mode::SensorTest { port, samples } => run_sensor_test(&port, samples),
        Mode::SensorDryRun { port, samples } => {
            run_sensor_cycle(&config, &runtime, &gateway, &port, samples, true)
        }
        Mode::SensorOneshot { port, samples } => {
            run_sensor_cycle(&config, &runtime, &gateway, &port, samples, false)
        }
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn parse_args_defaults_to_gui_mode() {
        let (mode, config_path) =
            parse_args_from(Vec::<String>::new()).expect("parse should succeed");
        assert_eq!(mode, Mode::Gui);
        assert!(config_path.is_none());
    }

    #[test]
    fn parse_args_accepts_sensor_test_mode_with_defaults() {
        let (mode, config_path) = parse_args_from(["sensor-test"]).expect("parse should succeed");
        assert_eq!(
            mode,
            Mode::SensorTest {
                port: "COM3".to_string(),
                samples: 5
            }
        );
        assert!(config_path.is_none());
    }

    #[test]
    fn parse_args_accepts_sensor_test_port_and_sample_options() {
        let (mode, config_path) =
            parse_args_from(["sensor-test", "--port", "COM7", "--samples", "3"])
                .expect("parse should succeed");
        assert_eq!(
            mode,
            Mode::SensorTest {
                port: "COM7".to_string(),
                samples: 3
            }
        );
        assert!(config_path.is_none());
    }

    #[test]
    fn parse_args_accepts_sensor_dry_run_mode() {
        let (mode, config_path) =
            parse_args_from(["sensor-dry-run", "--port", "COM7", "--samples", "3"])
                .expect("parse should succeed");
        assert_eq!(
            mode,
            Mode::SensorDryRun {
                port: "COM7".to_string(),
                samples: 3
            }
        );
        assert!(config_path.is_none());
    }

    #[test]
    fn parse_args_accepts_sensor_oneshot_mode() {
        let (mode, config_path) =
            parse_args_from(["sensor-oneshot", "--port", "COM7", "--samples", "3"])
                .expect("parse should succeed");
        assert_eq!(
            mode,
            Mode::SensorOneshot {
                port: "COM7".to_string(),
                samples: 3
            }
        );
        assert!(config_path.is_none());
    }

    #[test]
    fn sensor_lux_mapping_uses_current_room_as_reference() {
        let config = AppConfig::default();
        let runtime = RuntimeConfig::default();
        let policy = policy_from_config(&config);
        assert_eq!(compute_sensor_brightness_target(80.0, &runtime, policy), 72);
        assert_eq!(compute_sensor_brightness_target(20.0, &runtime, policy), 40);
        assert_eq!(
            compute_sensor_brightness_target(250.0, &runtime, policy),
            88
        );
    }

    #[test]
    fn sensor_lux_mapping_respects_policy_bounds() {
        let runtime = RuntimeConfig {
            sensor_calibration_curve: vec![
                SensorCurvePoint {
                    lux: 20.0,
                    brightness: 5,
                },
                SensorCurvePoint {
                    lux: 10000.0,
                    brightness: 95,
                },
            ],
            ..RuntimeConfig::default()
        };
        let policy = BrightnessPolicy {
            brightness_min: 20,
            brightness_max: 85,
            deadband: 2,
            maximum_step: 4,
        };
        assert_eq!(compute_sensor_brightness_target(0.0, &runtime, policy), 20);
        assert_eq!(
            compute_sensor_brightness_target(10000.0, &runtime, policy),
            85
        );
    }

    #[test]
    fn sensor_curve_interpolates_in_log_lux_space() {
        let runtime = RuntimeConfig {
            sensor_calibration_curve: vec![
                SensorCurvePoint {
                    lux: 20.0,
                    brightness: 40,
                },
                SensorCurvePoint {
                    lux: 80.0,
                    brightness: 72,
                },
            ],
            ..RuntimeConfig::default()
        };
        let policy = policy_from_config(&AppConfig::default());
        assert_eq!(compute_sensor_brightness_target(40.0, &runtime, policy), 56);
    }

    #[test]
    fn runtime_normalization_repairs_sensor_curve() {
        let mut runtime = RuntimeConfig {
            sensor_calibration_curve: vec![
                SensorCurvePoint {
                    lux: 100.0,
                    brightness: 120,
                },
                SensorCurvePoint {
                    lux: -5.0,
                    brightness: 50,
                },
                SensorCurvePoint {
                    lux: 20.0,
                    brightness: -10,
                },
                SensorCurvePoint {
                    lux: f64::NAN,
                    brightness: 40,
                },
            ],
            ..RuntimeConfig::default()
        };

        normalize_runtime_config(&mut runtime);

        assert_eq!(
            runtime.sensor_calibration_curve,
            vec![
                SensorCurvePoint {
                    lux: 20.0,
                    brightness: 0
                },
                SensorCurvePoint {
                    lux: 100.0,
                    brightness: 100
                },
            ]
        );
    }

    #[test]
    fn sensor_reading_parses_json_line_from_esp32() {
        let reading = parse_sensor_reading_line(
            r#"{"sensor":"bh1750","lux":34.17,"addr":"0x23","sda":4,"scl":5}"#,
        )
        .expect("sensor JSON should parse");
        assert_eq!(reading.sensor, "bh1750");
        assert_eq!(reading.addr, "0x23");
        assert_eq!(reading.sda, 4);
        assert_eq!(reading.scl, 5);
        assert!((reading.lux - 34.17).abs() < 0.001);
        assert_eq!(reading.relay, None);
        assert_eq!(reading.relay_gpio, None);
    }

    #[test]
    fn sensor_reading_parses_optional_relay_fields() {
        let reading = parse_sensor_reading_line(
            r#"{"sensor":"bh1750","lux":95.5,"addr":"0x23","sda":4,"scl":5,"relay":"on","relay_gpio":10}"#,
        )
        .expect("sensor JSON should parse");
        assert_eq!(reading.relay, Some(RelayState::On));
        assert_eq!(reading.relay_gpio, Some(10));
    }

    #[test]
    fn relay_contact_mode_maps_light_state_to_relay_state() {
        assert_eq!(
            RelayContactMode::No.relay_state_for_light(true),
            RelayState::On
        );
        assert_eq!(
            RelayContactMode::No.relay_state_for_light(false),
            RelayState::Off
        );
        assert_eq!(
            RelayContactMode::Nc.relay_state_for_light(true),
            RelayState::Off
        );
        assert_eq!(
            RelayContactMode::Nc.relay_state_for_light(false),
            RelayState::On
        );
        assert!(RelayState::On.light_on(RelayContactMode::No));
        assert!(RelayState::Off.light_on(RelayContactMode::Nc));
    }

    #[test]
    fn relay_response_parses_serial_ack() {
        let response = parse_relay_response_line(
            r#"{"command":"relay","action":"on","relay":"on","relay_gpio":10,"relay_active_low":true}"#,
        )
        .expect("relay response should parse");
        assert_eq!(response.command, "relay");
        assert_eq!(response.action, "on");
        assert_eq!(response.relay, RelayState::On);
        assert_eq!(response.relay_gpio, 10);
    }

    #[test]
    fn serial_empty_reads_back_off_before_retrying() {
        let source = include_str!("main.rs");
        assert!(
            source
                .matches("Ok(0) => {\n                thread::sleep(Duration::from_millis(10));")
                .count()
                >= 2,
            "sensor and relay serial loops should not spin on empty reads"
        );
    }

    #[test]
    fn light_rules_use_first_matching_priority_rule() {
        let context = LightRuleContext {
            now: LocalDateTime {
                year: 2026,
                month: 6,
                day: 29,
                hour: 21,
                minute: 15,
                second: 0,
                utc_offset_minutes: 8 * 60,
            },
            sunrise_minutes: Some(300),
            sunset_minutes: Some(1140),
            weather_kind: Some(WeatherKind::Clear),
            lux: Some(12.0),
            current_brightness: Some(70),
            target_brightness: Some(68),
        };
        let rules = vec![
            LightRule {
                name: "late bright monitor wins".to_string(),
                enabled: true,
                all: vec![
                    LightRuleCondition::TimeAfter { minutes: 21 * 60 },
                    LightRuleCondition::CurrentBrightnessAbove { brightness: 60 },
                ],
                any: Vec::new(),
                then_action: LightRuleAction::Off,
            },
            LightRule {
                name: "dark room would turn on".to_string(),
                enabled: true,
                all: vec![LightRuleCondition::LuxBelow { lux: 30.0 }],
                any: Vec::new(),
                then_action: LightRuleAction::On,
            },
        ];

        let decision = evaluate_light_rules(&rules, LightRuleAction::Keep, &context);

        assert_eq!(
            decision,
            LightRuleDecision {
                action: LightRuleAction::Off,
                matched_rule: Some("late bright monitor wins".to_string()),
            }
        );
    }

    #[test]
    fn light_rules_use_sunset_lux_and_fallback() {
        let mut context = LightRuleContext {
            now: LocalDateTime {
                year: 2026,
                month: 6,
                day: 29,
                hour: 20,
                minute: 0,
                second: 0,
                utc_offset_minutes: 8 * 60,
            },
            sunrise_minutes: Some(300),
            sunset_minutes: Some(1140),
            weather_kind: Some(WeatherKind::Cloudy),
            lux: Some(18.0),
            current_brightness: Some(56),
            target_brightness: Some(60),
        };
        let rules = vec![LightRule {
            name: "after sunset and dim room".to_string(),
            enabled: true,
            all: vec![
                LightRuleCondition::AfterSunset {
                    offset_minutes: -30,
                },
                LightRuleCondition::LuxBelow { lux: 30.0 },
            ],
            any: Vec::new(),
            then_action: LightRuleAction::On,
        }];

        assert_eq!(
            evaluate_light_rules(&rules, LightRuleAction::Off, &context),
            LightRuleDecision {
                action: LightRuleAction::On,
                matched_rule: Some("after sunset and dim room".to_string()),
            }
        );

        context.lux = Some(80.0);
        assert_eq!(
            evaluate_light_rules(&rules, LightRuleAction::Off, &context),
            LightRuleDecision {
                action: LightRuleAction::Off,
                matched_rule: None,
            }
        );
    }

    #[test]
    fn weather_fetch_is_needed_only_for_enabled_weather_rules() {
        let time_rule = LightRule {
            name: "time only".to_string(),
            enabled: true,
            all: vec![LightRuleCondition::TimeAfter { minutes: 19 * 60 }],
            any: Vec::new(),
            then_action: LightRuleAction::On,
        };
        let disabled_weather_rule = LightRule {
            name: "disabled weather".to_string(),
            enabled: false,
            all: vec![LightRuleCondition::WeatherIs {
                kind: WeatherKind::Rain,
            }],
            any: Vec::new(),
            then_action: LightRuleAction::Off,
        };
        let weather_rule = LightRule {
            name: "weather".to_string(),
            enabled: true,
            all: Vec::new(),
            any: vec![LightRuleCondition::WeatherIs {
                kind: WeatherKind::Cloudy,
            }],
            then_action: LightRuleAction::Off,
        };

        assert!(!light_rules_need_weather(std::slice::from_ref(&time_rule)));
        assert!(!light_rules_need_weather(&[
            time_rule,
            disabled_weather_rule
        ]));
        assert!(light_rules_need_weather(&[weather_rule]));
    }

    #[test]
    fn brightness_step_from_snapshot_does_not_rescan_monitors() {
        struct SnapshotGateway {
            list_calls: Cell<usize>,
            get_calls: Cell<usize>,
            set_values: RefCell<Vec<(String, i32)>>,
        }

        impl MonitorGateway for SnapshotGateway {
            fn list_monitor_info(&self) -> Result<Vec<MonitorInfo>, Box<dyn std::error::Error>> {
                self.list_calls.set(self.list_calls.get() + 1);
                Ok(Vec::new())
            }

            fn get_brightness(&self, _monitor_id: &str) -> Result<i32, Box<dyn std::error::Error>> {
                self.get_calls.set(self.get_calls.get() + 1);
                Ok(0)
            }

            fn set_brightness(
                &self,
                monitor_id: &str,
                brightness: i32,
            ) -> Result<(), Box<dyn std::error::Error>> {
                self.set_values
                    .borrow_mut()
                    .push((monitor_id.to_string(), brightness));
                Ok(())
            }
        }

        let gateway = SnapshotGateway {
            list_calls: Cell::new(0),
            get_calls: Cell::new(0),
            set_values: RefCell::new(Vec::new()),
        };
        let monitors = vec![MonitorInfo {
            identifier: "display-1".to_string(),
            description: "Display 1".to_string(),
        }];
        let mut current = BTreeMap::new();
        current.insert("display-1".to_string(), 10);
        let policy = BrightnessPolicy {
            brightness_min: 0,
            brightness_max: 100,
            deadband: 0,
            maximum_step: 4,
        };

        let next =
            apply_brightness_step_from_snapshot(&gateway, &monitors, &current, 11, policy, false);

        assert_eq!(gateway.list_calls.get(), 0);
        assert_eq!(gateway.get_calls.get(), 0);
        assert_eq!(
            gateway.set_values.borrow().as_slice(),
            &[("display-1".to_string(), 11)]
        );
        assert_eq!(next.get("display-1"), Some(&11));
    }

    #[test]
    fn brightness_step_from_snapshot_uses_batched_transition_writes() {
        let source = include_str!("main.rs");
        let apply_source = source
            .split("fn apply_brightness_step_from_snapshot")
            .nth(1)
            .expect("apply_brightness_step_from_snapshot should exist")
            .split("fn capture_calibration_point")
            .next()
            .expect("apply_brightness_step_from_snapshot should appear before capture");

        assert!(
            apply_source.contains("set_brightness_transition"),
            "smooth transitions should be written through one batched gateway call"
        );
        assert!(
            !apply_source.contains("gateway.set_brightness(&monitor.identifier, *point)"),
            "smooth transitions should not re-enter DDC monitor enumeration for every point"
        );
    }

    #[test]
    fn sensor_reading_rejects_wrong_sensor_name() {
        let error = parse_sensor_reading_line(
            r#"{"sensor":"other","lux":34.17,"addr":"0x23","sda":4,"scl":5}"#,
        )
        .expect_err("wrong sensor should be rejected");
        assert!(error.to_string().contains("unexpected sensor"));
    }

    #[test]
    fn default_config_matches_original_requirements() {
        let config = AppConfig::default();
        assert_eq!(config.location_name, "Shanghai");
        assert_eq!(config.latitude, 31.2304);
        assert_eq!(config.longitude, 121.4737);
        assert_eq!(config.brightness_min, 0);
        assert_eq!(config.brightness_max, 100);
        assert_eq!(config.weather_refresh_seconds, 300);
        assert_eq!(config.control_tick_seconds, 30);
        assert_eq!(config.brightness_deadband, 2);
        assert_eq!(config.maximum_step_per_tick, 4);
        assert_eq!(config.sensor_port, "COM3");
    }

    #[test]
    fn monitor_brightness_normalization_uses_full_ddc_percent_range() {
        assert_eq!(monitor_brightness_range(), 0..=100);
        assert_eq!(normalize_monitor_brightness(-1), 0);
        assert_eq!(normalize_monitor_brightness(0), 0);
        assert_eq!(normalize_monitor_brightness(100), 100);
        assert_eq!(normalize_monitor_brightness(101), 100);
        assert_eq!(normalize_monitor_brightness_bounds(-20, 140), (0, 100));
        assert_eq!(normalize_monitor_brightness_bounds(95, 5), (5, 95));
    }

    #[test]
    fn policy_from_config_normalizes_user_edited_values() {
        let config = AppConfig {
            brightness_min: 120,
            brightness_max: -10,
            brightness_deadband: -3,
            maximum_step_per_tick: 0,
            ..AppConfig::default()
        };
        let policy = policy_from_config(&config);
        assert_eq!(policy.brightness_min, 0);
        assert_eq!(policy.brightness_max, 100);
        assert_eq!(policy.deadband, 0);
        assert_eq!(policy.maximum_step, 1);
        assert_eq!(
            compute_target_brightness(
                BrightnessInputs {
                    daylight_factor: 0.5,
                    cloud_cover: 0,
                    visibility_km: 20.0,
                    precipitation_probability: 0.0,
                },
                policy,
            ),
            50
        );
    }

    #[test]
    fn runtime_config_normalization_handles_manual_edits() {
        let mut runtime = RuntimeConfig {
            daytime_peak_brightness: 160,
            night_target_brightness: -25,
            theme_mode: "sepia".to_string(),
            ..RuntimeConfig::default()
        };
        normalize_runtime_config(&mut runtime);
        assert_eq!(runtime.daytime_peak_brightness, 100);
        assert_eq!(runtime.night_target_brightness, 0);
        assert_eq!(runtime.theme_mode, "dark");
    }

    #[test]
    fn runtime_config_normalization_accepts_system_theme() {
        let mut runtime = RuntimeConfig {
            theme_mode: "system".to_string(),
            ..RuntimeConfig::default()
        };
        normalize_runtime_config(&mut runtime);
        assert_eq!(runtime.theme_mode, "system");
    }

    #[test]
    fn config_loaders_accept_utf8_bom() {
        let path = PathBuf::from("target/test-config-bom.json");
        fs::create_dir_all(path.parent().expect("target path should have parent")).unwrap();
        fs::write(
            &path,
            "\u{feff}{\"brightness_min\":5,\"brightness_max\":80,\"daytime_peak_brightness\":75,\"theme_mode\":\"light\"}",
        )
        .unwrap();

        let config = load_config(Some(&path)).expect("app config should load with BOM");
        let runtime = load_runtime_config(&path).expect("runtime config should load with BOM");

        assert_eq!(config.brightness_min, 5);
        assert_eq!(config.brightness_max, 80);
        assert_eq!(runtime.daytime_peak_brightness, 75);
        assert_eq!(runtime.theme_mode, "light");

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_peak_and_target_can_reach_full_range() {
        let config = AppConfig::default();
        let policy = policy_from_config(&config);
        let runtime = RuntimeConfig {
            daytime_peak_brightness: 100,
            night_target_brightness: 0,
            ..RuntimeConfig::default()
        };
        let day_target = adapt_brightness_target(100, 1.0, &runtime);
        let night_target = adapt_brightness_target(0, 0.0, &runtime);
        assert_eq!(smooth_brightness_step(100, day_target, policy), 100);
        assert_eq!(smooth_brightness_step(0, night_target, policy), 0);
    }

    #[test]
    fn weather_payload_normalizes_defaults() {
        let snapshot = normalize_weather_payload(OpenMeteoPayload { current: None });
        assert_eq!(snapshot.cloud_cover, 0);
        assert_eq!(snapshot.visibility_km, 20.0);
        assert_eq!(snapshot.precipitation_probability, 0.0);
    }

    #[test]
    fn overcast_midday_is_dimmer_than_clear_midday() {
        let policy = BrightnessPolicy {
            brightness_min: 10,
            brightness_max: 90,
            deadband: 2,
            maximum_step: 4,
        };
        let solar = SolarSnapshot {
            elevation_degrees: 45.0,
            daylight_factor: 0.85,
            is_daylight: true,
        };
        let clear = compute_environment_brightness(
            policy,
            solar,
            Some(WeatherSnapshot {
                cloud_cover: 0,
                visibility_km: 18.0,
                precipitation_probability: 0.0,
            }),
        );
        let overcast = compute_environment_brightness(
            policy,
            solar,
            Some(WeatherSnapshot {
                cloud_cover: 100,
                visibility_km: 8.0,
                precipitation_probability: 0.8,
            }),
        );
        assert!(clear > overcast);
    }

    #[test]
    fn smootherstep_eases_between_endpoints() {
        assert_eq!(smootherstep(0.0), 0.0);
        assert_eq!(smootherstep(1.0), 1.0);
        assert!((smootherstep(0.5) - 0.5).abs() < 0.000_001);
        assert!(smootherstep(0.1) < 0.1);
        assert!(smootherstep(0.9) > 0.9);
    }

    #[test]
    fn brightness_transition_reaches_target_without_tick_step_limit() {
        let policy = BrightnessPolicy {
            brightness_min: 10,
            brightness_max: 90,
            deadband: 2,
            maximum_step: 4,
        };
        assert_eq!(smooth_brightness_step(40, 41, policy), 40);
        assert_eq!(smooth_brightness_step(40, 60, policy), 60);
        assert_eq!(smooth_brightness_step(40, 20, policy), 20);
        assert_eq!(smooth_brightness_step(0, 10, policy), 10);
        assert_eq!(smooth_brightness_step(95, 90, policy), 90);

        let points = brightness_transition_points(10, 90, policy);
        assert!(points.len() > 8, "{points:?}");
        assert_eq!(points.last(), Some(&90));
        assert!(points.iter().any(|point| *point > 14));
        assert!(points.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn brightness_transition_honors_deadband_and_bounds() {
        let policy = BrightnessPolicy {
            brightness_min: 10,
            brightness_max: 90,
            deadband: 2,
            maximum_step: 4,
        };
        assert!(brightness_transition_points(40, 41, policy).is_empty());
        assert_eq!(
            brightness_transition_points(95, 0, policy).last(),
            Some(&10)
        );
    }

    #[test]
    fn solar_midday_and_predawn_match_expected_shape() {
        let config = AppConfig::default();
        let midday = compute_solar_snapshot(
            &config,
            LocalDateTime {
                year: 2026,
                month: 6,
                day: 23,
                hour: 12,
                minute: 0,
                second: 0,
                utc_offset_minutes: 8 * 60,
            },
        );
        let predawn = compute_solar_snapshot(
            &config,
            LocalDateTime {
                year: 2026,
                month: 6,
                day: 23,
                hour: 4,
                minute: 0,
                second: 0,
                utc_offset_minutes: 8 * 60,
            },
        );
        assert!(midday.daylight_factor > 0.7, "{midday:?}");
        assert!(midday.is_daylight);
        assert!(predawn.daylight_factor < 0.1, "{predawn:?}");
        assert!(!predawn.is_daylight);
    }
}
