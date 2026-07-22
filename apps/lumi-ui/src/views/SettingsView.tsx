import { Save } from "lucide-react";
import { useEffect, useState } from "react";
import type { BrightnessSource, LightAction, RelayContactMode, SettingsDocument, ThemeMode } from "../types";
import { ActionButton, Field, Section, Segmented, Toggle } from "../components/ui";

export default function SettingsView({
  document,
  relayAvailable,
  onSave,
}: {
  document: SettingsDocument;
  relayAvailable: boolean;
  onSave: (next: SettingsDocument) => Promise<void>;
}) {
  const [draft, setDraft] = useState(() => structuredClone(document));
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    if (!dirty) setDraft(structuredClone(document));
  }, [document, dirty]);

  function mutate(update: (next: SettingsDocument) => void) {
    const next = structuredClone(draft);
    update(next);
    setDraft(next);
    setDirty(true);
  }

  async function save() {
    try {
      await onSave(draft);
      setDirty(false);
    } catch {
      // App renders the Agent error while this view keeps the editable draft.
    }
  }

  return (
    <div className="view settings-view">
      <div className="view-heading sticky-heading">
        <div>
          <h1>Settings</h1>
          <p>Everyday behavior and control limits.</p>
        </div>
        <ActionButton variant="primary" icon={<Save size={16} />} disabled={!dirty} onClick={() => void save()}>Save</ActionButton>
      </div>

      <Section title="App">
        <Toggle
          label="Start when I sign in"
          description="Runs the low-resource Agent in the background."
          checked={draft.settings.start_at_login}
          onChange={(checked) => mutate((next) => { next.settings.start_at_login = checked; })}
        />
        <div className="settings-row">
          <div><strong>Appearance</strong><small>Follow Windows or choose a fixed theme.</small></div>
          <Segmented<ThemeMode>
            label="Appearance"
            value={draft.settings.theme}
            options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]}
            onChange={(value) => mutate((next) => { next.settings.theme = value; })}
          />
        </div>
        <Field label="Language">
          <select value={draft.settings.locale} onChange={(event) => mutate((next) => { next.settings.locale = event.target.value; })}>
            <option value="system">System default</option>
            <option value="en">English</option>
            <option value="zh">简体中文</option>
          </select>
        </Field>
      </Section>

      <Section title="Brightness control">
        <div className="settings-row source-setting">
          <div>
            <strong>Brightness source</strong>
            <small>Use the USB light sensor or a location-based weather and sunlight model.</small>
          </div>
          <Segmented<BrightnessSource>
            label="Brightness source"
            value={draft.settings.control.brightness_source}
            options={[
              { value: "sensor", label: "Sensor" },
              { value: "environment", label: "Weather & sun" },
            ]}
            onChange={(value) => mutate((next) => {
              next.settings.control.brightness_source = value;
              if (value === "environment") next.settings.weather.enabled = true;
            })}
          />
        </div>
        <div className="field-grid settings-grid">
          <Field label="Target deadband" hint="Prevents tiny changes">
            <label className="inline-number"><input type="number" min="0" max="20" value={draft.settings.control.target_deadband} onChange={(event) => mutate((next) => { next.settings.control.target_deadband = Number(event.target.value); })} /><span>%</span></label>
          </Field>
          <Field label="Transition time" hint="Smooth 0–100% movement">
            <label className="inline-number"><input type="number" min="0.1" max="30" step="0.1" value={draft.settings.control.transition.duration_ms / 1000} onChange={(event) => mutate((next) => { next.settings.control.transition.duration_ms = Math.round(Number(event.target.value) * 1000); })} /><span>s</span></label>
          </Field>
          <Field label="Maximum write rate" hint="DDC/CI commands per second">
            <label className="inline-number"><input type="number" min="1" max="20" value={draft.settings.control.transition.max_writes_per_second} onChange={(event) => mutate((next) => { next.settings.control.transition.max_writes_per_second = Number(event.target.value); })} /><span>/s</span></label>
          </Field>
          <Field label="Manual override" hint="How long automatic writes pause">
            <label className="inline-number"><input type="number" min="1" max="1440" value={Math.round(draft.settings.control.manual_override.grace_period_ms / 60000)} onChange={(event) => mutate((next) => { next.settings.control.manual_override.grace_period_ms = Number(event.target.value) * 60000; })} /><span>min</span></label>
          </Field>
          <Field label="Override sensitivity" hint="External brightness change threshold">
            <label className="inline-number"><input type="number" min="1" max="100" value={draft.settings.control.manual_override.detection_threshold} onChange={(event) => mutate((next) => { next.settings.control.manual_override.detection_threshold = Number(event.target.value); })} /><span>%</span></label>
          </Field>
          {draft.settings.control.brightness_source === "environment" && (
            <>
              <Field label="Recommendation offset" hint="Personal adjustment after the model">
                <label className="inline-number"><input type="number" min="-50" max="50" value={draft.settings.control.environment_brightness_offset} onChange={(event) => mutate((next) => { next.settings.control.environment_brightness_offset = Number(event.target.value); })} /><span>%</span></label>
              </Field>
              <Field label="Day peak" hint="Upper comfort bound">
                <label className="inline-number"><input type="number" min="0" max="100" value={draft.settings.control.daytime_peak_brightness} onChange={(event) => mutate((next) => { next.settings.control.daytime_peak_brightness = Number(event.target.value); })} /><span>%</span></label>
              </Field>
              <Field label="Night target" hint="Low-light comfort level">
                <label className="inline-number"><input type="number" min="0" max="100" value={draft.settings.control.night_target_brightness} onChange={(event) => mutate((next) => { next.settings.control.night_target_brightness = Number(event.target.value); })} /><span>%</span></label>
              </Field>
            </>
          )}
        </div>
        {draft.settings.control.brightness_source === "sensor" && (
          <details>
            <summary>Sensor filtering</summary>
            <div className="field-grid details-grid">
              <Field label="Median window"><input type="number" min="1" max="31" value={draft.settings.control.filter.median_window} onChange={(event) => mutate((next) => { next.settings.control.filter.median_window = Number(event.target.value); })} /></Field>
              <Field label="Rise response"><input type="number" min="0.01" max="1" step="0.01" value={draft.settings.control.filter.rise_alpha} onChange={(event) => mutate((next) => { next.settings.control.filter.rise_alpha = Number(event.target.value); })} /></Field>
              <Field label="Fall response"><input type="number" min="0.01" max="1" step="0.01" value={draft.settings.control.filter.fall_alpha} onChange={(event) => mutate((next) => { next.settings.control.filter.fall_alpha = Number(event.target.value); })} /></Field>
            </div>
          </details>
        )}
      </Section>

      {relayAvailable && (
        <Section title="Light strip">
          <div className="settings-row">
            <div><strong>Relay contact</strong><small>Invert the logical state for NC wiring.</small></div>
            <Segmented<RelayContactMode>
              label="Relay contact"
              value={draft.settings.relay.contact_mode}
              options={[{ value: "no", label: "NO" }, { value: "nc", label: "NC" }]}
              onChange={(value) => mutate((next) => { next.settings.relay.contact_mode = value; })}
            />
          </div>
          <div className="settings-row">
            <div><strong>Rule fallback</strong><small>Used when no enabled rule matches.</small></div>
            <Segmented<LightAction>
              label="Rule fallback"
              value={draft.settings.relay.fallback_action}
              options={[{ value: "keep", label: "Keep" }, { value: "on", label: "On" }, { value: "off", label: "Off" }]}
              onChange={(value) => mutate((next) => { next.settings.relay.fallback_action = value; })}
            />
          </div>
        </Section>
      )}

      <Section title="Weather and solar time">
        <Toggle
          label="Use local weather and sunrise/sunset"
          description={draft.settings.control.brightness_source === "environment" ? "Required by the selected brightness source." : undefined}
          checked={draft.settings.weather.enabled}
          disabled={draft.settings.control.brightness_source === "environment"}
          onChange={(checked) => mutate((next) => { next.settings.weather.enabled = checked; })}
        />
        <div className="field-grid settings-grid">
          <Field label="Location"><input type="text" maxLength={128} value={draft.settings.weather.location_name} onChange={(event) => mutate((next) => { next.settings.weather.location_name = event.target.value; })} /></Field>
          <Field label="Time zone"><input type="text" maxLength={128} value={draft.settings.weather.timezone} onChange={(event) => mutate((next) => { next.settings.weather.timezone = event.target.value; })} /></Field>
          <Field label="Latitude"><input type="number" min="-90" max="90" step="0.0001" value={draft.settings.weather.latitude} onChange={(event) => mutate((next) => { next.settings.weather.latitude = Number(event.target.value); })} /></Field>
          <Field label="Longitude"><input type="number" min="-180" max="180" step="0.0001" value={draft.settings.weather.longitude} onChange={(event) => mutate((next) => { next.settings.weather.longitude = Number(event.target.value); })} /></Field>
          <Field label="Weather refresh"><label className="inline-number"><input type="number" min="1" max="1440" value={Math.round(draft.settings.weather.refresh_seconds / 60)} onChange={(event) => mutate((next) => { next.settings.weather.refresh_seconds = Number(event.target.value) * 60; })} /><span>min</span></label></Field>
        </div>
        <small className="provider-attribution">Weather data by Open-Meteo, licensed under CC BY 4.0.</small>
      </Section>
    </div>
  );
}
