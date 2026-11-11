import { useEffect, useMemo, useState } from 'react';
import { CircleAlert, LoaderCircle, Play } from 'lucide-react';
import { cancelSharedTask, startGuiTask, readSharedTaskLogs } from '../lib/backend';
import { pipelineErrorMessage, taskFailureContext } from '../lib/pipelineError';
import { liveTrainingRemainingSeconds } from '../lib/runtimeEstimate';
import { taskCanContinue, taskIsActive, type SharedTask } from '../types/tasks';
import type { ColmapAccelerationStatus, ProjectSummary } from '../types/pipeline';
import { localizePipelineMessage, useI18n } from '../i18n';
import { taskProjectSummary } from '../app/taskPresentation';
import { useTaskProjectDetail } from '../app/useTaskProjectDetail';
import { useProgressMessage } from '../app/useProgressMessage';
import { isProgressEvent } from '../lib/progressState';
import { useTaskElapsed } from '../app/useTaskElapsed';
import { CompactError } from './CompactError';
import { TaskConfiguration } from './TaskConfiguration';
import { TaskProgress, type TaskLogLine } from './TaskProgress';
import { ProjectResultStats } from './ProjectResultStats';
import { InputAdoptionWarning } from './InputAdoptionWarning';

interface Props {
  task: SharedTask;
  project?: ProjectSummary | null;
  acceleration?: ColmapAccelerationStatus | null;
  onResume?: (project: ProjectSummary, expectedRunId?: string) => Promise<unknown> | void;
  showRuntimePanel?: boolean;
  canStart?: boolean;
  canContinue?: boolean;
  onBeforeContinue?: () => void;
  externalError?: string | null;
}

function boundLogLines(lines: TaskLogLine[]) {
  let bytes = 0;
  const result: TaskLogLine[] = [];
  for (let index = lines.length - 1; index >= 0 && result.length < 500; index--) {
    bytes += lines[index].text.length;
    if (bytes > 256 * 1024) break;
    result.push(lines[index]);
  }
  return result.reverse();
}

export function SharedTaskDetail({ task, project: metadata, acceleration = null, onResume, showRuntimePanel = true, canStart = true, canContinue = canStart, onBeforeContinue, externalError }: Props) {
  const { locale, t } = useI18n();
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [fileLogs, setFileLogs] = useState<TaskLogLine[]>([]);
  const detail = useTaskProjectDetail(task);
  const project = taskProjectSummary(task, metadata ?? detail?.project);
  const active = taskIsActive(task);
  const elapsedMs = useTaskElapsed(task);
  useEffect(() => {
    let disposed = false, pending = false, number = 0;
    let cursor: string | undefined;
    setFileLogs([]);
    const read = async () => {
      if (pending) return;
      pending = true;
      try {
        const page = await readSharedTaskLogs(task.task_id, task.run_id, cursor);
        if (disposed) return;
        cursor = page.next_cursor ?? undefined;
        const lines = page.entries.flatMap(entry => entry.text.split(/\r?\n/).filter(Boolean).map(text => ({ key: `file:${number++}`, source: entry.source, text })));
        setFileLogs(current => boundLogLines([...(page.cursor_reset ? [] : current), ...lines]));
      } catch { /* Tasks can fail before any registered engine log exists. */ }
      finally { pending = false; }
    };
    void read();
    const timer = window.setInterval(() => void read(), 15000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [task.task_id, task.run_id, task.status]);
  const action = async (fn: () => Promise<unknown> | void) => {
    setBusy(true); setActionError(null);
    try { await fn(); } catch (error) { setActionError(pipelineErrorMessage(error) ?? t('error.generic')); }
    finally { setBusy(false); }
  };
  const events = task.recent_events;
  const runtime = task.runtime;
  const stage = task.error?.failed_stage ?? (['failed', 'cancelled', 'interrupted'].includes(task.status)
    ? [...events].reverse().find(event => isProgressEvent(event) && !['failed', 'cancelled', 'completed'].includes(event.stage))?.stage ?? detail?.stage
    : task.stage) ?? null;
  const logs = useMemo(() => {
    const rawKeys = new Set(fileLogs.map(line => `${line.source}:${line.text.trim()}`));
    const eventLogs = events.filter(event => !['heartbeat', 'runtime'].includes(event.kind) && !rawKeys.has(`${event.engine}:${event.message.trim()}`)).map(event => ({
      key: `event:${event.sequence}`, source: event.engine, timestamp: event.timestamp, level: event.level,
      text: event.kind === 'log' && event.engine !== 'system' ? event.message : localizePipelineMessage(locale, event.message),
    }));
    return boundLogLines([...fileLogs, ...eventLogs]);
  }, [events, fileLogs, locale]);
  const continuable = taskCanContinue(task);
  const inheritedRun = task.runs?.find(run => run.run_id === task.run_id)?.kind;
  const error = actionError ?? externalError ?? task.error?.message;
  const message = useProgressMessage(task.status, stage, events, runtime, { current: task.current, total: task.total, substage: task.substage, unit: task.unit });
  return <section className="project-detail-page shared-task-detail" aria-label={locale === 'zh-CN' ? '任务详情' : 'Task details'}>
    {task.source === 'mcp' && <p className="task-origin">{locale === 'zh-CN' ? '由 AI Agent 创建' : 'Created by AI Agent'}</p>}
    {task.status === 'completed' && project && <ProjectResultStats project={project} detail={detail} inputImages={task.result?.inputImages} registeredImages={task.result?.registeredImages} />}
    <TaskConfiguration inputs={detail?.inputs} gpsAssistance={task.gps_assistance ?? detail?.inputs?.gpsAssistance} generationPolicy={detail?.generationPolicy ?? task.generation_policy} automaticallyRetried={task.automatically_retried ?? detail?.automaticallyRetried} active={active} reshootRecipe={detail?.reshootRecipe} sourcePath={task.input_path} projectsRoot={task.projects_root} inputType={task.input_type ?? detail?.inputType ?? (/\.(mp4|mov)$/i.test(task.input_path) ? 'video' : 'images')} quality={task.quality} plannerEnabled={task.planner_enabled} completed={task.status === 'completed'} inheritedFrom={task.task_kind === 'reshoot' ? 'reshoot' : inheritedRun && ['continue', 'resume', 'retry'].includes(inheritedRun) ? 'resume' : null} acceleration={[...events].reverse().find(event => event.acceleration)?.acceleration ?? acceleration} videoProjection={detail?.videoProjection} inputProjection={detail?.inputProjection} video={detail?.video} imageSequence={detail?.imageSequence} estimatedFrames={detail?.estimatedFrames} />
    {project?.taskKind !== "import" && project?.registeredRatio != null && project.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t('result.lowRegistration', { value: (project.registeredRatio * 100).toFixed(1) })}</p>}
    <InputAdoptionWarning inputs={detail?.inputs} warning={task.result?.warning} />
    {error && <div className="inline-error detail-error" role="alert"><CircleAlert size={16} /><CompactError message={error} context={!actionError && !externalError ? taskFailureContext(task.error) : undefined} /></div>}
    {task.error?.classification_is_heuristic && <p className="project-failure">{locale === 'zh-CN' ? '错误分类为启发式判断，请结合日志确认。' : 'Error classification is heuristic; inspect logs for evidence.'}</p>}
    {task.status === 'created' && <button className="primary-action" type="button" disabled={busy || !canStart} onClick={() => void action(() => startGuiTask(task.task_id))}>{busy ? <LoaderCircle className="spin" size={16} /> : <Play size={16} fill="currentColor" />}{t('generate.start')}</button>}
    {continuable && !task.project_id && task.run_id && <button className="primary-action" type="button" disabled={busy || !canContinue} onClick={() => { onBeforeContinue?.(); void action(() => startGuiTask(task.task_id, task.run_id!)); }}>{busy ? <LoaderCircle className="spin" size={16} /> : <Play size={16} fill="currentColor" />}{t('project.continue')}</button>}
    {continuable && project && onResume && task.run_id && <button className="primary-action" type="button" disabled={busy || !canContinue} onClick={() => { onBeforeContinue?.(); void action(() => onResume(project, task.run_id!)); }}>{busy ? <LoaderCircle className="spin" size={16} /> : <Play size={16} fill="currentColor" />}{t('project.continue')}</button>}
    <TaskProgress importedReconstruction={detail?.inputs?.importedReconstruction ?? task.imported_reconstruction} progressKey={`${task.task_id}:${task.run_id ?? 'pending'}`} operationAttempt={task.operation?.attempt} substage={task.substage} status={task.status} stage={stage} progress={task.display_progress ?? task.estimated_progress} stageProgress={task.progress} elapsedMs={elapsedMs} message={message} runtime={runtime} showRuntimePanel={showRuntimePanel} remainingSeconds={active ? liveTrainingRemainingSeconds(runtime ?? null, true, Date.now()) : null} logs={logs} logsTruncated={(task.dropped_event_count ?? 0) > 0} cancelling={busy || task.status === 'cancelling'} onCancel={task.run_id ? () => void action(() => cancelSharedTask(task.task_id, task.run_id!)) : undefined} />
  </section>;
}
