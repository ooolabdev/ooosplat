import { useI18n, type TranslationKey } from '../i18n';
import type { PipelineEvent, RuntimeSnapshot } from '../types/pipeline';

const labels: Record<string, TranslationKey> = {
  created: 'progress.preparing', probingVideo: 'generate.analyzing', planningFrames: 'progress.activePlanning',
  extractingFrames: 'progress.activeFrames', validatingReconstruction: 'progress.activeValidation',
  exporting: 'progress.activeExport',
  extractingFeatures: 'progress.activeFeatures', matching: 'progress.activeMatching',
  reconstructing: 'progress.activeReconstruction', trainingSplats: 'progress.activeTraining',
};
const terminalLabels: Record<string, TranslationKey> = {
  completed: 'stage.completed', failed: 'stage.failed', cancelled: 'stage.cancelled', interrupted: 'status.interrupted',
};

function validCount(current: number | null | undefined, total: number | null | undefined) {
  return current != null && total != null && Number.isFinite(current) && Number.isFinite(total) && total > 0
    ? { current: Math.min(total, Math.max(0, current)), total } : null;
}

/** Progress captions use only approved templates and observed counts, never log/error text. */
export function useProgressMessage(status: string, stage: string | null, events: PipelineEvent[], runtime: RuntimeSnapshot | null | undefined, counters?: { current: number | null; total: number | null }) {
  const { t, formatNumber } = useI18n();
  if (Object.hasOwn(terminalLabels, status)) return t(terminalLabels[status]);
  if (status === 'cancelling') return t('progress.terminating');
  if (status !== 'running') return t(status === 'analyzing' ? 'generate.analyzing' : 'progress.preparing');
  const latestStage = events.at(-1)?.stage;
  const terminalStage = latestStage && Object.hasOwn(terminalLabels, latestStage) ? latestStage : stage;
  if (terminalStage && Object.hasOwn(terminalLabels, terminalStage)) return t(terminalLabels[terminalStage]);
  const key = stage && Object.hasOwn(labels, stage) ? labels[stage] : null;
  if (!key) return t(stage ? 'progress.running' : 'progress.preparing');
  let count = stage === 'trainingSplats' ? validCount(runtime?.training?.iteration, runtime?.training?.total) : null;
  count ??= validCount(counters?.current, counters?.total);
  for (let index = events.length - 1; count == null && index >= 0; index--) {
    const event = events[index];
    if (event.stage === stage) count = validCount(event.current, event.total);
  }
  const message = count ? t('progress.activeCount', { label: t(key), current: formatNumber(count.current), total: formatNumber(count.total) }) : `${t(key)}…`;
  return ['reconstructing', 'trainingSplats'].includes(stage ?? '') ? t('progress.activeLongWait', { label: message }) : message;
}
