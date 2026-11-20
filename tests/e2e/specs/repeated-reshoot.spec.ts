import { test, expect, type NativeApp, type TaskRef, type Snapshot } from '../fixtures/native';
import { generate, derive, completed, criticalHashes, sha, json, interrupt, assertFrozenMasks } from '../fixtures/lifecycle';
import path from 'node:path';
import { assertInitialization } from '../expected/lifecycle';
import { flags, ordinary } from '../expected/recipes';

const inputs = ['a','b','c','d'].map(name=>({name,kind:'images' as const}));
async function seed(app: NativeApp) { const ref=await generate(app); const snapshot=await completed(app,ref.taskId); return {ref,snapshot}; }
async function checkParent(app: NativeApp, parent: TaskRef, child: TaskRef, warm: boolean, snapshot: Snapshot) {
  const original=app.project(snapshot,parent.taskId),derived=app.project(snapshot,child.taskId);
  const digest=await sha(path.join(original.path,'final.ply'));
  const manifest=await json(path.join(derived.path,'work/reshoot-snapshot-v3/snapshot.json'));
  expect(manifest.sourceProjectId).toBe(original.metadata.id);
  expect(manifest.sourcePly.sha256).toBe(digest);
  const train=snapshot.calls.filter(c=>c.scope?.taskId===child.taskId&&c.operation==='train').at(-1)!;
  const actual=flags(train.args);
  for(const [flag,value] of Object.entries({'--total-train-iters':ordinary.fast.steps,'--refine-every':ordinary.fast.refine,'--growth-grad-threshold':ordinary.fast.gradient,'--growth-select-fraction':ordinary.fast.fraction,'--growth-stop-iter':ordinary.fast.stop}))expect(actual[flag]).toBe(String(value));
  assertInitialization(train.datasetInitSha256?.map(pair=>pair[1])??[],warm?[digest]:[]);
  expect(await sha(path.join(derived.path,'final.ply'))).not.toBe(digest);
  expect(derived.state.inputs.executionPlan.trainingSeed.kind).toBe(warm?'gaussianSnapshot':'sparsePoints');
}

for (const route of ['warm-chain','warm-cold-warm','full-then-incremental'] as const) test.describe(`repeated ${route}`,()=>{
  test.use({scenario:{name:route,inputs,sharedJsonReaders:true}});
  test('each round freezes its current parent and preserves every ancestor',async({app})=>{
    let {ref:parent,snapshot}=await seed(app);
    const ancestors=new Map<string,Record<string,string>>();
    const hashes=new Set<string>([await sha(path.join(app.project(snapshot,parent.taskId).path,'final.ply'))]);
    const answers=route==='warm-cold-warm'?['good','appearance','good'] as const:route==='full-then-incremental'?['geometry','good'] as const:['good','good','good'] as const;
    for (let round=0;round<answers.length;round++) {
      ancestors.set(app.project(snapshot,parent.taskId).path,await criticalHashes(app.project(snapshot,parent.taskId).path));
      const child=await derive(app,parent,[round+1],answers[round]);snapshot=await completed(app,child.taskId);
      await checkParent(app,parent,child,answers[round]==='good',snapshot);
      expect(app.project(snapshot,child.taskId).state.inputs.executionPlan.reconstruction.kind).toBe(answers[round]==='geometry'?'full':'incremental');
      const digest=await sha(path.join(app.project(snapshot,child.taskId).path,'final.ply'));expect(hashes.has(digest)).toBe(false);hashes.add(digest);
      for (const [root,expected] of ancestors) expect(await criticalHashes(root)).toEqual(expected);
      parent=child;
    }
  });
});

test.describe('sibling reshoots',()=>{
  test.use({scenario:{name:'siblings',inputs}});
  test('B and C both initialize from A and never from their sibling',async({app})=>{
    const {ref:a,snapshot:base}=await seed(app);const root=app.project(base,a.taskId).path;const original=await criticalHashes(root);
    const b=await derive(app,a,[1]);let snapshot=await completed(app,b.taskId);await checkParent(app,a,b,true,snapshot);
    const bRoot=app.project(snapshot,b.taskId).path,bHash=await criticalHashes(bRoot);
    const c=await derive(app,a,[2]);snapshot=await completed(app,c.taskId);await checkParent(app,a,c,true,snapshot);
    expect(await sha(path.join(app.project(snapshot,c.taskId).path,'final.ply'))).not.toBe(await sha(path.join(bRoot,'final.ply')));
    expect(await criticalHashes(root)).toEqual(original);expect(await criticalHashes(bRoot)).toEqual(bHash);
  });
});

test.describe('recovered original then reshoot',()=>{
  test.use({scenario:{name:'recovered-original',inputs,barriers:[{id:'first-stop',operation:'training',boundary:'partial',task:1}]}});
  test('the recovered publication becomes the frozen parent',async({app})=>{
    const a=await generate(app);await interrupt(app,await app.waitHit('first-stop'),'crash');const snapshot=await completed(app,a.taskId);
    const root=app.project(snapshot,a.taskId).path,original=await criticalHashes(root);
    const b=await derive(app,a,[1]);await checkParent(app,a,b,true,await completed(app,b.taskId));expect(await criticalHashes(root)).toEqual(original);
  });
});

test.describe('repeated interruption of one reshoot',()=>{
  test.use({scenario:{name:'repeat-interruption',inputs,barriers:[
    {id:'prepare',operation:'sourcePrepared',boundary:'checkpoint',task:2,source:'b'},
    {id:'features',operation:'features',boundary:'checkpoint',task:2},
    {id:'training',operation:'training',boundary:'partial',task:2},
  ]}});
  test('new runs preserve source, route and initialization across three stops',async({app})=>{
    const {ref:a}=await seed(app);const b=await derive(app,a,[1]);
    for (const [id,mode] of [['prepare','cancel'],['features','cancel-restart'],['training','crash']] as const) await interrupt(app,await app.waitHit(id),mode);
    const snapshot=await completed(app,b.taskId);expect(app.task(snapshot,b.taskId).runs).toHaveLength(4);
    await assertFrozenMasks(app,snapshot,b.taskId);
    await checkParent(app,a,b,true,snapshot);expect(snapshot.lifecycle.consumed.sort()).toEqual(['features','prepare','training']);
  });
});

for (const exhausted of [false,true]) test.describe(`OOM allowance ${exhausted?'exhausted':'retained'}`,()=>{
  test.use({scenario:{name:`oom-continue-${exhausted}`,inputs,
    faults:[{operation:'brush:train',occurrence:1,task:2,outcome:'oom'},...(exhausted?[{operation:'brush:train',occurrence:3,task:2,outcome:'oom' as const}]:[])],
    barriers:[{id:'reduced-training',operation:'training',boundary:'partial',task:2}],
  }});
  test('an interrupted reduced attempt never receives a new OOM allowance',async({app})=>{
    const {ref:a}=await seed(app);const b=await derive(app,a,[1]);await interrupt(app,await app.waitHit('reduced-training'),'cancel-restart');
    const snapshot=exhausted?await app.waitTerminal('failed',b.taskId):await completed(app,b.taskId);
    const project=app.project(snapshot,b.taskId);expect(project.state.brushTraining.oomRetryUsed).toBe(true);
    expect(app.task(snapshot,b.taskId).automatically_retried).toBe(true);
    const train=snapshot.calls.filter(c=>c.scope?.taskId===b.taskId&&c.operation==='train');expect(train).toHaveLength(3);
    expect(train[1].args).toContain('800');expect(train[2].args).toEqual(train[1].args);
    if (!exhausted) {const c=await derive(app,b,[2]);await checkParent(app,b,c,true,await completed(app,c.taskId));}
  });
});

for(const exhausted of [false,true])test.describe(`cold fallback continuation ${exhausted?'exhausted':'retained'}`,()=>{
  test.use({scenario:{name:`cold-continue-${exhausted}`,inputs,faults:[{operation:'brush:train',occurrence:1,task:2,outcome:'zero'},...(exhausted?[{operation:'brush:train',occurrence:2,task:2,outcome:'zero' as const}]:[])],barriers:[{id:'cold',operation:'fallbackConsumed',boundary:'checkpoint',task:2}]}});
  test('the consumed Warm fallback and actual Cold route survive restart',async({app})=>{
    const {ref:a}=await seed(app);const b=await derive(app,a,[1]);await interrupt(app,await app.waitHit('cold'),'crash');let snapshot=exhausted?await app.waitTerminal('failed',b.taskId):await completed(app,b.taskId);
    const project=app.project(snapshot,b.taskId);expect(project.state.reshoot.recipe.trainingFallbacks).toBe(1);expect(project.state.reshoot.recipe.actual.training).toBe('cold');
    expect(app.task(snapshot,b.taskId).automatically_retried).toBe(true);
    expect(snapshot.calls.filter(c=>c.scope?.taskId===b.taskId&&c.operation==='train').map(c=>c.datasetInitFiles.length)).toEqual([1,0]);
    if(!exhausted){const c=await derive(app,b,[2]);snapshot=await completed(app,c.taskId);await checkParent(app,b,c,true,snapshot);}
  });
});

test.describe('partial adoption publication then reshoot',()=>{
  test.use({scenario:{name:'partial-next-round',inputs,faults:[1,2].map(occurrence=>({operation:'colmap:mapper',occurrence,task:2,outcome:'success' as const,registeredBySource:{b:30,c:0}})),barriers:[{id:'publish',operation:'publication:prepared',boundary:'checkpoint',task:2}]}});
  test('unused sources remain recorded after publish recovery and the next round',async({app})=>{
    const {ref:a}=await seed(app);const b=await derive(app,a,[1,2]);await interrupt(app,await app.waitHit('publish'),'crash');let snapshot=await completed(app,b.taskId);
    const root=app.project(snapshot,b.taskId).path,original=await criticalHashes(root),adoption=app.project(snapshot,b.taskId).state.inputs.sourceAdoption;
    expect(adoption.map((s:any)=>s.adopted)).toEqual([true,false]);
    await app.selectTask(b.taskId);await expect(app.page.getByRole('region',{name:'任务详情',exact:true}).locator('.input-adoption-warning').filter({hasText:'c'})).toBeVisible();
    const d=await derive(app,b,[3]);snapshot=await completed(app,d.taskId);await checkParent(app,b,d,true,snapshot);expect(await criticalHashes(root)).toEqual(original);
  });
});

test.describe('per-source low and disconnected registration',()=>{
  test.use({scenario:{name:'per-source-low',inputs,faults:[1,2].map(occurrence=>({operation:'colmap:mapper',occurrence,task:2,outcome:'success' as const,registeredBySource:{b:30,c:10,d:0}}))}});
  test('a strong first source cannot hide low and zero registration in later sources',async({app})=>{
    const {ref:a}=await seed(app);const b=await derive(app,a,[1,2,3]);const snapshot=await completed(app,b.taskId),project=app.project(snapshot,b.taskId);
    const adoption=project.state.inputs.sourceAdoption;expect(adoption.map((s:any)=>s.registeredFrames)).toEqual([30,10,0]);expect(adoption.map((s:any)=>s.adopted)).toEqual([true,true,false]);
    await app.selectTask(b.taskId);const detail=app.page.getByRole('region',{name:'任务详情',exact:true});await expect(detail.locator('.input-adoption-warning').filter({hasText:'d'})).toBeVisible();await expect(detail.locator('.input-adoption-warning').filter({hasText:/40\/90/})).toBeVisible();
  });
});

test.describe('all added sources unregistered',()=>{
  test.use({scenario:{name:'all-zero-per-source',inputs,faults:[1,2,3,4].map(occurrence=>({operation:'colmap:mapper',occurrence,task:2,outcome:'success' as const,registeredBySource:{b:0,c:0}}))}});
  test('cannot publish an unchanged old scene or launch training',async({app})=>{
    const {ref:a,snapshot:original}=await seed(app);const root=app.project(original,a.taskId).path,hashes=await criticalHashes(root);
    await app.reshoot(a.taskId);await app.chooseInputs([1,2]);await app.page.locator('.planner-switch').click();await app.generate();const dialog=app.page.locator('.preflight-dialog');await dialog.locator('input[value=appearance]').check();await dialog.locator('button.primary-action').click();
    for(let n=0;n<2;n++){await expect(dialog.locator('input[value=unsure]')).toBeVisible();await dialog.locator('input[value=unsure]').check();await dialog.locator('button.primary-action').click();}
    const b=await app.reference(a.taskId);const snapshot=await app.waitTerminal('failed',b.taskId);expect(snapshot.calls.filter(c=>c.scope?.taskId===b.taskId&&c.operation==='train')).toHaveLength(0);expect(app.task(snapshot,b.taskId).result).toBeNull();expect(await criticalHashes(root)).toEqual(hashes);
  });
});
