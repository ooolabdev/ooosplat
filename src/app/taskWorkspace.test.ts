// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import {
  createGenerationDraft,
  draftGenerationPolicy,
  draftDisplayName,
  createReshootDraft,
  loadTaskWorkspace,
  nextGenerationOrdinal,
  saveTaskWorkspace,
  TASK_WORKSPACE_STORAGE_KEY,
} from "./taskWorkspace";

describe("task workspace persistence", () => {
  it('uses policy v3 for new and restored custom-sampling drafts without changing user choices', () => {
    const draft = { ...createGenerationDraft(1), frameSamplingRatio: .755, autoRetryEnabled: false };
    expect(draftGenerationPolicy(draft)).toEqual({version:3,frameSamplingRatio:.755,autoRetryEnabled:false});
    saveTaskWorkspace([draft], {kind:'draft',id:draft.id}, 2);
    expect(draftGenerationPolicy(loadTaskWorkspace().drafts[0])).toEqual(draftGenerationPolicy(draft));
  });
  it('persists retry consent and manual base sampling independently of legacy optimization', () => {
    const draft = { ...createGenerationDraft(1, false), autoRetryEnabled: false, frameSamplingRatio: .755 };
    saveTaskWorkspace([draft], {kind:'draft',id:draft.id}, 2);
    expect(loadTaskWorkspace().drafts[0]).toMatchObject({autoRetryEnabled:false,frameSamplingRatio:.755,plannerEnabled:false});
    const stored = JSON.parse(localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY)!);
    delete stored.drafts[0].autoRetryEnabled; delete stored.drafts[0].frameSamplingRatio;
    localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY,JSON.stringify(stored));
    expect(loadTaskWorkspace().drafts[0]).toMatchObject({autoRetryEnabled:true,frameSamplingRatio:null});
  });
  beforeEach(() => localStorage.clear());

  it("hides display ordinals without changing draft identities or recovery order", () => {
    const drafts = [createGenerationDraft(1), createGenerationDraft(2)];
    expect(drafts.map(draft => draftDisplayName(draft, '新任务', '补拍'))).toEqual(['新任务', '新任务']);
    expect(draftDisplayName(drafts[0], 'New Task', 'Reshoot')).toBe('New Task');
    expect(draftDisplayName({ ...drafts[0], inputPath: 'E:/media/clip.mp4' }, '新任务', '补拍')).toBe('clip.mp4');
    saveTaskWorkspace(drafts, { kind: 'draft', id: drafts[1].id }, 3);
    const restored = loadTaskWorkspace();
    expect(restored.drafts.map(d => [d.id, d.ordinal])).toEqual(drafts.map(d => [d.id, d.ordinal]));
    expect(restored.selected).toEqual({ kind: 'draft', id: drafts[1].id });
  });

  it("creates a standby generation draft by default", () => {
    const workspace = loadTaskWorkspace();
    expect(workspace.drafts).toHaveLength(1);
    expect(workspace.drafts[0]).toMatchObject({ kind: "generation", running: false, generationPolicyVersion: 7 });
    expect(workspace.drafts[0].id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
  });

  it('preserves saved v6 and v7 policies instead of upgrading created tasks on restart', () => {
    const inputs = { mode: 'materials' as const, items: [{
      id: 'clip', source: { kind: 'video' as const, path: 'C:/clip.mp4' },
      projection: 'perspective' as const, imageMatchingStrategy: 'sequential' as const,
      frameRemovalFps: 0,
    }] };
    const previous = { ...createGenerationDraft(1), inputs, generationPolicyVersion: 6 as const, linkedProjectId: 'saved-v6' };
    const current = { ...createGenerationDraft(2), inputs, linkedProjectId: 'saved-v7' };
    saveTaskWorkspace([previous, current], { kind: 'draft', id: current.id }, 3);
    const restored = loadTaskWorkspace().drafts;
    expect(restored.find(draft => draft.id === previous.id)).toMatchObject({ generationPolicyVersion: 6, inputs });
    expect(restored.find(draft => draft.id === current.id)).toMatchObject({ generationPolicyVersion: 7, inputs });
    expect(draftGenerationPolicy(restored.find(draft => draft.id === current.id)!)).toMatchObject({ version: 7 });
  });

  it("persists source choices but discards stale analysis", () => {
    const draft = { ...createGenerationDraft(1, false), inputPath: "C:/capture.mov", video: { duration: 1 } as never, error: "old error", errorAt: 123 };
    saveTaskWorkspace([draft], { kind: "draft", id: draft.id }, 2);
    const restored = loadTaskWorkspace().drafts[0];
    expect(restored.inputPath).toBe("C:/capture.mov");
    expect(restored.video).toBeNull();
    expect(restored.needsValidation).toBe(true);
    expect(restored.plannerEnabled).toBe(false);
    expect(restored.plannerDefaultPending).toBe(false);
    expect(restored).toMatchObject({ error: null, errorAt: null });
  });

  it('upgrades only unsubmitted v5 drafts to the corrected v6 frame planner', () => {
    const input = { mode: 'materials' as const, items: [{
      id: 'clip', source: { kind: 'video' as const, path: 'C:/clip.mp4' },
      projection: 'perspective' as const, imageMatchingStrategy: 'sequential' as const,
      frameRemovalFps: 7,
    }] };
    const standby = { ...createGenerationDraft(1), inputs: input, generationPolicyVersion: 5 as const };
    const submitted = { ...createGenerationDraft(2), inputs: input, generationPolicyVersion: 5 as const, linkedProjectId: 'project-2' };
    saveTaskWorkspace([standby, submitted], { kind: 'draft', id: standby.id }, 3);

    const restored = loadTaskWorkspace().drafts;
    expect(restored.find(draft => draft.id === standby.id)).toMatchObject({
      generationPolicyVersion: 6,
      inputs: { mode: 'materials', items: [expect.objectContaining({ frameRemovalFps: 7 })] },
    });
    expect(restored.find(draft => draft.id === submitted.id)?.generationPolicyVersion).toBe(5);
  });
  it("persists the confirmed projection but not detection or preview data", () => {
    const draft = { ...createGenerationDraft(1), videoProjection: "equirectangular" as const, inputPath: "capture.mp4", projectionDetection: { confirmedErp: false, requiresChoice: true, supportedErp: true, reason: "ERP", previewJpeg: [1,2,3] } };
    saveTaskWorkspace([draft], { kind: "draft", id: draft.id }, 2);
    const restored = loadTaskWorkspace().drafts[0];
    expect(restored.videoProjection).toBe("equirectangular");
    expect(restored.projectionDetection).toBeNull();
    expect(restored.needsValidation).toBe(true);
  });

  it("marks legacy drafts for one-time planner default hydration", () => {
    localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify({
      schemaVersion: 1,
      nextOrdinal: 2,
      selected: { kind: "draft", id: "legacy" },
      drafts: [{ id: "legacy", ordinal: 1, kind: "generation", inputType: "video", inputPath: null, quality: "balanced", sourceProjectId: null, sourceProjectName: null, linkedProjectId: null, running: false }],
    }));
    expect(loadTaskWorkspace().drafts[0].plannerDefaultPending).toBe(true);
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
