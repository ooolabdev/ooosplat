import { test, expect } from '../fixtures/native';

test.describe('101 photos', () => {
  test.use({ scenario: { name: 'order-101', inputs: [{ name: 'ordered', kind: 'images', count: 101 }] } });
  test('no default order answer, no background task before confirmation', async ({ app }) => {
    await app.chooseInputs();
    await expect(app.page.locator('.material-order-question')).toBeVisible();
    await expect(app.page.locator('.material-order-question input:checked')).toHaveCount(0);
    await expect(app.page.getByRole('button', { name: '开始生成', exact: true }).first()).toBeDisabled();
    expect((await app.snapshot()).tasks).toHaveLength(0);
    await app.page.locator('.material-order-question input[value=sequential]').check();
    await app.quality('fast'); await app.generate();
    const s = await app.waitTerminal();
    expect(s.calls.some(c => c.operation === 'sequential_matcher')).toBe(true);
    expect(s.calls.some(c => c.operation === 'exhaustive_matcher')).toBe(false);
  });
});
test.describe('100 photos', () => {
  test.use({ scenario: { name: 'order-100', inputs: [{ name: 'unordered', kind: 'images', count: 100 }] } });
  test('100 photos retain exhaustive matching without an order question', async ({ app }) => {
    await app.chooseInputs(); await expect(app.page.locator('.material-order-question')).toHaveCount(0);
    await app.quality('fast'); await app.generate(); const s = await app.waitTerminal();
    expect(s.calls.some(c => c.operation === 'exhaustive_matcher')).toBe(true);
  });
});
test.describe('multiple GPUs', () => {
  test.use({ scenario: { name: 'gpu', multipleGpus: true, inputs: [{ name: 'photos', kind: 'images' }] } });
  test('selection is a concrete device and binds both engines', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const dialog = app.page.getByRole('dialog', { name: '选择生成使用的显卡' });
    await expect(dialog).toBeVisible();
    await expect(dialog.locator('input[type=radio]').nth(0)).toBeChecked();
    expect((await app.snapshot()).tasks).toHaveLength(0);
    await dialog.locator('input[type=radio]').nth(1).check();
    await dialog.getByRole('button', { name: '开始生成', exact: true }).click();
    const s = await app.waitTerminal();
    const train = s.calls.find(c => c.operation === 'train')!;
    expect(Object.fromEntries(train.environment).CUBECL_WGPU_DEFAULT_DEVICE).toBe('DiscreteGpu(1)');
    expect(s.projects[0].state.resolvedHardware.colmapGpuIndex).toBe(1);
  });
});
