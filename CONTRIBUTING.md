# Contributing to OOOSplat

[中文](#中文) · [English](#english)

<a id="中文"></a>

## 中文

OOOSplat 仍在持续开发中。我们欢迎各种形式的贡献，包括 Bug 修复、UI/UX 改进、文档、平台支持、性能优化和新功能。

参与贡献并不要求你熟悉 Gaussian Splatting。桌面应用、开发体验、文档和测试方面的改进同样很有价值。

### 开始之前

- 先搜索[现有 Issues](https://github.com/ooolabdev/ooosplat/issues)，确认是否已经有人处理相同问题。
- 小型 Bug 修复和文档修改可以直接提交 Pull Request。
- 对于较大的改动，建议先创建 Issue，讨论方向和范围。

以下类型的改动尤其建议先讨论：

- macOS 或 Linux 支持
- CUDA 加速
- COLMAP 流水线改动
- Brush 集成改动
- Gaussian 查看器或编辑器改动
- 新的输入格式
- 新的导出格式

如果仓库中有标记为 `good first issue` 或 `help wanted` 的 Issue，它们很适合新贡献者参与。

### 开发环境

当前正式支持的开发环境为 Windows 10/11 x64，需要：

- Node.js 22.12 或更高版本
- Rust stable，并安装 `x86_64-pc-windows-msvc` target
- Visual Studio 2022 Build Tools，并包含 **Desktop development with C++**
- Tauri 2 所需的 WebView2 开发和运行环境

先在 GitHub 上 Fork 仓库，然后克隆你自己的 Fork：

```powershell
git clone https://github.com/<your-github-username>/ooosplat.git
cd ooosplat
```

安装 npm 依赖和锁定版本的原生引擎：

```powershell
npm install
npm run setup:engines
```

`setup:engines` 会下载并校验 FFmpeg/FFprobe、COLMAP 和 Brush 运行时。这些大型二进制文件不会存入 Git。

启动 Tauri 开发版本：

```powershell
npm run tauri -- dev
```

OOOSplat 当前使用 Tauri 2、React、TypeScript、Vite、Rust 和 Tokio。原生处理流水线使用 FFmpeg/FFprobe、CPU/no-CUDA COLMAP 和 Brush。

### 分支

请从最新的 `main` 创建分支。项目不强制分支命名格式，但清晰的名称有助于理解改动：

```text
feature/linux-support
feature/cuda-colmap
fix/progress-display
docs/update-readme
```

### 保持 Pull Request 聚焦

每个 Pull Request 尽量只处理一个功能或问题。不要在功能改动中夹杂无关重构、大规模格式化或 UI 重设计。

- **推荐：**修复 FFmpeg 进度解析，并加入对应测试。
- **避免：**在一个 PR 中同时修复进度解析、重做任务界面并格式化无关 Rust 模块。

小而聚焦的 Pull Request 更容易 review、测试和合并。

### 测试

日常开发用 `npm test` 运行快速前端检查。Windows 原生 E2E 耗时较长，需主动执行 `npm run test:e2e`，或从 GitHub Actions 手动触发。 [原生 E2E 运行说明](tests/e2e/README.md)

请根据改动范围运行相关检查：

```powershell
# 前端测试
npm test

# TypeScript 类型检查和前端生产构建
npm run build

# Rust 测试
cargo test --manifest-path src-tauri\Cargo.toml

# Rust 静态检查
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings

# 内置引擎版本、哈希和 CPU/no-CUDA 校验
npm run verify:engines

# 项目和第三方许可校验
npm run verify:licenses
```

仓库目前没有独立的 npm lint、format 或 mock development 命令。请不要声称运行了仓库并不存在的检查。

如果改动涉及处理流水线，请说明实际测试过哪些环节：

- FFmpeg / FFprobe
- COLMAP
- Brush
- Gaussian PLY 输出

如果改动与平台有关，请附上操作系统及版本、CPU 架构，以及相关情况下的 GPU 型号。

### Pull Requests

Pull Request 应提交到 `main`。建议在描述中包含：

- 修改了什么
- 为什么需要修改
- 如何测试
- UI 改动的截图
- 关联的 Issue

使用 `Refs #123` 可以关联但不自动关闭 Issue。只有当 PR 完整解决对应 Issue 时才使用 `Closes #123`。

Pull Request 在提交时不需要做到完美。我们欢迎较早提交或使用 Draft PR，并可以在 review 中继续讨论实现细节。

### 报告 Bug

有效的 Bug 报告建议包含：

- OOOSplat 版本或 commit
- 操作系统
- CPU 和 GPU
- 输入视频或图片信息
- 失败的流水线阶段
- 相关日志
- 清晰的复现步骤

公开日志前，请移除私人路径和敏感信息。

### 功能建议

提出功能建议时，请说明：

- 它解决什么问题
- 预期行为
- 相关示例、项目或参考资料

清晰的使用场景有助于讨论功能范围，并不要求你提前提供完整技术设计。

### 许可证

OOOSplat 使用 [Apache License 2.0](LICENSE)。

向 OOOSplat 提交贡献，即表示你同意该贡献采用与本项目相同的 Apache License 2.0。

---

<a id="english"></a>

## English

OOOSplat is still under active development. Contributions are welcome in many forms: bug fixes, UI/UX improvements, documentation, platform support, performance work, and new features.

You do not need to be familiar with Gaussian Splatting to contribute. Improvements to the desktop app, developer experience, documentation, and testing are all valuable.

### Before You Start

- Search the [existing Issues](https://github.com/ooolabdev/ooosplat/issues) to see whether someone is already working on the same topic.
- Small bug fixes and documentation updates can go directly to a pull request.
- For larger changes, please open an Issue first so the direction and scope can be discussed.

Please discuss these areas before investing in a large implementation:

- macOS or Linux support
- CUDA acceleration
- COLMAP pipeline changes
- Brush integration changes
- Gaussian viewer or editor changes
- new input formats
- new export formats

Issues labeled `good first issue` or `help wanted`, when available, are good places for new contributors to start.

### Development Setup

The currently supported development environment is Windows 10/11 x64. You will need:

- Node.js 22.12 or newer
- Rust stable with the `x86_64-pc-windows-msvc` target
- Visual Studio 2022 Build Tools with **Desktop development with C++**
- the WebView2 development/runtime environment required by Tauri 2

Fork the repository on GitHub, then clone your fork:

```powershell
git clone https://github.com/<your-github-username>/ooosplat.git
cd ooosplat
```

Install the npm dependencies and the pinned native engines:

```powershell
npm install
npm run setup:engines
```

`setup:engines` downloads and verifies the FFmpeg/FFprobe, COLMAP, and Brush runtimes. These large binaries are not stored in Git.

Start the Tauri development application:

```powershell
npm run tauri -- dev
```

OOOSplat currently uses Tauri 2, React, TypeScript, Vite, Rust, and Tokio. Its native processing pipeline uses FFmpeg/FFprobe, CPU/no-CUDA COLMAP, and Brush.

### Branches

Create your branch from the latest `main`. Branch names are not enforced, but descriptive names help:

```text
feature/linux-support
feature/cuda-colmap
fix/progress-display
docs/update-readme
```

### Keep Pull Requests Focused

Try to keep each pull request focused on one feature or problem. Avoid mixing unrelated refactors, large formatting changes, or UI redesigns into a functional change.

- **Good:** fix FFmpeg progress parsing and add the related test.
- **Avoid:** fix progress parsing, redesign the task screen, and reformat unrelated Rust modules in one PR.

Small, focused pull requests are easier to review, test, and merge.

### Testing

Use `npm test` for fast frontend checks during development. The slower Windows native E2E suite runs on demand via `npm run test:e2e` or a manual GitHub Actions dispatch. [Native E2E instructions](tests/e2e/README.md)

Run the checks relevant to your change:

```powershell
# Frontend tests
npm test

# TypeScript type-check and frontend production build
npm run build

# Rust tests
cargo test --manifest-path src-tauri\Cargo.toml

# Rust static checks
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings

# Bundled engine version, hash, and CPU/no-CUDA checks
npm run verify:engines

# Project and third-party license checks
npm run verify:licenses
```

The repository does not currently define separate npm lint, format, or mock-development commands. Do not claim to have run checks that are not available.

For pipeline changes, describe which stages you tested in practice:

- FFmpeg / FFprobe
- COLMAP
- Brush
- Gaussian PLY output

For platform-specific changes, include the operating system and version, CPU architecture, and GPU model when relevant.

### Pull Requests

Open pull requests against `main`. A helpful description includes:

- what changed
- why the change is needed
- how it was tested
- screenshots for UI changes
- the related Issue

Use `Refs #123` to associate a pull request with an Issue without closing it. Use `Closes #123` only when the pull request completely resolves that Issue.

Your PR does not need to be perfect before submission. Early or draft PRs are welcome, and implementation details can be discussed during review.

### Reporting Bugs

A useful bug report includes:

- OOOSplat version or commit
- operating system
- CPU and GPU
- input video or image information
- the pipeline stage that failed
- relevant logs
- clear steps to reproduce the problem

Please remove private paths or sensitive information from logs before posting them publicly.

### Feature Requests

When proposing a feature, explain:

- what problem it solves
- the expected behavior
- relevant examples, projects, or references

Clear use cases help the project discuss scope without requiring a complete technical design.

## License

By submitting a contribution to OOOSplat, you agree that your contribution
will be licensed under the Apache License 2.0, the same license used by the project.
