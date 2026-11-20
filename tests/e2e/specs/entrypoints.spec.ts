import { test, expect } from '../fixtures/native';
import { McpClient } from '../fixtures/mcp';
import { ordinary, flags } from '../expected/recipes';
import { spawn } from 'node:child_process';
import net from 'node:net';
import path from 'node:path';
import { readFile } from 'node:fs/promises';

test('MCP preflight, creation and start use the same final balanced recipe', async ({ app }) => {
  const server = net.createServer(); await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as net.AddressInfo).port; await new Promise<void>(resolve => server.close(() => resolve()));
  await app.page.evaluate(({port,root}) => (window as any).__TAURI_INTERNALS__.invoke('set_mcp_settings', { settings: { enabled: true, port, inputRoots: [root] } }), {port,root:path.join(app.root,'fixtures')});
  const client = new McpClient(`http://127.0.0.1:${port}/mcp`); await client.initialize();
  const input = (await app.snapshot()).inputs[0];
  const inspection = await client.call('inspect_generation_input', { input_path: input, quality: 'balanced' });
  const task = await client.call('create_generation_task', {
    input_path: input, quality: 'balanced', client_request_id: 'balanced-e2e',
    preflight_id: inspection.preflight_id, input_source_identity: inspection.input_source_identity,
    gpu_selection: { localDeviceId: 'e2e-gpu-0', deviceKind: 'discreteGpu', selectionConfidence: 'ordered' }, answers: {},
  });
  await client.call('start_task', { task_id: task.task_id });
  const s = await app.waitTerminal('completed', task.task_id);
  expect(Number(flags(s.calls.find(c => c.operation === 'train')!.args)['--max-splats'])).toBe(ordinary.balanced.photoCap);
  expect(Number(flags(s.calls.find(c => c.operation === 'train')!.args)['--total-train-iters'])).toBe(ordinary.balanced.steps);
});

test('CLI generation uses the same final fast recipe without acquiring a real GPU', async ({ app }) => {
  const input = (await app.snapshot()).inputs[0];
  await app.stop();
  const token = await readFile(path.join(app.root, '.owner'), 'utf8');
  const result = await new Promise<{code:number|null;output:string}>((resolve,reject) => {
    const child=spawn(path.resolve('src-tauri/target/debug/splatstudio-e2e.exe'), ['generate', input, '--quality','fast','--gpu','e2e-gpu-0','--projects-root',path.join(app.root,'cli-projects')], {
      cwd:path.resolve('.'), windowsHide:true,env:{...process.env,OOOSPLAT_E2E_ROOT:app.root,OOOSPLAT_E2E_TOKEN:token,TEMP:path.join(app.root,'temp'),TMP:path.join(app.root,'temp')},stdio:['ignore','pipe','pipe'],
    });
    let output='';child.stdout.on('data',data=>output+=data);child.stderr.on('data',data=>output+=data);
    child.once('error',reject);child.once('exit',code=>resolve({code,output}));
  });
  expect(result.code,result.output).toBe(0);
  await app.start();const s=await app.snapshot();const train=s.calls.find(c=>c.operation==='train')!;
  expect(Number(flags(train.args)['--total-train-iters'])).toBe(ordinary.fast.steps);
  expect(Number(flags(train.args)['--max-splats'])).toBe(ordinary.fast.photoCap);
});
