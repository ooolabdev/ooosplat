import { localizePipelineMessage, useI18n, type TranslationKey } from '../i18n';
import type { PipelineEvent, RuntimeSnapshot } from '../types/pipeline';

const labels: Record<string, TranslationKey> = {
  extractingFeatures: 'progress.activeFeatures', matching: 'progress.activeMatching',
  reconstructing: 'progress.activeReconstruction', trainingSplats: 'progress.activeTraining',
};

/** Preserve the manual workflow's friendly counts for both GUI and Agent executions. */
export function useProgressMessage(status: string, stage: string | null, events: PipelineEvent[], runtime: RuntimeSnapshot | null | undefined, fallback?: string | null, counters?: { current: number | null; total: number | null }) {
  const { locale, t, formatNumber } = useI18n();
  const latest = events.at(-1);
  const key = status === 'running' && !['failed', 'cancelled', 'completed'].includes(latest?.stage ?? '') && stage ? labels[stage] : null;
  if (!key) return localizePipelineMessage(locale, fallback ?? latest?.message ?? t('progress.preparing'));
  const event = [...events].reverse().find(event => event.stage === stage && event.current != null && event.total != null && event.total > 0);
  const count = stage === 'trainingSplats' && runtime?.training?.iteration != null && runtime.training.total != null
    ? { current: runtime.training.iteration, total: runtime.training.total }
    : counters?.current != null && counters.total != null && counters.total > 0 ? { current: Math.min(counters.total, Math.max(0, counters.current)), total: counters.total }
    : event ? { current: Math.min(event.total!, Math.max(0, event.current!)), total: event.total! } : null;
  const message = count ? t('progress.activeCount', { label: t(key), current: formatNumber(count.current), total: formatNumber(count.total) }) : `${t(key)}…`;
  return ['reconstructing', 'trainingSplats'].includes(stage ?? '') ? t('progress.activeLongWait', { label: message }) : message;
}
