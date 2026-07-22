import { AlertTriangle, LampDesk, Play, RotateCcw } from "lucide-react";
import type { AgentSnapshot } from "../types";
import { ActionButton, InlineNotice, Section, StatusPill } from "../components/ui";

function percent(value: number | null): string {
  return value == null ? "--" : `${value}%`;
}

function age(milliseconds: number | null): string {
  if (milliseconds == null) return "No sample";
  if (milliseconds < 1_000) return "Just now";
  const seconds = Math.floor(milliseconds / 1_000);
  return seconds < 60 ? `${seconds}s ago` : `${Math.floor(seconds / 60)}m ago`;
}

function clock(minutes: number | null): string {
  if (minutes == null) return "--:--";
  const normalized = ((minutes % 1_440) + 1_440) % 1_440;
  return `${String(Math.floor(normalized / 60)).padStart(2, "0")}:${String(normalized % 60).padStart(2, "0")}`;
}

function duration(minutes: number | null): string {
  if (minutes == null) return "--";
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

function signedPercent(value: number): string {
  return `${value > 0 ? "+" : ""}${value}%`;
}

function weatherLabel(value: AgentSnapshot["environment"]["weather"]): string {
  if (value == null) return "Solar model";
  return { clear: "Clear", cloudy: "Cloudy", rain: "Rain", fog: "Fog" }[value];
}

export default function StatusView({
  snapshot,
  onResume,
  onSetLight,
  onClearOverride,
}: {
  snapshot: AgentSnapshot;
  onResume: () => Promise<void>;
  onSetLight: (on: boolean) => Promise<void>;
  onClearOverride: (monitorId?: string) => Promise<void>;
}) {
  const current = snapshot.monitors.find((monitor) => monitor.current_percent != null)?.current_percent ?? null;
  const overridden = snapshot.monitors.filter((monitor) => monitor.manual_override_remaining_ms != null);
  const isAutomatic = !snapshot.paused && snapshot.health !== "fault";

  return (
    <div className="view status-view">
      {snapshot.configuration_warning && (
        <InlineNotice tone="warning">
          <AlertTriangle size={16} />
          <span>{snapshot.configuration_warning}</span>
        </InlineNotice>
      )}

      <Section
        title="Now"
        className="now-section"
        action={
          <StatusPill tone={isAutomatic ? "good" : snapshot.paused ? "neutral" : "bad"}>
            {isAutomatic ? "Automatic" : snapshot.paused ? "Paused" : "Needs attention"}
          </StatusPill>
        }
      >
        <div className="brightness-summary">
          <div>
            <span className="metric-label">Current</span>
            <strong className="hero-metric">{percent(current)}</strong>
          </div>
          <div className="metric-divider" />
          <div>
            <span className="metric-label">Target</span>
            <strong className="hero-metric target-metric">{percent(snapshot.target_percent)}</strong>
          </div>
        </div>
        <div className="progress-track" aria-label={`Target brightness ${percent(snapshot.target_percent)}`}>
          <div style={{ width: `${snapshot.target_percent ?? 0}%` }} />
        </div>
        <div className="now-meta">
          <span>{snapshot.status_message}</span>
          {snapshot.paused && (
            <ActionButton variant="ghost" icon={<Play size={15} />} onClick={() => void onResume()}>
              Resume automatic control
            </ActionButton>
          )}
        </div>

        {snapshot.relay.available && (
          <div className="relay-strip">
            <div className="relay-state">
              <LampDesk size={17} />
              <span>Light strip</span>
              <strong className={snapshot.relay.light_on ? "text-good" : ""}>
                {snapshot.relay.light_on ? "On" : "Off"}
              </strong>
            </div>
            <div className="relay-actions" role="group" aria-label="Light strip control">
              <button
                type="button"
                className={snapshot.relay.light_on ? "is-active" : ""}
                onClick={() => void onSetLight(true)}
              >
                On
              </button>
              <button
                type="button"
                className={snapshot.relay.light_on === false ? "is-active" : ""}
                onClick={() => void onSetLight(false)}
              >
                Off
              </button>
            </div>
          </div>
        )}
        {snapshot.relay.rules_enabled && (
          <div className="match-line">
            {snapshot.relay.matched_rule_name
              ? `Matched: ${snapshot.relay.matched_rule_name}`
              : "No light rule matched; fallback is active"}
          </div>
        )}
      </Section>

      {snapshot.brightness_source === "sensor" ? (
        <Section
          title="Sensor"
          action={
            <StatusPill tone={snapshot.sensor.valid ? "good" : "bad"}>
              {snapshot.sensor.valid ? "Live" : "Unavailable"}
            </StatusPill>
          }
        >
          <div className="sensor-line">
            <strong>{snapshot.sensor.filtered_lux == null ? "--" : snapshot.sensor.filtered_lux.toFixed(1)} lux</strong>
            <span>{snapshot.device.port_name ?? "No port"}</span>
            <span>{age(snapshot.sensor.sample_age_ms)}</span>
          </div>
        </Section>
      ) : (
        <Section
          title="Weather & Sun"
          action={
            <StatusPill tone={snapshot.environment.last_error ? "warning" : snapshot.environment.weather ? "good" : "neutral"}>
              {snapshot.environment.last_error ? "Solar fallback" : snapshot.environment.weather ? "Live weather" : "Updating"}
            </StatusPill>
          }
        >
          <div className="environment-line">
            <strong>{weatherLabel(snapshot.environment.weather)}</strong>
            <span>Sun {snapshot.environment.solar_elevation_degrees == null ? "--" : `${snapshot.environment.solar_elevation_degrees.toFixed(1)}°`}</span>
            <span>{clock(snapshot.environment.sunrise_minutes)}–{clock(snapshot.environment.sunset_minutes)}</span>
            <span>Daylight {duration(snapshot.environment.daylight_minutes)}</span>
          </div>
          <div className="recommendation-line">
            <span>Model <strong>{percent(snapshot.environment.base_brightness_percent)}</strong></span>
            <span>Offset <strong>{signedPercent(snapshot.environment.brightness_offset_percent)}</strong></span>
            {snapshot.environment.weather_observed_at_unix_ms != null && (
              <span>Weather {age(Math.max(0, Date.now() - snapshot.environment.weather_observed_at_unix_ms))}</span>
            )}
          </div>
        </Section>
      )}

      {overridden.length > 0 && (
        <InlineNotice tone="warning">
          <span>{overridden.length} monitor override{overridden.length > 1 ? "s" : ""} active</span>
          <ActionButton
            variant="ghost"
            icon={<RotateCcw size={15} />}
            onClick={() => void onClearOverride()}
          >
            Return to auto
          </ActionButton>
        </InlineNotice>
      )}

      <Section title="Monitors" className="below-fold">
        <div className="monitor-rows">
          {snapshot.monitors.map((monitor) => (
            <div className="monitor-row" key={monitor.id}>
              <span>{monitor.display_name}</span>
              <span className="muted">{monitor.display_path}</span>
              <strong>{percent(monitor.current_percent)}</strong>
            </div>
          ))}
        </div>
      </Section>
    </div>
  );
}
