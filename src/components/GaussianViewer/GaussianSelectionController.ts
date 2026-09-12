import {
  GSplatProcessor,
  Mat4,
  type CameraComponent,
  type GSplatComponent,
  type GraphicsDevice,
  type Texture,
  WORKBUFFER_UPDATE_ONCE,
} from "playcanvas";
import type { GaussianCrop } from "../../types/pipeline";
import { getCurrentLocale, translate } from "../../i18n";
import { maskBit, packedMaskLength, setMaskBit } from "../../stores/gaussianTransformStore";

export type RectangleSelectionMode = "replace" | "add" | "remove";
export interface SelectionRectangle { minX: number; minY: number; maxX: number; maxY: number }

const PROCESS_SELECTION_GLSL = /* glsl */ `
uniform mat4 uOoosplatViewProjection;
uniform mat4 uOoosplatModelMatrix;
uniform vec4 uOoosplatSelectionRect;
uniform float uOoosplatSelectionCropKind;
uniform vec3 uOoosplatSelectionCropCenter;
uniform vec3 uOoosplatSelectionCropSize;
uniform float uOoosplatSelectionCropRadius;
uniform float uOoosplatSelectionOperation;

bool cropContains(vec3 center) {
    if (uOoosplatSelectionCropKind < 0.5) return true;
    if (uOoosplatSelectionCropKind < 1.5) return distance(center, uOoosplatSelectionCropCenter) <= uOoosplatSelectionCropRadius;
    return all(lessThanEqual(abs(center - uOoosplatSelectionCropCenter), uOoosplatSelectionCropSize * 0.5));
}

void process() {
    vec3 center = (uOoosplatModelMatrix * vec4(getCenter(), 1.0)).xyz;
    bool deleted = loadOoosplatDeleted().r > 0.5;
    bool hit;
    if (uOoosplatSelectionOperation > 0.5) {
        hit = !deleted && !cropContains(center);
    } else {
        vec4 clip = uOoosplatViewProjection * vec4(center, 1.0);
        vec2 ndc = clip.xy / max(abs(clip.w), 0.000001);
        hit = !deleted
            && clip.w > 0.0
            && ndc.x >= uOoosplatSelectionRect.x && ndc.x <= uOoosplatSelectionRect.z
            && ndc.y >= uOoosplatSelectionRect.y && ndc.y <= uOoosplatSelectionRect.w
            && cropContains(center);
    }
    writeOoosplatScratch(vec4(hit ? 1.0 : 0.0));
}
`;

function expandPackedMask(target: Uint8Array, packed: Uint8Array, splatCount: number) {
  target.fill(0);
  for (let index = 0; index < splatCount; index += 1) target[index] = maskBit(packed, index) ? 255 : 0;
}

function masksEqual(left: Uint8Array, right: Uint8Array) {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) if (left[index] !== right[index]) return false;
  return true;
}

export function packSelectionTextureData(data: Uint8Array, splatCount: number) {
  const packed = new Uint8Array(packedMaskLength(splatCount));
  for (let index = 0; index < splatCount; index += 1) if (data[index] > 127) setMaskBit(packed, index, true);
  return packed;
}

export function combineSelectionMasks(current: Uint8Array, hit: Uint8Array, mode: RectangleSelectionMode) {
  if (current.length !== hit.length) throw new Error(translate(getCurrentLocale(), "viewer.selectionMaskLength"));
  if (mode === "replace") return hit.slice();
  const next = current.slice();
  for (let index = 0; index < next.length; index += 1) {
    next[index] = mode === "add" ? next[index] | hit[index] : next[index] & ~hit[index];
  }
  return next;
}

export function combineDeletedMasks(current: Uint8Array, outsideCrop: Uint8Array) {
  if (current.length !== outsideCrop.length) throw new Error(translate(getCurrentLocale(), "viewer.deletedMaskLength"));
  const next = current.slice();
  for (let index = 0; index < next.length; index += 1) next[index] |= outsideCrop[index];
  return next;
}

export class GaussianSelectionController {
  private readonly processor: GSplatProcessor;
  private readonly selectedTexture: Texture;
  private readonly deletedTexture: Texture;
  private readonly scratchTexture: Texture;
  private readonly selectedPixels: Uint8Array;
  private readonly deletedPixels: Uint8Array;
  private readonly viewProjection = new Mat4();
  private selectedMask: Uint8Array;
  private deletedMask: Uint8Array;
  private destroyed = false;
  private queue: Promise<void> = Promise.resolve();
  private requestedSelectionRevision = 0;
  private appliedSelectionRevision = 0;

  constructor(
    device: GraphicsDevice,
    private readonly component: GSplatComponent,
    private readonly camera: CameraComponent,
    private readonly splatCount: number,
    private readonly requestRender: () => void,
  ) {
    const selected = component.getInstanceTexture("ooosplatSelected");
    const deleted = component.getInstanceTexture("ooosplatDeleted");
    const scratch = component.getInstanceTexture("ooosplatScratch");
    if (!selected || !deleted || !scratch) throw new Error(translate(getCurrentLocale(), "viewer.editTextures"));
    this.selectedTexture = selected;
    this.deletedTexture = deleted;
    this.scratchTexture = scratch;
    this.selectedPixels = new Uint8Array(selected.width * selected.height);
    this.deletedPixels = new Uint8Array(deleted.width * deleted.height);
    this.selectedMask = new Uint8Array(packedMaskLength(splatCount));
    this.deletedMask = new Uint8Array(packedMaskLength(splatCount));
    this.processor = new GSplatProcessor(
      device,
      { component },
      { component, streams: ["ooosplatScratch"] },
      { processGLSL: PROCESS_SELECTION_GLSL },
    );
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    this.processor.destroy();
  }

  private enqueue<T>(operation: () => T | Promise<T>) {
    const result = this.queue.catch(() => undefined).then(operation);
    this.queue = result.then(() => undefined, () => undefined);
    return result;
  }

  private validateMask(mask: Uint8Array) {
    if (mask.length !== packedMaskLength(this.splatCount)) throw new Error(translate(getCurrentLocale(), "viewer.editMaskLength"));
  }

  private uploadMask(texture: Texture, pixels: Uint8Array, mask: Uint8Array) {
    expandPackedMask(pixels, mask, this.splatCount);
    const textureData = texture.lock() as Uint8Array;
    textureData.set(pixels);
    texture.unlock();
  }

  private refreshWorkBuffer() {
    if (this.destroyed) return;
    this.component.workBufferUpdate = WORKBUFFER_UPDATE_ONCE;
    this.requestRender();
  }

  applyMasks(deletedMask: Uint8Array, selectedMask: Uint8Array) {
    const deleted = deletedMask.slice();
    const selected = selectedMask.slice();
    return this.enqueue(() => {
      if (this.destroyed) return;
      this.validateMask(deleted);
      this.validateMask(selected);
      const deletedChanged = !masksEqual(this.deletedMask, deleted);
      const selectedChanged = !masksEqual(this.selectedMask, selected);
      if (!deletedChanged && !selectedChanged) return;
      if (deletedChanged) {
        this.deletedMask = deleted;
        this.uploadMask(this.deletedTexture, this.deletedPixels, deleted);
      }
      if (selectedChanged) {
        this.selectedMask = selected;
        this.uploadMask(this.selectedTexture, this.selectedPixels, selected);
      }
      this.refreshWorkBuffer();
    });
  }

  select(rectangle: SelectionRectangle, mode: RectangleSelectionMode, crop: GaussianCrop) {
    const revision = ++this.requestedSelectionRevision;
    const execute = async () => {
      if (this.destroyed) return this.selectedMask.slice();
      this.viewProjection.mul2(this.camera.projectionMatrix, this.camera.camera.viewMatrix);
      const cropKind = crop?.kind === "sphere" ? 1 : crop?.kind === "box" ? 2 : 0;
      const center = crop?.center ?? [0, 0, 0];
      const size = crop?.kind === "box" ? crop.size : [1, 1, 1];
      const radius = crop?.kind === "sphere" ? crop.radius : 1;
      this.processor.setParameter("uOoosplatViewProjection", this.viewProjection.data);
      this.processor.setParameter("uOoosplatModelMatrix", this.component.entity.getWorldTransform().data);
      this.processor.setParameter("uOoosplatSelectionRect", [rectangle.minX, rectangle.minY, rectangle.maxX, rectangle.maxY]);
      this.processor.setParameter("uOoosplatSelectionCropKind", cropKind);
      this.processor.setParameter("uOoosplatSelectionCropCenter", center);
      this.processor.setParameter("uOoosplatSelectionCropSize", size);
      this.processor.setParameter("uOoosplatSelectionCropRadius", radius);
      this.processor.setParameter("uOoosplatSelectionOperation", 0);
      this.processor.process();
      const pixels = await this.scratchTexture.read(0, 0, this.scratchTexture.width, this.scratchTexture.height, { immediate: true }) as Uint8Array;
      const hit = packSelectionTextureData(pixels, this.splatCount);
      const next = combineSelectionMasks(this.selectedMask, hit, mode);
      if (this.destroyed || revision < this.appliedSelectionRevision) return this.selectedMask.slice();
      this.selectedMask = next.slice();
      this.appliedSelectionRevision = revision;
      this.uploadMask(this.selectedTexture, this.selectedPixels, this.selectedMask);
      this.refreshWorkBuffer();
      return next;
    };
    return this.enqueue(execute);
  }

  freezeCrop(crop: Exclude<GaussianCrop, null>, deletedMask: Uint8Array) {
    const currentDeleted = deletedMask.slice();
    const revision = ++this.requestedSelectionRevision;
    const execute = async () => {
      if (this.destroyed) throw new Error(translate(getCurrentLocale(), "viewer.selectorDestroyed"));
      this.validateMask(currentDeleted);
      if (!masksEqual(this.deletedMask, currentDeleted)) {
        this.deletedMask = currentDeleted;
        this.uploadMask(this.deletedTexture, this.deletedPixels, this.deletedMask);
      }
      const cropKind = crop.kind === "sphere" ? 1 : 2;
      const size = crop.kind === "box" ? crop.size : [1, 1, 1];
      const radius = crop.kind === "sphere" ? crop.radius : 1;
      this.processor.setParameter("uOoosplatModelMatrix", this.component.entity.getWorldTransform().data);
      this.processor.setParameter("uOoosplatSelectionCropKind", cropKind);
      this.processor.setParameter("uOoosplatSelectionCropCenter", crop.center);
      this.processor.setParameter("uOoosplatSelectionCropSize", size);
      this.processor.setParameter("uOoosplatSelectionCropRadius", radius);
      this.processor.setParameter("uOoosplatSelectionOperation", 1);
      this.processor.process();
      const pixels = await this.scratchTexture.read(0, 0, this.scratchTexture.width, this.scratchTexture.height, { immediate: true }) as Uint8Array;
      if (this.destroyed || revision < this.appliedSelectionRevision) throw new Error(translate(getCurrentLocale(), "viewer.cropExpired"));
      const outsideCrop = packSelectionTextureData(pixels, this.splatCount);
      const next = combineDeletedMasks(this.deletedMask, outsideCrop);
      this.deletedMask = next.slice();
      this.selectedMask = new Uint8Array(this.selectedMask.length);
      this.appliedSelectionRevision = revision;
      this.uploadMask(this.deletedTexture, this.deletedPixels, this.deletedMask);
      this.uploadMask(this.selectedTexture, this.selectedPixels, this.selectedMask);
      this.refreshWorkBuffer();
      return next;
    };
    return this.enqueue(execute);
  }
}
