# Native engine runtime

Native binaries are not stored in Git. Exact upstream URLs, archive hashes,
installation rules, reported versions, and executable hashes are pinned in
`manifest.json` (Windows) and `manifest.linux.json` (Ubuntu).

Restore the local release inputs before development or packaging:

```text
npm run setup:engines
npm run verify:engines
```

Downloaded archives are cached under `.cache/engines/`, and extracted runtimes
are placed in this directory. Both are ignored by Git. The finished NSIS
installer still embeds the complete verified runtimes, so Windows end users do
not need to download or configure engines. On Linux, setup installs only the
pinned Brush binary under `engines/linux/brush`; FFmpeg, FFprobe, and COLMAP are
provided by the Ubuntu package manager.
