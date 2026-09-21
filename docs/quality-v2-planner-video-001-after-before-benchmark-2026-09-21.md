# 视频 001 Quality v2 + Planner / 历史流程对照测试记录（2026-09-21）

本文分析同一份视频素材在 after 与 before 两条流程下的端到端结果。**After 同时启用了 Quality v2 和 Planner v2；Before 是既没有 Quality v2、也没有 Planner 的历史流程。** 截图编号沿用本次测试提供的顺序：图 1、图 2 为 after，其中图 1 是 Gaussian Splat 外观截图；图 3、图 4 为 before，其中图 3 是 Gaussian Splat 外观截图。

人工视觉结论为：**before 的外观略好，但差异不明显。** 本文将该观察与项目元数据、进程日志、COLMAP 稀疏模型和最终 PLY 数据结合分析。

## 1. 对照对象

| 组别 | 项目 | 档位 | 工程 Schema | 流程特征 |
|---|---|---|---:|---|
| After | `20260920-235202_001` | Quality v2 Balanced | 8 | 同时启用 Quality v2 与 Planner v2 |
| Before | `20260822-152216_001` | 历史均衡档，非 Quality v2 | 5 | 无 Quality v2、无 Planner；固定保留 50% 视频帧 |

两个项目中的源文件具有完全相同的 SHA-256：

`7D606C4FA4E00EE34D61D1E3D8613E284F35BB1860650488D6666A7633F6D293`

源素材参数相同：

- 时长：`35.572971 秒`；
- 分辨率：`1920 × 1080`；
- 帧率：`30 fps`；
- 总帧数：`1065`；
- 编码：`H.264 / yuv420p`；
- 文件大小：`20,762,595 bytes`。

Brush 与 COLMAP 可执行文件在两次运行中的 SHA-256 也分别完全相同，可以排除输入素材和核心引擎二进制差异。两次应用 Schema 和流水线逻辑不同，并且 After 同时引入了 Quality v2 与 Planner v2，因此本测试评价的是“Quality v2 + Planner”组合相对“无 Quality v2、无 Planner”历史流程的整体效果，不能将全部差异归因于 Planner 或 Quality v2 中的任意一方。

## 2. 端到端结果

| 指标 | After | Before | After 相对变化 |
|---|---:|---:|---:|
| 总耗时 | 18分49.046秒 | 1小时05分52.391秒 | `-71.4%` |
| 速度倍数 | 3.50× | 1.00× | After 约快 `3.50×` |
| 输入 SfM 帧数 | 177 | 533 | `-66.8%` |
| 实际采样密度 | 4.976 fps | 15.000 fps | `-66.8%` |
| 注册图片 | 174 / 177 | 524 / 533 | 注册率基本相同 |
| 注册率 | 98.305% | 98.311% | `-0.006` 个百分点 |
| 三维点 | 134,869 | 227,327 | `-40.7%` |
| Observations | 1,070,339 | 3,880,548 | `-72.4%` |
| 最终 Splat | 393,235 | 468,093 | `-16.0%` |
| PLY | 88.5 MiB | 105.4 MiB | `-16.0%` |

After 只使用 Before 约三分之一的 SfM 图片，总耗时减少 47分03.345秒，同时将注册率维持在几乎完全相同的 98.3%。从完成率和速度看，after 流程的效率收益明确。

三维点减少 40.7%，但最终 Splat 只减少 16.0%。这说明 Brush 在 after 中对较稀疏的初始点云进行了更强的净增密，使最终表达规模的差距显著小于 SfM 点数差距。

## 3. 阶段耗时

| 阶段 | After | Before | After 相对变化 |
|---|---:|---:|---:|
| FFprobe | 0.112 秒 | 0.029 秒 | 绝对差异可忽略 |
| 画面提取 | 1.046 秒 | 2.028 秒 | `-48.4%` |
| Feature Extraction | 10.128 秒 | 38.277 秒 | `-73.5%` |
| Matching | 2分47.236秒 | 51.351 秒 | `+225.7%`，约 3.26× |
| Incremental Mapper | 3分52.971秒 | 55分38.580秒 | `-93.0%`，约 14.33× |
| COLMAP 合计 | 6分50.335秒 | 57分08.208秒 | `-88.0%` |
| Brush | 11分55.226秒 | 8分41.972秒 | `+37.0%` |
| 已知进程合计 | 18分46.719秒 | 1小时05分52.237秒 | 与项目总耗时基本一致 |

端到端节省几乎全部来自 Mapper。该阶段节省 51分45.609秒，甚至大于最终总节省，因为 after 的 Matching 和 Brush 合计反向增加约 5分09.139秒，抵消了一部分收益。

After 的 COLMAP 总耗时仍减少 88.0%，说明大幅缩减 Mapper 输入规模在本案例中非常有效。与此同时，Pairing 与 Brush 并未随输入帧减少而同步加速。

## 4. Frame Planner 分析

After 的 Planner v2 将素材识别为：

| 指标 | 数值 |
|---|---:|
| Capture 类型 | `ObjectOrbit` |
| Motion level | `Low` |
| Temporal confidence | 0.95 |
| Loop prior | 0.6986 |
| Capture confidence | 0.95 |
| Balanced 预算 | 6 / 8 / 12 fps |
| 动态目标 FPS | 6.620 |
| 实际平均 FPS | 4.976 |
| Candidate FPS | 20 |
| 最终帧数 | 177 |

177 个入选帧覆盖了接近完整的视频范围，相邻源帧间隔主要为 6：170 个间隔为 6，另外各有 3 个间隔为 5 和 7。实际结果因此接近固定 5 fps 抽样。

按动态目标计算，本应产生约 `35.573 × 6.620 ≈ 235` 帧，实际只有 177 帧，少约 24.7%。更重要的是，实际 4.976 fps 已低于 Balanced 的 `min_fps = 6`。该测试记录表明，当时的 Frame Planner 在筛选后没有重新执行 Quality FPS 下限回填。

Before 没有执行 Quality v2 和 Planner，而是使用历史均衡档的固定 50% 保留比例，即 15 fps、533 帧。After 相比 Before 去掉约三分之二输入帧，是 Mapper 大幅提速和三维点减少的共同主因。

## 5. Pairing 与 View Graph

| 项目 | After | Before |
|---|---|---|
| Pairing | Sequential with Loop Closure | Sequential |
| Sequential overlap | 15 | 10 |
| Vocabulary Tree | 是，本地文件 | 否 |
| Matching 耗时 | 167.236 秒 | 51.351 秒 |

After 尽管图片数只有 Before 的三分之一，Matching 仍达到 Before 的 3.26 倍。主要差异是 overlap 从 10 增至 15，并自动执行 Vocabulary Tree Loop Closure。该 Pairing 决策在本案例中的时间收益为负。

After 的 View Graph 指标为：

- 图片数：177；
- 已连接图片：175；
- 连通分量：3；
- 最大连通分量比例：0.9887；
- 2-core 比例：0.9831；
- bridge 比例：0.0008；
- 中位度数：13；
- 中位 inliers：1214；
- 长距离边比例：0.1303；
- Graph decision：`Warning`。

图的主体连接非常强，但末端存在 3 张图片组成的弱区间，并出现一个严重瓶颈。Planner 没有进入 Rescue，Incremental Mapper 最终注册 174 张图片，与 Before 98.3% 的注册率几乎相同。该决策没有损害任务成功，但额外 Loop Closure 成本没有转化为更高注册率。

## 6. 稀疏重建质量

两个归档稀疏模型均使用同一 COLMAP `model_analyzer` 重新读取：

| 指标 | After | Before | After 相对变化 |
|---|---:|---:|---:|
| 注册图片 | 174 | 524 | `-66.8%` |
| 三维点 | 134,869 | 227,327 | `-40.7%` |
| Observations | 1,070,339 | 3,880,548 | `-72.4%` |
| Mean track length | 7.936 | 17.070 | `-53.5%` |
| Mean observations / image | 6151.374 | 7405.626 | `-16.9%` |
| Mean reprojection error | 0.5391 px | 0.5617 px | After 低 0.0226 px |
| 三维点 / 注册图片 | 775.1 | 433.8 | After 高约 78.7% |

Before 的图片和总观测更多，并形成了更长的特征轨迹，因此产生的三维点总数比 After 多 92,458 个。After 按注册图片归一化后的三维点数反而更高，这与入选画面之间冗余降低、单帧信息利用率提高的方向相符；但每个三维点由多张图片共同三角化，该比值不能解释为某一张图片独立创造了对应数量的点。由于图片总数减少约三分之二，After 的总点数仍明显下降。

After 的重投影误差略低，绝对改善约 0.023 px。结合基本相同的注册率，可以确认 After 不是以明显不稳定或错误的相机解算换取速度。它主要牺牲了重复观测数量、轨迹长度和最终几何密度。

两次日志中每张图片报告的平均 SIFT 数量接近：After 约 10,363，Before 约 10,302。单帧特征提取能力没有显著下降，三维点差异主要来自帧数、匹配关系和轨迹构成，而不是每张图片特征数量不足。

## 7. Brush 与最终输出

| 项目 | After | Before |
|---|---:|---:|
| Brush iterations | 15,000 | 15,000 |
| 最大分辨率 | 1600 | 1600 |
| Brush 二进制 | 完全相同 | 完全相同 |
| 初始三维点 | 134,869 | 227,327 |
| 最终 Splat | 393,235 | 468,093 |
| Splat / 三维点 | 2.916 | 2.059 |
| Brush 耗时 | 11分55.226秒 | 8分41.972秒 |

After 的 Brush 将每个初始三维点对应的最终 Splat 比例提高到 2.916，而 Before 为 2.059。这个净增密过程把初始点数 40.7% 的差距压缩为最终 Splat 16.0% 的差距，是两边视觉差异不明显的重要原因。

After 的 Brush 反而比 Before 慢 37.0%。由于两次训练迭代、最大分辨率和 Brush 二进制完全一致，现有日志不能把这 193 秒差异归因于 Quality 或 Planner 参数。GPU 负载、缓存、温度、驱动调度或运行时状态都可能影响结果，但本次记录没有足够证据确定具体原因。

## 8. 截图视觉分析

图 1 与图 3 展示的是同一建筑素材的 Gaussian Splat 结果。人工观察结论是 Before 略好，但差异不明显：

- 两边都保留了建筑主体、屋顶、窗户、墙面和主要轮廓，没有出现明显的大面积缺失或重建失败；
- Before 的屋顶纹理、窗框和部分高对比边缘略显稳定、完整；
- After 的局部表面更容易出现轻微软化和纹理融合，但没有达到明显劣化的程度；
- 这种轻微差距与 Before 多 74,858 个最终 Splat、多 92,458 个稀疏点和更高总观测量的方向一致；
- After 仍有 393,235 个 Splat、98.3% 注册率和更低的重投影误差，因此视觉结果仍保持较高完整度。

两张 Gaussian 截图不是严格相同的相机位置和缩放。项目元数据中，After 使用默认变换；Before 保存了 `X = 26°` 的旋转和 `10×` 缩放。两边均没有删除 Gaussian。由于观察视点和构图不同，本次视觉判断适合定性评价整体差异，不适合进行逐像素或局部锐度的严格量化。

## 9. Planner 收益拆解

### 9.1 Frame Planning 是主要正收益

Quality v2 为 Balanced 提供 `6 / 8 / 12 fps` 的预算范围，Planner 根据 `ObjectOrbit + Low Motion` 分析结果生成动态目标并选择具体画面。虽然本测试无法把 Quality 预算与 Planner 决策完全拆开，但从实际执行路径可以确认，Planner 的 Frame Planning 直接把 SfM 输入控制在 177 帧，而历史流程固定使用 533 帧。

这个决策带来的结果是：

- Incremental Mapper 耗时从 55分38.580秒降到 3分52.971秒，减少 93.0%；
- Feature Extraction 减少 73.5%；
- COLMAP 总耗时减少 88.0%；
- 注册率仍维持约 98.3%；
- 重投影误差由 0.5617 px 降至 0.5391 px；
- 按注册图片归一化的三维点由 433.8 提高到 775.1，与保留帧冗余更低、单位输入利用率更高的方向一致。

因此，Planner 在本案例中最明确的收益不是改变 Mapper 类型，而是用更少、信息密度更高的画面维持了稳定重建，并大幅缩小 Incremental Mapper 的问题规模。端到端 47分03.345秒的节省主要由这一决策产生。

### 9.2 Capture Analyzer 与 GraphQualityGate 的收益

Capture Analyzer 正确识别出有序、低运动、具有闭环先验的 `ObjectOrbit` 素材，为帧率和 Pairing 决策提供了素材先验。该模块本身不直接产生时间收益，其价值体现在后续决策可以区别于固定 50% 抽帧。

View Graph 主体连通良好，但存在末端弱区间，因此 GraphQualityGate 给出 `Warning`。Planner 没有进入 Frame / Matching Rescue，最终仍取得 174 / 177 注册和可用重建。这次“不救援”避免了额外 Feature Extraction、Matching 和 Mapper 轮次，对效率有正向作用；但由于没有执行 Rescue 对照，无法量化具体节省，也不能证明所有 Warning 场景都应该跳过救援。

### 9.3 ReconstructionValidator 的收益有限但正确

Planner 对唯一的 `normal-0-incremental` 候选执行了模型分析，确认其 98.3% 注册率、有限几何、7.936 平均轨迹和 0.5391 px 重投影误差满足 `Pass / Viable`，随后允许其进入 Brush。该判断与最终成功结果一致。

不过本次只有一个重建候选，没有发生 Normal、Rescue 或 SuccessRecovery 候选之间的实质比较。因此，本测试只能确认 Validator 没有错误拦截有效模型，不能评价 Best Reconstruction 排序带来的收益。

### 9.4 Pairing Planner 是负收益

Pairing Planner 根据较高的 Loop Prior 自动选择 Sequential with Loop Closure，并把 overlap 从历史流程的 10 提高到 15。最终 Matching 从 51.351 秒增加到 167.236 秒，多耗时 115.885 秒，而注册率并未高于 Before。

虽然 Loop Closure 可能帮助主体图保持强连接，但本测试没有“相同 177 帧、关闭 Loop Closure”的对照，不能证明它是达到 98.3% 注册所必需的。从已经确认的时间结果看，Pairing Planner 在本案例中属于明确的局部负收益。

### 9.5 未在本测试中产生收益的 Planner 能力

| Planner 能力 | 本次状态 | 可评价性 |
|---|---|---|
| Mapper 决策 | Incremental | Before 同样使用 Incremental，没有后端切换收益 |
| Normal Rescue | 未执行 | 无法评价 |
| SuccessRecovery | 未进入 | 无法评价 |
| 最低 30 帧保护 | 未触发 | 长视频且帧数充足，无法评价 |
| 多候选比较 | 只有一个候选 | 无法评价排序收益 |
| Frame Backfill | 未执行 | 实际 FPS 反而低于 Balanced 下限 |

### 9.6 Planner 净收益结论

本案例中，Planner 的净收益可以概括为：**Frame Planning 带来决定性的 Mapper 降本，并在注册率基本不变的情况下把端到端流程显著加速；GraphQualityGate 跳过不必要救援也保持了效率。与此同时，Pairing Planner 增加了约 1分56秒 Matching 时间，筛选后未恢复 Quality FPS 下限，并产生了轻微可见质量损失。**

因此，Planner 不是每个模块都获得正收益。当前可确认的核心价值集中在“减少 SfM 输入并避免额外救援”，而 Loop Closure、Recovery、多候选比较和最低帧数保护没有在本案例中证明收益。由于 After 同时启用了 Quality v2，本测试不能把 71.4% 的全部端到端加速都作为 Planner 的独立净收益。

## 10. 归因边界

### 可以直接确认

- 两次运行使用字节级相同的源视频。
- Brush 和 COLMAP 核心二进制完全相同。
- After 将输入从 533 帧减少到 177 帧，总耗时降低 71.4%。
- 两边注册率均约为 98.3%，After 没有降低整体重建成功率。
- Mapper 是主要收益来源，耗时降低 93.0%。
- After 的 Matching 和 Brush 都比 Before 更慢，抵消了部分 Mapper 收益。
- After 的三维点减少 40.7%，最终 Splat 减少 16.0%。
- 人工视觉验收认为 Before 略好，但差距不明显。

### 可以合理推断

- After 的轻微外观损失主要与更少的总观测、较短的轨迹和较低的最终 Splat 数量有关。
- After 的选帧降低了图像间冗余，按注册图片归一化的三维点数量明显提高。
- Brush 的更高净增密比例显著补偿了 After 的稀疏点下降，因此最终视觉差异远小于三维点差异。
- Loop Closure 加强了图连接，但没有在本案例中带来可见的注册率收益。

### 不能从本测试单独确认

- After 的 177 帧中，哪些具体画面对外观损失贡献最大。
- 如果将 After 回填到 Balanced 最低 6 fps 或首选 8 fps，三维点和视觉质量能够恢复多少。
- Quality v2 与 Planner 各自在耗时、三维点和视觉差异中贡献了多少，因为两项能力在 After 中同时启用。
- Matching 与 Brush 的运行时增量是否能在重复运行中稳定复现。
- 不同观察视点下的 PSNR、SSIM、LPIPS 或其他严格视觉指标。

## 11. 综合结论

本测试中，Quality v2 + Planner v2 组合将历史均衡流程的端到端耗时从 1小时05分52秒缩短到 18分49秒，速度提升约 3.50 倍，同时保持了与 Before 几乎相同的 98.3% 注册率。组合方案在生成效率和重建成功性方面有效，主要收益来自把 Incremental Mapper 输入从 533 帧缩减到 177 帧，使 Mapper 耗时降低 93.0%。由于 Quality v2 与 Planner 同时发生变化，本测试不能把这部分收益进一步拆分成各自的独立贡献。

该收益伴随可测量但相对温和的质量代价：After 的三维点减少 40.7%、总观测减少 72.4%，但 Brush 将最终 Splat 差距压缩到 16.0%。人工视觉结果与数据一致——Before 略好，但差异不明显，After 没有出现明显的结构缺失或不可用退化。

需要特别记录的是，After 的实际 4.976 fps 低于 Quality v2 Balanced 的 6 fps 最低预算，说明当时的 Planner 筛选结果没有重新满足 Quality FPS 下限；同时自动 Loop Closure 让 Matching 增加到 Before 的 3.26 倍。因而本案例的综合判断为：**Quality v2 + Planner 组合显著提升了总体效率并维持成功率，最终视觉质量仅轻微下降；主要不足是最终帧率低于档位下限，以及 Pairing 和 Brush 没有获得与降帧相匹配的时间收益。**
