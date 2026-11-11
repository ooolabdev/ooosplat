// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../stores/appStore";
import { useGaussianTransformStore } from "../stores/gaussianTransformStore";
import { LanguageProvider } from "../i18n";
import type { ProjectSummary } from "../types/pipeline";
import packageMetadata from '../../package.json';
import { createGenerationDraft, saveTaskWorkspace } from './taskWorkspace';

const mocks = vi.hoisted(() => ({
  selectMaterialPaths:vi.fn(),selectColmapDatabase:vi.fn(),classifyMaterialPaths:vi.fn(),classifyGenerationPaths:vi.fn(),inspectMaterialInputs:vi.fn(),estimateMaterialInputs:vi.fn(),startInputsPipeline:vi.fn(),detectMaterialGps:vi.fn(),
  cancelPipeline: vi.fn(),
  checkAppUpdate: vi.fn(),
  estimateProjectRuntime: vi.fn(),
  exportPly: vi.fn(),
  prepareGaussianPreview: vi.fn(),
  releaseGaussianPreview: vi.fn(),
  revealFile: vi.fn(),
  resumePipeline: vi.fn(),
  continueTaskPipeline: vi.fn(),
  getSharedTasks: vi.fn(),
  onTaskUpdate: vi.fn(),
  getAppRuntimeStatus: vi.fn(),
  getProjectOverview: vi.fn(),
  getProjectTaskDetail: vi.fn(),
  initializeTelemetry: vi.fn(),
  inspectReshootSource: vi.fn(),
  probeReshootInput: vi.fn(),
  startReshootPipeline: vi.fn(),
  setTelemetryConsent: vi.fn(),
  selectVideo: vi.fn(),
  selectImageSequence: vi.fn(),
  selectGaussianImportSource: vi.fn(),
  probeGaussianImport: vi.fn(),
  probeAndPlan: vi.fn(),
  confirmLargeImageSequence: vi.fn(),
  confirmSmallImageSequence: vi.fn(),
  startPipeline: vi.fn(),
  revealProject: vi.fn(),
  revealProjectLogs: vi.fn(),
  notifyPreviewDisposed: vi.fn(),
  prepareErrorReport: vi.fn(),
  sendErrorReport: vi.fn(),
  classifyDroppedInput: vi.fn(),
  onInputDragDrop: vi.fn(),
}));

vi.mock("../lib/updates", () => ({ checkAppUpdate: mocks.checkAppUpdate, copyUpdateText: vi.fn() }));

vi.mock("../lib/backend", () => ({
  selectColmapFolder:()=>mocks.selectImageSequence(),
  selectMaterialPaths:mocks.selectMaterialPaths,selectColmapDatabase:mocks.selectColmapDatabase,classifyMaterialPaths:mocks.classifyMaterialPaths,classifyGenerationPaths:mocks.classifyGenerationPaths,inspectMaterialInputs:mocks.inspectMaterialInputs,estimateMaterialInputs:mocks.estimateMaterialInputs,startInputsPipeline:mocks.startInputsPipeline,detectMaterialGps:mocks.detectMaterialGps,
  cancelPipeline: mocks.cancelPipeline,
  prepareErrorReport: mocks.prepareErrorReport,
  sendErrorReport: mocks.sendErrorReport,
  checkEngines: vi.fn().mockResolvedValue([]),
  checkColmapAcceleration: vi.fn().mockResolvedValue(null),
  classifyDroppedInput: mocks.classifyDroppedInput,
  confirmAndDeleteProject: vi.fn().mockResolvedValue(false),
  confirmLargeImageSequence: mocks.confirmLargeImageSequence,
  confirmSmallImageSequence: mocks.confirmSmallImageSequence,
  estimateProjectRuntime: mocks.estimateProjectRuntime,
  exportPly: mocks.exportPly,
  getSharedTasks: mocks.getSharedTasks,
  onTaskUpdate: mocks.onTaskUpdate,
  getAppRuntimeStatus: mocks.getAppRuntimeStatus,
  getProjectOverview: mocks.getProjectOverview,
  getProjectTaskDetail: mocks.getProjectTaskDetail,
  initializeTelemetry: mocks.initializeTelemetry,
  inspectReshootSource: mocks.inspectReshootSource,
  probeReshootInput: mocks.probeReshootInput,
  onPipelineEvent: vi.fn().mockResolvedValue(() => undefined),
  onInputDragDrop: mocks.onInputDragDrop,
  prepareGaussianPreview: mocks.prepareGaussianPreview,
  probeAndPlan: mocks.probeAndPlan,
  releaseGaussianPreview: mocks.releaseGaussianPreview,
  resumePipeline: mocks.resumePipeline,
  continueTaskPipeline: mocks.continueTaskPipeline,
  revealProject: mocks.revealProject,
  revealProjectLogs: mocks.revealProjectLogs,
  revealFile: mocks.revealFile,
  selectProjectsRoot: vi.fn(),
  selectImageSequence: mocks.selectImageSequence,
  selectGaussianImportSource: mocks.selectGaussianImportSource,
  probeGaussianImport: mocks.probeGaussianImport,
  selectVideo: mocks.selectVideo,
  setProjectsRoot: vi.fn(),
  setTelemetryConsent: mocks.setTelemetryConsent,
  startPipeline: mocks.startPipeline,
  startReshootPipeline: mocks.startReshootPipeline,
}));

vi.mock("../components/GaussianViewer", () => ({
  GaussianViewer: ({ previewSessionId, onExit, onDisposed }: { previewSessionId: number; onExit: () => void | Promise<void>; onDisposed: (projectId: string, previewSessionId: number) => void }) => <section className="preview-workspace"><h1>高斯泼溅预览</h1><button type="button" onClick={() => {
    void onExit();
    mocks.notifyPreviewDisposed(onDisposed, "11111111-1111-1111-1111-111111111111", previewSessionId);
  }}>返回任务</button></section>,
}));

import { App } from "./App";

const project: ProjectSummary = {
  id: "11111111-1111-1111-1111-111111111111",
  name: "示例项目",
  status: "completed",
  projectPath: "E:\\Projects\\示例项目",
  finalPly: "E:\\Projects\\示例项目\\final.ply",
  fileSize: 73_729_603,
  splatCount: 312_407,
  createdAt: "2026-08-22T14:00:00Z",
  completedAt: "2026-08-22T15:00:00Z",
  durationMs: 3_600_000,
  quality: "balanced",
  sourceName: "input.mp4",
  registeredRatio: 0.9,
  points3d: 10_000,
  failureMessage: null,
};

const flush = async () => {
  await act(async () => {
    await Promise.resolve();
    await new Promise((resolve) => window.setTimeout(resolve, 0));
  });
};

describe("App preview workspace", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(async () => {
    (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    // jsdom does not implement scrollIntoView, and starting a run renders the live log,
    // whose auto-scroll effect then calls it.
    Element.prototype.scrollIntoView = vi.fn();
    window.localStorage.setItem("ooo-splat-language", "zh-CN");
    if (!window.requestAnimationFrame) {
      window.requestAnimationFrame = (callback) => window.setTimeout(() => callback(performance.now()), 0);
      window.cancelAnimationFrame = (handle) => window.clearTimeout(handle);
    }
    useGaussianTransformStore.getState().close();
    mocks.checkAppUpdate.mockReset().mockResolvedValue(null);
    mocks.getSharedTasks.mockReset().mockResolvedValue([]);
    mocks.onTaskUpdate.mockReset().mockResolvedValue(() => undefined);
    useAppStore.setState({
      inputPath: null, inputType: "video", projectsRoot: "E:\\Projects", projects: [], quality: "balanced", colmapAcceleration: null, taskColmapAcceleration: null,
      video: null, imageSequence: null, plan: null, estimate: null, engines: [], phase: "idle", progress: 0, progressMessage: "",
      latestEvent: null, latestRuntime: null, lastEventSequence: 0, events: [], liveTask: null, result: null, error: null, errorAt: null,
    });
    mocks.prepareGaussianPreview.mockReset();
    mocks.classifyDroppedInput.mockReset().mockResolvedValue({ inputType: "video" });
    mocks.selectGaussianImportSource.mockReset().mockResolvedValue(null);
    mocks.probeGaussianImport.mockReset();
    mocks.onInputDragDrop.mockReset().mockResolvedValue(() => undefined);
    mocks.cancelPipeline.mockReset().mockResolvedValue(undefined);
    mocks.estimateProjectRuntime.mockReset().mockResolvedValue({
      estimatedMs: 4_000_000,
      lowerBoundMs: 3_000_000,
      upperBoundMs: 5_000_000,
      confidence: "medium",
      sampleCount: 3,
      basis: "按同档位历史任务校准",
    });
    mocks.releaseGaussianPreview.mockReset().mockResolvedValue(undefined);
    mocks.getAppRuntimeStatus.mockReset().mockImplementation(async () => ({
      pipelineRunning: false,
      pipelineRunElapsedMs: 0,
      pipelineElapsedOffsetMs: 0,
      previewProjectId: useGaussianTransformStore.getState().descriptor?.projectId ?? null,
      taskAcceleration: null,
    }));
    mocks.exportPly.mockReset().mockResolvedValue("E:\\Exports\\final.ply");
    mocks.revealFile.mockReset().mockResolvedValue(undefined);
    mocks.revealProject.mockReset().mockResolvedValue(undefined);
    mocks.revealProjectLogs.mockReset().mockResolvedValue(undefined);
    mocks.prepareErrorReport.mockReset();
    mocks.sendErrorReport.mockReset();
    mocks.resumePipeline.mockReset().mockResolvedValue({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 100,
      registeredImages: 90, registeredRatio: 0.9, points3d: 10_000,
      durationMs: project.durationMs, completedAt: project.completedAt, warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    });
    mocks.continueTaskPipeline.mockReset().mockResolvedValue({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 100,
      registeredImages: 90, registeredRatio: .9, points3d: 10000,
      durationMs: project.durationMs, completedAt: project.completedAt, warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    });
    mocks.getProjectOverview.mockReset().mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [project] });
    mocks.getProjectTaskDetail.mockReset().mockResolvedValue({
      project,
      inputType: "video",
      sourcePath: "E:\\Capture\\input.mp4",
      projectsRoot: "E:\\Projects",
      plannerEnabled: true,
      estimatedFrames: 320,
      stage: "completed",
      progress: 100,
      inputImages: 100,
      registeredImages: 90,
      video: null,
      imageSequence: null,
      sourceProjectId: null,
      logs: [],
    });
    mocks.initializeTelemetry.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.inspectReshootSource.mockReset().mockResolvedValue({
      projectId: project.id,
      projectName: project.name,
      quality: project.quality,
      plannerEnabled: true,
      cameraId: 1,
      cameraModel: "SIMPLE_RADIAL",
      width: 1920,
      height: 1080,
      sourceImageCount: 100,
      eligible: true,
      reason: null,
      imported: false,
    });
    mocks.probeReshootInput.mockReset();
    mocks.startReshootPipeline.mockReset();
    mocks.setTelemetryConsent.mockReset().mockResolvedValue({ analyticsEnabled: true, consentDecided: true, deliveryStatus: "configured" });
    mocks.selectVideo.mockReset().mockResolvedValue(null);
    mocks.selectImageSequence.mockReset().mockResolvedValue(null);
    mocks.probeAndPlan.mockReset();
    mocks.confirmLargeImageSequence.mockReset().mockResolvedValue(true);
    mocks.confirmSmallImageSequence.mockReset().mockResolvedValue(true);
    mocks.startPipeline.mockReset();
    // Keep the existing single-source fixtures while exercising the new transport
    // and unified input UI. Dedicated collection tests below assert its wire shape.
    const cache=new Map<string,any>();
    const pending=new Map<string,Promise<any>>();
    let inputId=0;
    mocks.selectMaterialPaths.mockReset().mockImplementation(async(directory:boolean)=>{
      const path=await (directory?mocks.selectImageSequence():mocks.selectVideo());return path?[path]:[];
    });
    mocks.selectColmapDatabase.mockReset().mockResolvedValue(null);
    mocks.detectMaterialGps.mockReset().mockResolvedValue(false);
    mocks.estimateMaterialInputs.mockReset().mockImplementation(async inputs=>cache.get(inputs.items?.[0]?.source.path)?.estimate??null);
    mocks.classifyMaterialPaths.mockReset().mockImplementation(async(paths:string[])=>paths.map(path=>({id:`source-${++inputId}`,source:/\.(mp4|mov)$/i.test(path)?{kind:'video',path}:{kind:'imageFolder',path},projection:'auto',imageMatchingStrategy:'exhaustive'})));
    mocks.classifyGenerationPaths.mockReset().mockImplementation(async(paths:string[])=>({kind:'materials',items:await mocks.classifyMaterialPaths(paths)}));
    mocks.inspectMaterialInputs.mockReset().mockImplementation(async(inputs,quality,policy)=>{
      const materials=await Promise.all(inputs.items.map(async(item:any)=>{
        const path=item.source.path;const old=cache.get(path);let raw=old;
        const key=JSON.stringify([path,quality,policy.frameSamplingFps]);
        if(!old||old.key!==key){
          let job=pending.get(key);
          if(!job){job=(async()=>{
            let result=await mocks.probeAndPlan(path,quality,true,old?.metadata,()=>{},policy);
            if(!result&&mocks.probeReshootInput.getMockImplementation()){
              const plan=await mocks.probeReshootInput(project.id,path,item.source.kind==='video'?'video':'images',quality,true,policy,undefined);
              if(plan)result={...plan,inputType:plan.inputType,video:plan.video??(plan.inputType==='video'?{width:plan.preparedWidth,height:plan.preparedHeight,duration:plan.duration,fps:30}:null),imageSequence:plan.imageSequence??(plan.inputType==='images'?{imageCount:plan.imageCount,width:plan.preparedWidth,height:plan.preparedHeight,hasAlpha:plan.hasAlpha}:null),plan:{estimatedFrames:plan.estimatedFrames}};
            }
            result??=old?.metadata;
            if(!result)throw new Error('Missing input fixture');
            const metadata={inputType:result.inputType,video:result.video,imageSequence:result.imageSequence,projectionDetection:result.projectionDetection??{sourceIdentity:path,confirmedErp:false,requiresChoice:false,supportedErp:false,reason:'',previewJpeg:[]}};
            const value={...result,metadata,key};cache.set(path,value);return value;
          })();pending.set(key,job);void job.finally(()=>pending.delete(key)).catch(()=>{});}
          raw=await job;
        }
        const video=raw.video?{fps:30,...raw.video}:null;
        if(policy.version>=5&&video?.duration>0&&video.fps>0){
          const total=video.totalFrames??Math.max(1,Math.round(video.duration*video.fps));
          const minimumTarget=policy.version>=6?Math.min(video.fps,Math.max(1,30/video.duration)):.1;
          const maximumTarget=policy.version>=6?video.fps:Math.max(.1,video.fps-.1);
          const makePlan=(target:number)=>{const count=Math.min(total,Math.max(Math.min(30,total),Math.round(video.duration*target)));return{estimatedFrames:count,samplingFps:count/video.duration,retentionRatio:count/total};};
          const tierValue={fast:6,balanced:8,high:12}[quality as 'fast'|'balanced'|'high'];
          const defaultRemoval=policy.version>=6?Math.max(0,video.fps-tierValue):tierValue;
          const removal=Math.min(video.fps-minimumTarget,Math.max(0,item.frameRemovalFps??defaultRemoval));
          return {id:item.id,identity:raw.metadata.projectionDetection.sourceIdentity,legacyIdentity:raw.metadata.projectionDetection.sourceIdentity,video,imageSequence:raw.imageSequence,projectionDetection:raw.metadata.projectionDetection,plan:makePlan(video.fps-removal),frameSamplingRange:{minimumTargetFps:minimumTarget,maximumTargetFps:maximumTarget,minimum:makePlan(minimumTarget),maximum:makePlan(maximumTarget)}};
        }
        return {id:item.id,identity:raw.metadata.projectionDetection.sourceIdentity,legacyIdentity:raw.metadata.projectionDetection.sourceIdentity,video,imageSequence:raw.imageSequence,projectionDetection:raw.metadata.projectionDetection,plan:raw.plan};
      }));return {materials,colmap:null,errors:{}};
    });
    mocks.startInputsPipeline.mockReset().mockImplementation((inputs,quality,projectsRoot,workspaceTaskId,policy,sourceId,options)=>{
      const item=inputs.items[0];
      if(sourceId)return mocks.startReshootPipeline({sourceProjectId:sourceId,inputPath:item.source.path,inputType:item.source.kind==='video'?'video':'images',quality,projectsRoot,plannerEnabled:true,workspaceTaskId,reshootOptions:{...options,cameraRelation:item.cameraRelation},generationPolicy:policy});
      return mocks.startPipeline(item.source.path,quality,projectsRoot,true,workspaceTaskId,item.projection,item.identity,item.imageMatchingStrategy,policy);
    });
    mocks.notifyPreviewDisposed.mockReset().mockImplementation((callback: (projectId: string, previewSessionId: number) => void, projectId: string, previewSessionId: number) => {
      queueMicrotask(() => callback(projectId, previewSessionId));
    });
    mocks.prepareGaussianPreview.mockResolvedValue({
      projectId: project.id,
      modelPath: project.finalPly,
      assetPath: `${project.projectPath}\\work\\preview\\preview-session-a.ply`,
      assetUrl: "http://asset.localhost/final.ply",
      format: "ply",
      fileSize: project.fileSize,
      splatCount: project.splatCount,
      transform: { position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 },
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => { root.render(<LanguageProvider><App /></LanguageProvider>); });
    await flush();
  });

  afterEach(async () => {
    await act(async () => { root.unmount(); });
    container.remove();
    window.localStorage.clear();
  });

  it("exposes the specific failed engine and its diagnostics", async () => {
    await act(async () => {
      useAppStore.setState({ engines: [{
        kind: "ffprobe", path: "E:\\engines\\ffprobe.exe", exists: true,
        canStart: false, version: null, cpuOnly: null, acceleration: null,
        detail: "exit code -1073741515\nMissing runtime dependency",
      }, {
        kind: "ffmpeg", path: "E:\\engines\\ffmpeg.exe", exists: false,
        canStart: false, version: null, cpuOnly: null, acceleration: null,
        detail: "Executable not found",
      }] });
    });
    const { checkEngines } = await import('../lib/backend');
    const previousChecks = vi.mocked(checkEngines).mock.calls.length;
    const details = container.querySelector<HTMLElement>(".engine-diagnostics")!;
    const trigger = details.querySelector<HTMLButtonElement>(".engine-summary")!;
    expect(trigger.textContent).toBe("2 个引擎异常");
    expect(trigger.textContent).not.toMatch(/ffmpeg|ffprobe|colmap|brush/i);
    expect(details.querySelector(".engine-diagnostics-panel")).toBeNull();
    await act(async () => { trigger.focus(); });
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    expect(details.textContent).toContain("FFprobe");
    expect(details.textContent).toContain("E:\\engines\\ffprobe.exe");
    expect(details.querySelector('.engine-item-flags')?.textContent).toContain("可启动否");
    expect(details.textContent).toContain("Missing runtime dependency");
    expect(vi.mocked(checkEngines).mock.calls.length).toBe(previousChecks);
  });

  it("shows the current package version and a start action without a trailing arrow", () => {
    expect(container.querySelector(".brand-name")?.textContent).toBe("OOOSplat");
    expect(container.querySelector(".version-tag")?.textContent).toBe(`LOCAL / ${packageMetadata.version}`);
    const startButton = container.querySelector(".primary-action");
    expect(startButton?.textContent?.trim()).toBe("开始生成");
    expect(startButton?.querySelectorAll("svg")).toHaveLength(1);
  });

  it("shows shared capture advice beside the material label", () => {
    const trigger = container.querySelector<HTMLButtonElement>(".capture-help-trigger");
    const tooltip = container.querySelector<HTMLElement>(".capture-help-tooltip");
    expect(trigger?.getAttribute("aria-label")).toBe("拍摄建议");
    expect(trigger?.getAttribute("aria-describedby")).toBe(tooltip?.id);
    expect(tooltip?.getAttribute("role")).toBe("tooltip");
    expect(tooltip?.textContent).toContain("正面、侧面、背面以及高低视角");
    expect(tooltip?.textContent).toContain("60%–80% 重叠");
    expect(tooltip?.textContent).toContain("固定镜头、焦距、分辨率和曝光");
    expect(tooltip?.textContent).toContain("AI 生成图片容易出现");
  });

  it('accepts a portrait MOV as ordinary video without a panorama warning or extra confirmation', async () => {
    mocks.selectVideo.mockResolvedValueOnce('E:/Download/LocalSend/IMG_8023.MOV');
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: 'video', video: { duration: 10, width: 1920, height: 1080, rotation: -90 }, imageSequence: null,
      plan: { estimatedFrames: 60 }, projectionDetection: { sourceIdentity: 'phone-mov', confirmedErp: false, requiresChoice: false, supportedErp: false, unsupported: false, reason: '', previewJpeg: [] },
    });
    mocks.startPipeline.mockImplementationOnce(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    expect(container.querySelector('.material-row-meta')?.textContent).toContain('普通视频');
    expect(container.querySelector('.media-source-warning, .video-type-select')).toBeNull();
    expect(container.querySelector<HTMLButtonElement>('.primary-action')?.disabled).toBe(false);
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    expect(mocks.startPipeline).toHaveBeenCalledWith('E:/Download/LocalSend/IMG_8023.MOV', 'balanced', 'E:\\Projects', true, expect.any(String), 'perspective', 'phone-mov', 'exhaustive', { version: 7, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps:null });
  });

  it("requires an inline choice for ambiguous ERP and never opens a projection dialog", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:/panorama.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: "video", video: { duration: 10, width: 3840, height: 1920 }, imageSequence: null, plan: { estimatedFrames: 60 }, projectionDetection: { sourceIdentity: 'source-A', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: "2:1", previewJpeg: [] } });
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")!.click()); await flush();
    const select = container.querySelector<HTMLSelectElement>('.video-type-select')!;
    expect(select.value).toBe('');
    expect(container.querySelector<HTMLButtonElement>(".primary-action")!.disabled).toBe(true);
    expect(container.querySelector(".panorama-preview")).toBeNull();
    await act(async () => { select.value = 'equirectangular'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    expect(container.querySelector('.media-source-copy > strong')?.getAttribute('title')).toContain('已选择完整全景视频');
    expect(container.querySelector('.media-source-copy > strong')?.getAttribute('title')).not.toContain('已识别完整 ERP 元数据');
    expect(container.querySelector<HTMLButtonElement>(".primary-action")!.disabled).toBe(false);
    // The choice remains editable, and only the final choice is submitted.
    await act(async () => { select.value = 'perspective'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    mocks.startPipeline.mockImplementationOnce(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    expect(mocks.startPipeline.mock.calls[0].slice(5, 8)).toEqual(['perspective', 'source-A', 'exhaustive']);
  });

  it("keeps settings available and persists the runtime monitor preference locally", async () => {
    const settingsButton = container.querySelector<HTMLButtonElement>(".settings-action:not(.language-action)")!;
    expect(settingsButton).not.toBeNull();
    await act(async () => settingsButton.click());
    expect(container.querySelector(".settings-dialog")).not.toBeNull();
    expect(container.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toContain("界面");
    await act(async () => container.querySelector<HTMLButtonElement>(".settings-switch")!.click());
    expect(window.localStorage.getItem("ooo-splat-ui-preferences-v1")).toContain('"showRuntimePanel":true');
  });

  it('requires inline choices for all drafts before enabling the queue without starting any task', async () => {
    const probe = { inputType: 'video', video: { duration: 10, width: 3840, height: 1920 }, imageSequence: null, plan: { estimatedFrames: 60 }, projectionDetection: { sourceIdentity: 'first', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: '2:1', previewJpeg: [] } };
    mocks.probeAndPlan.mockResolvedValueOnce(probe).mockResolvedValueOnce({ ...probe, projectionDetection: { ...probe.projectionDetection, sourceIdentity: 'second' } });
    mocks.selectVideo.mockResolvedValueOnce('E:/first.mp4').mockResolvedValueOnce('E:/second.mp4');
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.group-add-action')!.click());
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(container.querySelector('.queue-input-notice')?.textContent).toContain('first.mp4');
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    const choose = async (value: string) => {
      let select = container.querySelector<HTMLSelectElement>('.video-type-select');
      if (!select) {
        await act(async () => container.querySelector<HTMLButtonElement>('.material-row.needs-action .material-row-main')?.click());
        await flush();
        select = container.querySelector<HTMLSelectElement>('.video-type-select');
      }
      expect(select).not.toBeNull();
      await act(async () => { select!.value = value; select!.dispatchEvent(new Event('change', { bubbles: true })); });
      await flush();
    };
    await choose('equirectangular');
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    expect(container.querySelector('.queue-input-notice')?.textContent).toContain('second.mp4');
    await choose('perspective');
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('true');
    expect(mocks.startPipeline).not.toHaveBeenCalled();
    const saved = JSON.parse(localStorage.getItem('ooo-splat-task-workspace-v1')!);
    expect(saved.drafts.flatMap((d: {inputs?:{items?:Array<{projection:string}>}})=>d.inputs?.items?.map(i=>i.projection)??[])).toEqual(['equirectangular', 'perspective']);
  });

  it('disables the queue for new ambiguous input and requires manual re-enabling after choosing', async () => {
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('true');
    mocks.selectVideo.mockResolvedValueOnce('E:/later.mp4');
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: 'video', video: { duration: 1, width: 200, height: 100 }, imageSequence: null, plan: { estimatedFrames: 6 }, projectionDetection: { sourceIdentity: 'later', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: '', previewJpeg: [] } });
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    expect(container.querySelector('.queue-input-notice')?.textContent).toContain('later.mp4');
    await act(async () => { const select = container.querySelector<HTMLSelectElement>('.video-type-select')!; select.value = 'perspective'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    expect(mocks.startPipeline).not.toHaveBeenCalled();
    expect(mocks.cancelPipeline).not.toHaveBeenCalled();
  });

  it('automatically starts confirmed ERP without a projection question', async () => {
    mocks.selectVideo.mockResolvedValueOnce('E:/confirmed.mp4');
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: 'video', video: { duration: 1, width: 200, height: 100 }, imageSequence: null, plan: { estimatedFrames: 6 }, projectionDetection: { sourceIdentity: 'confirmed', confirmedErp: true, requiresChoice: false, supportedErp: true, reason: '', previewJpeg: [] } });
    mocks.startPipeline.mockImplementationOnce(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.startPipeline.mock.calls.at(-1)?.slice(5, 8)).toEqual(['equirectangular', 'confirmed', 'exhaustive']);
  });

  it.each([false, true])('shows sampling settings only while the selected draft contains video (reshoot: %s)', async reshoot => {
    if (reshoot) {
      await act(async () => container.querySelector<HTMLButtonElement>('.reshoot-link')!.click());
      await flush();
    }
    expect(container.querySelector('.more-settings-button')).toBeNull();
    expect(container.querySelector('.frame-sampling-control')).toBeNull();
    mocks.selectVideo.mockResolvedValue('E:/sampling.mov');
    mocks.selectImageSequence.mockResolvedValue('E:/photos');
    mocks.probeAndPlan.mockImplementation(async (path: string) => path.endsWith('.mov') ? {
      inputType: 'video', video: { duration: 60, width: 1920, height: 1080, fps: 30, totalFrames: 1800 },
      imageSequence: null, plan: { estimatedFrames: 480 },
    } : {
      inputType: 'images', video: null, imageSequence: { imageCount: 40, width: 800, height: 600, hasAlpha: false },
      plan: { estimatedFrames: 40 },
    });
    const addVideo = async () => {
      await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
      await flush();
    };
    const removeInput = async () => {
      await act(async () => container.querySelector<HTMLButtonElement>('.material-remove')!.click());
      await flush();
    };
    await addVideo();
    expect(container.querySelector('.more-settings-button')?.textContent).toBe('更多设置');
    await act(async () => container.querySelector<HTMLButtonElement>('.more-settings-button')!.click());
    expect(container.querySelector('.frame-sampling-control')).not.toBeNull();
    await removeInput();
    expect(container.querySelector('.more-settings-button')).toBeNull();
    expect(container.querySelector('.frame-sampling-control')).toBeNull();
    await addVideo();
    expect(container.querySelector('.more-settings-button')?.getAttribute('aria-expanded')).toBe('false');
    expect(container.querySelector('.frame-sampling-control')).toBeNull();
    await removeInput();
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:last-child')!.click());
    await flush();
    expect(container.querySelector('.more-settings-button')).toBeNull();
    expect(container.querySelector('.frame-sampling-control')).toBeNull();
  });

  it.each([.5,.001])('restores an old standby ratio %s with its legacy control without losing a verified projection choice',async ratio=>{
    await act(async()=>root.unmount());
    const draft={...createGenerationDraft(1),inputPath:'E:/legacy.mp4',frameSamplingRatio:ratio,projectionAnswer:{identity:'old-id',projection:'equirectangular' as const}};
    saveTaskWorkspace([draft],{kind:'draft',id:draft.id},2);
    const legacy=JSON.parse(localStorage.getItem('ooo-splat-task-workspace-v1')!);delete legacy.drafts[0].generationPolicyVersion;localStorage.setItem('ooo-splat-task-workspace-v1',JSON.stringify(legacy));
    const legacyFrames=Math.max(30,1800*ratio);
    mocks.probeAndPlan.mockResolvedValue({inputType:'video',video:{duration:60,width:200,height:100,fps:30,totalFrames:1800},imageSequence:null,plan:{estimatedFrames:legacyFrames,samplingFps:legacyFrames/60,retentionRatio:legacyFrames/1800},projectionDetection:{sourceIdentity:'old-id',confirmedErp:false,requiresChoice:true,supportedErp:true,reason:'',previewJpeg:[]}});
    root=createRoot(container);
    await act(async()=>root.render(<LanguageProvider><App/></LanguageProvider>));await flush();
    expect(container.querySelector<HTMLButtonElement>('.more-settings-button')?.textContent).toBe('更多设置');
    await act(async()=>container.querySelector<HTMLButtonElement>('.more-settings-button')!.click());
    expect(container.querySelector<HTMLButtonElement>('.more-settings-button')?.textContent).toBe('更少设置');
    expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe(ratio===.5?'50.0%':'0.1%');
    expect(container.querySelector<HTMLSelectElement>('.video-type-select')?.value).toBe('equirectangular');
    const stored=JSON.parse(localStorage.getItem('ooo-splat-task-workspace-v1')!).drafts[0];
    expect(stored.inputs.items[0]).toMatchObject({identity:'old-id',projection:'equirectangular'});
    expect(mocks.inspectMaterialInputs.mock.calls.at(-1)?.[2]).toMatchObject(ratio===.001?{version:4,frameSamplingRatio:ratio,frameSamplingFps:null}:{version:4,frameSamplingRatio:null,frameSamplingFps:15});
    await act(async()=>container.querySelector<HTMLButtonElement>('.frame-sampling-control button')!.click());await flush();
    expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('26.7%');
    expect(mocks.inspectMaterialInputs.mock.calls.at(-1)?.[2]).toMatchObject({version:4,frameSamplingRatio:null,frameSamplingFps:null});
  });

  it('drops a legacy projection answer when the rechecked input identity changed',async()=>{
    await act(async()=>root.unmount());
    const draft={...createGenerationDraft(1),inputPath:'E:/changed.mp4',projectionAnswer:{identity:'old-id',projection:'equirectangular' as const}};
    saveTaskWorkspace([draft],{kind:'draft',id:draft.id},2);
    mocks.probeAndPlan.mockResolvedValue({inputType:'video',video:{duration:60,width:200,height:100,fps:30},imageSequence:null,plan:{estimatedFrames:480},projectionDetection:{sourceIdentity:'new-id',confirmedErp:false,requiresChoice:true,supportedErp:true,reason:'',previewJpeg:[]}});
    root=createRoot(container);
    await act(async()=>root.render(<LanguageProvider><App/></LanguageProvider>));await flush();
    expect(container.querySelector<HTMLSelectElement>('.video-type-select')?.value).toBe('');
    expect(container.querySelector<HTMLButtonElement>('.primary-action')!.disabled).toBe(true);
  });

  it('replans manual base sampling from cached metadata, resets on quality change, and does not replan retry consent', async () => {
    const video = { duration:60,width:1920,height:1080,fps:30,totalFrames:1800,hasAlpha:false };
    const projectionDetection = {sourceIdentity:'sampling-input',confirmedErp:false,requiresChoice:false,supportedErp:false,reason:'',previewJpeg:[]};
    mocks.selectVideo.mockResolvedValueOnce('E:/sampling.mp4');
    const metadata = {inputType:'video',video,projectionDetection,imageSequence:null,plan:{estimatedFrames:480}};
    mocks.probeAndPlan.mockResolvedValue(metadata);
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async()=>container.querySelector<HTMLButtonElement>('.more-settings-button')!.click());
    expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('8 FPS');
    const slider = container.querySelector<HTMLInputElement>('.frame-sampling-control input')!;
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value')!.set!.call(slider,'8');
      slider.dispatchEvent(new Event('input',{bubbles:true}));
      slider.dispatchEvent(new Event('change',{bubbles:true}));
    }); await flush();
    expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('9 FPS');
    expect(mocks.probeAndPlan).toHaveBeenCalledTimes(1);
    expect(mocks.inspectMaterialInputs.mock.calls.at(-1)?.[0]).toMatchObject({mode:'materials',items:[expect.objectContaining({frameRemovalFps:21})]});
    expect(mocks.inspectMaterialInputs.mock.calls.at(-1)?.[2]).toEqual({version:7,autoRetryEnabled:true,frameSamplingRatio:null,frameSamplingFps:null});
    const calls = mocks.probeAndPlan.mock.calls.length;
    await act(async () => container.querySelector<HTMLButtonElement>('.planner-switch')!.click()); await flush();
    expect(mocks.probeAndPlan).toHaveBeenCalledTimes(calls);
    await act(async () => container.querySelectorAll<HTMLButtonElement>('.quality-option')[2].click()); await flush();
    expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('12 FPS');
    expect(mocks.inspectMaterialInputs.mock.calls.at(-1)?.[2]).toEqual({version:7,autoRetryEnabled:false,frameSamplingRatio:null,frameSamplingFps:null});
  });

  it.each(['perspective', 'equirectangular'] as const)('replans quality without redetecting %s or resetting the chosen type', async projection => {
    const video = { duration: 10, width: 200, height: 100 };
    const projectionDetection = { sourceIdentity: 'same-source', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: '', previewJpeg: [] };
    const probe = { inputType: 'video', video, projectionDetection, imageSequence: null, plan: { estimatedFrames: 20 } };
    mocks.selectVideo.mockResolvedValueOnce('E:/clip.mp4');
    mocks.probeAndPlan.mockResolvedValueOnce(probe);
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => { const select = container.querySelector<HTMLSelectElement>('.video-type-select')!; select.value = projection; select.dispatchEvent(new Event('change', { bubbles: true })); });
    await act(async()=>container.querySelector<HTMLButtonElement>('.more-settings-button')!.click());
    let finish!: (value: unknown) => void;
    mocks.probeAndPlan.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>('.quality-option:not(.selected)')!.click()); await flush();
    expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('分析中');
    expect(mocks.probeAndPlan).toHaveBeenLastCalledWith('E:/clip.mp4', 'fast', true, { inputType: 'video', video, imageSequence: null, projectionDetection }, expect.any(Function), { version: 7, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps:null });
    expect(container.querySelector('.media-source-status.checking')).toBeNull();
    expect(container.querySelector<HTMLSelectElement>('.video-type-select')?.value).toBe(projection);
    expect(container.querySelector<HTMLButtonElement>('.primary-action')?.disabled).toBe(true);
    // A rapid second change is folded into the pending plan, never another source probe.
    await act(async () => container.querySelectorAll<HTMLButtonElement>('.quality-option')[2].click()); await flush();
    expect(mocks.probeAndPlan).toHaveBeenCalledTimes(2);
    mocks.probeAndPlan.mockResolvedValueOnce({ ...probe, plan: { estimatedFrames: 80 } });
    await act(async () => finish(probe)); await flush();
    expect(mocks.probeAndPlan).toHaveBeenLastCalledWith('E:/clip.mp4', 'high', true, { inputType: 'video', video, imageSequence: null, projectionDetection }, expect.any(Function), { version: 7, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps:null });
    expect(container.querySelector<HTMLSelectElement>('.video-type-select')?.value).toBe(projection);
    expect(container.querySelector<HTMLButtonElement>('.primary-action')?.disabled).toBe(false);
    await act(async () => container.querySelector<HTMLButtonElement>('.quality-option.selected')!.click()); await flush();
    expect(mocks.probeAndPlan).toHaveBeenCalledTimes(3);
    mocks.probeAndPlan.mockResolvedValueOnce(probe);
    await act(async () => container.querySelector<HTMLButtonElement>('.planner-switch')!.click()); await flush();
    expect(mocks.probeAndPlan).toHaveBeenCalledTimes(3);
    expect(container.querySelector('.planner-switch')?.getAttribute('aria-checked')).toBe('false');
  });

  it('finishes the initial video detection once and plans with the latest quality', async () => {
    const video = { duration: 10, width: 200, height: 100 };
    const projectionDetection = { sourceIdentity: 'initial', confirmedErp: true, requiresChoice: false, supportedErp: true, reason: '', previewJpeg: [] };
    const probe = { inputType: 'video', video, projectionDetection, imageSequence: null, plan: { estimatedFrames: 20 } };
    let finish!: (value: unknown) => void;
    mocks.selectVideo.mockResolvedValueOnce('E:/clip.mp4');
    mocks.probeAndPlan.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.quality-option:not(.selected)')!.click()); await flush();
    expect(mocks.probeAndPlan).toHaveBeenCalledTimes(1);
    expect(container.querySelector('.media-source-status.checking')).not.toBeNull();
    mocks.probeAndPlan.mockResolvedValueOnce(probe);
    await act(async () => finish(probe)); await flush();
    expect(mocks.probeAndPlan).toHaveBeenLastCalledWith('E:/clip.mp4', 'fast', true, { inputType: 'video', video, imageSequence: null, projectionDetection }, expect.any(Function), { version: 7, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps:null });
    expect(container.querySelector('.media-source-status.checking')).toBeNull();
    expect(container.querySelector('.media-source-copy > strong')?.getAttribute('title')).toContain('已识别完整 ERP');
    expect(container.querySelector('.media-source-metadata')).not.toBeNull();
  });

  it('shows an empty prompt and loading, ignores late analysis and invalidates changed-source choices', async () => {
    expect(container.querySelector('.material-empty p')?.textContent).toBe('点击添加，或将视频、图片和图片文件夹拖到这里');
    expect([...container.querySelectorAll('.material-empty button')].map(button => button.textContent)).toEqual(['添加文件','添加文件夹']);
    const probe = { inputType: 'video', video: { duration: 1, width: 200, height: 100 }, imageSequence: null, plan: { estimatedFrames: 6 }, projectionDetection: { sourceIdentity: 'A', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: '', previewJpeg: [] } };
    let first!: (value: unknown) => void, second!: (value: unknown) => void;
    mocks.probeAndPlan.mockImplementationOnce(() => new Promise(resolve => { first = resolve; })).mockImplementationOnce(() => new Promise(resolve => { second = resolve; }));
    mocks.selectVideo.mockResolvedValueOnce('E:/first.mp4').mockResolvedValueOnce('E:/second.mp4');
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    expect(container.querySelector('.media-source-status.checking')?.textContent).toBe('正在检测视频类型…');
    expect(container.querySelector<HTMLButtonElement>('.primary-action')!.disabled).toBe(true);
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => first({ ...probe, projectionDetection: { ...probe.projectionDetection, confirmedErp: true, requiresChoice: false } })); await flush();
    expect(container.querySelector('.checking')).not.toBeNull();
    expect(container.querySelector('.media-source-status')?.textContent).not.toContain('已识别');
    await act(async () => second(probe)); await flush();
    await act(async () => { const select = container.querySelector<HTMLSelectElement>('.video-type-select')!; select.value = 'equirectangular'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    mocks.probeAndPlan.mockResolvedValueOnce({ ...probe, projectionDetection: { ...probe.projectionDetection, sourceIdentity: 'B' } });
    await act(async () => container.querySelector<HTMLButtonElement>('.quality-option:not(.selected)')!.click()); await flush();
    expect(container.querySelector<HTMLSelectElement>('.video-type-select')!.value).toBe('');
    expect(container.querySelector<HTMLButtonElement>('.primary-action')!.disabled).toBe(true);
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it('does not auto-start an unconfirmed input when it finishes analysis after the current task', async () => {
    const probe = { inputType: 'video', video: { duration: 1, width: 200, height: 100 }, imageSequence: null, plan: { estimatedFrames: 6 } };
    let finishAnalysis!: (value: unknown) => void;
    mocks.probeAndPlan.mockResolvedValueOnce(probe).mockImplementationOnce(() => new Promise(resolve => { finishAnalysis = resolve; }));
    mocks.selectVideo.mockResolvedValueOnce('E:/running.mp4').mockResolvedValueOnce('E:/unknown.mp4');
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    let finishRun!: (value: unknown) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise(resolve => { finishRun = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click()); await flush();
    const workspaceTaskId = mocks.startPipeline.mock.calls[0][4];
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: 'E:/Projects', projects: [{ ...project, workspaceTaskId }] });
    await act(async () => container.querySelectorAll<HTMLElement>('.draft-row')[1].click());
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('true');
    await act(async () => finishRun({ projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly, fileSize: 10, splatCount: 10, inputImages: 6, registeredImages: 6, registeredRatio: 1, points3d: 10, durationMs: 1, completedAt: project.completedAt, warning: null, logsDirectory: 'E:/logs' })); await flush();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    await act(async () => finishAnalysis({ ...probe, projectionDetection: { sourceIdentity: 'unknown', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: '', previewJpeg: [] } })); await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    expect(container.querySelector('.video-type-select')).not.toBeNull();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    expect(mocks.cancelPipeline).not.toHaveBeenCalled();
  });

  it.each(['completed', 'failed'] as const)('shows saved panorama processing in the %s project media box', async status => {
    await act(async () => useAppStore.getState().setProjects([{ ...project, status }]));
    mocks.getProjectTaskDetail.mockResolvedValueOnce({ project: { ...project, status }, inputType: 'video', videoProjection: 'equirectangular', sourcePath: 'E:/old.mp4', projectsRoot: 'E:/Projects', plannerEnabled: true, stage: 'trainingSplats', progress: 60, logs: [] });
    const group = status === 'completed' ? '#completed-task-group-content' : '#unfinished-task-group-content';
    await act(async () => container.querySelector<HTMLElement>(`${group} .project-row`)!.click()); await flush();
    expect(container.querySelector('.source-input-card.readonly .media-source-copy > strong')?.textContent).toBe('全景视频');
    expect(container.querySelector('.source-input-card.readonly .media-source-status')?.textContent).not.toContain('本任务按');
    expect(container.querySelector('.source-input-card select')).toBeNull();
    expect(mocks.probeAndPlan).not.toHaveBeenCalled();
  });

  it('keeps an inline image-order answer across quality replanning', async () => {
    const probe = { inputType: 'images', video: null, imageSequence: { imageCount: 101, width: 200, height: 100, requiresLargeSequenceConfirmation: false }, plan: { estimatedFrames: 101 }, projectionDetection: null };
    mocks.selectVideo.mockResolvedValueOnce('E:/photos');
    mocks.probeAndPlan.mockResolvedValue(probe);
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    const order = container.querySelector<HTMLInputElement>('.material-order-options input[value="sequential"]')!;
    await act(async () => order.click());
    await act(async () => container.querySelector<HTMLButtonElement>('.quality-option:not(.selected)')!.click()); await flush();
    expect(container.querySelector<HTMLInputElement>('.material-order-options input[value="sequential"]')?.checked).toBe(true);
    expect(container.querySelector('.preflight-dialog')).toBeNull();
  });

  it('waits for in-flight analysis when a running task finishes during queue activation', async () => {
    const probe = { inputType: 'video', video: { duration: 1, width: 200, height: 100 }, imageSequence: null, plan: { estimatedFrames: 6 } };
    let finishAnalysis!: (value: unknown) => void;
    mocks.probeAndPlan.mockResolvedValueOnce(probe).mockImplementationOnce(() => new Promise(resolve => { finishAnalysis = resolve; }));
    mocks.selectVideo.mockResolvedValueOnce('E:/running.mp4').mockResolvedValueOnce('E:/next.mp4');
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    let finishRun!: (value: unknown) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise(resolve => { finishRun = resolve; })).mockImplementationOnce(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click()); await flush();
    const workspaceTaskId = mocks.startPipeline.mock.calls[0][4];
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: 'E:/Projects', projects: [{ ...project, workspaceTaskId }] });
    await act(async () => container.querySelectorAll<HTMLElement>('.draft-row')[1].click());
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    await act(async () => finishRun({ projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly, fileSize: 10, splatCount: 10, inputImages: 6, registeredImages: 6, registeredRatio: 1, points3d: 10, durationMs: 1, completedAt: project.completedAt, warning: null, logsDirectory: 'E:/logs' })); await flush();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    await act(async () => finishAnalysis(probe)); await flush(); await flush();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(2);
    expect(mocks.startPipeline.mock.calls[1][0]).toBe('E:/next.mp4');
  });

  it("adds, selects, and deletes independent standby drafts while retaining one", async () => {
    expect(Array.from(container.querySelectorAll(".group-toggle > span"), (node) => node.textContent)).toEqual(["新任务", "已完成", "未完成"]);
    const add = container.querySelector<HTMLButtonElement>(".group-add-action");
    const heading = container.querySelector<HTMLElement>(".new-task-group .group-heading")!;
    const newTasksToggle = heading.querySelector<HTMLButtonElement>(".group-toggle")!;
    const queueToggle = heading.querySelector<HTMLButtonElement>(".queue-toggle")!;
    const projectCount = heading.querySelector<HTMLElement>(".group-count")!;
    expect(projectCount.textContent).toBe("1 个项目");
    expect(heading.children[0]).toBe(newTasksToggle);
    expect(heading.children[1]).toBe(add);
    expect(heading.children[2]).toBe(queueToggle);
    expect(heading.children[3]).toBe(projectCount);
    expect(queueToggle.getAttribute("aria-checked")).toBe("false");
    expect(container.querySelectorAll(".draft-row")).toHaveLength(1);
    await act(async () => add?.click());
    expect(container.querySelectorAll(".draft-row")).toHaveLength(2);
    expect(projectCount.textContent).toBe("2 个项目");
    expect(container.querySelectorAll(".draft-row.selected")).toHaveLength(1);

    const remove = container.querySelectorAll<HTMLButtonElement>(".draft-row .danger-link");
    await act(async () => remove[1].click());
    await act(async () => container.querySelector<HTMLButtonElement>(".draft-row .danger-link")?.click());
    expect(container.querySelectorAll(".draft-row")).toHaveLength(1);
  });

  it("offers file and folder imports from the completed group", async () => {
    const heading = container.querySelector<HTMLElement>(".completed-task-group .group-heading")!;
    const toggle = heading.querySelector<HTMLButtonElement>(".group-toggle")!;
    const add = container.querySelector<HTMLButtonElement>(".completed-import-actions .group-add-action")!;
    const count = heading.querySelector<HTMLElement>(".group-count")!;
    expect(add.textContent).toContain("导入");
    expect(heading.children[0]).toBe(toggle);
    expect(heading.children[1]).toBe(add.parentElement);
    expect(heading.children[2]).toBe(count);
    await act(async () => add.click());
    const actions = container.querySelectorAll<HTMLButtonElement>(".completed-import-menu button");
    expect(Array.from(actions, button => button.textContent)).toEqual(["导入文件", "导入文件夹"]);
    await act(async () => actions[0].click());
    expect(mocks.selectGaussianImportSource).toHaveBeenCalledWith("file");
  });

  it("uses the completed-project layout for imported models and hides irrelevant detail sections", async () => {
    const importedProject: ProjectSummary = {
      ...project,
      id: "33333333-3333-3333-3333-333333333333",
      name: "WuKong",
      taskKind: "import",
      durationMs: null,
      quality: null,
      modelFormat: "ply",
      registeredRatio: null,
      points3d: 618_770,
    };
    mocks.getProjectTaskDetail.mockResolvedValueOnce({
      project: importedProject,
      inputType: null,
      sourcePath: "E:\\GaussianSplatting\\WuKong\\wukong_final.ply",
      projectsRoot: "E:\\Projects",
      plannerEnabled: null,
      estimatedFrames: null,
      stage: null,
      progress: null,
      inputImages: null,
      registeredImages: null,
      video: null,
      imageSequence: null,
      sourceProjectId: null,
      logs: [],
      imported: {
        sourceModelPath: "E:\\GaussianSplatting\\WuKong\\wukong_final.ply",
        format: "ply",
        sourceAvailable: true,
        colmapRoot: "E:\\GaussianSplatting\\WuKong",
        sparseModelPath: "E:\\GaussianSplatting\\WuKong",
        databaseAvailable: false,
        imagesAvailable: false,
        reshootEligible: false,
        reshootReason: "缺少 database.db",
      },
    });
    await act(async () => useAppStore.getState().setProjects([importedProject]));
    await flush();

    const row = container.querySelector<HTMLElement>("#completed-task-group-content .project-row")!;
    expect(Array.from(row.querySelectorAll(".project-stats dt"), node => node.textContent)).toEqual(["PLY", "生成日期", "耗时", "档位"]);
    expect(Array.from(row.querySelectorAll(".project-stats dd"), node => node.textContent).slice(2)).toEqual(["/", "/"]);
    expect(row.querySelector(".project-stats")?.textContent).not.toContain("Gaussian 模型");

    await act(async () => row.click());
    await flush();
    const detail = container.querySelector<HTMLElement>(".project-detail-page")!;
    expect(detail.querySelector(".project-detail-stats")?.children).toHaveLength(4);
    expect(detail.querySelector(".project-configuration-detail")).not.toBeNull();
    expect(detail.querySelector(".path-picker")?.textContent).toContain("wukong_final.ply");
    expect(detail.querySelectorAll(".form-section")).toHaveLength(1);
    expect(detail.textContent).not.toContain("COLMAP");
    expect(detail.querySelector(".quality-settings")).toBeNull();
    expect(detail.querySelector(".acceleration-status")).toBeNull();
    expect(detail.querySelector(".live-process")).toBeNull();
  });

  it("sorts completed projects newest first so a fresh import appears at the top", async () => {
    const oldest = { ...project, id: "00000000-0000-0000-0000-000000000001", name: "Old", completedAt: "2026-10-15T08:00:00Z" };
    const generated = { ...project, id: "00000000-0000-0000-0000-000000000002", name: "Generated", completedAt: "2026-10-16T08:00:00Z" };
    const imported = { ...project, id: "00000000-0000-0000-0000-000000000003", name: "Imported", taskKind: "import" as const, quality: null, completedAt: "2026-10-17T08:00:00Z" };
    await act(async () => useAppStore.getState().setProjects([generated, oldest, imported]));
    await flush();

    expect(Array.from(container.querySelectorAll("#completed-task-group-content .project-title-line strong"), node => node.textContent)).toEqual(["Imported", "Generated", "Old"]);
  });

  it("keeps auto-run session-only and uses a non-button drag indicator", async () => {
    const toggle = container.querySelector<HTMLButtonElement>(".queue-toggle")!;
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    await act(async () => toggle.click());
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(window.localStorage.getItem("ooo-splat-task-workspace-v1")).not.toContain("autoRunNext");

    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    const before = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(container.querySelector(".draft-drag-handle")).toBeNull();
    expect(container.querySelector(".draft-drag-indicator")).not.toBeNull();
    expect(container.querySelector(".draft-drag-indicator")?.tagName).toBe("SPAN");
    const rows = container.querySelectorAll<HTMLElement>(".draft-row");
    await act(async () => rows[1].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", altKey: true, bubbles: true })));
    const after = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(after).toEqual([before[1], before[0]]);
    await flush();
    const persisted = JSON.parse(window.localStorage.getItem("ooo-splat-task-workspace-v1") ?? "{}");
    expect(persisted.drafts.map((draft: { ordinal: number }) => draft.ordinal)).toEqual([2, 1]);
  });

  it("starts pointer reordering as soon as a held task card moves beyond the drag threshold", async () => {
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    const before = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    const rows = container.querySelectorAll<HTMLElement>(".draft-row");
    rows.forEach((row, index) => {
      let captured = false;
      row.setPointerCapture = vi.fn(() => { captured = true; });
      row.hasPointerCapture = vi.fn(() => captured);
      row.releasePointerCapture = vi.fn(() => { captured = false; });
      row.getBoundingClientRect = () => ({ x: 0, y: index * 100, left: 0, top: index * 100, right: 400, bottom: index * 100 + 100, width: 400, height: 100, toJSON: () => ({}) });
    });
    const pointer = (type: string, x: number, y: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, clientX: x, clientY: y });
      Object.defineProperty(event, "pointerId", { value: 7 });
      return event;
    };

    await act(async () => rows[1].dispatchEvent(pointer("pointerdown", 200, 150)));
    expect(container.querySelector(".draft-row.dragging")).toBeNull();
    await act(async () => rows[1].dispatchEvent(pointer("pointermove", 202, 152)));
    expect(container.querySelector(".draft-row.dragging")).toBeNull();
    await act(async () => rows[1].dispatchEvent(pointer("pointermove", 200, 20)));
    expect(rows[1].classList.contains("dragging")).toBe(true);
    await act(async () => rows[1].dispatchEvent(pointer("pointerup", 200, 20)));

    const after = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(after).toEqual([before[1], before[0]]);
  });

  it("also starts pointer reordering after a stationary 200ms hold", async () => {
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    const before = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    const rows = container.querySelectorAll<HTMLElement>(".draft-row");
    rows.forEach((row, index) => {
      let captured = false;
      row.setPointerCapture = vi.fn(() => { captured = true; });
      row.hasPointerCapture = vi.fn(() => captured);
      row.releasePointerCapture = vi.fn(() => { captured = false; });
      row.getBoundingClientRect = () => ({ x: 0, y: index * 100, left: 0, top: index * 100, right: 400, bottom: index * 100 + 100, width: 400, height: 100, toJSON: () => ({}) });
    });
    const pointer = (type: string, x: number, y: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, clientX: x, clientY: y });
      Object.defineProperty(event, "pointerId", { value: 8 });
      return event;
    };

    vi.useFakeTimers();
    try {
      await act(async () => rows[1].dispatchEvent(pointer("pointerdown", 200, 150)));
      await act(async () => { vi.advanceTimersByTime(199); });
      expect(container.querySelector(".draft-row.dragging")).toBeNull();
      await act(async () => { vi.advanceTimersByTime(1); });
      expect(rows[1].classList.contains("dragging")).toBe(true);
      await act(async () => {
        rows[1].dispatchEvent(pointer("pointermove", 200, 20));
        rows[1].dispatchEvent(pointer("pointerup", 200, 20));
      });
    } finally {
      vi.useRealTimers();
    }

    const after = Array.from(container.querySelectorAll(".draft-row strong"), (node) => node.textContent);
    expect(after).toEqual([before[1], before[0]]);
  });

  it("accepts one native dropped video inside the material picker and reuses analysis", async () => {
    mocks.classifyDroppedInput.mockResolvedValueOnce({ inputType: "video" });
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, estimatedFrames: 360, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "drop" },
    });
    const zone = container.querySelector<HTMLElement>(".material-drop-zone")!;
    zone.getBoundingClientRect = () => ({ x: 10, y: 10, left: 10, top: 10, right: 410, bottom: 90, width: 400, height: 80, toJSON: () => ({}) });
    const handler = mocks.onInputDragDrop.mock.calls.at(-1)?.[0] as (event:
      | { type: "enter" | "over" | "drop"; paths: string[]; x: number; y: number }
      | { type: "leave"; paths: [] }) => void;

    await act(async () => handler({ type: "enter", paths: ["E:\\素材\\测试 clip.mov"], x: 20, y: 20 }));
    expect(container.querySelector(".material-drop-prompt")?.textContent).toContain("松开以导入素材");
    expect(container.querySelector(".material-input-content")?.getAttribute("aria-hidden")).toBe("true");
    await act(async () => handler({ type: "leave", paths: [] }));
    expect(container.querySelector(".material-empty")).not.toBeNull();
    expect(container.querySelector(".material-input-content")?.hasAttribute("aria-hidden")).toBe(false);

    await act(async () => handler({ type: "enter", paths: ["E:\\素材\\测试 clip.mov"], x: 20, y: 20 }));
    await act(async () => handler({ type: "drop", paths: ["E:\\素材\\测试 clip.mov"], x: 20, y: 20 }));
    await flush();

    expect(mocks.classifyMaterialPaths).toHaveBeenCalledWith(["E:\\素材\\测试 clip.mov"]);
    expect(mocks.probeAndPlan).toHaveBeenCalledWith("E:\\素材\\测试 clip.mov", "balanced", true, undefined, expect.any(Function), { version: 7, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps:null });
    expect(container.querySelector(".draft-row .project-path")?.textContent).toContain("测试 clip.mov");
  });

  it("serializes rapid starts and automatically runs only the immediate next ready draft", async () => {
    const probe = {
      inputType: "video" as const,
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low" as const, sampleCount: 0, basis: "queue" },
    };
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\first.mp4");
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Media\\second-images");
    mocks.probeAndPlan.mockImplementation(async (path: string) => path.endsWith("second-images") ? {
      ...probe,
      inputType: "images",
      video: null,
      imageSequence: { imageCount: 29, width: 1920, height: 1080, hasAlpha: false, requiresLargeSequenceConfirmation: false },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 29 },
    } : probe);
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-toolbar summary")?.click());
    const imageOption = [...container.querySelectorAll(".material-input-menu button")].find((button) => button.textContent?.includes("添加文件夹"));
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();

    const completedTaskIds: string[] = [];
    let activeStarts = 0;
    let maximumActiveStarts = 0;
    mocks.startPipeline.mockImplementation(async (_path, _quality, _root, _planner, workspaceTaskId: string) => {
      activeStarts += 1;
      maximumActiveStarts = Math.max(maximumActiveStarts, activeStarts);
      await Promise.resolve();
      activeStarts -= 1;
      completedTaskIds.push(workspaceTaskId);
      const suffix = completedTaskIds.length === 1 ? "111111111111" : "222222222222";
      return {
        projectId: `22222222-2222-4222-8222-${suffix}`,
        projectPath: `E:\\Projects\\${suffix}`,
        finalPly: `E:\\Projects\\${suffix}\\final.ply`,
        fileSize: 10, splatCount: 10, inputImages: 24, registeredImages: 24,
        registeredRatio: 1, points3d: 10, durationMs: 10, completedAt: "2026-10-04T00:00:00Z",
        warning: null, logsDirectory: `E:\\Projects\\${suffix}\\logs`,
      };
    });
    mocks.getProjectOverview.mockImplementation(async () => ({
      projectsRoot: "E:\\Projects",
      projects: completedTaskIds.map((workspaceTaskId, index) => ({
        ...project,
        id: index === 0 ? "22222222-2222-4222-8222-111111111111" : "22222222-2222-4222-8222-222222222222",
        name: `queued-${index}`,
        workspaceTaskId,
      })),
    }));

    await act(async () => container.querySelectorAll<HTMLElement>(".draft-row")[0].click());
    await act(async () => container.querySelector<HTMLButtonElement>(".queue-toggle")?.click());
    const start = container.querySelector<HTMLButtonElement>(".primary-action")!;
    await act(async () => {
      start.click();
      start.click();
    });
    await flush();
    await flush();
    await flush();

    expect(container.querySelector('.preflight-dialog')?.textContent).toContain('29');
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    await act(async()=>container.querySelector<HTMLInputElement>('.preflight-accept input')!.click());
    await act(async()=>container.querySelector<HTMLButtonElement>('.preflight-dialog .primary-action')!.click());await flush();await flush();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(2);
    expect(mocks.startPipeline.mock.calls.map((call) => call[0])).toEqual(["E:\\Media\\first.mp4", "E:\\Media\\second-images"]);
    expect(mocks.confirmSmallImageSequence).not.toHaveBeenCalled();
    expect(maximumActiveStarts).toBe(1);
    expect(container.querySelector<HTMLButtonElement>(".queue-toggle")?.getAttribute("aria-checked")).toBe("false");
  });

  it("keeps another draft's media summary and planner toggle independent while a task is running", async () => {
    const probe = {
      inputType: "video" as const,
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low" as const, sampleCount: 0, basis: "draft isolation" },
    };
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\running.mp4").mockResolvedValueOnce("E:\\Media\\next.mp4");
    mocks.probeAndPlan.mockResolvedValue(probe);
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    await act(async () => container.querySelectorAll<HTMLElement>(".draft-row")[0]?.click());

    let finishRun!: (value: Awaited<ReturnType<typeof mocks.startPipeline>>) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise((resolve) => { finishRun = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await act(async () => Promise.resolve());

    await act(async () => container.querySelectorAll<HTMLElement>(".draft-row")[1]?.click());
    const planner = container.querySelector<HTMLButtonElement>(".planner-switch")!;
    expect(planner.disabled).toBe(false);
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();
    expect(container.querySelector(".source-metrics")).toBeNull();
    expect(container.querySelector(".material-inspector .media-source-metadata")?.textContent).toContain("1920 × 1080");
    expect(container.querySelector(".material-row-meta")?.textContent).toBe("普通视频");
    await act(async () => planner.click());
    await flush();
    expect(planner.getAttribute("aria-checked")).toBe("false");
    expect(mocks.probeAndPlan).toHaveBeenLastCalledWith("E:\\Media\\next.mp4", "balanced", true, undefined, expect.any(Function), { version: 7, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps:null });
    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    expect(container.querySelector<HTMLButtonElement>(".planner-switch")?.getAttribute("aria-checked")).toBe("true");

    await act(async () => finishRun({
      projectId: project.id,
      projectPath: project.projectPath,
      finalPly: project.finalPly!,
      fileSize: project.fileSize!,
      splatCount: project.splatCount!,
      inputImages: 24,
      registeredImages: 24,
      registeredRatio: 1,
      points3d: 10,
      durationMs: 10,
      completedAt: project.completedAt!,
      warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    }));
    await flush();
  });

  it("keeps another project's historical detail isolated from the active draft", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\running.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "detail isolation" },
    });
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();

    let finishRun!: (value: Awaited<ReturnType<typeof mocks.startPipeline>>) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise((resolve) => { finishRun = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await act(async () => Promise.resolve());
    await act(async () => container.querySelector<HTMLElement>("#completed-task-group-content .project-row")?.click());
    await flush();

    expect(container.querySelector(".project-detail-stats")).not.toBeNull();
    expect(container.querySelector(".cancel-action")).toBeNull();
    expect(container.querySelector(".live-process:not(.historical)")).toBeNull();

    await act(async () => finishRun({
      projectId: project.id,
      projectPath: project.projectPath,
      finalPly: project.finalPly!,
      fileSize: project.fileSize!,
      splatCount: project.splatCount!,
      inputImages: 24,
      registeredImages: 24,
      registeredRatio: 1,
      points3d: 10,
      durationMs: 10,
      completedAt: project.completedAt!,
      warning: null,
      logsDirectory: `${project.projectPath}\\logs`,
    }));
    await flush();
  });

  it("promotes a successful task even when the project overview refresh fails", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\successful.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "refresh failure" },
    });
    const completedResult = {
      projectId: "33333333-3333-3333-3333-333333333333",
      projectPath: "E:\\Projects\\新完成项目",
      finalPly: "E:\\Projects\\新完成项目\\final.ply",
      fileSize: 12_345,
      splatCount: 678,
      inputImages: 24,
      registeredImages: 23,
      registeredRatio: 23 / 24,
      points3d: 456,
      durationMs: 10,
      completedAt: "2026-10-06T01:00:00Z",
      warning: null,
      logsDirectory: "E:\\Projects\\新完成项目\\logs",
    };
    mocks.startPipeline.mockResolvedValueOnce(completedResult);
    mocks.getProjectOverview.mockRejectedValueOnce(new Error("project-index busy"));

    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();

    expect(useAppStore.getState().phase).toBe("completed");
    expect(useAppStore.getState().projects.some((item) => item.id === completedResult.projectId)).toBe(true);
    expect(container.querySelector(".pane-header h1")?.textContent).toBe("新完成项目");
    expect(container.querySelector(".project-detail-page .inline-error")?.textContent).toContain("任务已完成，但刷新项目列表失败");
    expect(container.querySelector(".project-detail-page .inline-error")?.textContent).toContain("project-index busy");
    expect(mocks.cancelPipeline).not.toHaveBeenCalled();
  });

  it("ignores a late historical detail response after the active task completes", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\running-with-history.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 12, width: 1920, height: 1080, fps: 30, totalFrames: 360, codec: "h264", rotation: 0, hasAlpha: false, pixelFormat: "yuv420p" },
      imageSequence: null,
      plan: { retentionRatio: 1, samplingFps: 2, estimatedFrames: 24 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "late history" },
    });
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();

    let finishRun!: (value: unknown) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise((resolve) => { finishRun = resolve; }));
    let finishDetail!: (value: unknown) => void;
    mocks.getProjectTaskDetail.mockImplementationOnce(() => new Promise((resolve) => { finishDetail = resolve; }));
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await act(async () => Promise.resolve());
    await act(async () => container.querySelector<HTMLElement>("#completed-task-group-content .project-row")?.click());
    expect(container.querySelector(".task-detail-loading")).not.toBeNull();

    const completedResult = {
      projectId: "44444444-4444-4444-4444-444444444444",
      projectPath: "E:\\Projects\\并发完成项目",
      finalPly: "E:\\Projects\\并发完成项目\\final.ply",
      fileSize: 42_000,
      splatCount: 1_234,
      inputImages: 24,
      registeredImages: 24,
      registeredRatio: 1,
      points3d: 789,
      durationMs: 20,
      completedAt: "2026-10-06T01:30:00Z",
      warning: null,
      logsDirectory: "E:\\Projects\\并发完成项目\\logs",
    };
    await act(async () => finishRun(completedResult));
    await flush();
    expect(container.querySelector(".pane-header h1")?.textContent).toBe("并发完成项目");

    await act(async () => finishDetail({
      project,
      inputType: "video",
      sourcePath: "E:\\Capture\\input.mp4",
      projectsRoot: "E:\\Projects",
      plannerEnabled: true,
      estimatedFrames: 320,
      stage: "completed",
      progress: 100,
      inputImages: 100,
      registeredImages: 90,
      video: null,
      imageSequence: null,
      sourceProjectId: null,
      logs: [],
    }));
    await flush();

    expect(container.querySelector(".pane-header h1")?.textContent).toBe("并发完成项目");
    expect(useAppStore.getState().phase).toBe("completed");
    expect(mocks.cancelPipeline).not.toHaveBeenCalled();
  });

  it("collapses task groups, persists the preference, and expands new tasks when adding one", async () => {
    const newTasksToggle = container.querySelector<HTMLButtonElement>('[aria-controls="new-task-group-content"]')!;
    await act(async () => newTasksToggle.click());

    expect(newTasksToggle.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector<HTMLElement>("#new-task-group-content")?.hidden).toBe(true);
    expect(JSON.parse(window.localStorage.getItem("ooo-splat-task-groups-v1") ?? "{}").new).toBe(false);

    await act(async () => container.querySelector<HTMLButtonElement>(".group-add-action")?.click());
    expect(newTasksToggle.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector<HTMLElement>("#new-task-group-content")?.hidden).toBe(false);
  });

  it("offers a reshoot entry on a completed project and creates an incremental reshoot task", async () => {
    // Selected by class rather than by label: the row is translated, so asserting
    // on text would couple this test to the active interface language.
    const reshootButton = container.querySelector<HTMLButtonElement>(".reshoot-link");
    expect(reshootButton).not.toBeNull();

    await act(async () => { reshootButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.inspectReshootSource).toHaveBeenCalledWith(project.id);
    expect(mocks.prepareGaussianPreview).not.toHaveBeenCalled();
    expect(container.querySelector(".reshoot-workflow-dialog")).toBeNull();
    expect(container.querySelector(".reshoot-task-page")).not.toBeNull();
    expect(container.querySelector(".reshoot-source-summary")).toBeNull();
    expect(container.querySelector('.reshoot-capture-tips')).toBeNull();
    expect(container.querySelector('.reshoot-task-page')?.textContent).not.toContain('让补拍画面与原素材保持足够重叠，缓慢移动并避免模糊和大量运动物体。');
    expect(container.querySelectorAll(".reshoot-task-page .quality-option:disabled")).toHaveLength(0);
    expect(container.querySelector(".reshoot-task-page .locked-setting")).toBeNull();
    expect(container.querySelector(".reshoot-task-page .quality-option.selected")).not.toBeNull();
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch")?.disabled).toBe(false);
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch")?.getAttribute("aria-checked")).toBe("true");
    expect(container.querySelector(".reshoot-task-page .planner-switch em")).toBeNull();
  });

  it("defaults automatic retry to enabled independently of legacy source optimization", async () => {
    mocks.inspectReshootSource.mockResolvedValueOnce({
      projectId: project.id,
      projectName: project.name,
      quality: project.quality,
      plannerEnabled: false,
      cameraId: 1,
      cameraModel: "SIMPLE_RADIAL",
      width: 1920,
      height: 1080,
      sourceImageCount: 100,
      eligible: true,
      reason: null,
    });

    await act(async () => container.querySelector<HTMLButtonElement>(".reshoot-link")?.click());
    await flush();

    const planner = container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch");
    expect(planner?.disabled).toBe(false);
    expect(planner?.getAttribute("aria-checked")).toBe("true");
    expect(planner?.textContent).not.toContain("继承源任务");
  });

  it("lets imported projects choose the reshoot quality and automatic optimization", async () => {
    const importedProject: ProjectSummary = {
      ...project,
      taskKind: "import",
      quality: null,
      modelFormat: "spz",
      reshootEligible: true,
    };
    mocks.inspectReshootSource.mockResolvedValueOnce({
      projectId: importedProject.id,
      projectName: importedProject.name,
      quality: "balanced",
      plannerEnabled: true,
      cameraId: 1,
      cameraModel: "SIMPLE_RADIAL",
      width: 1920,
      height: 1080,
      sourceImageCount: 100,
      eligible: true,
      reason: null,
      imported: true,
    });
    await act(async () => useAppStore.setState({ projects: [importedProject] }));
    await act(async () => container.querySelector<HTMLButtonElement>(".reshoot-link")?.click());
    await flush();

    const qualityButtons = container.querySelectorAll<HTMLButtonElement>(".reshoot-task-page .quality-option");
    expect(Array.from(qualityButtons).every(button => !button.disabled)).toBe(true);
    expect(container.querySelector(".reshoot-task-page .locked-setting")).toBeNull();
    await act(async () => qualityButtons[2].click());
    expect(qualityButtons[2].getAttribute("aria-checked")).toBe("true");
    const planner = container.querySelector<HTMLButtonElement>(".reshoot-task-page .planner-switch")!;
    expect(planner.disabled).toBe(false);
    await act(async () => planner.click());
    expect(planner.getAttribute("aria-checked")).toBe("false");
  });

  it.each([false, true].flatMap(allowFallback => ['same', 'different', 'unsure'].map(cameraRelation => ({ allowFallback, cameraRelation }))))('starts reshoot after both answers (Auto Optimize=$allowFallback, camera=$cameraRelation)', async ({ allowFallback, cameraRelation }) => {
    mocks.inspectReshootSource.mockResolvedValue({ projectId:project.id,projectName:project.name,quality:'balanced',plannerEnabled:true,cameraId:1,cameraModel:'SIMPLE_RADIAL',width:1920,height:1080,sourceImageCount:100,eligible:true,reason:null,sourceIdentity:'source-v1',projection:'perspective',warmAvailable:true });
    mocks.selectVideo.mockResolvedValue('E:/new-camera.mov');
    mocks.probeReshootInput.mockResolvedValue({ inputType:'video',imageCount:null,duration:12,preparedWidth:1280,preparedHeight:1920,estimatedFrames:30,hasAlpha:false,maskCount:0,compatible:true,incompatibilityReason:null,sourceIdentity:'input-v1',estimate:{estimatedMs:1000,lowerBoundMs:500,upperBoundMs:2000,confidence:'low',sampleCount:0,basis:''} });
    mocks.startReshootPipeline.mockImplementation(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.reshoot-link')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.reshoot-task-page .material-input-menu button:first-child')!.click()); await flush();
    if (!allowFallback) await act(async () => container.querySelector<HTMLButtonElement>('.reshoot-task-page .planner-switch')!.click());
    expect(container.querySelector<HTMLButtonElement>('.reshoot-task-page .planner-switch')?.getAttribute('aria-checked')).toBe(String(allowFallback));
    await act(async () => container.querySelector<HTMLButtonElement>('.reshoot-task-page .primary-action')!.click()); await flush();
    expect(mocks.startReshootPipeline).not.toHaveBeenCalled();
    expect(container.querySelectorAll('.preflight-dialog input:checked')).toHaveLength(0);
    await act(async () => container.querySelector<HTMLInputElement>('.preflight-dialog input[value="good"]')!.click());
    expect(container.querySelector('.preflight-dialog input[type="checkbox"]')).toBeNull();
    expect(container.querySelector('.preflight-dialog')?.textContent).not.toContain('补拍方案');
    expect(mocks.startReshootPipeline).not.toHaveBeenCalled();
    await act(async () => container.querySelector<HTMLButtonElement>('.preflight-dialog footer .primary-action')!.click()); await flush();
    expect(mocks.startReshootPipeline).not.toHaveBeenCalled();
    expect(container.querySelector('.preflight-dialog h2')?.textContent).toBe('补拍素材与原素材的拍摄设置相同吗？');
    expect(container.querySelector('.preflight-dialog input:checked')).toBeNull();
    expect(container.querySelector<HTMLButtonElement>('.preflight-dialog .primary-action')?.disabled).toBe(true);
    await act(async () => container.querySelector<HTMLInputElement>(`.preflight-dialog input[value="${cameraRelation}"]`)!.click());
    await act(async () => container.querySelector<HTMLButtonElement>('.preflight-dialog footer .primary-action')!.click()); await flush();
    expect(mocks.startReshootPipeline).toHaveBeenCalledTimes(1);
    expect(mocks.startReshootPipeline).toHaveBeenCalledWith(expect.objectContaining({plannerEnabled:true,generationPolicy:expect.objectContaining({autoRetryEnabled:allowFallback}),reshootOptions:expect.objectContaining({condition:'good',cameraRelation,allowFallback,sourceIdentity:'source-v1',inputIdentity:expect.any(String),projection:'perspective'})}));
  });

  it("accepts different reshoot resolution while keeping source integrity errors blocking",async()=>{
    mocks.selectVideo.mockResolvedValue("E:/Capture/reshoot.mov");
    mocks.probeAndPlan.mockResolvedValue({inputType:"video",video:{duration:12,fps:30,width:1080,height:1920,hasAlpha:true},imageSequence:null,plan:{estimatedFrames:180}});
    await act(async()=>container.querySelector<HTMLButtonElement>(".reshoot-link")!.click());await flush();
    await act(async()=>container.querySelector<HTMLButtonElement>(".reshoot-task-page .material-input-menu button:first-child")!.click());await flush();
    expect(container.querySelector(".material-inspector .media-source-metadata")?.textContent).toContain("1080 × 1920");
    expect(container.querySelector(".material-row-meta")?.textContent).toBe("普通视频");
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .primary-action")?.disabled).toBe(false);
    expect(container.querySelector(".reshoot-plan.incompatible")).toBeNull();
    mocks.inspectReshootSource.mockResolvedValue({projectId:project.id,eligible:false,reason:"The source reconstruction is unavailable"});
    await act(async()=>container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")!.click());await flush();
    // Changing only the camera dimensions is allowed; source failure is not.
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
  });

  it("keeps an ineligible reshoot source disabled without rendering zero camera metrics", async () => {
    mocks.inspectReshootSource.mockResolvedValue({
      projectId: project.id,
      projectName: project.name,
      quality: project.quality,
      plannerEnabled: false,
      cameraId: 0,
      cameraModel: "",
      width: 0,
      height: 0,
      sourceImageCount: 0,
      eligible: false,
      reason: "The source reconstruction is unavailable",
    });

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".reshoot-link")?.click();
    });
    await flush();

    expect(container.querySelector(".reshoot-source-summary")).toBeNull();
    expect(container.querySelector(".reshoot-task-page .inline-error")).not.toBeNull();
    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>(".reshoot-task-page .input-picker button"))
        .every((button) => button.disabled),
    ).toBe(true);
  });

  it("shows the original-media rejection inside the reshoot task", async () => {
    mocks.selectVideo.mockResolvedValue("E:\\Capture\\original.mov");
    mocks.probeReshootInput.mockRejectedValue(
      new Error("补拍时不能再次使用原素材，请选择新拍摄的视频或图片序列。"),
    );
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".reshoot-link")?.click();
    });
    await flush();
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".reshoot-task-page .material-input-menu button:first-child")?.click();
    });
    await flush();

    expect(container.querySelector(".reshoot-task-page .material-error")?.textContent).toContain(
      "补拍时不能再次使用原素材",
    );
    expect(container.querySelector<HTMLButtonElement>(".reshoot-task-page .primary-action")?.disabled).toBe(true);
  });

  it("opens a plain preview without the reshoot workflow", async () => {
    const previewButton = container.querySelector<HTMLButtonElement>(".preview-link");

    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.querySelector(".preview-workspace")).not.toBeNull();
  });

  it("defers a late startup update until preview closes without remounting the viewer", async () => {
    let resolve!: (value: unknown) => void;
    mocks.checkAppUpdate.mockReturnValueOnce(new Promise(done => { resolve = done; }));
    await act(async () => root.unmount());
    root = createRoot(container);
    await act(async () => root.render(<LanguageProvider><App /></LanguageProvider>));
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".preview-link")!.click());
    await flush();
    const viewer = container.querySelector(".preview-workspace");
    await act(async () => resolve({ currentVersion: "0.6.0", status: "updateAvailable", checkedAt: "2026-10-11T08:00:00Z", fromCache: true,
      release: { version: "0.6.1", publishedAt: "2026-10-11T08:00:00Z", notes: { "zh-CN": ["更新"], en: ["Update"] },
        downloads: { github: "https://github.com/ooolabdev/ooosplat/releases/tag/0.6.1", quark: null, quarkCode: null } } }));
    expect(container.querySelector(".preview-workspace")).toBe(viewer);
    expect(container.querySelector(".update-notice-dialog")).toBeNull();
    const back = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(button => button.textContent === "返回任务")!;
    await act(async () => back.click());
    await flush();
    expect(container.querySelector(".update-notice-dialog")).not.toBeNull();
  });

  it("keeps the reshoot entry away from unfinished projects", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    expect(container.querySelector(".reshoot-link")).toBeNull();
    expect(container.querySelector(".resume-link")).not.toBeNull();
  });

  it("switches the complete task workspace to English without reloading", async () => {
    const languageButton = container.querySelector<HTMLButtonElement>(".language-action");
    expect(languageButton?.textContent).toContain("EN");
    expect(languageButton?.title).toBe("中英文切换 / Switch language");

    await act(async () => languageButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })));

    expect(container.textContent).toContain("Create New Task");
    expect(container.textContent).toContain("Task List");
    expect(container.textContent).toContain("Start Generation");
    expect(container.textContent).toContain("Checking engines");
    expect(languageButton?.textContent).toContain("中文");
    expect(languageButton?.title).toBe("中英文切换 / Switch language");
    expect(window.localStorage.getItem("ooo-splat-language")).toBe("en");
  });

  it("shows only the selected video's Alpha channel result inside the input box", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\alpha.mov");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: {
        duration: 10,
        width: 1920,
        height: 1080,
        fps: 30,
        totalFrames: 300,
        codec: "prores",
        rotation: 0,
        pixelFormat: "yuva444p10le",
        hasAlpha: true,
      },
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: { estimatedMs: 120_000, lowerBoundMs: 60_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "video" },
    });
    await act(async () => { container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click(); });
    await flush();

    expect(container.querySelector('.material-inspector .alpha-source-status')?.getAttribute('title')).toBe("检测到 Alpha 通道");
    expect(container.querySelector('.material-inspector .alpha-source-status')?.textContent).toBe("透明通道");
    expect(container.textContent).not.toContain("将自动提取透明画面和 COLMAP Mask");
    expect(container.querySelector('.source-metrics')).toBeNull();

    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\opaque.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: {
        duration: 10,
        width: 1920,
        height: 1080,
        fps: 30,
        totalFrames: 300,
        codec: "h264",
        rotation: 0,
        pixelFormat: "yuv420p",
        hasAlpha: false,
      },
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: { estimatedMs: 120_000, lowerBoundMs: 60_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "video" },
    });
    await act(async () => { container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click(); });
    await flush();
    expect(container.querySelectorAll(".material-item")).toHaveLength(2);
    await act(async()=>container.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
    expect(container.querySelector('.alpha-source-status')).toBeNull();
    await act(async()=>container.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
    expect(container.querySelector('.alpha-source-status')?.textContent).toBe("透明通道");
    await act(async()=>container.querySelector<HTMLButtonElement>(".material-remove")!.click());await flush();
    expect(container.querySelector('.alpha-source-status')).toBeNull();
  });

  it("hides the previous live process after selecting new media but keeps new analysis notices", async () => {
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: true, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();
    act(() => useAppStore.setState({
      phase: "failed",
      progress: 64,
      progressMessage: "old task output",
      latestEvent: {
        sequence: 1, timestamp: new Date().toISOString(), kind: "log", level: "error", stage: "failed",
        engine: "system", progress: 64, stageProgress: null, indeterminate: false, message: "old task output",
        current: null, total: null, unit: null, elapsedMs: 1_000, acceleration: null,
      },
      events: [{
        sequence: 1, timestamp: new Date().toISOString(), kind: "log", level: "error", stage: "failed",
        engine: "system", progress: 64, stageProgress: null, indeterminate: false, message: "old task output",
        current: null, total: null, unit: null, elapsedMs: 1_000, acceleration: null,
      }],
    }));
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\alpha.mov");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: { duration: 10, width: 1920, height: 1080, fps: 30, totalFrames: 300, codec: "prores", rotation: 0, pixelFormat: "yuva444p10le", hasAlpha: true },
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: { estimatedMs: 120_000, lowerBoundMs: 60_000, upperBoundMs: 180_000, confidence: "low", sampleCount: 0, basis: "video" },
    });

    expect(container.querySelector(".live-process")).not.toBeNull();
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: false, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();
    await act(async () => { container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click(); });
    await flush();

    expect(container.querySelector(".live-process")).toBeNull();
    expect(container.querySelector(".alpha-source-status")).not.toBeNull();
    expect(container.textContent).not.toContain("old task output");
  });

  it("shows a new-media analysis error without restoring the previous process log", async () => {
    act(() => useAppStore.setState({
      events: [{
        sequence: 1, timestamp: new Date().toISOString(), kind: "log", level: "info", stage: "completed",
        engine: "system", progress: 100, stageProgress: 100, indeterminate: false, message: "old completed log",
        current: null, total: null, unit: null, elapsedMs: 1_000, acceleration: null,
      }],
    }));
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\broken.mov");
    mocks.probeAndPlan.mockRejectedValueOnce(new Error("Unable to read the selected media"));

    await act(async () => { container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click(); });
    await flush();

    expect(container.querySelector(".live-process")).toBeNull();
    expect(container.querySelector(".source-analysis-error")?.textContent).toContain("Unable to read the selected media");
    expect(container.querySelector(".inline-error")).toBeNull();
    expect(container.textContent).not.toContain("old completed log");
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: 'video', video: { duration: 1, width: 1920, height: 1080 }, imageSequence: null, plan: { estimatedFrames: 6 } });
    await act(async () => container.querySelector<HTMLButtonElement>('.media-retry')!.click()); await flush();
    expect(container.querySelector('.source-analysis-error')).toBeNull();
    expect(container.querySelector('.media-source-copy > strong')?.textContent).toBe('普通视频');
    expect(container.querySelector('.material-inspector .media-source-metadata')?.textContent).toContain('1920 × 1080');
    expect(container.querySelector('.media-source-status')?.textContent).not.toContain('将按');
  });

  it("offers to continue an unfinished project from its checkpoint", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续生成");
    await act(async () => { resumeButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(mocks.resumePipeline).toHaveBeenCalledWith(project.id);
    expect(mocks.estimateProjectRuntime).toHaveBeenCalledWith(project.id);
  });

  it('turns off the automatic queue when continuing, even when no engine is ready', async () => {
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click());
    await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('true');
    const unfinished = { ...project, status: 'failed' as const, finalPly: null, completedAt: null };
    await act(async () => useAppStore.setState({ projects: [unfinished], engines: [{ kind: 'brush', exists: false, canStart: false, path: 'missing', detail: '', version: null, cpuOnly: null, acceleration: null }] }));
    await act(async () => container.querySelector<HTMLElement>('#unfinished-task-group-content .project-row')!.click());
    await flush();
    const action = container.querySelector<HTMLButtonElement>('.unfinished-task-detail .primary-action')!;
    expect(action.disabled).toBe(false);
    let finish!: (result: unknown) => void;
    mocks.resumePipeline.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    await act(async () => action.click());
    await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    await act(async () => finish({ projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: 100, splatCount: 10, inputImages: 100, registeredImages: 90, registeredRatio: .9,
      points3d: 1, durationMs: 1000, completedAt: project.completedAt, warning: null, logsDirectory: 'logs' }));
    await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it("keeps a resumed project in the completed list when the first catalog refresh misses it", async () => {
    const unfinished = { ...project, status: "failed" as const, finalPly: null, completedAt: null };
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [] });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续生成");
    await act(async () => { resumeButton?.click(); });
    await flush();

    const completed = useAppStore.getState().projects.find((item) => item.id === project.id);
    expect(completed).toMatchObject({ status: "completed", finalPly: project.finalPly });
    expect(container.querySelector(`#completed-task-group-content .project-row`)).not.toBeNull();
    expect(container.querySelector(`#unfinished-task-group-content .project-row`)).toBeNull();
  });

  it("shows actionable mapper guidance and opens the validated project log folder", async () => {
    const unfinished = { ...project, status: "failed" as const, finalPly: null, completedAt: null };
    mocks.resumePipeline.mockRejectedValueOnce({
      code: "pipeline_failed",
      message: "Could not find a good initial image pair",
      failedStage: "reconstructing",
      engine: "colmap",
      failureKind: "mapper_source",
      projectId: project.id,
    });
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });

    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续生成");
    await act(async () => { resumeButton?.click(); });
    await flush();

    const dialog = container.querySelector<HTMLElement>(".failure-guidance-dialog");
    expect(dialog).not.toBeNull();
    expect(dialog?.textContent).toContain("COLMAP");
    expect(dialog?.textContent).toContain("Could not find a good initial image pair");

    await act(async () => { dialog?.querySelector<HTMLButtonElement>("button.secondary")?.click(); });
    await flush();
    expect(mocks.revealProjectLogs).toHaveBeenCalledWith(project.id);

    await act(async () => { dialog?.querySelector<HTMLButtonElement>("button.primary")?.click(); });
    await flush();
    expect(mocks.resumePipeline).toHaveBeenCalledTimes(2);
  });

  it.each([
    ["brush_out_of_memory", "trainingSplats", "brush", "训练显存不足", "Not enough GPU memory for training"],
    ["brush_resource_limit", "trainingSplats", "brush", "GPU 资源超出限制", "GPU resource limit exceeded"],
    ["colmap_crash", "reconstructing", "colmap", "COLMAP 进程异常终止", "COLMAP terminated abnormally"],
    ["mapper_storage", "reconstructing", "colmap", "重建文件访问失败", "Reconstruction file access failed"],
    ["brush_dataset", "trainingSplats", "brush", "训练数据读取失败", "Training data could not be read"],
    [undefined, "reconstructing", "colmap", "任务未能完成", "The task could not finish"],
  ])("shows versioned %s guidance without inferring a different cause from raw logs", async (failureKind, failedStage, engine, zh, en) => {
    const message = "wgpu error: Out of Memory\nAn IO error happened\nFailed to create any sparse model";
    mocks.resumePipeline.mockRejectedValueOnce({ code: "pipeline_failed", message, failedStage, engine, failureKind, classificationVersion: 1, projectId: project.id });
    await act(async () => useAppStore.setState({ projects: [{ ...project, status: "failed", finalPly: null, completedAt: null }] }));
    await act(async () => [...container.querySelectorAll("button")].find(button => button.textContent === "继续生成")?.click());
    await flush();
    expect(container.querySelector(".failure-guidance-dialog h2")?.textContent).toBe(zh);
    expect(container.querySelector(".inline-error .compact-error")?.textContent).toBe(zh);
    expect(container.querySelector(".failure-guidance-dialog pre")?.textContent).toBe(message);
    expect(container.querySelector(".failure-guidance-dialog")?.textContent).not.toContain("重新拍摄时缓慢");
    await act(async () => container.querySelector<HTMLButtonElement>(".language-action")?.click());
    expect(container.querySelector(".failure-guidance-dialog h2")?.textContent).toBe(en);
  });

  it("classifies Brush dataset read errors separately and translates the open dialog", async () => {
    const unfinished = { ...project, status: "failed" as const, finalPly: null, completedAt: null };
    mocks.resumePipeline.mockRejectedValueOnce({
      code: "pipeline_failed",
      message: "IO error while loading dataset: early eof",
      failedStage: "trainingSplats",
      engine: "brush",
      failureKind: "brush_dataset",
      projectId: project.id,
    });
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });
    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续生成");
    await act(async () => { resumeButton?.click(); });
    await flush();

    const heading = container.querySelector<HTMLElement>(".failure-guidance-dialog h2")!;
    const chineseHeading = heading.textContent;
    expect(container.querySelector(".failure-guidance-dialog")?.textContent).toContain("early eof");
    await act(async () => { container.querySelector<HTMLButtonElement>(".language-action")?.click(); });
    expect(heading.textContent).not.toBe(chineseHeading);
  });

  it("does not show failure guidance for a cancelled run", async () => {
    const unfinished = { ...project, status: "cancelled" as const, finalPly: null, completedAt: null };
    mocks.resumePipeline.mockRejectedValueOnce({ code: "cancelled", message: "cancelled", projectId: project.id });
    await act(async () => { useAppStore.setState({ projects: [unfinished] }); });
    const resumeButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "继续生成");
    await act(async () => { resumeButton?.click(); });
    await flush();
    expect(container.querySelector(".failure-guidance-dialog")).toBeNull();
  });

  it.each([false, true])('continues a failure before project creation using the dialog-captured run (stale: %s)', async stale => {
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/before-project.mov']);
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: 'video', video: { duration: 10, fps: 30, width: 800, height: 600 }, imageSequence: null, plan: { estimatedFrames: 80 } });
    mocks.startInputsPipeline.mockImplementationOnce(async (...args) => {
      throw { code: 'pipeline_failed', message: 'Preparation failed before project binding', engine: 'system',
        taskId: args[3], runId: 'failed-before-project', projectId: null };
    });
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click());
    await flush();
    const taskId = mocks.startInputsPipeline.mock.calls[0][3];
    const action = container.querySelector<HTMLButtonElement>('.failure-guidance-dialog .primary')!;
    expect(action.textContent).toBe('继续生成');
    expect(action.disabled).toBe(false);
    expect(container.querySelector<HTMLButtonElement>('.failure-guidance-dialog .secondary')!.disabled).toBe(true);
    if (stale) {
      await publishTask(sharedTask({ task_id: taskId, run_id: 'newer-run', project_id: null, project_path: null,
        status: 'failed', stage: null, revision: 2, result: null }));
      mocks.continueTaskPipeline.mockRejectedValueOnce({ code: 'STALE_RUN_ID', message: 'stale execution' });
    }
    await act(async () => action.click());
    await flush();
    expect(mocks.continueTaskPipeline).toHaveBeenCalledExactlyOnceWith(taskId, 'failed-before-project');
    expect(mocks.resumePipeline).not.toHaveBeenCalled();
    expect(mocks.startInputsPipeline).toHaveBeenCalledOnce();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    expect(container.querySelector('.failure-guidance-dialog')).toBeNull();
  });

  it("offers voluntary reporting for a generic failure even with analytics off", async () => {
    mocks.initializeTelemetry.mockResolvedValue({ analyticsEnabled: false, consentDecided: true, deliveryStatus: "configured" });
    await act(async () => { root.render(<LanguageProvider><App key="analytics-disabled" /></LanguageProvider>); }); await flush();
    mocks.resumePipeline.mockRejectedValueOnce({ code: "pipeline_failed", message: "engine failed", failedStage: "matching", engine: "colmap", projectId: project.id, failureId: "failure-token" });
    await act(async () => { useAppStore.setState({ projects: [{ ...project, status: "failed", finalPly: null }] }); });
    await act(async () => { [...container.querySelectorAll("button")].find(button => button.textContent === "继续生成")?.click(); }); await flush();
    const report = [...container.querySelectorAll("button")].find(button => button.textContent === "发送错误报告");
    expect(report?.disabled).toBe(false); expect(mocks.sendErrorReport).not.toHaveBeenCalled();
    expect(container.querySelector(".failure-guidance-dialog h2")?.textContent).toBe("任务未能完成");
    expect(mocks.setTelemetryConsent).not.toHaveBeenCalled();
  });

  it("blocks the interface while a slow cancellation is still terminating processes", async () => {
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: true, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => useAppStore.setState({ phase: "running" }));
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();

    const cancelButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "取消任务并终止所有进程");
    await act(async () => { cancelButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    expect(mocks.cancelPipeline).toHaveBeenCalledTimes(1);
    expect(container.querySelector(".cancellation-backdrop")).toBeNull();

    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 350)); });
    expect(container.querySelector(".cancellation-backdrop")).not.toBeNull();
    expect(container.textContent).toContain("正在关闭当前阶段及其子进程");

    act(() => useAppStore.setState({ phase: "cancelled" }));
    await flush();
    expect(container.querySelector(".cancellation-backdrop")).toBeNull();
  });

  it("keeps material analysis without rendering standalone time or frame estimates", async () => {
    mocks.selectVideo.mockResolvedValueOnce("E:\\Media\\estimate.mp4");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "video",
      video: {
        duration: 10,
        width: 1920,
        height: 1080,
        fps: 30,
        totalFrames: 300,
        codec: "h264",
        rotation: 0,
        pixelFormat: "yuv420p",
        hasAlpha: false,
      },
      imageSequence: null,
      plan: { retentionRatio: 0.5, samplingFps: 15, estimatedFrames: 150 },
      estimate: {
        estimatedMs: 120_000,
        lowerBoundMs: 60_000,
        upperBoundMs: 180_000,
        confidence: "medium",
        sampleCount: 3,
        basis: "本机历史任务校准",
      },
    });
    await act(async () => { container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click(); });
    await flush();

    expect(mocks.probeAndPlan).toHaveBeenCalled();
    expect(container.querySelector('.material-inspector .media-source-metadata')?.textContent).toContain('1920 × 1080');
    expect(container.querySelector('.material-row-meta')?.textContent).toBe('普通视频');
    expect(container.querySelector('.source-metrics')).toBeNull();
    expect(container.textContent).not.toContain("预计时长");
    expect(container.textContent).not.toContain("约 2 分 0 秒");
    expect(container.querySelector('[title="本机历史任务校准"]')).toBeNull();
  });

  it("does not turn a Brush heartbeat into completed training steps", async () => {
    mocks.getAppRuntimeStatus.mockResolvedValueOnce({ pipelineRunning: true, pipelineProjectId: null, pipelineWorkspaceTaskId: null, previewProjectId: null, taskAcceleration: null });
    act(() => useAppStore.setState({
      phase: "running",
      progress: 79,
      progressMessage: "Brush 训练进程仍在运行",
      latestEvent: {
        sequence: 7,
        timestamp: new Date().toISOString(),
        kind: "heartbeat",
        level: "info",
        stage: "trainingSplats",
        engine: "brush",
        progress: 79,
        stageProgress: 50,
        indeterminate: false,
        message: "Brush 训练进程仍在运行",
        current: null,
        total: 15_000,
        unit: null,
        elapsedMs: 120_000,
        acceleration: null,
      },
    }));
    act(() => window.dispatchEvent(new Event("focus")));
    await flush();

    expect(container.querySelector(".current-message")?.textContent).toBe("正在生成高斯泼溅…。此步骤可能耗时较长，请耐心等待");
    expect(container.textContent).not.toContain("7,500/15,000");
  });

  it("shows only the task panes until a completed project is opened", async () => {
    expect(container.textContent).toContain("创建新任务");
    expect(container.textContent).toContain("任务列表");
    expect(container.querySelector(".preview-workspace")).toBeNull();

    const controlPane = container.querySelector<HTMLElement>(".control-pane");
    const projectsPane = container.querySelector<HTMLElement>(".projects-pane");
    if (controlPane) controlPane.scrollTop = 48;
    if (projectsPane) projectsPane.scrollTop = 96;

    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.querySelector(".topbar")).toBeNull();
    expect(container.textContent).toContain("高斯泼溅预览");
    expect(container.textContent).not.toContain("创建新任务");

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
    expect(container.querySelector<HTMLElement>(".control-pane")?.scrollTop).toBe(48);
    expect(container.querySelector<HTMLElement>(".projects-pane")?.scrollTop).toBe(96);
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("opens another completed project while a generation task keeps running", async () => {
    await publishTask(sharedTask({
      task_id: "running-task",
      project_id: null,
      project_path: null,
      status: "running",
      stage: "trainingSplats",
      estimated_progress: 45,
      result: null,
    }));
    const runtimeReads = mocks.getAppRuntimeStatus.mock.calls.length;
    mocks.getAppRuntimeStatus.mockImplementation(() => new Promise(() => undefined));

    const previewButton = container.querySelector<HTMLButtonElement>("#completed-task-group-content .preview-link");
    expect(previewButton?.disabled).toBe(false);
    await act(async () => previewButton?.click());
    await flush();

    expect(mocks.prepareGaussianPreview).toHaveBeenCalledWith(project.id);
    expect(container.querySelector(".preview-workspace")).not.toBeNull();
    expect(mocks.getAppRuntimeStatus).toHaveBeenCalledTimes(runtimeReads);
    expect(useAppStore.getState().phase).toBe("running");
  });

  it("keeps the task workspace visible when preview preparation fails", async () => {
    mocks.prepareGaussianPreview.mockRejectedValueOnce(new Error("PLY 无法读取"));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
    expect(container.textContent).toContain("PLY 无法读取");
    expect(container.querySelector(".preview-workspace")).toBeNull();
  });

  it("restores the task workspace before preview resource release settles", async () => {
    let finishRelease: (() => void) | undefined;
    mocks.releaseGaussianPreview.mockImplementationOnce(() => new Promise<void>((resolve) => { finishRelease = resolve; }));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
    expect(container.querySelector(".preview-workspace")).toBeNull();
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);

    await act(async () => { finishRelease?.(); });
    await flush();
    expect(useGaussianTransformStore.getState().descriptor).toBeNull();
  });

  it("does not revoke the PLY permission before the renderer is disposed", async () => {
    let finishDisposal: (() => void) | undefined;
    mocks.notifyPreviewDisposed.mockImplementationOnce((callback: (projectId: string, previewSessionId: number) => void, projectId: string, previewSessionId: number) => {
      finishDisposal = () => callback(projectId, previewSessionId);
    });
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();
    expect(container.textContent).toContain(project.name);
    expect(mocks.releaseGaussianPreview).not.toHaveBeenCalled();

    await act(async () => { finishDisposal?.(); });
    await flush();
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("unlocks the project after the preview disposal watchdog and ignores a late callback", async () => {
    let finishDisposal: (() => void) | undefined;
    mocks.notifyPreviewDisposed.mockImplementationOnce((callback: (projectId: string, previewSessionId: number) => void, projectId: string, previewSessionId: number) => {
      finishDisposal = () => callback(projectId, previewSessionId);
    });
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.click(); });
    await flush();

    vi.useFakeTimers();
    try {
      const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
      await act(async () => { backButton?.click(); });
      await act(async () => { await vi.advanceTimersByTimeAsync(8_001); });

      const reopenedButton = [...container.querySelectorAll<HTMLButtonElement>("button")].find((button) => button.textContent === "预览");
      expect(reopenedButton?.disabled).toBe(false);
      expect(useGaussianTransformStore.getState().descriptor).toBeNull();
      expect(mocks.releaseGaussianPreview).toHaveBeenCalledTimes(1);

      await act(async () => { finishDisposal?.(); });
      expect(mocks.releaseGaussianPreview).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the task workspace visible when preview resource release fails", async () => {
    mocks.releaseGaussianPreview.mockRejectedValueOnce(new Error("预览资源释放失败"));
    const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
    await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
    await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await flush();

    expect(container.textContent).toContain(project.name);
    expect(container.textContent).toContain("任务列表");
    expect(container.textContent).toContain("预览资源释放失败");
    expect(container.querySelector(".preview-workspace")).toBeNull();
  });

  it("can dispose and reopen the same project repeatedly", async () => {
    let session = 0;
    mocks.prepareGaussianPreview.mockImplementation(async () => ({
      projectId: project.id,
      modelPath: project.finalPly,
      assetPath: `${project.projectPath}\\work\\preview\\preview-session-${session + 1}.ply`,
      assetUrl: `http://asset.localhost/final.ply?previewSession=${++session}`,
      format: "ply",
      fileSize: project.fileSize,
      splatCount: project.splatCount,
      transform: { position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 },
    }));

    for (let cycle = 1; cycle <= 3; cycle += 1) {
      const previewButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "预览");
      await act(async () => { previewButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
      await flush();
      expect(useGaussianTransformStore.getState().descriptor?.assetUrl).toContain(`previewSession=${cycle}`);

      const backButton = [...container.querySelectorAll("button")].find((button) => button.textContent === "返回任务");
      await act(async () => { backButton?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
      await flush();
      expect(container.textContent).toContain(project.name);
      expect(useGaussianTransformStore.getState().descriptor).toBeNull();
    }

    expect(mocks.prepareGaussianPreview).toHaveBeenCalledTimes(3);
    expect(mocks.releaseGaussianPreview).toHaveBeenCalledTimes(3);
  });

  it('publishes image type independently of the deferred estimate and reuses analysis on quality change',async()=>{
    const projectionDetection={sourceIdentity:'photos-A',confirmedErp:false,requiresChoice:true,supportedErp:true,reason:'',previewJpeg:[]};
    const metadata={inputType:'images',video:null,imageSequence:{imageCount:94,width:4096,height:2048,hasAlpha:false,requiresLargeSequenceConfirmation:false},projectionDetection,plan:{estimatedFrames:94}};
    let finish!:(v:unknown)=>void;
    mocks.estimateMaterialInputs.mockImplementationOnce(()=>new Promise(resolve=>{finish=resolve;}));
    mocks.selectImageSequence.mockResolvedValueOnce('E:/Ballintoy/Input');
    mocks.probeAndPlan.mockResolvedValue(metadata);
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:nth-child(2)')!.click());await flush();
    expect(container.querySelector('.media-source-status.checking')).toBeNull();
    expect(container.querySelector<HTMLSelectElement>('select.video-type-select')?.getAttribute('aria-label')).toBe('请选择图片类型');
    await act(async()=>container.querySelector<HTMLButtonElement>('.queue-toggle')!.click());await flush();
    expect(container.querySelector('.queue-input-notice')?.textContent).toBe('请先为「Input」选择图片类型，再开启自动执行。');
    await act(async()=>{const select=container.querySelector<HTMLSelectElement>('select.video-type-select')!;select.value='equirectangular';select.dispatchEvent(new Event('change',{bubbles:true}));});
    expect(container.querySelector<HTMLButtonElement>('.primary-action')?.disabled).toBe(false);
    await act(async()=>container.querySelector<HTMLButtonElement>('.quality-option:not(.selected)')!.click());await flush();
    expect(mocks.probeAndPlan.mock.calls.at(-1)?.[3]).toMatchObject({imageSequence:metadata.imageSequence,projectionDetection});
    expect(container.querySelector<HTMLSelectElement>('select.video-type-select')?.value).toBe('equirectangular');
    await act(async()=>finish({estimatedMs:1}));await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
  });

  it("uses two empty-state input actions and opens folders without a type switch",async()=>{
    expect(container.querySelector('[aria-label="选择输入素材类型"]')).toBeNull();
    expect(container.querySelector(".material-add-menu")).toBeNull();
    expect([...container.querySelectorAll(".material-input-menu button")].map(b=>b.textContent)).toEqual(["添加文件","添加文件夹"]);
    mocks.selectImageSequence.mockResolvedValueOnce("E:/Photos/object");
    mocks.probeAndPlan.mockResolvedValueOnce({inputType:"images",video:null,imageSequence:{imageCount:24,width:1920,height:1080,hasAlpha:true},plan:{estimatedFrames:24}});
    await act(async()=>container.querySelector<HTMLButtonElement>(".material-input-menu button:nth-child(2)")!.click());await flush();
    expect(mocks.selectMaterialPaths).toHaveBeenCalledWith(true);
    expect(container.querySelectorAll(".material-item")).toHaveLength(1);
    expect(container.querySelector(".material-row-copy")?.textContent).toContain("object");
    expect(container.querySelector(".material-row-main > svg")).not.toBeNull();
    expect(container.textContent).toContain("24 张图片");
    expect(container.querySelector(".frame-sampling-control")).toBeNull();
  });

  it("shows the image-order question inline, not in the generation dialog, above 500 images", async () => {
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\large");
    mocks.probeAndPlan.mockResolvedValueOnce({
        inputType: "images",
        video: null,
        imageSequence: {
          imageCount: 501,
          width: 1920,
          height: 1080,
          hasAlpha: false,
          requiresLargeSequenceConfirmation: true,
        },
        plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 501 },
        estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "test" },
    });
    mocks.confirmLargeImageSequence.mockResolvedValueOnce(false);
    await act(async () => { container.querySelector<HTMLButtonElement>('.material-input-toolbar summary')?.click(); });
    const imageOption = [...container.querySelectorAll(".material-input-menu button")].find((button) => button.textContent?.includes("添加文件夹"));
    await act(async () => { imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
    await act(async () => { container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click(); });
    await flush();

    expect(container.querySelector('.material-order-question')?.textContent).toContain('501');
    expect(container.querySelector('.material-order-question')?.textContent).toContain('未按顺序排列时，生成可能耗时更长');
    expect(container.querySelector('.material-order-question')?.textContent).toContain('若为微距合成图片');
    expect(container.querySelectorAll('.material-order-question input[type="radio"]')).toHaveLength(2);
    expect(container.querySelector<HTMLButtonElement>('.primary-action')?.disabled).toBe(true);
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.confirmLargeImageSequence).not.toHaveBeenCalled();
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it.each([
    [100, 'perspective', 'exhaustive'],
    [101, 'perspective', 'sequential'],
    [500, 'equirectangular', 'exhaustive'],
    [501, 'equirectangular', 'sequential'],
  ] as const)('passes the confirmed strategy for %i %s images (%s)', async (count, projection, strategy) => {
    mocks.selectVideo.mockResolvedValueOnce('E:/photos');
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: 'images', video: null, imageSequence: { imageCount: count, width: 200, height: 100, hasAlpha: false, requiresLargeSequenceConfirmation: count > 500 }, plan: { estimatedFrames: count }, projectionDetection: { sourceIdentity: 'photos-identity', confirmedErp: projection === 'equirectangular', requiresChoice: false, supportedErp: projection === 'equirectangular', reason: '', previewJpeg: [] } });
    mocks.startPipeline.mockImplementation(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    if (count > 100) {
      expect(container.querySelector<HTMLButtonElement>('.primary-action')!.disabled).toBe(true);
      expect(container.querySelector('.material-order-question input:checked')).toBeNull();
      expect(mocks.startPipeline).not.toHaveBeenCalled();
      await act(async () => container.querySelector<HTMLInputElement>(`input[value="${strategy}"]`)!.click());
    }
    await act(async () => { container.querySelector<HTMLButtonElement>('.primary-action')!.click(); container.querySelector<HTMLButtonElement>('.primary-action')!.click(); }); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    expect(mocks.startPipeline.mock.calls[0][7]).toBe(strategy);
    expect(container.querySelector('.preflight-dialog')).toBeNull();
  });

  it.each(['unanswered', 'sequential', 'exhaustive'] as const)('waits on queued inline image order without skipping (%s)', async answer => {
    const video = { inputType: 'video', video: { duration: 1, width: 200, height: 100 }, imageSequence: null, plan: { estimatedFrames: 6 } };
    const images = { inputType: 'images', video: null, imageSequence: { imageCount: 101, width: 200, height: 100, hasAlpha: false, requiresLargeSequenceConfirmation: false }, plan: { estimatedFrames: 101 } };
    mocks.probeAndPlan.mockResolvedValueOnce(video).mockResolvedValueOnce(images).mockResolvedValueOnce(video);
    mocks.selectVideo.mockResolvedValueOnce('E:/running.mp4').mockResolvedValueOnce('E:/photos').mockResolvedValueOnce('E:/later.mp4');
    let finishRun!: (result: unknown) => void;
    mocks.startPipeline.mockImplementationOnce(() => new Promise(resolve => { finishRun = resolve; })).mockImplementationOnce(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click()); await flush();
    const workspaceTaskId = mocks.startPipeline.mock.calls[0][4];
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: 'E:/Projects', projects: [{ ...project, workspaceTaskId }] });
    await act(async () => container.querySelectorAll<HTMLElement>('.draft-row')[1].click());
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.group-add-action')!.click());
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click()); await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    await act(async () => finishRun({ projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly, fileSize: 10, splatCount: 10, inputImages: 6, registeredImages: 6, registeredRatio: 1, points3d: 10, durationMs: 1, completedAt: project.completedAt, warning: null, logsDirectory: 'E:/logs' })); await flush();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(container.querySelector('.material-order-question')?.textContent).toContain('当前共 101 张图片');
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    if (answer === 'unanswered') {
      expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
      expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
    } else {
      await act(async () => container.querySelector<HTMLInputElement>(`input[value="${answer}"]`)!.click());
      expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
      await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click()); await flush();
      expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('true');
    }
  });

  it("warns for fewer than 30 images, cancels the queue, and starts only after explicit confirmation", async () => {
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\small");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "images",
      video: null,
      imageSequence: { imageCount: 29, width: 1920, height: 1080, hasAlpha: false, requiresLargeSequenceConfirmation: false },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 29 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "test" },
    });
    mocks.confirmSmallImageSequence
      .mockResolvedValueOnce(false)
      .mockRejectedValueOnce(new Error("dialog failed"))
      .mockResolvedValueOnce(true);
    mocks.startPipeline.mockResolvedValueOnce({
      projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly,
      fileSize: project.fileSize, splatCount: project.splatCount, inputImages: 29,
      registeredImages: 29, registeredRatio: 1, points3d: 10, durationMs: 10,
      completedAt: project.completedAt, warning: null, logsDirectory: `${project.projectPath}\\logs`,
    });

    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-toolbar summary")?.click());
    const imageOption = [...container.querySelectorAll(".material-input-menu button")].find((button) => button.textContent?.includes("添加文件夹"));
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();

    await act(async () => container.querySelector<HTMLButtonElement>(".queue-toggle")?.click());
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(container.querySelector('.preflight-dialog')?.textContent).toContain('29');
    expect(mocks.startPipeline).not.toHaveBeenCalled();
    await act(async () => container.querySelector<HTMLButtonElement>('.preflight-dialog footer > button')!.click()); await flush();
    expect(container.querySelector<HTMLButtonElement>(".queue-toggle")?.getAttribute("aria-checked")).toBe("false");

    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(container.querySelector('.preflight-dialog')).not.toBeNull();
    expect(mocks.startPipeline).not.toHaveBeenCalled();
    await act(async () => container.querySelector<HTMLInputElement>('.preflight-accept input')!.click());
    await act(async () => container.querySelector<HTMLButtonElement>('.preflight-dialog .primary-action')!.click());
    await flush();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
  });

  it("starts an image task with exactly 30 images without the small-sequence warning", async () => {
    mocks.selectImageSequence.mockResolvedValueOnce("E:\\Photos\\thirty");
    mocks.probeAndPlan.mockResolvedValueOnce({
      inputType: "images",
      video: null,
      imageSequence: { imageCount: 30, width: 1920, height: 1080, hasAlpha: false, requiresLargeSequenceConfirmation: false },
      plan: { retentionRatio: 1, samplingFps: 0, estimatedFrames: 30 },
      estimate: { estimatedMs: 1, lowerBoundMs: 1, upperBoundMs: 2, confidence: "low", sampleCount: 0, basis: "test" },
    });
    mocks.startPipeline.mockRejectedValueOnce(new Error("stop after launch"));
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-toolbar summary")?.click());
    const imageOption = [...container.querySelectorAll(".material-input-menu button")].find((button) => button.textContent?.includes("添加文件夹"));
    await act(async () => imageOption?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    await act(async () => container.querySelector<HTMLButtonElement>(".material-input-menu button:first-child")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>(".primary-action")?.click());
    await flush();
    expect(mocks.confirmSmallImageSequence).not.toHaveBeenCalled();
    expect(mocks.startPipeline).toHaveBeenCalledTimes(1);
  });

  it('adds a video and a loose image set together without replacing existing inputs',async()=>{
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/clip.mov','E:/1.jpg','F:/2.png']);
    const items=[{id:'clip',source:{kind:'video',path:'E:/clip.mov'},projection:'auto',imageMatchingStrategy:'sequential'},{id:'loose',source:{kind:'images',paths:['E:/1.jpg','F:/2.png']},projection:'auto',imageMatchingStrategy:'exhaustive'}];
    mocks.classifyMaterialPaths.mockResolvedValueOnce(items);
    mocks.inspectMaterialInputs.mockResolvedValueOnce({materials:items.map(i=>({id:i.id,identity:i.id+'-id',video:i.id==='clip'?{width:1920,height:1080,fps:24}:null,imageSequence:i.id==='loose'?{imageCount:2,width:800,height:600}:null,plan:i.id==='clip'?{estimatedFrames:80}:null,projectionDetection:{sourceIdentity:i.id+'-id',requiresChoice:false,supportedErp:false,confirmedErp:false,reason:'',previewJpeg:[]}})),colmap:null,errors:{}});
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    expect(mocks.classifyMaterialPaths).toHaveBeenCalledWith(['E:/clip.mov','E:/1.jpg','F:/2.png']);
    expect(container.querySelectorAll('.material-item')).toHaveLength(2);
    await act(async()=>container.querySelectorAll<HTMLButtonElement>('.material-item .material-row-main')[1].click());
    expect(container.querySelector('.material-members')?.textContent).toContain('F:/2.png');
    expect(container.querySelector('.source-metrics')).toBeNull(); // never borrow the first clip's dimensions
    expect(container.querySelector<HTMLButtonElement>('.material-input-menu button:last-child')?.disabled).toBe(false);
    const saved=JSON.parse(localStorage.getItem('ooo-splat-task-workspace-v1')!);expect(saved.drafts[0].inputs.items.map((i:any)=>i.id)).toEqual(['clip','loose']);
  });

  it('ignores an older path-classification response after a newer selection completes', async () => {
    let finishOld!: (value: unknown) => void;
    const recent = { id: 'recent', source: { kind: 'video' as const, path: 'E:/recent.mov' }, projection: 'auto' as const, imageMatchingStrategy: 'sequential' as const };
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/old.mov']).mockResolvedValueOnce(['E:/recent.mov']);
    mocks.classifyGenerationPaths.mockImplementationOnce(() => new Promise(resolve => { finishOld = resolve; }))
      .mockResolvedValueOnce({ kind: 'materials', items: [recent] });
    mocks.inspectMaterialInputs.mockResolvedValueOnce({ materials: [{ id: recent.id, identity: 'recent-id', video: { width: 1280, height: 720, fps: 30 }, imageSequence: null, plan: { estimatedFrames: 30 }, projectionDetection: { sourceIdentity: 'recent-id', requiresChoice: false, confirmedErp: false, supportedErp: false, reason: '', previewJpeg: [] } }], colmap: null, errors: {} });

    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
    await flush();
    expect(container.textContent).toContain('recent.mov');
    await act(async () => finishOld({ kind: 'materials', items: [{ ...recent, id: 'old', source: { kind: 'video', path: 'E:/old.mov' } }] }));
    await flush();
    expect(container.textContent).not.toContain('old.mov');
    expect(container.querySelectorAll('.material-item')).toHaveLength(1);
  });

  it.each([false, true])('shows a typed COLMAP rejection for a new file/folder selection (folder=%s)', async directory => {
    mocks.selectMaterialPaths.mockResolvedValueOnce([directory ? 'E:/sparse' : 'E:/database.db']);
    mocks.classifyGenerationPaths.mockRejectedValueOnce({code:'UNSUPPORTED_INPUT',reason:'colmap_disabled'});
    await act(async()=>container.querySelector<HTMLButtonElement>(`.material-input-menu button:${directory?'last':'first'}-child`)!.click());await flush();
    expect(container.querySelector('[role="alertdialog"]')?.textContent).toContain('暂不支持导入 COLMAP 文件或文件夹。');
    expect(container.querySelector('.material-empty')).not.toBeNull();
    expect(container.querySelector('.colmap-inspector,.colmap-discovery')).toBeNull();
    expect(container.querySelector('.draft-row')?.textContent).not.toContain('COLMAP 处理失败');
    expect(mocks.inspectMaterialInputs).not.toHaveBeenCalled();
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
    await act(async()=>container.querySelector<HTMLElement>('.unsupported-input-dialog')!.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true})));
    await flush();
    expect(container.querySelector('[role="alertdialog"]')).toBeNull();
  });

  it.each(['colmap', 'colmapDiscovery'] as const)('rejects %s immediately during reshoot selection without replacing existing materials', async kind => {
    await act(async () => container.querySelector<HTMLButtonElement>('.reshoot-link')!.click());
    await flush();
    const existing = { id: 'existing', source: { kind: 'video', path: 'E:/new-capture.mov' }, projection: 'perspective', imageMatchingStrategy: 'sequential' };
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/new-capture.mov']);
    mocks.classifyGenerationPaths.mockResolvedValueOnce({ kind: 'materials', items: [existing] });
    mocks.inspectMaterialInputs.mockResolvedValue({ materials: [{ id: existing.id, identity: 'new-id', video: { width: 1920, height: 1080, fps: 30, duration: 2 }, imageSequence: null, plan: { estimatedFrames: 30 }, projectionDetection: { sourceIdentity: 'new-id', confirmedErp: false, supportedErp: false, requiresChoice: false, reason: '', previewJpeg: [] } }], colmap: null, errors: {} });
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
    await flush();
    expect(container.querySelectorAll('.material-item')).toHaveLength(1);
    const inspectCalls = mocks.inspectMaterialInputs.mock.calls.length;
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/sparse']);
    mocks.classifyGenerationPaths.mockResolvedValueOnce(kind === 'colmap'
      ? { kind, input: { model: 'E:/sparse/0', images: 'E:/images' } }
      : { kind, discovery: { models: [{ model: 'E:/sparse/0', imageCandidates: [] }] } });
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
    await flush();
    expect(container.querySelector('[role="alertdialog"]')?.textContent).toContain('暂不支持导入 COLMAP 文件或文件夹。');
    expect(container.querySelectorAll('.material-item')).toHaveLength(1);
    expect(container.querySelector('.material-item')?.textContent).toContain('new-capture.mov');
    expect(container.querySelector('.colmap-inspector,.colmap-discovery')).toBeNull();
    expect(mocks.inspectMaterialInputs).toHaveBeenCalledTimes(inspectCalls);
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
  });

  it('rejects a mixed unsupported addition atomically and retains the existing source and answers', async () => {
    mocks.selectVideo.mockResolvedValueOnce('E:/original.mov');
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    const before=localStorage.getItem('ooo-splat-task-workspace-v1');
    const calls=mocks.inspectMaterialInputs.mock.calls.length;
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/another.mov','E:/unknown.zip']);
    mocks.classifyGenerationPaths.mockRejectedValueOnce({code:'UNSUPPORTED_INPUT',reason:'unsupported_input'});
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    expect(container.querySelector('[role="alertdialog"]')?.textContent).toContain('不支持所选文件或文件夹，请选择当前入口支持的文件类型。');
    expect(container.querySelectorAll('.material-item')).toHaveLength(1);
    expect(container.querySelector('.material-item')?.textContent).toContain('original.mov');
    expect(localStorage.getItem('ooo-splat-task-workspace-v1')).toBe(before);
    expect(mocks.inspectMaterialInputs).toHaveBeenCalledTimes(calls);
  });

  it.each(['file','folder'] as const)('uses the same unsupported popup from Completed import (%s), without stacked dialogs', async kind => {
    mocks.selectGaussianImportSource.mockResolvedValueOnce({selectionId:'unsupported-selection',displayName:'database.db'});
    mocks.probeGaussianImport.mockRejectedValueOnce({code:'UNSUPPORTED_INPUT',reason:'colmap_disabled'});
    await act(async()=>container.querySelector<HTMLButtonElement>('.completed-import-actions .group-add-action')!.click());
    await act(async()=>container.querySelector<HTMLButtonElement>(`.completed-import-menu button:${kind==='file'?'first':'last'}-child`)!.click());await flush();
    expect(mocks.selectGaussianImportSource).toHaveBeenCalledWith(kind);
    expect(container.querySelector('.gaussian-import-dialog')).toBeNull();
    expect(container.querySelectorAll('[aria-modal="true"]')).toHaveLength(1);
    expect(container.querySelector('[role="alertdialog"]')?.textContent).toContain('暂不支持导入 COLMAP 文件或文件夹。');
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
  });

  it('rejects a native unsupported drop and ignores drops while its notice is open', async () => {
    const zone=container.querySelector<HTMLElement>('.material-drop-zone')!;
    zone.getBoundingClientRect=()=>({x:10,y:10,left:10,top:10,right:410,bottom:90,width:400,height:80,toJSON:()=>({})});
    const handler=mocks.onInputDragDrop.mock.calls.at(-1)![0];
    mocks.classifyGenerationPaths.mockRejectedValueOnce({code:'UNSUPPORTED_INPUT',reason:'unsupported_input'});
    await act(async()=>handler({type:'drop',paths:['E:/unknown.zip'],x:20,y:20}));await flush();
    const count=mocks.classifyGenerationPaths.mock.calls.length;
    expect(container.querySelectorAll('[role="alertdialog"]')).toHaveLength(1);
    await act(async()=>handler({type:'drop',paths:['E:/other.zip'],x:20,y:20}));await flush();
    expect(mocks.classifyGenerationPaths).toHaveBeenCalledTimes(count);
    expect(container.querySelectorAll('.material-item')).toHaveLength(0);
  });

  it('ignores an old rejection after a newer source selection succeeds', async () => {
    let rejectOld!: (error: unknown) => void;
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/database.db']).mockResolvedValueOnce(['E:/current.mov']);
    mocks.classifyGenerationPaths.mockImplementationOnce(()=>new Promise((_,reject)=>{rejectOld=reject;}));
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    await act(async()=>rejectOld({code:'UNSUPPORTED_INPUT',reason:'colmap_disabled'}));await flush();
    expect(container.querySelector('[role="alertdialog"]')).toBeNull();
    expect(container.querySelector('.material-item')?.textContent).toContain('current.mov');
  });

  it('does not revive a removed input from a late analysis rejection', async () => {
    let rejectAnalysis!: (error: unknown) => void;
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/pending.mov']);
    mocks.inspectMaterialInputs.mockImplementationOnce(()=>new Promise((_,reject)=>{rejectAnalysis=reject;}));
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-remove')!.click());await flush();
    await act(async()=>rejectAnalysis({code:'UNSUPPORTED_INPUT',reason:'unsupported_input'}));await flush();
    expect(container.querySelector('[role="alertdialog"]')).toBeNull();
    expect(container.querySelector('.material-empty')).not.toBeNull();
  });

  it('does not show an old native-picker rejection in a newly selected draft', async () => {
    let rejectPicker!: (error:unknown)=>void;
    mocks.selectMaterialPaths.mockImplementationOnce(()=>new Promise((_,reject)=>{rejectPicker=reject;}));
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());
    await act(async()=>container.querySelector<HTMLButtonElement>('.project-group .group-add-action')!.click());await flush();
    await act(async()=>rejectPicker({code:'UNSUPPORTED_INPUT',reason:'unsupported_input'}));await flush();
    expect(container.querySelector('[role="alertdialog"]')).toBeNull();
    expect(container.querySelector('.material-empty')).not.toBeNull();
  });

  it.each(['colmap', 'colmapDiscovery'] as const)('retains a blocked saved %s draft without reopening its editors or re-alerting', async kind => {
    await act(async()=>root.unmount());
    const draft={...createGenerationDraft(1),inputPath:'E:/sparse/0',...(kind==='colmap'
      ? {inputs:{mode:'colmap' as const,input:{model:'E:/sparse/0',images:'E:/photos'}}}
      : {colmapDiscovery:{models:[{model:'E:/sparse/0',imageCandidates:[]}]}})};
    saveTaskWorkspace([draft],{kind:'draft',id:draft.id},2);
    if(kind==='colmap')mocks.inspectMaterialInputs.mockRejectedValue({code:'UNSUPPORTED_INPUT',reason:'colmap_disabled'});
    else mocks.classifyGenerationPaths.mockRejectedValue({code:'UNSUPPORTED_INPUT',reason:'colmap_disabled'});
    root=createRoot(container);
    await act(async()=>root.render(<LanguageProvider><App/></LanguageProvider>));await flush();
    expect(container.querySelectorAll('[role="alertdialog"]')).toHaveLength(1);
    expect(container.querySelector('.material-list')).not.toBeNull();
    expect(container.querySelectorAll('.colmap-path')).toHaveLength(0);
    expect(container.querySelector<HTMLButtonElement>('.primary-action')!.disabled).toBe(true);
    await act(async()=>container.querySelector<HTMLButtonElement>('.unsupported-input-dialog footer button')!.click());await flush();
    await act(async()=>container.querySelector<HTMLButtonElement>('.quality-option:not(.selected)')!.click());await flush();
    expect(container.querySelector('[role="alertdialog"]')).toBeNull();
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-remove')!.click());await flush();
    expect(container.querySelector('.material-empty')).not.toBeNull();
  });

  it('keeps successful source detection visible while another source failed, blocking generation',async()=>{
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/a.mp4','E:/b.mov']);
    mocks.inspectMaterialInputs.mockResolvedValueOnce({materials:[{id:'source-1',identity:'a1',video:{width:100,height:80,fps:30},imageSequence:null,plan:{estimatedFrames:30},projectionDetection:{sourceIdentity:'a1',requiresChoice:false,confirmedErp:false,supportedErp:false,reason:'',previewJpeg:[]}}],colmap:null,errors:{'source-2':'读取失败'}});
    await act(async()=>container.querySelector<HTMLButtonElement>('.material-input-menu button:first-child')!.click());await flush();
    expect(container.querySelectorAll('.material-item')).toHaveLength(2);
    expect(container.querySelectorAll('.media-source-status.checking')).toHaveLength(0);
    expect(container.querySelector('.material-item')?.textContent).toContain('普通视频');
    expect(container.textContent).toContain('读取失败');expect(container.querySelector<HTMLButtonElement>('.primary-action')!.disabled).toBe(true);
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
  });

  it('hides the reshoot plan and inherited-setting badge in a running legacy project detail', async () => {
    const running: ProjectSummary = { ...project, id: '22222222-2222-2222-2222-222222222222', status: 'running', finalPly: null };
    mocks.getProjectTaskDetail.mockResolvedValue({
      project: running, inputType: 'video', sourcePath: 'E:/new-captures.mov', projectsRoot: 'E:/Projects',
      plannerEnabled: true, estimatedFrames: 90, stage: 'trainingSplats', progress: 65, video: null, imageSequence: null,
      sourceProjectId: project.id, logs: [], reshootRecipe: {
        options: { condition: 'appearance', allowFallback: true, sourceIdentity: 'source', inputIdentity: 'input', projection: 'perspective', imageMatchingStrategy: 'exhaustive' },
        initial: { reconstruction: 'incremental', training: 'cold' }, actual: { reconstruction: 'incremental', training: 'cold' },
      },
    });
    await act(async () => useAppStore.getState().setProjects([running]));
    await flush();
    await act(async () => container.querySelector<HTMLElement>('#unfinished-task-group-content .project-row')!.click());
    await flush();
    const configuration = container.querySelector('.unfinished-task-detail .project-configuration-detail')!;
    expect(configuration).not.toBeNull();
    expect(configuration.textContent).toContain('new-captures.mov');
    for (const caption of ['补拍方案', '整体结构正常，但生成效果不满意', '增量重建', '允许失败后']) expect(configuration.textContent).not.toContain(caption);
    expect(configuration.querySelector('.locked-setting')).toBeNull();
    expect(configuration.querySelectorAll('.quality-option:disabled')).toHaveLength(3);
    expect(configuration.querySelector<HTMLButtonElement>('.planner-switch')?.disabled).toBe(true);
    expect(mocks.startReshootPipeline).not.toHaveBeenCalled();
  });

  it.each([false, true])("shows completed-project statistics without plan details or a duplicate action bar (reshoot: %s)", async reshoot => {
    mocks.getProjectTaskDetail.mockResolvedValueOnce({
      project,
      inputType: "video",
      sourcePath: "E:\\Capture\\input.mp4",
      projectsRoot: "E:\\Projects",
      plannerEnabled: true,
      estimatedFrames: 90,
      stage: "completed",
      progress: 100,
      inputImages: 90,
      registeredImages: 90,
      video: { duration: 3, width: 2560, height: 3840, fps: 30, totalFrames: 90, codec: "h264", rotation: 0, hasAlpha: true, pixelFormat: "rgba" },
      imageSequence: null,
      sourceProjectId: reshoot ? 'original-project' : null,
      reshootRecipe: reshoot ? {
        options: { condition: 'appearance', allowFallback: true, sourceIdentity: 'source', inputIdentity: 'input', projection: 'perspective', imageMatchingStrategy: 'exhaustive' },
        initial: { reconstruction: 'incremental', training: 'cold' }, actual: { reconstruction: 'incremental', training: 'cold' },
      } : null,
      logs: [],
    });
    await act(async () => useAppStore.getState().setColmapAcceleration({
      backend: "gpu",
      detectionState: "ready",
      reasonCode: "gpuReady",
      reason: "NVIDIA GeForce RTX 3060 Ti",
      device: { index: 0, name: "NVIDIA GeForce RTX 3060 Ti", driverVersion: "616.92", computeCapability: "8.6", totalMemoryMb: 8192 },
      requirements: { minimumDriverVersion: "580.00", minimumComputeCapability: "7.5" },
      detectedNvidiaDeviceCount: 1,
    }));
    await act(async () => { container.querySelector<HTMLElement>(".project-group:not(.new-task-group):not(.unfinished) .project-row")?.click(); });
    await flush();

    const detail = container.querySelector(".project-detail-page");
    expect(detail).not.toBeNull();
    expect(detail?.querySelector('.result-actions')).toBeNull();
    expect(detail?.querySelector(":scope > .project-path")).toBeNull();
    expect(detail?.querySelector(".project-configuration-detail")).not.toBeNull();
    expect(detail?.querySelector(".path-picker.readonly")?.textContent).toContain("input.mp4");
    expect(detail?.querySelector(".locked-quality-settings [aria-checked=true]")?.textContent).toContain("均衡");
    expect(detail?.querySelector<HTMLButtonElement>(".planner-switch")?.disabled).toBe(true);
    expect(detail?.querySelector(".locked-setting")).toBeNull();
    for (const caption of ['补拍方案', '整体结构正常，但生成效果不满意', '增量重建', '允许失败后']) expect(detail?.querySelector('.project-configuration-detail')?.textContent).not.toContain(caption);
    expect(detail?.textContent).toContain("COLMAP GPU 加速开启");
    expect(detail?.textContent).not.toContain("COLMAP GPU 加速已开启");
    expect(detail?.querySelector('.source-input-card .material-inspector .alpha-source-status')?.textContent).toBe("透明通道");
    expect(detail?.querySelector('.alpha-source-status')?.classList.contains('readonly')).toBe(true);
    expect(detail?.textContent).not.toContain("已自动提取透明画面和 COLMAP Mask");
    expect(detail?.querySelector(".source-metrics")).toBeNull();
    expect(detail?.querySelector(".primary-action")).toBeNull();
    expect(container.querySelector(".project-detail-actions")).toBeNull();
    expect(container.querySelector("#completed-task-group-content .preview-link")).not.toBeNull();
    expect(container.querySelector("#completed-task-group-content .reshoot-link")).not.toBeNull();
    expect(container.querySelectorAll("#completed-task-group-content .project-actions button")).toHaveLength(4);
  });

  it("removes detail actions for unfinished projects while retaining their card actions", async () => {
    const failed = { ...project, id: "22222222-2222-2222-2222-222222222222", status: "failed" as const, finalPly: null };
    await act(async () => useAppStore.getState().setProjects([failed]));
    await flush();
    await act(async () => container.querySelector<HTMLElement>("#unfinished-task-group-content .project-row")?.click());
    await flush();

    expect(container.querySelector(".project-detail-actions")).toBeNull();
    expect(container.querySelector(".unfinished-task-detail")).not.toBeNull();
    expect(container.querySelector(".locked-quality-settings [aria-checked=true]")?.textContent).toContain("均衡");
    expect(container.querySelector<HTMLButtonElement>(".unfinished-task-detail .planner-switch")?.disabled).toBe(true);
    expect(container.querySelector(".unfinished-task-detail .primary-action")?.textContent).toContain("继续生成");
    expect(container.querySelector("#unfinished-task-group-content .resume-link")).not.toBeNull();
    expect(container.querySelectorAll("#unfinished-task-group-content .project-actions button")).toHaveLength(3);
  });

  it("keeps a task-detail load error scoped to that project", async () => {
    mocks.getProjectTaskDetail.mockRejectedValueOnce(new Error("找不到所需文件"));
    await act(async () => container.querySelector<HTMLElement>("#completed-task-group-content .project-row")?.click());
    await flush();

    expect(container.querySelector(".project-detail-page .inline-error")?.textContent).toContain("找不到所需文件");
    expect(useAppStore.getState().error).toBeNull();

    await act(async () => container.querySelector<HTMLElement>("#new-task-group-content .draft-row")?.click());
    expect(container.querySelector(".inline-error")).toBeNull();
  });

  it("shows only the latest unfinished-project error without a close action", async () => {
    const persisted = { ...project, status: "failed" as const, finalPly: null, failureMessage: "older persisted failure" };
    await act(async () => useAppStore.getState().setProjects([persisted]));
    mocks.revealProject.mockRejectedValueOnce(new Error("newest action failure"));
    await flush();
    await act(async () => container.querySelector<HTMLElement>("#unfinished-task-group-content .project-row")?.click());
    await flush();
    await act(async () => container.querySelector<HTMLButtonElement>("#unfinished-task-group-content .project-actions button:not(.resume-link):not(.danger-link)")?.click());
    await flush();

    const errors = container.querySelectorAll(".project-detail-page .inline-error");
    expect(errors).toHaveLength(1);
    expect(errors[0].textContent).toContain("newest action failure");
    expect(errors[0].textContent).not.toContain("older persisted failure");
    expect(errors[0].querySelector("button")).toBeNull();
  });

  it("restores the low-registration warning from historical project data", async () => {
    await act(async () => {
      useAppStore.getState().setProjects([{ ...project, registeredRatio: 0.62 }]);
    });
    await flush();
    expect(container.querySelector(".project-quality-warning")?.textContent).toContain("62.0%");
  });

  it("does not treat imported COLMAP provenance as this task's registration rate", async () => {
    await act(async () => {
      useAppStore.getState().setProjects([{ ...project, taskKind: "import", registeredRatio: 0 }]);
    });
    await flush();
    expect(container.querySelector(".project-quality-warning")).toBeNull();
  });

  it("summarizes long errors in both task panes and reveals their full detail on hover", async () => {
    const detail = "Brush exited with code 1\n" + "Detailed engine output\n".repeat(100);
    await act(async () => {
      useAppStore.setState({ projects: [{ ...project, status: "failed", finalPly: null, failureMessage: detail }] });
      useAppStore.getState().setError(detail);
    });
    for (const selector of [".inline-error .compact-error", ".project-failure .compact-error"]) {
      const error = container.querySelector<HTMLElement>(selector)!;
      expect(error.textContent).toBe("模型训练失败");
      expect(container.textContent).not.toContain("Detailed engine output");
      await act(async () => error.dispatchEvent(new MouseEvent("mouseover", { bubbles: true })));
      expect(document.querySelector(".error-detail-tooltip[role=tooltip]")?.textContent).toBe(detail);
      await act(async () => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })));
    }
    await act(async () => container.querySelector<HTMLButtonElement>(".inline-error button")!.click());
    expect(container.querySelector(".inline-error")).toBeNull();
    expect(container.querySelector(".project-failure")).not.toBeNull();
  });
  const sharedTask = (overrides: Partial<import("../types/tasks").SharedTask> = {}): import("../types/tasks").SharedTask => ({
    task_id: "shared-1", run_id: "run-1", project_id: project.id, project_path: project.projectPath,
    input_path: "E:\\Media\\orbit.mov", input_type: "video", quality: "balanced", source: "mcp", task_kind: "generation",
    planner_enabled: false, projects_root: "E:\\Projects", status: "completed", stage: "completed",
    revision: 1, sequence: 0, created_at: project.createdAt, updated_at: project.completedAt!, elapsed_ms: 60000,
    progress: 100, estimated_progress: 100, current: null, total: null, eta_seconds: null,
    error: null, result: null, recent_events: [], ...overrides,
  });
  const publishTask = async (task: import("../types/tasks").SharedTask) => {
    const handler = mocks.onTaskUpdate.mock.calls.at(-1)![0];
    await act(async () => handler({ task, events: [], dropped_event_count: 0 }));
    await flush();
  };

  it('changes the unified material GPS hint from the actual run summary, not from unrelated log text', async () => {
    mocks.selectMaterialPaths.mockResolvedValueOnce(['E:/gps-photos']);
    mocks.detectMaterialGps.mockResolvedValue(true);
    mocks.probeAndPlan.mockResolvedValueOnce({ inputType: 'images', video: null,
      imageSequence: { imageCount: 40, width: 800, height: 600 }, plan: { estimatedFrames: 40 } });
    await act(async () => container.querySelector<HTMLButtonElement>('.material-input-menu button:nth-child(2)')!.click());
    await flush();
    expect(container.querySelector('.gps-source-hint')?.textContent).toBe('GPS');
    expect(container.querySelector('.gps-source-hint')?.getAttribute('title')).toContain('将使用有效位置信息辅助生成');
    mocks.startInputsPipeline.mockImplementationOnce(() => new Promise(() => {}));
    await act(async () => container.querySelector<HTMLButtonElement>('.primary-action')!.click());
    await flush();
    const sourceId = mocks.startInputsPipeline.mock.calls.at(-1)![0].items[0].id;
    const workspaceTaskId = mocks.startInputsPipeline.mock.calls.at(-1)![3];
    await publishTask(sharedTask({ task_id: 'different-task', run_id: 'other-run', project_id: null, project_path: null,
      input_path: 'E:/gps-photos', input_type: 'images', source: 'gui', status: 'running', stage: 'reconstructing',
      gps_assistance: { status: 'fallback', detectedSourceIds: [sourceId] } }));
    expect(container.querySelector('.gps-source-hint')?.getAttribute('title')).toContain('将使用有效位置信息辅助生成');
    expect(container.querySelector('.gps-source-hint')?.getAttribute('title')).not.toContain('GPS 辅助未启用');
    await publishTask(sharedTask({ task_id: workspaceTaskId, run_id: 'gps-run', project_id: null, project_path: null,
      input_path: 'E:/gps-photos', input_type: 'images', source: 'gui', status: 'running', stage: 'reconstructing',
      gps_assistance: { status: 'fallback', detectedSourceIds: [sourceId] } }));
    expect(container.querySelector('.gps-source-hint')?.textContent).toBe('GPS');
    expect(container.querySelector('.gps-source-hint')?.getAttribute('title')).toContain('GPS 辅助未启用');
    await act(async () => useAppStore.getState().receiveEvent({ sequence: 999, kind: 'log', engine: 'system',
      taskId: 'unrelated', runId: 'old-run', message: '[GPS] detected' } as import('../types/pipeline').PipelineEvent));
    expect(container.querySelector('.gps-source-hint')?.getAttribute('title')).toContain('GPS 辅助未启用');
  });

  it.each(['failed', 'cancelled', 'interrupted'] as const)('continues a %s shared project with its displayed run identity and no new questions', async status => {
    await publishTask(sharedTask({ status, run_id: 'stopped-run', stage: 'trainingSplats', result: null }));
    const row = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    const button = row.querySelector<HTMLButtonElement>('.resume-link')!;
    expect(button.textContent).toBe('继续生成');
    expect(row.querySelector('.status-copy')?.textContent).toBe(
      { failed: '失败', cancelled: '已取消', interrupted: '已中断' }[status],
    );
    await act(async () => button.click());
    await flush();
    expect(mocks.resumePipeline).toHaveBeenCalledWith(project.id, 'stopped-run');
    expect(mocks.startPipeline).not.toHaveBeenCalled();
    expect(mocks.startReshootPipeline).not.toHaveBeenCalled();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
  });

  it.each(['TASK_BUSY', 'STALE_RUN_ID'])('does not fail a newer active run when an old card continuation returns %s', async code => {
    await act(async () => container.querySelector<HTMLButtonElement>('.queue-toggle')!.click());
    await flush();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('true');
    await publishTask(sharedTask({ status: 'failed', run_id: 'old-run', stage: 'trainingSplats' }));
    let rejectAdmission!: (reason: unknown) => void;
    mocks.resumePipeline.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectAdmission = reject; }));
    await act(async () => container.querySelector<HTMLButtonElement>('[data-task-id="shared-1"] .resume-link')!.click());
    await flush();
    expect(mocks.resumePipeline).toHaveBeenCalledWith(project.id, 'old-run');
    await publishTask(sharedTask({ status: 'running', run_id: 'new-run', stage: 'trainingSplats', revision: 2 }));
    await act(async () => rejectAdmission({ code, message: 'The task execution has changed' }));
    await flush();
    expect(useAppStore.getState().phase).toBe('running');
    expect(useAppStore.getState().liveTask?.run_id).toBe('new-run');
    expect(container.querySelector('.failure-guidance-dialog')).toBeNull();
    expect(container.querySelector('[data-task-id="shared-1"] .status-copy')?.textContent).toBe('运行中');
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
    expect(mocks.resumePipeline).toHaveBeenCalledOnce();
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it.each(['TASK_BUSY', 'STALE_RUN_ID'])('retires only an optimistic start after %s and a successful no-active snapshot', async code => {
    await publishTask(sharedTask({ status: 'failed', run_id: 'old-run', stage: 'trainingSplats' }));
    mocks.resumePipeline.mockRejectedValueOnce({ code, message: 'The task execution has changed' });
    await act(async () => container.querySelector<HTMLButtonElement>('[data-task-id="shared-1"] .resume-link')!.click());
    await flush();
    expect(useAppStore.getState()).toMatchObject({ phase: 'idle', liveTask: null, result: null });
    expect(container.querySelector('.failure-guidance-dialog')).toBeNull();
    expect(container.querySelector('.preflight-dialog')).toBeNull();
    expect(mocks.resumePipeline).toHaveBeenCalledOnce();
    expect(mocks.startInputsPipeline).not.toHaveBeenCalled();
  });

  it('keeps a new active event that arrives during the stale-continuation refresh', async () => {
    await publishTask(sharedTask({ status: 'failed', run_id: 'old-run', stage: 'trainingSplats' }));
    let rejectAdmission!: (reason: unknown) => void;
    mocks.resumePipeline.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectAdmission = reject; }));
    await act(async () => container.querySelector<HTMLButtonElement>('[data-task-id="shared-1"] .resume-link')!.click());
    await flush();
    let finishRefresh!: (tasks: unknown[]) => void;
    mocks.getSharedTasks.mockImplementationOnce(() => new Promise(resolve => { finishRefresh = resolve; }));
    await act(async () => rejectAdmission({ code: 'STALE_RUN_ID', message: 'The task execution has changed' }));
    await flush();
    expect(finishRefresh).toBeTypeOf('function');
    await publishTask(sharedTask({ status: 'running', run_id: 'new-run', revision: 3, stage: 'trainingSplats' }));
    await act(async () => finishRefresh([sharedTask({ status: 'failed', run_id: 'old-run', stage: 'trainingSplats' })]));
    await flush();
    expect(useAppStore.getState().phase).toBe('running');
    expect(useAppStore.getState().liveTask?.run_id).toBe('new-run');
    expect(container.querySelector('.failure-guidance-dialog')).toBeNull();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
  });

  it('does not infer success, failure or an idle state when refreshing a stale continuation fails', async () => {
    await publishTask(sharedTask({ status: 'failed', run_id: 'old-run', stage: 'trainingSplats' }));
    mocks.getSharedTasks.mockRejectedValueOnce(new Error('task registry temporarily unavailable'));
    mocks.resumePipeline.mockRejectedValueOnce({ code: 'STALE_RUN_ID', message: 'The task execution has changed' });
    await act(async () => container.querySelector<HTMLButtonElement>('[data-task-id="shared-1"] .resume-link')!.click());
    await flush();
    expect(useAppStore.getState()).toMatchObject({ phase: 'running', liveTask: null, result: null });
    expect(container.querySelector('.failure-guidance-dialog')).toBeNull();
    expect(mocks.resumePipeline).toHaveBeenCalledOnce();
    expect(container.querySelector('.queue-toggle')?.getAttribute('aria-checked')).toBe('false');
  });

  it("merges a shared task and its old project into one original project row with direct actions", async () => {
    await publishTask(sharedTask());
    const rows = container.querySelectorAll(".project-row:not(.draft-row)");
    expect(rows).toHaveLength(1);
    const row = rows[0];
    expect(row.querySelector(".project-stats")?.textContent).toContain("PLY");
    expect(row.querySelector(".project-path")?.textContent).toBe(project.projectPath);
    expect(row.querySelector(".task-origin")?.textContent).toContain("由 AI Agent 创建");
    expect(row.querySelector(".preview-link")).not.toBeNull();
    expect(row.querySelector(".reshoot-link")).not.toBeNull();
    expect(container.textContent).not.toContain("项目操作");
    expect(container.querySelector(".shared-task-detail")).toBeNull();
    await act(async () => row.querySelector<HTMLButtonElement>(".preview-link")!.click());
    expect(mocks.prepareGaussianPreview).toHaveBeenCalledWith(project.id);
  });

  it("updates completion and project metadata without switching the selected task or starting GUI drafts", async () => {
    await publishTask(sharedTask({ status: "running", stage: "trainingSplats", estimated_progress: 65 }));
    const row = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    await act(async () => row.click());
    expect(container.querySelector(".shared-task-detail .stage-timeline .active")).not.toBeNull();
    expect(container.querySelector(".pane-header .run-state.active")).not.toBeNull();
    const reads = mocks.getProjectOverview.mock.calls.length;
    await publishTask(sharedTask({ revision: 2 }));
    expect(mocks.getProjectOverview.mock.calls.length).toBeGreaterThan(reads);
    expect(container.querySelector(".shared-task-detail .project-detail-stats")).not.toBeNull();
    expect(container.querySelectorAll(".shared-task-detail .stage-timeline .done")).toHaveLength(7);
    expect(container.querySelector(".shared-task-detail .cancel-action")).toBeNull();
    expect(container.querySelector('#completed-task-group-content [data-task-id="shared-1"].selected')).not.toBeNull();
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it("uses the interrupted task state over a stale running project after reopening", async () => {
    mocks.getProjectOverview.mockResolvedValue({ projectsRoot: "E:\\Projects", projects: [{ ...project, status: "running" }] });
    await publishTask(sharedTask({ status: "interrupted", stage: "trainingSplats", estimated_progress: 65 }));
    await act(async () => container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!.click());
    expect(container.querySelector(".pane-header .run-state")?.textContent).toBe("已中断");
    expect(container.querySelector('.unfinished [data-task-id="shared-1"] .status-copy')?.textContent).toBe("已中断");
    expect(container.querySelector(".shared-task-detail .stage-timeline .interrupted")).not.toBeNull();
    expect(container.querySelector(".shared-task-detail .cancel-action")).toBeNull();
  });

  it('keeps the original new-task layout and disabled grip after submission without enabling sorting or deletion', async () => {
    await publishTask(sharedTask({ status: 'running', stage: 'trainingSplats', estimated_progress: 65 }));
    const row = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    expect(row.classList.contains('draft-row')).toBe(true);
    expect(row.classList.contains('drag-disabled')).toBe(true);
    expect(row.querySelector('.draft-drag-indicator svg')).not.toBeNull();
    expect(row.querySelector('.project-title-line strong')?.textContent).toBe('orbit.mov');
    expect(row.querySelector('.project-path')?.textContent).toBe('E:\\Media\\orbit.mov');
    expect(row.querySelector('.project-stats')).toBeNull();
    expect(row.querySelector<HTMLButtonElement>('.danger-link')?.disabled).toBe(true);
    expect(row.hasAttribute('data-draft-id')).toBe(false);
    await act(async () => { row.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true })); });
    expect(container.querySelector('[data-task-id="shared-1"]')).toBe(row);
    await act(async () => row.click());
    expect(container.querySelector('.shared-task-detail .locked-setting')).toBeNull();
    expect(container.querySelector<HTMLButtonElement>('.shared-task-detail .planner-switch')?.disabled).toBe(true);
  });

  it('reorders GUI drafts while ignoring a submitted new-task row in pointer hit testing', async () => {
    await act(async () => container.querySelector<HTMLButtonElement>('.group-add-action')!.click());
    await publishTask(sharedTask({ status: 'created', run_id: null, project_id: null, project_path: null, stage: null }));
    const submitted = container.querySelector<HTMLElement>('[data-task-id="shared-1"]')!;
    const drafts = [...container.querySelectorAll<HTMLElement>('.new-task-group [data-draft-id]')];
    expect(drafts).toHaveLength(2);
    const ids = drafts.map(row => row.dataset.draftId);
    submitted.getBoundingClientRect = () => ({ top: 0, height: 100 } as DOMRect);
    drafts.forEach((row, index) => { row.getBoundingClientRect = () => ({ top: 200 + index * 100, height: 100 } as DOMRect); });
    const held = drafts[0]; let captured = false;
    held.setPointerCapture = () => { captured = true; }; held.hasPointerCapture = () => captured; held.releasePointerCapture = () => { captured = false; };
    const pointer = (type: string, y: number) => {
      const event = new MouseEvent(type, { bubbles: true, button: 0, clientX: 100, clientY: y });
      Object.defineProperty(event, 'pointerId', { value: 9 }); return event;
    };
    await act(async () => {
      held.dispatchEvent(pointer('pointerdown', 220));
      held.dispatchEvent(pointer('pointermove', 450));
      held.dispatchEvent(pointer('pointerup', 450));
    });
    expect([...container.querySelectorAll<HTMLElement>('.new-task-group [data-draft-id]')].map(row => row.dataset.draftId)).toEqual([ids[1], ids[0]]);
    expect(submitted.classList.contains('drag-disabled')).toBe(true);
    expect(mocks.startPipeline).not.toHaveBeenCalled();
  });

  it('retains the source-project name and reshoot label after a GUI reshoot is submitted', async () => {
    await publishTask(sharedTask({ status: 'running', stage: 'trainingSplats', source: 'gui', task_kind: 'reshoot', source_project_id: project.id, project_id: null, project_path: null }));
    expect(container.querySelector('[data-task-id="shared-1"] .project-title-line strong')?.textContent).toBe(`${project.name} · 补拍`);
    expect(container.querySelector('[data-task-id="shared-1"] .draft-drag-indicator')).not.toBeNull();
  });

});
