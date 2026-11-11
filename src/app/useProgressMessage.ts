import { useI18n, type TranslationKey } from '../i18n';
import type { PipelineEvent, RuntimeSnapshot } from '../types/pipeline';
import { isProgressEvent } from '../lib/progressState';

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
    && current >= 0 && current <= total ? { current, total } : null;
}

/** Progress captions use only approved templates and observed counts, never log/error text. */
export function useProgressMessage(status: string, stage: string | null, events: PipelineEvent[], runtime: RuntimeSnapshot | null | undefined, counters?: { current: number | null; total: number | null; substage?: string | null; unit?: string | null }) {
  const { t, formatNumber } = useI18n();
  if (Object.hasOwn(terminalLabels, status)) return t(terminalLabels[status]);
  if (status === 'cancelling') return t('progress.terminating');
  if (status !== 'running') return t(status === 'analyzing' ? 'generate.analyzing' : 'progress.preparing');
  const latestStage = events.at(-1)?.stage;
  const terminalStage = latestStage && Object.hasOwn(terminalLabels, latestStage) ? latestStage : stage;
  if (terminalStage && Object.hasOwn(terminalLabels, terminalStage)) return t(terminalLabels[terminalStage]);
  const latestPhaseEvent = [...events].reverse().find(event => event.stage === stage && isProgressEvent(event));
  const source = counters ?? latestPhaseEvent;
  const substage = source?.substage;
  const count = validCount(source?.current, source?.total);
  if (substage === 'panoramaTimestamps' || substage === 'videoTimestamps') return t('progress.panoramaTimestamps');
  if (substage === 'panoramaImagesPreparation' && count) return t('progress.panoramaImagesPreparing', { current: formatNumber(count.current), total: formatNumber(count.total) });
  if (substage === 'panoramaImagesReconstruction' && count) return t('progress.panoramaImagesRegistered', { current: formatNumber(count.current), total: formatNumber(count.total) });
  if (stage === 'reconstructing') {
    const label = count
      ? substage === 'panoramaReconstruction'
        ? t('progress.registeredCount', { current: formatNumber(count.current), total: formatNumber(count.total) })
        : t('progress.activeCount', { label: t('progress.activeReconstruction'), current: formatNumber(count.current), total: formatNumber(count.total) })
      : t(substage === 'panoramaReconstruction' ? 'progress.panoramaReconstruction' : 'progress.activeReconstruction');
    return t('progress.activeLongWait', { label });
  }
  if (substage === 'trainingValidation') return t('progress.trainingValidating');
  if (substage === 'trainingCheckpoint') return t('progress.trainingCheckpoint');
  if (substage === 'trainingPreparation') return count
    ? t('progress.trainingPreparingCount', { current: formatNumber(count.current), total: formatNumber(count.total) })
    : t('stage.trainingPreparation');
  if (substage === 'panoramaTrainingValidation') return t('progress.panoramaValidating');
  if (substage === 'panoramaTrainingPreparation') {
    return count ? t(source?.unit === 'panoramaImageFaces' ? 'progress.panoramaImagesFaces' : 'progress.panoramaPreparing', { current: formatNumber(Math.floor(count.current / 6)), total: formatNumber(Math.floor(count.total / 6)), faces: formatNumber(count.current), totalFaces: formatNumber(count.total) }) : t('progress.panoramaChecking');
  }
  if (stage === 'matching') {
    if ((source?.unit === 'matchingImages' || source?.unit === 'matchingIndexImages') && count) return t(source.unit === 'matchingImages' ? 'progress.matchingImages' : 'progress.matchingIndexImages', { current: formatNumber(count.current), total: formatNumber(count.total) });
    if (source?.unit === 'matchingBatches' && count) return t('progress.matchingBatches', { current: formatNumber(count.current), total: formatNumber(count.total) });
    if (source?.unit === 'matchingLoop') return t('progress.matchingLoop');
    if (source?.unit === 'matchingSequential') return t('progress.matchingSequential');
    if (source?.unit === 'matchingLocal') return t('progress.matchingLocal');
    return `${t('progress.activeMatching')}…`;
  }
  const key = substage === 'panoramaExtraction' ? 'progress.panoramaFrames' : stage && Object.hasOwn(labels, stage) ? labels[stage] : null;
  if (!key) return t(stage ? 'progress.running' : 'progress.preparing');
  const steps = stage === 'trainingSplats' ? validCount(runtime?.training?.iteration, runtime?.training?.total)
    ?? (['steps', 'iterations', '步'].includes(source?.unit ?? '') || (!substage && source?.unit == null) ? count : null) : null;
  if (stage === 'trainingSplats' && steps) return t('progress.activeLongWait', { label: t('progress.activeCount', { label: t('progress.activeTraining'), current: formatNumber(steps.current), total: formatNumber(steps.total) }) });
  const message = stage !== 'trainingSplats' && count ? t('progress.activeCount', { label: t(key), current: formatNumber(count.current), total: formatNumber(count.total) }) : `${t(key)}…`;
  return ['reconstructing', 'trainingSplats'].includes(stage ?? '') ? t('progress.activeLongWait', { label: message }) : message;
}
