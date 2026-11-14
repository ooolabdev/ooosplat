import { CircleAlert, FileBox, LoaderCircle, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { beginGaussianImport, cancelGaussianImport, completeGaussianImport, probeGaussianImport } from "../lib/backend";
import { convertGaussianImport } from "../lib/gaussianImport";
import { formatBinarySize } from "../lib/fileSize";
import type { GaussianImportProbe, ProjectSummary } from "../types/pipeline";
import { localizePipelineMessage, useI18n } from "../i18n";
import { inputAdmissionReason, type InputAdmissionReason } from "../lib/inputAdmission";

interface Props {
  selectionId: string;
  onClose: () => void;
  onImported: (project: ProjectSummary) => void;
  onUnsupportedInput: (reason: InputAdmissionReason) => void;
}

const formatBytes = (bytes: number, locale: string) => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  return formatBinarySize(bytes, locale);
};

const messageOf = (error: unknown) => error instanceof Error ? error.message : String(error);

export function GaussianImportDialog({ selectionId, onClose, onImported, onUnsupportedInput }: Props) {
  const { locale, t } = useI18n();
  const [probe, setProbe] = useState<GaussianImportProbe | null>(null);
  const [modelId, setModelId] = useState("");
  const [name, setName] = useState("");
  const [phase, setPhase] = useState<"analysis" | "review" | "conversion" | "validation" | "registration">("analysis");
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const importId = useRef<string | null>(null);
  const cancelConversion = useRef<null | (() => void)>(null);
  const dialog = useRef<HTMLElement>(null);
  const mounted = useRef(true);
  const operationRevision = useRef(0);
  const startingRevision = useRef<number | null>(null);
  const unsupportedHandler = useRef(onUnsupportedInput);
  const busy = phase !== "review" && probe !== null;

  useEffect(() => { unsupportedHandler.current = onUnsupportedInput; }, [onUnsupportedInput]);

  useEffect(() => {
    mounted.current = true;
    const revision = ++operationRevision.current;
    setProbe(null);
    setModelId("");
    setError(null);
    setPhase("analysis");
    setProgress(0);
    dialog.current?.focus();
    let disposed = false;
    void probeGaussianImport(selectionId).then(value => {
      if (disposed || !mounted.current || operationRevision.current !== revision) return;
      setProbe(value);
      setModelId(value.defaultModelId ?? value.models[0]?.id ?? "");
      setName(value.suggestedName);
      setPhase("review");
      setProgress(0);
    }).catch(reason => {
      if (disposed || !mounted.current || operationRevision.current !== revision) return;
      const rejected = inputAdmissionReason(reason);
      if (rejected) {
        mounted.current = false;
        unsupportedHandler.current(rejected);
      } else {
        setError(localizePipelineMessage(locale, messageOf(reason)));
      }
    });
    return () => {
      disposed = true;
      mounted.current = false;
      cancelConversion.current?.();
      const activeImportId = importId.current;
      importId.current = null;
      if (activeImportId) void cancelGaussianImport(activeImportId).catch(() => undefined);
    };
  }, [locale, selectionId]);

  const model = probe?.models.find(candidate => candidate.id === modelId) ?? null;
  const start = async () => {
    if (!probe || !model || busy || !mounted.current) return;
    const revision = operationRevision.current;
    if (startingRevision.current === revision) return;
    startingRevision.current = revision;
    const isCurrent = () => mounted.current && operationRevision.current === revision;
    setError(null);
    setPhase("analysis");
    setProgress(0.02);
    let activeImportId: string | null = null;
    try {
      const session = await beginGaussianImport({ probeId: probe.probeId, modelId, colmapId: null, projectName: name });
      if (!isCurrent()) {
        await cancelGaussianImport(session.importId).catch(() => undefined);
        return;
      }
      activeImportId = session.importId;
      importId.current = session.importId;
      const conversion = convertGaussianImport(session, update => {
        if (!isCurrent()) return;
        setPhase(update.phase);
        setProgress(update.progress);
      });
      cancelConversion.current = conversion.cancel;
      const info = await conversion.result;
      if (!isCurrent()) return;
      cancelConversion.current = null;
      setPhase("validation");
      setProgress(0.9);
      const project = await completeGaussianImport({ importId: session.importId, ...info });
      if (!isCurrent()) return;
      importId.current = null;
      setPhase("registration");
      setProgress(1);
      onImported(project);
    } catch (reason) {
      if (!isCurrent()) return;
      if (activeImportId) await cancelGaussianImport(activeImportId).catch(() => undefined);
      if (!isCurrent()) return;
      cancelConversion.current = null;
      importId.current = null;
      const rejected = inputAdmissionReason(reason);
      if (rejected) {
        mounted.current = false;
        unsupportedHandler.current(rejected);
        return;
      }
      setError(localizePipelineMessage(locale, messageOf(reason)));
      try {
        const refreshed = await probeGaussianImport(selectionId);
        if (!isCurrent()) return;
        setProbe(refreshed);
        setModelId(refreshed.defaultModelId ?? refreshed.models[0]?.id ?? "");
      } catch (refreshError) {
        if (!isCurrent()) return;
        const rejected = inputAdmissionReason(refreshError);
        if (rejected) {
          mounted.current = false;
          unsupportedHandler.current(rejected);
          return;
        }
        setProbe(null);
      }
      setPhase("review");
    } finally {
      if (startingRevision.current === revision) startingRevision.current = null;
    }
  };
  const close = async () => {
    if (!mounted.current) return;
    mounted.current = false;
    operationRevision.current += 1;
    cancelConversion.current?.();
    if (importId.current) await cancelGaussianImport(importId.current).catch(() => undefined);
    importId.current = null;
    onClose();
  };
  const phaseLabel = useMemo(() => ({
    analysis: t("import.phase.analysis"), conversion: t("import.phase.conversion"), validation: t("import.phase.validation"), registration: t("import.phase.registration"), review: "",
  })[phase], [phase, t]);

  return <div className="privacy-backdrop gaussian-import-backdrop" role="presentation">
    <section ref={dialog} tabIndex={-1} className="gaussian-import-dialog" role="dialog" aria-modal="true" aria-labelledby="gaussian-import-title" onKeyDown={event => { if (event.key === "Escape") void close(); }}>
      <header><div><small>{t("import.eyebrow")}</small><h2 id="gaussian-import-title">{t("import.title")}</h2></div><button type="button" aria-label={t("common.close")} onClick={() => void close()}><X size={18} /></button></header>
      {!probe && !error && <div className="import-loading"><LoaderCircle className="spin" size={22} /><span>{t("import.scanning")}</span></div>}
      {error && <p className="import-error" role="alert"><CircleAlert size={16} />{error}</p>}
      {probe && <div className="gaussian-import-body">
        <label className="import-name"><span>{t("import.projectName")}</span><input value={name} maxLength={64} disabled={busy} onChange={event => setName(event.target.value)} /></label>
        <div className="import-model-choice">
          <label style={{ gridColumn: "1 / -1" }}><span>{t("import.model")}</span><select value={modelId} disabled={busy} onChange={event => setModelId(event.target.value)}>{probe.models.map(candidate => <option key={candidate.id} value={candidate.id}>{candidate.name}</option>)}</select></label>
        </div>
        <div className="import-summary-grid">
          <article style={{ gridColumn: "1 / -1" }}><FileBox size={18} /><small>{t("import.model")}</small><strong>{model?.format ?? "—"}</strong><span>{formatBytes(model?.fileSize ?? 0, locale)} · {model?.splatCount?.toLocaleString(locale) ?? t("import.readOnImport")}</span></article>
        </div>
        {model && !model.directPly && <p className="import-storage-note">{t("import.conversionEstimate", { size: formatBytes(Math.ceil(model.fileSize * 1.5), locale) })}</p>}
        {busy && <div className="import-progress" aria-live="polite"><div><span>{phaseLabel}</span><b>{Math.round(progress * 100)}%</b></div><progress max={1} value={progress} /></div>}
      </div>}
      <footer><button type="button" onClick={() => void close()}>{t("common.cancel")}</button><button className="primary-action" type="button" disabled={!probe || !model || !name.trim() || busy} onClick={() => void start()}>{busy ? <LoaderCircle className="spin" size={15} /> : null}{t("import.confirm")}</button></footer>
    </section>
  </div>;
}
