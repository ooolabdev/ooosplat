import { useLayoutEffect, useMemo, useRef, useState, type RefObject, type UIEventHandler } from 'react';
import { LoaderCircle, Square } from 'lucide-react';
import { useI18n } from '../i18n';
import { taskStageLabels, taskStagePosition, taskStages } from '../app/taskPresentation';
import { formatClockDuration } from '../lib/runtimeEstimate';
import type { RuntimeSnapshot } from '../types/pipeline';
import { RuntimePanel } from './RuntimePanel';
import { isTrainingPreparation, nextOverallPercent } from '../lib/progressState';

export interface TaskLogLine { key: string; text: string; source: string | null; timestamp?: string; level?: string }
interface Props {
  importedReconstruction?: boolean;
  status: string; stage: string | null; progress: number | null; stageProgress?: number | null;
  substage?: string | null;
  progressUnit?: string | null;
  operationAttempt?: number;
  progressKey?: string; message?: string | null; elapsedMs?: number | null;
  remainingSeconds?: number | null; runtime?: RuntimeSnapshot | null; showRuntimePanel?: boolean;
  logs: TaskLogLine[]; historical?: boolean; cancelling?: boolean; onCancel?: () => void;
  logRef?: RefObject<HTMLDivElement | null>; onLogScroll?: UIEventHandler<HTMLDivElement>;
  logsTruncated?: boolean;
  rawLogs?: TaskLogLine[];
  logsUnavailable?: boolean;
}

function ProgressPercentage({ progress, completed }: { progress: number | null; completed: boolean }) {
  const [previous, setPrevious] = useState(0);
  const value = nextOverallPercent(previous, progress, completed);
  // Adjust this small keyed leaf during render: no stale frame or monitor remount.
  if (value !== previous) setPrevious(value);
  return <span className="mono">{value.toFixed(1)}%</span>;
}

/** Original progress/timeline/log layout with task-local data and optional raw log columns. */
export function TaskProgress({ importedReconstruction,status, stage, progress, stageProgress, substage, progressUnit, operationAttempt, progressKey, message, elapsedMs, remainingSeconds, runtime, showRuntimePanel = true, logs, historical, cancelling, onCancel, logRef, onLogScroll, rawLogs, logsUnavailable }: Props) {
  const { locale, t, formatDuration } = useI18n();
  const active = ['starting', 'running', 'cancelling'].includes(status);
  const index = taskStagePosition(stage);
  const knownStage = stage != null;
  const preparing = stage === 'trainingSplats' && isTrainingPreparation(substage);
  // A cleared operation must not leak an old Brush monitor through a delayed
  // snapshot. Fresh preparation-process snapshots have no training fields.
  const currentRuntime = preparing && runtime?.training ? null : runtime;
  const stageLabel = stage && Object.hasOwn(taskStageLabels, stage) ? taskStageLabels[stage] : taskStages[index][1];
  const internalRef = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  useLayoutEffect(() => {
    if (!logRef && internalRef.current && follow.current) internalRef.current.scrollTop = internalRef.current.scrollHeight;
  }, [logs, logRef]);
  const logRows = useMemo(() => logs.map(line => <div className={`log-line ${line.level ?? 'historical'}`} key={line.key}><time>{line.timestamp ? new Date(line.timestamp).toLocaleTimeString(locale, { hour12: false }) : ''}</time><span>{line.source ?? ''}</span><p>{line.text}</p></div>), [logs, locale]);
  const rawGroups = useMemo(() => {
    const sources = new Map<string, TaskLogLine[]>();
    for (const line of rawLogs ?? []) {
      const source = line.source ?? 'system';
      const group = sources.get(source) ?? [];
      group.push(line);
      sources.set(source, group);
    }
    return [...sources].map(([source, lines]) => <details className="legacy-source-log" key={source}>
      <summary>{source}</summary>
      {lines.map(line => <div className="log-line historical" key={line.key}><time /><span>{source}</span><p>{line.text}</p></div>)}
    </details>);
  }, [rawLogs]);
  return <section className={historical ? 'live-process historical' : 'live-process'}>
    <div className="live-heading"><div>{active && <span className="live-dot" />}<strong>{t('progress.title')}</strong></div><ProgressPercentage key={progressKey ?? 'local'} progress={progress} completed={status === 'completed'} /></div>
    {message && <p className="current-message">{message}</p>}
    {operationAttempt != null && operationAttempt > 1 && <p className="runtime-note">{t('progress.operationAttempt', { count: operationAttempt })}</p>}
    {!preparing && runtime?.training && remainingSeconds != null && <p className="training-remaining">{t('runtime.remaining')} {formatClockDuration(remainingSeconds)}</p>}
    {!historical && <div className="process-metrics"><span><small>{t('progress.stage')}</small><b>{stage ? t(stageLabel) : '—'}</b></span><span><small>{t('progress.elapsed')}</small><b>{elapsedMs == null ? '—' : formatDuration(elapsedMs)}</b></span></div>}
    <ol className="stage-timeline">{taskStages.map(([key, label], position) => {
      if(importedReconstruction&&position>=2&&position<=4)return position===2?<li key="imported" className="imported-reconstruction" title={t('inputs.skip')}><span/><b>{t('inputs.imported')}</b></li>:null;
      const terminal = knownStage && position === index && ['failed', 'cancelled', 'interrupted'].includes(status) ? status : '';
      const className = (knownStage && position < index) || status === 'completed' ? 'done' : terminal || (knownStage && position === index && active ? 'active' : '');
      const percentage = stageProgress == null ? null : Math.min(100, Math.max(0, stageProgress)).toFixed(1);
      const mapperEstimate = stage === 'reconstructing' || (stage === 'validatingReconstruction' && progressUnit === 'registeredImages');
      const estimate = mapperEstimate && !substage?.startsWith('panorama') && percentage != null
        ? t('progress.estimated', { value: percentage }) : undefined;
      return <li key={key} className={className}><span /><b>{t(label)}</b>{position === index && active && <small title={estimate} aria-label={estimate}>{percentage == null ? t('progress.processing') : `${percentage}%`}</small>}</li>;
    })}</ol>
    {showRuntimePanel && status !== 'completed' && (active || currentRuntime) && <RuntimePanel snapshot={currentRuntime ?? null} running={active} waitingMessage={message} />}
    <div className="log-toolbar"><span>{t('progress.log')}</span><small>{t('progress.logCount', { count: logs.length + (rawLogs?.length ?? 0) })}</small></div>
    {logsUnavailable && <p className="runtime-note" role="status">{t('progress.logsRetrying')}</p>}
    <div className="live-log" aria-live="polite" role="log" ref={logRef ?? internalRef} onScroll={onLogScroll ?? (() => { const node = internalRef.current; if (node) follow.current = node.scrollHeight - node.scrollTop - node.clientHeight < 24; })}>
      {logRows}
      {rawGroups}
    </div>
    {active && onCancel && <button className="cancel-action" type="button" disabled={cancelling || status === 'cancelling'} onClick={onCancel}>{cancelling || status === 'cancelling' ? <LoaderCircle className="spin" size={13} /> : <Square size={12} fill="currentColor" />}{t(cancelling || status === 'cancelling' ? 'progress.terminating' : 'progress.cancel')}</button>}
  </section>;
}
