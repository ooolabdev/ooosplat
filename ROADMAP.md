# OOOSplat Roadmap

[中文](ROADMAP.md) | [English](ROADMAP_EN.md)

本路线图用于说明 OOOSplat 的产品方向和实施优先级。P0–P3 表示相对优先顺序，不代表版本号，也不承诺具体发布日期。

当前版本：**0.4.0**。本版本重点完善多类型素材输入、可恢复生成流程、进度反馈与非破坏式 Gaussian 编辑。

## 产品原则

- **一键工作流**：持续减少引擎配置和手动操作，让用户从输入素材直接获得可用的 Gaussian Splatting 结果。
- **本地优先**：重建、训练、预览和导出优先使用用户本机算力，不依赖云端处理服务。
- **安全与非破坏**：素材和工程数据默认保留在本地；编辑和导出尽量保留原始结果，并让文件位置和处理状态清晰可追踪。

## 优先级说明

- **P0 · 近期重点**：稳定性、速度、生成质量与错误体验。
- **P1 · 高优先级**：评测体系、数据沉淀、拍摄指导与产品体验。
- **P2 · 能力扩展**：新增输出格式、项目导入、补拍和 AI 调用能力。
- **P3 · 中长期探索**：推广与更多拍摄、使用场景。

## 待开发

| 优先级 | 事项 | 目标 | GitHub Issue |
| --- | --- | --- | --- |
| P0 | 超大 PLY 预览稳定性与性能 | 解决 WebView2 加载超过 1 GB PLY 时的内存峰值和 OOM 闪退，改善大规模 Gaussian 的加载、编辑与退出稳定性。 | [#21](https://github.com/ooolabdev/ooosplat/issues/21) |
| P0 | 速度优化 | 缩短素材准备、特征提取、匹配、重建和训练的总体耗时，同时保持现有结果兼容性。 | 待创建 |
| P0 | 生成质量优化 | 改善相机注册、几何完整性、细节表现和边缘质量，并建立可验证的优化策略。 | 待创建 |
| P0 | 报错提示优化 | 将引擎、素材、磁盘、GPU 和重建错误转换为清晰、可执行的用户提示与恢复建议。 | 待创建 |
| P0 | UI 简化 | 减少不必要的信息与操作层级，统一生成、历史任务和预览编辑的交互。 | 待创建 |
| P1 | 高斯泼溅生成 Benchmark | 建立可复现的素材集、硬件环境、质量指标和耗时指标，持续比较不同版本的速度、资源占用与结果质量。 | 待创建 |
| P1 | 数据搜集与沉淀 | 已具备活跃用户、Pipeline performance 和 Failure distribution 数据，后续完善数据质量、长期样本、分析看板与版本对比。 | 待创建 |
| P1 | 视频拍摄方法提示 UI | 在选择素材和开始生成前提供环绕路径、运动速度、重叠率、光照与常见问题提示。 | 待创建 |
| P1 | 新版本自动提示 | 检测可用的新版本，向用户展示版本信息、更新说明和安全下载入口。 | 待创建 |
| P2 | 导出 Mesh | 将重建结果转换并导出为常见 Mesh 格式，明确纹理、坐标系与质量选项。 | 待创建 |
| P2 | MCP 工具支持 | 提供可供 AI Agent 调用的安全 MCP 工具，覆盖素材分析、生成、状态查询、续跑和结果获取。 | 待创建 |
| P2 | 在“02 历史任务”中新增项目并导入 PLY | 允许不经过生成流程直接导入已有 PLY，建立 OOOSplat 项目并进入预览和编辑。 | 待创建 |
| P2 | 补拍支持 | 允许向已有项目补充视频或图片素材，重新匹配和重建缺失区域，同时保留可复用结果。 | 待创建 |
| P3 | 全景视频支持 | 探索将全景视频作为输入并生成可用 Gaussian Splatting 结果的工作流。 | 待创建 |

## 已完成

| 状态 | 事项 | 交付 | GitHub Issue / PR |
| --- | --- | --- | --- |
| 已完成 | Ubuntu 24.04 Alpha | 为 x86_64 提供 `.deb` 桌面安装包和 CLI，使用系统 FFmpeg/FFprobe/CPU COLMAP 与安装包内固定版本 Brush。 | [#5](https://github.com/ooolabdev/ooosplat/issues/5) / [PR #10](https://github.com/ooolabdev/ooosplat/pull/10) |
| 已完成 | Apple Silicon macOS Alpha | 为 macOS 15+ arm64 提供随应用交付的 FFmpeg、FFprobe、CPU COLMAP 和 Brush 工作流。 | [#4](https://github.com/ooolabdev/ooosplat/issues/4) / [PR #8](https://github.com/ooolabdev/ooosplat/pull/8) |
| 已完成 | 内嵌高斯泼溅预览与动画导出 | 支持 `.ply` 加载、相机浏览、整体 Transform、动画预览，以及 Gaussian 和竖屏视频导出。 | [#3](https://github.com/ooolabdev/ooosplat/issues/3) |
| 已完成 | COLMAP CUDA 自动加速 | 自动检测 NVIDIA 驱动和 Compute Capability，满足要求时启用 GPU 特征提取与匹配，否则回退 CPU。 | [#6](https://github.com/ooolabdev/ooosplat/issues/6) |
| 已完成 · 0.4.0 | 阶段级断点续跑与时间预估 | 校验并复用画面、Mask、特征、匹配和稀疏重建检查点；损坏或缺失阶段自动回退重跑，并提供预计时长。 | [PR #27](https://github.com/ooolabdev/ooosplat/pull/27) |
| 已完成 · 0.4.0 | Brush 训练进度展示优化 | 依据 Brush 实际训练 step 持续更新训练阶段百分比，使长时间训练过程可见、可追踪。 | 0.4.0 实现 |
| 已完成 · 0.4.0 | 去除 50% 注册率终止限制 | 只要存在有效注册图像和三维点即可继续进入 Brush；低注册率保留质量警告，不再因低于 50% 自动终止。 | 0.4.0 实现 |
| 已完成 · 0.4.0 | 透明视频与透明图片自动 Mask | 自动识别透明 MOV 和 PNG，保留 RGBA 数据供 Brush 使用，并生成 COLMAP Mask 排除透明背景。 | [PR #19](https://github.com/ooolabdev/ooosplat/pull/19) 及后续实现 |
| 已完成 · 0.4.0 | 图片序列输入 | 统一视频与图片输入入口；图片序列使用共享相机、穷举匹配和增量 Mapper，透明 PNG 自动生成 Mask。 | [PR #19](https://github.com/ooolabdev/ooosplat/pull/19) |
| 已完成 · 0.4.0 | Gaussian 编辑 | 支持矩形、球形和盒形选择、非破坏式删除、裁切冻结、撤销 / 重做，以及保存为 `edit.ply`；为后续 AI Agent 模式保留兼容基础。 | 0.4.0 实现（待建 Issue） |
| 已完成 · 0.4.0 | 应用英文版与中英文切换 | 支持简体中文与英文即时切换，按系统语言选择首次默认值，并持久保存用户的手动选择；覆盖任务、设置、预览、状态和操作提示。 | 0.4.0 实现（待建 Issue） |

## 跟踪与贡献

实际功能范围、技术讨论和实施进度以关联的 GitHub Issue 为准。欢迎在对应 Issue 中补充使用场景、参与讨论或贡献代码。

标记为“待创建”的事项尚无独立 Issue；创建后应将本页对应条目替换为固定的 Issue 编号和链接。路线图会根据项目反馈和实现条件调整，优先级变化不代表功能被取消。
