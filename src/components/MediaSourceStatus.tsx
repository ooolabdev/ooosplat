import { useId } from 'react';
import { CircleAlert, Globe, Images, LoaderCircle, MapPin, RotateCcw, TvMinimalPlay } from 'lucide-react';
import { localizePipelineMessage, useI18n } from '../i18n';
import type { InputType, ProjectionDetection, VideoProjection } from '../types/pipeline';
import { AlphaSourceStatus } from './AlphaSourceStatus';
import { CompactError } from './CompactError';

interface Props {
  inputType: InputType;
  checking?: boolean;
  error?: string | null;
  detection?: ProjectionDetection | null;
  projection?: VideoProjection | null;
  historical?: boolean;
  gpsDetected?: boolean;
  gpsFallback?: boolean;
  alphaDetected?: boolean | null;
  /** Undefined retains legacy descriptions; null means no saved metadata. */
  metadataSummary?: string | null;
  disabled?: boolean;
  onProjectionChange?: (value: VideoProjection) => void;
  onRetry?: () => void;
  showTypeIcon?: boolean;
}

/** The connected lower half of the media picker; no independent card or preview. */
export function MediaSourceStatus({ inputType, checking, error, detection, projection, historical, gpsDetected, gpsFallback, alphaDetected, metadataSummary, disabled, onProjectionChange, onRetry, showTypeIcon = true }: Props) {
  const { locale, t } = useI18n();
  const id = useId();
  const dimensionError = error?.includes('图片序列分辨率不一致：')
    ? error.slice(error.indexOf('图片序列分辨率不一致：')) : null;
  if (checking) return <div className="media-source-status checking" role="status" aria-live="polite"><LoaderCircle size={17} className="spin" /><span>{t(inputType === 'video' ? 'source.detectingVideo' : 'source.detectingImages')}</span></div>;
  if (error) return <div className="media-source-status source-analysis-error" role="alert"><CircleAlert size={17} /><div><strong>{t('source.analysisFailed')}</strong>{dimensionError ? <span className="source-dimension-error">{localizePipelineMessage(locale, dimensionError)}</span> : <CompactError message={error} />}</div>{onRetry && <button className="media-retry" type="button" disabled={disabled} onClick={onRetry}><RotateCcw size={13} />{t('source.retry')}</button>}</div>;
  const images = inputType === 'images';
  const panorama = projection === 'equirectangular';
  const choice = !historical && detection?.requiresChoice;
  const unresolved = choice && (!projection || projection === 'auto');
  const unknown = historical && !images && (!projection || projection === 'auto');
  const displayTypeIcon = showTypeIcon && !unresolved && !unknown;
  const TypeIcon = images ? Images : panorama ? Globe : TvMinimalPlay;
  const title = images ? unresolved ? 'source.chooseImageType' : panorama ? 'source.panoramaImages' : 'source.images' : unknown ? 'source.unknown' : unresolved ? 'source.chooseType' : panorama ? 'panorama.detected' : 'panorama.ordinaryDetected';
  const hint = historical
    ? images ? panorama ? 'source.historyPanoramaImages' : 'source.historyImages' : unknown ? 'source.unknownHint' : panorama ? 'source.historyPanorama' : 'source.historyOrdinary'
    : images ? unresolved ? 'source.ambiguousImages' : panorama ? detection?.confirmedErp ? 'source.confirmedImages' : 'source.manualPanoramaImages' : 'source.imagesHint' : unresolved ? 'panorama.ambiguous' : panorama ? detection?.confirmedErp ? 'panorama.confirmed' : 'source.manualPanorama' : 'panorama.ordinaryHint';
  const showGps = images && gpsDetected && (!historical || gpsFallback !== undefined);
  const gpsDescription = `${t('source.gpsDetected')}\n${t(gpsFallback ? 'source.gpsFallback' : 'source.gpsHint')}`;
  const showHint = metadataSummary === undefined || unresolved || unknown;
  const metadata = metadataSummary?.trim();
  return <div className={`media-source-status${displayTypeIcon ? ' has-type-icon' : ''}${historical ? ' readonly' : ''}`} aria-live="polite">
    <div className="media-source-heading source-status-hint">
      {displayTypeIcon && <TypeIcon className="media-source-type-icon" size={17} strokeWidth={1.7} aria-hidden="true" />}
      <div className="media-source-copy source-status-hint-copy">
        <strong title={!showHint ? t(hint) : undefined}>{t(title)}</strong>
        {showHint && <p id={`${id}-hint`}>{t(hint)}</p>}
        {metadata && <p className="media-source-metadata" id={!showHint ? `${id}-hint` : undefined}>{metadata}</p>}
      </div>
      {(showGps || alphaDetected === true) && <div className="source-attribute-tags">
        {showGps && <span className="gps-source-hint source-attribute-tag" role="status" title={gpsDescription} aria-label={gpsDescription} tabIndex={0}>
          <MapPin className="media-source-gps-icon" size={13} strokeWidth={1.7} aria-hidden="true" /><span>{t('source.gpsTag')}</span>
        </span>}
        <AlphaSourceStatus detected={alphaDetected} readOnly={historical} />
      </div>}
    </div>
    {choice && <select id={id} className="video-type-select" aria-label={t(images ? 'source.chooseImageType' : 'source.chooseType')} aria-describedby={showHint || metadata ? `${id}-hint` : undefined} required aria-invalid={unresolved || undefined} disabled={disabled} value={projection === 'auto' ? '' : projection ?? ''} onChange={event => onProjectionChange?.((event.target.value || 'auto') as VideoProjection)}>
      <option value="">{t('source.choosePlaceholder')}</option>
      <option value="perspective">{t(images ? 'source.images' : 'panorama.ordinaryDetected')}</option>
      <option value="equirectangular" disabled={!detection?.supportedErp}>{t(images ? 'source.panoramaImages' : 'source.fullPanorama')}</option>
    </select>}
    {!historical && detection?.unsupported && projection !== 'perspective' && <p className="media-source-warning">{t(images ? 'source.unsupportedImages' : 'panorama.unsupported')}</p>}
  </div>;
}
