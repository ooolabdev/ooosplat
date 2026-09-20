# Quality v2 与 Planner 同素材图片基准记录（2026-09-13～20）

本文合并 Quality v2 第一轮图片测试与 Auto Reconstruction Planner Beta 的同素材测试，用于回答两个问题：Quality v2 的预算调整是否合理，以及 Planner 对图片序列是否有效。

## 结论摘要

- Planner 在这组 90 张无序图片上选择 `exhaustive + Global Mapper`，一次成功注册 `90 / 90`，没有触发 normal rescue 或 Success Recovery。
- 在 SfM 前端设置相同的两次 Quality v2 运行中，Planner 将 Mapper 路径由 `95.177` 秒缩短至 `39.016` 秒（含 `7.663` 秒 View Graph 校准和 `31.353` 秒 Global Mapper），缩短约 `59.0%`；COLMAP 总耗时缩短约 `31.3%`。
- Quality v2 + Planner 的总耗时比 Quality v2 无 Planner 低 `38.4%`，但这不是 Planner 的纯收益：同期 Brush 从历史版本的 `50k @ 2400` 改为当前 High 基线 `30k @ 2000`，图片准备实现也已优化。
- Planner 输出的稀疏点和最终 Splat 分别少 `16.4%` 和 `15.2%`。注册成功率没有下降、平均轨迹长度略有上升，但缺少同视角视觉评分，因此只能确认“更快并成功”，不能据此确认画质等价。
- 三次运行并非同一构建和同一参数的严格 A/B。Planner 的算法收益可以从 Mapper 阶段单独观察；端到端收益仍需使用同一 Release Build、同一 Quality 参数再复测。

## 测试素材与数据一致性

三次项目的 `source/images` 均包含 90 张 `2560×3840` PNG，文件名为 `frame_000001.png` 至 `frame_000090.png`，均带 Alpha 通道，总字节数为 `2,074,037,492`。

对三组源文件分别计算 SHA-256 后，文件名、长度与内容哈希组成的清单摘要均为：

```text
B6555D40274011A69B8CB4638DB6072B509BEF35AF9B8FAB33B0E7A2541C64F3
```

因此可以确认三次测试输入相同，不是仅凭文件数量和分辨率推断为同一素材。

## 三代方案

| 代次 | 项目 | Quality v2 | Planner | SfM 前端 | Mapper | Brush |
| --- | --- | --- | --- | --- | --- | --- |
| A：原始基线 | `20260913-174439_images` | 无 | 无 | 引擎默认值 | Incremental | `30k @ 2000` |
| B：历史 Quality v2 | `20260919-155234_images` | 有 | 无 | `2400 / 8192` | Incremental | `50k @ 2400` |
| C：当前 Quality v2 + Planner | `20260920-144653_images` | 有 | 有 | `2400 / 8192` | Global | `30k @ 2000` |

注意：B 使用的是 2026-09-19 的历史 Quality v2 High 参数，不是当前代码中的 High。A 来自安装版引擎路径，B、C 来自 `target/debug`，并且 C 已包含图片准备优化。这些差异会影响绝对耗时，不能把 A、B、C 当作只切换一个开关的严格 A/B。

## 端到端结果

| 指标 | A：无 Quality v2 / 无 Planner | B：Quality v2 / 无 Planner | C：Quality v2 / Planner |
| --- | ---: | ---: | ---: |
| 状态 | 完成 | 完成 | 完成 |
| 注册图片 | 90 / 90 | 90 / 90 | 90 / 90 |
| 三维点 | 15,304 | 13,923 | 11,636 |
| Splat 数 | 114,941 | 123,785 | 104,915 |
| PLY | 25.9 MB（27,127,627 B） | 27.9 MB（29,214,811 B） | 23.6 MB（24,761,491 B） |
| 总耗时 | 26分06.149秒 | 1小时11分14.475秒 | 43分52.151秒 |

### 阶段耗时

| 阶段 | A | B | C |
| --- | ---: | ---: | ---: |
| Feature Extraction | 56.408 秒 | 55.695 秒 | 55.518 秒 |
| Matching | 27.213 秒 | 26.349 秒 | 27.246 秒 |
| View Graph 校准 | — | — | 7.663 秒 |
| Mapper | 99.805 秒 | 95.177 秒 | 31.353 秒 |
| COLMAP 合计 | 183.426 秒 | 177.221 秒 | 121.780 秒 |
| Brush | 22分22.714秒 | 58分21.137秒 | 37分29.063秒 |
| COLMAP 与 Brush 之外 | 40.009 秒 | 9分56.117秒 | 4分21.308秒 |

“COLMAP 与 Brush 之外”包含素材导入、画面准备、Mask、进程启动、结果导出等，不能等同于纯画面准备耗时。COLMAP 首条日志与任务开始时间的间隔约为 A `40` 秒、B `9分56秒`、C `4分20秒`，说明 B 与 C 的端到端差异中还包含图片准备优化，不能计入 Planner 算法收益。

## 稀疏重建质量指标

以下数据使用同一 COLMAP `model_analyzer` 对保留的模型重新读取；A、B 取最终包含 90 张图片的模型，忽略 Mapper 留下的 2～3 张小模型。

| 指标 | A：Incremental | B：Incremental | C：Global |
| --- | ---: | ---: | ---: |
| 注册图片 | 90 | 90 | 90 |
| 三维点 | 15,304 | 13,923 | 11,636 |
| Observations | 74,100 | 69,272 | 58,916 |
| Mean track length | 4.841871 | 4.975365 | 5.063252 |
| Mean observations / image | 823.333 | 769.689 | 654.622 |
| Mean reprojection error | 0.680956 px | 0.674402 px | 0.000021 px |

C 相比 B 的三维点减少 `16.4%`、Observations 减少 `15.0%`，Mean track length 增加约 `1.8%`。这说明 Global Mapper 生成了更精简、轨迹略长的稀疏结构，并不直接等价于画质下降。

Global Mapper 的 `0.000021 px` 重投影误差与 Incremental Mapper 的约 `0.67 px` 数值尺度明显不同。在确认两种后端的误差定义、归一化和导出方式一致之前，不能据此宣称 C 的几何精度提高了数万倍，也不应把它直接用于跨 Mapper 的质量排序。

## Planner 决策与有效性

Planner 对图片输入的实际决策如下：

| 项目 | 结果 |
| --- | --- |
| Capture prior | `unorderedPhotos`，置信度 `0.9` |
| Frame plan | 保留全部 90 张图片 |
| Pairing | `exhaustive`，估算 4,005 对 |
| View Graph | 单连通分量，LCC `1.0`，2-core `1.0`，bridge `0.0`，判定 `healthy` |
| Mapper | `global`，原因 `dense_redundant_view_graph` |
| Candidate | `normal-0-global`，`pass / viable` |
| Rescue | 0 轮；未进入 Success Recovery |

### 可归因给 Planner 的收益

B 与 C 的 Feature Extraction 参数相同，且都执行 exhaustive matching，因此 Mapper 路径是本组数据中最接近受控对比的部分：

- B：Incremental Mapper `95.177` 秒。
- C：View Graph 校准 `7.663` 秒 + Global Mapper `31.353` 秒，共 `39.016` 秒。
- Mapper 路径缩短 `56.161` 秒，即 `59.0%`。
- COLMAP 整体由 `177.221` 秒降至 `121.780` 秒，缩短 `31.3%`。
- 注册率保持 `100%`，一次通过，未消耗救援预算。

因此，对这组“小规模、无序、匹配图健康且冗余较高”的图片，Planner 的 Mapper 选择是有效的：它明显缩短了稀疏重建时间，同时维持完整注册。

### 不能归因给 Planner 的变化

B 到 C 的总耗时由 `71分14秒` 降至 `43分52秒`，缩短 `38.4%`，但同时发生了以下变化：

- Brush 从 `50k @ 2400` 改为 `30k @ 2000`，Brush 耗时缩短 `35.8%`。
- 图片准备已改为文件头快速预检、硬链接优先和 Alpha 单次解码，COLMAP 启动前间隔由约 `9分56秒` 降为约 `4分20秒`。
- B、C 均为 Debug 路径，但代码版本、Brush 版本和当时机器负载未被固定记录。

甚至 A 与 C 使用相同的 `30k @ 2000` Brush 参数，Brush 耗时仍分别为 `22分23秒` 和 `37分29秒`。这进一步说明跨版本端到端耗时受构建、引擎和运行环境影响，不能只按参数理论值归因。

### 当前判断

Planner 对本素材的结论为“有效，但证据范围有限”：

- 已证明：能够识别健康的无序图片图结构，选择 Global Mapper，一次成功并明显缩短 Mapper 时间。
- 尚未证明：最终视觉质量与 Incremental Mapper 等价；端到端 `38.4%` 的缩短全部来自 Planner；该收益可推广到弱纹理、低重叠或碎片化 View Graph。

## Quality v2 历史结论

### High 不应默认使用 `50k @ 2400`

A 到 B 的 COLMAP 耗时减少 `3.4%`，但 Brush 从 `30k @ 2000` 提升到 `50k @ 2400` 后，Brush 耗时增长 `160.8%`，总耗时增长 `172.9%`，最终 Splat 仅增加 `7.7%`。此前人工观察也未发现与额外耗时相称的明显视觉收益。

因此，历史 Quality v2 的 `50k @ 2400` 不再作为 High 基线。当前代码中的 High 基线已回到 `30k @ 2000`；`30k @ 2400` 和 `50k @ Native/Auto` 只作为扩展预算上限。

### 历史 Fast 补充测试

同一素材还测试过当时版本的 Fast：SfM `1280 / 4096`、Brush `15k @ 1600`。它完成了 `90 / 90` 注册，总耗时 `27分07秒`，输出 8,079 个三维点、100,653 个 Splat 和 22.7 MB PLY。

其 Feature Extraction、Matching、Incremental Mapper 和 Brush 分别耗时 `94.864`、`9.167`、`137.143` 和 `565.694` 秒。较低 SfM 预算没有带来更短的总 SfM 时间，Mapper 反而出现更多初始化失败、小模型丢弃和重新初始化。这支持“Fast 不能通过过度削减 SfM 前端来制造档位差异”的结论。

## 当前 Quality v2 预算

下表来自当前代码，历史运行 B 的参数不代表当前默认值。

| 参数 | Fast | Balanced | High |
| --- | ---: | ---: | ---: |
| SfM baseline | 1600 / 4096 | 1920 / 8192 | 2400 / 8192 |
| SfM rescue ceiling | 1920 / 8192 | 2400 / 12288 | 3200 / 16384 |
| Brush baseline | 8k @ 1200 | 15k @ 1600 | 30k @ 2000 |
| Brush extension | 15k @ 1600 | 30k @ 2000 | 30k @ 2400；50k @ Native/Auto |
| Rescue rounds | 1 | 2 | 3 |

Quality 负责定义 baseline 与资源上限；Planner 根据素材先验、View Graph 和重建结果选择算法路径，并在需要时使用救援预算。对于图片输入，Planner 不执行视频抽帧，但仍会规划 Pairing、分析 View Graph、选择 Mapper 并决定是否救援。
