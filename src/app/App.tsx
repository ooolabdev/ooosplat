import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  Blend, ChevronDown, ChevronRight, CircleAlert, Clapperboard, Cpu, FileBox, Images,
  Eye, FolderOpen, LoaderCircle, MapPin, Minus, Play, Plus, RotateCcw, Square, Trash2,
  Languages, Settings2, Zap,
} from "lucide-react";
import appLogo from "../../assets/app-icon.svg";
import packageMetadata from "../../package.json";
import { TelemetryPreferences } from "../components/TelemetryPreferences";
import {
  cancelPipeline, checkEngines, confirmAndDeleteProject, confirmLargeImageSequence,
  estimateProjectRuntime, getProjectOverview, onPipelineEvent, probeAndPlan, revealProject,
  selectImageSequence, selectProjectsRoot, selectVideo,
  setProjectsRoot, startPipeline, prepareGaussianPreview, releaseGaussianPreview,
  initializeTelemetry, setTelemetryConsent, resumePipeline,
} from "../lib/backend";
import { startElapsedTicker } from "../lib/elapsedTimer";
import { localizePipelineMessage, useI18n, type TranslationKey } from "../i18n";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import type { EngineStatus, InputType, ProjectStatus, ProjectSummary, Quality } from "../types/pipeline";
import type { TelemetryPreferences as TelemetryPreferencesState } from "../types/telemetry";

const GaussianViewer = lazy(() => import("../components/GaussianViewer").then((module) => ({ default: module.GaussianViewer })));
const CANCELLATION_OVERLAY_DELAY_MS = 300;

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

const rawMessageOf = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : null;
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
  if (["completed", "failed", "cancelled"].includes(stage)) return 6;
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

function ProjectRow({ project, busy, previewing, previewDisabled, onPreview, onResume, onDelete }: { project: ProjectSummary; busy: boolean; previewing: boolean; previewDisabled: boolean; onPreview: (project: ProjectSummary) => void; onResume: (project: ProjectSummary) => void; onDelete: (project: ProjectSummary) => void }) {
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
      <button type="button" onClick={() => void revealProject(project)}><MapPin size={14} />{t("project.reveal")}</button>
      <button className="danger-link" type="button" disabled={busy} onClick={() => onDelete(project)}><Trash2 size={14} />{t("project.delete")}</button>
    </div>
  </article>;
}

export function App() {
  const { locale, t, toggleLocale, formatNumber, formatDuration } = useI18n();
  const store = useAppStore();
  const loadGaussian = useGaussianTransformStore((state) => state.load);
  const closeGaussian = useGaussianTransformStore((state) => state.close);
  const isRunning = store.phase === "running";
  const logEnd = useRef<HTMLDivElement>(null);
  const workspaceRef = useRef<HTMLElement>(null);
  const controlPaneRef = useRef<HTMLElement>(null);
  const projectsPaneRef = useRef<HTMLElement>(null);
  const taskScrollPositions = useRef({ control: 0, projects: 0 });
  const previewReleasePromises = useRef(new Map<string, Promise<void>>());
  const releasedPreviewProjects = useRef(new Set<string>());
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
  const [disposedPreviewProjectId, setDisposedPreviewProjectId] = useState<string | null>(null);
  const [showZoomControls, setShowZoomControls] = useState(false);
  const [telemetryPreferences, setTelemetryPreferences] = useState<TelemetryPreferencesState | null>(null);
  const [privacySettingsOpen, setPrivacySettingsOpen] = useState(false);
  const [telemetryBusy, setTelemetryBusy] = useState(false);
  const [inputMenuOpen, setInputMenuOpen] = useState(false);
  const missingEngines = store.engines.filter((engine) => !engineReady(engine));
  const completed = useMemo(() => store.projects.filter((project) => project.status === "completed"), [store.projects]);
  const unfinished = useMemo(() => store.projects.filter((project) => project.status !== "completed"), [store.projects]);
  const activeStageIndex = stagePosition(store.latestEvent?.stage);
  const liveProgressLabel = store.latestEvent?.unit === "estimated_progress" && store.latestEvent.stageProgress != null
    ? t("progress.estimated", { value: store.latestEvent.stageProgress.toFixed(0) })
    : store.latestEvent?.current != null
      ? `${formatNumber(store.latestEvent.current)}${store.latestEvent.total ? ` / ${formatNumber(store.latestEvent.total)}` : ""}`
      : t("progress.continuing");
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
  };

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

  // The log keeps only the most recent 500 events, so its length stops changing once a run
  // passes that many while new lines keep arriving. Depend on the array itself, which
  // receiveEvent replaces on every event, or auto-scroll stops on exactly the long runs
  // that need it.
  useEffect(() => { logEnd.current?.scrollIntoView({ block: "nearest" }); }, [store.events]);

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
    const selected = inputType === "images" ? await selectImageSequence() : await selectVideo();
    if (selected) {
      store.setInputPath(selected, inputType);
      await analyze(selected, store.quality);
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
      store.setPhase(message.includes("取消") || message.toLowerCase().includes("cancel") ? "cancelled" : "failed");
    } finally {
      try { await refreshProjects(); } catch { /* the generated project remains on disk */ }
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
      store.setPhase(message.includes("取消") || message.toLowerCase().includes("cancel") ? "cancelled" : "failed");
    } finally {
      try { await refreshProjects(); } catch { /* the project remains on disk */ }
    }
  };

  const removeProject = async (project: ProjectSummary) => {
    try {
      if (await confirmAndDeleteProject(project)) {
        await refreshProjects();
      }
    } catch (error) { store.setError(messageOf(error)); }
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

  const previewProject = async (project: ProjectSummary) => {
    if (project.status !== "completed" || openingPreviewProjectId || closingPreviewProjectId) return;
    const previous = useGaussianTransformStore.getState().descriptor?.projectId;
    setOpeningPreviewProjectId(project.id);
    setDisposedPreviewProjectId(null);
    store.setError(null);
    try {
      closeGaussian();
      if (previous && previous !== project.id) await releasePreviewSession(previous);
      const descriptor = await prepareGaussianPreview(project.id);
      releasedPreviewProjects.current.delete(project.id);
      loadGaussian(descriptor);
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

  const exitPreview = async () => {
    const projectId = useGaussianTransformStore.getState().descriptor?.projectId;
    if (closingPreviewProjectId) return;
    if (projectId) setClosingPreviewProjectId(projectId);
    setViewMode("tasks");
  };

  const previewRendererDisposed = useCallback((projectId: string) => {
    setDisposedPreviewProjectId(projectId);
  }, []);

  useEffect(() => {
    if (viewMode !== "tasks" || !closingPreviewProjectId || disposedPreviewProjectId !== closingPreviewProjectId) return;
    const projectId = closingPreviewProjectId;
    void releasePreviewSession(projectId)
      .catch((error) => store.setError(messageOf(error)))
      .finally(() => {
        if (useGaussianTransformStore.getState().descriptor?.projectId === projectId) closeGaussian();
        setDisposedPreviewProjectId((current) => current === projectId ? null : current);
        setClosingPreviewProjectId((current) => current === projectId ? null : current);
      });
  }, [viewMode, closingPreviewProjectId, disposedPreviewProjectId, closeGaussian, releasePreviewSession, store.setError]);

  useEffect(() => () => {
    const projectId = useGaussianTransformStore.getState().descriptor?.projectId;
    if (projectId) {
      queueMicrotask(() => void releasePreviewSession(projectId).catch(() => undefined));
    }
  }, [releasePreviewSession]);

  if (viewMode === "preview") {
    return <main className="app-shell preview-mode">
      <Suspense fallback={<section className="preview-pane active preview-workspace"><div className="preview-empty"><LoaderCircle className="spin" size={24} /><strong>{t("preview.preparingModule")}</strong></div></section>}>
        <GaussianViewer onExit={exitPreview} onDisposed={previewRendererDisposed} pipelineRunning={isRunning} />
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

        <div className={`acceleration-status ${store.colmapAcceleration?.backend === "gpu" ? "gpu" : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly"].includes(store.colmapAcceleration.reasonCode) ? "warning" : "cpu"}`} aria-live="polite">
          <span className="acceleration-icon">{store.colmapAcceleration?.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly"].includes(store.colmapAcceleration.reasonCode) ? <CircleAlert size={17} /> : store.colmapAcceleration ? <Cpu size={17} /> : <LoaderCircle className="spin" size={17} />}</span>
          <span>
            <strong>{store.colmapAcceleration == null ? t("gpu.detecting") : store.colmapAcceleration.backend === "gpu" ? t("gpu.enabled") : t("gpu.cpu")}</strong>
            <small>{store.colmapAcceleration == null ? t("gpu.reading") : store.colmapAcceleration.backend === "gpu" && store.colmapAcceleration.device ? `${store.colmapAcceleration.device.name}${store.colmapAcceleration.device.totalMemoryMb ? ` · ${t("gpu.memory", { value: (store.colmapAcceleration.device.totalMemoryMb / 1024).toFixed(1) })}` : ""} · ${t("gpu.driver", { value: store.colmapAcceleration.device.driverVersion })} · Compute Capability ${store.colmapAcceleration.device.computeCapability}` : store.colmapAcceleration.reasonCode === "macOsCpuOnly" ? localizePipelineMessage(locale, store.colmapAcceleration.reason) : `${localizePipelineMessage(locale, store.colmapAcceleration.reason)} · ${t("gpu.requirements", { driver: store.colmapAcceleration.requirements.minimumDriverVersion, capability: store.colmapAcceleration.requirements.minimumComputeCapability })}`}</small>
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
          <p className="current-message">{store.latestEvent ? localizePipelineMessage(locale, store.latestEvent.message) : store.progressMessage ? localizePipelineMessage(locale, store.progressMessage) : t("progress.preparing")}</p>
          <div className="process-metrics">
            <span><small>{t("progress.stage")}</small><b>{currentStageLabel(store.latestEvent?.stage, activeStageIndex)}</b></span>
            <span><small>{t("progress.progress")}</small><b>{liveProgressLabel}</b></span>
            <span><small>{t("progress.elapsed")}</small><b>{formatDuration(liveElapsedMs)}</b></span>
          </div>
          <ol className="stage-timeline">
            {stages.map(([key, label], index) => <li key={key} className={index < activeStageIndex || store.phase === "completed" ? "done" : index === activeStageIndex && isRunning ? "active" : ""}><span /><b>{t(label)}</b>{index === activeStageIndex && isRunning && <small>{store.latestEvent?.indeterminate ? t("progress.running") : `${(store.latestEvent?.stageProgress ?? 0).toFixed(0)}%`}</small>}</li>)}
          </ol>
          <div className="log-toolbar"><span>{t("progress.log")}</span><small>{t("progress.logCount", { count: store.events.length })}</small></div>
          <div className="live-log" aria-live="polite">
            {store.events.map((event, index) => <div className={`log-line ${event.level}`} key={`${event.sequence}-${index}`}><time>{new Date(event.timestamp).toLocaleTimeString(locale, { hour12: false })}</time><span>{event.engine ?? "system"}</span><p>{event.kind === "log" ? event.message : localizePipelineMessage(locale, event.message)}</p></div>)}
            <div ref={logEnd} />
          </div>
          {isRunning && <button className="cancel-action" type="button" disabled={isCancellationRequested} onClick={() => void requestCancellation()}>{isCancellationRequested ? <LoaderCircle className="spin" size={13} /> : <Square size={12} fill="currentColor" />}{isCancellationRequested ? t("progress.terminating") : t("progress.cancel")}</button>}
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

        {completed.length > 0 && <div className="project-group"><div className="group-heading"><span>{t("history.completed")}</span><small>{t("history.projects", { count: completed.length })}</small></div>{completed.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} previewing={openingPreviewProjectId === project.id} previewDisabled={openingPreviewProjectId !== null || closingPreviewProjectId !== null} onPreview={(item) => void previewProject(item)} onResume={() => undefined} onDelete={(item) => void removeProject(item)} />)}</div>}
        {unfinished.length > 0 && <div className="project-group unfinished"><div className="group-heading"><span>{t("history.unfinished")}</span><small>{t("history.projects", { count: unfinished.length })}</small></div>{unfinished.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} previewing={false} previewDisabled onPreview={() => undefined} onResume={(item) => void resume(item)} onDelete={(item) => void removeProject(item)} />)}</div>}
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
    {telemetryPreferences && !telemetryPreferences.consentDecided && <TelemetryPreferences mode="consent" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} />}
    {telemetryPreferences && privacySettingsOpen && <TelemetryPreferences mode="settings" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} onClose={() => setPrivacySettingsOpen(false)} />}
  </main>;
}
