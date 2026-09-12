import { ShieldCheck, X } from "lucide-react";
import { useI18n } from "../i18n";
import type { TelemetryPreferences as Preferences } from "../types/telemetry";

interface TelemetryPreferencesProps {
  mode: "consent" | "settings";
  preferences: Preferences;
  busy: boolean;
  onChange: (enabled: boolean) => void;
  onClose?: () => void;
}

export function TelemetryPreferences({ mode, preferences, busy, onChange, onClose }: TelemetryPreferencesProps) {
  const { t } = useI18n();
  const consent = mode === "consent";
  return <div className="privacy-backdrop" role="presentation">
    <section className={`privacy-dialog ${consent ? "consent" : "settings"}`} role="dialog" aria-modal="true" aria-labelledby="privacy-title">
      <header className="privacy-heading">
        <span className="privacy-symbol"><ShieldCheck size={20} /></span>
        <div>
          <small>{consent ? t("privacy.firstUse") : t("privacy.settingsPath")}</small>
          <h2 id="privacy-title">{consent ? t("privacy.improve") : t("privacy.title")}</h2>
        </div>
        {!consent && <button className="privacy-close" type="button" aria-label={t("privacy.closeAria")} disabled={busy} onClick={onClose}><X size={18} /></button>}
      </header>

      {consent ? <>
        <p className="privacy-intro">{t("privacy.intro")}</p>
        <div className="privacy-columns">
          <div><strong>{t("privacy.mayCollect")}</strong><ul><li>{t("privacy.collect1")}</li><li>{t("privacy.collect2")}</li><li>{t("privacy.collect3")}</li><li>{t("privacy.collect4")}</li></ul></div>
          <div className="never"><strong>{t("privacy.neverCollect")}</strong><ul><li>{t("privacy.never1")}</li><li>{t("privacy.never2")}</li><li>{t("privacy.never3")}</li><li>{t("privacy.never4")}</li></ul></div>
        </div>
        <p className="privacy-footnote">{t("privacy.uuid")}</p>
        {preferences.deliveryStatus === "notConfigured" && <p className="privacy-delivery-note">{t("privacy.noEndpoint")}</p>}
        {preferences.deliveryStatus === "debug" && <p className="privacy-delivery-note">{t("privacy.debug")}</p>}
        <div className="privacy-actions">
          <button className="privacy-secondary" type="button" disabled={busy} onClick={() => onChange(false)}>{t("privacy.decline")}</button>
          <button className="privacy-primary" type="button" disabled={busy} onClick={() => onChange(true)}>{busy ? t("privacy.saving") : t("privacy.share")}</button>
        </div>
      </> : <>
        <div className="privacy-setting-row">
          <div><strong>{t("privacy.analytics")}</strong><p>{t("privacy.analyticsHint")}</p></div>
          <button
            type="button"
            role="switch"
            aria-checked={preferences.analyticsEnabled}
            aria-label={t("privacy.analytics")}
            className={preferences.analyticsEnabled ? "privacy-switch enabled" : "privacy-switch"}
            disabled={busy}
            onClick={() => onChange(!preferences.analyticsEnabled)}
          ><span /></button>
        </div>
        <div className="privacy-summary">
          <p><b>{t("privacy.collectSummary")}</b>{t("privacy.collectSummaryText")}</p>
          <p><b>{t("privacy.noCollectSummary")}</b>{t("privacy.noCollectSummaryText")}</p>
          {preferences.deliveryStatus === "notConfigured" && <p><b>{t("privacy.network")}</b>{t("privacy.networkOff")}</p>}
          {preferences.deliveryStatus === "debug" && <p><b>{t("privacy.network")}</b>{t("privacy.networkDebug")}</p>}
        </div>
      </>}
    </section>
  </div>;
}
