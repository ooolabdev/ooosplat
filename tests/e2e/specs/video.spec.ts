import { test, expect } from '../fixtures/native';
import { flags, ordinary, panorama, brushOptions } from '../expected/recipes';
import { assertSampling } from '../expected/contracts';

for (const quality of ['fast', 'balanced', 'high'] as const) {
  test.describe(`ordinary video ${quality}`, () => {
    test.use({ scenario: { name: `video-${quality}`, inputs: [{ name: 'clip.mp4', kind: 'video', fps: 30, duration: 10 }] } });
    test('retained FPS, exact extraction mapping and training recipe agree', async ({ app }) => {
      await app.chooseInputs(); await app.quality(quality);
      await app.page.getByRole('button', { name: '更多设置', exact: true }).click();
      await expect(app.page.locator('.frame-sampling-head output')).toHaveText(`${{ fast: 6, balanced: 8, high: 12 }[quality]} FPS`);
      await app.generate(); const snapshot = await app.waitTerminal();
      const source = snapshot.projects[0].state.inputs.sources[0];
      assertSampling(source.frames, { fast: 6, balanced: 8, high: 12 }[quality], { fast: 60, balanced: 80, high: 120 }[quality]);
      expect(source.frames.initialExtractedFrames).toBe({ fast: 60, balanced: 80, high: 120 }[quality]);
      const calls = snapshot.calls.filter(c => c.operation === 'train');
      expect(calls).toHaveLength(1);
      expect(Number(flags(calls[0].args)['--total-train-iters'])).toBe(ordinary[quality].steps);
      expect(snapshot.calls.filter(c => c.operation === 'extract')).toHaveLength(1);
      expect(snapshot.calls.filter(c => c.operation === 'sequential_matcher').length).toBeGreaterThan(0);
    });
  });
  test.describe(`panorama ${quality}`, () => {
    test.use({ scenario: { name: `erp-${quality}`, inputs: [{ name: 'erp.mp4', kind: 'video', fps: 30, duration: 2, width: 128, height: 64, erp: true }] } });
    test('spherical reconstruction and six-face training use the panorama recipe', async ({ app }) => {
      await app.chooseInputs(); await app.quality(quality); await app.generate();
      const snapshot = await app.waitTerminal();
      const train = snapshot.calls.find(c => c.operation === 'train')!;
      const actual = flags(train.args), expected = panorama[quality];
      const normalized = { ...actual }; delete normalized['--export-path'];
      expect(normalized).toEqual(brushOptions(quality, true));
      expect(Number(actual['--total-train-iters'])).toBe(expected.steps);
      expect(Number(actual['--growth-grad-threshold'])).toBeCloseTo(expected.gradient, 10);
      expect(Number(actual['--growth-select-fraction'])).toBeCloseTo(expected.fraction, 10);
      expect(Number(actual['--growth-stop-iter'])).toBe(expected.stop);
      expect(snapshot.projects[0].state.inputs.sources[0].names).toHaveLength(30);
      if (quality === 'high') expect(Number(actual['--max-splats'])).toBe(32 * 40);
      expect(snapshot.calls.filter(c => c.operation === 'mapper').every(c => flags(c.args)['--Mapper.ba_global_backend'] === 'CERES')).toBe(true);
    });
  });
}
test.describe('short video', () => {
  test.use({ scenario: { name: 'short', inputs: [{ name: 'short.mp4', kind: 'video', fps: 15, duration: 1 }] } });
  test('source below 30 frames retains every frame and cannot backfill', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const s = await app.waitTerminal();
    expect(s.projects[0].state.inputs.sources[0].names).toHaveLength(15);
    expect(s.calls.filter(c => c.operation === 'extract')).toHaveLength(1);
  });
});

test.describe('29.97 FPS with nonzero VFR timestamps', () => {
  const timestamps = Array.from({ length: 300 }, (_, n) => 4 + n / 29.97 + (n % 2 ? .002 : 0));
  test.use({ scenario: { name: 'vfr', inputs: [{ name: 'vfr.mp4', kind: 'video', fps: 29.97, duration: 10, timestamps }] } });
  test('keeps precise source endpoint and validates actual PTS mappings', async ({ app }) => {
    await app.chooseInputs(); await app.quality('balanced');
    await app.page.getByRole('button', { name: '更多设置', exact: true }).click();
    await expect(app.page.locator('.frame-sampling-head output')).toHaveText('8 FPS');
    await expect(app.page.locator('.frame-sampling-track-row > span').last()).toHaveText('29.97 FPS');
    await app.generate(); const s = await app.waitTerminal();
    expect(s.projects[0].state.inputs.sources[0].frames.initialExtractedFrames).toBe(80);
    const extraction = s.calls.find(c => c.operation === 'extract')!;
    expect(extraction.args.filter(arg => arg === 'passthrough')).toHaveLength(1);
    expect(extraction.args).toContain('-stats_enc_pre');
  });
});
test.describe('all frames', () => {
  test.use({ scenario: { name: 'all-frames', inputs: [{ name: 'all.mp4', kind: 'video', duration: 10 }] } });
  test('maximum slider value stores zero removal and preserves every source frame', async ({ app }) => {
    await app.chooseInputs();
    await app.page.getByRole('button', { name: '更多设置', exact: true }).click();
    const slider = app.page.locator('.frame-sampling-track input');
    await slider.focus(); await slider.press('End');
    await expect(app.page.locator('.frame-sampling-head output')).toHaveText('30 FPS');
    await expect(app.page.locator('.frame-sampling-foot')).toContainText('保留 100% 帧');
    await app.generate(); const s = await app.waitTerminal();
    expect(s.projects[0].state.inputs.request.items[0].frameRemovalFps).toBe(0);
    expect(s.projects[0].state.inputs.sources[0].names).toHaveLength(300);
  });
});
