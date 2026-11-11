// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import { MediaSourceStatus } from './MediaSourceStatus';
import { TaskConfiguration } from './TaskConfiguration';
import type { ComponentProps } from 'react';
let container: HTMLDivElement, root: Root;
const detection = { sourceIdentity: 'source', confirmedErp: false, requiresChoice: true, supportedErp: true, unsupported: false, reason: '', previewJpeg: [] };
beforeEach(() => { Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true }); localStorage.setItem('ooo-splat-language', 'zh-CN'); container = document.createElement('div'); document.body.append(container); root = createRoot(container); });
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });
async function render(props: ComponentProps<typeof MediaSourceStatus>) { await act(async () => root.render(<LanguageProvider key={localStorage.getItem('ooo-splat-language')}><MediaSourceStatus {...props} /></LanguageProvider>)); }

it('distinguishes retry permission, an executed retry, and legacy optimization in completed details', async () => {
  const props = {sourcePath:'E:/input.mp4',projectsRoot:'E:/projects',inputType:'video' as const,quality:'high' as const,plannerEnabled:true,completed:true,acceleration:null};
  for (const used of [false,true]) {
    await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} generationPolicy={{version:1,autoRetryEnabled:true,frameSamplingRatio:null}} automaticallyRetried={used} /></LanguageProvider>));
    expect(container.textContent?.includes('已自动重试')).toBe(used);
    expect(container.querySelector('.planner-switch strong')?.textContent).toBe('自动重试');
  }
  await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} /></LanguageProvider>));
  expect(container.querySelector('.planner-switch strong')?.textContent).toBe('自动优化（实验性）');
  expect(container.textContent).not.toContain('已自动重试');
});

it('uses the saved initial sampling plan in readonly task details', async () => {
  const video = { duration: 60, width: 1920, height: 1080, fps: 30, totalFrames: 1800, codec: 'h264', rotation: 0, pixelFormat: 'yuv420p', hasAlpha: false };
  const inputs = {
    request: { mode: 'materials' as const, items: [{ id: 'clip', source: { kind: 'video' as const, path: 'E:/clip.mp4' }, projection: 'perspective' as const, imageMatchingStrategy: 'sequential' as const }] },
    sources: [{ id: 'clip', identity: 'clip-id', projection: 'perspective' as const, names: ['clip.mp4'], video, adopted: 900, frames: { retentionRatio: .5, samplingFps: 15, estimatedFrames: 900, initialExtractedFrames: 900, extractedFrames: 1080 } }],
    importedReconstruction: false,
    unused: [],
  };
  await act(async () => root.render(<LanguageProvider><TaskConfiguration sourcePath="E:/clip.mp4" projectsRoot="E:/projects" inputType="video" quality="balanced" plannerEnabled completed acceleration={null} inputs={inputs} generationPolicy={{ version: 6, autoRetryEnabled: true, frameSamplingRatio: null, frameSamplingFps: null }} /></LanguageProvider>));
  const toggle = container.querySelector<HTMLButtonElement>('.more-settings-button')!;
  expect(toggle.getAttribute('aria-expanded')).toBe('false');
  await act(async () => toggle.click());
  expect(toggle.getAttribute('aria-expanded')).toBe('true');
  expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('15 FPS');
  expect(container.querySelector('.frame-sampling-foot > span')?.textContent).toContain('50');

  await act(async () => root.render(<LanguageProvider><TaskConfiguration sourcePath="E:/clip.mp4" projectsRoot="E:/projects" inputType="video" quality="balanced" plannerEnabled completed acceleration={null} inputs={inputs} /></LanguageProvider>));
  expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('15 FPS');

  const missingPlanInputs = {
    ...inputs,
    sources: inputs.sources.map(source => ({ ...source, frames: { ...source.frames, samplingFps: 0, retentionRatio: 0 } })),
  };
  await act(async () => root.render(<LanguageProvider><TaskConfiguration sourcePath="E:/clip.mp4" projectsRoot="E:/projects" inputType="video" quality="balanced" plannerEnabled completed acceleration={null} inputs={missingPlanInputs} /></LanguageProvider>));
  expect(container.querySelector('.frame-sampling-control output')?.textContent).toBe('-');
});

it.each(['zh-CN', 'en'] as const)('keeps media type on the left and GPS in a compact right-side tag without coordinates or false history claims (%s)', async locale => {
  localStorage.setItem('ooo-splat-language', locale);
  await render({ inputType: 'images', gpsDetected: true });
  expect(container.querySelectorAll('.media-source-status')).toHaveLength(1);
  const typeRow = container.querySelector('.media-source-status > .media-source-heading.source-status-hint');
  const tags = typeRow?.querySelector('.source-attribute-tags');
  const gps = tags?.querySelector<HTMLElement>('.gps-source-hint.source-attribute-tag');
  expect(typeRow?.querySelector('.source-status-hint-copy strong')?.textContent).toBe(locale === 'zh-CN' ? '普通图片' : 'Ordinary images');
  expect(typeRow?.querySelector('.source-status-hint-copy p')?.textContent).toBe(locale === 'zh-CN' ? '将按普通图片处理' : 'These images will use ordinary-image processing.');
  expect(typeRow?.querySelector('svg.lucide-images')?.getAttribute('aria-hidden')).toBe('true');
  expect(typeRow?.lastElementChild).toBe(tags);
  expect(gps?.textContent).toBe('GPS');
  expect(gps?.title).toContain(locale === 'zh-CN' ? '已识别图片中的 GPS 信息' : 'GPS information detected in images');
  expect(gps?.title).toContain(locale === 'zh-CN' ? '将使用有效位置信息辅助生成' : 'Valid location information will be used to assist generation.');
  expect(gps?.getAttribute('aria-label')).toBe(gps?.title);
  expect(gps?.tabIndex).toBe(0);
  expect(gps?.querySelector('svg.lucide-map-pin')?.getAttribute('aria-hidden')).toBe('true');
  expect(gps?.querySelector('p')).toBeNull();
  expect(container.querySelector('.media-source-status > .gps-source-hint')).toBeNull();
  expect(container.textContent).not.toContain(locale === 'zh-CN' ? '将使用有效位置信息辅助生成' : 'Valid location information will be used to assist generation.');
  await render({ inputType: 'images', gpsDetected: true, gpsFallback: true });
  const fallback = container.querySelector<HTMLElement>('.gps-source-hint')!;
  expect(fallback.textContent).toBe('GPS');
  expect(fallback.title).toContain(locale === 'zh-CN' ? 'GPS 辅助未启用，已切换为不使用 GPS 的生成方式。' : 'GPS assistance is unavailable. Generation will continue without GPS.');
  expect(container.querySelectorAll('.source-attribute-tag')).toHaveLength(1);
  for (const props of [{ inputType: 'images' as const, historical: true }, { inputType: 'video' as const }, { inputType: 'images' as const, gpsDetected: false }]) {
    await render({ gpsDetected: true, ...props }); expect(container.querySelector('.gps-source-hint')).toBeNull();
  }
  for (const props of [{ checking: true }, { error: '读取失败' }]) {
    await render({ inputType: 'images', gpsDetected: true, ...props });
    expect(container.querySelector('.gps-source-hint')).toBeNull();
    expect(container.querySelector('.media-source-type-icon')).toBeNull();
  }
});

it.each(['zh-CN', 'en'] as const)('renders only the two approved metadata tag types without changing the media description (%s)', async locale => {
  localStorage.setItem('ooo-splat-language', locale);
  await render({ inputType: 'images', projection: 'perspective', gpsDetected: true, alphaDetected: true });
  const heading = container.querySelector('.media-source-heading')!;
  const copy = heading.querySelector('.media-source-copy')!;
  const tags = heading.querySelector('.source-attribute-tags')!;
  expect(copy.nextElementSibling).toBe(tags);
  expect([...tags.querySelectorAll('.source-attribute-tag')].map(tag => tag.textContent)).toEqual(['GPS', locale === 'zh-CN' ? '透明通道' : 'Alpha channel']);
  const alpha = tags.querySelector<HTMLElement>('.alpha-source-status')!;
  expect(alpha.title).toBe(locale === 'zh-CN' ? '检测到 Alpha 通道' : 'Alpha channel detected');
  expect(alpha.getAttribute('aria-label')).toBe(alpha.title);
  expect(alpha.tabIndex).toBe(0);
  expect(alpha.querySelector('.lucide-blend')?.getAttribute('aria-hidden')).toBe('true');
  expect(alpha.querySelector('small,p')).toBeNull();
  for (const props of [{ checking: true }, { error: '读取失败' }]) {
    await render({ inputType: 'images', gpsDetected: true, alphaDetected: true, ...props });
    expect(container.querySelector('.source-attribute-tags')).toBeNull();
  }
  await render({ inputType: 'video', projection: 'perspective', alphaDetected: true, gpsDetected: true });
  expect(container.querySelectorAll('.source-attribute-tag')).toHaveLength(1);
  expect(container.querySelector('.alpha-source-status')).not.toBeNull();
  await render({ inputType: 'images', historical: true, projection: 'perspective', alphaDetected: true, gpsDetected: true, gpsFallback: true });
  expect(container.querySelectorAll('.source-attribute-tag')).toHaveLength(2);
  expect(container.querySelector('.alpha-source-status.readonly')).not.toBeNull();
  expect(container.querySelector('.media-source-status.readonly')).not.toBeNull();
});

it('shows loading instead of an old projection result and offers retry only for analysis errors', async () => {
  await render({ inputType: 'video', checking: true, projection: 'equirectangular', detection });
  expect(container.textContent).toBe('正在检测视频类型…');
  expect(container.querySelector('.spin')).not.toBeNull();
  const retry = vi.fn();
  await render({ inputType: 'video', error: '文件不存在', onRetry: retry });
  expect(container.textContent).toContain('素材检测失败');
  expect(container.textContent).not.toContain('普通视频');
  await act(async () => container.querySelector<HTMLButtonElement>('button')!.click());
  expect(retry).toHaveBeenCalledOnce();
});

it('requires an explicit inline choice, keeps it editable, and never renders a preview', async () => {
  const change = vi.fn();
  await render({ inputType: 'video', projection: 'auto', detection, onProjectionChange: change });
  const select = container.querySelector<HTMLSelectElement>('select')!;
  expect(select.value).toBe('');
  expect(select.getAttribute('aria-label')).toBe('请选择视频类型');
  expect(container.querySelector('.media-source-copy > p')?.textContent).toBe('无法判断视频类型。当前不支持双鱼眼、双目、VR180、裁切全景和厂商私有原片。');
  expect(select.querySelector('option[value=equirectangular]')?.textContent).toBe('全景视频');
  expect(container.querySelector('.media-source-type-icon')).toBeNull();
  await act(async () => { select.value = 'equirectangular'; select.dispatchEvent(new Event('change', { bubbles: true })); });
  expect(change).toHaveBeenCalledWith('equirectangular');
  await render({ inputType: 'video', projection: 'equirectangular', detection, onProjectionChange: change });
  expect(container.textContent).toContain('已选择完整全景视频');
  expect(container.textContent).not.toContain('已识别完整 ERP 元数据');
  expect(container.querySelector('select')).not.toBeNull();
  expect(container.querySelector('img,details')).toBeNull();
});

it('shows the metadata explanation only for confirmed ERP and disallows unsupported panorama', async () => {
  await render({ inputType: 'video', projection: 'equirectangular', detection: { ...detection, confirmedErp: true, requiresChoice: false } });
  expect(container.textContent).toContain('已识别完整 ERP 元数据，将自动按全景视频生成');
  expect(container.querySelector('select')).toBeNull();
  await render({ inputType: 'video', projection: 'auto', detection: { ...detection, supportedErp: false, unsupported: true } });
  expect(container.querySelector<HTMLOptionElement>('option[value=equirectangular]')!.disabled).toBe(true);
  expect(container.textContent).toContain('不支持此素材');
});

it.each(['zh-CN', 'en'] as const)('keeps an unsupported ERP choice disabled but removes its warning after choosing ordinary video (%s)', async locale => {
  localStorage.setItem('ooo-splat-language', locale);
  const unsupported = { ...detection, supportedErp: false, unsupported: true };
  await render({ inputType: 'video', projection: 'auto', detection: unsupported });
  expect(container.querySelector('.media-source-warning')).not.toBeNull();
  await render({ inputType: 'video', projection: 'perspective', detection: unsupported });
  expect(container.querySelector('.media-source-warning')).toBeNull();
  expect(container.querySelector<HTMLOptionElement>('option[value=equirectangular]')?.disabled).toBe(true);
  expect(container.querySelector('.media-source-copy > p')?.textContent).toBe(locale === 'zh-CN' ? '将按普通视频处理' : 'This video will use ordinary-video processing.');
});

it('shows an ordinary rotated MOV result without an unsupported-panorama warning or type question', async () => {
  await render({ inputType: 'video', projection: 'auto', detection: { ...detection, requiresChoice: false, supportedErp: false, unsupported: false } });
  expect(container.querySelector('.media-source-copy > p')?.textContent).toBe('将按普通视频处理');
  expect(container.querySelector('.media-source-warning, select')).toBeNull();
});

it.each([
  ['equirectangular', '.lucide-globe'],
  ['perspective', '.lucide-tv-minimal-play'],
] as const)('shows a left-side type icon for %s, including readonly history', async (projection, icon) => {
  for (const historical of [false, true]) {
    await render({ inputType: 'video', projection, historical });
    const typeIcon = container.querySelector(`.has-type-icon .media-source-type-icon${icon}`);
    expect(typeIcon).not.toBeNull();
    expect(typeIcon?.getAttribute('aria-hidden')).toBe('true');
  }
  await render({ inputType: 'images' });
  expect(container.querySelector('.media-source-type-icon.lucide-images')?.getAttribute('aria-hidden')).toBe('true');
});

it('keeps type-icon suppression explicit without suppressing the GPS meaning', async () => {
  await render({ inputType: 'images', showTypeIcon: false, gpsDetected: true });
  expect(container.querySelector('.media-source-type-icon')).toBeNull();
  expect(container.querySelector('.gps-source-hint .lucide-map-pin')?.getAttribute('aria-hidden')).toBe('true');
  await render({ inputType: 'images', historical: true, projection: 'perspective', gpsDetected: true, gpsFallback: false });
  expect(container.querySelector('.media-source-type-icon.lucide-images')).not.toBeNull();
  expect(container.querySelector('.gps-source-hint .lucide-map-pin')).not.toBeNull();
  expect(container.textContent).toContain('本任务按普通图片处理');
});

it.each([true, false])('keeps the readonly saved type without routine copy or invented metadata with completed=%s', async completed => {
  const props = { sourcePath: 'E:/old.mp4', projectsRoot: 'E:/projects', inputType: 'video' as const, quality: 'fast' as const, plannerEnabled: false, completed, acceleration: null };
  await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} videoProjection="equirectangular" /></LanguageProvider>));
  expect(container.querySelector('.source-input-card.readonly .media-source-copy strong')?.textContent).toBe('全景视频');
  expect(container.querySelector('.source-input-card.readonly .media-source-copy p')).toBeNull();
  expect(container.querySelector('select')).toBeNull();
  expect(container.querySelector('.source-input-card .media-path svg')).toBeNull();
  expect(container.querySelector('.source-input-card .media-path strong')?.textContent).toBe('old.mp4');
  await act(async () => root.render(<LanguageProvider><TaskConfiguration {...props} videoProjection="perspective" /></LanguageProvider>));
  expect(container.querySelector('.media-source-status .media-source-copy strong')?.textContent).toBe('普通视频');
  expect(container.querySelector('.media-source-status .media-source-copy p')).toBeNull();
});

it.each(['zh-CN', 'en'] as const)('replaces routine processing copy with supplied source metadata without changing the type or tags (%s)', async locale => {
  localStorage.setItem('ooo-splat-language', locale);
  for (const historical of [false, true]) {
    await render({ inputType: 'images', projection: 'perspective', historical,
      metadataSummary: '2 images · 800 × 600', alphaDetected: true, gpsDetected: true, gpsFallback: false });
    expect(container.querySelector('.media-source-copy strong')?.textContent).toBe(locale === 'zh-CN' ? '普通图片' : 'Ordinary images');
    expect(container.querySelector('.media-source-copy > p')?.textContent).toBe('2 images · 800 × 600');
    expect(container.querySelector('.media-source-copy')?.textContent).not.toContain(locale === 'zh-CN' ? '按普通图片处理' : 'ordinary-image processing');
    expect(container.querySelectorAll('.source-attribute-tag')).toHaveLength(2);
    await render({ inputType: 'images', projection: 'perspective', historical, metadataSummary: null });
    expect(container.querySelector('.media-source-copy > p')).toBeNull();
    expect(container.querySelector('.media-source-copy strong')).not.toBeNull();
  }
});

it('retains required projection questions and unknown-history explanations separately from source metadata', async () => {
  await render({ inputType: 'video', projection: 'auto', detection, metadataSummary: '1920 × 1080 · 29.97 FPS' });
  const select = container.querySelector<HTMLSelectElement>('select')!;
  const hintId = select.getAttribute('aria-describedby')!;
  expect(document.getElementById(hintId)?.textContent).toContain('无法判断视频类型');
  expect(container.querySelector('.media-source-metadata')?.textContent).toBe('1920 × 1080 · 29.97 FPS');
  expect(select.value).toBe('');
  await render({ inputType: 'video', historical: true, projection: null, metadataSummary: '1920 × 1080 · 30 FPS' });
  expect(container.querySelector('.media-source-copy strong')?.textContent).toBe('未记录视频类型');
  expect(container.querySelector('.media-source-copy')?.textContent).toContain('此任务没有可用的视频类型记录');
  expect(container.querySelector('.media-source-metadata')?.textContent).toBe('1920 × 1080 · 30 FPS');
});

it('does not guess a historical type from dimensions and distinguishes image sequences', async () => {
  await render({ inputType: 'video', historical: true, projection: null });
  expect(container.textContent).toContain('未记录视频类型');
  await render({ inputType: 'images', historical: true });
  expect(container.textContent).toContain('本任务按普通图片处理');
  await render({ inputType: 'images' });
  expect(container.textContent).toContain('将按普通图片处理');
});

it('renders the corresponding English copy', async () => {
  localStorage.setItem('ooo-splat-language', 'en');
  await render({ inputType: 'video', projection: 'perspective' });
  expect(container.textContent).toContain('This video will use ordinary-video processing.');
  await render({ inputType: 'video', projection: 'auto', detection });
  expect(container.querySelector('.media-source-copy > p')?.textContent).toContain('Unable to determine the video type.');
  expect(container.querySelector('option[value=equirectangular]')?.textContent).toBe('Panorama video');
});

it('uses only approved image choices, differentiates confirmed and manual ERP, and blocks unsupported ERP', async () => {
  const change=vi.fn();
  await render({inputType:'images',projection:'auto',detection,onProjectionChange:change});
  const select=container.querySelector<HTMLSelectElement>('select')!;
  expect(select.value).toBe('');expect(select.getAttribute('aria-label')).toBe('请选择图片类型');
  expect(container.textContent).toContain('无法判断图片类型。请选择普通图片或全景图片。');
  expect([...select.options].map(o=>o.textContent)).toEqual(['请选择','普通图片','全景图片']);
  await act(async()=>{select.value='equirectangular';select.dispatchEvent(new Event('change',{bubbles:true}));});
  expect(change).toHaveBeenCalledWith('equirectangular');
  await render({inputType:'images',projection:'equirectangular',detection});
  expect(container.textContent).toContain('已选择全景图片，将按全景图片生成');expect(container.querySelector('select')).not.toBeNull();
  await render({inputType:'images',projection:'equirectangular',detection:{...detection,confirmedErp:true,requiresChoice:false}});
  expect(container.textContent).toContain('已识别完整 ERP 元数据，将自动按全景图片生成');expect(container.querySelector('select')).toBeNull();
  await render({inputType:'images',projection:'auto',detection:{...detection,unsupported:true,supportedErp:false}});
  expect(container.querySelector<HTMLOptionElement>('option[value=equirectangular]')!.disabled).toBe(true);expect(container.textContent).toContain('不支持此文件夹的全景格式。');
});

it('displays saved panorama image type in readonly history and approved English copy',async()=>{
  await render({inputType:'images',projection:'equirectangular',historical:true});expect(container.textContent).toContain('本任务按全景图片处理');expect(container.querySelector('select')).toBeNull();
  localStorage.setItem('ooo-splat-language','en');
  await render({inputType:'images',projection:'equirectangular',detection});expect(container.textContent).toContain('Panorama images selected. These images will use panorama generation.');
});
