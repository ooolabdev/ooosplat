# Managed COLMAP runtime

Restore the reviewed, hash-locked COLMAP 4.2.1 CUDA/Caspar/Ceres archive with
`npm run setup:engines`. The executable is `bin/colmap`; Linux never falls back
to apt or PATH COLMAP. Runtime binaries, libraries and build outputs are ignored.
