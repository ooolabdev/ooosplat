import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, CheckCircle2, LoaderCircle, Send, ShieldCheck, X } from "lucide-react";
import { useI18n, type TranslationKey } from "../i18n";
import { prepareErrorReport, sendErrorReport } from "../lib/backend";
import type { ErrorReportDraft } from "../types/diagnostics";

const errorKey = (error: unknown): TranslationKey => {
  const code = typeof error === "object" && error !== null && "code" in error ? String(error.code) : "";
  return code === "report_expired" ? "report.expired" : code === "report_busy" ? "report.busy" : code === "report_too_large" ? "report.tooLarge" : "report.unavailable";
};

export function ErrorReportDialog({ failureId, onBack, onSent }: { failureId: string; onBack: () => void; onSent?: () => void }) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<ErrorReportDraft | null>(null);
  const [phase, setPhase] = useState<"preparing" | "preview" | "sending" | "sent" | "error">("preparing");
  const [problem, setProblem] = useState<TranslationKey | null>(null);
  const generation = useRef(0);
  const dialogRef = useRef<HTMLDivElement>(null);

  const prepare = useCallback(async () => {
    const request = ++generation.current;
    setPhase("preparing"); setProblem(null); setDraft(null);
    try {
      const next = await prepareErrorReport(failureId);
      if (generation.current !== request) return;
      setDraft(next); setPhase("preview");
    } catch (error) {
      if (generation.current !== request) return;
      setProblem(errorKey(error)); setPhase("error");
    }
  }, [failureId]);
  useEffect(() => {
    dialogRef.current?.focus();
    void prepare();
    return () => { generation.current += 1; };
    // The failure identity, not the UI language, owns this immutable preview.
  }, [prepare]);
  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && phase !== "sending") { event.stopImmediatePropagation(); onBack(); }
      if (event.key === "Tab") {
        const targets = dialogRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled), [tabindex='0']");
        if (!targets?.length) return;
        const first = targets[0], last = targets[targets.length - 1];
        if (event.shiftKey && (document.activeElement === first || document.activeElement === dialogRef.current)) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && (document.activeElement === last || document.activeElement === dialogRef.current)) { event.preventDefault(); first.focus(); }
      }
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  }, [onBack, phase]);

  const send = async () => {
    if (!draft || phase === "sending" || phase === "sent") return;
    const request = ++generation.current;
    setPhase("sending"); setProblem(null);
    try {
      const receipt = await sendErrorReport(draft.draftId);
      if (generation.current !== request) return;
      if (!receipt.accepted || receipt.reportId !== draft.report.reportId) throw new Error("Invalid receipt");
      setPhase("sent");
      onSent?.();
    } catch (error) {
      if (generation.current !== request) return;
      const key = errorKey(error);
      if (key === "report.expired") setDraft(null);
      setProblem(key); setPhase("error");
    }
  };
  const disabled = phase === "preparing" || phase === "sending";
  return <div ref={dialogRef} tabIndex={-1} className="failure-guidance-backdrop" role="dialog" aria-modal="true" aria-labelledby="error-report-title" aria-describedby="error-report-privacy">
    <section className="failure-guidance-dialog error-report-dialog">
      <div className="failure-guidance-heading">
        <span className="report-shield"><ShieldCheck size={22} /></span>
        <div><small>OOOSplat</small><h2 id="error-report-title">{t("report.title")}</h2></div>
        <button type="button" disabled={phase === "sending"} aria-label={t("common.close")} onClick={onBack}><X size={17} /></button>
      </div>
      <p id="error-report-privacy">{t("report.privacy")}</p>
      <p className="report-retention">{t("report.retention")}</p>
      {phase === "preparing" && <p role="status"><LoaderCircle className="spin" size={14} /> {t("report.preparing")}</p>}
      {draft && <>
        <div className="report-summary"><span>{t("report.system")}</span><strong>{draft.report.environment.system.name} {draft.report.environment.system.version ?? ""} · {draft.report.environment.system.arch}</strong><span>{t("report.gpus")}</span><strong>{draft.report.environment.gpus.map(gpu => gpu.name).join(" / ") || t("report.unknown")}</strong></div>
        <strong>{t("report.exactPreview")}</strong>
        <pre className="report-preview" tabIndex={0} aria-label={t("report.exactPreview")}>{JSON.stringify(draft.report, null, 2)}</pre>
        {draft.report.logsTruncated && <p>{t("report.truncated")}</p>}
      </>}
      {problem && <p className="report-problem" role="alert">{t(problem)}</p>}
      {phase === "sent" && <p className="report-success" role="status"><CheckCircle2 size={16} /> {t("report.sent", { id: draft?.report.reportId ?? "" })}</p>}
      <div className="failure-guidance-actions">
        <button type="button" className="secondary" disabled={phase === "sending"} onClick={onBack}><ArrowLeft size={14} />{t(phase === "sent" ? "common.close" : "common.cancel")}</button>
        {phase !== "sent" && <button type="button" className="primary" disabled={disabled} onClick={() => void (draft ? send() : prepare())}>{disabled ? <LoaderCircle className="spin" size={14} /> : <Send size={14} />}{t(phase === "sending" ? "report.sending" : draft ? "report.agreeSend" : "common.retry")}</button>}
      </div>
    </section>
  </div>;
}
