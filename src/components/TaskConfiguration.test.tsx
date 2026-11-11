// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { LanguageProvider } from '../i18n';
import { TaskConfiguration } from './TaskConfiguration';
import type { SavedInputs } from '../types/inputs';
import type { VideoInfo } from '../types/pipeline';

let host: HTMLDivElement, root: Root;
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.setItem('ooo-splat-language', 'zh-CN');
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); });

const video: VideoInfo = { width: 1920, height: 1080, fps: 30, duration: 42, totalFrames: 1260,
  codec: 'prores', pixelFormat: 'rgba', hasAlpha: true, rotation: 0 };
const props = { sourcePath: 'E:/input.mov', projectsRoot: 'E:/Projects', inputType: 'video' as const,
  quality: 'balanced' as const, plannerEnabled: true, completed: false, acceleration: null,
  video, estimatedFrames: 251, estimate: { estimatedMs: 120000, lowerBoundMs: 90000, upperBoundMs: 150000,
    confidence: 'low' as const, sampleCount: 0, basis: '' } };

it.each([{ completed: true, active: false }, { completed: false, active: false }, { completed: false, active: true }])(
  'keeps old single-source Alpha in its gray input detail without standalone metrics (%j)', async state => {
    await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} {...state}/></LanguageProvider>));
    expect(host.querySelector('.source-metrics')).toBeNull();
    expect(host.querySelectorAll('.source-input-card .material-inspector .alpha-source-status.readonly')).toHaveLength(1);
    expect(host.querySelector('.alpha-source-status')?.textContent).toBe('透明通道');
    expect(host.querySelector('.media-source-heading > .source-attribute-tags .alpha-source-status')?.getAttribute('title')).toBe('检测到 Alpha 通道');
    expect(host.querySelector('.media-source-status')?.textContent).toContain('1920 × 1080 · 30 FPS');
    expect(host.textContent).not.toMatch(/预计帧数|预计时长|COLMAP Mask|素材时长/);
    expect(host.querySelector('.acceleration-status')).not.toBeNull();
  });

it('shows the saved per-source header, not an unrelated aggregate or inferred mask result', async () => {
  const inputs: SavedInputs = { request: { mode: 'materials', items: [
    { id: 'opaque', identity: 'v1', source: { kind: 'video', path: 'E:/opaque.mp4' }, projection: 'perspective', imageMatchingStrategy: 'sequential' },
    { id: 'alpha', identity: 'v2', source: { kind: 'imageFolder', path: 'E:/alpha-images' }, projection: 'perspective', imageMatchingStrategy: 'exhaustive' },
  ] }, sources: [
    { id: 'opaque', identity: 'v1', projection: 'perspective', names: ['a.jpg'], video: { ...video, hasAlpha: false }, adopted: 1 },
    { id: 'alpha', identity: 'v2', projection: 'perspective', names: ['b.png'], video: null,
      imageSequence: { imageCount: 2, width: 800, height: 600, hasAlpha: true, requiresLargeSequenceConfirmation: false }, adopted: 1 },
  ], importedReconstruction: false, unused: [] };
  await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} inputs={inputs} completed/></LanguageProvider>));
  expect(host.querySelector('.alpha-source-status')).toBeNull();
  expect([...host.querySelectorAll('.material-row-meta')].map(meta => meta.textContent)).toEqual(['普通视频', '普通图片']);
  expect(host.querySelector('.material-row-meta small,.material-dimensions,.material-history-hint')).toBeNull();
  expect(host.querySelector('.material-inspector')?.textContent).toContain('1920 × 1080 · 30 FPS');
  await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
  expect(host.querySelector('.material-inspector .alpha-source-status.readonly')?.textContent).toBe('透明通道');
  expect(host.querySelector('.alpha-source-status small')).toBeNull();
  expect(host.querySelector('.material-inspector .media-source-copy p')?.textContent).toBe('2 张图片 · 800 × 600');
  expect(host.querySelector('.material-inspector')?.textContent).not.toContain('本任务按普通图片处理');
});

it('does not guess missing historical Alpha or COLMAP Alpha from aggregate metadata', async () => {
  await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} video={{ ...video, hasAlpha: undefined as unknown as boolean }}/></LanguageProvider>));
  expect(host.querySelector('.alpha-source-status')).toBeNull();
  const inputs: SavedInputs = { request: { mode: 'colmap', input: { model: 'E:/model', images: 'E:/images', identity: 'colmap' } },
    sources: [], importedReconstruction: true, unused: [] };
  await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} inputs={inputs}/></LanguageProvider>));
  expect(host.querySelector('.alpha-source-status')).toBeNull();
  expect(host.textContent).toContain('使用导入结果');
});
