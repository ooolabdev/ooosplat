import { test, expect, type BarrierRule } from '../fixtures/native';
import { mixedInputs, generate, derive, completed, interrupt, assertPreserved, assertFrozenMasks, cacheArchives, json, type StopMode } from '../fixtures/lifecycle';
import { flags, ordinary } from '../expected/recipes';
import { readdir, readFile } from 'node:fs/promises';
import path from 'node:path';

const checkpoints = ['creationIntent', 'project', 'configuration', 'sourcePrepared', 'prepared', 'features', 'matching', 'reconstruction', 'trainingDataset', 'candidate', 'candidateSummary', 'publication:prepared', 'publication:outputInstalled', 'publication:committed', 'projectState', 'projectMetadata', 'publication:projectViewsReconciled', 'taskView', 'publication:reconciled'];
const windows: [string, BarrierRule['boundary'], StopMode, string?][] = [
  ['project', 'written', 'crash'], ['configuration', 'written', 'crash'],
  ['analysis', 'entered', 'cancel'], ['analysis', 'written', 'crash'], ['sourcePlan', 'written', 'crash'],
  ['sourcePrepared', 'written', 'crash', 'alpha'], ['prepared', 'written', 'crash'],
  ['images', 'partial', 'cancel', 'alpha'], ['masks', 'partial', 'cancel', 'alpha'], ['ffmpeg:extract', 'entered', 'cancel'], ['ffmpeg:extract', 'written', 'crash'],
  ['features', 'partial', 'cancel'], ['features', 'written', 'crash'],
  ['matchingIntra', 'partial', 'cancel'], ['matchingCross', 'partial', 'cancel'], ['matching', 'written', 'crash'],
  ['model', 'partial', 'cancel'], ['colmap:mapper', 'written', 'crash'], ['reconstruction', 'written', 'crash'],
  ['trainingDataset', 'partial', 'cancel'], ['trainingDatasetFiles', 'written', 'crash'],
  ['training', 'partial', 'cancel'], ['brush:train', 'written', 'crash'], ['candidate', 'written', 'crash'], ['candidateSummary', 'written', 'crash'],
  ['publication:installed', 'written', 'crash'],
];
const cases = [
  ...checkpoints.flatMap(operation => (['cancel', 'cancel-restart', 'crash'] as const).map(mode => ({operation, boundary:'checkpoint' as const, mode, source:operation==='sourcePrepared'?'first.mp4':undefined}))),
  ...windows.map(([operation,boundary,mode,source]) => ({operation,boundary,mode,source})),
];
for (const kind of ['ordinary', 'reshoot'] as const) for (const point of [...cases,...(kind==='reshoot'?(['cancel','cancel-restart','crash'] as const).map(mode=>({operation:'sourcePrepared',boundary:'checkpoint' as const,mode,source:'__source'})):[])]) {
  const label = `${kind} ${point.operation} ${point.boundary} ${point.mode}${point.source==='__source'?' frozen ancestor':''}`;
  test.describe(`lifecycle ${label}`, () => {
    test.use({ scenario: { name: label, sharedJsonReaders:true,
      inputs:kind==='ordinary'?mixedInputs:[{name:'base',kind:'images'},...mixedInputs],
      barriers:[{id:'stop',operation:point.operation,boundary:point.boundary,source:point.source,task:kind==='ordinary'?1:2}],
    }});
    test('continues the same durable work order with no refreshed allowances', async ({app}) => {
      let ref;
      if (kind==='reshoot') { const a=await generate(app);await completed(app,a.taskId);ref=await derive(app,a,[1,2,3]); }
      else ref=await generate(app,[0,1,2]);
      const hit=await app.waitHit('stop');
      expect(hit.scope.taskId).toBe(ref.taskId);expect(hit.scope.runId).toBe(ref.runId);
      if(point.source==='__source')expect(hit.scope.sourceId).toBe('__source');else if(point.source)expect(hit.scope.sourceName).toBe(point.source);
      const before=await app.snapshot();
      if(['images','masks'].includes(point.operation)){
        const prefix=`b${app.project(before,ref.taskId).metadata.id.replaceAll('-','')}_s0002_`;
        const files=(await readdir(hit.evidence.path)).filter(name=>name.startsWith(prefix));
        expect(files).toHaveLength(1);
        expect([...(await readFile(path.join(hit.evidence.path,files[0]))).subarray(0,8)]).toEqual([137,80,78,71,13,10,26,10]);
      }
      if(point.operation==='model')expect(before.calls.some(call=>call.scope?.taskId===ref.taskId&&call.scope.runId===ref.runId&&['mapper','pose_prior_mapper'].includes(call.operation)&&call.outcome==='running')).toBe(true);
      if (point.boundary==='partial') expect(hit.evidence.written).toBeLessThan(hit.evidence.total);
      const canRestorePreparedBase = kind === 'reshoot' && point.operation === 'features' && (
        (point.boundary === 'partial' && point.mode === 'cancel') ||
        (point.boundary === 'written' && point.mode === 'crash')
      );
      const archivesBefore = canRestorePreparedBase ? await cacheArchives(app.project(before, ref.taskId).path) : undefined;
      await interrupt(app,hit,point.mode);
      const after=await completed(app,ref.taskId);
      if(kind==='reshoot')await assertFrozenMasks(app,after,ref.taskId);
      expect(after.lifecycle.hits.filter(h=>h.ruleId==='stop')).toHaveLength(1);
      expect(after.lifecycle.consumed).toContain('stop');
      const oldTask=app.task(before,ref.taskId),newTask=app.task(after,ref.taskId);
      expect(newTask.creation_intent).toEqual(oldTask.creation_intent);
      const training=after.calls.filter(c=>c.scope?.taskId===ref.taskId&&c.operation==='train');
      for(const call of training)expect(flags(call.args)['--total-train-iters']).toBe(String(ordinary.fast.steps));
      expect(app.project(after,ref.taskId).state.inputs.executionPlan.trainingSeed.kind).toBe(kind==='reshoot'?'gaussianSnapshot':'sparsePoints');
      if (point.operation==='training' || point.operation==='brush:train') {
        expect(training).toHaveLength(2);
        expect(flags(training[1].args)).toEqual(flags(training[0].args));
        expect(training[1].datasetInitSha256).toEqual(training[0].datasetInitSha256);
        expect(training[1].runId).not.toBe(training[0].runId);
      }
      if (point.operation.startsWith('publication:') || ['projectState','projectMetadata','taskView','candidateSummary'].includes(point.operation) || (point.operation==='candidate'&&point.boundary==='checkpoint')) expect(training).toHaveLength(1);
      if(point.operation==='candidate'&&point.boundary==='written')expect(training).toHaveLength(2);
      const rerun=point.boundary==='partial'?({features:'feature_extractor',matchingIntra:'matches_importer',matchingCross:'matches_importer',model:'mapper'} as Record<string,string>)[point.operation]:point.boundary==='written'?({features:'feature_extractor',matching:'matches_importer',reconstruction:'mapper','colmap:mapper':'mapper'} as Record<string,string>)[point.operation]:undefined;
      if(rerun)expect(after.calls.filter(c=>c.scope?.taskId===ref.taskId&&c.operation===rerun).length).toBeGreaterThan(before.calls.filter(c=>c.scope?.taskId===ref.taskId&&c.operation===rerun).length);
      if (newTask.run_id!==oldTask.run_id) { expect(newTask.runs).toHaveLength(oldTask.runs.length+1);expect(newTask.runs.at(-1).kind).toBe('continue'); }
      if (before.projects.some(p=>p.taskId===ref.taskId)) await assertPreserved(app,before,after,ref.taskId,archivesBefore);
      expect(app.project(after,ref.taskId).state.generationPolicy).toEqual(oldTask.request_parameters.generationPolicy);
      expect(newTask.generation_policy).toEqual(oldTask.request_parameters.generationPolicy);
    });
  });
}

test.describe('durable task policy snapshot',()=>{
  test.use({scenario:{name:'durable-task-policy',inputs:[{name:'photos',kind:'images'}]}});
  test('completed task records retain the independently saved generation policy',async({app})=>{
    const ref=await generate(app),snapshot=await completed(app,ref.taskId);
    const expected={version:7,autoRetryEnabled:true,frameSamplingRatio:null,frameSamplingFps:null};
    const assertPolicy=async(current:typeof snapshot)=>{
      expect(app.project(current,ref.taskId).state.generationPolicy).toEqual(expected);
      const task=app.task(current,ref.taskId);
      expect(task.generation_policy).toEqual(expected);
      expect(task.request_parameters.generationPolicy).toEqual(expected);
      expect(task.actual_configuration.generation_policy).toEqual(expected);
      expect((await json(path.join(app.root,'app-data/tasks.json'))).tasks[ref.taskId].generation_policy).toEqual(expected);
    };
    await assertPolicy(snapshot);
    await app.stop();await app.start();
    const restored=await completed(app,ref.taskId);
    await assertPolicy(restored);
    expect(app.task(restored,ref.taskId).run_id).toBe(ref.runId);
    expect(app.task(restored,ref.taskId).runs).toEqual(app.task(snapshot,ref.taskId).runs);
    expect(restored.calls.filter(call=>call.scope?.taskId===ref.taskId&&call.operation==='train')).toEqual(snapshot.calls.filter(call=>call.scope?.taskId===ref.taskId&&call.operation==='train'));
  });
});
