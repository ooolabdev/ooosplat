import { test, expect } from '../fixtures/native';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

for (const phase of ['prepared', 'committed'] as const) {
  test.describe(`publication ${phase}`, () => {
    test.use({ scenario: { name: `publication-${phase}`, sharedJsonReaders: true, inputs: [{ name: 'photos', kind: 'images' }],
      faults: [{ operation: `publication:${phase}`, occurrence: 1, outcome: 'io', gate: 'publish-boundary' }],
    } });
    test('durable candidate/commit survives restart without starting any engine again', async ({ app }) => {
      await app.chooseInputs(); await app.quality('fast'); await app.generate();
      await expect.poll(async () => (await app.snapshot()).pending).toContain('publish-boundary');
      const before = await app.snapshot();
      expect(app.task(before).status).not.toBe('completed');
      const transaction = JSON.parse(await readFile(path.join(app.project(before).path, 'work/publish-transaction.json'), 'utf8'));
      expect(transaction.phase).toBe(phase);
      await app.release('publish-boundary');
      await app.waitTerminal('failed');
      await app.stop(); await app.start();
      let s = await app.snapshot();
      if (app.task(s).status !== 'completed') {
        await app.continueTask();
        s = await app.waitTerminal();
      }
      expect(s.calls.filter(c => c.engine !== 'publication')).toEqual(before.calls.filter(c => c.engine !== 'publication'));
      expect(app.project(s).metadata.output.splatCount).toBe(64);
    });
  });
}
