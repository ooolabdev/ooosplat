# Toy dataset: Ceres/Caspar mapper comparison

Date: 2026-10-04 (Asia/Shanghai)

## Test conditions

- Input: `E:\GaussianSplatting\test\20261004-105210_toyWithMask\work\frames`
- Input frames: 251
- Database snapshot: `873C958D96A54BBBC79896F5791081025BA06EEA128E2CCB1AB55387E94ACD4D`
- COLMAP: 4.2.1, upstream commit `bd1fcf6`
- Executable SHA-256: `F3948D0B485CD62A36D1CCB2397D56DF4E0DE044997A7E513BAAFDE68FF34D54`
- GPU: NVIDIA GeForce RTX 3060 Ti, 8 GiB; driver 616.92; GPU index 0
- Ceres BA used CPU (`--Mapper.ba_use_gpu 0`).
- Each variant started from an independent copy of the same database snapshot.
- Only `mapper` was timed. Feature extraction, matching, and Brush training were excluded.
- Variants ran serially. Run 1 order: all Ceres, local Caspar, global Caspar, all Caspar. Run 2 used the reverse order.
- Mapper defaults were otherwise unchanged. Logs used `--log_level 1` and contain no per-iteration progress printing.

## Mapper timing

| Variant | Local BA | Global BA | Run 1 | Run 2 | Mean | Change vs all Ceres in each run |
|---|---|---|---:|---:|---:|---:|
| All Ceres | Ceres | Ceres | 79.567 s | 128.924 s | 104.245 s | baseline |
| Local Caspar | Caspar | Ceres | 113.737 s | 165.148 s | 139.443 s | +42.9%, +28.1% |
| Global Caspar | Ceres | Caspar | 113.619 s | 146.899 s | 130.259 s | +42.8%, +13.9% |
| All Caspar | Caspar | Caspar | 231.519 s | 214.309 s | 222.914 s | +191.0%, +66.2% |

Absolute times varied substantially between runs, including the all-Ceres baseline. The direction was stable: every Caspar configuration was slower than all Ceres in both runs.

## Main model quality

All variants produced two models. The main model registered the same 236 image names in every variant and run (sorted-name SHA-256 `939a31cb97a35e8873dc458b0bc19d1288d65e80fc2977d38060dc139d4021ec`). The secondary model registered 16 images.

| Variant | Registered images | Points, runs 1/2 | Observations, runs 1/2 | Mean reprojection error, runs 1/2 |
|---|---:|---:|---:|---:|
| All Ceres | 236 | 57,552 / 57,552 | 327,556 / 327,556 | 0.599634 / 0.599634 px |
| Local Caspar | 236 | 57,415 / 57,414 | 327,880 / 327,882 | 0.610290 / 0.610409 px |
| Global Caspar | 236 | 57,600 / 57,636 | 327,197 / 327,258 | 0.596336 / 0.597231 px |
| All Caspar | 236 | 57,454 / 57,486 | 327,591 / 327,573 | 0.611413 / 0.608794 px |

Coverage is equivalent for this dataset. Global-only Caspar produced a slightly lower mean reprojection error than all Ceres, while local and all-Caspar were about 1.5% to 2.0% higher. These small differences do not establish a general quality ranking from one dataset.

## Caspar solver exits

| Variant and run | Local calls (diag / max-iter) | Global calls (diag / max-iter) |
|---|---:|---:|
| Local Caspar, run 1 | 488 (486 / 2) | n/a |
| Local Caspar, run 2 | 489 (487 / 2) | n/a |
| Global Caspar, run 1 | n/a | 64 (19 / 45) |
| Global Caspar, run 2 | n/a | 63 (23 / 40) |
| All Caspar, run 1 | 488 (480 / 8) | 77 (9 / 68) |
| All Caspar, run 2 | 490 (480 / 10) | 71 (6 / 65) |

Most global Caspar calls reached the 200-iteration limit. Most local calls exited through the damping limit after fewer iterations. A damping-limit exit is not by itself a failed model, but the global iteration counts explain why Caspar did not accelerate this small dataset.

## Conclusion for this dataset

Use all Ceres for this toy workload. Global-only Caspar is the least costly Caspar configuration and preserved coverage and quality, but it was still 13.9% to 42.8% slower than all Ceres. Do not use all Caspar for workloads of this size under the current solver defaults.

Raw results:

- Run 1: `E:\Project\.tmp\caspar-toy-benchmark-20261004-1405`
- Run 2: `E:\Project\.tmp\caspar-toy-benchmark-20261004-1415-run2`
