// @vitest-environment jsdom
import { afterEach, describe, expect, it } from 'vitest';
import { createGenerationDraft, createReshootDraft, draftGenerationPolicy, loadTaskWorkspace, saveTaskWorkspace, TASK_WORKSPACE_STORAGE_KEY } from './taskWorkspace';
import { collectPreflightQuestions, confirmedInputs, preflightSnapshot } from './generationPreflight';
import { draftProjectionReady, firstQueueInputIssue, queuedDraftIds } from './taskQueue';
import type { MaterialAnalysis } from '../types/inputs';
const ready = () => {
    const draft = createGenerationDraft(1);
    draft.inputPath = 'E:/photos';
    draft.plan = { estimatedFrames: 150 } as any;
    draft.inputs = { mode: 'materials', items: [{ id: 'ordinary', source: { kind: 'imageFolder', path: 'E:/photos' }, identity: 'o1', projection: 'perspective', imageMatchingStrategy: 'sequential', confirmedImageOrder: true }, { id: 'erp', source: { kind: 'imageFolder', path: 'E:/erp' }, identity: 'e1', projection: 'equirectangular', imageMatchingStrategy: 'exhaustive' }] };
    const analysis = (id: string, identity: string, count: number): MaterialAnalysis => ({ id, identity, video: null, imageSequence: { imageCount: count } as any, plan: null, projectionDetection: { sourceIdentity: identity, confirmedErp: false, requiresChoice: false, supportedErp: id === 'erp', reason: '', previewJpeg: [] } });
    draft.inputAnalyses = { ordinary: analysis('ordinary', 'o1', 101), erp: analysis('erp', 'e1', 2) };
    return draft;
};
afterEach(() => localStorage.clear());
describe('multi-input admission and restoration', () => {
    it('keeps image order inline while retaining the remaining source-bound warning', () => {
        const draft = ready();
        const questions = collectPreflightQuestions([draft]);
        expect(questions.map(q => [q.sourceId, q.kind])).toEqual([['erp', 'smallImages']]);
        expect(questions[0].draft.inputPath).toBe('E:/erp');
        const answered = confirmedInputs(draft, { [questions[0].id]: 'accepted' });
        if (answered.mode !== 'materials')
            throw Error();
        expect(answered.items[0].imageMatchingStrategy).toBe('sequential');
        expect(answered.items[1].confirmedSmallImages).toBe(true);
        const before = preflightSnapshot(draft);
        if (draft.inputs?.mode === 'materials')
            draft.inputs.items[0].identity = 'changed';
        expect(preflightSnapshot(draft)).not.toBe(before);
    });
    it('asks model condition once and camera relation for each ordinary reshoot source only', () => {
        const draft = { ...ready(), ...createReshootDraft(1, 'project', 'source', 'high'), inputs: ready().inputs, inputAnalyses: ready().inputAnalyses, inputPath: 'E:/photos' };
        const questions = collectPreflightQuestions([draft]);
        expect(questions.map(q => q.kind)).toEqual(['reshootCondition', 'reshootCamera']);
        expect(questions[1].sourceId).toBe('ordinary');
    });
    it('blocks the whole task/queue on any unknown, stale or checking item and never skips it', () => {
        const draft = ready(), later = ready();
        later.id = 'later';
        expect(draftProjectionReady(draft)).toBe(true);
        if (draft.inputs?.mode === 'materials') draft.inputs.items[0].confirmedImageOrder = false;
        expect(draftProjectionReady(draft)).toBe(false);
        expect(firstQueueInputIssue([draft, later])?.reason).toBe('imageOrder');
        if (draft.inputs?.mode === 'materials') draft.inputs.items[0].confirmedImageOrder = true;
        if (draft.inputs?.mode === 'materials')
            draft.inputs.items[1].projection = 'auto';
        expect(firstQueueInputIssue([draft, later])?.draft.id).toBe(draft.id);
        expect(queuedDraftIds([{ ...ready(), id: 'active', running: true }, draft, later], 'active', true).size).toBe(0);
        draft.inputChecking = true;
        expect(firstQueueInputIssue([draft])?.reason).toBe('checking');
        draft.inputChecking = false;
        if (draft.inputs?.mode === 'materials')
            draft.inputs.items[1].identity = 'stale';
        expect(draftProjectionReady(draft)).toBe(false);
    });
    it('restores source-bound choices and FPS, but never trusts cached analyses after restart', () => {
        const draft = ready();
        draft.frameSamplingFps = 7.25;
        saveTaskWorkspace([draft], { kind: 'draft', id: draft.id }, 2);
        const restored = loadTaskWorkspace().drafts[0];
        expect(restored.inputs).toEqual(draft.inputs);
        expect(restored.frameSamplingFps).toBe(7.25);
        expect(restored.needsValidation).toBe(true);
        expect(restored.inputAnalyses).toBeUndefined();
        expect(draftProjectionReady(restored)).toBe(false);
        const stored = JSON.parse(localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY)!);
        stored.drafts[0].inputs = { mode: 'materials', items: [null] };
        stored.drafts[0].frameSamplingFps = -1;
        localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify(stored));
        expect(loadTaskWorkspace().drafts[0].inputs).toMatchObject({mode:'materials',items:[{identity:null,projection:'auto'}]});
        expect(draftProjectionReady(loadTaskWorkspace().drafts[0])).toBe(false);
        expect(loadTaskWorkspace().drafts[0].frameSamplingFps).toBeNull();
    });
    it('restores all-frames values in v6 and migrates unsubmitted v5 drafts', () => {
        const draft = createGenerationDraft(1);
        draft.generationPolicyVersion = 6;
        draft.inputPath = 'E:/clip.mp4';
        draft.inputs = { mode: 'materials', items: [{
            id: 'clip', source: { kind: 'video', path: draft.inputPath }, projection: 'perspective',
            imageMatchingStrategy: 'sequential', frameRemovalFps: 0,
        }] };
        saveTaskWorkspace([draft], { kind: 'draft', id: draft.id }, 2);
        let restored = loadTaskWorkspace().drafts[0];
        expect(restored.generationPolicyVersion).toBe(6);
        expect(restored.inputs).toEqual(draft.inputs);

        const stored = JSON.parse(localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY)!);
        stored.drafts[0].generationPolicyVersion = 5;
        localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify(stored));
        restored = loadTaskWorkspace().drafts[0];
        expect(restored.generationPolicyVersion).toBe(6);
        expect(restored.inputs).toMatchObject({ mode: 'materials', items: [{ source: { path: 'E:/clip.mp4' } }] });
        if (restored.inputs?.mode !== 'materials') throw Error();
        expect(restored.inputs.items[0].frameRemovalFps).toBeUndefined();

        stored.drafts[0].inputs.items[0].frameRemovalFps = .1;
        localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify(stored));
        restored = loadTaskWorkspace().drafts[0];
        expect(restored.generationPolicyVersion).toBe(6);
        if (restored.inputs?.mode !== 'materials') throw Error();
        expect(restored.inputs.items[0].frameRemovalFps).toBe(.1);
    });
    it('restores unresolved COLMAP discovery without converting it into an image folder', () => {
        const draft = createGenerationDraft(1);
        draft.inputPath = 'E:/capture/sparse/0';
        draft.colmapDiscovery = {
            models: [{ model: draft.inputPath, imageCandidates: [] }],
            database: 'E:/capture/database.db',
            selectedModel: draft.inputPath,
        };
        saveTaskWorkspace([draft], { kind: 'draft', id: draft.id }, 2);
        const restored = loadTaskWorkspace().drafts[0];
        expect(restored.colmapDiscovery).toEqual(draft.colmapDiscovery);
        expect(restored.inputs).toBeUndefined();
        expect(firstQueueInputIssue([restored])?.reason).toBe('colmap');

        const stored = JSON.parse(localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY)!);
        stored.drafts[0].colmapDiscovery.selectedModel = 'E:/capture/sparse/missing';
        localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY, JSON.stringify(stored));
        const damaged = loadTaskWorkspace().drafts[0];
        expect(damaged.colmapDiscovery).toBeUndefined();
        expect(damaged.inputs).toBeUndefined();
        expect(damaged.inputPath).toBeNull();
    });
    it('migrates only standby legacy inputs, preserving identity-bound choices and tiny ratios', () => {
        const draft={...createGenerationDraft(1),inputPath:'E:/clip.mp4',frameSamplingRatio:.001,projectionAnswer:{identity:'old-file-stat',projection:'equirectangular' as const}};
        const running={...createGenerationDraft(2),inputPath:'E:/active.mp4',running:true};
        saveTaskWorkspace([draft,running],{kind:'draft',id:draft.id},3);
        const legacy=JSON.parse(localStorage.getItem(TASK_WORKSPACE_STORAGE_KEY)!);delete legacy.drafts[0].generationPolicyVersion;delete legacy.drafts[1].generationPolicyVersion;localStorage.setItem(TASK_WORKSPACE_STORAGE_KEY,JSON.stringify(legacy));
        const [restored,active]=loadTaskWorkspace().drafts;
        expect(restored.inputs).toMatchObject({mode:'materials',items:[{id:draft.id,identity:'legacy:old-file-stat',projection:'equirectangular',source:{kind:'video',path:draft.inputPath}}]});
        expect(draftGenerationPolicy(restored)).toMatchObject({version:4,frameSamplingRatio:.001,frameSamplingFps:null});
        expect(restored.needsValidation).toBe(true);
        expect(active.inputs).toBeUndefined();
        expect(active.running).toBe(true);
        expect(draftGenerationPolicy({...restored,frameSamplingFps:2})).toMatchObject({frameSamplingFps:2,frameSamplingRatio:null});
        expect(draftGenerationPolicy({...ready(),frameSamplingRatio:.001})).toMatchObject({frameSamplingRatio:null});
    });
});
