// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import { mergeTask, type SharedTask } from '../types/tasks';
import type { ProjectSummary, ReshootRecipeSummary } from '../types/pipeline';

const mocks = vi.hoisted(() => ({ logs: vi.fn(), detail: vi.fn(), start: vi.fn(), cancel: vi.fn() }));
vi.mock('../lib/backend', () => ({ readSharedTaskLogs: mocks.logs, getProjectTaskDetail: mocks.detail, startGuiTask: mocks.start, cancelSharedTask: mocks.cancel }));
import { SharedTaskDetail } from './SharedTaskDetail';

const task = (overrides: Partial<SharedTask> = {}): SharedTask => ({
  task_id: 'agent-1', run_id: 'run-1', project_id: null, project_path: null, input_path: 'E:\\Media\\orbit.mov', input_type: 'video',
  quality: 'balanced', source: 'mcp', task_kind: 'generation', planner_enabled: false, projects_root: 'E:\\Projects',
  status: 'running', stage: 'trainingSplats', revision: 2, sequence: 1, created_at: '2026-10-06T00:00:00Z', updated_at: '2026-10-06T00:01:00Z',
  elapsed_ms: 60000, progress: 25, estimated_progress: 65, current: 3750, total: 15000, eta_seconds: null,
  error: null, result: null, recent_events: [], ...overrides,
});
const project: ProjectSummary = { id: 'project-1', name: 'orbit', workspaceTaskId: 'agent-1', status: 'completed', projectPath: 'E:\\Projects\\orbit', finalPly: 'E:\\Projects\\orbit\\final.ply', fileSize: 2048, splatCount: 1500, createdAt: '2026-10-06T00:00:00Z', completedAt: '2026-10-06T00:01:00Z', durationMs: 60000, quality: 'balanced', sourceName: 'orbit.mov', registeredRatio: .9, points3d: 3000, failureMessage: null };
const page = (text = '', source = 'brush') => ({ entries: text ? [{ source, text, partial_line: false }] : [], next_cursor: 'next', cursor_reset: false, has_more: false, reset_reason: null });
const reshootRecipe: ReshootRecipeSummary = {
  options: { condition: 'appearance', allowFallback: true, sourceIdentity: 'source', inputIdentity: 'input', projection: 'perspective', imageMatchingStrategy: 'exhaustive' },
  initial: { reconstruction: 'incremental', training: 'cold' },
  actual: { reconstruction: 'incremental', training: 'cold' },
};
let root: Root, container: HTMLDivElement;
const resume = vi.fn();
const render = async (value: SharedTask) => { await act(async () => root.render(<LanguageProvider><SharedTaskDetail key={`${value.task_id}:${value.run_id}`} task={value} project={value.project_id ? project : null} onResume={resume} showRuntimePanel={false} /></LanguageProvider>)); };
const button = (text: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === text);
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.localStorage.setItem('ooo-splat-language', 'zh-CN');
  vi.resetAllMocks(); mocks.logs.mockResolvedValue(page()); mocks.detail.mockRejectedValue(new Error('No checkpoint'));
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
});
afterEach(async () => { await act(async () => root.unmount()); container.remove(); window.localStorage.clear(); });

it('shows new retry consent without waiting for a project detail and marks only actual completed retries', async () => {
  await render(task({generation_policy:{version:1,autoRetryEnabled:false,frameSamplingRatio:.5},status:'created',stage:null,project_id:null}));
  expect(container.querySelector('.planner-switch strong')?.textContent).toBe('自动重试');
  expect(container.querySelector('.planner-switch')?.getAttribute('aria-checked')).toBe('false');
  expect(container.textContent).not.toContain('已自动重试');
  await render(task({generation_policy:{version:1,autoRetryEnabled:true,frameSamplingRatio:.5},status:'completed',automatically_retried:true,project_id:null}));
  expect(container.textContent).toContain('已自动重试');
});

describe('restored shared task detail', () => {
  it.each([
    ['ordinary video', 'video', 'perspective', ['video-a'], [90]],
    ['ERP video', 'video', 'equirectangular', ['erp-video'], [122]],
    ['ERP photos', 'images', 'equirectangular', ['erp-images'], [40]],
    ['multiple videos', 'video', 'perspective', ['video-a', 'video-b'], [30, 60]],
    ['video and photos', 'images', 'perspective', ['video', 'images'], [30, 60]],
    ['reshoot new sources', 'images', 'perspective', ['new-video', 'new-images'], [30, 60]],
  ] as const)('shows global feature progress for %s without disguising group changes as retries', async (label, inputType, projection, sources, counts) => {
    mocks.detail.mockResolvedValue({ project, inputType, videoProjection: projection, inputProjection: projection });
    const total = counts.reduce((sum, value) => sum + value, 0);
    let completed = 0, sequence = 0;
    for (const [index, sourceId] of sources.entries()) {
      const operation = { id: index + 1, attempt: 1, sourceId };
      for (const count of [completed, completed + 1, completed + counts[index]]) {
        const progress = { sequence: ++sequence, kind: count === completed ? 'stage' : 'progress', stage: 'extractingFeatures',
          engine: 'colmap', operation, current: count, total, unit: 'images', message: 'actual engine step',
        } as import('../types/pipeline').PipelineEvent;
        await render(task({ project_id: project.id, task_kind: label === 'reshoot new sources' ? 'reshoot' : 'generation', input_type: inputType,
          stage: 'extractingFeatures', operation, revision: sequence, sequence, current: count, total, unit: 'images',
          recent_events: [progress], progress: count / total * 100,
        }));
        expect(container.querySelector('.current-message')?.textContent).toBe(`正在分析画面（${count}/${total}）`);
        expect(container.textContent).not.toContain('本阶段第');
      }
      completed += counts[index];
    }
    const operation = { id: sources.length + 1, attempt: 2, sourceId: sources.at(-1) };
    await render(task({ project_id: project.id, input_type: inputType, stage: 'extractingFeatures', operation, current: completed - 1,
      total, unit: 'images', recent_events: [], progress: null }));
    expect(container.querySelector('.current-message')?.textContent).toBe(`正在分析画面（${completed - 1}/${total}）`);
    expect(container.textContent).toContain('本阶段第 2 次执行');
  });

  it('restores authoritative global feature counters without borrowing a previous source or completed Bridge count', async () => {
    const old = { sequence: 1, kind: 'progress', stage: 'extractingFeatures', current: 90, total: 90, unit: 'images',
      operation: { id: 1, attempt: 1, sourceId: 'source-a' }, message: 'old completed feature step',
    } as import('../types/pipeline').PipelineEvent;
    await render(task({ stage: 'extractingFeatures', operation: { id: 2, attempt: 1, sourceId: 'source-b' },
      current: 31, total: 90, unit: 'images', recent_events: [old] }));
    expect(container.querySelector('.current-message')?.textContent).toBe('正在分析画面（31/90）');
    await render(task({ stage: 'extractingFeatures', operation: { id: 3, attempt: 1, sourceId: 'source-b:bridge' },
      current: 0, total: 18, unit: 'images', recent_events: [old] }));
    expect(container.querySelector('.current-message')?.textContent).toBe('正在分析画面（0/18）');
    expect(container.textContent).not.toContain('本阶段第');
    await render(task({ stage: 'matching', operation: { id: 4, attempt: 2 }, current: null, total: null, recent_events: [] }));
    expect(container.textContent).toContain('本阶段第 2 次执行');
    await render(task({ stage: 'trainingSplats', substage: 'training', operation: { id: 5, attempt: 2 }, current: 0, total: 12000, unit: 'iterations', recent_events: [] }));
    expect(container.textContent).toContain('本阶段第 2 次执行');
  });

  it('keeps same-operation feature snapshots stable and preserves reconstruction estimate semantics', async () => {
    const known = task({ stage: 'extractingFeatures', operation: { id: 1, attempt: 1, sourceId: 'images' },
      current: 18, total: 90, unit: 'images', progress: 20, estimated_progress: 25,
    });
    let tasks = mergeTask({}, known);
    await render(tasks[known.task_id]);
    tasks = mergeTask(tasks, { ...known, revision: 3, current: null, total: null, unit: null, progress: null });
    await render(tasks[known.task_id]);
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe('20.0%');
    expect(container.querySelector('.current-message')?.textContent).toBe('正在分析画面（18/90）');
    const accepted = tasks;
    tasks = mergeTask(tasks, { ...known, revision: 4, operation: { id: 1, attempt: 1, sourceId: 'wrong-reused-id' },
      current: 0, progress: 0 });
    expect(tasks).toBe(accepted);
    await render(tasks[known.task_id]);
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe('20.0%');
    expect(container.querySelector('.current-message')?.textContent).toBe('正在分析画面（18/90）');
    tasks = mergeTask(tasks, { ...known, revision: 4, operation: { id: 2, attempt: 1, sourceId: 'video' } });
    await render(tasks[known.task_id]);
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe('20.0%');
    await render(task({ stage: 'reconstructing', operation: { id: 3, attempt: 1 }, current: 100, total: 100,
      unit: 'registeredImages', progress: 90, estimated_progress: 54 }));
    const estimate = container.querySelector('.stage-timeline li.active small');
    expect(estimate?.textContent).toBe('90.0%');
    expect(estimate?.getAttribute('title')).toBe('估算 90.0%');
    await render(task({ stage: 'reconstructing', operation: { id: 4, attempt: 2 }, current: 100, total: 100,
      unit: 'registeredImages', progress: null, estimated_progress: 54 }));
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe('处理中');
    expect(container.textContent).toContain('本阶段第 2 次执行');
  });

  it.each(['zh-CN', 'en'] as const)('keeps ordinary Mapper captions stable through real submodel reset patterns without hiding observed counts or estimates (%s)', async locale => {
    localStorage.setItem('ooo-splat-language', locale);
    const operation = { id: 8, attempt: 1, sourceId: 'ordinary-images' };
    const events: import('../types/pipeline').PipelineEvent[] = [];
    const observations = [[0, 0], [2, 2], [3, 3], [0, 3], [2, 3], [4, 4], [0, 4], [6, 6], [0, 6]];
    for (const [index, [current, estimate]] of observations.entries()) {
      events.push({ sequence: index + 1, kind: 'progress', stage: 'reconstructing', engine: 'colmap',
        current, total: 90, unit: 'registeredImages', operation, stageProgress: estimate,
        progress: 45 + estimate * .13, indeterminate: false,
        message: current === 0 ? 'Registering initial image pair' : `已注册 ${current} 张图像`,
      } as import('../types/pipeline').PipelineEvent);
      await render(task({ stage: 'reconstructing', input_type: 'images', operation, current, total: 90,
        unit: 'registeredImages', progress: estimate, estimated_progress: 45 + estimate * .13,
        revision: index + 3, sequence: index + 1, recent_events: [...events] }));
      expect(container.querySelector('.current-message')?.textContent).toBe(locale === 'en'
        ? 'Rebuilding the captured scene. This step may take a while. Please wait.'
        : '正在还原拍摄场景。此步骤可能耗时较长，请耐心等待');
      expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe(`${estimate.toFixed(1)}%`);
      expect(container.querySelector('.live-log')?.textContent).toContain('Registering initial image pair');
      expect(events.at(-1)?.current).toBe(current);
    }
    await render(task({ stage: 'reconstructing', substage: 'panoramaReconstruction', current: 3, total: 90,
      unit: 'registeredImages', progress: null, recent_events: [] }));
    expect(container.querySelector('.current-message')?.textContent).toBe(locale === 'en'
      ? 'Current model: 3/90 original images registered. This step may take a while. Please wait.'
      : '当前模型已注册 3/90 张原始画面。此步骤可能耗时较长，请耐心等待');
  });

  it('allows a continuation without globally ready engines and closes the queue before starting', async () => {
    const before = vi.fn();
    await act(async () => root.render(<LanguageProvider><SharedTaskDetail task={task({ status: 'failed' })}
      canStart={false} canContinue onBeforeContinue={before} /></LanguageProvider>));
    expect(button('继续生成')!.disabled).toBe(false);
    await act(async () => button('继续生成')!.click());
    expect(before).toHaveBeenCalledOnce();
    expect(mocks.start).toHaveBeenCalledWith('agent-1', 'run-1');
    expect(before.mock.invocationCallOrder[0]).toBeLessThan(mocks.start.mock.invocationCallOrder[0]);
  });
  it('shows persisted GPS fallback only for sources included in the structured summary', async () => {
    const inputs: import('../types/inputs').SavedInputs = {
      request: { mode: 'materials', items: ['gps', 'other'].map(id => ({ id, source: { kind: 'imageFolder', path: `E:/${id}` }, projection: 'perspective', imageMatchingStrategy: 'exhaustive' })) },
      sources: ['gps', 'other'].map(id => ({ id, identity: id, projection: 'perspective', names: [], video: null, imageSequence: { imageCount: 40, width: 800, height: 600 } as import('../types/pipeline').ImageSequenceInfo, adopted: 40 })),
      unused: [], importedReconstruction: false,
    };
    mocks.detail.mockResolvedValue({ project, inputs, inputType: 'images' });
    await render(task({ project_id: project.id, status: 'completed', input_type: 'images', gps_assistance: { status: 'fallback', detectedSourceIds: ['gps'] } }));
    expect(container.querySelector('.gps-source-hint')?.textContent).toBe('GPS');
    expect(container.querySelector('.gps-source-hint')?.getAttribute('title')).toContain('GPS 辅助未启用');
    const rows = container.querySelectorAll<HTMLButtonElement>('.material-row-main');
    await act(async () => rows[1].click());
    expect(container.querySelector('.gps-source-hint')).toBeNull();
    expect(container.querySelector('.material-inspector select')).toBeNull();
  });
  it('shows localized quiet notices as system logs without inventing counts', async () => {
    localStorage.setItem('ooo-splat-language', 'en');
    const notice = { kind: 'log', engine: 'system', stage: 'matching', sequence: 1,
      message: '图像匹配进程仍在运行，最近 15 秒无新输出。此步骤可能耗时较长，请耐心等待' } as import('../types/pipeline').PipelineEvent;
    await render(task({ stage: 'matching', current: null, total: null, recent_events: [notice] }));
    expect(container.querySelector('.live-log')?.textContent).toContain('no new output for 15 seconds');
    expect(container.querySelector('.live-log')?.textContent).toContain('system');
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).not.toBe('100.0%');
  });
  it.each(['zh-CN', 'en'] as const)('localizes Brush progress and legacy stage events but preserves raw event/file logs and the current caption (%s)', async locale => {
    localStorage.setItem('ooo-splat-language', locale);
    const legacyStart = 'Brush 训练开始（使用可用图形后端）· 15000 iterations · 最大分辨率 1920 · 预计约 100:05:09';
    const step = 'Brush 训练 · 1200/15000 步';
    const rawEvent = 'Brush 训练 · 7/15000 步';
    const rawFile = 'Brush 训练 · 9/15000 步';
    mocks.logs.mockResolvedValue(page(rawFile));
    const progress: import('../types/pipeline').PipelineEvent = {
      sequence: 2, timestamp: '2026-10-09T00:00:00Z', kind: 'progress', level: 'info',
      stage: 'trainingSplats', engine: 'brush', progress: 66, stageProgress: 8, indeterminate: false,
      message: step, current: 1200, total: 15_000, unit: 'steps', elapsedMs: 1000, acceleration: null,
    };
    await render(task({ current: 1200, total: 15_000, unit: 'steps', substage: 'training', recent_events: [
      { ...progress, sequence: 1, kind: 'stage', message: legacyStart, current: 0, unit: 'iterations',
        progress: 60, stageProgress: null, indeterminate: true },
      progress,
      { ...progress, sequence: 3, kind: 'log', message: rawEvent, current: null, unit: null,
        progress: 66, stageProgress: null, indeterminate: true },
    ] }));
    const lines = [...container.querySelectorAll('.live-log .log-line p')].map(line => line.textContent);
    expect(lines).toEqual(locale === 'zh-CN' ? [legacyStart, step, rawEvent, rawFile] : [
      'Brush training started (using an available graphics backend) · 15000 iterations · max resolution 1920 · estimated about 100:05:09',
      'Brush training · 1200/15000 steps', rawEvent, rawFile,
    ]);
    expect(container.querySelector('.legacy-source-log > summary')?.textContent).toBe('brush');
    expect(container.querySelector('.legacy-source-log')?.hasAttribute('open')).toBe(false);
    expect(container.querySelector('.current-message')?.textContent).toBe(locale === 'zh-CN'
      ? '正在生成高斯泼溅（1,200/15,000）。此步骤可能耗时较长，请耐心等待'
      : 'Generating Gaussian splats (1,200/15,000). This step may take a while. Please wait.');
  });
  it('does not revive an old monitor or count after the task snapshot explicitly cleared them', async () => {
    const old = { sequence: 1, timestamp: '2026-10-09T00:00:00Z', kind: 'progress', stage: 'trainingSplats', message: '', engine: 'system',
      substage: 'panoramaTrainingPreparation', current: 72, total: 228, unit: 'panoramaFaces',
      runtime: { training: { iteration: 999, total: 8000 } } } as unknown as import('../types/pipeline').PipelineEvent;
    await render(task({ substage: 'training', current: null, total: null, progress: null, runtime: null,
      operation: { id: 2, attempt: 2 }, recent_events: [old] }));
    expect(container.querySelector('.current-message')?.textContent).not.toMatch(/72|228|999/);
    expect(container.querySelector('.stage-timeline li.active b')?.textContent).toBe('Splat 训练');
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe('处理中');
    expect(container.textContent).toContain('本阶段第 2 次执行');
  });
  it('uses the same generic training preparation and checkpoint captions as the main task', async () => {
    const old = { sequence: 1, timestamp: '2026-10-09T00:00:00Z', kind: 'progress', stage: 'trainingSplats', message: '',
      substage: 'training', current: 8000, total: 8000, unit: 'iterations', operation: { id: 1, attempt: 1 } } as import('../types/pipeline').PipelineEvent;
    for (const [substage, caption] of [
      ['trainingPreparation', '正在准备训练图片 12/251'],
      ['trainingValidation', '正在校验训练图片和相机数据'],
      ['trainingCheckpoint', '正在保存生成检查点'],
    ]) {
      await render(task({ substage, current: 12, total: 251, unit: 'images', runtime: null,
        operation: { id: 2, attempt: 0 }, recent_events: [old] }));
      expect(container.querySelector('.current-message')?.textContent).toBe(caption);
      expect(container.querySelector('.stage-timeline li.active b')?.textContent).toBe('Splat 训练');
      expect(container.querySelector('.training-remaining')).toBeNull();
      expect(container.querySelector('.runtime-step-heading')).toBeNull();
      expect(container.querySelector('.current-message')?.textContent).not.toMatch(/全景|8,000/);
    }
  });
  it('shows the saved projection in the readonly media box without re-detecting input', async () => {
    mocks.detail.mockResolvedValue({ project, inputType: 'video', videoProjection: 'equirectangular', video: null, imageSequence: null });
    await render(task({ project_id: project.id, status: 'cancelled' }));
    expect(container.querySelector('.source-input-card .media-source-copy > strong')?.textContent).toBe('全景视频');
    expect(container.querySelector('.source-input-card .media-source-status')?.textContent).not.toContain('本任务按');
    expect(container.querySelector('.source-input-card .media-source-metadata')).toBeNull();
    expect(container.querySelector('.source-input-card select')).toBeNull();
  });
  it.each(['zh-CN', 'en'])('uses only preset progress captions for every stage in %s', async locale => {
    window.localStorage.setItem('ooo-splat-language', locale);
    const stages = [
      'created', 'probingVideo', 'planningFrames', 'extractingFrames', 'extractingFeatures', 'matching',
      'reconstructing', 'validatingReconstruction', 'trainingSplats', 'exporting', 'completed', 'failed', 'cancelled',
      'unknownEngineStage', '__proto__', null,
    ];
    const raw = 'RAW_ENGINE_DETAIL: CUDA worker 42 at E:\\Private\\input.png';
    for (const stage of stages) {
      const event: import('../types/pipeline').PipelineEvent = { sequence: 1, timestamp: '2026-10-09T00:00:00Z',
        kind: 'log', level: 'info', stage: stage ?? 'created', engine: 'colmap', progress: 50, stageProgress: null,
        indeterminate: true, message: raw, current: null, total: null, unit: null, elapsedMs: 1000, acceleration: null };
      await render(task({ stage, current: null, total: null, recent_events: [event] }));
      const caption = container.querySelector('.current-message')?.textContent;
      expect(caption, `caption for ${stage}`).toBeTruthy();
      expect(caption).not.toContain('RAW_ENGINE_DETAIL');
      expect(caption).not.toContain('Private');
      expect(container.querySelector('.live-log')?.textContent).toContain(raw);
      if (stage === 'planningFrames') expect(caption).toBe(locale === 'zh-CN' ? '正在规划画面…' : 'Planning frames…');
      if (stage === 'unknownEngineStage' || stage === '__proto__') expect(caption).toBe(locale === 'zh-CN' ? '运行中' : 'Running');
      if (stage == null) expect(caption).toBe(locale === 'zh-CN' ? '正在准备任务' : 'Preparing task');
    }
  });

  it('uses lifecycle templates without leaking the failure detail into the progress caption', async () => {
    const expected = { created: '正在准备任务', starting: '正在准备任务', cancelling: '正在终止任务',
      completed: '已完成', failed: '任务失败', cancelled: '已取消', interrupted: '已中断' };
    const error = { code: 'pipeline_failed', message: 'RAW_FAILURE_DETAIL: GPU driver reset at source.cpp:88',
      failed_stage: 'trainingSplats', engine: 'brush', exit_code: 2, failure_id: null,
      classification: null, classification_is_heuristic: false };
    for (const [status, caption] of Object.entries(expected)) {
      await render(task({ status: status as SharedTask['status'], error }));
      expect(container.querySelector('.current-message')?.textContent).toBe(caption);
      expect(container.querySelector('.current-message')?.textContent).not.toContain('RAW_FAILURE_DETAIL');
    }
    await act(async () => container.querySelector<HTMLElement>('.inline-error .compact-error')!.focus());
    expect(document.querySelector('.error-detail-tooltip')?.textContent).toBe(error.message);
  });

  it('uses the new classification for the error summary but leaves legacy summaries alone', async () => {
    const error = { code: 'pipeline_failed', message: 'wgpu error: Out of Memory\nIO error while loading dataset: early eof', failed_stage: 'trainingSplats', engine: 'brush', exit_code: 101, failure_id: null,
      classification: 'brush_out_of_memory', classification_version: 1, classification_is_heuristic: false };
    await render(task({ status: 'failed', error }));
    expect(container.querySelector('.inline-error .compact-error')?.textContent).toBe('训练显存不足');
    expect(container.textContent).not.toContain('错误分类为启发式判断');
    await render(task({ status: 'failed', error: { ...error, classification: 'brush_dataset', classification_version: undefined, classification_is_heuristic: true } }));
    expect(container.querySelector('.inline-error .compact-error')?.textContent).toBe('生成所需图片读取不完整');
    expect(container.textContent).toContain('错误分类为启发式判断');
  });

  it('uses the original configuration, timeline and log layout with frozen settings and observed counters', async () => {
    await render(task({ input_path: '\\\\?\\E:\\Media\\orbit.mov', projects_root: '\\\\?\\E:\\Projects' }));
    expect(container.querySelectorAll('.path-picker.readonly')).toHaveLength(2);
    expect(container.querySelector('.project-configuration-detail')?.textContent).toContain('E:\\Media\\orbit.mov');
    expect(container.querySelector('.project-configuration-detail')?.textContent).not.toContain('\\\\?\\');
    expect(container.querySelectorAll('.quality-option')).toHaveLength(3);
    expect(container.querySelector('.quality-option.selected')?.textContent).toContain('均衡');
    expect(container.querySelector('.planner-switch')?.getAttribute('aria-checked')).toBe('false');
    expect(container.querySelector('.locked-setting')).toBeNull();
    expect(container.querySelectorAll('.stage-timeline li')).toHaveLength(7);
    expect(container.querySelectorAll('.stage-timeline li')[5].className).toBe('active');
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('65.0%');
    expect(container.querySelector('.current-message')?.textContent).toContain('3,750/15,000');
    expect(container.querySelector('.live-log')).not.toBeNull();
    expect(container.querySelector('progress')).toBeNull();
  });

  it('starts a created task and cancels only the selected task/run identity', async () => {
    await render(task({ status: 'created', run_id: null, stage: null, progress: null, estimated_progress: null }));
    await act(async () => button('开始生成')!.click());
    expect(mocks.start).toHaveBeenCalledWith('agent-1');
    await render(task());
    await act(async () => container.querySelector<HTMLButtonElement>('.cancel-action')!.click());
    expect(mocks.cancel).toHaveBeenCalledWith('agent-1', 'run-1');
    await render(task({ status: 'cancelling' }));
    expect(container.querySelector<HTMLButtonElement>('.cancel-action')!.disabled).toBe(true);
  });

  it.each(['failed', 'cancelled', 'interrupted'] as const)('continues a %s pre-project run through the compatible task/run entry point', async status => {
    await render(task({status,project_id:null,stage:null}));
    expect(button('继续生成')).toBeDefined();
    await act(async () => button('继续生成')!.click());
    expect(mocks.start).toHaveBeenCalledWith('agent-1','run-1');
    expect(resume).not.toHaveBeenCalled();
  });

  it.each(['cancelled', 'interrupted'] as const)('shows %s consistently while allowing the backend to validate continuation', async status => {
    await render(task({ status, stage: null, estimated_progress: null }));
    expect(button('继续生成')).toBeDefined();
    expect(container.querySelector('.cancel-action')).toBeNull();
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('0.0%');
    expect(container.querySelectorAll('.stage-timeline .failed, .stage-timeline .cancelled, .stage-timeline .interrupted')).toHaveLength(0);
  });

  it('marks a known failed stage and keeps the useful error without replacing it with a guessed cause', async () => {
    await render(task({ status: 'failed', stage: 'failed', error: { code: 'pipeline_failed', message: 'device lost', failed_stage: 'trainingSplats', engine: 'brush', exit_code: 2, failure_id: null, classification: 'brush_gpu', classification_is_heuristic: true } }));
    await act(async () => container.querySelector<HTMLElement>('.inline-error .compact-error')!.focus());
    expect(document.querySelector('.error-detail-tooltip')?.textContent).toBe('device lost');
    expect(container.querySelectorAll('.stage-timeline li')[5].className).toBe('failed');
    expect(container.textContent).toContain('启发式判断');
  });

  it.each(['failed', 'cancelled', 'interrupted'] as const)('offers GUI continuation for %s without depending on a successful detail read', async status => {
    await render(task({ project_id: project.id, project_path: project.projectPath, status }));
    await act(async () => button('继续生成')!.click());
    expect(resume).toHaveBeenCalledWith(expect.objectContaining({ id: project.id, status }), 'run-1');
    expect(mocks.start).not.toHaveBeenCalled();
  });

  it.each(['zh-CN', 'en'] as const)('uses one continuation caption and blocks deleted/busy tasks (%s)', async locale => {
    localStorage.setItem('ooo-splat-language', locale);
    const caption = locale === 'zh-CN' ? '继续生成' : 'Continue generation';
    await render(task({ status: 'failed', project_id: project.id, project_path: project.projectPath }));
    expect(button(caption)).toBeDefined();
    await render(task({ status: 'failed', project_id: project.id, project_path: project.projectPath, project_deleted: true }));
    expect(button(caption)).toBeUndefined();
    await render(task({ status: 'running' }));
    expect(button(caption)).toBeUndefined();
  });

  it('shows persisted per-input adoption warnings after completion', async () => {
    mocks.detail.mockResolvedValue({
      project,
      inputType: 'images',
      inputs: {
        request: { mode: 'materials', items: [
          { id: 'used', source: { kind: 'imageFolder', path: 'E:\\Media\\used' }, projection: 'perspective', imageMatchingStrategy: 'exhaustive' },
          { id: 'unused', source: { kind: 'imageFolder', path: 'E:\\Media\\detail-set' }, projection: 'perspective', imageMatchingStrategy: 'exhaustive' },
        ] },
        sources: [
          { id: 'used', identity: 'one', projection: 'perspective', names: ['0001.jpg'], video: null, adopted: 12 },
          { id: 'unused', identity: 'two', projection: 'perspective', names: ['0002.jpg'], video: null, adopted: 0 },
        ],
        importedReconstruction: false,
        unused: [],
        sourceAdoption: [
          { sourceId: 'used', plannedFrames: 12, preparedFrames: 12, registeredFrames: 12, adopted: true },
          { sourceId: 'unused', plannedFrames: 10, preparedFrames: 10, registeredFrames: 0, adopted: false, omittedReason: 'not_connected' },
        ],
      },
    });
    await render(task({ project_id: project.id, project_path: project.projectPath, status: 'completed', stage: 'completed' }));
    const warning = container.querySelector('.input-adoption-warning');
    expect(warning?.textContent).toContain('detail-set');
    expect(warning?.textContent).toContain('部分素材未能参与生成');
  });

  it('restores result statistics without adding a detail action bar', async () => {
    const result = { projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly!, fileSize: 2048, splatCount: 1500, inputImages: 100, registeredImages: 90, registeredRatio: .9, points3d: 3000, durationMs: 60000, completedAt: project.completedAt!, warning: null, logsDirectory: `${project.projectPath}\\logs` };
    await render(task({ project_id: project.id, project_path: project.projectPath, status: 'completed', stage: 'completed', result, estimated_progress: 100 }));
    expect(container.querySelector('.project-detail-stats')?.textContent).toContain('90 / 100');
    expect(container.querySelector('.result-actions')).toBeNull();
    expect(container.querySelectorAll('.stage-timeline .done')).toHaveLength(7);
  });

  it('keeps inherited configuration for continuation and historical resume/retry records', async () => {
    for (const kind of ['continue', 'resume', 'retry']) {
      await render(task({ status: 'interrupted', runs: [{ run_id: 'run-1', kind }] }));
      expect(container.querySelector('.locked-setting')?.textContent).toContain('沿用原任务');
    }
    await render(task({ status: 'interrupted', task_kind: 'reshoot' }));
    expect(container.querySelector('.locked-setting')?.textContent).toContain('继承源任务');
  });

  it.each(['zh-CN', 'en'] as const)('hides plan details and inheritance while starting/running/cancelling (%s)', async locale => {
    localStorage.setItem('ooo-splat-language', locale);
    mocks.detail.mockResolvedValue({ project, reshootRecipe, inputType: 'video' });
    for (const status of ['starting', 'running', 'cancelling'] as const) {
      await render(task({ task_kind: 'reshoot', project_id: project.id, status }));
      const configuration = container.querySelector('.project-configuration-detail')!;
      expect(configuration).not.toBeNull();
      expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '补拍方案' : 'Reshoot plan');
      expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '整体结构正常' : 'The overall structure');
      expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '增量重建' : 'Incremental reconstruction');
      expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '允许失败后' : 'Allow a slower fallback');
      expect(configuration.querySelector('.locked-setting')).toBeNull();
      expect(configuration.querySelectorAll('.quality-option:disabled')).toHaveLength(3);
      expect(configuration.querySelector<HTMLButtonElement>('.planner-switch')?.disabled).toBe(true);
      expect(mocks.detail).toHaveBeenCalledWith(project.id);
    }
  });

  it.each(['zh-CN', 'en'] as const)('hides plan details and inheritance after a reshoot completes (%s)', async locale => {
    localStorage.setItem('ooo-splat-language', locale);
    mocks.detail.mockResolvedValue({ project, reshootRecipe, inputType: 'video' });
    await render(task({ task_kind: 'reshoot', project_id: project.id, status: 'running' }));
    await render(task({ task_kind: 'reshoot', project_id: project.id, status: 'completed', stage: 'completed' }));
    const configuration = container.querySelector('.project-configuration-detail')!;
    expect(configuration).not.toBeNull();
    expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '补拍方案' : 'Reshoot plan');
    expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '整体结构正常' : 'The overall structure');
    expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '增量重建' : 'Incremental reconstruction');
    expect(configuration.textContent).not.toContain(locale === 'zh-CN' ? '允许失败后' : 'Allow a slower fallback');
    expect(configuration.querySelector('input[type="checkbox"]')).toBeNull();
    expect(configuration.querySelector('.locked-setting')).toBeNull();
    expect(configuration.querySelectorAll('.quality-option:disabled')).toHaveLength(3);
    expect(configuration.querySelector<HTMLButtonElement>('.planner-switch')?.disabled).toBe(true);
    expect(configuration.textContent).toContain('E:\\Media\\orbit.mov');
    expect(reshootRecipe.options.allowFallback).toBe(true);
  });

  it('preserves saved plan and fallback decisions for unfinished inactive reshoots', async () => {
    mocks.detail.mockResolvedValue({ project, reshootRecipe, inputType: 'video' });
    await render(task({ task_kind: 'reshoot', project_id: project.id, status: 'running' }));
    expect(container.querySelector('.project-configuration-detail')?.textContent).not.toContain('补拍方案');
    for (const status of ['failed', 'cancelled', 'interrupted'] as const) {
      await render(task({ task_kind: 'reshoot', project_id: project.id, status }));
      expect(container.querySelector('.project-configuration-detail')?.textContent).toContain('补拍方案');
      expect(container.querySelector('.project-configuration-detail')?.textContent).toContain('整体结构正常，但生成效果不满意');
      expect(container.querySelector<HTMLInputElement>('.project-configuration-detail input[type="checkbox"]')?.checked).toBe(true);
    }
    expect(reshootRecipe.options.allowFallback).toBe(true);
  });

  it('keeps file logs bounded and does not invent timestamps or levels for raw lines', async () => {
    mocks.logs.mockResolvedValue(page(Array.from({ length: 1200 }, (_, index) => `raw ${index}`).join('\n')));
    await render(task());
    expect(container.querySelectorAll('.log-line')).toHaveLength(500);
    expect(container.querySelector('.log-line time')?.textContent).toBe('');
    expect(container.querySelector('.log-line')?.className).toBe('log-line historical');
    expect(container.querySelector('.live-log')?.textContent).not.toContain('raw 0');
  });

  it('keeps log rows stable during runtime-only updates without a display-truncation notice', async () => {
    mocks.logs.mockResolvedValue({ ...page(), activity_events: [], activity_journal_persisted: true });
    const events: import('../types/pipeline').PipelineEvent[] = [{ sequence: 1, timestamp: '2026-10-09T00:00:00Z',
      kind: 'log', level: 'info', stage: 'reconstructing', engine: 'colmap', progress: 50, stageProgress: null,
      indeterminate: true, message: 'registration detail', current: null, total: null, unit: null,
      elapsedMs: 1000, acceleration: null }];
    await render(task({ stage: 'reconstructing', recent_events: events }));
    const format = vi.spyOn(Date.prototype, 'toLocaleTimeString');
    try {
      for (let index = 0; index < 10; index++) await render(task({ stage: 'reconstructing', revision: index + 3,
        elapsed_ms: 60000 + index * 250, recent_events: events, dropped_event_count: 25 }));
      expect(format).not.toHaveBeenCalled();
      expect(container.querySelectorAll('.log-line')).toHaveLength(1);
      expect(container.textContent).not.toContain('完整内容保存在项目日志中');
      await render(task({ task_id: 'other-task', run_id: 'other-run' }));
      expect(container.textContent).not.toContain('完整内容保存在项目日志中');
    } finally { format.mockRestore(); }
  });

  it('does not claim complete disk history without a successful journal and scopes read failures to logs', async () => {
    mocks.logs.mockRejectedValue(new Error('LOG_ACCESS_DENIED'));
    await render(task({ dropped_event_count: 50, recent_events: [{ sequence: 1, kind: 'log', engine: 'system', message: 'useful line' } as import('../types/pipeline').PipelineEvent] }));
    expect(container.querySelector('.live-log')?.textContent).toContain('useful line');
    expect(container.textContent).toContain('日志暂时无法更新，正在重新读取');
    expect(container.textContent).not.toContain('完整内容保存在项目日志中');
    expect(container.querySelector('.inline-error')).toBeNull();
    expect(container.querySelector('.current-message')?.textContent).not.toContain('LOG_ACCESS_DENIED');
  });

  it('shows completed legacy logs without treating an absent activity journal as a read failure', async () => {
    mocks.logs.mockResolvedValue(page('saved legacy completion'));
    await render(task({ status: 'completed', stage: 'completed', progress: 100, recent_events: [] }));
    expect(container.querySelector('.legacy-source-log')?.textContent).toContain('saved legacy completion');
    expect(container.querySelector('.live-process [role="status"]')).toBeNull();
    expect(container.querySelector('.inline-error')).toBeNull();
    // A real read error must remain visible even for a completed task.
    mocks.logs.mockRejectedValueOnce(new Error('LOG_ACCESS_DENIED'));
    await render(task({ task_id: 'other-completed', run_id: 'other-run', status: 'completed', stage: 'completed' }));
    expect(container.querySelector('.live-process [role="status"]')).not.toBeNull();
    expect(container.querySelector('.inline-error')).toBeNull();
  });

  it('keeps structured history when a task completes and does not duplicate engine text from the file page', async () => {
    const event = { sequence: 3, taskId: 'agent-1', runId: 'run-1', timestamp: '2026-10-09T00:00:00Z', kind: 'log', engine: 'brush', message: 'repeated engine line' } as import('../types/pipeline').PipelineEvent;
    mocks.logs.mockResolvedValue({ ...page('repeated engine line'), activity_events: [event], activity_journal_persisted: true });
    await render(task({ recent_events: [event] }));
    await render(task({ status: 'completed', stage: 'completed', recent_events: [] }));
    expect([...container.querySelectorAll('.live-log .log-line p')].map(line => line.textContent)).toEqual(['repeated engine line']);
    expect(container.querySelector('.legacy-source-log')).toBeNull();
  });

  it('ignores delayed log reads from another task/run after selection changes', async () => {
    let finish!: (value: ReturnType<typeof page>) => void;
    mocks.logs.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockResolvedValue(page('new run log'));
    await render(task());
    await render(task({ task_id: 'agent-2', run_id: 'run-2', input_path: 'E:\\Media\\other.mp4' }));
    await act(async () => finish(page('old run secret')));
    expect(container.querySelector('.live-log')?.textContent).toContain('new run log');
    expect(container.querySelector('.live-log')?.textContent).not.toContain('old run secret');
    expect(mocks.logs).toHaveBeenLastCalledWith('agent-2', 'run-2', undefined);
  });
});
