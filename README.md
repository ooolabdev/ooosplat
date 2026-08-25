# OOOSplat

[中文](README.md) | [English](README_EN.md)

<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat Logo" width="180">
</p>

OOOSplat 是一款本地视频转 3D Gaussian Splatting 桌面应用。Windows 和 Apple Silicon macOS Alpha 均随应用内置 FFmpeg、FFprobe、COLMAP 和 Brush；Linux 支持目前仅作为 Ubuntu 24.04 LTS x86_64 Alpha 提供。React 界面通过 Tauri 直接调用本机 Rust 后端，不需要远程服务或 localhost API。

当前版本：**0.2.0**

> 当前交付目标是从视频生成并管理 `final.ply`。应用暂不包含 3D Viewer，生成结果需要使用其他支持 Gaussian Splatting PLY 的工具查看。

## 主要功能

- 从 MP4、MOV 视频创建 Gaussian Splatting 项目。
- 自动完成视频分析、均匀抽帧、特征提取、顺序匹配、相机重建、Brush 训练和 PLY 发布。
- Windows 安装包内置 CUDA 版 COLMAP；macOS Alpha 内置 arm64 CPU 版 COLMAP；Ubuntu 使用系统 CPU 版 COLMAP。三个平台均使用固定并校验的 FFmpeg/Brush 方案。
- COLMAP 会自动检查内置 CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。
- 实时显示处理阶段、引擎输出、关键计数、累计耗时和最多 500 条界面日志。
- 原始进程输出完整写入项目的 `logs` 目录。
- 支持取消任务，并通过 Windows Job Object 或 Unix process group 终止整个子进程树。
- 支持自定义项目根目录，默认位置为 `Documents\SplatStudio\Projects`。
- 自动记录已完成、失败、中断和取消的历史任务。
- 可在文件管理器中定位 `final.ply`，或将整个项目移入系统回收站。
- 可拖动中央分界线调整左右面板宽度；右下角支持 80%–140% 整体界面缩放。
- 支持中文、空格、长文件名和 UNC 项目路径。

## 处理流程

```text
输入视频
  │
  ├─ FFprobe：读取时长、分辨率、帧率和总帧数
  ├─ FFmpeg：按照质量档位均匀抽取画面
  ├─ COLMAP：自动选择 CPU 或 CUDA GPU 进行特征提取与顺序匹配
  ├─ COLMAP：增量重建并验证注册率和三维点
  ├─ Brush：使用可用 GPU 训练 Gaussian Splats
  └─ 校验 PLY 后原子发布为 final.ply
```

COLMAP 注册图像比例低于 50% 时任务停止；50%–80% 时给出质量警告并继续；达到 80% 时视为正常。

## 系统要求

- Windows 10 或 Windows 11，x64。
- 支持 WebView2 Runtime。
- Brush 训练需要可用的 GPU 图形后端，建议使用独立显卡。
- COLMAP 的 CUDA 加速需要 NVIDIA 显卡、Windows 驱动 528.33 或更高版本，以及 Compute Capability 5.0 或更高版本；不满足要求时程序会自动使用 CPU，无需用户配置。
- 项目磁盘需要容纳源视频副本、抽帧图像、COLMAP 数据、Brush 中间文件和最终 PLY。长视频或精细档位可能占用大量空间。
- 安装模式为整机安装，安装时可能需要管理员权限。

Windows 内置的 COLMAP 使用同时支持 CPU 与 CUDA GPU 的构建，运行前会自动选择可用后端；Brush 训练使用可用图形后端，二者的 GPU 检测与运行机制相互独立。

### macOS 15+ Alpha（仅限 Apple Silicon）

> 当前交付为未签名、未公证的 `.app`/`.dmg` Alpha，仅支持 M1 或更新的 Apple Silicon Mac，不支持 Intel Mac 或 Universal Binary。

- 内置原生 arm64 FFmpeg 8.1.2、独立 FFprobe、COLMAP 4.0.4 CPU CLI-only 和 Brush v0.3.0。
- 用户不需要安装 Homebrew，也不会回退到 Homebrew 或系统 `PATH` 中的同名程序。
- COLMAP 固定使用 CPU；Brush 独立选择可用的 Metal 图形后端，界面会明确显示该原因。
- 首次打开未签名版本时，macOS Gatekeeper 可能阻止启动。请在 Finder 中右键应用并选择“打开”；正式版本将在后续接入 Apple 签名和公证。

### Ubuntu 24.04 Alpha（仅限 x86_64）

> 本 Alpha 仅交付可从源码构建的 Ubuntu 24.04 本机可执行文件，不包含 Linux 安装包，也不声明支持 Ubuntu 22.04、其他 Linux 发行版或生产环境部署。

- Ubuntu 24.04 LTS，x86_64。
- Brush 支持的图形后端和对应驱动；Brush 官方支持 AMD、Intel 和 NVIDIA GPU。当前端到端验证使用 NVIDIA GPU，CPU-only 软件图形后端尚未验证，但不会被启动检查人为阻止。
- Node.js 22.12+、Rust stable 和 Tauri 2 的 WebKitGTK 开发依赖。
- Ubuntu 24.04 系统 `ffmpeg`、`ffprobe` 和 CPU 版 `colmap`（仓库版本为 COLMAP 3.9）。
- Brush v0.3.0 Linux x86_64，由 `npm run setup:engines` 下载并校验。

Ubuntu 依赖安装：

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file ffmpeg colmap \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
```

请为显卡安装可用的 Vulkan 驱动（例如 NVIDIA 专有驱动，或 AMD/Intel 的 Mesa 驱动）。Ubuntu 24.04 仓库中的无 CUDA COLMAP 构建会自动使用 CPU；Brush 会在运行时选择可用的图形后端。完全 CPU-only 的软件 Vulkan 后端尚未完成端到端验证。

## 安装与使用

1. Windows 运行 `OOOSplat_0.2.0_x64-setup.exe`；Apple Silicon Mac 打开未签名 Alpha DMG 并将 OOOSplat 拖入“应用程序”。
2. 启动 OOOSplat，确认顶栏中的内置引擎状态正常。
3. 在“01 创建新任务”中选择输入视频。
4. 选择项目根目录；程序会记住上次使用的位置。
5. 选择“快速”“均衡”或“精细”档位。
6. 查看自动检测到的 COLMAP 加速状态及原因，然后点击“开始生成”。
7. 在左侧查看实时阶段、指标和日志；完成后，在“02 历史任务”中查看项目路径、PLY 大小、Splat 数量、生成日期和耗时。

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
| FFmpeg / FFprobe | Windows x64 8.1 LGPL shared；macOS arm64 8.1.2 LGPL shared | 视频分析与抽帧 |
| COLMAP | Windows 4.0.4 CUDA；macOS arm64 4.0.4 CPU CLI-only | 特征、匹配和相机重建 |
| Brush | v0.3.0 Windows x64 / macOS arm64 | Gaussian Splatting 训练与 PLY 导出 |

Windows、Ubuntu 和 macOS 的来源与校验策略分别记录在 [`engines/manifest.json`](engines/manifest.json)、[`engines/manifest.linux.json`](engines/manifest.linux.json) 和 [`engines/manifest.macos.json`](engines/manifest.macos.json)。大型引擎文件不会提交到 Git；开发者通过 `npm run setup:engines` 恢复本地运行时。Release 打包前会校验来源、哈希、架构、动态库闭包和 Brush CLI 参数。

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

### Ubuntu 24.04 Alpha 开发

安装上面的 Ubuntu 系统依赖、Node.js 和 Rust 后：

```bash
npm ci
npm run setup:engines
npm run verify:engines
npm run verify:licenses
npm run tauri -- dev
```

也可以在仓库根目录运行 `./scripts/start-app-linux.sh`，或使用 `npm run start:app:linux`。启动脚本会先校验本机引擎和许可映射，仅在源码更新时重新构建 Release 可执行文件。

Ubuntu 24.04 Alpha 的 `setup:engines` 只安装校验后的 Brush 到 `engines/linux/brush/`；FFmpeg、FFprobe 和 CPU 版 COLMAP 保持为系统软件包。该流程只生成不带安装包的 x86_64 本机可执行文件。

Ubuntu 24.04 Alpha 自动检查位于 `.github/workflows/ubuntu.yml`。普通 GitHub runner 会执行前端、许可映射、Rust、Clippy、FFmpeg 集成和无安装包 Tauri 构建；Brush 端到端验证需要具有可用图形后端的主机或自托管 runner。目前完整流水线仅在 NVIDIA 主机上验证，欢迎补充 AMD、Intel 和软件 Vulkan 的测试结果。

### macOS 15+ Apple Silicon Alpha 开发

在 Apple Silicon Mac 上安装 Node.js、Rust 和 Xcode Command Line Tools 后：

```bash
npm ci
npm run setup:engines
npm run verify:engines
npm run verify:licenses
npm run tauri -- dev
```

`setup:engines` 从同仓库的固定 GitHub Release 下载完整 arm64 运行时；Homebrew 只用于维护者先执行 `npm run setup:build-deps:macos`，再执行 `npm run build:engines:macos` 重建引擎。`.github/workflows/macos.yml` 构建未签名应用和 DMG，`.github/workflows/macos-engines.yml` 生成并发布锁定的引擎归档。

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
src-tauri\target\release\bundle\nsis\OOOSplat_0.2.0_x64-setup.exe
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

Linux 还可分别使用 `OOOSPLAT_FFMPEG`、`OOOSPLAT_FFPROBE`、`OOOSPLAT_COLMAP` 和 `OOOSPLAT_BRUSH` 指定可执行文件；未指定时按仓库托管目录和系统 `PATH` 依次发现。

## 常见问题

### 如何让 COLMAP 使用显卡？

无需手动选择。应用会检查内置 COLMAP CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时自动使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。当前最低要求为 Windows 驱动 528.33、Compute Capability 5.0；实际检测结果和未启用原因会显示在“01 创建新任务”中。Brush 与 COLMAP 相互独立，会在运行时选择可用的图形后端。

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
- 进程树管理：Windows Job Object；Linux Unix process group

## 许可说明

### 代码许可

OOOSplat 的第一方代码及随附文档以 [Apache License 2.0](LICENSE) 发布。版权声明见 [NOTICE](NOTICE)。

### 第三方组件

FFmpeg / FFprobe、COLMAP 和 Brush 分别适用其自身许可证，不因与 OOOSplat 一同分发而改用 Apache-2.0。直接引擎的版本、来源、许可证和许可证正文入口见 [第三方通知](licenses/THIRD_PARTY_NOTICES.txt) 与 [引擎清单](engines/manifest.json)。该清单不表示已经完成 Qt、Boost、Ceres 等传递依赖的完整许可审计。

### 品牌

Apache-2.0 不授予 “OOOSplat” 名称、Logo、图标或其他视觉标识的商标使用权。真实引用、教程、截图、官方未修改版本分发及修改版命名规则见 [OOOSplat Trademark Policy](TRADEMARK_POLICY.md)。

### 生成模型

`final.ply` 等生成结果不会仅因使用 OOOSplat 而自动适用 Apache、GPL、LGPL 或其他随附软件许可证。该说明不判断模型著作权归属，也不授予输入素材或第三方内容的权利；完整边界见 [Generated Outputs](GENERATED_OUTPUTS.md)。
