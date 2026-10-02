// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LanguageProvider, useI18n } from "../i18n";
import type { ErrorReportDraft } from "../types/diagnostics";

const mocks = vi.hoisted(() => ({ prepare: vi.fn(), send: vi.fn() }));
vi.mock("../lib/backend", () => ({ prepareErrorReport: mocks.prepare, sendErrorReport: mocks.send }));
import { ErrorReportDialog } from "./ErrorReportDialog";

const draft: ErrorReportDraft = { draftId: "draft-token", report: { schemaVersion: 1, reportId: "report-id", timestamp: "2026-10-02T10:00:00Z", appVersion: "0.5.0", stage: "trainingSplats", engine: "brush", errorCode: "brush_dataset", reason: "Could not read data", detail: "early eof [REDACTED]", logTail: "last error", logsTruncated: true, environment: { system: { name: "Windows 11", version: "10.0", arch: "x86_64" }, gpus: [{ name: "Intel UHD", driverVersion: "1", totalMemoryMb: null, computeCapability: null }, { name: "NVIDIA RTX", driverVersion: "2", totalMemoryMb: 8192, computeCapability: "8.6" }], actualDevice: null } } };
let container: HTMLDivElement, root: Root;
const back = vi.fn(), sent = vi.fn();
async function flush() { await act(async () => { await Promise.resolve(); }); }
function LanguageButton() { const { toggleLocale } = useI18n(); return <button onClick={toggleLocale}>switch</button>; }
async function render(failureId = "failure-id") {
  await act(async () => { root.render(<LanguageProvider><LanguageButton /><ErrorReportDialog failureId={failureId} onBack={back} onSent={sent} /></LanguageProvider>); });
  await flush();
}
function button(text: string) { return [...container.querySelectorAll("button")].find(value => value.textContent === text)!; }
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear(); localStorage.setItem("ooo-splat-language", "zh-CN");
  mocks.prepare.mockReset().mockResolvedValue(draft); mocks.send.mockReset().mockResolvedValue({ reportId: "report-id", accepted: true }); back.mockReset(); sent.mockReset();
  container = document.createElement("div"); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });

describe("one-time diagnostic consent", () => {
  it("previews the exact payload without uploading, including multiple GPUs", async () => {
    await render();
    expect(mocks.prepare).toHaveBeenCalledWith("failure-id"); expect(mocks.send).not.toHaveBeenCalled();
    expect(container.querySelector(".report-preview")?.textContent).toBe(JSON.stringify(draft.report, null, 2));
    expect(container.textContent).toContain("Intel UHD / NVIDIA RTX"); expect(container.textContent).toContain("不改变匿名统计设置");
    await act(async () => button("取消").click()); expect(back).toHaveBeenCalledOnce(); expect(mocks.send).not.toHaveBeenCalled();
  });
  it("sends only after consent, prevents double clicks and shows an acknowledged ID", async () => {
    let finish!: (value: unknown) => void; mocks.send.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    await render(); await act(async () => button("同意并发送").click());
    expect(mocks.send).toHaveBeenCalledWith("draft-token"); expect(button("发送中…").disabled).toBe(true);
    await act(async () => button("发送中…").click()); expect(mocks.send).toHaveBeenCalledOnce();
    await act(async () => finish({ reportId: "report-id", accepted: true })); await flush();
    expect(container.textContent).toContain("报告编号：report-id"); expect(sent).toHaveBeenCalledOnce(); expect(button("同意并发送")).toBeUndefined();
  });
  it("retains the reviewed draft after failure and only retries manually", async () => {
    mocks.send.mockRejectedValueOnce({ code: "report_unavailable" }); await render();
    await act(async () => button("同意并发送").click()); await flush();
    expect(container.querySelector("[role=alert]")?.textContent).toContain("未能发送"); expect(mocks.send).toHaveBeenCalledOnce();
    await act(async () => button("同意并发送").click()); await flush(); expect(mocks.send).toHaveBeenCalledTimes(2); expect(mocks.prepare).toHaveBeenCalledOnce();
  });
  it("requires a fresh preview after expiry and rejects false success", async () => {
    mocks.send.mockRejectedValueOnce({ code: "report_expired" }); await render();
    await act(async () => button("同意并发送").click()); await flush();
    expect(container.querySelector(".report-preview")).toBeNull(); expect(button("重试")).toBeDefined();
    await act(async () => button("重试").click()); await flush();
    mocks.send.mockResolvedValueOnce({ reportId: "wrong-id", accepted: true });
    await act(async () => button("同意并发送").click()); await flush(); expect(sent).not.toHaveBeenCalled(); expect(container.querySelector("[role=alert]")).not.toBeNull();
  });
  it("ignores an older asynchronous preview and translates without preparing again", async () => {
    let finish!: (value: unknown) => void;
    mocks.prepare.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    await render("old"); await render("new"); await act(async () => finish({ ...draft, draftId: "stale" })); await flush();
    await act(async () => button("switch").click()); expect(container.textContent).toContain("Review and send error report"); expect(mocks.prepare).toHaveBeenCalledTimes(2);
    await act(async () => button("Agree and send").click()); await flush(); expect(mocks.send).toHaveBeenCalledWith("draft-token");
  });
  it("allows Esc cancellation without uploading and returns focus inside the dialog", async () => {
    await render(); expect(document.activeElement?.getAttribute("role")).toBe("dialog");
    await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))); expect(back).toHaveBeenCalledOnce(); expect(mocks.send).not.toHaveBeenCalled();
  });
});
