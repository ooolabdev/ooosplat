import { Monitor, Settings2, ShieldCheck, X } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useI18n } from "../i18n";
import type { TelemetryPreferences } from "../types/telemetry";
import { PrivacySettingsPanel } from "./TelemetryPreferences";

type SettingsSection = "interface" | "privacy";
const sections: SettingsSection[] = ["interface", "privacy"];

export function SettingsDialog({ preferences, telemetryBusy, showRuntimePanel, onTelemetryChange, onRuntimePanelChange, onClose }: {
  preferences: TelemetryPreferences | null;
  telemetryBusy: boolean;
  showRuntimePanel: boolean;
  onTelemetryChange: (enabled: boolean) => void;
  onRuntimePanelChange: (enabled: boolean) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [section, setSection] = useState<SettingsSection>("interface");
  const tabs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    tabs.current[0]?.focus();
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);

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
        <div><small>{t("settings.path")}</small><h2 id="settings-title">{t("settings.title")}</h2></div>
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
          >{item === "interface" ? <Monitor size={15} /> : <ShieldCheck size={15} />}<span>{t(item === "interface" ? "settings.interface" : "settings.privacy")}</span></button>)}
        </nav>
        <div className="settings-content">
          {section === "interface" && <section id="settings-panel-interface" role="tabpanel" aria-labelledby="settings-tab-interface">
            <div className="settings-section-heading"><h3>{t("settings.interface")}</h3><p>{t("settings.interfaceHint")}</p></div>
            <div className="settings-row">
              <div><strong>{t("settings.runtimePanel")}</strong><p>{t("settings.runtimePanelHint")}</p></div>
              <button type="button" role="switch" aria-checked={showRuntimePanel} aria-label={t("settings.runtimePanel")} className={showRuntimePanel ? "settings-switch enabled" : "settings-switch"} onClick={() => onRuntimePanelChange(!showRuntimePanel)}><span /></button>
            </div>
          </section>}
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
