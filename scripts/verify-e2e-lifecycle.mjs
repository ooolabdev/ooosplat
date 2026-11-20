// Local evidence only. No uploads, cleanup, packaging or failed-test retries.
import { spawn, spawnSync } from 'node:child_process';
import { mkdir, readFile, writeFile, appendFile, readdir, statfs, copyFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import path from 'node:path';

const mode=process.argv[2];
if(!['checks','native-rounds'].includes(mode))throw new Error('Usage: node scripts/verify-e2e-lifecycle.mjs checks|native-rounds');
const identity=new Date().toISOString().replace(/[^0-9]/g,'');
const root=path.resolve('.cache/e2e/lifecycle-validation',`${identity}-${mode}`);
await mkdir(root,{recursive:true});
const results=[];
const record=async()=>writeFile(path.join(root,'verification.json'),JSON.stringify({mode,identity,codeFingerprint,results},null,2));
const hash=buffer=>createHash('sha256').update(buffer).digest('hex');
async function fingerprint(){
  const files=[];
  async function walk(directory){for(const entry of await readdir(directory,{withFileTypes:true})){const file=path.join(directory,entry.name);if(entry.isDirectory())await walk(file);else if(/\.(rs|tsx?|m?js|json|toml|css|html|svg)$/.test(file))files.push(file);}}
  for(const directory of ['src','src-tauri/src','tests/e2e'])await walk(directory);
  files.push('playwright.config.ts','vite.config.ts','vitest.config.ts','index.html','tsconfig.json','tsconfig.app.json','tsconfig.node.json','scripts/playwright-e2e.mjs','scripts/verify-docs.mjs','scripts/verify-e2e-lifecycle.mjs','package.json','package-lock.json','src-tauri/Cargo.toml','src-tauri/Cargo.lock','src-tauri/tauri.conf.json','src-tauri/tauri.e2e.conf.json','rust-toolchain.toml');
  const digest=createHash('sha256');for(const file of files.sort()){digest.update(file.replaceAll('\\','/'));digest.update(await readFile(file));}return digest.digest('hex');
}
const codeFingerprint=await fingerprint();
const listing=spawnSync(process.execPath,['node_modules/@playwright/test/cli.js','test','--config=playwright.config.ts','--list','--reporter=json'],{encoding:'utf8',windowsHide:true,env:{...process.env,FORCE_COLOR:'0'}});
if(listing.status!==0)throw new Error(`Cannot enumerate native cases: ${listing.stderr}`);
const cases=[];
function collect(suites,parents=[]){for(const suite of suites??[]){const titles=[...parents,suite.title];for(const spec of suite.specs??[])cases.push({file:spec.file,line:spec.line,title:[...titles,spec.title].join(' > ')});collect(suite.suites,titles);}}
collect(JSON.parse(listing.stdout).suites);
await writeFile(path.join(root,'case-manifest.json'),JSON.stringify({codeFingerprint,count:cases.length,cases},null,2));
async function capacity(){const drives={};for(const drive of ['C:\\',path.resolve('.')]){const value=await statfs(drive);drives[drive]=Number(value.bavail)*Number(value.bsize);}if(drives['C:\\']<2*1024**3||drives[path.resolve('.')]<8*1024**3)throw new Error(`Insufficient free space; no cache was removed: ${JSON.stringify(drives)}`);return drives;}
async function run(name,command,args,env={}){
  const freeBytes=/build|binaries/.test(name)?await capacity():undefined;
  const log=path.join(root,`${name}.log`),started=new Date().toISOString();await writeFile(log,'');
  console.log(`START ${name}`);
  let writes=Promise.resolve();
  const code=await new Promise((resolve,reject)=>{const child=spawn(command,args,{cwd:path.resolve('.'),env:{...process.env,...env},windowsHide:true,stdio:['ignore','pipe','pipe']});
    for(const stream of [child.stdout,child.stderr])stream.on('data',chunk=>{writes=writes.then(()=>appendFile(log,chunk));});
    child.once('error',reject);child.once('exit',(value,signal)=>resolve(value??(signal?1:0)));});
  await writes;const result={name,command:[command,...args],started,finished:new Date().toISOString(),code,log,freeBytes};results.push(result);await record();console.log(`END ${name}: ${code}`);return result;
}
await writeFile(path.join(root,'initial-capacity.json'),JSON.stringify(await capacity(),null,2));
if(mode==='checks'){
  // Use the installed npm launcher through cmd on Windows, with fixed arguments.
  const npm=(name,script)=>run(name,'cmd.exe',['/d','/s','/c',`npm run ${script}`]);
  await npm('frontend-tests','test');
  await run('e2e-types',process.execPath,['node_modules/typescript/bin/tsc','-p','tests/e2e/tsconfig.json']);
  for(const features of [[],['--features','e2e']])await run(features.length?'rust-e2e':'rust-ordinary','cargo',['test','--manifest-path','src-tauri/Cargo.toml','--all-targets',...features]);
  await npm('frontend-build','build');
  await run('ordinary-binaries','cargo',['build','--manifest-path','src-tauri/Cargo.toml','--bin','ooo-splat','--bin','splatstudio']);
  await run('e2e-binaries','cargo',['build','--manifest-path','src-tauri/Cargo.toml','--features','e2e','--bin','ooo-splat-e2e','--bin','splatstudio-e2e'],{TAURI_CONFIG:await readFile('src-tauri/tauri.e2e.conf.json','utf8')});
  await run('rustfmt','cargo',['fmt','--manifest-path','src-tauri/Cargo.toml','--check']);
  await run('diff-whitespace','git',['-c','safe.directory=E:/Project/OOOSplat','diff','--check']);
  await npm('docs','verify:docs');
  for(const features of [[],['--features','e2e']])await run(features.length?'clippy-e2e':'clippy-ordinary','cargo',['clippy','--manifest-path','src-tauri/Cargo.toml','--all-targets',...features,'--','-D','warnings']);
}else{
  for(let round=1;round<=3;round++){
    if(await fingerprint()!==codeFingerprint)throw new Error('Source changed during consecutive full native rounds');
    const free=await capacity();const result=await run(`native-${round}`,process.execPath,['scripts/playwright-e2e.mjs','run']);
    const {runId}=JSON.parse(await readFile('.cache/e2e/latest.json','utf8'));const report=path.resolve('.cache/e2e/artifacts',runId,'report.json');
    result.runId=runId;result.freeBytes=free;result.codeFingerprint=codeFingerprint;
    try{
      const json=JSON.parse(await readFile(report,'utf8'));result.stats=json.stats;result.report=report;result.caseCount=cases.length;result.green=result.code===0&&json.stats.expected===cases.length&&json.stats.unexpected===0&&json.stats.skipped===0&&json.stats.flaky===0;
      await copyFile(report,path.join(root,`native-${round}-report.json`));
      const outcomes=[];
      async function evidence(suites,parents=[]){for(const suite of suites??[]){const titles=[...parents,suite.title];for(const spec of suite.specs??[])for(const test of spec.tests??[]){const attempts=[];for(const attempt of test.results??[]){const attachment=attempt.attachments?.find(a=>a.name==='actual-execution');let snapshot;try{snapshot=JSON.parse(attachment.path?await readFile(attachment.path,'utf8'):Buffer.from(attachment.body,'base64').toString('utf8'));}catch{}attempts.push({status:attempt.status,durationMs:attempt.duration,errors:attempt.errors,evidenceRoot:snapshot?.root,tasks:snapshot?.tasks?.map(t=>({taskId:t.task_id,runId:t.run_id,status:t.status,error:t.error})),attachments:attempt.attachments});}outcomes.push({title:[...titles,spec.title].join(' > '),file:spec.file,line:spec.line,status:test.status,expectedStatus:test.expectedStatus,attempts});}await evidence(suite.suites,titles);}}
      await evidence(json.suites);await writeFile(path.join(root,`native-${round}-cases.json`),JSON.stringify(outcomes,null,2));
    }catch(error){result.reportError=String(error);result.green=false;}
    result.binarySha256=hash(await readFile('src-tauri/target/debug/ooo-splat-e2e.exe'));await record();console.log(JSON.stringify({round,runId,stats:result.stats,green:result.green}));
  }
}
await record();console.log(`Evidence: ${root}`);
if(results.some(result=>result.code!==0||result.green===false))process.exitCode=1;
