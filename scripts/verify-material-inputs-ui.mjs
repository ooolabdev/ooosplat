// Real browser layout checks only; native dialogs, drops and Tauri IPC remain separate acceptance.
import {build} from 'vite';
import {createServer} from 'node:http';
import {mkdir,readFile,writeFile} from 'node:fs/promises';
import {spawn} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import assert from 'node:assert/strict';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const out=path.join(root,'.cache/multi-input-ui',String(Date.now()));await mkdir(out,{recursive:true});
await build({root,logLevel:'warn',define:{'process.env.NODE_ENV':'"production"'},build:{outDir:out,emptyOutDir:false,lib:{entry:path.join(root,'scripts/fixtures/material-inputs.tsx'),name:'MaterialInputsFixture',formats:['iife'],fileName:()=> 'runtime.js',cssFileName:'style'}}});
const server=createServer(async(req,res)=>{try{const pathname=new URL(req.url,'http://localhost').pathname;if(pathname==='/'){res.setHeader('Content-Type','text/html; charset=utf-8');res.end('<meta charset="utf-8"><link rel="stylesheet" href="/style.css"><div id="root"></div><script src="/runtime.js"></script>');}else if(['/runtime.js','/style.css'].includes(pathname)){res.setHeader('Content-Type',pathname.endsWith('.js')?'application/javascript':'text/css');res.end(await readFile(path.join(out,pathname.slice(1))));}else{res.statusCode=404;res.end();}}catch(e){res.statusCode=500;res.end(String(e));}});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=spawn(process.env.OOOSPLAT_UI_TEST_BROWSER||'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',['--headless=new','--no-first-run','--disable-background-networking','--disable-sync','--disable-extensions','--remote-debugging-port=0',`--user-data-dir=${path.join(out,'profile')}`,'about:blank'],{windowsHide:true,stdio:['ignore','ignore','pipe']});
let diagnostics='',socket,ready,reject;const startup=new Promise((resolve,fail)=>{ready=resolve;reject=fail;});browser.on('error',reject);browser.on('exit',code=>reject(Error(`Browser exited ${code}`)));browser.stderr.on('data',data=>{diagnostics+=data.toString();const port=diagnostics.match(/DevTools listening on ws:\/\/127\.0\.0\.1:(\d+)/)?.[1];if(port)ready(Number(port));});
const startupTimer=setTimeout(()=>reject(Error('Browser startup timed out')),30000);const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));const proofs=[],runtimeExceptions=[];
try{
 const port=await startup;const pages=await(await fetch(`http://127.0.0.1:${port}/json/list`)).json();socket=new WebSocket(pages.find(p=>p.type==='page').webSocketDebuggerUrl);await new Promise(resolve=>socket.addEventListener('open',resolve,{once:true}));let sequence=0;
 socket.addEventListener('message',event=>{const message=JSON.parse(event.data);if(message.method==='Runtime.exceptionThrown'){runtimeExceptions.push(message.params.exceptionDetails);if(runtimeExceptions.length>100)runtimeExceptions.shift();}});
 const call=(method,params={})=>new Promise((resolve,reject)=>{const id=++sequence;const timeout=setTimeout(()=>reject(Error(`${method} timed out`)),15000);const receive=e=>{const m=JSON.parse(e.data);if(m.id!==id)return;clearTimeout(timeout);socket.removeEventListener('message',receive);m.error?reject(Error(JSON.stringify(m.error))):resolve(m.result);};socket.addEventListener('message',receive);socket.send(JSON.stringify({id,method,params}));});
 const evaluate=async expression=>{const result=await call('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(result.exceptionDetails)throw Error(JSON.stringify(result.exceptionDetails));return result.result.value;};
 const wait=async expression=>{for(let i=0;i<100;i++){if(await evaluate(expression))return;await pause(40);}throw Error(`${expression}: ${JSON.stringify(runtimeExceptions.slice(-3))}`);};
 const accordionProof=()=>evaluate(`(()=>{const box=document.querySelector('.material-inputs'),list=box?.querySelector('.material-list'),inspector=box?.querySelector('.material-inspector'),row=box?.querySelector('.material-row.active'),entry=row?.closest('.material-list-entry'),button=row?.querySelector('.material-row-main'),controlled=button?.getAttribute('aria-controls'),panel=controlled?document.getElementById(controlled):null;return {activeMaterialId:entry?.getAttribute('data-material-id'),activeCount:box?.querySelectorAll('.material-row.active').length??0,inspectorCount:box?.querySelectorAll('.material-inspector').length??0,inspectorInsideActiveEntry:!!inspector&&!!entry&&entry.contains(inspector)&&!!list&&list.contains(entry),inspectorAfterActiveRow:!!inspector&&!!row?.nextElementSibling&&(row.nextElementSibling===inspector||row.nextElementSibling.contains(inspector)),expanded:button?.getAttribute('aria-expanded'),controlsMatch:!!panel&&!!inspector&&(panel===inspector||panel.contains(inspector)),inactiveCollapsed:[...(box?.querySelectorAll('.material-row:not(.active) .material-row-main')??[])].every(button=>button.getAttribute('aria-expanded')==='false')};})()`);
 const assertAccordion=(proof,context)=>{assert.equal(proof.inspectorCount,1,context);assert.equal(proof.activeCount,1,context);assert.equal(proof.inspectorInsideActiveEntry,true,context);assert.equal(proof.inspectorAfterActiveRow,true,context);assert.equal(proof.expanded,'true',context);assert.equal(proof.controlsMatch,true,context);assert.equal(proof.inactiveCollapsed,true,context);};
 const metadataProof=()=>evaluate(`(()=>{const box=document.querySelector('.material-inputs'),inspector=box?.querySelector('.material-inspector'),copy=inspector?.querySelector('.source-status-hint-copy')?.textContent??'';return {rows:[...(box?.querySelectorAll('.material-row-meta')??[])].map(meta=>({text:meta.textContent.trim(),children:[...meta.children].map(child=>child.tagName)})),oldRowDimensions:box?.querySelectorAll('.material-row .material-dimensions').length??0,details:[...(inspector?.querySelectorAll('.media-source-metadata')??[])].map(meta=>meta.textContent),routineDescription:['将按普通图片处理','将按普通视频处理','These images will use ordinary-image processing.','This video will use ordinary-video processing.'].some(text=>copy.includes(text))};})()`);
 const assertRowTypesOnly=proof=>{assert.equal(proof.oldRowDimensions,0);for(const row of proof.rows){assert.deepEqual(row.children,['STRONG']);assert.doesNotMatch(row.text,/×|\bFPS\b|\d+\s*(?:张|images)/);}};
 const framelessControlsProof=()=>evaluate(`([...document.querySelectorAll('.material-add-actions button,.frame-sampling-head select')].map((element,index)=>{const style=getComputedStyle(element),rect=element.getBoundingClientRect();return {index,tag:element.tagName,text:element.textContent,disabled:element.disabled,background:style.backgroundColor,borders:['Top','Right','Bottom','Left'].map(side=>({color:style['border'+side+'Color'],width:style['border'+side+'Width'],style:style['border'+side+'Style']})),outline:{style:style.outlineStyle,width:style.outlineWidth,color:style.outlineColor},focusVisible:element.matches(':focus-visible'),focused:document.activeElement===element,rect:{left:rect.left,top:rect.top,width:rect.width,height:rect.height}};}))`);
 const materialReadonlyProof=()=>evaluate(`(()=>{
  const box=document.querySelector('.material-inputs'),reference=document.createElement('button');
  reference.className='path-picker readonly';reference.style.cssText='position:fixed;visibility:hidden;pointer-events:none';box.parentElement.append(reference);
  const referenceStyle=getComputedStyle(reference),expected={color:referenceStyle.color,background:referenceStyle.backgroundColor,border:referenceStyle.borderTopColor,borderWidth:referenceStyle.borderTopWidth,borderStyle:referenceStyle.borderTopStyle};reference.remove();
  const textSelectors={summaryHeading:'.material-input-summary strong',summaryStatus:'.material-input-summary small',rowName:'.material-row-copy > strong',rowPath:'.material-row-copy > small',rowType:'.material-row-meta > strong,.material-row-type',rowIcon:'.material-row-main > svg',inspectorText:'.material-inspector strong,.material-inspector p,.material-inspector small,.material-inspector summary',inspectorIcon:'.material-inspector svg',tag:'.material-inspector .source-attribute-tag'};
  const text=Object.entries(textSelectors).flatMap(([role,selector])=>[...box.querySelectorAll(selector)].map(element=>({role,text:element.textContent,color:getComputedStyle(element).color})));
  const surfaces=[['frame',box],['summary',box.querySelector('.material-input-summary')],...Array.from(box.querySelectorAll('.material-row'),element=>['row',element]),['inspector',box.querySelector('.material-inspector')]].filter(([,element])=>element).map(([role,element])=>({role,background:getComputedStyle(element).backgroundColor}));
  const style=getComputedStyle(box),borders=['Top','Right','Bottom','Left'].map(side=>({side,color:style['border'+side+'Color'],width:style['border'+side+'Width'],style:style['border'+side+'Style']}));
  return {readonly:box.classList.contains('read-only'),expected,text,surfaces,borders};
 })()`);
 const assertMaterialReadonly=(proof,context)=>{
  assert.equal(proof.readonly,true,context);assert.equal(proof.expected.color,'rgb(127, 137, 153)',context);assert.equal(proof.expected.background,'rgb(242, 244, 247)',context);assert.equal(proof.expected.border,'rgb(212, 218, 227)',context);
  for(const role of ['summaryHeading','summaryStatus','rowName','rowPath','rowType','rowIcon','inspectorText'])assert.ok(proof.text.some(element=>element.role===role),`${context}: missing ${role} coverage`);
  for(const element of proof.text)assert.equal(element.color,proof.expected.color,`${context}: ${element.role} ${element.text}`);
  for(const surface of proof.surfaces)assert.equal(surface.background,proof.expected.background,`${context}: ${surface.role} background`);
  for(const border of proof.borders){assert.equal(border.color,proof.expected.border,`${context}: ${border.side} frame border`);assert.equal(border.width,proof.expected.borderWidth,`${context}: ${border.side} frame width matches the same-zoom path picker`);assert.equal(border.style,proof.expected.borderStyle,context);}
 };
 const transparent=color=>color==='transparent'||color==='rgba(0, 0, 0, 0)';
 const assertFrameless=(controls,context)=>{for(const control of controls){assert.equal(transparent(control.background),true,`${context}: ${control.tag} background`);for(const border of control.borders)assert.ok(border.style==='none'||parseFloat(border.width)===0||transparent(border.color),`${context}: ${control.tag} must have no visible permanent border`);}};
 const controlInteractionProof=async controls=>{
  const results=[];
  for(const control of controls.filter(control=>!control.disabled&&control.rect.width>0&&control.rect.height>0)){
   await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:control.rect.left+control.rect.width/2,y:control.rect.top+control.rect.height/2});await pause(20);
   const hovered=(await framelessControlsProof())[control.index];assertFrameless([hovered],'hover');
   // A real Tab event establishes keyboard modality before programmatic focus;
   // no CSS pseudo-state is forced, and no control value is changed.
   await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Tab',code:'Tab',windowsVirtualKeyCode:9,nativeVirtualKeyCode:9});
   await call('Input.dispatchKeyEvent',{type:'keyUp',key:'Tab',code:'Tab',windowsVirtualKeyCode:9,nativeVirtualKeyCode:9});
   await evaluate(`document.querySelectorAll('.material-add-actions button,.frame-sampling-head select')[${control.index}].focus()`);
   const focused=(await framelessControlsProof())[control.index];assert.equal(focused.focused,true);assert.equal(focused.focusVisible,true);assert.ok(parseFloat(focused.outline.width)>0);assert.notEqual(focused.outline.style,'none');assert.equal(transparent(focused.outline.color),false);
   results.push({index:control.index,tag:control.tag,hovered,focused});
  }
  await evaluate('document.activeElement?.blur()');await call('Input.dispatchMouseEvent',{type:'mouseMoved',x:0,y:0});return results;
 };
 const samplingHeaderProof=()=>evaluate(`(()=>{
  const header=document.querySelector('.frame-sampling-head'),title=header?.querySelector('strong'),output=header?.querySelector('output'),select=header?.querySelector('select'),picker=header?.querySelector('.frame-sampling-material-picker'),measure=picker?.querySelector('.frame-sampling-material-measure'),arrow=picker?.querySelector('.frame-sampling-material-arrow');
  const rect=element=>{const value=element?.getBoundingClientRect();return value?{left:value.left,right:value.right,top:value.top,bottom:value.bottom,width:value.width,height:value.height}:null;};
  const style=measure?getComputedStyle(measure):null,selectStyle=select?getComputedStyle(select):null,label=select?.selectedOptions[0]?.textContent??'',canvas=document.createElement('canvas'),context=canvas.getContext('2d');
  if(context&&selectStyle)context.font=selectStyle.fontStyle+' '+selectStyle.fontWeight+' '+selectStyle.fontSize+' '+selectStyle.fontFamily;
  const reservedSpace=style?['paddingLeft','paddingRight','borderLeftWidth','borderRightWidth'].reduce((sum,key)=>sum+parseFloat(style[key]||'0'),0):null;
  const widthFont=font=>font?Object.fromEntries(['fontFamily','fontSize','fontWeight','fontStyle','fontStretch','fontVariant','letterSpacing'].map(key=>[key,font[key]])):null;
  const selectRect=rect(select),arrowRect=rect(arrow),textWidth=context?context.measureText(label).width:null,zoom=selectRect&&select?.offsetWidth?selectRect.width/select.offsetWidth:null,availableTextWidth=select&&selectStyle?select.clientWidth-parseFloat(selectStyle.paddingLeft)-parseFloat(selectStyle.paddingRight):null;
  const textArrowGap=arrowRect&&selectRect&&zoom&&selectStyle?((arrowRect.left-selectRect.left)/zoom)-parseFloat(selectStyle.borderLeftWidth)-parseFloat(selectStyle.paddingLeft)-textWidth:null;
  return {header:rect(header),title:rect(title),output:rect(output),select:selectRect,picker:rect(picker),arrow:arrowRect,arrowHidden:arrow?.getAttribute('aria-hidden'),arrowPointerEvents:arrow?getComputedStyle(arrow).pointerEvents:null,appearance:selectStyle?.appearance,availableTextWidth,textArrowGap,label:output?.textContent,whiteSpace:output?getComputedStyle(output).whiteSpace:null,selectedId:select?.value,selectedName:label,fullTitle:select?.title,mirrorText:measure?.textContent,mirrorHidden:measure?.getAttribute('aria-hidden'),mirrorVisibility:style?.visibility,mirrorFont:widthFont(style),selectFont:widthFont(selectStyle),pickerOffsetWidth:picker?.offsetWidth,textWidth,reservedSpace,rightTrackWidth:header?parseFloat(getComputedStyle(header).gridTemplateColumns.split(' ').at(-1)):null,overflow:document.documentElement.scrollWidth>innerWidth+1};
 })()`);
 const assertSamplingHeader=(proof,context)=>{
  assert.ok(proof.header&&proof.title&&proof.output&&proof.select&&proof.picker&&proof.arrow,context);
  assert.equal(proof.overflow,false,context);
  assert.ok(proof.title.right<=proof.output.left+1,`${context}: title and FPS must not overlap`);
  assert.ok(proof.output.right<=proof.select.left+1,`${context}: FPS and filename must not overlap`);
  assert.ok(proof.select.right<=proof.header.right+1,`${context}: selector must stay inside the header`);
  assert.ok(proof.select.left>=proof.header.left-1,context);
  assert.ok(proof.output.left>=proof.header.left-1,context);
  assert.ok(Math.abs((proof.output.left+proof.output.right-proof.header.left-proof.header.right)/2)<1,`${context}: FPS stays centered`);
  assert.equal(proof.whiteSpace,'nowrap',context);
  assert.equal(proof.mirrorText,proof.selectedName,context);
  assert.equal(proof.fullTitle,proof.selectedName,context);
  assert.equal(proof.mirrorHidden,'true',context);
  assert.equal(proof.mirrorVisibility,'hidden',context);
  assert.deepEqual(proof.mirrorFont,proof.selectFont,context);
  assert.equal(proof.appearance,'none',context);
  assert.equal(proof.arrowHidden,'true',context);
  assert.equal(proof.arrowPointerEvents,'none',context);
  assert.ok(proof.arrow.left>=proof.select.left&&proof.arrow.right<=proof.select.right+1,context);
  assert.ok(proof.textWidth>0,context);
  assert.ok(proof.pickerOffsetWidth>proof.reservedSpace,context);
  assert.ok(proof.pickerOffsetWidth-proof.textWidth<=proof.reservedSpace+2,`${context}: selected text must not leave a large blank gap before the arrow`);
  if(proof.textWidth+proof.reservedSpace<Math.min(proof.rightTrackWidth,210)-2){
   assert.ok(Math.abs(proof.pickerOffsetWidth-proof.textWidth-proof.reservedSpace)<2,`${context}: short selected names size the picker, not longer unselected options`);
   assert.ok(proof.availableTextWidth+1>=proof.textWidth,`${context}: an unclipped short name fits without native extra padding`);
   assert.ok(proof.textArrowGap>=3&&proof.textArrowGap<=11,`${context}: arrow stays about 4–10 CSS pixels from complete selected text, got ${proof.textArrowGap}`);
  }
 };
 const selectSamplingSource=async index=>{
  const selected=await evaluate(`(()=>{const select=document.querySelector('.frame-sampling-head select'),option=select.options[${index}];select.value=option.value;select.dispatchEvent(new Event('change',{bubbles:true}));return {id:option.value,label:option.textContent};})()`);
  await wait(`document.querySelector('.frame-sampling-material-measure')?.textContent===${JSON.stringify(selected.label)}&&document.querySelector('.frame-sampling-head select')?.value===${JSON.stringify(selected.id)}`);
  return samplingHeaderProof();
 };
 await call('Runtime.enable');
 for(const locale of ['zh-CN','en'])for(const scale of [80,100,140]){
  await call('Emulation.setDeviceMetricsOverride',{width:1100,height:780,deviceScaleFactor:1,mobile:false});
  await call('Page.navigate',{url:`http://127.0.0.1:${server.address().port}/?locale=${locale}&scale=${scale}`});await wait('typeof window.setMaterialsFixture==="function" && !!document.querySelector(".material-inputs")');
  let mixedHeight=0;
  for(const state of ['empty','loading','mixed','error','colmap','readonly','alpha','alpha-readonly','drag','gps','gps-alpha','gps-readonly','gps-alpha-readonly','gps-fallback','gps-alpha-fallback','gps-fallback-readonly']){
   await evaluate(`window.setMaterialsFixture(${JSON.stringify(state)})`);await pause(80);
   const proof=await evaluate(`(()=>{const surfaceColor=element=>{for(let current=element;current;current=current.parentElement){const color=getComputedStyle(current).backgroundColor;if(color!=='transparent'&&color!=='rgba(0, 0, 0, 0)')return color;}return null;};const box=document.querySelector('.material-inputs'),r=box.getBoundingClientRect(),list=box.querySelector('.material-list'),inspector=box.querySelector('.material-inspector'),content=box.querySelector('.material-input-content'),alpha=inspector?.querySelector('.alpha-source-status'),status=inspector?.querySelector('.media-source-status'),heading=status?.querySelector('.media-source-heading'),copy=heading?.querySelector('.media-source-copy'),tags=heading?.querySelector('.source-attribute-tags'),copyRect=copy?.getBoundingClientRect(),tagsRect=tags?.getBoundingClientRect();return {left:r.left,right:r.right,width:r.width,height:r.height,overflow:document.documentElement.scrollWidth>innerWidth+1,items:box.querySelectorAll('.material-item').length,addIcons:box.querySelectorAll('.material-add-actions svg').length,inspectors:box.querySelectorAll('.material-inspector').length,inspectorAfterList:!!list&&list.nextElementSibling===inspector,inspectorBackground:inspector?surfaceColor(inspector):null,dropPrompt:!!box.querySelector('.material-drop-prompt'),contentHidden:getComputedStyle(content).visibility==='hidden',text:box.textContent,colors:[...box.querySelectorAll('.media-source-status')].map(s=>getComputedStyle(s).color),alphaText:alpha?.textContent,alphaColor:alpha?getComputedStyle(alpha).color:null,alphaDescription:!!alpha?.querySelector('small,p'),statusCount:inspector?.querySelectorAll('.media-source-status').length??0,hints:[...(status?.querySelectorAll('.source-status-hint')??[])].map(row=>{const s=getComputedStyle(row),icon=row.querySelector('svg'),title=row.querySelector('strong');return {color:s.color,gap:s.gap,alignment:s.alignItems,borders:[s.borderTopWidth,s.borderRightWidth,s.borderBottomWidth,s.borderLeftWidth],titleSize:title?getComputedStyle(title).fontSize:null,titleColor:title?getComputedStyle(title).color:null,iconColor:icon?getComputedStyle(icon).color:null,iconHidden:icon?.getAttribute('aria-hidden'),iconSize:icon?.getAttribute('width'),iconClass:icon?.getAttribute('class')};}),tagsInsideHeading:!!tags&&tags.parentElement===heading,tagsRightOfCopy:!!tagsRect&&!!copyRect&&tagsRect.left>=copyRect.right-1,tags:[...(tags?.querySelectorAll('.source-attribute-tag')??[])].map(tag=>{const s=getComputedStyle(tag),rect=tag.getBoundingClientRect(),icon=tag.querySelector('svg');return {gps:tag.classList.contains('gps-source-hint'),alpha:tag.classList.contains('alpha-source-status'),text:tag.textContent,title:tag.getAttribute('title'),label:tag.getAttribute('aria-label'),tabIndex:tag.getAttribute('tabindex'),color:s.color,background:s.backgroundColor,surfaceBackground:surfaceColor(tag),whiteSpace:s.whiteSpace,left:rect.left,right:rect.right,centerY:rect.top+rect.height/2,iconColor:icon?getComputedStyle(icon).color:null,iconHidden:icon?.getAttribute('aria-hidden'),iconSize:icon?.getAttribute('width'),iconClass:icon?.getAttribute('class')};}),fps:document.querySelector('output')?.textContent};})()`);
   assert.equal(proof.overflow,false,JSON.stringify({locale,scale,state,...proof}));assert.ok(proof.right<=1100);
   proof.accordion=await accordionProof();delete proof.inspectorAfterList;
   if(proof.inspectors)assertAccordion(proof.accordion,`${locale}/${scale}/${state}`);
   proof.metadata=await metadataProof();assertRowTypesOnly(proof.metadata);
   proof.framelessControls=await framelessControlsProof();assertFrameless(proof.framelessControls,`${locale}/${scale}/${state}`);
   if(state==='empty'||state==='mixed')proof.controlInteractions=await controlInteractionProof(proof.framelessControls);
   proof.typeDescriptionColors=await evaluate(`[...document.querySelectorAll('.material-inspector .source-status-hint-copy > p')].map(description=>getComputedStyle(description).color)`);
   proof.typeTitleWeights=await evaluate(`[...document.querySelectorAll('.material-inspector .source-status-hint-copy > strong')].map(title=>getComputedStyle(title).fontWeight)`);
   proof.tagBorderColors=await evaluate(`[...document.querySelectorAll('.material-inspector .source-attribute-tag')].map(tag=>getComputedStyle(tag).borderTopColor)`);
   proof.addActionColors=await evaluate(`[...document.querySelectorAll('.material-add-actions button:not(:disabled)')].map(button=>getComputedStyle(button).color)`);
   const readonly=state==='readonly'||state.endsWith('-readonly'),titleColor=readonly?'rgb(127, 137, 153)':'rgb(83, 96, 120)',accentColor=readonly?'rgb(127, 137, 153)':'rgb(113, 131, 160)';
   if(readonly){proof.readonlyStyle=await materialReadonlyProof();assertMaterialReadonly(proof.readonlyStyle,`${locale}/${scale}/${state}`);}
   if(state==='mixed'){
    proof.editableStyle=await materialReadonlyProof();assert.equal(proof.editableStyle.readonly,false);
    const colors={summaryHeading:'rgb(23, 32, 51)',summaryStatus:'rgb(102, 112, 132)',rowName:'rgb(23, 32, 51)',rowPath:'rgb(102, 112, 132)',rowType:'rgb(83, 96, 120)',rowIcon:'rgb(30, 92, 255)'};
    for(const element of proof.editableStyle.text)if(element.role in colors)assert.equal(element.color,colors[element.role],`${locale}/${scale}: editable ${element.role} retains its original color`);
   }
   for(const type of proof.hints){assert.equal(type.color,titleColor);assert.equal(type.titleColor,titleColor);if(type.iconColor!==null)assert.equal(type.iconColor,accentColor);}
   for(const weight of proof.typeTitleWeights)assert.equal(weight,'600');
   for(const color of proof.tagBorderColors)assert.equal(color,'rgb(223, 228, 237)');
   for(const color of proof.addActionColors)assert.equal(color,'rgb(30, 92, 255)');
   for(const color of proof.typeDescriptionColors)assert.equal(color,readonly?'rgb(127, 137, 153)':'rgb(102, 112, 132)');
   for(const tag of proof.tags){assert.equal(proof.inspectorBackground,'rgb(242, 244, 247)');assert.equal(tag.surfaceBackground,proof.inspectorBackground,`${locale}/${scale}/${state}: metadata tags must share the inspector gray`);assert.equal(tag.color,accentColor);assert.equal(tag.iconColor,accentColor);}
   if(state==='readonly'){
    assert.ok(proof.colors.every(c=>c==='rgb(127, 137, 153)'));proof.readonlyInspectorSwitches=[];
    for(const sourceId of ['video','erp','loose']){
     await evaluate(`document.querySelector('.material-list-entry[data-material-id="${sourceId}"] .material-row-main').click()`);
     await wait(`document.querySelector('.material-list-entry[data-material-id="${sourceId}"] .material-row.active')!==null`);
     const switched=await materialReadonlyProof();assertMaterialReadonly(switched,`${locale}/${scale}/readonly/${sourceId}`);proof.readonlyInspectorSwitches.push({sourceId,...switched});
    }
   }
   if(state.startsWith('alpha')){assert.equal(proof.alphaText,locale==='zh-CN'?'透明通道':'Alpha channel');assert.equal(proof.alphaDescription,false);assert.equal(proof.tagsInsideHeading,true);assert.equal(proof.tagsRightOfCopy,true);assert.equal(proof.tags.length,1);assert.equal(proof.metadata.details.length,1);assert.match(proof.metadata.details[0],/1920\s*×\s*1080/);assert.match(proof.metadata.details[0],/29\.97\s*FPS/);assert.equal(proof.metadata.routineDescription,false);if(state==='alpha-readonly')assert.equal(proof.alphaColor,'rgb(127, 137, 153)');}
   if(state==='empty'||state==='mixed'||state==='loading'||state==='error')assert.equal(proof.addIcons,2);
   if(state==='mixed'){
    mixedHeight=proof.height;assert.equal(proof.items,3);assert.equal(proof.inspectors,1);assert.equal(proof.fps,'8 FPS');assert.match(proof.text,/IMG_8023.MOV/);
    proof.accordionSwitches=[];
    for(const sourceId of ['video','erp','loose',proof.accordion.activeMaterialId]){
     await evaluate(`document.querySelector('.material-list-entry[data-material-id="${sourceId}"] .material-row-main').click()`);
     await wait(`document.querySelector('.material-list-entry[data-material-id="${sourceId}"] .material-row.active')!==null`);
     const switched=await accordionProof();assert.equal(switched.activeMaterialId,sourceId);assertAccordion(switched,`${locale}/${scale}/click/${sourceId}`);switched.metadata=await metadataProof();assertRowTypesOnly(switched.metadata);assert.equal(switched.metadata.details.length,1);assert.equal(switched.metadata.routineDescription,false);assert.match(switched.metadata.details[0],sourceId==='video'?/1920\s*×\s*1080/:/4096\s*×\s*2048/);if(sourceId==='video')assert.match(switched.metadata.details[0],/29\.97\s*FPS/);proof.accordionSwitches.push(switched);
    }
    proof.samplingHeader=await samplingHeaderProof();assertSamplingHeader(proof.samplingHeader,`${locale}/${scale}/wide-long-name`);
    proof.samplingShortHeader=await selectSamplingSource(1);assertSamplingHeader(proof.samplingShortHeader,`${locale}/${scale}/wide-short-name`);assert.match(proof.samplingShortHeader.selectedName,/Camera B/);
    await selectSamplingSource(0);
   }
   if(state==='drag'){assert.equal(proof.dropPrompt,true);assert.equal(proof.contentHidden,true);assert.equal(proof.items,3);assert.equal(proof.inspectors,1);assert.ok(Math.abs(proof.height-mixedHeight)<1);}
   if(state.startsWith('gps')){
    assert.equal(proof.statusCount,1);assert.equal(proof.inspectors,1);assert.equal(proof.hints.length,1);assert.equal(proof.tagsInsideHeading,true);assert.equal(proof.tagsRightOfCopy,true);
    assert.equal(proof.metadata.details.length,1);assert.match(proof.metadata.details[0],/4096\s*×\s*2048/);assert.match(proof.metadata.details[0],locale==='zh-CN'?/94\s*张图片/:/94\s*images/);assert.equal(proof.metadata.routineDescription,false);
    const color=accentColor;
    const type=proof.hints[0];assert.match(type.iconClass,/lucide-images/);assert.deepEqual(type.borders,['0px','0px','0px','0px']);assert.equal(type.gap,'8px');assert.equal(type.alignment,'center');assert.equal(type.titleSize,'12px');assert.equal(type.titleColor,titleColor);assert.equal(type.iconColor,accentColor);assert.equal(type.iconHidden,'true');assert.equal(type.iconSize,'17');
    assert.equal(proof.tags.length,state.includes('-alpha')?2:1);const gpsTag=proof.tags.find(tag=>tag.gps);assert.equal(gpsTag.text,'GPS');assert.match(gpsTag.iconClass,/lucide-map-pin/);assert.equal(gpsTag.label,gpsTag.title);assert.match(gpsTag.title,state.includes('-fallback')?(locale==='zh-CN'?/GPS 辅助未启用/:/GPS assistance is unavailable/):(locale==='zh-CN'?/有效位置信息/:/Valid location information/));
    for(const tag of proof.tags){assert.equal(tag.whiteSpace,'nowrap');assert.equal(tag.color,color);assert.equal(tag.iconColor,color);assert.equal(tag.iconHidden,'true');assert.equal(tag.iconSize,'13');assert.equal(tag.tabIndex,'0');assert.ok(tag.title);assert.ok(tag.right<=proof.right);assert.ok(Math.abs(tag.centerY-gpsTag.centerY)<1);}
    if(state.includes('-alpha')){const alphaTag=proof.tags.find(tag=>tag.alpha);assert.equal(alphaTag.text,locale==='zh-CN'?'透明通道':'Alpha channel');assert.match(alphaTag.iconClass,/lucide-blend/);assert.equal(alphaTag.title,locale==='zh-CN'?'检测到 Alpha 通道':'Alpha channel detected');assert.equal(proof.alphaColor,color);assert.ok(gpsTag.right<=alphaTag.left,'GPS and Alpha tags must coexist without overlapping');}else assert.equal(proof.alphaColor,null);
   }
   const image=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(out,`${locale}-${scale}-${state}.png`),Buffer.from(image.data,'base64'));
   if(locale==='zh-CN'&&scale===100&&state==='gps-alpha'){
    const clip=await evaluate(`(()=>{const rect=document.querySelector('.material-inputs').getBoundingClientRect();return {x:rect.left,y:rect.top,width:rect.width,height:rect.height,scale:1};})()`);
    const preview=await call('Page.captureScreenshot',{format:'png',clip});await writeFile(path.join(out,'detail-preview.png'),Buffer.from(preview.data,'base64'));
   }
   proofs.push({locale,scale,state,...proof});
  }
 }
 for(const locale of ['zh-CN','en'])for(const scale of [80,100,140])for(const panelWidth of [360,420]){
  await call('Emulation.setDeviceMetricsOverride',{width:1100,height:780,deviceScaleFactor:1,mobile:false});
  await call('Page.navigate',{url:`http://127.0.0.1:${server.address().port}/?locale=${locale}&scale=${scale}&panelWidth=${panelWidth}&quality=high`});
  await wait('typeof window.setMaterialsFixture==="function" && !!document.querySelector(".frame-sampling-head select")');
  await evaluate('window.setMaterialsFixture("alpha")');await pause(80);
  const proof=await samplingHeaderProof();assertSamplingHeader(proof,`${locale}/${scale}/${panelWidth}`);assert.equal(proof.label,'12 FPS');assert.match(proof.selectedName,/VID_20200408_184530_00_001_very-long-original-video-filename/);
  const controls=await framelessControlsProof();assertFrameless(controls,`${locale}/${scale}/${panelWidth}`);
  const image=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(out,`${locale}-${scale}-sampling-${panelWidth}.png`),Buffer.from(image.data,'base64'));proofs.push({locale,scale,panelWidth,state:'narrow-sampling-header',...proof});
  const shortProof=await selectSamplingSource(1);assertSamplingHeader(shortProof,`${locale}/${scale}/${panelWidth}/short`);assert.match(shortProof.selectedName,/Camera B/);assert.equal(shortProof.label,'12 FPS');
  const shortImage=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(out,`${locale}-${scale}-sampling-${panelWidth}-short.png`),Buffer.from(shortImage.data,'base64'));proofs.push({locale,scale,panelWidth,state:'narrow-short-sampling-header',...shortProof});
 }
 for(const locale of ['zh-CN','en'])for(const scale of [80,100,140])for(const width of [640,1100])for(const reason of ['colmap_disabled','unsupported_input']){
  await call('Emulation.setDeviceMetricsOverride',{width,height:780,deviceScaleFactor:1,mobile:false});
  await call('Page.navigate',{url:`http://127.0.0.1:${server.address().port}/?locale=${locale}&scale=${scale}&notice=${reason}`});
  await wait('typeof window.setInputNoticeFixture==="function" && !!document.querySelector(".unsupported-input-dialog") && document.activeElement===document.querySelector(".unsupported-input-dialog footer button")');
  const proof=await evaluate(`(()=>{const modal=document.querySelector('.unsupported-input-dialog'),backdrop=modal.parentElement,rect=element=>{const r=element.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,height:r.height};};return {modal:rect(modal),backdrop:rect(backdrop),title:document.getElementById(modal.getAttribute('aria-labelledby'))?.textContent,description:document.getElementById(modal.getAttribute('aria-describedby'))?.textContent,ariaModal:modal.getAttribute('aria-modal'),role:modal.getAttribute('role'),focusedClose:document.activeElement===modal.querySelector('footer button'),buttons:[...modal.querySelectorAll('button')].map(button=>({label:button.textContent||button.getAttribute('aria-label'),...rect(button)})),modalOverflow:modal.scrollWidth>modal.clientWidth+1,pageOverflow:document.documentElement.scrollWidth>innerWidth+1,materialRows:document.querySelectorAll('.material-row').length};})()`);
  const context=`${locale}/${scale}/${width}/${reason}`;
  // Keep evidence even when an assertion fails. Compact widths deliberately
  // override only the isolated fixture's desktop min-width, not product CSS.
  await writeFile(path.join(out,`${locale}-${scale}-${width}-${reason}-proof.json`),JSON.stringify(proof,null,2));
  const shot=await call('Page.captureScreenshot',{format:'png'});await writeFile(path.join(out,`${locale}-${scale}-${width}-${reason}.png`),Buffer.from(shot.data,'base64'));
  assert.equal(proof.ariaModal,'true',context);assert.equal(proof.role,'alertdialog',context);assert.equal(proof.focusedClose,true,context);
  assert.equal(proof.title,locale==='zh-CN'?'不支持的输入':'Unsupported input',context);
  assert.equal(proof.description,reason==='colmap_disabled'
   ? (locale==='zh-CN'?'暂不支持导入 COLMAP 文件或文件夹。':'Importing COLMAP files or folders is temporarily unavailable.')
   : (locale==='zh-CN'?'不支持所选文件或文件夹，请选择当前入口支持的文件类型。':'The selected file or folder is unsupported. Choose a file type supported by this input.'),context);
  assert.equal(proof.modalOverflow,false,`${context}: modal horizontal overflow`);assert.equal(proof.pageOverflow,false,`${context}: fixture page horizontal overflow`);
  assert.ok(proof.modal.left>=-1&&proof.modal.top>=-1&&proof.modal.right<=width+1&&proof.modal.bottom<=781,`${context}: dialog must remain inside the viewport`);
  assert.ok(proof.backdrop.left<=1&&proof.backdrop.top<=1&&proof.backdrop.right>=width-1&&proof.backdrop.bottom>=779,`${context}: backdrop must cover the viewport`);
  assert.equal(proof.buttons.length,2,context);
  for(const button of proof.buttons){assert.equal(button.label,locale==='zh-CN'?'关闭':'Close',context);assert.ok(button.left>=proof.modal.left-1&&button.right<=proof.modal.right+1&&button.top>=proof.modal.top-1&&button.bottom<=proof.modal.bottom+1,context);}
  const dispatchKey=async(key,code,virtualKey,shift=false)=>{
   await call('Input.dispatchKeyEvent',{type:'keyDown',key,code,windowsVirtualKeyCode:virtualKey,nativeVirtualKeyCode:virtualKey,modifiers:shift?8:0});
   await call('Input.dispatchKeyEvent',{type:'keyUp',key,code,windowsVirtualKeyCode:virtualKey,nativeVirtualKeyCode:virtualKey,modifiers:shift?8:0});
  };
  await dispatchKey('Tab','Tab',9);assert.equal(await evaluate('document.activeElement===document.querySelector(".unsupported-input-dialog header button")'),true,context);
  await dispatchKey('Tab','Tab',9,true);assert.equal(await evaluate('document.activeElement===document.querySelector(".unsupported-input-dialog footer button")'),true,context);
  await dispatchKey('Escape','Escape',27);await wait('!document.querySelector(".unsupported-input-dialog")');
  assert.equal(await evaluate('document.querySelectorAll(".material-row").length'),proof.materialRows,context);
  await evaluate(`document.querySelector('.material-add-actions button').focus();window.setInputNoticeFixture(${JSON.stringify(reason)})`);
  await wait('document.activeElement===document.querySelector(".unsupported-input-dialog footer button")');
  await evaluate('document.querySelector(".unsupported-input-dialog footer button").click()');await wait('!document.querySelector(".unsupported-input-dialog")');
  assert.equal(await evaluate('document.activeElement===document.querySelector(".material-add-actions button")'),true,`${context}: restore originating focus`);
  proofs.push({locale,scale,width,reason,state:'unsupported-input-notice',...proof,keyboardTrap:true,escapeCloses:true,originatingFocusRestored:true});
 }
 assert.deepEqual(runtimeExceptions,[],'The layout fixture must not throw browser runtime errors');
 await writeFile(path.join(out,'proof.json'),JSON.stringify({nativeIPC:false,proofs},null,2));console.log(`Unified inputs browser layout verified: ${out}`);
}finally{clearTimeout(startupTimer);socket?.close();browser.kill();server.closeAllConnections();server.close();await writeFile(path.join(out,'browser.log'),diagnostics);await writeFile(path.join(out,'runtime-errors.json'),JSON.stringify(runtimeExceptions,null,2));}
