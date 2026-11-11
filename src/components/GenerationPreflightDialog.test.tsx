// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
import { createGenerationDraft, createReshootDraft } from '../app/taskWorkspace';
import { collectPreflightQuestions } from '../app/generationPreflight';
import { GenerationPreflightDialog } from './GenerationPreflightDialog';
let root: Root, container: HTMLDivElement;
const finish = vi.fn();
const draft = { ...createGenerationDraft(1), inputType: 'images' as const, inputPath: 'E:/small-images', imageSequence: { imageCount: 20, width: 100, height: 100, hasAlpha: false, requiresLargeSequenceConfirmation: false } };
const button = () => container.querySelector<HTMLButtonElement>('.primary-action')!;
beforeEach(() => { Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true }); localStorage.setItem('ooo-splat-language', 'zh-CN'); finish.mockReset(); container = document.createElement('div'); document.body.append(container); root = createRoot(container); });
afterEach(async () => { await act(async () => root.unmount()); container.remove(); });
async function render() { await act(async () => root.render(<LanguageProvider><GenerationPreflightDialog session={{ questions: collectPreflightQuestions([draft]), finalLabel: '开始生成', resolve: finish }} onFinish={finish} /></LanguageProvider>)); }
it('submits the remaining small-image warning once', async () => {
  await render(); expect(button().disabled).toBe(true);
  expect(container.querySelector('img')).toBeNull();
  await act(async () => container.querySelector<HTMLInputElement>('input')!.click());
  await act(async () => { button().click(); button().click(); });
  expect(finish).toHaveBeenCalledTimes(1);
  expect(finish.mock.calls[0][0]).toEqual({ [`${draft.id}:smallImages`]: 'accepted' });
});
it('Esc cancels without submitting any partial answer and traps keyboard focus', async () => {
  await render();
  const dialog = container.querySelector<HTMLElement>('[role="dialog"]')!;
  await act(async () => dialog.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true })));
  expect(document.activeElement).toBe(container.querySelector('footer > button'));
  await act(async () => document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true })));
  expect(document.activeElement).toBe(container.querySelector('header button'));
  await act(async () => container.querySelector<HTMLInputElement>('input')!.click());
  const cancel = container.querySelector<HTMLButtonElement>('footer > button')!;
  cancel.focus();
  await act(async () => cancel.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
  expect(finish).toHaveBeenCalledWith(null);
});
it('does not include the image-order question in the generation dialog', async () => {
  localStorage.setItem('ooo-splat-language', 'en');
  await render();
  expect(container.querySelector('h2')?.textContent).toBe('Not enough images');
  expect(container.textContent).not.toContain('Confirm image order');
});

it.each(['good','appearance','geometry','unsure'])('reshoot explains each approach and asks camera settings before starting (%s)', async condition => {
  const reshoot = {...createReshootDraft(1,'source','Original','balanced'), inputPath:'E:/new-images', reshootSource:{ projectId:'source',projectName:'Original',quality:'balanced' as const,plannerEnabled:true,cameraId:1,cameraModel:'SIMPLE_RADIAL',width:100,height:100,sourceImageCount:20,eligible:true,reason:null,imported:false,warmAvailable:true }};
  await act(async () => root.render(<LanguageProvider><GenerationPreflightDialog session={{ questions:collectPreflightQuestions([reshoot]),finalLabel:'开始生成',resolve:finish }} onFinish={finish} /></LanguageProvider>));
  expect(container.querySelector('h2')?.textContent).toBe('原模型目前是什么情况？');
  expect(container.querySelectorAll('input[type="radio"]')).toHaveLength(4);
  expect(container.querySelector('input:checked')).toBeNull();
  expect(button().disabled).toBe(true);
  expect(button().textContent).toBe('下一步');
  expect(container.querySelectorAll('.preflight-condition small')).toHaveLength(4);
  expect(container.querySelector('.preflight-choices')?.textContent).not.toMatch(/增量|重建|微调|COLMAP|Brush|SfM/);
  expect(container.querySelector('input[type="checkbox"]')).toBeNull();
  await act(async () => container.querySelector<HTMLInputElement>(`input[value="${condition}"]`)!.click());
  await act(async () => button().click());
  expect(container.querySelector('h2')?.textContent).toBe('补拍素材与原素材的拍摄设置相同吗？');
  expect(container.querySelector('input:checked')).toBeNull();
  expect(button().disabled).toBe(true);
  expect(finish).not.toHaveBeenCalled();
  await act(async () => container.querySelector<HTMLInputElement>('input[value="same"]')!.click());
  await act(async () => { button().click(); button().click(); });
  expect(finish).toHaveBeenCalledTimes(1);
  expect(finish.mock.calls[0][0]).toEqual({[`${reshoot.id}:reshootCondition`]:condition,[`${reshoot.id}:reshootCamera`]:'same'});
});

it('reshoot cancellation submits no partial answer and missing models are explained without promising reuse', async () => {
  const reshoot = {...createReshootDraft(1,'source','Original','high'), inputPath:'E:/new.mp4'};
  await act(async () => root.render(<LanguageProvider><GenerationPreflightDialog session={{questions:collectPreflightQuestions([reshoot]),finalLabel:'开始生成',resolve:finish}} onFinish={finish} /></LanguageProvider>));
  await act(async () => container.querySelector<HTMLInputElement>('input[value="good"]')!.click());
  expect(container.textContent).toContain('原模型不可用时重新生成');
  await act(async () => container.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true})));
  expect(finish).toHaveBeenCalledWith(null);
});

it('reshoot keeps image order outside the modal and preserves Back answers', async () => {
  const reshoot = {...createReshootDraft(1,'source','Original','balanced'), inputType:'images' as const, inputPath:'E:/new-images',imageSequence:{...draft.imageSequence,imageCount:101}};
  await act(async () => root.render(<LanguageProvider><GenerationPreflightDialog session={{questions:collectPreflightQuestions([reshoot]),finalLabel:'开始生成',resolve:finish}} onFinish={finish} /></LanguageProvider>));
  await act(async () => container.querySelector<HTMLInputElement>('input[value="appearance"]')!.click());
  await act(async () => button().click());
  expect(container.querySelector('h2')?.textContent).toBe('补拍素材与原素材的拍摄设置相同吗？');
  await act(async () => container.querySelector<HTMLInputElement>('input[value="different"]')!.click());
  await act(async () => container.querySelector<HTMLButtonElement>('footer > div > .secondary-action')!.click());
  expect(container.querySelector<HTMLInputElement>('input[value="appearance"]')!.checked).toBe(true);
  await act(async () => button().click());
  expect(container.querySelector<HTMLInputElement>('input[value="different"]')!.checked).toBe(true);
  await act(async () => button().click());
  expect(finish).toHaveBeenCalledWith({[`${reshoot.id}:reshootCondition`]:'appearance',[`${reshoot.id}:reshootCamera`]:'different'});
});

it('reshoot descriptions use plain English without a second plan confirmation', async () => {
  localStorage.setItem('ooo-splat-language', 'en');
  const reshoot = {...createReshootDraft(1,'source','Original','balanced'), inputPath:'E:/new.mov'};
  await act(async () => root.render(<LanguageProvider><GenerationPreflightDialog session={{questions:collectPreflightQuestions([reshoot]),finalLabel:'Start generation',resolve:finish}} onFinish={finish} /></LanguageProvider>));
  expect(container.querySelectorAll('.preflight-condition small')).toHaveLength(4);
  expect(container.querySelector('.preflight-choices')?.textContent).toContain('camera positions');
  expect(container.querySelector('.preflight-choices')?.textContent).not.toMatch(/incremental|reconstruction|fine-tun|COLMAP|Brush|SfM/i);
  expect(button().textContent).toBe('Next');
  await act(async () => container.querySelector<HTMLInputElement>('input[value="good"]')!.click());
  await act(async () => button().click());
  expect(container.querySelector('#preflight-description')?.textContent).toContain('stabilization');
  expect(container.querySelectorAll('input[type="radio"]')).toHaveLength(3);
  expect(container.querySelector('input:checked')).toBeNull();
  expect(button().textContent).toBe('Start generation');
});
