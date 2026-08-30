# OOOSplat Roadmap

本路线图用于说明 OOOSplat 的产品方向和实施优先级。P0–P3 表示相对优先顺序，不代表版本号，也不承诺具体发布日期。

## 产品原则

- **一键工作流**：持续减少引擎配置和手动操作，让用户从输入素材直接获得可用的 Gaussian Splatting 结果。
- **本地优先**：重建、训练、预览和导出优先使用用户本机算力，不依赖云端处理服务。
- **安全与非破坏**：素材和工程数据默认保留在本地；编辑和导出尽量保留原始结果，并让文件位置和处理状态清晰可追踪。

## 优先级说明

- **P0 · 近期重点**：优先推进核心体验与重点平台工作。
- **P1 · 高优先级**：扩展输入方式并提升重建性能。
- **P2 · 平台扩展**：将完整桌面工作流带到更多操作系统。
- **P3 · 中长期探索**：面向更多拍摄方式和使用场景进行能力探索。

## 路线图

| 优先级 | 事项 | 目标 | GitHub Issue |
| --- | --- | --- | --- |
| P0 | 失败或暂停后支持断点续跑 | 保留可复用的阶段结果，使中断任务能够从合适的处理阶段继续。 | 待创建 |
| P1 | 提升重建性能（GLOMAP / 词汇树环路） | 用 GLOMAP 全局建图替代增量 mapper 以提速并提高注册率；用词汇树/环路检测提升环绕视频首尾闭环、降低“注册率<50%”失败。依赖：内置 COLMAP 需含 `global_mapper`；词汇树需 faiss 兼容树（当前公开预训练树为 flann，与内置 faiss 版 COLMAP 不兼容）。 | 待创建 |
| P3 | 全景视频支持 | 探索将全景视频作为输入并生成可用 Gaussian Splatting 结果的工作流。 | 待创建 |

## 已完成

| 状态 | 事项 | 交付 | GitHub Issue |
| --- | --- | --- | --- |
| 已完成 | Ubuntu 24.04 Alpha | 为 x86_64 提供 `.deb` 桌面安装包和 CLI，使用系统 FFmpeg/FFprobe/CPU COLMAP 与安装包内固定版本 Brush；不代表支持其他 Linux 发行版。 | [#5 Add Linux support](https://github.com/ooolabdev/ooosplat/issues/5)，由 [PR #10](https://github.com/ooolabdev/ooosplat/pull/10) 交付 |
| 已完成 | Apple Silicon macOS Alpha | 为 macOS 15+ arm64 提供随应用交付的 FFmpeg、FFprobe、CPU COLMAP 和 Brush 工作流。 | [#4 Add macOS support](https://github.com/ooolabdev/ooosplat/issues/4)，由 [PR #8](https://github.com/ooolabdev/ooosplat/pull/8) 交付 |
| 已完成 | 内嵌高斯泼溅预览 | 已支持加载 `.ply`、相机浏览、整体 Transform、撤销 / 重做、动画预览，以及非破坏式 Gaussian 和竖屏视频导出。 | [#3 关于集成查看功能](https://github.com/ooolabdev/ooosplat/issues/3) |
| 已完成 | COLMAP CUDA 自动加速 | 已支持检测 NVIDIA 驱动和 Compute Capability，满足要求时自动启用 GPU 特征提取与匹配，否则无中断地回退 CPU。 | [#6 Add CUDA-accelerated pipeline for NVIDIA GPUs](https://github.com/ooolabdev/ooosplat/issues/6)；相关用户反馈 [#2](https://github.com/ooolabdev/ooosplat/issues/2) |
| 已完成 | 支持输入图片序列 | 新增“图片文件夹 / 图片序列”输入入口：前端可选视频或图片文件夹；后端按输入类型自动选择匹配器（图片→穷举匹配，视频→顺序匹配）并复用整条 COLMAP + Brush 链路，无需先制作视频。 | 本次实现（待建 Issue） |
| 已完成 | 重建性能基础优化 | 特征提取 `max_num_features=8192` 提升特征密度；图片序列默认 `exhaustive_matcher` 提升无序图集注册率；新增 GLOMAP 与词汇树/环路匹配的代码接入（运行时检测 + 自动回退，避免旧引擎/缺资产时破坏流程）。 | 本次实现（待建 Issue） |
| 已完成 | Windows NSIS 客户端打包 | 补齐构建环境（Rust MSVC + VS2022 BuildTools + SDK），使用国内镜像下载并校验内置引擎（FFmpeg/COLMAP/Brush），产出 `OOOSplat-0.3.0-x64-setup.exe`（含内置引擎，本地未签名）。 | 本次实现（待建 Issue） |

## 跟踪与贡献

实际功能范围、技术讨论和实施进度以关联的 GitHub Issue 为准。欢迎在对应 Issue 中补充使用场景、参与讨论或贡献代码。

标记为“待创建”的事项尚无独立 Issue；创建后应将本页对应条目替换为固定的 Issue 编号和链接。路线图会根据项目反馈和实现条件调整，优先级变化不代表功能被取消。
