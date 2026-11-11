// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it } from 'vitest';
import { LanguageProvider } from '../i18n';
import { useProgressMessage } from './useProgressMessage';
import { TaskProgress } from '../components/TaskProgress';
import type { PipelineEvent } from '../types/pipeline';
it('keeps face counts separate from Brush iterations and hides preparation ETA', async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'zh-CN');
  const container = document.createElement('div'), root = createRoot(container);
  const face = { stage: 'trainingSplats', substage: 'panoramaTrainingPreparation', engine: 'system', current: 72, total: 228, unit: 'panoramaFaces' } as PipelineEvent;
  const training = { ...face, substage: 'training', engine: 'brush', current: 1200, total: 8000, unit: 'iterations' } as PipelineEvent;
  function View({ events }: { events: PipelineEvent[] }) { const last = events.at(-1)!; const message = useProgressMessage('running', 'trainingSplats', events, null); return <TaskProgress status="running" stage="trainingSplats" substage={last.substage} progress={60} message={message} remainingSeconds={50} logs={[]} />; }
  await act(async () => root.render(<LanguageProvider><View events={[face]} /></LanguageProvider>));
  expect(container.textContent).toContain('全景帧 12/38，六面图片 72/228');
  expect(container.querySelector('.training-remaining')).toBeNull();
  expect(container.textContent).toContain('准备训练图片');
  await act(async () => root.render(<LanguageProvider><View events={[face, training]} /></LanguageProvider>));
  expect(container.textContent).toContain('正在生成高斯泼溅（1,200/8,000）。此步骤可能耗时较长，请耐心等待');
  expect(container.textContent).not.toContain('72/228');
  await act(async () => root.unmount());
});

it.each([
  ['reconstructing', null, 40, 173, 'registeredImages', '正在还原拍摄场景（40/173）。此步骤可能耗时较长，请耐心等待'],
  ['reconstructing', 'panoramaReconstruction', 40, 173, 'registeredImages', '当前模型已注册 40/173 张原始画面。此步骤可能耗时较长，请耐心等待'],
  ['reconstructing', 'panoramaImagesReconstruction', 40, 94, 'registeredImages', '当前模型已注册 40/94 张原始图片。此步骤可能耗时较长，请耐心等待'],
  ['extractingFrames', 'panoramaImagesPreparation', 12, 94, 'images', '正在准备全景图片 12/94'],
  ['trainingSplats', 'panoramaTrainingPreparation', 72, 228, 'panoramaImageFaces', '正在准备训练图片：全景图片 12/38，六面图片 72/228'],
  ['trainingSplats', null, 1200, 8000, 'steps', '正在生成高斯泼溅（1,200/8,000）。此步骤可能耗时较长，请耐心等待'],
  ['trainingSplats', 'training', 1200, 8000, 'iterations', '正在生成高斯泼溅（1,200/8,000）。此步骤可能耗时较长，请耐心等待'],
  ['trainingSplats', 'panoramaTrainingValidation', null, null, null, '正在校验训练图片和相机数据…'],
  ['trainingSplats', 'trainingPreparation', 12, 251, 'images', '正在准备训练图片 12/251'],
  ['trainingSplats', 'trainingValidation', null, null, null, '正在校验训练图片和相机数据'],
  ['trainingSplats', 'trainingCheckpoint', null, null, null, '正在保存生成检查点'],
  ['extractingFrames', 'videoTimestamps', null, null, null, '正在读取视频帧时间信息…'],
] as const)('uses the requested %s/%s caption without changing counter semantics', async (stage, substage, current, total, unit, expected) => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'zh-CN');
  const container = document.createElement('div'), root = createRoot(container);
  const event = { kind: 'progress', stage, substage, current, total, unit } as PipelineEvent;
  function Caption() { return <p>{useProgressMessage('running', stage, [event], null)}</p>; }
  try {
    await act(async () => root.render(<LanguageProvider><Caption /></LanguageProvider>));
    expect(container.textContent).toBe(expected);
    expect(container.textContent).not.toContain('非完成进度');
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});

it('keeps ordinary preparation in the Splat node and never borrows stale Brush iterations', async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'), root = createRoot(container);
  const oldTraining = { phase: 'training', processId: 999, training: { iteration: 8000, total: 8000 } } as import('../types/pipeline').RuntimeSnapshot;
  function Caption({ substage }: { substage: string }) {
    const message = useProgressMessage('running', 'trainingSplats', [], oldTraining, { substage, current: 12, total: 251, unit: 'images' });
    return <TaskProgress status="running" stage="trainingSplats" substage={substage} progress={60} message={message} runtime={oldTraining} remainingSeconds={50} logs={[]} />;
  }
  try {
    await act(async () => root.render(<LanguageProvider><Caption substage="trainingPreparation" /></LanguageProvider>));
    expect(container.querySelector('.current-message')?.textContent).toBe('Preparing training images 12/251');
    expect(container.querySelector('.stage-timeline li.active b')?.textContent).toBe('Splat training');
    expect(container.querySelector('.training-remaining')).toBeNull();
    expect(container.querySelector('.runtime-step-heading')).toBeNull();
    expect(container.querySelector('.runtime-panel')?.textContent).not.toMatch(/8,000|999/);
    const panel = container.querySelector('.runtime-panel');
    for (const substage of ['trainingValidation', 'trainingCheckpoint']) {
      await act(async () => root.render(<LanguageProvider><Caption substage={substage} /></LanguageProvider>));
      expect(container.querySelector('.runtime-panel')).toBe(panel);
      expect(container.querySelector('.runtime-step-heading')).toBeNull();
      expect(container.querySelector('.training-remaining')).toBeNull();
    }
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});

it('does not reuse face counts or cleared counters when displaying the common training caption', async () => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  localStorage.setItem('ooo-splat-language', 'en');
  const container = document.createElement('div'), root = createRoot(container);
  const event = { kind: 'progress', stage: 'trainingSplats', substage: 'panoramaTrainingPreparation', current: 72, total: 228, unit: 'panoramaFaces' } as PipelineEvent;
  function Caption() { return <p>{useProgressMessage('running', 'trainingSplats', [event], null, { current: null, total: null, substage: 'training', unit: 'steps' })}</p>; }
  try {
    await act(async () => root.render(<LanguageProvider><Caption /></LanguageProvider>));
    expect(container.textContent).toContain('Generating Gaussian splats');
    expect(container.textContent).toContain('Please wait');
    expect(container.textContent).not.toMatch(/72|228/);
  } finally { await act(async () => root.unmount()); localStorage.clear(); }
});
