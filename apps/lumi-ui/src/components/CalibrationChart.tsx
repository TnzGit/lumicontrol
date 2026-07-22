import { useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent, KeyboardEvent } from "react";
import type { SensorCurvePoint } from "../types";

const VIEW_WIDTH = 600;
const VIEW_HEIGHT = 260;
const LEFT = 52;
const TOP = 18;
const PLOT_WIDTH = 526;
const PLOT_HEIGHT = 202;

function sameCurve(left: SensorCurvePoint[], right: SensorCurvePoint[]): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export default function CalibrationChart({
  points,
  currentLux,
  selectedIndex,
  onSelect,
  onChange,
  onCommit,
}: {
  points: SensorCurvePoint[];
  currentLux: number | null;
  selectedIndex: number | null;
  onSelect: (index: number | null) => void;
  onChange: (points: SensorCurvePoint[]) => void;
  onCommit: (before: SensorCurvePoint[], after: SensorCurvePoint[]) => void;
}) {
  const svg = useRef<SVGSVGElement>(null);
  const dragBefore = useRef<SensorCurvePoint[] | null>(null);
  const [dragging, setDragging] = useState<number | null>(null);

  const domain = useMemo(() => {
    const luxValues = points.map((point) => point.lux).filter((lux) => lux > 0);
    const minimum = Math.max(0.1, Math.min(...luxValues, currentLux ?? Infinity) / 2);
    const maximum = Math.max(100, Math.max(...luxValues, currentLux ?? 0) * 1.5);
    return { minimum, maximum };
  }, [points, currentLux]);

  const logMin = Math.log10(domain.minimum);
  const logSpan = Math.max(0.001, Math.log10(domain.maximum) - logMin);
  const xForLux = (lux: number) => LEFT + ((Math.log10(Math.max(domain.minimum, lux)) - logMin) / logSpan) * PLOT_WIDTH;
  const luxForX = (x: number) => 10 ** (logMin + ((x - LEFT) / PLOT_WIDTH) * logSpan);
  const yForBrightness = (brightness: number) => TOP + PLOT_HEIGHT * (1 - brightness / 100);
  const brightnessForY = (y: number) => Math.round((1 - (y - TOP) / PLOT_HEIGHT) * 100);

  const polyline = points
    .map((point) => `${xForLux(point.lux).toFixed(2)},${yForBrightness(point.brightness).toFixed(2)}`)
    .join(" ");

  function updateFromPointer(event: ReactPointerEvent<SVGSVGElement>) {
    if (dragging == null || !svg.current) return;
    const rectangle = svg.current.getBoundingClientRect();
    const x = ((event.clientX - rectangle.left) / rectangle.width) * VIEW_WIDTH;
    const y = ((event.clientY - rectangle.top) / rectangle.height) * VIEW_HEIGHT;
    const previousLux = points[dragging - 1]?.lux ?? domain.minimum;
    const nextLux = points[dragging + 1]?.lux ?? domain.maximum;
    const lux = Math.max(previousLux * 1.01, Math.min(nextLux / 1.01, luxForX(Math.max(LEFT, Math.min(LEFT + PLOT_WIDTH, x)))));
    const brightness = Math.max(0, Math.min(100, brightnessForY(Math.max(TOP, Math.min(TOP + PLOT_HEIGHT, y)))));
    const next = points.map((point, index) =>
      index === dragging ? { lux: Number(lux.toPrecision(5)), brightness } : point,
    );
    onChange(next);
  }

  function beginDrag(index: number, event: ReactPointerEvent<SVGCircleElement>) {
    event.currentTarget.setPointerCapture(event.pointerId);
    dragBefore.current = structuredClone(points);
    setDragging(index);
    onSelect(index);
  }

  function finishDrag() {
    if (dragging == null) return;
    const before = dragBefore.current;
    setDragging(null);
    dragBefore.current = null;
    if (before && !sameCurve(before, points)) onCommit(before, points);
  }

  function moveWithKeyboard(index: number, event: KeyboardEvent<SVGCircleElement>) {
    if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
    event.preventDefault();
    const before = structuredClone(points);
    const point = points[index];
    const multiplier = event.shiftKey ? 1.25 : 1.08;
    let lux = point.lux;
    let brightness = point.brightness;
    if (event.key === "ArrowLeft") lux /= multiplier;
    if (event.key === "ArrowRight") lux *= multiplier;
    if (event.key === "ArrowUp") brightness += event.shiftKey ? 5 : 1;
    if (event.key === "ArrowDown") brightness -= event.shiftKey ? 5 : 1;
    const previousLux = points[index - 1]?.lux ?? domain.minimum;
    const nextLux = points[index + 1]?.lux ?? domain.maximum;
    const next = points.map((candidate, candidateIndex) =>
      candidateIndex === index
        ? {
            lux: Number(Math.max(previousLux * 1.01, Math.min(nextLux / 1.01, lux)).toPrecision(5)),
            brightness: Math.max(0, Math.min(100, brightness)),
          }
        : candidate,
    );
    onChange(next);
    onCommit(before, next);
  }

  const ticks = [0, 25, 50, 75, 100];
  const luxTicks = Array.from(new Set([
    domain.minimum,
    1,
    10,
    100,
    1000,
    domain.maximum,
  ].filter((lux) => lux >= domain.minimum && lux <= domain.maximum))).sort((a, b) => a - b);

  return (
    <svg
      ref={svg}
      className="calibration-chart"
      viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
      role="img"
      aria-label="Ambient lux to brightness calibration curve"
      onPointerMove={updateFromPointer}
      onPointerUp={finishDrag}
      onPointerCancel={finishDrag}
      onPointerLeave={(event) => {
        if (event.buttons === 0) finishDrag();
      }}
    >
      <rect className="chart-plot" x={LEFT} y={TOP} width={PLOT_WIDTH} height={PLOT_HEIGHT} rx="4" />
      {ticks.map((tick) => {
        const y = yForBrightness(tick);
        return (
          <g key={tick}>
            <line className="chart-grid" x1={LEFT} x2={LEFT + PLOT_WIDTH} y1={y} y2={y} />
            <text className="chart-label" x={LEFT - 10} y={y + 4} textAnchor="end">{tick}%</text>
          </g>
        );
      })}
      {luxTicks.map((lux) => {
        const x = xForLux(lux);
        return (
          <g key={lux}>
            <line className="chart-grid chart-grid-vertical" x1={x} x2={x} y1={TOP} y2={TOP + PLOT_HEIGHT} />
            <text className="chart-label" x={x} y={TOP + PLOT_HEIGHT + 21} textAnchor="middle">
              {lux < 1 ? lux.toFixed(1) : Math.round(lux)}
            </text>
          </g>
        );
      })}
      {currentLux != null && currentLux >= domain.minimum && currentLux <= domain.maximum && (
        <g className="current-lux-marker">
          <line x1={xForLux(currentLux)} x2={xForLux(currentLux)} y1={TOP} y2={TOP + PLOT_HEIGHT} />
          <text x={xForLux(currentLux)} y={12} textAnchor="middle">Now</text>
        </g>
      )}
      <polyline className="curve-line-halo" points={polyline} />
      <polyline className="curve-line" points={polyline} />
      {points.map((point, index) => (
        <circle
          key={index}
          className={`curve-point${selectedIndex === index ? " is-selected" : ""}`}
          cx={xForLux(point.lux)}
          cy={yForBrightness(point.brightness)}
          r={selectedIndex === index ? 7 : 6}
          tabIndex={0}
          role="button"
          aria-label={`Point ${index + 1}: ${point.lux.toFixed(1)} lux, ${point.brightness}% brightness`}
          onPointerDown={(event) => beginDrag(index, event)}
          onKeyDown={(event) => moveWithKeyboard(index, event)}
          onFocus={() => onSelect(index)}
        />
      ))}
      <text className="chart-axis-title" x={LEFT + PLOT_WIDTH / 2} y={VIEW_HEIGHT - 4} textAnchor="middle">Ambient light (lux, logarithmic)</text>
    </svg>
  );
}
