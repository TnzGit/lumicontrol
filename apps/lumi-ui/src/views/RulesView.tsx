import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  FlaskConical,
  Plus,
  Trash2,
} from "lucide-react";
import type {
  AgentSnapshot,
  ConditionExpression,
  LightAction,
  LightCondition,
  LightRule,
  SettingsDocument,
  WeatherKind,
} from "../types";
import { ActionButton, Field, InlineNotice, Section, Segmented, Toggle } from "../components/ui";

type ConditionKind = LightCondition["kind"];
const MAX_RULES = 64;
const MAX_CONDITIONS_PER_RULE = 32;

const conditionLabels: Array<{ value: ConditionKind; label: string }> = [
  { value: "time_after", label: "Time is after" },
  { value: "time_before", label: "Time is before" },
  { value: "after_sunrise", label: "After sunrise" },
  { value: "before_sunset", label: "Before sunset" },
  { value: "after_sunset", label: "After sunset" },
  { value: "lux_below", label: "Ambient light below" },
  { value: "lux_above", label: "Ambient light above" },
  { value: "current_brightness_below", label: "Current brightness below" },
  { value: "current_brightness_above", label: "Current brightness above" },
  { value: "target_brightness_below", label: "Target brightness below" },
  { value: "target_brightness_above", label: "Target brightness above" },
  { value: "weather_is", label: "Weather is" },
];

function defaultCondition(kind: ConditionKind = "lux_below"): LightCondition {
  switch (kind) {
    case "time_after": return { kind, minutes: 18 * 60 };
    case "time_before": return { kind, minutes: 7 * 60 };
    case "after_sunrise":
    case "before_sunset":
    case "after_sunset": return { kind, offset_minutes: 0 };
    case "lux_below": return { kind, lux: 35 };
    case "lux_above": return { kind, lux: 150 };
    case "current_brightness_below":
    case "target_brightness_below": return { kind, brightness: 35 };
    case "current_brightness_above":
    case "target_brightness_above": return { kind, brightness: 75 };
    case "weather_is": return { kind, weather: "clear" };
  }
}

function countConditions(expression: ConditionExpression): number {
  return expression.kind === "condition"
    ? 1
    : expression.conditions.reduce((total, child) => total + countConditions(child), 0);
}

function newRule(index: number): LightRule {
  return {
    id: `rule-${Date.now()}-${index}`,
    name: `Rule ${index + 1}`,
    enabled: true,
    when: { kind: "and", conditions: [{ kind: "condition", condition: defaultCondition() }] },
    then: "on",
  };
}

function minutesToTime(minutes: number): string {
  const normalized = ((minutes % 1440) + 1440) % 1440;
  return `${String(Math.floor(normalized / 60)).padStart(2, "0")}:${String(normalized % 60).padStart(2, "0")}`;
}

function timeToMinutes(value: string): number {
  const [hours, minutes] = value.split(":").map(Number);
  return (hours || 0) * 60 + (minutes || 0);
}

function ConditionRow({
  value,
  onChange,
  onRemove,
}: {
  value: LightCondition;
  onChange: (condition: LightCondition) => void;
  onRemove: () => void;
}) {
  return (
    <div className="condition-row">
      <select
        aria-label="Condition type"
        value={value.kind}
        onChange={(event) => onChange(defaultCondition(event.target.value as ConditionKind))}
      >
        {conditionLabels.map((condition) => (
          <option value={condition.value} key={condition.value}>{condition.label}</option>
        ))}
      </select>
      {(value.kind === "time_after" || value.kind === "time_before") && (
        <input
          type="time"
          aria-label="Time"
          value={minutesToTime(value.minutes)}
          onChange={(event) => onChange({ ...value, minutes: timeToMinutes(event.target.value) })}
        />
      )}
      {(value.kind === "after_sunrise" || value.kind === "before_sunset" || value.kind === "after_sunset") && (
        <label className="inline-number">
          <input
            type="number"
            min="-720"
            max="720"
            value={value.offset_minutes}
            aria-label="Offset in minutes"
            onChange={(event) => onChange({ ...value, offset_minutes: Number(event.target.value) })}
          />
          <span>min</span>
        </label>
      )}
      {(value.kind === "lux_below" || value.kind === "lux_above") && (
        <label className="inline-number">
          <input
            type="number"
            min="0"
            step="1"
            value={value.lux}
            aria-label="Lux threshold"
            onChange={(event) => onChange({ ...value, lux: Math.max(0, Number(event.target.value)) })}
          />
          <span>lux</span>
        </label>
      )}
      {(value.kind === "current_brightness_below" ||
        value.kind === "current_brightness_above" ||
        value.kind === "target_brightness_below" ||
        value.kind === "target_brightness_above") && (
        <label className="inline-number">
          <input
            type="number"
            min="0"
            max="100"
            value={value.brightness}
            aria-label="Brightness threshold"
            onChange={(event) => onChange({ ...value, brightness: Math.max(0, Math.min(100, Number(event.target.value))) })}
          />
          <span>%</span>
        </label>
      )}
      {value.kind === "weather_is" && (
        <select
          aria-label="Weather kind"
          value={value.weather}
          onChange={(event) => onChange({ ...value, weather: event.target.value as WeatherKind })}
        >
          <option value="clear">Clear</option>
          <option value="cloudy">Cloudy</option>
          <option value="rain">Rain</option>
          <option value="fog">Fog</option>
        </select>
      )}
      <button className="icon-button" type="button" title="Remove condition" aria-label="Remove condition" onClick={onRemove}>
        <Trash2 size={15} />
      </button>
    </div>
  );
}

function ExpressionEditor({
  expression,
  onChange,
  depth = 0,
  onRemove,
}: {
  expression: ConditionExpression;
  onChange: (expression: ConditionExpression) => void;
  depth?: number;
  onRemove?: () => void;
}) {
  if (expression.kind === "condition") {
    return <ConditionRow value={expression.condition} onChange={(condition) => onChange({ kind: "condition", condition })} onRemove={() => onRemove?.()} />;
  }
  const group = expression;

  function updateChild(index: number, child: ConditionExpression) {
    onChange({ ...group, conditions: group.conditions.map((item, itemIndex) => itemIndex === index ? child : item) });
  }

  function removeChild(index: number) {
    onChange({ ...group, conditions: group.conditions.filter((_, itemIndex) => itemIndex !== index) });
  }

  return (
    <div className={`expression-group depth-${Math.min(depth, 2)}`}>
      <div className="group-toolbar">
        <Segmented
          label="Condition operator"
          value={group.kind}
          options={[{ value: "and", label: "AND" }, { value: "or", label: "OR" }]}
          onChange={(kind) => onChange({ kind, conditions: group.conditions })}
        />
        <ActionButton
          variant="ghost"
          icon={<Plus size={14} />}
          disabled={countConditions(group) >= MAX_CONDITIONS_PER_RULE}
          onClick={() => onChange({ ...group, conditions: [...group.conditions, { kind: "condition", condition: defaultCondition() }] })}
        >
          Condition
        </ActionButton>
        {depth < 2 && (
          <ActionButton
            variant="ghost"
            icon={<Plus size={14} />}
            disabled={countConditions(group) >= MAX_CONDITIONS_PER_RULE}
            onClick={() => onChange({
              ...group,
              conditions: [...group.conditions, { kind: group.kind === "and" ? "or" : "and", conditions: [{ kind: "condition", condition: defaultCondition() }] }],
            })}
          >
            Group
          </ActionButton>
        )}
        {onRemove && <ActionButton variant="ghost" icon={<Trash2 size={14} />} onClick={onRemove}>Group</ActionButton>}
      </div>
      <div className="expression-children">
        {group.conditions.map((child, index) => (
          <div className="expression-child" key={index}>
            {index > 0 && <span className="operator-label">{group.kind.toUpperCase()}</span>}
            <ExpressionEditor
              expression={child}
              depth={depth + 1}
              onChange={(next) => updateChild(index, next)}
              onRemove={() => removeChild(index)}
            />
          </div>
        ))}
        {group.conditions.length === 0 && <InlineNotice tone="warning">Add at least one condition.</InlineNotice>}
      </div>
    </div>
  );
}

interface TestContext {
  minutes: number;
  lux: number;
  current: number;
  target: number;
  sunrise: number | null;
  sunset: number | null;
  weather: WeatherKind | null;
}

function normalizeMinutes(minutes: number): number {
  return ((minutes % 1440) + 1440) % 1440;
}

function matchesCondition(condition: LightCondition, context: TestContext): boolean {
  switch (condition.kind) {
    case "time_after": return context.minutes >= normalizeMinutes(condition.minutes);
    case "time_before": return context.minutes <= normalizeMinutes(condition.minutes);
    case "after_sunrise": return context.sunrise !== null && context.minutes >= normalizeMinutes(context.sunrise + condition.offset_minutes);
    case "before_sunset": return context.sunset !== null && context.minutes <= normalizeMinutes(context.sunset + condition.offset_minutes);
    case "after_sunset": return context.sunset !== null && context.minutes >= normalizeMinutes(context.sunset + condition.offset_minutes);
    case "lux_below": return context.lux < condition.lux;
    case "lux_above": return context.lux > condition.lux;
    case "current_brightness_below": return context.current < condition.brightness;
    case "current_brightness_above": return context.current > condition.brightness;
    case "target_brightness_below": return context.target < condition.brightness;
    case "target_brightness_above": return context.target > condition.brightness;
    case "weather_is": return context.weather === condition.weather;
  }
}

function matchesExpression(expression: ConditionExpression, context: TestContext): boolean {
  if (expression.kind === "condition") return matchesCondition(expression.condition, context);
  if (expression.conditions.length === 0) return false;
  return expression.kind === "and"
    ? expression.conditions.every((child) => matchesExpression(child, context))
    : expression.conditions.some((child) => matchesExpression(child, context));
}

const PRESETS: Record<string, Omit<LightRule, "id">> = {
  evening: {
    name: "Evening light",
    enabled: true,
    when: { kind: "or", conditions: [
      { kind: "condition", condition: { kind: "after_sunset", offset_minutes: 0 } },
      { kind: "condition", condition: { kind: "lux_below", lux: 35 } },
    ] },
    then: "on",
  },
  late: {
    name: "Late night off",
    enabled: true,
    when: { kind: "and", conditions: [{ kind: "condition", condition: { kind: "time_after", minutes: 90 } }] },
    then: "off",
  },
  bright: {
    name: "Bright room off",
    enabled: true,
    when: { kind: "and", conditions: [{ kind: "condition", condition: { kind: "lux_above", lux: 180 } }] },
    then: "off",
  },
};

export default function RulesView({
  snapshot,
  document,
  onSave,
}: {
  snapshot: AgentSnapshot;
  document: SettingsDocument;
  onSave: (next: SettingsDocument) => Promise<void>;
}) {
  const [rules, setRules] = useState(() => structuredClone(document.settings.relay.rules));
  const [preset, setPreset] = useState("evening");
  const documentRef = useRef(document);
  const pendingSave = useRef<SettingsDocument | null>(null);
  const saveTimer = useRef<number | null>(null);
  const liveTestContext = (): TestContext => ({
    minutes: snapshot.environment.now_minutes,
    lux: snapshot.sensor.filtered_lux ?? 50,
    current: snapshot.monitors[0]?.current_percent ?? 50,
    target: snapshot.target_percent ?? 50,
    sunrise: snapshot.environment.sunrise_minutes,
    sunset: snapshot.environment.sunset_minutes,
    weather: snapshot.environment.weather,
  });
  const [test, setTest] = useState<TestContext>(liveTestContext);

  useEffect(() => {
    documentRef.current = document;
    setRules(structuredClone(document.settings.relay.rules));
  }, [document]);

  useEffect(() => () => {
    if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    const pending = pendingSave.current;
    if (pending) void onSave(pending).catch(() => undefined);
  }, [onSave]);

  function persist(
    nextRules: LightRule[],
    mutate?: (next: SettingsDocument) => void,
    deferred = false,
  ) {
    setRules(nextRules);
    const next = structuredClone(documentRef.current);
    next.settings.relay.rules = structuredClone(nextRules);
    mutate?.(next);
    documentRef.current = next;

    if (saveTimer.current !== null) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = null;
    }
    pendingSave.current = deferred ? next : null;
    if (deferred) {
      saveTimer.current = window.setTimeout(() => {
        saveTimer.current = null;
        const pending = pendingSave.current;
        pendingSave.current = null;
        if (pending) void onSave(pending).catch(() => undefined);
      }, 350);
      return;
    }
    void onSave(next).catch(() => undefined);
  }

  const testedRule = useMemo(
    () => rules.find((rule) => rule.enabled && matchesExpression(rule.when, test)) ?? null,
    [rules, test],
  );

  function move(index: number, direction: -1 | 1) {
    const target = index + direction;
    if (target < 0 || target >= rules.length) return;
    const next = [...rules];
    [next[index], next[target]] = [next[target], next[index]];
    persist(next);
  }

  function addPreset() {
    const template = PRESETS[preset];
    const rule = { ...structuredClone(template), id: `${preset}-${Date.now()}` };
    persist([...rules, rule]);
  }

  return (
    <div className="view rules-view">
      <div className="view-heading">
        <div>
          <h1>Light rules</h1>
          <p>The first enabled rule that matches wins.</p>
        </div>
        <div className="preset-controls">
          <select aria-label="Rule preset" value={preset} onChange={(event) => setPreset(event.target.value)}>
            <option value="evening">Evening light</option>
            <option value="late">Late night off</option>
            <option value="bright">Bright room off</option>
          </select>
          <ActionButton icon={<Plus size={15} />} disabled={rules.length >= MAX_RULES} onClick={addPreset}>Add preset</ActionButton>
        </div>
      </div>

      <Toggle
        label="Enable rules mode"
        description="Rules are evaluated from top to bottom whenever inputs change."
        checked={document.settings.relay.rules_enabled}
        onChange={(checked) => persist(rules, (next) => { next.settings.relay.rules_enabled = checked; })}
      />

      {rules.map((rule, index) => (
        <Section
          title={`${index + 1}. ${rule.name}`}
          className={`rule-section${snapshot.relay.matched_rule_id === rule.id ? " is-matched" : ""}`}
          key={rule.id}
          action={snapshot.relay.matched_rule_id === rule.id ? <span className="matched-badge">Matched now</span> : undefined}
        >
          <div className="rule-heading-row">
            <input
              className="rule-name"
              aria-label={`Rule ${index + 1} name`}
              maxLength={128}
              value={rule.name}
              onChange={(event) => {
                const next = rules.map((item, itemIndex) => itemIndex === index ? { ...item, name: event.target.value } : item);
                persist(next, undefined, true);
              }}
            />
            <label className="compact-check"><input type="checkbox" checked={rule.enabled} onChange={(event) => {
              const next = rules.map((item, itemIndex) => itemIndex === index ? { ...item, enabled: event.target.checked } : item);
              persist(next);
            }} /> Enabled</label>
            <div className="priority-buttons">
              <button type="button" className="icon-button" title="Move up" aria-label="Move rule up" disabled={index === 0} onClick={() => move(index, -1)}><ArrowUp size={15} /></button>
              <button type="button" className="icon-button" title="Move down" aria-label="Move rule down" disabled={index === rules.length - 1} onClick={() => move(index, 1)}><ArrowDown size={15} /></button>
              <button type="button" className="icon-button" title="Delete rule" aria-label="Delete rule" onClick={() => persist(rules.filter((_, itemIndex) => itemIndex !== index))}><Trash2 size={15} /></button>
            </div>
          </div>
          <ExpressionEditor
            expression={rule.when}
            onChange={(when) => {
              const next = rules.map((item, itemIndex) => itemIndex === index ? { ...item, when } : item);
              persist(next, undefined, true);
            }}
          />
          <div className="then-row">
            <strong>THEN</strong>
            <Segmented<LightAction>
              label="Light action"
              value={rule.then}
              options={[{ value: "on", label: "On" }, { value: "off", label: "Off" }, { value: "keep", label: "Keep" }]}
              onChange={(then) => {
                const next = rules.map((item, itemIndex) => itemIndex === index ? { ...item, then } : item);
                persist(next);
              }}
            />
          </div>
        </Section>
      ))}

      <ActionButton icon={<Plus size={16} />} disabled={rules.length >= MAX_RULES} onClick={() => persist([...rules, newRule(rules.length)])}>Add blank rule</ActionButton>

      <Section
        title="Test rules"
        action={(
          <ActionButton variant="ghost" icon={<FlaskConical size={15} />} onClick={() => setTest(liveTestContext())}>
            Use live inputs
          </ActionButton>
        )}
      >
        <div className="test-grid">
          <Field label="Time"><input type="time" value={minutesToTime(test.minutes)} onChange={(event) => setTest({ ...test, minutes: timeToMinutes(event.target.value) })} /></Field>
          <Field label="Sunrise"><input type="time" value={test.sunrise === null ? "" : minutesToTime(test.sunrise)} onChange={(event) => setTest({ ...test, sunrise: event.target.value ? timeToMinutes(event.target.value) : null })} /></Field>
          <Field label="Sunset"><input type="time" value={test.sunset === null ? "" : minutesToTime(test.sunset)} onChange={(event) => setTest({ ...test, sunset: event.target.value ? timeToMinutes(event.target.value) : null })} /></Field>
          <Field label="Ambient lux"><input type="number" min="0" value={test.lux} onChange={(event) => setTest({ ...test, lux: Number(event.target.value) })} /></Field>
          <Field label="Current %"><input type="number" min="0" max="100" value={test.current} onChange={(event) => setTest({ ...test, current: Number(event.target.value) })} /></Field>
          <Field label="Target %"><input type="number" min="0" max="100" value={test.target} onChange={(event) => setTest({ ...test, target: Number(event.target.value) })} /></Field>
          <Field label="Weather"><select value={test.weather ?? ""} onChange={(event) => setTest({ ...test, weather: event.target.value ? event.target.value as WeatherKind : null })}><option value="">Unavailable</option><option value="clear">Clear</option><option value="cloudy">Cloudy</option><option value="rain">Rain</option><option value="fog">Fog</option></select></Field>
        </div>
        <InlineNotice tone={testedRule ? "good" : "neutral"}>
          {testedRule ? `Rule ${rules.indexOf(testedRule) + 1} matches: ${testedRule.name} -> ${testedRule.then.toUpperCase()}` : `No rule matches -> ${document.settings.relay.fallback_action.toUpperCase()}`}
        </InlineNotice>
      </Section>
    </div>
  );
}
