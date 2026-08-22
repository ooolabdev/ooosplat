export const elapsedSince = (startedAt: number, now: number) => Math.max(0, now - startedAt);

export function startElapsedTicker(
  startedAt: number,
  onTick: (elapsedMs: number) => void,
  now: () => number = Date.now,
  intervalMs = 1000,
) {
  const update = () => onTick(elapsedSince(startedAt, now()));
  update();
  const timer = globalThis.setInterval(update, intervalMs);
  return () => globalThis.clearInterval(timer);
}
