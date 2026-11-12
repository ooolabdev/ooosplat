import { useEffect, useState } from "react";
import { useI18n, type TranslationKey } from "../i18n";
import { formatClockDuration, liveTrainingRemainingSeconds, runtimeOutputAgeMs } from "../lib/runtimeEstimate";
import type { RuntimeSnapshot } from "../types/pipeline";

const phases: Record<string, TranslationKey> = {
  starting: "runtime.starting",
  running: "runtime.running",
  gpuInitialization: "runtime.gpuInitialization",
  loading: "runtime.loading",
  splatInitialization: "runtime.splatInitialization",
  trainerPreparation: "runtime.trainerPreparation",
  training: "runtime.training",
  evaluation: "runtime.evaluation",
  lodPreparation: "runtime.lodPreparation",
  exporting: "runtime.exporting",
  finishing: "runtime.finishing",
};

const settings: Record<string, TranslationKey> = {
  train_iters: "runtime.configSteps",
  total_iters: "runtime.configTotal",
  start_iter: "runtime.configStart",
  lod_levels: "runtime.configLod",
  lod_refine_steps: "runtime.configLodSteps",
  max_resolution: "runtime.configResolution",
  max_splats: "runtime.configSplats",
  growth_start_iter: "runtime.configGrowthStart",
  growth_stop_iter: "runtime.configGrowthStop",
  refine_every: "runtime.configRefine",
  seed: "runtime.configSeed",
  eval_every: "runtime.configEval",
  every: "runtime.configExport",
};

export function RuntimePanel({ snapshot, running, waitingMessage }: { snapshot: RuntimeSnapshot | null; running: boolean; waitingMessage?: string | null }) {
  const { t, locale } = useI18n();
  const [now, setNow] = useState(Date.now);

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [running]);

  const unavailable = t("runtime.unavailable");
  const number = (value: number) => value.toLocaleString(locale, { maximumFractionDigits: 1 });
  const training = snapshot?.training;
  const current = training?.iteration;
  const total = training?.total;
  const percent = current != null && total != null && total > 0
    ? Math.min(100, current / total * 100)
    : null;
  const stopped = snapshot?.phase === 'stopped';
  const processRunning = running && snapshot != null && !stopped;
  const outputAge = snapshot ? runtimeOutputAgeMs(snapshot, processRunning, now) : 0;
  // A resource response can arrive after its process has exited. It must not
  // be presented as the new process's sample, even during a same-stage switch.
  const resources = !stopped && snapshot?.resources?.processId === snapshot?.processId ? snapshot?.resources : null;
  const pendingResources = t(stopped ? 'runtime.stopped' : 'runtime.sampling');
  const stale = resources != null && now - Date.parse(resources.sampledAt) > 6_000;
  const speedVisible = processRunning && outputAge <= 10_000 && snapshot?.phase === "training";
  const remainingSeconds = liveTrainingRemainingSeconds(snapshot, processRunning, now);

  return <div className="runtime-panel" role={!snapshot ? 'status' : undefined} aria-busy={running && !stopped && !resources}>
    <div className="runtime-heading">
      <strong>{t(!snapshot ? 'runtime.waiting' : snapshot.phase === 'stopped' ? 'runtime.stopped' : phases[snapshot.phase] ?? "runtime.running")}</strong>
      <span>{snapshot ? processRunning ? t("runtime.process", { pid: snapshot.processId }) : t("runtime.stopped") : '—'}</span>
    </div>
    {training && <>
      <div className="runtime-step-heading">
        <span>{t("runtime.steps")}</span>
        <b>{current != null && total != null ? `${number(current)} / ${number(total)}` : "—"}{training.lod > 0 && <small> · LOD {training.lod}</small>}</b>
        <span>{percent == null ? "—" : `${percent.toFixed(1)}%`}</span>
      </div>
      <progress className="runtime-track" max={100} {...(percent == null ? {} : { value: percent })} aria-label={t("runtime.steps")} />
      <dl className="runtime-metrics">
        <div><dt>{t("runtime.speed")}</dt><dd>{speedVisible && training.stepsPerSecond != null ? t("runtime.stepRate", { value: number(training.stepsPerSecond) }) : "—"}</dd></div>
        <div><dt>{t("runtime.remaining")}</dt><dd>{remainingSeconds != null ? formatClockDuration(remainingSeconds) : "—"}</dd></div>
      </dl>
    </>}
    <p className={`runtime-output ${processRunning && outputAge > 10_000 ? "quiet" : ""}`}>
      {snapshot ? <>{t("runtime.output")}: {t("runtime.secondsAgo", { value: Math.floor(outputAge / 1_000) })}
        {processRunning && outputAge > 10_000 && <span>{t("runtime.silent")}</span>}</>
        : waitingMessage || t('runtime.waitingHint')}
    </p>
    <dl className={`runtime-metrics runtime-resources ${stale ? "stale" : ""}`}>
      <div><dt>{t("runtime.cpu")}</dt><dd>{resources?.cpuPercent != null ? `${number(resources.cpuPercent)}%` : !resources || resources.memoryBytes != null ? pendingResources : unavailable}</dd></div>
      <div><dt>{t("runtime.memory")}</dt><dd>{resources?.memoryBytes != null ? `${number(resources.memoryBytes / 1024 / 1024)} MiB` : !resources ? pendingResources : unavailable}</dd></div>
    </dl>
    {stale && <p className="runtime-note">{t("runtime.stale")}</p>}
    <div className="runtime-gpus">
      <small>{t("runtime.gpu")} · {t("runtime.gpuScope")}</small>
      {resources?.gpus.length ? resources.gpus.map((gpu) => <div className="runtime-gpu" key={gpu.uuid}>
        <span title={gpu.uuid}>{gpu.name}</span>
        <b>{gpu.utilizationPercent == null ? unavailable : `${number(gpu.utilizationPercent)}%`}</b>
        <span>{gpu.memoryUsedMib == null ? unavailable : `${number(gpu.memoryUsedMib)} MiB`} / {gpu.memoryTotalMib == null ? unavailable : `${number(gpu.memoryTotalMib)} MiB`}</span>
      </div>) : <div className="runtime-gpu runtime-gpu-placeholder"><span>{resources?.gpuStatus === "unsupported" ? t("runtime.unsupported") : resources ? unavailable : pendingResources}</span><b>—</b><span>—</span></div>}
    </div>
    {(training || snapshot?.device || Object.keys(snapshot?.config ?? {}).length > 0) && <details className="runtime-details">
      <summary>{t("runtime.details")}</summary>
      <dl>
        <div><dt>{t("runtime.device")}</dt><dd>{snapshot?.device ?? unavailable}{snapshot?.backend && ` · ${snapshot.backend}`}</dd></div>
        {training && <>
          <div><dt>{t("runtime.splats")}</dt><dd>{training.splatCount == null ? unavailable : number(training.splatCount)}</dd></div>
          <div><dt>PSNR / SSIM</dt><dd>{training.psnr?.toFixed(2) ?? "—"} / {training.ssim?.toFixed(4) ?? "—"}</dd></div>
        </>}
        {Object.entries(snapshot?.config ?? {}).filter(([key]) => key in settings).map(([key, value]) => <div key={key}><dt>{t(settings[key])}</dt><dd>{value}</dd></div>)}
      </dl>
    </details>}
  </div>;
}
