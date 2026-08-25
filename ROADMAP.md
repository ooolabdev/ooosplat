# OOOSplat Roadmap

本路线图用于说明 OOOSplat 的产品方向和实施优先级。P0–P3 表示相对优先顺序，不代表版本号，也不承诺具体发布日期。

## 优先级说明

- **P0 · 近期重点**：优先推进核心体验与重点平台工作。
- **P1 · 高优先级**：扩展输入方式并提升重建性能。
- **P2 · 平台扩展**：将完整桌面工作流带到更多操作系统。
- **P3 · 中长期探索**：面向更多拍摄方式和使用场景进行能力探索。

## 路线图

| 优先级 | 事项 | 目标 | GitHub Issue |
| --- | --- | --- | --- |
| P0 | 增加高斯泼溅预览 | 在 OOOSplat 中直接查看生成的 Gaussian Splatting 结果，减少依赖外部查看工具。 | [#3 关于集成查看功能](https://github.com/ooolabdev/ooosplat/issues/3) |
| P0 | 失败或暂停后支持断点续跑 | 保留可复用的阶段结果，使中断任务能够从合适的处理阶段继续。 | 待创建 |
| P0 | macOS 版本 | 为 Apple Silicon Mac 提供完整的 OOOSplat 桌面应用与内置引擎工作流。 | [#4 Add macOS support](https://github.com/ooolabdev/ooosplat/issues/4) |
| P1 | 支持输入图片序列 | 允许用户以有序图片集作为重建输入，而不必先制作视频文件。 | 待创建 |
| P1 | CUDA 加速版本 | 为受支持的 NVIDIA GPU 提供可选的 CUDA 重建流水线，同时保留 CPU 路径。 | [#6 Add CUDA-accelerated pipeline for NVIDIA GPUs](https://github.com/ooolabdev/ooosplat/issues/6)；相关用户反馈 [#2](https://github.com/ooolabdev/ooosplat/issues/2) 已关闭 |
| P3 | 全景视频支持 | 探索将全景视频作为输入并生成可用 Gaussian Splatting 结果的工作流。 | 待创建 |

## 已完成

| 状态 | 事项 | 交付 | GitHub Issue |
| --- | --- | --- | --- |
| 已完成 | Ubuntu 24.04 Alpha | 为 x86_64 提供从源码构建的无安装包桌面应用和 CLI，使用系统 FFmpeg/FFprobe/CPU COLMAP 与固定版本 Brush；不代表支持其他 Linux 发行版。 | [#5 Add Linux support](https://github.com/ooolabdev/ooosplat/issues/5)，由 [PR #10](https://github.com/ooolabdev/ooosplat/pull/10) 交付 |

## 跟踪与贡献

实际功能范围、技术讨论和实施进度以关联的 GitHub Issue 为准。欢迎在对应 Issue 中补充使用场景、参与讨论或贡献代码。

标记为“待创建”的事项尚无独立 Issue；创建后应将本页对应条目替换为固定的 Issue 编号和链接。路线图会根据项目反馈和实现条件调整，优先级变化不代表功能被取消。
