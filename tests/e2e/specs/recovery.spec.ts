import { test, expect } from '../fixtures/native';
import { flags } from '../expected/recipes';

test.describe('OOM retries', () => {
  test.use({ scenario: { name: 'oom', inputs: [{ name: 'photos', kind: 'images' }], faults: [{ operation: 'brush:train', occurrence: 1, outcome: 'oom' }] } });
  test('only the ceiling changes and the task records its one controlled retry', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    const s = await app.waitTerminal();
    const runs = s.calls.filter(c => c.operation === 'train'); expect(runs).toHaveLength(2);
    const first = flags(runs[0].args), second = flags(runs[1].args);
    expect(second['--max-splats']).toBe('800');
    delete first['--max-splats']; delete second['--max-splats'];
    expect(first).toEqual(second);
    expect(app.task(s).automatically_retried).toBe(true);
  });
  test('disabled automatic retry fails after exactly one launch', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast');
    await app.page.locator('.planner-switch').click(); await app.generate();
    const s = await app.waitTerminal('failed');
    expect(s.calls.filter(c => c.operation === 'train')).toHaveLength(1);
    expect(app.project(s).state.brushTraining.oomRetryUsed).toBe(false);
  });
});
test.describe('cancel and restart', () => {
  test.use({ scenario: { name: 'continue', inputs: [{ name: 'photos', kind: 'images' }], faults: [{ operation: 'brush:train', occurrence: 1, outcome: 'success', gate: 'first-training' }] } });
  test('continuation retains the recipe and reuses reconstruction across application restart', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate();
    await expect.poll(async () => (await app.snapshot()).pending).toContain('first-training');
    const before = await app.snapshot();
    await app.cancelTask();
    await app.waitTerminal('cancelled');
    await app.stop(); await app.start();
    await app.continueTask();
    const after = await app.waitTerminal();
    expect(after.calls.filter(c => c.operation === 'mapper')).toHaveLength(1);
    const train = after.calls.filter(c => c.operation === 'train'); expect(train).toHaveLength(2);
    expect(flags(train[0].args)).toEqual(flags(train[1].args));
    expect(app.project(after).state.generationPolicy).toEqual(app.project(before).state.generationPolicy);
  });
});
