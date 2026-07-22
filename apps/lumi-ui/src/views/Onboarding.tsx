import { useMemo, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  CloudSun,
  LampDesk,
  MonitorCheck,
  RefreshCw,
  Usb,
} from "lucide-react";
import type { AgentSnapshot, BrightnessSource, SettingsDocument, ThemeMode } from "../types";
import { ActionButton, Field, InlineNotice, Segmented, Toggle } from "../components/ui";

const steps = ["Source", "Monitors", "Comfort", "Light", "Finish"];

export default function Onboarding({
  snapshot,
  document,
  error,
  onSave,
  onRefresh,
  onSetLight,
}: {
  snapshot: AgentSnapshot;
  document: SettingsDocument;
  error: string | null;
  onSave: (next: SettingsDocument) => Promise<void>;
  onRefresh: () => Promise<void>;
  onSetLight: (on: boolean) => Promise<void>;
}) {
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState(() => structuredClone(document));
  const [comfort, setComfort] = useState(snapshot.target_percent ?? 60);
  const [finishing, setFinishing] = useState(false);
  const [advancing, setAdvancing] = useState(false);
  const source = draft.settings.control.brightness_source;
  const relayAvailable = snapshot.device.capabilities.includes("relay");
  const qualified = snapshot.monitors.filter((monitor) => monitor.qualified);
  const environmentReady = draft.settings.weather.enabled
    && draft.settings.weather.location_name.trim().length > 0
    && draft.settings.weather.timezone.trim().length > 0
    && Number.isFinite(draft.settings.weather.latitude)
    && draft.settings.weather.latitude >= -90
    && draft.settings.weather.latitude <= 90
    && Number.isFinite(draft.settings.weather.longitude)
    && draft.settings.weather.longitude >= -180
    && draft.settings.weather.longitude <= 180;
  const canContinue = step === 0
    ? source === "sensor"
      ? snapshot.device.state === "connected" && snapshot.sensor.valid
      : environmentReady
    : step === 1
      ? qualified.length > 0
      : step === steps.length - 1
        ? qualified.length > 0 && (source === "sensor" ? snapshot.sensor.valid : snapshot.target_percent != null)
        : true;

  const status = useMemo(() => {
    if (snapshot.device.state === "connected") return `Connected on ${snapshot.device.port_name ?? "USB"}`;
    if (snapshot.device.state === "discovering") return "Looking for a Lumi device";
    return snapshot.device.last_error ?? "Device not found";
  }, [snapshot.device]);

  async function finish() {
    setFinishing(true);
    try {
      const next = structuredClone(draft);
      if (source === "sensor") {
        const delta = comfort - (snapshot.target_percent ?? comfort);
        if (delta !== 0) {
          next.settings.control.sensor_curve = next.settings.control.sensor_curve.map((point) => ({
            ...point,
            brightness: Math.max(0, Math.min(100, point.brightness + delta)),
          }));
        }
      }
      next.settings.onboarding_completed = true;
      await onSave(next);
    } finally {
      setFinishing(false);
    }
  }

  function selectSource(value: BrightnessSource) {
    const next = structuredClone(draft);
    next.settings.control.brightness_source = value;
    if (value === "environment") next.settings.weather.enabled = true;
    setDraft(next);
  }

  async function continueSetup() {
    if (step !== 0) {
      setStep((value) => value + 1);
      return;
    }
    setAdvancing(true);
    try {
      await onSave(draft);
      setStep(1);
    } catch {
      // The shared error banner reports validation and Agent errors.
    } finally {
      setAdvancing(false);
    }
  }

  return (
    <main className="onboarding-shell">
      <header className="onboarding-header">
        <strong>LumiControl setup</strong>
        <span>{step + 1} of {steps.length}</span>
      </header>
      <ol className="stepper" aria-label="Setup progress">
        {steps.map((label, index) => (
          <li className={index === step ? "is-current" : index < step ? "is-done" : ""} key={label}>
            <span>{index < step ? <Check size={13} /> : index + 1}</span>
            <small>{label}</small>
          </li>
        ))}
      </ol>

      <div className="onboarding-content">
        {error && <InlineNotice tone="bad">{error}</InlineNotice>}
        {step === 0 && (
          <div className="setup-panel">
            {source === "sensor" ? <Usb size={28} /> : <CloudSun size={28} />}
            <h1>Choose automatic brightness source</h1>
            <Segmented<BrightnessSource>
              label="Brightness source"
              value={source}
              options={[
                { value: "sensor", label: "USB sensor" },
                { value: "environment", label: "Weather & sun" },
              ]}
              onChange={selectSource}
            />
            {source === "sensor" ? (
              <>
                <p>Connect the ESP32-C3 ambient light sensor. LumiControl checks its identity automatically.</p>
                <InlineNotice tone={snapshot.device.state === "connected" ? "good" : "neutral"}>
                  <span className="connection-dot" />
                  <strong>{status}</strong>
                  {snapshot.device.serial_number && <span>{snapshot.device.serial_number}</span>}
                  {snapshot.device.state === "connected" && !snapshot.sensor.valid && <span>Waiting for a valid sensor reading</span>}
                </InlineNotice>
                <ActionButton icon={<RefreshCw size={16} />} onClick={() => void onRefresh()}>
                  Scan again
                </ActionButton>
              </>
            ) : (
              <>
                <p>No sensor is required. Your location is used for local weather, sun height, sunrise, sunset, and seasonal daylight.</p>
                <div className="onboarding-location-grid">
                  <Field label="Location name">
                    <input type="text" maxLength={128} value={draft.settings.weather.location_name} onChange={(event) => {
                      const next = structuredClone(draft);
                      next.settings.weather.location_name = event.target.value;
                      setDraft(next);
                    }} />
                  </Field>
                  <Field label="Time zone">
                    <input type="text" maxLength={128} value={draft.settings.weather.timezone} onChange={(event) => {
                      const next = structuredClone(draft);
                      next.settings.weather.timezone = event.target.value;
                      setDraft(next);
                    }} />
                  </Field>
                  <Field label="Latitude">
                    <input type="number" min="-90" max="90" step="0.0001" value={draft.settings.weather.latitude} onChange={(event) => {
                      const next = structuredClone(draft);
                      next.settings.weather.latitude = Number(event.target.value);
                      setDraft(next);
                    }} />
                  </Field>
                  <Field label="Longitude">
                    <input type="number" min="-180" max="180" step="0.0001" value={draft.settings.weather.longitude} onChange={(event) => {
                      const next = structuredClone(draft);
                      next.settings.weather.longitude = Number(event.target.value);
                      setDraft(next);
                    }} />
                  </Field>
                </div>
              </>
            )}
          </div>
        )}

        {step === 1 && (
          <div className="setup-panel">
            <MonitorCheck size={28} />
            <h1>Qualify your monitors</h1>
            <p>Compatible displays must expose DDC/CI brightness control.</p>
            <div className="qualification-list">
              {snapshot.monitors.map((monitor) => (
                <div key={monitor.id}>
                  <span>{monitor.display_name}</span>
                  <small>{monitor.display_path}</small>
                  <strong className={monitor.qualified ? "text-good" : "text-bad"}>
                    {monitor.qualified ? "Ready" : "Unsupported"}
                  </strong>
                </div>
              ))}
              {snapshot.monitors.length === 0 && <InlineNotice>No monitors found</InlineNotice>}
            </div>
            <ActionButton icon={<RefreshCw size={16} />} onClick={() => void onRefresh()}>
              Check again
            </ActionButton>
          </div>
        )}

        {step === 2 && (
          <div className="setup-panel comfort-panel">
            <h1>{source === "sensor" ? "Set a comfortable level" : "Tune the recommendation"}</h1>
            {source === "sensor" ? (
              <>
                <p>Choose how bright the display should feel in the current room light.</p>
                <strong className="comfort-value">{comfort}%</strong>
                <input
                  type="range"
                  min="0"
                  max="100"
                  value={comfort}
                  aria-label="Comfortable brightness"
                  onChange={(event) => setComfort(Number(event.target.value))}
                />
                <small>Current room reading: {snapshot.sensor.filtered_lux?.toFixed(1) ?? "--"} lux</small>
              </>
            ) : (
              <>
                <p>Add a personal offset to the model recommendation. The weather and sunlight curve remains automatic.</p>
                <strong className="comfort-value">
                  {snapshot.environment.base_brightness_percent == null
                    ? "--"
                    : `${Math.max(0, Math.min(100, snapshot.environment.base_brightness_percent + draft.settings.control.environment_brightness_offset))}%`}
                </strong>
                <input
                  type="range"
                  min="-50"
                  max="50"
                  value={draft.settings.control.environment_brightness_offset}
                  aria-label="Recommendation offset"
                  onChange={(event) => {
                    const next = structuredClone(draft);
                    next.settings.control.environment_brightness_offset = Number(event.target.value);
                    setDraft(next);
                  }}
                />
                <small>Model {snapshot.environment.base_brightness_percent ?? "--"}% · offset {draft.settings.control.environment_brightness_offset > 0 ? "+" : ""}{draft.settings.control.environment_brightness_offset}%</small>
              </>
            )}
          </div>
        )}

        {step === 3 && (
          <div className="setup-panel">
            <LampDesk size={28} />
            <h1>{relayAvailable ? "Verify the light strip" : source === "environment" ? "No light hardware required" : "Sensor-only hardware detected"}</h1>
            {relayAvailable ? (
              <>
                <p>Test both relay states, then choose how your relay contacts are wired.</p>
                <div className="button-row">
                  <ActionButton variant="primary" onClick={() => void onSetLight(true)}>Turn on</ActionButton>
                  <ActionButton onClick={() => void onSetLight(false)}>Turn off</ActionButton>
                </div>
                <Segmented
                  label="Relay contact mode"
                  value={draft.settings.relay.contact_mode}
                  options={[{ value: "no", label: "NO" }, { value: "nc", label: "NC" }]}
                  onChange={(value) => {
                    const next = structuredClone(draft);
                    next.settings.relay.contact_mode = value;
                    setDraft(next);
                    void onSave(next).catch(() => undefined);
                  }}
                />
              </>
            ) : (
              <p>No light-strip hardware is required. Its controls will stay hidden.</p>
            )}
          </div>
        )}

        {step === 4 && (
          <div className="setup-panel finish-panel">
            <Check size={28} />
            <h1>Ready for automatic control</h1>
            <p>{qualified.length} compatible monitor{qualified.length === 1 ? "" : "s"} using {source === "sensor" ? "the live USB sensor" : "weather and sunlight"}.</p>
            <Toggle
              label="Start LumiControl when I sign in"
              checked={draft.settings.start_at_login}
              onChange={(checked) => {
                const next = structuredClone(draft);
                next.settings.start_at_login = checked;
                setDraft(next);
              }}
            />
            <div className="field compact-field">
              <span><strong>Appearance</strong></span>
              <Segmented<ThemeMode>
                label="Appearance"
                value={draft.settings.theme}
                options={[
                  { value: "system", label: "System" },
                  { value: "light", label: "Light" },
                  { value: "dark", label: "Dark" },
                ]}
                onChange={(value) => {
                  const next = structuredClone(draft);
                  next.settings.theme = value;
                  setDraft(next);
                }}
              />
            </div>
          </div>
        )}
      </div>

      <footer className="onboarding-footer">
        <ActionButton
          icon={<ArrowLeft size={16} />}
          disabled={step === 0}
          onClick={() => setStep((value) => value - 1)}
        >
          Back
        </ActionButton>
        {step < steps.length - 1 ? (
          <ActionButton
            variant="primary"
            icon={<ArrowRight size={16} />}
            disabled={!canContinue || advancing}
            onClick={() => void continueSetup()}
          >
            {advancing ? "Saving" : "Continue"}
          </ActionButton>
        ) : (
          <ActionButton variant="primary" icon={<Check size={16} />} disabled={!canContinue || finishing} onClick={() => void finish().catch(() => undefined)}>
            {finishing ? "Finishing" : "Finish setup"}
          </ActionButton>
        )}
      </footer>
    </main>
  );
}
