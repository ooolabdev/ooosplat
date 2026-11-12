import { taskStatusLabel, taskProjectSummary } from "./taskPresentation";
import { NewTaskRow } from "../components/NewTaskRow";
import { ProjectRow } from "../components/ProjectRow";
import { ProjectResultStats } from "../components/ProjectResultStats";
import { InputAdoptionWarning } from "../components/InputAdoptionWarning";
import { TaskConfiguration } from "../components/TaskConfiguration";
import { FrameSamplingControl } from '../components/FrameSamplingControl';
import { FrameFpsControl } from '../components/FrameFpsControl';
import { MaterialInputs } from '../components/MaterialInputs';
import { sourcePath, type GenerationInputs, type ColmapInput } from '../types/inputs';
import { selectMaterialPaths,selectColmapDatabase,selectColmapFolder,classifyGenerationPaths,inspectMaterialInputs,estimateMaterialInputs,startInputsPipeline,detectMaterialGps } from '../lib/backend';
import { EngineStatusPopover } from '../components/EngineStatusPopover';
import { isProgressEvent, stagePercent } from '../lib/progressState';
import { TaskProgress } from "../components/TaskProgress";
import { HistoricalTaskProgress } from '../components/HistoricalTaskProgress';
import { taskIsActive } from "../types/tasks";
import { SharedTaskDetail } from "../components/SharedTaskDetail";
import { useSharedTasks } from "./useSharedTasks";
import { useProgressMessage } from "./useProgressMessage";
import { useTaskActivityLog } from './useTaskActivityLog';
import { useGpsDetection } from './useGpsDetection';
import { lazy, Suspense, useCallback, useEffect, useId, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  BellDot, ChevronDown, ChevronRight, CircleAlert, CircleHelp, Clapperboard, Cpu,
  FolderOpen, Images, Languages, LoaderCircle, Minus, Play, Plus, RotateCcw, Settings2,
  Send, Upload, X, Zap, Lock,
} from "lucide-react";
import appLogo from "../../assets/app-icon.svg";
import packageMetadata from "../../package.json";
import { TelemetryPreferences } from "../components/TelemetryPreferences";
import { SettingsDialog } from "../components/SettingsDialog";
import { UpdateNoticeDialog, UpdateSettingsRow } from "../components/UpdateNotice";
import { useUpdateNotice } from "./useUpdateNotice";
import { ErrorReportDialog } from "../components/ErrorReportDialog";
import { CompactError } from "../components/CompactError";
import { failureCopy, failureGuidanceKind } from "../lib/failureGuidance";
import { taskFailureContext, type FailureContext } from "../lib/pipelineError";
import { GaussianImportDialog } from "../components/GaussianImportDialog";
import { UnsupportedInputDialog } from "../components/UnsupportedInputDialog";
import { inputAdmissionReason, type InputAdmissionReason } from "../lib/inputAdmission";
import { isTaskAdmissionConflict } from '../lib/taskAdmission';
import { ImportedProjectDetails } from "../components/ImportedProjectDetails";
import {
  cancelPipeline, checkColmapAcceleration, checkEngines, classifyDroppedInput, confirmAndDeleteProject,
  estimateProjectRuntime, getAppRuntimeStatus, getProjectOverview, getProjectTaskDetail, probeAndPlan, revealProject, revealProjectLogs,
  selectImageSequence, selectProjectsRoot, selectVideo,
  setProjectsRoot, startPipeline, prepareGaussianPreview, releaseGaussianPreview,
  initializeTelemetry, inspectReshootSource, onInputDragDrop, probeReshootInput, setTelemetryConsent, resumePipeline, startReshootPipeline,
  selectGaussianImportSource, revealImportedSource, relinkImportedProject, continueTaskPipeline,
} from "../lib/backend";
import { startElapsedTicker } from "../lib/elapsedTimer";
import { useTaskElapsed } from "./useTaskElapsed";
import { liveTrainingRemainingSeconds } from "../lib/runtimeEstimate";
import { pipelineCommandError, pipelineErrorMessage, pipelineWasCancelled, type PipelineFailureKind } from "../lib/pipelineError";
import { displayPath } from "../lib/displayPath";
import { localizePipelineMessage, useI18n, type TranslationKey } from "../i18n";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import type { EngineStatus, InputType, PipelineEvent, PipelineResult, ProjectStatus, ProjectSummary, ProjectTaskDetail, Quality } from "../types/pipeline";
import type { TelemetryPreferences as TelemetryPreferencesState } from "../types/telemetry";
import { createGenerationDraft, createReshootDraft, draftDisplayName, draftGenerationPolicy, loadTaskWorkspace, nextGenerationOrdinal, saveTaskWorkspace, type TaskDraft, type TaskSelection } from "./taskWorkspace";
import { displayStatusForDraft, draftIsRunnable, draftProjectionReady, firstQueueInputIssue, queuedDraftIds, reorderDrafts, type DraftDisplayStatus } from "./taskQueue";
import { loadUiPreferences, saveUiPreferences } from "./uiPreferences";
import { GenerationPreflightDialog, type PreflightSession } from '../components/GenerationPreflightDialog';
import { answerIsValid, collectPreflightQuestions,confirmedInputs, imageMatchingStrategy, preflightSnapshot, projectionIdentity, resolvedProjection, type PreflightAnswers, type PreflightQuestion } from './generationPreflight';

const GaussianViewer = lazy(() => import("../components/GaussianViewer").then((module) => ({ default: module.GaussianViewer })));
const CANCELLATION_OVERLAY_DELAY_MS = 300;
const PREVIEW_CLOSE_TIMEOUT_MS = 8_000;
const NATIVE_ACTION_TIMEOUT_MS = 8_000;
const DRAFT_DRAG_START_DISTANCE = 4;
const DRAFT_DRAG_HOLD_MS = 200;
const TASK_GROUPS_STORAGE_KEY = "ooo-splat-task-groups-v1";
type TaskGroupKey = "new" | "completed" | "unfinished";
type TaskGroupVisibility = Record<TaskGroupKey, boolean>;
const DEFAULT_TASK_GROUP_VISIBILITY: TaskGroupVisibility = { new: true, completed: true, unfinished: true };
type DetailErrorSource = "action" | "detail" | "draft" | "persisted" | "global";
type DetailErrorRecord = { message: string; occurredAt: number; source: DetailErrorSource; contextKey: string; errorContext?: FailureContext };
type DraftDragHold = { pointerId: number; draftId: string; startX: number; startY: number; target: HTMLElement; timer: number; active: boolean };
const ERROR_SOURCE_PRIORITY: Record<DetailErrorSource, number> = { action: 4, draft: 4, detail: 3, persisted: 2, global: 1 };

function CaptureAdviceHelp() {
  const { t } = useI18n();
  const tooltipId = useId();
  return <span className="capture-help">
    <button className="capture-help-trigger" type="button" aria-label={t("captureAdvice.title")} aria-describedby={tooltipId}>
      <CircleHelp size={16} aria-hidden="true" />
    </button>
    <span className="capture-help-tooltip" id={tooltipId} role="tooltip">
      <strong>{t("captureAdvice.title")}</strong>
      <ul>
        <li>{t("captureAdvice.coverage")}</li>
        <li>{t("captureAdvice.overlap")}</li>
        <li>{t("captureAdvice.consistency")}</li>
      </ul>
      <small>{t("captureAdvice.aiWarning")}</small>
    </span>
  </span>;
}

const timestampOf = (value: string | null | undefined): number => {
  const parsed = value ? Date.parse(value) : Number.NaN;
  return Number.isFinite(parsed) ? parsed : 0;
};

const loadTaskGroupVisibility = (): TaskGroupVisibility => {
  try {
    const saved = JSON.parse(window.localStorage.getItem(TASK_GROUPS_STORAGE_KEY) ?? "null") as Partial<TaskGroupVisibility> | null;
    if (!saved) return DEFAULT_TASK_GROUP_VISIBILITY;
    return {
      new: typeof saved.new === "boolean" ? saved.new : true,
      completed: typeof saved.completed === "boolean" ? saved.completed : true,
      unfinished: typeof saved.unfinished === "boolean" ? saved.unfinished : true,
    };
  } catch {
    return DEFAULT_TASK_GROUP_VISIBILITY;
  }
};
type FailureDialogState = {
  kind: PipelineFailureKind | "generic";
  classificationVersion?: number;
  projectId: string | null;
  taskId: string | null;
  runId: string | null;
  rawMessage: string;
  failureId: string | null;
  engine: string;
};

const withTimeout = <T,>(operation: Promise<T>, timeoutMs: number, timeoutMessage: string): Promise<T> => new Promise<T>((resolve, reject) => {
  const timer = window.setTimeout(() => reject(new Error(timeoutMessage)), timeoutMs);
  operation.then(
    (value) => { window.clearTimeout(timer); resolve(value); },
    (error) => { window.clearTimeout(timer); reject(error); },
  );
});

const inferFailureDialog = (error: unknown, fallbackProjectId?: string): FailureDialogState | null => {
  const structured = pipelineCommandError(error);
  if (structured?.code === "cancelled") return null;
  const rawMessage = structured?.message ?? pipelineErrorMessage(error) ?? "";
  const kind = failureGuidanceKind(error);
  if (kind == null) return null;
  return { kind, classificationVersion: structured?.classificationVersion, projectId: structured?.projectId ?? fallbackProjectId ?? null, taskId: structured?.taskId ?? null, runId: structured?.runId ?? null, rawMessage, failureId: structured?.failureId ?? null, engine: structured?.engine ?? "OOOSplat" };
};

function FailureGuidanceDialog({ failure, action, canContinue, onClose, onRetry, onOpenLogs }: {
  failure: FailureDialogState;
  action: "retry" | "logs" | null;
  canContinue: boolean;
  onClose: () => void;
  onRetry: () => void;
  onOpenLogs: () => void;
}) {
  const { locale, t } = useI18n();
  const [reportOpen, setReportOpen] = useState(false);
  const [reportSent, setReportSent] = useState(false);
  useEffect(() => { setReportOpen(false); setReportSent(false); }, [failure]);
  const mapper = failure.kind === "mapper_source" || failure.kind === "mapper_storage" || failure.kind === "colmap_crash";
  const deviceLost = failure.kind === "brush_device_lost";
  const generic = failure.kind === "generic";
  const copy = failureCopy(failure.kind, failure.classificationVersion);
  const title = t(copy.title), description = t(copy.description), tips = copy.tips;

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && action === null && !reportOpen) onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [action, onClose, reportOpen]);

  if (reportOpen && failure.failureId) return <ErrorReportDialog key={failure.failureId} failureId={failure.failureId} onBack={() => setReportOpen(false)} onSent={() => setReportSent(true)} />;

  return <div className="failure-guidance-backdrop" role="dialog" aria-modal="true" aria-labelledby="failure-guidance-title">
    <section className="failure-guidance-dialog">
      <div className="failure-guidance-heading">
        <span><CircleAlert size={22} /></span>
        <div><small>{generic ? failure.engine : mapper ? "COLMAP" : "Brush"}</small><h2 id="failure-guidance-title">{title}</h2></div>
        <button type="button" aria-label={t("common.close")} disabled={action !== null} onClick={onClose}><X size={17} /></button>
      </div>
      <p>{description}</p>
      {tips.length > 0 && <><strong>{t("failure.solutions")}</strong><ul>{tips.map((key) => <li key={key}>{t(key)}</li>)}</ul></>}
      {failure.rawMessage && <details><summary>{t("failure.details")}</summary><pre>{deviceLost ? localizePipelineMessage(locale, failure.rawMessage) : failure.rawMessage}</pre></details>}
      <div className="failure-guidance-actions">
        <button type="button" className="secondary" disabled={action !== null || !failure.projectId} onClick={onOpenLogs}>{action === "logs" ? <LoaderCircle className="spin" size={14} /> : <FolderOpen size={14} />}{t("failure.openLogs")}</button>
        <button type="button" className="secondary" disabled={action !== null || !failure.failureId || reportSent} title={!failure.failureId ? t("report.notAvailable") : undefined} onClick={() => setReportOpen(true)}><Send size={14} />{t(reportSent ? "report.sentShort" : "report.open")}</button>
        <button type="button" className="primary" disabled={action !== null || !canContinue || (!failure.projectId && !(failure.taskId && failure.runId))} onClick={onRetry}>{action === "retry" ? <LoaderCircle className="spin" size={14} /> : <Play size={14} fill="currentColor" />}{t("project.continue")}</button>
      </div>
    </section>
  </div>;
}

const qualities: Array<{ value: Quality; label: TranslationKey; description: TranslationKey }> = [
  { value: "fast", label: "quality.fast", description: "quality.fastHint" },
  { value: "balanced", label: "quality.balanced", description: "quality.balancedHint" },
  { value: "high", label: "quality.high", description: "quality.highHint" },
];

const rawMessageOf = pipelineErrorMessage;
const basename = (path: string) => displayPath(path).split(/[\\/]/).at(-1) ?? displayPath(path);
const completedProjectSummary = (result: PipelineResult, draft: TaskDraft): ProjectSummary => ({
  id: result.projectId,
  workspaceTaskId: draft.id,
  taskKind: draft.kind,
  name: basename(result.projectPath),
  status: "completed",
  projectPath: result.projectPath,
  finalPly: result.finalPly,
  fileSize: result.fileSize,
  splatCount: result.splatCount,
  createdAt: result.completedAt,
  completedAt: result.completedAt,
  durationMs: result.durationMs,
  quality: draft.quality,
  sourceName: basename(draft.inputPath ?? result.projectPath),
  registeredRatio: result.registeredRatio,
  points3d: result.points3d,
  failureMessage: null,
});
const resumedProjectSummary = (result: PipelineResult, project: ProjectSummary): ProjectSummary => ({
  ...project,
  id: result.projectId,
  status: "completed",
  projectPath: result.projectPath,
  finalPly: result.finalPly,
  fileSize: result.fileSize,
  splatCount: result.splatCount,
  completedAt: result.completedAt,
  durationMs: result.durationMs,
  registeredRatio: result.registeredRatio,
  points3d: result.points3d,
  failureMessage: null,
});
const parentPath = (path: string) => path.replace(/[\\/][^\\/]+[\\/]?$/, "") || path;
const qualityKey: Record<Quality, TranslationKey> = { fast: "quality.fast", balanced: "quality.balanced", high: "quality.high" };
const statusKey: Record<ProjectStatus, TranslationKey> = { running: "status.running", completed: "status.completed", failed: "status.failed", cancelled: "status.cancelled", interrupted: "status.interrupted" };
const readSavedNumber = (key: string, fallback: number) => {
  try {
    const value = Number(window.localStorage.getItem(key));
    return Number.isFinite(value) && value > 0 ? value : fallback;
  } catch {
    return fallback;
  }
};

function engineReady(engine: EngineStatus) {
  return engine.canStart;
}

function DraftRow({ draft, selected, status, insertion, dragging, onSelect, onDelete, onPointerDown, onPointerMove, onPointerEnd, onKeyboardMove }: {
  draft: TaskDraft;
  selected: boolean;
  status: DraftDisplayStatus;
  insertion: "before" | "after" | null;
  dragging: boolean;
  onSelect: () => void;
  onDelete: () => void;
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerEnd: (event: ReactPointerEvent<HTMLElement>) => void;
  onKeyboardMove: (offset: -1 | 1) => void;
}) {
  const { t } = useI18n();
  const statusKeyByValue: Record<DraftDisplayStatus, TranslationKey> = { standby: "task.idle", preparing: "task.preparing", queued: "task.queued", running: "task.running" };
  return <NewTaskRow draftId={draft.id} title={draftDisplayName(draft, t("workspace.newTask"), t("project.reshoot"),t("inputs.mixed"))} path={draft.inputPath} status={status} statusLabel={t(statusKeyByValue[status])} selected={selected} canReorder={!draft.running} error={draft.error} errorContext={draft.errorContext} insertion={insertion} dragging={dragging} onSelect={onSelect} onDelete={onDelete} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerEnd={onPointerEnd} onKeyboardMove={onKeyboardMove} />;
}


export function App() {
  const { locale, t, toggleLocale } = useI18n();
  const store = useAppStore();
  const { tasks: sharedTasks, syncError: taskSyncError, refreshTasks } = useSharedTasks();
  const loadGaussian = useGaussianTransformStore((state) => state.load);
  const closeGaussian = useGaussianTransformStore((state) => state.close);
  const isRunning = store.phase === "running" || Object.values(sharedTasks).some(taskIsActive);
  const systemAcceleration = store.colmapAcceleration;
  const systemDetectionTemporary = systemAcceleration?.detectionState === "temporarilyUnavailable";
  const systemAccelerationWarning = systemDetectionTemporary || Boolean(systemAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly"].includes(systemAcceleration.reasonCode) && systemAcceleration.backend !== "gpu");
  const liveLogRef = useRef<HTMLDivElement>(null);
  const followLiveLogRef = useRef(true);
  const workspaceRef = useRef<HTMLElement>(null);
  const controlPaneRef = useRef<HTMLElement>(null);
  const projectsPaneRef = useRef<HTMLElement>(null);
  const inputDropZoneRef = useRef<HTMLDivElement>(null);
  const taskScrollPositions = useRef({ control: 0, projects: 0 });
  const previewReleasePromises = useRef(new Map<string, Promise<void>>());
  const releasedPreviewProjects = useRef(new Set<string>());
  const previewSessionSequence = useRef(0);
  const activePreviewSession = useRef<{ projectId: string; sessionId: number } | null>(null);
  const previewCloseWatchdog = useRef<number | null>(null);
  const pipelineCommandPending = useRef(false);
  const pipelineCommandRevision = useRef(0);
  const autoRunNextRef = useRef(false);
  const queueRevisionRef = useRef(0);
  const inputAnalysisPromises = useRef(new Map<string, Promise<boolean>>());
  const inputAnalysisRevisions = useRef(new Map<string, number>());
  const inputRejectionNotices = useRef(new Set<string>());
  const gaussianSelectionRevision = useRef(0);
  const runStartedAt = useRef<number | null>(null);
  const runElapsedOffset = useRef(0);
  const cancellationOverlayTimer = useRef<number | null>(null);
  const pipelineRunningRef = useRef(isRunning);
  const accelerationRequestRevision = useRef(0);
  const [fallbackLiveElapsedMs, setLiveElapsedMs] = useState(0);
  const sharedElapsedMs = useTaskElapsed(store.liveTask);
  const liveElapsedMs = sharedElapsedMs ?? fallbackLiveElapsedMs;
  const [elapsedAnchorRevision, setElapsedAnchorRevision] = useState(0);
  const [isCancellationRequested, setIsCancellationRequested] = useState(false);
  const [showCancellationOverlay, setShowCancellationOverlay] = useState(false);
  const [leftPanePercent, setLeftPanePercent] = useState(() => Math.min(68, Math.max(32, readSavedNumber("ooo-splat-left-pane", 44))));
  const [uiScale, setUiScale] = useState(() => Math.min(140, Math.max(80, readSavedNumber("ooo-splat-ui-scale", 100))));
  const [isResizing, setIsResizing] = useState(false);
  const [viewMode, setViewMode] = useState<"tasks" | "preview">("tasks");
  const [openingPreviewProjectId, setOpeningPreviewProjectId] = useState<string | null>(null);
  const [closingPreviewProjectId, setClosingPreviewProjectId] = useState<string | null>(null);
  const [previewSessionId, setPreviewSessionId] = useState(0);
  const [deletingProjectId, setDeletingProjectId] = useState<string | null>(null);
  const [revealingProjectId, setRevealingProjectId] = useState<string | null>(null);
  const [failureDialog, setFailureDialog] = useState<FailureDialogState | null>(null);
  const [failureDialogAction, setFailureDialogAction] = useState<"retry" | "logs" | null>(null);
  const [showZoomControls, setShowZoomControls] = useState(false);
  const [telemetryPreferences, setTelemetryPreferences] = useState<TelemetryPreferencesState | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const updates = useUpdateNotice();
  const checkUpdatesManually = async () => {
    const result = await updates.check(true);
    if (result?.status === "updateAvailable") setSettingsOpen(false);
  };
  const [uiPreferences, setUiPreferences] = useState(loadUiPreferences);
  const [telemetryBusy, setTelemetryBusy] = useState(false);
  const telemetryRequestRevision = useRef(0);
  const telemetryRequestPending = useRef(false);
  const [inputMenuOpen, setInputMenuOpen] = useState(false);
  const [taskWorkspace, setTaskWorkspace] = useState(loadTaskWorkspace);
  const taskWorkspaceRef = useRef(taskWorkspace);
  taskWorkspaceRef.current = taskWorkspace;
  const [runGateBusy, setRunGateBusy] = useState(false);
  const [autoRunNext, setAutoRunNext] = useState(false);
  const [preflight, setPreflight] = useState<PreflightSession | null>(null);
  const preflightRef = useRef<PreflightSession | null>(null);
  const queueCheckingRef = useRef(false);
  const queueCompletionRef = useRef<Promise<void> | null>(null);
  const [queueChecking, setQueueChecking] = useState(false);
  const [queueInputNotice, setQueueInputNotice] = useState<string | null>(null);
  const queueIssueDraftIdRef = useRef<string | null>(null);
  useEffect(() => () => { preflightRef.current?.resolve(null); preflightRef.current = null; }, []);
  const [inputDropActive, setInputDropActive] = useState(false);
  const [advancedSamplingOpen, setAdvancedSamplingOpen] = useState(false);
  const inputSelectionRevisions = useRef(new Map<string, number>());
  const [draftDrag, setDraftDrag] = useState<{ id: string; insertionIndex: number } | null>(null);
  const draftDragRef = useRef(draftDrag);
  draftDragRef.current = draftDrag;
  const draftDragHoldRef = useRef<DraftDragHold | null>(null);
  const suppressDraftSelectionRef = useRef<string | null>(null);
  const [reorderAnnouncement, setReorderAnnouncement] = useState("");
  const [activeDraftId, setActiveDraftId] = useState<string | null>(() => taskWorkspace.drafts.find((draft) => draft.running)?.id ?? null);
  const [activeProjectId, setActiveProjectId] = useState<string | null>(null);
  const [projectDetail, setProjectDetail] = useState<ProjectTaskDetail | null>(null);
  const [projectEstimate, setProjectEstimate] = useState<{ projectId: string; value: Awaited<ReturnType<typeof estimateProjectRuntime>> } | null>(null);
  const [projectDetailLoading, setProjectDetailLoading] = useState(false);
  const [projectDetailError, setProjectDetailError] = useState<{ projectId: string; message: string; occurredAt: number } | null>(null);
  const [projectActionError, setProjectActionError] = useState<{ projectId: string; message: string; occurredAt: number; errorContext?: FailureContext } | null>(null);
  const [globalDetailError, setGlobalDetailError] = useState<DetailErrorRecord | null>(null);
  const dismissedErrorThrough = useRef(new Map<string, number>());
  const [dismissedErrorRevision, setDismissedErrorRevision] = useState(0);
  const [expandedGroups, setExpandedGroups] = useState<TaskGroupVisibility>(loadTaskGroupVisibility);
  const [reshootBusy, setReshootBusy] = useState(false);
  const [importMenuOpen, setImportMenuOpen] = useState(false);
  const [gaussianImportSelectionId, setGaussianImportSelectionId] = useState<string | null>(null);
  const [unsupportedInput, setUnsupportedInput] = useState<InputAdmissionReason | null>(null);
  const gaussianImportSelectionRef = useRef(gaussianImportSelectionId);
  gaussianImportSelectionRef.current = gaussianImportSelectionId;
  const inputModalOpen = useRef(false);
  inputModalOpen.current = Boolean(unsupportedInput || gaussianImportSelectionId || preflight || settingsOpen || failureDialog || showCancellationOverlay || (telemetryPreferences && !telemetryPreferences.consentDecided));
  const drafts = taskWorkspace.drafts;
  const selectedTask = taskWorkspace.selected;
  const selectedSharedTask = selectedTask.kind === "task" ? sharedTasks[selectedTask.id] : selectedTask.kind === "project" ? Object.values(sharedTasks).find(task => task.project_id === selectedTask.id && !task.project_deleted) : null;
  const submittedTasks=Object.values(sharedTasks).filter(task=>!task.project_deleted).sort((a,b)=>b.created_at.localeCompare(a.created_at));
  const submittedNew=submittedTasks.filter(task=>task.status==="created"||taskIsActive(task));
  const submittedCompleted=submittedTasks.filter(task=>task.status==="completed");
  const submittedUnfinished=submittedTasks.filter(task=>!["created","completed"].includes(task.status)&&!taskIsActive(task));
  const visibleDrafts=drafts.filter(draft=>!sharedTasks[draft.id]);
  const submittedIdsRef = useRef(new Set<string>());
  submittedIdsRef.current = new Set(Object.keys(sharedTasks));
  const submittedIdsKey = Object.keys(sharedTasks).sort().join('|');
  const selectedDraft = selectedTask.kind === "draft" ? drafts.find((draft) => draft.id === selectedTask.id) ?? null : null;
  const selectedHasVideoInput = selectedDraft?.inputs
    ? selectedDraft.inputs.mode === 'materials' && selectedDraft.inputs.items.some(item => item.source.kind === 'video' && !!item.source.path)
    : !!(selectedDraft?.inputPath && !selectedDraft.colmapDiscovery && selectedDraft.inputType === 'video');
  useEffect(() => setAdvancedSamplingOpen(false), [selectedTask.kind, selectedTask.id, selectedHasVideoInput]);
  const selectedProject = selectedTask.kind === "project"
    ? store.projects.find((project) => project.id === selectedTask.id)
      ?? (projectDetail?.project.id === selectedTask.id ? projectDetail.project : null)
    : null;
  const detailContextKey = selectedSharedTask ? `task:${selectedSharedTask.task_id}` : selectedDraft ? `draft:${selectedDraft.id}` : selectedProject ? `project:${selectedProject.id}` : "global";
  const missingEngines = store.engines.filter((engine) => !engineReady(engine));
  const sharedProjectIds = new Set(Object.values(sharedTasks).filter(task => !task.project_deleted).map(task => task.project_id));
  const completed = store.projects.filter(project => project.status === "completed" && !sharedProjectIds.has(project.id));
  const completedRows = [
    ...submittedCompleted.map(task => ({ kind: "task" as const, id: task.task_id, time: timestampOf(task.result?.completedAt ?? task.updated_at), task })),
    ...completed.map(project => ({ kind: "project" as const, id: project.id, time: timestampOf(project.completedAt ?? project.createdAt), project })),
  ].sort((left, right) => right.time - left.time || right.id.localeCompare(left.id));
  const unfinished = useMemo(() => store.projects.filter((project) => project.status !== "completed" && !sharedProjectIds.has(project.id) && !drafts.some((draft) => draft.running && (draft.linkedProjectId === project.id || draft.id === project.workspaceTaskId))), [drafts, store.projects, sharedTasks]);
  const projectSyncKey = Object.values(sharedTasks).filter(task => task.project_id && !task.project_deleted)
    .map(task => `${task.project_id}:${task.run_id}:${task.status}`).sort().join("|");
  useEffect(() => {
    if (!projectSyncKey) return;
    let disposed = false;
    void getProjectOverview().then(overview => {
      if (!disposed) useAppStore.getState().setProjects(overview.projects);
    }).catch(() => { /* Shared snapshots still supply status and results; manual refresh retries metadata. */ });
    return () => { disposed = true; };
  }, [projectSyncKey]);
  const progressEvent = useMemo(() => {
    if (!store.latestEvent || !["failed", "cancelled"].includes(store.latestEvent.stage)) return store.latestEvent;
    return [...store.events].reverse().find((event) => isProgressEvent(event) && !["failed", "cancelled"].includes(event.stage)) ?? null;
  }, [store.events, store.latestEvent]);
  const currentMessage = useProgressMessage(store.phase, progressEvent?.stage ?? null, store.events, store.latestRuntime, progressEvent ?? undefined);
  const liveActivity = useTaskActivityLog(selectedSharedTask ? null : store.liveTask?.task_id, store.liveTask?.run_id,
    store.phase === 'running', store.events, locale);
  const messageOf = useCallback((error: unknown) => rawMessageOf(error) ?? t("error.generic"), [t]);
  const queuedDrafts = useMemo(
    () => queuedDraftIds(drafts, activeDraftId, autoRunNext, Boolean(store.projectsRoot) && missingEngines.length === 0),
    [activeDraftId, autoRunNext, drafts, missingEngines.length, store.projectsRoot],
  );
  const runBusy = isRunning || runGateBusy;
  useEffect(() => {
    setTaskWorkspace(workspace => workspace.selected.kind === "draft" && sharedTasks[workspace.selected.id]
      ? { ...workspace, selected: { kind: "task", id: workspace.selected.id } }
      : workspace);
  }, [sharedTasks]);

  const mutateTaskWorkspace = useCallback((updater: (workspace: typeof taskWorkspace) => typeof taskWorkspace) => {
    const next = updater(taskWorkspaceRef.current);
    taskWorkspaceRef.current = next;
    setTaskWorkspace(next);
  }, []);

  const updateDraft = useCallback((id: string, update: Partial<TaskDraft> | ((draft: TaskDraft) => Partial<TaskDraft>)) => {
    mutateTaskWorkspace((workspace) => ({
      ...workspace,
      drafts: workspace.drafts.map((draft) => {
        if (draft.id !== id) return draft;
        const patch = typeof update === "function" ? update(draft) : update;
        const errorStamp = Object.prototype.hasOwnProperty.call(patch, "error")
          ? { errorAt: patch.error ? Date.now() : null, errorContext: patch.errorContext }
          : {};
        return { ...draft, ...patch, ...errorStamp };
      }),
    }));
  }, [mutateTaskWorkspace]);

  useGpsDetection(drafts, updateDraft);
  useEffect(() => {
    const task = store.liveTask;
    const summary = task?.gps_assistance;
    const draft = drafts.find(candidate => candidate.id === activeDraftId && candidate.running);
    if (!task || !summary || !draft?.inputAnalyses || task.task_id !== draft.id || draft.inputs?.mode !== 'materials'
      || (draft.linkedProjectId && task.project_id !== draft.linkedProjectId)) return;
    // Only structured summaries from this live run may update known sources.
    // A task-wide log line cannot establish which source supplied usable GPS.
    const detected = new Set(summary.detectedSourceIds);
    let changed = false;
    const inputAnalyses = Object.fromEntries(Object.entries(draft.inputAnalyses).map(([id, analysis]) => {
      if (!detected.has(id)) return [id, analysis];
      const gpsFallback = summary.status === 'fallback' || summary.status === 'unavailable';
      if (analysis.gpsDetected && analysis.gpsFallback === gpsFallback) return [id, analysis];
      changed = true;
      return [id, { ...analysis, gpsDetected: true, gpsFallback }];
    }));
    if (changed) updateDraft(draft.id, { inputAnalyses });
  }, [store.liveTask, drafts, activeDraftId, updateDraft]);

  useEffect(() => {
    if (!store.error) {
      setGlobalDetailError(null);
      return;
    }
    const occurredAt = store.errorAt ?? Date.now();
    setGlobalDetailError((current) => current?.occurredAt === occurredAt && current.message === store.error
      ? current
      : { message: store.error!, occurredAt, source: "global", contextKey: detailContextKey });
  }, [detailContextKey, store.error, store.errorAt]);

  const visibleDetailError = useMemo<DetailErrorRecord | null>(() => {
    const candidates: DetailErrorRecord[] = [];
    if (selectedDraft?.error && selectedDraft.error !== selectedDraft.inputError) candidates.push({
      message: selectedDraft.error,
      errorContext: selectedDraft.errorContext,
      occurredAt: selectedDraft.errorAt ?? 0,
      source: "draft",
      contextKey: detailContextKey,
    });
    if (selectedProject) {
      if (projectActionError?.projectId === selectedProject.id) candidates.push({ ...projectActionError, source: "action", contextKey: detailContextKey });
      if (projectDetailError?.projectId === selectedProject.id) candidates.push({ ...projectDetailError, source: "detail", contextKey: detailContextKey });
      if (selectedProject.failureMessage) candidates.push({
        message: selectedProject.failureMessage,
        occurredAt: timestampOf(selectedProject.completedAt ?? selectedProject.createdAt),
        source: "persisted",
        contextKey: detailContextKey,
      });
    }
    if (globalDetailError?.contextKey === detailContextKey) candidates.push(globalDetailError);
    const dismissedAt = dismissedErrorThrough.current.get(detailContextKey) ?? -1;
    return candidates
      .filter((error) => error.occurredAt > dismissedAt)
      .sort((left, right) => right.occurredAt - left.occurredAt || ERROR_SOURCE_PRIORITY[right.source] - ERROR_SOURCE_PRIORITY[left.source])[0] ?? null;
  }, [detailContextKey, dismissedErrorRevision, globalDetailError, projectActionError, projectDetailError, selectedDraft, selectedProject]);

  const closeVisibleDetailError = useCallback(() => {
    if (!visibleDetailError) return;
    dismissedErrorThrough.current.set(visibleDetailError.contextKey, visibleDetailError.occurredAt);
    setDismissedErrorRevision((revision) => revision + 1);
    if (visibleDetailError.source === "draft" && selectedDraft) updateDraft(selectedDraft.id, { error: null });
    if (visibleDetailError.source === "action") setProjectActionError(null);
    if (visibleDetailError.source === "detail") setProjectDetailError(null);
    if (visibleDetailError.source === "global") store.setError(null);
  }, [selectedDraft, store.setError, updateDraft, visibleDetailError]);

  const changeRuntimePanelPreference = useCallback((showRuntimePanel: boolean) => {
    setUiPreferences((current) => {
      const next = { ...current, showRuntimePanel };
      saveUiPreferences(next);
      return next;
    });
  }, []);

  const claimRunGate = useCallback(() => {
    if (pipelineCommandPending.current || useAppStore.getState().phase === "running") return false;
    pipelineCommandPending.current = true;
    pipelineCommandRevision.current += 1;
    setRunGateBusy(true);
    return true;
  }, []);

  const releaseRunGate = useCallback(() => {
    pipelineCommandPending.current = false;
    setRunGateBusy(false);
  }, []);

  const setAutoQueueEnabled = useCallback((enabled: boolean) => {
    autoRunNextRef.current = enabled;
    queueRevisionRef.current += 1;
    setAutoRunNext(enabled);
  }, []);

  async function refreshAfterTaskConflict() {
    const command = pipelineCommandRevision.current;
    setAutoQueueEnabled(false);
    queueIssueDraftIdRef.current = null;
    setQueueInputNotice(null);
    const refreshed = await refreshTasks().catch(() => null);
    const current = useAppStore.getState();
    // Only retire this command's optimistic start after a successful snapshot
    // proves no task is active. New live events and failed refreshes win.
    if (refreshed && !refreshed.hasActiveTask && pipelineCommandPending.current
      && pipelineCommandRevision.current === command && current.phase === 'running'
      && !current.liveTask && !current.latestEvent) current.setPhase('idle');
  }

  const showUnsupportedInput = useCallback((reason: InputAdmissionReason) => {
    setAutoQueueEnabled(false);
    setUnsupportedInput(reason);
    setInputDropActive(false);
  }, [setAutoQueueEnabled]);

  useEffect(() => {
    if (!selectedDraft?.inputRejection || selectedDraft.running) return;
    const key = `${selectedDraft.id}:${selectedDraft.inputPath}:${selectedDraft.inputRejection}`;
    if (inputRejectionNotices.current.has(key)) return;
    inputRejectionNotices.current.add(key);
    showUnsupportedInput(selectedDraft.inputRejection);
  }, [selectedDraft?.id, selectedDraft?.inputPath, selectedDraft?.inputRejection, selectedDraft?.running, showUnsupportedInput]);

  function finishPreflight(answers: PreflightAnswers | null) {
    const session = preflightRef.current;
    preflightRef.current = null;
    setPreflight(null);
    session?.resolve(answers);
  }

  async function confirmPreflight(questions: PreflightQuestion[], finalLabel: string): Promise<PreflightAnswers | null> {
    if (!questions.length) return {};
    if (preflightRef.current) return null;
    const answers = await new Promise<PreflightAnswers | null>(resolve => {
      const session = { questions, finalLabel, resolve };
      preflightRef.current = session;
      setPreflight(session);
    });
    if (!answers || !questions.every(question => answerIsValid(question, answers[question.id]))) return null;
    // Apply answers atomically only to the exact task/input snapshots shown.
    for (const question of questions) {
      const current = taskWorkspaceRef.current.drafts.find(draft => draft.id === question.draft.id);
      if (!current || current.inputChecking || preflightSnapshot(current) !== question.snapshot) {
        if (current) updateDraft(current.id, { error: t('preflight.changed') });
        return null;
      }
    }
    return answers;
  }

  function showQueueInputIssue(issue: NonNullable<ReturnType<typeof firstQueueInputIssue>>) {
    setAutoQueueEnabled(false);
    queueIssueDraftIdRef.current = issue.draft.id;
    const unresolved=issue.draft.inputs?.mode==='materials'?issue.draft.inputs.items.find(item=>item.projection==='auto'||!issue.draft.inputAnalyses?.[item.id]||(issue.reason==='imageOrder'&&(issue.draft.inputAnalyses?.[item.id]?.imageSequence?.imageCount??0)>100&&!item.confirmedImageOrder)):undefined;
    const inputType=unresolved?(unresolved.source.kind==='video'?'video':'images'):issue.draft.inputType;
    const name=unresolved?basename(sourcePath(unresolved.source)):draftDisplayName(issue.draft,t('workspace.newTask'),t('project.reshoot'),t('inputs.mixed'));
    if (issue.draft.inputRejection) {
      setQueueInputNotice(t(issue.draft.inputRejection === 'colmap_disabled' ? 'input.colmapDisabled' : 'input.unsupportedSelection'));
      showUnsupportedInput(issue.draft.inputRejection);
    } else setQueueInputNotice(t(issue.reason==='imageOrder'?'source.queueChooseImageOrder':issue.reason==='colmap'?'source.queueResolveColmap':issue.reason==='projection'?(inputType==='images'?'source.queueChooseImageType':'source.queueChooseType'):issue.reason==='checking'?'source.queueChecking':'source.queueAnalysisFailed',{name}));
    selectDraft(issue.draft.id);
  }

  function chooseVideoProjection(id: string, identity: string, value: import('../types/pipeline').VideoProjection) {
    const draft = taskWorkspaceRef.current.drafts.find(item => item.id === id);
    if (!draft || draft.running || draft.inputChecking || projectionIdentity(draft) !== identity || !draft.projectionDetection?.requiresChoice) return;
    if (value === 'equirectangular' && !draft.projectionDetection.supportedErp) return;
    queueRevisionRef.current += 1;
    updateDraft(id, { videoProjection: value, projectionAnswer: value === 'auto' ? null : { identity, projection: value }, error: null });
    queueIssueDraftIdRef.current = null;
    setQueueInputNotice(null);
  }

  async function checkQueueInputs(enable: boolean) {
    if (queueCheckingRef.current || preflightRef.current) return;
    queueCheckingRef.current = true;
    setQueueChecking(true);
    setQueueInputNotice(null);
    let complete!: () => void;
    const revision = queueRevisionRef.current;
    queueCompletionRef.current = new Promise<void>(resolve => { complete = resolve; });
    try {
      while (true) {
        const draft = taskWorkspaceRef.current.drafts.find(item => !item.running && item.inputPath && (item.needsValidation || item.inputChecking || (!(item.kind === 'reshoot' ? item.reshootPlan : item.plan) && !item.inputError && !item.error)));
        if (!draft?.inputPath) break;
        const pending = inputAnalysisPromises.current.get(draft.id);
        if (pending) await pending;
        else if (!draft.inputChecking) {
          if (draft.kind === 'reshoot' && draft.sourceProjectId) await analyzeReshoot(draft.id, draft.sourceProjectId, draft.inputPath, draft.inputType);
          else await analyze(draft.id, draft.inputPath, draft.quality, draft.plannerEnabled);
        }
        else break;
      }
      const issue = firstQueueInputIssue(taskWorkspaceRef.current.drafts);
      if (issue) showQueueInputIssue(issue);
      else {
        queueIssueDraftIdRef.current = null;
        if (enable && revision === queueRevisionRef.current) setAutoQueueEnabled(true);
      }
    } catch (error) {
      setAutoQueueEnabled(false);
      useAppStore.getState().setError(messageOf(error));
    } finally { queueCheckingRef.current = false; queueCompletionRef.current = null; complete(); setQueueChecking(false); }
  }

  // A new ambiguous source stops the queue; type selection stays in its media picker.
  useEffect(() => {
    if (autoRunNext && !queueChecking) {
      const issue = firstQueueInputIssue(drafts);
      if (issue && issue.reason !== 'checking') showQueueInputIssue(issue);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRunNext, drafts, queueChecking]);

  const selectDraft = useCallback((id: string) => {
    mutateTaskWorkspace((workspace) => ({ ...workspace, selected: { kind: "draft", id } }));
    setProjectDetail(null);
    setProjectEstimate(null);
    setProjectDetailError(null);
    setProjectActionError(null);
  }, [mutateTaskWorkspace]);

  const selectProject = useCallback((project: ProjectSummary) => {
    mutateTaskWorkspace((workspace) => ({ ...workspace, selected: { kind: "project", id: project.id } }));
    setProjectDetail(null);
    setProjectEstimate(null);
    setProjectDetailError(null);
    setProjectActionError((error) => error?.projectId === project.id ? error : null);
  }, [mutateTaskWorkspace]);

  const promoteCompletedDraft = useCallback((draft: TaskDraft, result: PipelineResult) => {
    const project = completedProjectSummary(result, draft);
    const currentProjects = useAppStore.getState().projects;
    useAppStore.getState().setProjects([
      project,
      ...currentProjects.filter((item) => item.id !== project.id),
    ]);
    const nextIndex = Math.max(0, taskWorkspaceRef.current.drafts.findIndex((item) => item.id === draft.id));
    mutateTaskWorkspace((workspace) => ({
      ...workspace,
      drafts: workspace.drafts.filter((item) => item.id !== draft.id),
      selected: { kind: "project", id: project.id },
    }));
    setProjectDetail(null);
    setProjectEstimate(null);
    setProjectDetailError(null);
    setProjectActionError(null);
    return { project, nextIndex };
  }, [mutateTaskWorkspace]);

  const addGenerationDraft = useCallback(() => {
    queueRevisionRef.current += 1;
    setExpandedGroups((groups) => ({ ...groups, new: true }));
    mutateTaskWorkspace((workspace) => {
      const ordinal = nextGenerationOrdinal(workspace.drafts);
      const draft = createGenerationDraft(ordinal, useAppStore.getState().plannerEnabled);
      return { drafts: [...workspace.drafts, draft], selected: { kind: "draft", id: draft.id }, nextOrdinal: Math.max(workspace.nextOrdinal, ordinal + 1) };
    });
  }, [mutateTaskWorkspace]);

  const toggleTaskGroup = useCallback((group: TaskGroupKey) => {
    setExpandedGroups((groups) => ({ ...groups, [group]: !groups[group] }));
  }, []);

  const chooseGaussianImport = useCallback(async (kind: "file" | "folder") => {
    const revision = ++gaussianSelectionRevision.current;
    setImportMenuOpen(false);
    try {
      const selection = await selectGaussianImportSource(kind);
      if (gaussianSelectionRevision.current !== revision) return;
      if (selection) setGaussianImportSelectionId(selection.selectionId);
    } catch (error) {
      if (gaussianSelectionRevision.current !== revision) return;
      const reason = inputAdmissionReason(error);
      if (reason) showUnsupportedInput(reason);
      else useAppStore.getState().setError(messageOf(error));
    }
  }, [messageOf, showUnsupportedInput]);

  const finishGaussianImport = useCallback((project: ProjectSummary) => {
    useAppStore.getState().setProjects([project, ...useAppStore.getState().projects.filter(item => item.id !== project.id)]);
    setGaussianImportSelectionId(null);
    setExpandedGroups(groups => ({ ...groups, completed: true }));
    selectProject(project);
  }, [selectProject]);

  const relinkImportedSource = useCallback(async (projectId: string) => {
    try {
      const selection = await selectGaussianImportSource("file");
      if (!selection) return;
      const updated = await relinkImportedProject(projectId, selection.selectionId);
      const projects = useAppStore.getState().projects;
      useAppStore.getState().setProjects(projects.map(item => item.id === updated.id ? updated : item));
      setProjectDetail(null);
    } catch (error) {
      setProjectActionError({ projectId, message: messageOf(error), occurredAt: Date.now() });
    }
  }, [messageOf]);

  const removeDraft = useCallback((id: string) => {
    queueRevisionRef.current += 1;
    mutateTaskWorkspace((workspace) => {
      const target = workspace.drafts.find((draft) => draft.id === id);
      if (!target || target.running) return workspace;
      let drafts = workspace.drafts.filter((draft) => draft.id !== id);
      let nextOrdinal = workspace.nextOrdinal;
      if (!drafts.some((draft) => draft.kind === "generation" && !draft.running)) {
        const ordinal = nextGenerationOrdinal(drafts);
        drafts = [...drafts, createGenerationDraft(ordinal, useAppStore.getState().plannerEnabled)];
        nextOrdinal = Math.max(nextOrdinal, ordinal + 1);
      }
      const selected = workspace.selected.kind === "draft" && workspace.selected.id === id
        ? { kind: "draft" as const, id: drafts[0].id }
        : workspace.selected;
      return { drafts, selected, nextOrdinal };
    });
  }, [mutateTaskWorkspace]);

  const applyDraftOrder = useCallback((draftId: string, insertionIndex: number) => {
    if (submittedIdsRef.current.has(draftId)) return;
    queueRevisionRef.current += 1;
    mutateTaskWorkspace((workspace) => ({
      ...workspace,
      drafts: reorderDrafts(workspace.drafts, draftId, insertionIndex),
    }));
  }, [mutateTaskWorkspace]);

  const moveDraftWithKeyboard = useCallback((draftId: string, offset: -1 | 1) => {
    if (submittedIdsRef.current.has(draftId)) return;
    queueRevisionRef.current += 1;
    mutateTaskWorkspace(workspace => {
      const visible = workspace.drafts.filter(draft => !draft.running && !submittedIdsRef.current.has(draft.id));
      const position = visible.findIndex(draft => draft.id === draftId);
      const target = visible[position + offset];
      if (position < 0 || !target) return workspace;
      const boundary = workspace.drafts.findIndex(draft => draft.id === target.id) + (offset === 1 ? 1 : 0);
      const drafts = reorderDrafts(workspace.drafts, draftId, boundary);
      setReorderAnnouncement(t("workspace.reordered", { position: position + offset + 1 }));
      return { ...workspace, drafts };
    });
  }, [mutateTaskWorkspace, t]);

  const activateDraftDrag = useCallback((hold: DraftDragHold) => {
    if (draftDragHoldRef.current !== hold || hold.active) return false;
    const currentDraft = taskWorkspaceRef.current.drafts.find((draft) => draft.id === hold.draftId);
    if (!currentDraft || currentDraft.running || submittedIdsRef.current.has(hold.draftId)) {
      if (hold.target.hasPointerCapture(hold.pointerId)) hold.target.releasePointerCapture(hold.pointerId);
      draftDragHoldRef.current = null;
      return false;
    }
    hold.active = true;
    const insertionIndex = taskWorkspaceRef.current.drafts.findIndex((draft) => draft.id === hold.draftId);
    const next = { id: hold.draftId, insertionIndex };
    draftDragRef.current = next;
    setDraftDrag(next);
    return true;
  }, []);

  const beginDraftDrag = useCallback((event: ReactPointerEvent<HTMLElement>, draftId: string) => {
    const origin = event.target instanceof Element ? event.target : null;
    if (event.button !== 0 || origin?.closest("button, a, input, textarea, select") || submittedIdsRef.current.has(draftId) || taskWorkspaceRef.current.drafts.find((draft) => draft.id === draftId)?.running) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const hold = {
      pointerId: event.pointerId,
      draftId,
      startX: event.clientX,
      startY: event.clientY,
      target: event.currentTarget,
      timer: 0,
      active: false,
    };
    hold.timer = window.setTimeout(() => activateDraftDrag(hold), DRAFT_DRAG_HOLD_MS);
    draftDragHoldRef.current = hold;
  }, [activateDraftDrag]);

  const updateDraftDrag = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    const hold = draftDragHoldRef.current;
    if (!hold || hold.pointerId !== event.pointerId || !event.currentTarget.hasPointerCapture(event.pointerId)) return;
    if (!hold.active) {
      if (Math.hypot(event.clientX - hold.startX, event.clientY - hold.startY) < DRAFT_DRAG_START_DISTANCE) return;
      if (!activateDraftDrag(hold)) return;
    }
    const current = draftDragRef.current;
    if (!hold.active || !current || submittedIdsRef.current.has(current.id)) return;
    event.preventDefault();
    const rows = Array.from(projectsPaneRef.current?.querySelectorAll<HTMLElement>(".new-task-group .draft-row[data-draft-id]:not(.drag-disabled)") ?? []);
    const fullDrafts = taskWorkspaceRef.current.drafts;
    let insertionIndex = rows.length ? fullDrafts.findIndex(draft => draft.id === rows.at(-1)!.dataset.draftId) + 1 : 0;
    for (let index = 0; index < rows.length; index += 1) {
      const bounds = rows[index].getBoundingClientRect();
      if (event.clientY < bounds.top + bounds.height / 2) {
        insertionIndex = fullDrafts.findIndex(draft => draft.id === rows[index].dataset.draftId);
        break;
      }
    }
    if (insertionIndex !== current.insertionIndex) {
      const next = { ...current, insertionIndex };
      draftDragRef.current = next;
      setDraftDrag(next);
    }
  }, [activateDraftDrag]);

  const endDraftDrag = useCallback((event: ReactPointerEvent<HTMLElement>) => {
    const hold = draftDragHoldRef.current;
    if (!hold || hold.pointerId !== event.pointerId) return;
    window.clearTimeout(hold.timer);
    const current = hold.active && event.type !== "pointercancel" ? draftDragRef.current : null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    draftDragHoldRef.current = null;
    draftDragRef.current = null;
    setDraftDrag(null);
    if (current) {
      suppressDraftSelectionRef.current = current.id;
      applyDraftOrder(current.id, current.insertionIndex);
      window.setTimeout(() => {
        if (suppressDraftSelectionRef.current === current.id) suppressDraftSelectionRef.current = null;
      }, 0);
    }
  }, [applyDraftOrder]);

  useEffect(() => () => {
    const hold = draftDragHoldRef.current;
    if (hold) window.clearTimeout(hold.timer);
  }, []);

  const clearCancellationFeedback = useCallback(() => {
    if (cancellationOverlayTimer.current != null) {
      window.clearTimeout(cancellationOverlayTimer.current);
      cancellationOverlayTimer.current = null;
    }
    setIsCancellationRequested(false);
    setShowCancellationOverlay(false);
  }, []);

  const refreshProjects = async () => {
    const overview = await getProjectOverview();
    const plannerDefault = overview.plannerEnabled ?? true;
    store.setProjectsRoot(overview.projectsRoot);
    store.setPlannerEnabled(plannerDefault);
    store.setProjects(overview.projects);
    setTaskWorkspace((workspace) => ({
      ...workspace,
      drafts: workspace.drafts.map((draft) => draft.plannerDefaultPending
        ? { ...draft, plannerEnabled: plannerDefault, plannerDefaultPending: false }
        : draft),
    }));
    return overview;
  };

  const refreshCompletedProject = async (project: ProjectSummary) => {
    try {
      const overview = await refreshProjects();
      if (!overview.projects.some((item) => item.id === project.id)) {
        throw new Error(t("project.syncMissing"));
      }
      setProjectActionError(null);
    } catch (error) {
      const currentProjects = useAppStore.getState().projects;
      if (!currentProjects.some((item) => item.id === project.id)) {
        useAppStore.getState().setProjects([project, ...currentProjects]);
      }
      setProjectActionError({
        projectId: project.id,
        message: t("project.syncFailed", { detail: messageOf(error) }),
        occurredAt: Date.now(),
      });
    }
  };

  const reconcileRuntimeState = useCallback(async () => {
    const runtime = await getAppRuntimeStatus();
    const appState = useAppStore.getState();
    appState.setTaskColmapAcceleration(runtime.taskAcceleration ?? null);
    if (runtime.pipelineRunning) {
      runElapsedOffset.current = runtime.pipelineElapsedOffsetMs ?? 0;
      const runElapsed = runtime.pipelineRunElapsedMs ?? 0;
      runStartedAt.current = Date.now() - runElapsed;
      setLiveElapsedMs(runElapsedOffset.current + runElapsed);
      setElapsedAnchorRevision((revision) => revision + 1);
      if (appState.phase !== "running") appState.setPhase("running");
      setTaskWorkspace((workspace) => {
        const persistedId = runtime.pipelineWorkspaceTaskId;
        const fallbackId = workspace.selected.kind === "draft"
          ? workspace.selected.id
          : workspace.drafts.find((draft) => draft.running)?.id;
        const runningId = persistedId ?? fallbackId ?? null;
        if (!runningId) return workspace;
        setActiveDraftId(runningId);
        return {
          ...workspace,
          drafts: workspace.drafts.map((draft) => draft.id === runningId
            ? { ...draft, running: true, linkedProjectId: runtime.pipelineProjectId ?? draft.linkedProjectId }
            : { ...draft, running: false }),
        };
      });
    } else {
      setTaskWorkspace((workspace) => ({
        ...workspace,
        drafts: workspace.drafts.map((draft) => draft.running ? { ...draft, running: false } : draft),
      }));
    }
    setActiveProjectId(runtime.pipelineProjectId ?? null);
    if (!runtime.pipelineRunning && !pipelineCommandPending.current) {
      runStartedAt.current = null;
      if (appState.phase === "running") appState.setPhase("idle");
      clearCancellationFeedback();
      setActiveDraftId(null);
      setActiveProjectId(null);
    }
    const active = activePreviewSession.current;
    if (viewMode === "tasks" && active && runtime.previewProjectId !== active.projectId) {
      if (previewCloseWatchdog.current != null) {
        window.clearTimeout(previewCloseWatchdog.current);
        previewCloseWatchdog.current = null;
      }
      activePreviewSession.current = null;
      if (useGaussianTransformStore.getState().descriptor?.projectId === active.projectId) closeGaussian();
      setClosingPreviewProjectId((current) => current === active.projectId ? null : current);
    }
    return runtime;
  }, [clearCancellationFeedback, closeGaussian, viewMode]);

  const refreshSystemAcceleration = useCallback(async () => {
    const revision = ++accelerationRequestRevision.current;
    const acceleration = await checkColmapAcceleration();
    if (revision === accelerationRequestRevision.current && acceleration) {
      useAppStore.getState().setColmapAcceleration(acceleration);
    }
  }, []);

  useEffect(() => {
    void getProjectOverview()
      .then((overview) => {
        const plannerDefault = overview.plannerEnabled ?? true;
        store.setError(null);
        store.setProjectsRoot(overview.projectsRoot);
        store.setPlannerEnabled(plannerDefault);
        store.setProjects(overview.projects);
        setTaskWorkspace((workspace) => {
          const linked = new Set(overview.projects.map((project) => project.workspaceTaskId).filter(Boolean));
          const drafts = workspace.drafts
            .filter((draft) => !linked.has(draft.id))
            .map((draft) => draft.plannerDefaultPending
              ? { ...draft, plannerEnabled: plannerDefault, plannerDefaultPending: false }
              : draft);
          const promotedProject = workspace.selected.kind === "draft"
            ? overview.projects.find((project) => project.workspaceTaskId === workspace.selected.id)
            : null;
          const selectedExists = workspace.selected.kind === "draft"
            ? drafts.some((draft) => draft.id === workspace.selected.id)
            : overview.projects.some((project) => project.id === workspace.selected.id);
          const selected = promotedProject
            ? { kind: "project" as const, id: promotedProject.id }
            : selectedExists
              ? workspace.selected
              : { kind: "draft" as const, id: drafts[0].id };
          return { ...workspace, drafts, selected };
        });
      })
      .catch((error) => store.setError(messageOf(error)));
    const revision = ++accelerationRequestRevision.current;
    void checkEngines()
      .then((engines) => {
        store.setEngines(engines);
        if (revision === accelerationRequestRevision.current) {
          store.setColmapAcceleration(engines.find((engine) => engine.kind === "colmap")?.acceleration ?? null);
        }
      })
      .catch(() => undefined);
  }, [store.setEngines, store.setProjects, store.setProjectsRoot, store.setPlannerEnabled, store.setColmapAcceleration, store.setError]);

  useEffect(() => {
    const retry = () => {
      if (document.visibilityState === "visible") void refreshSystemAcceleration().catch(() => undefined);
    };
    const interval = window.setInterval(retry, 15_000);
    document.addEventListener("visibilitychange", retry);
    return () => {
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", retry);
    };
  }, [refreshSystemAcceleration]);

  useEffect(() => {
    if (viewMode === "tasks") void reconcileRuntimeState().catch(() => undefined);
    const onFocus = () => {
      void reconcileRuntimeState().catch(() => undefined);
      void refreshSystemAcceleration().catch(() => undefined);
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [reconcileRuntimeState, refreshSystemAcceleration, viewMode]);

  useEffect(() => {
    let disposed = false;
    const revision = telemetryRequestRevision.current;
    void initializeTelemetry()
      .then((preferences) => {
        if (!disposed && telemetryRequestRevision.current === revision) setTelemetryPreferences(preferences);
      })
      .catch((error) => {
        if (!disposed && telemetryRequestRevision.current === revision) {
          setTelemetryPreferences({ analyticsEnabled: false, consentDecided: true, deliveryStatus: "notConfigured", settingsError: rawMessageOf(error) ?? String(error) });
        }
      });
    return () => { disposed = true; };
  }, []);

  // The install guard must read live state: a download can outlive many renders.
  useEffect(() => { pipelineRunningRef.current = isRunning; }, [isRunning]);

  // The log keeps only the most recent 500 events, so depend on the replaced array rather
  // than its length. Updating scrollTop directly confines auto-follow to the log viewport
  // and never moves the surrounding task pane.
  useEffect(() => {
    if (store.events.length === 0) {
      followLiveLogRef.current = true;
      return;
    }
    const log = liveLogRef.current;
    if (log && followLiveLogRef.current) log.scrollTop = log.scrollHeight;
  }, [store.events]);

  const updateLiveLogFollow = useCallback(() => {
    const log = liveLogRef.current;
    if (!log) return;
    followLiveLogRef.current = log.scrollHeight - log.scrollTop - log.clientHeight <= 24;
  }, []);

  useEffect(() => {
    if (!isRunning || store.liveTask) return;
    const startedAt = runStartedAt.current;
    if (startedAt == null) return;
    return startElapsedTicker(startedAt, (elapsed) => {
      setLiveElapsedMs(runElapsedOffset.current + elapsed);
    });
  }, [elapsedAnchorRevision, isRunning, store.liveTask?.run_id]);

  useEffect(() => {
    if (!isRunning) clearCancellationFeedback();
  }, [clearCancellationFeedback, isRunning]);

  useEffect(() => () => {
    if (cancellationOverlayTimer.current != null) window.clearTimeout(cancellationOverlayTimer.current);
    if (previewCloseWatchdog.current != null) window.clearTimeout(previewCloseWatchdog.current);
  }, []);

  useEffect(() => {
    try { window.localStorage.setItem("ooo-splat-left-pane", leftPanePercent.toFixed(1)); } catch { /* optional preference */ }
  }, [leftPanePercent]);

  useEffect(() => {
    try { window.localStorage.setItem("ooo-splat-ui-scale", String(uiScale)); } catch { /* optional preference */ }
  }, [uiScale]);

  useEffect(() => {
    try { window.localStorage.setItem(TASK_GROUPS_STORAGE_KEY, JSON.stringify(expandedGroups)); } catch { /* optional preference */ }
  }, [expandedGroups]);

  useEffect(() => {
    saveTaskWorkspace(taskWorkspace.drafts.filter(draft => !submittedIdsRef.current.has(draft.id)), taskWorkspace.selected, taskWorkspace.nextOrdinal);
  }, [taskWorkspace, submittedIdsKey]);

  useEffect(() => { setInputDropActive(false); }, [selectedTask.kind, selectedTask.id]);

  useEffect(() => {
    if (selectedTask.kind !== "project" || selectedSharedTask) return;
    if (isRunning && selectedTask.id === activeProjectId) {
      setProjectDetail(null);
      setProjectEstimate(null);
      setProjectDetailLoading(false);
      return;
    }
    let cancelled = false;
    setProjectDetailLoading(true);
    setProjectDetailError(null);
    setProjectEstimate(null);
    void getProjectTaskDetail(selectedTask.id)
      .then((detail) => {
        if (!cancelled) {
          setProjectDetail(detail);
          setProjectDetailError(null);
        }
      })
      .catch((error) => {
        if (!cancelled) setProjectDetailError({ projectId: selectedTask.id, message: messageOf(error), occurredAt: Date.now() });
      })
      .finally(() => { if (!cancelled) setProjectDetailLoading(false); });
    const selected = useAppStore.getState().projects.find((project) => project.id === selectedTask.id);
    if (selected?.status !== "completed") {
      void estimateProjectRuntime(selectedTask.id)
        .then((value) => { if (!cancelled) setProjectEstimate({ projectId: selectedTask.id, value }); })
        .catch(() => undefined);
    }
    return () => { cancelled = true; };
  }, [selectedTask.kind, selectedTask.id, activeProjectId, isRunning, messageOf, selectedSharedTask?.task_id]);

  useEffect(() => {
    if (viewMode !== "tasks") return;
    const timer = window.setTimeout(() => {
      if (controlPaneRef.current) controlPaneRef.current.scrollTop = taskScrollPositions.current.control;
      if (projectsPaneRef.current) projectsPaneRef.current.scrollTop = taskScrollPositions.current.projects;
    }, 0);
    return () => window.clearTimeout(timer);
  }, [viewMode]);

  const resizePanes = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!isResizing || !workspaceRef.current) return;
    const bounds = workspaceRef.current.getBoundingClientRect();
    const next = ((event.clientX - bounds.left) / bounds.width) * 100;
    setLeftPanePercent(Math.min(68, Math.max(32, next)));
  };

  const stopResizing = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    setIsResizing(false);
  };

  const changeScale = (delta: number) => setUiScale((current) => Math.min(140, Math.max(80, current + delta)));

  const changeTelemetryConsent = async (enabled: boolean) => {
    if (telemetryRequestPending.current) return;
    telemetryRequestPending.current = true;
    const revision = ++telemetryRequestRevision.current;
    setTelemetryBusy(true);
    if (!enabled) {
      setTelemetryPreferences((previous) => ({ ...previous ?? { consentDecided: true, deliveryStatus: "notConfigured" }, analyticsEnabled: false }));
    }
    try {
      const preferences = await setTelemetryConsent(enabled);
      if (telemetryRequestRevision.current === revision) {
        setTelemetryPreferences((previous) => ({ ...preferences, analyticsEnabled: enabled && preferences.analyticsEnabled, consentDecided: preferences.settingsError && previous?.consentDecided === false ? false : preferences.consentDecided }));
      }
    } catch (error) {
      if (telemetryRequestRevision.current === revision) {
        setTelemetryPreferences((previous) => ({ ...previous ?? { consentDecided: true, deliveryStatus: "notConfigured" }, analyticsEnabled: false, settingsError: messageOf(error) }));
      }
    } finally {
      if (telemetryRequestRevision.current === revision) {
        telemetryRequestPending.current = false;
        setTelemetryBusy(false);
      }
    }
  };

  const rejectDraftInput = (draftId: string, reason: InputAdmissionReason) => {
    updateDraft(draftId, {
      inputChecking: false, inputPlanning: false, inputPlanningMaterialId: null,
      needsValidation: false, plan: null, reshootPlan: null, colmapInspection: null,
      inputRejection: reason, error: null,
      inputError: t(reason === 'colmap_disabled' ? 'input.colmapDisabled' : 'input.unsupportedSelection'),
    });
  };

  const analyzeColmapDiscovery = (draftId: string): Promise<boolean> => {
    const draft = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
    const model = draft?.colmapDiscovery?.selectedModel ?? draft?.colmapDiscovery?.models[0]?.model;
    if (!draft || !model) return Promise.resolve(false);
    const revision = (inputAnalysisRevisions.current.get(draftId) ?? 0) + 1;
    inputAnalysisRevisions.current.set(draftId, revision);
    updateDraft(draftId, { inputChecking: true, needsValidation: false });
    const promise = classifyGenerationPaths([model]).then(() => {
      if (inputAnalysisRevisions.current.get(draftId) === revision) updateDraft(draftId, { inputChecking: false });
      return false;
    }).catch(error => {
      if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
      const reason = inputAdmissionReason(error);
      if (reason) rejectDraftInput(draftId, reason);
      else updateDraft(draftId, { inputChecking: false, inputError: messageOf(error) });
      return false;
    });
    inputAnalysisPromises.current.set(draftId, promise);
    void promise.finally(() => { if (inputAnalysisPromises.current.get(draftId) === promise) inputAnalysisPromises.current.delete(draftId); });
    return promise;
  };

  const analyzeCollection = (draftId:string):Promise<boolean> => {
    const revision=(inputAnalysisRevisions.current.get(draftId)??0)+1;inputAnalysisRevisions.current.set(draftId,revision);
    const draft=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);if(!draft?.inputs)return Promise.resolve(false);
    const snapshot=JSON.stringify([draft.inputs,draft.quality,draft.frameSamplingFps,draft.frameSamplingRatio,draft.generationPolicyVersion]);
    updateDraft(draftId,{inputChecking:true,inputPlanning:draft.inputPlanning||!!draft.plan||Object.values(draft.inputAnalyses??{}).some(analysis=>analysis.plan!=null),estimate:null,inputError:null,error:null,needsValidation:false});
    const promise=(async()=>{try{
      const result=await inspectMaterialInputs(draft.inputs!.mode==='colmap'?{mode:'colmap',input:{...draft.inputs!.input,identity:null}}:draft.inputs!,draft.quality,draftGenerationPolicy(draft));
      const current=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);
      if(inputAnalysisRevisions.current.get(draftId)!==revision||!current?.inputs)return false;
      if(JSON.stringify([current.inputs,current.quality,current.frameSamplingFps,current.frameSamplingRatio,current.generationPolicyVersion])!==snapshot)return analyzeCollection(draftId);
      const analyses=Object.fromEntries(result.materials.map(a=>[a.id,a]));
      if(draft.frameSamplingRatio!=null&&draft.frameSamplingFps==null&&result.materials.length===1&&result.materials[0].video&&draft.frameSamplingRatio*result.materials[0].video.fps>=.1){
        updateDraft(draftId,{frameSamplingFps:Math.max(.1,draft.frameSamplingRatio*result.materials[0].video.fps),frameSamplingRatio:null});
        return analyzeCollection(draftId);
      }
      const inputs:GenerationInputs=current.inputs.mode==='colmap'?{mode:'colmap',input:{...current.inputs.input,identity:result.colmap?.identity}}:{mode:'materials',items:current.inputs.items.map(item=>{const a=analyses[item.id];if(!a)return {...item,identity:null,projection:'auto' as const};const changed=item.identity!==a.identity&&item.identity!==`legacy:${a.legacyIdentity??''}`;return {...item,identity:a.identity,projection:!changed&&item.projection!=='auto'&&(item.projection!=='equirectangular'||a.projectionDetection.supportedErp)?item.projection:a.projectionDetection.confirmedErp?'equirectangular':a.projectionDetection.requiresChoice?'auto':'perspective',...(changed?{confirmedSmallImages:false,confirmedImageOrder:false}:{} )};})};
      const count=result.colmap?.imageCount??result.materials.reduce((n,a)=>n+(a.plan?.estimatedFrames??a.imageSequence?.imageCount??0),0);
      const video=result.materials.find(a=>a.video)?.video??null;
      const source=current.kind==='reshoot'&&current.sourceProjectId?await inspectReshootSource(current.sourceProjectId):null;
      if(inputAnalysisRevisions.current.get(draftId)!==revision)return false;
      const latest=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);if(!latest||JSON.stringify([latest.inputs,latest.quality,latest.frameSamplingFps,latest.frameSamplingRatio,latest.generationPolicyVersion])!==snapshot)return analyzeCollection(draftId);
      updateDraft(draftId,{inputs,inputAnalyses:analyses,colmapInspection:result.colmap,inputChecking:false,inputPlanning:false,inputPlanningMaterialId:null,needsValidation:false,video,inputType:video?'video':'images',imageSequence:video?null:{imageCount:count,width:Math.max(0,...result.materials.map(a=>a.imageSequence?.width??0)),height:Math.max(0,...result.materials.map(a=>a.imageSequence?.height??0)),hasAlpha:result.materials.some(a=>a.imageSequence?.hasAlpha),requiresLargeSequenceConfirmation:false},inputErrors:result.errors,plan:Object.keys(result.errors??{}).length?null:{estimatedFrames:count,samplingFps:current.frameSamplingFps??({fast:6,balanced:8,high:12}[current.quality]),retentionRatio:0},error:null,inputError:Object.values(result.errors??{})[0]??null,...(source?{reshootSource:source,reshootPlan:{compatible:source.eligible,sourceIdentity:JSON.stringify(result.materials.map(a=>a.identity)),inputType:video?'video':'images',imageCount:count,duration:null,preparedWidth:0,preparedHeight:0,estimatedFrames:count,hasAlpha:false,maskCount:0,incompatibilityReason:source.reason,estimate:{estimatedMs:0,lowerBoundMs:0,upperBoundMs:0,confidence:'low',sampleCount:0,basis:''}}}:{} )});
      void estimateMaterialInputs(inputs,draft.quality,draftGenerationPolicy(draft)).then(estimate=>{const latest=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);if(latest&&inputAnalysisRevisions.current.get(draftId)===revision&&latest.quality===draft.quality&&latest.frameSamplingFps===draft.frameSamplingFps&&latest.generationPolicyVersion===draft.generationPolicyVersion)updateDraft(draftId,{estimate});}).catch(()=>{});
      if(inputs.mode==='materials')for(const item of inputs.items){const a=analyses[item.id];if(a?.imageSequence)void detectMaterialGps(item,a.identity).then(detected=>{const latest=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);if(latest?.inputAnalyses?.[item.id]?.identity===a.identity)updateDraft(draftId,{inputAnalyses:{...latest.inputAnalyses,[item.id]:{...latest.inputAnalyses[item.id],gpsDetected:detected}}});}).catch(()=>{});}
      return !Object.keys(result.errors??{}).length;
    }catch(error){if(inputAnalysisRevisions.current.get(draftId)===revision){const reason=inputAdmissionReason(error);if(reason)rejectDraftInput(draftId,reason);else updateDraft(draftId,{inputChecking:false,inputPlanning:false,inputPlanningMaterialId:null,plan:null,inputError:messageOf(error),error:messageOf(error)});}return false;}})();
    inputAnalysisPromises.current.set(draftId,promise);void promise.finally(()=>{if(inputAnalysisPromises.current.get(draftId)===promise)inputAnalysisPromises.current.delete(draftId);});return promise;
  };
  const setCollection=(draftId:string,inputs:GenerationInputs)=>{
    if (inputs.mode === 'colmap' && taskWorkspaceRef.current.drafts.find(draft => draft.id === draftId)?.kind === 'reshoot') {
      updateDraft(draftId, { error: t('reshoot.colmapUnsupported') });
      return;
    }
    inputSelectionRevisions.current.set(draftId,(inputSelectionRevisions.current.get(draftId)??0)+1);
    inputAnalysisRevisions.current.set(draftId,(inputAnalysisRevisions.current.get(draftId)??0)+1);
    const path=inputs.mode==='colmap'?inputs.input.model:inputs.items[0]?sourcePath(inputs.items[0].source):null;
    const policyVersion=taskWorkspaceRef.current.drafts.find(draft=>draft.id===draftId)?.generationPolicyVersion??6;
    queueRevisionRef.current+=1;
    updateDraft(draftId,{inputs,generationPolicyVersion:policyVersion,colmapDiscovery:null,inputPath:path||null,plan:null,inputPlanningMaterialId:null,error:null,inputError:null,inputRejection:null,needsValidation:true,...(inputs.mode!=='materials'||inputs.items.length!==1||inputs.items[0].source.kind!=='video'?{frameSamplingRatio:null}:{} )});
    if(path&&(inputs.mode!=='colmap'||inputs.input.images))void analyzeCollection(draftId);
    else updateDraft(draftId,{inputChecking:false,inputPlanning:false,needsValidation:false,inputAnalyses:{},colmapInspection:null});
  };
  const addMaterialPaths=async(draftId:string,paths:string[],selectionRevision?:number)=>{
    if(!paths.length)return;
    const draft=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);if(!draft||draft.running)return;
    if(draft.inputs?.mode==='colmap'||draft.colmapDiscovery){updateDraft(draftId,{error:t('inputs.exclusive')});return;}
    const revision=selectionRevision??(inputSelectionRevisions.current.get(draftId)??0)+1;
    if(selectionRevision!=null&&inputSelectionRevisions.current.get(draftId)!==selectionRevision)return;
    inputSelectionRevisions.current.set(draftId,revision);
    try{const selection=await classifyGenerationPaths(paths);if(inputSelectionRevisions.current.get(draftId)!==revision)return;const current=taskWorkspaceRef.current.drafts.find(d=>d.id===draftId);if(!current||current.running)return;
      if (current.kind === 'reshoot' && selection.kind !== 'materials') { showUnsupportedInput('colmap_disabled'); return; }
      if(current.inputs?.mode==='colmap'||current.colmapDiscovery){updateDraft(draftId,{error:t('inputs.exclusive')});return;}
      const previous=current.inputs?.mode==='materials'?current.inputs.items:(current.inputPath?[{id:current.id,source:current.inputType==='video'?{kind:'video' as const,path:current.inputPath}:{kind:'imageFolder' as const,path:current.inputPath},projection:resolvedProjection(current),imageMatchingStrategy:'exhaustive' as const}]:[]);
      if(selection.kind==='materials')setCollection(draftId,{mode:'materials',items:[...previous,...selection.items]});
      else if(previous.length)updateDraft(draftId,{error:t('inputs.exclusive')});
      else if(selection.kind==='colmap')setCollection(draftId,{mode:'colmap',input:selection.input});
      else {
        const selectedModel=selection.discovery.models.length===1?selection.discovery.models[0].model:null;
        const discovery={...selection.discovery,selectedModel};
        queueRevisionRef.current+=1;
        updateDraft(draftId,{inputs:null,colmapDiscovery:discovery,inputPath:selectedModel||selection.discovery.models[0]?.model||null,inputChecking:false,needsValidation:false,inputAnalyses:{},colmapInspection:null,plan:null,error:null,inputError:null});
      }
    }catch(error){if(inputSelectionRevisions.current.get(draftId)===revision){const reason=inputAdmissionReason(error);if(reason){const selected=taskWorkspaceRef.current.selected;if(selected.kind==='draft'&&selected.id===draftId)showUnsupportedInput(reason);}else updateDraft(draftId,{error:messageOf(error)});}}
  };
  const chooseMaterials=async(directory:boolean)=>{
    const id=selectedDraft?.id;if(!id)return;
    const revision=(inputSelectionRevisions.current.get(id)??0)+1;inputSelectionRevisions.current.set(id,revision);
    try{const paths=await selectMaterialPaths(directory);if(inputSelectionRevisions.current.get(id)!==revision)return;await addMaterialPaths(id,paths,revision);}
    catch(error){if(inputSelectionRevisions.current.get(id)!==revision)return;const selected=taskWorkspaceRef.current.selected;if(selected.kind!=='draft'||selected.id!==id||!taskWorkspaceRef.current.drafts.some(draft=>draft.id===id))return;const reason=inputAdmissionReason(error);if(reason)showUnsupportedInput(reason);else updateDraft(id,{error:messageOf(error)});}
  };
  const chooseColmapField=async(field:keyof ColmapInput)=>{
    const draft=selectedDraft;if(!draft||field==='identity')return;
    if(draft.colmapDiscovery&&field==='images'){
      const model=draft.colmapDiscovery.selectedModel;if(!model)return;
      const images=await selectColmapFolder('images');if(images)setCollection(draft.id,{mode:'colmap',input:{model,images,database:draft.colmapDiscovery.database??null}});
      return;
    }
    if(draft.inputs?.mode!=='colmap')return;
    const before=JSON.stringify([draft.inputs.input.model,draft.inputs.input.images,draft.inputs.input.database]);
    const path=field==='database'?await selectColmapDatabase():await selectColmapFolder(field);
    if(!path)return;
    const current=taskWorkspaceRef.current.drafts.find(d=>d.id===draft.id);
    if(current?.inputs?.mode==='colmap'&&JSON.stringify([current.inputs.input.model,current.inputs.input.images,current.inputs.input.database])===before)
      setCollection(draft.id,{mode:'colmap',input:{...current.inputs.input,[field]:path,identity:null}});
  };
  const chooseColmapModel=(draftId:string,model:string)=>{const draft=taskWorkspaceRef.current.drafts.find(item=>item.id===draftId);const discovery=draft?.colmapDiscovery;if(!draft||!discovery)return;const candidate=discovery.models.find(item=>item.model===model);if(!candidate)return;if(candidate.imageCandidates.length===1){setCollection(draftId,{mode:'colmap',input:{model,images:candidate.imageCandidates[0],database:discovery.database??null}});return;}queueRevisionRef.current+=1;updateDraft(draftId,{colmapDiscovery:{...discovery,selectedModel:model},inputPath:model,error:null});};
  const materialPicker = (draft: TaskDraft) => {
    const fallbackInputs: GenerationInputs | null = draft.inputs ?? (!draft.colmapDiscovery && draft.inputPath ? {
      mode: 'materials',
      items: [{
        id: draft.id,
        source: draft.inputType === 'video' ? { kind: 'video', path: draft.inputPath } : { kind: 'imageFolder', path: draft.inputPath },
        projection: resolvedProjection(draft),
        imageMatchingStrategy: 'exhaustive',
      }],
    } : null);
    const fallbackAnalyses = draft.inputAnalyses ?? (draft.inputPath && draft.projectionDetection ? {
      [draft.id]: {
        id: draft.id,
        identity: draft.projectionDetection.sourceIdentity ?? '',
        video: draft.video,
        imageSequence: draft.imageSequence,
        plan: draft.plan,
        projectionDetection: draft.projectionDetection,
      },
    } : {});
    return <div ref={inputDropZoneRef} className={`material-drop-zone${inputDropActive ? ' material-drop-active' : ''}`}>
      <MaterialInputs inputs={fallbackInputs} discovery={draft.colmapDiscovery} analyses={fallbackAnalyses}
        colmapBlocked={draft.inputRejection === 'colmap_disabled'}
        errors={draft.inputErrors} checking={draft.inputChecking || draft.needsValidation} disabled={draft.running} dragActive={inputDropActive}
        error={draft.inputError} onFiles={() => void chooseMaterials(false)} onFolders={() => void chooseMaterials(true)}
        onColmapField={field => void chooseColmapField(field)} onColmapModel={model => chooseColmapModel(draft.id, model)}
        onColmapImages={(model, images) => setCollection(draft.id, { mode: 'colmap', input: { model, images, database: draft.colmapDiscovery?.database ?? null } })}
        onRemove={id => {
          inputSelectionRevisions.current.set(draft.id,(inputSelectionRevisions.current.get(draft.id)??0)+1);
          if (id === 'colmap-discovery') {
            queueRevisionRef.current += 1;
            inputAnalysisRevisions.current.set(draft.id, (inputAnalysisRevisions.current.get(draft.id) ?? 0) + 1);
            updateDraft(draft.id, { inputs: null, colmapDiscovery: null, inputPath: null, plan: null, error: null, inputError: null, inputRejection: null, needsValidation: false });
          } else if (draft.inputs?.mode === 'materials') {
            setCollection(draft.id, { mode: 'materials', items: draft.inputs.items.filter(item => item.id !== id) });
          } else setCollection(draft.id, { mode: 'materials', items: [] });
        }}
        onProjection={(id, value) => {
          if (!draft.inputs) { chooseVideoProjection(draft.id, projectionIdentity(draft) ?? '', value); return; }
          if (draft.inputs.mode === 'materials') {
            queueRevisionRef.current += 1;
            queueIssueDraftIdRef.current = null;
            setQueueInputNotice(null);
            updateDraft(draft.id, { inputs: { mode: 'materials', items: draft.inputs.items.map(item => item.id === id ? { ...item, projection: value } : item) } });
          }
        }}
        onImageMatchingStrategy={(id, value) => {
          if (draft.inputs?.mode === 'materials') {
            queueRevisionRef.current += 1;
            queueIssueDraftIdRef.current = null;
            setQueueInputNotice(null);
            updateDraft(draft.id, { inputs: { mode: 'materials', items: draft.inputs.items.map(item => item.id === id ? { ...item, imageMatchingStrategy: value, confirmedImageOrder: true } : item) } });
          }
        }} onRetry={() => void analyzeCollection(draft.id)} />
    </div>;
  };
  const fpsPicker=(draft:TaskDraft)=>{
    if(!draft.inputs)return draft.inputType==='video'?<FrameSamplingControl video={draft.video} quality={draft.quality} value={draft.frameSamplingRatio} disabled={draft.running} onChange={changeFrameSampling}/>:null;
    if(draft.inputs.mode!=='materials')return null;
    const materialItems=draft.inputs.items;
    const defaultTarget={fast:6,balanced:8,high:12}[draft.quality];
    if(draft.generationPolicyVersion<5&&materialItems.length===1){
      const analysis=draft.inputAnalyses?.[materialItems[0].id];
      if(analysis?.video){
        const legacyRatio=draft.frameSamplingFps!=null?draft.frameSamplingFps/analysis.video.fps:draft.frameSamplingRatio;
        return <FrameSamplingControl video={analysis.video} quality={draft.quality} value={legacyRatio} disabled={draft.running} onChange={ratio=>{
          queueRevisionRef.current+=1;
          updateDraft(draft.id,{frameSamplingRatio:ratio,frameSamplingFps:null,inputPlanningMaterialId:null});
          void analyzeCollection(draft.id);
        }}/>;
      }
    }
    const materials=materialItems.flatMap(item=>{const analysis=draft.inputAnalyses?.[item.id];if(!analysis?.video)return[];const legacyTarget=draft.frameSamplingFps??(draft.frameSamplingRatio!=null&&materialItems.length===1?draft.frameSamplingRatio*analysis.video.fps:null)??Math.min(defaultTarget,analysis.video.fps);const legacyRemoval=Math.max(.1,analysis.video.fps-Math.min(analysis.video.fps,legacyTarget));return[{id:item.id,name:basename(sourcePath(item.source)),video:analysis.video,plan:analysis.plan,frameSamplingRange:analysis.frameSamplingRange??null,planning:Boolean(draft.inputPlanning&&(draft.inputPlanningMaterialId==null||draft.inputPlanningMaterialId===item.id)),removalFps:draft.generationPolicyVersion>=5?(item.frameRemovalFps??null):legacyRemoval,defaultRemovalFps:draft.generationPolicyVersion===5?defaultTarget:draft.generationPolicyVersion>=6?undefined:Math.max(.1,analysis.video.fps-Math.min(analysis.video.fps,defaultTarget))}];});
    return <FrameFpsControl materials={materials} quality={draft.quality} disabled={draft.running} onChange={(id,value)=>{
      const inputs={mode:'materials' as const,items:materialItems.map(item=>{if(draft.generationPolicyVersion>=5)return item.id===id?{...item,frameRemovalFps:value}:item;const analysis=draft.inputAnalyses?.[item.id];if(!analysis?.video)return item;const target=draft.frameSamplingFps??(draft.frameSamplingRatio!=null&&materialItems.length===1?draft.frameSamplingRatio*analysis.video.fps:null)??Math.min(defaultTarget,analysis.video.fps);return{...item,frameRemovalFps:item.id===id?value:Math.max(.1,analysis.video.fps-Math.min(analysis.video.fps,target))};})};
      const inputAnalyses=draft.inputAnalyses
        ? Object.fromEntries(Object.entries(draft.inputAnalyses).map(([materialId,analysis])=>[materialId,materialId===id?{...analysis,plan:null}:analysis]))
        : draft.inputAnalyses;
      updateDraft(draft.id,{inputs,inputAnalyses,inputPlanning:true,generationPolicyVersion:draft.generationPolicyVersion,frameSamplingFps:null,frameSamplingRatio:null,inputPlanningMaterialId:id});
      void analyzeCollection(draft.id);
    }}/>;
  };

  const analyze = (draftId: string, path: string, quality: Quality, plannerEnabled: boolean, reuseVideo = false) => {
    const draft = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
    if(draft?.colmapDiscovery)return analyzeColmapDiscovery(draftId);
    if(draft?.inputs)return analyzeCollection(draftId);
    let videoAnalysis: import('../types/pipeline').InputAnalysis | undefined = reuseVideo && draft?.inputPath === path && (draft.video || draft.imageSequence) && draft.projectionDetection?.sourceIdentity
      ? { inputType: draft.inputType, video: draft.video, imageSequence: draft.imageSequence, projectionDetection: draft.projectionDetection } : undefined;
    const revision = (inputAnalysisRevisions.current.get(draftId) ?? 0) + 1;
    inputAnalysisRevisions.current.set(draftId, revision);
    updateDraft(draftId, { error: null, inputError: null, needsValidation: false, inputChecking: true, inputPlanning: !!videoAnalysis });
    const analysis = (async () => {
      try {
        let result: Awaited<ReturnType<typeof probeAndPlan>>;
        // A pending probe/plan owns this draft until its result matches the latest settings.
        // Quality changes during detection must not launch another FFprobe process.
        for (;;) {
          const settings = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
          const policy = settings ? draftGenerationPolicy(settings) : undefined;
          result = await probeAndPlan(path, quality, plannerEnabled, videoAnalysis, metadata => {
            if (inputAnalysisRevisions.current.get(draftId) !== revision) return;
            const current = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
            if (!current || current.inputPath !== path) return;
            const changed = current.projectionAnswer && current.projectionAnswer.identity !== metadata.projectionDetection.sourceIdentity;
            updateDraft(draftId, { ...metadata, inputPlanning: true, ...(changed ? { projectionAnswer: null, videoProjection: 'auto' as const } : {}) });
            videoAnalysis = metadata;
          }, policy);
          if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
          const latest = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
          if (!latest || latest.inputPath !== path) return false;
          if (latest.quality === quality && latest.frameSamplingRatio === policy?.frameSamplingRatio) break;
          quality = latest.quality;
          plannerEnabled = latest.plannerEnabled;
          videoAnalysis = result.projectionDetection?.sourceIdentity
            ? { inputType: result.inputType, video: result.video, imageSequence: result.imageSequence, projectionDetection: result.projectionDetection } : undefined;
        }
        if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
        const current = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
        const changedIdentity = current?.projectionAnswer && current.projectionAnswer.identity !== result.projectionDetection?.sourceIdentity;
        updateDraft(draftId, { inputType: result.inputType, video: result.video, projectionDetection: result.projectionDetection ?? null, imageSequence: result.imageSequence, plan: result.plan, estimate: result.estimate, error: null, inputError: null, inputChecking: false, inputPlanning: false, ...(changedIdentity ? { projectionAnswer: null, videoProjection: 'auto' as const } : {}) });
        return true;
      } catch (error) {
        if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
        updateDraft(draftId, { video: null, projectionDetection: null, imageSequence: null, plan: null, estimate: null, error: messageOf(error), inputError: messageOf(error), inputChecking: false, inputPlanning: false });
        return false;
      }
    })();
    inputAnalysisPromises.current.set(draftId, analysis);
    void analysis.finally(() => {
      if (inputAnalysisPromises.current.get(draftId) === analysis) inputAnalysisPromises.current.delete(draftId);
    });
    return analysis;
  };

  const analyzeReshoot = (draftId: string, sourceProjectId: string, path: string, inputType: InputType) => {
    if(taskWorkspaceRef.current.drafts.find(d=>d.id===draftId)?.colmapDiscovery)return analyzeColmapDiscovery(draftId);
    if(taskWorkspaceRef.current.drafts.find(d=>d.id===draftId)?.inputs)return analyzeCollection(draftId);
    const revision = (inputAnalysisRevisions.current.get(draftId) ?? 0) + 1;
    inputAnalysisRevisions.current.set(draftId, revision);
    updateDraft(draftId, { error: null, needsValidation: false, inputChecking: true, reshootPlan: null });
    setReshootBusy(true);
    const analysis = (async () => {
      try {
        const source = await inspectReshootSource(sourceProjectId);
        if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
        const current = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
        updateDraft(draftId, { reshootSource: source, ...(current?.plannerDefaultPending && !source.imported ? { plannerEnabled: source.plannerEnabled, plannerDefaultPending: false } : {}) });
        if (!source.eligible) {
          updateDraft(draftId, { error: source.reason ?? t("reshoot.ineligible"), inputChecking: false });
          return false;
        }
        let settings = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
        let cached = settings?.video && settings.projectionDetection ? {video:settings.video,projectionDetection:settings.projectionDetection} : undefined;
        let plan: Awaited<ReturnType<typeof probeReshootInput>>;
        for (;;) {
          const policy = settings ? draftGenerationPolicy(settings) : undefined;
          plan = await probeReshootInput(sourceProjectId, path, inputType, settings?.quality, true, policy, cached);
          if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
          const latest = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
          if (!latest || latest.inputPath !== path) return false;
          if (latest.quality === settings?.quality && latest.frameSamplingRatio === policy?.frameSamplingRatio) break;
          settings = latest;
          cached = plan.video && plan.projectionDetection ? {video:plan.video,projectionDetection:plan.projectionDetection} : undefined;
        }
        if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
        updateDraft(draftId, { reshootPlan: plan, video: plan.video ?? null, projectionDetection: plan.projectionDetection ?? null, imageSequence: plan.imageSequence ?? null, estimate: plan.estimate, error: plan.compatible ? null : plan.incompatibilityReason ?? t("reshoot.ineligible"), inputChecking: false });
        return plan.compatible;
      } catch (error) {
        if (inputAnalysisRevisions.current.get(draftId) !== revision) return false;
        updateDraft(draftId, { error: messageOf(error), inputChecking: false });
        return false;
      } finally {
        setReshootBusy(false);
      }
    })();
    inputAnalysisPromises.current.set(draftId, analysis);
    void analysis.finally(() => {
      if (inputAnalysisPromises.current.get(draftId) === analysis) inputAnalysisPromises.current.delete(draftId);
    });
    return analysis;
  };

  const applyInputToDraft = async (draftId: string, path: string, inputType: InputType) => {
    const draft = taskWorkspaceRef.current.drafts.find((item) => item.id === draftId);
    if (!draft || draft.running) return false;
    queueRevisionRef.current += 1;
    updateDraft(draftId, {
      ...(draft.inputPath !== path ? { videoProjection: "auto" as const, projectionDetection: null, projectionAnswer: null, frameSamplingRatio: null } : {}),
      inputPath: path, inputType, video: null, imageSequence: null, plan: null, inputError: null,
      estimate: null, reshootPlan: null, error: null,
    });
    if (draft.kind === "generation") {
      if (!isRunning) {
        store.setInputPath(path, inputType);
        setActiveDraftId(null);
      }
      return analyze(draftId, path, draft.quality, draft.plannerEnabled);
    }
    if (!draft.sourceProjectId) return false;
    return analyzeReshoot(draftId, draft.sourceProjectId, path, inputType);
  };

  const chooseInput = async (inputType: InputType) => {
    if (!selectedDraft || (selectedDraft.kind !== "generation" && !selectedDraft.reshootSource?.imported)) return;
    try {
      const selected = inputType === "images" ? await selectImageSequence() : await selectVideo();
      if (selected) await applyInputToDraft(selectedDraft.id, selected, inputType);
    } catch (error) {
      updateDraft(selectedDraft.id, { error: messageOf(error) });
    }
  };

  const chooseInputType = (inputType: InputType) => {
    setInputMenuOpen(false);
    if (selectedDraft && inputType !== selectedDraft.inputType) {
      inputAnalysisRevisions.current.set(selectedDraft.id, (inputAnalysisRevisions.current.get(selectedDraft.id) ?? 0) + 1);
      queueRevisionRef.current += 1;
      updateDraft(selectedDraft.id, {
        inputPath: null,
        inputType,
        video: null,
        imageSequence: null,
        plan: null,
        estimate: null,
        reshootPlan: null,
        projectionDetection: null,
        projectionAnswer: null,
        videoProjection: 'auto',
        inputChecking: false,
        error: null,
      });
    }
  };

  useEffect(() => {
    let unlisten: undefined | (() => void);
    void onInputDragDrop((event) => {
      const zone = inputDropZoneRef.current;
      if (event.type === "leave" || !zone || inputModalOpen.current) {
        setInputDropActive(false);
        return;
      }
      const bounds = zone.getBoundingClientRect();
      const inside = event.x >= bounds.left && event.x <= bounds.right && event.y >= bounds.top && event.y <= bounds.bottom;
      setInputDropActive(inside);
      if (event.type !== "drop") return;
      setInputDropActive(false);
      if (!inside) return;
      const selection = taskWorkspaceRef.current.selected;
      const draft = selection.kind === "draft" ? taskWorkspaceRef.current.drafts.find((item) => item.id === selection.id) : null;
      if (!draft || draft.running) return;
      void addMaterialPaths(draft.id,event.paths);
    }).then((dispose) => { unlisten = dispose; });
    return () => unlisten?.();
  // Native listener reads the latest workspace through refs and is installed once per locale.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [messageOf, t]);

  const chooseRoot = async () => {
    const selected = await selectProjectsRoot(store.projectsRoot);
    if (!selected) return;
    try {
      const settings = await setProjectsRoot(selected);
      store.setProjectsRoot(settings.projectsRoot);
      store.setPlannerEnabled(settings.plannerEnabled);
      await refreshProjects();
      store.setError(null);
    } catch (error) { store.setError(messageOf(error)); }
  };

  const chooseQuality = async (quality: Quality) => {
    if (!selectedDraft || selectedDraft.running || selectedDraft.quality === quality) return;
    queueRevisionRef.current += 1;
    const inputAnalyses=selectedDraft.inputAnalyses
      ? Object.fromEntries(Object.entries(selectedDraft.inputAnalyses).map(([materialId,analysis])=>[materialId,{...analysis,plan:null}]))
      : selectedDraft.inputAnalyses;
    updateDraft(selectedDraft.id, { quality, generationPolicyVersion:selectedDraft.generationPolicyVersion, inputs:selectedDraft.inputs?.mode==='materials'?{mode:'materials',items:selectedDraft.inputs.items.map(item=>({...item,frameRemovalFps:null}))}:selectedDraft.inputs, inputAnalyses, inputPlanning:true, frameSamplingRatio: null,frameSamplingFps:null,inputPlanningMaterialId:null, plan: null, estimate: null, error: null });
    if (selectedDraft.kind === "generation" && selectedDraft.inputPath && !selectedDraft.inputChecking) await analyze(selectedDraft.id, selectedDraft.inputPath, quality, selectedDraft.plannerEnabled, true);
    if (selectedDraft.kind === "reshoot" && selectedDraft.sourceProjectId && selectedDraft.inputPath && !selectedDraft.inputChecking) await analyzeReshoot(selectedDraft.id, selectedDraft.sourceProjectId, selectedDraft.inputPath, selectedDraft.inputType);
  };

  const changePlannerEnabled = async () => {
    if (!selectedDraft || selectedDraft.running) return;
    const enabled = !selectedDraft.autoRetryEnabled;
    queueRevisionRef.current += 1;
    updateDraft(selectedDraft.id, { autoRetryEnabled: enabled });
  };

  const changeFrameSampling = (ratio: number | null) => {
    if (!selectedDraft || selectedDraft.running) return;
    queueRevisionRef.current += 1;
    updateDraft(selectedDraft.id, { frameSamplingRatio: ratio });
    if (!selectedDraft.inputPath || selectedDraft.inputChecking) return;
    if (selectedDraft.kind === 'generation') void analyze(selectedDraft.id, selectedDraft.inputPath, selectedDraft.quality, true, true);
    else if (selectedDraft.sourceProjectId) void analyzeReshoot(selectedDraft.id, selectedDraft.sourceProjectId, selectedDraft.inputPath, selectedDraft.inputType);
  };

  const requestCancellation = async () => {
    if (!isRunning || isCancellationRequested) return;
    if (activeDraftId && autoRunNextRef.current) setAutoQueueEnabled(false);
    setIsCancellationRequested(true);
    cancellationOverlayTimer.current = window.setTimeout(() => {
      cancellationOverlayTimer.current = null;
      setShowCancellationOverlay(true);
    }, CANCELLATION_OVERLAY_DELAY_MS);
    try {
      await cancelPipeline();
    } catch (error) {
      clearCancellationFeedback();
      const message = t("progress.cancelError", { detail: messageOf(error) });
      if (selectedTask.kind === "draft") updateDraft(selectedTask.id, { error: message });
      else setProjectActionError({ projectId: selectedTask.id, message, occurredAt: Date.now() });
    }
  };

  const generate = async (draftId = selectedDraft?.id, source: "manual" | "queue" = "manual") => {
    const draft = taskWorkspaceRef.current.drafts.find((item) => item.id === draftId);
    if (!draft || (draft.kind !== "generation"&&!draft.inputs) || !draft.inputPath || !draft.plan || !store.projectsRoot) return;
    if (draft.inputChecking || draft.needsValidation || !draftProjectionReady(draft) || (draft.kind==='reshoot'&&!draft.reshootSource?.eligible)) return;
    if (preflightRef.current || queueCheckingRef.current) return;
    if (!claimRunGate()) return;
    let preflightAnswers: PreflightAnswers | null = null;
    try {
      const snapshot = preflightSnapshot(draft);
      preflightAnswers = await confirmPreflight(collectPreflightQuestions([draft]).filter(question => !!draft.inputs || source === 'manual' || question.kind !== 'smallImages'), t('generate.start'));
      const current = taskWorkspaceRef.current.drafts.find(item => item.id === draftId);
      if (!current || snapshot !== preflightSnapshot(current) || current.inputChecking || !current.inputPath || !current.plan) preflightAnswers = null;
    } catch (error) {
      updateDraft(draft.id, { error: messageOf(error) });
      preflightAnswers = null;
    }
    if (!preflightAnswers) {
      releaseRunGate();
      if (autoRunNextRef.current) setAutoQueueEnabled(false);
      return;
    }
    if (!draft?.inputPath || !draft.plan) { releaseRunGate(); return; }
    clearCancellationFeedback();
    runElapsedOffset.current = 0;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(0);
    setFailureDialog(null);
    setActiveDraftId(draft.id);
    selectDraft(draft.id);
    mutateTaskWorkspace((workspace) => {
      let drafts = workspace.drafts.map((item) => item.id === draft.id ? { ...item, running: true, error: null } : { ...item, running: false });
      let nextOrdinal = workspace.nextOrdinal;
      if (!drafts.some((item) => item.kind === "generation" && !item.running)) {
        const ordinal = nextGenerationOrdinal(drafts);
        drafts = [...drafts, createGenerationDraft(ordinal, useAppStore.getState().plannerEnabled)];
        nextOrdinal = Math.max(nextOrdinal, ordinal + 1);
      }
      return { ...workspace, drafts, nextOrdinal };
    });
    store.beginRun();
    let succeeded = false;
    let promoted = false;
    let nextIndex = 0;
    try {
      const options=draft.kind==='reshoot'?{condition:preflightAnswers[`${draft.id}:reshootCondition`] as import('../types/pipeline').ReshootCondition,allowFallback:draft.autoRetryEnabled,sourceIdentity:draft.reshootSource!.sourceIdentity!,inputIdentity:JSON.stringify(Object.values(draft.inputAnalyses??{}).map(a=>a.identity)),projection:'perspective' as const,imageMatchingStrategy:'exhaustive' as const}:undefined;
      const result = draft.inputs?await startInputsPipeline(confirmedInputs(draft,preflightAnswers),draft.quality,store.projectsRoot,draft.id,draftGenerationPolicy(draft),draft.sourceProjectId,options):await startPipeline(draft.inputPath, draft.quality, store.projectsRoot, true, draft.id, resolvedProjection(taskWorkspaceRef.current.drafts.find(item => item.id === draft.id) ?? draft), draft.projectionDetection?.sourceIdentity, imageMatchingStrategy(draft, preflightAnswers), draftGenerationPolicy(draft));
      setLiveElapsedMs((current) => Math.max(current, result.durationMs));
      store.setResult(result);
      store.setPhase("completed");
      succeeded = true;
      const promotion = promoteCompletedDraft(draft, result);
      promoted = true;
      nextIndex = promotion.nextIndex;
      await refreshCompletedProject(promotion.project);
      const queueIssueDraftId = queueIssueDraftIdRef.current;
      if (queueIssueDraftId && taskWorkspaceRef.current.drafts.some(item => item.id === queueIssueDraftId)) selectDraft(queueIssueDraftId);
    } catch (error) {
      if (isTaskAdmissionConflict(error)) {
        store.setError(messageOf(error));
        await refreshAfterTaskConflict();
        return;
      }
      if (runStartedAt.current != null) {
        const backendElapsed = useAppStore.getState().latestEvent?.elapsedMs ?? 0;
        setLiveElapsedMs(Math.max(backendElapsed, Date.now() - runStartedAt.current));
      }
      const admission = inputAdmissionReason(error);
      if (admission) {
        rejectDraftInput(draft.id, admission);
        store.setPhase('idle');
        return;
      }
      const message = messageOf(error);
      updateDraft(draft.id, { error: message, errorContext: pipelineCommandError(error) ?? undefined });
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      const inputChanged=typeof error==='object' && error!==null && 'code' in error && error.code==='INPUT_CHANGED';
      if (inputChanged) {
        store.setPhase('idle');
        await analyze(draft.id, draft.inputPath, draft.quality, draft.plannerEnabled);
        updateDraft(draft.id, { error: t('preflight.changed') });
      } else store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled && !inputChanged) {
        setFailureDialog(inferFailureDialog(error));
      }
    } finally {
      if (!succeeded) {
        try {
          const overview = await refreshProjects();
          const project = overview.projects.find((item) => item.workspaceTaskId === draft.id);
          nextIndex = Math.max(0, taskWorkspaceRef.current.drafts.findIndex((item) => item.id === draft.id));
          if (project) {
            promoted = true;
            mutateTaskWorkspace((workspace) => ({ ...workspace, drafts: workspace.drafts.filter((item) => item.id !== draft.id), selected: { kind: "project", id: project.id } }));
          } else {
            updateDraft(draft.id, { running: false });
          }
        } catch { updateDraft(draft.id, { running: false }); }
      }
      setActiveDraftId(null);
      releaseRunGate();
    }
    if (!succeeded || !promoted) {
      if (autoRunNextRef.current) setAutoQueueEnabled(false);
      return;
    }
    await continueAutoQueue(nextIndex);
  };

  const resume = async (project: ProjectSummary, expectedRunId?: string) => {
    if (!claimRunGate()) return;
    // A deliberate continuation is one operation, never a queue trigger.
    setAutoQueueEnabled(false);
    queueIssueDraftIdRef.current = null;
    setQueueInputNotice(null);
    // Bind the displayed execution before any asynchronous refresh. Only old
    // catalog-only projects without a task record use the project-ID adapter.
    const boundRunId = expectedRunId ?? Object.values(sharedTasks)
      .find(task => task.project_id === project.id && !task.project_deleted)?.run_id ?? undefined;
    selectProject(project);
    setProjectActionError(null);
    setActiveProjectId(project.id);
    clearCancellationFeedback();
    runElapsedOffset.current = project.durationMs ?? 0;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(runElapsedOffset.current);
    try {
      store.setEstimate(await estimateProjectRuntime(project.id));
    } catch {
      store.setEstimate(null);
    }
    setFailureDialog(null);
    store.beginRun();
    let completedProject: ProjectSummary | null = null;
    try {
      const result = boundRunId
        ? await resumePipeline(project.id, boundRunId)
        : await resumePipeline(project.id);
      setLiveElapsedMs((current) => Math.max(current, result.durationMs));
      store.setResult(result);
      store.setPhase("completed");
      completedProject = resumedProjectSummary(result, project);
      selectProject(completedProject);
      await refreshCompletedProject(completedProject);
      await refreshTasks();
    } catch (error) {
      if (isTaskAdmissionConflict(error)) {
        setProjectActionError({ projectId: project.id, message: messageOf(error), occurredAt: Date.now() });
        await refreshAfterTaskConflict();
        return;
      }
      const backendElapsed = useAppStore.getState().latestEvent?.elapsedMs ?? 0;
      setLiveElapsedMs(runElapsedOffset.current + backendElapsed);
      const message = messageOf(error);
      setProjectActionError({ projectId: project.id, message, occurredAt: Date.now(), errorContext: pipelineCommandError(error) ?? undefined });
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled) {
        setFailureDialog(inferFailureDialog(error, project.id));
      }
    } finally {
      if (!completedProject) {
        try {
          const overview = await refreshProjects();
          const refreshed = overview.projects.find((item) => item.id === project.id);
          if (refreshed) selectProject(refreshed);
        } catch { /* the project remains on disk */ }
      }
      releaseRunGate();
      setActiveProjectId(null);
    }
  };

  const removeProject = async (project: ProjectSummary) => {
    if (deletingProjectId) return;
    setProjectActionError(null);
    setDeletingProjectId(project.id);
    try {
      await reconcileRuntimeState();
      if (await confirmAndDeleteProject(project)) {
        await refreshProjects();
        await refreshTasks();
        if ((selectedTask.kind === "project" && selectedTask.id === project.id) || (selectedTask.kind === "task" && sharedTasks[selectedTask.id]?.project_id === project.id)) {
          const fallback = drafts.find((draft) => !draft.running) ?? drafts[0];
          if (fallback) selectDraft(fallback.id);
        }
      }
    } catch (error) {
      setProjectActionError({ projectId: project.id, message: messageOf(error), occurredAt: Date.now() });
    } finally {
      setDeletingProjectId((current) => current === project.id ? null : current);
    }
  };

  const showProject = async (project: ProjectSummary) => {
    if (revealingProjectId) return;
    setProjectActionError(null);
    setRevealingProjectId(project.id);
    try {
      await withTimeout(revealProject(project), NATIVE_ACTION_TIMEOUT_MS, t("error.timeout"));
    } catch (error) {
      setProjectActionError({ projectId: project.id, message: t("error.openFolder", { detail: messageOf(error) }), occurredAt: Date.now() });
    } finally {
      setRevealingProjectId((current) => current === project.id ? null : current);
    }
  };

  const releasePreviewSession = useCallback((projectId: string) => {
    if (releasedPreviewProjects.current.has(projectId)) return Promise.resolve();
    const pending = previewReleasePromises.current.get(projectId);
    if (pending) return pending;
    const release = releaseGaussianPreview(projectId)
      .then(() => { releasedPreviewProjects.current.add(projectId); })
      .finally(() => { previewReleasePromises.current.delete(projectId); });
    previewReleasePromises.current.set(projectId, release);
    return release;
  }, []);

  const clearPreviewSession = useCallback((projectId: string, sessionId: number) => {
    const active = activePreviewSession.current;
    if (!active || active.projectId !== projectId || active.sessionId !== sessionId) return;
    if (previewCloseWatchdog.current != null) {
      window.clearTimeout(previewCloseWatchdog.current);
      previewCloseWatchdog.current = null;
    }
    activePreviewSession.current = null;
    if (useGaussianTransformStore.getState().descriptor?.projectId === projectId) closeGaussian();
    setClosingPreviewProjectId((current) => current === projectId ? null : current);
  }, [closeGaussian]);

  const finishPreviewClose = useCallback(async (projectId: string, sessionId: number, forced = false) => {
    const active = activePreviewSession.current;
    if (!active || active.projectId !== projectId || active.sessionId !== sessionId) return;
    if (forced) {
      clearPreviewSession(projectId, sessionId);
      setProjectActionError({ projectId, message: t("error.previewCleanupTimeout"), occurredAt: Date.now() });
      void releasePreviewSession(projectId).catch(() => undefined);
      return;
    }
    try {
      await withTimeout(releasePreviewSession(projectId), PREVIEW_CLOSE_TIMEOUT_MS, t("error.previewCleanupTimeout"));
    } catch (error) {
      setProjectActionError({ projectId, message: messageOf(error), occurredAt: Date.now() });
    } finally {
      clearPreviewSession(projectId, sessionId);
    }
  }, [clearPreviewSession, messageOf, releasePreviewSession, t]);

  const previewProject = async (project: ProjectSummary) => {
    if (project.status !== "completed" || openingPreviewProjectId || closingPreviewProjectId === project.id) return;
    const previous = useGaussianTransformStore.getState().descriptor?.projectId;
    setOpeningPreviewProjectId(project.id);
    setProjectActionError(null);
    try {
      closeGaussian();
      if (previous) await withTimeout(releasePreviewSession(previous), PREVIEW_CLOSE_TIMEOUT_MS, t("error.previewCleanupTimeout"));
      const descriptor = await prepareGaussianPreview(project.id);
      releasedPreviewProjects.current.delete(project.id);
      loadGaussian(descriptor);
      const sessionId = ++previewSessionSequence.current;
      activePreviewSession.current = { projectId: project.id, sessionId };
      setPreviewSessionId(sessionId);
      taskScrollPositions.current = {
        control: controlPaneRef.current?.scrollTop ?? 0,
        projects: projectsPaneRef.current?.scrollTop ?? 0,
      };
      setViewMode("preview");
    } catch (error) {
      setProjectActionError({ projectId: project.id, message: messageOf(error), occurredAt: Date.now() });
    } finally {
      setOpeningPreviewProjectId(null);
    }
  };

  const openReshoot = (project: ProjectSummary) => {
    if (reshootBusy) return;
    queueRevisionRef.current += 1;
    const draft = createReshootDraft(taskWorkspace.nextOrdinal, project.id, project.name, project.quality ?? "balanced", useAppStore.getState().plannerEnabled);
    setExpandedGroups((groups) => ({ ...groups, new: true }));
    setProjectActionError(null);
    setTaskWorkspace((workspace) => ({ drafts: [...workspace.drafts, draft], selected: { kind: "draft", id: draft.id }, nextOrdinal: workspace.nextOrdinal + 1 }));
  };

  const chooseReshootInput = async (inputType: InputType) => {
    const draft = selectedDraft;
    if (!draft || draft.kind !== "reshoot" || !draft.sourceProjectId || reshootBusy) return;
    const selected = inputType === "images" ? await selectImageSequence() : await selectVideo();
    if (!selected) return;
    await applyInputToDraft(draft.id, selected, inputType);
  };

  const runReshoot = async (draftId = selectedDraft?.id) => {
    const draft = taskWorkspaceRef.current.drafts.find((item) => item.id === draftId);
    if(draft?.inputs){await generate(draftId);return;}
    if (!draft || draft.kind !== "reshoot" || !draft.sourceProjectId || !draft.inputPath || !draft.reshootPlan?.compatible || !store.projectsRoot || !draftProjectionReady(draft)) return;
    if (!claimRunGate()) return;
    let preflightPassed = false;
    let reshootAnswers: PreflightAnswers | null = null;
    let confirmedDraft = draft;
    try {
      const source = await inspectReshootSource(draft.sourceProjectId);
      if (!source.eligible) {
        updateDraft(draft.id, { reshootSource: source, error: source.reason ?? t("reshoot.ineligible") });
        if (autoRunNextRef.current) setAutoQueueEnabled(false);
        return;
      }
      const finalPlan = await probeReshootInput(draft.sourceProjectId, draft.inputPath, draft.inputType, draft.quality, true, draftGenerationPolicy(draft), draft.video && draft.projectionDetection ? {video:draft.video,projectionDetection:draft.projectionDetection} : undefined);
      if (!finalPlan.compatible) {
        updateDraft(draft.id, { reshootPlan: finalPlan, error: finalPlan.incompatibilityReason ?? t("reshoot.ineligible") });
        if (autoRunNextRef.current) setAutoQueueEnabled(false);
        return;
      }
      updateDraft(draft.id, { reshootSource: source, reshootPlan: finalPlan, projectionDetection: finalPlan.projectionDetection ?? null });
      confirmedDraft = taskWorkspaceRef.current.drafts.find(item => item.id === draft.id) ?? draft;
      if (!draftProjectionReady(confirmedDraft)) return;
      reshootAnswers = await confirmPreflight(collectPreflightQuestions([confirmedDraft]), t('generate.start'));
      if (!reshootAnswers) { if (autoRunNextRef.current) setAutoQueueEnabled(false); return; }
      const latestSource = await inspectReshootSource(draft.sourceProjectId);
      const latestPlan = await probeReshootInput(draft.sourceProjectId, draft.inputPath, draft.inputType, confirmedDraft.quality, true, draftGenerationPolicy(confirmedDraft), confirmedDraft.video && confirmedDraft.projectionDetection ? {video:confirmedDraft.video,projectionDetection:confirmedDraft.projectionDetection} : undefined);
      if (latestSource.sourceIdentity !== source.sourceIdentity || latestPlan.sourceIdentity !== finalPlan.sourceIdentity) throw new Error(t('preflight.changed'));
      const current = taskWorkspaceRef.current.drafts.find(item => item.id === draft.id);
      if (!current || current.inputChecking || preflightSnapshot(current) !== preflightSnapshot(confirmedDraft) || useAppStore.getState().projectsRoot !== store.projectsRoot) throw new Error(t('preflight.changed'));
      preflightPassed = true;
    } catch (error) {
      updateDraft(draft.id, { error: messageOf(error) });
      if (autoRunNextRef.current) setAutoQueueEnabled(false);
      return;
    } finally {
      if (!preflightPassed) releaseRunGate();
    }
    clearCancellationFeedback();
    runElapsedOffset.current = 0;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(0);
    setFailureDialog(null);
    setActiveDraftId(draft.id);
    selectDraft(draft.id);
    updateDraft(draft.id, { running: true, error: null });
    store.beginRun();
    let succeeded = false;
    let promoted = false;
    let nextIndex = 0;
    try {
      const result = await startReshootPipeline({
        sourceProjectId: draft.sourceProjectId,
        reshootPath: draft.inputPath,
        inputType: draft.inputType,
        projectsRoot: store.projectsRoot,
        workspaceTaskId: draft.id,
        quality: confirmedDraft.quality,
        plannerEnabled: true,
        generationPolicy: draftGenerationPolicy(confirmedDraft),
        reshootOptions: {
          condition: reshootAnswers![`${draft.id}:reshootCondition`] as import('../types/pipeline').ReshootCondition,
          cameraRelation: confirmedDraft.reshootSource!.projection === 'equirectangular' ? undefined : reshootAnswers![`${draft.id}:reshootCamera`] as import('../types/pipeline').ReshootCameraRelation,
          allowFallback: confirmedDraft.autoRetryEnabled,
          sourceIdentity: confirmedDraft.reshootSource!.sourceIdentity!,
          inputIdentity: confirmedDraft.reshootPlan!.sourceIdentity!,
          projection: confirmedDraft.reshootSource!.projection ?? 'perspective',
          imageMatchingStrategy: imageMatchingStrategy(confirmedDraft,reshootAnswers!),
        },
      });
      store.setResult(result);
      store.setPhase("completed");
      succeeded = true;
      const promotion = promoteCompletedDraft(draft, result);
      promoted = true;
      nextIndex = promotion.nextIndex;
      await refreshCompletedProject(promotion.project);
    } catch (error) {
      if (isTaskAdmissionConflict(error)) {
        store.setError(messageOf(error));
        await refreshAfterTaskConflict();
        return;
      }
      const admission = inputAdmissionReason(error);
      if (admission) {
        rejectDraftInput(draft.id, admission);
        store.setPhase('idle');
        return;
      }
      const message = messageOf(error);
      updateDraft(draft.id, { error: message, errorContext: pipelineCommandError(error) ?? undefined });
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled) {
        setFailureDialog(inferFailureDialog(error));
      }
    } finally {
      if (!succeeded) {
        try {
          const overview = await refreshProjects();
          const project = overview.projects.find((item) => item.workspaceTaskId === draft.id);
          nextIndex = Math.max(0, taskWorkspaceRef.current.drafts.findIndex((item) => item.id === draft.id));
          if (project) {
            promoted = true;
            mutateTaskWorkspace((workspace) => ({ ...workspace, drafts: workspace.drafts.filter((item) => item.id !== draft.id), selected: { kind: "project", id: project.id } }));
          }
          else updateDraft(draft.id, { running: false });
        } catch { updateDraft(draft.id, { running: false }); }
      }
      setActiveDraftId(null);
      releaseRunGate();
    }
    if (!succeeded || !promoted) {
      if (autoRunNextRef.current) setAutoQueueEnabled(false);
      return;
    }
    await continueAutoQueue(nextIndex);
  };

  async function continueAutoQueue(nextIndex: number): Promise<void> {
    if (queueCompletionRef.current) await queueCompletionRef.current;
    while (autoRunNextRef.current) {
      if (queueCompletionRef.current) { await queueCompletionRef.current; if (!autoRunNextRef.current) return; }
      await checkQueueInputs(false);
      if (!autoRunNextRef.current) return;
      const revision = queueRevisionRef.current;
      let candidate = taskWorkspaceRef.current.drafts[nextIndex];
      if (!candidate || !candidate.inputPath || !store.projectsRoot) {
        setAutoQueueEnabled(false);
        return;
      }
      selectDraft(candidate.id);
      const pending = inputAnalysisPromises.current.get(candidate.id);
      if (pending) await pending;
      else if (candidate.needsValidation) {
        if (candidate.kind === "generation") await analyze(candidate.id, candidate.inputPath, candidate.quality, candidate.plannerEnabled);
        else if (candidate.sourceProjectId) await analyzeReshoot(candidate.id, candidate.sourceProjectId, candidate.inputPath, candidate.inputType);
      }
      if (!autoRunNextRef.current) return;
      if (revision !== queueRevisionRef.current) continue;
      candidate = taskWorkspaceRef.current.drafts[nextIndex];
      const enginesReady = useAppStore.getState().engines.every(engineReady);
      if (!candidate || !draftIsRunnable(candidate) || !enginesReady || !useAppStore.getState().projectsRoot) {
        setAutoQueueEnabled(false);
        return;
      }
      if (candidate.kind === "generation") await generate(candidate.id, "queue");
      else await runReshoot(candidate.id);
      return;
    }
  }

  useEffect(() => {
    const draft = selectedDraft;
    if (!draft?.needsValidation || draft.running) return;
    if (draft.kind === "generation") {
      if (draft.inputPath) void analyze(draft.id, draft.inputPath, draft.quality, draft.plannerEnabled);
      return;
    }
    if (!draft.sourceProjectId) return;
    if (draft.inputPath) void analyzeReshoot(draft.id, draft.sourceProjectId, draft.inputPath, draft.inputType);
    else {
      const revision = (inputAnalysisRevisions.current.get(draft.id) ?? 0) + 1;
      inputAnalysisRevisions.current.set(draft.id, revision);
      updateDraft(draft.id, { inputChecking: true, needsValidation: false });
      void inspectReshootSource(draft.sourceProjectId)
        .then((source) => {
          if (inputAnalysisRevisions.current.get(draft.id) !== revision) return;
          const current = taskWorkspaceRef.current.drafts.find(item => item.id === draft.id);
          updateDraft(draft.id, { reshootSource: source, ...(current?.plannerDefaultPending && !source.imported ? { plannerEnabled: source.plannerEnabled, plannerDefaultPending: false } : {}), error: source.eligible ? null : source.reason ?? t("reshoot.ineligible"), inputChecking: false });
        })
        .catch((error) => { if (inputAnalysisRevisions.current.get(draft.id) === revision) updateDraft(draft.id, { error: messageOf(error), inputChecking: false }); });
    }
  // Validation is intentionally lazy and keyed only by selection.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTask.kind, selectedTask.id]);

  const closeFailureDialog = useCallback(() => {
    if (failureDialogAction === null) setFailureDialog(null);
  }, [failureDialogAction]);

  const retryFailedProject = async () => {
    const failure = failureDialog;
    if (!failure || failureDialogAction) return;
    // Capture the failed run shown by this dialog, not a later task snapshot.
    if (failure.taskId && failure.runId) {
      if (!claimRunGate()) return;
      setFailureDialogAction('retry');
      setAutoQueueEnabled(false);
      queueIssueDraftIdRef.current = null;
      setQueueInputNotice(null);
      setFailureDialog(null);
      mutateTaskWorkspace(workspace => ({ ...workspace, selected: { kind: 'task', id: failure.taskId! } }));
      clearCancellationFeedback();
      store.beginRun();
      try {
        const result = await continueTaskPipeline(failure.taskId, failure.runId);
        store.setResult(result);
        store.setPhase('completed');
        await Promise.allSettled([refreshProjects(), refreshTasks()]);
      } catch (error) {
        if (isTaskAdmissionConflict(error)) {
          // The server rejected this old request; do not fail the newer run.
          await refreshAfterTaskConflict();
        } else {
          store.setPhase(pipelineWasCancelled(error) ? 'cancelled' : 'failed');
          setFailureDialog(inferFailureDialog(error, failure.projectId ?? undefined));
        }
        store.setError(messageOf(error));
      } finally {
        releaseRunGate();
        setFailureDialogAction(null);
      }
      return;
    }
    const projectId = failure.projectId;
    if (!projectId || failureDialogAction) return;
    setFailureDialogAction("retry");
    try {
      if (Object.values(sharedTasks).some(task => task.project_id === projectId && !task.project_deleted)) {
        throw new Error(t('preflight.changed'));
      }
      const overview = await refreshProjects();
      const project = overview.projects.find((item) => item.id === projectId);
      if (!project) throw new Error(t("failure.logsUnavailable"));
      setFailureDialog(null);
      await resume(project);
    } catch (error) {
      store.setError(messageOf(error));
    } finally {
      setFailureDialogAction(null);
    }
  };

  const openFailureLogs = async () => {
    const projectId = failureDialog?.projectId;
    if (!projectId || failureDialogAction) return;
    setFailureDialogAction("logs");
    try {
      await withTimeout(revealProjectLogs(projectId), NATIVE_ACTION_TIMEOUT_MS, t("error.timeout"));
    } catch (error) {
      setProjectActionError({ projectId, message: t("error.openFolder", { detail: messageOf(error) }), occurredAt: Date.now() });
    } finally {
      setFailureDialogAction(null);
    }
  };

  const exitPreview = async () => {
    const active = activePreviewSession.current;
    if (!active || closingPreviewProjectId) return;
    setClosingPreviewProjectId(active.projectId);
    setViewMode("tasks");
    if (previewCloseWatchdog.current != null) window.clearTimeout(previewCloseWatchdog.current);
    previewCloseWatchdog.current = window.setTimeout(() => {
      previewCloseWatchdog.current = null;
      void finishPreviewClose(active.projectId, active.sessionId, true);
    }, PREVIEW_CLOSE_TIMEOUT_MS);
  };

  const previewRendererDisposed = useCallback((projectId: string, disposedSessionId: number) => {
    void finishPreviewClose(projectId, disposedSessionId);
  }, [finishPreviewClose]);

  useEffect(() => () => {
    const active = activePreviewSession.current;
    if (active) {
      void releasePreviewSession(active.projectId).catch(() => undefined);
    }
  }, [releasePreviewSession]);

  const selectedShowsLiveRun = (isRunning || store.events.length > 0) && (
    (selectedTask.kind === "draft" && selectedTask.id === activeDraftId)
    || (selectedTask.kind === "project" && selectedTask.id === activeProjectId)
  );
  const trainingRemainingSeconds = selectedShowsLiveRun
    ? liveTrainingRemainingSeconds(store.latestRuntime, isRunning, Date.now())
    : null;
  const activeStageProgress = store.latestEvent == null
    ? null
    : stagePercent(store.latestEvent);
  const detailErrorDismissible = !selectedProject || selectedProject.status === "completed";
  const detailErrorBanner = visibleDetailError
    ? <div className="inline-error detail-error" role="alert"><CircleAlert size={16} /><CompactError message={visibleDetailError.message} context={visibleDetailError.errorContext} />{detailErrorDismissible && <button type="button" onClick={closeVisibleDetailError}>{t("common.close")}</button>}</div>
    : null;
  const projectConfigurationDetails = selectedProject && projectDetail
    ? projectDetail.imported
      ? <ImportedProjectDetails detail={projectDetail} onRevealSource={() => void revealImportedSource(selectedProject.id).catch(error => setProjectActionError({ projectId: selectedProject.id, message: messageOf(error), occurredAt: Date.now() }))} onRelink={() => void relinkImportedSource(selectedProject.id)} />
      : <TaskConfiguration inputs={projectDetail.inputs} generationPolicy={projectDetail.generationPolicy} automaticallyRetried={projectDetail.automaticallyRetried} active={selectedProject.status === "running"} reshootRecipe={projectDetail.reshootRecipe} sourcePath={projectDetail.sourcePath ?? selectedProject.sourceName} projectsRoot={projectDetail.projectsRoot ?? parentPath(selectedProject.projectPath)} inputType={projectDetail.inputType!} quality={selectedProject.quality!} plannerEnabled={projectDetail.plannerEnabled!} completed={selectedProject.status === "completed"} inheritedFrom={selectedProject.status === "completed" ? null : projectDetail.sourceProjectId ? "reshoot" : "resume"} acceleration={systemAcceleration} videoProjection={projectDetail.videoProjection} inputProjection={projectDetail.inputProjection} video={projectDetail.video} imageSequence={projectDetail.imageSequence} estimatedFrames={projectDetail.estimatedFrames} estimate={projectEstimate?.projectId === selectedProject.id ? projectEstimate.value : null} />
    : null;
  const historicalTaskProgress = selectedProject && projectDetail
    ? projectDetail.imported || projectDetail.stage == null || projectDetail.progress == null
      ? null
      : <HistoricalTaskProgress project={selectedProject} detail={projectDetail} />
    : null;

  const renderSharedNewRows = () => submittedNew.map(task => {
    const title = task.task_kind === "reshoot"
      ? `${store.projects.find(project => project.id === task.source_project_id)?.name ?? basename(task.input_path)} · ${t("project.reshoot")}`
      : basename(task.input_path);
    return <NewTaskRow key={task.task_id} taskId={task.task_id} title={title} path={task.input_path}
      status={taskIsActive(task) ? "running" : task.status} statusLabel={taskStatusLabel(locale, task.status)}
      selected={(selectedTask.kind === "task" && selectedTask.id === task.task_id) || (selectedTask.kind === "project" && selectedTask.id === task.project_id)}
      canReorder={false} agentCreated={task.source === "mcp"} error={task.error?.message} errorContext={taskFailureContext(task.error)}
      onSelect={() => mutateTaskWorkspace(workspace => ({ ...workspace, selected: { kind: "task", id: task.task_id } }))} />;
  });

  const renderSharedRow = (task: import("../types/tasks").SharedTask) => {
    const project = taskProjectSummary(task, store.projects.find(item => item.id === task.project_id));
    return <ProjectRow key={`task:${task.task_id}`} task={task} project={project}
      selected={(selectedTask.kind === "task" && selectedTask.id === task.task_id) || (selectedTask.kind === "project" && selectedTask.id === task.project_id)}
      busy={runBusy} previewing={openingPreviewProjectId === task.project_id}
      previewDisabled={openingPreviewProjectId !== null || closingPreviewProjectId === task.project_id}
      deleting={deletingProjectId === task.project_id} revealing={revealingProjectId === task.project_id}
      onSelect={selectProject} onSelectTask={() => mutateTaskWorkspace(workspace => ({ ...workspace, selected: { kind: "task", id: task.task_id } }))}
      onPreview={item => void previewProject(item)} onReshoot={item => void openReshoot(item)}
      onResume={(item, expectedRunId) => void resume(item, expectedRunId)} onReveal={item => void showProject(item)} onDelete={item => void removeProject(item)} />;
  };
  const renderSharedRows = (items: import("../types/tasks").SharedTask[]) => items.map(renderSharedRow);
  const renderCompletedProjectRow = (project: ProjectSummary) => <ProjectRow key={`project:${project.id}`} project={project} selected={selectedTask.kind === "project" && selectedTask.id === project.id} busy={runBusy} previewing={openingPreviewProjectId === project.id} previewDisabled={openingPreviewProjectId !== null || closingPreviewProjectId === project.id} deleting={deletingProjectId === project.id} revealing={revealingProjectId === project.id} onSelect={selectProject} onPreview={(item) => void previewProject(item)} onReshoot={(item) => void openReshoot(item)} onResume={() => undefined} onReveal={(item) => void showProject(item)} onDelete={(item) => void removeProject(item)} />;

  if (viewMode === "preview") {
    return <main className="app-shell preview-mode">
      <Suspense fallback={<section className="preview-pane active preview-workspace"><div className="preview-empty"><LoaderCircle className="spin" size={24} /><strong>{t("preview.preparingModule")}</strong></div></section>}>
        <GaussianViewer previewSessionId={previewSessionId} onExit={exitPreview} onDisposed={previewRendererDisposed} pipelineRunning={isRunning} />
      </Suspense>
    </main>;
  }

  return <main className={isResizing ? "app-shell resizing" : "app-shell"}>
    <div className="interface-frame" style={{ "--ui-scale": uiScale / 100, "--ui-size": `${10000 / uiScale}%` } as CSSProperties}>
    <header className="topbar">
      <div className="brand-lockup"><span className="brand-mark"><img src={appLogo} alt="" aria-hidden="true" /></span><span className="brand-name">OOO<span>Splat</span></span><span className="version-tag">LOCAL / {packageMetadata.version}</span></div>
      <div className="topbar-actions">
        <button className="settings-action language-action" type="button" title={t("language.switchTo")} aria-label={t("language.switchTo")} onClick={toggleLocale}><Languages size={15} />{t("language.target")}</button>
        {updates.release && <button className="settings-action update-notice-action" type="button" onClick={updates.open}><BellDot size={15} />{t("update.found")}</button>}
        <button className="settings-action" type="button" onClick={() => setSettingsOpen(true)}><Settings2 size={15} />{t("top.settings")}</button>
        <EngineStatusPopover engines={store.engines} />
      </div>
    </header>

    <section className="workspace" ref={workspaceRef} style={{ "--left-pane-width": `${leftPanePercent}%` } as CSSProperties}>
      <section className="control-pane" ref={controlPaneRef} aria-label={t("task.console")}>
        <div className="pane-header"><h1>{selectedSharedTask ? basename(selectedSharedTask.input_path) : selectedProject?.name ?? (selectedDraft?.kind === "reshoot" ? draftDisplayName(selectedDraft, t("workspace.newTask"), t("project.reshoot"),t("inputs.mixed")) : t("task.create"))}</h1><span className={(selectedSharedTask ? taskIsActive(selectedSharedTask) : selectedShowsLiveRun) ? "run-state active" : "run-state"}>{selectedSharedTask ? taskStatusLabel(locale,selectedSharedTask.status) : selectedProject ? t(statusKey[selectedProject.status]) : selectedShowsLiveRun ? t("task.running") : t("task.idle")}</span></div>

        {selectedSharedTask && <SharedTaskDetail key={`${selectedSharedTask.task_id}:${selectedSharedTask.run_id}`} task={selectedSharedTask} externalError={projectActionError?.projectId === selectedSharedTask.project_id ? projectActionError.message : null} project={store.projects.find(item => item.id === selectedSharedTask.project_id)} acceleration={systemAcceleration} showRuntimePanel={uiPreferences.showRuntimePanel} canStart={!isRunning && !runGateBusy && missingEngines.length === 0} canContinue={!isRunning && !runGateBusy} onBeforeContinue={() => { setAutoQueueEnabled(false); queueIssueDraftIdRef.current = null; setQueueInputNotice(null); }} onResume={resume} />}

        {selectedDraft?.kind === "generation" && <><div className="form-section">
          <div className="field-label-row input-label-row"><label className="field-label">{t("input.label")}</label><CaptureAdviceHelp /></div>
          {materialPicker(selectedDraft)}
        </div>

        <div className="form-section">
          <label className="field-label">{t("project.root")}</label>
          <button className="path-picker compact" type="button" disabled={selectedDraft.running} onClick={() => void chooseRoot()}>
            <FolderOpen size={18} /><span><strong>{store.projectsRoot ? basename(store.projectsRoot) : t("project.readingRoot")}</strong><small>{store.projectsRoot ? displayPath(store.projectsRoot) : "Documents / SplatStudio / Projects"}</small></span><ChevronRight size={16} />
          </button>
          <p className="field-note">{t("project.rootHint")}</p>
        </div>

        <div className="form-section">
          <div className="field-label-row"><label className="field-label">{t("quality.label")}</label>{selectedHasVideoInput&&<button className="more-settings-button" type="button" aria-expanded={advancedSamplingOpen} onClick={()=>setAdvancedSamplingOpen(open=>!open)}>{t(advancedSamplingOpen?'sampling.less':'sampling.more')}</button>}</div>
          <div className="quality-settings">
            {selectedHasVideoInput&&advancedSamplingOpen&&fpsPicker(selectedDraft)}
            <div className="quality-list" role="radiogroup">
              {qualities.map((quality) => <button key={quality.value} type="button" role="radio" disabled={selectedDraft.running} aria-checked={selectedDraft.quality === quality.value} className={selectedDraft.quality === quality.value ? "quality-option selected" : "quality-option"} onClick={() => void chooseQuality(quality.value)}>
                <span className="radio-mark"><span /></span><span><strong>{t(quality.label)}</strong><small>{t(quality.description)}</small></span>
              </button>)}
            </div>
            <button className="planner-switch" type="button" role="switch" aria-checked={selectedDraft.autoRetryEnabled} disabled={selectedDraft.running} onClick={() => void changePlannerEnabled()}>
              <span><strong>{t("retry.label")}</strong><small>{t("retry.hint")}</small></span>
              <i aria-hidden="true"><span /></i>
            </button>
          </div>
        </div>

        <div className={`acceleration-status ${systemDetectionTemporary ? "warning" : systemAcceleration?.backend === "gpu" ? "gpu" : systemAccelerationWarning ? "warning" : "cpu"}`} aria-live="polite">
          <span className="acceleration-icon">{systemAcceleration == null ? <LoaderCircle className="spin" size={17} /> : systemDetectionTemporary || systemAccelerationWarning ? <CircleAlert size={17} /> : systemAcceleration.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : <Cpu size={17} />}</span>
          <span>
            <strong>{systemAcceleration == null ? t("gpu.detecting") : systemDetectionTemporary ? t("gpu.temporarilyUnavailable") : systemAcceleration.backend === "gpu" ? t("gpu.enabled") : t("gpu.cpu")}</strong>
            <small>{systemAcceleration == null ? t("gpu.reading") : systemDetectionTemporary ? `${t("gpu.temporaryHint")}${systemAcceleration.device ? ` · ${systemAcceleration.device.name}` : ""}` : systemAcceleration.backend === "gpu" && systemAcceleration.device ? `${systemAcceleration.device.name}${systemAcceleration.device.totalMemoryMb ? ` · ${t("gpu.memory", { value: (systemAcceleration.device.totalMemoryMb / 1024).toFixed(1) })}` : ""} · ${t("gpu.driver", { value: systemAcceleration.device.driverVersion })} · Compute Capability ${systemAcceleration.device.computeCapability}` : systemAcceleration.reasonCode === "macOsCpuOnly" ? localizePipelineMessage(locale, systemAcceleration.reason) : `${localizePipelineMessage(locale, systemAcceleration.reason)} · ${t("gpu.requirements", { driver: systemAcceleration.requirements.minimumDriverVersion, capability: systemAcceleration.requirements.minimumComputeCapability })}`}</small>
            {selectedShowsLiveRun && store.taskColmapAcceleration && <small>{t("gpu.currentTask")} · {store.taskColmapAcceleration.backend === "gpu" ? t("gpu.taskGpu", { device: store.taskColmapAcceleration.device?.name ?? "NVIDIA GPU" }) : t("gpu.taskCpu")}</small>}
          </span>
        </div>

        <button className="primary-action" type="button" disabled={runBusy || queueChecking || !!preflight || selectedDraft.inputChecking || selectedDraft.needsValidation || !draftProjectionReady(selectedDraft) || !selectedDraft.inputPath || !selectedDraft.plan || !store.projectsRoot || missingEngines.length > 0} onClick={() => void generate()}><Play size={16} fill="currentColor" />{t("generate.start")}</button>
        {detailErrorBanner}
        </>}

        {selectedDraft?.kind === "reshoot" && <section className="reshoot-task-page">
          <p className="reshoot-task-intro">{t(selectedDraft.inputs?'reshoot.multiCameraHint':selectedDraft.reshootSource?.projection === 'equirectangular' ? 'reshoot.panoramaHint' : 'reshoot.multiCameraHint')}</p>

          <div className="form-section">
            <div className="field-label-row input-label-row"><label className="field-label">{t("input.label")}</label><CaptureAdviceHelp /></div>
            {materialPicker(selectedDraft)}
          </div>

          <div className="form-section">
            <label className="field-label">{t("project.root")}</label>
            <button className="path-picker compact" type="button" disabled={selectedDraft.running} onClick={() => void chooseRoot()}>
              <FolderOpen size={18} /><span><strong>{store.projectsRoot ? basename(store.projectsRoot) : t("project.readingRoot")}</strong><small>{store.projectsRoot ? displayPath(store.projectsRoot) : "Documents / SplatStudio / Projects"}</small></span><ChevronRight size={16} />
            </button>
            <p className="field-note">{t("project.rootHint")}</p>
          </div>

          <div className="form-section">
            <div className="field-label-row"><label className="field-label">{t("quality.label")}</label>{selectedHasVideoInput&&<button className="more-settings-button" type="button" aria-expanded={advancedSamplingOpen} onClick={()=>setAdvancedSamplingOpen(open=>!open)}>{t(advancedSamplingOpen?'sampling.less':'sampling.more')}</button>}</div>
            <div className="quality-settings">
              {selectedHasVideoInput&&advancedSamplingOpen&&fpsPicker(selectedDraft)}
              <div className="quality-list" role="radiogroup" aria-label={t("quality.label")}>
                {qualities.map((quality) => <button key={quality.value} type="button" role="radio" disabled={selectedDraft.running} aria-checked={selectedDraft.quality === quality.value} className={selectedDraft.quality === quality.value ? "quality-option selected" : "quality-option"} onClick={() => void chooseQuality(quality.value)}>
                  <span className="radio-mark"><span /></span><span><strong>{t(quality.label)}</strong><small>{t(quality.description)}</small></span>
                </button>)}
              </div>
              <button className="planner-switch" type="button" role="switch" aria-checked={selectedDraft.autoRetryEnabled} disabled={selectedDraft.running} onClick={() => void changePlannerEnabled()}>
                <span><strong>{t("retry.label")}</strong><small>{t("retry.hint")}</small></span>
                <i aria-hidden="true"><span /></i>
              </button>
            </div>
          </div>

          <div className={`acceleration-status ${systemDetectionTemporary ? "warning" : systemAcceleration?.backend === "gpu" ? "gpu" : systemAccelerationWarning ? "warning" : "cpu"}`} aria-live="polite">
            <span className="acceleration-icon">{systemAcceleration == null ? <LoaderCircle className="spin" size={17} /> : systemDetectionTemporary || systemAccelerationWarning ? <CircleAlert size={17} /> : systemAcceleration.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : <Cpu size={17} />}</span>
            <span>
              <strong>{systemAcceleration == null ? t("gpu.detecting") : systemDetectionTemporary ? t("gpu.temporarilyUnavailable") : systemAcceleration.backend === "gpu" ? t("gpu.enabled") : t("gpu.cpu")}</strong>
              <small>{systemAcceleration == null ? t("gpu.reading") : systemDetectionTemporary ? `${t("gpu.temporaryHint")}${systemAcceleration.device ? ` · ${systemAcceleration.device.name}` : ""}` : systemAcceleration.backend === "gpu" && systemAcceleration.device ? `${systemAcceleration.device.name}${systemAcceleration.device.totalMemoryMb ? ` · ${t("gpu.memory", { value: (systemAcceleration.device.totalMemoryMb / 1024).toFixed(1) })}` : ""} · ${t("gpu.driver", { value: systemAcceleration.device.driverVersion })} · Compute Capability ${systemAcceleration.device.computeCapability}` : systemAcceleration.reasonCode === "macOsCpuOnly" ? localizePipelineMessage(locale, systemAcceleration.reason) : `${localizePipelineMessage(locale, systemAcceleration.reason)} · ${t("gpu.requirements", { driver: systemAcceleration.requirements.minimumDriverVersion, capability: systemAcceleration.requirements.minimumComputeCapability })}`}</small>
            </span>
          </div>

          {!selectedDraft.inputs && selectedDraft.reshootPlan && !selectedDraft.reshootPlan.compatible && <div className="reshoot-plan incompatible"><strong>{selectedDraft.inputPath ? basename(selectedDraft.inputPath) : ""}</strong><span>{selectedDraft.reshootPlan.preparedWidth} × {selectedDraft.reshootPlan.preparedHeight}</span><span>{selectedDraft.reshootPlan.incompatibilityReason}</span></div>}
          <button className="primary-action" type="button" disabled={runBusy || reshootBusy || !!preflight || selectedDraft.inputChecking || !draftProjectionReady(selectedDraft) || !selectedDraft.reshootPlan?.compatible || !store.projectsRoot || missingEngines.length > 0} onClick={() => void runReshoot()}><Play size={16} fill="currentColor" />{t("generate.start")}</button>
          {detailErrorBanner}
        </section>}

        {selectedProject && !selectedSharedTask && <section className="project-detail-page">
          {projectDetailLoading && !projectDetail ? <div className="task-detail-loading"><LoaderCircle className="spin" size={20} />{t("workspace.loadingTask")}</div> : selectedProject.status === "completed" ? <>
            <ProjectResultStats project={selectedProject} detail={projectDetail} />
            {projectConfigurationDetails}
            <InputAdoptionWarning inputs={projectDetail?.inputs} />
            {selectedProject.taskKind !== "import" && selectedProject.registeredRatio != null && selectedProject.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t("result.lowRegistration", { value: (selectedProject.registeredRatio * 100).toFixed(1) })}</p>}
            {detailErrorBanner}
            {historicalTaskProgress}
          </> : projectDetail ? <div className="unfinished-task-detail">
            {projectConfigurationDetails}
            <InputAdoptionWarning inputs={projectDetail?.inputs} />
            {detailErrorBanner}
            <button className="primary-action" type="button" disabled={runBusy || selectedProject.status === "running"} onClick={() => void resume(selectedProject)}><Play size={16} fill="currentColor" />{t("project.continue")}</button>
            {historicalTaskProgress}
          </div> : detailErrorBanner}
        </section>}

        {!selectedSharedTask && selectedShowsLiveRun && <TaskProgress importedReconstruction={store.liveTask?.imported_reconstruction||selectedDraft?.inputs?.mode==='colmap'||projectDetail?.inputs?.importedReconstruction} progressKey={store.liveTask ? `${store.liveTask.task_id}:${store.liveTask.run_id ?? 'pending'}` : progressEvent?.runId ? `${progressEvent.taskId}:${progressEvent.runId}` : `local:${runStartedAt.current}`} operationAttempt={progressEvent?.operation?.attempt} substage={progressEvent?.substage} progressUnit={progressEvent?.unit} status={store.phase} stage={progressEvent?.stage ?? null} progress={store.progress} stageProgress={activeStageProgress} message={currentMessage} elapsedMs={liveElapsedMs} remainingSeconds={trainingRemainingSeconds} runtime={store.latestRuntime} showRuntimePanel={uiPreferences.showRuntimePanel} logs={liveActivity.logs} rawLogs={liveActivity.rawLogs} logsUnavailable={liveActivity.unavailable} logsTruncated={liveActivity.journalPersisted && liveActivity.truncated} cancelling={isCancellationRequested} onCancel={() => void requestCancellation()} logRef={liveLogRef} onLogScroll={updateLiveLogFollow} />}


      </section>

      <div
        className="pane-resizer"
        role="separator"
        tabIndex={0}
        aria-label={t("layout.resize")}
        aria-orientation="vertical"
        aria-valuemin={32}
        aria-valuemax={68}
        aria-valuenow={Math.round(leftPanePercent)}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.currentTarget.setPointerCapture(event.pointerId);
          setIsResizing(true);
        }}
        onPointerMove={resizePanes}
        onPointerUp={stopResizing}
        onPointerCancel={stopResizing}
        onDoubleClick={() => setLeftPanePercent(44)}
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
            event.preventDefault();
            setLeftPanePercent((current) => Math.min(68, Math.max(32, current + (event.key === "ArrowLeft" ? -2 : 2))));
          }
          if (event.key === "Home") setLeftPanePercent(44);
        }}
      ><span /></div>

      <section className="projects-pane" ref={projectsPaneRef} aria-label={t("history.aria")}>
        <div className="pane-header"><h2>{t("history.title")}</h2><button className="refresh-action" type="button" onClick={() => { void refreshTasks(); void refreshProjects().catch(error => store.setError(messageOf(error))); }}><RotateCcw size={14} />{t("history.refresh")}</button></div>
        <div className="archive-summary task-summary"><span><b>{visibleDrafts.length + submittedNew.length}</b><small>{t("workspace.newTasks")}</small></span><span><b>{completed.length + submittedCompleted.length}</b><small>{t("history.completed")}</small></span><span><b>{unfinished.length + submittedUnfinished.length}</b><small>{t("history.unfinished")}</small></span></div>

        <div className="project-group new-task-group">
          <div className="group-heading">
            <button className="group-toggle" type="button" aria-expanded={expandedGroups.new} aria-controls="new-task-group-content" aria-label={t(expandedGroups.new ? "workspace.collapseGroup" : "workspace.expandGroup", { group: t("workspace.newTasks") })} onClick={() => toggleTaskGroup("new")}><ChevronDown size={14} aria-hidden="true" /><span>{t("workspace.newTasks")}</span></button>
            <button className="group-add-action" type="button" onClick={addGenerationDraft}><Plus size={13} />{t("workspace.addTask")}</button>
            <button className="queue-toggle" type="button" role="switch" aria-checked={autoRunNext} disabled={queueChecking || !!preflight} onClick={() => autoRunNext ? setAutoQueueEnabled(false) : void checkQueueInputs(true)}><i aria-hidden="true"><span /></i><span>{t("workspace.autoRunNext")}</span></button>
            <small className="group-count">{t("history.projects", { count: visibleDrafts.length + submittedNew.length })}</small>
          </div>
          {queueInputNotice && <p className="queue-input-notice" role="status">{queueInputNotice}</p>}
          {taskSyncError && <p role="status">{taskSyncError}</p>}
          <div id="new-task-group-content" className="project-group-content" hidden={!expandedGroups.new}>{renderSharedNewRows()}{visibleDrafts.map((draft, index) => <DraftRow key={draft.id} draft={draft} status={displayStatusForDraft(draft, queuedDrafts)} insertion={draftDrag?.insertionIndex === drafts.findIndex(item => item.id === draft.id) ? "before" : draftDrag?.insertionIndex === drafts.findIndex(item => item.id === draft.id) + 1 && index === visibleDrafts.length - 1 ? "after" : null} dragging={draftDrag?.id === draft.id} selected={selectedTask.kind === "draft" && selectedTask.id === draft.id} onSelect={() => {
            if (suppressDraftSelectionRef.current !== draft.id) selectDraft(draft.id);
          }} onDelete={() => removeDraft(draft.id)} onPointerDown={(event) => beginDraftDrag(event, draft.id)} onPointerMove={updateDraftDrag} onPointerEnd={endDraftDrag} onKeyboardMove={(offset) => moveDraftWithKeyboard(draft.id, offset)} />)}</div>
          <span className="visually-hidden" aria-live="polite">{reorderAnnouncement}</span>
        </div>
        <div className="project-group completed-task-group">
          <div className="group-heading"><button className="group-toggle" type="button" aria-expanded={expandedGroups.completed} aria-controls="completed-task-group-content" aria-label={t(expandedGroups.completed ? "workspace.collapseGroup" : "workspace.expandGroup", { group: t("history.completed") })} onClick={() => toggleTaskGroup("completed")}><ChevronDown size={14} aria-hidden="true" /><span>{t("history.completed")}</span></button><div className="completed-import-actions"><button className="group-add-action" type="button" disabled={runBusy} aria-haspopup="menu" aria-expanded={!runBusy && importMenuOpen} onClick={() => setImportMenuOpen(open => !open)}><Plus size={13} />{t("import.add")}</button>{!runBusy && importMenuOpen && <div className="completed-import-menu" role="menu"><button type="button" role="menuitem" onClick={() => void chooseGaussianImport("file")}><Upload size={14} />{t("import.file")}</button><button type="button" role="menuitem" onClick={() => void chooseGaussianImport("folder")}><FolderOpen size={14} />{t("import.folder")}</button></div>}</div><small className="group-count">{t("history.projects", { count: completed.length + submittedCompleted.length })}</small></div>
          <div id="completed-task-group-content" className="project-group-content" hidden={!expandedGroups.completed}>{completedRows.map(item => item.kind === "task" ? renderSharedRow(item.task) : renderCompletedProjectRow(item.project))}</div>
        </div>
        <div className="project-group unfinished">
          <div className="group-heading"><button className="group-toggle" type="button" aria-expanded={expandedGroups.unfinished} aria-controls="unfinished-task-group-content" aria-label={t(expandedGroups.unfinished ? "workspace.collapseGroup" : "workspace.expandGroup", { group: t("history.unfinished") })} onClick={() => toggleTaskGroup("unfinished")}><ChevronDown size={14} aria-hidden="true" /><span>{t("history.unfinished")}</span><small>{t("history.projects", { count: unfinished.length + submittedUnfinished.length })}</small></button></div>
          <div id="unfinished-task-group-content" className="project-group-content" hidden={!expandedGroups.unfinished}>{renderSharedRows(submittedUnfinished)}{unfinished.map((project) => <ProjectRow key={project.id} project={project} selected={selectedTask.kind === "project" && selectedTask.id === project.id} busy={runBusy} previewing={false} previewDisabled deleting={deletingProjectId === project.id} revealing={revealingProjectId === project.id} onSelect={selectProject} onPreview={() => undefined} onReshoot={() => undefined} onResume={(item) => void resume(item)} onReveal={(item) => void showProject(item)} onDelete={(item) => void removeProject(item)} />)}</div>
        </div>
      </section>
    </section>
    </div>

    <aside className={showZoomControls ? "zoom-dock open" : "zoom-dock"} aria-label={t("zoom.aria")}>
      {showZoomControls && <div className="zoom-controls">
        <button type="button" aria-label={t("zoom.out")} disabled={uiScale <= 80} onClick={() => changeScale(-10)}><Minus size={16} /></button>
        <button className="zoom-reset" type="button" title={t("zoom.resetTitle")} onClick={() => setUiScale(100)}>{t("zoom.reset")}</button>
        <button type="button" aria-label={t("zoom.in")} disabled={uiScale >= 140} onClick={() => changeScale(10)}><Plus size={16} /></button>
      </div>}
      <button className="zoom-trigger" type="button" aria-expanded={showZoomControls} onClick={() => setShowZoomControls((visible) => !visible)}>{uiScale}%</button>
    </aside>
    {showCancellationOverlay && isRunning && <div className="cancellation-backdrop" role="dialog" aria-modal="true" aria-labelledby="cancellation-title" aria-describedby="cancellation-description">
      <div className="cancellation-status" aria-live="assertive" aria-busy="true">
        <span className="cancellation-spinner" aria-hidden="true"><LoaderCircle className="spin" size={26} /></span>
        <div>
          <strong id="cancellation-title">{t("cancel.title")}</strong>
          <p id="cancellation-description">{t("cancel.description")}</p>
        </div>
      </div>
    </div>}
    {failureDialog && <FailureGuidanceDialog failure={failureDialog} action={failureDialogAction} canContinue={!runBusy} onClose={closeFailureDialog} onRetry={() => void retryFailedProject()} onOpenLogs={() => void openFailureLogs()} />}
    {telemetryPreferences && !telemetryPreferences.consentDecided && <TelemetryPreferences mode="consent" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} />}
    {settingsOpen && <SettingsDialog preferences={telemetryPreferences} telemetryBusy={telemetryBusy} showRuntimePanel={uiPreferences.showRuntimePanel} onTelemetryChange={(enabled) => void changeTelemetryConsent(enabled)} onRuntimePanelChange={changeRuntimePanelPreference} onClose={() => setSettingsOpen(false)} updatePanel={<UpdateSettingsRow result={updates.result} busy={updates.busy} failed={updates.failed} cooldownSeconds={updates.cooldownSeconds} onCheck={() => void checkUpdatesManually()} />} />}
    {unsupportedInput && !settingsOpen && !failureDialog && !showCancellationOverlay && !gaussianImportSelectionId && !(telemetryPreferences && !telemetryPreferences.consentDecided) && <UnsupportedInputDialog reason={unsupportedInput} onClose={() => setUnsupportedInput(null)} />}
    {preflight && !unsupportedInput && !settingsOpen && !failureDialog && !showCancellationOverlay && !gaussianImportSelectionId && !(telemetryPreferences && !telemetryPreferences.consentDecided) && <GenerationPreflightDialog key={preflight.questions.map(item => item.snapshot).join('|')} session={preflight} onFinish={finishPreflight} />}
    {updates.dialogRequested && updates.release && !unsupportedInput && !gaussianImportSelectionId && !preflight && !queueChecking && !settingsOpen && !failureDialog && !showCancellationOverlay && !(telemetryPreferences && !telemetryPreferences.consentDecided) && <UpdateNoticeDialog currentVersion={updates.result!.currentVersion} release={updates.release} onClose={updates.dismiss} />}
    {gaussianImportSelectionId && <GaussianImportDialog selectionId={gaussianImportSelectionId} onClose={() => { gaussianSelectionRevision.current += 1; setGaussianImportSelectionId(null); }} onImported={finishGaussianImport} onUnsupportedInput={reason => { if (gaussianImportSelectionRef.current !== gaussianImportSelectionId) return; gaussianSelectionRevision.current += 1; setGaussianImportSelectionId(null); showUnsupportedInput(reason); }} />}
  </main>;
}
