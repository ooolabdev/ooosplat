import { closeSync, mkdirSync, mkdtempSync, openSync, readFileSync, renameSync, rmSync, watch, type FSWatcher } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('generated cache dev-watch policy', () => {
  it('excludes both pipeline diagnostics and Rust validation workspaces from HMR', () => {
    const config = readFileSync(new URL('../../vite.config.ts', import.meta.url), 'utf8');
    expect(config).toContain('"**/.cache/**"');
    expect(config).toContain('"**/src-tauri/**"');
  });

  it.skipIf(process.platform !== 'win32')('per-directory watchers can deny renames, and releasing them restores normal I/O', async () => {
    const parent = resolve('src-tauri/target');
    mkdirSync(parent, { recursive: true });
    const root = mkdtempSync(resolve(parent, 'watch-policy-'));
    const source = resolve(root, 'brush');
    const images = resolve(source, 'dataset/images');
    const destination = resolve(root, 'archived-brush');
    mkdirSync(images, { recursive: true });
    closeSync(openSync(resolve(images, 'fixture'), 'wx'));
    const watchers: FSWatcher[] = [];
    const release = async () => {
      await Promise.all(watchers.splice(0).map(watcher => new Promise<void>(done => {
        watcher.once('close', done);
        watcher.close();
      })));
    };
    try {
      renameSync(source, destination);
      renameSync(destination, source);
      for (const directory of [source, resolve(source, 'dataset'), images]) {
        watchers.push(watch(directory, { recursive: false }, () => undefined));
      }
      expect(() => renameSync(source, destination)).toThrow();
      await release();
      expect(() => renameSync(source, destination)).not.toThrow();
    } finally {
      await release();
      // Only the directory created by this test can ever reach this cleanup.
      rmSync(root, { recursive: true, force: true });
    }
  });
});
