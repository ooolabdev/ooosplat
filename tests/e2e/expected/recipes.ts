// Reviewed independent expectations. Never import the production resolver.
export const ordinary = {
  fast: { steps: 8000, refine: 200, gradient: 0.0022, fraction: 0.11, stop: 7000, coefficient: 0.2, sfmSize: 1200, features: 4096, photoCap: 6192 },
  balanced: { steps: 15000, refine: 200, gradient: 0.002, fraction: 0.12, stop: 13000, coefficient: 0.4, sfmSize: 1600, features: 8192, photoCap: 12385 },
  high: { steps: 30000, refine: 200, gradient: 0.0015, fraction: 0.15, stop: 24000, coefficient: 0.5, sfmSize: 3200, features: 16384, photoCap: 15481 },
} as const;
export const panorama = {
  fast: { steps: 12000, gradient: 0.0018, fraction: 0.2, stop: 10000, cap: 1000000 },
  balanced: { steps: 30000, gradient: 0.0012, fraction: 0.25, stop: 25000, cap: 2000000 },
  high: { steps: 45000, gradient: 0.001, fraction: 0.3, stop: 38000, cap: 1280 },
} as const;
/** Exact option set: absent LR/noise overrides are part of the contract too. */
export function brushOptions(quality: keyof typeof ordinary, erp = false) {
  const r = erp ? panorama[quality] : ordinary[quality];
  return {
    '--total-train-iters': String(r.steps), '--max-resolution': erp ? '32' : '96', '--refine-every': '200',
    '--max-splats': String(erp ? panorama[quality].cap : ordinary[quality].photoCap),
    '--growth-grad-threshold': String(r.gradient), '--growth-select-fraction': String(r.fraction),
    '--growth-stop-iter': String(r.stop), '--export-every': String(r.steps), '--export-name': 'final.ply.tmp',
    ...(erp ? { '--growth-start-iter': '0', '--sh-degree': '3' } : {}),
  };
}
export function flags(args: string[]) {
  const output: Record<string, string> = {};
  for (let i = 0; i < args.length; i++) if (args[i].startsWith('--')) {
    if (args[i] in output) throw new Error(`Duplicate command flag: ${args[i]}`);
    if (args[i + 1] == null || args[i + 1].startsWith('--')) throw new Error(`Missing command value: ${args[i]}`);
    output[args[i]] = args[++i];
  }
  return output;
}
