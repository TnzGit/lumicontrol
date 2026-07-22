import { useMemo, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  LampDesk,
  MonitorCheck,
  RefreshCw,
  Usb,
} from "lucide-react";
import type { AgentSnapshot, SettingsDocument, ThemeMode } from "../types";
import { ActionButton, InlineNotice, Segmented, Toggle } from "../components/ui";

const steps = ["Device", "Monitors", "Comfort", "Light", "Finish"];

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
  const relayAvailable = snapshot.device.capabilities.includes("relay");
  const qualified = snapshot.monitors.filter((monitor) => monitor.qualified);
  const canContinue = step === 0
    ? snapshot.device.state === "connected" && snapshot.sensor.valid
    : step === 1
      ? qualified.length > 0
      : step === steps.length - 1
        ? snapshot.sensor.valid && qualified.length > 0
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
      const delta = comfort - (snapshot.target_percent ?? comfort);
      if (delta !== 0) {
        next.settings.control.sensor_curve = next.settings.control.sensor_curve.map((point) => ({
          ...point,
          brightness: Math.max(0, Math.min(100, point.brightness + delta)),
        }));
      }
      next.settings.onboarding_completed = true;
      await onSave(next);
    } finally {
      setFinishing(false);
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
            <Usb size={28} />
            <h1>Connect the ambient light sensor</h1>
            <p>LumiControl checks the device identity and supported hardware automatically.</p>
            <InlineNotice tone={snapshot.device.state === "connected" ? "good" : "neutral"}>
              <span className="connection-dot" />
              <strong>{status}</strong>
              {snapshot.device.serial_number && <span>{snapshot.device.serial_number}</span>}
              {snapshot.device.state === "connected" && !snapshot.sensor.valid && <span>Waiting for a valid sensor reading</span>}
            </InlineNotice>
            <ActionButton icon={<RefreshCw size={16} />} onClick={() => void onRefresh()}>
              Scan again
            </ActionButton>
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
            <h1>Set a comfortable level</h1>
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
          </div>
        )}

        {step === 3 && (
          <div className="setup-panel">
            <LampDesk size={28} />
            <h1>{relayAvailable ? "Verify the light strip" : "Sensor-only hardware detected"}</h1>
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
              <p>Light-strip controls will stay hidden. Brightness automation is fully available.</p>
            )}
          </div>
        )}

        {step === 4 && (
          <div className="setup-panel finish-panel">
            <Check size={28} />
            <h1>Ready for automatic control</h1>
            <p>{qualified.length} compatible monitor{qualified.length === 1 ? "" : "s"} and {snapshot.sensor.valid ? "a live sensor" : "sensor pending"}.</p>
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
            disabled={!canContinue}
            onClick={() => setStep((value) => value + 1)}
          >
            Continue
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
