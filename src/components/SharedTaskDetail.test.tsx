// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import type { SharedTask } from '../types/tasks';
import type { ProjectSummary } from '../types/pipeline';

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

describe('restored shared task detail', () => {
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

  it.each(['failed', 'cancelled', 'interrupted'] as const)('shows %s consistently without offering resume before a project/checkpoint exists', async status => {
    await render(task({ status, stage: null, estimated_progress: null }));
    expect(button('继续生成')).toBeUndefined();
    expect(container.querySelector('.cancel-action')).toBeNull();
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('—');
    expect(container.querySelectorAll('.stage-timeline .failed, .stage-timeline .cancelled, .stage-timeline .interrupted')).toHaveLength(0);
  });

  it('marks a known failed stage and keeps the useful error without replacing it with a guessed cause', async () => {
    await render(task({ status: 'failed', stage: 'failed', error: { code: 'pipeline_failed', message: 'device lost', failed_stage: 'trainingSplats', engine: 'brush', exit_code: 2, failure_id: null, classification: 'brush_gpu', classification_is_heuristic: true } }));
    await act(async () => container.querySelector<HTMLElement>('.inline-error .compact-error')!.focus());
    expect(document.querySelector('.error-detail-tooltip')?.textContent).toBe('device lost');
    expect(container.querySelectorAll('.stage-timeline li')[5].className).toBe('failed');
    expect(container.textContent).toContain('启发式判断');
  });

  it('offers explicit GUI resume only after an existing checkpoint can be read', async () => {
    mocks.detail.mockResolvedValue({ project, stage: 'trainingSplats', inputType: 'video', video: null, imageSequence: null });
    await render(task({ project_id: project.id, project_path: project.projectPath, status: 'interrupted' }));
    await act(async () => button('继续生成')!.click());
    expect(resume).toHaveBeenCalledWith(expect.objectContaining({ id: project.id, status: 'interrupted' }));
    expect(mocks.start).not.toHaveBeenCalled();
  });

  it('restores result statistics without adding a detail action bar', async () => {
    const result = { projectId: project.id, projectPath: project.projectPath, finalPly: project.finalPly!, fileSize: 2048, splatCount: 1500, inputImages: 100, registeredImages: 90, registeredRatio: .9, points3d: 3000, durationMs: 60000, completedAt: project.completedAt!, warning: null, logsDirectory: `${project.projectPath}\\logs` };
    await render(task({ project_id: project.id, project_path: project.projectPath, status: 'completed', stage: 'completed', result, estimated_progress: 100 }));
    expect(container.querySelector('.project-detail-stats')?.textContent).toContain('90 / 100');
    expect(container.querySelector('.result-actions')).toBeNull();
    expect(container.querySelectorAll('.stage-timeline .done')).toHaveLength(7);
  });

  it('shows inherited configuration only for an explicit resume or reshoot', async () => {
    await render(task({ runs: [{ run_id: 'run-1', kind: 'resume' }] }));
    expect(container.querySelector('.locked-setting')?.textContent).toContain('沿用原任务');
    await render(task({ task_kind: 'reshoot' }));
    expect(container.querySelector('.locked-setting')?.textContent).toContain('继承源任务');
  });

  it('keeps file logs bounded and does not invent timestamps or levels for raw lines', async () => {
    mocks.logs.mockResolvedValue(page(Array.from({ length: 1200 }, (_, index) => `raw ${index}`).join('\n')));
    await render(task());
    expect(container.querySelectorAll('.log-line')).toHaveLength(500);
    expect(container.querySelector('.log-line time')?.textContent).toBe('');
    expect(container.querySelector('.log-line')?.className).toBe('log-line historical');
    expect(container.querySelector('.live-log')?.textContent).not.toContain('raw 0');
  });

  it('keeps log rows stable during runtime-only updates and explains display truncation', async () => {
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
      expect(container.textContent).toContain('完整内容保存在项目日志中');
      await render(task({ task_id: 'other-task', run_id: 'other-run' }));
      expect(container.textContent).not.toContain('完整内容保存在项目日志中');
    } finally { format.mockRestore(); }
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
