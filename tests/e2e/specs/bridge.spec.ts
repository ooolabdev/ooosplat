import { test, expect } from '../fixtures/native';

test.describe('per-source Bridge', () => {
  test.use({ scenario: {
    name: 'bridge-source-registration',
    inputs: [{ name: 'large.mp4', kind: 'video', duration: 15 }, { name: 'small.mp4', kind: 'video', duration: 5 }],
    faults: [{ operation: 'colmap:mapper', occurrence: 1, outcome: 'success', registered: 100 }],
  } });
  test('high overall registration does not hide a weak second source', async ({ app }) => {
    await app.chooseInputs([0, 1]); await app.quality('fast'); await app.generate();
    const s = await app.waitTerminal();
    const sources = s.projects[0].state.inputs.sources;
    expect(sources[0].bridgeFrames).toHaveLength(0);
    expect(sources[1].bridgeFrames.length).toBeGreaterThan(0);
    expect(s.calls.filter(c => c.operation === 'extract')).toHaveLength(3);
    expect(s.calls.filter(c => c.operation === 'mapper')).toHaveLength(2);
  });
});

test.describe('manual per-source Bridge budget', () => {
  test.use({ scenario: { name: 'manual-bridge', inputs: [{ name: 'a.mp4', kind: 'video', duration: 5 }, { name: 'b.mp4', kind: 'video', duration: 5, fps: 60 }],
    faults: [{ operation: 'colmap:mapper', occurrence: 1, outcome: 'success', registered: 40 }],
  } });
  test('each video freezes at most twenty percent extra without sharing budget', async ({ app }) => {
    await app.chooseInputs([0, 1]); await app.quality('fast');
    await app.page.getByRole('button', { name: '更多设置', exact: true }).click();
    for (const index of [0, 1]) {
      await app.page.locator('.frame-sampling-material-picker select').selectOption({ index });
      const slider = app.page.locator('.frame-sampling-track input');
      await slider.focus(); await slider.press('Home'); await expect(app.page.locator('.frame-sampling-head output')).toHaveText('6 FPS');
      await slider.press('ArrowRight'); await expect(app.page.locator('.frame-sampling-head output')).toHaveText('7 FPS');
    }
    await app.generate(); const s = await app.waitTerminal();
    const sources = s.projects[0].state.inputs.sources;
    expect(sources.map((source: any) => source.frames.initialExtractedFrames)).toEqual([35, 35]);
    expect(sources[0].bridgeFrames).toHaveLength(0);
    expect(sources[1].bridgeFrames.length).toBeGreaterThan(0);
    expect(sources[1].bridgeFrames.length).toBeLessThanOrEqual(7);
    expect(sources.map((source: any) => source.frames.rescueMaxFrames)).toEqual([42, 42]);
    expect(s.calls.filter(c => c.operation === 'mapper')).toHaveLength(2);
  });
});
