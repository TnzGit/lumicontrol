use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fmt;

pub const MIN_BRIGHTNESS: i32 = 0;
pub const MAX_BRIGHTNESS: i32 = 100;
pub const MIN_ENVIRONMENT_BRIGHTNESS_OFFSET: i32 = -50;
pub const MAX_ENVIRONMENT_BRIGHTNESS_OFFSET: i32 = 50;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrightnessSource {
    #[default]
    Sensor,
    Environment,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SensorCurvePoint {
    pub lux: f64,
    pub brightness: i32,
}

pub fn default_sensor_curve() -> Vec<SensorCurvePoint> {
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

pub fn normalize_sensor_curve(points: &[SensorCurvePoint]) -> Vec<SensorCurvePoint> {
    let mut normalized = points
        .iter()
        .filter(|point| point.lux.is_finite() && point.lux > 0.0)
        .map(|point| SensorCurvePoint {
            lux: point.lux.max(0.001),
            brightness: normalize_brightness(point.brightness),
        })
        .collect::<Vec<_>>();
    normalized.sort_by(|left, right| left.lux.partial_cmp(&right.lux).unwrap_or(Ordering::Equal));
    normalized.dedup_by(|left, right| {
        if (left.lux - right.lux).abs() < f64::EPSILON {
            left.brightness = right.brightness;
            true
        } else {
            false
        }
    });
    if normalized.is_empty() {
        default_sensor_curve()
    } else {
        normalized
    }
}

pub fn normalize_brightness(value: i32) -> i32 {
    value.clamp(MIN_BRIGHTNESS, MAX_BRIGHTNESS)
}

pub fn map_lux_to_brightness(lux: f64, points: &[SensorCurvePoint]) -> i32 {
    let curve = normalize_sensor_curve(points);
    map_normalized_lux_to_brightness(lux, &curve)
}

pub fn map_normalized_lux_to_brightness(lux: f64, curve: &[SensorCurvePoint]) -> i32 {
    if curve.is_empty() {
        return map_lux_to_brightness(lux, &default_sensor_curve());
    }
    if !lux.is_finite() || lux <= curve[0].lux {
        return curve[0].brightness;
    }
    let last = curve.last().expect("normalized curve is never empty");
    if lux >= last.lux {
        return last.brightness;
    }
    for pair in curve.windows(2) {
        let left = &pair[0];
        let right = &pair[1];
        if lux <= right.lux {
            let left_log = left.lux.ln();
            let right_log = right.lux.ln();
            let position = if (right_log - left_log).abs() < f64::EPSILON {
                0.0
            } else {
                (lux.max(0.001).ln() - left_log) / (right_log - left_log)
            };
            let value = left.brightness as f64
                + (right.brightness - left.brightness) as f64 * position.clamp(0.0, 1.0);
            return normalize_brightness(value.round() as i32);
        }
    }
    last.brightness
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentWeatherInput {
    pub kind: WeatherKind,
    pub cloud_cover_percent: i32,
    pub visibility_km: f64,
    pub precipitation_probability: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentBrightnessInput {
    pub solar_elevation_degrees: f64,
    pub day_of_year: u32,
    pub latitude: f64,
    pub sunrise_minutes: Option<i32>,
    pub sunset_minutes: Option<i32>,
    pub weather: Option<EnvironmentWeatherInput>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentBrightnessRecommendation {
    pub base_percent: i32,
    pub adjusted_percent: i32,
    pub daylight_factor: f64,
    pub season_factor: f64,
    pub weather_factor: f64,
}

pub fn recommend_environment_brightness(
    input: EnvironmentBrightnessInput,
    night_target_percent: i32,
    daytime_peak_percent: i32,
    offset_percent: i32,
) -> EnvironmentBrightnessRecommendation {
    let night = normalize_brightness(night_target_percent);
    let day = normalize_brightness(daytime_peak_percent).max(night);
    let elevation = if input.solar_elevation_degrees.is_finite() {
        input.solar_elevation_degrees
    } else {
        -90.0
    };

    // Civil twilight starts at -6 degrees. Smootherstep keeps both velocity and
    // acceleration continuous as the recommendation moves into daytime.
    let daylight_factor = smootherstep((elevation + 6.0) / 51.0);
    let season_factor = environment_season_factor(input);
    let weather_factor = input.weather.map(environment_weather_factor).unwrap_or(1.0);
    let effective_daylight = (daylight_factor * season_factor * weather_factor).clamp(0.0, 1.0);
    let base_percent = normalize_brightness(
        (night as f64 + f64::from(day - night) * effective_daylight).round() as i32,
    );
    let offset = offset_percent.clamp(
        MIN_ENVIRONMENT_BRIGHTNESS_OFFSET,
        MAX_ENVIRONMENT_BRIGHTNESS_OFFSET,
    );

    EnvironmentBrightnessRecommendation {
        base_percent,
        adjusted_percent: normalize_brightness(base_percent + offset),
        daylight_factor,
        season_factor,
        weather_factor,
    }
}

fn environment_season_factor(input: EnvironmentBrightnessInput) -> f64 {
    if let (Some(sunrise), Some(sunset)) = (input.sunrise_minutes, input.sunset_minutes) {
        let daylight_minutes = (sunset - sunrise).rem_euclid(24 * 60);
        if daylight_minutes > 0 {
            let centered =
                (f64::from(daylight_minutes.clamp(6 * 60, 18 * 60)) - 12.0 * 60.0) / (6.0 * 60.0);
            return (1.0 + 0.08 * centered).clamp(0.92, 1.08);
        }
    }

    let day = input.day_of_year.clamp(1, 366) as f64;
    let hemisphere = if input.latitude < 0.0 { -1.0 } else { 1.0 };
    let latitude_strength = (input.latitude.abs() / 66.5).clamp(0.0, 1.0);
    let seasonal_alignment = (std::f64::consts::TAU * (day - 172.0) / 365.2425).cos() * hemisphere;
    (1.0 + 0.08 * latitude_strength * seasonal_alignment).clamp(0.92, 1.08)
}

fn environment_weather_factor(weather: EnvironmentWeatherInput) -> f64 {
    let cloud_cover = f64::from(weather.cloud_cover_percent.clamp(0, 100)) / 100.0;
    let precipitation = if weather.precipitation_probability.is_finite() {
        weather.precipitation_probability.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let visibility = if weather.visibility_km.is_finite() {
        weather.visibility_km.max(0.0)
    } else {
        20.0
    };
    let kind_factor = match weather.kind {
        WeatherKind::Clear => 1.03,
        WeatherKind::Cloudy => 0.95,
        WeatherKind::Rain => 0.84,
        WeatherKind::Fog => 0.80,
    };
    let cloud_factor = 1.0 - 0.32 * cloud_cover.powf(1.25);
    let precipitation_factor = 1.0 - 0.14 * precipitation;
    let visibility_factor = if visibility >= 10.0 {
        1.0
    } else {
        0.75 + 0.25 * (visibility / 10.0)
    };
    (kind_factor * cloud_factor * precipitation_factor * visibility_factor).clamp(0.45, 1.05)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LogLuxFilterConfig {
    pub median_window: usize,
    pub rise_alpha: f64,
    pub fall_alpha: f64,
}

impl Default for LogLuxFilterConfig {
    fn default() -> Self {
        Self {
            median_window: 3,
            rise_alpha: 0.35,
            fall_alpha: 0.22,
        }
    }
}

impl LogLuxFilterConfig {
    pub fn validate(self) -> Result<Self, CoreError> {
        if self.median_window == 0 || self.median_window > 31 {
            return Err(CoreError::InvalidConfiguration(
                "median_window must be in 1..=31".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.rise_alpha) || self.rise_alpha == 0.0 {
            return Err(CoreError::InvalidConfiguration(
                "rise_alpha must be in (0, 1]".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.fall_alpha) || self.fall_alpha == 0.0 {
            return Err(CoreError::InvalidConfiguration(
                "fall_alpha must be in (0, 1]".to_string(),
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct LogLuxFilter {
    config: LogLuxFilterConfig,
    window: VecDeque<f64>,
    scratch: Vec<f64>,
    filtered_log_lux: Option<f64>,
}

impl LogLuxFilter {
    pub fn new(config: LogLuxFilterConfig) -> Result<Self, CoreError> {
        Ok(Self {
            config: config.validate()?,
            window: VecDeque::with_capacity(config.median_window),
            scratch: Vec::with_capacity(config.median_window),
            filtered_log_lux: None,
        })
    }

    pub fn push(&mut self, lux: f64) -> Result<f64, CoreError> {
        if !lux.is_finite() || lux < 0.0 {
            return Err(CoreError::InvalidSample(lux));
        }
        let log_lux = lux.max(0.001).ln();
        self.window.push_back(log_lux);
        while self.window.len() > self.config.median_window {
            self.window.pop_front();
        }
        self.scratch.clear();
        self.scratch.extend(self.window.iter().copied());
        self.scratch
            .sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
        let median = if self.scratch.len() % 2 == 1 {
            self.scratch[self.scratch.len() / 2]
        } else {
            let right = self.scratch.len() / 2;
            (self.scratch[right - 1] + self.scratch[right]) / 2.0
        };
        let filtered = match self.filtered_log_lux {
            None => median,
            Some(previous) => {
                let alpha = if median >= previous {
                    self.config.rise_alpha
                } else {
                    self.config.fall_alpha
                };
                previous + alpha * (median - previous)
            }
        };
        self.filtered_log_lux = Some(filtered);
        Ok(filtered.exp().max(0.0))
    }

    pub fn current(&self) -> Option<f64> {
        self.filtered_log_lux.map(f64::exp)
    }

    pub fn reset(&mut self) {
        self.window.clear();
        self.scratch.clear();
        self.filtered_log_lux = None;
    }
}

#[derive(Clone, Debug)]
pub struct TargetStabilizer {
    deadband: i32,
    target: Option<i32>,
}

impl TargetStabilizer {
    pub fn new(deadband: i32) -> Self {
        Self {
            deadband: deadband.max(0),
            target: None,
        }
    }

    pub fn update(&mut self, candidate: i32) -> i32 {
        let candidate = normalize_brightness(candidate);
        match self.target {
            Some(current) if (candidate - current).abs() <= self.deadband => current,
            _ => {
                self.target = Some(candidate);
                candidate
            }
        }
    }

    pub fn reset(&mut self) {
        self.target = None;
    }
}

pub fn smootherstep(position: f64) -> f64 {
    let t = position.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransitionSpec {
    pub duration_ms: u64,
    pub max_writes_per_second: u16,
}

impl Default for TransitionSpec {
    fn default() -> Self {
        Self {
            duration_ms: 1500,
            max_writes_per_second: 10,
        }
    }
}

impl TransitionSpec {
    pub fn validate(self) -> Result<Self, CoreError> {
        if !(100..=30_000).contains(&self.duration_ms) {
            return Err(CoreError::InvalidConfiguration(
                "transition duration_ms must be in 100..=30000".to_string(),
            ));
        }
        if !(1..=20).contains(&self.max_writes_per_second) {
            return Err(CoreError::InvalidConfiguration(
                "max_writes_per_second must be in 1..=20".to_string(),
            ));
        }
        Ok(self)
    }

    pub fn minimum_write_interval_ms(self) -> u64 {
        (1000 / self.max_writes_per_second.max(1) as u64).max(1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionPlan {
    from: i32,
    to: i32,
    started_at_ms: u64,
    spec: TransitionSpec,
}

impl TransitionPlan {
    pub fn new(
        from: i32,
        to: i32,
        started_at_ms: u64,
        spec: TransitionSpec,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            from: normalize_brightness(from),
            to: normalize_brightness(to),
            started_at_ms,
            spec: spec.validate()?,
        })
    }

    pub fn value_at(self, now_ms: u64) -> i32 {
        if self.from == self.to {
            return self.to;
        }
        let elapsed = now_ms.saturating_sub(self.started_at_ms);
        let position = elapsed as f64 / self.spec.duration_ms as f64;
        let eased = smootherstep(position);
        normalize_brightness(
            (self.from as f64 + (self.to - self.from) as f64 * eased).round() as i32,
        )
    }

    pub fn is_complete(self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.started_at_ms) >= self.spec.duration_ms
    }

    pub fn retarget(self, now_ms: u64, new_target: i32) -> Result<Self, CoreError> {
        Self::new(self.value_at(now_ms), new_target, now_ms, self.spec)
    }

    pub fn target(self) -> i32 {
        self.to
    }

    pub fn minimum_write_interval_ms(self) -> u64 {
        self.spec.minimum_write_interval_ms()
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelayContactMode {
    #[default]
    No,
    Nc,
}

impl RelayContactMode {
    pub fn energized_for_light(self, light_on: bool) -> bool {
        match self {
            RelayContactMode::No => light_on,
            RelayContactMode::Nc => !light_on,
        }
    }

    pub fn light_on(self, energized: bool) -> bool {
        match self {
            RelayContactMode::No => energized,
            RelayContactMode::Nc => !energized,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LightAction {
    #[default]
    Keep,
    On,
    Off,
}

impl LightAction {
    pub fn light_on(self) -> Option<bool> {
        match self {
            LightAction::Keep => None,
            LightAction::On => Some(true),
            LightAction::Off => Some(false),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WeatherKind {
    Clear,
    Cloudy,
    Rain,
    Fog,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LightCondition {
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
    WeatherIs { weather: WeatherKind },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionExpression {
    Condition {
        condition: LightCondition,
    },
    And {
        conditions: Vec<ConditionExpression>,
    },
    Or {
        conditions: Vec<ConditionExpression>,
    },
}

impl ConditionExpression {
    pub fn condition(condition: LightCondition) -> Self {
        Self::Condition { condition }
    }

    pub fn matches(&self, context: &RuleContext) -> bool {
        match self {
            ConditionExpression::Condition { condition } => condition.matches(context),
            ConditionExpression::And { conditions } => {
                !conditions.is_empty()
                    && conditions
                        .iter()
                        .all(|condition| condition.matches(context))
            }
            ConditionExpression::Or { conditions } => conditions
                .iter()
                .any(|condition| condition.matches(context)),
        }
    }

    pub fn condition_count(&self) -> usize {
        match self {
            ConditionExpression::Condition { .. } => 1,
            ConditionExpression::And { conditions } | ConditionExpression::Or { conditions } => {
                conditions
                    .iter()
                    .map(ConditionExpression::condition_count)
                    .sum()
            }
        }
    }
}

impl LightCondition {
    pub fn matches(&self, context: &RuleContext) -> bool {
        match self {
            LightCondition::TimeAfter { minutes } => {
                context.now_minutes >= normalize_minutes(*minutes)
            }
            LightCondition::TimeBefore { minutes } => {
                context.now_minutes <= normalize_minutes(*minutes)
            }
            LightCondition::AfterSunrise { offset_minutes } => {
                context.sunrise_minutes.is_some_and(|sunrise| {
                    context.now_minutes >= normalize_minutes(sunrise + offset_minutes)
                })
            }
            LightCondition::BeforeSunset { offset_minutes } => {
                context.sunset_minutes.is_some_and(|sunset| {
                    context.now_minutes <= normalize_minutes(sunset + offset_minutes)
                })
            }
            LightCondition::AfterSunset { offset_minutes } => {
                context.sunset_minutes.is_some_and(|sunset| {
                    context.now_minutes >= normalize_minutes(sunset + offset_minutes)
                })
            }
            LightCondition::LuxBelow { lux } => context.lux.is_some_and(|value| value < *lux),
            LightCondition::LuxAbove { lux } => context.lux.is_some_and(|value| value > *lux),
            LightCondition::CurrentBrightnessBelow { brightness } => context
                .current_brightness
                .is_some_and(|value| value < *brightness),
            LightCondition::CurrentBrightnessAbove { brightness } => context
                .current_brightness
                .is_some_and(|value| value > *brightness),
            LightCondition::TargetBrightnessBelow { brightness } => context
                .target_brightness
                .is_some_and(|value| value < *brightness),
            LightCondition::TargetBrightnessAbove { brightness } => context
                .target_brightness
                .is_some_and(|value| value > *brightness),
            LightCondition::WeatherIs { weather } => context.weather == Some(*weather),
        }
    }
}

fn normalize_minutes(minutes: i32) -> i32 {
    minutes.rem_euclid(24 * 60)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RuleContext {
    pub now_minutes: i32,
    pub sunrise_minutes: Option<i32>,
    pub sunset_minutes: Option<i32>,
    pub weather: Option<WeatherKind>,
    pub lux: Option<f64>,
    pub current_brightness: Option<i32>,
    pub target_brightness: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LightRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub when: ConditionExpression,
    pub then: LightAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleDecision {
    pub action: LightAction,
    pub matched_rule_id: Option<String>,
    pub matched_rule_name: Option<String>,
}

pub fn evaluate_rules(
    rules: &[LightRule],
    fallback: LightAction,
    context: &RuleContext,
) -> RuleDecision {
    for rule in rules {
        if rule.enabled && rule.when.matches(context) {
            return RuleDecision {
                action: rule.then,
                matched_rule_id: Some(rule.id.clone()),
                matched_rule_name: Some(rule.name.clone()),
            };
        }
    }
    RuleDecision {
        action: fallback,
        matched_rule_id: None,
        matched_rule_name: None,
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManualOverrideConfig {
    pub detection_threshold: i32,
    pub grace_period_ms: u64,
}

impl Default for ManualOverrideConfig {
    fn default() -> Self {
        Self {
            detection_threshold: 4,
            grace_period_ms: 15 * 60 * 1000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ManualOverrideGuard {
    config: ManualOverrideConfig,
    suppressed_until_ms: Option<u64>,
}

impl ManualOverrideGuard {
    pub fn new(config: ManualOverrideConfig) -> Self {
        Self {
            config,
            suppressed_until_ms: None,
        }
    }

    pub fn observe(
        &mut self,
        now_ms: u64,
        expected_brightness: i32,
        observed_brightness: i32,
        transition_active: bool,
    ) -> bool {
        if !transition_active
            && (expected_brightness - observed_brightness).abs()
                >= self.config.detection_threshold.max(1)
        {
            self.suppressed_until_ms = Some(now_ms.saturating_add(self.config.grace_period_ms));
            true
        } else {
            false
        }
    }

    pub fn is_suppressed(&self, now_ms: u64) -> bool {
        self.suppressed_until_ms
            .is_some_and(|deadline| now_ms < deadline)
    }

    pub fn remaining_ms(&self, now_ms: u64) -> Option<u64> {
        self.suppressed_until_ms
            .and_then(|deadline| deadline.checked_sub(now_ms))
            .filter(|remaining| *remaining > 0)
    }

    pub fn clear(&mut self) {
        self.suppressed_until_ms = None;
    }
}

#[derive(Debug, PartialEq)]
pub enum CoreError {
    InvalidConfiguration(String),
    InvalidSample(f64),
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreError::InvalidConfiguration(message) => write!(formatter, "{message}"),
            CoreError::InvalidSample(value) => write!(formatter, "invalid lux sample: {value}"),
        }
    }
}

impl std::error::Error for CoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment_input(
        elevation: f64,
        sunrise_minutes: i32,
        sunset_minutes: i32,
        weather: Option<EnvironmentWeatherInput>,
    ) -> EnvironmentBrightnessInput {
        EnvironmentBrightnessInput {
            solar_elevation_degrees: elevation,
            day_of_year: 172,
            latitude: 31.2304,
            sunrise_minutes: Some(sunrise_minutes),
            sunset_minutes: Some(sunset_minutes),
            weather,
        }
    }

    #[test]
    fn environment_brightness_keeps_night_stable_and_applies_offset() {
        let recommendation =
            recommend_environment_brightness(environment_input(-12.0, 330, 1_170, None), 18, 90, 7);
        assert_eq!(recommendation.base_percent, 18);
        assert_eq!(recommendation.adjusted_percent, 25);
        assert_eq!(recommendation.daylight_factor, 0.0);
    }

    #[test]
    fn environment_brightness_responds_to_sun_weather_and_season() {
        let clear = EnvironmentWeatherInput {
            kind: WeatherKind::Clear,
            cloud_cover_percent: 10,
            visibility_km: 20.0,
            precipitation_probability: 0.0,
        };
        let cloudy = EnvironmentWeatherInput {
            kind: WeatherKind::Cloudy,
            cloud_cover_percent: 80,
            visibility_km: 12.0,
            precipitation_probability: 0.1,
        };
        let rain = EnvironmentWeatherInput {
            kind: WeatherKind::Rain,
            cloud_cover_percent: 95,
            visibility_km: 5.0,
            precipitation_probability: 0.8,
        };
        let clear_target = recommend_environment_brightness(
            environment_input(35.0, 300, 1_260, Some(clear)),
            18,
            90,
            0,
        );
        let cloudy_target = recommend_environment_brightness(
            environment_input(35.0, 300, 1_260, Some(cloudy)),
            18,
            90,
            0,
        );
        let rain_target = recommend_environment_brightness(
            environment_input(35.0, 300, 1_260, Some(rain)),
            18,
            90,
            0,
        );
        assert!(clear_target.base_percent > cloudy_target.base_percent);
        assert!(cloudy_target.base_percent > rain_target.base_percent);

        let winter = recommend_environment_brightness(
            environment_input(20.0, 480, 960, Some(clear)),
            18,
            90,
            0,
        );
        let summer = recommend_environment_brightness(
            environment_input(20.0, 240, 1_200, Some(clear)),
            18,
            90,
            0,
        );
        assert!(summer.season_factor > winter.season_factor);
        assert!(summer.base_percent > winter.base_percent);
    }

    #[test]
    fn environment_brightness_is_monotonic_and_bounded() {
        let low = recommend_environment_brightness(
            environment_input(2.0, 360, 1_080, None),
            12,
            92,
            -100,
        );
        let high = recommend_environment_brightness(
            environment_input(40.0, 360, 1_080, None),
            12,
            92,
            100,
        );
        assert!(high.base_percent > low.base_percent);
        assert_eq!(low.adjusted_percent, 0);
        assert_eq!(high.adjusted_percent, 100);
    }

    #[test]
    fn curve_interpolates_in_log_lux_space() {
        let curve = vec![
            SensorCurvePoint {
                lux: 10.0,
                brightness: 20,
            },
            SensorCurvePoint {
                lux: 1000.0,
                brightness: 80,
            },
        ];
        assert_eq!(map_lux_to_brightness(100.0, &curve), 50);
        assert_eq!(map_normalized_lux_to_brightness(100.0, &curve), 50);
        assert_eq!(
            map_normalized_lux_to_brightness(100.0, &[]),
            map_lux_to_brightness(100.0, &default_sensor_curve())
        );
    }

    #[test]
    fn median_window_rejects_a_single_large_outlier() {
        let mut filter = LogLuxFilter::new(LogLuxFilterConfig {
            median_window: 3,
            rise_alpha: 1.0,
            fall_alpha: 1.0,
        })
        .unwrap();
        filter.push(10.0).unwrap();
        filter.push(1000.0).unwrap();
        let filtered = filter.push(11.0).unwrap();
        assert!((filtered - 11.0).abs() < 0.01);
    }

    #[test]
    fn target_stabilizer_holds_values_inside_deadband() {
        let mut stabilizer = TargetStabilizer::new(2);
        assert_eq!(stabilizer.update(60), 60);
        assert_eq!(stabilizer.update(62), 60);
        assert_eq!(stabilizer.update(63), 63);
    }

    #[test]
    fn transition_is_eased_retargetable_and_reaches_endpoint() {
        let plan = TransitionPlan::new(0, 100, 1_000, TransitionSpec::default()).unwrap();
        assert_eq!(plan.value_at(1_000), 0);
        assert_eq!(plan.value_at(2_500), 100);
        let midpoint = plan.value_at(1_750);
        assert_eq!(midpoint, 50);
        let retargeted = plan.retarget(1_750, 20).unwrap();
        assert_eq!(retargeted.value_at(1_750), midpoint);
        assert_eq!(retargeted.value_at(3_250), 20);
    }

    #[test]
    fn nested_and_or_rules_use_first_matching_priority() {
        let rules = vec![
            LightRule {
                id: "evening-dark".to_string(),
                name: "Evening and dark".to_string(),
                enabled: true,
                when: ConditionExpression::And {
                    conditions: vec![
                        ConditionExpression::condition(LightCondition::AfterSunset {
                            offset_minutes: 0,
                        }),
                        ConditionExpression::Or {
                            conditions: vec![
                                ConditionExpression::condition(LightCondition::LuxBelow {
                                    lux: 30.0,
                                }),
                                ConditionExpression::condition(LightCondition::WeatherIs {
                                    weather: WeatherKind::Rain,
                                }),
                            ],
                        },
                    ],
                },
                then: LightAction::On,
            },
            LightRule {
                id: "late".to_string(),
                name: "Late".to_string(),
                enabled: true,
                when: ConditionExpression::condition(LightCondition::TimeAfter { minutes: 60 }),
                then: LightAction::Off,
            },
        ];
        let context = RuleContext {
            now_minutes: 20 * 60,
            sunset_minutes: Some(18 * 60),
            lux: Some(12.0),
            ..RuleContext::default()
        };
        let decision = evaluate_rules(&rules, LightAction::Keep, &context);
        assert_eq!(decision.action, LightAction::On);
        assert_eq!(decision.matched_rule_id.as_deref(), Some("evening-dark"));
    }

    #[test]
    fn empty_groups_do_not_accidentally_match() {
        let context = RuleContext::default();
        assert!(!ConditionExpression::And { conditions: vec![] }.matches(&context));
        assert!(!ConditionExpression::Or { conditions: vec![] }.matches(&context));
    }

    #[test]
    fn relay_contact_mode_maps_logical_light_state() {
        assert!(RelayContactMode::No.energized_for_light(true));
        assert!(!RelayContactMode::Nc.energized_for_light(true));
        assert!(RelayContactMode::Nc.light_on(false));
    }

    #[test]
    fn manual_override_starts_and_expires_grace_period() {
        let mut guard = ManualOverrideGuard::new(ManualOverrideConfig {
            detection_threshold: 4,
            grace_period_ms: 1_000,
        });
        assert!(guard.observe(5_000, 70, 60, false));
        assert!(guard.is_suppressed(5_999));
        assert_eq!(guard.remaining_ms(1_000), Some(5_000));
        assert!(!guard.is_suppressed(6_000));
        assert_eq!(guard.remaining_ms(6_000), None);
    }
}
