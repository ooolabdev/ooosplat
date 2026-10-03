OOOBrush ooo-v1.0.0 Ubuntu x86_64 runtime. The headless brush-cli is installed
as brush_app; the viewer is excluded. `npm run setup:brush` downloads and
verifies `brush_app` into this directory; the binary itself is never committed.

This README is tracked so the directory exists in a fresh clone.
`src-tauri/tauri.linux.conf.json` declares the CLI and metadata as bundle resources,
and the Tauri build script aborts when a declared resource path is missing --
which would otherwise make `cargo test` fail before compiling any test.
