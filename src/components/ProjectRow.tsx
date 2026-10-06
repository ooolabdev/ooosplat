import { CircleAlert, Eye, Film, LoaderCircle, MapPin, Play, Trash2 } from 'lucide-react';
import { useI18n } from '../i18n';
import { taskBasename, taskFormatBytes, taskStatusLabel } from '../app/taskPresentation';
import { useTaskProjectDetail } from '../app/useTaskProjectDetail';
import { useTaskElapsed } from '../app/useTaskElapsed';
import { displayPath } from '../lib/displayPath';
import type { ProjectSummary } from '../types/pipeline';
import { taskIsActive, type SharedTask } from '../types/tasks';
import { CompactError } from './CompactError';

interface Props {
  project: ProjectSummary | null;
  task?: SharedTask;
  selected: boolean;
  busy: boolean;
  previewing: boolean;
  previewDisabled: boolean;
  deleting: boolean;
  revealing: boolean;
  onSelect: (project: ProjectSummary) => void;
  onSelectTask?: () => void;
  onPreview: (project: ProjectSummary) => void;
  onReshoot: (project: ProjectSummary) => void;
  onResume: (project: ProjectSummary) => void;
  onReveal: (project: ProjectSummary) => void;
  onDelete: (project: ProjectSummary) => void;
}

/** The original project row, also used for submitted tasks before project creation. */
export function ProjectRow({ project, task, selected, busy, previewing, previewDisabled, deleting, revealing, onSelect, onSelectTask, onPreview, onReshoot, onResume, onReveal, onDelete }: Props) {
  const { locale, t, formatDate, formatDuration } = useI18n();
  const status = task?.status ?? project?.status ?? 'created';
  const active = task ? taskIsActive(task) : status === 'running';
  const unfinishedTask = task && !active && ['failed', 'cancelled', 'interrupted'].includes(status) ? task : null;
  // A successful detail read confirms that a registered checkpoint can be decoded.
  const checkpoint = useTaskProjectDetail(unfinishedTask);
  const taskElapsed = useTaskElapsed(task);
  const canResume = project && status !== 'completed' && status !== 'created' && !active && (!task || checkpoint != null);
  const select = () => { if (onSelectTask) onSelectTask(); else if (project) onSelect(project); };
  const path = task?.project_path ?? project?.projectPath ?? task?.input_path ?? '';
  const visiblePath = displayPath(path);
  const failure = task ? task.error?.message : project?.failureMessage;
  const duration = task?.run_id ? taskElapsed : project?.durationMs ?? null;
  return <article className={selected ? 'project-row selected' : 'project-row'} data-task-id={task?.task_id} tabIndex={0} role="button" aria-pressed={selected} onClick={select} onKeyDown={event => {
    if (event.target !== event.currentTarget) return;
    if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); select(); }
  }}>
    <div className="project-row-main">
      <div className="project-title-line"><span className={`project-status ${active ? 'running' : status}`} /><strong>{project?.name ?? taskBasename(task?.input_path ?? '')}</strong><span className="status-copy">{taskStatusLabel(locale, status)}</span></div>
      <p className="project-path" title={visiblePath}>{visiblePath}</p>
      {task?.source === 'mcp' && <p className="task-origin">{locale === 'zh-CN' ? '由 AI Agent 创建' : 'Created by AI Agent'}</p>}
      {failure && <p className="project-failure"><CompactError message={failure} /></p>}
      {project?.registeredRatio != null && project.registeredRatio < 0.8 && <p className="project-quality-warning" role="status"><CircleAlert size={13} />{t('result.lowRegistration', { value: (project.registeredRatio * 100).toFixed(1) })}</p>}
    </div>
    <dl className="project-stats">
      <div><dt>PLY</dt><dd>{taskFormatBytes(project?.fileSize ?? null, locale)}</dd></div>
      <div><dt>{t('project.date')}</dt><dd>{formatDate(project?.completedAt ?? project?.createdAt ?? task?.created_at ?? '')}</dd></div>
      <div><dt>{t('project.elapsed')}</dt><dd>{duration == null ? '—' : formatDuration(duration)}</dd></div>
      <div><dt>{t('project.quality')}</dt><dd>{t(`quality.${task?.quality ?? project?.quality ?? 'balanced'}`)}</dd></div>
    </dl>
    {project && <div className="project-actions">
      {status === 'completed' && <button className="preview-link" type="button" disabled={previewDisabled || !project.finalPly} onClick={event => { event.stopPropagation(); select(); onPreview(project); }}>{previewing ? <LoaderCircle className="spin" size={14} /> : <Eye size={14} />}{t(previewing ? 'project.opening' : 'project.preview')}</button>}
      {status === 'completed' && <button className="reshoot-link" type="button" disabled={previewDisabled || !project.finalPly} onClick={event => { event.stopPropagation(); select(); onReshoot(project); }}><Film size={14} />{t('project.reshoot')}</button>}
      {canResume && <button className="resume-link" type="button" disabled={busy} onClick={event => { event.stopPropagation(); select(); onResume(project); }}><Play size={14} fill="currentColor" />{t('project.resume')}</button>}
      <button type="button" disabled={revealing} onClick={event => { event.stopPropagation(); select(); onReveal(project); }}>{revealing ? <LoaderCircle className="spin" size={14} /> : <MapPin size={14} />}{t('project.reveal')}</button>
      <button className="danger-link" type="button" disabled={busy || active || deleting} onClick={event => { event.stopPropagation(); onDelete(project); }}>{deleting ? <LoaderCircle className="spin" size={14} /> : <Trash2 size={14} />}{t('project.delete')}</button>
    </div>}
  </article>;
}
