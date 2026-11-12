// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
const mocks = vi.hoisted(() => ({ detail: vi.fn() }));
vi.mock('../lib/backend', () => ({ getProjectTaskDetail: mocks.detail }));
import { HistoricalTaskProgress } from './HistoricalTaskProgress';
import { LanguageProvider } from '../i18n';
import type { PipelineEvent, ProjectSummary, ProjectTaskDetail } from '../types/pipeline';

it('uses the same canonical event display for historical projects and labels legacy raw sources separately', async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'); document.body.append(container);
  const root = createRoot(container);
  const project = { id: 'project', status: 'completed' } as ProjectSummary;
  const detail = { stage: 'completed', progress: 100, logs: [{ source: 'brush', message: 'legacy raw' }],
    activityEvents: [{ sequence: 1, message: '正在检查可复用结果', engine: 'system', kind: 'log' } as PipelineEvent],
    activityJournalPersisted: true,
  } as ProjectTaskDetail;
  try {
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={detail} /></LanguageProvider>));
    expect(container.querySelector('.live-log')?.textContent).toContain('Checking reusable results');
    expect(container.textContent).not.toContain('legacy raw');
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={{ ...detail, activityEvents: null, activityJournalPersisted: undefined }} /></LanguageProvider>));
    expect(container.querySelector('.legacy-source-log > summary')?.textContent).toBe('brush');
    expect(container.querySelector('.legacy-source-log')?.textContent).toContain('legacy raw');
    expect(container.textContent).not.toContain('完整内容保存在项目日志中');
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={{ ...detail, logsUnavailable: true }} /></LanguageProvider>));
    expect(container.textContent).toContain('Logs could not be updated. Reading again.');
    expect(container.querySelector('.inline-error')).toBeNull();
  } finally { await act(async () => root.unmount()); container.remove(); localStorage.clear(); }
});

it('retries unavailable historical logs after 15 seconds, updates only logs and stops after success', async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers(); mocks.detail.mockReset(); localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'); document.body.append(container); const root = createRoot(container);
  const project = { id: 'project', status: 'completed' } as ProjectSummary;
  const detail = { stage: 'completed', progress: 100, logsUnavailable: true,
    logs: [{ source: 'brush', message: 'retained legacy line' }],
  } as ProjectTaskDetail;
  mocks.detail.mockResolvedValue({ ...detail, stage: 'matching', progress: 1, logsUnavailable: false,
    activityEvents: [{ sequence: 1, kind: 'log', engine: 'system', message: '正在恢复结果发布' }], logs: [],
  });
  try {
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={detail} /></LanguageProvider>));
    expect(mocks.detail).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTimeAsync(15000));
    expect(mocks.detail).toHaveBeenCalledWith('project');
    expect(container.querySelector('.live-log')?.textContent).toContain('Resuming result publication');
    expect(container.textContent).not.toContain('Logs could not be updated');
    expect(container.querySelector('.live-heading .mono')?.textContent).toBe('100.0%');
    expect(container.querySelectorAll('.stage-timeline .done')).toHaveLength(7);
    await act(async () => vi.advanceTimersByTimeAsync(60000));
    expect(mocks.detail).toHaveBeenCalledOnce();
  } finally { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); localStorage.clear(); }
});

it('disposes historical retry timers and ignores a delayed read after changing projects', async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers(); mocks.detail.mockReset(); localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'); document.body.append(container); const root = createRoot(container);
  const project = { id: 'old', status: 'completed' } as ProjectSummary;
  const detail = { stage: 'completed', progress: 100, logsUnavailable: true, logs: [] } as unknown as ProjectTaskDetail;
  let finish!: (value: ProjectTaskDetail) => void;
  mocks.detail.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockResolvedValueOnce({ ...detail, logsUnavailable: false, logs: [{ source: 'brush', message: 'new project log' }] });
  try {
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={detail} /></LanguageProvider>));
    await act(async () => vi.advanceTimersByTimeAsync(15000));
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={{ ...project, id: 'new' }} detail={detail} /></LanguageProvider>));
    await act(async () => finish({ ...detail, logsUnavailable: false, logs: [{ source: 'brush', message: 'old project secret' }] }));
    expect(container.textContent).not.toContain('old project secret');
    await act(async () => vi.advanceTimersByTimeAsync(15000));
    expect(mocks.detail).toHaveBeenLastCalledWith('new');
    expect(container.querySelector('.live-log')?.textContent).toContain('new project log');
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={{ ...project, id: 'pending' }} detail={detail} /></LanguageProvider>));
    await act(async () => root.unmount());
    await act(async () => vi.advanceTimersByTimeAsync(60000));
    expect(mocks.detail).toHaveBeenCalledTimes(2);
  } finally { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); localStorage.clear(); }
});

it('does not report or retry absent legacy journals, or journals without a complete persistence marker', async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers(); mocks.detail.mockReset(); localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'); document.body.append(container); const root = createRoot(container);
  const project = { id: 'legacy', status: 'completed' } as ProjectSummary;
  const detail = { stage: 'completed', progress: 100,
    logs: [{ source: 'brush', message: 'verified legacy output' }],
  } as ProjectTaskDetail;
  try {
    for (const fields of [{}, { activityEvents: null }, { activityEvents: [], activityJournalPersisted: false }]) {
      await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={{ ...detail, ...fields }} /></LanguageProvider>));
      expect(container.textContent).not.toContain('Logs could not be updated');
      await act(async () => vi.advanceTimersByTimeAsync(30000));
    }
    expect(mocks.detail).not.toHaveBeenCalled();
  } finally { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); localStorage.clear(); }
});

it('keeps repaired logs across unrelated detail updates but respects a new failing log read with identical contents', async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers(); mocks.detail.mockReset(); localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'); document.body.append(container); const root = createRoot(container);
  const project = { id: 'project', status: 'completed' } as ProjectSummary;
  const detail = { stage: 'completed', progress: 100, logsUnavailable: true,
    logs: [{ source: 'brush', message: 'original verified output' }],
  } as ProjectTaskDetail;
  mocks.detail.mockResolvedValue({ ...detail, logsUnavailable: undefined,
    logs: [{ source: 'brush', message: 'recovered output' }],
  });
  try {
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={detail} /></LanguageProvider>));
    expect(container.textContent).toContain('Logs could not be updated');
    await act(async () => vi.advanceTimersByTimeAsync(15000));
    expect(container.textContent).toContain('recovered output');
    expect(container.textContent).not.toContain('Logs could not be updated');

    // Unrelated detail updates must not resurrect an already repaired error,
    // or start an endless new polling session.
    const equivalent = { ...detail, progress: 99 };
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={equivalent} /></LanguageProvider>));
    expect(container.textContent).toContain('recovered output');
    expect(container.textContent).not.toContain('Logs could not be updated');
    await act(async () => vi.advanceTimersByTimeAsync(60000));
    expect(mocks.detail).toHaveBeenCalledOnce();

    // A new failed backend read may retain the very same verified lines; its
    // explicit failure must not be hidden by the prior successful recovery.
    const changed = { ...detail, logs: detail.logs.map(line => ({ ...line })) };
    await act(async () => root.render(<LanguageProvider><HistoricalTaskProgress project={project} detail={changed} /></LanguageProvider>));
    expect(container.textContent).toContain('Logs could not be updated');
    expect(container.textContent).not.toContain('recovered output');
    await act(async () => vi.advanceTimersByTimeAsync(15000));
    expect(mocks.detail).toHaveBeenCalledTimes(2);
    expect(container.textContent).not.toContain('Logs could not be updated');
  } finally { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); localStorage.clear(); }
});
