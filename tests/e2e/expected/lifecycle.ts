import assert from 'node:assert/strict';

/** Independent observations only. No production planner/parser imports. */
export interface SourceContract { id: string; samplingFps: number; initialFrames: number; rescueMaxFrames: number }
export function assertSourceContract(actual: SourceContract[], expected: SourceContract[]) {
  if(actual.length!==expected.length)throw new Error('Unexpected source count');
  for(const saved of expected){const found=actual.find(s=>s.id===saved.id);if(!found)throw new Error('Unexpected source identity');
    if(found.samplingFps!==saved.samplingFps||found.initialFrames!==saved.initialFrames)throw new Error('Borrowed source sampling plan');
    if(found.rescueMaxFrames!==saved.rescueMaxFrames)throw new Error('Refreshed or transferred source budget');}
}
export function assertInitialization(actual:string[],expected:string[]) {
  if(actual.length!==expected.length||actual.some((hash,n)=>hash!==expected[n]))throw new Error('Wrong frozen initialization identity');
}
export function assertFrameMapping(actual:{sourceIndex:number;pts:number}[],expected:{sourceIndex:number;pts:number}[]) {
  if(actual.length!==expected.length||actual.some((frame,n)=>frame.sourceIndex!==expected[n].sourceIndex||frame.pts!==expected[n].pts))throw new Error('Wrong source frame or PTS');
}
export function assertTrustedCandidate(candidate:{complete:boolean;valid:boolean;identity:string},expectedIdentity:string) {
  if(!candidate.complete||!candidate.valid||candidate.identity!==expectedIdentity)throw new Error('Untrusted partial candidate');
}

export const reshootBaseFiles = [
  'work/colmap/base-database.db',
  'work/colmap/base-model/cameras.bin',
  'work/colmap/base-model/images.bin',
  'work/colmap/base-model/points3D.bin',
];

interface PreparedFile {
  relativePath: string;
  fileType: string;
  fileSize: number;
  modifiedNanos?: number | null;
  sha256?: string | null;
}

interface PreparedManifest {
  artifactIdentity: string;
  upstreamIdentity: string;
  semanticIdentity?: string | null;
  expectedCount: number;
  files: PreparedFile[];
}

export function assertPreparedBaseRebind(before: PreparedManifest, after: PreparedManifest) {
  assert.match(after.artifactIdentity, /^[a-f\d]{64}$/, 'Prepared artifact identity');
  assert.notEqual(after.artifactIdentity, before.artifactIdentity, 'Expected a prepared identity rebind');
  assert.equal(after.upstreamIdentity, before.upstreamIdentity, 'Prepared upstream identity');
  assert.equal(after.semanticIdentity, before.semanticIdentity, 'Prepared semantic identity');
  assert.equal(after.expectedCount, before.expectedCount, 'Prepared expected count');
  assert.deepEqual(after.files.map(file => file.relativePath), before.files.map(file => file.relativePath), 'Prepared file inventory');

  const baseFiles = before.files.filter(file => reshootBaseFiles.includes(file.relativePath));
  assert.deepEqual(baseFiles.map(file => file.relativePath).sort(), [...reshootBaseFiles].sort(), 'Prepared base file inventory');

  let timestampChanged = false;
  for (const [index, original] of before.files.entries()) {
    const current = after.files[index];
    if (reshootBaseFiles.includes(original.relativePath)) {
      assert.equal(original.fileType, 'regularFile', `Prepared base file type: ${original.relativePath}`);
      const sha256 = original.sha256;
      assert.ok(sha256 && /^[a-f\d]{64}$/.test(sha256), `Prepared base SHA-256: ${original.relativePath}`);
      assert.ok(typeof current.modifiedNanos === 'number' && Number.isFinite(current.modifiedNanos), `Prepared base modification time: ${original.relativePath}`);
      assert.deepEqual(current, { ...original, modifiedNanos: current.modifiedNanos }, `Prepared base file content: ${original.relativePath}`);
      timestampChanged ||= current.modifiedNanos !== original.modifiedNanos;
    } else {
      assert.deepEqual(current, original, `Unchanged prepared file: ${original.relativePath}`);
    }
  }
  assert.ok(timestampChanged, 'Prepared identity changed without a restored base timestamp change');
  return baseFiles;
}

export function assertNewCacheArchive(before: readonly string[], after: readonly string[]) {
  const created = after.filter(name => !before.includes(name));
  assert.equal(created.length, 1, 'Expected one new cache archive');
  return created[0];
}

// Reviewed tables: 300 source frames, Fast 9 FPS candidate pool; no production resolver.
export const default300At6 = [0, 3, 10, 13, 20, 23, 30, 33, 40, 43, 50, 53, 60, 63, 70, 73, 80, 83, 90, 93, 100, 103, 110, 113, 120, 123, 130, 133, 140, 143, 150, 153, 160, 163, 170, 173, 180, 183, 190, 193, 200, 203, 210, 213, 220, 223, 230, 233, 240, 243, 250, 253, 260, 263, 270, 273, 280, 283, 290, 293];
export const manual300At7 = [0, 6, 13, 20, 33, 40, 46, 60, 66, 73, 80, 93, 100, 106, 120, 126, 133, 140, 153, 160, 166, 180, 186, 193, 200, 213, 220, 226, 240, 246, 253, 260, 273, 280, 286];
