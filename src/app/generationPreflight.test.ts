// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from 'vitest';
import { createGenerationDraft, loadTaskWorkspace, saveTaskWorkspace } from './taskWorkspace';
import { answerIsValid, collectPreflightQuestions, preflightSnapshot, resolvedProjection } from './generationPreflight';
import { draftProjectionReady } from './taskQueue';
const video = () => ({ ...createGenerationDraft(1), inputPath: 'E:/clip.mp4', projectionDetection: { sourceIdentity: 'source-A', confirmedErp: false, requiresChoice: true, supportedErp: true, reason: '', previewJpeg: [] } });
beforeEach(() => localStorage.clear());
describe('generation preflight registry', () => {
  it('automatically resolves confirmed ERP and ordinary input but never infers panorama from ratio alone', () => {
    const draft = video();
    expect(resolvedProjection(draft)).toBe('auto');
    expect(collectPreflightQuestions([draft])).toHaveLength(0);
    expect(draftProjectionReady(draft)).toBe(false);
    expect(collectPreflightQuestions([{ ...draft, projectionDetection: { ...draft.projectionDetection, confirmedErp: true, requiresChoice: false } }])).toHaveLength(0);
    expect(resolvedProjection({ ...draft, projectionDetection: { ...draft.projectionDetection, confirmedErp: true } })).toBe('equirectangular');
    expect(collectPreflightQuestions([{ ...draft, projectionDetection: null }])).toHaveLength(0);
    expect(collectPreflightQuestions([{ ...draft, running: true }])).toHaveLength(0);
  });
  it.each(['video','images'] as const)('persists the exact %s source answer, not stale analysis, and invalidates changed sources', inputType => {
    const draft = { ...video(), inputType, projectionAnswer: { identity: 'source-A', projection: 'perspective' as const } };
    saveTaskWorkspace([draft], { kind: 'draft', id: draft.id }, 2);
    const restored = loadTaskWorkspace().drafts[0];
    expect(restored.projectionDetection).toBeNull();
    expect(restored.projectionAnswer).toEqual(draft.projectionAnswer);
    expect(resolvedProjection({ ...restored, projectionDetection: draft.projectionDetection })).toBe('perspective');
    const changed = { ...draft, projectionDetection: { ...draft.projectionDetection, sourceIdentity: 'source-B' } };
    expect(resolvedProjection(changed)).toBe('auto');
    expect(draftProjectionReady(changed)).toBe(false);
    expect(preflightSnapshot(changed)).not.toBe(preflightSnapshot(draft));
  });
  it('keeps image warnings in the registry and rejects unsupported inline ERP choices', () => {
    const panorama = video();
    const images = { ...createGenerationDraft(2), inputType: 'images' as const, inputPath: 'E:/photos', imageSequence: { imageCount: 20, width: 100, height: 100, hasAlpha: false, requiresLargeSequenceConfirmation: true } };
    expect(collectPreflightQuestions([panorama, images]).map(item => item.kind)).toEqual(['smallImages']);
    expect(draftProjectionReady({ ...panorama, projectionAnswer: { identity: 'source-A', projection: 'equirectangular' }, projectionDetection: { ...panorama.projectionDetection, supportedErp: false } })).toBe(false);
    const question = collectPreflightQuestions([images])[0];
    expect(answerIsValid(question, 'accepted')).toBe(true);
    expect(answerIsValid(question)).toBe(false);
  });
  it.each([100, 101, 500, 501])('leaves image-order choices to the inline input control (%i)', imageCount => {
    for (const projection of ['perspective', 'equirectangular'] as const) {
      const draft = { ...createGenerationDraft(1), inputType: 'images' as const, inputPath: 'E:/images', projectionAnswer: { identity: 'E:/images', projection }, imageSequence: { imageCount, width: 200, height: 100, hasAlpha: false, requiresLargeSequenceConfirmation: imageCount > 500 } };
      const questions = collectPreflightQuestions([draft]);
      expect(questions).toEqual([]);
      const reshoot = { ...draft, kind: 'reshoot' as const, reshootSource: { projection } as import('../types/pipeline').ReshootSourceInfo };
      expect(collectPreflightQuestions([reshoot]).map(q => q.kind)).toEqual(['reshootCondition', ...(projection === 'perspective' ? ['reshootCamera'] : [])]);
      if (projection === 'perspective') {
        const camera = collectPreflightQuestions([reshoot])[1];
        for (const answer of [undefined, '', 'accepted', 'good']) expect(answerIsValid(camera, answer)).toBe(false);
        for (const answer of ['same', 'different', 'unsure']) expect(answerIsValid(camera, answer)).toBe(true);
      }
      expect(collectPreflightQuestions([{ ...draft, inputType: 'video' }])).toHaveLength(0);
      expect(preflightSnapshot({ ...draft, quality: 'high' })).not.toBe(preflightSnapshot(draft));
      expect(preflightSnapshot({ ...draft, imageSequence: { ...draft.imageSequence, imageCount: imageCount + 1 } })).not.toBe(preflightSnapshot(draft));
    }
  });
});
