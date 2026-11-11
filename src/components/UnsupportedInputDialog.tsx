import { useEffect, useId, useRef, type KeyboardEvent } from "react";
import { X } from "lucide-react";
import { useI18n } from "../i18n";
import type { InputAdmissionReason } from "../lib/inputAdmission";

interface Props {
  reason: InputAdmissionReason;
  onClose: () => void;
}

export function UnsupportedInputDialog({ reason, onClose }: Props) {
  const { t } = useI18n();
  const id = useId();
  const dialog = useRef<HTMLElement>(null);
  const close = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const element = dialog.current;
    close.current?.focus({ preventScroll: true });
    return () => {
      const active = document.activeElement;
      const ownsFocus = active === document.body || (active !== null && element?.contains(active));
      const otherModal = [...document.querySelectorAll<HTMLElement>('[aria-modal="true"]')]
        .some(modal => modal !== element && modal.isConnected);
      if (ownsFocus && !otherModal && previous?.isConnected) previous.focus({ preventScroll: true });
    };
  }, []);

  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
    } else if (event.key === "Tab") {
      const buttons = dialog.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)");
      if (!buttons?.length) return;
      const first = buttons[0], last = buttons[buttons.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  };

  return <div className="preflight-backdrop unsupported-input-backdrop" role="presentation">
    <section ref={dialog} tabIndex={-1} className="preflight-dialog unsupported-input-dialog" role="alertdialog"
      aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`} onKeyDown={onKeyDown}>
      <header><h2 id={`${id}-title`}>{t("input.unsupportedTitle")}</h2>
        <button type="button" aria-label={t("common.close")} onClick={onClose}><X size={18} aria-hidden="true" /></button>
      </header>
      <div className="preflight-body"><p id={`${id}-description`}>{t(reason === "colmap_disabled" ? "input.colmapDisabled" : "input.unsupportedSelection")}</p></div>
      <footer><div><button ref={close} type="button" className="secondary-action" onClick={onClose}>{t("common.close")}</button></div></footer>
    </section>
  </div>;
}
