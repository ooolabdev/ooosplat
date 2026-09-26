# OOOSplat 全链路 AMD GPU 加速实现方案（DirectML）

> 让 OOOSplat（视频/图片 → 3D Gaussian Splatting 桌面工具）的 COLMAP 重建阶段在 AMD 显卡上实现 GPU 加速。
>
> **验证环境**：AMD Radeon RX 6750 GRE 10GB（RDNA2）· Windows · COLMAP 4.2.0 · onnxruntime-directml 1.17.3
>
> **最终状态**：✅ 全链路验证通过（ALIKED 特征提取 + LightGlue 匹配在 AMD 卡上跑通，OOOSplat UI 显示「COLMAP GPU（DirectML/ONNX）已就绪」）

---

## 1. 背景与目标

### 1.1 问题

OOOSplat 的流水线为「抽帧 → COLMAP 稀疏重建 → Brush 高斯训练 → 预览」。其中：

- **Brush 训练**：基于 Rust 的 `wgpu` 抽象层（Vulkan/Metal/DX12），**天然支持 AMD**，无需改动。
- **COLMAP 重建**：特征提取 + 特征匹配 + 位姿估计。其 GPU 加速从诞生起就是 **NVIDIA CUDA 专属**（SIFT-GPU），AMD 卡只能回退 CPU。

因此「全链路 AMD」的唯一瓶颈是 **COLMAP 重建阶段**。

### 1.2 目标

让 AMD 显卡也能 GPU 加速 COLMAP 的特征提取与匹配，使整个 OOOSplat 流水线 100% 不依赖 NVIDIA。

---

## 2. 技术路线选型

| 候选路线 | 结论 | 原因 |
|---|---|---|
| ZLUDA（CUDA→HIP 翻译层） | ❌ 排除 | RDNA2 是 experimental；且 OOOSplat 启动时会主动探测真实 CUDA 驱动，探测失败即回退 CPU，ZLUDA 的兼容层不被认可 |
| COLMAP 4.2.0 官方 HIP/ROCm | ❌ 无效 | HIP 只加速 `patch_match_stereo`（稠密 MVS），而 OOOSplat 只用稀疏重建（SfM），用不到 |
| COLMAP 官方 Windows 二进制 | ❌ 无效 | 只带 CUDA 版 onnxruntime，ONNX 推理在 AMD 上无 DirectML 后端 |
| **COLMAP 4.2.0 ONNX 特征 + DirectML** | ✅ **选定** | 4.2.0 新增 ALIKED/LoMa 特征 + LightGlue 匹配（走 ONNX），DirectML 可在任意 DX12 显卡（含 AMD）上加速 |

**核心思路**：COLMAP 4.2.0 已经写好了 ONNX 特征这条加速路（ALIKED + LightGlue），但默认只接了 CUDA 后端。我们要做的是**给 ONNX 推理接上 DirectML 后端**，让它跑在 AMD 卡上。

---

## 3. COLMAP 源码改造

> 关键前提：COLMAP 4.2.0 官方源码里，ONNX 推理的执行后端枚举 `ONNXExecutionProvider` 只有 `CPU / CUDA / COREML` 三种，**没有 DirectML**。所以光换 DirectML 版 onnxruntime 不够，必须补上 DirectML 后端。

共修改 6 个文件：

### 3.1 `cmake/FindDependencies.cmake` — 定义 DML 编译宏

在 `if(TARGET onnxruntime::onnxruntime)` 块内，新增：

```cmake
    # Enable the DirectML execution provider for ONNX inference on Windows
    # AMD/Intel GPUs (requires a DirectML-capable onnxruntime, no CUDA).
    if(DML_ENABLED)
        list(APPEND COLMAP_COMPILE_DEFINITIONS COLMAP_DML_ENABLED)
        message(STATUS "Enabling ONNX DirectML execution provider")
    endif()
```

### 3.2 `CMakeLists.txt` — 新增 DML_ENABLED 编译选项

```cmake
option(ONNX_ENABLED "Whether to enable ONNX, if available" ON)
option(DML_ENABLED "Whether to enable the ONNX DirectML execution provider (Windows AMD/Intel GPUs)" OFF)
```

### 3.3 `src/colmap/feature/onnx_utils.h` — 枚举加 DML

```cpp
enum class ONNXExecutionProvider {
  CPU,
  CUDA,
  COREML,
  DML,   // 新增
};
```

### 3.4 `src/colmap/feature/onnx_utils.cc` — 三处改动

**① 引入 DirectML 头文件**（在 CoreML include 后）：

```cpp
#ifdef COLMAP_DML_ENABLED
#include <dml_provider_factory.h>
#endif
```

**② `SelectONNXExecutionProvider` 加 DML 分支**：

```cpp
#ifdef COLMAP_CUDA_ENABLED
  return ONNXExecutionProvider::CUDA;
#elif defined(COLMAP_COREML_ENABLED)
  return ONNXExecutionProvider::COREML;
#elif defined(COLMAP_DML_ENABLED)      // 新增
  return ONNXExecutionProvider::DML;
#else
  return ONNXExecutionProvider::CPU;
#endif
```

**③ `InitializeSession` 里启用 DirectML provider**：

```cpp
  const bool use_dml = execution_provider_ == ONNXExecutionProvider::DML;
#ifdef COLMAP_DML_ENABLED
  if (use_dml) {
    VLOG(2) << "Enabling DirectML execution provider";
    const OrtApi* ort_api = OrtGetApiBase()->GetApi(ORT_API_VERSION);
    const OrtDmlApi* dml_api = nullptr;
    Ort::ThrowOnError(ort_api->GetExecutionProviderApi(
        "DML", ORT_API_VERSION, reinterpret_cast<const void**>(&dml_api)));
    Ort::ThrowOnError(dml_api->SessionOptionsAppendExecutionProvider_DML(
        static_cast<OrtSessionOptions*>(session_options_), /*device_id=*/0));
  }
#endif
```

> 注意：`device_id=0` 对应 DXGI 枚举的默认显卡（主显示 GPU）。

### 3.5 `src/colmap/feature/extractor.cc` — 放行 DML

`FeatureExtractionOptions::Check()` 中，原检查会拒绝非 CUDA/OpenGL 的 GPU：

```cpp
#if !defined(COLMAP_GPU_ENABLED) && !defined(COLMAP_CUDA_ENABLED) && \
    !defined(COLMAP_DML_ENABLED)
    LOG(ERROR) << "Cannot use GPU feature extraction without CUDA, OpenGL, or "
                  "DirectML support. Consider setting use_gpu to false.";
    return false;
#endif
```

### 3.6 `src/colmap/feature/matcher.cc` — 放行 DML

`FeatureMatchingOptions::Check()` 同理：

```cpp
#if !defined(COLMAP_GPU_ENABLED) && !defined(COLMAP_DML_ENABLED)
    LOG(ERROR) << "Cannot use GPU feature matching without CUDA, OpenGL, or "
                  "DirectML support. Set use_gpu to false.";
    return false;
#endif
```

### 3.7 `aliked.cc` / `loma.cc` — 描述子维度运行时解析

部分执行后端（如 DirectML）在 session 创建时把模型输出维度报告成动态 `-1`（ALIKED/LoMa 的描述子维度实际是静态的）。因此构造时不再强制要求正数，而是接受动态维度，提取时再从输出张量解析真实值：

```cpp
// 构造时：接受动态维度（-1）
descriptor_dim_ = static_cast<int>(model_.output_shapes()[1][2]);
// DirectML 等后端会把静态描述子维度报告成动态 -1，这里接受，
// 提取时再从输出张量解析真实值。
THROW_CHECK(descriptor_dim_ == -1 || descriptor_dim_ > 0);

// 提取时：从实际输出张量解析真实维度
const int descriptor_dim = static_cast<int>(descriptors_shape[2]);
THROW_CHECK_GT(descriptor_dim, 0);
if (descriptor_dim_ > 0) {
  THROW_CHECK_EQ(descriptor_dim, descriptor_dim_);
}
```

> 说明：这里没有硬编码 128，而是做了通用处理，`loma.cc` 用同样的方式改了一遍，这样对任何执行后端都健壮。

---

## 4. onnxruntime 版本问题（核心突破）

### 4.1 问题：onnxruntime-directml 1.18+ 在 AMD 上有崩溃 bug

初始使用 onnxruntime-directml **1.24.4**，在 RX 6750 GRE 上 `Ort::Session` 创建时**段错误崩溃（SIGSEGV，退出码 139）**。

排查后确认这是 **onnxruntime-directml 的已知 bug**（社区 issue #20713 / #22867）：

> - ORT 1.17.3 → ✅ working（AMD 稳定）
> - ORT 1.18 / 1.18.1 / 1.19 / 1.20 → ❌ 在 AMD GPU 上崩溃（1.20 甚至导致显卡驱动崩溃）

### 4.2 解决：降级到 1.17.3

换用 **onnxruntime-directml 1.17.3**，依赖 **Microsoft.AI.DirectML 1.13.1**：

| 版本 | onnxruntime | Microsoft.AI.DirectML |
|---|---|---|
| 原方案（崩溃） | 1.24.4 | 1.15.4 |
| **最终方案** | **1.17.3** | **1.13.1** |

### 4.3 降级带来的两个关键差异

1. **`ORT_API_VERSION` 变化**：1.24.4 是 `24`，1.17.3 是 `17`。**必须连头文件一起换**（否则编译出的 `GetApi(24)` 在 1.17.3 DLL 里返回空指针）。
2. **运行时文件形态变化**：1.17.3 把 DirectML 编译进 `onnxruntime.dll`，**没有 `onnxruntime_providers_shared.dll`**，只需 `onnxruntime.dll` + `DirectML.dll` 两个文件。

### 4.4 运行时文件整理

`ort-dml` 目录最终结构：

```
ort-dml/
  bin/
    onnxruntime.dll          # 1.17.3（内建 DirectML）
    DirectML.dll             # Microsoft.AI.DirectML 1.13.1
  lib/
    onnxruntime.lib          # 1.17.3 导入库
  include/
    onnxruntime_c_api.h      # ORT_API_VERSION=17
    onnxruntime_cxx_api.h
    dml_provider_factory.h
    ...（其余头文件）
  cmake/
    onnxruntimeConfig.cmake  # 自写最小版（NuGet 包不含此文件）
```

`onnxruntimeConfig.cmake`（最小版，让 COLMAP 的 `find_package(onnxruntime CONFIG)` 能找到）：

```cmake
get_filename_component(_ORT_ROOT "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
set(onnxruntime_VERSION "1.17.3")
set(onnxruntime_INCLUDE_DIR "${_ORT_ROOT}/include")
set(onnxruntime_LIB_DIR "${_ORT_ROOT}/lib")
set(onnxruntime_BINARY_DIR "${_ORT_ROOT}/bin")
set(onnxruntime_FOUND TRUE)
if(NOT TARGET onnxruntime::onnxruntime)
  add_library(onnxruntime::onnxruntime SHARED IMPORTED)
  set_target_properties(onnxruntime::onnxruntime PROPERTIES
    IMPORTED_LOCATION "${_ORT_ROOT}/bin/onnxruntime.dll"
    IMPORTED_IMPLIB "${_ORT_ROOT}/lib/onnxruntime.lib"
    INTERFACE_INCLUDE_DIRECTORIES "${_ORT_ROOT}/include")
endif()
```

---

## 5. COLMAP 构建步骤

### 5.1 前置条件

- Visual Studio 2026（含「使用 C++ 的桌面开发」工作负载，即 MSVC）
- CMake ≥ 3.25、Git、PowerShell Core 7.x、7zip

### 5.2 依赖准备

1. **下载 onnxruntime-directml 1.17.3 + DirectML 1.13.1**（NuGet 包，解压取 `onnxruntime.dll`/`onnxruntime.lib`/头文件/`DirectML.dll`），整理成上面的 `ort-dml` 目录。
2. **下载 ONNX 模型**（运行时需要，缓存到 `%USERPROFILE%\.cache\colmap`）：

```
aliked-n16rot.onnx    (ALIKED 特征，SHA256 39c423d0...78a547)
aliked-lightglue.onnx (LightGlue 匹配，SHA256 b9a5de72...2dceb8d)
```

   文件名格式：`<sha256>-<name>`。

### 5.3 源码准备

无需改动 `vcpkg.json`（保持上游原样）。代码里已新增 `DML_ENABLED` 选项：开启 `-DDML_ENABLED=ON` 时，`CMakeLists.txt` 会自动跳过 vcpkg 的 `onnx` 特性（该特性会拉 CUDA 版 onnxruntime），改用自备的 DirectML 版 onnxruntime（通过 `find_package` 接入）。

### 5.4 配置与编译

```powershell
cmake -B build -G "Visual Studio 18 2026" -A x64 `
  -DCMAKE_TOOLCHAIN_FILE=D:\a\VC\vcpkg\scripts\buildsystems\vcpkg.cmake `
  -DONNX_ENABLED=ON -DFETCH_ONNX=OFF -DCUDA_ENABLED=OFF `
  -DDML_ENABLED=ON `
  -DGUI_ENABLED=OFF -DMVS_ENABLED=OFF -DCGAL_ENABLED=OFF `
  -Donnxruntime_CONFIG_DIR_HINTS=C:\...\ort-dml\cmake `
  -Donnxruntime_INCLUDE_DIR_HINTS=C:\...\ort-dml\include `
  -Donnxruntime_LIBRARY_DIR_HINTS=C:\...\ort-dml\lib

cmake --build build --config Release
```

关键开关说明：

| 开关 | 作用 |
|---|---|
| `-DONNX_ENABLED=ON` | 启用 ALIKED/LoMa/LightGlue |
| `-DFETCH_ONNX=OFF` | 不从 vcpkg 拉 CUDA 版 onnxruntime，改用自备 DirectML 版 |
| `-DDML_ENABLED=ON` | 启用 DirectML 后端（本次改造新增） |
| `-DGUI/MVS/CGAL=OFF` | 关闭 OOOSplat 用不到的模块，大幅省编译时间 |
| `-DCUDA_ENABLED=OFF` | 不需要 NVIDIA |

编译产物：`build/src/colmap/exe/Release/colmap.exe` + 依赖 DLL。

---

## 6. OOOSplat 代码改造

### 6.1 `src-tauri/src/engines/colmap.rs`

新增 `ColmapFeatureMode` 枚举，DirectML 模式下切换特征/匹配类型：

```rust
pub enum ColmapFeatureMode { Sift, Onnx }

impl ColmapFeatureMode {
    pub const fn feature_type(self) -> &'static str {
        match self { Self::Sift => "SIFT", Self::Onnx => "ALIKED_N16ROT" }
    }
    pub const fn matcher_type(self) -> &'static str {
        match self { Self::Sift => "SIFT_BRUTEFORCE", Self::Onnx => "ALIKED_LIGHTGLUE" }
    }
}
```

ONNX 模式输出 `--FeatureExtraction.type ALIKED_N16ROT` + `--FeatureMatching.type ALIKED_LIGHTGLUE`。

### 6.2 `src-tauri/src/engines/health.rs`

新增 DirectML 检测与模式：

- `directml_staged_near_colmap()`：在 colmap 目录查找 `DirectML.dll`（主信号）
- `OOOSPLAT_COLMAP_GPU=directml` 模式
- `feature_mode()`：DirectML 下返回 `Onnx`
- `gpu_index()`：DirectML 下返回 `None`（不传 `--*gpu_index`，device_id 固定 0）
- `wants_colmap_gpu()`：DirectML 下返回 `true`（传 `--*use_gpu 1`）

### 6.3 `src-tauri/src/pipeline/runner.rs`

按加速状态计算 `feature_mode` 并传给 COLMAP 三阶段调用。

### 6.4 前端

`src/types/pipeline.ts` 加 `directmlReady` 类型；`App.tsx` / `i18n` 加「COLMAP GPU（DirectML/ONNX）」文案。

---

## 7. OOOSplat 编译与部署

### 7.1 编译（关键：必须用 Tauri CLI，不能直接 cargo build）

```powershell
npm install
npm run build   # 前端构建（生成 dist/）

# 用 tauri CLI 构建（--config 覆盖 beforeBuildCommand 绕过引擎版本校验）
npx tauri build --no-bundle --config '{"build":{"beforeBuildCommand":"npm run build"}}'
```

> **重要坑**：直接 `cargo build --release` **不会设置 Tauri 的 `TAURI_ENV_*` 构建环境变量**，导致 exe 仍走 devUrl（`localhost:1420`）而不是嵌入前端 `dist/`，双击报「localhost 拒绝连接」。必须用 `npx tauri build`。
>
> `--config` 把 `beforeBuildCommand` 覆盖成 `npm run build`（绕过默认 `build:bundle` 里的 `verify:engines`，因为 colmap 已换成 4.2.0 会导致引擎版本校验失败）。

产物：`src-tauri/target/release/ooo-splat.exe`（已嵌入前端）。

### 7.2 部署 colmap 引擎

1. 把编译出的 `colmap.exe` + 全部依赖 DLL（含 `onnxruntime.dll`、`DirectML.dll`）复制到 `engines/colmap/bin/`。
2. 清理旧的 CUDA/ZLUDA 残留 DLL（`onnxruntime_providers_cuda.dll`、`nvcuda.dll`、`cudart64_12.dll` 等）。

### 7.3 清除遗留环境变量

删除之前 ZLUDA 方案设置的用户级环境变量（它会强制走 CPU）：

```powershell
[Environment]::SetEnvironmentVariable('OOOSPLAT_COLMAP_GPU', $null, 'User')
```

删除后 OOOSplat 走 Auto 模式，自动识别 DirectML（Auto 模式下 DirectML 优先级高于 ZLUDA）。

---

## 8. 验证结果

### 8.1 命令行验证（COLMAP 直接跑）

```powershell
# ALIKED 特征提取（GPU/DirectML）
colmap.exe feature_extractor --database_path test.db --image_path images `
  --ImageReader.camera_model SIMPLE_RADIAL --ImageReader.single_camera 1 `
  --FeatureExtraction.type ALIKED_N16ROT --FeatureExtraction.use_gpu 1

# LightGlue 匹配（GPU/DirectML）
colmap.exe sequential_matcher --database_path test.db `
  --FeatureMatching.type ALIKED_LIGHTGLUE --FeatureMatching.use_gpu 1
```

结果：退出码 0，5 张测试图特征数 454/477/492/417（与 CPU 结果一致，推理正确），耗时 0.025 分钟（CPU 为 0.047 分钟，GPU 快近一倍）。

### 8.2 OOOSplat 引擎检测验证

```powershell
splatstudio.exe --engine-dir D:/soft/OOOSplat/engines health
```

COLMAP 段输出：

```json
"acceleration": {
  "backend": "gpu",
  "reasonCode": "directmlReady",
  "reason": "COLMAP GPU（DirectML/ONNX）已就绪",
  "device": { "index": 0, "name": "AMD/Intel (DirectML)" }
}
```

---

## 9. 踩坑记录（重要教训）

| # | 坑 | 解法 |
|---|---|---|
| 1 | onnxruntime-directml **1.18+ 在 AMD 上崩溃**（社区已知 bug） | 降级到 **1.17.3**（最后稳定版） |
| 2 | 降级后 `ORT_API_VERSION` 24→17 | 头文件、lib、dll **必须整套一起换** |
| 3 | COLMAP 源码**没有 DirectML 后端**（枚举只有 CPU/CUDA/COREML） | 需改 6 个文件补 DirectML 支持 |
| 4 | `extractor.cc`/`matcher.cc` 的 `Check()` 有前置 GPU 检查会拒绝 DML | 加 `!defined(COLMAP_DML_ENABLED)` 放行 |
| 5 | DirectML 把模型输出维度报成动态 `-1`（描述子维度） | `aliked.cc`/`loma.cc` 构造时接受动态维度，提取时运行时解析 |
| 6 | deprecated 的 `OrtSessionOptionsAppendExecutionProvider_DML` | 改用官方推荐的 `OrtDmlApi`（`GetExecutionProviderApi` 获取） |
| 7 | NuGet 包不含 `onnxruntimeConfig.cmake` | 自写最小版 config |
| 8 | 直接 `cargo build` 不嵌入前端（Tauri 环境变量缺失） | 用 `npx tauri build --no-bundle` |
| 9 | 遗留 `OOOSPLAT_COLMAP_GPU=off` 环境变量强制走 CPU | 删除该用户级环境变量 |

---

## 10. 最终交付物

| 产物 | 路径 |
|---|---|
| 可运行程序 | `D:\soft\OOOSplat\ooo-splat-directml.exe`（release，已嵌入前端） |
| 原始 exe | `D:\soft\OOOSplat\src-tauri\target\release\ooo-splat.exe` |
| DirectML 版 COLMAP | `D:\soft\OOOSplat\engines\colmap\bin\colmap.exe`（4.2.0 + DirectML） |
| onnxruntime 运行时 | `D:\soft\OOOSplat\engines\colmap\bin\onnxruntime.dll` + `DirectML.dll` |
| COLMAP 源码 | `C:\Users\ccpp1220\colmap`（含全部 DirectML 改动） |
| ONNX 模型缓存 | `C:\Users\ccpp1220\.cache\colmap\` |

### 使用方式

双击 `ooo-splat-directml.exe`，进设置确认 COLMAP 加速状态显示「COLMAP GPU（DirectML/ONNX）已就绪」，即可在 AMD 显卡上进行 GPU 加速重建。

---

## 附：COLMAP 代码改动文件清单

```
colmap/
  CMakeLists.txt                              # 新增 DML_ENABLED 选项 + DML 下跳过 onnx 特性
  cmake/FindDependencies.cmake                # 定义 COLMAP_DML_ENABLED 宏
  src/colmap/feature/onnx_utils.h             # 枚举加 DML
  src/colmap/feature/onnx_utils.cc            # DML provider 调用（OrtDmlApi）
  src/colmap/feature/extractor.cc             # Check() 放行 DML
  src/colmap/feature/matcher.cc               # Check() 放行 DML
  src/colmap/feature/aliked.cc                # 描述子维度运行时解析
  src/colmap/feature/loma.cc                  # 描述子维度运行时解析
```

> 上述 COLMAP 改动已作为 **PR #4777** 提交给 colmap 上游（https://github.com/colmap/colmap/pull/4777），本仓库附带的 `colmap-directml.patch` 与其保持一致。
