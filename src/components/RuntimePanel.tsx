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

export function RuntimePanel({ snapshot, running }: { snapshot: RuntimeSnapshot; running: boolean }) {
  const { t, locale } = useI18n();
  const [now, setNow] = useState(Date.now);

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [running]);

  const unavailable = t("runtime.unavailable");
  const number = (value: number) => value.toLocaleString(locale, { maximumFractionDigits: 1 });
  const training = snapshot.training;
  const current = training?.iteration;
  const total = training?.total;
  const percent = current != null && total != null && total > 0
    ? Math.min(100, current / total * 100)
    : null;
  const outputAge = runtimeOutputAgeMs(snapshot, running, now);
  const resources = snapshot.resources;
  const stale = resources != null && now - Date.parse(resources.sampledAt) > 6_000;
  const speedVisible = running && outputAge <= 10_000 && snapshot.phase === "training";
  const remainingSeconds = liveTrainingRemainingSeconds(snapshot, running, now);

  return <div className="runtime-panel">
    <div className="runtime-heading">
      <strong>{t(phases[snapshot.phase] ?? "runtime.running")}</strong>
      <span>{running ? t("runtime.process", { pid: snapshot.processId }) : t("runtime.stopped")}</span>
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
    <p className={`runtime-output ${running && outputAge > 10_000 ? "quiet" : ""}`}>
      {t("runtime.output")}: {t("runtime.secondsAgo", { value: Math.floor(outputAge / 1_000) })}
      {running && outputAge > 10_000 && <span>{t("runtime.silent")}</span>}
    </p>
    <dl className={`runtime-metrics runtime-resources ${stale ? "stale" : ""}`}>
      <div><dt>{t("runtime.cpu")}</dt><dd>{resources?.cpuPercent != null ? `${number(resources.cpuPercent)}%` : !resources || resources.memoryBytes != null ? t("runtime.sampling") : unavailable}</dd></div>
      <div><dt>{t("runtime.memory")}</dt><dd>{resources?.memoryBytes != null ? `${number(resources.memoryBytes / 1024 / 1024)} MiB` : unavailable}</dd></div>
    </dl>
    {stale && <p className="runtime-note">{t("runtime.stale")}</p>}
    <div className="runtime-gpus">
      <small>{t("runtime.gpu")} · {t("runtime.gpuScope")}</small>
      {resources?.gpus.length ? resources.gpus.map((gpu) => <div className="runtime-gpu" key={gpu.uuid}>
        <span title={gpu.uuid}>{gpu.name}</span>
        <b>{gpu.utilizationPercent == null ? unavailable : `${number(gpu.utilizationPercent)}%`}</b>
        <span>{gpu.memoryUsedMib == null ? unavailable : `${number(gpu.memoryUsedMib)} MiB`} / {gpu.memoryTotalMib == null ? unavailable : `${number(gpu.memoryTotalMib)} MiB`}</span>
      </div>) : <p>{resources?.gpuStatus === "unsupported" ? t("runtime.unsupported") : resources ? unavailable : t("runtime.sampling")}</p>}
    </div>
    {(training || snapshot.device || Object.keys(snapshot.config).length > 0) && <details className="runtime-details">
      <summary>{t("runtime.details")}</summary>
      <dl>
        <div><dt>{t("runtime.device")}</dt><dd>{snapshot.device ?? unavailable}{snapshot.backend && ` · ${snapshot.backend}`}</dd></div>
        {training && <>
          <div><dt>{t("runtime.splats")}</dt><dd>{training.splatCount == null ? unavailable : number(training.splatCount)}</dd></div>
          <div><dt>PSNR / SSIM</dt><dd>{training.psnr?.toFixed(2) ?? "—"} / {training.ssim?.toFixed(4) ?? "—"}</dd></div>
        </>}
        {Object.entries(snapshot.config).filter(([key]) => key in settings).map(([key, value]) => <div key={key}><dt>{t(settings[key])}</dt><dd>{value}</dd></div>)}
      </dl>
    </details>}
  </div>;
}
