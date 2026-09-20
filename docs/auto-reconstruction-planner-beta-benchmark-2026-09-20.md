# Auto Reconstruction Planner Beta 同素材测试记录（2026-09-20）

本文记录 Auto Reconstruction Planner Beta 第一轮视频同素材对照测试。结论用于判断 Planner 各组件是否有效，并指导后续阈值调整；由于两个案例来自不同工程 schema、使用不同 Brush 预算且均位于 Debug 工程路径，本记录不构成严格的 Release 性能 A/B。

## 测试对象与数据来源

| 项目 | Planner | 工程目录 | Schema |
| --- | --- | --- | ---: |
| `20260920-140113_003` | 开启 | `E:\GaussianSplatting\test\20260920-140113_003` | 8 |
| `20260919-184300_003` | 关闭 | `E:\GaussianSplatting\test\20260919-184300_003` | 6 |

两个工程中的 `source/input.mov` SHA-256 均为：

```text
8F9E19C146797471C27B3CE8F3545D6B103E7B92061110B043AAD099B6A3B864
```

输入为同一个 `1920×1080`、30 FPS、56.2 秒的 ProRes 4444 Alpha 视频，共 1,686 帧。两个任务均选择 Fast。

分析依据：

- 两个工程的 `project.json`、`state.json`。
- `logs/ffmpeg.log`、`logs/colmap.log`、`logs/brush.log`。
- Planner 工程的 `logs/planner.json` 和 `work/planner/frame-candidates.json`。
- 对两个工程保留的稀疏模型补充运行 COLMAP `model_analyzer`，用于取得统一口径的 observations、track length 和 reprojection error。

## 结果概览

| 指标 | Planner | 未启用 Planner | 变化 |
| --- | ---: | ---: | ---: |
| 输入画面 | 225 | 337 | -33.2% |
| 注册画面 | 208 | 115 | +80.9% |
| 注册率 | 92.44% | 34.12% | +58.32 个百分点；2.71× |
| 三维点 | 4,829 | 3,199 | +51.0% |
| Observations | 51,606 | 21,532 | +139.7% |
| Mean track length | 10.687 | 6.731 | +58.8% |
| Mean observations / image | 248.106 | 187.235 | +32.5% |
| Mean reprojection error | 0.779 px | 0.749 px | Planner 高 3.9%，两者均低于 1 px |
| 有效模型结构 | 208 + 2 张 | 115 + 99 + 98 + 2 张 | Planner 碎片更少 |
| Splat 数 | 9,408 | 11,862 | -20.7% |
| PLY 大小 | 2,221,837 B | 2,800,982 B | -20.7% |
| 总耗时 | 363.070 秒 | 498.033 秒 | -27.1% |

Planner 在减少 112 张输入画面的情况下，多注册了 93 张画面，并产生更多三维点、观测和更长的特征轨迹。重投影误差略高，但差距只有约 `0.029 px`，不足以抵消覆盖率和连通性的显著提升。

Splat 数和 PLY 大小不能用于直接判断 Planner 的重建质量，因为两次 Brush 预算不同。

## 阶段耗时

| 阶段 | Planner | 未启用 Planner | 说明 |
| --- | ---: | ---: | --- |
| FFprobe | 0.060 秒 | 0.058 秒 | 基本一致 |
| FFmpeg 正式画面提取 | 4.078 秒 | 5.117 秒 | Planner 少 33.2% 画面 |
| Feature Extraction | 13.106 秒 | 18.153 秒 | Planner 快 27.8% |
| Matching | 20.218 秒 | 2.062 秒 | Planner 启用了 Loop Closure，耗时约 9.81× |
| View Graph Calibrator | 1.018 秒 | 无 | 仅 Global 候选使用 |
| Global Mapper | 27.319 秒 | 无 | 产生 214 个相机但 0 个三维点，候选被拒绝 |
| Incremental Mapper | 21.222 秒 | 28.238 秒 | Planner 快 24.9% |
| COLMAP 合计 | 82.883 秒 | 48.453 秒 | Planner 慢 71.1% |
| Brush | 272.528 秒 | 443.843 秒 | 两次预算不同，不可直接归因于 Planner |
| Brush 前总耗时 | 90.542 秒 | 54.190 秒 | Planner 慢 67.1% |
| 任务总耗时 | 363.070 秒 | 498.033 秒 | Planner 快 27.1% |

总耗时减少约 135 秒，但 Brush 本身减少约 171 秒；Planner 的 Brush 前阶段反而增加约 36 秒。因此总耗时下降主要来自 Brush 预算变化，而不是 Planner 核心流程变快。

两次实际 Brush 参数为：

| 项目 | Brush 参数 |
| --- | --- |
| Planner | `8000 steps @ 1200` |
| 未启用 Planner | `15000 steps @ 1600` |

必须使用当前同一版本、相同 Brush baseline 重新运行 Planner On/Off，才能评价 Planner 对总耗时的净影响。

## Planner 决策链分析

### Capture 与选帧

CaptureAnalyzer 输出：

- Capture type：`ObjectOrbit`
- Temporal order confidence：`0.95`
- Loop prior：`0.698`
- Motion level：`Low`
- Motion variance：`0.00989`
- Blur level：`0.809`
- Exposure variance：`0.00318`

分析扫描以 12 FPS 得到 674 个候选，最终选择 225 帧，实际平均密度约 4.004 FPS，接近 Fast 的 normal minimum。

本次选中帧的相邻 source frame gap 只有 `7` 和 `8`，两者各出现 112 次。这相当于规则的约 4 FPS 均匀采样，没有体现明显的非均匀动态密度。候选分数的整体均值也未显示选中帧比未选中帧更清晰：

| 候选集合 | Sharpness | Motion | Exposure |
| --- | ---: | ---: | ---: |
| 选中 225 帧 | 0.18988 | 0.11221 | 0.17228 |
| 未选中 449 帧 | 0.19084 | 0.12712 | 0.17249 |

因此，本次收益不能归因于“高质量非均匀选帧”；更准确的解释是 Planner 将低运动素材降到约 4 FPS，减少冗余，同时保留了完整时间覆盖。

该素材带 Alpha，大面积透明背景可能稀释全画面 sharpness、motion 和 exposure 指标。这是后续实现 Alpha/前景加权分析的重要测试样本。

### Pairing 与 View Graph

Planner 选择 `SequentialWithLoopClosure`：

```text
Sequential overlap = 10
Loop detection images = 16
Estimated pairs = 2250
```

View Graph 结果：

| 指标 | 数值 |
| --- | ---: |
| Images / connected images | 225 / 224 |
| Connected components | 2 |
| Largest component ratio | 99.56% |
| Median degree | 7.0 |
| Normalized degree | 0.03125 |
| Two-core ratio | 96.0% |
| Bridge ratio | 0.956% |
| Temporal edge ratio | 46.71% |
| Long-range edge ratio | 43.73% |
| Median inliers | 88 |
| Graph decision | Warning |

Loop Closure 增加了显著的长距离连接，匹配时间也从 2.1 秒增长到 20.2 秒。结合最终模型的注册覆盖和 track length，它很可能是本次重建提升的主要来源之一。

### Mapper 与恢复

MapperSelector 根据高 LCC、高 two-core、低 bridge 和较高 long-range ratio，首先选择 Global Mapper，并执行 View Graph Calibrator。

Global 候选结果：

```text
Registered images: 214
Points: 0
Observations: 0
```

该候选虽然有相机位姿，但没有可用三维点，几何校验将其判定为 NotViable，没有送入 Brush。Fast 的一次 Normal Rescue 随后执行 Alternate Mapper，Incremental 候选得到：

```text
Registered images: 208 / 225
Points: 4829
Observations: 51606
Mean track length: 10.686685
Mean reprojection error: 0.778738 px
Decision: Pass
Viability: Viable
```

本次没有进入 Success Recovery，也没有突破 Fast normal budget。

这证明以下机制有效：

- 无效 Global 结果不会仅凭注册数量进入 Brush。
- Mapper 数据库隔离允许 Incremental 从未被 Global 污染的 matched baseline 重新开始。
- Alternate Mapper 和最佳有效候选选择可以自动恢复任务。

同时也说明 MapperSelector 的 Global 判定仍需收紧。对于具有多个 component、weak runs 或 bottleneck 的 temporal/object-orbit 图，即使 LCC 和 two-core 较高，也应考虑优先 Incremental，或降低 Global 的 Fast 档位优先级。本次无效 Global 路径浪费约 28.3 秒。

## 有效性结论

| 目标 | 结论 |
| --- | --- |
| 提高注册成功率 | 明显有效 |
| 提高几何覆盖和连通性 | 明显有效 |
| 拒绝错误几何 | 有效；214 相机但 0 点的 Global 候选被拒绝 |
| 自动 Mapper 恢复 | 有效；Incremental fallback 成功 |
| 在 Fast normal budget 内完成 | 有效；未进入 Success Recovery |
| 降低 Planner/SfM 耗时 | 本次无效；Brush 前慢 67.1% |
| 非均匀智能选帧 | 本次没有得到证据，实际接近均匀 4 FPS |
| 降低任务总耗时 | 表面有效，但主要由不同 Brush 预算造成，不能归因于 Planner |
| 提高最终渲染细节 | 当前数据不足，需同 Brush 参数和固定视角截图 |

总体判断：Planner Beta 在本素材上的首要产品目标“尽可能生成可用重建”已经得到正向验证。它把旧流程的 34.1% 注册率提升到 92.4%，并避免把无三维点的 Global 结果交给 Brush。当前主要问题不是成功率，而是 Global 首选失误、Loop Closure 成本和智能选帧尚未体现。