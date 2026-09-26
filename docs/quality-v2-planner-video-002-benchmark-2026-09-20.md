# Quality v2 + Planner 同素材视频 002 对照测试记录（2026-09-20）

本文分析同一份视频在两个版本路径下的端到端结果：图 1 为未使用 Quality v2 和 Planner 的历史流程，图 2 为同时启用 Quality v2 与 Planner v2 的当前流程。本文只记录测试数据、归因边界和有效性结论。

## 1. 对照对象

| 组别 | 项目 | 流程 | 工程 Schema | 执行路径 |
|---|---|---|---:|---|
| A：历史基线 | `20260901-094941_002` | 无 Quality v2、无 Planner | 4 | 安装版引擎 |
| B：当前组合 | `20260920-190836_002` | Quality v2 High + Planner v2 | 8 | Debug 工程引擎 |

两个工程中的源文件均为：

- 时长：`56.2 秒`
- 分辨率：`1920 × 1080`
- 帧率：`30 fps`
- 总帧数：`1686`
- 编码：`ProRes / yuva444p12le`
- 文件大小：`1,190,545,156 bytes`
- SHA-256：`7BAEB03E1D91A55EC5ED6CC50D0E423FF81FB0D1AE36F7B9E4DA8D7705B855E8`

补充人工确认的拍摄结构：素材不是单次环绕，而是由多段视频组成；各段均在近似相同的相机高度，围绕同一个物体重复完成 `360°` 旋转。因此，素材在时间上包含多轮重复轨迹，方位角被反复覆盖，但没有通过改变拍摄高度显著增加俯仰方向的视角多样性。

这一结构意味着“视频总帧数”不等于“独立视角数量”。A 的 1686 帧包含大量相同或邻近方位的重复观测；B 的 422 帧也很可能已经覆盖多轮完整方位，但每一轮内部的局部角度采样更稀疏。对本案例的判断需要同时区分全局方位覆盖、局部角度密度、重复观测质量和最终外观密度。

因此可以确认两次运行使用的是字节级完全相同的输入素材。

需要注意，A 与 B 的应用版本、工程 Schema 和引擎所在路径不同，而且一次同时改变了 Quality 和 Planner，不能将全部差异解释成 Planner 的独立净收益。本测试可以评价“当前 Quality v2 + Planner 组合相对历史流程是否有效”，也可以通过阶段日志判断各模块的贡献方向，但不是只切换 Planner 开关的严格 A/B。

## 2. 端到端结果

| 指标 | A：历史基线 | B：Quality v2 + Planner | 变化 |
|---|---:|---:|---:|
| 总耗时 | 2小时18分10.396秒 | 34分02.036秒 | 缩短 1小时44分08.360秒，`-75.4%` |
| 速度倍数 | 1.00× | 4.06× | B 约为 A 的 `4.06×` |
| 输入 SfM 帧数 | 1686 | 422 | `-75.0%` |
| 实际采样密度 | 30.000 fps | 7.509 fps | `-75.0%` |
| 注册图片 | 1686 / 1686 | 422 / 422 | 两者均为 `100%` |
| 三维点 | 378,744 | 48,689 | `-87.1%` |
| 最终 Splat | 378,788 | 62,293 | `-83.6%` |
| PLY 大小 | 85.3 MiB | 14.0 MiB | `-83.6%` |

组合方案在保留约四分之一输入帧的情况下仍完成全部入选画面的注册，总耗时缩短 75.4%。从“生成成功、注册完整、显著降低等待时间”的产品目标看，组合方案有效。

但 B 的三维点、Splat 和文件体积均明显小于 A。较小的 PLY 不能自动等同于质量下降；本次补充的人工视觉验收进一步确认，A 的外观效果明显优于 B，因此在这份素材上，B 的效率收益没有维持与 A 等价的视觉结果。日志中的密度下降与实际外观差异方向一致。

## 3. 阶段耗时

| 阶段 | A：历史基线 | B：Quality v2 + Planner | 变化 |
|---|---:|---:|---:|
| FFprobe | 0.025 秒 | 0.091 秒 | 可忽略 |
| 正式画面提取 | 11.152 秒 | 5.146 秒 | `-53.9%` |
| Feature Extraction | 1分12.563秒 | 20.232 秒 | `-72.1%` |
| Matching | 21.148 秒 | 1分59.383秒 | `+464.5%`，约 5.65× |
| Incremental Mapper | 1小时47分11.044秒 | 5分27.902秒 | `-94.9%`，约 19.61× |
| COLMAP 合计 | 1小时48分44.755秒 | 7分47.517秒 | `-92.8%` |
| Brush | 29分13.442秒 | 26分04.033秒 | `-10.8%` |
| Brush 前总耗时 | 1小时48分56.954秒 | 7分58.003秒 | `-92.7%` |
| 任务总耗时 | 2小时18分10.396秒 | 34分02.036秒 | `-75.4%` |

日志中的已知进程耗时几乎覆盖全部任务耗时：A 只有约 `1.0 秒`、B 约 `5.2 秒`属于进程启动、Planner 分析、状态保存和发布等其他开销，因此阶段对比具有较高可信度。

### 3.1 总节省来自哪里

Mapper 从 `6431.044 秒`降至 `327.902 秒`，单阶段节省 `6103.142 秒`，相当于全部端到端节省时间的约 `97.7%`。

两次运行使用的都是 Incremental Mapper，所以这一收益不是 Mapper 后端变化带来的。主要原因是 Mapper 输入从 1686 帧减少到 422 帧，显著降低了相机注册、三角化和 Bundle Adjustment 的规模。

Feature Extraction 另外节省约 `52.3 秒`，正式抽帧节省约 `6.0 秒`，Brush 节省约 `189.4 秒`。Matching 则多耗时约 `98.2 秒`，抵消了一部分其他阶段收益。

## 4. Frame Planner 分析

B 的 FramePlan 记录：

| 指标 | 数值 |
|---|---:|
| Capture 类型 | `ObjectOrbit` |
| Motion level | `Low` |
| Temporal confidence | 0.95 |
| Loop prior | 0.6961 |
| Capture confidence | 0.95 |
| High 正常目标 FPS | 8.294 |
| 实际平均 FPS | 7.509 |
| 筛选前目标帧数 | 466 |
| 筛选后帧数 | 422 |
| 最低数量回填 | 0 |
| Minimum required FPS | 0.534 |
| Minimum Frame Override | 未触发 |

Planner 将 1686 帧减少到 422 帧，减少 `1264` 帧，即 `75.0%`。入选帧覆盖完整视频时间范围，帧索引大部分以约 4 帧间隔推进，与实际约 7.5 fps 一致。

进一步核对 FramePlan 后可以确认，422 个入选帧之间的 421 个帧间隔全部为 4，即本次结果实际上是严格的均匀时间降采样。入选帧平均清晰度为 `0.20237`，未入选帧为 `0.20135`，差异很小；以每个入选帧附近的四帧窗口计算，入选帧恰好是局部最清晰帧的比例只有 `55 / 422 = 13.0%`。因此，本次降帧主要依赖固定时间间隔，而不是用明显更清晰的画面替代被删除的画面。

结合多段重复 360° 轨迹来看，均匀覆盖完整时间轴并不等同于有效地消除重复视角。它会在每一轮旋转中都降低局部角度密度，同时仍从不同片段保留若干相同或邻近方位的重复观测。Planner 当前记录中没有按圈次或方位角聚类后再选择“同一方位的最佳观测”，所以无法确认 422 帧是否构成了最有效的 422 个视角。B 的 100% 注册说明全局方位覆盖足以完成重建，但不能证明局部表面采样和外观观测已经充分。

这段 56.2 秒素材不是短视频，最低 30 帧保护没有参与决策。422 帧远高于最低目标，因此本案例不能用于评价 Minimum Frame Protection；它主要验证的是 Quality FPS Budget 与自适应筛选。

从结果看，Frame Planner 的效率收益非常明确：输入减少四分之三后，全部 422 帧仍被注册，View Graph 仍保持完整连通，Mapper 耗时降低 94.9%。

## 5. Pairing Planner 分析

两次 Matching 路径不同：

| 项目 | A：历史基线 | B：Quality v2 + Planner |
|---|---|---|
| 策略 | Sequential | Sequential with Loop Closure |
| Sequential overlap | 10 | 20 |
| Loop Detection | 否 | 是 |
| Vocabulary Tree | 否 | 是，本地打包文件 |
| Matching 耗时 | 21.148 秒 | 119.383 秒 |

B 虽然只有约四分之一的帧，却因为 overlap 加倍并执行 Vocabulary Tree Loop Closure，使 Matching 耗时达到 A 的约 `5.65×`。从纯耗时角度看，本次 Pairing Planner 是明确的负收益。

B 生成的 View Graph 指标为：

- 图片数：422；
- 已连接图片：422；
- 连通分量：1；
- 最大连通分量比例：1.0；
- 2-core 比例：1.0；
- bridge 比例：0；
- 中位度数：12；
- 中位 inliers：771；
- 长距离边比例：0.4705；
- Graph decision：`Healthy`。

这些指标说明 B 的匹配图连接强、没有桥接瓶颈，并包含较多长距离联系，与 Loop Closure 的目标一致。但由于没有“同样 422 帧、关闭 Loop Closure”的对照，无法证明健康 View Graph 必须依赖这 98.2 秒额外 Matching 成本。

多轮重复 360° 轨迹也为这些指标提供了新的解释：相隔很远的时间点可能处于相同或相近方位，Vocabulary Tree Loop Closure 很容易建立跨片段长距离边，并让同一批稳定特征形成更长轨迹。因此，`longRangeEdgeRatio = 0.4705`、完整 2-core 和较长 track 不只代表视角覆盖良好，也包含重复轨迹自身带来的连接增益。健康 View Graph 能证明相机网络稳固，但不能单独证明独立表面区域、局部角度采样或外观信息足够。

因此 Pairing Planner 在本案例中的结论是：连通性结果优秀，但时间效率不佳；是否属于必要成本，现有数据不能单独证明。

## 6. Mapper、Graph Gate 与 Recovery

| 项目 | B 的实际决策 |
|---|---|
| GraphQualityGate | `Healthy` |
| Mapper planned | `Incremental` |
| Mapper actual | `Incremental` |
| Mapper reason | `production_incremental_only` |
| Normal Rescue | 0 轮 |
| SuccessRecovery | 未进入 |
| Budget override | 否 |

GraphQualityGate 对健康图没有触发补帧或扩大 Matching，避免了额外 Rescue 成本，这一决策与 View Graph 指标一致。

两次运行的 Mapper 都是 Incremental，因此本测试没有验证 MapperSelector，也没有发生 Global 与 Incremental 的选择收益。B 的 Mapper 加速应归因于更小且仍然连通的 SfM 输入，而不是 Mapper 类型变化。

Normal Rescue、SuccessRecovery、最低帧数保护和 Global 实验路径均未被触发，因此本案例不能评价这些模块的有效性。

## 7. 稀疏重建质量

A 的旧工程没有保存新版 Planner 指标。为统一口径，本次对其归档稀疏模型执行了只读 `COLMAP model_analyzer`；B 使用 Planner 保存的最佳候选指标。

| 指标 | A：历史基线 | B：Quality v2 + Planner | 变化 |
|---|---:|---:|---:|
| 注册率 | 100% | 100% | 相同 |
| 三维点 | 378,744 | 48,689 | `-87.1%` |
| Observations | 4,382,417 | 980,482 | `-77.6%` |
| Mean track length | 11.571 | 20.138 | `+74.0%` |
| Mean observations / image | 2599.298 | 2323.417 | `-10.6%` |
| Mean reprojection error | 0.3269 px | 0.3917 px | 增加 0.0649 px，`+19.8%` |

B 的稀疏点和总观测显著减少，这是输入帧减少后的直接结果。与此同时，平均轨迹长度提高 74.0%，说明保留下来的每个三维点平均获得了更多视角支持；这与健康、高连通的 View Graph 一致。

在多轮重复同高度轨迹下，更长的平均轨迹还可能表示同一三维点在多次经过相近方位时被重复观测。它提高了已有点的支持强度，却不会自动增加新的表面点。B 的帧数是 A 的 `25.0%`，三维点却只有 A 的 `12.9%`，说明几何密度下降速度快于帧数下降；重复圈次和 Loop Closure 主要加强了少量稳定点，而没有补回被局部降采样削弱的细节特征、轮廓点和低纹理区域。

B 的平均重投影误差比 A 高约 0.065 px，但两者都低于 0.4 px，绝对差值较小。B 的每图平均观测下降 10.6%，没有随输入帧数同比下降。

综合这些指标，B 不是简单地以失败或脆弱重建换取速度。它保留了完整注册和较强的跨视角轨迹支持，但稀疏点密度明显降低。结合人工视觉验收，当前可以确认这种密度下降已经转化为可见的外观损失；“注册稳定”与“外观质量充分”在本案例中并不等价。

## 8. 稀疏模型选择与候选验证

B 的 Incremental Mapper 生成了两个模型：

| 模型 | 注册图片 | 三维点 | 平均轨迹 | 重投影误差 |
|---|---:|---:|---:|---:|
| 模型 0 | 2 | 1,211 | 2.000 | 0.1292 px |
| 模型 1 | 422 | 48,689 | 20.138 | 0.3917 px |

Pipeline 在同一次 Mapper 输出内部按注册图片数选择模型 1，再将其记录为 `normal-0-incremental` 候选并交给 Brush。该过程正确避开了只有 2 张图片的小模型，说明稀疏模型校验与候选构建在本案例中有效。

本次只有一个 Planner reconstruction candidate，因此 `ReconstructionComparator` 没有发生多个 Normal / Rescue 候选之间的实质排序。本案例不能单独评价跨候选比较策略。

## 9. Brush 与最终输出

| 项目 | A：历史基线 | B：Quality v2 + Planner |
|---|---:|---:|
| Brush iterations | 30,000 | 30,000 |
| 传入最大分辨率 | 2000 | 1920 |
| Brush 耗时 | 29分13.442秒 | 26分04.033秒 |
| 最终 Splat | 378,788 | 62,293 |
| PLY | 85.3 MiB | 14.0 MiB |

### 9.1 实测外观对比

![Video 002 的 A/B 外观效果对比](assets/quality-v2-planner-video-002-appearance-comparison.png)

*图：用户提供的 video 002 同素材外观效果对比。人工视觉验收中，A 的表面细节、局部连续性和整体完整度明显优于 B。该图片作为定性证据，与下方稀疏点和最终 Splat 数量的定量差异结合判断。*

两次 Brush 迭代数相同，B 的最大分辨率按源素材尺寸限制为 1920，A 传入 2000。B 的 Brush 只缩短 10.8%，远小于 SfM 阶段的 92.8% 降幅，说明固定 30,000 次训练仍是 B 的主要耗时来源。

两次运行使用的 Brush 可执行文件 SHA-256 完全相同，COLMAP 可执行文件 SHA-256 也完全相同，因此可以排除引擎二进制版本差异。两边源图宽度均为 1920，A 的 2000 与 B 的 1920 是最大分辨率上限，这个参数差异也不足以解释约 6 倍的最终 Splat 差距。抽查的对应入选帧文件为字节级一致，A、B 每图平均 SIFT 特征分别为 `3203.68` 和 `3203.04`，特征上限没有造成显著的单帧特征差异。

A 从 378,744 个稀疏点得到 378,788 个最终 Splat，最终数量几乎与初始稀疏点一致；B 从 48,689 个稀疏点增长到 62,293 个 Splat，虽增加约 27.9%，最终仍只有 A 的 16.4%。这表明当前 Brush 的最终表达密度高度依赖 COLMAP 提供的稀疏几何种子，固定 30,000 次训练没有补回因降帧而缺失的大量点。人工观察到的 B 外观下降与这一数据关系一致。

重复同高度环绕能够增加相同可见表面的重复观测，却不能提供新的俯仰视角；A、B 都受这一素材结构限制。A 的优势主要来自更密集的局部角度与重复观测形成了更多稀疏点和 Gaussian，而不是获得了额外的顶部或底部几何覆盖。

## 10. 归因边界

### 可以直接确认

- 两次输入视频完全相同。
- Quality v2 + Planner 组合将 SfM 输入从 1686 帧降至 422 帧。
- 422 帧全部注册，View Graph 为 Healthy，没有触发 Rescue。
- 组合方案总耗时缩短 75.4%，Brush 前耗时缩短 92.7%。
- 最大收益来自更少输入帧带来的 Incremental Mapper 降本，不来自 Global Mapper 或 Mapper 后端切换。
- Pairing Planner 的 Loop Closure 在本案例中增加约 98.2 秒 Matching 时间。
- Pipeline 的稀疏模型校验正确选择了 422 张图片的完整模型，而不是 2 张图片的小模型。
- Brush 与 COLMAP 二进制在两次运行中完全相同；两边每图平均 SIFT 特征数基本一致。
- 人工视觉验收确认 A 的外观效果明显优于 B，二者在本素材上不具备视觉质量等价性。

### 可以合理推断，但不是独立因果证明

- 均匀降帧去除了大量时间冗余，并保留了足以完成全注册的全局方位覆盖，但没有保留与 A 等价的局部角度密度和外观表达密度。
- Loop Closure 与较高长距离边比例、完整 2-core 和零 bridge 的健康图结构相符。
- 更长的平均轨迹表明降帧后保留的稀疏点具有较强的多视角支持，同时也受到多轮重复方位观测的强化。
- B 的视觉损失主要与稀疏点和最终 Splat 密度下降有关，而不是引擎版本、训练迭代数或单帧特征数量差异。

### 不能从本组数据单独确认

- Planner 相对于“同一 Quality v2、仅关闭 Planner”的独立净收益。
- Loop Closure 是否是达到 100% 注册所必需的。
- Minimum Frame Protection、Normal Rescue 和 SuccessRecovery 是否有效。
- Global Mapper 与 Incremental Mapper 的相对效果。
- Quality v2 与 Planner 各自对视觉损失承担的独立比例，因为本次没有“同一 Quality v2、仅关闭 Planner”的严格对照。

## 11. Planner 有效性结论

在这份 56.2 秒、由多段同高度重复 360° 环绕组成的视频上，当前 Quality v2 + Planner 组合对“生成成功与时间效率”的作用明确有效：它只使用原始帧数的约 25%，仍实现入选帧 100% 注册、单一健康 View Graph、较长的特征轨迹，并把端到端时间从 2小时18分10秒降至 34分02秒。

Planner 的主要时间收益来自 Frame Planning 和对健康图不执行 Rescue。它们共同缩小了 Incremental Mapper 的规模，使 Mapper 耗时减少 94.9%；稀疏模型校验也正确保留了完整模型。但是，本次 FramePlan 实际采用固定四帧间隔，没有按重复圈次和方位选择最佳观测。对这种周期性轨迹，覆盖完整时间轴并不能保证保留最有效的视角集合。

Planner 的 Pairing 决策在时间维度上表现不佳：Sequential with Loop Closure 比历史 Sequential 多耗时约 98.2 秒。虽然最终图结构优秀，但本组数据不能证明这部分额外成本不可缺少。

从几何指标看，组合方案维持了完整注册，平均轨迹长度更高，重投影误差只增加约 0.065 px；但重复轨迹会天然强化 Loop Closure、长距离边和已有点的 track length，这些健康指标没有反映独立表面点密度。B 的三维点只有 A 的 12.9%，最终 Splat 只有 A 的 16.4%，而 Brush 没有在相同 30,000 次训练内补回差距。

人工视觉验收已经确认 A 的外观明显优于 B，因此本案例不再只是“视觉等价性未被证明”，而是已经观察到明确的质量回退。更准确的综合判断为：**Planner 显著提高了时间效率并保持了重建成功，但当前针对重复同高度环绕素材的均匀降帧过度压缩了局部观测与稀疏几何密度；Graph Healthy 和 100% 注册未能预测这一外观损失，Pairing 还额外增加了 Matching 时间。**

## 12. 当前 High 与 COLMAP-style High 严格对照实验（2026-09-25）

本节在上述 B 组 `Quality v2 High + Planner v2` 基础上，单独验证 COLMAP High Quality 风格参数是否能改善 422 帧方案的稀疏几何密度。为避免与本文原有 A/B 组别混淆，本节将两组记为：

- H0：当前 OOOSplat High，沿用项目 `20260920-190836_002` 的结果；
- H1：当前 OOOSplat High + Affine SIFT + Guided Matching + 更充分 BA。

### 12.1 控制变量与实验参数

H1 直接复用 H0 已生成的 422 张 PNG 及 422 张 Alpha Mask，没有重新运行 Capture Analyzer、Frame Planner 或 Smart Frame Filter。两组使用相同的图片文件、相机模型、Pairing 策略、Sequential overlap、Loop Closure、Vocabulary Tree、Incremental Mapper、Brush 参数和引擎二进制。

源图最长边为 1920，因此 High 的 `max_image_size = 2400` 预算在两组中的实际执行值均被源分辨率限制为 `1920`；`max_num_features` 均为 `8192`。

| 参数 | H0：当前 High | H1：COLMAP-style High |
|---|---:|---:|
| SfM frames | 422 | 422 |
| SIFT max image size（实际） | 1920 | 1920 |
| SIFT max features | 8192 | 8192 |
| `SiftExtraction.estimate_affine_shape` | 默认 `0` | `1` |
| `SiftExtraction.domain_size_pooling` | 默认 `0` | `0` |
| `FeatureMatching.guided_matching` | 默认 `0` | `1` |
| `Mapper.ba_local_max_num_iterations` | 默认 `25` | `30` |
| `Mapper.ba_local_max_refinements` | 默认 `2` | `3` |
| `Mapper.ba_global_max_num_iterations` | 默认 `50` | `75` |
| Pairing | Sequential + Loop Closure | 相同 |
| Sequential overlap / loop images | 20 / 32 | 20 / 32 |
| Mapper | Incremental | Incremental |
| Brush | 30000 @ 1920 | 30000 @ 1920 |

本次参数名均由当前打包 COLMAP 4.x 的实际 `-h` 输出确认。实验没有启用 DSP-SIFT、Global Mapper、额外补帧、更多 neighbors、Rescue 或其他重建阈值修改。

工程中的实验开关为环境变量 `OOOSPLAT_COLMAP_HIGH_QUALITY_EXPERIMENT`，默认关闭。只有值为 `1`、`true`、`yes` 或 `on` 且 Quality 为 High 时，才应用 H1 参数；其他情况保持 H0 行为。实验状态会写入项目检查点，恢复任务时若环境开关与检查点不一致，流水线会拒绝混用参数。

### 12.2 Feature Extraction 与 Matching

| 指标 | H0：当前 High | H1：COLMAP-style High | 变化 |
|---|---:|---:|---:|
| Detected features | 1,351,683 | 1,580,026 | `+16.9%` |
| Mean features / image | 3,203.040 | 3,744.137 | `+16.9%` |
| Median features / image | 3,281.5 | 3,865.5 | `+17.8%` |
| Raw match pairs | 3,151 | 3,033 | `-3.7%` |
| Raw matches | 2,547,587 | 3,053,771 | `+19.9%` |
| Geometrically verified pairs | 2,931 | 2,961 | `+1.0%` |
| Verified correspondences | 2,524,443 | 3,509,273 | `+39.0%` |

Affine SIFT 使每图检测特征数增加约 16.9%。Guided Matching 没有显著扩大已验证图像对的数量，verified pairs 只增加 30 对；它的主要收益是让已匹配图像对中的有效对应关系更密集，verified correspondences 增加约 98.5 万，增幅 39.0%。

H1 的 raw match pairs 比 H0 少 118 对，但 raw matches 增加约 50.6 万。该结果表明实验参数没有通过增加 Pairing 范围获得收益，而是在基本相同的图连接范围内增加单对图片的特征对应密度，符合本次控制变量要求。

### 12.3 稀疏重建结果

两组 Incremental Mapper 均产生两个模型，流水线口径均选择注册图片最多的完整模型。H1 的另一个模型只注册 2 张图片，因此未参与 Brush。

| 指标 | H0：当前 High | H1：COLMAP-style High | 变化 |
|---|---:|---:|---:|
| Registered images | 422 / 422 | 422 / 422 | 均为 `100%` |
| Sparse 3D Points | 48,689 | 54,980 | `+12.9%` |
| Observations | 980,482 | 1,271,600 | `+29.7%` |
| Mean track length | 20.138 | 23.128 | `+14.9%` |
| Mean observations / image | 2,323.417 | 3,013.270 | `+29.7%` |
| Mean reprojection error | 0.3917 px | 0.7244 px | `+84.9%` |

H1 在不增加帧数和 Pairing 范围的情况下新增 6,291 个稀疏点，并显著增加 Observations 和平均轨迹长度。说明 Affine SIFT、Guided Matching 与更充分 BA 确实提高了这组 422 帧数据的稀疏几何密度和多视角支持强度。

但密度提升伴随明显更高的平均重投影误差：从 `0.3917 px` 增至 `0.7244 px`。H1 仍完成 100% 注册，且误差绝对值低于 1 px，但新增几何并没有保持 H0 的拟合精度。因此，本实验得到的是“更密但更松”的稀疏模型，而不是所有质量指标同时改善。

### 12.4 阶段耗时与最终输出

| 指标 | H0：当前 High | H1：COLMAP-style High | 变化 |
|---|---:|---:|---:|
| Feature Extraction | 20.232 秒 | 56.046 秒 | `+177.0%` |
| Matching | 119.383 秒 | 126.227 秒 | `+5.7%` |
| Incremental Mapper | 327.902 秒 | 604.403 秒 | `+84.3%` |
| COLMAP total | 467.517 秒 | 786.676 秒 | `+68.3%` |
| Brush | 1,564.033 秒 | 928.989 秒 | `-40.6%` |
| COLMAP + Brush | 2,031.550 秒 | 1,715.665 秒 | `-15.6%` |
| Final splats | 62,293 | 67,221 | `+7.9%` |
| PLY bytes | 14,702,698 | 15,865,706 | `+7.9%` |

COLMAP 本身的成本明显上升：Feature Extraction 增加 35.814 秒，Mapper 增加 276.501 秒，COLMAP 合计增加 319.159 秒，即 68.3%。其中 Guided Matching 只增加 6.844 秒；主要成本来自 Affine SIFT 和更充分 BA。

H1 的 Brush 实测比 H0 快约 10分35秒，因此本次手工固定帧对照的 `COLMAP + Brush` 合计反而缩短 15.6%。这一变化不能归因于 COLMAP High 参数：两组 Brush 配置相同，而 GPU 运行状态、缓存与 Brush autotune 状态并未作为独立变量锁定。可靠的耗时结论仅限于 COLMAP 阶段增加 68.3%；Brush 和合计耗时只记录实测值，不作为实验参数带来加速的证据。本节也没有重新执行素材导入和画面准备，因此不将合计值称为完整端到端耗时。

最终 Splat 增加 4,928 个，增幅 7.9%，低于稀疏点的 12.9% 增幅。H0 从 48,689 个稀疏点增长到 62,293 个 Splat，增幅约 27.9%；H1 从 54,980 个稀疏点增长到 67,221 个 Splat，增幅约 22.3%。这说明新增稀疏点有一部分传递到了最终高斯数量，但 Brush 没有按同等比例放大其收益。

### 12.5 实验结论

在 video 002 固定 422 帧的严格对照下，COLMAP-style High 参数对“Sparse Geometry Density”有效：Detected features 增加 16.9%，Verified correspondences 增加 39.0%，Sparse 3D Points 增加 12.9%，Observations 增加 29.7%，最终 Splat 增加 7.9%。这些收益不是由更多帧、更大 Pairing 范围、Global Mapper 或 Brush 参数变化造成的。

代价同样明确：COLMAP 总耗时增加 68.3%，而平均重投影误差增加 84.9%。相对于约 5.3 分钟的额外 COLMAP 时间，得到约 6,291 个额外稀疏点和 4,928 个额外 Splat。就本素材而言，几何密度收益真实存在，但低于耗时增幅，并伴随拟合精度下降。

本次没有生成与 H0 完全一致观察视角的人工外观截图，因此只能确认稀疏几何和最终表达数量增加，不能仅凭 Splat 数断言高斯泼溅外观已经改善。结合本文原有结论，H1 仍只有历史 1686 帧基线三维点数量的约 14.5% 和最终 Splat 数量的约 17.7%，COLMAP-style High 能部分补回 Planner 降帧造成的密度损失，但没有恢复到历史全帧结果的规模。

## 13. High Geometry Screening + Probe 实测（2026-09-26）

本节记录 High 档加入一次性 Geometry Screening + Probe 后，对同一 video 002 素材的首次完整端到端运行。实验工程为 `20260926-113010_002`，Planner version 为 `3`，COLMAP-style High 实验开关关闭；因此本次新增变量是 Geometry Screening 与定向补帧 Probe，而不是 Affine SIFT、Guided Matching 或更高 BA 参数。

为避免跨次运行的小幅非确定性干扰，本节优先使用本次工程 checkpoint 中保存的 Initial reconstruction 与 Geometry Probe reconstruction 做内部前后对照；同时以 H0 工程 `20260920-190836_002` 作为最终输出和端到端耗时参考。

### 13.1 Screening 判定

第一次 Incremental Mapper 使用 422 帧得到可用 reconstruction，422 帧全部注册。Geometry Screening 的聚合结果如下：

| 指标 | Initial 实测值 | Provisional threshold | 判定 |
|---|---:|---:|---|
| Sparse 3D Points | 48,679 | — | — |
| Observations | 980,579 | — | — |
| Mean track length | 20.144 | `> 14.0` | Track 条件命中 |
| Point diversity ratio | 0.04964 | `< 0.06` | Diversity 条件命中 |
| Median triangulation ratio | 0.73285 | `< 0.30` | 未命中 |
| P25 triangulation ratio | 0.69276 | `< 0.15` | 未命中 |
| Minimum triangulation ratio | 0.41009 | 仅 telemetry | 不参与单点触发 |
| Continuous weak intervals | 0 | 至少一段达到连续长度门槛 | 未命中 |

本次 `triangulation_underfilled=false`、`continuous_weak_region=false`，说明 422 帧模型并不存在全局特征三角化不足，也没有检测到连续的几何薄弱时间区间。实际唯一触发项为：

`point_diversity_ratio < 0.06 AND mean_track_length > 14.0`

因此 `track_redundancy_high=true`，Screening 决策为 `ProbeRecommended`。这与此前对 video 002 的判断一致：重复的同高度 360° 环绕使大量 observations 反复支持相对有限的一批独立三维点；Graph Healthy、100% 注册和较长 Track 并不能代表独立表面点已经足够丰富。

### 13.2 Probe 执行结果

Probe 预算按当前 422 帧的 12% 计算，`ceil(422 × 0.12) = 51`。High 的 15 fps 上限在 56.2 秒素材上允许约 843 帧，因此此次 51 帧补充未触碰 Quality v2 上限。由于只命中 Track Redundancy，Targeted Backfill 使用 Largest Temporal Gap 策略，将新增帧分散插入原有选帧的较大时间间隔，而不是按 Candidate Pool 顺序连续取帧。

| Probe 状态 | 实测值 |
|---|---:|
| Triggered reason | `trackRedundancy` |
| Selected before | 422 |
| Requested additional | 51 |
| Actual additional | 51 |
| Selected after | 473 |
| Probe status | `completed` |
| Probe duration | 425.918 秒 |
| Mapper | Incremental |

只对 51 张新增帧执行的画面提取、Feature Extraction 和局部 Matching 分别耗时 3.045 秒、3.044 秒和 2.038 秒；Probe Incremental Mapper 耗时 417.018 秒。Probe 的绝大部分成本来自重新构建 473 帧 sparse model，而不是新增帧解码、特征提取或局部匹配。

### 13.3 Initial 与 Probe reconstruction 对照

| 指标 | Initial（422 帧） | Geometry Probe（473 帧） | 变化 |
|---|---:|---:|---:|
| Registered images | 422 / 422 | 473 / 473 | 均为 `100%` |
| Sparse 3D Points | 48,679 | 53,247 | `+9.4%` |
| Observations | 980,579 | 1,118,578 | `+14.1%` |
| Mean track length | 20.144 | 21.007 | `+4.3%` |
| Mean reprojection error | 0.3916 px | 0.3851 px | `-1.7%` |

新增帧数量增加 12.1%，Sparse 3D Points 增加 9.4%，Observations 增加 14.1%。点数增幅略低于帧数增幅，但新增帧不只是重复增加相同点的观测：独立三维点实际增加了 4,568 个，同时 observations 增加 137,999 个。平均 Track Length 继续提高，而平均重投影误差由 0.3916 px 小幅降至 0.3851 px，说明本次几何增量没有以注册率或拟合精度恶化为代价。

Probe reconstruction 通过现有可用性校验，473 帧全部注册，最终工程保存的 `53,247` 个 Sparse 3D Points 与 Probe candidate 一致，说明流水线实际采用了 Probe 结果，没有回退到 Initial reconstruction。本次只执行了一轮 Probe，没有进入循环式 enrichment。

### 13.4 最终输出与耗时

| 指标 | H0：原 High + Planner | H2：High Geometry Probe | 变化 |
|---|---:|---:|---:|
| SfM frames | 422 | 473 | `+12.1%` |
| Sparse 3D Points | 48,689 | 53,247 | `+9.4%` |
| Final splats | 62,293 | 65,703 | `+5.5%` |
| PLY bytes | 14,702,698 | 15,507,458 | `+5.5%` |
| Brush | 1,564.033 秒 | 919.514 秒 | `-41.2%` |
| 完整端到端耗时 | 2,042.036 秒（34分02秒） | 1,796.101 秒（29分56秒） | `-12.0%` |

最终 Splat 增加 3,410 个，增幅 5.5%，低于 Sparse 3D Points 的 9.4% 增幅，说明 Probe 增加的稀疏几何只有一部分继续转化为最终高斯数量。最终 PLY 为 15,507,458 bytes，界面显示约 14.8 MB。

虽然 H2 加入 Probe 后端到端实测反而比 H0 短约 4分06秒，但这不能解释为 Probe 带来了加速。Probe 自身明确增加了 425.918 秒；合计时间下降主要来自本次 Brush 比 H0 快约 10分45秒。两次 Brush 参数相同，GPU 状态、缓存和 autotune 状态并未作为独立变量锁定，因此 Brush 波动和端到端缩短只能作为本次运行记录，不能归因于 Geometry Probe。

### 13.5 与 H1 COLMAP-style High 的关系

H1 在固定 422 帧上通过 Affine SIFT、Guided Matching 和更充分 BA 得到 54,980 个 Sparse 3D Points 与 67,221 个最终 Splat；H2 使用默认 High COLMAP 参数，通过增加 51 张定向帧得到 53,247 个 Sparse 3D Points 与 65,703 个最终 Splat。H2 分别比 H1 少约 3.2% 和 2.3%，但平均重投影误差为 0.3851 px，明显低于 H1 的 0.7244 px。

两组实验改变的变量不同，不能据此宣称其中一种方案在所有素材上更优；但对 video 002 而言，一次 12% 的定向补帧在不启用更激进 COLMAP 参数的情况下，已经获得接近 H1 的稀疏点和最终 Splat 规模，并保持了 H0 水平的低重投影误差。

### 13.6 实验结论

本次 High Geometry Screening 正确识别了 video 002 的核心模式：问题不是 Feature 大量未三角化，也不是连续时间区间完全缺少几何，而是 observations 过度集中于较少的独立三维点。由 Track Redundancy 触发的一次 Largest Temporal Gap Probe 成功增加 51 帧，并使 Sparse 3D Points、Observations 和最终 Splat 分别提高 9.4%、14.1% 和 5.5%；注册率保持 100%，重投影误差还小幅下降。

因此，从可直接测量的几何指标看，本次实验对 High 档有效：它在保持 bounded、只执行一次 Probe 的前提下，补回了部分均匀降帧造成的几何密度损失，而且没有破坏已有可用 reconstruction。收益仍然有限：H2 的 Sparse 3D Points 仅为历史 1686 帧基线的约 14.1%，最终 Splat 约为历史基线的 17.3%，远未恢复全帧模型规模。

本次用户提供的截图是项目结果卡片，只能确认工程完成、耗时、档位与 PLY 大小，没有提供与 H0 相同观察视角的高斯泼溅渲染对比。因此本节可以确认 Geometry Probe 带来了定量几何增益，但不能仅凭点数、Splat 数或文件大小断言外观质量已有可见改善。
