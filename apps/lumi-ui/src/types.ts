export type HealthLevel = "healthy" | "degraded" | "fault" | "starting";
export type DeviceConnectionState =
  | "discovering"
  | "connected"
  | "backing_off"
  | "disconnected"
  | "fault";
export type Capability = "ambient_lux" | "relay";
export type ThemeMode = "light" | "dark" | "system";
export type RelayContactMode = "no" | "nc";
export type LightAction = "keep" | "on" | "off";
export type WeatherKind = "clear" | "cloudy" | "rain" | "fog";

export interface DeviceSnapshot {
  state: DeviceConnectionState;
  product_id: string | null;
  serial_number: string | null;
  hardware_version: string | null;
  firmware_version: string | null;
  bootloader_version: string | null;
  protocol_min: number | null;
  protocol_max: number | null;
  negotiated_protocol: number | null;
  port_name: string | null;
  capabilities: Capability[];
  reconnect_count: number;
  last_error: string | null;
}

export interface SensorSnapshot {
  raw_lux: number | null;
  filtered_lux: number | null;
  sample_age_ms: number | null;
  valid: boolean;
  sequence_gaps: number;
  malformed_frames: number;
}

export interface MonitorSnapshot {
  id: string;
  display_name: string;
  display_path: string;
  qualified: boolean;
  current_percent: number | null;
  target_percent: number | null;
  transition_active: boolean;
  manual_override_remaining_ms: number | null;
  ddc_error_count: number;
  last_error: string | null;
}

export interface RelaySnapshot {
  available: boolean;
  light_on: boolean | null;
  energized: boolean | null;
  rules_enabled: boolean;
  matched_rule_id: string | null;
  matched_rule_name: string | null;
  last_error: string | null;
}

export interface EnvironmentSnapshot {
  configured: boolean;
  now_minutes: number;
  sunrise_minutes: number | null;
  sunset_minutes: number | null;
  timezone: string | null;
  weather: WeatherKind | null;
  weather_observed_at_unix_ms: number | null;
  last_error: string | null;
}

export interface ResourceSnapshot {
  process_id: number;
  uptime_seconds: number;
  cpu_usage_basis_points: number | null;
  cpu_time_ms: number | null;
  thread_count: number | null;
  handle_count: number | null;
  working_set_bytes: number | null;
}

export interface AgentSnapshot {
  api_version: number;
  revision: number;
  generated_at_unix_ms: number;
  health: HealthLevel;
  status_message: string;
  configuration_warning: string | null;
  paused: boolean;
  target_percent: number | null;
  device: DeviceSnapshot;
  sensor: SensorSnapshot;
  monitors: MonitorSnapshot[];
  relay: RelaySnapshot;
  environment: EnvironmentSnapshot;
  resources: ResourceSnapshot;
}

export interface UpdateChannelStatus {
  configured: boolean;
  currentVersion: string;
}

export interface UpdateMetadata {
  version: string;
  currentVersion: string;
  notes: string | null;
}

export interface SensorCurvePoint {
  lux: number;
  brightness: number;
}

export interface FilterSettings {
  median_window: number;
  rise_alpha: number;
  fall_alpha: number;
}

export interface TransitionSettings {
  duration_ms: number;
  max_writes_per_second: number;
}

export interface ManualOverrideSettings {
  detection_threshold: number;
  grace_period_ms: number;
}

export type LightCondition =
  | { kind: "time_after"; minutes: number }
  | { kind: "time_before"; minutes: number }
  | { kind: "after_sunrise"; offset_minutes: number }
  | { kind: "before_sunset"; offset_minutes: number }
  | { kind: "after_sunset"; offset_minutes: number }
  | { kind: "lux_below"; lux: number }
  | { kind: "lux_above"; lux: number }
  | { kind: "current_brightness_below"; brightness: number }
  | { kind: "current_brightness_above"; brightness: number }
  | { kind: "target_brightness_below"; brightness: number }
  | { kind: "target_brightness_above"; brightness: number }
  | { kind: "weather_is"; weather: WeatherKind };

export type ConditionExpression =
  | { kind: "condition"; condition: LightCondition }
  | { kind: "and"; conditions: ConditionExpression[] }
  | { kind: "or"; conditions: ConditionExpression[] };

export interface LightRule {
  id: string;
  name: string;
  enabled: boolean;
  when: ConditionExpression;
  then: LightAction;
}

export interface MonitorProfile {
  display_name: string;
  enabled: boolean;
  calibration: {
    minimum_raw: number | null;
    maximum_raw: number | null;
    perceptual_points: SensorCurvePoint[];
  };
}

export interface SettingsDocument {
  schema_version: number;
  settings: {
    paused: boolean;
    start_at_login: boolean;
    onboarding_completed: boolean;
    locale: string;
    theme: ThemeMode;
    control: {
      sensor_curve: SensorCurvePoint[];
      filter: FilterSettings;
      target_deadband: number;
      transition: TransitionSettings;
      manual_override: ManualOverrideSettings;
      daytime_peak_brightness: number;
      night_target_brightness: number;
    };
    relay: {
      contact_mode: RelayContactMode;
      rules_enabled: boolean;
      rules: LightRule[];
      fallback_action: LightAction;
    };
    weather: {
      enabled: boolean;
      location_name: string;
      latitude: number;
      longitude: number;
      timezone: string;
      refresh_seconds: number;
    };
    monitors: Record<string, MonitorProfile>;
  };
  migration: {
    imported_from_v1: boolean;
    source_path: string | null;
    imported_at_unix_seconds: number | null;
    legacy_device_port: string | null;
    legacy_monitor_calibrations: unknown;
    warnings: string[];
  };
}

export type SaveStatus = "idle" | "saving" | "saved" | "error";
export type ViewId = "status" | "calibration" | "rules" | "hardware" | "settings" | "support";
