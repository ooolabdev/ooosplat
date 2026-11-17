import type { Call, Snapshot } from '../fixtures/native';
import { flags } from './recipes';

/** Shared by native positive cases and deliberately broken negative controls.
 * Do not derive expectations by importing a production planner/resolver. */
export function assertTraining(s: Pick<Snapshot, 'calls'>, expected: { steps: number; cap?: number; initFiles: number; launches?: number }) {
  const calls = s.calls.filter(c => c.engine === 'brush' && c.operation === 'train');
  if (calls.length !== (expected.launches ?? 1)) throw new Error('Unexpected Brush launch count');
  for (const call of calls) {
    const args = flags(call.args);
    if (Number(args['--total-train-iters']) !== expected.steps) throw new Error('Unexpected training iterations');
    if (expected.cap != null && Number(args['--max-splats']) !== expected.cap) throw new Error('Unexpected Splat ceiling');
    if (call.datasetInitFiles.length !== expected.initFiles) throw new Error('Unexpected Gaussian initialization');
  }
}
export function assertSampling(plan: { samplingFps: number; initialExtractedFrames: number }, fps: number, count: number) {
  if (Math.abs(plan.samplingFps - fps) > 0.005 || plan.initialExtractedFrames !== count) throw new Error('Unexpected initial frame plan');
}
export const trainingCall = (steps: number): Call => ({ sequence: 1, engine: 'brush', operation: 'train', args: ['--total-train-iters', String(steps)], environment: [], outcome: 'success', datasetInitFiles: [] });
