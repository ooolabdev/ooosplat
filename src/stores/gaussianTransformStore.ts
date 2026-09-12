import { create } from "zustand";
import { getCurrentLocale, translate } from "../i18n";
import type { GaussianCrop, GaussianEditState, GaussianEditorTool, GaussianPreviewDescriptor, GaussianTransform } from "../types/pipeline";

export const IDENTITY_TRANSFORM: GaussianTransform = { position: [0, 0, 0], rotation: [0, 0, 0], scale: 1 };
const HISTORY_LIMIT = 100;
const HISTORY_BYTES_LIMIT = 64 * 1024 * 1024;

const cloneTransform = (value: GaussianTransform): GaussianTransform => ({ position: [...value.position], rotation: [...value.rotation], scale: value.scale });
export const cloneCrop = (crop: GaussianCrop): GaussianCrop => crop?.kind === "sphere"
  ? { kind: "sphere", center: [...crop.center], radius: crop.radius }
  : crop?.kind === "box" ? { kind: "box", center: [...crop.center], size: [...crop.size] } : null;
const equalTransform = (a: GaussianTransform, b: GaussianTransform) => a.scale === b.scale && a.position.every((v, i) => v === b.position[i]) && a.rotation.every((v, i) => v === b.rotation[i]);
const equalCrop = (a: GaussianCrop, b: GaussianCrop) => JSON.stringify(a) === JSON.stringify(b);

export const packedMaskLength = (splatCount: number) => Math.ceil(Math.max(0, splatCount) / 8);
export const maskBit = (mask: Uint8Array, index: number) => Boolean(mask[index >>> 3] & (1 << (index & 7)));
export function setMaskBit(mask: Uint8Array, index: number, value: boolean) {
  const byte = index >>> 3;
  const bit = 1 << (index & 7);
  if (value) mask[byte] |= bit;
  else mask[byte] &= ~bit;
}
export function countMaskBits(mask: Uint8Array) {
  let count = 0;
  for (const byte of mask) for (let value = byte; value; value &= value - 1) count += 1;
  return count;
}

type HistoryEntry =
  | { kind: "transform"; before: GaussianTransform; after: GaussianTransform; bytes: number }
  | { kind: "crop"; before: GaussianCrop; after: GaussianCrop; bytes: number }
  | { kind: "cropFreeze"; crop: Exclude<GaussianCrop, null>; delta: Uint8Array; bytes: number }
  | { kind: "deletion"; delta: Uint8Array; bytes: number };

function trimHistory(entries: HistoryEntry[]) {
  const output = entries.slice(-HISTORY_LIMIT);
  let bytes = output.reduce((total, entry) => total + entry.bytes, 0);
  while (output.length > 0 && bytes > HISTORY_BYTES_LIMIT) bytes -= output.shift()!.bytes;
  return output;
}
const xorMask = (value: Uint8Array, delta: Uint8Array) => {
  const result = value.slice();
  for (let index = 0; index < result.length; index += 1) result[index] ^= delta[index] ?? 0;
  return result;
};
const pushHistory = (history: HistoryEntry[], entry: HistoryEntry) => trimHistory([...history, entry]);

interface GaussianTransformState {
  descriptor: (GaussianPreviewDescriptor & { assetUrl: string; editMaskAssetUrl: string | null }) | null;
  transform: GaussianTransform;
  editing: GaussianEditState;
  tool: GaussianEditorTool;
  deletedMask: Uint8Array;
  selectionMask: Uint8Array;
  selectedCount: number;
  history: HistoryEntry[];
  future: HistoryEntry[];
  transactionStart: GaussianTransform | null;
  cropTransactionStart: GaussianCrop | undefined;
  revision: number;
  editChangeSerial: number;
  saveState: "saved" | "saving" | "dirty" | "error";
  saveError: string | null;
  load: (descriptor: GaussianPreviewDescriptor & { assetUrl: string; editMaskAssetUrl: string | null }) => void;
  close: () => void;
  setTool: (tool: GaussianEditorTool) => void;
  beginTransaction: () => void;
  setTransformLive: (transform: GaussianTransform) => void;
  commitTransaction: () => void;
  beginCropTransaction: () => void;
  setCropLive: (crop: GaussianCrop) => void;
  commitCropTransaction: () => void;
  commitCropFreeze: (crop: Exclude<GaussianCrop, null>, deletedMask: Uint8Array) => void;
  setInitialDeletedMask: (mask: Uint8Array) => void;
  setSelectionMask: (mask: Uint8Array) => void;
  clearSelection: () => void;
  deleteSelection: () => void;
  resetAll: () => void;
  applySavedEditing: (editing: GaussianEditState, savedSerial: number) => void;
  undo: () => void;
  redo: () => void;
  setSaveState: (saveState: GaussianTransformState["saveState"], saveError?: string | null) => void;
}

const EMPTY_EDITING: GaussianEditState = { crop: null, revision: 0, sourceSplatCount: 0, deletedCount: 0 };
function applyEntry(state: GaussianTransformState, entry: HistoryEntry, direction: "undo" | "redo") {
  if (entry.kind === "transform") return { transform: cloneTransform(direction === "undo" ? entry.before : entry.after) };
  if (entry.kind === "crop") return { editing: { ...state.editing, crop: cloneCrop(direction === "undo" ? entry.before : entry.after) }, editChangeSerial: state.editChangeSerial + 1 };
  if (entry.kind === "cropFreeze") {
    const deletedMask = xorMask(state.deletedMask, entry.delta);
    return {
      deletedMask,
      editing: { ...state.editing, crop: direction === "undo" ? cloneCrop(entry.crop) : null, deletedCount: countMaskBits(deletedMask) },
      selectionMask: new Uint8Array(state.selectionMask.length), selectedCount: 0,
      tool: (direction === "undo" ? entry.crop.kind : "transform") as GaussianEditorTool,
      editChangeSerial: state.editChangeSerial + 1,
    };
  }
  const deletedMask = xorMask(state.deletedMask, entry.delta);
  return {
    deletedMask,
    editing: { ...state.editing, deletedCount: countMaskBits(deletedMask) },
    selectionMask: new Uint8Array(state.selectionMask.length), selectedCount: 0,
    editChangeSerial: state.editChangeSerial + 1,
  };
}

export const useGaussianTransformStore = create<GaussianTransformState>((set, get) => ({
  descriptor: null,
  transform: cloneTransform(IDENTITY_TRANSFORM),
  editing: { ...EMPTY_EDITING },
  tool: "transform",
  deletedMask: new Uint8Array(), selectionMask: new Uint8Array(), selectedCount: 0,
  history: [], future: [], transactionStart: null, cropTransactionStart: undefined,
  revision: 0, editChangeSerial: 0, saveState: "saved", saveError: null,
  load: (descriptor) => {
    const length = packedMaskLength(descriptor.splatCount);
    const loadedEditing = descriptor.editing ?? { ...EMPTY_EDITING, sourceSplatCount: descriptor.splatCount };
    set({
      descriptor, transform: cloneTransform(descriptor.transform),
      editing: { ...loadedEditing, crop: cloneCrop(loadedEditing.crop), sourceSplatCount: descriptor.splatCount },
      tool: "transform", deletedMask: new Uint8Array(length), selectionMask: new Uint8Array(length), selectedCount: 0,
      history: [], future: [], transactionStart: null, cropTransactionStart: undefined,
      revision: 0, editChangeSerial: 0, saveState: "saved", saveError: null,
    });
  },
  close: () => set({
    descriptor: null, transform: cloneTransform(IDENTITY_TRANSFORM), editing: { ...EMPTY_EDITING }, tool: "transform",
    deletedMask: new Uint8Array(), selectionMask: new Uint8Array(), selectedCount: 0,
    history: [], future: [], transactionStart: null, cropTransactionStart: undefined,
    revision: 0, editChangeSerial: 0, saveState: "saved", saveError: null,
  }),
  setTool: (tool) => set((state) => ({ tool, selectionMask: tool === "rectangle" ? state.selectionMask : new Uint8Array(state.selectionMask.length), selectedCount: tool === "rectangle" ? state.selectedCount : 0 })),
  beginTransaction: () => { if (!get().transactionStart) set({ transactionStart: cloneTransform(get().transform) }); },
  setTransformLive: (transform) => set({ transform: cloneTransform(transform), saveState: "dirty" }),
  commitTransaction: () => {
    const state = get(); const start = state.transactionStart;
    if (!start || equalTransform(start, state.transform)) { set({ transactionStart: null }); return; }
    set({ history: pushHistory(state.history, { kind: "transform", before: start, after: cloneTransform(state.transform), bytes: 112 }), future: [], transactionStart: null, revision: state.revision + 1, saveState: "dirty" });
  },
  beginCropTransaction: () => { if (get().cropTransactionStart === undefined) set({ cropTransactionStart: cloneCrop(get().editing.crop) }); },
  setCropLive: (crop) => set((state) => ({ editing: { ...state.editing, crop: cloneCrop(crop) }, saveState: "dirty" })),
  commitCropTransaction: () => {
    const state = get(); const start = state.cropTransactionStart;
    if (start === undefined || equalCrop(start, state.editing.crop)) { set({ cropTransactionStart: undefined }); return; }
    set({ history: pushHistory(state.history, { kind: "crop", before: cloneCrop(start), after: cloneCrop(state.editing.crop), bytes: 160 }), future: [], cropTransactionStart: undefined, revision: state.revision + 1, editChangeSerial: state.editChangeSerial + 1, saveState: "dirty" });
  },
  commitCropFreeze: (crop, deletedMask) => {
    const state = get();
    if (!equalCrop(state.editing.crop, crop)) throw new Error(translate(getCurrentLocale(), "viewer.cropChanged"));
    if (deletedMask.length !== state.deletedMask.length) throw new Error(translate(getCurrentLocale(), "viewer.freezeMaskLength"));
    const next = deletedMask.slice();
    const delta = new Uint8Array(next.length);
    for (let index = 0; index < next.length; index += 1) delta[index] = state.deletedMask[index] ^ next[index];
    set({
      editing: { ...state.editing, crop: null, deletedCount: countMaskBits(next) },
      deletedMask: next,
      selectionMask: new Uint8Array(state.selectionMask.length), selectedCount: 0,
      tool: "transform",
      history: pushHistory(state.history, { kind: "cropFreeze", crop: cloneCrop(crop)!, delta, bytes: delta.byteLength + 192 }),
      future: [], transactionStart: null, cropTransactionStart: undefined,
      revision: state.revision + 1, editChangeSerial: state.editChangeSerial + 1, saveState: "dirty",
    });
  },
  setInitialDeletedMask: (mask) => {
    const state = get();
    if (mask.length !== state.deletedMask.length) throw new Error(translate(getCurrentLocale(), "viewer.deletedMaskCount"));
    const deletedMask = mask.slice();
    set({ deletedMask, editing: { ...state.editing, deletedCount: countMaskBits(deletedMask) } });
  },
  setSelectionMask: (mask) => { const copy = mask.slice(); set({ selectionMask: copy, selectedCount: countMaskBits(copy) }); },
  clearSelection: () => set((state) => ({ selectionMask: new Uint8Array(state.selectionMask.length), selectedCount: 0 })),
  deleteSelection: () => {
    const state = get(); if (state.selectedCount === 0) return;
    const next = state.deletedMask.slice(); const delta = new Uint8Array(next.length);
    for (let index = 0; index < next.length; index += 1) { const value = next[index] | state.selectionMask[index]; delta[index] = next[index] ^ value; next[index] = value; }
    if (!delta.some(Boolean)) { set({ selectionMask: new Uint8Array(state.selectionMask.length), selectedCount: 0 }); return; }
    set({ deletedMask: next, selectionMask: new Uint8Array(state.selectionMask.length), selectedCount: 0, editing: { ...state.editing, deletedCount: countMaskBits(next) }, history: pushHistory(state.history, { kind: "deletion", delta, bytes: delta.byteLength + 32 }), future: [], revision: state.revision + 1, editChangeSerial: state.editChangeSerial + 1, saveState: "dirty" });
  },
  resetAll: () => {
    const state = get();
    const alreadyOriginal = equalTransform(state.transform, IDENTITY_TRANSFORM)
      && state.editing.crop === null
      && !state.deletedMask.some(Boolean);
    set({
      transform: cloneTransform(IDENTITY_TRANSFORM),
      editing: { ...state.editing, crop: null, deletedCount: 0 },
      deletedMask: new Uint8Array(state.deletedMask.length),
      selectionMask: new Uint8Array(state.selectionMask.length),
      selectedCount: 0,
      history: [], future: [], transactionStart: null, cropTransactionStart: undefined,
      revision: alreadyOriginal ? state.revision : state.revision + 1,
      editChangeSerial: alreadyOriginal ? state.editChangeSerial : state.editChangeSerial + 1,
      saveState: alreadyOriginal ? state.saveState : "dirty",
    });
  },
  applySavedEditing: (editing, savedSerial) => set((state) => ({
    editing: state.editChangeSerial === savedSerial
      ? { ...editing, crop: cloneCrop(editing.crop) }
      : { ...state.editing, revision: editing.revision, sourceSplatCount: editing.sourceSplatCount },
  })),
  undo: () => {
    const state = get(); const entry = state.history.at(-1); if (!entry) return;
    set({ ...applyEntry(state, entry, "undo"), history: state.history.slice(0, -1), future: [entry, ...state.future].slice(0, HISTORY_LIMIT), transactionStart: null, cropTransactionStart: undefined, revision: state.revision + 1, saveState: "dirty" });
  },
  redo: () => {
    const state = get(); const entry = state.future[0]; if (!entry) return;
    set({ ...applyEntry(state, entry, "redo"), history: pushHistory(state.history, entry), future: state.future.slice(1), transactionStart: null, cropTransactionStart: undefined, revision: state.revision + 1, saveState: "dirty" });
  },
  setSaveState: (saveState, saveError = null) => set({ saveState, saveError }),
}));
