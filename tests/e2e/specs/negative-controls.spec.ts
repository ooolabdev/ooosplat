import { test, expect } from '@playwright/test';
import { assertTraining, assertSampling, trainingCall } from '../expected/contracts';
import { flags } from '../expected/recipes';
import {
  assertSourceContract, assertInitialization, assertFrameMapping, assertTrustedCandidate,
  assertNewCacheArchive, assertPreparedBaseRebind, reshootBaseFiles,
} from '../expected/lifecycle';

test('lifecycle assertions reject borrowed sources, refreshed budgets, wrong snapshots and partial outputs',()=>{
  const sources=[{id:'a',samplingFps:6,initialFrames:60,rescueMaxFrames:72},{id:'b',samplingFps:7,initialFrames:35,rescueMaxFrames:42}];
  expect(()=>assertSourceContract(sources,sources)).not.toThrow();
  expect(()=>assertSourceContract([sources[0],{...sources[1],id:'c'}],sources)).toThrow('identity');
  expect(()=>assertSourceContract([sources[0],{...sources[1],samplingFps:6}],sources)).toThrow('sampling');
  expect(()=>assertSourceContract([sources[0],{...sources[1],rescueMaxFrames:49}],sources)).toThrow('budget');
  expect(()=>assertInitialization(['parent-C'],['parent-C'])).not.toThrow();expect(()=>assertInitialization(['original-A'],['parent-C'])).toThrow('initialization');
  const frames=[{sourceIndex:0,pts:4000000},{sourceIndex:1,pts:4034000}];expect(()=>assertFrameMapping(frames,frames)).not.toThrow();expect(()=>assertFrameMapping([{sourceIndex:0,pts:0},frames[1]],frames)).toThrow('PTS');
  expect(()=>assertTrustedCandidate({complete:false,valid:true,identity:'candidate'},'candidate')).toThrow('partial');expect(()=>assertTrustedCandidate({complete:true,valid:false,identity:'candidate'},'candidate')).toThrow('partial');

  const prepared = {
    artifactIdentity: 'a'.repeat(64),
    upstreamIdentity: 'snapshot-C',
    semanticIdentity: null,
    expectedCount: 1,
    files: [...reshootBaseFiles, 'work/frames/frame.jpg'].map(relativePath => ({
      relativePath, fileType: 'regularFile', fileSize: 32, modifiedNanos: 10, sha256: 'b'.repeat(64),
    })),
  };
  const restored = {
    ...prepared,
    artifactIdentity: 'c'.repeat(64),
    files: prepared.files.map(file => reshootBaseFiles.includes(file.relativePath) ? { ...file, modifiedNanos: 20 } : file),
  };
  expect(assertPreparedBaseRebind(prepared, restored)).toHaveLength(4);
  expect(() => assertPreparedBaseRebind(prepared, { ...restored, upstreamIdentity: 'snapshot-A' })).toThrow('upstream identity');
  expect(() => assertPreparedBaseRebind(prepared, {
    ...restored, files: [{ ...restored.files[0], sha256: 'd'.repeat(64) }, ...restored.files.slice(1)],
  })).toThrow('Prepared base file content');
  expect(() => assertPreparedBaseRebind(prepared, {
    ...restored, files: [...restored.files.slice(0, -1), { ...restored.files.at(-1)!, modifiedNanos: 20 }],
  })).toThrow('Unchanged prepared file');
  expect(() => assertPreparedBaseRebind(prepared, { ...restored, files: prepared.files })).toThrow('without a restored base timestamp change');
  expect(assertNewCacheArchive(['old'], ['old', 'continued'])).toBe('continued');
  expect(() => assertNewCacheArchive(['old'], ['old'])).toThrow('Expected one new cache archive');
});

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
