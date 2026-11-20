import { test, expect } from '../fixtures/native';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

test.describe('saved concrete GPU', () => {
  test.use({ scenario: { name: 'saved-gpu', multipleGpus: true, inputs: [{ name: 'photos', kind: 'images' }] } });
  test('remembered choice is reused by the next task without asking again', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const dialog = app.page.getByRole('dialog', { name: '选择生成使用的显卡' });
    await dialog.locator('.gpu-device-option').filter({hasText:'E2E GPU 1'}).locator('input[type=radio]').check();
    await dialog.locator('input[type=checkbox]').check();
    await dialog.getByRole('button', { name: '开始生成', exact: true }).click();
    await app.waitTerminal();
    const settings = JSON.parse(await readFile(path.join(app.root, 'app-data/settings.json'), 'utf8'));
    expect(settings.hardware.defaultGpu.localDeviceId).toBe('e2e-gpu-1');
    expect(settings.hardware.askOnMultipleGpus).toBe(false);
    await app.page.locator('.new-task-group .group-add-action').click();
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const s = await app.waitCompleted(2);
    expect(s.calls.filter(c => c.operation === 'train').map(c => Object.fromEntries(c.environment).CUBECL_WGPU_DEFAULT_DEVICE)).toEqual(['DiscreteGpu(1)', 'DiscreteGpu(1)']);
    await expect(dialog).toHaveCount(0);
  });
});

test.describe('partial reshoot adoption', () => {
  test.use({ scenario: { name: 'partial-reshoot', inputs: [{ name: 'original', kind: 'images' }, { name: 'connected', kind: 'images' }, { name: 'disconnected', kind: 'images' }],
    faults: [{ operation: 'colmap:mapper', occurrence: 2, outcome: 'success', registered: 60 }, { operation: 'colmap:mapper', occurrence: 3, outcome: 'success', registered: 60 }],
  } });
  test('one connected source can finish while the omitted source remains recorded after restart', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate(); await app.waitTerminal();
    await app.reshoot();
    await app.chooseInputs([1, 2]); await app.generate();
    const dialog = app.page.locator('.preflight-dialog');
    await dialog.locator('input[value=appearance]').check(); await dialog.locator('button.primary-action').click();
    // Ordinary camera relationship is answered separately for each source.
    for (let i = 0; i < 2; i++) {
      await expect(dialog.locator('input[value=unsure]')).toBeVisible();
      await dialog.locator('input[value=unsure]').check(); await dialog.locator('button.primary-action').click();
    }
    const s = await app.waitCompleted(2);
    const derived = app.project(s);
    const adoption = derived.state.inputs.sourceAdoption;
    expect(adoption.map((source: any) => source.registeredFrames)).toEqual([30, 0]);
    expect(adoption.map((source: any) => source.adopted)).toEqual([true, false]);
    await app.stop(); await app.start();
    expect((await app.snapshot()).projects.find(p => p.taskId === derived.taskId)!.state.inputs.sourceAdoption).toEqual(adoption);
  });
});

for (const outcome of ['geometry', 'io'] as const) {
  test.describe(`initial mapper ${outcome}`, () => {
    test.use({ scenario: { name: `initial-${outcome}`, inputs: [{ name: 'photos', kind: 'images' }], faults: [{ operation: 'colmap:mapper', occurrence: 1, outcome }] } });
    test('only explicit geometry failure may try another initialization seed', async ({ app }) => {
      await app.chooseInputs(); await app.quality('fast'); await app.generate();
      const s = await app.waitTerminal(outcome === 'geometry' ? 'completed' : 'failed');
      const mappers = s.calls.filter(c => c.operation === 'mapper');
      if (outcome === 'geometry') {
        expect(mappers.length).toBeGreaterThan(1); expect(mappers.length).toBeLessThanOrEqual(4);
        expect(mappers[1].args).toContain('--Mapper.init_image_id1');
        expect(app.task(s).automatically_retried).toBe(false);
      } else {
        expect(mappers).toHaveLength(1);
        expect(s.calls.filter(c => c.operation === 'train')).toHaveLength(0);
      }
    });
  });
}
