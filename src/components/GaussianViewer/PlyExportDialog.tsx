import { Download, FolderOpen, LoaderCircle, TriangleAlert, X } from "lucide-react";
import { useEffect, useRef } from "react";
import { useI18n } from "../../i18n";
import type { ModelFormat, SpzAdjustment } from "../../lib/modelExportTypes";
import { formatBinarySize } from "../../lib/fileSize";

export type PlyExportDialogState =
  | { phase: "closed" | "preparing" | "encoding" | "validating" | "publishing" | "cancelling" }
  | { phase: "awaitingConfirmation"; exportId: string; confirmationId: number; adjustment: SpzAdjustment }
  | { phase: "completed"; path: string; fileSize?: number; revealError?: string; coordinateAdjustment?: SpzAdjustment }
  | { phase: "error"; error: string };
interface Props {
  state: PlyExportDialogState; progress: number | null; format?: ModelFormat;
  onClose: () => void; onRetry: () => void; onReveal: (path: string) => void; onCancel?: () => void;
  onConfirmAdjustment?: (exportId: string, confirmationId: number) => void;
}
export function PlyExportDialog({ state, progress, format = "ply", onClose, onRetry, onReveal, onCancel, onConfirmAdjustment }: Props) {
  const { t, locale } = useI18n();
  const dialog = useRef<HTMLDivElement>(null), visible = state.phase !== "closed";
  const cancelButton = useRef<HTMLButtonElement>(null), waiting = state.phase === "awaitingConfirmation";
  useEffect(() => {
    if (!visible) return;
    const previous = document.activeElement; dialog.current?.focus();
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, [visible]);
  useEffect(() => { if (visible && dialog.current && !dialog.current.contains(document.activeElement)) dialog.current.focus(); }, [visible, state.phase]);
  useEffect(() => { if (waiting) cancelButton.current?.focus(); }, [waiting]);
  if (state.phase === "closed") return null;
  const active = state.phase !== "completed" && state.phase !== "error";
  const title = t(waiting ? "export.spzAdjustTitle" : state.phase === "completed" ? "export.modelCompleted" : state.phase === "error" ? "export.modelFailed" : "export.model");
  return <div className="gaussian-save-backdrop" ref={dialog} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby="model-export-title" aria-describedby="model-export-description" onKeyDown={event => {
    if (event.key === "Escape") { event.stopPropagation(); if (waiting) onCancel?.(); else if (!active) onClose(); }
    if (event.key === "Tab") {
      const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
      const first = buttons[0], last = buttons.at(-1);
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === dialog.current)) { event.preventDefault(); first.focus(); }
    }
  }}>
    <div className={`gaussian-save-dialog gaussian-export-dialog${waiting ? " awaiting-confirmation" : ""}`}>
      <div className={`gaussian-save-symbol ${state.phase}`}>{waiting ? <TriangleAlert size={19} /> : active ? <LoaderCircle className="spin" size={19} /> : state.phase === "completed" ? <FolderOpen size={19} /> : <X size={19} />}</div>
      <div id="model-export-description">
        <h2 id="model-export-title">{title}</h2>
        <p>{t(`export.format.${format}`)}</p>
        {waiting && <>{(["export.spzAdjustReason", "export.spzAdjustAction", "export.spzAdjustImpact", "export.spzAdjustAdvice"] as const).map(key => <p key={key}>{t(key)}</p>)}</>}
        {state.phase === "preparing" && <p>{t("export.plyPreparing")}</p>}
        {["encoding", "validating", "publishing", "cancelling"].includes(state.phase) && <p role="status">{t(state.phase === "encoding" ? "export.modelEncoding" : state.phase === "cancelling" ? "export.modelCancelling" : "export.modelValidating")}</p>}
        {["preparing", "encoding"].includes(state.phase) && <div className="gaussian-export-progress" role="status"><progress max={100} value={progress ?? undefined} /><b>{progress == null ? "…" : `${Math.floor(progress)}%`}</b></div>}
        {state.phase === "completed" && <>
          <p>{t("export.plyCompletedDescription")}</p><code className="gaussian-export-path">{state.path}</code>
          {state.fileSize != null && <p>{formatBinarySize(state.fileSize, locale)}</p>}
          {state.coordinateAdjustment && <>
            <p>{t("export.spzAdjustment")}: <code>p' = (p - c) × s</code></p>
            <p>{t("export.spzAdjustmentCenter")}<code className="gaussian-export-path">c = [{state.coordinateAdjustment.center.join(", ")}]</code></p>
            <p>{t("export.spzAdjustmentScale")}: <code>s = {String(state.coordinateAdjustment.scale)}</code></p>
          </>}
          {state.revealError && <p className="gaussian-export-error">{state.revealError}</p>}
        </>}
        {state.phase === "error" && <p className="gaussian-export-error">{state.error.includes("SPZ_SCALE_RANGE") ? t("export.spzScaleRange") : state.error.includes("SPZ_POSITION_RANGE") ? t("export.spzRange") : `${t("export.modelFailed")}: ${state.error}`}</p>}
      </div>
      {active && onCancel && <div className="gaussian-save-actions"><button ref={cancelButton} type="button" className="secondary" disabled={state.phase === "publishing" || state.phase === "cancelling"} onClick={onCancel}>{t("common.cancel")}</button>{state.phase === "awaitingConfirmation" && <button type="button" className="primary" onClick={() => onConfirmAdjustment?.(state.exportId, state.confirmationId)}>{t("export.spzAdjustConfirm")}</button>}</div>}
      {state.phase === "completed" && <div className="gaussian-save-actions"><button autoFocus type="button" className="secondary" onClick={onClose}>{t("common.close")}</button><button type="button" className="primary" onClick={() => onReveal(state.path)}><FolderOpen size={14} />{t("export.openFolder")}</button></div>}
      {state.phase === "error" && <div className="gaussian-save-actions"><button autoFocus type="button" className="secondary" onClick={onClose}>{t("common.close")}</button><button type="button" className="primary" onClick={onRetry}><Download size={14} />{t("export.plyRetry")}</button></div>}
    </div>
  </div>;
}
