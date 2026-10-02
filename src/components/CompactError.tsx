import { useEffect, useId, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { localizePipelineMessage, useI18n } from "../i18n";
import { errorSummaryKey, shortErrorMessage } from "../lib/errorSummary";

export function CompactError({ message }: { message: string }) {
  const { locale, t } = useI18n();
  const id = useId();
  const anchor = useRef<HTMLSpanElement>(null);
  const tooltip = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [position, setPosition] = useState<CSSProperties | null>(null);
  const summaryKey = errorSummaryKey(message);
  const summary = summaryKey ? t(summaryKey) : shortErrorMessage(localizePipelineMessage(locale, message), locale === "en" ? 80 : 36);

  const cancelClose = () => {
    if (closeTimer.current !== null) clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  const scheduleClose = () => {
    cancelClose();
    closeTimer.current = setTimeout(() => setPosition(null), 150);
  };
  const show = () => {
    cancelClose();
    const rect = anchor.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(520, window.innerWidth - 24);
    const below = window.innerHeight - rect.bottom;
    const above = rect.top;
    const useBelow = below >= 140 || below >= above;
    setPosition({
      width, left: Math.max(12, Math.min(rect.left, window.innerWidth - width - 12)),
      ...(useBelow ? { top: rect.bottom + 6 } : { bottom: window.innerHeight - rect.top + 6 }),
      maxHeight: Math.max(48, Math.min(320, (useBelow ? below : above) - 18)),
    });
  };

  useEffect(() => () => {
    if (closeTimer.current !== null) clearTimeout(closeTimer.current);
  }, []);
  const open = position !== null;
  useEffect(() => {
    if (!open) return;
    const close = () => setPosition(null);
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    const scroll = (event: Event) => {
      if (event.target instanceof Node && tooltip.current?.contains(event.target)) return;
      close();
    };
    window.addEventListener("keydown", escape);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", scroll, true);
    return () => {
      window.removeEventListener("keydown", escape);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", scroll, true);
    };
  }, [open]);

  return <>
    <span ref={anchor} className="compact-error" tabIndex={0} aria-describedby={open ? id : undefined} aria-label={`${summary} · ${t("error.detailsHint")}`} onMouseEnter={show} onMouseLeave={scheduleClose} onFocus={show} onBlur={scheduleClose}>
      {summary}
    </span>
    {position && createPortal(<div ref={tooltip} id={id} role="tooltip" tabIndex={0} className="error-detail-tooltip" style={position} onMouseEnter={cancelClose} onMouseLeave={scheduleClose} onFocus={cancelClose} onBlur={scheduleClose}>{message}</div>, document.body)}
  </>;
}
