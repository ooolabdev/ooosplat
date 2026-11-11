// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { LanguageProvider } from '../i18n';
import type { SavedInputs } from '../types/inputs';
import { InputAdoptionWarning } from './InputAdoptionWarning';

let root: Root, host: HTMLDivElement;
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.setItem('ooo-splat-language', 'zh-CN');
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); });
const saved = (registered = 1, prepared = 1000): SavedInputs => ({
  request: { mode: 'materials', items: [{ id: 'new', source: { kind: 'imageFolder', path: 'E:/new-images' }, projection: 'perspective', imageMatchingStrategy: 'exhaustive' }] },
  sources: [
    { id: '__source', identity: 'old', names: [], projection: 'perspective', video: null, adopted: 10000 },
    { id: 'new', identity: 'new', names: [], projection: 'perspective', video: null, adopted: registered },
  ],
  importedReconstruction: false, unused: [],
  sourceAdoption: [
    { sourceId: '__source', plannedFrames: 10000, preparedFrames: 10000, registeredFrames: 10000, adopted: true },
    { sourceId: 'new', plannedFrames: prepared, preparedFrames: prepared, registeredFrames: registered, adopted: registered > 0 },
  ],
});

it('restores the low-new-registration warning without letting old frames inflate the ratio', async () => {
  await act(async () => root.render(<LanguageProvider><InputAdoptionWarning inputs={saved()} /></LanguageProvider>));
  expect(host.textContent).toContain('补拍画面已加入 1/1000 张');
  expect(host.textContent).not.toContain('10001/11000');
});
it('uses source-local warnings when the new overall ratio is good', async () => {
  const inputs = saved(1, 10);
  inputs.sourceAdoption!.push({ sourceId: 'good', plannedFrames: 100, preparedFrames: 100, registeredFrames: 100, adopted: true });
  await act(async () => root.render(<LanguageProvider><InputAdoptionWarning inputs={inputs} /></LanguageProvider>));
  expect(host.textContent).toContain('new-images：补拍画面已加入 1/10 张');
});
it.each([0, 80, 100])('does not invent a low-new warning for zero or acceptable registration (%s)', async registered => {
  await act(async () => root.render(<LanguageProvider><InputAdoptionWarning inputs={saved(registered, 100)} /></LanguageProvider>));
  expect(host.textContent).not.toContain('部分新增画面');
});
it('keeps ordinary generation and missing historical counts unchanged', async () => {
  const inputs = saved(); inputs.sources = inputs.sources.filter(source => source.id !== '__source');
  await act(async () => root.render(<LanguageProvider><InputAdoptionWarning inputs={inputs} /></LanguageProvider>));
  expect(host.textContent).toBe('');
  inputs.sourceAdoption = undefined;
  await act(async () => root.render(<LanguageProvider><InputAdoptionWarning inputs={inputs} /></LanguageProvider>));
  expect(host.textContent).toBe('');
});
it('localizes persisted multiline warnings without duplication', async () => {
  localStorage.setItem('ooo-splat-language', 'en');
  const warning = '部分素材未能参与生成：missing。本次仅使用已连接到同一场景的素材。\nnew-images：补拍画面已加入 1/10 张，部分新增画面未能找到与原场景的联系，效果可能受影响。';
  await act(async () => root.render(<LanguageProvider><InputAdoptionWarning warning={warning} /></LanguageProvider>));
  expect(host.querySelectorAll('.input-adoption-warning')).toHaveLength(2);
  expect(host.textContent).toContain('Some inputs could not be included: missing.');
  expect(host.textContent).toContain('new-images: Added 1/10 reshoot images.');
  expect(host.textContent).not.toMatch(/补拍|部分/);
});
