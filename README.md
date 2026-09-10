# OOOSplat

[中文](README.md) | [English](README_EN.md)

<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat Logo" width="180">
</p>

<p align="center">
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.4.0"><strong>⬇️ 下载 OOOSplat 0.4.0（Windows / macOS / Ubuntu）</strong></a>
</p>

OOOSplat 是一款将普通环绕拍摄视频或图片序列一键转换为 3D Gaussian Splatting 的本地桌面应用。选择素材、项目目录和质量档位后，应用会自动完成画面准备、相机重建、训练与 PLY 发布，并可直接预览、调整和导出结果。

Windows 和 Apple Silicon macOS Alpha 均随应用提供 FFmpeg、FFprobe、COLMAP 和 Brush；Linux 支持目前仅作为 Ubuntu 24.04 LTS x86_64 Alpha 提供。整个生成流程使用本机 CPU 和 GPU，输入素材、工程文件、模型与日志无需上传到云端重建或训练服务。React 界面通过 Tauri 直接调用本机 Rust 后端，不需要远程服务或 localhost API。

当前版本：**0.4.0**

查看 [OOOSplat Roadmap](ROADMAP.md) 了解后续规划。

> 0.4.0 新增图片序列输入、透明 MOV/PNG 自动 Mask、阶段级断点续跑，以及矩形、球形和盒形 Gaussian 编辑；原始 `final.ply` 始终保留。

## 核心优势

- **一键生成高斯泼溅**：只需选择输入视频或图片序列、项目目录和质量档位，即可自动完成画面准备、COLMAP 相机重建、Brush 训练和 `final.ply` 发布，无需手动拼接命令或配置引擎。
- **全平台兼容**：支持 Windows、macOS 和 Linux，可在三大主流桌面系统上完成本地高斯泼溅生成；具体系统与处理器要求请参阅下方兼容性说明。
- **安全与隐私保护**：素材、抽帧、相机重建数据、高斯模型和日志默认只保存在用户选择的本地项目目录，核心生成流程在本机完成，无需将原始视频、图像或模型上传到第三方重建与训练平台，从而减少数据在网络传输、云端留存和未经授权访问过程中的泄露风险。
- **完全本地化算力**：重建与训练均在用户自己的电脑上运行，不调用远程计算服务。满足要求时 COLMAP 自动使用本机 NVIDIA GPU 加速，否则回退 CPU，过程和数据始终由用户掌控。

## 界面预览

### 创建与管理任务

![OOOSplat 创建新任务与历史任务界面](assets/screenshots/task-workspace.png)

### 高斯泼溅预览与调整

![OOOSplat 高斯泼溅预览与 Transform 调整界面](assets/screenshots/gaussian-preview.png)

## 主要功能

- 从 MP4、MOV 视频，或包含 JPG、JPEG、PNG 的图片序列文件夹创建 Gaussian Splatting 项目。
- 视频使用均匀抽帧和顺序匹配；图片序列保留全部图片并使用共享相机、穷举匹配和现有增量 Mapper。
- 自动检测透明 MOV 的 Alpha 通道，同步提取 RGBA PNG 画面与 COLMAP Mask；透明区域不会参与特征提取，同时保留给 Brush 训练使用。
- 自动检测透明 PNG，保留 Alpha 供 Brush 使用，并生成 COLMAP Mask 排除完全透明区域。
- Windows 安装包内置 CUDA 版 COLMAP；macOS Alpha 内置 arm64 CPU 版 COLMAP；Ubuntu 使用系统 CPU 版 COLMAP。三个平台均使用固定并校验的 FFmpeg/Brush 方案。
- COLMAP 会自动检查内置 CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。
- 实时显示处理阶段、引擎输出、关键计数、累计耗时和最多 500 条界面日志。
- 原始进程输出完整写入项目的 `logs` 目录。
- 支持取消任务，并通过 Windows Job Object 或 Unix process group 终止整个子进程树。
- 支持自定义项目根目录，默认位置为 `Documents\SplatStudio\Projects`。
- 自动记录已完成、失败、中断和取消的历史任务。
- 支持阶段级断点续跑：重新检查抽帧、Mask、COLMAP 数据库、稀疏重建和 PLY 检查点，复用可信阶段，并从最早的不可信阶段安全重跑。
- 根据素材规模、质量档位和本机历史任务估算生成时长；Brush 训练阶段持续更新进度。
- 在“03 预览”中直接加载历史项目的 `.ply`，支持 Orbit、Pan 和 Zoom；“调整 / 动画”双模式切换不会重新加载模型或重置相机。
- 调整模式支持整个 Gaussian 模型的位置、旋转、等比缩放，以及撤销 / 重做。
- 提供矩形、球形和盒形 Gaussian 选择工具：矩形可穿透框选并非破坏式删除点，球形和盒形区域则实时保留区域内的 Gaussian。
- 裁切区域和删除记录会自动保存；点击“保存”时将编辑结果写入唯一的 `edit.ply`，后续保存会安全替换该文件，始终不覆盖原始 `final.ply`。
- 动画模式依次播放 5 秒显现、8 秒冲击波和持续相机环绕，并可导出带 OOOSplat 水印的 1080×1920、30 fps、23 秒 H.264 MP4。
- 可在平台文件管理器中定位 `final.ply`，或将整个项目移入系统回收站。
- 可拖动中央分界线调整左右面板宽度；右下角支持 80%–140% 整体界面缩放。
- 支持中文、空格、长文件名和 UNC 项目路径。

### Gaussian 编辑快捷键

- 矩形选择时，左键拖动替换选区，`Shift + 左键拖动`添加，`Ctrl + 左键拖动`移除；中键旋转视角，右键平移，滚轮缩放。
- 黄色高亮表示当前选区；按 `Delete` 或 `Backspace` 删除选中 Gaussian，按 `Esc` 清空临时选区。
- 球形或盒形选择会自动进入正交视图，可切换侧视图、正视图和顶视图，并通过视口控制器或数值面板调整位置与大小。
- `Ctrl + Z` 撤销，`Ctrl + Shift + Z` 或 `Ctrl + Y` 重做。模型变换、裁切和删除共享同一历史记录；相机移动和临时黄色选区不计入历史。

## 处理流程

```text
输入视频或图片序列
  │
  ├─ 视频：FFprobe 分析，FFmpeg 按质量档位均匀抽帧；透明视频同步生成 RGBA 画面与 Mask
  ├─ 图片：按文件名排序并保留全部图片；透明 PNG 自动生成 Mask
  ├─ COLMAP：自动选择 CPU 或 CUDA GPU 提取特征；视频顺序匹配，图片穷举匹配
  ├─ COLMAP：增量重建并验证注册率和三维点
  ├─ Brush：使用可用 GPU 训练 Gaussian Splats
  └─ 校验 PLY 后原子发布为 final.ply
```

只要 COLMAP 生成了至少一张注册图像和有效三维点，任务就会继续进入 Brush；注册率低于 80% 时会给出质量警告，但不再因低于 50% 自动终止。

## 系统要求

- Windows 11，x64。
- 支持 WebView2 Runtime。
- 视频导出需要 WebView2 提供 WebCodecs AVC 编码能力；不支持时仍可在“动画”模式播放效果，但“导出视频”会显示不可用原因。
- Brush 训练需要可用的 GPU 图形后端，建议使用独立显卡。
- COLMAP 的 CUDA 加速需要 NVIDIA 显卡、Windows 驱动 528.33 或更高版本，以及 Compute Capability 5.0 或更高版本；不满足要求时程序会自动使用 CPU，无需用户配置。
- 项目磁盘需要容纳源素材副本、输入图像、COLMAP 数据、Brush 中间文件和最终 PLY。长视频、大型图片序列或精细档位可能占用大量空间。
- 安装模式为整机安装，安装时可能需要管理员权限。

Windows 内置的 COLMAP 使用同时支持 CPU 与 CUDA GPU 的构建，运行前会自动选择可用后端；Brush 训练使用可用图形后端，二者的 GPU 检测与运行机制相互独立。

### macOS 15+ Alpha（仅限 Apple Silicon）

> 当前交付为未签名、未公证的 `.app`/`.dmg` Alpha，仅支持 M1 或更新的 Apple Silicon Mac，不支持 Intel Mac 或 Universal Binary。

- 内置原生 arm64 FFmpeg 8.1.2、独立 FFprobe、COLMAP 4.0.4 CPU CLI-only 和 Brush v0.3.0。
- 用户不需要安装 Homebrew，也不会回退到 Homebrew 或系统 `PATH` 中的同名程序。
- COLMAP 固定使用 CPU；Brush 独立选择可用的 Metal 图形后端，界面会明确显示该原因。
- 首次打开未签名版本时，macOS Gatekeeper 可能阻止启动。请在 Finder 中右键应用并选择“打开”；正式版本将在后续接入 Apple 签名和公证。

### Ubuntu 24.04 Alpha（仅限 x86_64）

> 本 Alpha 交付由 Ubuntu 24.04 构建的 x86_64 `.deb` 安装包；不声明支持 Ubuntu 22.04、其他 Linux 发行版或生产环境部署。

- Ubuntu 24.04 LTS，x86_64。
- Brush 支持的图形后端和对应驱动；Brush 官方支持 AMD、Intel 和 NVIDIA GPU。当前端到端验证使用 NVIDIA GPU，CPU-only 软件图形后端尚未验证，但不会被启动检查人为阻止。
- 从源码构建需要 Node.js 22.12+、Rust stable 和 Tauri 2 的 WebKitGTK 开发依赖；安装 `.deb` 的用户不需要这些开发工具。
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

从 GitHub Actions 下载 `OOOSplat-0.4.0-x64-linux` Artifact 后，可执行：

```bash
sudo apt install ./OOOSplat-0.4.0-x64-linux.deb
```

`.deb` 会通过 Ubuntu 包管理器安装 FFmpeg、FFprobe 和 CPU 版 COLMAP；固定版本 Brush 已包含在安装包中。

## 安装与使用

1. Windows 运行 `OOOSplat-0.4.0-x64-windows.exe`；Apple Silicon Mac 打开 `OOOSplat-0.4.0-arm64-macos.dmg` 并将 OOOSplat 拖入“应用程序”；Ubuntu 24.04 使用 `sudo apt install ./OOOSplat-0.4.0-x64-linux.deb`。
2. 启动 OOOSplat，确认顶栏中的内置引擎状态正常。
3. 在“01 创建新任务”的输入类型下拉栏选择“视频”或“图片”，再点击输入框选择视频文件或图片序列文件夹。
4. 选择项目根目录；程序会记住上次使用的位置。
5. 选择“快速”“均衡”或“精细”档位。
6. 查看自动检测到的 COLMAP 加速状态及原因，然后点击“开始生成”。
7. 在左侧查看实时阶段、指标和日志；完成后，在“02 历史任务”中查看项目并点击“预览”。
8. 在“03 预览”的“调整”模式中修改模型，Transform 会自动保存；点击“保存”生成或更新 `edit.ply`。
9. 切换到“动画”可查看竖屏构图并重新播放效果；点击“导出视频”会在项目目录生成 23 秒竖屏 MP4。

使用提示：

- 拖动左右面板之间的分界线可以调整宽度；双击分界线恢复默认比例。
- 点击右下角百分比按钮可缩小、恢复或放大整个界面。
- 任务运行期间不可修改输入素材、项目目录和质量档位。
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
<项目根目录>\<yyyyMMdd-HHmmss_素材名>\
  final.ply             最终 Gaussian Splatting 文件
  project.json          项目元数据与结果指标
  edit.ply              当前编辑结果；再次保存时安全替换（可选）
  preview.mp4           首次动画预览视频导出（可选）
  preview-2.mp4         后续视频导出自动编号（可选）
  state.json            流水线状态
  source\
    input.<ext>         源视频副本（视频项目）
    images\             规范化的源图片副本（图片序列项目）
  work\
    frames\             抽取或准备后的输入画面
    masks\              透明素材对应的 COLMAP Mask（按需）
    colmap\             COLMAP 数据库与稀疏重建
    brush\              Brush 数据集与训练中间文件
  logs\                 FFmpeg、COLMAP、Brush 等完整日志
```

项目名中的 Windows 非法字符会被清理；发生重名时自动追加 `-2`、`-3` 等后缀。

应用设置和项目索引保存在：

```text
%LOCALAPPDATA%\SplatStudio\settings.json
%LOCALAPPDATA%\SplatStudio\project-index.json
%LOCALAPPDATA%\SplatStudio\telemetry.json
```

## 匿名使用统计

OOOSplat 默认开启匿名使用统计，用于了解稳定性和各阶段耗时。可在右上角 **设置 → 隐私** 中随时关闭，关闭后不再发送任何请求。

会发送的内容：

| 字段 | 说明 |
| --- | --- |
| 安装 ID | 首次启动生成的随机 UUID，不读取硬件序列号、MAC 地址或设备指纹 |
| 应用版本、操作系统、CPU 架构 | 例如 `0.4.0` / `windows` / `x86_64` |
| 事件名 | `daily_active`、`generation_started`、`generation_completed`、`generation_failed`、`pipeline_stage_completed`。`daily_active` 每天最多一次，应用升级后当天会再报一次 |
| 质量档位与输入类型 | 枚举值，例如 `balanced` / `video`；图片序列输入报 `images` |
| 阶段耗时与总耗时 | 毫秒 |
| 帧数与视频时长 | 分桶值，不是原始数量 |
| 失败阶段与错误码 | 枚举值，例如 `colmap_mapper_failed`；不含原始错误文本 |

不会发送的内容：视频、图片、PLY 等任何素材；文件名、路径和项目名称；日志与命令输出；用户名或任何个人信息。

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

Ubuntu 24.04 Alpha 的 `setup:engines` 只安装校验后的 Brush 到 `engines/linux/brush/`；FFmpeg、FFprobe 和 CPU 版 COLMAP 保持为系统软件包。Tauri 将 Brush 作为资源打入 x86_64 `.deb`，并在 Debian 依赖中声明 FFmpeg 和 COLMAP。

Ubuntu 24.04 Alpha 自动检查位于 `.github/workflows/ubuntu.yml`。普通 GitHub runner 会执行前端、许可映射、Rust、Clippy、FFmpeg 集成、`.deb` 构建、包结构校验并上传安装包与 SHA-256；Brush 端到端验证需要具有可用图形后端的主机或自托管 runner。目前完整流水线仅在 NVIDIA 主机上验证，欢迎补充 AMD、Intel 和软件 Vulkan 的测试结果。

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
npm run package:windows
```

NSIS 安装包输出到：

```text
dist-artifacts\OOOSplat-0.4.0-x64-windows.exe
```

首次构建前必须运行 `npm run setup:engines`。`beforeBuildCommand` 会自动执行引擎校验和前端生产构建，但不会在打包过程中隐式访问网络。

### 应用内更新与发布

只有官方签名发布版会检查更新。官方 Windows 包在启动时会从官方 GitHub Release 检查更新；发现新版本后，顶栏会显示“更新至 <版本号>”。点击后，应用会先在页面内下载更新并显示进度，签名校验通过后再启动更新安装程序并重启应用。用户无需到其他渠道重新下载安装包。

当 FFmpeg、COLMAP 或 Brush 正在执行管线任务时，更新按钮会被禁用并显示“任务完成后可更新”；反过来，更新包正在下载时也不能开始新任务。安装更新会重启应用，因此这两种情况都不会让更新中断正在进行的处理。

开发者自行打包和 Fork 构建不会查询官方更新源，也不会显示任何更新入口。updater 需要同时满足两个条件才会启用：前端由发布 CI 以 `VITE_UPDATER_ENABLED=true` 构建，同时 Rust 侧以 `--features updater` 编译并注入 `plugins.updater` 配置。若只满足前端条件而二进制未编译 updater feature，应用会识别出缺少 updater 命令并直接隐藏更新入口，而不是一直显示“检查更新失败”。

更新清单固定为 `https://github.com/ooolabdev/ooosplat/releases/latest/download/latest.json`。每个更新包都由 Tauri updater 公钥验证；网络地址、Release 资产或 `latest.json` 被替换时，签名不匹配的包不会安装。

签名认证必须由 `ooolabdev/ooosplat` 的仓库管理人独占。首次启用发布前，管理人在自己的安全环境中生成一对 Tauri signer 密钥，并在上游仓库配置：

- Actions Secrets 中的 `TAURI_SIGNING_PRIVATE_KEY`：Tauri signer 生成的私钥全文；不得提交、共享或上传到 Release。
- Actions Secrets 中的 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：只有生成私钥时设置了密码才需要。
- Actions Variables 中的 `TAURI_UPDATER_PUBLIC_KEY`：与该私钥配对的 `.pub` 文件单行内容。它是公钥而不是 Secret，因此放在 Variables 中；它只在 CI 构建时注入客户端配置，源码和 PR 中不保存具体公钥。

私钥和密码只注入“Build signed updater installer”这一个 step；前面的 `npm ci`、测试、引擎校验等步骤都无法读取它们。

密钥生成示例（只在仓库管理人的安全环境中执行一次）：

```powershell
npm run tauri -- signer generate --write-keys "$HOME\.tauri\ooosplat-updater.key"
```

发布时，将 `package.json`、`src-tauri/Cargo.toml` 和 `src-tauri/tauri.conf.json` 的版本一致地更新为下一个 SemVer 版本，合入 `main` 后创建匹配的 tag，例如 `v0.3.1`。`.github/workflows/release.yml` 会将管理员配置的公钥临时注入构建、生成 NSIS 更新包、`.sig` 和 `latest.json`，并将它们上传至同一个 GitHub Release。tag 和版本不一致、未配置公钥或未产出签名时，工作流会失败而不是发布不可验证更新。

预发布版本通过 SemVer 后缀区分，例如 `v0.5.0-beta.1`。这类 tag 会自动创建为 GitHub pre-release，并且不生成 `latest.json`，因此不会进入稳定更新通道，`/releases/latest/` 始终指向最近一个稳定版。

同一个版本号的二进制发布后不再覆盖。工作流在目标 Release 已完整发布时会直接跳过重复上传；若已发布的 Release 资产不完整，则直接失败并提示改为发布新的 patch 版本。这样既不会用不同内容覆盖同名安装包，也不会在版本已发布后悄悄补齐资产。草稿（draft）Release 尚未公开下载，因此允许补齐资产后由维护者手动发布。

> Windows 安装模式保持为整机安装（per-machine）。应用内更新会使用 NSIS passive 模式，必要时 Windows 仍会请求管理员权限。

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

# 图片序列使用同一命令，传入文件夹即可
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Photos\object" --projects-root "D:\Splat Projects" --quality balanced
```

开发或诊断时可以通过全局参数 `--engine-dir <路径>`，或环境变量 `OOOSPLAT_ENGINE_DIR`，覆盖默认引擎目录。

Linux 还可分别使用 `OOOSPLAT_FFMPEG`、`OOOSPLAT_FFPROBE`、`OOOSPLAT_COLMAP` 和 `OOOSPLAT_BRUSH` 指定可执行文件；未指定时按仓库托管目录和系统 `PATH` 依次发现。

## 常见问题

### 如何让 COLMAP 使用显卡？

无需手动选择。应用会检查内置 COLMAP CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时自动使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。当前最低要求为 Windows 驱动 528.33、Compute Capability 5.0；实际检测结果和未启用原因会显示在“01 创建新任务”中。Brush 与 COLMAP 相互独立，会在运行时选择可用的图形后端。

### 为什么会出现注册率较低的警告？

注册率较低通常表示可用于重建的连续视角不足。任务仍会继续进入 Brush，但结果质量可能受影响。建议使用曝光稳定、画面清晰、运动连续、视角重叠充分的环绕拍摄视频，避免快速转动、强反光、大面积纯色和运动物体。

### 为什么项目占用空间很大？

每个项目会保留源视频副本、抽帧、COLMAP 数据和 Brush 中间文件，便于诊断和追溯。确认结果后，可通过历史任务中的“删除”将整个项目移入回收站。

### 可以直接在应用中查看 final.ply 吗？

可以。在“02 历史任务”中选择已完成项目并点击“预览”，即可在独立预览工作区浏览 `.ply`。“调整”模式用于编辑整个模型的位置、旋转和等比缩放；“动画”模式提供 5 秒显现、8 秒冲击波、持续环绕以及带水印的竖屏 MP4 导出。当前不支持单个 Gaussian 选择、删除、裁剪或清理；`.sog` 和 `.spz` 也尚未开放。

## 技术栈

- 桌面框架：Tauri 2
- 后端：Rust、Tokio
- 前端：React 19、TypeScript、Vite、Zustand
- Gaussian 预览与动画：PlayCanvas Engine、PlayCanvas React
- 视频编码与封装：WebCodecs、Mediabunny
- 原生流水线：FFmpeg / FFprobe、COLMAP、Brush
- 进程树管理：Windows Job Object；Linux Unix process group

## 许可说明

### 代码许可

OOOSplat 的第一方代码及随附文档以 [Apache License 2.0](LICENSE) 发布。版权声明见 [NOTICE](NOTICE)。

### 第三方组件

FFmpeg / FFprobe、COLMAP、Brush、PlayCanvas 和 Mediabunny 分别适用其自身许可证，不因与 OOOSplat 一同分发而改用 Apache-2.0。直接组件的版本、来源、许可证和许可证正文入口见 [第三方通知](licenses/THIRD_PARTY_NOTICES.txt) 与 [引擎清单](engines/manifest.json)。该清单不表示已经完成 Qt、Boost、Ceres 等传递依赖的完整许可审计。

### 品牌

Apache-2.0 不授予 “OOOSplat” 名称、Logo、图标或其他视觉标识的商标使用权。真实引用、教程、截图、官方未修改版本分发及修改版命名规则见 [OOOSplat Trademark Policy](TRADEMARK_POLICY.md)。

### 生成模型

`final.ply` 等生成结果不会仅因使用 OOOSplat 而自动适用 Apache、GPL、LGPL 或其他随附软件许可证。该说明不判断模型著作权归属，也不授予输入素材或第三方内容的权利；完整边界见 [Generated Outputs](GENERATED_OUTPUTS.md)。
