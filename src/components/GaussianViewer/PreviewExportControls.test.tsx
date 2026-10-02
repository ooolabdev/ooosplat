// @vitest-environment jsdom
import { act, useEffect, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "../../i18n";
import type { VideoOrientation } from "../../types/pipeline";
import { PreviewExportMenu, PreviewOrientationControl } from "./PreviewExportControls";
describe("preview export controls", () => {
    let root: Root, container: HTMLDivElement;
    beforeEach(() => {
        (globalThis as typeof globalThis & {
            IS_REACT_ACT_ENVIRONMENT: boolean;
        }).IS_REACT_ACT_ENVIRONMENT = true;
        localStorage.setItem("ooo-splat-language", "zh-CN");
        container = document.createElement("div");
        document.body.append(container);
        root = createRoot(container);
    });
    afterEach(async () => { await act(async () => root.unmount()); container.remove(); localStorage.clear(); });
    const button = (text: string) => Array.from(container.querySelectorAll("button")).find(b => b.textContent === text)!;
    const click = async (text: string) => act(async () => button(text).click());
    it("offers HTML only in Adjust, and closes after choosing it", async () => {
        const onHtml = vi.fn();
        await act(async () => root.render(<LanguageProvider><PreviewExportMenu mode="adjust" disabled={false} videoSupported videoReason={null} onVideo={vi.fn()} onHtml={onHtml}/></LanguageProvider>));
        await click("导出");
        expect(button("导出视频")).toBeUndefined();
        await click("导出离线 HTML");
        expect(onHtml).toHaveBeenCalledOnce();
        expect(container.querySelector("#preview-export-options")).toBeNull();
    });
    it("offers video and HTML in Animation, while unsupported video does not block HTML", async () => {
        await act(async () => root.render(<LanguageProvider><PreviewExportMenu mode="preview" disabled={false} videoSupported={false} videoReason="unsupported" onVideo={vi.fn()} onHtml={vi.fn()}/></LanguageProvider>));
        await click("导出");
        expect(button("导出视频").disabled).toBe(true);
        expect(button("导出离线 HTML").disabled).toBe(false);
        await act(async () => button("导出离线 HTML").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
        expect(container.querySelector("#preview-export-options")).toBeNull();
        expect(document.activeElement).toBe(button("导出"));
    });
    it("disables repeated export while busy and closes an existing menu", async () => {
        const render = (disabled: boolean) => <LanguageProvider><PreviewExportMenu mode="preview" disabled={disabled} videoSupported videoReason={null} onVideo={vi.fn()} onHtml={vi.fn()}/></LanguageProvider>;
        await act(async () => root.render(render(false)));
        await click("导出");
        await act(async () => root.render(render(true)));
        expect(container.querySelector("#preview-export-options")).toBeNull();
        expect(button("导出").disabled).toBe(true);
    });
    it("defaults to portrait, changes direction without remounting and locks while exporting", async () => {
        const mounts = vi.fn();
        function Harness() { const [orientation, setOrientation] = useState<VideoOrientation>("portrait"); useEffect(() => { mounts(); }, []); return <PreviewOrientationControl orientation={orientation} disabled={false} onChange={setOrientation}/>; }
        await act(async () => root.render(<LanguageProvider><Harness /></LanguageProvider>));
        expect(button("竖屏").getAttribute("aria-pressed")).toBe("true");
        await click("横屏");
        expect(button("横屏").getAttribute("aria-pressed")).toBe("true");
        expect(mounts).toHaveBeenCalledOnce();
        await act(async () => root.render(<LanguageProvider><PreviewOrientationControl orientation="landscape" disabled onChange={vi.fn()}/></LanguageProvider>));
        expect(button("竖屏").disabled).toBe(true);
    });
    it("uses the English labels", async () => {
        localStorage.setItem("ooo-splat-language", "en");
        await act(async () => root.render(<LanguageProvider><PreviewExportMenu mode="preview" disabled={false} videoSupported videoReason={null} onVideo={vi.fn()} onHtml={vi.fn()}/></LanguageProvider>));
        await click("Export");
        expect(button("Export video")).toBeDefined();
        expect(button("Export offline HTML")).toBeDefined();
    });
});
