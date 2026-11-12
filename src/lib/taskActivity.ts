import type { PipelineEvent } from '../types/pipeline';

export interface ActivityIdentity {
  taskId: string; runId: string | null;
  /** Only a task-bound snapshot may supply legacy events without identifiers. */
  allowUnbound?: boolean;
}
export const ACTIVITY_LIMIT = 500;
export const ACTIVITY_BYTES = 256 * 1024;

export function activityBelongsTo(event: PipelineEvent, identity: ActivityIdentity): boolean {
  if (!identity.allowUnbound && (!event.taskId || (identity.runId != null && !event.runId))) return false;
  return (!event.taskId || event.taskId === identity.taskId)
    && (!event.runId || event.runId === identity.runId);
}

/** History has its own identity, independent of the live-state watermark.
 * Never deduplicate text: two executions can legitimately print the same line.
 */
export function mergeActivityEvents(previous: PipelineEvent[], incoming: PipelineEvent[], identity?: ActivityIdentity): PipelineEvent[] {
  const bySequence = new Map<number, PipelineEvent>();
  for (const event of [...previous, ...incoming]) {
    if (event.kind === 'runtime' || event.kind === 'heartbeat'
      || !Number.isSafeInteger(event.sequence) || event.sequence < 0
      || (identity && !activityBelongsTo(event, identity))) continue;
    if (!bySequence.has(event.sequence)) bySequence.set(event.sequence, event);
  }
  const ordered = [...bySequence.values()].sort((a, b) => a.sequence - b.sequence);
  let bytes = 0;
  let start = ordered.length;
  while (start > 0 && ordered.length - start < ACTIVITY_LIMIT) {
    const size = ordered[start - 1].message.length;
    if (bytes + size > ACTIVITY_BYTES) break;
    bytes += size;
    start--;
  }
  const result = ordered.slice(start);
  return result.length === previous.length && result.every((event, index) => event === previous[index]) ? previous : result;
}
