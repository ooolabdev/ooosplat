import { Cpu, MemoryStick, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "../i18n";
import type { GpuInventory, GpuSelection } from "../types/pipeline";

export interface GpuSelectionResult {
  selection: GpuSelection;
  remember: boolean;
}

const selectionFor = (device: GpuInventory["devices"][number]): GpuSelection => ({
  localDeviceId: device.localDeviceId,
  deviceKind: device.deviceKind,
  selectionConfidence: device.selectionConfidence,
});

export function GpuSelectionDialog({ inventory, initialDeviceId, onFinish }: {
  inventory: GpuInventory;
  initialDeviceId?: string | null;
  onFinish: (result: GpuSelectionResult | null) => void;
}) {
  const { t } = useI18n();
  const initial = inventory.devices.find(device => device.localDeviceId === initialDeviceId)
    ?? inventory.devices.find(device => device.recommended)
    ?? inventory.devices[0];
  const [selected, setSelected] = useState(initial?.localDeviceId ?? "");
  const [remember, setRemember] = useState(false);
  const cancel = useRef<HTMLButtonElement>(null);
  const duplicateModels = useMemo(() => inventory.devices.some(device => device.duplicateModel), [inventory.devices]);

  useEffect(() => {
    cancel.current?.focus();
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onFinish(null);
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [onFinish]);

  const submit = () => {
    const device = inventory.devices.find(candidate => candidate.localDeviceId === selected);
    if (device) onFinish({ selection: selectionFor(device), remember });
  };

  return <div className="privacy-backdrop" role="presentation">
    <section className="gpu-selection-dialog" role="dialog" aria-modal="true" aria-labelledby="gpu-selection-title" aria-describedby="gpu-selection-description">
      <header className="settings-heading">
        <span className="settings-symbol"><Cpu size={19} /></span>
        <div><h2 id="gpu-selection-title">{t("gpu.selectTitle")}</h2><p id="gpu-selection-description">{t("gpu.selectDescription")}</p></div>
        <button type="button" aria-label={t("common.close")} onClick={() => onFinish(null)}><X size={18} /></button>
      </header>
      <div className="gpu-device-list" role="radiogroup" aria-label={t("gpu.selectTitle")}>
        {inventory.devices.map(device => <label key={device.localDeviceId} className={selected === device.localDeviceId ? "gpu-device-option selected" : "gpu-device-option"}>
          <input type="radio" name="generation-gpu" checked={selected === device.localDeviceId} onChange={() => setSelected(device.localDeviceId)} />
          <span className="gpu-device-main"><strong>{device.name}</strong><span>{device.vendor} · {device.deviceKind === "discreteGpu" ? t("gpu.discrete") : t("gpu.integrated")}</span>
            <small>{device.colmapCompatible ? t("gpu.enabledCompleted") : t("gpu.trainingOnly")}</small>
          </span>
          <span className="gpu-device-meta">{device.recommended && <em>{t("gpu.recommended")}</em>}<span><MemoryStick size={13} />{device.totalMemoryMb != null ? `${Math.round(device.totalMemoryMb / 1024)} GiB` : t("common.unavailable")}</span></span>
        </label>)}
      </div>
      {duplicateModels && <p className="gpu-order-warning">{t("gpu.duplicateWarning")}</p>}
      <label className="gpu-remember"><input type="checkbox" checked={remember} onChange={event => setRemember(event.target.checked)} />{t("gpu.remember")}</label>
      <footer><button ref={cancel} type="button" className="secondary-action" onClick={() => onFinish(null)}>{t("common.cancel")}</button><button type="button" className="primary-action" disabled={!selected} onClick={submit}>{t("generate.start")}</button></footer>
    </section>
  </div>;
}
