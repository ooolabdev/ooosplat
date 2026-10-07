# 自动优化参数配置

OOOSplat 的重建与训练档位集中定义在 [`config/pipeline-optimization.json`](../config/pipeline-optimization.json)。Rust 代码只负责执行和校验流程，档位参数、显存阈值、Bridge 阈值、Caspar 数据量阈值及 High 的 OOM 降级目标均从该文件读取。

默认配置在编译时嵌入程序，确保正式包和 CI 可复现。开发者也可以在启动进程前设置 `OOOSPLAT_PIPELINE_CONFIG`，指向一份外部 JSON；程序每次启动只读取一次。外部文件不存在、JSON 格式错误、出现未知字段或参数不安全时，会输出原因并回退到内置默认配置，不会使用半份配置。

PowerShell 示例：

```powershell
$env:OOOSPLAT_PIPELINE_CONFIG = "E:\configs\pipeline-optimization.json"
npm run dev:local
```

修改配置后需要重启应用。`schemaVersion` 当前必须为 `1`。

## 自动优化关闭

关闭后保留旧版流程：视频按固定比例均匀抽帧，初次特征提取和增量补拍使用相同的档位 SFM 限制，不执行候选帧池、Bridge Backfill、分档分辨率规划、Caspar BA 或 High 的 OOM 自动重试。图片输入仍保留全部有效图片并使用 Exhaustive Matching。

| 档位 | 视频保留比例 | SFM 最大边 / 每张最大特征数 | Brush 迭代 | Brush 最大分辨率 | 梯度阈值 / 选择比例 / 增密截止 |
| --- | ---: | ---: | ---: | ---: | --- |
| Fast | 30% | 1200 / 8192 | 8,000 | 1200 | 0.0025 / 0.10 / 15,000 |
| Balanced | 50% | 1600 / 8192 | 15,000 | 1600 | 0.0025 / 0.10 / 15,000 |
| High | 100% | 2000 / 8192 | 30,000 | 2000 | 0.0025 / 0.10 / 15,000 |

`sfmMaxImageSize` 和 `sfmMaxFeatures` 同时用于首次重建和增量补拍，确保补拍沿用原任务的 SFM 限制。长边限制只影响 COLMAP 特征提取时的工作分辨率，不会缩放或改写源图片。旧版外部配置中的 `incrementalSfmMaxImageSize` 和 `incrementalSfmMaxFeatures` 仍作为兼容别名接受。所有档位的 Brush `refineEvery` 默认都是 200。

三档的兼容密化配置位于各自的 `automaticOptimizationOff.brush.densification`，实际执行时会显式传入 `--growth-grad-threshold 0.0025`、`--growth-select-fraction 0.1` 和 `--growth-stop-iter 15000`。快速档在 8,000 步结束；均衡档在 15,000 步结束；精细档在第 15,000 步停止基于梯度的密化，后续继续优化。截止参数不关闭新版 Brush 的其他 refine 行为，例如过大屏幕尺寸分裂。应用不额外传入 `--max-splats`，固定 OOOBrush 1.0.0 运行时仍采用自身的 10,000,000 上限，也不额外修改屏幕尺寸分裂或尺度过滤参数。

这组设置保留新版梯度阈值，恢复旧版的 0.10 选择比例。新旧版梯度统计方式不同，不能把旧阈值 0.00004 直接套到新版。2026-10-06 的 RTX 3060 Ti 快速档 toy 对照固定了同一份 COLMAP 模型、8,000 步和 1,200 训练长边，保留 19 个评估视角；比例从 0.25 降至 0.10 后，点数由 533,775 降为 316,951，耗时减少约 28.27%，渲染与旧版略更接近，相对原照片的前景指标小幅下降。该实测只覆盖快速档的一个场景，均衡和精细档采用同一兼容策略，但尚未完成同等实机画质验收。

旧版外部 JSON 没有 `densification` 字段或将其设为 `null` 时仍可加载，并沿用“不显式覆盖引擎密化默认值”的原有行为。新建任务读取当前配置；已保存的任务恢复时继续使用自身已记录的训练快照，不重写历史任务参数。

## 自动优化开启

视频先按目标 FPS 建立初始帧集，同时按 Rescue FPS 建立候选池。短视频会把目标提高到至少 30 帧，但不会超过源视频 FPS 或总帧数。视频按档位缩小到工作长边且不放大；图片输入保留原始尺寸和全部图片。

| 档位 | 初始 / Rescue FPS | 视频工作长边 | SFM 最大边 / 特征数 | Two-view tracks | Brush 迭代 / 最大分辨率 |
| --- | ---: | ---: | ---: | --- | ---: |
| Fast | 6 / 9 | 1600 | 1200 / 4096 | 关闭 | 8,000 / 1600 |
| Balanced | 8 / 12 | 1920 | 1600 / 8192 | 关闭 | 15,000 / 1920 |
| High Low | 12 / 15 | 3200 | 3200 / 16384 | 开启 | 30,000 / 3200 |
| High Standard | 12 / 15 | 3840 | 3200 / 16384 | 开启 | 30,000 / 3840 |
| High Large | 12 / 15 | 源素材长边 | 3200 / 16384 | 开启 | 30,000 / 源素材长边 |

High 初始档位按检测到的可用显存选择：未知或低于 8192 MiB 使用 Low，8192–12287 MiB 使用 Standard，达到 12288 MiB 使用 Large。

Brush 配置如下：

| Profile | 梯度阈值 | 选择比例 | 停止增密迭代 | Splat 上限 | OOM 后一次降级 |
| --- | ---: | ---: | ---: | ---: | --- |
| Fast | 0.0022 | 0.11 | 7,000 | 500,000 | 无 |
| Balanced | 0.0020 | 0.12 | 13,000 | 1,000,000 | 无 |
| High Low | 0.0018 | 0.13 | 21,000 | 1,200,000 | High Emergency |
| High Standard | 0.0016 | 0.14 | 23,000 | 1,500,000 | High Low |
| High Large | 0.0015 | 0.15 | 24,000 | 4,000,000 | High Standard |
| High Emergency | 0.0023 | 0.10 | 16,000 | 1,000,000 | 无 |

实际 `maxSplats` 不会低于初始 SfM 三维点数。Fast 和 Balanced 不增加 OOM 降级链；High 只有在错误明确分类为显存不足时才按当前 Profile 的 `oomFallback` 降低一级，并且整次任务最多自动重试一次；`DeviceLost` 或普通进程错误不会触发该降级。

## Bridge 与 Caspar

- Bridge 仅用于开启自动优化的视频。初始注册率低于 `bridgeTriggerRatio`（默认 0.8）且候选池仍有预算时，优先填补最大的内部未注册区，再处理首尾缺口；失败会回滚到初始模型。图片输入不执行 Bridge。最终重建低于 `goodRegistrationRatio`（默认同为 0.8）时给出质量警告，但仍允许继续训练。
- SIFT GPU 与 Caspar BA 分开判断。SIFT GPU 可用不代表 Caspar 一定启用。
- Caspar 仅在自动优化开启、设备与驱动兼容、真实 GPU 探测成功，并且数据库关键点总数和几何验证匹配总数分别达到配置阈值（默认均为 2,000,000）时使用。此时局部 BA 为 Ceres、全局 BA 为 Caspar；其他情况全部使用 Ceres。
- Caspar Mapper 进程失败时会清理该次 Mapper 输出，显式改为局部/全局 Ceres 重试一次，并在当前应用进程内禁用该引擎与 GPU 组合的 Caspar。
- macOS 始终使用 Ceres CPU。

### Caspar 策略依据

当前“较小数据集全 Ceres；达到数据量阈值后局部 Ceres、全局 Caspar”的策略基于以下两组 2026-10-04 实机对照：

- [Toy dataset: Ceres/Caspar mapper comparison](caspar-toy-benchmark-report-20261004.md)：251 帧数据在 RTX 3060 Ti 上重复两轮后，所有 Caspar 组合都慢于全 Ceres；其中全 Caspar 最慢。这支持对小数据集保留全 Ceres，并用关键点数和几何验证匹配数设置启用门槛。
- [001 局部 Caspar 收益调查](caspar-001-local-report-20261004.md)：在全局 Caspar 固定的对照中，局部 Caspar 虽缩短局部处理阶段，但增加了后续全局调用和迭代工作量，使 Mapper 总耗时增加约 19%–31%。这支持启用 Caspar 时保持局部 Ceres、只将全局 BA 交给 Caspar。

两份报告都只覆盖 COLMAP 4.2.1、RTX 3060 Ti、当前求解器默认参数和各自数据集，且没有评估最终 Brush 渲染质量。因此它们是当前保守策略的工程依据，不是对所有 GPU、数据规模和场景的普遍性能结论。`2,000,000` 的双阈值仍应通过更多真实数据集持续校准；在新证据充分前，不因单次更快结果放宽真实 GPU 探测、失败回退或局部 Ceres 约束。

## 配置边界

配置加载时会拒绝未知字段、非正数尺寸/迭代、非法比例、Rescue FPS 小于初始 FPS、无界或循环的 OOM 回退链，以及 High Profile 缺少 Densification 或 Splat 上限。算法选择本身不由 JSON 表达：Sequential/Exhaustive Matching、Incremental Mapper、Bridge 的确定性选帧、Ceres 回退和最多一次 OOM 重试仍由代码固定，以免错误配置破坏项目或 checkpoint 语义。
