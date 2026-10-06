import { useI18n } from '../i18n';
import { taskFormatBytes, taskStageLabels } from '../app/taskPresentation';
import type { ProjectSummary, ProjectTaskDetail } from '../types/pipeline';

export function ProjectResultStats({ project, detail, inputImages, registeredImages }: { project: ProjectSummary; detail?: ProjectTaskDetail | null; inputImages?: number | null; registeredImages?: number | null }) {
  const { locale, t, formatNumber, formatDuration } = useI18n();
  const registered = registeredImages ?? detail?.registeredImages;
  const input = inputImages ?? detail?.inputImages;
  return <dl className="project-detail-stats">
    <div><dt>{t('result.splats')}</dt><dd>{project.splatCount == null ? '—' : formatNumber(project.splatCount)}</dd></div>
    <div><dt>{t('result.fileSize')}</dt><dd>{taskFormatBytes(project.fileSize, locale)}</dd></div>
    <div><dt>{t('result.registered')}</dt><dd>{registered == null ? '—' : `${formatNumber(registered)} / ${input == null ? '—' : formatNumber(input)}`}</dd></div>
    <div><dt>{t('result.points')}</dt><dd>{project.points3d == null ? '—' : formatNumber(project.points3d)}</dd></div>
    <div><dt>{t('project.elapsed')}</dt><dd>{project.durationMs == null ? '—' : formatDuration(project.durationMs)}</dd></div>
    <div><dt>{t('project.quality')}</dt><dd>{t(`quality.${project.quality}`)}</dd></div>
    <div><dt>{t('progress.stage')}</dt><dd>{t(taskStageLabels[detail?.stage ?? 'completed'] ?? 'stage.completed')}</dd></div>
  </dl>;
}
