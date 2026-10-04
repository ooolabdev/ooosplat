// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import {
  createGenerationDraft,
  createReshootDraft,
  loadTaskWorkspace,
  nextGenerationOrdinal,
  saveTaskWorkspace,
  TASK_WORKSPACE_STORAGE_KEY,
} from "./taskWorkspace";

describe("task workspace persistence", () => {
  beforeEach(() => localStorage.clear());

  it("creates a standby generation draft by default", () => {
    const workspace = loadTaskWorkspace();
    expect(workspace.drafts).toHaveLength(1);
    expect(workspace.drafts[0]).toMatchObject({ kind: "generation", running: false });
    expect(workspace.drafts[0].id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
  });

  it("persists source choices but discards stale analysis", () => {
    const draft = { ...createGenerationDraft(1), inputPath: "C:/capture.mov", video: { duration: 1 } as never, error: "old error", errorAt: 123 };
    saveTaskWorkspace([draft], { kind: "draft", id: draft.id }, 2);
    const restored = loadTaskWorkspace().drafts[0];
    expect(restored.inputPath).toBe("C:/capture.mov");
    expect(restored.video).toBeNull();
    expect(restored.needsValidation).toBe(true);
    expect(restored).toMatchObject({ error: null, errorAt: null });
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

  it("numbers generation drafts from the largest visible generation ordinal", () => {
    expect(nextGenerationOrdinal([])).toBe(1);
    expect(nextGenerationOrdinal([createGenerationDraft(1), createGenerationDraft(3)])).toBe(4);
  });

  it("reuses the highest generation ordinal after that draft is deleted", () => {
    const remaining = [createGenerationDraft(1), createGenerationDraft(2)];
    expect(nextGenerationOrdinal(remaining)).toBe(3);
  });

  it("does not let reshoot drafts consume generation ordinals", () => {
    const drafts = [
      createGenerationDraft(1),
      createReshootDraft(99, "source-project", "Source project", "balanced"),
    ];
    expect(nextGenerationOrdinal(drafts)).toBe(2);
  });

  it("restores an old workspace without using its historical counter for a new standby task", () => {
    const running = { ...createGenerationDraft(1), running: true };
    const reshoot = createReshootDraft(99, "source-project", "Source project", "balanced");
    saveTaskWorkspace([running, reshoot], { kind: "draft", id: running.id }, 100);

    const restored = loadTaskWorkspace();
    expect(restored.drafts.find((draft) => draft.kind === "generation" && !draft.running)?.ordinal).toBe(2);
    expect(restored.nextOrdinal).toBe(100);
  });

  it("keeps at most one stale running marker and resets transient input checks", () => {
    const first = { ...createGenerationDraft(1), running: true, inputChecking: true };
    const second = { ...createGenerationDraft(2), running: true, inputChecking: true };
    saveTaskWorkspace([first, second], { kind: "draft", id: first.id }, 3);

    const restored = loadTaskWorkspace().drafts;
    expect(restored.filter((draft) => draft.running)).toHaveLength(1);
    expect(restored.every((draft) => !draft.inputChecking)).toBe(true);
  });
});
