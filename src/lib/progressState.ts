import type { PipelineEvent } from '../types/pipeline';

export const isProgressEvent = (event: PipelineEvent) => !event.kind || event.kind === 'stage' || event.kind === 'progress';

/** Preparing images is part of Splat training, not a Brush iteration phase. */
export function isTrainingPreparation(substage: string | null | undefined) {
  return substage === 'trainingPreparation' || substage === 'trainingValidation' || substage === 'trainingCheckpoint'
    || substage === 'panoramaTrainingPreparation' || substage === 'panoramaTrainingValidation';
}

export function staleOperation(previous: PipelineEvent | null, event: PipelineEvent) {
  if (!previous?.operation || !event.operation) return false;
  return event.operation.id < previous.operation.id
    || (event.kind !== 'stage' && event.operation.id !== previous.operation.id);
}

export function changesOperation(previous: PipelineEvent | null, event: PipelineEvent) {
  if (!previous) return event.kind === 'stage';
  return (event.kind === 'stage' || (!event.operation && isProgressEvent(event)))
    && (previous?.stage !== event.stage || previous?.substage !== event.substage
      || previous?.operation?.id !== event.operation?.id);
}

/** Unknown progress stays unknown; never look behind a new operation for an old percentage. */
export function stagePercent(event: PipelineEvent | null | undefined) {
  return !event || event.indeterminate || event.stageProgress == null || !Number.isFinite(event.stageProgress)
    ? null : Math.min(100, Math.max(0, event.stageProgress));
}

export function overallPercent(progress: number | null, completed: boolean) {
  if (completed) return 100;
  return progress == null || !Number.isFinite(progress) ? null : Math.min(99.9, Math.max(0, progress));
}

/** Display-only retention, scoped by the task/run key at the component boundary. */
export function nextOverallPercent(previous: number, progress: number | null, completed: boolean) {
  if (completed) return 100;
  return Math.min(99.9, Math.max(0, Number.isFinite(previous) ? previous : 0, overallPercent(progress, false) ?? 0));
}
