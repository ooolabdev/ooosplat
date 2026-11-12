// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import { MaterialInputs } from './MaterialInputs';
import { FrameFpsControl, fpsLevels } from './FrameFpsControl';
import type { GenerationInputs, MaterialAnalysis } from '../types/inputs';
import type { VideoInfo } from '../types/pipeline';
describe('unified input controls', () => {
    let host: HTMLDivElement, root: Root;
    beforeEach(() => { (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true; localStorage.setItem('ooo-splat-language', 'zh-CN'); host = document.createElement('div'); document.body.append(host); root = createRoot(host); });
    afterEach(async () => { await act(async () => root.unmount()); host.remove(); localStorage.clear(); });
    const input: GenerationInputs = { mode: 'materials', items: [{ id: 'v', source: { kind: 'video', path: 'E:/长路径/clip.mov' }, identity: 'v1', projection: 'perspective', imageMatchingStrategy: 'sequential' }, { id: 'p', source: { kind: 'images', paths: ['E:/a.jpg', 'F:/a.jpg'] }, identity: 'p1', projection: 'auto', imageMatchingStrategy: 'exhaustive' }] };
    const analysis: MaterialAnalysis = { id: 'v', identity: 'v1', video: { width: 1920, height: 1080, fps: 29.97 } as VideoInfo, imageSequence: null, plan: null, projectionDetection: { sourceIdentity: 'v1', confirmedErp: false, supportedErp: false, requiresChoice: false, reason: '', previewJpeg: [] } };
    const expectOnlyExpanded = (id: string) => {
        const entry = host.querySelector<HTMLElement>(`.material-list-entry[data-material-id="${id}"]`)!;
        const row = entry.querySelector('.material-row')!;
        const trigger = row.querySelector<HTMLButtonElement>('.material-row-main')!;
        const inspector = host.querySelector<HTMLElement>('.material-inspector')!;
        expect(host.querySelectorAll('.material-inspector')).toHaveLength(1);
        expect(inspector.parentElement).toBe(entry);
        expect(row.nextElementSibling).toBe(inspector);
        expect(trigger.getAttribute('aria-expanded')).toBe('true');
        expect(inspector.id).not.toBe('');
        expect(trigger.getAttribute('aria-controls')).toBe(inspector.id);
        for (const other of host.querySelectorAll<HTMLButtonElement>('.material-row-main')) {
            if (other !== trigger) {
                expect(other.getAttribute('aria-expanded')).toBe('false');
                expect(other.closest('.material-list-entry')?.querySelector('.material-inspector')).toBeNull();
            }
        }
    };
    it('keeps selected image type on the left and attributes in right-side tags without leaking another source hints', async () => {
        const ready = { ...input, items: input.items.map(item => ({ ...item, projection: 'perspective' as const })) };
        const images: MaterialAnalysis = { ...analysis, id: 'p', identity: 'p1', video: null, gpsDetected: true,
            imageSequence: { imageCount: 2, width: 800, height: 600, hasAlpha: true, requiresLargeSequenceConfirmation: false } };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={ready} analyses={{ v: analysis, p: images }} checking={false} disabled={false}/></LanguageProvider>));
        expect(host.querySelector('.material-inspector .media-source-type-icon.lucide-tv-minimal-play')).not.toBeNull();
        expect(host.querySelector('.material-inspector .gps-source-hint')).toBeNull();
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        const inspector = host.querySelector('.material-inspector')!;
        expect(inspector.querySelectorAll('.media-source-status')).toHaveLength(1);
        expect(inspector.querySelector('.media-source-status > .media-source-heading.source-status-hint .lucide-images')).not.toBeNull();
        expect(inspector.querySelector('.media-source-heading > .source-attribute-tags .gps-source-hint .lucide-map-pin')).not.toBeNull();
        expect(inspector.querySelector('.alpha-source-status .lucide-blend')).not.toBeNull();
        expect(inspector.querySelector('.media-source-copy p')?.textContent).toBe('2 张图片 · 800 × 600');
        expect(inspector.textContent).not.toContain('将按普通图片处理');
        expect(inspector.textContent).not.toContain('将使用有效位置信息辅助生成');
        expect(inspector.querySelector('.gps-source-hint')?.getAttribute('title')).toContain('将使用有效位置信息辅助生成');
        expect(inspector.querySelectorAll('.source-status-hint-copy')).toHaveLength(1);
        expect([...inspector.querySelectorAll('.source-attribute-tag')].map(tag => tag.textContent)).toEqual(['GPS', '透明通道']);
        expect(inspector.querySelector('.media-source-status > .gps-source-hint,.material-inspector > .alpha-source-status')).toBeNull();
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
        expect(host.querySelector('.material-inspector .gps-source-hint')).toBeNull();
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        expect([...host.querySelectorAll('.material-inspector .source-attribute-tag')].map(tag => tag.textContent)).toEqual(['GPS', '透明通道']);
    });
    it('keeps Alpha header detection local to the selected input without claiming pixels or masks', async () => {
        const ready = { ...input, items: input.items.map(item => ({ ...item, projection: 'perspective' as const })) };
        const video = { ...analysis, video: { ...analysis.video!, hasAlpha: false } };
        const images: MaterialAnalysis = { ...analysis, id: 'p', identity: 'p1', video: null,
            imageSequence: { imageCount: 2, width: 800, height: 600, hasAlpha: true, requiresLargeSequenceConfirmation: false } };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={ready} analyses={{ v: video, p: images }} checking={false} disabled={false}/></LanguageProvider>));
        expect(host.querySelector('.alpha-source-status')).toBeNull();
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        const hint = host.querySelector('.material-inspector .alpha-source-status')!;
        expect(hint.textContent).toBe('透明通道');
        expect(hint.getAttribute('title')).toBe('检测到 Alpha 通道');
        expect(hint.closest('.source-attribute-tags')?.parentElement?.classList.contains('media-source-heading')).toBe(true);
        expect(hint.querySelectorAll('svg')).toHaveLength(1);
        expect(hint.querySelector('small')).toBeNull();
        expect(host.textContent).not.toContain('Mask');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
        expect(host.querySelector('.alpha-source-status')).toBeNull();
    });
    it('keeps only the type on each row and moves the selected source dimensions, FPS or image count into its inspector', async () => {
        const ready = { ...input, items: input.items.map(item => ({ ...item, projection: 'perspective' as const })) };
        const images: MaterialAnalysis = { ...analysis, id: 'p', identity: 'p1', video: null,
            imageSequence: { imageCount: 2, width: 800, height: 600, hasAlpha: false, requiresLargeSequenceConfirmation: false },
            dimensions: [[800, 600], [1024, 768]] };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={ready} analyses={{ v: analysis, p: images }} checking={false} disabled={false}/></LanguageProvider>));
        expect([...host.querySelectorAll('.material-row-meta')].map(meta => meta.textContent)).toEqual(['普通视频', '普通图片']);
        expect(host.querySelector('.material-row-meta small,.material-dimensions,.material-history-hint')).toBeNull();
        expect(host.querySelector('.material-inspector .media-source-copy p')?.textContent).toBe('1920 × 1080 · 29.97 FPS');
        expect(host.querySelector('.material-inspector')?.textContent).not.toContain('将按普通视频处理');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        expectOnlyExpanded('p');
        expect(host.querySelector('.material-inspector .media-source-copy p')?.textContent).toBe('2 张图片 · 800 × 600 / 1024 × 768');
        expect(host.querySelector('.material-inspector')?.textContent).not.toContain('29.97 FPS');
    });
    it('suppresses old Alpha during replacement, failed analysis and loading and mutes saved detection', async () => {
        const one: GenerationInputs = { mode: 'materials', items: [input.items[0]] };
        const alpha = { ...analysis, video: { ...analysis.video!, hasAlpha: true } };
        const render = async (options: { checking?: boolean; error?: string; readOnly?: boolean; identity?: string } = {}) => act(async () => root.render(<LanguageProvider><MaterialInputs
            inputs={{ ...one, items: [{ ...one.items[0], identity: options.identity ?? 'v1' }] }} analyses={{ v: alpha }}
            errors={options.error ? { v: options.error } : {}} checking={options.checking ?? false} disabled={false} readOnly={options.readOnly}/></LanguageProvider>));
        await render();
        expect(host.querySelector('.alpha-source-status')).not.toBeNull();
        await render({ checking: true });
        expect(host.querySelector('.alpha-source-status')).toBeNull();
        await render({ error: '读取失败' });
        expect(host.querySelector('.alpha-source-status')).toBeNull();
        await render({ identity: 'new-identity' });
        expect(host.querySelector('.alpha-source-status')).toBeNull();
        await render({ readOnly: true });
        expect(host.querySelector('.alpha-source-status.readonly')?.textContent).toBe('透明通道');
        localStorage.setItem('ooo-splat-language', 'en');
        await act(async () => root.render(<LanguageProvider key="en"><MaterialInputs inputs={one} analyses={{ v: alpha }} checking={false} disabled readOnly/></LanguageProvider>));
        expect(host.querySelector('.alpha-source-status.readonly')?.textContent).toBe('Alpha channel');
    });
    it('uses compact rows, source-local errors, the two media icons and per-row remove actions', async () => {
        const remove = vi.fn(), retry = vi.fn();
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={input} analyses={{ v: analysis }} errors={{ p: '读取失败' }} checking={false} disabled={false} onRemove={remove} onRetry={retry}/></LanguageProvider>));
        expect(host.querySelectorAll('.material-row')).toHaveLength(2);
        expect(host.querySelector('.material-row-meta')?.textContent).toBe('普通视频');
        expect(host.querySelector('.material-row-meta small,.material-dimensions')).toBeNull();
        expect(host.querySelectorAll('.material-row-main > svg')).toHaveLength(2);
        expect(host.querySelector('.material-inspector')?.textContent).toContain('读取失败');
        expect(host.querySelectorAll('.media-source-status.checking')).toHaveLength(0);
        expect(host.textContent).toContain('读取失败');
        await act(async () => host.querySelector<HTMLButtonElement>('.material-remove')!.click());
        expect(remove).toHaveBeenCalledWith('v');
    });
    it.each(['zh-CN', 'en'] as const)('keeps a mixed-dimension source explanation and dimensions visible without a generic engine summary (%s)', async locale => {
        localStorage.setItem('ooo-splat-language', locale);
        const ready = { ...input, items: input.items.map(item => ({ ...item, projection: 'perspective' as const })) };
        const mismatch = '视频无效：图片序列分辨率不一致：E:/Projects/COLMAP/very-long-project-name/work/colmap_frames/source_10.png 为 1024×768，预期 800×600';
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={ready} analyses={{ v: analysis }}
            errors={{ p: mismatch }} checking={false} disabled={false}/></LanguageProvider>));
        expectOnlyExpanded('p');
        const failure = host.querySelector('.material-inspector [role="alert"]')!;
        expect(failure.querySelector('.source-dimension-error')?.textContent).toBe(locale === 'en'
            ? 'Image sequence dimensions do not match: E:/Projects/COLMAP/very-long-project-name/work/colmap_frames/source_10.png is 1024×768; expected 800×600'
            : mismatch.replace(/^视频无效：/, ''));
        expect(failure.textContent).toContain('1024×768');
        expect(failure.textContent).toContain('800×600');
        expect(failure.textContent).not.toMatch(/COLMAP 处理失败|COLMAP processing failed/);
        expect(failure.querySelector('.compact-error')).toBeNull();
        expect(document.querySelector('[role="tooltip"]')).toBeNull();
        expect(host.querySelectorAll('.material-row')[0].classList.contains('ready')).toBe(true);
        expect(host.querySelectorAll('.material-row')[1].classList.contains('error')).toBe(true);
    });
    it('keeps exactly one inspector directly below the selected row and lets users leave an unanswered item', async () => {
        const analyses = { v: analysis };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={input} analyses={analyses}
            errors={{ p: '读取失败' }} checking={false} disabled={false}/></LanguageProvider>));
        expectOnlyExpanded('p');
        expect(host.querySelector('.material-list > .material-inspector')).toBeNull();
        expect(host.querySelectorAll('.material-row')[1].classList.contains('active')).toBe(true);

        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
        expect(host.querySelectorAll('.material-row')[0].classList.contains('active')).toBe(true);
        expect(host.querySelectorAll('.material-row')[1].classList.contains('active')).toBe(false);
        expectOnlyExpanded('v');

        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={input} analyses={analyses}
            errors={{ p: '读取失败' }} checking={false} disabled={false}/></LanguageProvider>));
        expect(host.querySelectorAll('.material-row')[0].classList.contains('active')).toBe(true);
        expectOnlyExpanded('v');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        expectOnlyExpanded('p');
    });
    it('opens the adjacent surviving row when the expanded material is removed', async () => {
        const ready = { ...input, items: input.items.map(item => ({ ...item, projection: 'perspective' as const })) };
        const third = { ...ready.items[1], id: 'third', identity: 'p3' };
        const all = { ...ready, items: [...ready.items, third] };
        const imageAnalysis: MaterialAnalysis = { ...analysis, id: 'p', identity: 'p1', video: null,
            imageSequence: { imageCount: 2, width: 800, height: 600, hasAlpha: false, requiresLargeSequenceConfirmation: false } };
        const analyses = { v: analysis, p: imageAnalysis, third: { ...imageAnalysis, id: 'third', identity: 'p3' } };
        const remove = vi.fn();
        const render = async (inputs: GenerationInputs) => act(async () => root.render(<LanguageProvider><MaterialInputs inputs={inputs}
            analyses={analyses} checking={false} disabled={false} onRemove={remove}/></LanguageProvider>));
        await render(all);
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        expectOnlyExpanded('p');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-remove')[1].click());
        expect(remove).toHaveBeenCalledWith('p');
        await render({ ...all, items: all.items.filter(item => item.id !== 'p') });
        expectOnlyExpanded('third');
    });
    it('uses direct icon buttons, concise readiness copy and a temporary drag prompt', async () => {
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={input} analyses={{ v: analysis }}
            checking={false} disabled={false} onFiles={() => { }} onFolders={() => { }}/></LanguageProvider>));
        const actions = host.querySelectorAll<HTMLButtonElement>('.material-input-summary .material-add-actions button');
        expect([...actions].map(button => button.textContent)).toEqual(['添加文件', '添加文件夹']);
        expect(host.querySelectorAll('.material-input-summary .material-add-actions svg')).toHaveLength(2);
        expect(host.textContent).toContain('1 组需确认');
        expect(host.querySelector('.material-kind-counts,.material-add-menu')).toBeNull();

        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={input} analyses={{ v: analysis }}
            checking={false} disabled={false} dragActive onFiles={() => { }} onFolders={() => { }}/></LanguageProvider>));
        expect(host.querySelector('.material-drop-prompt')?.textContent).toContain('松开以导入素材');
        expect(host.querySelector('.material-input-content')?.getAttribute('aria-hidden')).toBe('true');
        expect(host.querySelector('.material-list,.material-input-summary,.material-inspector')).not.toBeNull();

        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={input} analyses={{ v: analysis }}
            checking={false} disabled={false} onFiles={() => { }} onFolders={() => { }}/></LanguageProvider>));
        expect(host.querySelectorAll('.material-row')[0].classList.contains('active')).toBe(true);
        expect(host.querySelector('.material-drop-prompt')).toBeNull();
        expect(host.querySelector('.material-input-content')?.hasAttribute('aria-hidden')).toBe(false);
    });
    it('shows all-ready copy for read-only inputs without rescanning their source files', async () => {
        const saved = { ...input, items: input.items.map(i => ({ ...i, projection: 'perspective' as const })) };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={saved} analyses={{}} checking={false} disabled readOnly/></LanguageProvider>));
        expect(host.textContent).toContain('全部素材就绪');
        expect(host.textContent).not.toContain('需确认');
    });
    it('renders read-only saved types without controls or analysis', async () => {
        const saved = { ...input, items: input.items.map(i => ({ ...i, projection: 'equirectangular' as const })) };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={saved} analyses={{}} checking={false} disabled readOnly/></LanguageProvider>));
        expect(host.querySelector('.material-add-menu')).toBeNull();
        expect(host.querySelector('.media-source-status.checking,select,.material-remove')).toBeNull();
        expect(host.querySelector('.material-row-meta')?.textContent).toBe('全景视频');
        expect(host.querySelectorAll('.material-row-meta')[1]?.textContent).toBe('全景图片');
        expect(host.querySelector('.material-history-hint,.material-row-meta small')).toBeNull();
        expect(host.querySelector('.material-inspector .media-source-copy strong')?.textContent).toBe('全景视频');
        expect(host.querySelector('.material-inspector .media-source-copy p')).toBeNull();
        expect(host.textContent).not.toContain('本任务按');
        expectOnlyExpanded('v');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        expectOnlyExpanded('p');
        expect(host.querySelector('.material-inspector .media-source-copy strong')?.textContent).toBe('全景图片');
        expect(host.querySelector('.material-inspector')?.textContent).toContain('2 张图片');
    });
    it('keeps COLMAP mutually exclusive and exposes the optional database selector', async () => {
        const field = vi.fn();
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={{ mode: 'colmap', input: { model: 'E:/sparse/0', images: 'E:/images', identity: 'model1' } }} analyses={{}} checking={false} disabled={false} onFiles={() => { }} onFolders={() => { }} onColmapField={field}/></LanguageProvider>));
        const buttons = host.querySelectorAll<HTMLButtonElement>('.material-input-menu button');
        expect(buttons[0].disabled && buttons[1].disabled).toBe(true);
        expect(host.textContent).toContain('使用导入结果');
        expectOnlyExpanded('colmap');
        await act(async () => host.querySelector<HTMLButtonElement>('.material-row-main')!.click());
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.colmap-path')[2].click());
        expect(field).toHaveBeenCalledWith('database');
    });
    it.each(['colmap', 'discovery'] as const)('keeps a blocked %s draft removable without allowing new COLMAP choices', async kind => {
        const remove = vi.fn(), field = vi.fn(), choose = vi.fn();
        const selected: GenerationInputs = { mode: 'colmap', input: { model: 'E:/sparse/0', images: 'E:/images', identity: 'old-model' } };
        const discovery = { models: [{ model: 'E:/sparse/0', imageCandidates: ['E:/images'] }], selectedModel: 'E:/sparse/0', database: null };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={kind === 'colmap' ? selected : null}
            discovery={kind === 'discovery' ? discovery : null} analyses={{}} checking={false} disabled={false} colmapBlocked
            onRemove={remove} onColmapField={field} onColmapImages={choose}/></LanguageProvider>));
        expectOnlyExpanded(kind === 'colmap' ? 'colmap' : 'colmap-discovery');
        expect(host.querySelector('.material-inspector')?.textContent).toContain('暂不支持导入 COLMAP 文件或文件夹。');
        expect(host.querySelector('.material-row')?.classList.contains('ready')).toBe(false);
        expect(host.querySelector('.colmap-path,.colmap-candidates,.colmap-discovery')).toBeNull();
        const button = host.querySelector<HTMLButtonElement>('.material-remove')!;
        expect(button.disabled).toBe(false);
        await act(async () => button.click());
        expect(remove).toHaveBeenCalledWith(kind === 'colmap' ? 'colmap' : 'colmap-discovery');
        expect(field).not.toHaveBeenCalled(); expect(choose).not.toHaveBeenCalled();
    });
    it('does not apply the temporary new-import block to historical read-only COLMAP details', async () => {
        const selected: GenerationInputs = { mode: 'colmap', input: { model: 'E:/sparse/0', images: 'E:/images', identity: 'saved-model' } };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={selected} analyses={{}}
            checking={false} disabled={false} colmapBlocked readOnly/></LanguageProvider>));
        expect(host.textContent).not.toContain('暂不支持导入 COLMAP 文件或文件夹。');
        expect(host.querySelectorAll('.colmap-path')).toHaveLength(3);
        expect([...host.querySelectorAll<HTMLButtonElement>('.colmap-path')].every(button => button.disabled)).toBe(true);
        expect(host.querySelector('.material-remove')).toBeNull();
    });
    it('keeps ambiguous COLMAP discovery inline until the model and images are resolved', async () => {
        const model = vi.fn(), field = vi.fn();
        const discovery = {
            models: [
                { model: 'E:/capture/sparse/0', imageCandidates: ['E:/capture/images'] },
                { model: 'E:/capture/sparse/1', imageCandidates: [] },
            ],
            database: 'E:/capture/database.db',
            selectedModel: null,
        };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={null} discovery={discovery} analyses={{}}
            checking={false} disabled={false} onColmapModel={model} onColmapField={field}/></LanguageProvider>));
        expect(host.textContent).toContain('检测到多个 COLMAP 模型');
        expect(host.querySelectorAll('.colmap-candidates input')).toHaveLength(2);
        expectOnlyExpanded('colmap-discovery');
        await act(async () => host.querySelectorAll<HTMLInputElement>('.colmap-candidates input')[1].click());
        expect(model).toHaveBeenCalledWith('E:/capture/sparse/1');

        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={null}
            discovery={{ ...discovery, selectedModel: 'E:/capture/sparse/1' }} analyses={{}}
            checking={false} disabled={false} onColmapModel={model} onColmapField={field}/></LanguageProvider>));
        const chooseImages = [...host.querySelectorAll<HTMLButtonElement>('.colmap-discovery button')]
            .find(button => button.textContent?.includes('图片文件夹'))!;
        await act(async () => chooseImages.click());
        expect(field).toHaveBeenCalledWith('images');
    });
    it('keeps the image-order decision inline and exposes the confirmed matching strategy', async () => {
        const choose = vi.fn();
        const images: GenerationInputs = { mode: 'materials', items: [{ id: 'photos', source: { kind: 'imageFolder', path: 'E:/photos' }, identity: 'p1', projection: 'perspective', imageMatchingStrategy: 'exhaustive', confirmedImageOrder: false }] };
        const imageAnalysis: MaterialAnalysis = { id: 'photos', identity: 'p1', video: null, imageSequence: { imageCount: 101, width: 800, height: 600 } as any, plan: null, projectionDetection: { sourceIdentity: 'p1', confirmedErp: false, supportedErp: false, requiresChoice: false, reason: '', previewJpeg: [] } };
        await act(async () => root.render(<LanguageProvider><MaterialInputs inputs={images} analyses={{ photos: imageAnalysis }} checking={false} disabled={false} onImageMatchingStrategy={choose}/></LanguageProvider>));
        expect(host.textContent).toContain('未按顺序排列时，生成可能耗时更长');
        expect(host.textContent).toContain('仅匹配相邻图片');
        expect(host.textContent).toContain('所有图片都互相匹配');
        const radios = host.querySelectorAll<HTMLInputElement>('.material-order-options input');
        expect([...radios].every(input => !input.checked)).toBe(true);
        await act(async () => radios[0].click());
        expect(choose).toHaveBeenCalledWith('photos', 'sequential');
    });
    it('moves the single order-question inspector between rows without trapping selection or losing the saved answer', async () => {
        const choose = vi.fn();
        const imageAnalysis: MaterialAnalysis = { ...analysis, id: 'p', identity: 'p1', video: null,
            imageSequence: { imageCount: 101, width: 800, height: 600, hasAlpha: false, requiresLargeSequenceConfirmation: false } };
        const ready = { ...input, items: input.items.map(item => ({ ...item, projection: 'perspective' as const })) };
        const render = async (inputs: GenerationInputs) => act(async () => root.render(<LanguageProvider><MaterialInputs inputs={inputs}
            analyses={{ v: analysis, p: imageAnalysis }} checking={false} disabled={false} onImageMatchingStrategy={choose}/></LanguageProvider>));
        await render(ready);
        expectOnlyExpanded('p');
        expect(host.querySelector('.material-order-question')).not.toBeNull();
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
        expectOnlyExpanded('v');
        expect(host.querySelector('.material-order-question')).toBeNull();
        await render(ready);
        expectOnlyExpanded('v');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        await act(async () => host.querySelector<HTMLInputElement>('.material-order-options input[value="sequential"]')!.click());
        expect(choose).toHaveBeenCalledWith('p', 'sequential');
        const answered = { ...ready, items: ready.items.map(item => item.id === 'p'
            ? { ...item, confirmedImageOrder: true, imageMatchingStrategy: 'sequential' as const } : item) };
        await render(answered);
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[0].click());
        expectOnlyExpanded('v');
        await act(async () => host.querySelectorAll<HTMLButtonElement>('.material-row-main')[1].click());
        expectOnlyExpanded('p');
        expect(host.querySelector<HTMLInputElement>('.material-order-options input[value="sequential"]')?.checked).toBe(true);
    });
    it('has target-FPS detents, exact endpoints and saved compatibility values', () => {
        const levels = fpsLevels(1, 29.97, 21.97, 22.72);
        expect(levels).toEqual([1, ...Array.from({ length: 28 }, (_, i) => i + 2), 21.97, 22.72, 29.97].sort((a, b) => a - b));
        expect(fpsLevels(3, 30, 22, null)).toEqual(Array.from({ length: 28 }, (_, i) => i + 3));
        expect(fpsLevels(Number.NaN, 30, 22, null)).toEqual([]);
        expect(fpsLevels(3, 2, 2, null)).toEqual([]);
    });
    it('shows authoritative sampling plans, switches videos and converts target FPS back to removal FPS', async () => {
        const change = vi.fn();
        const videos = [{ fps: 24, totalFrames: 240, duration: 10 }, { fps: 59.94, totalFrames: 600, duration: 10 }] as VideoInfo[];
        const materials=[
            {id:'a',name:'a.mp4',video:videos[0],removalFps:7.25,plan:{samplingFps:16.8,estimatedFrames:168,retentionRatio:.7},frameSamplingRange:{minimumTargetFps:3,maximumTargetFps:24,minimum:{samplingFps:3,estimatedFrames:30,retentionRatio:.125},maximum:{samplingFps:24,estimatedFrames:240,retentionRatio:1}}},
            {id:'b',name:'b.mp4',video:videos[1],removalFps:8.04,plan:{samplingFps:51.9,estimatedFrames:519,retentionRatio:.865},frameSamplingRange:{minimumTargetFps:3,maximumTargetFps:59.94,minimum:{samplingFps:3,estimatedFrames:30,retentionRatio:.05},maximum:{samplingFps:59.94,estimatedFrames:600,retentionRatio:1}}}
        ];
        await act(async () => root.render(<LanguageProvider><FrameFpsControl materials={materials} quality="balanced" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('16.75 FPS');
        expect(host.textContent).toContain('抽帧过多会导致质量下降，但速度更快');
        expect(host.textContent).toContain('保留 70% 帧');
        expect(host.querySelector('.frame-sampling-track-row')?.textContent).toContain('3 FPS');
        expect(host.querySelector('.frame-sampling-track-row')?.textContent).toContain('24 FPS');
        const slider = host.querySelector<HTMLInputElement>('input')!;
        expect(slider.step).toBe('1');
        expect(slider.getAttribute('aria-valuetext')).toBe('16.75 FPS');

        const selector=host.querySelector<HTMLSelectElement>('select')!;
        await act(async()=>{selector.value='b';selector.dispatchEvent(new Event('change',{bubbles:true}));});
        expect(host.querySelector('output')?.textContent).toBe('51.9 FPS');
        expect(host.textContent).toContain('保留 86.5% 帧');
        const secondSlider=host.querySelector<HTMLInputElement>('input')!;
        await act(async()=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value')!.set!.call(secondSlider,String(Number(secondSlider.value)+1));secondSlider.dispatchEvent(new Event('input',{bubbles:true}));});
        expect(change.mock.calls.at(-1)?.[0]).toBe('b');
        expect(change.mock.calls.at(-1)?.[1]).toBeCloseTo(7.94);
        await act(async()=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value')!.set!.call(secondSlider,secondSlider.max);secondSlider.dispatchEvent(new Event('input',{bubbles:true}));});
        expect(change.mock.calls.at(-1)).toEqual(['b',0]);

        await act(async()=>{selector.value='a';selector.dispatchEvent(new Event('change',{bubbles:true}));});
        await act(async () => host.querySelector<HTMLButtonElement>('button')!.click());
        expect(change).toHaveBeenCalledWith('a',null);
        await act(async () => root.render(<LanguageProvider><FrameFpsControl materials={[]} quality="high" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('input')).toBeNull();
    });
    it('shows configured integer targets without container-duration noise', async () => {
        const change=vi.fn();
        const video={fps:30,totalFrames:1800,duration:60.005} as VideoInfo;
        const range={
            minimumTargetFps:1,maximumTargetFps:30,
            minimum:{samplingFps:.999,estimatedFrames:60,retentionRatio:1/30},
            maximum:{samplingFps:29.999,estimatedFrames:1800,retentionRatio:1},
        };
        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{
            id:'clip',name:'clip.mp4',video,removalFps:null,
            plan:{samplingFps:12.001,estimatedFrames:720,retentionRatio:.4},frameSamplingRange:range,
        }]} quality="high" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('12 FPS');
        expect(host.querySelector('.frame-sampling-track-row')?.textContent).toContain('1 FPS');
        expect(host.querySelector('.frame-sampling-track-row')?.textContent).toContain('30 FPS');
        expect(host.querySelector<HTMLInputElement>('input')?.getAttribute('aria-valuetext')).toBe('12 FPS');

        const ntsc={fps:29.97,totalFrames:1798,duration:60} as VideoInfo;
        const ntscPlan={samplingFps:29.97,estimatedFrames:1798,retentionRatio:1};
        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{
            id:'ntsc',name:'ntsc.mp4',video:ntsc,removalFps:0,plan:ntscPlan,
            frameSamplingRange:{minimumTargetFps:1,maximumTargetFps:29.97,minimum:range.minimum,maximum:ntscPlan},
        }]} quality="high" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('29.97 FPS');
    });
    it('does not show stale plan values while planning and collapses an all-frames-only range', async () => {
        const change = vi.fn();
        const video={fps:2,totalFrames:20,duration:10} as VideoInfo;
        const plan={samplingFps:2,estimatedFrames:20,retentionRatio:1};
        const range={minimumTargetFps:2,maximumTargetFps:2,minimum:plan,maximum:plan};
        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{id:'short',name:'short.mov',video,removalFps:null,plan,frameSamplingRange:range,planning:true}]} quality="fast" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('分析中');
        expect(host.textContent).not.toContain('保留 100% 帧');
        expect(host.querySelector('input')).toBeNull();
        expect(host.querySelector('.frame-sampling-head label')).toBeNull();
        expect(host.querySelector('output')?.hasAttribute('for')).toBe(false);
        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{id:'short',name:'short.mov',video,removalFps:null,plan,frameSamplingRange:range}]} quality="fast" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('2 FPS');
        expect(host.textContent).toContain('保留 100% 帧');
        expect(host.querySelector('input')).toBeNull();
    });
    it('keeps rapid slider changes and reset available while a new plan is pending', async () => {
        const change=vi.fn();
        const video={fps:30,totalFrames:1800,duration:60} as VideoInfo;
        const range={minimumTargetFps:1,maximumTargetFps:30,minimum:{samplingFps:1,estimatedFrames:60,retentionRatio:1/30},maximum:{samplingFps:30,estimatedFrames:1800,retentionRatio:1}};
        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{id:'clip',name:'clip.mp4',video,removalFps:8,plan:{samplingFps:22,estimatedFrames:1320,retentionRatio:1320/1800},frameSamplingRange:range,planning:true}]} quality="balanced" disabled={false} onChange={change}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('分析中');
        const slider=host.querySelector<HTMLInputElement>('input')!;
        expect(slider.disabled).toBe(false);
        await act(async()=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value')!.set!.call(slider,slider.max);slider.dispatchEvent(new Event('input',{bubbles:true}));});
        expect(change).toHaveBeenLastCalledWith('clip',0);
        const reset=host.querySelector<HTMLButtonElement>('.frame-sampling-foot button')!;
        expect(reset.disabled).toBe(false);
        await act(async()=>reset.click());
        expect(change).toHaveBeenLastCalledWith('clip',null);
    });
    it('uses a placeholder for incomplete historical plans without dangling form associations', async () => {
        const video={fps:30,totalFrames:1800,duration:60} as VideoInfo;
        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{id:'old',name:'old.mp4',video,removalFps:null,plan:{samplingFps:Number.NaN,estimatedFrames:0,retentionRatio:Number.POSITIVE_INFINITY}}]} quality="balanced" disabled onChange={vi.fn()}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('-');
        expect(host.querySelector('input')).toBeNull();
        expect(host.querySelector('.frame-sampling-head label')).toBeNull();
        expect(host.querySelector('output')?.hasAttribute('for')).toBe(false);
        expect(host.textContent).not.toContain('NaN');
        expect(host.textContent).not.toContain('Infinity');

        await act(async()=>root.render(<LanguageProvider><FrameFpsControl materials={[{id:'old',name:'old.mp4',video,removalFps:null,plan:{samplingFps:12.001,estimatedFrames:720,retentionRatio:.4}}]} quality="high" disabled onChange={vi.fn()}/></LanguageProvider>));
        expect(host.querySelector('output')?.textContent).toBe('12 FPS');
    });
});
