# Official CUDA toolkit metadata fixtures

These complete JSON documents come directly from NVIDIA's repository metadata.
Linux's CUDA SDK label is a date-based build identity, not the release label.
The 13.2.2 documents are negative fixtures; the build remains locked to 13.2.0.

| Fixture | Official source |
| --- | --- |
| linux-13.2.0.json | https://developer.download.nvidia.com/compute/cuda/repos/runfile/x86_64/version_13.2.0.json |
| linux-13.2.2.json | https://developer.download.nvidia.com/compute/cuda/repos/runfile/x86_64/version_13.2.2.json |
| windows-13.2.0.json | https://developer.download.nvidia.com/compute/cuda/repos/windows/x86_64/version_13.2.0.json |
| windows-13.2.2.json | https://developer.download.nvidia.com/compute/cuda/repos/windows/x86_64/version_13.2.2.json |

Tests parse both LF and CRLF checkouts without changing the metadata fields.
