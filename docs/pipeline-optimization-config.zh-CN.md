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

关闭后保留旧版流程：视频按固定比例均匀抽帧，初次特征提取使用 COLMAP 默认 SIFT 限制，不执行候选帧池、Bridge Backfill、分档分辨率规划、Caspar BA 或 High 的 OOM 自动重试。图片输入仍保留全部有效图片并使用 Exhaustive Matching。

| 档位 | 视频保留比例 | 增量补拍 SFM 最大边 / 特征数 | Brush 迭代 | Brush 最大分辨率 | Densification / Splat 上限 |
| --- | ---: | ---: | ---: | ---: | --- |
| Fast | 30% | 1600 / 4096 | 8,000 | 1200 | 不额外设置 |
| Balanced | 50% | 1920 / 8192 | 15,000 | 1600 | 不额外设置 |
| High | 100% | 3200 / 16384 | 30,000 | 2000 | 不额外设置 |

`incrementalSfmMaxImageSize` 和 `incrementalSfmMaxFeatures` 用于增量补拍；旧版首次特征提取继续使用 COLMAP 自身默认值，避免配置化工作改变关闭自动优化时的历史行为。所有档位的 Brush `refineEvery` 默认都是 200。

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
| Fast | 0.00004 | 0.15 | 6,000 | 无 | 无 |
| Balanced | 0.00003 | 0.20 | 12,000 | 无 | 无 |
| High Low | 0.00002 | 0.25 | 23,000 | 200,000 | High Emergency |
| High Standard | 0.00002 | 0.30 | 25,000 | 1,500,000 | High Low |
| High Large | 0.00002 | 0.30 | 25,000 | 4,000,000 | High Standard |
| High Emergency | 0.00004 | 0.20 | 20,000 | 200,000 | 无 |

实际 `maxSplats` 不会低于初始 SfM 三维点数。High 只有在错误明确分类为显存不足时才按当前 Profile 的 `oomFallback` 降低一级，并且整次任务最多自动重试一次；`DeviceLost` 或普通进程错误不会触发该降级。

## Bridge 与 Caspar

- Bridge 仅用于开启自动优化的视频。初始注册率低于 `bridgeTriggerRatio`（默认 0.8）且候选池仍有预算时，优先填补最大的内部未注册区，再处理首尾缺口；失败会回滚到初始模型。图片输入不执行 Bridge。最终重建低于 `goodRegistrationRatio`（默认同为 0.8）时给出质量警告，但仍允许继续训练。
- SIFT GPU 与 Caspar BA 分开判断。SIFT GPU 可用不代表 Caspar 一定启用。
- Caspar 仅在自动优化开启、设备与驱动兼容、真实 GPU 探测成功，并且数据库关键点总数和几何验证匹配总数分别达到配置阈值（默认均为 2,000,000）时使用。此时局部 BA 为 Ceres、全局 BA 为 Caspar；其他情况全部使用 Ceres。
- Caspar Mapper 进程失败时会清理该次 Mapper 输出，显式改为局部/全局 Ceres 重试一次，并在当前应用进程内禁用该引擎与 GPU 组合的 Caspar。
- macOS 始终使用 Ceres CPU。

## 配置边界

配置加载时会拒绝未知字段、非正数尺寸/迭代、非法比例、Rescue FPS 小于初始 FPS、无界或循环的 OOM 回退链，以及 High Profile 缺少 Densification 或 Splat 上限。算法选择本身不由 JSON 表达：Sequential/Exhaustive Matching、Incremental Mapper、Bridge 的确定性选帧、Ceres 回退和最多一次 OOM 重试仍由代码固定，以免错误配置破坏项目或 checkpoint 语义。
