Apple Silicon macOS runtime. `npm run setup:engines:macos` prepares the arm64
FFmpeg/FFprobe and Brush closure in `bin/` and `lib/`, then restores the locked
COLMAP Release separately under `colmap/`; binaries are never committed.

This README is tracked so the directory exists in a fresh clone.
`src-tauri/tauri.macos.conf.json` declares the directory as a bundle resource,
and the Tauri build script aborts when a declared resource path is missing --
which would otherwise make `cargo test` fail before compiling any test.
