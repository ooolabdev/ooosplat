import { Application, Asset, BoundingBox, Color, Entity, GSplatResource, Vec3, createGraphicsDevice, DEVICETYPE_WEBGPU, DEVICETYPE_WEBGL2, GAMMA_SRGB, TONEMAP_LINEAR, WORKBUFFER_UPDATE_ALWAYS, WORKBUFFER_UPDATE_ONCE } from "playcanvas";
import { ViewerControls } from "../src/components/GaussianViewer/ViewerControls";
import { PREVIEW_ANIMATION_GLSL, PREVIEW_ANIMATION_WGSL, animationEffectsActive, effectRadialLimitForBounds, orbitDegreesAt, robustEffectBounds, VIDEO_DURATION_SECONDS } from "../src/components/GaussianViewer/PreviewAnimation";
import type { GaussianHtmlView } from "../src/types/pipeline";
import logo from "../assets/app-icon.svg?raw";
import { configureViewerResolution } from "./resolution";
import { configurePreviewRenderer } from "../src/components/GaussianViewer/PreviewBackend";
const config = JSON.parse(document.getElementById("viewer-config")!.textContent!) as {
    locale: "zh-CN" | "en";
    view: GaussianHtmlView;
    splatCount: number;
};
const zh = config.locale === "zh-CN";
const labels = zh ? { play: "播放动画", pause: "暂停", replay: "重播", reset: "恢复初始视角", loading: "正在加载模型", init: "正在初始化图形渲染器", failure: "无法打开预览", help: "模型可能超出浏览器或设备能力。请关闭其他占用内存的程序，更新显卡驱动后重新打开。", controls: "左键旋转 · 右键平移 · 滚轮缩放", lost: "图形设备已中断，请重新打开此文件。" } : { play: "Play animation", pause: "Pause", replay: "Replay", reset: "Reset view", loading: "Loading model", init: "Initializing graphics renderer", failure: "Preview unavailable", help: "This model may exceed your browser or device capacity. Close other memory-intensive apps, update your graphics driver, then reopen this file.", controls: "Left: rotate · Right: pan · Wheel: zoom", lost: "The graphics device was lost. Reopen this file to retry." };
const canvas = document.getElementById("scene") as HTMLCanvasElement;
const play = document.getElementById("play") as HTMLButtonElement;
const replay = document.getElementById("replay") as HTMLButtonElement;
const reset = document.getElementById("reset") as HTMLButtonElement;
const title = document.getElementById("loading-title")!;
const detail = document.getElementById("loading-detail")!;
const loading = document.getElementById("loading")!;
const status = document.getElementById("status")!;
play.textContent = labels.play;
replay.textContent = labels.replay;
reset.textContent = labels.reset;
document.querySelector("nav")!.setAttribute("aria-label", zh ? "动画控制" : "Animation controls");
(document.getElementById("logo") as HTMLImageElement).src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(logo)}`;
let app: Application | undefined, asset: Asset | undefined, controls: ViewerControls | undefined, objectUrl: string | undefined;
let playing = false, elapsed = 0, disposed = false;
let releaseResolution: (() => void) | undefined;
let startView = config.view;
function dispose() {
    if (disposed)
        return;
    disposed = true;
    playing = false;
    controls?.destroy();
    releaseResolution?.();
    releaseResolution = undefined;
    if (objectUrl)
        URL.revokeObjectURL(objectUrl);
    if (app) {
        if (asset) {
            asset.unload();
            app.assets.remove(asset);
        }
        app.destroy();
    }
}
function fail(error: unknown) {
    title.textContent = labels.failure;
    detail.textContent = `${labels.help} ${error instanceof Error ? error.message : String(error)}`;
    loading.hidden = false;
    [play, replay, reset].forEach(b => { b.disabled = true; });
    dispose();
}
function pause() { playing = false; play.textContent = labels.play; }
window.addEventListener("pagehide", dispose, { once: true });
canvas.addEventListener("webglcontextlost", e => { e.preventDefault(); fail(labels.lost); });
canvas.addEventListener("pointerdown", pause, true);
canvas.addEventListener("wheel", pause, { passive: true, capture: true });
async function boot() {
    title.textContent = labels.init;
    const device = await createGraphicsDevice(canvas, { deviceTypes: [DEVICETYPE_WEBGPU, DEVICETYPE_WEBGL2], antialias: false, powerPreference: "high-performance" });
    if (disposed) {
        device.destroy();
        return;
    }
    if (!device.isWebGPU && !device.isWebGL2) {
        device.destroy();
        throw new Error(zh ? "浏览器不支持所需图形功能。" : "This browser does not support the required graphics features.");
    }
    if (Math.ceil(Math.sqrt(config.splatCount)) > device.maxTextureSize) {
        device.destroy();
        throw new Error(zh ? "模型超出显卡纹理容量。" : "The model exceeds this GPU's texture capacity.");
    }
    app = new Application(canvas, { graphicsDevice: device });
    app.scene.gsplatCentersEnabled = true;
    configurePreviewRenderer(app);
    app.scene.gsplat.colorUpdateAngle = 0;
    device.on("devicelost", () => fail(labels.lost));
    releaseResolution = configureViewerResolution(app);
    const camera = new Entity("Camera");
    camera.addComponent("camera", { clearColor: new Color(0.055, 0.067, 0.09), fov: config.view.fov, nearClip: config.view.nearClip, farClip: config.view.farClip, gammaCorrection: GAMMA_SRGB, toneMapping: TONEMAP_LINEAR });
    app.root.addChild(camera);
    controls = new ViewerControls(canvas, camera);
    controls.restore(config.view);
    title.textContent = labels.loading;
    const chunks = Array.from(document.querySelectorAll<HTMLScriptElement>("[data-ply-chunk]"));
    const parts: BlobPart[] = [];
    for (let i = 0; i < chunks.length; i++) {
        if (disposed)
            return;
        const binary = atob(chunks[i].textContent!);
        const bytes = new Uint8Array(binary.length);
        for (let j = 0; j < binary.length; j++)
            bytes[j] = binary.charCodeAt(j);
        parts.push(bytes);
        chunks[i].textContent = "";
        chunks[i].remove();
        detail.textContent = `${Math.round((i + 1) / chunks.length * 50)}%`;
        await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
    }
    objectUrl = URL.createObjectURL(new Blob(parts, { type: "application/octet-stream" }));
    parts.length = 0;
    asset = new Asset("Gaussian model", "gsplat", { url: objectUrl, filename: "model.ply" }, { reorder: false });
    app.assets.add(asset);
    asset.on("progress", (received: number, total: number) => { if (total > 0)
        detail.textContent = `${Math.round(50 + received / total * 50)}%`; });
    await new Promise<void>((resolve, reject) => { asset!.once("load", resolve); asset!.once("error", reject); app!.assets.load(asset!); });
    if (disposed)
        return;
    URL.revokeObjectURL(objectUrl);
    objectUrl = undefined;
    const splat = new Entity("Gaussian model");
    splat.setLocalEulerAngles(0, 0, 180);
    app.root.addChild(splat);
    splat.addComponent("gsplat", { asset, unified: true });
    const resource = asset.resource as GSplatResource;
    const worldBounds = new BoundingBox();
    worldBounds.setFromTransformedAabb(resource.aabb, splat.getWorldTransform());
    controls.setSceneBounds(worldBounds);
    controls.restore(config.view);
    const robust = robustEffectBounds(resource.centers, { center: [resource.aabb.center.x, resource.aabb.center.y, resource.aabb.center.z], halfExtents: [resource.aabb.halfExtents.x, resource.aabb.halfExtents.y, resource.aabb.halfExtents.z] });
    const bounds = new BoundingBox();
    bounds.setFromTransformedAabb(new BoundingBox(new Vec3(...robust.center), new Vec3(...robust.halfExtents)), splat.getWorldTransform());
    const component = splat.gsplat!;
    // Exported PLY has already been filtered; no edit state textures are necessary.
    component.setWorkBufferModifier({ glsl: PREVIEW_ANIMATION_GLSL.replaceAll("loadOoosplatDeleted()", "vec4(0.0)").replaceAll("loadOoosplatSelected()", "vec4(0.0)"), wgsl: PREVIEW_ANIMATION_WGSL.replaceAll("loadOoosplatDeleted()", "vec4f(0.0)").replaceAll("loadOoosplatSelected()", "vec4f(0.0)") });
    component.setParameter("uOoosplatEffectCenter", new Float32Array([bounds.center.x, bounds.center.y, bounds.center.z]));
    component.setParameter("uOoosplatEffectExtent", new Float32Array([bounds.halfExtents.x, bounds.halfExtents.y, bounds.halfExtents.z]));
    component.setParameter("uOoosplatEffectRadialLimit", effectRadialLimitForBounds({ center: [worldBounds.center.x, worldBounds.center.y, worldBounds.center.z], halfExtents: [worldBounds.halfExtents.x, worldBounds.halfExtents.y, worldBounds.halfExtents.z] }, { center: [bounds.center.x, bounds.center.y, bounds.center.z], halfExtents: [bounds.halfExtents.x, bounds.halfExtents.y, bounds.halfExtents.z] }));
    component.setParameter("uOoosplatCropKind", 0);
    component.setParameter("uOoosplatCropCenter", new Float32Array(3));
    component.setParameter("uOoosplatCropSize", new Float32Array([1, 1, 1]));
    component.setParameter("uOoosplatCropRadius", 1);
    component.setParameter("uOoosplatShowSelection", 0);
    function uniforms(enabled: boolean) { component.setParameter("uOoosplatAnimationEnabled", enabled ? 1 : 0); component.setParameter("uOoosplatAnimationTime", elapsed); component.workBufferUpdate = enabled && animationEffectsActive(elapsed) ? WORKBUFFER_UPDATE_ALWAYS : WORKBUFFER_UPDATE_ONCE; app!.renderNextFrame = true; }
    uniforms(false);
    play.onclick = () => { if (playing) {
        pause();
        return;
    } if (elapsed === 0 || elapsed >= VIDEO_DURATION_SECONDS) {
        startView = { ...controls!.snapshot(), fov: config.view.fov, nearClip: config.view.nearClip, farClip: config.view.farClip };
        elapsed = 0;
    } playing = true; play.textContent = labels.pause; uniforms(true); };
    replay.onclick = () => { elapsed = 0; controls!.restore(config.view); startView = config.view; playing = true; play.textContent = labels.pause; uniforms(true); };
    reset.onclick = () => { pause(); elapsed = 0; controls!.restore(config.view); uniforms(false); };
    app.on("update", (dt: number) => {
        if (!playing)
            return;
        elapsed = Math.min(elapsed + dt, VIDEO_DURATION_SECONDS);
        controls!.restore(startView);
        controls!.setOrbitYaw(startView.yaw + orbitDegreesAt(elapsed));
        uniforms(true);
        if (elapsed >= VIDEO_DURATION_SECONDS)
            pause();
    });
    [play, replay, reset].forEach(b => { b.disabled = false; });
    loading.hidden = true;
    status.textContent = `${device.isWebGPU ? "WEBGPU" : "WEBGL2"} / UNIFIED GSPLAT · ${labels.controls}`;
    app.start();
}
void boot().catch(fail);
