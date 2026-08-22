import { useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent } from "react";
import {
  Aperture, Check, ChevronRight, CircleAlert, Clapperboard, Cpu, Download, FileBox,
  FolderOpen, LoaderCircle, MapPin, Minus, Play, Plus, RefreshCw, RotateCcw, Sparkles, Square, Trash2,
  X, Zap,
} from "lucide-react";
import appLogo from "../../assets/app-icon.svg";
import { SplatViewer } from "../components/SplatViewer";
import {
  cancelPipeline, checkEngines, confirmAndDeleteProject, downloadMissingEngines,
  getProjectOverview, onEngineDownloadProgress, onPipelineEvent, probeAndPlan,
  revealProject, selectProjectsRoot, selectVideo, setProjectsRoot, startPipeline,
} from "../lib/backend";
import { startElapsedTicker } from "../lib/elapsedTimer";
import { useAppStore } from "../stores/appStore";
import type { EngineDownloadProgress, EngineStatus, ProjectStatus, ProjectSummary, Quality } from "../types/pipeline";

const engineDescriptions: Record<string, { label: string; role: string; hint: string }> = {
  ffmpeg: {
    label: "FFmpeg",
    role: "视频抽帧提取",
    hint: "macOS 可在终端执行 brew install ffmpeg，或将可执行文件放入 engines/ffmpeg/ 目录。",
  },
  ffprobe: {
    label: "FFprobe",
    role: "视频元数据与规格探测",
    hint: "macOS 可在终端执行 brew install ffmpeg，或将可执行文件放入 engines/ffmpeg/ 目录。",
  },
  colmap: {
    label: "COLMAP",
    role: "特征提取、顺序匹配与相机轨迹重建",
    hint: "macOS 可在终端执行 brew install colmap，或将可执行文件放入 engines/colmap/bin/ 目录。",
  },
  brush: {
    label: "Brush",
    role: "基于 GPU/Metal 的 3D 高斯泼溅模型训练",
    hint: "请从 GitHub (ArthurBrussee/brush) Releases 下载对应 macOS 版本的 brush_app，放入 engines/brush/ 目录或加入系统 PATH。",
  },
};

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

const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/i.test(navigator.userAgent || "");
const revealLabel = isMac ? "在访达中显示" : "在资源管理器中显示";

function ProjectRow({
  project,
  busy,
  onDelete,
  onPreview,
}: {
  project: ProjectSummary;
  busy: boolean;
  onDelete: (project: ProjectSummary) => void;
  onPreview: (project: ProjectSummary) => void;
}) {
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
      <div><dt>SPLAT</dt><dd>{project.splatCount?.toLocaleString() ?? "—"}</dd></div>
      <div><dt>生成日期</dt><dd>{formatDate(project.completedAt ?? project.createdAt)}</dd></div>
      <div><dt>耗时</dt><dd>{formatDuration(project.durationMs)}</dd></div>
      <div><dt>档位</dt><dd>{qualityLabel(project.quality)}</dd></div>
    </dl>
    <div className="project-actions">
      {project.status === "completed" && project.finalPly && (
        <button
          type="button"
          style={{ color: "var(--blue)", fontWeight: 650 }}
          onClick={() => onPreview(project)}
        >
          <Sparkles size={14} />3D 预览
        </button>
      )}
      <button type="button" onClick={() => void revealProject(project)}><MapPin size={14} />{revealLabel}</button>
      <button className="danger-link" type="button" disabled={busy} onClick={() => onDelete(project)}><Trash2 size={14} />删除</button>
    </div>
  </article>;
}

export function App() {
  const store = useAppStore();
  const isRunning = store.phase === "running";
  const logEnd = useRef<HTMLDivElement>(null);
  const workspaceRef = useRef<HTMLElement>(null);
  const runStartedAt = useRef<number | null>(null);
  const [liveElapsedMs, setLiveElapsedMs] = useState(0);
  const [leftPanePercent, setLeftPanePercent] = useState(() => Math.min(68, Math.max(32, readSavedNumber("ooo-splat-left-pane", 44))));
  const [uiScale, setUiScale] = useState(() => Math.min(140, Math.max(80, readSavedNumber("ooo-splat-ui-scale", 100))));
  const [isResizing, setIsResizing] = useState(false);
  const [showZoomControls, setShowZoomControls] = useState(false);
  const [showEngineModal, setShowEngineModal] = useState(false);
  const [isCheckingEngines, setIsCheckingEngines] = useState(false);
  const [isDownloadingEngines, setIsDownloadingEngines] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<EngineDownloadProgress | null>(null);
  const [previewProject, setPreviewProject] = useState<ProjectSummary | null>(null);
  const missingEngines = store.engines.filter((engine) => !engineReady(engine));
  const completed = useMemo(() => store.projects.filter((project) => project.status === "completed"), [store.projects]);
  const unfinished = useMemo(() => store.projects.filter((project) => project.status !== "completed"), [store.projects]);
  const activeStageIndex = stagePosition(store.latestEvent?.stage);

  const refreshProjects = async () => {
    const overview = await getProjectOverview();
    store.setProjectsRoot(overview.projectsRoot);
    store.setProjects(overview.projects);
  };

  const refreshEngines = async () => {
    setIsCheckingEngines(true);
    try {
      const engines = await checkEngines();
      store.setEngines(engines);
      store.setColmapAcceleration(engines.find((engine) => engine.kind === "colmap")?.acceleration ?? null);
    } catch (error) {
      store.setError(messageOf(error));
    } finally {
      setIsCheckingEngines(false);
    }
  };

  const handleDownloadEngines = async () => {
    setIsDownloadingEngines(true);
    setDownloadProgress({
      engine: "brush",
      phase: "downloading",
      percent: 0,
      message: "准备下载适配当前系统的原生引擎...",
    });
    try {
      const engines = await downloadMissingEngines();
      store.setEngines(engines);
      store.setColmapAcceleration(engines.find((engine) => engine.kind === "colmap")?.acceleration ?? null);
      if (store.videoPath && !store.plan) {
        await analyze(store.videoPath, store.quality);
      }
    } catch (error) {
      store.setError(messageOf(error));
    } finally {
      setIsDownloadingEngines(false);
      setDownloadProgress(null);
    }
  };

  useEffect(() => {
    let unlisten: undefined | (() => void);
    void onEngineDownloadProgress((progress) => {
      setDownloadProgress(progress);
    }).then((fn) => { unlisten = fn; });
    return () => unlisten?.();
  }, []);

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
      if (await confirmAndDeleteProject(project)) await refreshProjects();
    } catch (error) { store.setError(messageOf(error)); }
  };

  return <main className={isResizing ? "app-shell resizing" : "app-shell"}>
    <div className="interface-frame" style={{ "--ui-scale": uiScale / 100, "--ui-size": `${10000 / uiScale}%` } as CSSProperties}>
    <header className="topbar">
      <div className="brand-lockup"><span className="brand-mark"><img src={appLogo} alt="" aria-hidden="true" /></span><span className="brand-name">OOO<span>Splat</span></span><span className="version-tag">LOCAL / 0.2.0</span></div>
      <button className="engine-summary" type="button" title="点击查看所有引擎状态与详细错误" onClick={() => setShowEngineModal(true)}>
        <span className={missingEngines.length ? "status-light warning" : "status-light"} />
        <span>{store.engines.length === 0 ? "正在检查内置引擎" : missingEngines.length ? `${missingEngines.length} 个引擎异常（点击查看）` : "FFmpeg · COLMAP · Brush 就绪"}</span>
      </button>
    </header>

    <section className="workspace" ref={workspaceRef} style={{ "--left-pane-width": `${leftPanePercent}%` } as CSSProperties}>
      <section className="control-pane" aria-label="生成控制台">
        <div className="pane-header"><h1>01 创建新任务</h1><span className={isRunning ? "run-state active" : "run-state"}>{isRunning ? "运行中" : "待命"}</span></div>

        {(missingEngines.length > 0 || isDownloadingEngines) && (
          <div className={isDownloadingEngines ? "engine-banner downloading" : "engine-banner"}>
            <div className="engine-banner-content">
              <div className="engine-banner-text">
                {isDownloadingEngines ? (
                  <>
                    <LoaderCircle className="spin" size={16} />
                    <span>{downloadProgress?.message || "正在下载并配置适配当前系统的引擎…"} ({Math.round(downloadProgress?.percent ?? 0)}%)</span>
                  </>
                ) : (
                  <>
                    <CircleAlert size={16} />
                    <span>检测到 {missingEngines.length} 个引擎未就绪 ({missingEngines.map((e) => engineDescriptions[e.kind]?.label || e.kind).join(", ")})</span>
                  </>
                )}
              </div>
              <button
                className="engine-download-btn"
                type="button"
                disabled={isDownloadingEngines}
                onClick={() => void handleDownloadEngines()}
              >
                {isDownloadingEngines ? <LoaderCircle className="spin" size={14} /> : <Download size={14} />}
                {isDownloadingEngines ? "正在配置…" : "一键自动下载并配置"}
              </button>
            </div>
            {isDownloadingEngines && (
              <div className="engine-banner-bar">
                <div className="engine-banner-fill" style={{ width: `${Math.max(5, downloadProgress?.percent ?? 0)}%` }} />
              </div>
            )}
          </div>
        )}

        <div className="form-section">
          <label className="field-label">输入视频</label>
          <button className="path-picker" type="button" disabled={isRunning} onClick={() => void chooseVideo()}>
            <Clapperboard size={18} /><span><strong>{store.videoPath ? basename(store.videoPath) : "选择 MP4 或 MOV 视频"}</strong><small>{store.videoPath ?? "从本机选择环绕拍摄素材"}</small></span><FolderOpen size={16} />
          </button>
        </div>

        <div className="form-section">
          <label className="field-label">项目根目录</label>
          <button className="path-picker compact" type="button" disabled={isRunning} onClick={() => void chooseRoot()}>
            <FolderOpen size={18} /><span><strong>{store.projectsRoot ? basename(store.projectsRoot) : "正在读取默认目录"}</strong><small>{store.projectsRoot || "Documents\\SplatStudio\\Projects"}</small></span><ChevronRight size={16} />
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

        <div className={`acceleration-status ${store.colmapAcceleration?.backend === "gpu" ? "gpu" : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "colmapUnavailable", "colmapCudaUnavailable"].includes(store.colmapAcceleration.reasonCode) ? "warning" : "cpu"}`} aria-live="polite">
          <span className="acceleration-icon">{store.colmapAcceleration?.backend === "gpu" ? <Zap size={17} fill="currentColor" /> : store.colmapAcceleration && !["nvidiaSmiNotFound", "noNvidiaGpu", "colmapUnavailable", "colmapCudaUnavailable"].includes(store.colmapAcceleration.reasonCode) ? <CircleAlert size={17} /> : store.colmapAcceleration ? <Cpu size={17} /> : <LoaderCircle className="spin" size={17} />}</span>
          <span>
            <strong>{store.colmapAcceleration == null ? "正在检测 COLMAP 加速…" : store.colmapAcceleration.backend === "gpu" ? "COLMAP GPU 加速已开启" : "COLMAP 使用 CPU"}</strong>
            <small>{store.colmapAcceleration == null ? "正在读取硬件加速能力" : store.colmapAcceleration.backend === "gpu" && store.colmapAcceleration.device ? `${store.colmapAcceleration.device.name} · 驱动 ${store.colmapAcceleration.device.driverVersion} · Compute Capability ${store.colmapAcceleration.device.computeCapability}` : ["driverTooOld", "computeCapabilityTooLow", "driverVersionUnknown", "computeCapabilityUnknown"].includes(store.colmapAcceleration.reasonCode) ? `${store.colmapAcceleration.reason} · 最低要求：驱动 ${store.colmapAcceleration.requirements.minimumDriverVersion}，Compute Capability ${store.colmapAcceleration.requirements.minimumComputeCapability}` : store.colmapAcceleration.reason}</small>
          </span>
        </div>

        {store.video && store.plan && <div className="source-metrics">
          <span><small>时长</small><b>{formatVideoDuration(store.video.duration)}</b></span>
          <span><small>分辨率</small><b>{store.video.width} × {store.video.height}</b></span>
          <span><small>预计帧数</small><b>约 {store.plan.estimatedFrames.toLocaleString()}</b></span>
        </div>}

        {!isRunning && <button className="primary-action" type="button" disabled={!store.videoPath || !store.plan || !store.projectsRoot || store.phase === "analyzing" || missingEngines.length > 0} onClick={() => void generate()}>
          {store.phase === "analyzing" ? <LoaderCircle className="spin" size={17} /> : <Play size={16} fill="currentColor" />}
          {store.phase === "analyzing" ? "正在分析视频" : "开始生成"}<ChevronRight size={16} />
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
          {store.phase === "completed" && store.result && (
            <button
              className="primary-action"
              type="button"
              style={{ marginTop: 12, background: "var(--blue)" }}
              onClick={() => {
                const res = store.result!;
                setPreviewProject({
                  id: res.projectId,
                  name: basename(res.projectPath),
                  status: "completed",
                  projectPath: res.projectPath,
                  finalPly: res.finalPly,
                  fileSize: res.fileSize,
                  splatCount: res.splatCount,
                  createdAt: new Date().toISOString(),
                  completedAt: res.completedAt,
                  durationMs: res.durationMs,
                  quality: store.quality,
                  sourceName: basename(store.videoPath || "video"),
                  registeredRatio: res.registeredRatio,
                  points3d: res.points3d,
                  failureMessage: null,
                });
              }}
            >
              <Sparkles size={16} />立即在应用内 3D 预览<ChevronRight size={16} />
            </button>
          )}
          {isRunning && <button className="cancel-action" type="button" onClick={() => void cancelPipeline()}><Square size={12} fill="currentColor" />取消任务并终止所有进程</button>}
        </section>}

        {store.error && <div className="inline-error"><CircleAlert size={16} /><span>{store.error}</span><button type="button" onClick={() => store.setError(null)}>关闭</button></div>}
      </section>

      <div
        className="pane-resizer"
        role="separator"
        tabIndex={0}
        aria-label="调整左右面板宽度"
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

      <section className="projects-pane" aria-label="项目成果">
        <div className="pane-header"><h2>02 历史任务</h2><button className="refresh-action" type="button" disabled={isRunning} onClick={() => void refreshProjects()}><RotateCcw size={14} />刷新</button></div>
        <div className="archive-summary"><span><b>{completed.length}</b><small>已完成</small></span><span><b>{unfinished.length}</b><small>未完成</small></span></div>

        {completed.length === 0 && unfinished.length === 0 && <div className="empty-state"><FileBox size={30} strokeWidth={1.4} /><strong>还没有生成项目</strong><p>选择视频和项目目录后开始生成，成果会自动出现在这里。</p></div>}

        {completed.length > 0 && <div className="project-group"><div className="group-heading"><span>已完成</span><small>{completed.length} 个项目</small></div>{completed.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} onDelete={(item) => void removeProject(item)} onPreview={(item) => setPreviewProject(item)} />)}</div>}
        {unfinished.length > 0 && <div className="project-group unfinished"><div className="group-heading"><span>未完成</span><small>{unfinished.length} 个项目</small></div>{unfinished.map((project) => <ProjectRow key={project.id} project={project} busy={isRunning} onDelete={(item) => void removeProject(item)} onPreview={(item) => setPreviewProject(item)} />)}</div>}
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

    {showEngineModal && (
      <div className="engine-modal-backdrop" onClick={() => setShowEngineModal(false)}>
        <div className="engine-modal" onClick={(e) => e.stopPropagation()}>
          <div className="engine-modal-header">
            <h3><Cpu size={18} />内置引擎状态详情</h3>
            <button
              className="engine-modal-close"
              type="button"
              aria-label="关闭"
              onClick={() => setShowEngineModal(false)}
            >
              <X size={18} />
            </button>
          </div>
          <div className="engine-modal-body">
            {store.engines.length === 0 ? (
              <p style={{ color: "var(--muted)", textAlign: "center", padding: "20px 0" }}>正在检测本地引擎状态…</p>
            ) : (
              store.engines.map((engine) => {
                const desc = engineDescriptions[engine.kind] ?? { label: engine.kind, role: "原生引擎", hint: "" };
                const ready = engine.canStart;
                return (
                  <div key={engine.kind} className={ready ? "engine-card" : "engine-card error"}>
                    <div className="engine-card-header">
                      <span className="engine-card-title">
                        {ready ? <Check size={16} color="var(--green)" /> : <CircleAlert size={16} color="var(--red)" />}
                        {desc.label} <small style={{ fontWeight: 400, color: "var(--muted)", fontSize: 12 }}>({desc.role})</small>
                      </span>
                      <span className={ready ? "engine-badge ok" : "engine-badge err"}>
                        {ready ? "就绪" : "异常"}
                      </span>
                    </div>
                    <div className="engine-card-path" title={engine.path}>
                      {engine.path}
                    </div>
                    <div className="engine-card-detail">
                      {engine.detail || (ready ? "引擎已就绪并可正常调用" : "引擎文件缺失或无法启动")}
                      {engine.version && <span style={{ display: "block", color: "var(--faint)", marginTop: 2 }}>版本: {engine.version}</span>}
                    </div>
                    {!ready && desc.hint && (
                      <div className="engine-card-hint">
                        💡 <strong>解决提示</strong>：{desc.hint}
                      </div>
                    )}
                  </div>
                );
              })
            )}
          </div>
          <div className="engine-modal-footer">
            <span style={{ fontSize: 12, color: "var(--muted)" }}>
              {missingEngines.length === 0 ? "全部 4 个引擎工作正常" : `当前有 ${missingEngines.length} 个引擎未就绪`}
            </span>
            <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
              {missingEngines.length > 0 && (
                <button
                  className="engine-download-btn"
                  type="button"
                  disabled={isDownloadingEngines}
                  onClick={() => void handleDownloadEngines()}
                >
                  {isDownloadingEngines ? <LoaderCircle className="spin" size={14} /> : <Download size={14} />}
                  {isDownloadingEngines ? "正在下载…" : "一键自动下载并配置"}
                </button>
              )}
              <button
                className="engine-refresh-btn"
                type="button"
                disabled={isCheckingEngines || isDownloadingEngines}
                onClick={() => void refreshEngines()}
              >
                <RefreshCw size={14} className={isCheckingEngines ? "spin" : ""} />
                {isCheckingEngines ? "正在检查…" : "重新检查"}
              </button>
            </div>
          </div>
        </div>
      </div>
    )}

    {previewProject && (
      <SplatViewer
        project={previewProject}
        onClose={() => setPreviewProject(null)}
      />
    )}
  </main>;
}
