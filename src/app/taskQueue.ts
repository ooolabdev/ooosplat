import type { TaskDraft } from "./taskWorkspace";

export type DraftDisplayStatus = "standby" | "preparing" | "queued" | "running";

export function draftIsRunnable(draft: TaskDraft): boolean {
  if (draft.running || draft.inputChecking || draft.error || !draft.inputPath) return false;
  if (draft.kind === "generation") return Boolean(draft.plan);
  return Boolean(draft.sourceProjectId && draft.reshootSource?.eligible && draft.reshootPlan?.compatible);
}

export function queuedDraftIds(
  drafts: TaskDraft[],
  activeDraftId: string | null,
  enabled: boolean,
  prerequisitesReady = true,
): Set<string> {
  const queued = new Set<string>();
  if (!enabled || !activeDraftId || !prerequisitesReady) return queued;
  const activeIndex = drafts.findIndex((draft) => draft.id === activeDraftId);
  if (activeIndex < 0) return queued;
  for (let index = activeIndex + 1; index < drafts.length; index += 1) {
    const draft = drafts[index];
    if (!draftIsRunnable(draft)) break;
    queued.add(draft.id);
  }
  return queued;
}

/** Reorder to a boundary in the original list (0..length), as used by pointer insertion lines. */
export function reorderDrafts(drafts: TaskDraft[], draftId: string, insertionIndex: number): TaskDraft[] {
  const sourceIndex = drafts.findIndex((draft) => draft.id === draftId);
  if (sourceIndex < 0 || drafts[sourceIndex].running) return drafts;
  const next = [...drafts];
  const [draft] = next.splice(sourceIndex, 1);
  const adjusted = sourceIndex < insertionIndex ? insertionIndex - 1 : insertionIndex;
  next.splice(Math.max(0, Math.min(next.length, adjusted)), 0, draft);
  return next.every((item, index) => item === drafts[index]) ? drafts : next;
}

export function moveDraft(drafts: TaskDraft[], draftId: string, offset: -1 | 1): TaskDraft[] {
  const sourceIndex = drafts.findIndex((draft) => draft.id === draftId);
  if (sourceIndex < 0 || drafts[sourceIndex].running) return drafts;
  const destination = sourceIndex + offset;
  if (destination < 0 || destination >= drafts.length) return drafts;
  const next = [...drafts];
  const [draft] = next.splice(sourceIndex, 1);
  next.splice(destination, 0, draft);
  return next;
}

export function displayStatusForDraft(
  draft: TaskDraft,
  queued: ReadonlySet<string>,
): DraftDisplayStatus {
  if (draft.running) return "running";
  if (draft.inputChecking) return "preparing";
  if (queued.has(draft.id)) return "queued";
  return "standby";
}
