import { useEffect, useId, useMemo, useState, type CSSProperties } from 'react';
import { ChevronDown } from 'lucide-react';
import { useI18n } from '../i18n';
import type { FramePlan, Quality, VideoInfo } from '../types/pipeline';

export interface FrameSamplingRange {
    minimumTargetFps: number;
    maximumTargetFps: number;
    minimum: FramePlan;
    maximum: FramePlan;
}

export interface FrameFpsMaterial {
    id: string;
    name: string;
    video: VideoInfo;
    plan?: FramePlan | null;
    frameSamplingRange?: FrameSamplingRange | null;
    removalFps: number | null;
    defaultRemovalFps?: number;
    planning?: boolean;
}

/** Version-6 quality defaults are retained target rates, not removal rates. */
export const defaultTargetFps = (quality: Quality) => ({ fast: 6, balanced: 8, high: 12 })[quality];

/** User-facing target FPS detents. The backend remains authoritative for the resulting plan. */
export function fpsLevels(minimum: number, maximum: number, defaultTarget: number, compatibilityTarget: number | null) {
    if (!Number.isFinite(minimum) || !Number.isFinite(maximum) || maximum < minimum) return [];
    const options = [minimum, Math.min(maximum, Math.max(minimum, defaultTarget)), maximum];
    for (let fps = Math.ceil(minimum); fps <= Math.floor(maximum); fps++) options.push(fps);
    if (compatibilityTarget != null && Number.isFinite(compatibilityTarget))
        options.push(Math.min(maximum, Math.max(minimum, compatibilityTarget)));
    return options.sort((a, b) => a - b).filter((value, index, values) => index === 0 || Math.abs(value - values[index - 1]) > 1e-9);
}

const formatFps = (fps: number) => {
    const nearestInteger = Math.round(fps);
    const displayValue = Math.abs(fps - nearestInteger) <= 0.005 ? nearestInteger : fps;
    return Number(displayValue.toFixed(3)).toString();
};
const closeEnough = (left: number, right: number) => Math.abs(left - right) < 1e-9;

export function FrameFpsControl({ materials, quality, disabled, onChange }: {
    materials: FrameFpsMaterial[];
    quality: Quality;
    disabled: boolean;
    onChange: (id: string, value: number | null) => void;
}) {
    const { t } = useI18n();
    const id = useId();
    const [selectedId, setSelectedId] = useState(materials[0]?.id ?? '');
    useEffect(() => {
        if (!materials.some(material => material.id === selectedId)) setSelectedId(materials[0]?.id ?? '');
    }, [materials, selectedId]);
    const material = materials.find(item => item.id === selectedId) ?? materials[0];
    const range = material?.frameSamplingRange ?? null;
    const minimumTarget = range?.minimumTargetFps ?? 0;
    const maximumTarget = range?.maximumTargetFps ?? 0;
    const maximumRemoval = Math.max(0, (material?.video.fps ?? 0) - minimumTarget);
    const qualityTarget = material
        ? Math.min(maximumTarget, Math.max(minimumTarget, defaultTargetFps(quality)))
        : 0;
    const defaultRemoval = Math.min(maximumRemoval, Math.max(0,
        material?.defaultRemovalFps ?? ((material?.video.fps ?? 0) - qualityTarget)));
    const removal = Math.min(maximumRemoval, Math.max(0, material?.removalFps ?? defaultRemoval));
    const target = material ? Math.min(maximumTarget, Math.max(minimumTarget, material.video.fps - removal)) : 0;
    const defaultTarget = material ? Math.min(maximumTarget, Math.max(minimumTarget, material.video.fps - defaultRemoval)) : 0;
    const levels = useMemo(() => range
        ? fpsLevels(minimumTarget, maximumTarget, defaultTarget, target)
        : [], [range, minimumTarget, maximumTarget, defaultTarget, target]);
    if (!material) return null;

    const index = Math.max(0, levels.findIndex(value => closeEnough(value, target)));
    const defaultIndex = levels.findIndex(value => closeEnough(value, defaultTarget));
    const progress = levels.length > 1 ? index / (levels.length - 1) * 100 : 0;
    const defaultPosition = levels.length > 1 ? Math.max(0, defaultIndex) / (levels.length - 1) * 100 : 0;
    const collapsedRange = !!range && Math.abs(range.maximum.samplingFps - range.minimum.samplingFps) < 1e-9;
    const planning = material.planning === true;
    const candidatePlan = planning ? null : material.plan;
    const plan = candidatePlan
        && Number.isFinite(candidatePlan.samplingFps)
        && Number.isFinite(candidatePlan.retentionRatio)
        ? candidatePlan
        : null;
    // The slider represents the requested retained rate. Frame-count rounding and
    // container-duration noise can make count/duration report 11.999 or 12.001;
    // exposing that noise makes an integer preset look incorrectly configured.
    const displayedFps = range ? target : plan?.samplingFps;
    const label = plan && displayedFps != null ? `${formatFps(displayedFps)} FPS` : planning ? t('metrics.analyzing') : '-';
    const retained = plan
        ? t('sampling.retained', { percent: Number((Math.max(0, Math.min(1, plan.retentionRatio)) * 100).toFixed(1)) })
        : '-';
    const hasSlider = !!range && !collapsedRange && levels.length > 1;
    const sliderDisabled = disabled || !hasSlider;
    const materialLabel = `${material.name} · ${formatFps(material.video.fps)} FPS`;

    return <div className={`frame-sampling-control${disabled ? ' disabled' : ''}${planning ? ' planning' : ''}`} style={{ '--sampling-progress': `${progress}%`, '--sampling-default': `${defaultPosition}%` } as CSSProperties}>
        <div className="frame-sampling-head">
            <strong>{hasSlider ? <label htmlFor={id}>{t('inputs.fps')}</label> : <span>{t('inputs.fps')}</span>}</strong>
            <output {...(hasSlider ? { htmlFor: id } : {})} aria-live="polite">{label}</output>
            {materials.length > 1 && <div className="frame-sampling-material-picker">
                <span className="frame-sampling-material-measure" aria-hidden="true">{materialLabel}</span>
                <select aria-label={t('sampling.material')} title={materialLabel} disabled={disabled} value={material.id} onChange={event => setSelectedId(event.target.value)}>{materials.map(item => <option key={item.id} value={item.id}>{item.name} · {formatFps(item.video.fps)} FPS</option>)}</select>
                <ChevronDown className="frame-sampling-material-arrow" size={12} strokeWidth={1.8} aria-hidden="true" />
            </div>}
        </div>
        {hasSlider && <div className="frame-sampling-track-row">
            <span>{formatFps(range.minimumTargetFps)} FPS</span>
            <div className="frame-sampling-track"><i /><b aria-hidden="true" /><input id={id} type="range" min={0} max={levels.length - 1} step={1} value={index} disabled={sliderDisabled} aria-valuetext={label} onChange={event => {
                const selectedTarget = levels[Number(event.target.value)];
                if (selectedTarget == null) return;
                const selectedRemoval = Math.max(0, material.video.fps - selectedTarget);
                onChange(material.id, closeEnough(selectedTarget, defaultTarget) ? null : selectedRemoval);
            }} /></div>
            <span>{formatFps(range.maximumTargetFps)} FPS</span>
        </div>}
        <div className="frame-sampling-foot">
            <small>{t('sampling.removalWarning')}</small>
            <span>{retained}</span>
            <button type="button" disabled={disabled || material.removalFps == null} onClick={() => onChange(material.id, null)}>{t('sampling.reset')}</button>
        </div>
    </div>;
}
