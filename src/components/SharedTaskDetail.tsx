import { useEffect, useState } from 'react';
import { CircleAlert, LoaderCircle, Play } from 'lucide-react';
import { cancelSharedTask, startGuiTask, readSharedTaskLogs } from '../lib/backend';
import { pipelineErrorMessage } from '../lib/pipelineError';
import { liveTrainingRemainingSeconds } from '../lib/runtimeEstimate';
import { taskIsActive, type SharedTask } from '../types/tasks';
import type { ColmapAccelerationStatus, ProjectSummary } from '../types/pipeline';
import { localizePipelineMessage, useI18n } from '../i18n';
import { taskProjectSummary, taskStatusLabel } from '../app/taskPresentation';
import { useTaskProjectDetail } from '../app/useTaskProjectDetail';
import { useProgressMessage } from '../app/useProgressMessage';
import { useTaskElapsed } from '../app/useTaskElapsed';
import { CompactError } from './CompactError';
import { TaskConfiguration } from './TaskConfiguration';
import { TaskProgress, type TaskLogLine } from './TaskProgress';
import { ProjectResultStats } from './ProjectResultStats';

interface Props {
  task: SharedTask;
  project?: ProjectSummary | null;
  acceleration?: ColmapAccelerationStatus | null;
  onResume?: (project: ProjectSummary) => Promise<unknown> | void;
  showRuntimePanel?: boolean;
  canStart?: boolean;
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

export function SharedTaskDetail({ task, project: metadata, acceleration = null, onResume, showRuntimePanel = true, canStart = true, externalError }: Props) {
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
  const lastEvent = events.at(-1);
  const runtime = task.runtime ?? [...events].reverse().find(event => event.runtime)?.runtime;
  const stage = task.error?.failed_stage ?? (['failed', 'cancelled', 'interrupted'].includes(task.status)
    ? [...events].reverse().find(event => !['failed', 'cancelled', 'completed'].includes(event.stage))?.stage ?? detail?.stage
    : task.stage) ?? null;
  const rawKeys = new Set(fileLogs.map(line => `${line.source}:${line.text.trim()}`));
  const eventLogs = events.filter(event => !['heartbeat', 'runtime'].includes(event.kind) && !rawKeys.has(`${event.engine}:${event.message.trim()}`)).map(event => ({
    key: `event:${event.sequence}`, source: event.engine, timestamp: event.timestamp, level: event.level,
    text: event.kind === 'log' ? event.message : localizePipelineMessage(locale, event.message),
  }));
  const logs = boundLogLines([...fileLogs, ...eventLogs]);
  const resumable = project && detail && !active && ['failed', 'cancelled', 'interrupted'].includes(task.status);
  const error = actionError ?? externalError ?? task.error?.message;
  const message = useProgressMessage(task.status, stage, events, runtime, error ?? lastEvent?.message ?? taskStatusLabel(locale, task.status), { current: task.current, total: task.total });
  return <section className="project-detail-page shared-task-detail" aria-label={locale === 'zh-CN' ? '任务详情' : 'Task details'}>
    {task.source === 'mcp' && <p className="task-origin">{locale === 'zh-CN' ? '由 AI Agent 创建' : 'Created by AI Agent'}</p>}
    {task.status === 'completed' && project && <ProjectResultStats project={project} detail={detail} inputImages={task.result?.inputImages} registeredImages={task.result?.registeredImages} />}
    <TaskConfiguration sourcePath={task.input_path} projectsRoot={task.projects_root} inputType={task.input_type ?? detail?.inputType ?? (/\.(mp4|mov)$/i.test(task.input_path) ? 'video' : 'images')} quality={task.quality} plannerEnabled={task.planner_enabled} completed={task.status === 'completed'} inheritedFrom={task.task_kind === 'reshoot' ? 'reshoot' : task.runs?.find(run => run.run_id === task.run_id)?.kind === 'resume' ? 'resume' : null} acceleration={[...events].reverse().find(event => event.acceleration)?.acceleration ?? acceleration} video={detail?.video} imageSequence={detail?.imageSequence} estimatedFrames={detail?.estimatedFrames} />
    {project?.registeredRatio != null && project.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t('result.lowRegistration', { value: (project.registeredRatio * 100).toFixed(1) })}</p>}
    {error && <div className="inline-error detail-error" role="alert"><CircleAlert size={16} /><CompactError message={error} /></div>}
    {task.error?.classification_is_heuristic && <p className="project-failure">{locale === 'zh-CN' ? '错误分类为启发式判断，请结合日志确认。' : 'Error classification is heuristic; inspect logs for evidence.'}</p>}
    {task.status === 'created' && <button className="primary-action" type="button" disabled={busy || !canStart} onClick={() => void action(() => startGuiTask(task.task_id))}>{busy ? <LoaderCircle className="spin" size={16} /> : <Play size={16} fill="currentColor" />}{t('generate.start')}</button>}
    {resumable && onResume && <button className="primary-action" type="button" disabled={busy || !canStart} onClick={() => void action(() => onResume(project))}><Play size={16} fill="currentColor" />{t('project.continue')}</button>}
    <TaskProgress status={task.status} stage={stage} progress={task.estimated_progress} estimated stageProgress={task.progress} elapsedMs={elapsedMs} message={message} runtime={runtime} showRuntimePanel={showRuntimePanel} remainingSeconds={active ? liveTrainingRemainingSeconds(runtime ?? null, true, Date.now()) : null} logs={logs} cancelling={busy || task.status === 'cancelling'} onCancel={task.run_id ? () => void action(() => cancelSharedTask(task.task_id, task.run_id!)) : undefined} />
  </section>;
}
