import { invoke } from "@tauri-apps/api/core";
import type { AgentSnapshot, SettingsDocument, UpdateChannelStatus, UpdateMetadata } from "./types";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

const isTauri = typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;
const demoOnboarding = !isTauri && new URLSearchParams(window.location.search).has("onboarding");

let demoSettings: SettingsDocument = {
  schema_version: 2,
  settings: {
    paused: false,
    start_at_login: true,
    onboarding_completed: !demoOnboarding,
    locale: "system",
    theme: "system",
    control: {
      sensor_curve: [
        { lux: 1, brightness: 12 },
        { lux: 15, brightness: 28 },
        { lux: 60, brightness: 58 },
        { lux: 180, brightness: 78 },
        { lux: 600, brightness: 92 },
      ],
      filter: { median_window: 3, rise_alpha: 0.35, fall_alpha: 0.22 },
      target_deadband: 2,
      transition: { duration_ms: 1500, max_writes_per_second: 10 },
      manual_override: { detection_threshold: 4, grace_period_ms: 900000 },
      daytime_peak_brightness: 92,
      night_target_brightness: 12,
    },
    relay: {
      contact_mode: "no",
      rules_enabled: true,
      fallback_action: "keep",
      rules: [
        {
          id: "evening-light",
          name: "Evening light",
          enabled: true,
          when: {
            kind: "or",
            conditions: [
              { kind: "condition", condition: { kind: "after_sunset", offset_minutes: 0 } },
              { kind: "condition", condition: { kind: "lux_below", lux: 35 } },
            ],
          },
          then: "on",
        },
        {
          id: "late-off",
          name: "Late night off",
          enabled: true,
          when: {
            kind: "and",
            conditions: [{ kind: "condition", condition: { kind: "time_after", minutes: 90 } }],
          },
          then: "off",
        },
      ],
    },
    weather: {
      enabled: true,
      location_name: "Shanghai",
      latitude: 31.2304,
      longitude: 121.4737,
      timezone: "Asia/Shanghai",
      refresh_seconds: 300,
    },
    monitors: {},
  },
  migration: {
    imported_from_v1: false,
    source_path: null,
    imported_at_unix_seconds: null,
    legacy_device_port: null,
    legacy_monitor_calibrations: null,
    warnings: [],
  },
};

let demoRevision = 18;
const demoSnapshot = (): AgentSnapshot => ({
  api_version: 2,
  revision: demoRevision,
  generated_at_unix_ms: Date.now(),
  health: "healthy",
  status_message: "Automatic control is active",
  configuration_warning: null,
  paused: demoSettings.settings.paused,
  target_percent: 61,
  device: {
    state: "connected",
    product_id: "LC-SR1",
    serial_number: "DEV-DEMO00000001",
    hardware_version: "1.0",
    firmware_version: "2.0.0",
    bootloader_version: "1.0.0",
    protocol_min: 2,
    protocol_max: 2,
    negotiated_protocol: 2,
    port_name: "COM3",
    capabilities: ["ambient_lux", "relay"],
    reconnect_count: 0,
    last_error: null,
  },
  sensor: {
    raw_lux: 67.2,
    filtered_lux: 64.8,
    sample_age_ms: 740,
    valid: true,
    sequence_gaps: 0,
    malformed_frames: 0,
  },
  monitors: [
    {
      id: "monitor-0acc4c63c9ae27bd",
      display_name: "Dell U2723QE",
      display_path: "DISPLAY2",
      qualified: true,
      current_percent: 60,
      target_percent: 61,
      transition_active: false,
      manual_override_remaining_ms: null,
      ddc_error_count: 0,
      last_error: null,
    },
    {
      id: "monitor-0e181325d3849635",
      display_name: "Dell U2723QE",
      display_path: "DISPLAY1",
      qualified: true,
      current_percent: 61,
      target_percent: 61,
      transition_active: false,
      manual_override_remaining_ms: null,
      ddc_error_count: 0,
      last_error: null,
    },
  ],
  relay: {
    available: true,
    light_on: true,
    energized: true,
    rules_enabled: demoSettings.settings.relay.rules_enabled,
    matched_rule_id: "evening-light",
    matched_rule_name: "Evening light",
    last_error: null,
  },
  environment: {
    configured: true,
    now_minutes: new Date().getHours() * 60 + new Date().getMinutes(),
    sunrise_minutes: 314,
    sunset_minutes: 1158,
    timezone: "Asia/Shanghai",
    weather: "cloudy",
    weather_observed_at_unix_ms: Date.now() - 95_000,
    last_error: null,
  },
  resources: {
    process_id: 18420,
    uptime_seconds: 3864,
    cpu_usage_basis_points: 3,
    cpu_time_ms: 1280,
    thread_count: 13,
    handle_count: 132,
    working_set_bytes: 18_874_368,
  },
});

const delay = (ms: number) => new Promise<void>((resolve) => window.setTimeout(resolve, ms));

export async function getSnapshot(): Promise<AgentSnapshot> {
  return isTauri ? invoke<AgentSnapshot>("get_snapshot") : demoSnapshot();
}

export async function waitForSnapshot(afterRevision: number, timeoutMs = 25_000): Promise<AgentSnapshot> {
  if (isTauri) {
    return invoke<AgentSnapshot>("wait_for_snapshot", { afterRevision, timeoutMs });
  }
  await delay(Math.min(timeoutMs, 1_500));
  demoRevision += 1;
  return demoSnapshot();
}

export async function getSettings(): Promise<SettingsDocument> {
  return isTauri ? invoke<SettingsDocument>("get_settings") : structuredClone(demoSettings);
}

export async function saveSettings(document: SettingsDocument): Promise<void> {
  if (isTauri) {
    await invoke("save_settings", { document });
  } else {
    await delay(120);
    demoSettings = structuredClone(document);
    demoRevision += 1;
  }
}

export const setPaused = (paused: boolean) =>
  isTauri ? invoke<void>("set_paused", { paused }) : saveDemoPause(paused);

async function saveDemoPause(paused: boolean): Promise<void> {
  demoSettings.settings.paused = paused;
  demoRevision += 1;
}

export const runNow = () => (isTauri ? invoke<void>("run_now") : Promise.resolve());
export const refreshHardware = () =>
  isTauri ? invoke<void>("refresh_hardware") : Promise.resolve();
export const setLight = (lightOn: boolean) =>
  isTauri ? invoke<void>("set_light", { lightOn }) : setDemoLight(lightOn);

async function setDemoLight(lightOn: boolean): Promise<void> {
  demoRevision += 1;
  void lightOn;
}

export const clearManualOverride = (monitorId?: string) =>
  isTauri ? invoke<void>("clear_manual_override", { monitorId: monitorId ?? null }) : Promise.resolve();
export const exportDiagnostics = () =>
  isTauri
    ? invoke<string>("export_diagnostics")
    : Promise.resolve("C:\\Users\\Demo\\Desktop\\LumiControl-diagnostics.zip");

export const updateChannelStatus = () =>
  isTauri
    ? invoke<UpdateChannelStatus>("update_channel_status")
    : Promise.resolve({ configured: true, currentVersion: "0.2.0" });

export const checkForUpdate = () =>
  isTauri
    ? invoke<UpdateMetadata | null>("check_for_update")
    : delay(350).then(() => ({
      version: "0.2.1",
      currentVersion: "0.2.0",
      notes: "Reliability and device compatibility improvements.",
    }));

export const installUpdate = () =>
  isTauri ? invoke<void>("install_update") : delay(750);

export const setWindowMode = (onboarding: boolean) =>
  isTauri ? invoke<void>("set_window_mode", { onboarding }) : Promise.resolve();

export const runningInTauri = isTauri;
