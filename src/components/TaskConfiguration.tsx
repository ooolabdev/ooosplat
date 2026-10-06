import { Blend, CircleAlert, Clapperboard, Cpu, FolderOpen, Images, LoaderCircle, Lock, Zap } from 'lucide-react';
import { localizePipelineMessage, useI18n } from '../i18n';
import { taskBasename } from '../app/taskPresentation';
import { displayPath } from '../lib/displayPath';
import type { ColmapAccelerationStatus, ImageSequenceInfo, InputType, Quality, RuntimeEstimate, VideoInfo } from '../types/pipeline';

interface Props {
  sourcePath: string;
  projectsRoot: string;
  inputType: InputType;
  quality: Quality;
  plannerEnabled: boolean;
  completed: boolean;
  acceleration: ColmapAccelerationStatus | null;
  video?: VideoInfo | null;
  imageSequence?: ImageSequenceInfo | null;
  estimatedFrames?: number | null;
  estimate?: RuntimeEstimate | null;
  inheritedFrom?: 'resume' | 'reshoot' | null;
}

export function TaskConfiguration({ sourcePath, projectsRoot, inputType, quality, plannerEnabled, completed, acceleration, video, imageSequence, estimatedFrames, estimate, inheritedFrom }: Props) {
  const { locale, t, formatNumber, formatDuration } = useI18n();
  const temporary = acceleration?.detectionState === 'temporarilyUnavailable';
  const warning = acceleration?.backend === 'cpu';
  const duration = video ? `${Math.floor(video.duration / 60)}:${Math.round(video.duration % 60).toString().padStart(2, '0')}` : '—';
  const visibleSourcePath = displayPath(sourcePath);
  const visibleProjectsRoot = displayPath(projectsRoot);
  return <div className="project-configuration-detail">
    <div className="form-section"><label className="field-label">{t('input.label')}</label>
      <div className="path-picker readonly" title={visibleSourcePath}>{inputType === 'images' ? <Images size={18} /> : <Clapperboard size={18} />}<span><strong>{taskBasename(visibleSourcePath)}</strong><small>{visibleSourcePath}</small></span></div>
    </div>
    <div className="form-section"><label className="field-label">{t('project.root')}</label>
      <div className="path-picker compact readonly" title={visibleProjectsRoot}><FolderOpen size={18} /><span><strong>{taskBasename(visibleProjectsRoot)}</strong><small>{visibleProjectsRoot}</small></span></div>
    </div>
    <div className="form-section">
      <div className="field-label-row"><label className="field-label">{t('quality.label')}</label>{!completed && inheritedFrom && <span className="locked-setting"><Lock size={12} />{t(inheritedFrom === 'reshoot' ? 'reshoot.inheritedReadonly' : 'project.inheritedReadonly')}</span>}</div>
      <div className="quality-settings locked-quality-settings">
        <div className="quality-list" role="radiogroup" aria-label={t('quality.label')}>{(['fast', 'balanced', 'high'] as const).map(value => <button key={value} type="button" role="radio" disabled aria-checked={quality === value} className={quality === value ? 'quality-option selected' : 'quality-option'}><span className="radio-mark"><span /></span><span><strong>{t(`quality.${value}`)}</strong><small>{t(`quality.${value}Hint`)}</small></span></button>)}</div>
        <button className="planner-switch" type="button" role="switch" aria-checked={plannerEnabled} disabled><span><strong>{t('planner.label')}</strong><small>{t('planner.hint')}</small></span><i aria-hidden="true"><span /></i></button>
      </div>
    </div>
    <div className={`acceleration-status ${temporary ? 'warning' : acceleration?.backend === 'gpu' ? 'gpu' : warning ? 'warning' : 'cpu'}`} aria-live="polite">
      <span className="acceleration-icon">{acceleration == null ? <LoaderCircle className="spin" size={17} /> : temporary || warning ? <CircleAlert size={17} /> : acceleration.backend === 'gpu' ? <Zap size={17} fill="currentColor" /> : <Cpu size={17} />}</span>
      <span><strong>{acceleration == null ? t('gpu.detecting') : temporary ? t('gpu.temporarilyUnavailable') : acceleration.backend === 'gpu' ? t(completed ? 'gpu.enabledCompleted' : 'gpu.enabled') : t('gpu.cpu')}</strong><small>{acceleration == null ? t('gpu.reading') : temporary ? t('gpu.temporaryHint') : localizePipelineMessage(locale, acceleration.reason)}</small></span>
    </div>
    {(video || imageSequence) && <div className="source-metrics project-source-metrics">
      <span><small>{t(inputType === 'images' ? 'metrics.imageCount' : 'metrics.duration')}</small><b>{imageSequence ? t('common.images', { count: formatNumber(imageSequence.imageCount) }) : duration}</b></span>
      <span><small>{t('metrics.resolution')}</small><b>{imageSequence?.width ?? video?.width} × {imageSequence?.height ?? video?.height}</b></span>
      {!completed && <span><small>{t('metrics.estimatedFrames')}</small><b>{estimatedFrames == null ? '—' : t('metrics.approx', { value: formatNumber(estimatedFrames) })}</b></span>}
      {!completed && <span><small>{t('metrics.estimate')}</small><b>{estimate ? t('metrics.approx', { value: formatDuration(estimate.estimatedMs) }) : '—'}</b></span>}
    </div>}
    {(video?.hasAlpha || imageSequence?.hasAlpha) && <div className="alpha-source-status" role="status"><Blend size={17} /><span><strong>{t(inputType === 'images' ? 'alpha.imagesTitle' : 'alpha.videoTitle')}</strong><small>{inputType === 'images' ? t(completed ? 'alpha.imagesCompletedHint' : 'alpha.imagesHint') : t(completed ? 'alpha.videoCompletedHint' : 'alpha.videoHint', { format: video?.pixelFormat || 'Alpha' })}</small></span></div>}
  </div>;
}
