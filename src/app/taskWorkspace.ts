import type { FramePlan, ImageSequenceInfo, InputType, Quality, ReshootInputInfo, ReshootSourceInfo, RuntimeEstimate, VideoInfo } from "../types/pipeline";

export const TASK_WORKSPACE_STORAGE_KEY = "ooo-splat-task-workspace-v1";

export type TaskSelection = { kind: "draft" | "project"; id: string };

export interface TaskDraft {
  id: string;
  ordinal: number;
  kind: "generation" | "reshoot";
  inputType: InputType;
  inputPath: string | null;
  quality: Quality;
  sourceProjectId: string | null;
  sourceProjectName: string | null;
  linkedProjectId: string | null;
  running: boolean;
  needsValidation: boolean;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  plan: FramePlan | null;
  estimate: RuntimeEstimate | null;
  reshootSource: ReshootSourceInfo | null;
  reshootPlan: ReshootInputInfo | null;
  error: string | null;
}

interface PersistedWorkspace {
  schemaVersion: 1;
  nextOrdinal: number;
  selected: TaskSelection;
  drafts: Array<Pick<TaskDraft, "id" | "ordinal" | "kind" | "inputType" | "inputPath" | "quality" | "sourceProjectId" | "sourceProjectName" | "linkedProjectId" | "running">>;
}

const newId = () => {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
  const bytes = new Uint8Array(16);
  if (globalThis.crypto?.getRandomValues) globalThis.crypto.getRandomValues(bytes);
  else for (let index = 0; index < bytes.length; index += 1) bytes[index] = Math.floor(Math.random() * 256);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (value) => value.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10).join("")}`;
};

export function createGenerationDraft(ordinal: number): TaskDraft {
  return {
    id: newId(), ordinal, kind: "generation", inputType: "video", inputPath: null,
    quality: "balanced", sourceProjectId: null, sourceProjectName: null,
    linkedProjectId: null, running: false, needsValidation: false,
    video: null, imageSequence: null, plan: null, estimate: null,
    reshootSource: null, reshootPlan: null, error: null,
  };
}

export function createReshootDraft(ordinal: number, sourceProjectId: string, sourceProjectName: string, quality: Quality): TaskDraft {
  return {
    ...createGenerationDraft(ordinal), kind: "reshoot", sourceProjectId, sourceProjectName,
    quality, needsValidation: true,
  };
}

export function loadTaskWorkspace(): { drafts: TaskDraft[]; selected: TaskSelection; nextOrdinal: number } {
  const fallback = createGenerationDraft(1);
  try {
    const raw = window.localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY);
    if (!raw) return { drafts: [fallback], selected: { kind: "draft", id: fallback.id }, nextOrdinal: 2 };
    const value = JSON.parse(raw) as Partial<PersistedWorkspace>;
    if (value.schemaVersion !== 1 || !Array.isArray(value.drafts)) throw new Error("invalid workspace");
    const drafts: TaskDraft[] = value.drafts.filter((draft) => typeof draft.id === "string").map((draft) => ({
      ...createGenerationDraft(Number(draft.ordinal) || 1),
      ...draft,
      needsValidation: Boolean(draft.inputPath || draft.kind === "reshoot"),
      video: null, imageSequence: null, plan: null, estimate: null,
      reshootSource: null, reshootPlan: null, error: null,
    }));
    const normalStandby = drafts.some((draft) => draft.kind === "generation" && !draft.running);
    const nextOrdinal = Math.max(Number(value.nextOrdinal) || 1, ...drafts.map((draft) => draft.ordinal + 1), 1);
    if (!normalStandby) drafts.push(createGenerationDraft(nextOrdinal));
    const next = normalStandby ? nextOrdinal : nextOrdinal + 1;
    let selected = value.selected && ["draft", "project"].includes(value.selected.kind ?? "")
      ? value.selected as TaskSelection
      : { kind: "draft" as const, id: drafts[0].id };
    if (selected.kind === "draft" && !drafts.some((draft) => draft.id === selected.id)) {
      selected = { kind: "draft", id: drafts[0].id };
    }
    return { drafts, selected, nextOrdinal: next };
  } catch {
    return { drafts: [fallback], selected: { kind: "draft", id: fallback.id }, nextOrdinal: 2 };
  }
}

export function saveTaskWorkspace(drafts: TaskDraft[], selected: TaskSelection, nextOrdinal: number) {
  const persisted: PersistedWorkspace = {
    schemaVersion: 1,
    nextOrdinal,
    selected,
    drafts: drafts.map(({ id, ordinal, kind, inputType, inputPath, quality, sourceProjectId, sourceProjectName, linkedProjectId, running }) => ({
      id, ordinal, kind, inputType, inputPath, quality, sourceProjectId, sourceProjectName,
      linkedProjectId, running,
    })),
  };
  try { window.localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify(persisted)); } catch { /* optional local recovery */ }
}

export function draftDisplayName(draft: TaskDraft, newTaskLabel: string, reshootLabel: string) {
  if (draft.kind === "reshoot") return `${draft.sourceProjectName ?? newTaskLabel} · ${reshootLabel}`;
  if (draft.inputPath) return draft.inputPath.split(/[\\/]/).at(-1) ?? `${newTaskLabel} ${draft.ordinal}`;
  return `${newTaskLabel} ${draft.ordinal}`;
}
