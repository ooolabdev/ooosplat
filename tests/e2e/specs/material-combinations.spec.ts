import { test, expect, type NativeApp, type Scenario } from '../fixtures/native';
import { generate, derive, completed, interrupt, json } from '../fixtures/lifecycle';
import path from 'node:path';
import { writeFile, readdir, readFile } from 'node:fs/promises';
import { assertSourceContract, assertFrameMapping, default300At6, manual300At7 } from '../expected/lifecycle';
import { flags, ordinary, panorama } from '../expected/recipes';

async function material(app: NativeApp, name: string) {
  const row=app.page.locator('.material-item').filter({has:app.page.locator('.material-row-copy > strong').filter({hasText:new RegExp('^'+name.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')+'$')})});
  await expect(row).toHaveCount(1);await row.locator('.material-row-main').click();return row;
}
async function projection(app: NativeApp,name:string) {const row=await material(app,name);const select=row.locator('.video-type-select');if(await select.count())await select.selectOption('equirectangular');}
async function fps(app:NativeApp,name:string,target:'all'|number) {
  const row=await material(app,name),id=(await row.getAttribute('data-material-id'))!;
  const picker=app.page.locator('.frame-sampling-material-picker select');if(await picker.count())await picker.selectOption(id);
  const slider=app.page.locator('.frame-sampling-track input');await slider.focus();
  if(target==='all')await slider.press('End');else{await slider.press('Home');for(let n=6;n<target;n++)await slider.press('ArrowRight');}
  await expect(app.page.locator('.frame-sampling-head output')).toHaveText(`${target==='all'?15:target} FPS`);
}
function source(project:ReturnType<NativeApp['project']>,name:string) {
  const item=project.state.inputs.request.items.find((i:any)=>path.basename(i.source.path)===name);
  if(!item)throw new Error(`Missing requested source ${name}`);
  const actual=project.state.inputs.sources.find((s:any)=>s.id===item.id);if(!actual)throw new Error(`Missing prepared source ${name}`);return actual;
}

test.describe('four material sampling policies',()=>{
  const timestamps=Array.from({length:300},(_,n)=>4+n/29.97+(n%2?.002:0));
  test.use({scenario:{name:'mixed-fps',inputs:[{name:'default.mp4',kind:'video',fps:29.97,duration:10,timestamps},{name:'manual.mp4',kind:'video',fps:60,duration:5},{name:'all.mp4',kind:'video',fps:15,duration:5},{name:'photos',kind:'images',width:120,height:80,portrait:true}]}});
  test('keeps independent plans, PTS, camera groups and matching requests',async({app})=>{
    await app.chooseInputs([0,1,2,3]);await app.quality('fast');await app.page.getByRole('button',{name:'更多设置',exact:true}).click();
    await fps(app,'manual.mp4',7);await fps(app,'all.mp4','all');const uiIds:Record<string,string>={};for(const name of ['default.mp4','manual.mp4','all.mp4'])uiIds[name]=(await (await material(app,name)).getAttribute('data-material-id'))!;await app.generate();const ref=await app.reference();const s=await completed(app,ref.taskId),project=app.project(s);
    for(const [name,rate,count] of [['default.mp4',6,60],['manual.mp4',7,35],['all.mp4',15,75]] as const){const actual=source(project,name);expect(actual.frames.samplingFps).toBe(rate);expect(actual.frames.initialExtractedFrames).toBe(count);}
    const first=source(project,'default.mp4');for(const record of first.panoramaRecords){expect(record.timestampSeconds).toBeCloseTo(timestamps[record.sourceIndex],8);}
    const all=source(project,'all.mp4');expect(all.panoramaRecords.map((r:any)=>r.sourceIndex)).toEqual(Array.from({length:75},(_,n)=>n));
    assertSourceContract(['default.mp4','manual.mp4','all.mp4'].map(name=>{const actual=source(project,name);return {id:actual.id,samplingFps:actual.frames.samplingFps,initialFrames:actual.frames.initialExtractedFrames,rescueMaxFrames:actual.frames.rescueMaxFrames};}),[['default.mp4',6,60,90],['manual.mp4',7,35,42],['all.mp4',15,75,75]].map(([name,samplingFps,initialFrames,rescueMaxFrames])=>({id:uiIds[String(name)],samplingFps:Number(samplingFps),initialFrames:Number(initialFrames),rescueMaxFrames:Number(rescueMaxFrames)})));
    const firstMap=await json(path.join(project.path,'work/inputs/0000/frame-extraction-map.json'));
    expect(firstMap.mappingValid).toBe(true);expect(firstMap.emittedFrames).toHaveLength(60);
    assertFrameMapping(firstMap.emittedFrames.map((f:any)=>({sourceIndex:f.sourceFrameIndex,pts:f.inputPts})),default300At6.map(sourceIndex=>({sourceIndex,pts:Math.round(timestamps[sourceIndex]*1e6)})));
    const manualMap=await json(path.join(project.path,'work/inputs/0001/frame-extraction-map.json'));
    assertFrameMapping(manualMap.emittedFrames.map((f:any)=>({sourceIndex:f.sourceFrameIndex,pts:f.inputPts})),manual300At7.map(sourceIndex=>({sourceIndex,pts:Math.round(sourceIndex/60*1e6)})));
    for(const [index,name] of ['default.mp4','manual.mp4','all.mp4'].entries()){
      const call=s.calls.find(c=>c.operation==='extract'&&c.scope?.sourceId===uiIds[name]);expect(call).toBeTruthy();expect(call!.scope!.sourceName).toBe(name);
      expect(call!.args[call!.args.indexOf('-i')+1].replace(/^\\\\\?\\/, '')).toBe(s.inputs[index]);
    }
    const allMap=await json(path.join(project.path,'work/inputs/0002/frame-extraction-map.json'));
    assertFrameMapping(allMap.emittedFrames.map((f:any)=>({sourceIndex:f.sourceFrameIndex,pts:f.inputPts})),Array.from({length:75},(_,sourceIndex)=>({sourceIndex,pts:Math.round(sourceIndex/15*1e6)})));
    expect(source(project,'photos').cameraIds).toHaveLength(2);
    const groups=project.state.inputs.sources.map((s:any)=>s.cameraIds);expect(new Set(groups.flat()).size).toBe(groups.flat().length);
    const matcher=s.calls.find(c=>c.operation==='matches_importer')!;const pairPath=matcher.args[matcher.args.indexOf('--match_list_path')+1];const pairs=(await readFile(pairPath,'utf8')).trim().split('\n');
    expect(pairs.some(pair=>first.names.some((name:string)=>pair.includes(name))&&all.names.some((name:string)=>pair.includes(name)))).toBe(true);
  });
});

const combinations: {name:string;inputs:Scenario['inputs']}[]=[
  {name:'different image dimensions',inputs:[{name:'small',kind:'images'},{name:'large',kind:'images',width:120,height:80},{name:'rotated',kind:'images',portrait:true}]},
  {name:'ordinary video and images',inputs:[{name:'clip.mp4',kind:'video'},{name:'photos',kind:'images'}]},
  {name:'ordinary video ERP images',inputs:[{name:'clip.mp4',kind:'video'},{name:'erp-photos',kind:'images',width:128,height:64,erp:true}]},
  {name:'ordinary images ERP video',inputs:[{name:'photos',kind:'images'},{name:'erp.mp4',kind:'video',width:128,height:64,erp:true}]},
  {name:'ordinary ERP images',inputs:[{name:'photos',kind:'images'},{name:'erp-photos',kind:'images',width:128,height:64,erp:true}]},
  {name:'ordinary ERP videos',inputs:[{name:'clip.mp4',kind:'video'},{name:'erp.mp4',kind:'video',width:128,height:64,erp:true}]},
  {name:'transparent opaque and same filenames',inputs:[{name:'one/clip.mp4',kind:'video',fps:24,duration:5},{name:'two/clip.mp4',kind:'video',fps:60,duration:3},{name:'alpha',kind:'images',alpha:true,transparent:true},{name:'opaque',kind:'images',alpha:true,transparent:false}]},
];
for(const combination of combinations)test.describe(`combination ${combination.name}`,()=>{
  test.use({scenario:{name:combination.name,inputs:combination.inputs}});
  test('preserves every declared source identity and camera ownership',async({app})=>{
    await app.chooseInputs(combination.inputs.map((_,n)=>n));for(const input of combination.inputs)if(input.erp)await projection(app,path.basename(input.name));
    await app.quality('fast');await app.generate();const ref=await app.reference();const snapshot=await completed(app,ref.taskId);const project=app.project(snapshot);
    expect(project.state.inputs.sources).toHaveLength(combination.inputs.length);
    expect(new Set(project.state.inputs.sources.map((s:any)=>s.identity)).size).toBe(combination.inputs.length);
    const names=project.state.inputs.sources.flatMap((s:any)=>s.names);expect(new Set(names).size).toBe(names.length);
    for(const input of combination.inputs.filter(i=>i.kind==='images'))expect(source(project,path.basename(input.name)).cameraIds.length).toBe(input.portrait?2:1);
    const raw=await json(path.join(project.path,'state.json'));expect(raw.inputs).toEqual(project.state.inputs);
    const train=snapshot.calls.find(c=>c.scope?.taskId===ref.taskId&&c.operation==='train')!;
    expect(Number(flags(train.args)['--total-train-iters'])).toBe(combination.inputs.some(i=>i.erp)?panorama.fast.steps:ordinary.fast.steps);expect(train.datasetInitFiles).toHaveLength(0);
    if(combination.inputs.some(i=>i.transparent))expect(flags(snapshot.calls.find(c=>c.scope?.taskId===ref.taskId&&c.operation==='mapper')!.args)['--Mapper.tri_ignore_two_view_tracks']).toBe('0');
  });
});

test.describe('separate large image order answers',()=>{
  test.use({scenario:{name:'large-order-answers',inputs:[{name:'ordered',kind:'images',count:151},{name:'unordered',kind:'images',count:151}]}});
  test('each set requires its own answer and cross-source retrieval stays bounded',async({app})=>{
    await app.chooseInputs([0,1]);await app.quality('fast');const ordered=await material(app,'ordered');await expect(ordered.locator('input:checked')).toHaveCount(0);await ordered.locator('input[value=sequential]').click();await expect(ordered.locator('.material-row')).toHaveClass(/ready/);
    await expect(app.page.getByRole('button',{name:'开始生成',exact:true})).toBeDisabled();const unordered=await material(app,'unordered');await expect(unordered.locator('input:checked')).toHaveCount(0);await unordered.locator('input[value=exhaustive]').click();await expect(unordered.locator('.material-row')).toHaveClass(/ready/);
    await app.generate();const ref=await app.reference();const snapshot=await completed(app,ref.taskId),project=app.project(snapshot);
    expect(project.state.inputs.request.items.map((i:any)=>i.imageMatchingStrategy)).toEqual(['sequential','exhaustive']);
    expect(snapshot.calls.some(c=>c.operation==='vocab_tree_retriever')).toBe(true);expect(project.state.inputs.sources.map((s:any)=>s.names.length)).toEqual([151,151]);
    const queries=snapshot.calls.filter(c=>c.operation==='vocab_tree_retriever');expect(queries).toHaveLength(2);
    for(const call of queries){const loop=call.args.includes('--num_neighbors');expect(call.args[call.args.indexOf('--num_images')+1]).toBe(loop?'50':'30');
      expect(call.inputLists?.['--database_image_list_path']).toEqual(source(project,'ordered').names);
      expect(call.inputLists?.['--query_image_list_path']).toEqual(loop?source(project,'ordered').names.filter((_:string,index:number)=>index%10===0):source(project,'unordered').names);}
  });
});

for(const kind of ['ordinary','reshoot'] as const)for(const media of ['video','images'] as const)for(const point of [{mode:'cancel',boundary:'partial'},{mode:'cancel-restart',boundary:'partial'},{mode:'crash',boundary:'partial'},{mode:'crash',boundary:'written'}] as const)test.describe(`ERP faces ${kind} ${media} ${point.mode} ${point.boundary}`,()=>{
  const mode=point.mode;
  const erp={name:media==='video'?'erp.mp4':'erp-photos',kind:media,width:128,height:64,erp:true} as const;
  test.use({scenario:{name:`erp-faces-${kind}-${media}-${mode}`,inputs:kind==='ordinary'?[erp]:[{name:'base',kind:'images'},erp],barriers:[{id:'faces',operation:point.boundary==='partial'?'erpFaces':'trainingDatasetFiles',boundary:point.boundary,task:kind==='ordinary'?1:2}]}});
  test('partial six-face output is validated on continuation',async({app})=>{
    let ref;if(kind==='reshoot'){const a=await generate(app);await completed(app,a.taskId);await app.reshoot(a.taskId);await app.chooseInputs([1]);await projection(app,erp.name);await app.generate();const dialog=app.page.locator('.preflight-dialog');await dialog.locator('input[value=good]').check();await dialog.locator('button.primary-action').click();ref=await app.reference(a.taskId);}
    else{await app.chooseInputs();await projection(app,erp.name);await app.quality('fast');await app.generate();ref=await app.reference();}
    const hit=await app.waitHit('faces');if(point.boundary==='partial'){expect(hit.evidence.written).toBe(1);expect(hit.evidence.total).toBe(180);}await interrupt(app,hit,mode);const snapshot=await completed(app,ref.taskId);expect(snapshot.calls.filter(c=>c.scope?.taskId===ref.taskId&&c.operation==='train')).toHaveLength(1);
  });
});

for(const operation of ['sourcePrepared','extract','bridgeDecision','bridgeRescue','bridgeModel','bridgePublication'])test.describe(`collection interruption ${operation}`,()=>{
  test.use({scenario:{name:`collection-stop-${operation}`,inputs:[{name:'first.mp4',kind:'video',duration:5},{name:'second.mp4',kind:'video',duration:5,fps:60}],
    faults:operation.startsWith('bridge')?[{operation:'colmap:mapper',occurrence:1,outcome:'success',registeredBySource:{'first.mp4':35,'second.mp4':10}}]:[],
    barriers:[{id:'stop',operation:operation==='bridgeRescue'?'extract':operation==='bridgeModel'?'colmap:mapper':operation,boundary:operation==='extract'||operation==='bridgeRescue'?'partial':operation==='bridgeModel'?'written':'checkpoint',source:operation==='extract'||operation==='bridgeRescue'?'second.mp4':undefined,occurrence:operation==='bridgeRescue'||operation==='bridgeModel'?2:1}],
  }});
  test('keeps completed source work and frozen per-source Bridge budgets',async({app})=>{
    await app.chooseInputs([0,1]);await app.quality('fast');await app.page.getByRole('button',{name:'更多设置',exact:true}).click();await fps(app,'first.mp4',7);await fps(app,'second.mp4',7);await app.generate();const ref=await app.reference();const hit=await app.waitHit('stop');const before=await app.snapshot();
    await interrupt(app,hit,'crash');const after=await completed(app,ref.taskId),project=app.project(after);
    expect(project.state.inputs.sources.map((s:any)=>s.frames.initialExtractedFrames)).toEqual([35,35]);expect(project.state.inputs.sources.map((s:any)=>s.frames.rescueMaxFrames)).toEqual([42,42]);
    for(const item of app.project(before).state.inputs.sources){const resumed=project.state.inputs.sources.find((s:any)=>s.id===item.id);if(item.frames)expect(resumed.frames.rescueMaxFrames).toBe(item.frames.rescueMaxFrames);}
    expect(source(project,'first.mp4').bridgeFrames).toHaveLength(0);expect(source(project,'second.mp4').bridgeFrames.length).toBeLessThanOrEqual(7);
    for(const item of project.state.inputs.sources)expect(item.names.length).toBeLessThanOrEqual(item.frames.rescueMaxFrames);
    if(operation==='bridgePublication')for(const op of ['extract','feature_extractor','matches_importer','mapper'])expect(after.calls.filter(c=>c.operation===op)).toEqual(before.calls.filter(c=>c.operation===op));
  });
});

test.describe('material admission edits',()=>{
  test.use({scenario:{name:'admission-edits',inputs:[{name:'a',kind:'images'},{name:'b',kind:'images'},{name:'c',kind:'images'}]}});
  test('duplicate paths and overlapping members block before task creation',async({app})=>{
    await app.chooseInputs([0]);const inputs=(await app.snapshot()).inputs;await app.page.evaluate(p=>(window as any).__e2eDialogResults.push(p),[inputs[0]]);await app.page.getByRole('button',{name:'添加文件夹',exact:true}).click();
    await expect(app.page.locator('.material-error').filter({hasText:'素材存在重复输入，请移除重复项。'})).toBeVisible();expect((await app.snapshot()).tasks).toHaveLength(0);
    const rows=app.page.locator('.material-item');for(const id of await rows.evaluateAll(nodes=>nodes.map(n=>n.getAttribute('data-material-id')!)))await app.page.locator(`[data-material-id="${id}"] .material-remove`).click();
    await app.chooseInputs([0]);const files=(await readdir(inputs[0])).slice(0,2).map(n=>path.join(inputs[0],n));await app.page.evaluate(p=>(window as any).__e2eDialogResults.push(p),files);await app.page.getByRole('button',{name:'添加文件',exact:true}).click();
    await expect(app.page.locator('.material-error').filter({hasText:'素材存在重复输入，请移除重复项。'})).toBeVisible();expect((await app.snapshot()).tasks).toHaveLength(0);
  });
  test('unsupported input is rejected without a task or engine invocation',async({app})=>{
    const file=path.join(app.root,'fixtures','unsupported.xyz');await writeFile(file,'unsupported local fixture');await app.page.evaluate(p=>(window as any).__e2eDialogResults.push([p]),file);await app.page.getByRole('button',{name:'添加文件',exact:true}).click();await expect(app.page.getByRole('alertdialog')).toBeVisible();expect((await app.snapshot()).tasks).toHaveLength(0);expect((await app.snapshot()).calls.filter(c=>c.operation==='train'||c.operation==='mapper')).toHaveLength(0);
  });
  test('remove and re-add uses the new order without borrowing analysis',async({app})=>{
    await app.chooseInputs([0,1]);const row=await material(app,'a');await row.locator('.material-remove').click();await app.chooseInputs([2,0]);await app.quality('fast');await app.generate();const ref=await app.reference();const snapshot=await completed(app,ref.taskId);expect(app.project(snapshot).state.inputs.request.items.map((i:any)=>path.basename(i.source.path))).toEqual(['b','c','a']);
  });
});
