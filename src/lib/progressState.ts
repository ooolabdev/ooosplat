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
    || (event.operation.id === previous.operation.id
      && (event.operation.sourceId ?? null) !== (previous.operation.sourceId ?? null))
    || (event.kind !== 'stage' && event.operation.id !== previous.operation.id);
}

export function changesOperation(previous: PipelineEvent | null, event: PipelineEvent) {
  if (!previous) return event.kind === 'stage';
  return (event.kind === 'stage' || (!event.operation && isProgressEvent(event)))
    && (previous?.stage !== event.stage || previous?.substage !== event.substage
      || previous?.operation?.id !== event.operation?.id
      || (previous?.operation?.sourceId ?? null) !== (event.operation?.sourceId ?? null));
}

/** Unknown progress stays unknown; never look behind a new operation for an old percentage. */
export function stagePercent(event: PipelineEvent | null | undefined) {
  return !event || event.indeterminate || event.stageProgress == null || !Number.isFinite(event.stageProgress)
    ? null : Math.min(100, Math.max(0, event.stageProgress));
}

interface OperationProgress {
  taskId?: string | null; runId?: string | null;
  stage: string | null; substage?: string | null;
  operation?: PipelineEvent['operation'];
  current?: number | null; total?: number | null; unit?: string | null;
  value: number | null;
}

/** Empty updates are not a new measurement. Retain only facts from the same
 * explicit operation with compatible counters; never carry them across a new
 * process, source, phase or run, and never infer a percentage from image counts.
 */
export function retainOperationProgress(previous: OperationProgress | null | undefined, incoming: OperationProgress): OperationProgress {
  if (!previous?.operation || !incoming.operation
    || previous.operation.id !== incoming.operation.id
    || (previous.operation.sourceId ?? null) !== (incoming.operation.sourceId ?? null)
    || previous.stage !== incoming.stage || (previous.substage ?? null) !== (incoming.substage ?? null)
    || (previous.taskId && incoming.taskId && previous.taskId !== incoming.taskId)
    || (previous.runId && incoming.runId && previous.runId !== incoming.runId)
    || (incoming.unit != null && incoming.unit !== previous.unit)
    || (incoming.current != null && incoming.current !== previous.current)
    || (incoming.total != null && incoming.total !== previous.total)) return incoming;
  return {
    ...incoming,
    current: incoming.current ?? previous.current,
    total: incoming.total ?? previous.total,
    unit: incoming.unit ?? previous.unit,
    value: incoming.value != null && Number.isFinite(incoming.value) ? incoming.value : previous.value,
  };
}

export function retainEventProgress(previous: PipelineEvent | null, incoming: PipelineEvent): PipelineEvent {
  if (!isProgressEvent(incoming)) return incoming;
  const observed = retainOperationProgress(previous && { ...previous, value: stagePercent(previous) },
    { ...incoming, value: stagePercent(incoming) });
  if (observed.value === stagePercent(incoming) && observed.current === incoming.current
    && observed.total === incoming.total && observed.unit === incoming.unit) return incoming;
  return { ...incoming, current: observed.current ?? null, total: observed.total ?? null,
    unit: observed.unit ?? null, stageProgress: observed.value, indeterminate: observed.value == null };
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
