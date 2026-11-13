import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const rootDocuments = [
  "README.md",
  "README_EN.md",
  "README_JA.md",
  "ROADMAP.md",
  "ROADMAP_EN.md",
  "CONTRIBUTING.md",
  "TRADEMARK_POLICY.md",
  "GENERATED_OUTPUTS.md",
];

const decoder = new TextDecoder("utf-8", { fatal: true });
const errors = [];

async function collectMarkdown(directory) {
  const result = [];
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch (error) {
    if (error?.code === "ENOENT") return result;
    throw error;
  }
  for (const entry of entries) {
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) result.push(...(await collectMarkdown(target)));
    else if (entry.isFile() && entry.name.toLowerCase().endsWith(".md")) result.push(target);
  }
  return result;
}

function relative(file) {
  return path.relative(root, file).split(path.sep).join("/");
}

function stripFencedCode(text) {
  const lines = text.split("\n");
  let fence = null;
  return lines
    .map((line) => {
      const match = line.match(/^\s*(`{3,}|~{3,})/);
      if (match) {
        const marker = match[1][0];
        if (fence === null) fence = marker;
        else if (fence === marker) fence = null;
        return "";
      }
      return fence === null ? line : "";
    })
    .join("\n");
}

function githubSlug(value) {
  return value
    .trim()
    .toLowerCase()
    .replace(/<[^>]*>/g, "")
    .replace(/[`*_~]/g, "")
    .replace(/[^\p{L}\p{N}\s_-]/gu, "")
    .replace(/\s/g, "-");
}

function anchors(text) {
  const result = new Set();
  const counts = new Map();
  for (const line of stripFencedCode(text).split("\n")) {
    const match = line.match(/^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/);
    if (!match) continue;
    const base = githubSlug(match[1]);
    if (!base) continue;
    const count = counts.get(base) ?? 0;
    counts.set(base, count + 1);
    result.add(count === 0 ? base : `${base}-${count}`);
  }
  return result;
}

function lineNumber(text, offset) {
  return text.slice(0, offset).split("\n").length;
}

function externalTarget(target) {
  return /^(?:https?:|mailto:|tel:|data:)/i.test(target) || target.startsWith("//");
}

async function readableText(file) {
  const bytes = await readFile(file);
  return decoder.decode(bytes);
}

async function exists(file) {
  try {
    await stat(file);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

const sources = [
  ...rootDocuments.map((file) => path.join(root, file)),
  ...(await collectMarkdown(path.join(root, "docs"))),
];
const documentCache = new Map();

for (const file of sources) {
  let text;
  try {
    text = await readableText(file);
  } catch (error) {
    errors.push(`${relative(file)}: invalid or unreadable UTF-8 (${error.message})`);
    continue;
  }
  documentCache.set(path.resolve(file), text);
  if (text.includes("\0")) errors.push(`${relative(file)}: contains a NUL byte`);
  if (!text.endsWith("\n")) errors.push(`${relative(file)}: must end with a newline`);
  text.split("\n").forEach((line, index) => {
    if (/[ \t]+$/.test(line)) errors.push(`${relative(file)}:${index + 1}: trailing whitespace`);
  });
}

for (const file of sources) {
  const original = documentCache.get(path.resolve(file));
  if (original === undefined) continue;
  const text = stripFencedCode(original);
  const references = [];
  const linkPattern = /!?\[[^\]]*\]\(([^)]+)\)/g;
  for (const match of text.matchAll(linkPattern)) {
    references.push({ target: match[1], offset: match.index });
  }
  const htmlReferencePattern = /<(?:a|img)\b[^>]*?\b(?:href|src)\s*=\s*(["'])(.*?)\1/gi;
  for (const match of text.matchAll(htmlReferencePattern)) {
    references.push({ target: match[2], offset: match.index });
  }

  for (const reference of references) {
    let target = reference.target.trim();
    if (target.startsWith("<") && target.endsWith(">")) target = target.slice(1, -1);
    target = target.replace(/\s+(?:"[^"]*"|'[^']*')\s*$/, "");
    if (!target || externalTarget(target)) continue;

    const hashIndex = target.indexOf("#");
    const rawPath = hashIndex >= 0 ? target.slice(0, hashIndex) : target;
    const fragment = hashIndex >= 0 ? target.slice(hashIndex + 1) : "";
    let decodedPath;
    let decodedFragment;
    try {
      decodedPath = decodeURIComponent(rawPath.split("?")[0]);
      decodedFragment = decodeURIComponent(fragment);
    } catch {
      errors.push(`${relative(file)}:${lineNumber(text, reference.offset)}: invalid URL encoding in ${target}`);
      continue;
    }

    const resolved = path.resolve(path.dirname(file), decodedPath || ".");
    const withinRoot = resolved === root || resolved.startsWith(`${root}${path.sep}`);
    if (!withinRoot) {
      errors.push(`${relative(file)}:${lineNumber(text, reference.offset)}: link leaves the repository: ${target}`);
      continue;
    }
    if (!(await exists(resolved))) {
      errors.push(`${relative(file)}:${lineNumber(text, reference.offset)}: missing local target: ${target}`);
      continue;
    }
    if (!decodedFragment) continue;

    const targetFile = decodedPath ? resolved : path.resolve(file);
    if (path.extname(targetFile).toLowerCase() !== ".md") continue;
    let targetText = documentCache.get(targetFile);
    if (targetText === undefined) {
      try {
        targetText = await readableText(targetFile);
        documentCache.set(targetFile, targetText);
      } catch (error) {
        errors.push(`${relative(file)}:${lineNumber(text, reference.offset)}: cannot read anchor target ${target}: ${error.message}`);
        continue;
      }
    }
    if (!anchors(targetText).has(decodedFragment.toLowerCase())) {
      errors.push(`${relative(file)}:${lineNumber(text, reference.offset)}: missing Markdown anchor: ${target}`);
    }
  }
}

if (errors.length > 0) {
  console.error(`Documentation verification failed with ${errors.length} error(s):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exitCode = 1;
} else {
  console.log(`Documentation verification passed for ${sources.length} Markdown files.`);
}
