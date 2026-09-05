Apple Silicon macOS runtime. `npm run setup:engines:macos` downloads and verifies
the arm64 FFmpeg/FFprobe/COLMAP/Brush closure into `bin/` and `lib/`; the
binaries themselves are never committed.

This README is tracked so the directory exists in a fresh clone.
`src-tauri/tauri.macos.conf.json` declares the directory as a bundle resource,
and the Tauri build script aborts when a declared resource path is missing --
which would otherwise make `cargo test` fail before compiling any test.
