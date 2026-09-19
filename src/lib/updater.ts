import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";

export type UpdateDownloadProgress = {
  downloadedBytes: number;
  totalBytes: number | null;
};

/**
 * A large NSIS installer over a slow connection easily exceeds two minutes, and
 * an aborted download looks like a broken updater to the user.
 */
const DOWNLOAD_TIMEOUT_MS = 30 * 60 * 1000;
const CHECK_TIMEOUT_MS = 15_000;

const inTauri = () => "__TAURI_INTERNALS__" in window;

/**
 * Only the maintainer-signed release job sets this flag. Developer builds,
 * forks, and ordinary CI bundles must never query the official feed, because
 * they ship without the matching public key and would otherwise show a
 * permanent "update check failed" state.
 */
export function isUpdaterEnabled(): boolean {
  return import.meta.env.VITE_UPDATER_ENABLED === "true";
}

/**
 * A binary compiled without the `updater` Cargo feature registers no updater
 * commands, so every call fails even though the frontend flag was set. Such a
 * build cannot update itself and must behave like a disabled updater instead of
 * reporting a broken feed on every start.
 */
export function isUpdaterPluginMissing(error: unknown): boolean {
  const message = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return /not[ _]allowed|not found|unknown command|not registered|not initialized/i.test(message);
}

/**
 * Checks the configured, signed release feed. The build flag is the single
 * source of truth, so a browser development session without the flag behaves as
 * if no update were available instead of reporting a broken feed.
 */
export async function checkForAppUpdate(): Promise<Update | null> {
  if (!isUpdaterEnabled() || !inTauri()) return null;
  return check({ timeout: CHECK_TIMEOUT_MS });
}

/** Downloads the signed package without installing it yet. */
export async function downloadAppUpdate(
  update: Update,
  onProgress: (progress: UpdateDownloadProgress) => void,
): Promise<void> {
  let downloadedBytes = 0;
  let totalBytes: number | null = null;

  await update.download((event: DownloadEvent) => {
    if (event.event === "Started") {
      totalBytes = event.data.contentLength ?? null;
      onProgress({ downloadedBytes, totalBytes });
      return;
    }
    if (event.event === "Progress") {
      downloadedBytes += event.data.chunkLength;
      onProgress({ downloadedBytes, totalBytes });
    }
  }, { timeout: DOWNLOAD_TIMEOUT_MS });
}

/**
 * Runs the verified native installer. On Windows this exits the application, so
 * callers must confirm that no pipeline is still writing project files.
 */
export async function installAppUpdate(update: Update): Promise<void> {
  await update.install({ restartAfterInstall: true });
}

/** Releases a downloaded package that will not be installed. */
export async function discardAppUpdate(update: Update): Promise<void> {
  try {
    await update.close();
  } catch {
    // The native resource may already be gone; discarding must never throw.
  }
}
