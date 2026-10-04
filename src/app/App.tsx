import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  Blend, ChevronDown, ChevronRight, CircleAlert, Clapperboard, Cpu, Eye,
  Film, FolderOpen, Images, Languages, LoaderCircle, MapPin, Minus, Play, Plus, RotateCcw, Settings2,
  Send, Square, Trash2, X, Zap,
} from "lucide-react";
import appLogo from "../../assets/app-icon.svg";
import packageMetadata from "../../package.json";
import { TelemetryPreferences } from "../components/TelemetryPreferences";
import { ErrorReportDialog } from "../components/ErrorReportDialog";
import { CompactError } from "../components/CompactError";
import {
  cancelPipeline, checkColmapAcceleration, checkEngines, confirmAndDeleteProject, confirmLargeImageSequence,
  estimateProjectRuntime, getAppRuntimeStatus, getProjectOverview, getProjectTaskDetail, onPipelineEvent, probeAndPlan, revealProject, revealProjectLogs,
  selectImageSequence, selectProjectsRoot, selectVideo,
  setPlannerEnabled, setProjectsRoot, startPipeline, prepareGaussianPreview, releaseGaussianPreview,
  initializeTelemetry, inspectReshootSource, probeReshootInput, setTelemetryConsent, resumePipeline, startReshootPipeline,
} from "../lib/backend";
import { startElapsedTicker } from "../lib/elapsedTimer";
import { pipelineCommandError, pipelineErrorMessage, pipelineWasCancelled, type PipelineFailureKind } from "../lib/pipelineError";
import { localizePipelineMessage, useI18n, type TranslationKey } from "../i18n";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import type { EngineStatus, InputType, PipelineEvent, ProjectStatus, ProjectSummary, ProjectTaskDetail, Quality } from "../types/pipeline";
import type { TelemetryPreferences as TelemetryPreferencesState } from "../types/telemetry";
import { createGenerationDraft, createReshootDraft, draftDisplayName, loadTaskWorkspace, saveTaskWorkspace, type TaskDraft, type TaskSelection } from "./taskWorkspace";

const GaussianViewer = lazy(() => import("../components/GaussianViewer").then((module) => ({ default: module.GaussianViewer })));
const CANCELLATION_OVERLAY_DELAY_MS = 300;
const PREVIEW_CLOSE_TIMEOUT_MS = 8_000;
const NATIVE_ACTION_TIMEOUT_MS = 8_000;
const friendlyProgressKeyByStage: Record<string, TranslationKey> = {
  extractingFeatures: "progress.activeFeatures",
  matching: "progress.activeMatching",
  reconstructing: "progress.activeReconstruction",
  trainingSplats: "progress.activeTraining",
};

const countFromProgressEvent = (event: PipelineEvent, stage: string): { current: number; total: number } | null => {
  if (event.stage !== stage || event.total == null || event.total <= 0) return null;
  const directCurrent = event.current;
  const estimatedTrainingCurrent = stage === "trainingSplats" && event.stageProgress != null
    ? Math.round(event.total * event.stageProgress / 100)
    : null;
  const current = directCurrent ?? estimatedTrainingCurrent;
  if (current == null || !Number.isFinite(current)) return null;
  return { current: Math.max(0, Math.min(event.total, current)), total: event.total };
};

type FailureDialogState = {
  kind: PipelineFailureKind | "generic";
  projectId: string | null;
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

const inferFailureDialog = (error: unknown, fallbackStage?: string, fallbackProjectId?: string): FailureDialogState | null => {
  const structured = pipelineCommandError(error);
  if (structured?.code === "cancelled") return null;
  const stage = structured?.failedStage ?? fallbackStage;
  const rawMessage = structured?.message ?? pipelineErrorMessage(error) ?? "";
  let kind = structured?.failureKind;
  if (!kind && stage === "reconstructing") kind = "mapper_source";
  if (!kind && stage === "trainingSplats") {
    kind = /devicelost|device lost|device_lost|parent device is lost|vk_error_device_lost|dxgi_error_device_removed|显卡设备连接中断/i.test(rawMessage)
      ? "brush_device_lost"
      : /early eof|failed to load dataset|i\/o error|io error|no such file|access denied|permission denied/i.test(rawMessage)
        ? "brush_dataset"
        : "brush_gpu";
  }
  return { kind: kind ?? "generic", projectId: structured?.projectId ?? fallbackProjectId ?? null, rawMessage, failureId: structured?.failureId ?? null, engine: structured?.engine ?? "OOOSplat" };
};

function FailureGuidanceDialog({ failure, action, onClose, onRetry, onOpenLogs }: {
  failure: FailureDialogState;
  action: "retry" | "logs" | null;
  onClose: () => void;
  onRetry: () => void;
  onOpenLogs: () => void;
}) {
  const { locale, t } = useI18n();
  const [reportOpen, setReportOpen] = useState(false);
  const [reportSent, setReportSent] = useState(false);
  useEffect(() => { setReportOpen(false); setReportSent(false); }, [failure]);
  const mapper = failure.kind === "mapper_source" || failure.kind === "mapper_storage";
  const dataset = failure.kind === "brush_dataset";
  const deviceLost = failure.kind === "brush_device_lost";
  const generic = failure.kind === "generic";
  const title = generic ? t("failure.genericTitle") : mapper ? t("failure.mapperTitle") : dataset ? t("failure.brushDatasetTitle") : deviceLost ? t("failure.brushDeviceLostTitle") : t("failure.brushTitle");
  const description = generic ? t("failure.genericDescription") : failure.kind === "mapper_source"
    ? t("failure.mapperSource")
    : failure.kind === "mapper_storage"
      ? t("failure.mapperStorage")
      : dataset
        ? t("failure.brushDataset")
        : deviceLost
          ? t("failure.brushDeviceLost")
        : t("failure.brushGpu");
  const tips: TranslationKey[] = generic ? ["failure.genericTip1", "failure.storageTip1"] : failure.kind === "mapper_source"
    ? ["failure.mapperTip1", "failure.mapperTip2", "failure.mapperTip3"]
    : failure.kind === "mapper_storage"
      ? ["failure.storageTip1", "failure.storageTip2"]
      : dataset
        ? ["failure.datasetTip1", "failure.datasetTip2"]
        : deviceLost
          ? ["failure.deviceLostTip1", "failure.deviceLostTip2", "failure.deviceLostTip3"]
        : ["failure.brushTip1", "failure.brushTip2", "failure.brushTip3"];

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
      <strong>{t("failure.solutions")}</strong>
      <ul>{tips.map((key) => <li key={key}>{t(key)}</li>)}</ul>
      {failure.rawMessage && <details><summary>{t("failure.details")}</summary><pre>{deviceLost ? localizePipelineMessage(locale, failure.rawMessage) : failure.rawMessage}</pre></details>}
      <div className="failure-guidance-actions">
        <button type="button" className="secondary" disabled={action !== null || !failure.projectId} onClick={onOpenLogs}>{action === "logs" ? <LoaderCircle className="spin" size={14} /> : <FolderOpen size={14} />}{t("failure.openLogs")}</button>
        <button type="button" className="secondary" disabled={action !== null || !failure.failureId || reportSent} title={!failure.failureId ? t("report.notAvailable") : undefined} onClick={() => setReportOpen(true)}><Send size={14} />{t(reportSent ? "report.sentShort" : "report.open")}</button>
        <button type="button" className="primary" disabled={action !== null || !failure.projectId} onClick={onRetry}>{action === "retry" ? <LoaderCircle className="spin" size={14} /> : <RotateCcw size={14} />}{t("failure.retry")}</button>
      </div>
    </section>
  </div>;
}

const qualities: Array<{ value: Quality; label: TranslationKey; description: TranslationKey }> = [
  { value: "fast", label: "quality.fast", description: "quality.fastHint" },
  { value: "balanced", label: "quality.balanced", description: "quality.balancedHint" },
  { value: "high", label: "quality.high", description: "quality.highHint" },
];

const stages = [
  ["probingVideo", "stage.material"], ["extractingFrames", "stage.frames"],
  ["extractingFeatures", "stage.features"], ["matching", "stage.matching"],
  ["reconstructing", "stage.reconstruction"], ["trainingSplats", "stage.training"],
  ["exporting", "stage.export"],
] as const;

const rawMessageOf = pipelineErrorMessage;
const basename = (path: string) => path.split(/[\\/]/).at(-1) ?? path;
const formatBytes = (bytes: number | null, locale: string) => {
  if (bytes == null) return "—";
  const [value, unit, digits] = bytes >= 1024 ** 3
    ? [bytes / 1024 ** 3, "GB", 2] as const
    : bytes >= 1024 ** 2
      ? [bytes / 1024 ** 2, "MB", 1] as const
      : [bytes / 1024, "KB", 1] as const;
  return `${new Intl.NumberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(value)} ${unit}`;
};
const formatVideoDuration = (seconds: number) => `${Math.floor(seconds / 60)}:${Math.round(seconds % 60).toString().padStart(2, "0")}`;
const qualityKey: Record<Quality, TranslationKey> = { fast: "quality.fast", balanced: "quality.balanced", high: "quality.high" };
const statusKey: Record<ProjectStatus, TranslationKey> = { running: "status.running", completed: "status.completed", failed: "status.failed", cancelled: "status.cancelled", interrupted: "status.interrupted" };
const stagePosition = (stage?: string) => {
  if (!stage || ["created", "probingVideo", "planningFrames"].includes(stage)) return 0;
  if (stage === "validatingReconstruction") return 4;
  if (stage === "completed") return 6;
  const index = stages.findIndex(([key]) => key === stage);
  return index < 0 ? 0 : index;
};

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

function ProjectRow({ project, selected, busy, previewing, previewDisabled, deleting, revealing, onSelect, onPreview, onReshoot, onResume, onReveal, onDelete }: {
  project: ProjectSummary;
  selected: boolean;
  busy: boolean;
  previewing: boolean;
  previewDisabled: boolean;
  deleting: boolean;
  revealing: boolean;
  onSelect: (project: ProjectSummary) => void;
  onPreview: (project: ProjectSummary) => void;
  onReshoot: (project: ProjectSummary) => void;
  onResume: (project: ProjectSummary) => void;
  onReveal: (project: ProjectSummary) => void;
  onDelete: (project: ProjectSummary) => void;
}) {
  const { locale, t, formatDate, formatDuration } = useI18n();
  return <article className={selected ? "project-row selected" : "project-row"} tabIndex={0} role="button" aria-pressed={selected} onClick={() => onSelect(project)} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect(project); } }}>
    <div className="project-row-main">
      <div className="project-title-line">
        <span className={`project-status ${project.status}`} />
        <strong>{project.name}</strong>
        <span className="status-copy">{t(statusKey[project.status])}</span>
      </div>
      <p className="project-path" title={project.projectPath}>{project.projectPath}</p>
      {project.failureMessage && <p className="project-failure"><CompactError message={project.failureMessage} /></p>}
      {project.registeredRatio != null && project.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t("result.lowRegistration", { value: (project.registeredRatio * 100).toFixed(1) })}</p>}
    </div>
    <dl className="project-stats">
      <div><dt>PLY</dt><dd>{formatBytes(project.fileSize, locale)}</dd></div>
      <div><dt>{t("project.date")}</dt><dd>{formatDate(project.completedAt ?? project.createdAt)}</dd></div>
      <div><dt>{t("project.elapsed")}</dt><dd>{formatDuration(project.durationMs)}</dd></div>
      <div><dt>{t("project.quality")}</dt><dd>{t(qualityKey[project.quality])}</dd></div>
    </dl>
    <div className="project-actions">
      {project.status === "completed" && <button className="preview-link" type="button" disabled={previewDisabled} onClick={(event) => { event.stopPropagation(); onSelect(project); onPreview(project); }}>{previewing ? <LoaderCircle className="spin" size={14} /> : <Eye size={14} />}{previewing ? t("project.opening") : t("project.preview")}</button>}
      {project.status === "completed" && <button className="reshoot-link" type="button" disabled={busy || previewDisabled} onClick={(event) => { event.stopPropagation(); onSelect(project); onReshoot(project); }}><Film size={14} />{t("project.reshoot")}</button>}
      {project.status !== "completed" && project.status !== "running" && <button className="resume-link" type="button" disabled={busy} onClick={(event) => { event.stopPropagation(); onSelect(project); onResume(project); }}><Play size={14} fill="currentColor" />{t("project.resume")}</button>}
      <button type="button" disabled={revealing} onClick={(event) => { event.stopPropagation(); onSelect(project); onReveal(project); }}>{revealing ? <LoaderCircle className="spin" size={14} /> : <MapPin size={14} />}{t("project.reveal")}</button>
      <button className="danger-link" type="button" disabled={busy || deleting} onClick={(event) => { event.stopPropagation(); onDelete(project); }}>{deleting ? <LoaderCircle className="spin" size={14} /> : <Trash2 size={14} />}{t("project.delete")}</button>
    </div>
  </article>;
}

function DraftRow({ draft, selected, onSelect, onDelete }: { draft: TaskDraft; selected: boolean; onSelect: () => void; onDelete: () => void }) {
  const { t } = useI18n();
  return <article className={selected ? "project-row draft-row selected" : "project-row draft-row"} tabIndex={0} role="button" aria-pressed={selected} onClick={onSelect} onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onSelect(); } }}>
    <div className="project-title-line"><span className={draft.running ? "project-status running" : "project-status"} /><strong>{draftDisplayName(draft, t("workspace.newTask"), t("project.reshoot"))}</strong><span className="status-copy">{t(draft.running ? "task.running" : "task.idle")}</span></div>
    <p className="project-path">{draft.inputPath ?? t("workspace.awaitingInput")}</p>
    {draft.error && <p className="project-failure"><CompactError message={draft.error} /></p>}
    <div className="project-actions"><button className="danger-link" type="button" disabled={draft.running} onClick={(event) => { event.stopPropagation(); onDelete(); }}><Trash2 size={14} />{t("project.delete")}</button></div>
  </article>;
}

export function App() {
  const { locale, t, toggleLocale, formatNumber, formatDuration } = useI18n();
  const store = useAppStore();
  const loadGaussian = useGaussianTransformStore((state) => state.load);
  const closeGaussian = useGaussianTransformStore((state) => state.close);
  const isRunning = store.phase === "running";
  const systemAcceleration = store.colmapAcceleration;
  const systemDetectionTemporary = systemAcceleration?.detectionState === "temporarilyUnavailable";
  const systemAccelerationWarning = systemDetectionTemporary || Boolean(systemAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly"].includes(systemAcceleration.reasonCode) && systemAcceleration.backend !== "gpu");
  const liveLogRef = useRef<HTMLDivElement>(null);
  const followLiveLogRef = useRef(true);
  const workspaceRef = useRef<HTMLElement>(null);
  const controlPaneRef = useRef<HTMLElement>(null);
  const projectsPaneRef = useRef<HTMLElement>(null);
  const taskScrollPositions = useRef({ control: 0, projects: 0 });
  const previewReleasePromises = useRef(new Map<string, Promise<void>>());
  const releasedPreviewProjects = useRef(new Set<string>());
  const previewSessionSequence = useRef(0);
  const activePreviewSession = useRef<{ projectId: string; sessionId: number } | null>(null);
  const previewCloseWatchdog = useRef<number | null>(null);
  const pipelineCommandPending = useRef(false);
  const runStartedAt = useRef<number | null>(null);
  const runElapsedOffset = useRef(0);
  const cancellationOverlayTimer = useRef<number | null>(null);
  const pipelineRunningRef = useRef(isRunning);
  const accelerationRequestRevision = useRef(0);
  const [liveElapsedMs, setLiveElapsedMs] = useState(0);
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
  const [privacySettingsOpen, setPrivacySettingsOpen] = useState(false);
  const [telemetryBusy, setTelemetryBusy] = useState(false);
  const [inputMenuOpen, setInputMenuOpen] = useState(false);
  const [taskWorkspace, setTaskWorkspace] = useState(loadTaskWorkspace);
  const [activeDraftId, setActiveDraftId] = useState<string | null>(() => taskWorkspace.drafts.find((draft) => draft.running)?.id ?? null);
  const [activeProjectId, setActiveProjectId] = useState<string | null>(null);
  const [projectDetail, setProjectDetail] = useState<ProjectTaskDetail | null>(null);
  const [projectDetailLoading, setProjectDetailLoading] = useState(false);
  const [reshootBusy, setReshootBusy] = useState(false);
  const drafts = taskWorkspace.drafts;
  const selectedTask = taskWorkspace.selected;
  const selectedDraft = selectedTask.kind === "draft" ? drafts.find((draft) => draft.id === selectedTask.id) ?? null : null;
  const selectedProject = selectedTask.kind === "project" ? store.projects.find((project) => project.id === selectedTask.id) ?? projectDetail?.project ?? null : null;
  const missingEngines = store.engines.filter((engine) => !engineReady(engine));
  const completed = useMemo(() => store.projects.filter((project) => project.status === "completed"), [store.projects]);
  const unfinished = useMemo(() => store.projects.filter((project) => project.status !== "completed" && !drafts.some((draft) => draft.running && (draft.linkedProjectId === project.id || draft.id === project.workspaceTaskId))), [drafts, store.projects]);
  const progressEvent = useMemo(() => {
    if (!store.latestEvent || !["failed", "cancelled"].includes(store.latestEvent.stage)) return store.latestEvent;
    return [...store.events].reverse().find((event) => !["failed", "cancelled"].includes(event.stage)) ?? null;
  }, [store.events, store.latestEvent]);
  const activeStageIndex = stagePosition(progressEvent?.stage);
  const latestMessage = store.latestEvent
    ? localizePipelineMessage(locale, store.latestEvent.message)
    : store.progressMessage
      ? localizePipelineMessage(locale, store.progressMessage)
      : t("progress.preparing");
  const latestEventIsTerminal = store.latestEvent != null && ["failed", "cancelled"].includes(store.latestEvent.stage);
  const friendlyProgressKey = store.phase === "running" && !latestEventIsTerminal && progressEvent
    ? friendlyProgressKeyByStage[progressEvent.stage]
    : undefined;
  const friendlyProgressCount = friendlyProgressKey && progressEvent
    ? [progressEvent, ...store.events.slice().reverse()]
      .map((event) => countFromProgressEvent(event, progressEvent.stage))
      .find((count) => count != null) ?? null
    : null;
  const friendlyProgressMessage = friendlyProgressKey
    ? friendlyProgressCount
      ? t("progress.activeCount", {
        label: t(friendlyProgressKey),
        current: formatNumber(friendlyProgressCount.current),
        total: formatNumber(friendlyProgressCount.total),
      })
      : `${t(friendlyProgressKey)}…`
    : null;
  const currentMessage = friendlyProgressMessage
    ? ["reconstructing", "trainingSplats"].includes(progressEvent?.stage ?? "")
      ? t("progress.activeLongWait", { label: friendlyProgressMessage })
      : friendlyProgressMessage
    : latestMessage;
  const messageOf = useCallback((error: unknown) => rawMessageOf(error) ?? t("error.generic"), [t]);
  const currentStageLabel = useCallback((stage: string | undefined, index: number) => {
    if (stage === "completed") return t("stage.completed");
    if (stage === "failed") return t("stage.failed");
    if (stage === "cancelled") return t("stage.cancelled");
    return t(stages[index]?.[1] ?? "stage.preparing");
  }, [t]);

  const updateDraft = useCallback((id: string, update: Partial<TaskDraft> | ((draft: TaskDraft) => Partial<TaskDraft>)) => {
    setTaskWorkspace((workspace) => ({
      ...workspace,
      drafts: workspace.drafts.map((draft) => draft.id === id ? { ...draft, ...(typeof update === "function" ? update(draft) : update) } : draft),
    }));
  }, []);

  const selectDraft = useCallback((id: string) => {
    setTaskWorkspace((workspace) => ({ ...workspace, selected: { kind: "draft", id } }));
    setProjectDetail(null);
  }, []);

  const selectProject = useCallback((project: ProjectSummary) => {
    setTaskWorkspace((workspace) => ({ ...workspace, selected: { kind: "project", id: project.id } }));
    setProjectDetail(null);
  }, []);

  const addGenerationDraft = useCallback(() => {
    setTaskWorkspace((workspace) => {
      const draft = createGenerationDraft(workspace.nextOrdinal);
      return { drafts: [...workspace.drafts, draft], selected: { kind: "draft", id: draft.id }, nextOrdinal: workspace.nextOrdinal + 1 };
    });
  }, []);

  const removeDraft = useCallback((id: string) => {
    setTaskWorkspace((workspace) => {
      const target = workspace.drafts.find((draft) => draft.id === id);
      if (!target || target.running) return workspace;
      let drafts = workspace.drafts.filter((draft) => draft.id !== id);
      let nextOrdinal = workspace.nextOrdinal;
      if (!drafts.some((draft) => draft.kind === "generation" && !draft.running)) {
        drafts = [...drafts, createGenerationDraft(nextOrdinal++)];
      }
      const selected = workspace.selected.kind === "draft" && workspace.selected.id === id
        ? { kind: "draft" as const, id: drafts[0].id }
        : workspace.selected;
      return { drafts, selected, nextOrdinal };
    });
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
    store.setProjectsRoot(overview.projectsRoot);
    store.setPlannerEnabled(overview.plannerEnabled ?? true);
    store.setProjects(overview.projects);
    return overview;
  };

  const reconcileRuntimeState = useCallback(async () => {
    const runtime = await getAppRuntimeStatus();
    const appState = useAppStore.getState();
    appState.setTaskColmapAcceleration(runtime.taskAcceleration ?? null);
    if (runtime.pipelineRunning) {
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
        store.setProjectsRoot(overview.projectsRoot);
        store.setPlannerEnabled(overview.plannerEnabled ?? true);
        store.setProjects(overview.projects);
        setTaskWorkspace((workspace) => {
          const linked = new Set(overview.projects.map((project) => project.workspaceTaskId).filter(Boolean));
          const drafts = workspace.drafts.filter((draft) => !linked.has(draft.id));
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
    void initializeTelemetry()
      .then(setTelemetryPreferences)
      .catch(() => undefined);
  }, []);

  // The install guard must read live state: a download can outlive many renders.
  useEffect(() => { pipelineRunningRef.current = isRunning; }, [isRunning]);

  useEffect(() => {
    let unlisten: undefined | (() => void);
    void onPipelineEvent((event) => {
      store.receiveEvent(event);
      if (["completed", "failed", "cancelled"].includes(event.stage)) {
        setLiveElapsedMs(runElapsedOffset.current + event.elapsedMs);
      }
    }).then((fn) => { unlisten = fn; });
    return () => unlisten?.();
  }, [store.receiveEvent]);

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
    if (!isRunning || runStartedAt.current == null) return;
    return startElapsedTicker(runStartedAt.current, (elapsed) => {
      setLiveElapsedMs(runElapsedOffset.current + elapsed);
    });
  }, [isRunning]);

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
    saveTaskWorkspace(taskWorkspace.drafts, taskWorkspace.selected, taskWorkspace.nextOrdinal);
  }, [taskWorkspace]);

  useEffect(() => {
    if (selectedTask.kind !== "project") return;
    if (isRunning && selectedTask.id === activeProjectId) {
      setProjectDetail(null);
      setProjectDetailLoading(false);
      return;
    }
    let cancelled = false;
    setProjectDetailLoading(true);
    void getProjectTaskDetail(selectedTask.id)
      .then((detail) => { if (!cancelled) setProjectDetail(detail); })
      .catch((error) => { if (!cancelled) store.setError(messageOf(error)); })
      .finally(() => { if (!cancelled) setProjectDetailLoading(false); });
    return () => { cancelled = true; };
  }, [selectedTask.kind, selectedTask.id, activeProjectId, isRunning, store.setError, messageOf]);

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
    if (telemetryBusy) return;
    setTelemetryBusy(true);
    try {
      const preferences = await setTelemetryConsent(enabled);
      setTelemetryPreferences(preferences);
    } catch (error) {
      store.setError(t("progress.privacyError", { detail: messageOf(error) }));
    } finally {
      setTelemetryBusy(false);
    }
  };

  const analyze = async (draftId: string, path: string, quality: Quality, plannerEnabled = store.plannerEnabled) => {
    updateDraft(draftId, { error: null, needsValidation: false });
    try {
      const result = await probeAndPlan(path, quality, plannerEnabled);
      updateDraft(draftId, { inputType: result.inputType, video: result.video, imageSequence: result.imageSequence, plan: result.plan, estimate: result.estimate, error: null });
    } catch (error) {
      updateDraft(draftId, { video: null, imageSequence: null, plan: null, estimate: null, error: messageOf(error) });
    }
  };

  const chooseInput = async (inputType: InputType) => {
    if (!selectedDraft || selectedDraft.kind !== "generation") return;
    try {
      const selected = inputType === "images" ? await selectImageSequence() : await selectVideo();
      if (selected) {
        if (!isRunning) {
          store.setInputPath(selected, inputType);
          setActiveDraftId(null);
        }
        updateDraft(selectedDraft.id, { inputPath: selected, inputType, video: null, imageSequence: null, plan: null, estimate: null, error: null });
        await analyze(selectedDraft.id, selected, selectedDraft.quality);
      }
    } catch (error) {
      updateDraft(selectedDraft.id, { error: messageOf(error) });
    }
  };

  const chooseInputType = (inputType: InputType) => {
    setInputMenuOpen(false);
    if (selectedDraft && inputType !== selectedDraft.inputType) updateDraft(selectedDraft.id, { inputPath: null, inputType, video: null, imageSequence: null, plan: null, estimate: null, error: null });
  };

  const chooseRoot = async () => {
    const selected = await selectProjectsRoot(store.projectsRoot);
    if (!selected) return;
    try {
      const settings = await setProjectsRoot(selected);
      store.setProjectsRoot(settings.projectsRoot);
      store.setPlannerEnabled(settings.plannerEnabled);
      await refreshProjects();
    } catch (error) { store.setError(messageOf(error)); }
  };

  const chooseQuality = async (quality: Quality) => {
    if (!selectedDraft || selectedDraft.kind !== "generation") return;
    updateDraft(selectedDraft.id, { quality, plan: null, estimate: null, error: null });
    if (selectedDraft.inputPath) await analyze(selectedDraft.id, selectedDraft.inputPath, quality);
  };

  const changePlannerEnabled = async () => {
    if (isRunning) return;
    const enabled = !store.plannerEnabled;
    try {
      const settings = await setPlannerEnabled(enabled);
      store.setPlannerEnabled(settings.plannerEnabled);
      if (selectedDraft?.kind === "generation" && selectedDraft.inputPath) await analyze(selectedDraft.id, selectedDraft.inputPath, selectedDraft.quality, settings.plannerEnabled);
    } catch (error) {
      store.setError(messageOf(error));
    }
  };

  const requestCancellation = async () => {
    if (!isRunning || isCancellationRequested) return;
    setIsCancellationRequested(true);
    cancellationOverlayTimer.current = window.setTimeout(() => {
      cancellationOverlayTimer.current = null;
      setShowCancellationOverlay(true);
    }, CANCELLATION_OVERLAY_DELAY_MS);
    try {
      await cancelPipeline();
    } catch (error) {
      clearCancellationFeedback();
      store.setError(t("progress.cancelError", { detail: messageOf(error) }));
    }
  };

  const generate = async () => {
    const draft = selectedDraft;
    if (!draft || draft.kind !== "generation" || !draft.inputPath || !draft.plan || !store.projectsRoot) return;
    if (
      draft.inputType === "images"
      && draft.imageSequence?.requiresLargeSequenceConfirmation
      && !(await confirmLargeImageSequence(draft.imageSequence.imageCount))
    ) return;
    clearCancellationFeedback();
    runElapsedOffset.current = 0;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(0);
    setFailureDialog(null);
    pipelineCommandPending.current = true;
    setActiveDraftId(draft.id);
    setTaskWorkspace((workspace) => {
      let drafts = workspace.drafts.map((item) => item.id === draft.id ? { ...item, running: true, error: null } : item);
      let nextOrdinal = workspace.nextOrdinal;
      if (!drafts.some((item) => item.kind === "generation" && !item.running)) drafts = [...drafts, createGenerationDraft(nextOrdinal++)];
      return { ...workspace, drafts, nextOrdinal };
    });
    store.beginRun();
    try {
      const result = await startPipeline(draft.inputPath, draft.quality, store.projectsRoot, store.plannerEnabled, draft.id);
      setLiveElapsedMs((current) => Math.max(current, result.durationMs));
      store.setResult(result);
      store.setPhase("completed");
    } catch (error) {
      if (runStartedAt.current != null) {
        const backendElapsed = useAppStore.getState().latestEvent?.elapsedMs ?? 0;
        setLiveElapsedMs(Math.max(backendElapsed, Date.now() - runStartedAt.current));
      }
      const message = messageOf(error);
      store.setError(message);
      updateDraft(draft.id, { error: message });
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled) {
        const fallbackStage = [...useAppStore.getState().events].reverse().find((event) => !["failed", "cancelled"].includes(event.stage))?.stage;
        setFailureDialog(inferFailureDialog(error, fallbackStage));
      }
    } finally {
      try {
        const overview = await refreshProjects();
        const project = overview.projects.find((item) => item.workspaceTaskId === draft.id);
        if (project) {
          setTaskWorkspace((workspace) => ({ ...workspace, drafts: workspace.drafts.filter((item) => item.id !== draft.id), selected: { kind: "project", id: project.id } }));
        } else {
          updateDraft(draft.id, { running: false });
        }
      } catch { updateDraft(draft.id, { running: false }); }
      setActiveDraftId(null);
      pipelineCommandPending.current = false;
    }
  };

  const resume = async (project: ProjectSummary) => {
    selectProject(project);
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
    pipelineCommandPending.current = true;
    store.beginRun();
    try {
      const result = await resumePipeline(project.id);
      setLiveElapsedMs((current) => Math.max(current, result.durationMs));
      store.setResult(result);
      store.setPhase("completed");
    } catch (error) {
      const backendElapsed = useAppStore.getState().latestEvent?.elapsedMs ?? 0;
      setLiveElapsedMs(runElapsedOffset.current + backendElapsed);
      const message = messageOf(error);
      store.setError(message);
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled) {
        const fallbackStage = [...useAppStore.getState().events].reverse().find((event) => !["failed", "cancelled"].includes(event.stage))?.stage;
        setFailureDialog(inferFailureDialog(error, fallbackStage, project.id));
      }
    } finally {
      try {
        const overview = await refreshProjects();
        const refreshed = overview.projects.find((item) => item.id === project.id);
        if (refreshed) selectProject(refreshed);
      } catch { /* the project remains on disk */ }
      pipelineCommandPending.current = false;
      setActiveProjectId(null);
    }
  };

  const removeProject = async (project: ProjectSummary) => {
    if (deletingProjectId) return;
    setDeletingProjectId(project.id);
    try {
      await reconcileRuntimeState();
      if (await confirmAndDeleteProject(project)) {
        await refreshProjects();
        if (selectedTask.kind === "project" && selectedTask.id === project.id) {
          const fallback = drafts.find((draft) => !draft.running) ?? drafts[0];
          if (fallback) selectDraft(fallback.id);
        }
      }
    } catch (error) {
      store.setError(messageOf(error));
    } finally {
      setDeletingProjectId((current) => current === project.id ? null : current);
    }
  };

  const showProject = async (project: ProjectSummary) => {
    if (revealingProjectId) return;
    setRevealingProjectId(project.id);
    try {
      await withTimeout(revealProject(project), NATIVE_ACTION_TIMEOUT_MS, t("error.timeout"));
    } catch (error) {
      store.setError(t("error.openFolder", { detail: messageOf(error) }));
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
      store.setError(t("error.previewCleanupTimeout"));
      void releasePreviewSession(projectId).catch(() => undefined);
      return;
    }
    try {
      await withTimeout(releasePreviewSession(projectId), PREVIEW_CLOSE_TIMEOUT_MS, t("error.previewCleanupTimeout"));
    } catch (error) {
      store.setError(messageOf(error));
    } finally {
      clearPreviewSession(projectId, sessionId);
    }
  }, [clearPreviewSession, messageOf, releasePreviewSession, store.setError, t]);

  const previewProject = async (project: ProjectSummary) => {
    if (project.status !== "completed" || openingPreviewProjectId || closingPreviewProjectId === project.id) return;
    const previous = useGaussianTransformStore.getState().descriptor?.projectId;
    setOpeningPreviewProjectId(project.id);
    store.setError(null);
    try {
      await reconcileRuntimeState();
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
      store.setError(messageOf(error));
    } finally {
      setOpeningPreviewProjectId(null);
    }
  };

  const openReshoot = (project: ProjectSummary) => {
    if (isRunning || reshootBusy) return;
    const draft = createReshootDraft(taskWorkspace.nextOrdinal, project.id, project.name, project.quality);
    setTaskWorkspace((workspace) => ({ drafts: [...workspace.drafts, draft], selected: { kind: "draft", id: draft.id }, nextOrdinal: workspace.nextOrdinal + 1 }));
  };

  const chooseReshootInput = async (inputType: InputType) => {
    const draft = selectedDraft;
    if (!draft || draft.kind !== "reshoot" || !draft.sourceProjectId || reshootBusy) return;
    updateDraft(draft.id, { inputType });
    const selected = inputType === "images" ? await selectImageSequence() : await selectVideo();
    if (!selected) return;
    setReshootBusy(true);
    updateDraft(draft.id, { inputPath: selected, reshootPlan: null, error: null });
    try {
      const plan = await probeReshootInput(draft.sourceProjectId, selected, inputType);
      updateDraft(draft.id, { reshootPlan: plan, estimate: plan.estimate, error: plan.compatible ? null : plan.incompatibilityReason ?? t("reshoot.ineligible") });
      if (!plan.compatible) {
        store.setError(plan.incompatibilityReason);
      }
    } catch (error) {
      const message = messageOf(error);
      updateDraft(draft.id, { error: message });
      store.setError(message);
    } finally {
      setReshootBusy(false);
    }
  };

  const runReshoot = async () => {
    const draft = selectedDraft;
    if (!draft || draft.kind !== "reshoot" || !draft.sourceProjectId || !draft.inputPath || !draft.reshootPlan?.compatible || isRunning || !store.projectsRoot) return;
    const source = await inspectReshootSource(draft.sourceProjectId);
    if (!source.eligible) {
      updateDraft(draft.id, { reshootSource: source, error: source.reason ?? t("reshoot.ineligible") });
      return;
    }
    const finalPlan = await probeReshootInput(draft.sourceProjectId, draft.inputPath, draft.inputType);
    if (!finalPlan.compatible) {
      updateDraft(draft.id, { reshootPlan: finalPlan, error: finalPlan.incompatibilityReason ?? t("reshoot.ineligible") });
      return;
    }
    clearCancellationFeedback();
    runElapsedOffset.current = 0;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(0);
    setFailureDialog(null);
    pipelineCommandPending.current = true;
    setActiveDraftId(draft.id);
    updateDraft(draft.id, { running: true, error: null });
    store.beginRun();
    try {
      const result = await startReshootPipeline({
        sourceProjectId: draft.sourceProjectId,
        reshootPath: draft.inputPath,
        inputType: draft.inputType,
        projectsRoot: store.projectsRoot,
        workspaceTaskId: draft.id,
      });
      store.setResult(result);
      store.setPhase("completed");
    } catch (error) {
      const message = messageOf(error);
      store.setError(message);
      updateDraft(draft.id, { error: message });
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled) {
        const fallbackStage = [...useAppStore.getState().events].reverse().find((event) => !["failed", "cancelled"].includes(event.stage))?.stage;
        setFailureDialog(inferFailureDialog(error, fallbackStage));
      }
    } finally {
      try {
        const overview = await refreshProjects();
        const project = overview.projects.find((item) => item.workspaceTaskId === draft.id);
        if (project) setTaskWorkspace((workspace) => ({ ...workspace, drafts: workspace.drafts.filter((item) => item.id !== draft.id), selected: { kind: "project", id: project.id } }));
        else updateDraft(draft.id, { running: false });
      } catch { updateDraft(draft.id, { running: false }); }
      setActiveDraftId(null);
      pipelineCommandPending.current = false;
    }
  };

  useEffect(() => {
    const draft = selectedDraft;
    if (!draft?.needsValidation || draft.running) return;
    updateDraft(draft.id, { needsValidation: false, error: null });
    if (draft.kind === "generation") {
      if (draft.inputPath) void analyze(draft.id, draft.inputPath, draft.quality);
      return;
    }
    if (!draft.sourceProjectId) return;
    let cancelled = false;
    setReshootBusy(true);
    void inspectReshootSource(draft.sourceProjectId)
      .then(async (source) => {
        if (cancelled) return;
        updateDraft(draft.id, { reshootSource: source, error: source.eligible ? null : source.reason ?? t("reshoot.ineligible") });
        if (source.eligible && draft.inputPath) {
          const plan = await probeReshootInput(draft.sourceProjectId!, draft.inputPath, draft.inputType);
          if (!cancelled) updateDraft(draft.id, { reshootPlan: plan, estimate: plan.estimate, error: plan.compatible ? null : plan.incompatibilityReason });
        }
      })
      .catch((error) => { if (!cancelled) updateDraft(draft.id, { error: messageOf(error) }); })
      .finally(() => { if (!cancelled) setReshootBusy(false); });
    return () => {
      cancelled = true;
      setReshootBusy(false);
    };
  // Validation is intentionally lazy and keyed only by selection.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedTask.kind, selectedTask.id]);

  const closeFailureDialog = useCallback(() => {
    if (failureDialogAction === null) setFailureDialog(null);
  }, [failureDialogAction]);

  const retryFailedProject = async () => {
    const projectId = failureDialog?.projectId;
    if (!projectId || failureDialogAction) return;
    setFailureDialogAction("retry");
    try {
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
      store.setError(t("error.openFolder", { detail: messageOf(error) }));
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
  const detailStageIndex = stagePosition(projectDetail?.stage);

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
        {telemetryPreferences && <button className="settings-action" type="button" onClick={() => setPrivacySettingsOpen(true)}><Settings2 size={15} />{t("top.settings")}</button>}
        <div className="engine-summary"><span className={missingEngines.length ? "status-light warning" : "status-light"} />{store.engines.length === 0 ? t("top.checkingEngines") : missingEngines.length ? t("top.engineIssues", { count: missingEngines.length }) : t("top.enginesReady")}</div>
      </div>
    </header>

    <section className="workspace" ref={workspaceRef} style={{ "--left-pane-width": `${leftPanePercent}%` } as CSSProperties}>
      <section className="control-pane" ref={controlPaneRef} aria-label={t("task.console")}>
        <div className="pane-header"><h1>{selectedProject?.name ?? (selectedDraft?.kind === "reshoot" ? draftDisplayName(selectedDraft, t("workspace.newTask"), t("project.reshoot")) : t("task.create"))}</h1><span className={selectedShowsLiveRun ? "run-state active" : "run-state"}>{selectedProject ? t(statusKey[selectedProject.status]) : selectedShowsLiveRun ? t("task.running") : t("task.idle")}</span></div>

        {selectedDraft?.kind === "generation" && <><div className="form-section">
          <label className="field-label">{t("input.label")}</label>
          <div className="input-picker">
            <div className="input-type-picker">
              <button className="input-picker-toggle" type="button" disabled={selectedDraft.running} aria-label={t("input.typeAria")} aria-expanded={inputMenuOpen} onClick={() => setInputMenuOpen((open) => !open)}>
                {selectedDraft.inputType === "images" ? <Images size={16} /> : <Clapperboard size={16} />}
                <span>{selectedDraft.inputType === "images" ? t("input.images") : t("input.video")}</span>
                <ChevronDown size={14} />
              </button>
              {inputMenuOpen && <div className="input-picker-menu" role="menu">
                <button type="button" role="menuitemradio" aria-checked={selectedDraft.inputType === "video"} onClick={() => chooseInputType("video")}><Clapperboard size={15} /><span><strong>{t("input.video")}</strong><small>{t("input.videoTypes")}</small></span></button>
                <button type="button" role="menuitemradio" aria-checked={selectedDraft.inputType === "images"} onClick={() => chooseInputType("images")}><Images size={15} /><span><strong>{t("input.images")}</strong><small>{t("input.imageTypes")}</small></span></button>
              </div>}
            </div>
            <button className="path-picker" type="button" disabled={selectedDraft.running} onClick={() => void chooseInput(selectedDraft.inputType)}>
              {selectedDraft.inputType === "images" ? <Images size={18} /> : <Clapperboard size={18} />}
              <span>
                <strong>{selectedDraft.inputPath ? basename(selectedDraft.inputPath) : selectedDraft.inputType === "images" ? t("input.selectImages") : t("input.selectVideo")}</strong>
                <small>{selectedDraft.inputPath ?? (selectedDraft.inputType === "images" ? t("input.selectImagesHint") : t("input.selectVideoHint"))}</small>
              </span>
            </button>
          </div>
        </div>

        <div className="form-section">
          <label className="field-label">{t("project.root")}</label>
          <button className="path-picker compact" type="button" disabled={selectedDraft.running} onClick={() => void chooseRoot()}>
            <FolderOpen size={18} /><span><strong>{store.projectsRoot ? basename(store.projectsRoot) : t("project.readingRoot")}</strong><small>{store.projectsRoot || "Documents / SplatStudio / Projects"}</small></span><ChevronRight size={16} />
          </button>
          <p className="field-note">{t("project.rootHint")}</p>
        </div>

        <div className="form-section">
          <label className="field-label">{t("quality.label")}</label>
          <div className="quality-settings">
            <div className="quality-list" role="radiogroup">
              {qualities.map((quality) => <button key={quality.value} type="button" role="radio" disabled={selectedDraft.running} aria-checked={selectedDraft.quality === quality.value} className={selectedDraft.quality === quality.value ? "quality-option selected" : "quality-option"} onClick={() => void chooseQuality(quality.value)}>
                <span className="radio-mark"><span /></span><span><strong>{t(quality.label)}</strong><small>{t(quality.description)}</small></span>
              </button>)}
            </div>
            <button className="planner-switch" type="button" role="switch" aria-checked={store.plannerEnabled} disabled={isRunning} onClick={() => void changePlannerEnabled()}>
              <span><strong>{t("planner.label")}</strong><small>{t("planner.hint")}</small></span>
              <i aria-hidden="true"><span /></i>
            </button>
          </div>
        </div>

        <div className={`acceleration-status ${systemDetectionTemporary ? "warning" : systemAcceleration?.backend === "gpu" ? "gpu" : systemAccelerationWarning ? "warning" : "cpu"}`} aria-live="polite">
          <span className="acceleration-icon">{systemAcceleration == null ? <LoaderCircle className="spin" size={17} /> : systemDetectionTemporary || systemAccelerationWarning ? <CircleAlert size={17} /> : systemAcceleration.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : <Cpu size={17} />}</span>
          <span>
            <strong>{systemAcceleration == null ? t("gpu.detecting") : systemDetectionTemporary ? t("gpu.temporarilyUnavailable") : systemAcceleration.backend === "gpu" ? t("gpu.enabled") : t("gpu.cpu")}</strong>
            <small>{systemAcceleration == null ? t("gpu.reading") : systemDetectionTemporary ? `${t("gpu.temporaryHint")}${systemAcceleration.device ? ` · ${systemAcceleration.device.name}` : ""}` : systemAcceleration.backend === "gpu" && systemAcceleration.device ? `${systemAcceleration.device.name}${systemAcceleration.device.totalMemoryMb ? ` · ${t("gpu.memory", { value: (systemAcceleration.device.totalMemoryMb / 1024).toFixed(1) })}` : ""} · ${t("gpu.driver", { value: systemAcceleration.device.driverVersion })} · Compute Capability ${systemAcceleration.device.computeCapability}` : systemAcceleration.reasonCode === "macOsCpuOnly" ? localizePipelineMessage(locale, systemAcceleration.reason) : `${localizePipelineMessage(locale, systemAcceleration.reason)} · ${t("gpu.requirements", { driver: systemAcceleration.requirements.minimumDriverVersion, capability: systemAcceleration.requirements.minimumComputeCapability })}`}</small>
            {isRunning && store.taskColmapAcceleration && <small>{t("gpu.currentTask")} · {store.taskColmapAcceleration.backend === "gpu" ? t("gpu.taskGpu", { device: store.taskColmapAcceleration.device?.name ?? "NVIDIA GPU" }) : t("gpu.taskCpu")}</small>}
          </span>
        </div>

        {(selectedDraft.video || selectedDraft.imageSequence) && selectedDraft.plan && <div className="source-metrics">
          <span><small>{selectedDraft.inputType === "images" ? t("metrics.imageCount") : t("metrics.duration")}</small><b>{selectedDraft.imageSequence ? t("common.images", { count: formatNumber(selectedDraft.imageSequence.imageCount) }) : formatVideoDuration(selectedDraft.video!.duration)}</b></span>
          <span><small>{t("metrics.resolution")}</small><b>{selectedDraft.imageSequence?.width ?? selectedDraft.video?.width} × {selectedDraft.imageSequence?.height ?? selectedDraft.video?.height}</b></span>
          <span><small>{selectedDraft.inputType === "images" ? t("metrics.processingImages") : t("metrics.estimatedFrames")}</small><b>{selectedDraft.inputType === "images" ? t("metrics.keepAll") : t("metrics.approx", { value: formatNumber(selectedDraft.plan.estimatedFrames) })}</b></span>
          <span title={selectedDraft.estimate ? localizePipelineMessage(locale, selectedDraft.estimate.basis) : undefined}><small>{t("metrics.estimate")}</small><b>{selectedDraft.estimate ? t("metrics.approx", { value: formatDuration(selectedDraft.estimate.estimatedMs) }) : t("metrics.analyzing")}</b>{selectedDraft.estimate && <em>{formatDuration(selectedDraft.estimate.lowerBoundMs)}–{formatDuration(selectedDraft.estimate.upperBoundMs)}</em>}</span>
        </div>}

        {(selectedDraft.video?.hasAlpha || selectedDraft.imageSequence?.hasAlpha) && <div className="alpha-source-status" role="status">
          <Blend size={17} />
          <span><strong>{selectedDraft.inputType === "images" ? t("alpha.imagesTitle") : t("alpha.videoTitle")}</strong><small>{selectedDraft.inputType === "images" ? t("alpha.imagesHint") : t("alpha.videoHint", { format: selectedDraft.video?.pixelFormat || "Alpha" })}</small></span>
        </div>}

        {selectedDraft.imageSequence?.requiresLargeSequenceConfirmation && <div className="sequence-warning" role="status"><CircleAlert size={16} /><span><strong>{t("sequence.title")}</strong><small>{t("sequence.hint")}</small></span></div>}

        <button className="primary-action" type="button" disabled={isRunning || !selectedDraft.inputPath || !selectedDraft.plan || !store.projectsRoot || missingEngines.length > 0} onClick={() => void generate()}><Play size={16} fill="currentColor" />{t("generate.start")}</button>
        {selectedDraft.error && <div className="inline-error" role="alert"><CircleAlert size={16} /><CompactError message={selectedDraft.error} /><button type="button" onClick={() => updateDraft(selectedDraft.id, { error: null })}>{t("common.close")}</button></div>}
        </>}

        {selectedDraft?.kind === "reshoot" && <section className="reshoot-task-page">
          <p className="reshoot-task-intro">{t("reshoot.sameCameraHint")}</p>
          <ul className="reshoot-capture-tips"><li>{t("reshoot.sameDevice")}</li><li>{t("reshoot.sameFraming")}</li><li>{t("reshoot.keepOverlap")}</li></ul>
          {selectedDraft.reshootSource && <dl className="reshoot-source-summary"><div><dt>{t("project.quality")}</dt><dd>{t(qualityKey[selectedDraft.quality])}</dd></div><div><dt>{t("metrics.resolution")}</dt><dd>{selectedDraft.reshootSource.width} × {selectedDraft.reshootSource.height}</dd></div><div><dt>{t("reshoot.camera")}</dt><dd>{selectedDraft.reshootSource.cameraModel} · ID {selectedDraft.reshootSource.cameraId}</dd></div></dl>}
          <div className="reshoot-input-options"><button type="button" disabled={reshootBusy || !selectedDraft.reshootSource?.eligible || selectedDraft.running} onClick={() => void chooseReshootInput("video")}><Clapperboard size={18} /><strong>{t("reshoot.chooseVideo")}</strong><small>{t("reshoot.chooseVideoHint")}</small></button><button type="button" disabled={reshootBusy || !selectedDraft.reshootSource?.eligible || selectedDraft.running} onClick={() => void chooseReshootInput("images")}><Images size={18} /><strong>{t("reshoot.chooseImages")}</strong><small>{t("reshoot.chooseImagesHint")}</small></button></div>
          {reshootBusy && <p className="reshoot-analysis"><LoaderCircle className="spin" size={14} />{t("reshoot.analyzing")}</p>}
          {selectedDraft.reshootPlan && <div className={selectedDraft.reshootPlan.compatible ? "reshoot-plan compatible" : "reshoot-plan incompatible"}><strong>{selectedDraft.inputPath ? basename(selectedDraft.inputPath) : ""}</strong><span>{selectedDraft.reshootPlan.imageCount != null ? t("reshoot.imageCount", { count: selectedDraft.reshootPlan.imageCount }) : t("reshoot.duration", { value: formatVideoDuration(selectedDraft.reshootPlan.duration ?? 0) })}</span><span>{selectedDraft.reshootPlan.preparedWidth} × {selectedDraft.reshootPlan.preparedHeight} · {selectedDraft.reshootPlan.hasAlpha ? t("reshoot.alphaDetected") : t("reshoot.opaque")}</span><span>{t("reshoot.estimated", { value: formatDuration(selectedDraft.reshootPlan.estimate.estimatedMs) })}</span></div>}
          {selectedDraft.error && <div className="inline-error" role="alert"><CircleAlert size={16} /><CompactError message={selectedDraft.error} /><button type="button" onClick={() => updateDraft(selectedDraft.id, { error: null })}>{t("common.close")}</button></div>}
          <button className="primary-action" type="button" disabled={isRunning || reshootBusy || !selectedDraft.reshootPlan?.compatible} onClick={() => void runReshoot()}><Play size={16} fill="currentColor" />{t("generate.start")}</button>
        </section>}

        {selectedProject && <section className="project-detail-page">
          {projectDetailLoading && !projectDetail ? <div className="task-detail-loading"><LoaderCircle className="spin" size={20} />{t("workspace.loadingTask")}</div> : <>
            <p className="project-path" title={selectedProject.projectPath}>{selectedProject.projectPath}</p>
            <dl className="project-detail-stats"><div><dt>{t("result.splats")}</dt><dd>{selectedProject.splatCount == null ? "-" : formatNumber(selectedProject.splatCount)}</dd></div><div><dt>{t("result.fileSize")}</dt><dd>{formatBytes(selectedProject.fileSize, locale)}</dd></div><div><dt>{t("result.registered")}</dt><dd>{projectDetail?.registeredImages == null ? "-" : `${formatNumber(projectDetail.registeredImages)} / ${formatNumber(projectDetail.inputImages ?? 0)}`}</dd></div><div><dt>{t("result.points")}</dt><dd>{selectedProject.points3d == null ? "-" : formatNumber(selectedProject.points3d)}</dd></div><div><dt>{t("project.elapsed")}</dt><dd>{formatDuration(selectedProject.durationMs)}</dd></div><div><dt>{t("project.quality")}</dt><dd>{t(qualityKey[selectedProject.quality])}</dd></div><div><dt>{t("progress.stage")}</dt><dd>{currentStageLabel(projectDetail?.stage, detailStageIndex)}</dd></div></dl>
            {selectedProject.registeredRatio != null && selectedProject.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t("result.lowRegistration", { value: (selectedProject.registeredRatio * 100).toFixed(1) })}</p>}
            {selectedProject.failureMessage && <div className="inline-error" role="alert"><CircleAlert size={16} /><CompactError message={selectedProject.failureMessage} /></div>}
            <div className="project-detail-actions">{selectedProject.status === "completed" ? <><button type="button" onClick={() => void previewProject(selectedProject)}><Eye size={14} />{t("project.preview")}</button><button type="button" disabled={isRunning} onClick={() => void openReshoot(selectedProject)}><Film size={14} />{t("project.reshoot")}</button></> : selectedProject.status !== "running" && <button type="button" disabled={isRunning} onClick={() => void resume(selectedProject)}><Play size={14} />{t("project.resume")}</button>}<button type="button" onClick={() => void showProject(selectedProject)}><FolderOpen size={14} />{t("project.reveal")}</button><button type="button" disabled={isRunning} onClick={() => void removeProject(selectedProject)}><Trash2 size={14} />{t("project.delete")}</button></div>
            {projectDetail && <section className="live-process historical"><div className="live-heading"><div><strong>{t("progress.title")}</strong></div><span className="mono">{projectDetail.progress.toFixed(1)}%</span></div><ol className="stage-timeline">{stages.map(([key, label], index) => <li key={key} className={index < detailStageIndex || selectedProject.status === "completed" ? "done" : index === detailStageIndex ? selectedProject.status === "failed" ? "failed" : selectedProject.status === "cancelled" ? "cancelled" : "active" : ""}><span /><b>{t(label)}</b></li>)}</ol><div className="log-toolbar"><span>{t("progress.log")}</span><small>{t("progress.logCount", { count: projectDetail.logs.length })}</small></div><div className="live-log">{projectDetail.logs.map((line, index) => <div className="log-line historical" key={`${line.source}-${index}`}><time /><span>{line.source}</span><p>{line.message}</p></div>)}</div></section>}
          </>}
        </section>}

        {selectedShowsLiveRun && <section className="live-process">
          <div className="live-heading"><div><span className="live-dot" /><strong>{t("progress.title")}</strong></div><span className="mono">{store.progress.toFixed(1)}%</span></div>
          <p className="current-message">{currentMessage}</p>
          <div className="process-metrics">
            <span><small>{t("progress.stage")}</small><b>{currentStageLabel(store.latestEvent?.stage, activeStageIndex)}</b></span>
            <span><small>{t("progress.elapsed")}</small><b>{formatDuration(liveElapsedMs)}</b></span>
          </div>
          <ol className="stage-timeline">
            {stages.map(([key, label], index) => {
              const terminalClass = index === activeStageIndex && store.phase === "failed" ? "failed" : index === activeStageIndex && store.phase === "cancelled" ? "cancelled" : "";
              const className = index < activeStageIndex || store.phase === "completed"
                ? "done"
                : terminalClass || (index === activeStageIndex && isRunning ? "active" : "");
              return <li key={key} className={className}><span /><b>{t(label)}</b>{index === activeStageIndex && isRunning && <small>{progressEvent?.indeterminate ? t("progress.running") : `${(progressEvent?.stageProgress ?? 0).toFixed(0)}%`}</small>}</li>;
            })}
          </ol>
          <div className="log-toolbar"><span>{t("progress.log")}</span><small>{t("progress.logCount", { count: store.events.length })}</small></div>
          <div className="live-log" aria-live="polite" ref={liveLogRef} onScroll={updateLiveLogFollow}>
            {store.events.map((event, index) => <div className={`log-line ${event.level}`} key={`${event.sequence}-${index}`}><time>{new Date(event.timestamp).toLocaleTimeString(locale, { hour12: false })}</time><span>{event.engine ?? "system"}</span><p>{event.kind === "log" ? event.message : localizePipelineMessage(locale, event.message)}</p></div>)}
          </div>
          {isRunning && <button className="cancel-action" type="button" disabled={isCancellationRequested} onClick={() => void requestCancellation()}>{isCancellationRequested ? <LoaderCircle className="spin" size={13} /> : <Square size={12} fill="currentColor" />}{isCancellationRequested ? t("progress.terminating") : t("progress.cancel")}</button>}
        </section>}

        {store.error && <div className="inline-error" role="alert"><CircleAlert size={16} /><CompactError message={store.error} /><button type="button" onClick={() => store.setError(null)}>{t("common.close")}</button></div>}
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
        <div className="pane-header"><h2>{t("history.title")}</h2><button className="refresh-action" type="button" onClick={() => void refreshProjects()}><RotateCcw size={14} />{t("history.refresh")}</button></div>
        <div className="archive-summary task-summary"><span><b>{drafts.length}</b><small>{t("workspace.newTasks")}</small></span><span><b>{completed.length}</b><small>{t("history.completed")}</small></span><span><b>{unfinished.length}</b><small>{t("history.unfinished")}</small></span></div>

        <div className="project-group new-task-group"><div className="group-heading"><span>{t("workspace.newTasks")}</span><button className="group-add-action" type="button" onClick={addGenerationDraft}><Plus size={13} />{t("workspace.addTask")}</button></div>{drafts.map((draft) => <DraftRow key={draft.id} draft={draft} selected={selectedTask.kind === "draft" && selectedTask.id === draft.id} onSelect={() => selectDraft(draft.id)} onDelete={() => removeDraft(draft.id)} />)}</div>
        <div className="project-group"><div className="group-heading"><span>{t("history.completed")}</span><small>{t("history.projects", { count: completed.length })}</small></div>{completed.map((project) => <ProjectRow key={project.id} project={project} selected={selectedTask.kind === "project" && selectedTask.id === project.id} busy={isRunning} previewing={openingPreviewProjectId === project.id} previewDisabled={openingPreviewProjectId !== null || closingPreviewProjectId === project.id} deleting={deletingProjectId === project.id} revealing={revealingProjectId === project.id} onSelect={selectProject} onPreview={(item) => void previewProject(item)} onReshoot={(item) => void openReshoot(item)} onResume={() => undefined} onReveal={(item) => void showProject(item)} onDelete={(item) => void removeProject(item)} />)}</div>
        <div className="project-group unfinished"><div className="group-heading"><span>{t("history.unfinished")}</span><small>{t("history.projects", { count: unfinished.length })}</small></div>{unfinished.map((project) => <ProjectRow key={project.id} project={project} selected={selectedTask.kind === "project" && selectedTask.id === project.id} busy={isRunning} previewing={false} previewDisabled deleting={deletingProjectId === project.id} revealing={revealingProjectId === project.id} onSelect={selectProject} onPreview={() => undefined} onReshoot={() => undefined} onResume={(item) => void resume(item)} onReveal={(item) => void showProject(item)} onDelete={(item) => void removeProject(item)} />)}</div>
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
    {failureDialog && <FailureGuidanceDialog failure={failureDialog} action={failureDialogAction} onClose={closeFailureDialog} onRetry={() => void retryFailedProject()} onOpenLogs={() => void openFailureLogs()} />}
    {telemetryPreferences && !telemetryPreferences.consentDecided && <TelemetryPreferences mode="consent" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} />}
    {telemetryPreferences && privacySettingsOpen && <TelemetryPreferences mode="settings" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} onClose={() => setPrivacySettingsOpen(false)} />}
  </main>;
}
