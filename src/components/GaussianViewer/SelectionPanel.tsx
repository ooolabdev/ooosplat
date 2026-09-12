import type { GaussianCrop } from "../../types/pipeline";
import { useI18n } from "../../i18n";
import { NumberField } from "./TransformPanel";

export function SelectionPanel({ crop, kind, onBegin, onChange, onCommit, onEnable }: {
  crop: GaussianCrop;
  kind: "sphere" | "box";
  onBegin: () => void;
  onChange: (crop: Exclude<GaussianCrop, null>) => void;
  onCommit: () => void;
  onEnable: () => void;
}) {
  const { t } = useI18n();
  const activeCrop = crop?.kind === kind ? crop : null;
  const kindLabel = kind === "sphere" ? t("panel.sphere") : t("panel.box");

  const centerField = (index: 0 | 1 | 2) => {
    if (!activeCrop) return null;
    const axis = ["X", "Y", "Z"][index];
    return <NumberField key={axis} label={axis} name={t("panel.regionPosition", { axis })} value={activeCrop.center[index]} onBegin={onBegin} onCommit={onCommit} onChange={(value) => {
      const center = [...activeCrop.center] as [number, number, number];
      center[index] = value;
      onChange({ ...activeCrop, center });
    }} />;
  };

  const sizeField = (index: 0 | 1 | 2) => {
    if (activeCrop?.kind !== "box") return null;
    const axis = ["X", "Y", "Z"][index];
    return <NumberField key={axis} label={axis} name={t("panel.boxSize", { axis })} value={activeCrop.size[index]} mode="scale" onBegin={onBegin} onCommit={onCommit} onChange={(value) => {
      const size = [...activeCrop.size] as [number, number, number];
      size[index] = value;
      onChange({ ...activeCrop, size });
    }} />;
  };

  return <aside className={`transform-panel selection-panel ${activeCrop ? "enabled" : "disabled"}`} aria-label={t("panel.region")}>
    <div className="transform-panel-heading">
      <strong>{t("panel.region")}</strong>
      <small>{t("panel.keepInside", { kind: kindLabel })}</small>
    </div>
    {activeCrop ? <>
      <section><h4>{t("panel.position")}</h4><div className="transform-fields">{centerField(0)}{centerField(1)}{centerField(2)}</div></section>
      <section><h4>{activeCrop.kind === "sphere" ? t("panel.radius") : t("panel.size")}</h4>{activeCrop.kind === "sphere"
        ? <NumberField label="R" name={t("panel.sphereRadius")} value={activeCrop.radius} mode="scale" onBegin={onBegin} onCommit={onCommit} onChange={(radius) => onChange({ ...activeCrop, radius })} />
        : <div className="transform-fields">{sizeField(0)}{sizeField(1)}{sizeField(2)}</div>}</section>
    </> : <div className="selection-empty">
      <p>{t("panel.noCrop", { kind: kindLabel })}</p>
      <button type="button" onClick={onEnable}>{t("panel.enableCrop", { kind: kindLabel })}</button>
    </div>}
  </aside>;
}
