import type { FramePlan, ImageSequenceInfo, InputType, Quality, ReshootInputInfo, ReshootSourceInfo, RuntimeEstimate, VideoInfo } from "../types/pipeline";

export const TASK_WORKSPACE_STORAGE_KEY = "ooo-splat-task-workspace-v1";

export type TaskSelection = { kind: "draft" | "project" | "task"; id: string };

export interface TaskDraft {
  inputs?: import('../types/inputs').GenerationInputs | null;
  colmapDiscovery?: import('../types/inputs').ColmapDiscovery | null;
  inputAnalyses?: Record<string, import('../types/inputs').MaterialAnalysis>;
  inputErrors?:Record<string,string>;
  colmapInspection?: import('../types/inputs').ColmapInspection | null;
  frameSamplingFps?: number | null;
  generationPolicyVersion: 4 | 5 | 6 | 7;
  autoRetryEnabled: boolean;
  frameSamplingRatio: number | null;
  gpsDetection?: { sourceIdentity: string; detected: boolean };
  gpsFallback?: string;
  videoProjection: import("../types/pipeline").VideoProjection;
  projectionDetection: import("../types/pipeline").ProjectionDetection | null;
  projectionAnswer?: { identity: string; projection: "perspective" | "equirectangular" } | null;
  id: string;
  ordinal: number;
  kind: "generation" | "reshoot";
  inputType: InputType;
  inputPath: string | null;
  quality: Quality;
  plannerEnabled: boolean;
  plannerDefaultPending: boolean;
  sourceProjectId: string | null;
  sourceProjectName: string | null;
  linkedProjectId: string | null;
  running: boolean;
  inputChecking: boolean;
  inputPlanning?: boolean;
  inputPlanningMaterialId?: string | null;
  inputError?: string | null;
  /** Admission result for this session only; never persisted as task history. */
  inputRejection?: import('../lib/inputAdmission').InputAdmissionReason | null;
  needsValidation: boolean;
  video: VideoInfo | null;
  imageSequence: ImageSequenceInfo | null;
  plan: FramePlan | null;
  estimate: RuntimeEstimate | null;
  reshootSource: ReshootSourceInfo | null;
  reshootPlan: ReshootInputInfo | null;
  error: string | null;
  errorContext?: import('../lib/pipelineError').FailureContext;
  errorAt: number | null;
}

interface PersistedWorkspace {
  schemaVersion: 1;
  nextOrdinal: number;
  selected: TaskSelection;
  drafts: Array<Pick<TaskDraft, "id" | "ordinal" | "kind" | "inputType" | "inputPath" | "quality" | "sourceProjectId" | "sourceProjectName" | "linkedProjectId" | "running" | "projectionAnswer" | "inputs" | "colmapDiscovery" | "frameSamplingFps"> & { generationPolicyVersion?: 4 | 5 | 6 | 7; autoRetryEnabled?: boolean; frameSamplingRatio?: number | null; plannerEnabled?: boolean; videoProjection?: import("../types/pipeline").VideoProjection }>;
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

function restoredInputs(value: unknown, policyVersion?: 4 | 5 | 6 | 7): import('../types/inputs').GenerationInputs | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const input = value as import('../types/inputs').GenerationInputs;
  if(input.mode==='colmap')return input.input && typeof input.input.model==='string' && typeof input.input.images==='string' && (input.input.database==null||typeof input.input.database==='string') ? input : undefined;
  if(input.mode!=='materials'||!Array.isArray(input.items))return undefined;
  const ids=new Set<string>();
  for(const item of input.items){
    if(!item||typeof item.id!=='string'||!item.id||ids.has(item.id)||!item.source||!['auto','perspective','equirectangular'].includes(item.projection)||!['sequential','exhaustive'].includes(item.imageMatchingStrategy))return undefined;
    ids.add(item.id);
    const source=item.source;
    if(source.kind==='images'){if(!Array.isArray(source.paths)||!source.paths.every(p=>typeof p==='string'&&p.length>0))return undefined;}
    else if(!['video','imageFolder'].includes(source.kind)||typeof source.path!=='string')return undefined;
    if(item.frameRemovalFps!=null&&(!Number.isFinite(item.frameRemovalFps)||item.frameRemovalFps<((policyVersion??4)>=6?0:.1)))return undefined;
  }
  return input;
}

function restoredColmapDiscovery(value: unknown): import('../types/inputs').ColmapDiscovery | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const discovery = value as import('../types/inputs').ColmapDiscovery;
  if (!Array.isArray(discovery.models) || !discovery.models.length) return undefined;
  if (!discovery.models.every(candidate => candidate && typeof candidate.model === 'string' && candidate.model.length > 0 && Array.isArray(candidate.imageCandidates) && candidate.imageCandidates.every(path => typeof path === 'string' && path.length > 0))) return undefined;
  if (discovery.database != null && typeof discovery.database !== 'string') return undefined;
  if (discovery.selectedModel != null && (!discovery.models.some(candidate => candidate.model === discovery.selectedModel))) return undefined;
  return discovery;
}

export function createGenerationDraft(ordinal: number, plannerEnabled = true, plannerDefaultPending = false): TaskDraft {
  return {
    autoRetryEnabled: true, frameSamplingRatio: null, generationPolicyVersion: 7,
    videoProjection: "auto", projectionDetection: null,
    id: newId(), ordinal, kind: "generation", inputType: "video", inputPath: null,
    quality: "balanced", plannerEnabled, plannerDefaultPending, sourceProjectId: null, sourceProjectName: null,
    linkedProjectId: null, running: false, inputChecking: false, needsValidation: false,
    video: null, imageSequence: null, plan: null, estimate: null,
    reshootSource: null, reshootPlan: null, error: null, errorAt: null,
  };
}

export function createReshootDraft(ordinal: number, sourceProjectId: string, sourceProjectName: string, quality: Quality, plannerEnabled = true): TaskDraft {
  return {
    ...createGenerationDraft(ordinal, plannerEnabled), kind: "reshoot", sourceProjectId, sourceProjectName,
    quality, needsValidation: true, plannerDefaultPending: true,
  };
}

export function nextGenerationOrdinal(drafts: TaskDraft[]): number {
  return drafts.reduce(
    (maximum, draft) => draft.kind === "generation" ? Math.max(maximum, draft.ordinal) : maximum,
    0,
  ) + 1;
}

export function loadTaskWorkspace(): { drafts: TaskDraft[]; selected: TaskSelection; nextOrdinal: number } {
  const fallback = createGenerationDraft(1, true, true);
  try {
    const raw = window.localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY);
    if (!raw) return { drafts: [fallback], selected: { kind: "draft", id: fallback.id }, nextOrdinal: 2 };
    const value = JSON.parse(raw) as Partial<PersistedWorkspace>;
    if (value.schemaVersion !== 1 || !Array.isArray(value.drafts)) throw new Error("invalid workspace");
    let restoredRunning = false;
    const drafts: TaskDraft[] = value.drafts.filter((draft) => typeof draft.id === "string").map((draft) => {
      const running = Boolean(draft.running) && !restoredRunning;
      restoredRunning ||= running;
      let inputs=restoredInputs(draft.inputs, draft.generationPolicyVersion);
      const savedPolicyVersion = draft.generationPolicyVersion === 7 ? 7 : draft.generationPolicyVersion === 6 ? 6 : draft.generationPolicyVersion === 5 ? 5 : 4;
      // Version 5's editable draft UI interpreted 6/8/12 as removal rates and
      // could never select the exact source rate. It has not started a task yet,
      // so move only that standby intent to the corrected v6 planner. Created
      // and running tasks retain their saved policy snapshot.
      const generationPolicyVersion = savedPolicyVersion === 5 && !running && !draft.linkedProjectId
        ? 6
        : savedPolicyVersion;
      const hadColmapDiscovery = draft.colmapDiscovery != null;
      const colmapDiscovery=restoredColmapDiscovery(draft.colmapDiscovery);
      const inputPath = hadColmapDiscovery && !colmapDiscovery ? null : draft.inputPath;
      if(!running&&!inputs&&!hadColmapDiscovery&&inputPath){
        const answer=draft.projectionAnswer;
        const projection=answer&&typeof answer.identity==='string'&&['perspective','equirectangular'].includes(answer.projection)?answer.projection:'auto';
        inputs={mode:'materials',items:[{id:draft.id,source:{kind:draft.inputType==='images'?'imageFolder':'video',path:inputPath},projection,identity:answer?.identity?`legacy:${answer.identity}`:null,imageMatchingStrategy:'exhaustive'}]};
      }
      return ({
      ...createGenerationDraft(Number(draft.ordinal) || 1),
      ...draft,
      inputPath,
      inputs,
      colmapDiscovery,
      generationPolicyVersion,
      frameSamplingFps: typeof draft.frameSamplingFps==='number' && Number.isFinite(draft.frameSamplingFps) && draft.frameSamplingFps>=.1 ? draft.frameSamplingFps : null,
      autoRetryEnabled: typeof draft.autoRetryEnabled === 'boolean' ? draft.autoRetryEnabled : true,
      frameSamplingRatio: typeof draft.frameSamplingRatio === 'number' && Number.isFinite(draft.frameSamplingRatio) && draft.frameSamplingRatio >= .001 && draft.frameSamplingRatio <= 1 ? draft.frameSamplingRatio : null,
      videoProjection: ["auto", "perspective", "equirectangular"].includes(draft.videoProjection ?? "") ? draft.videoProjection! : "auto",
      projectionAnswer: draft.projectionAnswer && typeof draft.projectionAnswer.identity === "string" && ["perspective", "equirectangular"].includes(draft.projectionAnswer.projection) ? draft.projectionAnswer : null,
      plannerEnabled: typeof draft.plannerEnabled === "boolean" ? draft.plannerEnabled : true,
      plannerDefaultPending: typeof draft.plannerEnabled !== "boolean",
      running,
      inputChecking: false,
      inputPlanningMaterialId: null,
      needsValidation: Boolean(draft.inputPath || inputs || draft.kind === "reshoot"),
      video: null, imageSequence: null, plan: null, estimate: null,
      reshootSource: null, reshootPlan: null, error: null, errorAt: null,
      });
    });
    const normalStandby = drafts.some((draft) => draft.kind === "generation" && !draft.running);
    let nextOrdinal = Math.max(Number(value.nextOrdinal) || 1, ...drafts.map((draft) => draft.ordinal + 1), 1);
    if (!normalStandby) {
      const ordinal = nextGenerationOrdinal(drafts);
      drafts.push(createGenerationDraft(ordinal, true, true));
      nextOrdinal = Math.max(nextOrdinal, ordinal + 1);
    }
    let selected = value.selected && ["draft", "project", "task"].includes(value.selected.kind ?? "")
      ? value.selected as TaskSelection
      : { kind: "draft" as const, id: drafts[0].id };
    if (selected.kind === "draft" && !drafts.some((draft) => draft.id === selected.id)) {
      selected = { kind: "draft", id: drafts[0].id };
    }
    return { drafts, selected, nextOrdinal };
  } catch {
    return { drafts: [fallback], selected: { kind: "draft", id: fallback.id }, nextOrdinal: 2 };
  }
}

export function saveTaskWorkspace(drafts: TaskDraft[], selected: TaskSelection, nextOrdinal: number) {
  const persisted: PersistedWorkspace = {
    schemaVersion: 1,
    nextOrdinal,
    selected,
    drafts: drafts.map(({ id, ordinal, kind, inputType, inputPath, quality, plannerEnabled, plannerDefaultPending, sourceProjectId, sourceProjectName, linkedProjectId, running, videoProjection, projectionAnswer, autoRetryEnabled, frameSamplingRatio,inputs,colmapDiscovery,frameSamplingFps,generationPolicyVersion }) => ({
      inputs,colmapDiscovery,frameSamplingFps,generationPolicyVersion,
      autoRetryEnabled, frameSamplingRatio,
      videoProjection, projectionAnswer,
      id, ordinal, kind, inputType, inputPath, quality, sourceProjectId, sourceProjectName,
      linkedProjectId, running,
      ...(plannerDefaultPending ? {} : { plannerEnabled }),
    })),
  };
  try { window.localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify(persisted)); } catch { /* optional local recovery */ }
}

export function draftDisplayName(draft: TaskDraft, newTaskLabel: string, reshootLabel: string, mixedLabel?:string) {
  if (draft.kind === "reshoot") return `${draft.sourceProjectName ?? newTaskLabel} · ${reshootLabel}`;
  if(draft.inputs?.mode==='materials'&&draft.inputs.items.length>1&&mixedLabel)return mixedLabel;
  if (draft.inputPath) return draft.inputPath.split(/[\\/]/).at(-1) || newTaskLabel;
  return newTaskLabel;
}

export const draftGenerationPolicy = (draft: TaskDraft): import('../types/pipeline').GenerationPolicy => draft.inputs ? ({version:draft.generationPolicyVersion,autoRetryEnabled:draft.autoRetryEnabled,frameSamplingRatio:draft.frameSamplingFps==null&&draft.inputs.mode==='materials'&&draft.inputs.items.length===1&&draft.inputs.items[0].source.kind==='video'?draft.frameSamplingRatio:null,frameSamplingFps:draft.frameSamplingFps??null}) : ({ version: 3, autoRetryEnabled: draft.autoRetryEnabled, frameSamplingRatio: draft.inputType === 'video' ? draft.frameSamplingRatio : null });
