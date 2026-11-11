import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Clapperboard, FilePlus2, FolderPlus, Images, LoaderCircle, Upload, X } from 'lucide-react';
import { localizePipelineMessage, useI18n } from '../i18n';
import { displayPath } from '../lib/displayPath';
import { MediaSourceStatus } from './MediaSourceStatus';
import {
  sourcePath,
  type ColmapDiscovery,
  type ColmapInput,
  type GenerationInputs,
  type MaterialAnalysis,
  type MaterialInput,
} from '../types/inputs';
import type { ImageMatchingStrategy, VideoProjection } from '../types/pipeline';

interface Props {
  inputs: GenerationInputs | null;
  discovery?: ColmapDiscovery | null;
  analyses: Record<string, MaterialAnalysis>;
  errors?: Record<string, string>;
  checking: boolean;
  disabled: boolean;
  dragActive?: boolean;
  readOnly?: boolean;
  colmapBlocked?: boolean;
  error?: string | null;
  onFiles?: () => void;
  onFolders?: () => void;
  onRemove?: (id: string) => void;
  onProjection?: (id: string, value: VideoProjection) => void;
  onImageMatchingStrategy?: (id: string, value: ImageMatchingStrategy) => void;
  onRetry?: () => void;
  onColmapField?: (field: keyof ColmapInput) => void;
  onColmapModel?: (model: string) => void;
  onColmapImages?: (model: string, images: string) => void;
}

const nameOf = (path: string) => path.split(/[\\/]/).at(-1) || path;

function itemName(item: MaterialInput, count: number | null, t: ReturnType<typeof useI18n>['t']) {
  if (item.source.kind === 'images') return t('inputs.looseImages', { count: count ?? item.source.paths.length });
  return nameOf(item.source.path);
}

export function materialMetadataSummary(
  analysis: Pick<MaterialAnalysis, 'video' | 'imageSequence' | 'dimensions'> | undefined,
  imageCount: (count: number) => string,
  fallbackImageCount?: number,
) {
  const positive = (value: number) => Number.isFinite(value) && value > 0;
  if (analysis?.video) {
    const { width, height, fps } = analysis.video;
    return [positive(width) && positive(height) ? `${width} × ${height}` : null,
      positive(fps) ? `${Number(fps.toFixed(3))} FPS` : null].filter(Boolean).join(' · ') || null;
  }
  if (analysis?.imageSequence) {
    const sequence = analysis.imageSequence;
    const dimensions = (analysis.dimensions?.length ? analysis.dimensions : [[sequence.width, sequence.height]])
      .filter(([width, height]) => positive(width) && positive(height))
      .slice(0, 2).map(([width, height]) => `${width} × ${height}`).join(' / ');
    return [positive(sequence.imageCount) ? imageCount(sequence.imageCount) : null, dimensions]
      .filter(Boolean).join(' · ') || null;
  }
  return fallbackImageCount && positive(fallbackImageCount) ? imageCount(fallbackImageCount) : null;
}

function typeKey(item: MaterialInput, analysis: MaterialAnalysis | undefined, readOnly: boolean) {
  const images = item.source.kind !== 'video';
  if (!readOnly && !analysis) return images ? 'inputs.imageSet' : 'inputs.video';
  if (item.projection === 'equirectangular') return images ? 'source.panoramaImages' : 'panorama.detected';
  if (item.projection === 'perspective') return images ? 'source.images' : 'panorama.ordinaryDetected';
  return images ? 'source.chooseImageType' : 'source.chooseType';
}

function ImageOrderQuestion({ item, count, disabled, onChange }: {
  item: MaterialInput;
  count: number;
  disabled: boolean;
  onChange?: (value: ImageMatchingStrategy) => void;
}) {
  const { t } = useI18n();
  return <fieldset className="material-order-question" disabled={disabled}>
    <legend>{t('preflight.imageOrderQuestion', { count })}</legend>
    <p>{t('preflight.imageOrderDescriptionInline')}</p>
    <p className="material-order-macro">{t('preflight.imageOrderMacro')}</p>
    <div className="material-order-options">
      {(['sequential', 'exhaustive'] as const).map(strategy => <label key={strategy} className={item.confirmedImageOrder && item.imageMatchingStrategy === strategy ? 'selected' : ''}>
        <input type="radio" name={`${item.id}-image-order`} value={strategy}
          checked={!!item.confirmedImageOrder && item.imageMatchingStrategy === strategy}
          onChange={() => onChange?.(strategy)} />
        <span><strong>{t(strategy === 'sequential' ? 'preflight.ordered' : 'preflight.unordered')}</strong>
          <small>{t(strategy === 'sequential' ? 'preflight.orderedHint' : 'preflight.unorderedHint')}</small></span>
      </label>)}
    </div>
  </fieldset>;
}

function MaterialInspector({ id, item, analysis, error, checking, disabled, readOnly, onProjection, onRetry, onImageMatchingStrategy }: {
  id?: string;
  item: MaterialInput;
  analysis?: MaterialAnalysis;
  error?: string | null;
} & Pick<Props, 'checking' | 'disabled' | 'readOnly' | 'onProjection' | 'onRetry' | 'onImageMatchingStrategy'>) {
  const { t } = useI18n();
  const images = item.source.kind !== 'video';
  const count = analysis?.imageSequence?.imageCount ?? (item.source.kind === 'images' ? item.source.paths.length : 0);
  const currentAnalysis = analysis && (!item.identity || item.identity === analysis.identity
    || item.identity === `legacy:${analysis.legacyIdentity ?? ''}`) ? analysis : undefined;
  return <div id={id} className="material-inspector">
    <MediaSourceStatus inputType={images ? 'images' : 'video'}
      checking={!analysis && !error && checking} error={error}
      historical={readOnly} gpsDetected={analysis?.gpsDetected} gpsFallback={analysis?.gpsFallback} detection={analysis?.projectionDetection ?? null}
      alphaDetected={!checking && !error
        ? (images ? currentAnalysis?.imageSequence?.hasAlpha : currentAnalysis?.video?.hasAlpha)
        : undefined}
      metadataSummary={materialMetadataSummary(currentAnalysis, value => t('inputs.count', { count: value }),
        item.source.kind === 'images' ? item.source.paths.length : undefined)}
      projection={item.projection} disabled={disabled}
      onProjectionChange={value => onProjection?.(item.id, value)} onRetry={onRetry} />
    {!readOnly && images && count > 100 && analysis && !error && <ImageOrderQuestion item={item} count={count} disabled={disabled}
      onChange={value => onImageMatchingStrategy?.(item.id, value)} />}
    {item.source.kind === 'images' && <details className="material-members">
      <summary>{t('inputs.count', { count: item.source.paths.length })}</summary>
      {item.source.paths.map((member, index) => <small key={`${index}:${member}`}>{displayPath(member)}</small>)}
    </details>}
  </div>;
}

function ColmapInspector({ id, input, disabled, readOnly, checking, onColmapField }: {
  id?: string;
  input: ColmapInput;
} & Pick<Props, 'disabled' | 'readOnly' | 'checking' | 'onColmapField'>) {
  const { t } = useI18n();
  return <div id={id} className="material-inspector colmap-inspector">
    {(['model', 'images', 'database'] as const).map(field => <button className="colmap-path" key={field} type="button"
      disabled={disabled || readOnly} onClick={() => onColmapField?.(field)}>
      <strong>{t(`inputs.${field}`)}</strong><small>{displayPath(input[field] || '—')}</small>
    </button>)}
    <p className="field-note">{t('inputs.databaseHint')}</p>
    {checking ? <div className="material-inline-state"><LoaderCircle size={15} className="spin" />{t('inputs.checkingColmap')}</div>
      : input.identity ? <div className="material-imported"><strong>{t('inputs.imported')}</strong><small>{t('inputs.skip')}</small></div> : null}
  </div>;
}

function ColmapDiscoveryInspector({ id, discovery, disabled, onColmapModel, onColmapImages, onColmapField }: {
  id?: string;
  discovery: ColmapDiscovery;
} & Pick<Props, 'disabled' | 'onColmapModel' | 'onColmapImages' | 'onColmapField'>) {
  const { t } = useI18n();
  const selected = discovery.models.find(candidate => candidate.model === discovery.selectedModel);
  return <div id={id} className="material-inspector colmap-discovery" role="group" aria-label={t('inputs.colmapReconstruction')}>
    <strong>{discovery.models.length > 1 ? t('inputs.chooseColmapModel') : t('inputs.colmapImagesMissing')}</strong>
    {discovery.models.length > 1 && <div className="colmap-candidates">
      {discovery.models.map(candidate => <label key={candidate.model} className={candidate.model === discovery.selectedModel ? 'selected' : ''}>
        <input type="radio" name="colmap-model" checked={candidate.model === discovery.selectedModel}
          onChange={() => onColmapModel?.(candidate.model)} disabled={disabled} />
        <span><strong>{nameOf(candidate.model)}</strong><small>{displayPath(candidate.model)}</small></span>
      </label>)}
    </div>}
    {selected && selected.imageCandidates.length > 1 && <div className="colmap-candidates">
      <p>{t('inputs.chooseColmapImages')}</p>
      {selected.imageCandidates.map(images => <button type="button" key={images} disabled={disabled}
        onClick={() => onColmapImages?.(selected.model, images)}>{displayPath(images)}</button>)}
    </div>}
    {selected && selected.imageCandidates.length === 1 && <button type="button" className="secondary-action"
      disabled={disabled} onClick={() => onColmapImages?.(selected.model, selected.imageCandidates[0])}>{t('inputs.useDetectedImages')}</button>}
    {selected && selected.imageCandidates.length === 0 && <button type="button" className="secondary-action"
      disabled={disabled} onClick={() => onColmapField?.('images')}>{t('inputs.chooseColmapImages')}</button>}
  </div>;
}

export function MaterialInputs({ inputs, discovery, analyses, errors = {}, checking, disabled, dragActive = false, readOnly = false, colmapBlocked = false, error,
  onFiles, onFolders, onRemove, onProjection, onImageMatchingStrategy, onRetry, onColmapField, onColmapModel, onColmapImages }: Props) {
  const { locale, t } = useI18n();
  const inspectorId = useId();
  const list = useRef<HTMLDivElement>(null);
  const autoFocusedIssue = useRef<string | null>(null);
  const colmapUnavailable = !readOnly && colmapBlocked;
  const items = inputs?.mode === 'materials' ? inputs.items : [];
  const total = inputs?.mode === 'colmap' || discovery ? 1 : items.length;
  const itemIds = useMemo(() => discovery
    ? ['colmap-discovery']
    : inputs?.mode === 'colmap'
      ? ['colmap']
      : items.map(item => item.id), [discovery, inputs, items]);
  const issueId = useMemo(() => {
    if (discovery) return 'colmap-discovery';
    if (inputs?.mode === 'colmap') return colmapUnavailable || checking || error || !inputs.input.identity ? 'colmap' : null;
    return items.find(item => {
      const analysis = analyses[item.id];
      return !!errors[item.id] || !analysis || item.projection === 'auto'
        || ((analysis.imageSequence?.imageCount ?? 0) > 100 && !item.confirmedImageOrder);
    })?.id ?? null;
  }, [analyses, checking, colmapUnavailable, discovery, error, errors, inputs, items]);
  const [activeId, setActiveId] = useState<string | null>(() => issueId ?? itemIds[0] ?? null);
  useEffect(() => {
    let frame: number | null = null;
    if (issueId && autoFocusedIssue.current !== issueId) {
      autoFocusedIssue.current = issueId;
      setActiveId(issueId);
      frame = requestAnimationFrame(() => [...(list.current?.querySelectorAll<HTMLElement>('[data-material-id]') ?? [])]
        .find(element => element.dataset.materialId === issueId)?.scrollIntoView?.({ block: 'nearest' }));
    } else {
      if (!issueId) autoFocusedIssue.current = null;
      if (!activeId || !itemIds.includes(activeId)) setActiveId(itemIds[0] ?? null);
    }
    return () => { if (frame !== null) cancelAnimationFrame(frame); };
  }, [activeId, issueId, itemIds]);
  const unresolvedMaterialCount = items.filter(item => {
    const analysis = analyses[item.id];
    return !!errors[item.id] || !analysis || item.projection === 'auto' || ((analysis.imageSequence?.imageCount ?? 0) > 100 && !item.confirmedImageOrder);
  }).length;
  const actionCount = readOnly ? 0
    : discovery ? 1
      : inputs?.mode === 'colmap' ? (colmapUnavailable || checking || error || !inputs.input.identity ? 1 : 0)
        : Math.max(unresolvedMaterialCount, error && !Object.keys(errors).length && total ? 1 : 0);
  const exclusive = inputs?.mode === 'colmap' || !!discovery;
  const remove = (id: string) => {
    if (activeId === id) {
      const index = itemIds.indexOf(id);
      setActiveId(itemIds[index + 1] ?? itemIds[index - 1] ?? null);
    }
    onRemove?.(id);
  };

  return <div className={`material-inputs source-input-card${readOnly ? ' read-only' : ''}${dragActive ? ' drag-active' : ''}`}>
    <div className={`material-input-content${dragActive ? ' hidden-for-drop' : ''}`} aria-hidden={dragActive || undefined} inert={dragActive || undefined}>
      {total > 0 && <div className="material-input-summary">
        <div><strong>{t('inputs.groups', { count: total })}</strong>
          <small>{actionCount ? t(actionCount === 1 ? 'inputs.oneNeedsConfirmation' : 'inputs.needsConfirmation', { count: actionCount }) : t('inputs.allReady')}</small></div>
        {!readOnly && <div className="material-add-actions material-input-menu">
          <button type="button" disabled={disabled || exclusive} onClick={onFiles}><FilePlus2 size={14} aria-hidden="true" />{t('inputs.files')}</button>
          <button type="button" disabled={disabled || exclusive} onClick={onFolders}><FolderPlus size={14} aria-hidden="true" />{t('inputs.folders')}</button>
        </div>}
      </div>}

      {!total && !readOnly && <div className="material-empty">
        <p>{t('inputs.empty')}</p><div className="material-empty-actions material-add-actions material-input-menu">
          <button type="button" disabled={disabled} onClick={onFiles}><FilePlus2 size={14} aria-hidden="true" />{t('inputs.files')}</button>
          <button type="button" disabled={disabled} onClick={onFolders}><FolderPlus size={14} aria-hidden="true" />{t('inputs.folders')}</button>
        </div>
      </div>}

      {total > 0 && <div className="material-list" ref={list}>
      {discovery && <div className="material-list-entry" data-material-id="colmap-discovery">
        <div className={`material-row needs-action${activeId === 'colmap-discovery' ? ' active' : ''}`}>
          <button type="button" className="material-row-main" aria-pressed={activeId === 'colmap-discovery'} aria-expanded={activeId === 'colmap-discovery'} aria-controls={activeId === 'colmap-discovery' ? inspectorId : undefined} onClick={() => setActiveId('colmap-discovery')}><Images size={18} />
            <span className="material-row-copy"><strong>{t('inputs.colmapReconstruction')}</strong><small>{discovery.models.length > 1 ? t('inputs.multipleColmapModels') : displayPath(discovery.models[0]?.model || '')}</small></span>
            <span className="material-row-type">{t('inputs.needsChoice')}</span></button>
          <button type="button" className="material-remove" disabled={disabled} aria-label={t('common.close')} onClick={() => remove('colmap-discovery')}><X size={15} /></button>
        </div>
        {activeId === 'colmap-discovery' && (colmapUnavailable
          ? <div id={inspectorId} className="material-inspector colmap-inspector" role="status"><p className="field-note">{t('input.colmapDisabled')}</p></div>
          : <ColmapDiscoveryInspector id={inspectorId} discovery={discovery} disabled={disabled}
            onColmapModel={onColmapModel} onColmapImages={onColmapImages} onColmapField={onColmapField} />)}
      </div>}

      {inputs?.mode === 'colmap' && <div className="material-list-entry" data-material-id="colmap">
        <div className={`material-row${colmapUnavailable ? ' needs-action' : checking ? ' checking' : error ? ' error' : inputs.input.identity ? ' ready' : ' needs-action'}${activeId === 'colmap' ? ' active' : ''}`}>
          <button type="button" className="material-row-main" aria-pressed={activeId === 'colmap'} aria-expanded={activeId === 'colmap'} aria-controls={activeId === 'colmap' ? inspectorId : undefined} onClick={() => setActiveId('colmap')}><Images size={18} />
            <span className="material-row-copy"><strong>{t('inputs.colmapReconstruction')}</strong><small>{displayPath(inputs.input.model)}</small></span>
            <span className="material-row-type">{t('inputs.imported')}</span></button>
          {!readOnly && <button type="button" className="material-remove" disabled={disabled} aria-label={t('common.close')} onClick={() => remove('colmap')}><X size={15} /></button>}
        </div>
        {activeId === 'colmap' && (colmapUnavailable
          ? <div id={inspectorId} className="material-inspector colmap-inspector" role="status"><p className="field-note">{t('input.colmapDisabled')}</p></div>
          : <ColmapInspector id={inspectorId} input={inputs.input} checking={checking}
            disabled={disabled} readOnly={readOnly} onColmapField={onColmapField} />)}
      </div>}

      {items.map(item => {
        const analysis = analyses[item.id];
        const itemError = errors[item.id] ?? (!analysis && !checking ? error : null);
        const count = analysis?.imageSequence?.imageCount ?? (item.source.kind === 'images' ? item.source.paths.length : null);
        const orderMissing = (analysis?.imageSequence?.imageCount ?? 0) > 100 && !item.confirmedImageOrder;
        const state = itemError ? 'error' : !analysis && checking ? 'checking' : item.projection === 'auto' || orderMissing ? 'needs-action' : 'ready';
        const ItemIcon = item.source.kind === 'video' ? Clapperboard : Images;
        const path = sourcePath(item.source);
        const name = itemName(item, count, t);
        return <div className="material-list-entry material-item" data-material-id={item.id} key={item.id}>
          <div className={`material-row ${state}${activeId === item.id ? ' active' : ''}`}>
            <button type="button" className="material-row-main" aria-pressed={activeId === item.id} aria-expanded={activeId === item.id} aria-controls={activeId === item.id ? inspectorId : undefined} onClick={() => setActiveId(item.id)}>
              <ItemIcon size={18} /><span className="material-row-copy"><strong>{name}</strong>
                <small>{item.source.kind === 'images' ? t('inputs.count', { count: count ?? item.source.paths.length }) : displayPath(path)}</small></span>
              <span className="material-row-meta"><strong>{t(typeKey(item, analysis, readOnly))}</strong></span>
            </button>
            {!readOnly && <button type="button" className="material-remove" disabled={disabled} aria-label={`${t('common.close')} ${name}`} onClick={() => remove(item.id)}><X size={15} /></button>}
          </div>
          {activeId === item.id && <MaterialInspector id={inspectorId} item={item} analysis={analysis} error={itemError} readOnly={readOnly}
            checking={!errors[item.id] && !error && checking} disabled={disabled} onProjection={onProjection}
            onRetry={onRetry} onImageMatchingStrategy={onImageMatchingStrategy} />}
        </div>;
      })}
      </div>}

      {error && !Object.keys(errors).length && inputs?.mode !== 'colmap' && !discovery && <div className="material-error">
        <span>{localizePipelineMessage(locale, error)}</span>
        {!readOnly && <button type="button" disabled={disabled || checking} onClick={onRetry}>{t('common.retry')}</button>}
      </div>}

    </div>
    {dragActive && <div className="material-drop-prompt" role="status" aria-live="polite">
      <Upload size={22} aria-hidden="true" /><strong>{t('input.dropHere')}</strong>
    </div>}
  </div>;
}
