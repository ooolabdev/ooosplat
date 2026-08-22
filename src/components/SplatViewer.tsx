import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type WheelEvent as ReactWheelEvent } from "react";
import {
  ArrowLeft, Camera, Check, ChevronDown, ChevronLeft, ChevronRight, ChevronUp,
  Compass, Copy, Crosshair, Download, Eye, Focus, Gamepad2, Grid3X3,
  Locate, Maximize2, Minimize2, Move, Navigation, Plane, RefreshCcw, RotateCw,
  SlidersHorizontal, Sparkles, Sun, Target, Video, X, Zap,
} from "lucide-react";
import { exportPly, readPlyBytes } from "../lib/backend";
import { parsePlyBuffer, type ParsedSplatData } from "../lib/splat/plyParser";
import { SplatRenderer, type BackgroundMode, type RenderMode } from "../lib/splat/renderer";
import type { ProjectSummary } from "../types/pipeline";

interface SplatViewerProps {
  project: ProjectSummary;
  onClose: () => void;
}

export function SplatViewer({ project, onClose }: SplatViewerProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const rendererRef = useRef<SplatRenderer | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  const [loading, setLoading] = useState(true);
  const [loadingText, setLoadingText] = useState("正在读取 3D 高斯点云数据...");
  const [error, setError] = useState<string | null>(null);
  const [splatCount, setSplatCount] = useState(project.splatCount ?? 0);
  const [fps, setFps] = useState(60);

  // Settings
  const [renderMode, setRenderMode] = useState<RenderMode>("splat");
  const [backgroundMode, setBackgroundMode] = useState<BackgroundMode>("grid");
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [splatScale, setSplatScale] = useState(1.0);
  const [flySpeed, setFlySpeed] = useState(1.0);
  const [showFlightRemote, setShowFlightRemote] = useState(true);
  const [copiedSnapshot, setCopiedSnapshot] = useState(false);

  // Focus & Telemetry State
  const [focusTarget, setFocusTarget] = useState<[number, number, number]>([0, 0, 0]);
  const [bounds, setBounds] = useState<ParsedSplatData["bounds"] | null>(null);
  const [showTargetPanel, setShowTargetPanel] = useState(false);
  const [clickIndicator, setClickIndicator] = useState<{ x: number; y: number; text: string } | null>(null);

  const [telemetry, setTelemetry] = useState({
    x: 0,
    y: 0,
    z: 0,
    pitchDeg: 0,
    yawDeg: 0,
    speed: 1.0,
  });

  // Pointer interaction state
  const isDragging = useRef(false);
  const dragButton = useRef(0);
  const lastMousePos = useRef({ x: 0, y: 0 });

  // Load and parse PLY
  useEffect(() => {
    let active = true;
    const loadModel = async () => {
      if (!project.finalPly) {
        setError("未找到该项目的 3D 模型文件 (final.ply)");
        setLoading(false);
        return;
      }
      try {
        setLoading(true);
        setLoadingText("正在从本地读取 PLY 数据...");
        const bytes = await readPlyBytes(project.finalPly);
        if (!active) return;

        setLoadingText(`正在解析 ${Math.round(bytes.length / (1024 * 1024))} MB 高斯点云...`);
        await new Promise((r) => setTimeout(r, 20));
        if (!active) return;

        const data = parsePlyBuffer(bytes);
        if (!active) return;

        setSplatCount(data.count);
        setBounds(data.bounds);
        setFocusTarget([...data.bounds.center]);

        if (canvasRef.current) {
          const renderer = new SplatRenderer(canvasRef.current, data);
          rendererRef.current = renderer;
        }
        setLoading(false);
      } catch (err: unknown) {
        if (!active) return;
        const msg = err instanceof Error ? err.message : String(err);
        setError(msg);
        setLoading(false);
      }
    };

    void loadModel();

    return () => {
      active = false;
      rendererRef.current?.destroy();
      rendererRef.current = null;
    };
  }, [project.finalPly]);

  // Sync FPS & Telemetry
  useEffect(() => {
    const timer = setInterval(() => {
      if (rendererRef.current) {
        const cam = rendererRef.current.camera;
        setFps(rendererRef.current.fps);
        setFlySpeed(Number(cam.speedMultiplier.toFixed(1)));

        setTelemetry({
          x: Number(cam.pos[0].toFixed(2)),
          y: Number(cam.pos[1].toFixed(2)),
          z: Number(cam.pos[2].toFixed(2)),
          pitchDeg: Math.round((cam.pitch * 180) / Math.PI),
          yawDeg: Math.round((((cam.yaw * 180) / Math.PI) % 360 + 360) % 360),
          speed: Number(cam.speedMultiplier.toFixed(1)),
        });

        const curTarget = cam.getPivot();
        setFocusTarget((prev) => {
          if (
            Math.abs(prev[0] - curTarget[0]) > 0.001 ||
            Math.abs(prev[1] - curTarget[1]) > 0.001 ||
            Math.abs(prev[2] - curTarget[2]) > 0.001
          ) {
            return [curTarget[0], curTarget[1], curTarget[2]];
          }
          return prev;
        });
      }
    }, 120);
    return () => clearInterval(timer);
  }, []);

  // Keyboard 6-DoF Free Fly Navigation & Shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const camera = rendererRef.current?.camera;

      if (e.key === "Escape") {
        if (showTargetPanel) {
          setShowTargetPanel(false);
        } else if (isFullscreen) {
          void document.exitFullscreen?.();
          setIsFullscreen(false);
        } else {
          onClose();
        }
        return;
      }

      if (e.key.toLowerCase() === "r") {
        handleResetView();
        return;
      }
      if (e.key.toLowerCase() === "f") {
        toggleFullscreen();
        return;
      }
      if (e.key.toLowerCase() === "p") {
        setShowTargetPanel((prev) => !prev);
        return;
      }

      // Free fly key states
      if (camera) {
        const key = e.key.toLowerCase();
        if (key === "w" || e.key === "ArrowUp") { camera.keyMovement.forward = true; }
        if (key === "s" || e.key === "ArrowDown") { camera.keyMovement.backward = true; }
        if (key === "a" || e.key === "ArrowLeft") { camera.keyMovement.left = true; }
        if (key === "d" || e.key === "ArrowRight") { camera.keyMovement.right = true; }
        if (key === "e" || e.key === " ") { e.preventDefault(); camera.keyMovement.up = true; }
        if (key === "q" || key === "c") { camera.keyMovement.down = true; }
        if (e.shiftKey) { camera.keyMovement.boost = true; }
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      const camera = rendererRef.current?.camera;
      if (camera) {
        const key = e.key.toLowerCase();
        if (key === "w" || e.key === "ArrowUp") camera.keyMovement.forward = false;
        if (key === "s" || e.key === "ArrowDown") camera.keyMovement.backward = false;
        if (key === "a" || e.key === "ArrowLeft") camera.keyMovement.left = false;
        if (key === "d" || e.key === "ArrowRight") camera.keyMovement.right = false;
        if (key === "e" || e.key === " ") camera.keyMovement.up = false;
        if (key === "q" || key === "c") camera.keyMovement.down = false;
        if (!e.shiftKey) camera.keyMovement.boost = false;
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [isFullscreen, onClose, showTargetPanel]);

  // Pointer interactions
  const handlePointerDown = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    isDragging.current = true;
    dragButton.current = e.button;
    lastMousePos.current = { x: e.clientX, y: e.clientY };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    if (!isDragging.current || !rendererRef.current) return;
    const dx = e.clientX - lastMousePos.current.x;
    const dy = e.clientY - lastMousePos.current.y;
    lastMousePos.current = { x: e.clientX, y: e.clientY };

    const camera = rendererRef.current.camera;
    if (dragButton.current === 2 || e.shiftKey) {
      // Right click or Shift + Drag = Pan / Translate
      camera.pan(dx, dy);
    } else {
      // Left click / Drag = Look Around (Positive Direct Mapping)
      camera.lookAround(dx * 0.0045, dy * 0.0045);
    }
  };

  const handlePointerUp = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    isDragging.current = false;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
  };

  // Mouse wheel smoothly modulates fly speed
  const handleWheel = (e: ReactWheelEvent<HTMLCanvasElement>) => {
    e.preventDefault();
    if (!rendererRef.current) return;
    rendererRef.current.camera.zoom(e.deltaY);
  };

  // Double click: Raycast and look towards 3D surface point
  const handleDoubleClick = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    if (!rendererRef.current) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const clickX = e.clientX - rect.left;
    const clickY = e.clientY - rect.top;

    const hit = rendererRef.current.pickPoint(clickX, clickY);
    if (hit) {
      rendererRef.current.camera.setPivot(hit[0], hit[1], hit[2]);
      setFocusTarget(hit);
      setClickIndicator({
        x: e.clientX,
        y: e.clientY,
        text: `已对准目标 (${hit[0].toFixed(2)}, ${hit[1].toFixed(2)}, ${hit[2].toFixed(2)})`,
      });
      setTimeout(() => setClickIndicator(null), 2000);
    } else {
      handleResetView();
    }
  };

  const handleResetView = () => {
    if (rendererRef.current) {
      rendererRef.current.camera.setViewPreset("reset", rendererRef.current.data.bounds);
      setFocusTarget([...rendererRef.current.data.bounds.center]);
    }
  };

  const handleSetTargetToCentroid = () => {
    if (rendererRef.current && bounds) {
      rendererRef.current.camera.setPivot(bounds.centroid[0], bounds.centroid[1], bounds.centroid[2]);
      setFocusTarget([...bounds.centroid]);
    }
  };

  const handleSetTargetToBoundingCenter = () => {
    if (rendererRef.current && bounds) {
      rendererRef.current.camera.setPivot(bounds.center[0], bounds.center[1], bounds.center[2]);
      setFocusTarget([...bounds.center]);
    }
  };

  const handleSetCoordinate = (axis: 0 | 1 | 2, val: number) => {
    if (!rendererRef.current) return;
    const next: [number, number, number] = [...focusTarget];
    next[axis] = val;
    rendererRef.current.camera.setPivot(next[0], next[1], next[2]);
    setFocusTarget(next);
  };

  const handleFlySpeedPreset = (mult: number) => {
    setFlySpeed(mult);
    if (rendererRef.current) {
      rendererRef.current.camera.speedMultiplier = mult;
    }
  };

  const handlePreset = (preset: "front" | "top" | "side" | "iso") => {
    if (rendererRef.current) {
      rendererRef.current.camera.setViewPreset(preset, rendererRef.current.data.bounds);
    }
  };

  const toggleFullscreen = () => {
    if (!containerRef.current) return;
    if (!document.fullscreenElement) {
      void containerRef.current.requestFullscreen?.();
      setIsFullscreen(true);
    } else {
      void document.exitFullscreen?.();
      setIsFullscreen(false);
    }
  };

  const handleRenderModeChange = (mode: RenderMode) => {
    setRenderMode(mode);
    if (rendererRef.current) {
      rendererRef.current.renderMode = mode;
    }
  };

  const handleBackgroundChange = (mode: BackgroundMode) => {
    setBackgroundMode(mode);
    if (rendererRef.current) {
      rendererRef.current.backgroundMode = mode;
    }
  };

  const handleSplatScale = (scale: number) => {
    setSplatScale(scale);
    if (rendererRef.current) {
      rendererRef.current.splatScale = scale;
    }
  };

  const handleSnapshot = () => {
    if (!rendererRef.current) return;
    const dataUrl = rendererRef.current.captureScreenshot();
    const link = document.createElement("a");
    link.download = `${project.name}-snapshot.png`;
    link.href = dataUrl;
    link.click();
    setCopiedSnapshot(true);
    setTimeout(() => setCopiedSnapshot(false), 2000);
  };

  const sendMoveKey = (fwd: number, right: number, up: number) => {
    if (rendererRef.current?.camera) {
      rendererRef.current.camera.keyMovement.forward = fwd > 0;
      rendererRef.current.camera.keyMovement.backward = fwd < 0;
      rendererRef.current.camera.keyMovement.right = right > 0;
      rendererRef.current.camera.keyMovement.left = right < 0;
      rendererRef.current.camera.keyMovement.up = up > 0;
      rendererRef.current.camera.keyMovement.down = up < 0;
    }
  };

  const releaseMoveKey = () => {
    if (rendererRef.current?.camera) {
      rendererRef.current.camera.keyMovement.forward = false;
      rendererRef.current.camera.keyMovement.backward = false;
      rendererRef.current.camera.keyMovement.right = false;
      rendererRef.current.camera.keyMovement.left = false;
      rendererRef.current.camera.keyMovement.up = false;
      rendererRef.current.camera.keyMovement.down = false;
    }
  };

  const formatBytes = (bytes: number | null) =>
    bytes == null
      ? "—"
      : bytes >= 1024 ** 3
      ? `${(bytes / 1024 ** 3).toFixed(2)} GB`
      : bytes >= 1024 ** 2
      ? `${(bytes / 1024 ** 2).toFixed(1)} MB`
      : `${(bytes / 1024).toFixed(1)} KB`;

  return (
    <div className="splat-viewer-root" ref={containerRef}>
      {/* 1. macOS Glassmorphism Topbar (Strict Single-Line) */}
      <header className="viewer-topbar">
        <div className="viewer-topbar-left">
          <button className="viewer-back-btn" type="button" onClick={onClose} title="返回控制台 (Esc)">
            <ChevronLeft size={17} />
            <span>返回任务</span>
          </button>

          <div className="viewer-title-group">
            <h2 className="viewer-project-name" title={project.name}>{project.name}</h2>
            <div className="viewer-badges">
              <span className="viewer-badge splat-count">
                <Sparkles size={12} />
                <b>{splatCount ? splatCount.toLocaleString() : "—"}</b> 点
              </span>
              <span className="viewer-badge file-size">
                {formatBytes(project.fileSize)}
              </span>
            </div>
          </div>
        </div>

        <div className="viewer-topbar-center">
          {/* Mode Title Badge */}
          <div className="nav-chip active" style={{ cursor: "default" }}>
            <Plane size={13} />
            <span>6-DoF 自由漫游模式</span>
          </div>
        </div>

        <div className="viewer-topbar-right">
          {/* Target Inspector Trigger */}
          <button
            className={showTargetPanel ? "viewer-action-btn active" : "viewer-action-btn"}
            type="button"
            onClick={() => setShowTargetPanel((v) => !v)}
            title="调节观察目标坐标 (快捷键 P)"
          >
            <Target size={14} color={showTargetPanel ? "var(--blue)" : "currentColor"} />
            <span>目标坐标</span>
          </button>

          <button
            className="viewer-action-btn"
            type="button"
            onClick={handleSnapshot}
            title="截取当前视角高清 PNG"
          >
            {copiedSnapshot ? <Check size={14} color="var(--green)" /> : <Camera size={14} />}
            <span>{copiedSnapshot ? "已保存" : "截图"}</span>
          </button>

          <button
            className="viewer-action-btn"
            type="button"
            onClick={() => project.finalPly && void exportPly({ ...project, finalPly: project.finalPly } as any)}
            title="另存为 PLY 模型文件"
          >
            <Download size={14} />
            <span>导出</span>
          </button>

          <button className="viewer-close-btn" type="button" onClick={onClose} aria-label="关闭预览" title="关闭 (Esc)">
            <X size={16} />
          </button>
        </div>
      </header>

      {/* 2. WebGL2 Viewport Canvas */}
      <div className="viewer-canvas-container">
        <canvas
          ref={canvasRef}
          className="viewer-canvas drone-mode"
          onContextMenu={(e) => e.preventDefault()}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerUp}
          onWheel={handleWheel}
          onDoubleClick={handleDoubleClick}
        />

        {/* Center Flight Reticle */}
        {!loading && !error && (
          <div className="drone-reticle">
            <div className="reticle-circle" />
            <div className="reticle-line h" />
            <div className="reticle-line v" />
          </div>
        )}

        {/* Double-Click Indicator Bubble */}
        {clickIndicator && (
          <div
            className="pivot-indicator-bubble"
            style={{ left: clickIndicator.x, top: clickIndicator.y }}
          >
            <span className="pivot-indicator-dot" />
            <span className="pivot-indicator-text">{clickIndicator.text}</span>
          </div>
        )}

        {/* Loading Overlay */}
        {loading && (
          <div className="viewer-loader-backdrop">
            <div className="viewer-loader-card">
              <div className="viewer-spinner" />
              <strong>{loadingText}</strong>
              <small>正在使用 GPU Metal / WebGL2 进行 3D 协方差光栅化准备</small>
            </div>
          </div>
        )}

        {/* Error Overlay */}
        {error && (
          <div className="viewer-loader-backdrop">
            <div className="viewer-error-card">
              <X size={24} color="var(--red)" />
              <strong>模型加载失败</strong>
              <p>{error}</p>
              <button className="viewer-action-btn" type="button" onClick={onClose}>
                返回
              </button>
            </div>
          </div>
        )}

        {/* Target Coordinates Inspector Floating Popover (Top Right, Non-colliding) */}
        {showTargetPanel && bounds && (
          <div className="pivot-popover-panel">
            <div className="pivot-panel-header">
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <Target size={15} color="var(--blue)" />
                <strong>目标与重心坐标控制</strong>
              </div>
              <button
                type="button"
                className="viewer-close-btn"
                onClick={() => setShowTargetPanel(false)}
                title="关闭"
              >
                <X size={14} />
              </button>
            </div>

            <p className="pivot-panel-tip">
              💡 <b>快捷操作</b>：在 3D 视口中<b>双击模型任意表面</b>，即可自动对准该点为航向目标！
            </p>

            <div className="pivot-coord-group">
              <div className="pivot-coord-row">
                <span className="coord-tag axis-x">X</span>
                <input
                  type="range"
                  min={bounds.min[0] - bounds.radius * 0.5}
                  max={bounds.max[0] + bounds.radius * 0.5}
                  step={0.01}
                  value={focusTarget[0]}
                  onChange={(e) => handleSetCoordinate(0, parseFloat(e.target.value))}
                />
                <input
                  type="number"
                  step={0.05}
                  value={Number(focusTarget[0].toFixed(3))}
                  onChange={(e) => handleSetCoordinate(0, parseFloat(e.target.value) || 0)}
                  className="coord-input"
                />
              </div>

              <div className="pivot-coord-row">
                <span className="coord-tag axis-y">Y</span>
                <input
                  type="range"
                  min={bounds.min[1] - bounds.radius * 0.5}
                  max={bounds.max[1] + bounds.radius * 0.5}
                  step={0.01}
                  value={focusTarget[1]}
                  onChange={(e) => handleSetCoordinate(1, parseFloat(e.target.value))}
                />
                <input
                  type="number"
                  step={0.05}
                  value={Number(focusTarget[1].toFixed(3))}
                  onChange={(e) => handleSetCoordinate(1, parseFloat(e.target.value) || 0)}
                  className="coord-input"
                />
              </div>

              <div className="pivot-coord-row">
                <span className="coord-tag axis-z">Z</span>
                <input
                  type="range"
                  min={bounds.min[2] - bounds.radius * 0.5}
                  max={bounds.max[2] + bounds.radius * 0.5}
                  step={0.01}
                  value={focusTarget[2]}
                  onChange={(e) => handleSetCoordinate(2, parseFloat(e.target.value))}
                />
                <input
                  type="number"
                  step={0.05}
                  value={Number(focusTarget[2].toFixed(3))}
                  onChange={(e) => handleSetCoordinate(2, parseFloat(e.target.value) || 0)}
                  className="coord-input"
                />
              </div>
            </div>

            <div className="pivot-presets">
              <button
                type="button"
                className="pivot-preset-btn"
                onClick={handleSetTargetToCentroid}
                title="对准所有高斯点坐标均值"
              >
                <Locate size={13} />
                <span>对准点云重心</span>
              </button>

              <button
                type="button"
                className="pivot-preset-btn"
                onClick={handleSetTargetToBoundingCenter}
                title="对准 3D 包围盒几何中点"
              >
                <Focus size={13} />
                <span>对准包围盒中心</span>
              </button>

              <button
                type="button"
                className="pivot-preset-btn"
                onClick={() => {
                  if (rendererRef.current) {
                    rendererRef.current.camera.setPivot(0, 0, 0);
                    setFocusTarget([0, 0, 0]);
                  }
                }}
                title="归零坐标"
              >
                <RefreshCcw size={13} />
                <span>坐标原点 (0,0,0)</span>
              </button>
            </div>
          </div>
        )}

        {/* 3. Non-Overlapping Bottom HUD (Pinned Bottom-Left) */}
        {!loading && !error && (
          <div className="viewer-bottom-hud">
            <div className="hud-metric">
              <Zap size={13} color={fps >= 55 ? "var(--green)" : fps >= 30 ? "var(--amber)" : "var(--red)"} />
              <span><b>{fps}</b> FPS</span>
            </div>
            <div className="hud-divider" />
            <div className="hud-metric">
              <Eye size={13} />
              <span><b>{(splatCount / 10000).toFixed(1)}w</b> 点</span>
            </div>
            <div className="hud-divider" />
            <div className="hud-metric telemetry-badge">
              <span>高度 <b>{telemetry.y >= 0 ? `+${telemetry.y}` : telemetry.y}m</b></span>
              <span>航速 <b>{flySpeed}x</b></span>
            </div>
          </div>
        )}

        {/* 4. Non-Overlapping Unified Action Dock (Pinned Bottom-Center) */}
        {!loading && !error && (
          <nav className="viewer-action-dock" aria-label="3D 视口控制栏">
            {/* Reset / Home */}
            <button
              className="dock-action-btn"
              type="button"
              onClick={handleResetView}
              title="复位到初始视角 (快捷键 R)"
            >
              <RefreshCcw size={14} />
              <span>复位</span>
            </button>

            {/* Remote Controller Toggle */}
            <button
              className={showFlightRemote ? "dock-action-btn active" : "dock-action-btn"}
              type="button"
              onClick={() => setShowFlightRemote((v) => !v)}
              title="显示/隐藏漫游遥控台"
            >
              <Gamepad2 size={14} />
              <span>遥控</span>
            </button>

            <div className="dock-separator" />

            {/* Camera Presets */}
            <div className="dock-segmented">
              <button type="button" onClick={() => handlePreset("iso")} title="等轴视角">等轴</button>
              <button type="button" onClick={() => handlePreset("front")} title="正面视角">正视</button>
              <button type="button" onClick={() => handlePreset("top")} title="俯视视角">俯视</button>
              <button type="button" onClick={() => handlePreset("side")} title="侧面视角">侧视</button>
            </div>

            <div className="dock-separator" />

            {/* Render Mode */}
            <div className="dock-segmented">
              <button
                type="button"
                className={renderMode === "splat" ? "active" : ""}
                onClick={() => handleRenderModeChange("splat")}
                title="真实高斯泼溅渲染"
              >
                光影
              </button>
              <button
                type="button"
                className={renderMode === "pointCloud" ? "active" : ""}
                onClick={() => handleRenderModeChange("pointCloud")}
                title="几何点云视图"
              >
                点云
              </button>
            </div>

            <div className="dock-separator" />

            {/* Background Mode */}
            <div className="dock-segmented">
              <button
                type="button"
                className={backgroundMode === "grid" ? "active" : ""}
                onClick={() => handleBackgroundChange("grid")}
                title="网格地平面"
              >
                <Grid3X3 size={13} />
              </button>
              <button
                type="button"
                className={backgroundMode === "dark" ? "active" : ""}
                onClick={() => handleBackgroundChange("dark")}
                title="深空纯黑底色"
              >
                黑
              </button>
              <button
                type="button"
                className={backgroundMode === "studio" ? "active" : ""}
                onClick={() => handleBackgroundChange("studio")}
                title="工作室灰底色"
              >
                灰
              </button>
            </div>

            <div className="dock-separator" />

            {/* Splat Scale / Sharpness */}
            <div className="dock-segmented">
              <button
                type="button"
                className={splatScale === 0.75 ? "active" : ""}
                onClick={() => handleSplatScale(0.75)}
                title="精细高斯 (0.75x)"
              >
                精细
              </button>
              <button
                type="button"
                className={splatScale === 1.0 ? "active" : ""}
                onClick={() => handleSplatScale(1.0)}
                title="标准高斯 (1.0x)"
              >
                标准
              </button>
              <button
                type="button"
                className={splatScale === 1.5 ? "active" : ""}
                onClick={() => handleSplatScale(1.5)}
                title="柔和饱满 (1.5x)"
              >
                柔和
              </button>
            </div>

            <div className="dock-separator" />

            {/* Fullscreen Toggle */}
            <button
              className={isFullscreen ? "dock-action-btn active" : "dock-action-btn"}
              type="button"
              onClick={toggleFullscreen}
              title="沉浸全屏 (快捷键 F)"
            >
              {isFullscreen ? <Minimize2 size={14} /> : <Maximize2 size={14} />}
            </button>
          </nav>
        )}

        {/* 5. Non-Overlapping Drone Mini-Remote (Pinned Bottom-Right) */}
        {!loading && !error && showFlightRemote && (
          <div className="drone-mini-remote">
            <div className="remote-top-bar">
              <span className="remote-title">
                <Plane size={13} color="var(--blue)" />
                <b>漫游遥控台</b>
              </span>
              <div className="remote-speed-chips">
                <button
                  type="button"
                  className={flySpeed === 0.5 ? "speed-btn active" : "speed-btn"}
                  onClick={() => handleFlySpeedPreset(0.5)}
                >
                  0.5x
                </button>
                <button
                  type="button"
                  className={flySpeed === 1.0 ? "speed-btn active" : "speed-btn"}
                  onClick={() => handleFlySpeedPreset(1.0)}
                >
                  1.0x
                </button>
                <button
                  type="button"
                  className={flySpeed === 2.5 ? "speed-btn active" : "speed-btn"}
                  onClick={() => handleFlySpeedPreset(2.5)}
                >
                  2.5x
                </button>
              </div>
            </div>

            <div className="remote-pads-row">
              {/* Left D-Pad: Lift & Turn */}
              <div className="remote-dpad-col">
                <span className="dpad-caption">升降 / 转向</span>
                <div className="dpad-cross">
                  <button
                    type="button"
                    className="dpad-key up"
                    onMouseDown={() => sendMoveKey(0, 0, 1)}
                    onMouseUp={releaseMoveKey}
                    onMouseLeave={releaseMoveKey}
                    title="升高 (E / 空格)"
                  >
                    <ChevronUp size={15} />
                  </button>
                  <button
                    type="button"
                    className="dpad-key left"
                    onMouseDown={() => {
                      if (rendererRef.current?.camera) rendererRef.current.camera.lookAround(-0.06, 0);
                    }}
                    title="向左转向"
                  >
                    <ChevronLeft size={15} />
                  </button>
                  <div className="dpad-core"><Compass size={13} /></div>
                  <button
                    type="button"
                    className="dpad-key right"
                    onMouseDown={() => {
                      if (rendererRef.current?.camera) rendererRef.current.camera.lookAround(0.06, 0);
                    }}
                    title="向右转向"
                  >
                    <ChevronRight size={15} />
                  </button>
                  <button
                    type="button"
                    className="dpad-key down"
                    onMouseDown={() => sendMoveKey(0, 0, -1)}
                    onMouseUp={releaseMoveKey}
                    onMouseLeave={releaseMoveKey}
                    title="降低 (Q / C)"
                  >
                    <ChevronDown size={15} />
                  </button>
                </div>
              </div>

              {/* Right D-Pad: Forward / Backward / Strafe */}
              <div className="remote-dpad-col">
                <span className="dpad-caption">推进 / 平移</span>
                <div className="dpad-cross">
                  <button
                    type="button"
                    className="dpad-key up"
                    onMouseDown={() => sendMoveKey(1, 0, 0)}
                    onMouseUp={releaseMoveKey}
                    onMouseLeave={releaseMoveKey}
                    title="前进推进 (W / ↑)"
                  >
                    <ChevronUp size={15} />
                  </button>
                  <button
                    type="button"
                    className="dpad-key left"
                    onMouseDown={() => sendMoveKey(0, -1, 0)}
                    onMouseUp={releaseMoveKey}
                    onMouseLeave={releaseMoveKey}
                    title="向左平移 (A / ←)"
                  >
                    <ChevronLeft size={15} />
                  </button>
                  <div className="dpad-core"><Navigation size={13} /></div>
                  <button
                    type="button"
                    className="dpad-key right"
                    onMouseDown={() => sendMoveKey(0, 1, 0)}
                    onMouseUp={releaseMoveKey}
                    onMouseLeave={releaseMoveKey}
                    title="向右平移 (D / →)"
                  >
                    <ChevronRight size={15} />
                  </button>
                  <button
                    type="button"
                    className="dpad-key down"
                    onMouseDown={() => sendMoveKey(-1, 0, 0)}
                    onMouseUp={releaseMoveKey}
                    onMouseLeave={releaseMoveKey}
                    title="后退倒飞 (S / ↓)"
                  >
                    <ChevronDown size={15} />
                  </button>
                </div>
              </div>
            </div>

            <div className="remote-keys-legend">
              <span><b>WASD</b> 移动 · <b>滚轮</b> 调速 · <b>拖拽</b> 视角</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
