import { useLayoutEffect, useMemo, useRef, type RefObject, type UIEventHandler } from 'react';
import { LoaderCircle, Square } from 'lucide-react';
import { useI18n } from '../i18n';
import { taskStageLabels, taskStagePosition, taskStages } from '../app/taskPresentation';
import { formatClockDuration } from '../lib/runtimeEstimate';
import type { RuntimeSnapshot } from '../types/pipeline';
import { RuntimePanel } from './RuntimePanel';

export interface TaskLogLine { key: string; text: string; source: string | null; timestamp?: string; level?: string }
interface Props {
  status: string; stage: string | null; progress: number | null; stageProgress?: number | null;
  estimated?: boolean; message?: string | null; elapsedMs?: number | null;
  remainingSeconds?: number | null; runtime?: RuntimeSnapshot | null; showRuntimePanel?: boolean;
  logs: TaskLogLine[]; historical?: boolean; cancelling?: boolean; onCancel?: () => void;
  logRef?: RefObject<HTMLDivElement | null>; onLogScroll?: UIEventHandler<HTMLDivElement>;
  logsTruncated?: boolean;
}

/** Original progress/timeline/log layout with task-local data and optional raw log columns. */
export function TaskProgress({ status, stage, progress, stageProgress, estimated, message, elapsedMs, remainingSeconds, runtime, showRuntimePanel = true, logs, historical, cancelling, onCancel, logRef, onLogScroll, logsTruncated }: Props) {
  const { locale, t, formatDuration } = useI18n();
  const active = ['starting', 'running', 'cancelling'].includes(status);
  const index = taskStagePosition(stage);
  const knownStage = stage != null;
  const stageLabel = stage && Object.hasOwn(taskStageLabels, stage) ? taskStageLabels[stage] : taskStages[index][1];
  const internalRef = useRef<HTMLDivElement>(null);
  const follow = useRef(true);
  useLayoutEffect(() => {
    if (!logRef && internalRef.current && follow.current) internalRef.current.scrollTop = internalRef.current.scrollHeight;
  }, [logs, logRef]);
  const logRows = useMemo(() => logs.map(line => <div className={`log-line ${line.level ?? 'historical'}`} key={line.key}><time>{line.timestamp ? new Date(line.timestamp).toLocaleTimeString(locale, { hour12: false }) : ''}</time><span>{line.source ?? ''}</span><p>{line.text}</p></div>), [logs, locale]);
  return <section className={historical ? 'live-process historical' : 'live-process'}>
    <div className="live-heading"><div>{active && <span className="live-dot" />}<strong>{t('progress.title')}</strong></div><span className="mono" title={estimated ? (locale === 'zh-CN' ? '估算总进度' : 'Estimated overall progress') : undefined}>{progress == null ? '—' : `${progress.toFixed(1)}%`}</span></div>
    {message && <p className="current-message">{message}</p>}
    {remainingSeconds != null && <p className="training-remaining">{t('runtime.remaining')} {formatClockDuration(remainingSeconds)}</p>}
    {!historical && <div className="process-metrics"><span><small>{t('progress.stage')}</small><b>{stage ? t(stageLabel) : '—'}</b></span><span><small>{t('progress.elapsed')}</small><b>{elapsedMs == null ? '—' : formatDuration(elapsedMs)}</b></span></div>}
    <ol className="stage-timeline">{taskStages.map(([key, label], position) => {
      const terminal = knownStage && position === index && ['failed', 'cancelled', 'interrupted'].includes(status) ? status : '';
      const className = (knownStage && position < index) || status === 'completed' ? 'done' : terminal || (knownStage && position === index && active ? 'active' : '');
      return <li key={key} className={className}><span /><b>{t(label)}</b>{position === index && active && stageProgress != null && <small>{Math.min(100, Math.max(0, stageProgress)).toFixed(1)}%</small>}</li>;
    })}</ol>
    {showRuntimePanel && runtime && <RuntimePanel snapshot={runtime} running={active} />}
    <div className="log-toolbar"><span>{t('progress.log')}</span><small>{t('progress.logCount', { count: logs.length })}</small></div>
    {logsTruncated && <p className="runtime-note">{t('progress.logsTruncated')}</p>}
    <div className="live-log" aria-live="polite" role="log" ref={logRef ?? internalRef} onScroll={onLogScroll ?? (() => { const node = internalRef.current; if (node) follow.current = node.scrollHeight - node.scrollTop - node.clientHeight < 24; })}>
      {logRows}
    </div>
    {active && onCancel && <button className="cancel-action" type="button" disabled={cancelling || status === 'cancelling'} onClick={onCancel}>{cancelling || status === 'cancelling' ? <LoaderCircle className="spin" size={13} /> : <Square size={12} fill="currentColor" />}{t(cancelling || status === 'cancelling' ? 'progress.terminating' : 'progress.cancel')}</button>}
  </section>;
}
