// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import { FrameSamplingControl } from './FrameSamplingControl';
import { FrameFpsControl } from './FrameFpsControl';
import type { Quality, VideoInfo } from '../types/pipeline';

let container: HTMLDivElement, root: Root;
const video = { fps: 30, duration: 60, totalFrames: 1800 } as VideoInfo;
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'zh-CN');
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); localStorage.clear(); });
async function render(quality: Quality, value: number | null = null, onChange = vi.fn(), source: VideoInfo | null = video) {
  await act(async () => root.render(<LanguageProvider><FrameSamplingControl quality={quality} value={value} video={source} disabled={false} onChange={onChange} /></LanguageProvider>));
}

it('sizes the native material picker from the current label instead of a longer unselected option', async () => {
  const changed = vi.fn();
  const shortName = '10月30日(3).mp4';
  const longName = 'VID_20200408_184530_00_001_a-much-longer-unselected-original-video-filename.mp4';
  const materials = [
    { id: 'short', name: shortName, video, removalFps: null },
    { id: 'long', name: longName, video: { ...video, fps: 60 }, removalFps: null },
  ];
  await act(async () => root.render(<LanguageProvider><FrameFpsControl materials={materials} quality="high" disabled={false} onChange={changed} /></LanguageProvider>));
  const picker = container.querySelector('.frame-sampling-material-picker')!;
  const mirror = picker.querySelector('.frame-sampling-material-measure')!;
  const select = picker.querySelector('select')!;
  const arrow = picker.querySelector('.frame-sampling-material-arrow')!;
  expect(arrow.getAttribute('aria-hidden')).toBe('true');
  expect(arrow.getAttribute('width')).toBe('12');
  expect(mirror.getAttribute('aria-hidden')).toBe('true');
  expect(mirror.textContent).toBe(`${shortName} · 30 FPS`);
  expect(mirror.textContent).toBe(select.selectedOptions[0].textContent);
  expect(mirror.textContent).not.toContain(longName);
  expect(select.title).toBe(`${shortName} · 30 FPS`);
  expect(select.getAttribute('aria-label')).toBe('选择视频素材');
  await act(async () => { select.value = 'long'; select.dispatchEvent(new Event('change', { bubbles: true })); });
  expect(mirror.textContent).toBe(`${longName} · 60 FPS`);
  expect(select.value).toBe('long');
  expect(select.title).toBe(mirror.textContent);
  await act(async () => { select.value = 'short'; select.dispatchEvent(new Event('change', { bubbles: true })); });
  expect(mirror.textContent).toBe(`${shortName} · 30 FPS`);
  expect(select.title).toBe(mirror.textContent);
  expect(changed).not.toHaveBeenCalled();
});
it.each([['fast','20.0%'],['balanced','26.7%'],['high','40.0%']] as const)('shows the %s base ratio, never the rescue ratio', async (quality, percent) => {
  await render(quality);
  expect(container.querySelector('output')?.textContent).toBe(percent);
  expect(container.textContent).not.toContain('仅首次抽帧，不含自动补帧。');
  expect(container.querySelector('.field-note')).toBeNull();
  expect(container.querySelector('input')?.getAttribute('aria-label')).toBe('抽帧比例');
});
it('supports the entire manual range and restores the exact FPS default', async () => {
  const changed = vi.fn();
  await render('balanced', .75, changed);
  expect(container.querySelector('output')?.textContent).toBe('75.0%');
  const slider = container.querySelector('input')!;
  expect([slider.min, slider.max, slider.step]).toEqual(['0','12','1']);
  await act(async () => container.querySelector<HTMLButtonElement>('button')!.click());
  expect(changed).toHaveBeenCalledWith(null);
  await render('balanced');
  expect(container.querySelector('output')?.textContent).toBe('26.7%');
});
it('does not invent a percentage before valid video metadata exists', async () => {
  await render('high', null, vi.fn(), null); expect(container.textContent).toBe('');
  await render('high', null, vi.fn(), {...video,fps:0}); expect(container.textContent).toBe('');
});

it('uses only ten-point detents plus the exact source-FPS default', async () => {
  const changed = vi.fn();
  await render('balanced', null, changed);
  const slider = container.querySelector('input')!;
  const expected = [.001, .1, .2, null, .3, .4, .5, .6, .7, .8, .9, 1];
  expect(slider.max).toBe(String(expected.length - 1));
  for (let i = 0; i < expected.length; i++) {
    await act(async () => {
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(slider, String(i));
      slider.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect(changed).toHaveBeenLastCalledWith(expected[i]);
    await render('balanced', expected[i], changed);
  }
});

it('does not lose a restored custom detent or shift indices while new metadata objects arrive', async () => {
  const changed = vi.fn();
  await render('balanced', .333, changed);
  const count = container.querySelector('input')!.max;
  expect(container.querySelector('output')?.textContent).toBe('33.3%');
  await render('balanced', .7, changed, { ...video });
  expect(container.querySelector('input')!.max).toBe(count);
  await render('balanced', .333, changed, { ...video });
  expect(container.querySelector('output')?.textContent).toBe('33.3%');
  expect(changed).not.toHaveBeenCalled();
});

it('keeps 29.97 FPS defaults exact and uses the confirmed English without a hint row', async () => {
  localStorage.setItem('ooo-splat-language', 'en');
  const changed = vi.fn();
  await render('balanced', null, changed, { ...video, fps: 29.97 });
  expect(container.querySelector('label')?.textContent).toBe('Frame sampling');
  expect(container.querySelector('output')?.textContent).toBe('26.7%');
  expect(container.querySelector('.field-note')).toBeNull();
  expect(changed).not.toHaveBeenCalled();
  const id = container.querySelector('input')!.id;
  expect(container.querySelector('label')?.htmlFor).toBe(id);
  expect(container.querySelector('output')?.htmlFor.value).toBe(id);
});
