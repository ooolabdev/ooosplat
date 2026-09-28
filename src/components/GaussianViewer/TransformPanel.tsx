import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { useI18n } from "../../i18n";
import type { GaussianTransform } from "../../types/pipeline";

type ScrubMode = "linear" | "rotation" | "scale";
type DragState = {
  pointerId: number;
  startX: number;
  startValue: number;
  dragging: boolean;
};

const TRANSFORM_SCALE_MIN = 0.001;
const TRANSFORM_SCALE_MAX = 10000;
const clampValue = (value: number, minimum?: number, maximum?: number) => Math.min(maximum ?? Number.POSITIVE_INFINITY, Math.max(minimum ?? Number.NEGATIVE_INFINITY, value));
const formatValue = (value: number) => {
  const absolute = Math.abs(value);
  if (absolute !== 0 && (absolute >= 100_000_000 || absolute < 0.0001)) {
    return value.toExponential(4).replace(/\.?(0+)(?=e)/, "");
  }
  return String(Number(value.toFixed(4)));
};

function scrubValue(startValue: number, deltaX: number, mode: ScrubMode, fine: boolean, minimum?: number, maximum?: number) {
  const sensitivity = fine ? 0.1 : 1;
  if (mode === "scale") return clampValue(startValue * Math.exp(deltaX * 0.01 * sensitivity), minimum, maximum);
  const step = mode === "rotation" ? 0.25 : Math.max(Math.abs(startValue) * 0.005, 0.01);
  return clampValue(startValue + deltaX * step * sensitivity, minimum, maximum);
}

export function NumberField({
  label, name, value, onBegin, onChange, onCommit, mode = "linear", min, max,
}: {
  label: string;
  name: string;
  value: number;
  onBegin: () => void;
  onChange: (value: number) => void;
  onCommit: () => void;
  mode?: ScrubMode;
  min?: number;
  max?: number;
}) {
  const { t } = useI18n();
  const [text, setText] = useState(formatValue(value));
  const [focused, setFocused] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);
  const dragRef = useRef<DragState | null>(null);
  const minimum = min ?? (mode === "scale" ? TRANSFORM_SCALE_MIN : undefined);
  const maximum = max ?? (mode === "scale" ? TRANSFORM_SCALE_MAX : undefined);

  useEffect(() => {
    if (!focused && !dragRef.current?.dragging) setText(formatValue(value));
  }, [focused, value]);

  const commit = () => {
    const parsed = Number(text);
    if (!Number.isFinite(parsed) || (minimum != null && parsed < minimum) || (maximum != null && parsed > maximum)) {
      setText(formatValue(value));
      setValidationError(t("panel.invalidRange", {
        min: minimum ?? "-∞",
        max: maximum ?? "∞",
      }));
    } else {
      onChange(parsed);
      setValidationError(null);
    }
    setFocused(false);
    onCommit();
  };

  const keyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") event.currentTarget.blur();
    if (event.key === "Escape") {
      setText(formatValue(value));
      event.currentTarget.blur();
    }
  };

  const pointerDown = (event: PointerEvent<HTMLButtonElement>) => {
    if (event.button !== 0) return;
    dragRef.current = { pointerId: event.pointerId, startX: event.clientX, startValue: value, dragging: false };
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };

  const pointerMove = (event: PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const deltaX = event.clientX - drag.startX;
    if (!drag.dragging) {
      if (Math.abs(deltaX) < 3) return;
      drag.dragging = true;
      onBegin();
      event.currentTarget.classList.add("dragging");
    }
    event.preventDefault();
    const next = scrubValue(drag.startValue, deltaX, mode, event.shiftKey, minimum, maximum);
    setValidationError(null);
    setText(formatValue(next));
    onChange(next);
  };

  const pointerEnd = (event: PointerEvent<HTMLButtonElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    event.currentTarget.classList.remove("dragging");
    dragRef.current = null;
    if (drag.dragging) onCommit();
  };

  const longLabel = label.length > 2;
  return <div className={`transform-field${longLabel ? " long-label" : ""}`}>
    <button
      className={`transform-scrubber${longLabel ? " long-label" : ""}`}
      type="button"
      title={t("panel.scrubTitle", { name })}
      aria-label={t("panel.scrubAria", { name })}
      onPointerDown={pointerDown}
      onPointerMove={pointerMove}
      onPointerUp={pointerEnd}
      onPointerCancel={pointerEnd}
    >{label}</button>
    <input
      type="text"
      inputMode="decimal"
      aria-label={name}
      aria-invalid={validationError !== null}
      className={validationError ? "invalid" : undefined}
      title={validationError ?? undefined}
      value={text}
      onFocus={() => { setFocused(true); setValidationError(null); onBegin(); }}
      onChange={(event) => {
        setText(event.target.value);
        const parsed = Number(event.target.value);
        if (Number.isFinite(parsed) && (minimum == null || parsed >= minimum) && (maximum == null || parsed <= maximum)) onChange(parsed);
      }}
      onBlur={commit}
      onKeyDown={keyDown}
    />
  </div>;
}

export function TransformPanel({ transform, onBegin, onChange, onCommit }: { transform: GaussianTransform; onBegin: () => void; onChange: (transform: GaussianTransform) => void; onCommit: () => void }) {
  const { t } = useI18n();
  const vectorField = (group: "position" | "rotation", index: 0 | 1 | 2, value: number) => {
    const axis = ["X", "Y", "Z"][index];
    const groupName = group === "position" ? t("panel.position") : t("panel.rotation");
    return <NumberField
      key={`${group}-${axis}`}
      label={axis}
      name={`${groupName} ${axis}`}
      value={value}
      mode={group === "rotation" ? "rotation" : "linear"}
      onBegin={onBegin}
      onCommit={onCommit}
      onChange={(next) => {
        const vector = [...transform[group]] as [number, number, number];
        vector[index] = next;
        onChange({ ...transform, [group]: vector });
      }}
    />;
  };

  return <aside className="transform-panel" aria-label={t("panel.modelTransform")}>
    <div className="transform-panel-heading"><strong>{t("viewer.transform")}</strong><small>{t("panel.dragHint")}</small></div>
    <section><h4>{t("panel.position")}</h4><div className="transform-fields">{vectorField("position", 0, transform.position[0])}{vectorField("position", 1, transform.position[1])}{vectorField("position", 2, transform.position[2])}</div></section>
    <section><h4>{t("panel.rotation")}</h4><div className="transform-fields">{vectorField("rotation", 0, transform.rotation[0])}{vectorField("rotation", 1, transform.rotation[1])}{vectorField("rotation", 2, transform.rotation[2])}</div><p>{t("panel.angle")}</p></section>
    <section><h4>{t("panel.scale")}</h4><NumberField label={t("panel.uniform")} name={t("panel.uniformScale")} value={transform.scale} mode="scale" min={TRANSFORM_SCALE_MIN} max={TRANSFORM_SCALE_MAX} onBegin={onBegin} onCommit={onCommit} onChange={(scale) => onChange({ ...transform, scale })} /></section>
  </aside>;
}
