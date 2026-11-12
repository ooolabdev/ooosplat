import { useState } from 'react';
import { CircleAlert, LoaderCircle, Play } from 'lucide-react';
import { cancelSharedTask, startGuiTask } from '../lib/backend';
import { pipelineErrorMessage, taskFailureContext } from '../lib/pipelineError';
import { liveTrainingRemainingSeconds } from '../lib/runtimeEstimate';
import { taskCanContinue, taskIsActive, type SharedTask } from '../types/tasks';
import type { ColmapAccelerationStatus, ProjectSummary } from '../types/pipeline';
import { useI18n } from '../i18n';
import { taskProjectSummary } from '../app/taskPresentation';
import { useTaskProjectDetail } from '../app/useTaskProjectDetail';
import { useProgressMessage } from '../app/useProgressMessage';
import { isProgressEvent } from '../lib/progressState';
import { useTaskElapsed } from '../app/useTaskElapsed';
import { useTaskActivityLog } from '../app/useTaskActivityLog';
import { CompactError } from './CompactError';
import { TaskConfiguration } from './TaskConfiguration';
import { TaskProgress } from './TaskProgress';
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

export function SharedTaskDetail({ task, project: metadata, acceleration = null, onResume, showRuntimePanel = true, canStart = true, canContinue = canStart, onBeforeContinue, externalError }: Props) {
  const { locale, t } = useI18n();
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const detail = useTaskProjectDetail(task);
  const project = taskProjectSummary(task, metadata ?? detail?.project);
  const active = taskIsActive(task);
  const elapsedMs = useTaskElapsed(task);
  const activity = useTaskActivityLog(task.task_id, task.run_id, active, task.recent_events, locale);
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
    <TaskProgress importedReconstruction={detail?.inputs?.importedReconstruction ?? task.imported_reconstruction} progressKey={`${task.task_id}:${task.run_id ?? 'pending'}`} operationAttempt={task.operation?.attempt} substage={task.substage} progressUnit={task.unit} status={task.status} stage={stage} progress={task.display_progress ?? task.estimated_progress} stageProgress={task.progress} elapsedMs={elapsedMs} message={message} runtime={runtime} showRuntimePanel={showRuntimePanel} remainingSeconds={active ? liveTrainingRemainingSeconds(runtime ?? null, true, Date.now()) : null} logs={activity.logs} rawLogs={activity.rawLogs} logsUnavailable={activity.unavailable} logsTruncated={activity.journalPersisted && ((task.dropped_event_count ?? 0) > 0 || activity.truncated)} cancelling={busy || task.status === 'cancelling'} onCancel={task.run_id ? () => void action(() => cancelSharedTask(task.task_id, task.run_id!)) : undefined} />
  </section>;
}
