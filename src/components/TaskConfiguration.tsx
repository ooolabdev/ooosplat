import { CircleAlert, Cpu, FolderOpen, LoaderCircle, Lock, Zap } from 'lucide-react';
import { localizePipelineMessage, useI18n } from '../i18n';
import { taskBasename } from '../app/taskPresentation';
import { displayPath } from '../lib/displayPath';
import { MediaSourceStatus } from './MediaSourceStatus';
import { FrameSamplingControl } from './FrameSamplingControl';
import {MaterialInputs, materialMetadataSummary} from './MaterialInputs';
import {FrameFpsControl} from './FrameFpsControl';
import { useState } from 'react';
import type { ColmapAccelerationStatus, FramePlan, ImageSequenceInfo, InputType, Quality, RuntimeEstimate, VideoInfo } from '../types/pipeline';

interface Props {
  inputs?:import('../types/inputs').SavedInputs|null;
  gpsAssistance?: import('../types/inputs').GpsAssistanceSummary | null;
  generationPolicy?: import('../types/pipeline').GenerationPolicy | null;
  automaticallyRetried?: boolean;
  sourcePath: string;
  projectsRoot: string;
  inputType: InputType;
  quality: Quality;
  plannerEnabled: boolean;
  completed: boolean;
  active?: boolean;
  acceleration: ColmapAccelerationStatus | null;
  video?: VideoInfo | null;
  imageSequence?: ImageSequenceInfo | null;
  estimatedFrames?: number | null;
  estimate?: RuntimeEstimate | null;
  inheritedFrom?: 'resume' | 'reshoot' | null;
  videoProjection?: import('../types/pipeline').VideoProjection | null;
  inputProjection?: import('../types/pipeline').VideoProjection | null;
  reshootRecipe?: import('../types/pipeline').ReshootRecipeSummary | null;
}

function savedFramePlan(frames: import('../types/inputs').SavedInputs['sources'][number]['frames']): FramePlan | null {
  if (!frames
    || !Number.isFinite(frames.samplingFps)
    || frames.samplingFps <= 0
    || !Number.isFinite(frames.retentionRatio)
    || frames.retentionRatio <= 0) return null;
  return {
    samplingFps: frames.samplingFps,
    retentionRatio: frames.retentionRatio,
    estimatedFrames: frames.initialExtractedFrames && frames.initialExtractedFrames > 0
      ? frames.initialExtractedFrames
      : frames.estimatedFrames,
    minimumFrameOverrideApplied: frames.minimumFrameOverrideApplied,
  };
}

export function TaskConfiguration({ inputs,gpsAssistance = inputs?.gpsAssistance,generationPolicy, automaticallyRetried, sourcePath, projectsRoot, inputType, quality, plannerEnabled, completed, active = false, acceleration, video, imageSequence, inheritedFrom, videoProjection, inputProjection, reshootRecipe }: Props) {
  const { locale, t } = useI18n();
  const [samplingOpen,setSamplingOpen]=useState(false);
  const temporary = acceleration?.detectionState === 'temporarilyUnavailable';
  const warning = acceleration?.backend === 'cpu';
  const visibleSourcePath = displayPath(sourcePath);
  const visibleProjectsRoot = displayPath(projectsRoot);
  const savedAnalyses = Object.fromEntries((inputs?.sources ?? []).map(source => [source.id, {
    id: source.id, identity: source.identity, video: source.video,
    imageSequence: source.imageSequence ?? null, dimensions: source.dimensions,
    plan: savedFramePlan(source.frames),
    gpsDetected: gpsAssistance?.detectedSourceIds.includes(source.id),
    gpsFallback: gpsAssistance == null ? undefined : ['fallback', 'unavailable'].includes(gpsAssistance.status),
    projectionDetection: { sourceIdentity: source.identity, confirmedErp: source.projection === 'equirectangular',
      supportedErp: source.projection === 'equirectangular', requiresChoice: false, reason: '', previewJpeg: [] },
  }]));
  const savedVideoMaterials=inputs?.request.mode==='materials'?inputs.request.items.flatMap(item=>{const source=inputs.sources.find(value=>value.id===item.id);if(!source?.video)return[];const defaultTarget={fast:6,balanced:8,high:12}[quality];const target=generationPolicy?.frameSamplingFps??(generationPolicy?.frameSamplingRatio!=null&&inputs.sources.filter(value=>value.video).length===1?generationPolicy.frameSamplingRatio*source.video.fps:null)??Math.min(defaultTarget,source.video.fps);const policyVersion=generationPolicy?.version??0;return[{id:item.id,name:source.names[0]??item.id,video:source.video,plan:savedFramePlan(source.frames),frameSamplingRange:null,removalFps:policyVersion>=5?(item.frameRemovalFps??null):Math.max(.1,source.video.fps-Math.min(source.video.fps,target)),defaultRemovalFps:policyVersion===5?defaultTarget:policyVersion>=6?undefined:Math.max(.1,source.video.fps-Math.min(source.video.fps,defaultTarget))}];}):[];
  return <div className="project-configuration-detail">
    {!active && !completed && reshootRecipe && <div className="form-section"><label className="field-label">{t('reshoot.planTitle')}</label>
      <p>{t(`reshoot.condition.${reshootRecipe.options.condition}`)}</p>
      {(reshootRecipe.initial.reconstruction !== reshootRecipe.actual.reconstruction || reshootRecipe.initial.training !== reshootRecipe.actual.training) && <p>{t(reshootRecipe.initial.reconstruction === 'full' ? 'reshoot.planFull' : reshootRecipe.initial.training === 'warm' ? 'reshoot.planWarm' : 'reshoot.planCold')} →</p>}
      <p>{t(reshootRecipe.actual.reconstruction === 'full' ? 'reshoot.planFull' : reshootRecipe.actual.training === 'warm' ? 'reshoot.planWarm' : 'reshoot.planCold')}</p>
      <label><input type="checkbox" checked={reshootRecipe.options.allowFallback} disabled /> {t('reshoot.allowFallback')}</label>
    </div>}
    <div className="form-section"><label className="field-label">{t('input.label')}</label>
      {inputs ? <MaterialInputs inputs={inputs.request} analyses={savedAnalyses} checking={false} disabled readOnly />
        : <div className="source-input-card readonly"><div className="path-picker media-path readonly" title={visibleSourcePath}><span><strong>{taskBasename(visibleSourcePath)}</strong><small>{visibleSourcePath}</small></span></div><div className="material-inspector"><MediaSourceStatus historical inputType={inputType} projection={inputProjection ?? videoProjection} alphaDetected={inputType === 'images' ? imageSequence?.hasAlpha : video?.hasAlpha} metadataSummary={materialMetadataSummary({ video: video ?? null, imageSequence: imageSequence ?? null }, value => t('inputs.count', { count: value }))} /></div></div>}
    </div>
    <div className="form-section"><label className="field-label">{t('project.root')}</label>
      <div className="path-picker compact readonly" title={visibleProjectsRoot}><FolderOpen size={18} /><span><strong>{taskBasename(visibleProjectsRoot)}</strong><small>{visibleProjectsRoot}</small></span></div>
    </div>
    <div className="form-section">
      <div className="field-label-row"><label className="field-label">{t('quality.label')}</label><span className="quality-header-actions">{savedVideoMaterials.length>0&&<button className="more-settings-button" type="button" aria-expanded={samplingOpen} onClick={()=>setSamplingOpen(open=>!open)}>{t(samplingOpen?'sampling.less':'sampling.more')}</button>}{!active && !completed && inheritedFrom && <span className="locked-setting"><Lock size={12} />{t(inheritedFrom === 'reshoot' ? 'reshoot.inheritedReadonly' : 'project.inheritedReadonly')}</span>}</span></div>
      <div className="quality-settings locked-quality-settings">
        {samplingOpen && (inputs
          ? <FrameFpsControl materials={savedVideoMaterials} quality={quality} disabled onChange={()=>{}}/>
          : generationPolicy && inputType === 'video'
            ? <FrameSamplingControl video={video ?? null} quality={quality} value={generationPolicy.frameSamplingRatio} disabled onChange={() => {}} />
            : null)}
        <div className="quality-list" role="radiogroup" aria-label={t('quality.label')}>{(['fast', 'balanced', 'high'] as const).map(value => <button key={value} type="button" role="radio" disabled aria-checked={quality === value} className={quality === value ? 'quality-option selected' : 'quality-option'}><span className="radio-mark"><span /></span><span><strong>{t(`quality.${value}`)}</strong><small>{t(`quality.${value}Hint`)}</small></span></button>)}</div>
        <button className="planner-switch" type="button" role="switch" aria-checked={generationPolicy?.autoRetryEnabled ?? plannerEnabled} disabled><span><strong>{t(generationPolicy ? 'retry.label' : 'planner.label')}</strong><small>{t(generationPolicy ? 'retry.hint' : 'planner.hint')}</small></span><i aria-hidden="true"><span /></i></button>
        {completed && automaticallyRetried && <p className="field-note">{t('retry.used')}</p>}
      </div>
    </div>
    <div className={`acceleration-status ${temporary ? 'warning' : acceleration?.backend === 'gpu' ? 'gpu' : warning ? 'warning' : 'cpu'}`} aria-live="polite">
      <span className="acceleration-icon">{acceleration == null ? <LoaderCircle className="spin" size={17} /> : temporary || warning ? <CircleAlert size={17} /> : acceleration.backend === 'gpu' ? <Zap size={17} fill="currentColor" /> : <Cpu size={17} />}</span>
      <span><strong>{acceleration == null ? t('gpu.detecting') : temporary ? t('gpu.temporarilyUnavailable') : acceleration.backend === 'gpu' ? t(completed ? 'gpu.enabledCompleted' : 'gpu.enabled') : t('gpu.cpu')}</strong><small>{acceleration == null ? t('gpu.reading') : temporary ? t('gpu.temporaryHint') : localizePipelineMessage(locale, acceleration.reason)}</small></span>
    </div>
  </div>;
}
