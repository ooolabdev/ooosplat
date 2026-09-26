import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  Blend, ChevronDown, ChevronRight, CircleAlert, Clapperboard, Cpu, FileBox, Images,
  Download, Eye, FolderOpen, LoaderCircle, MapPin, Minus, Play, Plus, RotateCcw, Square, Trash2,
  Languages, Settings2, X, Zap,
} from "lucide-react";
import appLogo from "../../assets/app-icon.svg";
import packageMetadata from "../../package.json";
import { TelemetryPreferences } from "../components/TelemetryPreferences";
import {
  cancelPipeline, checkEngines, confirmAndDeleteProject, confirmLargeImageSequence,
  estimateProjectRuntime, exportPly, getAppRuntimeStatus, getProjectOverview, onPipelineEvent, probeAndPlan, revealProject, revealProjectLogs,
  selectImageSequence, selectProjectsRoot, selectVideo,
  setProjectsRoot, startPipeline, prepareGaussianPreview, releaseGaussianPreview,
  initializeTelemetry, setTelemetryConsent, resumePipeline,
} from "../lib/backend";
import { startElapsedTicker } from "../lib/elapsedTimer";
import { pipelineCommandError, pipelineErrorMessage, pipelineWasCancelled, type PipelineFailureKind } from "../lib/pipelineError";
import { localizePipelineMessage, useI18n, type TranslationKey } from "../i18n";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import type { EngineStatus, InputType, ProjectStatus, ProjectSummary, Quality } from "../types/pipeline";
import type { TelemetryPreferences as TelemetryPreferencesState } from "../types/telemetry";

const GaussianViewer = lazy(() => import("../components/GaussianViewer").then((module) => ({ default: module.GaussianViewer })));
const CANCELLATION_OVERLAY_DELAY_MS = 300;
const PREVIEW_CLOSE_TIMEOUT_MS = 8_000;
const NATIVE_ACTION_TIMEOUT_MS = 8_000;
const MAPPER_REFINEMENT_PATTERN = /retriangulation|global bundle adjustment/i;

type FailureDialogState = {
  kind: PipelineFailureKind;
  projectId: string | null;
  rawMessage: string;
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
    kind = /early eof|failed to load dataset|i\/o error|io error|no such file|access denied|permission denied/i.test(rawMessage)
      ? "brush_dataset"
      : "brush_gpu";
  }
  if (!kind) return null;
  return { kind, projectId: structured?.projectId ?? fallbackProjectId ?? null, rawMessage };
};

function FailureGuidanceDialog({ failure, action, onClose, onRetry, onOpenLogs }: {
  failure: FailureDialogState;
  action: "retry" | "logs" | null;
  onClose: () => void;
  onRetry: () => void;
  onOpenLogs: () => void;
}) {
  const { t } = useI18n();
  const mapper = failure.kind === "mapper_source" || failure.kind === "mapper_storage";
  const dataset = failure.kind === "brush_dataset";
  const title = mapper ? t("failure.mapperTitle") : dataset ? t("failure.brushDatasetTitle") : t("failure.brushTitle");
  const description = failure.kind === "mapper_source"
    ? t("failure.mapperSource")
    : failure.kind === "mapper_storage"
      ? t("failure.mapperStorage")
      : dataset
        ? t("failure.brushDataset")
        : t("failure.brushGpu");
  const tips: TranslationKey[] = failure.kind === "mapper_source"
    ? ["failure.mapperTip1", "failure.mapperTip2", "failure.mapperTip3"]
    : failure.kind === "mapper_storage"
      ? ["failure.storageTip1", "failure.storageTip2"]
      : dataset
        ? ["failure.datasetTip1", "failure.datasetTip2"]
        : ["failure.brushTip1", "failure.brushTip2", "failure.brushTip3"];

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && action === null) onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [action, onClose]);

  return <div className="failure-guidance-backdrop" role="dialog" aria-modal="true" aria-labelledby="failure-guidance-title">
    <section className="failure-guidance-dialog">
      <div className="failure-guidance-heading">
        <span><CircleAlert size={22} /></span>
        <div><small>{mapper ? "COLMAP" : "Brush"}</small><h2 id="failure-guidance-title">{title}</h2></div>
        <button type="button" aria-label={t("common.close")} disabled={action !== null} onClick={onClose}><X size={17} /></button>
      </div>
      <p>{description}</p>
      <strong>{t("failure.solutions")}</strong>
      <ul>{tips.map((key) => <li key={key}>{t(key)}</li>)}</ul>
      {failure.rawMessage && <details><summary>{t("failure.details")}</summary><pre>{failure.rawMessage}</pre></details>}
      <div className="failure-guidance-actions">
        <button type="button" className="secondary" disabled={action !== null || !failure.projectId} onClick={onOpenLogs}>{action === "logs" ? <LoaderCircle className="spin" size={14} /> : <FolderOpen size={14} />}{t("failure.openLogs")}</button>
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

function ProjectRow({ project, busy, previewing, previewDisabled, deleting, revealing, onPreview, onResume, onReveal, onDelete }: {
  project: ProjectSummary;
  busy: boolean;
  previewing: boolean;
  previewDisabled: boolean;
  deleting: boolean;
  revealing: boolean;
  onPreview: (project: ProjectSummary) => void;
  onResume: (project: ProjectSummary) => void;
  onReveal: (project: ProjectSummary) => void;
  onDelete: (project: ProjectSummary) => void;
}) {
  const { locale, t, formatDate, formatDuration } = useI18n();
  return <article className="project-row">
    <div className="project-row-main">
      <div className="project-title-line">
        <span className={`project-status ${project.status}`} />
        <strong>{project.name}</strong>
        <span className="status-copy">{t(statusKey[project.status])}</span>
      </div>
      <p className="project-path" title={project.projectPath}>{project.projectPath}</p>
      {project.failureMessage && <p className="project-failure">{localizePipelineMessage(locale, project.failureMessage)}</p>}
      {project.registeredRatio != null && project.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t("result.lowRegistration", { value: (project.registeredRatio * 100).toFixed(1) })}</p>}
    </div>
    <dl className="project-stats">
      <div><dt>PLY</dt><dd>{formatBytes(project.fileSize, locale)}</dd></div>
      <div><dt>{t("project.date")}</dt><dd>{formatDate(project.completedAt ?? project.createdAt)}</dd></div>
      <div><dt>{t("project.elapsed")}</dt><dd>{formatDuration(project.durationMs)}</dd></div>
      <div><dt>{t("project.quality")}</dt><dd>{t(qualityKey[project.quality])}</dd></div>
    </dl>
    <div className="project-actions">
      {project.status === "completed" && <button className="preview-link" type="button" disabled={previewDisabled} onClick={() => onPreview(project)}>{previewing ? <LoaderCircle className="spin" size={14} /> : <Eye size={14} />}{previewing ? t("project.opening") : t("project.preview")}</button>}
      {project.status !== "completed" && <button className="resume-link" type="button" disabled={busy} onClick={() => onResume(project)}><Play size={14} fill="currentColor" />{t("project.resume")}</button>}
      <button type="button" disabled={revealing} onClick={() => onReveal(project)}>{revealing ? <LoaderCircle className="spin" size={14} /> : <MapPin size={14} />}{t("project.reveal")}</button>
      <button className="danger-link" type="button" disabled={busy || deleting} onClick={() => onDelete(project)}>{deleting ? <LoaderCircle className="spin" size={14} /> : <Trash2 size={14} />}{t("project.delete")}</button>
    </div>
  </article>;
}

export function App() {
  const { locale, t, toggleLocale, formatNumber, formatDuration } = useI18n();
  const store = useAppStore();
  const loadGaussian = useGaussianTransformStore((state) => state.load);
  const closeGaussian = useGaussianTransformStore((state) => state.close);
  const isRunning = store.phase === "running";
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
  const missingEngines = store.engines.filter((engine) => !engineReady(engine));
  const completed = useMemo(() => store.projects.filter((project) => project.status === "completed"), [store.projects]);
  const unfinished = useMemo(() => store.projects.filter((project) => project.status !== "completed"), [store.projects]);
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
  const mapperPhaseKeepsRegistrationCount = store.latestEvent?.stage === "reconstructing"
    && store.latestEvent.current != null
    && store.latestEvent.total != null
    && MAPPER_REFINEMENT_PATTERN.test(store.latestEvent.message);
  const currentMessage = mapperPhaseKeepsRegistrationCount
    ? `${latestMessage} · ${t("progress.registered", {
      current: formatNumber(store.latestEvent!.current!),
      total: formatNumber(store.latestEvent!.total!),
    })}`
    : latestMessage;
  const messageOf = useCallback((error: unknown) => rawMessageOf(error) ?? t("error.generic"), [t]);
  const currentStageLabel = useCallback((stage: string | undefined, index: number) => {
    if (stage === "completed") return t("stage.completed");
    if (stage === "failed") return t("stage.failed");
    if (stage === "cancelled") return t("stage.cancelled");
    return t(stages[index]?.[1] ?? "stage.preparing");
  }, [t]);

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
    store.setProjects(overview.projects);
    return overview;
  };

  const reconcileRuntimeState = useCallback(async () => {
    const runtime = await getAppRuntimeStatus();
    const appState = useAppStore.getState();
    if (!runtime.pipelineRunning && !pipelineCommandPending.current && appState.phase === "running") {
      appState.setPhase("idle");
      clearCancellationFeedback();
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

  useEffect(() => {
    void Promise.all([checkEngines(), getProjectOverview()])
      .then(([engines, overview]) => {
        store.setEngines(engines);
        store.setProjectsRoot(overview.projectsRoot);
        store.setProjects(overview.projects);
        store.setColmapAcceleration(engines.find((engine) => engine.kind === "colmap")?.acceleration ?? null);
      })
      .catch((error) => store.setError(messageOf(error)));
  }, [store.setEngines, store.setProjects, store.setProjectsRoot, store.setColmapAcceleration, store.setError]);

  useEffect(() => {
    if (viewMode === "tasks") void reconcileRuntimeState().catch(() => undefined);
    const onFocus = () => { void reconcileRuntimeState().catch(() => undefined); };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [reconcileRuntimeState, viewMode]);

  useEffect(() => {
    void initializeTelemetry()
      .then(setTelemetryPreferences)
      .catch(() => undefined);
  }, []);

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

  const analyze = async (path: string, quality: Quality) => {
    store.setPhase("analyzing");
    store.setError(null);
    try {
      const result = await probeAndPlan(path, quality);
      store.setAnalysis(result.inputType, result.video, result.imageSequence, result.plan, result.estimate);
      store.setPhase("idle");
    } catch (error) {
      store.setError(messageOf(error));
      store.setPhase("failed");
    }
  };

  const chooseInput = async (inputType: InputType) => {
    try {
      const selected = inputType === "images" ? await selectImageSequence() : await selectVideo();
      if (selected) {
        store.setInputPath(selected, inputType);
        await analyze(selected, store.quality);
      }
    } catch (error) {
      store.setError(messageOf(error));
    }
  };

  const chooseInputType = (inputType: InputType) => {
    setInputMenuOpen(false);
    if (inputType !== store.inputType) store.setInputPath(null, inputType);
  };

  const chooseRoot = async () => {
    const selected = await selectProjectsRoot(store.projectsRoot);
    if (!selected) return;
    try {
      const settings = await setProjectsRoot(selected);
      store.setProjectsRoot(settings.projectsRoot);
      await refreshProjects();
    } catch (error) { store.setError(messageOf(error)); }
  };

  const chooseQuality = async (quality: Quality) => {
    store.setQuality(quality);
    if (store.inputPath) await analyze(store.inputPath, quality);
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
    if (!store.inputPath || !store.plan || !store.projectsRoot) return;
    if (
      store.inputType === "images"
      && store.imageSequence?.requiresLargeSequenceConfirmation
      && !(await confirmLargeImageSequence(store.imageSequence.imageCount))
    ) return;
    clearCancellationFeedback();
    runElapsedOffset.current = 0;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(0);
    setFailureDialog(null);
    pipelineCommandPending.current = true;
    store.beginRun();
    try {
      const result = await startPipeline(store.inputPath, store.quality, store.projectsRoot);
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
      const latestStage = useAppStore.getState().latestEvent?.stage;
      const cancelled = pipelineWasCancelled(error, latestStage);
      store.setPhase(cancelled ? "cancelled" : "failed");
      if (!cancelled) {
        const fallbackStage = [...useAppStore.getState().events].reverse().find((event) => !["failed", "cancelled"].includes(event.stage))?.stage;
        setFailureDialog(inferFailureDialog(error, fallbackStage));
      }
    } finally {
      try { await refreshProjects(); } catch { /* the generated project remains on disk */ }
      pipelineCommandPending.current = false;
    }
  };

  const resume = async (project: ProjectSummary) => {
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
      try { await refreshProjects(); } catch { /* the project remains on disk */ }
      pipelineCommandPending.current = false;
    }
  };

  const removeProject = async (project: ProjectSummary) => {
    if (deletingProjectId) return;
    setDeletingProjectId(project.id);
    try {
      await reconcileRuntimeState();
      if (await confirmAndDeleteProject(project)) {
        await refreshProjects();
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

  const previewCompletedResult = () => {
    const project = store.result && store.projects.find((item) => item.id === store.result?.projectId);
    if (project) void previewProject(project);
  };

  const exportCompletedResult = async () => {
    if (!store.result) return;
    try {
      await exportPly(store.result);
    } catch (error) {
      store.setError(messageOf(error));
    }
  };

  const revealCompletedResult = async () => {
    if (!store.result) return;
    const project = store.projects.find((item) => item.id === store.result?.projectId);
    if (project) await showProject(project);
    else store.setError(t("failure.logsUnavailable"));
  };

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
        <div className="pane-header"><h1>{t("task.create")}</h1><span className={isRunning ? "run-state active" : "run-state"}>{isRunning ? t("task.running") : t("task.idle")}</span></div>

        <div className="form-section">
          <label className="field-label">{t("input.label")}</label>
          <div className="input-picker">
            <div className="input-type-picker">
              <button className="input-picker-toggle" type="button" disabled={isRunning} aria-label={t("input.typeAria")} aria-expanded={inputMenuOpen} onClick={() => setInputMenuOpen((open) => !open)}>
                {store.inputType === "images" ? <Images size={16} /> : <Clapperboard size={16} />}
                <span>{store.inputType === "images" ? t("input.images") : t("input.video")}</span>
                <ChevronDown size={14} />
              </button>
              {inputMenuOpen && <div className="input-picker-menu" role="menu">
                <button type="button" role="menuitemradio" aria-checked={store.inputType === "video"} onClick={() => chooseInputType("video")}><Clapperboard size={15} /><span><strong>{t("input.video")}</strong><small>{t("input.videoTypes")}</small></span></button>
                <button type="button" role="menuitemradio" aria-checked={store.inputType === "images"} onClick={() => chooseInputType("images")}><Images size={15} /><span><strong>{t("input.images")}</strong><small>{t("input.imageTypes")}</small></span></button>
              </div>}
            </div>
            <button className="path-picker" type="button" disabled={isRunning} onClick={() => void chooseInput(store.inputType)}>
              {store.inputType === "images" ? <Images size={18} /> : <Clapperboard size={18} />}
              <span>
                <strong>{store.inputPath ? basename(store.inputPath) : store.inputType === "images" ? t("input.selectImages") : t("input.selectVideo")}</strong>
                <small>{store.inputPath ?? (store.inputType === "images" ? t("input.selectImagesHint") : t("input.selectVideoHint"))}</small>
              </span>
            </button>
          </div>
        </div>

        <div className="form-section">
          <label className="field-label">{t("project.root")}</label>
          <button className="path-picker compact" type="button" disabled={isRunning} onClick={() => void chooseRoot()}>
            <FolderOpen size={18} /><span><strong>{store.projectsRoot ? basename(store.projectsRoot) : t("project.readingRoot")}</strong><small>{store.projectsRoot || "Documents / SplatStudio / Projects"}</small></span><ChevronRight size={16} />
          </button>
          <p className="field-note">{t("project.rootHint")}</p>
        </div>

        <div className="form-section">
          <label className="field-label">{t("quality.label")}</label>
          <div className="quality-list" role="radiogroup">
            {qualities.map((quality) => <button key={quality.value} type="button" role="radio" disabled={isRunning} aria-checked={store.quality === quality.value} className={store.quality === quality.value ? "quality-option selected" : "quality-option"} onClick={() => void chooseQuality(quality.value)}>
              <span className="radio-mark"><span /></span><span><strong>{t(quality.label)}</strong><small>{t(quality.description)}</small></span>
            </button>)}
          </div>
        </div>

        <div className={`acceleration-status ${store.colmapAcceleration?.backend === "gpu" ? "gpu" : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly", "colmapGpuDisabled"].includes(store.colmapAcceleration.reasonCode) ? "warning" : "cpu"}`} aria-live="polite">
          <span className="acceleration-icon">{store.colmapAcceleration?.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly", "colmapGpuDisabled"].includes(store.colmapAcceleration.reasonCode) ? <CircleAlert size={17} /> : store.colmapAcceleration ? <Cpu size={17} /> : <LoaderCircle className="spin" size={17} />}</span>
          <span>
            <strong>{store.colmapAcceleration == null ? t("gpu.detecting") : store.colmapAcceleration.reasonCode === "directmlReady" ? t("gpu.directmlEnabled") : store.colmapAcceleration.reasonCode === "zludaReady" ? t("gpu.zludaEnabled") : store.colmapAcceleration.backend === "gpu" ? t("gpu.enabled") : t("gpu.cpu")}</strong>
            <small>{store.colmapAcceleration == null ? t("gpu.reading") : store.colmapAcceleration.reasonCode === "directmlReady" ? `${store.colmapAcceleration.device?.name ?? t("gpu.directmlEnabled")} · ${t("gpu.directmlHint")} · ${localizePipelineMessage(locale, store.colmapAcceleration.reason)}` : store.colmapAcceleration.reasonCode === "zludaReady" ? `${store.colmapAcceleration.device?.name ?? t("gpu.zludaEnabled")} · ${t("gpu.zludaHint")} · ${localizePipelineMessage(locale, store.colmapAcceleration.reason)}` : store.colmapAcceleration.backend === "gpu" && store.colmapAcceleration.device ? `${store.colmapAcceleration.device.name}${store.colmapAcceleration.device.totalMemoryMb ? ` · ${t("gpu.memory", { value: (store.colmapAcceleration.device.totalMemoryMb / 1024).toFixed(1) })}` : ""} · ${t("gpu.driver", { value: store.colmapAcceleration.device.driverVersion })} · Compute Capability ${store.colmapAcceleration.device.computeCapability}` : store.colmapAcceleration.reasonCode === "macOsCpuOnly" ? localizePipelineMessage(locale, store.colmapAcceleration.reason) : `${localizePipelineMessage(locale, store.colmapAcceleration.reason)} · ${t("gpu.requirements", { driver: store.colmapAcceleration.requirements.minimumDriverVersion, capability: store.colmapAcceleration.requirements.minimumComputeCapability })}`}</small>
          </span>
        </div>

        {(store.video || store.imageSequence) && store.plan && <div className="source-metrics">
          <span><small>{store.inputType === "images" ? t("metrics.imageCount") : t("metrics.duration")}</small><b>{store.imageSequence ? t("common.images", { count: formatNumber(store.imageSequence.imageCount) }) : formatVideoDuration(store.video!.duration)}</b></span>
          <span><small>{t("metrics.resolution")}</small><b>{store.imageSequence?.width ?? store.video?.width} × {store.imageSequence?.height ?? store.video?.height}</b></span>
          <span><small>{store.inputType === "images" ? t("metrics.processingImages") : t("metrics.estimatedFrames")}</small><b>{store.inputType === "images" ? t("metrics.keepAll") : t("metrics.approx", { value: formatNumber(store.plan.estimatedFrames) })}</b></span>
          <span title={store.estimate ? localizePipelineMessage(locale, store.estimate.basis) : undefined}><small>{t("metrics.estimate")}</small><b>{store.estimate ? t("metrics.approx", { value: formatDuration(store.estimate.estimatedMs) }) : t("metrics.analyzing")}</b>{store.estimate && <em>{formatDuration(store.estimate.lowerBoundMs)}–{formatDuration(store.estimate.upperBoundMs)}</em>}</span>
        </div>}

        {(store.video?.hasAlpha || store.imageSequence?.hasAlpha) && <div className="alpha-source-status" role="status">
          <Blend size={17} />
          <span><strong>{store.inputType === "images" ? t("alpha.imagesTitle") : t("alpha.videoTitle")}</strong><small>{store.inputType === "images" ? t("alpha.imagesHint") : t("alpha.videoHint", { format: store.video?.pixelFormat || "Alpha" })}</small></span>
        </div>}

        {store.imageSequence?.requiresLargeSequenceConfirmation && <div className="sequence-warning" role="status"><CircleAlert size={16} /><span><strong>{t("sequence.title")}</strong><small>{t("sequence.hint")}</small></span></div>}

        {!isRunning && <button className="primary-action" type="button" disabled={!store.inputPath || !store.plan || !store.projectsRoot || store.phase === "analyzing" || missingEngines.length > 0} onClick={() => void generate()}>
          {store.phase === "analyzing" ? <LoaderCircle className="spin" size={17} /> : <Play size={16} fill="currentColor" />}
          {store.phase === "analyzing" ? t("generate.analyzing") : t("generate.start")}
        </button>}

        {(isRunning || store.events.length > 0) && <section className="live-process">
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

        {store.phase === "completed" && store.result && <section className="completion-result" aria-labelledby="completion-result-title">
          <div className="completion-result-heading"><div><span className="result-status-dot" /><strong id="completion-result-title">{t("result.title")}</strong></div><span>{t("result.completed")}</span></div>
          <dl className="completion-result-stats">
            <div><dt>{t("result.splats")}</dt><dd>{formatNumber(store.result.splatCount)}</dd></div>
            <div><dt>{t("result.fileSize")}</dt><dd>{formatBytes(store.result.fileSize, locale)}</dd></div>
            <div><dt>{t("result.registered")}</dt><dd>{formatNumber(store.result.registeredImages)} / {formatNumber(store.result.inputImages)}</dd></div>
            <div><dt>{t("result.points")}</dt><dd>{formatNumber(store.result.points3d)}</dd></div>
            <div><dt>{t("result.elapsed")}</dt><dd>{formatDuration(store.result.durationMs)}</dd></div>
          </dl>
          {store.result.registeredRatio < 0.8 && <p className="completion-warning" role="status"><CircleAlert size={15} />{t("result.lowRegistration", { value: (store.result.registeredRatio * 100).toFixed(1) })}</p>}
          <div className="completion-result-actions">
            <button type="button" disabled={!store.projects.some((project) => project.id === store.result?.projectId && project.status === "completed")} onClick={previewCompletedResult}><Eye size={14} />{t("project.preview")}</button>
            <button type="button" onClick={() => void exportCompletedResult()}><Download size={14} />{t("result.export")}</button>
            <button type="button" onClick={() => void revealCompletedResult()}><FolderOpen size={14} />{t("result.reveal")}</button>
          </div>
        </section>}

        {store.error && <div className="inline-error"><CircleAlert size={16} /><span>{localizePipelineMessage(locale, store.error)}</span><button type="button" onClick={() => store.setError(null)}>{t("common.close")}</button></div>}
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
        <div className="pane-header"><h2>{t("history.title")}</h2><button className="refresh-action" type="button" disabled={isRunning} onClick={() => void refreshProjects()}><RotateCcw size={14} />{t("history.refresh")}</button></div>
        <div className="archive-summary"><span><b>{completed.length}</b><small>{t("history.completed")}</small></span><span><b>{unfinished.length}</b><small>{t("history.unfinished")}</small></span></div>

        {completed.length === 0 && unfinished.length === 0 && <div className="empty-state"><FileBox size={30} strokeWidth={1.4} /><strong>{t("history.emptyTitle")}</strong><p>{t("history.emptyHint")}</p></div>}

        {completed.length > 0 && <div className="project-group"><div className="group-heading"><span>{t("history.completed")}</span><small>{t("history.projects", { count: completed.length })}</small></div>{completed.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} previewing={openingPreviewProjectId === project.id} previewDisabled={openingPreviewProjectId !== null || closingPreviewProjectId === project.id} deleting={deletingProjectId === project.id} revealing={revealingProjectId === project.id} onPreview={(item) => void previewProject(item)} onResume={() => undefined} onReveal={(item) => void showProject(item)} onDelete={(item) => void removeProject(item)} />)}</div>}
        {unfinished.length > 0 && <div className="project-group unfinished"><div className="group-heading"><span>{t("history.unfinished")}</span><small>{t("history.projects", { count: unfinished.length })}</small></div>{unfinished.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} previewing={false} previewDisabled deleting={deletingProjectId === project.id} revealing={revealingProjectId === project.id} onPreview={() => undefined} onResume={(item) => void resume(item)} onReveal={(item) => void showProject(item)} onDelete={(item) => void removeProject(item)} />)}</div>}
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
