import { test, expect } from '../fixtures/native';
import { flags, ordinary } from '../expected/recipes';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';

for (const condition of ['appearance', 'geometry', 'unsure', 'good'] as const) {
  test.describe(`reshoot ${condition}`, () => {
    test.use({ scenario: { name: `reshoot-${condition}`, sharedJsonReaders: true, inputs: [{ name: 'original', kind: 'images' }, { name: 'additional', kind: 'images' }] } });
    test('user answers reach reconstruction and Gaussian initialization without changing the source', async ({ app }) => {
      await app.chooseInputs(); await app.quality('fast'); await app.generate();
      const original = await app.waitTerminal();
      const source = app.project(original);
      const digest = async (name: string) => createHash('sha256').update(await readFile(path.join(source.path, name))).digest('hex');
      const before = await Promise.all(['project.json', 'state.json', 'final.ply'].map(digest));
      await app.reshoot();
      await app.chooseInputs([1]); await app.generate(); await app.answerReshoot(condition, condition === 'good' ? 'same' : condition === 'geometry' ? 'different' : 'unsure');
      const result = await app.waitCompleted(2);
      const training = result.calls.filter(c => c.operation === 'train'); expect(training).toHaveLength(2);
      const actual = flags(training[1].args);
      expect(Number(actual['--total-train-iters'])).toBe(ordinary.fast.steps);
      expect(training[1].datasetInitFiles.length).toBe(condition === 'good' ? 1 : 0);
      if (condition === 'good') expect(training[1].datasetInitSha256?.[0]?.[1]).toBe(before[2]);
      const derived = app.project(result);
      expect(derived.state.inputs.executionPlan.reconstruction.kind).toBe(condition === 'geometry' ? 'full' : 'incremental');
      expect(derived.state.inputs.executionPlan.trainingSeed.kind).toBe(condition === 'good' ? 'gaussianSnapshot' : 'sparsePoints');
      if (condition === 'good') {
        const extract = result.calls.filter(c => c.operation === 'feature_extractor').at(-1)!;
        expect(flags(extract.args)['--ImageReader.existing_camera_id']).toBe('1');
      }
      expect(await Promise.all(['project.json', 'state.json', 'final.ply'].map(digest))).toEqual(before);
    });
  });
}
