import type { TaskDraft } from './taskWorkspace';
import type { ImageMatchingStrategy, VideoProjection } from '../types/pipeline';
import {sourcePath,type GenerationInputs} from '../types/inputs';

export type PreflightKind = 'smallImages' | 'reshootCondition' | 'reshootCamera';
export interface PreflightQuestion { id: string; kind: PreflightKind; draft: TaskDraft; snapshot: string; sourceId?:string }
export type PreflightAnswers = Record<string, string>;
export const projectionIdentity = (draft: TaskDraft) => draft.projectionDetection?.sourceIdentity || draft.inputPath || '';
export const preflightSnapshot = (draft: TaskDraft) => JSON.stringify([draft.id,draft.inputs,draft.frameSamplingFps, draft.inputPath, draft.inputType, draft.quality, draft.autoRetryEnabled, draft.frameSamplingRatio, projectionIdentity(draft), draft.imageSequence?.imageCount, draft.projectionDetection?.confirmedErp, draft.projectionDetection?.supportedErp, resolvedProjection(draft), draft.reshootSource?.sourceIdentity, draft.reshootPlan?.sourceIdentity, draft.reshootPlan?.imageCount, draft.reshootSource?.warmAvailable]);
export function resolvedProjection(draft: TaskDraft): VideoProjection {
  const detection = draft.projectionDetection;
  if (detection?.confirmedErp) return 'equirectangular';
  const answer = draft.projectionAnswer;
  if (answer?.identity === projectionIdentity(draft) && (answer.projection !== 'equirectangular' || detection?.supportedErp)) return answer.projection;
  return 'auto';
}
// Ordered registry: add another check without coupling rendering to task startup.
const checks: Array<{ kind: PreflightKind; applies: (draft: TaskDraft) => boolean }> = [
  { kind: 'smallImages', applies: d => d.inputType === 'images' && !!d.imageSequence && d.imageSequence.imageCount < 30 },
];
export function collectPreflightQuestions(drafts: TaskDraft[]): PreflightQuestion[] {
  return drafts.filter(d => !d.running && !!d.inputPath).flatMap(draft => {
    if(draft.inputs){
      if(draft.inputs.mode==='colmap')return [];
      const snapshot=preflightSnapshot(draft);const questions:PreflightQuestion[]=draft.kind==='reshoot'?[{id:`${draft.id}:reshootCondition`,kind:'reshootCondition',draft,snapshot}]:[];
      for(const item of draft.inputs.items){const a=draft.inputAnalyses?.[item.id];const virtual={...draft,inputPath:sourcePath(item.source),inputType:item.source.kind==='video'?'video' as const:'images' as const,imageSequence:a?.imageSequence??null,reshootPlan:null};
        if(draft.kind==='reshoot'&&item.projection==='perspective')questions.push({id:`${draft.id}:${item.id}:reshootCamera`,kind:'reshootCamera',draft:virtual,snapshot,sourceId:item.id});
        const count=a?.imageSequence?.imageCount;
        if(count!=null&&count<30&&draft.kind!=='reshoot')questions.push({id:`${draft.id}:${item.id}:smallImages`,kind:'smallImages',draft:virtual,snapshot,sourceId:item.id});
      }return questions;
    }
    return draft.kind === 'reshoot' ? [
    { id: `${draft.id}:reshootCondition`, kind: 'reshootCondition' as const, draft, snapshot: preflightSnapshot(draft) },
    ...(draft.reshootSource?.projection === 'equirectangular' ? [] : [{ id: `${draft.id}:reshootCamera`, kind: 'reshootCamera' as const, draft, snapshot: preflightSnapshot(draft) }]),
  ] : checks
    .filter(check => check.applies(draft))
    .map(check => ({ id: `${draft.id}:${check.kind}`, kind: check.kind, draft, snapshot: preflightSnapshot(draft) }));});
}
export function confirmedInputs(draft:TaskDraft,answers:PreflightAnswers):GenerationInputs{
  if(!draft.inputs)throw new Error('Missing inputs');if(draft.inputs.mode==='colmap')return draft.inputs;
  return {mode:'materials',items:draft.inputs.items.map(item=>({...item,cameraRelation:answers[`${draft.id}:${item.id}:reshootCamera`] as typeof item.cameraRelation,confirmedSmallImages:draft.kind==='reshoot'||answers[`${draft.id}:${item.id}:smallImages`]==='accepted'}))};
}
export function answerIsValid(question: PreflightQuestion, answer?: string) {
  if (question.kind === 'reshootCondition') return ['good','appearance','geometry','unsure'].includes(answer ?? '');
  if (question.kind === 'reshootCamera') return ['same','different','unsure'].includes(answer ?? '');
  return answer === 'accepted';
}
export function imageMatchingStrategy(_draft: TaskDraft, _answers: PreflightAnswers): ImageMatchingStrategy {
  return 'exhaustive';
}
