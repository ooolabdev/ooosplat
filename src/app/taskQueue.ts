import type { TaskDraft } from "./taskWorkspace";
import { resolvedProjection } from './generationPreflight';

export type DraftDisplayStatus = "standby" | "preparing" | "queued" | "running";

export function draftIsRunnable(draft: TaskDraft): boolean {
  if (draft.running || draft.inputChecking || draft.inputRejection || draft.error || !draft.inputPath) return false;
  if (!draftProjectionReady(draft)) return false;
  if (draft.kind === "generation") return Boolean(draft.plan);
  return Boolean(draft.sourceProjectId && draft.reshootSource?.eligible && draft.reshootPlan?.compatible);
}

export function draftProjectionReady(draft: TaskDraft): boolean {
  if (draft.inputRejection) return false;
  if(draft.colmapDiscovery)return false;
  if(draft.inputs){if(draft.inputs.mode==='colmap')return !!draft.colmapInspection?.identity;return draft.inputs.items.length>0&&draft.inputs.items.every(i=>{const a=draft.inputAnalyses?.[i.id];const orderReady=(a?.imageSequence?.imageCount??0)<=100||!!i.confirmedImageOrder;return !!a&&!!i.identity&&i.identity===a.identity&&i.projection!=='auto'&&(i.projection!=='equirectangular'||a.projectionDetection.supportedErp)&&orderReady;});}
  if (draft.kind === 'reshoot' && !draft.projectionDetection) return true;
  const projection = resolvedProjection(draft);
  if (draft.kind === 'reshoot' && draft.reshootSource?.projection && projection !== 'auto' && projection !== draft.reshootSource.projection) return false;
  if (projection === 'equirectangular') return !!draft.projectionDetection?.supportedErp;
  return projection !== 'auto' || !draft.projectionDetection?.requiresChoice;
}

export function firstQueueInputIssue(drafts: TaskDraft[]): { draft: TaskDraft; reason: 'checking' | 'analysis' | 'projection' | 'imageOrder' | 'colmap' } | null {
  for (const draft of drafts) {
    if (draft.running || !draft.inputPath) continue;
    if(draft.inputRejection)return {draft,reason:draft.inputRejection==='colmap_disabled'?'colmap':'analysis'};
    if(draft.colmapDiscovery)return {draft,reason:'colmap'};
    if (draft.inputChecking || draft.needsValidation) return { draft, reason: 'checking' };
    if (draft.inputError || draft.error || (draft.kind === 'reshoot' ? !draft.reshootPlan?.compatible : !draft.plan)) return { draft, reason: 'analysis' };
    if(draft.inputs?.mode==='materials'&&draft.inputs.items.some(item=>(draft.inputAnalyses?.[item.id]?.imageSequence?.imageCount??0)>100&&!item.confirmedImageOrder))return {draft,reason:'imageOrder'};
    if (!draftProjectionReady(draft)) return { draft, reason: 'projection' };
  }
  return null;
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
