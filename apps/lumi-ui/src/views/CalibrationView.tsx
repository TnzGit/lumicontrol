import { Plus, RotateCcw, Trash2, Undo2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { AgentSnapshot, SensorCurvePoint, SettingsDocument } from "../types";
import CalibrationChart from "../components/CalibrationChart";
import { ActionButton, Field, InlineNotice, Section } from "../components/ui";

const DEFAULT_CURVE: SensorCurvePoint[] = [
  { lux: 20, brightness: 40 },
  { lux: 80, brightness: 72 },
  { lux: 250, brightness: 88 },
];
const MAX_CURVE_POINTS = 64;

function curvesEqual(left: SensorCurvePoint[], right: SensorCurvePoint[]) {
  return JSON.stringify(left) === JSON.stringify(right);
}

export default function CalibrationView({
  snapshot,
  document,
  onSave,
}: {
  snapshot: AgentSnapshot;
  document: SettingsDocument;
  onSave: (next: SettingsDocument) => Promise<void>;
}) {
  const [points, setPoints] = useState(() => structuredClone(document.settings.control.sensor_curve));
  const [selected, setSelected] = useState<number | null>(null);
  const documentRef = useRef(document);
  const history = useRef<SensorCurvePoint[][]>([]);
  const [historyCount, setHistoryCount] = useState(0);
  const pendingEditBefore = useRef<SensorCurvePoint[] | null>(null);
  const pendingEditAfter = useRef<SensorCurvePoint[] | null>(null);
  const editTimer = useRef<number | null>(null);

  useEffect(() => {
    documentRef.current = document;
    setPoints(structuredClone(document.settings.control.sensor_curve));
  }, [document]);

  useEffect(() => () => {
    if (editTimer.current !== null) window.clearTimeout(editTimer.current);
    const before = pendingEditBefore.current;
    const after = pendingEditAfter.current;
    if (before && after) void persist(before, after).catch(() => undefined);
  }, []);

  async function persist(before: SensorCurvePoint[], after: SensorCurvePoint[], remember = true) {
    if (curvesEqual(before, after)) return;
    if (remember) {
      history.current = [...history.current, structuredClone(before)].slice(-3);
      setHistoryCount(history.current.length);
    }
    const next = structuredClone(documentRef.current);
    next.settings.control.sensor_curve = structuredClone(after);
    await onSave(next);
  }

  function commit(before: SensorCurvePoint[], after: SensorCurvePoint[], remember = true) {
    void persist(before, after, remember).catch(() => undefined);
  }

  function takePendingEdit(): SensorCurvePoint[] | null {
    if (editTimer.current !== null) {
      window.clearTimeout(editTimer.current);
      editTimer.current = null;
    }
    const before = pendingEditBefore.current;
    pendingEditBefore.current = null;
    pendingEditAfter.current = null;
    return before;
  }

  function queueEdit(before: SensorCurvePoint[], after: SensorCurvePoint[]) {
    if (!pendingEditBefore.current) pendingEditBefore.current = structuredClone(before);
    pendingEditAfter.current = structuredClone(after);
    if (editTimer.current !== null) window.clearTimeout(editTimer.current);
    editTimer.current = window.setTimeout(() => {
      editTimer.current = null;
      const first = pendingEditBefore.current;
      const latest = pendingEditAfter.current;
      pendingEditBefore.current = null;
      pendingEditAfter.current = null;
      if (first && latest) commit(first, latest);
    }, 350);
  }

  function addPoint() {
    if (points.length >= MAX_CURVE_POINTS) return;
    const before = takePendingEdit() ?? structuredClone(points);
    const index = selected ?? points.length - 1;
    const left = points[Math.max(0, index)];
    const right = points[index + 1];
    const point = right
      ? {
          lux: Math.sqrt(left.lux * right.lux),
          brightness: Math.round((left.brightness + right.brightness) / 2),
        }
      : { lux: left.lux * 2, brightness: Math.min(100, left.brightness + 8) };
    const next = [...points, point].sort((a, b) => a.lux - b.lux);
    setPoints(next);
    setSelected(next.indexOf(point));
    commit(before, next);
  }

  function removePoint() {
    if (selected == null || points.length <= 2) return;
    const before = takePendingEdit() ?? structuredClone(points);
    const next = points.filter((_, index) => index !== selected);
    setPoints(next);
    setSelected(Math.min(selected, next.length - 1));
    commit(before, next);
  }

  function resetCurve() {
    const before = takePendingEdit() ?? structuredClone(points);
    const next = structuredClone(DEFAULT_CURVE);
    setPoints(next);
    setSelected(null);
    commit(before, next);
  }

  function revert() {
    const pending = takePendingEdit();
    if (pending) {
      setPoints(pending);
      setSelected(null);
      return;
    }
    const previous = history.current.pop();
    if (!previous) return;
    const before = structuredClone(points);
    setPoints(previous);
    setSelected(null);
    setHistoryCount(history.current.length);
    commit(before, previous, false);
  }

  function editSelected(key: "lux" | "brightness", value: number) {
    if (selected == null || !Number.isFinite(value)) return;
    const before = structuredClone(points);
    const edited = {
      ...points[selected],
      [key]: key === "lux" ? Math.max(0.001, value) : Math.max(0, Math.min(100, Math.round(value))),
    };
    const next = points.map((point, index) =>
      index === selected ? edited : point,
    ).sort((a, b) => a.lux - b.lux);
    setPoints(next);
    setSelected(next.indexOf(edited));
    queueEdit(before, next);
  }

  return (
    <div className="view calibration-view">
      <div className="view-heading">
        <div>
          <h1>Calibration</h1>
          <p>Shape how ambient light maps to monitor brightness.</p>
        </div>
        <div className="button-row">
          <ActionButton icon={<Undo2 size={16} />} disabled={historyCount === 0} onClick={revert}>
            Revert{historyCount ? ` (${historyCount})` : ""}
          </ActionButton>
          <ActionButton variant="ghost" icon={<RotateCcw size={16} />} onClick={resetCurve}>Reset</ActionButton>
        </div>
      </div>

      <Section title="Lux to brightness curve">
        <CalibrationChart
          points={points}
          currentLux={snapshot.sensor.filtered_lux}
          selectedIndex={selected}
          onSelect={setSelected}
          onChange={setPoints}
          onCommit={(before, after) => commit(takePendingEdit() ?? before, after)}
        />
        <div className="chart-toolbar">
          <span>{points.length} points · changes save automatically</span>
          <div className="button-row">
            <ActionButton icon={<Plus size={15} />} disabled={points.length >= MAX_CURVE_POINTS} onClick={addPoint}>Add point</ActionButton>
            <ActionButton
              variant="ghost"
              icon={<Trash2 size={15} />}
              disabled={selected == null || points.length <= 2}
              onClick={removePoint}
            >
              Remove
            </ActionButton>
          </div>
        </div>
      </Section>

      {selected != null ? (
        <Section title={`Point ${selected + 1}`} className="point-editor">
          <div className="field-grid">
            <Field label="Ambient light" hint="lux">
              <input
                type="number"
                min="0.001"
                step="0.1"
                value={Number(points[selected].lux.toFixed(3))}
                onChange={(event) => editSelected("lux", Number(event.target.value))}
              />
            </Field>
            <Field label="Screen brightness" hint="percent">
              <input
                type="number"
                min="0"
                max="100"
                value={points[selected].brightness}
                onChange={(event) => editSelected("brightness", Number(event.target.value))}
              />
            </Field>
          </div>
        </Section>
      ) : (
        <InlineNotice>Select or drag a point. Arrow keys make precise adjustments; hold Shift for larger steps.</InlineNotice>
      )}
    </div>
  );
}
