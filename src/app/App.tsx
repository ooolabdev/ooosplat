import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  ChevronRight, CircleAlert, Clapperboard, Cpu, FileBox,
  Eye, FolderOpen, LoaderCircle, MapPin, Minus, Play, Plus, RotateCcw, Square, Trash2,
  Settings2, Zap,
} from "lucide-react";
import appLogo from "../../assets/app-icon.svg";
import { TelemetryPreferences } from "../components/TelemetryPreferences";
import {
  cancelPipeline, checkEngines, confirmAndDeleteProject, getProjectOverview,
  onPipelineEvent, probeAndPlan, revealProject, selectImageFolder, selectProjectsRoot, selectVideo,
  setProjectsRoot, startPipeline, prepareGaussianPreview, releaseGaussianPreview,
  initializeTelemetry, setTelemetryConsent,
} from "../lib/backend";
import { startElapsedTicker } from "../lib/elapsedTimer";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import type { EngineStatus, ProjectStatus, ProjectSummary, Quality } from "../types/pipeline";
import type { TelemetryPreferences as TelemetryPreferencesState } from "../types/telemetry";

const GaussianViewer = lazy(() => import("../components/GaussianViewer").then((module) => ({ default: module.GaussianViewer })));

const qualities: Array<{ value: Quality; label: string; description: string }> = [
  { value: "fast", label: "快速", description: "快速验证素材与拍摄路径" },
  { value: "balanced", label: "均衡", description: "质量与处理时间的推荐平衡" },
  { value: "high", label: "精细", description: "更充分地利用视频画面细节" },
];

const stages = [
  ["probingVideo", "视频分析"], ["extractingFrames", "画面提取"],
  ["extractingFeatures", "特征提取"], ["matching", "顺序匹配"],
  ["reconstructing", "相机重建"], ["trainingSplats", "Splat 训练"],
  ["exporting", "结果发布"],
] as const;

const messageOf = (error: unknown) => typeof error === "string" ? error : error instanceof Error ? error.message : "处理失败，请查看项目日志。";
const basename = (path: string) => path.split(/[\\/]/).at(-1) ?? path;
const formatBytes = (bytes: number | null) => bytes == null ? "—" : bytes >= 1024 ** 3 ? `${(bytes / 1024 ** 3).toFixed(2)} GB` : bytes >= 1024 ** 2 ? `${(bytes / 1024 ** 2).toFixed(1)} MB` : `${(bytes / 1024).toFixed(1)} KB`;
const formatDuration = (milliseconds: number | null) => {
  if (milliseconds == null) return "—";
  const seconds = Math.floor(milliseconds / 1000);
  if (seconds < 60) return `${seconds} 秒`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
  return `${Math.floor(seconds / 3600)} 小时 ${Math.floor((seconds % 3600) / 60)} 分`;
};
const formatVideoDuration = (seconds: number) => `${Math.floor(seconds / 60)}:${Math.round(seconds % 60).toString().padStart(2, "0")}`;
const formatDate = (value: string | null) => value ? new Intl.DateTimeFormat("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }).format(new Date(value)) : "—";
const qualityLabel = (quality: Quality) => qualities.find((item) => item.value === quality)?.label ?? quality;
const statusLabel: Record<ProjectStatus, string> = { running: "处理中", completed: "已完成", failed: "失败", cancelled: "已取消", interrupted: "已中断" };
const stagePosition = (stage?: string) => {
  if (!stage || ["created", "probingVideo", "planningFrames"].includes(stage)) return 0;
  if (stage === "validatingReconstruction") return 4;
  if (["completed", "failed", "cancelled"].includes(stage)) return 6;
  const index = stages.findIndex(([key]) => key === stage);
  return index < 0 ? 0 : index;
};

const currentStageLabel = (stage: string | undefined, activeStageIndex: number) => {
  if (stage === "completed") return "已完成";
  if (stage === "failed") return "任务失败";
  if (stage === "cancelled") return "已取消";
  return stages[activeStageIndex]?.[1] ?? "准备";
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

function ProjectRow({ project, busy, previewing, previewDisabled, onPreview, onDelete }: { project: ProjectSummary; busy: boolean; previewing: boolean; previewDisabled: boolean; onPreview: (project: ProjectSummary) => void; onDelete: (project: ProjectSummary) => void }) {
  return <article className="project-row">
    <div className="project-row-main">
      <div className="project-title-line">
        <span className={`project-status ${project.status}`} />
        <strong>{project.name}</strong>
        <span className="status-copy">{statusLabel[project.status]}</span>
      </div>
      <p className="project-path" title={project.projectPath}>{project.projectPath}</p>
      {project.failureMessage && <p className="project-failure">{project.failureMessage}</p>}
    </div>
    <dl className="project-stats">
      <div><dt>PLY</dt><dd>{formatBytes(project.fileSize)}</dd></div>
      <div><dt>生成日期</dt><dd>{formatDate(project.completedAt ?? project.createdAt)}</dd></div>
      <div><dt>耗时</dt><dd>{formatDuration(project.durationMs)}</dd></div>
      <div><dt>档位</dt><dd>{qualityLabel(project.quality)}</dd></div>
    </dl>
    <div className="project-actions">
      {project.status === "completed" && <button className="preview-link" type="button" disabled={previewDisabled} onClick={() => onPreview(project)}>{previewing ? <LoaderCircle className="spin" size={14} /> : <Eye size={14} />}{previewing ? "正在打开" : "预览"}</button>}
      <button type="button" onClick={() => void revealProject(project)}><MapPin size={14} />在文件管理器中显示</button>
      <button className="danger-link" type="button" disabled={busy} onClick={() => onDelete(project)}><Trash2 size={14} />删除</button>
    </div>
  </article>;
}

export function App() {
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
  const [liveElapsedMs, setLiveElapsedMs] = useState(0);
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
  const missingEngines = store.engines.filter((engine) => !engineReady(engine));
  const completed = useMemo(() => store.projects.filter((project) => project.status === "completed"), [store.projects]);
  const unfinished = useMemo(() => store.projects.filter((project) => project.status !== "completed"), [store.projects]);
  const activeStageIndex = stagePosition(store.latestEvent?.stage);

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
        setLiveElapsedMs(event.elapsedMs);
      }
    }).then((fn) => { unlisten = fn; });
    return () => unlisten?.();
  }, [store.receiveEvent]);

  useEffect(() => { logEnd.current?.scrollIntoView({ block: "nearest" }); }, [store.events.length]);

  useEffect(() => {
    if (!isRunning || runStartedAt.current == null) return;
    return startElapsedTicker(runStartedAt.current, setLiveElapsedMs);
  }, [isRunning]);

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
      store.setError(`无法保存隐私设置：${messageOf(error)}`);
    } finally {
      setTelemetryBusy(false);
    }
  };

  const analyze = async (path: string, quality: Quality) => {
    store.setPhase("analyzing");
    store.setError(null);
    try {
      const result = await probeAndPlan(path, quality);
      store.setAnalysis(result.video, result.plan);
      store.setPhase("idle");
    } catch (error) {
      store.setError(messageOf(error));
      store.setPhase("failed");
    }
  };

  const chooseVideo = async () => {
    const selected = await selectVideo();
    if (selected) { store.setVideoPath(selected); await analyze(selected, store.quality); }
  };

  const chooseImageFolder = async () => {
    const selected = await selectImageFolder();
    if (selected) { store.setVideoPath(selected); await analyze(selected, store.quality); }
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
    if (store.videoPath) await analyze(store.videoPath, quality);
  };

  const generate = async () => {
    if (!store.videoPath || !store.plan || !store.projectsRoot) return;
    runStartedAt.current = Date.now();
    setLiveElapsedMs(0);
    store.beginRun();
    try {
      const result = await startPipeline(store.videoPath, store.quality, store.projectsRoot);
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
      store.setPhase(message.includes("取消") ? "cancelled" : "failed");
    } finally {
      try { await refreshProjects(); } catch { /* the generated project remains on disk */ }
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
      <Suspense fallback={<section className="preview-pane active preview-workspace"><div className="preview-empty"><LoaderCircle className="spin" size={24} /><strong>正在准备预览模块</strong></div></section>}>
        <GaussianViewer onExit={exitPreview} onDisposed={previewRendererDisposed} pipelineRunning={isRunning} />
      </Suspense>
    </main>;
  }

  return <main className={isResizing ? "app-shell resizing" : "app-shell"}>
    <div className="interface-frame" style={{ "--ui-scale": uiScale / 100, "--ui-size": `${10000 / uiScale}%` } as CSSProperties}>
    <header className="topbar">
      <div className="brand-lockup"><span className="brand-mark"><img src={appLogo} alt="" aria-hidden="true" /></span><span className="brand-name">OOO<span>Splat</span></span><span className="version-tag">LOCAL / 0.3.0</span></div>
      <div className="topbar-actions">
        {telemetryPreferences && <button className="settings-action" type="button" onClick={() => setPrivacySettingsOpen(true)}><Settings2 size={15} />设置</button>}
        <div className="engine-summary"><span className={missingEngines.length ? "status-light warning" : "status-light"} />{store.engines.length === 0 ? "正在检查内置引擎" : missingEngines.length ? `${missingEngines.length} 个引擎异常` : "FFmpeg · COLMAP · Brush 就绪"}</div>
      </div>
    </header>

    <section className="workspace" ref={workspaceRef} style={{ "--left-pane-width": `${leftPanePercent}%` } as CSSProperties}>
      <section className="control-pane" ref={controlPaneRef} aria-label="生成控制台">
        <div className="pane-header"><h1>01 创建新任务</h1><span className={isRunning ? "run-state active" : "run-state"}>{isRunning ? "运行中" : "待命"}</span></div>

        <div className="form-section">
          <label className="field-label">输入素材</label>
          <button className="path-picker" type="button" disabled={isRunning} onClick={() => void chooseVideo()}>
            <Clapperboard size={18} /><span><strong>{store.video ? basename(store.videoPath ?? "") : "选择视频"}</strong><small>MP4 或 MOV 环绕视频</small></span><FolderOpen size={16} />
          </button>
          <button className="path-picker" type="button" disabled={isRunning} onClick={() => void chooseImageFolder()}>
            <FileBox size={18} /><span><strong>{store.videoPath && !store.video ? basename(store.videoPath) : "选择图片文件夹"}</strong><small>有序图片序列作为重建输入</small></span><FolderOpen size={16} />
          </button>
        </div>

        <div className="form-section">
          <label className="field-label">项目根目录</label>
          <button className="path-picker compact" type="button" disabled={isRunning} onClick={() => void chooseRoot()}>
            <FolderOpen size={18} /><span><strong>{store.projectsRoot ? basename(store.projectsRoot) : "正在读取默认目录"}</strong><small>{store.projectsRoot || "Documents / SplatStudio / Projects"}</small></span><ChevronRight size={16} />
          </button>
          <p className="field-note">每次生成会在此处创建独立项目文件夹，final.ply 直接保存在项目根部。</p>
        </div>

        <div className="form-section">
          <label className="field-label">生成质量</label>
          <div className="quality-list" role="radiogroup">
            {qualities.map((quality) => <button key={quality.value} type="button" role="radio" disabled={isRunning} aria-checked={store.quality === quality.value} className={store.quality === quality.value ? "quality-option selected" : "quality-option"} onClick={() => void chooseQuality(quality.value)}>
              <span className="radio-mark"><span /></span><span><strong>{quality.label}</strong><small>{quality.description}</small></span>
            </button>)}
          </div>
        </div>

        <div className={`acceleration-status ${store.colmapAcceleration?.backend === "gpu" ? "gpu" : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly"].includes(store.colmapAcceleration.reasonCode) ? "warning" : "cpu"}`} aria-live="polite">
          <span className="acceleration-icon">{store.colmapAcceleration?.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "macOsCpuOnly"].includes(store.colmapAcceleration.reasonCode) ? <CircleAlert size={17} /> : store.colmapAcceleration ? <Cpu size={17} /> : <LoaderCircle className="spin" size={17} />}</span>
          <span>
            <strong>{store.colmapAcceleration == null ? "正在检测 COLMAP GPU 加速…" : store.colmapAcceleration.backend === "gpu" ? "COLMAP GPU 加速已开启" : "COLMAP 使用 CPU"}</strong>
            <small>{store.colmapAcceleration == null ? "正在读取 COLMAP 加速能力" : store.colmapAcceleration.backend === "gpu" && store.colmapAcceleration.device ? `${store.colmapAcceleration.device.name} · 驱动 ${store.colmapAcceleration.device.driverVersion} · Compute Capability ${store.colmapAcceleration.device.computeCapability}` : store.colmapAcceleration.reasonCode === "macOsCpuOnly" ? store.colmapAcceleration.reason : `${store.colmapAcceleration.reason} · 最低要求：驱动 ${store.colmapAcceleration.requirements.minimumDriverVersion}，Compute Capability ${store.colmapAcceleration.requirements.minimumComputeCapability}`}</small>
          </span>
        </div>

        {store.plan && (store.video ? <div className="source-metrics">
          <span><small>时长</small><b>{formatVideoDuration(store.video.duration)}</b></span>
          <span><small>分辨率</small><b>{store.video.width} × {store.video.height}</b></span>
          <span><small>预计帧数</small><b>约 {store.plan.estimatedFrames.toLocaleString()}</b></span>
        </div> : <div className="source-metrics">
          <span><small>图片数量</small><b>{store.plan.estimatedFrames.toLocaleString()} 张</b></span>
          <span><small>输入类型</small><b>图片序列</b></span>
          <span><small>预计帧数</small><b>全部保留</b></span>
        </div>)}

        {!isRunning && <button className="primary-action" type="button" disabled={!store.videoPath || !store.plan || !store.projectsRoot || store.phase === "analyzing" || missingEngines.length > 0} onClick={() => void generate()}>
          {store.phase === "analyzing" ? <LoaderCircle className="spin" size={17} /> : <Play size={16} fill="currentColor" />}
          {store.phase === "analyzing" ? "正在分析视频" : "开始生成"}
        </button>}

        {(isRunning || store.events.length > 0) && <section className="live-process">
          <div className="live-heading"><div><span className="live-dot" /><strong>实时进程</strong></div><span className="mono">{store.progress.toFixed(1)}%</span></div>
          <p className="current-message">{store.progressMessage || "正在准备任务"}</p>
          <div className="process-metrics">
            <span><small>当前阶段</small><b>{currentStageLabel(store.latestEvent?.stage, activeStageIndex)}</b></span>
            <span><small>进度</small><b>{store.latestEvent?.current != null ? `${store.latestEvent.current.toLocaleString()}${store.latestEvent.total ? ` / ${store.latestEvent.total.toLocaleString()}` : ""}` : "持续运行"}</b></span>
            <span><small>总耗时</small><b>{formatDuration(liveElapsedMs)}</b></span>
          </div>
          <ol className="stage-timeline">
            {stages.map(([key, label], index) => <li key={key} className={index < activeStageIndex || store.phase === "completed" ? "done" : index === activeStageIndex && isRunning ? "active" : ""}><span /><b>{label}</b>{index === activeStageIndex && isRunning && <small>{store.latestEvent?.indeterminate ? "运行中" : `${(store.latestEvent?.stageProgress ?? 0).toFixed(0)}%`}</small>}</li>)}
          </ol>
          <div className="log-toolbar"><span>任务日志</span><small>最近 {store.events.length} / 500 条</small></div>
          <div className="live-log" aria-live="polite">
            {store.events.map((event, index) => <div className={`log-line ${event.level}`} key={`${event.sequence}-${index}`}><time>{new Date(event.timestamp).toLocaleTimeString("zh-CN", { hour12: false })}</time><span>{event.engine ?? "system"}</span><p>{event.message}</p></div>)}
            <div ref={logEnd} />
          </div>
          {isRunning && <button className="cancel-action" type="button" onClick={() => void cancelPipeline()}><Square size={12} fill="currentColor" />取消任务并终止所有进程</button>}
        </section>}

        {store.error && <div className="inline-error"><CircleAlert size={16} /><span>{store.error}</span><button type="button" onClick={() => store.setError(null)}>关闭</button></div>}
      </section>

      <div
        className="pane-resizer"
        role="separator"
        tabIndex={0}
        aria-label="调整创建任务与历史任务面板宽度"
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

      <section className="projects-pane" ref={projectsPaneRef} aria-label="项目成果">
        <div className="pane-header"><h2>02 历史任务</h2><button className="refresh-action" type="button" disabled={isRunning} onClick={() => void refreshProjects()}><RotateCcw size={14} />刷新</button></div>
        <div className="archive-summary"><span><b>{completed.length}</b><small>已完成</small></span><span><b>{unfinished.length}</b><small>未完成</small></span></div>

        {completed.length === 0 && unfinished.length === 0 && <div className="empty-state"><FileBox size={30} strokeWidth={1.4} /><strong>还没有生成项目</strong><p>选择视频和项目目录后开始生成，成果会自动出现在这里。</p></div>}

        {completed.length > 0 && <div className="project-group"><div className="group-heading"><span>已完成</span><small>{completed.length} 个项目</small></div>{completed.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} previewing={openingPreviewProjectId === project.id} previewDisabled={openingPreviewProjectId !== null || closingPreviewProjectId !== null} onPreview={(item) => void previewProject(item)} onDelete={(item) => void removeProject(item)} />)}</div>}
        {unfinished.length > 0 && <div className="project-group unfinished"><div className="group-heading"><span>未完成</span><small>{unfinished.length} 个项目</small></div>{unfinished.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} previewing={false} previewDisabled onPreview={() => undefined} onDelete={(item) => void removeProject(item)} />)}</div>}
      </section>
    </section>
    </div>

    <aside className={showZoomControls ? "zoom-dock open" : "zoom-dock"} aria-label="界面缩放">
      {showZoomControls && <div className="zoom-controls">
        <button type="button" aria-label="缩小界面" disabled={uiScale <= 80} onClick={() => changeScale(-10)}><Minus size={16} /></button>
        <button className="zoom-reset" type="button" title="恢复 100%" onClick={() => setUiScale(100)}>恢复</button>
        <button type="button" aria-label="放大界面" disabled={uiScale >= 140} onClick={() => changeScale(10)}><Plus size={16} /></button>
      </div>}
      <button className="zoom-trigger" type="button" aria-expanded={showZoomControls} onClick={() => setShowZoomControls((visible) => !visible)}>{uiScale}%</button>
    </aside>
    {telemetryPreferences && !telemetryPreferences.consentDecided && <TelemetryPreferences mode="consent" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} />}
    {telemetryPreferences && privacySettingsOpen && <TelemetryPreferences mode="settings" preferences={telemetryPreferences} busy={telemetryBusy} onChange={(enabled) => void changeTelemetryConsent(enabled)} onClose={() => setPrivacySettingsOpen(false)} />}
  </main>;
}
