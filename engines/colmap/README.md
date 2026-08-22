Bundled from the official `colmap-x64-windows-cuda.zip` release asset. The
application invokes `bin/colmap.exe` directly; feature extraction and sequential
matching select the SIFT backend through `--FeatureExtraction.use_gpu` /
`--FeatureMatching.use_gpu` (CUDA build ships both GPU and CPU paths). The
application selects the backend automatically from the NVIDIA driver version
and GPU compute capability, and passes the chosen GPU index explicitly. The
adjacent `plugins/` directory is preserved as published.
