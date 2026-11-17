import { test, expect } from '@playwright/test';
import { assertTraining, assertSampling, trainingCall } from '../expected/contracts';
import { flags } from '../expected/recipes';

test('recipe assertions reject wrong steps, FPS, unexpected init and duplicate starts', () => {
  const expected = { steps: 8000, initFiles: 0 };
  const good = trainingCall(8000);
  expect(() => assertTraining({ calls: [good] }, expected)).not.toThrow();
  expect(() => assertTraining({ calls: [trainingCall(3000)] }, expected)).toThrow('iterations');
  expect(() => assertTraining({ calls: [{ ...good, datasetInitFiles: ['init.ply'] }] }, expected)).toThrow('initialization');
  expect(() => assertTraining({ calls: [good, good] }, expected)).toThrow('launch count');
  expect(() => assertSampling({ samplingFps: 6, initialExtractedFrames: 60 }, 6, 60)).not.toThrow();
  expect(() => assertSampling({ samplingFps: 12, initialExtractedFrames: 120 }, 6, 60)).toThrow('frame plan');
  expect(() => flags(['--sh-degree'])).toThrow('Missing command value');
  expect(() => flags(['--sh-degree', '--total-train-iters', '8000'])).toThrow('Missing command value');
  expect(() => flags(['--max-splats', '800', '--max-splats', '1600'])).toThrow('Duplicate command flag');
});
