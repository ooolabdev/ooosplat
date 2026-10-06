# OOOSplat

[中文](README.md) | [English](README_EN.md)

<p align="center">
  <img src="assets/readme-logo.svg" alt="OOOSplat Logo" width="180">
</p>

<p align="center">
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/daily?language=Rust" alt="OOOSplat — Trendshift Rust 日榜荣誉" width="250" height="55" /></a>
  <a href="https://trendshift.io/repositories/177239?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-177239" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/177239/weekly?language=Rust" alt="OOOSplat — Trendshift Rust 周榜荣誉" width="250" height="55" /></a>
</p>

<p align="center">
  <a href="https://github.com/ooolabdev/ooosplat/releases/tag/0.5.0"><strong>⬇️ 下载 OOOSplat 0.5.0（Windows / macOS / Ubuntu）</strong></a>
</p>

OOOSplat 是一款将普通环绕拍摄视频或图片序列一键转换为 3D Gaussian Splatting 的本地桌面应用。选择素材、项目目录和质量档位后，应用会自动完成画面准备、相机重建、训练与 PLY 发布，并可直接预览、调整和导出结果。

Windows 和 Apple Silicon macOS Alpha 均随应用提供 FFmpeg、FFprobe、COLMAP 和 Brush；Linux 支持目前仅作为 Ubuntu 24.04 LTS x86_64 Alpha 提供。整个生成流程使用本机 CPU 和 GPU，输入素材、工程文件、模型与日志无需上传到云端重建或训练服务。React 界面通过 Tauri 直接调用本机 Rust 后端，不需要远程服务或 localhost API。

当前版本：**0.5.0**

开发分支新增可选的本地 MCP v1：在设置中授权素材目录并启用后，AI Agent 可通过七个工具创建、启动、查看及取消同一应用中的任务。默认关闭，仅监听本机；连接与日志说明见 [MCP v1 开发文档](docs/mcp-v1.md)。

查看 [OOOSplat Roadmap](ROADMAP.md) 了解后续规划。

> 0.5.0 新增默认开启、可关闭的实验性自动优化，可按素材、档位和显存规划重建与训练参数，并在视频重建覆盖不足时尝试桥接补帧；同时加入同相机增量高清补拍、Windows Brush 显卡稳定性与预览体验改进。

## 核心优势

- **一键生成高斯泼溅**：只需选择输入视频或图片序列、项目目录和质量档位，即可自动完成画面准备、COLMAP 相机重建、Brush 训练和 `final.ply` 发布，无需手动拼接命令或配置引擎。
- **全平台兼容**：支持 Windows、macOS 和 Linux，可在三大主流桌面系统上完成本地高斯泼溅生成；具体系统与处理器要求请参阅下方兼容性说明。
- **安全与隐私保护**：素材、抽帧、相机重建数据、高斯模型和日志默认只保存在用户选择的本地项目目录，核心生成流程在本机完成，无需将原始视频、图像或模型上传到第三方重建与训练平台，从而减少数据在网络传输、云端留存和未经授权访问过程中的泄露风险。
- **完全本地化算力**：重建与训练均在用户自己的电脑上运行，不调用远程计算服务。满足要求时 COLMAP 自动使用本机 NVIDIA GPU 加速，否则回退 CPU，过程和数据始终由用户掌控。

## 展示视频

https://github.com/user-attachments/assets/5b9e8cef-4c71-4bfa-ba23-641fcdd37659

## 界面预览

### 创建与管理任务

![OOOSplat 创建新任务与历史任务界面](assets/screenshots/task-workspace.png)

### 高斯泼溅预览与调整

![OOOSplat 高斯泼溅预览与 Transform 调整界面](assets/screenshots/gaussian-preview.png)

## 主要功能

- 从 MP4、MOV 视频，或包含 JPG、JPEG、PNG 的图片序列文件夹创建 Gaussian Splatting 项目。
- 默认开启“自动优化（实验性）”：视频根据档位采用 6 / 8 / 12 FPS 初始采样，并在画面准备、COLMAP 和 Brush 阶段使用对应的分辨率策略；初始重建覆盖不足时，会在剩余预算内尝试补充桥接画面。关闭后继续使用旧版固定比例抽帧和分辨率策略。
- 图片序列保留全部图片并使用共享相机、穷举匹配和现有增量 Mapper；视频使用顺序匹配，桥接补帧不适用于图片序列。
- 自动检测透明 MOV 的 Alpha 通道，同步提取 RGBA PNG 画面与 COLMAP Mask；透明区域不会参与特征提取，同时保留给 Brush 训练使用。
- 自动检测透明 PNG，保留 Alpha 供 Brush 使用，并生成 COLMAP Mask 排除完全透明区域。
- 已完成项目支持“高清补拍”：使用同一设备、同一镜头和相同分辨率补充视频或图片后，OOOSplat 会复用原数据库、共享相机与稀疏模型，只处理新增画面的特征、匹配和注册，再使用全部已注册画面完整重训 Brush。透明 MOV/PNG 补拍素材会保留 RGBA 并自动生成 Mask；补拍会创建独立派生项目，不覆盖原项目。
- 三个平台内置同一 commit 的 COLMAP 4.2.1；Windows/Linux 为自编译 CUDA + Caspar + Ceres 包，macOS 为 arm64 Ceres CPU 包。Brush 使用锁定的 OOOBrush ooo-v1.0.0 无界面 CLI；FFmpeg 策略保持不变。
- COLMAP 会自动检查内置 CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。
- 精细档会依据检测到的显存选择 Brush 训练分辨率和 Splat 上限；若明确检测到显存不足，会安全降低一次训练配置后重试。Windows 单 NVIDIA 独显环境还会优先稳定选择该独显，并为显卡设备中断提供明确提示。
- 实时显示处理阶段、引擎输出、关键计数、累计耗时和最多 500 条界面日志。
- 原始进程输出完整写入项目的 `logs` 目录。
- 支持取消任务，并通过 Windows Job Object 或 Unix process group 终止整个子进程树。
- 支持自定义项目根目录，默认位置为 `Documents\SplatStudio\Projects`。
- 自动记录已完成、失败、中断和取消的历史任务。
- 支持阶段级断点续跑：重新检查抽帧、Mask、COLMAP 数据库、稀疏重建和 PLY 检查点，复用可信阶段，并从最早的不可信阶段安全重跑。
- 根据素材规模、质量档位和本机历史任务估算生成时长；Brush 训练阶段持续更新进度。
- 在“03 预览”中直接加载历史项目的 `.ply`，支持 Orbit、Pan 和 Zoom；“调整 / 动画”双模式切换不会重新加载模型或重置相机。
- 调整模式支持整个 Gaussian 模型的位置、旋转、等比缩放，以及撤销 / 重做。
- 预览地面网格使用持久渲染资源，减少长时间预览中的重复绘制开销；模型 Transform 缩放范围扩展至 `0.001–10000`。
- 提供矩形、球形和盒形 Gaussian 选择工具：矩形可穿透框选并非破坏式删除点，球形和盒形区域则实时保留区域内的 Gaussian。
- 裁切区域和删除记录会自动保存；点击“保存”时将编辑结果写入唯一的 `edit.ply`，后续保存会安全替换该文件，始终不覆盖原始 `final.ply`。
- 动画模式依次播放 5 秒显现、8 秒冲击波和持续相机环绕，可切换竖屏（1080×1920）或横屏（1920×1080）构图，并导出带 OOOSplat 水印的 30 fps、23 秒 H.264 MP4。
- “导出”菜单还支持完整画质的单文件离线 HTML：内嵌查看器与模型，无需联网，双击即可旋转、平移、缩放和手动播放动画。保留当前变换、裁切、删除结果及全部未删除 Gaussian / SH 数据，不修改 `final.ply` 或 `edit.ply`。文件约为对应 PLY 的 4/3，另加查看器；超大模型仍受浏览器和设备能力限制。
- 可在平台文件管理器中定位 `final.ply`，或将整个项目移入系统回收站。
- 可拖动中央分界线调整左右面板宽度；右下角支持 80%–140% 整体界面缩放。
- 支持中文、空格、长文件名和 UNC 项目路径。
- 界面支持简体中文与英文即时切换；首次启动按系统语言自动选择，手动切换后会记住用户选择。

### Gaussian 编辑快捷键

- 矩形选择时，左键拖动替换选区，`Shift + 左键拖动`添加，`Ctrl + 左键拖动`移除；中键旋转视角，右键平移，滚轮缩放。
- 黄色高亮表示当前选区；按 `Delete` 或 `Backspace` 删除选中 Gaussian，按 `Esc` 清空临时选区。
- 球形或盒形选择会自动进入正交视图，可切换侧视图、正视图和顶视图，并通过视口控制器或数值面板调整位置与大小。
- `Ctrl + Z` 撤销，`Ctrl + Shift + Z` 或 `Ctrl + Y` 重做。模型变换、裁切和删除共享同一历史记录；相机移动和临时黄色选区不计入历史。

## 处理流程

```text
输入视频或图片序列
  │
  ├─ 视频：FFprobe 分析，FFmpeg 按档位目标 FPS 和工作分辨率抽帧；透明视频同步生成 RGBA 画面与 Mask
  ├─ 图片：按文件名排序并保留全部图片；透明 PNG 自动生成 Mask
  ├─ COLMAP：自动选择 CPU 或 CUDA GPU 提取特征；视频顺序匹配，图片穷举匹配
  ├─ COLMAP：增量重建并验证注册率和三维点；自动优化开启时可按需尝试桥接补帧
  ├─ Brush：使用可用 GPU 训练 Gaussian Splats；精细档按显存选择训练配置
  └─ 校验 PLY 后原子发布为 final.ply
```

只要 COLMAP 生成了至少一张注册图像和有效三维点，任务就会继续进入 Brush；注册率低于 80% 时会给出质量警告，但不再因低于 50% 自动终止。

## 系统要求

- Windows 11，x64。
- 支持 WebView2 Runtime。
- 视频导出需要 WebView2 提供 WebCodecs AVC 编码能力；不支持时仍可在“动画”模式播放效果，但“导出视频”会显示不可用原因。
- Brush 训练需要可用的 GPU 图形后端，建议使用独立显卡。
- COLMAP 的 CUDA 加速需要 NVIDIA 显卡、Windows 驱动 580.00 或更高版本，以及 Compute Capability 7.5 或更高版本；不满足要求时程序会自动使用 CPU，无需用户配置。
- 项目磁盘需要容纳源素材副本、输入图像、COLMAP 数据、Brush 中间文件和最终 PLY。长视频、大型图片序列或精细档位可能占用大量空间。
- 安装模式为整机安装，安装时可能需要管理员权限。

Windows 内置的 COLMAP 使用同时支持 CPU 与 CUDA GPU 的构建，运行前会自动选择可用后端；Brush 训练使用可用图形后端，二者的 GPU 检测与运行机制相互独立。

### macOS 15+ Alpha（仅限 Apple Silicon）

> 当前交付为未签名、未公证的 `.app`/`.dmg` Alpha，仅支持 M1 或更新的 Apple Silicon Mac，不支持 Intel Mac 或 Universal Binary。

- 内置原生 arm64 FFmpeg 8.1.2、独立 FFprobe、COLMAP 4.2.1 Ceres CPU CLI-only 和 OOOBrush ooo-v1.0.0 CLI。
- 用户不需要安装 Homebrew，也不会回退到 Homebrew 或系统 `PATH` 中的同名程序。
- COLMAP 固定使用 CPU；Brush 独立选择可用的 Metal 图形后端，界面会明确显示该原因。
- 首次打开未签名版本时，macOS Gatekeeper 可能阻止启动。请在 Finder 中右键应用并选择“打开”；正式版本将在后续接入 Apple 签名和公证。

### Ubuntu 24.04 Alpha（仅限 x86_64）

> 本 Alpha 交付由 Ubuntu 24.04 构建的 x86_64 `.deb` 安装包；不声明支持 Ubuntu 22.04、其他 Linux 发行版或生产环境部署。

- Ubuntu 24.04 LTS，x86_64。
- Brush 支持的图形后端和对应驱动；Brush 官方支持 AMD、Intel 和 NVIDIA GPU。当前端到端验证使用 NVIDIA GPU，CPU-only 软件图形后端尚未验证，但不会被启动检查人为阻止。
- 从源码构建需要 Node.js 22.12+、Rust stable 和 Tauri 2 的 WebKitGTK 开发依赖；安装 `.deb` 的用户不需要这些开发工具。
- Ubuntu 24.04 系统 `ffmpeg`、`ffprobe`；COLMAP 4.2.1 使用校验后随应用分发的自编译版本，不使用 apt/PATH COLMAP。
- OOOBrush ooo-v1.0.0 Linux x86_64 CLI，由 `npm run setup:engines` 下载并校验。

Ubuntu 依赖安装：

```bash
sudo apt update
sudo apt install -y \
  build-essential curl file ffmpeg \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev
```

请为显卡安装可用的 Vulkan 驱动（例如 NVIDIA 专有驱动，或 AMD/Intel 的 Mesa 驱动）。内置 COLMAP 在兼容 NVIDIA 设备上使用 CUDA/Caspar，否则使用 CPU/Ceres；Brush 独立选择图形后端。完全 CPU-only 的软件 Vulkan 后端尚未完成端到端验证。

从 GitHub Actions 下载 `OOOSplat-0.5.0-x64-linux` Artifact 后，可执行：

```bash
sudo apt install ./OOOSplat-0.5.0-x64-linux.deb
```

`.deb` 仅通过 Ubuntu 包管理器安装 FFmpeg/FFprobe；锁定的 COLMAP 和 Brush 已包含在安装包中。

## 安装与使用

测试素材：[夸克网盘下载](https://pan.quark.cn/s/1dde892a1324)，可用于测试或体验生成流程。

1. Windows 运行 `OOOSplat-0.5.0-x64-windows.exe`；Apple Silicon Mac 打开 `OOOSplat-0.5.0-arm64-macos.dmg` 并将 OOOSplat 拖入“应用程序”；Ubuntu 24.04 使用 `sudo apt install ./OOOSplat-0.5.0-x64-linux.deb`。
2. 启动 OOOSplat，确认顶栏中的内置引擎状态正常；可使用右上角的 `EN / 中文` 按钮即时切换界面语言。
3. 在“01 创建新任务”的输入类型下拉栏选择“视频”或“图片”，再点击输入框选择视频文件或图片序列文件夹。
4. 选择项目根目录；程序会记住上次使用的位置。
5. 选择“快速”“均衡”或“精细”档位。
6. 查看自动检测到的 COLMAP 加速状态及原因，然后点击“开始生成”。
7. 在左侧查看实时阶段、指标和日志；完成后，在“02 历史任务”中查看项目并点击“预览”。
8. 在“03 预览”的“调整”模式中修改模型，Transform 会自动保存；点击“保存”生成或更新 `edit.ply`。
9. 切换到“动画”，在命令栏中间选择“竖屏 / 横屏”，通过“导出 → 导出视频”生成 23 秒 MP4；调整或动画模式均可通过“导出 → 导出离线 HTML”分享当前编辑结果。导出期间可取消，完成后可打开所在文件夹。
10. 如需补充细节，在“02 历史任务”的已完成项目中点击“高清补拍”，使用原项目相同设备、镜头、方向、分辨率和缩放倍率拍摄，并确保补拍画面与原素材有足够重叠。

使用提示：

- 拖动左右面板之间的分界线可以调整宽度；双击分界线恢复默认比例。
- 点击右下角百分比按钮可缩小、恢复或放大整个界面。
- 任务运行期间不可修改输入素材、项目目录和质量档位。
- 点击“删除”会回收整个项目目录，包括源视频副本和所有中间文件；如果移入回收站失败，程序不会降级为永久删除。

## 质量档位

“自动优化（实验性）”默认开启。视频会在抽帧时一次性缩放到对应工作分辨率，并保持原始宽高比且不会放大低分辨率素材；实际采样不会超过源视频 FPS 或总帧数。图片序列始终保留全部有效图片和原始输入分辨率，但 COLMAP 与 Brush 仍使用对应档位的处理上限。

自动优化开启/关闭时各档位的完整参数、Bridge/Caspar/OOM 处理逻辑，以及开发环境外部配置覆盖方法见[自动优化参数配置](docs/pipeline-optimization-config.zh-CN.md)。默认参数集中维护在 `config/pipeline-optimization.json`。

| 档位 | 视频初始 / 桥接上限 | 视频工作长边 | COLMAP 长边 / 最大特征数 | Brush 训练 |
| --- | ---: | ---: | ---: | --- |
| 快速 | 6 / 9 FPS | 最大 1,600 | 1,200 / 4,096 | 8,000 iterations，最大分辨率 1,600 |
| 均衡 | 8 / 12 FPS | 最大 1,920 | 1,600 / 8,192 | 15,000 iterations，最大分辨率 1,920 |
| 精细 | 12 / 15 FPS | 依据显存为最大 3,200、3,840 或原生分辨率 | 3,200 / 16,384 | 30,000 iterations，依据显存选择最大分辨率和 Splat 上限 |

精细档的初始配置按可用显存划分：显存低于 8 GB 或无法读取时，视频和 Brush 最大长边为 3,200；8 GB（含）至 12 GB 之间为 3,840；12 GB（含）以上保留视频原生长边，并让 Brush 使用原生长边。三个精细配置的 COLMAP 图像长边均限制为 3,200。桥接补帧仅在视频初始重建覆盖不足且仍有候选画面时尝试；失败会安全回退到初始重建，不适用于图片序列。精细档在明确检测到显存不足时最多自动降低一次训练配置并重试。

关闭自动优化后，快速、均衡、精细档分别恢复为保留源视频 30%、50%、100% 画面的旧版策略；视频画面长边沿用旧版最大 1,920，Brush 最大训练分辨率分别为 1,200、1,600、2,000，且不额外设置 Splat 数量上限。

## 项目与文件位置

每次生成都会在项目根目录下创建一个独立文件夹：

```text
<项目根目录>\<yyyyMMdd-HHmmss_素材名>\
  final.ply             最终 Gaussian Splatting 文件
  project.json          项目元数据与结果指标
  edit.ply              当前编辑结果；再次保存时安全替换（可选）
  preview.mp4           首次动画预览视频导出（可选）
  preview-2.mp4         后续视频导出自动编号（可选）
  preview-landscape.mp4 横屏视频导出（后续自动编号）
  preview.html          离线查看器（后续 preview-2.html 等自动编号）
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

OOOSplat 默认开启匿名使用统计，用于了解稳定性和各阶段耗时。可在右上角 **设置 → 隐私** 中随时关闭，关闭后不再发送匿名统计请求。

会发送的内容：

| 字段 | 说明 |
| --- | --- |
| 安装 ID | 首次启动生成的随机 UUID，不读取硬件序列号、MAC 地址或设备指纹 |
| 应用版本、操作系统、CPU 架构 | 例如 `0.5.0` / `windows` / `x86_64` |
| 事件名 | `daily_active`、`generation_started`、`generation_completed`、`generation_failed`、`pipeline_stage_completed`、`planner_evaluation`。`daily_active` 每天最多一次，应用升级后当天会再报一次 |
| 质量档位与输入类型 | 枚举值，例如 `balanced` / `video`；图片序列输入报 `images` |
| 阶段耗时与总耗时 | 毫秒 |
| 帧数与视频时长 | 分桶值，不是原始数量 |
| 自动优化效果指标 | 匿名的素材尺寸与数量、规划结果、注册结果、桥接补帧、Brush 配置、Splat 数量和阶段耗时，用于比较自动优化效果 |
| 失败阶段与错误码 | 枚举值，例如 `colmap_mapper_failed`；不含原始错误文本 |

匿名统计不会发送的内容：视频、图片、PLY 等任何素材；文件名、路径和项目名称；日志与命令输出；用户名或任何个人信息。

### 主动发送错误报告

生成失败后，可在报错弹窗点击“发送错误报告”，先查看脱敏后的完整内容，再选择“同意并发送”。报告包含错误原因、详细错误、失败时最近最多 200 行／64 KiB 日志、应用版本、系统版本和全部可检测的显卡信息；不包含素材、模型、设备序列号或匿名安装标识，路径、文件名和用户名等会移除。读取失败的信息会留空，不会猜测实际使用的显卡。

这是独立的一次性授权：即使关闭匿名统计，也可以主动发送，不会开启统计。取消预览不上传；发送失败仅允许手动重试，成功后显示报告编号。本地报告及预览在本次应用运行期间有效，没有查看时限，关闭应用后清除；云端收到的报告保留 30 天。接收服务尚未部署或网络不可用时，会明确提示发送失败，不会假报成功。服务端部署说明见 [错误报告接入说明](server/diagnostics/README.md)。

## 内置引擎

| 引擎 | 固定版本/构建 | 用途 |
| --- | --- | --- |
| FFmpeg / FFprobe | Windows x64 8.1 LGPL shared；macOS arm64 8.1.2 LGPL shared | 视频分析与抽帧 |
| COLMAP | 4.2.1 同一 commit；Windows/Linux CUDA + Caspar/Ceres；macOS Ceres CPU | 特征、匹配和相机重建 |
| Brush | OOOBrush ooo-v1.0.0；Windows/Linux x64、macOS arm64 | 无界面 Gaussian Splatting 训练与 PLY 导出 |

Windows、Ubuntu 和 macOS 的来源与校验策略分别记录在 [`engines/manifest.json`](engines/manifest.json)、[`engines/manifest.linux.json`](engines/manifest.linux.json) 和 [`engines/manifest.macos.json`](engines/manifest.macos.json)。大型引擎文件不会提交到 Git；开发者通过 `npm run setup:engines` 恢复本地运行时。Release 打包前会校验来源、哈希、架构、动态库闭包和 Brush CLI 参数。

第三方许可和通知位于 [`licenses/`](licenses/)：

- FFmpeg：LGPL-2.1-or-later，本项目选用的构建禁用 GPL/nonfree 组件。
- COLMAP：BSD-3-Clause。
- Brush：Apache-2.0。

## 本地开发

### 下载 COLMAP 开发包

手动运行 Actions 中的 **COLMAP-only development runtime**，选择 `all` 或指定平台，
构建成功后下载对应 Artifact。解开外层 ZIP 和内部运行归档，将顶层目录的内容放到：

- Windows：`engines/colmap/`，可执行文件为 `bin/colmap.exe`。
- Linux：`engines/linux/colmap/`，可执行文件为 `bin/colmap`。
- macOS：`engines/macos/arm64/colmap/`，可执行文件为 `bin/colmap`。

完整保留 COLMAP 运行库、许可证和元数据，不要多套一层目录。已有 FFmpeg 保持不变。
随后运行 `npm run dev:local` 或 `npm run build:local`；这两个本地入口自动下载、校验并缓存锁定的 OOOBrush，COLMAP 则不安装、下载或校验，
也不修改正式清单。应用运行时健康检查仍然保留，正式构建继续执行严格校验。

Artifact 保留 30 天；手动流水线需先存在于默认分支才能触发。
详细步骤见 [引擎开发包说明](engines/README.md#download-colmap-only-development-builds)。

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

Ubuntu 24.04 Alpha 的 `setup:engines` 安装校验后的 COLMAP 到 `engines/linux/colmap/`，并安装 Brush；FFmpeg/FFprobe 保持为系统软件包。Tauri 将 COLMAP 和 Brush 打入 `.deb`，不再声明系统 COLMAP 依赖。

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
dist-artifacts\OOOSplat-0.5.0-x64-windows.exe
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

# 图片序列使用同一命令，传入文件夹即可
cargo run --manifest-path src-tauri\Cargo.toml --bin splatstudio -- generate "D:\Photos\object" --projects-root "D:\Splat Projects" --quality balanced
```

开发或诊断时可以通过全局参数 `--engine-dir <路径>`，或环境变量 `OOOSPLAT_ENGINE_DIR`，覆盖默认引擎目录。

Linux 的 FFmpeg/FFprobe 保留各自的环境变量与 PATH 发现方式；Brush 默认只使用托管目录 `linux/brush/brush_app`，显式 `OOOSPLAT_BRUSH` 诊断覆盖也必须支持新 CLI，不再自动回退 PATH。COLMAP 只使用托管目录 `linux/colmap/bin/colmap`，不接受单独的 `OOOSPLAT_COLMAP` 或 PATH 回退。

## 常见问题

### 如何让 COLMAP 使用显卡？

无需手动选择。应用会检查内置 COLMAP CUDA 运行时、NVIDIA 驱动版本和显卡 Compute Capability，满足要求时自动使用 GPU 加速特征提取与匹配，否则自动回退到 CPU。CUDA 13.2.0 的最低兼容要求为 NVIDIA 驱动 580.00、Compute Capability 7.5；Caspar 另需实际 BA 探测通过，失败仅回退 Ceres，不影响可用的 SIFT GPU；实际检测结果和未启用原因会显示在“01 创建新任务”中。Brush 与 COLMAP 相互独立，会在运行时选择可用的图形后端。

### 不同显卡组合会如何处理？

COLMAP 和 Brush 是两个独立阶段：COLMAP 只有兼容的 NVIDIA CUDA 环境才能使用 GPU；Brush 在 Windows 和 Linux 上使用 Vulkan，在 Apple Silicon macOS 上使用 Metal。常见组合按以下方式处理；表中未另行标注平台的组合均指 Windows：

| 平台与显卡组合 | COLMAP | Brush 训练 | 自动优化的精细档显存配置 |
|---|---|---|---|
| Windows：单张 NVIDIA 独显 | 满足驱动和 Compute Capability 要求时使用 CUDA | 明确选择第一张独立 GPU，通过 Vulkan 运行 | 使用该 NVIDIA 显卡的总显存选择 Low、Standard 或 Large |
| Intel 核显 + 单张 NVIDIA 独显 | 使用符合要求的 NVIDIA CUDA | 明确选择第一张独立 GPU，避免误用 Intel 核显 | 使用 NVIDIA 总显存分档 |
| AMD 核显 + 单张 NVIDIA 独显 | 使用符合要求的 NVIDIA CUDA | 明确选择第一张独立 GPU，并仅为 Brush 子进程规避 AMD Switchable Graphics 隐式层 | 使用 NVIDIA 总显存分档 |
| 仅 AMD，或 Intel 核显 + AMD 独显 | 自动回退 CPU | 由 Vulkan 自动选择 AMD GPU；不会禁用 AMD 图形层 | 当前使用保守的 Low 配置 |
| 仅 Intel 核显或 Intel 独显 | 自动回退 CPU | 由 Vulkan 自动选择 Intel GPU | 当前使用保守的 Low 配置 |
| 多张 NVIDIA 显卡 | 选择 Compute Capability 最高的兼容显卡；相同时选择 NVIDIA 索引较小者 | 因 Vulkan 与 CUDA 的设备索引不能可靠对应，不强制 Brush 的设备索引，由 Vulkan 自动选择 | 使用 COLMAP 选中显卡的总显存分档 |
| AMD 独显 + NVIDIA 独显 | COLMAP 使用符合要求的 NVIDIA | 当前会进入单 NVIDIA 优先策略，但跨品牌独显的 Vulkan 排序无法完全保证；建议同时在 Windows 图形设置中把 `brush_app.exe` 设为“高性能”并检查任务日志中的实际适配器 | 使用 NVIDIA 总显存分档 |
| NVIDIA 驱动过旧、型号不兼容或检测失败 | 自动回退 CPU | Brush 仍会尝试通过 Vulkan 自动选择可用 GPU，但不会强制 NVIDIA | 当前使用保守的 Low 配置 |
| 没有可用 GPU 图形后端 | COLMAP 使用 CPU | Brush 可能无法启动；CPU-only 软件图形后端尚未完成端到端验证 | 不适用 |
| macOS：Apple Silicon M 系列（含 Pro、Max、Ultra） | 当前使用 CPU，不启用 CUDA | 自动使用 M 系列芯片的 Metal GPU 和统一内存 | 当前不会按统一内存容量提升显存档，使用保守的 Low 配置 |
| Ubuntu Alpha：NVIDIA、AMD 或 Intel GPU | 当前使用 CPU | 通过 Vulkan 自动选择可用 GPU | 当前使用保守的 Low 配置 |

上述强制选卡和 AMD 隐式层规避只通过环境变量传给单次 `brush_app.exe` 子进程，不会修改系统环境、驱动设置或其他应用，也不会将 Brush 切换到 D3D12。AMD-only 设备不会应用 AMD 隐式层规避。

如果 Brush 日志显示 `Device Lost`，OOOSplat 会将其与显存不足区分并给出提示，但不会自动降低质量重试。建议接通电源、关闭其他 GPU 高负载程序，并在 Windows“设置 → 系统 → 显示 → 图形”中将 `brush_app.exe` 设为“高性能”。精细档只有在明确识别为显存不足时才会降低一级配置，并且最多重试一次。

M 系列芯片的 CPU 与 GPU 共享统一内存，但当前 Planner 不会把统一内存等同于独立显存。因此，即使是内存较大的 M 系列 Pro、Max 或 Ultra，精细档目前仍采用 Low 配置；这属于保守兼容策略，不代表 Metal 训练只能使用少量统一内存。

### 为什么会出现注册率较低的警告？

注册率较低通常表示可用于重建的连续视角不足。任务仍会继续进入 Brush，但结果质量可能受影响。建议使用曝光稳定、画面清晰、运动连续、视角重叠充分的环绕拍摄视频，避免快速转动、强反光、大面积纯色和运动物体。

### 为什么项目占用空间很大？

每个项目会保留源视频副本、抽帧、COLMAP 数据和 Brush 中间文件，便于诊断和追溯。确认结果后，可通过历史任务中的“删除”将整个项目移入回收站。

### 可以直接在应用中查看 final.ply 吗？

可以。在“02 历史任务”中选择已完成项目并点击“预览”，即可在独立预览工作区浏览 `.ply`。“调整”模式支持整体 Transform，以及矩形、球形和盒形 Gaussian 选择；可以非破坏式删除、裁切、撤销和重做。编辑状态会保存在项目中，点击“保存”生成或更新唯一的 `edit.ply`，原始 `final.ply` 始终不变。“动画”模式提供 5 秒显现、8 秒冲击波、持续环绕以及带水印的竖屏 MP4 导出；`.sog` 和 `.spz` 尚未开放。

## 技术栈

预览的“导出”菜单支持竖屏/横屏视频和单文件离线 HTML。HTML 初始展示完整编辑结果，可手动播放动画，鼠标操作会暂停播放；支持 WebGPU 优先、WebGL2 回退，不包含项目路径、源素材、日志或遥测。开发和构建命令会预先生成离线查看器资源，单独运行 Rust 前可先执行 `npm run build:html-viewer`。

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
