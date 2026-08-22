# OOOSplat

OOOSplat 是一款面向 Windows 的本地视频转 3D Gaussian Splatting 桌面应用。发布流程会将 FFmpeg、FFprobe、COLMAP（CUDA 构建）和 Brush 打包进安装程序，用户无需配置系统 `PATH` 或单独安装原生引擎。源码仓库不保存这些大型二进制文件，而是通过固定来源和 SHA-256 在构建前恢复。

当前版本：**0.1.0**

> 当前交付目标是从视频生成并管理 `final.ply`。应用暂不包含 3D Viewer，生成结果需要使用其他支持 Gaussian Splatting PLY 的工具查看。

## 主要功能

- 从 MP4、MOV 视频创建 Gaussian Splatting 项目。
- 自动完成视频分析、均匀抽帧、特征提取、顺序匹配、相机重建、Brush 训练和 PLY 发布。
- FFmpeg、FFprobe、COLMAP（CUDA 构建）和 Brush 随安装包提供。
- COLMAP 特征提取与匹配可在 CPU 与 GPU 之间手动切换，选择会持久化。
- 实时显示处理阶段、引擎输出、关键计数、累计耗时和最多 500 条界面日志。
- 原始进程输出完整写入项目的 `logs` 目录。
- 支持取消任务，并通过 Windows Job Object 终止整个子进程树。
- 支持自定义项目根目录，默认位置为 `Documents\SplatStudio\Projects`。
- 自动记录已完成、失败、中断和取消的历史任务。
- 可在资源管理器中定位 `final.ply`，或将整个项目移入 Windows 回收站。
- 可拖动中央分界线调整左右面板宽度；右下角支持 80%–140% 整体界面缩放。
- 支持中文、空格、长文件名和 UNC 项目路径。

## 处理流程

```text
输入视频
  │
  ├─ FFprobe：读取时长、分辨率、帧率和总帧数
  ├─ FFmpeg：按照质量档位均匀抽取画面
  ├─ COLMAP：按所选后端（CPU 或 GPU）进行特征提取与顺序匹配
  ├─ COLMAP：增量重建并验证注册率和三维点
  ├─ Brush：使用可用 GPU 训练 Gaussian Splats
  └─ 校验 PLY 后原子发布为 final.ply
```

COLMAP 注册图像比例低于 50% 时任务停止；50%–80% 时给出质量警告并继续；达到 80% 时视为正常。

## 系统要求

- Windows 10 或 Windows 11，x64。
- 支持 WebView2 Runtime。
- Brush 训练需要可用的 GPU 图形后端，建议使用独立显卡。
- COLMAP 的 GPU 加速模式需要 NVIDIA 显卡及对应驱动；没有 NVIDIA 显卡时应选择 CPU 模式。
- 项目磁盘需要容纳源视频副本、抽帧图像、COLMAP 数据、Brush 中间文件和最终 PLY。长视频或精细档位可能占用大量空间。
- 安装模式为整机安装，安装时可能需要管理员权限。

COLMAP 使用 CUDA 构建，可在 CPU 与 GPU 之间手动切换；Brush 训练使用可用 GPU。

## 安装与使用

1. 运行 `OOOSplat_0.1.0_x64-setup.exe` 完成安装。
2. 启动 OOOSplat，确认顶栏中的内置引擎状态正常。
3. 在“01 创建新任务”中选择输入视频。
4. 选择项目根目录；程序会记住上次使用的位置。
5. 选择“快速”“均衡”或“精细”档位。
6. 选择“COLMAP 加速”为 CPU 或 GPU；程序会记住上次选择。GPU 模式需要 NVIDIA 显卡和驱动。
7. 点击“开始生成”，在左侧查看实时阶段、指标和日志。
8. 完成后，在“02 历史任务”中查看项目路径、PLY 大小、Splat 数量、生成日期和耗时。

使用提示：

- 拖动左右面板之间的分界线可以调整宽度；双击分界线恢复默认比例。
- 点击右下角百分比按钮可缩小、恢复或放大整个界面。
- 任务运行期间不可修改视频、项目目录和质量档位。
- 点击“删除”会回收整个项目目录，包括源视频副本和所有中间文件；如果移入回收站失败，程序不会降级为永久删除。

## 质量档位

| 档位 | 保留画面 | FFmpeg 抽帧率 | Brush iterations | 最大训练分辨率 |
| --- | ---: | ---: | ---: | ---: |
| 快速 | 30% | 源视频 FPS × 0.30 | 8,000 | 1,200 |
| 均衡 | 50% | 源视频 FPS × 0.50 | 15,000 | 1,600 |
| 精细 | 100% | 源视频 FPS × 1.00 | 30,000 | 2,000 |

抽帧由 FFmpeg 完成，COLMAP 不负责减少帧数。程序不设置最大抽帧数量，也没有额外的 Splat 数量上限；最终 Splat 数量由素材、重建结果和 Brush 训练过程决定。

## 项目与文件位置

每次生成都会在项目根目录下创建一个独立文件夹：

```text
<项目根目录>\<yyyyMMdd-HHmmss_视频名>\
  final.ply             最终 Gaussian Splatting 文件
  project.json          项目元数据与结果指标
  state.json            流水线状态
  source\
    input.<ext>         源视频副本
  work\
    frames\             FFmpeg 抽取的画面
    colmap\             COLMAP 数据库与稀疏重建
    brush\              Brush 数据集与训练中间文件
  logs\                 FFmpeg、COLMAP、Brush 等完整日志
```

项目名中的 Windows 非法字符会被清理；发生重名时自动追加 `-2`、`-3` 等后缀。

应用设置和项目索引保存在：

```text
%LOCALAPPDATA%\SplatStudio\settings.json
%LOCALAPPDATA%\SplatStudio\project-index.json
```

## 内置引擎

| 引擎 | 固定版本/构建 | 用途 |
| --- | --- | --- |
| FFmpeg / FFprobe | 8.1 系列 Windows x64 LGPL shared | 视频分析与抽帧 |
| COLMAP | 4.0.4 发布资产，CUDA 构建 | 特征、匹配与相机重建（CPU/GPU 可选） |
| Brush | v0.3.0 Windows x64 | Gaussian Splatting 训练与 PLY 导出 |

详细来源、实际版本、下载与安装规则、压缩包哈希和可执行文件哈希记录在 [`engines/manifest.json`](engines/manifest.json)。大型引擎文件不会提交到 Git；开发者通过 `npm run setup:engines` 下载并恢复到本地。Release 打包前会运行校验；文件缺失、哈希变化、COLMAP CUDA 运行时缺失或 Brush CLI 参数不符合预期时，打包会被阻止。

第三方许可和通知位于 [`licenses/`](licenses/)：

- FFmpeg：LGPL-2.1-or-later，本项目选用的构建禁用 GPL/nonfree 组件。
- COLMAP：BSD-3-Clause。
- Brush：Apache-2.0。

## 本地开发

### 开发环境

- Node.js 22.12 或更高版本。
- Rust stable，目标为 `x86_64-pc-windows-msvc`。
- Visual Studio 2022 Build Tools，包含 Desktop development with C++。
- Tauri 2 所需的 WebView2 开发/运行环境。

安装依赖并启动开发模式：

```powershell
npm install
npm run setup:engines
npm run tauri -- dev
```

### 测试与检查

```powershell
# 前端测试
npm test

# 前端生产构建
npm run build

# Rust 测试
cargo test --manifest-path src-tauri\Cargo.toml

# Rust 静态检查
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings

# 校验内置引擎版本、哈希和 COLMAP CUDA 运行时
npm run verify:engines

# 校验第一方许可、第三方通知及安装包资源映射
npm run verify:licenses
```

### 生成 Windows 安装包

```powershell
npm run tauri -- build
```

NSIS 安装包输出到：

```text
src-tauri\target\release\bundle\nsis\OOOSplat_0.1.0_x64-setup.exe
```

首次构建前必须运行 `npm run setup:engines`。`beforeBuildCommand` 会自动执行引擎校验和前端生产构建，但不会在打包过程中隐式访问网络。

## CLI

仓库同时提供 `splatstudio` 诊断 CLI：

```powershell
# 检查所有内置引擎
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- health

# 读取视频信息
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- probe "D:\Videos\orbit.mp4"

# 查看抽帧计划但不写出画面
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- plan "D:\Videos\orbit.mp4" --quality balanced

# 单独抽帧
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- extract "D:\Videos\orbit.mp4" "D:\Frames" --quality fast

# 运行完整流水线（自动检测并选择 COLMAP GPU 或 CPU）
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Videos\orbit.mp4" --projects-root "D:\Splat Projects" --quality balanced
```

开发或诊断时可以通过全局参数 `--engine-dir <路径>`，或环境变量 `OOOSPLAT_ENGINE_DIR`，覆盖默认引擎目录。

## 常见问题

### 如何让 COLMAP 使用显卡？

无需手动选择。应用会检查内置 COLMAP CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时自动使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。当前最低要求为 Windows 驱动 528.33、Compute Capability 5.0；实际检测结果和未启用原因会显示在“01 创建新任务”中。

### 为什么任务在相机重建后停止？

通常是相机注册率低于 50%。建议使用曝光稳定、画面清晰、运动连续、视角重叠充分的环绕拍摄视频，避免快速转动、强反光、大面积纯色和运动物体。

### 为什么项目占用空间很大？

每个项目会保留源视频副本、抽帧、COLMAP 数据和 Brush 中间文件，便于诊断和追溯。确认结果后，可通过历史任务中的“删除”将整个项目移入回收站。

### 可以直接在应用中查看 final.ply 吗？

暂不支持。本版本提供生成、项目管理和资源管理器定位，不包含 3D 渲染预览。

## 技术栈

- 桌面框架：Tauri 2
- 后端：Rust、Tokio
- 前端：React 19、TypeScript、Vite、Zustand
- 原生流水线：FFmpeg / FFprobe、COLMAP、Brush
- Windows 进程管理：Job Object

## 许可说明

### 代码许可

OOOSplat 的第一方代码及随附文档以 [Apache License 2.0](LICENSE) 发布。版权声明见 [NOTICE](NOTICE)。

### 第三方组件

FFmpeg / FFprobe、COLMAP 和 Brush 分别适用其自身许可证，不因与 OOOSplat 一同分发而改用 Apache-2.0。直接引擎的版本、来源、许可证和许可证正文入口见 [第三方通知](licenses/THIRD_PARTY_NOTICES.txt) 与 [引擎清单](engines/manifest.json)。该清单不表示已经完成 Qt、Boost、Ceres 等传递依赖的完整许可审计。

### 品牌

Apache-2.0 不授予 “OOOSplat” 名称、Logo、图标或其他视觉标识的商标使用权。真实引用、教程、截图、官方未修改版本分发及修改版命名规则见 [OOOSplat Trademark Policy](TRADEMARK_POLICY.md)。

### 生成模型

`final.ply` 等生成结果不会仅因使用 OOOSplat 而自动适用 Apache、GPL、LGPL 或其他随附软件许可证。该说明不判断模型著作权归属，也不授予输入素材或第三方内容的权利；完整边界见 [Generated Outputs](GENERATED_OUTPUTS.md)。
