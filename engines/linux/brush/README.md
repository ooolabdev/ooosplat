Brush v0.3.0 Ubuntu x86_64 runtime. `npm run setup:engines:linux` downloads and
verifies `brush_app` into this directory; the binary itself is never committed.

This README is tracked so the directory exists in a fresh clone.
`src-tauri/tauri.linux.conf.json` declares the directory as a bundle resource,
and the Tauri build script aborts when a declared resource path is missing --
which would otherwise make `cargo test` fail before compiling any test.
