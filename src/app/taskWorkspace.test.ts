// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import { createGenerationDraft, loadTaskWorkspace, saveTaskWorkspace, TASK_WORKSPACE_STORAGE_KEY } from "./taskWorkspace";

describe("task workspace persistence", () => {
  beforeEach(() => localStorage.clear());

  it("creates a standby generation draft by default", () => {
    const workspace = loadTaskWorkspace();
    expect(workspace.drafts).toHaveLength(1);
    expect(workspace.drafts[0]).toMatchObject({ kind: "generation", running: false });
    expect(workspace.drafts[0].id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
  });

  it("persists source choices but discards stale analysis", () => {
    const draft = { ...createGenerationDraft(1), inputPath: "C:/capture.mov", video: { duration: 1 } as never };
    saveTaskWorkspace([draft], { kind: "draft", id: draft.id }, 2);
    const restored = loadTaskWorkspace().drafts[0];
    expect(restored.inputPath).toBe("C:/capture.mov");
    expect(restored.video).toBeNull();
    expect(restored.needsValidation).toBe(true);
  });

  it("recovers from damaged storage", () => {
    localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, "{");
    expect(loadTaskWorkspace().drafts).toHaveLength(1);
  });

  it("falls back to an existing draft when the saved selection is stale", () => {
    const draft = createGenerationDraft(1);
    saveTaskWorkspace([draft], { kind: "draft", id: "00000000-0000-4000-8000-000000000000" }, 2);
    expect(loadTaskWorkspace().selected).toEqual({ kind: "draft", id: draft.id });
  });
});
