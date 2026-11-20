import { test, expect } from '../fixtures/native';
import { ordinary, flags, brushOptions } from '../expected/recipes';
import { assertTraining } from '../expected/contracts';

for (const quality of ['fast', 'balanced', 'high'] as const) {
  test(`ordinary images: ${quality} reaches the real executor with the approved recipe`, async ({ app }) => {
    await app.chooseInputs();
    await app.quality(quality);
    await app.generate();
    const snapshot = await app.waitTerminal();
    assertTraining(snapshot, { steps: ordinary[quality].steps, cap: ordinary[quality].photoCap, initFiles: 0 });
    const training = snapshot.calls.filter(c => c.engine === 'brush' && c.operation === 'train');
    expect(training).toHaveLength(1);
    const actual = flags(training[0].args);
    const normalized = { ...actual }; delete normalized['--export-path'];
    expect(normalized).toEqual(brushOptions(quality));
    const expected = ordinary[quality];
    expect(Number(actual['--total-train-iters'])).toBe(expected.steps);
    expect(Number(actual['--refine-every'])).toBe(expected.refine);
    expect(Number(actual['--growth-grad-threshold'])).toBeCloseTo(expected.gradient, 10);
    expect(Number(actual['--growth-select-fraction'])).toBeCloseTo(expected.fraction, 10);
    expect(Number(actual['--growth-stop-iter'])).toBe(expected.stop);
    expect(Number(actual['--max-splats'])).toBe(expected.photoCap);
    expect(Number(actual['--max-resolution'])).toBe(96);
    const extract = flags(snapshot.calls.find(c => c.operation === 'feature_extractor')!.args);
    expect(Number(extract['--FeatureExtraction.max_image_size'])).toBe(expected.sfmSize);
    expect(Number(extract['--SiftExtraction.max_num_features'])).toBe(expected.features);
    expect(training[0].datasetInitFiles).toEqual([]);
    expect(training[0].runId).toBe(app.task(snapshot).run_id);
    expect(training[0].pipelineOperation?.id).toBeGreaterThan(0);
    expect(Object.fromEntries(training[0].environment).CUBECL_WGPU_DEFAULT_DEVICE).toBe('DiscreteGpu(0)');
    const state = app.project(snapshot).state;
    expect(state.stage).toBe('completed');
    expect(state.brushTraining.resolved.preset.totalSteps).toBe(expected.steps);
    expect(Number(actual['--max-splats'])).toBe(state.brushTraining.resolved.preset.maxSplats);
    expect(app.project(snapshot).metadata.output.splatCount).toBe(64);
  });
}

test.describe('orientation grouping', () => {
  test.use({ scenario: { name: 'portrait', inputs: [{ name: 'portrait-photos', kind: 'images', portrait: true }] } });
  test('one landscape/portrait set produces two shared camera groups', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const snapshot = await app.waitTerminal();
    expect(snapshot.calls.filter(c => c.operation === 'feature_extractor')).toHaveLength(2);
    expect(snapshot.calls.filter(c => c.operation === 'mapper')).toHaveLength(1);
  });
});
