# Quality v2 与 Auto Reconstruction Planner v1

> 本文保留最初的 Planner v1 设计记录，部分数值已经过后续迭代，不应作为当前运行参数。当前唯一参数基线及配置方法见[自动优化参数配置](pipeline-optimization-config.zh-CN.md)和 `config/pipeline-optimization.json`。

## 目标

第一版采用确定性的帧预算和一次性成功率补救，不识别 Walkthrough / Object Orbit，不运行 Geometry Probe，也不在运行时选择 Mapper。所有视频始终使用 Sequential Matching 与 Incremental Mapper；Loop Closure 和 Global Mapper 不进入生产决策。

## Quality v2

| 档位 | 初始视频采样 | Bridge 上限 | SfM 最大边 | Feature 上限 | Brush |
| --- | ---: | ---: | ---: | ---: | ---: |
| Fast | 6 fps | 9 fps | 1600 | 4096 | 8,000 / 1200 |
| Balanced | 8 fps | 12 fps | 1920 | 8192 | 15,000 / 1600 |
| High | 全部源帧 | 无额外预算 | 1920 | 8192 | 30,000 / 2000 |

视频至少以 30 帧为目标：`effective_initial_fps = max(preferred_fps, 30 / duration)`，并受源帧率限制。Fast 与 Balanced 的候选池分别按 9 fps 和 12 fps 建立，但初始阶段不解码候选帧。High 初始即提取全部源帧。

图片序列始终保留全部图片并使用 Exhaustive Matching，不执行 Bridge Backfill。

Planner 默认开启。关闭后使用 `main` 原有的保留比例、Feature 默认参数和重建流程；旧项目的 checkpoint 缺少 Planner 字段时按关闭处理，避免恢复任务时改变历史策略。

## Bridge Backfill

初始 Incremental Mapper 产生可用模型但注册率低于 80% 时，Fast 或 Balanced 最多执行一次 Bridge Backfill：

1. 从 `images.bin` 的图片名恢复源帧索引，按源视频时间而不是 COLMAP image ID 排序。
2. 查找连续未注册区，内部双锚点断区优先于视频首尾的单锚点断区。
3. 从时间跨度最大的断区选择最接近中点的未选候选帧。
4. 将断区二分；预算仍有剩余时继续处理当前最大的子区。
5. 一次性确定全部新增帧，保存到 checkpoint 后再执行补帧。

补帧数量不得超过当前档位候选池。30 帧最低保护已经达到或超过 Rescue 上限时没有额外预算；High 因为已经保留全部源帧，也不会额外补帧。

## 增量重建与回退

Bridge 仅解码新增帧，只为新增图片提取 Feature，并将每个新增帧与时间轴两侧最近的已有帧和新增帧建立局部匹配。随后通过 COLMAP `mapper --input_path` 复用初始模型继续 Incremental Mapper。

初始模型保存在 `sparse`，Bridge 模型写入独立的 `sparse-bridge`。只有 Bridge 模型完整可用且注册图片数不低于初始模型时才采用；提取、匹配、Mapper 或验证失败时删除新增画面并回退初始模型。Bridge 完成或失败后均不会触发第二轮补救。

Checkpoint 保存 Bridge 状态、初始注册率、预算、每次“断区起止 → 中点候选”的确定性决策轨迹、新增源帧索引、初始模型、最终模型及最终注册图片数和点数。恢复时复用同一份帧清单；已完成的 Bridge 不会重复执行。实时日志记录首个最长断区、内部桥接与边缘延伸数量，以及补救前后的注册率、三维点数和耗时。
