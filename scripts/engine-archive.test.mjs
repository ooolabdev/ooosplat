import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const helper = fileURLToPath(new URL("./expand-engine-archive.ps1", import.meta.url));
const setup = readFileSync(new URL("./setup-engines.ps1", import.meta.url), "utf8");

test("Windows setup verifies the pinned archive before extraction and the staging anchor before installation", () => {
  const hash = setup.indexOf("if ($archiveHash -ne $engine.archiveSha256)");
  const extraction = setup.indexOf("expand-engine-archive.ps1");
  const anchor = setup.indexOf("if ($anchors.Count -ne 1)");
  const installation = setup.indexOf("$destination = Assert-WorkspaceDestination");
  assert.ok(hash >= 0 && extraction > hash && anchor > extraction && installation > anchor);
  assert.doesNotMatch(setup, /\bExpand-Archive\s+-/);
  const script = readFileSync(helper, "utf8");
  assert.match(script, /ZipFile\]::ExtractToDirectory\(\s*\$[\w]+\s*,\s*\$[\w]+\s*\)/);
});

function powershell(shell, script, variables = {}) {
  return spawnSync(shell, ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", Buffer.from(script, "utf16le").toString("base64")], {
    encoding: "utf8", windowsHide: true, timeout: 30_000,
    env: { ...process.env, OOOSPLAT_TEST_ARCHIVE_HELPER: helper, ...variables },
  });
}

function succeeded(result) {
  assert.ifError(result.error);
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
}

function extract(shell, archive, destination) {
  return powershell(shell, `
    $ErrorActionPreference = 'Stop'
    function global:Expand-Archive { throw 'The incompatible Archive module must not be called.' }
    & $env:OOOSPLAT_TEST_ARCHIVE_HELPER -ArchivePath $env:OOOSPLAT_TEST_ARCHIVE -DestinationPath $env:OOOSPLAT_TEST_DESTINATION
  `, { OOOSPLAT_TEST_ARCHIVE: archive, OOOSPLAT_TEST_DESTINATION: destination });
}

const shells = [
  { name: "Windows PowerShell 5.1", command: path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), optional: false },
  { name: "PowerShell 7", command: "pwsh", optional: true },
];

for (const shell of shells) {
  test(`${shell.name}: extracts ZIP without Expand-Archive and preserves data on failure`, { skip: process.platform !== "win32" && "Windows archive compatibility test", timeout: 120_000 }, async t => {
    const availability = powershell(shell.command, "$PSVersionTable.PSVersion.ToString()");
    if (shell.optional && availability.error?.code === "ENOENT") {
      t.skip("PowerShell 7 is not installed on this runner");
      return;
    }
    succeeded(availability);
    assert.match(availability.stdout.trim(), shell.optional ? /^7\./ : /^5\.1\./);

    const fixture = mkdtempSync(path.join(tmpdir(), "ooosplat-archive-test-"));
    t.after(() => rmSync(fixture, { recursive: true, force: true }));
    const source = path.join(fixture, "source [zip] 图片");
    mkdirSync(path.join(source, "bin"), { recursive: true });
    writeFileSync(path.join(source, "bin", "ffmpeg.exe"), "synthetic engine fixture");
    writeFileSync(path.join(source, "bin", "说明 [1].txt"), "unchanged metadata\n");
    const archive = path.join(fixture, "archive [1] 图片.zip");
    succeeded(powershell(shell.command, `
      $ErrorActionPreference = 'Stop'
      Add-Type -AssemblyName System.IO.Compression.FileSystem
      [System.IO.Compression.ZipFile]::CreateFromDirectory($env:OOOSPLAT_TEST_SOURCE, $env:OOOSPLAT_TEST_ARCHIVE)
    `, { OOOSPLAT_TEST_SOURCE: source, OOOSPLAT_TEST_ARCHIVE: archive }));
    const archiveBefore = readFileSync(archive);
    const installed = path.join(fixture, "existing-installation");
    mkdirSync(installed);
    writeFileSync(path.join(installed, "ffmpeg.exe"), "existing installation must remain unchanged");

    await t.test("normal ZIP, literal paths, and existing empty staging directory", () => {
      const destination = path.join(fixture, "stage [1] 图片");
      mkdirSync(destination);
      succeeded(extract(shell.command, archive, destination));
      assert.equal(readFileSync(path.join(destination, "bin", "ffmpeg.exe"), "utf8"), "synthetic engine fixture");
      assert.equal(readFileSync(path.join(destination, "bin", "说明 [1].txt"), "utf8"), "unchanged metadata\n");
      assert.deepEqual(readFileSync(archive), archiveBefore);
    });

    await t.test("nonempty destination is rejected before unrelated files are added or overwritten", () => {
      const result = extract(shell.command, archive, installed);
      assert.ifError(result.error);
      assert.notEqual(result.status, 0);
      assert.deepEqual(readdirSync(installed), ["ffmpeg.exe"]);
      assert.equal(readFileSync(path.join(installed, "ffmpeg.exe"), "utf8"), "existing installation must remain unchanged");
    });

    await t.test("corrupt ZIP fails without touching an existing installation", () => {
      const corrupt = path.join(fixture, "corrupt.zip");
      writeFileSync(corrupt, "not a ZIP archive");
      const destination = path.join(fixture, "broken-stage");
      mkdirSync(destination);
      const result = extract(shell.command, corrupt, destination);
      assert.ifError(result.error);
      assert.notEqual(result.status, 0);
      assert.deepEqual(readdirSync(destination), []);
      assert.equal(readFileSync(path.join(installed, "ffmpeg.exe"), "utf8"), "existing installation must remain unchanged");
    });

    await t.test("directory junction destination cannot redirect extraction", () => {
      const actual = path.join(fixture, "junction-target");
      const redirected = path.join(fixture, "junction-stage");
      mkdirSync(actual);
      symlinkSync(actual, redirected, "junction");
      const result = extract(shell.command, archive, redirected);
      assert.ifError(result.error);
      assert.notEqual(result.status, 0);
      assert.deepEqual(readdirSync(actual), []);
    });

    await t.test("ZIP entries cannot escape staging", () => {
      const unsafe = path.join(fixture, "traversal.zip");
      succeeded(powershell(shell.command, `
        $ErrorActionPreference = 'Stop'
        Add-Type -AssemblyName System.IO.Compression
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $zip = [System.IO.Compression.ZipFile]::Open($env:OOOSPLAT_TEST_ARCHIVE, [System.IO.Compression.ZipArchiveMode]::Create)
        try {
          $entry = $zip.CreateEntry('../escape.txt')
          $writer = New-Object System.IO.StreamWriter($entry.Open())
          try { $writer.Write('must not escape') } finally { $writer.Dispose() }
        } finally { $zip.Dispose() }
      `, { OOOSPLAT_TEST_ARCHIVE: unsafe }));
      const destination = path.join(fixture, "traversal-stage");
      mkdirSync(destination);
      const result = extract(shell.command, unsafe, destination);
      assert.ifError(result.error);
      assert.notEqual(result.status, 0);
      assert.equal(existsSync(path.join(fixture, "escape.txt")), false);
      assert.deepEqual(readdirSync(destination), []);
    });
  });
}
