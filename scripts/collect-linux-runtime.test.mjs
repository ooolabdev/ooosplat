import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { collectLinuxRuntime, isLinuxRuntimeElf, linuxLddDependency } from "./collect-linux-runtime.mjs";

function elf(type = 3, endian = 1, bits = 2) {
  const header = Buffer.alloc(64);
  Buffer.from([0x7f, 0x45, 0x4c, 0x46, bits, endian, 1]).copy(header);
  if (endian === 1) header.writeUInt16LE(type, 16);
  else header.writeUInt16BE(type, 16);
  return header;
}

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ooosplat-linux-closure-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const stage = path.join(root, "stage"), installed = path.join(root, "installed"), cuda = path.join(root, "cuda");
  const write = (relative, content) => {
    const file = path.join(stage, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, content);
    return file;
  };
  const binary = write("bin/colmap", elf(2));
  fs.mkdirSync(path.join(stage, "lib"));
  return { root, stage, installed, cuda, binary, write };
}

test("PoseLib archives and non-runtime development files never enter ldd and remain for package trimming", t => {
  const f = fixture(t);
  const development = [
    ["lib/libPoseLib.a", "!<arch>\n"], ["lib/libfaiss.a", "!<arch>\n"],
    ["lib/kernel.o", elf(1)], ["lib/cmake/colmap-config.cmake", "set(COLMAP_FOUND TRUE)"],
    ["lib/pkgconfig/colmap.pc", "Name: colmap"], ["lib/build-metadata.json", "{}"],
  ].map(([name, contents]) => f.write(name, contents));
  const library = f.write("lib/libOpenImageIO.so.3.1", elf());
  const scanned = [];
  const origins = collectLinuxRuntime(f.stage, f.installed, f.cuda, { execute: (command, [file]) => {
    assert.equal(command, "ldd");
    scanned.push(file);
    if (development.includes(file)) return { status: 1, stdout: "", stderr: "not a dynamic executable" };
    return { status: 0, stdout: "", stderr: "" };
  } });
  assert.deepEqual(new Set(scanned), new Set([f.binary, library]));
  assert.deepEqual(origins, []);
  for (const file of [...development, library]) assert.ok(fs.existsSync(file), `Collector must not delete ${file}`);
  assert.ok(fs.existsSync(path.join(f.stage, "DEPENDENCY-ORIGINS.json")));
});

test("unnamed dynamically loaded ELF plugins and both ELF byte orders retain dependency checks", t => {
  const f = fixture(t);
  const plugins = [
    f.write("lib/plugins/image_decoder", elf(3)),
    f.write("lib/plugins/libdecoder.so", elf(3, 2, 1)),
  ];
  const scanned = [];
  collectLinuxRuntime(f.stage, f.installed, f.cuda, { execute: (_command, [file]) => {
    scanned.push(file);
    return { status: 0, stdout: "", stderr: "" };
  } });
  for (const plugin of plugins) {
    assert.ok(scanned.includes(plugin));
    assert.ok(fs.existsSync(plugin));
  }
});

test("invalid shared libraries and malformed ELF headers fail closed without deleting files", t => {
  const f = fixture(t);
  for (const [name, contents] of [
    ["lib/text.so", "not an ELF library"], ["lib/truncated.so", Buffer.from([0x7f, 0x45, 0x4c, 0x46])],
    ["lib/object.so", elf(1)], ["lib/invalid-endian", elf(3, 3)],
  ]) {
    const file = f.write(name, contents);
    assert.throws(() => isLinuxRuntimeElf(file), /ELF/);
    assert.ok(fs.existsSync(file));
  }
  assert.throws(() => collectLinuxRuntime(f.stage, f.installed, f.cuda, { execute: () => {
    assert.fail("Invalid shared libraries must fail before the closure is accepted");
  } }), /ELF/);
});

test("failed shared-library ldd execution and unresolved dependencies still block collection", t => {
  for (const failure of [
    { status: 1, stdout: "", stderr: "not a dynamic executable" },
    { status: 0, stdout: "libmissing.so.1 => not found\n", stderr: "" },
    { status: null, stdout: "", stderr: "", error: new Error("ldd could not start") },
  ]) {
    const f = fixture(t), library = f.write("lib/librequired.so", elf());
    assert.throws(() => collectLinuxRuntime(f.stage, f.installed, f.cuda, { execute: (_command, [file]) => {
      return file === f.binary ? { status: 0, stdout: "", stderr: "" } : failure;
    } }), /Unresolved dependency/);
    assert.ok(fs.existsSync(library));
    assert.equal(fs.existsSync(path.join(f.stage, "DEPENDENCY-ORIGINS.json")), false);
  }
});

test("ldd resolves Chinese and spaced library source paths without silently dropping a dependency", () => {
  assert.deepEqual(linuxLddDependency("\tlibdecode.so.1 => /home/中文工程/build cache/libdecode.so.1 (0x00007f00)"), {
    name: "libdecode.so.1", source: "/home/中文工程/build cache/libdecode.so.1",
  });
  assert.deepEqual(linuxLddDependency("libcodec.so.2 => /usr/lib/libcodec.so.2 (0x00007f00)"), {
    name: "libcodec.so.2", source: "/usr/lib/libcodec.so.2",
  });
  assert.equal(linuxLddDependency("libmissing.so.1 => not found"), null);
  assert.equal(linuxLddDependency("linux-vdso.so.1 (0x00007f00)"), null);
});

test("shared-library and plugin dependencies are copied transitively with origins preserved", {
  skip: process.platform !== "linux",
}, t => {
  const f = fixture(t);
  const plugin = f.write("lib/plugins/image_decoder", elf());
  const dependencies = path.join(f.root, "中文 upstream");
  fs.mkdirSync(dependencies);
  const first = path.join(dependencies, "libdecode.so.1"), second = path.join(dependencies, "libcodec.so.2");
  fs.writeFileSync(first, elf());
  fs.writeFileSync(second, elf());
  const copiedFirst = path.join(f.stage, "lib/libdecode.so.1"), copiedSecond = path.join(f.stage, "lib/libcodec.so.2");
  const scanned = [];
  const origins = collectLinuxRuntime(f.stage, f.installed, f.cuda, { execute: (_command, [file]) => {
    scanned.push(file);
    let stdout = "";
    if (file === plugin) stdout = `libdecode.so.1 => ${first} (0x00007f00)\nlibcuda.so.1 => /host/libcuda.so.1 (0x00007f00)\n`;
    if (file === copiedFirst) stdout = `libcodec.so.2 => ${second} (0x00007f00)\nlibc.so.6 => /host/libc.so.6 (0x00007f00)\n`;
    return { status: 0, stdout, stderr: "" };
  } });
  for (const [source, copied] of [[first, copiedFirst], [second, copiedSecond]]) {
    assert.deepEqual(fs.readFileSync(copied), fs.readFileSync(source));
    assert.ok(scanned.includes(copied), "Each copied dependency must be inspected transitively");
  }
  assert.deepEqual(origins, [
    { file: "lib/libdecode.so.1", source: fs.realpathSync(first) },
    { file: "lib/libcodec.so.2", source: fs.realpathSync(second) },
  ]);
  assert.equal(fs.existsSync(path.join(f.stage, "lib/libcuda.so.1")), false);
  assert.equal(fs.existsSync(path.join(f.stage, "lib/libc.so.6")), false);
});
