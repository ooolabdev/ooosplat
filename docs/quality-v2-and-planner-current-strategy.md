# OOOSplat 当前 Quality v2 与 Auto Reconstruction Planner 策略

> 状态：当前生产策略基线
> 更新日期：2026-09-20
> Planner 版本：`2`
> 适用分支：`feature/quality-v2-auto-parameters`

本文记录 OOOSplat 当前代码中实际生效的 Quality v2 与 Auto Reconstruction Planner 策略。它用于统一产品、开发、测试和后续调参时对“档位控制什么、Planner 决定什么、何时救援”的理解。

历史测试数据与阶段性结论见：

- [Quality v2 与 Planner 同素材图片基准记录](quality-v2-benchmark-2026-09-19.md)
- [Auto Reconstruction Planner Beta 同素材视频测试记录](auto-reconstruction-planner-beta-benchmark-2026-09-20.md)
- [Quality v2 + Planner 同素材视频 002 对照测试记录](quality-v2-planner-video-002-benchmark-2026-09-20.md)
- [视频 001：Quality v2 + Planner / 无 Quality v2、无 Planner 对照测试记录](quality-v2-planner-video-001-after-before-benchmark-2026-09-21.md)

上述基准包含旧参数和旧 Planner 路径，其中出现的自动 Global Mapper 已不代表当前生产策略。本文优先级高于历史测试文档中的策略描述。

## 1. 总体原则

Quality 与 Planner 的职责分开：

- **Quality v2 是资源预算包络**：规定帧率范围、SfM 分辨率与特征数、Matching 邻域、Brush 迭代数与分辨率，以及各档允许的 Normal Rescue 次数。
- **Planner 是算法决策层**：分析素材、选择帧、选择 Pairing、分析 View Graph、触发救援、验证候选重建并选择最佳结果。
- **Quality 不直接决定算法类型**：例如 Fast 不等于 Sequential，High 也不等于 Exhaustive。
- **Planner 不随意突破 Quality**：正常流程受所选档位约束；只有最低帧数保护和 SuccessRecovery 可以有记录地突破部分正常预算。
- **当前生产 Mapper 固定为 Incremental**：Normal、Normal Rescue、SuccessRecovery 全部使用 Incremental。Global 仅保留为实验能力，不参与自动决策。
- **成功率优先于 60% 注册率**：注册率低于 60% 不再单独禁止 Brush；只要候选满足最低几何可用条件，就可以作为降级但可用结果继续训练。

## 2. 当前生产流程

```text
Capture Analyzer
  ↓
Frame Planner
  ├─ Quality FPS Budget
  └─ Minimum Frame Protection
  ↓
候选评分与自适应选帧
  ↓
Minimum Frame Backfill（需要时）
  ↓
Feature Extraction
  ↓
Pairing Planner
  ↓
Matching
  ↓
ViewGraphAnalyzer
  ↓
GraphQualityGate
  ↓
Frame / Matching Normal Rescue（需要时）
  ↓
Incremental Mapper
  ↓
ReconstructionValidator
  ↓
SuccessRecovery（需要时，仍使用 Incremental）
  ↓
Best Reconstruction
  ↓
Brush
```

## 3. Quality v2 预算

### 3.1 视频帧预算

| 档位 | 最低 FPS | 首选 FPS | 正常最高 FPS | 分析 FPS |
|---|---:|---:|---:|---:|
| Fast | 4 | 6 | 9 | 12 |
| Balanced | 6 | 8 | 12 | 20 |
| High | 8 | 10 | 15 | 30 |

这些值是 Planner 的预算边界，不是固定 FFmpeg 抽帧率。

视频活动度位于 `0～1`：

- `0～0.5`：在 `min_fps → preferred_fps` 之间插值。
- `0.5～1`：在 `preferred_fps → max_fps` 之间插值。
- 活动度来自候选帧的运动分数与视角变化分数。
- 最终正常目标不会超过源视频 FPS。

### 3.2 SfM 基线与档位内救援预算

| 档位 | 基线最大边 | 基线最大特征数 | 档位内救援最大边 | 档位内救援最大特征数 |
|---|---:|---:|---:|---:|
| Fast | 1600 | 4096 | 1920 | 8192 |
| Balanced | 1920 | 8192 | 2400 | 12288 |
| High | 2400 | 8192 | 3200 | 16384 |

实际 SfM 最大边不会主动放大超过源素材尺寸。

### 3.3 Matching 预算

| 档位 | Sequential overlap | Prefilter neighbors |
|---|---:|---:|
| Fast | 10 | 16 |
| Balanced | 15 | 24 |
| High | 20 | 32 |

这些参数只规定资源规模。实际使用 Exhaustive、Sequential、Sequential with Loop Closure 或 Prefilter，由 Pairing Planner 决定。

### 3.4 Brush 基线

| 档位 | 迭代数 | 最大分辨率 |
|---|---:|---:|
| Fast | 8000 | 1200 |
| Balanced | 15000 | 1600 |
| High | 30000 | 2000 |

Planner 开启时，固定分辨率仍会受源素材原生尺寸限制，不会为了达到档位值主动放大素材。

代码中还声明了以下 Brush 扩展档：

| 档位 | 扩展档 |
|---|---|
| Fast | `15000 @ 1600` |
| Balanced | `30000 @ 2000` |
| High | `30000 @ 2400`、`50000 @ Auto/Native（首选上限 3200）` |

这些扩展档是保留预算，**当前主 Pipeline 不会自动升级 Brush 档位**。当前自动运行始终使用 Quality baseline Brush。

### 3.5 Normal Rescue 预算

| 档位 | 最大轮数 | 允许补帧 | 配置上允许 Local Exhaustive |
|---|---:|---|---|
| Fast | 1 | 是 | 否 |
| Balanced | 2 | 是 | 是 |
| High | 3 | 是 | 是 |

Normal Rescue 属于 Quality 档位内救援，不等同于跨预算的 SuccessRecovery。

## 4. 最低有效帧数保护

当前 `MinimumCapturePolicy`：

- `min_selected_frames = 30`
- `preferred_min_frames = 40`
- 不设置最低视频时长，不因视频过短直接拒绝任务。

视频输入时：

```text
minimum_required_fps = 30 / video_duration
effective_target_fps = max(normal_target_fps, minimum_required_fps)
```

`effective_target_fps` 最终仍受源视频实际 FPS 限制，但允许突破 Quality 的正常 `max_fps`。这种突破属于 `MinimumFrameOverride`，不是提高 Quality 档位。

示例：

- Fast、3 秒视频：最低需求为 `10 fps`，因此可突破 Fast 的 `9 fps` 正常上限，首次计划即以约 30 帧为目标。
- Fast、10 秒视频：最低需求为 `3 fps`，低于正常目标，仍按 Fast 动态目标运行。
- Fast、60 秒视频：最低需求仅 `0.5 fps`，不会改变长视频行为。

如果源视频本身不足 30 帧：

- 不失败；
- 尽量保留全部可用帧；
- 记录 `minimum_frame_target_unreachable = true`；
- 继续 Feature Extraction、Matching、View Graph、Incremental Mapper 和后续救援。

### 4.1 自适应选帧与最低数量回填

Planner 使用 320×180 灰度候选流分析画面，不为分析阶段写入全分辨率图片。

候选质量分数为：

```text
0.42 × sharpness
+ 0.28 × exposure
+ 0.20 × view_change
+ 0.10 × motion
```

自适应选择优先从各时间窗口中选择质量较好的画面。若选择后不足最低目标，则只从 Candidate Pool 中补足所需数量，不恢复全部候选帧。

最低数量回填排序同时考虑：

- 65% 时间覆盖与连续性；
- 35% 候选质量分数。

当前代码没有单独的落盘后 Smart Filter；候选评分、自适应筛选和最低数量回填均在正式 FFmpeg 抽取之前完成，因此不会先少抽一次、失败后再为最低 30 帧重新执行完整流程。

### 4.2 FramePlan 记录字段

FramePlan、项目 `state.json` 和本地 Planner 快照记录：

- `shortCapture`
- `minimumRequiredFps`
- `minimumFrameTarget`
- `minimumFrameOverrideApplied`
- `minimumFrameTargetUnreachable`
- `selectedFramesBeforeFilter`
- `selectedFramesAfterFilter`
- `backfilledForMinimumCount`

其中 `shortCapture` 表示正常 Quality 目标帧数低于 `preferred_min_frames = 40`，不代表任务必然失败。

## 5. 视频与图片输入的差异

| 能力 | 视频输入 | 图片序列输入 |
|---|---|---|
| Capture Analyzer | 低分辨率候选流分析 | 标记为 `UnorderedPhotos` |
| 动态 FPS | 是 | 否 |
| 最低 30 帧保护 | 是 | 否，使用用户提供的全部图片 |
| 自适应选帧 | 是 | 否 |
| Frame Backfill | 可从 Candidate Pool 补帧 | 不补帧 |
| Pairing Planner | 是 | 是 |
| View Graph / Graph Gate | 是 | 是 |
| Incremental Mapper | 是 | 是 |
| Matching / SfM Recovery | 是 | 是 |

图片输入仍会执行 Planner，但 Planner 不删减用户提供的图片，也不应用视频 FPS 预算。

## 6. Pairing Planner

默认阈值：

- Exhaustive pair limit：`12000` 对；
- Temporal confidence threshold：`0.62`；
- Loop prior threshold：`0.48`。

决策顺序：

1. 无序图片且全配对不超过 12000 对：`Exhaustive`。这大约对应不超过 155 张图片。
2. 时间顺序置信度达到 0.62，且 loop prior 达到 0.48：`SequentialWithLoopClosure`。
3. 时间顺序置信度达到 0.62：`Sequential`。
4. 时间顺序不可靠且全配对超过 12000 对：`Prefilter`。
5. 其他可承受规模：`Exhaustive`。

Loop Closure 和 Prefilter 使用随安装包提供的 COLMAP vocabulary tree，不在生成期间从 GitHub 下载。

## 7. View Graph 与 GraphQualityGate

ViewGraphAnalyzer 从 COLMAP `two_view_geometries` 读取已经通过几何验证的连接关系，记录：

- 已连接图片数和连通分量数；
- 最大连通分量比例；
- 中位度数和归一化度数；
- 2-core 比例；
- bridge 比例；
- 时间邻接边与长距离边比例；
- 中位 inliers；
- weak runs 和 bottlenecks。

GraphQualityGate 默认规则：

### NeedRescue

满足任一条件：

- 没有图片；
- 已连接图片少于 2；
- 最大连通分量比例 `< 0.70`；
- 中位 inliers `< 20`。

### Warning

未达到 NeedRescue，但满足任一条件：

- 最大连通分量比例 `< 0.88`；
- 2-core 比例 `< 0.35`；
- bridge 比例 `> 0.62`；
- 存在 weak runs。

### Healthy

以上问题均不存在。

ViewGraphAnalyzer 和 GraphQualityGate 不负责选择 Global/Incremental。它们继续用于决定补帧、扩大 Matching 和是否进入 Rescue。

## 8. Mapper 策略

当前 production Planner 的唯一 Mapper 计划为：

```text
MapperBackend::Incremental
calibrate_view_graph = false
reason = production_incremental_only
```

适用于：

- 正常重建；
- Graph Quality Gate 之后的 Normal Rescue；
- SuccessRecovery 的补帧、Matching 扩展和 SfM 升级；
- 从旧 Planner 检查点恢复但尚未完成的重建。

当前不会自动执行：

- MapperSelector 的 Global / Incremental 评分；
- Global Mapper 首选；
- Incremental 失败后的 Global fallback；
- SuccessRecovery 中的 Alternate Mapper。

`MapperBackend::Global`、Global 执行实现和 MapperSelector 仍保留，供显式实验使用。旧检查点中的 Global 候选不会参与当前未完成任务的候选比较。

## 9. Normal Rescue

Initial Matching 后先分析 View Graph。如果 GraphQualityGate 返回 `NeedRescue`，则在 Quality 的 Normal Rescue 轮数内尝试：

1. **视频补帧**：从 Candidate Pool 增加当前已选帧数约 25%，只为新增帧抽取特征并补充匹配。
2. **Matching 扩大**：若仍为 NeedRescue 且初始策略不是 Exhaustive，则执行 Exhaustive Matching。
3. 重新分析 View Graph。
4. 使用 Incremental Mapper。

图片输入没有视频 Candidate Pool，因此跳过补帧，但仍可执行 Matching Rescue。

Normal Rescue 不自动尝试 Global Mapper。

## 10. Reconstruction 验证与候选选择

### 10.1 最低几何可用条件

候选要进入可用范围，至少需要：

- 产生有限几何；
- 注册图片数大于 0；
- 三维点数大于 0；
- 重投影误差 `≤ 4 px`；
- 平均轨迹长度 `≥ 1.5`；
- 三维点数 `≥ 100`；
- 注册图片数 `≥ 12`。

### 10.2 决策等级

| 决策 | 条件摘要 |
|---|---|
| Pass | 注册率 `≥80%`、重投影误差 `≤2 px`、平均轨迹长度 `≥2` |
| Warning | 注册率 `≥60%` 且几何满足可用条件 |
| NeedRescue | 注册率 `<60%`，但几何仍满足最低可用条件 |
| Critical | 没有有效几何，或未达到最低可用条件 |

注册率低于 60% 但几何可用时，候选标记为 `DegradedButViable`。它会优先进入救援，但如果所有救援结束后仍是最佳可用结果，可以继续 Brush。

### 10.3 Best Reconstruction 排序

候选按以下优先级选择：

1. Viable 高于 DegradedButViable；
2. Pass 高于 Warning，高于 NeedRescue；
3. 更低的重投影误差；
4. 更长的平均轨迹；
5. 更多注册图片；
6. 更多三维点。

不可用候选不参与最终选择。

## 11. SuccessRecovery

如果正常候选没有达到 Pass，则进入 SuccessRecovery。默认策略：

| 项目 | 当前值 |
|---|---:|
| 最大恢复轮数 | 4 |
| 允许突破 Quality 补帧 | 是 |
| 允许扩大 Pairing | 是 |
| 允许提高 SfM 预算 | 是 |
| 允许 Alternate Mapper | 否 |
| 最大恢复 FPS | 15 |
| 跨档 SfM 最大边 | 4096 |
| 跨档 SfM 最大特征数 | 32768 |

恢复动作按当前 Pipeline 顺序为：

1. **视频补帧**：目标 FPS 为 `max(当前 FPS × 1.25, 当前 FPS + 1)`，上限 15 FPS；新增帧使用当前 Quality 的 `sfm_rescue` 预算抽取特征。
2. **Pairing escalation**：小规模使用 Exhaustive，大规模使用 Prefilter；恢复邻域至少为 `sequential_overlap=20`、`prefilter_neighbors=32`。
3. **SfM escalation**：用 `4096 / 32768` 重新提取全部特征并重新规划 Matching。
4. 每轮重建仍只使用 Incremental Mapper。

SuccessRecovery 不会自动执行 Global Mapper。

## 12. Planner 关闭时的兼容路径

`planner_enabled = false` 时，不执行 Capture Analyzer、动态选帧、最低 30 帧保护、Pairing Planner、View Graph Gate 或 Planner Rescue。

兼容路径为：

| 项目 | Fast | Balanced | High |
|---|---:|---:|---:|
| 视频保留比例 | 30% | 50% | 100% |

其他行为：

- 视频使用 legacy Sequential Matching，overlap 为 10；
- 图片使用 Exhaustive Matching；
- 使用 legacy Feature Extraction 参数路径；
- Mapper 使用 Incremental；
- Brush 仍使用当前 Quality baseline：Fast `8000@1200`、Balanced `15000@1600`、High `30000@2000`；
- 不使用 Planner 的候选比较和 SuccessRecovery。

因此，“Planner 关闭”主要表示恢复旧的抽帧、特征、匹配和重建控制逻辑，并不表示取消当前 Quality 的 Brush 档位。

## 13. 决策记录与可观测性

每个 Planner 项目会保存：

- `state.json`：可恢复的 Pipeline 和 Planner 权威状态；
- `logs/planner.json`：便于检查的 Planner 决策快照；
- `work/planner/frame-candidates.json`：视频候选帧分析结果；
- 实时任务日志：素材分析、画面规划、抽帧、Matching、View Graph、Rescue、Mapper 和 Brush 进度。

Planner 记录的主要决策包括：

- Quality budget snapshot；
- Capture prior；
- FramePlan 与实际选择帧；
- Pairing planned / actual；
- ViewGraph report 与 Graph decision；
- Mapper planned / actual；
- Reconstruction candidates 与 best candidate；
- Normal / SuccessRecovery 历史；
- 是否突破正常预算；
- 正常流程和恢复流程耗时。

## 14. 当前实现边界

以下能力虽然已经有数据结构或实验实现，但不属于当前 production 自动流程：

- Global Mapper 自动选择；
- Global fallback；
- Alternate Mapper SuccessRecovery；
- Brush 扩展档自动升级；
- `LegacySafeFallback` 和 `allow_full_frame_backfill` 的主 Runner 执行路径。

另外需要注意两项当前代码行为：

1. Quality 中声明了 `allow_local_exhaustive`，但当前 Runner 的 pre-mapper Exhaustive Rescue 实际由 `NeedRescue + 剩余轮数 + 初始策略非 Exhaustive` 控制，没有再次检查该开关。
2. SuccessRecovery 进入后，视频补帧成功产生 Pass 候选时，当前实现仍可能继续执行同一恢复流程中的 Pairing escalation；SfM escalation 前会再次检查是否已经存在 Pass。

这两项应作为后续收敛候选，不应在测试分析中误认为理想策略。

## 15. 回归测试基线

修改 Quality 或 Planner 时至少验证：

- Fast + 3 秒视频：目标不少于约 30 帧，允许有效 FPS 大于 9；
- Fast + 10 秒视频：`minimum_required_fps=3`，正常目标保持约 6 FPS；
- 长视频：最低帧数保护不改变正常档位差异；
- 源总帧数不足 30：尽量全取，不失败；
- 初选 35、筛选后 22：只回填到尽量不少于 30；
- 图片输入：保留全部图片，但仍执行 Pairing、View Graph、Incremental Mapper 和 Recovery；
- Normal、Normal Rescue、SuccessRecovery：Mapper 全部为 Incremental；
- Planner 不自动执行 Global Mapper；
- 注册率低于 60% 但几何可用：允许选择降级候选并进入 Brush；
- Planner 关闭：保持 30% / 50% / 100% legacy 视频保留比例。

## 16. 代码依据

- Quality 预算：`src-tauri/src/presets/quality.rs`
- Frame Planner 与最低帧数保护：`src-tauri/src/planner/frame.rs`
- Capture Analyzer：`src-tauri/src/planner/capture_analyzer.rs`
- Pairing Planner：`src-tauri/src/planner/pairing_planner.rs`
- View Graph：`src-tauri/src/planner/view_graph_analyzer.rs`
- Graph Gate：`src-tauri/src/planner/graph_quality_gate.rs`
- Planner 数据结构与 Mapper 固定策略：`src-tauri/src/planner/plan.rs`
- Reconstruction 分类与候选比较：`src-tauri/src/planner/reconstruction.rs`
- 实际 Pipeline 编排：`src-tauri/src/pipeline/runner.rs`
