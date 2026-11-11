import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { LanguageProvider } from '../../src/i18n';
import { MaterialInputs } from '../../src/components/MaterialInputs';
import { FrameFpsControl, type FrameFpsMaterial } from '../../src/components/FrameFpsControl';
import { UnsupportedInputDialog } from '../../src/components/UnsupportedInputDialog';
import type { InputAdmissionReason } from '../../src/lib/inputAdmission';
import type { GenerationInputs, MaterialAnalysis } from '../../src/types/inputs';
import type { VideoInfo } from '../../src/types/pipeline';
import '../../src/styles.css';
const query = new URLSearchParams(location.search);
const noticeParameter = query.get('notice');
const initialNotice: InputAdmissionReason | null = noticeParameter === 'colmap_disabled' || noticeParameter === 'unsupported_input' ? noticeParameter : null;
// This fixture tests a compact component viewport, not the desktop window's
// independent 1100px minimum-size policy. Leave production CSS untouched.
if (initialNotice) for (const element of [document.documentElement, document.body, document.getElementById('root')]) {
    element?.style.setProperty('min-width', '0');
}
localStorage.setItem('ooo-splat-language', query.get('locale') || 'zh-CN');
const scale = Number(query.get('scale') || 100) / 100;
const requestedPanelWidth = Number(query.get('panelWidth') || 780);
const panelWidth = Number.isFinite(requestedPanelWidth) && requestedPanelWidth > 0 ? requestedPanelWidth : 780;
const quality = (['fast', 'balanced', 'high'] as const).find(value => value === query.get('quality')) ?? 'balanced';
const defaultSamplingFps = { fast: 6, balanced: 8, high: 12 }[quality];
const paths = ['E:/拍摄素材/非常长的项目目录/Camera A/2026-10-30/IMG_8023.MOV', '\\\\server\\share\\全景照片\\Input'];
const inputs: GenerationInputs = { mode: 'materials', items: [{ id: 'video', source: { kind: 'video', path: paths[0] }, identity: 'v1', projection: 'perspective', imageMatchingStrategy: 'sequential' }, { id: 'erp', source: { kind: 'imageFolder', path: paths[1] }, identity: 'e1', projection: 'equirectangular', imageMatchingStrategy: 'exhaustive' }, { id: 'loose', source: { kind: 'images', paths: ['E:/Photos/第一张.jpg', 'F:/另外一个很长的文件夹/第二张.png'] }, identity: 'p1', projection: 'auto', imageMatchingStrategy: 'exhaustive' }] };
const analysis = (id: string, identity: string, erp = false): MaterialAnalysis => ({ id, identity, video: id === 'video' ? { width: 1920, height: 1080, fps: 29.97, hasAlpha: true } as VideoInfo : null, imageSequence: id !== 'video' ? { imageCount: 94, width: 4096, height: 2048, hasAlpha: false, requiresLargeSequenceConfirmation: false } : null, dimensions: [[4096, 2048]], plan: null, projectionDetection: { sourceIdentity: identity, confirmedErp: erp, supportedErp: erp || id === 'loose', requiresChoice: id === 'loose', reason: '', previewJpeg: [] } });
function Fixture() {
    const [state, setState] = useState('mixed');
    const [notice, setNotice] = useState<InputAdmissionReason | null>(initialNotice);
    const [removals, setRemovals] = useState<Record<string, number | null>>({});
    (window as any).setMaterialsFixture = setState;
    (window as any).setInputNoticeFixture = setNotice;
    const readonly = state === 'readonly' || state.endsWith('-readonly');
    const gps = state.startsWith('gps');
    const gpsItem = { id: 'gps', source: { kind: 'imageFolder' as const, path: 'E:/拍摄素材/带有 GPS 信息的普通图片/Camera A' }, identity: 'g1', projection: 'perspective' as const, imageMatchingStrategy: 'exhaustive' as const };
    const gpsAnalysis: MaterialAnalysis = {
        ...analysis('gps', 'g1'), gpsDetected: true,
        // Historical GPS summaries are displayed only when actual adoption is recorded.
        gpsFallback: state.includes('-fallback') ? true : readonly ? false : undefined,
        imageSequence: { imageCount: 94, width: 4096, height: 2048, hasAlpha: state.includes('-alpha'), requiresLargeSequenceConfirmation: false },
    };
    const visible = state === 'empty' ? null : state === 'colmap' ? { mode: 'colmap' as const, input: { model: 'E:/reconstruction/sparse/0', images: 'E:/photos', identity: 'model1' } } : gps ? { mode: 'materials' as const, items: [gpsItem] } : state.startsWith('alpha') ? { ...inputs, items: inputs.items.slice(0, 1) } : inputs;
    // Explicit synthetic backend plans for layout only, not a second production planner.
    const materials: FrameFpsMaterial[] = [29.97, 60].map((fps, index) => {
        const id = `sampling-${index}`;
        const removalFps = removals[id] ?? null;
        const target = removalFps == null ? defaultSamplingFps : fps - removalFps;
        return {
            id, name: index === 0 ? 'VID_20200408_184530_00_001_very-long-original-video-filename.mp4' : 'Camera B.mp4',
            video: { width: 1920, height: 1080, fps } as VideoInfo,
            removalFps,
            plan: { samplingFps: target, retentionRatio: target / fps, estimatedFrames: Math.round(60 * target) },
            frameSamplingRange: {
                minimumTargetFps: 1, maximumTargetFps: fps,
                minimum: { samplingFps: 1, retentionRatio: 1 / fps, estimatedFrames: 60 },
                maximum: { samplingFps: fps, retentionRatio: 1, estimatedFrames: Math.round(60 * fps) },
            },
        };
    });
    return <main style={{ width: Math.min(panelWidth, (innerWidth - 48) / scale), margin: '24px', zoom: scale }}><MaterialInputs inputs={visible} analyses={state === 'loading' ? {} : { video: analysis('video', 'v1'), erp: analysis('erp', 'e1', true), loose: analysis('loose', 'p1'), gps: gpsAnalysis }} errors={state === 'error' ? { loose: 'Some inputs overlap or are duplicated. Remove the duplicates.' } : {}} checking={state === 'loading'} disabled={readonly} dragActive={state === 'drag'} readOnly={readonly} onFiles={() => { }} onFolders={() => { }} onRemove={() => { }} onProjection={() => { }} onRetry={() => { }} onColmapField={() => { }}/>{!gps && !['colmap', 'empty'].includes(state) && <FrameFpsControl materials={materials} quality={quality} disabled={readonly} onChange={(id, value) => setRemovals(previous => ({ ...previous, [id]: value }))}/>}{notice && <UnsupportedInputDialog reason={notice} onClose={() => setNotice(null)} />}</main>;
}
createRoot(document.getElementById('root')!).render(<LanguageProvider><Fixture /></LanguageProvider>);
