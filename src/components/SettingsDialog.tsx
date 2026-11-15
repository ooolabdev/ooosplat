import { Cpu, Monitor, RefreshCw, Settings2, ShieldCheck, X } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useI18n } from "../i18n";
import type { TelemetryPreferences } from "../types/telemetry";
import { PrivacySettingsPanel } from "./TelemetryPreferences";

import { McpSettingsPanel } from "./McpSettingsPanel";
import { getGpuSettings, setGpuSettings } from "../lib/backend";
import type { HardwareSettingsView } from "../types/pipeline";

type SettingsSection = "interface" | "hardware" | "updates" | "privacy" | "mcp";
const sections: SettingsSection[] = ["interface", "hardware", "updates", "privacy", "mcp"];

export function SettingsDialog({ preferences, telemetryBusy, showRuntimePanel, onTelemetryChange, onRuntimePanelChange, onClose, updatePanel }: {
  updatePanel?: ReactNode;
  preferences: TelemetryPreferences | null;
  telemetryBusy: boolean;
  showRuntimePanel: boolean;
  onTelemetryChange: (enabled: boolean) => void;
  onRuntimePanelChange: (enabled: boolean) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [section, setSection] = useState<SettingsSection>("interface");
  const [hardware, setHardware] = useState<HardwareSettingsView | null>(null);
  const [hardwareBusy, setHardwareBusy] = useState(false);
  const [hardwareError, setHardwareError] = useState<string | null>(null);
  const tabs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    tabs.current[0]?.focus();
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);

  useEffect(() => {
    if (section !== "hardware" || hardware || hardwareBusy) return;
    setHardwareBusy(true);
    void getGpuSettings(true).then(setHardware).catch(error => setHardwareError(String(error))).finally(() => setHardwareBusy(false));
  }, [hardware, hardwareBusy, section]);

  const saveHardware = async (next: HardwareSettingsView["settings"]) => {
    setHardwareBusy(true); setHardwareError(null);
    try { setHardware(await setGpuSettings(next)); }
    catch (error) { setHardwareError(String(error)); }
    finally { setHardwareBusy(false); }
  };

  const selectFromKeyboard = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let next = index;
    if (event.key === "ArrowDown") next = (index + 1) % sections.length;
    else if (event.key === "ArrowUp") next = (index - 1 + sections.length) % sections.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = sections.length - 1;
    else return;
    event.preventDefault();
    setSection(sections[next]);
    tabs.current[next]?.focus();
  };

  return <div className="privacy-backdrop" role="presentation">
    <section className="settings-dialog" role="dialog" aria-modal="true" aria-labelledby="settings-title">
      <header className="settings-heading">
        <span className="settings-symbol"><Settings2 size={19} /></span>
        <div><small>{t("settings.title")} / {section === "mcp" ? "MCP" : t(section === "interface" ? "settings.interface" : section === "hardware" ? "settings.hardware" : section === "updates" ? "update.settingsTitle" : "settings.privacy")}</small><h2 id="settings-title">{t("settings.title")}</h2></div>
        <button type="button" aria-label={t("settings.closeAria")} onClick={onClose}><X size={18} /></button>
      </header>
      <div className="settings-layout">
        <nav className="settings-sidebar" aria-label={t("settings.categories")} role="tablist" aria-orientation="vertical">
          {sections.map((item, index) => <button
            key={item}
            ref={(node) => { tabs.current[index] = node; }}
            type="button"
            role="tab"
            id={`settings-tab-${item}`}
            aria-selected={section === item}
            aria-controls={`settings-panel-${item}`}
            tabIndex={section === item ? 0 : -1}
            onClick={() => setSection(item)}
            onKeyDown={(event) => selectFromKeyboard(event, index)}
          >{item === "interface" ? <Monitor size={15} /> : item === "hardware" ? <Cpu size={15} /> : item === "updates" ? <RefreshCw size={15} /> : <ShieldCheck size={15} />}<span>{item === "mcp" ? "MCP" : t(item === "interface" ? "settings.interface" : item === "hardware" ? "settings.hardware" : item === "updates" ? "update.settingsTitle" : "settings.privacy")}</span></button>)}
        </nav>
        <div className="settings-content">
          {section === "interface" && <section id="settings-panel-interface" role="tabpanel" aria-labelledby="settings-tab-interface">
            <div className="settings-section-heading"><h3>{t("settings.interface")}</h3><p>{t("settings.interfaceHint")}</p></div>
            <div className="settings-row">
              <div><strong>{t("settings.runtimePanel")}</strong><p>{t("settings.runtimePanelHint")}</p></div>
              <button type="button" role="switch" aria-checked={showRuntimePanel} aria-label={t("settings.runtimePanel")} className={showRuntimePanel ? "settings-switch enabled" : "settings-switch"} onClick={() => onRuntimePanelChange(!showRuntimePanel)}><span /></button>
            </div>
          </section>}
          {section === "updates" && <section id="settings-panel-updates" role="tabpanel" aria-labelledby="settings-tab-updates">
            {updatePanel}
          </section>}
          {section === "hardware" && <section id="settings-panel-hardware" role="tabpanel" aria-labelledby="settings-tab-hardware">
            <div className="settings-section-heading"><h3>{t("settings.hardware")}</h3><p>{t("settings.hardwareHint")}</p></div>
            {hardware ? <>
              <div className="settings-row settings-row-stacked"><div><strong>{t("settings.defaultGpu")}</strong><p>{t("settings.defaultGpuHint")}</p></div>
                <select disabled={hardwareBusy} value={hardware.settings.defaultGpu?.localDeviceId ?? ""} onChange={event => {
                  const device = hardware.inventory.devices.find(item => item.localDeviceId === event.target.value);
                  if (device) void saveHardware({ ...hardware.settings, defaultGpu: { localDeviceId: device.localDeviceId, deviceKind: device.deviceKind, selectionConfidence: device.selectionConfidence } });
                }}>
                  <option value="" disabled>{hardware.inventory.devices.find(item => item.recommended)?.name ? `${t("gpu.recommended")}: ${hardware.inventory.devices.find(item => item.recommended)!.name}` : t("common.unavailable")}</option>
                  {hardware.inventory.devices.map(device => <option key={device.localDeviceId} value={device.localDeviceId}>{device.name}{device.recommended ? ` · ${t("gpu.recommended")}` : ""}</option>)}
                </select>
              </div>
              <div className="settings-row"><div><strong>{t("settings.askGpu")}</strong><p>{t("settings.askGpuHint")}</p></div>
                <button type="button" role="switch" aria-checked={hardware.settings.askOnMultipleGpus} aria-label={t("settings.askGpu")} disabled={hardwareBusy || (hardware.settings.askOnMultipleGpus && !hardware.settings.defaultGpu)} className={hardware.settings.askOnMultipleGpus ? "settings-switch enabled" : "settings-switch"} onClick={() => void saveHardware({ ...hardware.settings, askOnMultipleGpus: !hardware.settings.askOnMultipleGpus })}><span /></button>
              </div>
            </> : hardwareBusy ? <p className="settings-unavailable">{t("gpu.scanningDevices")}</p> : null}
            {hardwareError && <p className="settings-unavailable">{hardwareError}</p>}
          </section>}
          {section === "mcp" && <section id="settings-panel-mcp" role="tabpanel" aria-labelledby="settings-tab-mcp"><McpSettingsPanel /></section>}
          {section === "privacy" && <section id="settings-panel-privacy" role="tabpanel" aria-labelledby="settings-tab-privacy">
            <div className="settings-section-heading"><h3>{t("settings.privacy")}</h3><p>{t("settings.privacyHint")}</p></div>
            {preferences
              ? <PrivacySettingsPanel preferences={preferences} busy={telemetryBusy} onChange={onTelemetryChange} />
              : <p className="settings-unavailable">{t("settings.privacyUnavailable")}</p>}
          </section>}
        </div>
      </div>
    </section>
  </div>;
}
