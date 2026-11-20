import { test, expect } from '../fixtures/native';
import { flags, panorama } from '../expected/recipes';

for (const transparent of [false, true]) {
  test.describe(`Alpha pixels ${transparent}`, () => {
    test.use({ scenario: { name: `alpha-${transparent}`, sharedJsonReaders: true, inputs: [{ name: 'alpha', kind: 'images', alpha: true, transparent }] } });
    test('two-view triangulation follows actual transparency, not the channel alone', async ({ app }) => {
      await app.chooseInputs(); await app.quality('fast'); await app.generate();
      const s = await app.waitTerminal();
      expect(flags(s.calls.find(c => c.operation === 'mapper')!.args)['--Mapper.tri_ignore_two_view_tracks']).toBe(transparent ? '0' : undefined);
      expect(s.calls.some(c => c.operation === 'image_undistorter')).toBe(false);
    });
  });
}
test.describe('mixed ordinary and ERP', () => {
  test.use({ scenario: { name: 'mixed-erp', inputs: [{ name: 'ordinary', kind: 'images' }, { name: 'erp.mp4', kind: 'video', erp: true, width: 128, height: 64 }] } });
  test('one joint reconstruction uses the panorama recipe and authoritative sparse budget', async ({ app }) => {
    await app.chooseInputs([0, 1]); await app.quality('high'); await app.generate();
    const s = await app.waitTerminal();
    expect(s.calls.filter(c => c.operation === 'mapper')).toHaveLength(1);
    const args = flags(s.calls.find(c => c.operation === 'train')!.args);
    expect(Number(args['--total-train-iters'])).toBe(panorama.high.steps);
    expect(Number(args['--max-splats'])).toBe(1280);
    expect(flags(s.calls.find(c => c.operation === 'mapper')!.args)['--Mapper.ba_global_backend']).toBe('CERES');
  });
});
test.describe('non-CUDA selected GPU', () => {
  test.use({ scenario: { name: 'intel', cpuColmap: true, inputs: [{ name: 'photos', kind: 'images' }] } });
  test('COLMAP uses CPU/Ceres while Brush remains pinned to the selected adapter', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const s = await app.waitTerminal();
    expect(flags(s.calls.find(c => c.operation === 'feature_extractor')!.args)['--FeatureExtraction.use_gpu']).toBe('0');
    expect(flags(s.calls.find(c => c.operation === 'mapper')!.args)['--Mapper.ba_global_backend']).toBe('CERES');
    expect(Object.fromEntries(s.calls.find(c => c.operation === 'train')!.environment).CUBECL_WGPU_DEFAULT_DEVICE).toBe('DiscreteGpu(0)');
  });
});
test.describe('invalid mixed dimensions', () => {
  test.use({ scenario: { name: 'wrong-dimensions', inputs: [{ name: 'photos', kind: 'images', mismatchedDimensions: true }] } });
  test('a non-transposed size blocks generation before task creation', async ({ app }) => {
    await app.chooseInputs();
    await expect(app.page.getByRole('button', { name: '开始生成', exact: true })).toBeDisabled();
    expect((await app.snapshot()).tasks).toHaveLength(0);
    expect((await app.snapshot()).calls.filter(c => c.engine === 'colmap')).toHaveLength(0);
  });
});
test.describe('JPEG inputs', () => {
  test.use({ scenario: { name: 'jpeg', inputs: [{ name: 'photos', kind: 'images', jpeg: true }] } });
  test('real JPEG headers pass admission and publication', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const s = await app.waitTerminal(); expect(app.project(s).metadata.output.splatCount).toBe(64);
  });
});
