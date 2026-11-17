import { test, expect } from '../fixtures/native';

for (const outcome of ['zero', 'export', 'crash'] as const) {
  test.describe(`warm failure ${outcome}`, () => {
    test.use({ scenario: { name: `warm-${outcome}`, inputs: [{ name: 'original', kind: 'images' }, { name: 'new', kind: 'images' }],
      faults: [{ operation: 'brush:train', occurrence: 2, outcome }],
    } });
    test('only a numerical result may trigger authorized cold fallback', async ({ app }) => {
      await app.chooseInputs(); await app.quality('fast'); await app.generate(); await app.waitTerminal();
      await app.page.getByRole('button', { name: '补拍', exact: true }).first().click();
      await app.chooseInputs([1]); await app.generate(); await app.answerReshoot('good');
      await expect.poll(async () => (await app.snapshot()).tasks.filter(t => ['completed', 'failed'].includes(t.status)).length).toBe(2);
      const s = await app.snapshot();
      const derived = s.projects.find(p => p.state.reshoot?.recipe)!;
      const task = s.tasks.find(t => t.task_id === derived.taskId)!;
      const train = s.calls.filter(c => c.operation === 'train');
      expect(train[1].datasetInitFiles).toHaveLength(1);
      if (outcome === 'zero') {
        expect(task.status).toBe('completed'); expect(train).toHaveLength(3);
        expect(train[2].datasetInitFiles).toHaveLength(0);
        expect(derived.state.reshoot.recipe.trainingFallbacks).toBe(1);
      } else {
        expect(task.status).toBe('failed'); expect(train).toHaveLength(2);
        expect(derived.state.reshoot.recipe.trainingFallbacks).toBe(0);
        expect(task.automatically_retried).toBe(false);
      }
    });
  });
}

test.describe('zero new registration', () => {
  test.use({ scenario: { name: 'no-new-frames', inputs: [{ name: 'original', kind: 'images' }, { name: 'new', kind: 'images' }],
    faults: [{ operation: 'colmap:mapper', occurrence: 2, outcome: 'success', registered: 30 }, { operation: 'colmap:mapper', occurrence: 3, outcome: 'success', registered: 30 }],
  } });
  test('retry authorization disabled cannot publish just the old scene', async ({ app }) => {
    await app.chooseInputs(); await app.quality('fast'); await app.generate(); await app.waitTerminal();
    await app.page.getByRole('button', { name: '补拍', exact: true }).first().click();
    await app.chooseInputs([1]); await app.page.locator('.planner-switch').click();
    await app.generate(); await app.answerReshoot('appearance');
    await expect.poll(async () => (await app.snapshot()).tasks.some(t => t.status === 'failed')).toBe(true);
    const s = await app.snapshot(); expect(s.calls.filter(c => c.operation === 'train')).toHaveLength(1);
    // The normal candidate expansion is allowed even when algorithm fallback
    // is not authorized; neither attempt may turn the old-only model into success.
    expect(s.calls.filter(c => c.operation === 'mapper')).toHaveLength(3);
  });
});
