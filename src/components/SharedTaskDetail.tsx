import { useEffect, useState } from 'react';
import { cancelSharedTask, exportPly, startGuiTask, readSharedTaskLogs } from '../lib/backend';
import { taskIsActive, type SharedTask } from '../types/tasks';
import { useI18n } from '../i18n';
import { CompactError } from './CompactError';
import { RuntimePanel } from './RuntimePanel';

export function SharedTaskDetail({ task, onPreview }: { task: SharedTask; onPreview: (projectId: string) => void }) {
  const { locale, t } = useI18n();
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [logs, setLogs] = useState<Array<{ source: string; text: string }>>([]);
  useEffect(() => {
    let disposed = false, pending = false;
    let cursor: string | undefined;
    setLogs([]);
    const read = async () => {
      if (pending) return;
      pending = true;
      try {
        const page = await readSharedTaskLogs(task.task_id, task.run_id, cursor);
        if (disposed) return;
        cursor = page.next_cursor ?? undefined;
        setLogs(current => [...(page.cursor_reset ? [] : current), ...page.entries].slice(-500));
      } catch { /* Live Runner events remain visible; a missing log is not a task failure. */ }
      finally { pending = false; }
    };
    void read();
    const timer = window.setInterval(() => void read(), 15000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [task.task_id, task.run_id, task.status]);
  const labels = locale === 'zh-CN'
    ? { created: '待启动', starting: '正在启动', running: '运行中', cancelling: '正在取消', completed: '已完成', failed: '失败', cancelled: '已取消', interrupted: '已中断' }
    : { created: 'Ready', starting: 'Starting', running: 'Running', cancelling: 'Cancelling', completed: 'Completed', failed: 'Failed', cancelled: 'Cancelled', interrupted: 'Interrupted' };
  const action = async (fn: () => Promise<unknown>) => {
    setBusy(true); setActionError(null);
    try { await fn(); } catch (error) { setActionError(error instanceof Error ? error.message : JSON.stringify(error)); }
    finally { setBusy(false); }
  };
  const runtime = [...task.recent_events].reverse().find(event => event.runtime)?.runtime;
  return <section className="shared-task-detail" aria-label={locale === 'zh-CN' ? '任务详情' : 'Task details'}>
    <div className="form-section"><h3>{task.input_path.split(/[\\/]/).at(-1)}</h3><p className="project-path">{task.input_path}</p>
      <p>{task.source === 'mcp' ? (locale === 'zh-CN' ? '由 AI Agent 创建' : 'Created by AI Agent') : (locale === 'zh-CN' ? '手动创建' : 'Created in app')} · {labels[task.status]} · {task.quality}</p>
      <p className="project-path">{task.projects_root}</p>
      <p>{t('planner.label')}：{task.planner_enabled ? 'On' : 'Off'}</p>
    </div>
    <div className="progress-section" aria-live="polite"><strong>{task.stage ? t(`stage.${task.stage}` as Parameters<typeof t>[0]) : labels[task.status]}</strong>
      <p>{task.current != null && task.total != null ? `${task.current} / ${task.total}` : labels[task.status]} · {Math.floor(task.elapsed_ms / 1000)} s</p>
      {task.estimated_progress != null && <><progress max={100} value={task.estimated_progress} aria-label={locale === 'zh-CN' ? '估算总进度' : 'Estimated overall progress'} /><small>{locale === 'zh-CN' ? '估算总进度' : 'Estimated overall progress'} {task.estimated_progress.toFixed(1)}%</small></>}
    </div>
    {runtime && <RuntimePanel snapshot={runtime} running={taskIsActive(task)} />}
    {(task.error || actionError) && <div className="project-failure"><CompactError message={actionError ?? task.error!.message} />{task.error?.classification_is_heuristic && <small>{locale === 'zh-CN' ? '错误分类为启发式判断，请结合日志确认。' : 'Error classification is heuristic; inspect logs for evidence.'}</small>}</div>}
    <div className="project-actions">
      {task.status === 'created' && <button type="button" disabled={busy} onClick={() => void action(() => startGuiTask(task.task_id))}>{t('generate.start')}</button>}
      {taskIsActive(task) && task.run_id && <button type="button" disabled={busy || task.status === 'cancelling'} onClick={() => void action(() => cancelSharedTask(task.task_id, task.run_id!))}>{t('progress.cancel')}</button>}
      {task.status === 'completed' && task.project_id && <button type="button" onClick={() => onPreview(task.project_id!)}>{t('project.preview')}</button>}
      {task.status === 'completed' && task.result?.finalPly && <button type="button" disabled={busy} onClick={() => void action(() => exportPly(task.result!))}>{t('export.label')}</button>}
    </div>
    <div className="pipeline-log" role="log" aria-label={locale === 'zh-CN' ? '任务日志' : 'Task logs'}>{logs.length ? logs.map((entry, index) => <div key={`${entry.source}:${index}`}><small>{entry.source}</small> {entry.text}</div>) : task.recent_events.map(event => <div key={`${task.run_id}:${event.sequence}`}><span>{event.message}</span></div>)}</div>
  </section>;
}
