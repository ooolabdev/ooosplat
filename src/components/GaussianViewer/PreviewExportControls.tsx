import { useEffect, useRef, useState } from "react";
import { ChevronDown, Download, FileCode2, Film } from "lucide-react";
import { useI18n } from "../../i18n";
import type { VideoOrientation } from "../../types/pipeline";
export function PreviewOrientationControl({ orientation, disabled, onChange }: {
    orientation: VideoOrientation;
    disabled: boolean;
    onChange: (value: VideoOrientation) => void;
}) {
    const { t } = useI18n();
    return <div className="preview-view-control preview-orientation-control" role="group" aria-label={t("export.orientation")}>
    {(["portrait", "landscape"] as const).map(value => <button key={value} type="button" className={orientation === value ? "active" : ""} aria-pressed={orientation === value} disabled={disabled} onClick={() => onChange(value)}>{t(value === "portrait" ? "export.portrait" : "export.landscape")}</button>)}
  </div>;
}
export function PreviewExportMenu({ mode, disabled, videoSupported, videoReason, onVideo, onHtml }: {
    mode: "adjust" | "preview";
    disabled: boolean;
    videoSupported: boolean;
    videoReason: string | null;
    onVideo: () => void;
    onHtml: () => void;
}) {
    const { t } = useI18n();
    const [open, setOpen] = useState(false);
    const trigger = useRef<HTMLButtonElement>(null);
    useEffect(() => { setOpen(false); }, [mode, disabled]);
    const close = () => { setOpen(false); trigger.current?.focus(); };
    return <div className="preview-export-menu" onKeyDown={event => { if (event.key === "Escape") {
        event.stopPropagation();
        close();
    } }}>
    <button ref={trigger} type="button" aria-expanded={open} aria-controls="preview-export-options" disabled={disabled} onClick={() => setOpen(value => !value)}><Download size={14}/>{t("export.label")}<ChevronDown size={12}/></button>
    {open && <>
      <button type="button" className="export-menu-dismiss" aria-label={t("common.close")} onClick={close}/>
      <div id="preview-export-options" className="preview-export-options">
        {mode === "preview" && <button type="button" disabled={!videoSupported} title={videoReason ?? undefined} onClick={() => { close(); onVideo(); }}><Film size={14}/>{t("export.video")}</button>}
        <button type="button" onClick={() => { close(); onHtml(); }}><FileCode2 size={14}/>{t("export.html")}</button>
      </div>
    </>}
  </div>;
}
