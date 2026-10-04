import { describe, expect, it } from "vitest";
import { createGenerationDraft, createReshootDraft, type TaskDraft } from "./taskWorkspace";
import { displayStatusForDraft, draftIsRunnable, moveDraft, queuedDraftIds, reorderDrafts } from "./taskQueue";

const readyGeneration = (ordinal: number): TaskDraft => ({
  ...createGenerationDraft(ordinal), id: `g${ordinal}`, inputPath: `video-${ordinal}.mp4`, plan: {} as TaskDraft["plan"],
});

const readyReshoot = (ordinal: number): TaskDraft => ({
  ...createReshootDraft(ordinal, "source", "source", "balanced"), id: `r${ordinal}`, inputPath: `images-${ordinal}`,
  reshootSource: { eligible: true } as TaskDraft["reshootSource"],
  reshootPlan: { compatible: true } as TaskDraft["reshootPlan"],
});

describe("task queue ordering", () => {
  it("reorders by pointer insertion boundary without changing task identity", () => {
    const drafts = [readyGeneration(1), readyGeneration(2), readyGeneration(3)];
    expect(reorderDrafts(drafts, "g1", 3).map((draft) => draft.id)).toEqual(["g2", "g3", "g1"]);
    expect(reorderDrafts(drafts, "g3", 0).map((draft) => draft.id)).toEqual(["g3", "g1", "g2"]);
    expect(reorderDrafts(drafts, "g2", 2)).toBe(drafts);
  });

  it("does not move the running task, but lets another task cross it", () => {
    const drafts = [readyGeneration(1), { ...readyGeneration(2), running: true }, readyGeneration(3)];
    expect(reorderDrafts(drafts, "g2", 0)).toBe(drafts);
    expect(reorderDrafts(drafts, "g3", 1).map((draft) => draft.id)).toEqual(["g1", "g3", "g2"]);
  });

  it("supports accessible one-step keyboard moves", () => {
    const drafts = [readyGeneration(1), readyGeneration(2), readyGeneration(3)];
    expect(moveDraft(drafts, "g2", -1).map((draft) => draft.id)).toEqual(["g2", "g1", "g3"]);
    expect(moveDraft(drafts, "g2", 1).map((draft) => draft.id)).toEqual(["g1", "g3", "g2"]);
  });
});

describe("task queue status", () => {
  it("queues a contiguous ready generation/reshoot prefix and stops at the first barrier", () => {
    const active = { ...readyGeneration(1), running: true };
    const blocked = { ...readyGeneration(3), inputPath: null, plan: null };
    const drafts = [active, readyReshoot(2), blocked, readyGeneration(4)];
    expect([...queuedDraftIds(drafts, active.id, true)]).toEqual(["r2"]);
    expect(displayStatusForDraft(active, new Set())).toBe("running");
    expect(displayStatusForDraft({ ...blocked, inputChecking: true }, new Set())).toBe("preparing");
  });

  it("does not queue while disabled, without an active draft, or without prerequisites", () => {
    const drafts = [{ ...readyGeneration(1), running: true }, readyGeneration(2)];
    expect(queuedDraftIds(drafts, "g1", false).size).toBe(0);
    expect(queuedDraftIds(drafts, null, true).size).toBe(0);
    expect(queuedDraftIds(drafts, "g1", true, false).size).toBe(0);
  });

  it("requires completed input checks and compatible input", () => {
    expect(draftIsRunnable({ ...readyGeneration(1), inputChecking: true })).toBe(false);
    expect(draftIsRunnable({ ...readyReshoot(2), reshootPlan: null })).toBe(false);
    expect(draftIsRunnable(readyReshoot(3))).toBe(true);
  });
});
