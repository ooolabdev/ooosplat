// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import type { ProjectSummary } from '../types/pipeline';
import type { SharedTask } from '../types/tasks';

const mocks = vi.hoisted(() => ({ detail: vi.fn() }));
vi.mock('../lib/backend', () => ({ getProjectTaskDetail: mocks.detail }));
import { ProjectRow } from './ProjectRow';

const project: ProjectSummary = {
  id: 'project-1', name: 'capture', status: 'interrupted', projectPath: 'E:\\Projects\\capture',
  finalPly: null, fileSize: null, splatCount: null, createdAt: '2026-11-01T00:00:00Z',
  completedAt: null, durationMs: 1000, quality: 'balanced', sourceName: 'capture.mov',
  registeredRatio: null, points3d: null, failureMessage: null,
};
const task: SharedTask = {
  task_id: 'task-1', run_id: 'run-1', project_id: project.id, project_path: project.projectPath,
  input_path: 'E:\\Media\\capture.mov', input_type: 'video', quality: 'balanced', source: 'gui',
  task_kind: 'generation', planner_enabled: true, projects_root: 'E:\\Projects', status: 'interrupted',
  stage: 'trainingSplats', revision: 1, sequence: 1, created_at: project.createdAt,
  updated_at: project.createdAt, elapsed_ms: 1000, progress: 50, estimated_progress: 75,
  current: null, total: null, eta_seconds: null, error: null, result: null, recent_events: [],
};

let root: Root;
let container: HTMLDivElement;
beforeEach(() => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.localStorage.setItem('ooo-splat-language', 'zh-CN');
  mocks.detail.mockReset().mockResolvedValue({ project, stage: 'trainingSplats' });
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  window.localStorage.clear();
});

it.each((['failed', 'cancelled', 'interrupted'] as const).flatMap(status =>
  (['zh-CN', 'en'] as const).map(locale => [status, locale] as const),
))('passes the current run identity when continuing a %s project without a detail read (%s)', async (status, locale) => {
  window.localStorage.setItem('ooo-splat-language', locale);
  const resume = vi.fn();
  await act(async () => root.render(<LanguageProvider><ProjectRow
    project={{ ...project, status }} task={{ ...task, status }} selected={false} busy={false} previewing={false}
    previewDisabled deleting={false} revealing={false} onSelect={() => undefined}
    onPreview={() => undefined} onReshoot={() => undefined} onResume={resume}
    onReveal={() => undefined} onDelete={() => undefined}
  /></LanguageProvider>));
  await act(async () => { await Promise.resolve(); });
  const button = container.querySelector<HTMLButtonElement>('.resume-link')!;
  expect(button.textContent).toBe(locale === 'zh-CN' ? '继续生成' : 'Continue generation');
  await act(async () => button.click());
  expect(resume).toHaveBeenCalledWith(expect.objectContaining({ id: project.id, status }), 'run-1');
  expect(mocks.detail).not.toHaveBeenCalled();
});

it('renders a persisted adoption warning from the completed task result', async () => {
  await act(async () => root.render(<LanguageProvider><ProjectRow
    project={{ ...project, status: 'completed', finalPly: 'E:\\Projects\\capture\\final.ply' }}
    task={{ ...task, status: 'completed', result: {
      projectId: project.id, projectPath: project.projectPath, finalPly: 'E:\\Projects\\capture\\final.ply',
      fileSize: 1, splatCount: 1, inputImages: 10, registeredImages: 8, registeredRatio: .8,
      points3d: 1, durationMs: 1000, completedAt: project.createdAt,
      warning: '部分素材未能参与生成：detail-set。本次仅使用已连接到同一场景的素材。', logsDirectory: 'logs',
    }}}
    selected={false} busy={false} previewing={false} previewDisabled deleting={false} revealing={false}
    onSelect={() => undefined} onPreview={() => undefined} onReshoot={() => undefined}
    onResume={() => undefined} onReveal={() => undefined} onDelete={() => undefined}
  /></LanguageProvider>));
  expect(container.querySelector('.input-adoption-warning')?.textContent).toContain('detail-set');
});
