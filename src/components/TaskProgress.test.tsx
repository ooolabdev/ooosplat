// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it } from 'vitest';
import { LanguageProvider } from '../i18n';
import { TaskProgress } from './TaskProgress';
import type { RuntimeSnapshot } from '../types/pipeline';

it.each([
  ['zh-CN', 'running'], ['zh-CN', 'completed'], ['en', 'running'], ['en', 'completed'],
])('omits the truncation notice for %s %s logs', async (locale, status) => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', locale);
  const container = document.createElement('div'), root = createRoot(container);
  try {
    await act(async () => root.render(<LanguageProvider><TaskProgress progress={50} status={status}
      stage="matching" logsTruncated logs={[{ key: 'entry-1', text: 'saved log entry', source: 'system' }]} /></LanguageProvider>));
    expect(container.textContent).not.toContain('完整内容保存在项目日志中');
    expect(container.textContent).not.toContain('Complete output is saved in the project logs.');
    expect(container.querySelector('.live-process > .runtime-note')).toBeNull();
    expect(container.querySelector('.live-log')?.textContent).toContain('saved log entry');
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});

it('always displays a scoped monotonic percentage without remounting the monitor', async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'zh-CN');
  const container = document.createElement('div');
  const root = createRoot(container);
  const render = (progress: number | null, progressKey = 'task-a:run-1', status = 'running') => act(async () => root.render(
    <LanguageProvider><TaskProgress progressKey={progressKey} progress={progress} status={status} stage="matching" logs={[]} /></LanguageProvider>));
  const heading = () => container.querySelector('.live-heading .mono')!.textContent;
  try {
    await render(null); expect(heading()).toBe('0.0%');
    const panel = container.querySelector('.runtime-panel');
    await render(63.8); expect(heading()).toBe('63.8%');
    for (const value of [null, NaN, Infinity, -5, 20]) {
      await render(value); expect(heading()).toBe('63.8%');
      expect(container.querySelector('.runtime-panel')).toBe(panel);
    }
    await render(99.99); expect(heading()).toBe('99.9%');
    expect(container.querySelector('.stage-timeline li.active small')?.textContent).toBe('处理中');
    await render(null, 'task-a:run-1', 'failed'); expect(heading()).toBe('99.9%');
    await render(null, 'task-b:run-1'); expect(heading()).toBe('0.0%');
    await render(42, 'task-b:run-1'); expect(heading()).toBe('42.0%');
    await render(null, 'task-b:run-2'); expect(heading()).toBe('0.0%');
    await render(null, 'task-b:run-2', 'completed'); expect(heading()).toBe('100.0%');
    expect(container.querySelector('.live-heading')!.textContent).not.toMatch(/估算|—/);
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});

it('retains fresh preparation-process resource monitoring without showing Brush training metrics', async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'), root = createRoot(container);
  const snapshot: RuntimeSnapshot = { processId: 24, phase: 'running', updatedAt: new Date().toISOString(),
    lastOutputAgeMs: 0, training: null, device: null, backend: null, config: {}, resources: null };
  try {
    await act(async () => root.render(<LanguageProvider><TaskProgress progress={60} status="running" stage="trainingSplats"
      substage="trainingPreparation" runtime={snapshot} message="Preparing training images 12/251" remainingSeconds={50} logs={[]} /></LanguageProvider>));
    expect(container.querySelector('.runtime-heading')?.textContent).toContain('24');
    expect(container.querySelector('.runtime-resources')).not.toBeNull();
    expect(container.querySelector('.runtime-step-heading')).toBeNull();
    expect(container.querySelector('.training-remaining')).toBeNull();
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});

it('marks ordinary Bridge Mapper percentages as estimates, but not its exact feature counts or unknown progress', async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'), root = createRoot(container);
  const render = async (unit: string, stageProgress: number | null) => act(async () => root.render(<LanguageProvider>
    <TaskProgress status="running" stage="validatingReconstruction" progress={50} stageProgress={stageProgress} progressUnit={unit} logs={[]} />
  </LanguageProvider>));
  try {
    await render('registeredImages', 36);
    const slot = () => container.querySelector('.stage-timeline li.active small');
    expect(slot()?.textContent).toBe('36.0%');
    expect(slot()?.getAttribute('aria-label')).toBe('Estimated 36.0%');
    await render('images', 36);
    expect(slot()?.getAttribute('aria-label')).toBeNull();
    await render('registeredImages', null);
    expect(slot()?.textContent).toBe('Processing');
    expect(slot()?.getAttribute('aria-label')).toBeNull();
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});
