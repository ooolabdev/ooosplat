import { test, expect } from '../fixtures/native';
import { generate, completed, interrupt } from '../fixtures/lifecycle';
import { flags } from '../expected/recipes';
import { json } from '../fixtures/lifecycle';
import path from 'node:path';

test.describe('disabled retry authorization continuation',()=>{
  test.use({scenario:{name:'disabled-retry-continue',inputs:[{name:'photos',kind:'images'}],faults:[{operation:'brush:train',occurrence:2,task:1,outcome:'oom'}],barriers:[{id:'train',operation:'training',boundary:'partial',task:1}]}});
  test('continuing cannot enable a saved disabled retry policy',async({app})=>{
    await app.chooseInputs();await app.quality('fast');await app.page.locator('.planner-switch').click();await app.generate();const ref=await app.reference();
    const expected={version:7,autoRetryEnabled:false,frameSamplingRatio:null,frameSamplingFps:null};
    const hit=await app.waitHit('train');expect(app.project(await app.snapshot(),ref.taskId).state.generationPolicy).toEqual(expected);
    await interrupt(app,hit,'cancel-restart');const snapshot=await app.waitTerminal('failed',ref.taskId),task=app.task(snapshot,ref.taskId);
    expect(snapshot.calls.filter(c=>c.scope?.taskId===ref.taskId&&c.operation==='train')).toHaveLength(2);
    expect(task.automatically_retried).toBe(false);expect(task.runs).toHaveLength(2);
    expect(app.project(snapshot,ref.taskId).state.generationPolicy).toEqual(expected);
    expect((await json(path.join(app.root,'app-data/tasks.json'))).tasks[ref.taskId].generation_policy).toEqual(expected);
  });
});

test.describe('concrete GPU continuation',()=>{
  test.use({scenario:{name:'gpu-one-continue',multipleGpus:true,inputs:[{name:'photos',kind:'images'}],barriers:[{id:'training',operation:'training',boundary:'partial',task:1}]}});
  test('a non-default GPU remains pinned in commands and durable runs after restart',async({app})=>{
    await app.chooseInputs();await app.quality('fast');await app.generate();const dialog=app.page.getByRole('dialog',{name:'选择生成使用的显卡'});await dialog.locator('.gpu-device-option').filter({hasText:'E2E GPU 1'}).locator('input[type=radio]').check();await dialog.locator('input[type=checkbox]').check();await dialog.getByRole('button',{name:'开始生成',exact:true}).click();
    const ref=await app.reference();await interrupt(app,await app.waitHit('training'),'cancel-restart');const snapshot=await completed(app,ref.taskId);
    expect(app.task(snapshot,ref.taskId).gpu_selection.localDeviceId).toBe('e2e-gpu-1');
    for(const call of snapshot.calls.filter(c=>c.operation==='train'))expect(Object.fromEntries(call.environment).CUBECL_WGPU_DEFAULT_DEVICE).toBe('DiscreteGpu(1)');
    for(const call of snapshot.calls.filter(c=>c.operation==='feature_extractor'))expect(flags(call.args)['--FeatureExtraction.gpu_index']).toBe('1');
    expect(app.task(snapshot,ref.taskId).runs).toHaveLength(2);
  });
});

test.describe('late run and duplicate continuation isolation',()=>{
  test.use({scenario:{name:'late-old-run',inputs:[{name:'photos',kind:'images'}],lateEvent:{id:'old-sample',task:1,operation:'brush:train'},barriers:[
    {id:'old',operation:'training',boundary:'partial',task:1},
    {id:'new',operation:'brush:train',boundary:'entered',task:1,occurrence:2},
  ]}});
  test('rejects stale actions and real delayed callbacks from the previous run',async({app})=>{
    const original=await generate(app);await app.waitHit('old');await app.cancelTask(original.taskId);await app.waitTerminal('cancelled',original.taskId);await app.continueTask(original.taskId);const current=await app.waitHit('new');
    expect(current.scope.runId).not.toBe(original.runId);
    const before=app.task(await app.snapshot(),original.taskId);
    await app.page.evaluate(ref=>(window as any).__TAURI_INTERNALS__.invoke('e2e_deliver_late',{id:'old-sample',taskId:ref.taskId,runId:ref.runId}),original);
    const after=app.task(await app.snapshot(),original.taskId);
    for(const key of ['run_id','stage','operation','sequence','runtime'])expect(after[key]).toEqual(before[key]);
    const errors=await app.page.evaluate(async ref=>{
      const invoke=(window as any).__TAURI_INTERNALS__.invoke;
      return Promise.all(['start_gui_task','cancel_shared_task'].map(async command=>{try{await invoke(command,{taskId:ref.taskId,runId:ref.runId,retryFailedRunId:ref.runId});return 'unexpected success';}catch(error){return (error as any).code;}}));
    },original);
    expect(errors).toEqual(['STALE_RUN_ID','STALE_RUN_ID']);
    expect(app.task(await app.snapshot(),original.taskId).runs).toHaveLength(2);
    await app.releaseHit(current);const snapshot=await completed(app,original.taskId);expect(snapshot.calls.filter(c=>c.operation==='train')).toHaveLength(2);
  });
});

test.describe('duplicate UI starts',()=>{
  test.use({scenario:{name:'double-start',inputs:[{name:'photos',kind:'images'}],barriers:[{id:'train',operation:'brush:train',boundary:'entered',task:1}]}});
  test('double clicks and duplicate admitted starts own one execution',async({app})=>{
    await app.chooseInputs();await app.quality('fast');app.taskId=(await app.page.locator('.project-row[data-draft-id][aria-pressed=true]').getAttribute('data-draft-id'))!;
    await app.page.getByRole('button',{name:'开始生成',exact:true}).dblclick();const ref=await app.reference();const hit=await app.waitHit('train');
    const receipts=await app.page.evaluate(async taskId=>Promise.all([1,2].map(()=>(window as any).__TAURI_INTERNALS__.invoke('start_gui_task',{taskId}))),ref.taskId);
    expect(receipts.map(r=>r.run_id)).toEqual([ref.runId,ref.runId]);expect(receipts.every(r=>!r.accepted)).toBe(true);
    await app.releaseHit(hit);const snapshot=await completed(app,ref.taskId);expect(snapshot.tasks).toHaveLength(1);expect(app.task(snapshot,ref.taskId).runs).toHaveLength(1);expect(snapshot.calls.filter(c=>c.operation==='train')).toHaveLength(1);
  });
});

test.describe('native automatic queue',()=>{
  test.use({scenario:{name:'queue-order',inputs:[{name:'a',kind:'images'},{name:'b',kind:'images',width:120,height:80},{name:'c',kind:'images',portrait:true}],barriers:[{id:'first',operation:'brush:train',boundary:'entered',task:1},{id:'next',operation:'brush:train',boundary:'entered',task:2}]}});
  test('reordered queued tasks keep their own sources and start only after the predecessor',async({app})=>{
    const ids:Record<string,string>={};
    for(const [name,index] of [['a',0],['b',1],['c',2]] as const){if(index)await app.page.locator('.new-task-group .group-add-action').click();await app.chooseInputs([index]);await app.quality('fast');ids[name]=(await app.page.locator('.project-row[data-draft-id][aria-pressed=true]').getAttribute('data-draft-id'))!;}
    const c=app.page.locator(`[data-draft-id="${ids.c}"]`);await c.focus();await c.press('Alt+ArrowUp');
    expect(await app.page.locator('.new-task-group .project-row[data-draft-id]').evaluateAll(nodes=>nodes.map(n=>n.getAttribute('data-draft-id')))).toEqual([ids.a,ids.c,ids.b]);
    await app.page.getByRole('switch',{name:'自动执行下一个任务',exact:true}).click();await app.selectTask(ids.a);await app.generate();const first=await app.waitHit('first');expect(first.scope.taskId).toBe(ids.a);expect((await app.snapshot()).tasks).toHaveLength(1);await app.releaseHit(first);
    const next=await app.waitHit('next');expect(next.scope.taskId).toBe(ids.c);expect((await app.snapshot()).tasks.some(t=>t.task_id===ids.b)).toBe(false);await app.releaseHit(next);
    for(const id of [ids.a,ids.c,ids.b])await completed(app,id);
    const snapshot=await app.snapshot();expect(snapshot.tasks).toHaveLength(3);for(const [name,id] of Object.entries(ids)){const project=app.project(snapshot,id);expect(project.state.inputs.request.items[0].source.path.replaceAll('\\','/')).toMatch(new RegExp(`/fixtures/${name}$`));expect(snapshot.calls.filter(c=>c.scope?.taskId===id&&c.operation==='train')).toHaveLength(1);}
  });
  test('cancellation disables the queue and continuing cannot start a waiting task',async({app})=>{
    await app.chooseInputs();await app.quality('fast');const a=(await app.page.locator('.project-row[data-draft-id][aria-pressed=true]').getAttribute('data-draft-id'))!;
    await app.page.locator('.new-task-group .group-add-action').click();await app.chooseInputs([1]);await app.quality('fast');const b=(await app.page.locator('.project-row[data-draft-id][aria-pressed=true]').getAttribute('data-draft-id'))!;
    await app.page.getByRole('switch',{name:'自动执行下一个任务',exact:true}).click();await app.selectTask(a);await app.generate();await app.waitHit('first');await app.cancelTask(a);await app.waitTerminal('cancelled',a);await expect(app.page.getByRole('switch',{name:'自动执行下一个任务',exact:true})).toHaveAttribute('aria-checked','false');await app.continueTask(a);await completed(app,a);expect((await app.snapshot()).tasks.some(t=>t.task_id===b)).toBe(false);
  });
});
