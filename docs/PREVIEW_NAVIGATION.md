# 预览导航：平移、漫游与定位视角

此文档对应当前 `main` 源码，不代表已发布安装包。

## 使用方式

- 按住任一 WASD 时申请锁定并隐藏鼠标，无需按住鼠标键即可原地转向；W/S 的行进方向随视线转向，包含俯仰变化，A/D 始终水平侧移。最后一个 WASD 松开或按 Esc 后恢复光标；Esc 不清空框选，需重新按下漫游键才可再次进入。锁定期间不响应编辑、框选、操作轴和滚轮；不支持或拒绝锁定时保留原有 WASD＋拖动操作，不弹窗或循环申请。鼠标转向保留侧倾、FOV、距离及画面偏移，不触发绕模型旋转的调平。

- 点击预览画布或“自动定位视角”后即可使用 W/S 沿当前视线前后移动：抬头按 W 上升，低头按 W 下降，S 沿相反方向移动。A/D 按相机水平朝向左右侧移，不受俯仰或侧倾影响；垂直朝上／朝下时仍保留当前水平朝向。相机与观察中心同步平移，不重新朝向模型。基础速度为每秒当前可见画面高度的15%（原倍率的15%），随滚轮放大减速、缩小加速，Shift 三倍加速；相同距离和 FOV 下倍率一致。只在“调整”模式的透视视图中生效；正交视图不响应、不自动转为透视。
- 输入框、弹窗、导出、选择手势和操作轴拖动不会触发漫游；释放按键、失焦、切换工具或离开页面时停止。一次连续漫游结束后保存视角，不逐帧写本地存储。
- 底部黑色状态栏右侧的“漫游速度”支持0.1×–5.0×、每级0.1×，默认1.0×保持上述15%基础速度，Shift在此基础上继续三倍。偏好按本机保存，不进入项目或撤销记录；正交／三视图下灰显。滑轨为2px，保留18px操作高度。
- 画布编辑、编辑按钮、撤销／重做、保存结束后安全归还画布焦点；数值输入通过Enter主动确认、鼠标调速结束时也可直接继续WASD。Tab切换输入框、弹窗及正在执行的编辑／导出不被抢焦点；键盘调速时方向键仍操作滑轨，Enter再回到画布。迟到的异步操作、旧项目或旧预览实例不能恢复焦点。
- 三视图旁的纯文字按钮“自动定位视角”始终恢复首帧训练参考相机的位置、完整朝向及透视视场；从正视／侧视／顶视或自由正交视角点击，也会退出正交投影并取消三视图选中，不再仅居中原点并缩放。定位可以被鼠标、滚轮或键盘操作打断。
- 打开项目仍优先恢复上次本机视角。定位不改模型、不产生编辑撤销记录。
- 相机侧倾导致网格看起来歪斜时，主动旋转拖动超过阈值后会用约 180 毫秒平滑调平。自动定位和视角恢复仍保留原始完整朝向；单击、右键平移、滚轮和 WASD 不触发调平。矩形选择工具沿用中键旋转手势，不抢占左键选择。网格、模型和缩放均不因此调整。
- 右上角统一为“图标 + 按键 + 功能”，如“左键 旋转”“右键 平移”“滚轮 缩放”“WASD 漫游”；不再展开 Shift 加速或透视限制说明，正交模式下漫游仍灰显且不生效。定位同步将焦点交还画布，动画结束不会抢走后来打开的输入框或弹窗焦点。

2026-10-27 漫游速度滑轨、编辑后焦点恢复及抽帧控件的历史验证见[验证记录](PREVIEW_CONTROLS_VALIDATION.md)：632项前端与7项诊断测试、构建、中英文三种缩放布局及真实全景模型WebGL2/WebGPU预览均通过；这些记录不代表本次 W/S 视线移动已完成实际渲染验收，原生桌面人工验收仍待完成。

### 2026-11-10：W/S 沿视线移动

- W/S 使用包含俯仰的当前视线方向，A/D 继续按水平朝向侧移；合并方向后归一化，不改变15%基础速度、调速倍率、Shift加速和Pointer Lock生命周期。
- 前端完整844项、诊断协议7项、生产构建和差异检查通过。113项相机／鼠标转向定向测试覆盖抬头、低头、W/S反向、正负90°极点、侧倾下水平侧移、组合键、帧率与速度、锁定及退出。相机矩阵采用Float32，精确速度通过观察中心验证，并独立检查眼点同步平移误差不超过1e-6。
- 真实Edge浏览器15组交互通过，证据：`.cache/viewer-mouse-look/1794321811896/`。包括W/S升降与反向、带侧倾的A/D水平移动，以及既有锁定、组合键、鼠标转向、Esc、焦点隔离、拒绝降级和销毁；运行时异常为空。使用实际控制器和浏览器API的示意2D画布，原生Tauri／WebView2及真实Gaussian GPU渲染未在本轮验收。

W/S now follows the full viewing direction, including pitch; A/D remains world-horizontal and roll-independent. Speed, Shift, normalization and pointer-lock ownership are unchanged. All 844 frontend tests, 7 diagnostic tests, the production build and 15 real Edge controller/input cases passed. Native Tauri/WebView2 and Gaussian GPU rendering remain unverified for this change.

### 历史验证记录：2026-11-10 按住 WASD 鼠标转向

以下为 W/S 仍沿水平面移动时的验证记录；当前 W/S 已改为沿视线移动，测试数量及浏览器证据不追溯改写，也不作为该方向调整的新验收结论。

- 前端完整测试833项、诊断协议测试7项及生产构建通过；新增29项控制器测试、5项编辑事件边界测试和1项中英文提示测试。覆盖组合键、Esc、最后增量保存、固定眼位、侧倾／投影偏移、void／Promise接口、拒绝降级、销毁和迟到授权隔离。
- `node scripts/verify-viewer-mouse-look.mjs` 的7组真实Edge浏览器交互通过，证据位于 `.cache/viewer-mouse-look/1794319848370/`。实际使用浏览器Pointer Lock、可信键鼠事件和生产ViewerControls，校验无按钮转向、最后松键、Esc、反向按键固定眼位、投影参数、输入焦点／正交隔离、sandbox拒绝降级及销毁释放；运行时异常为空。
- 浏览器测试仅使用示意2D画布和相机节点，不等同于Gaussian、WebGPU／WebGL2渲染、原生Tauri／WebView2或OS光标外观验收；这些仍未验证。Esc后的浏览器原生短暂锁定冷却仅在测试中等待，产品代码不自动循环申请。
- 新增锁定状态不持久化，不改变漫游速度、调速偏好、相机记录、训练或模型。React事件边界已复核：锁定期间在捕获阶段拦截鼠标编辑，Esc先结束导航而不清空框选；无新增逐帧React状态更新或持续重绘。

The mouse-look implementation verified in this historical run enabled first-person mouse look only while WASD was held in a focused, interactive perspective canvas. Mouse look preserved the eye, roll and projection; movement was horizontal at the existing speed. Last-key release or Escape restored the cursor, and unavailable/denied Pointer Lock fell back to keyboard/drag controls. All 833 frontend tests, 7 diagnostic tests, the production build and 7 real Edge controller/input cases passed. These results predate the current W/S view-direction movement; native Tauri/WebView2 and Gaussian GPU rendering remained unverified.

## 参考相机来源

普通任务从实际传给 Brush 的 `work/brush/dataset/sparse/0` 读取，支持二进制和文本 COLMAP 相机／姿态。按当前固定 Brush 加载器的文件名顺序选择首张有效训练图；OOOSplat 当前未传帧下采样或评估集分割参数，不以随机训练批次定义首帧。参考：[固定版本的加载器](https://github.com/ooolabdev/OOOBrush/blob/866a7d65b4d6a591ba0d52a88f8b20ea3465c1e5/crates/brush-dataset/src/formats/colmap.rs)。

全景任务读取 `work/panorama-training/manifest.json`，按实际时间戳选择最早已注册全景帧的 `front` 面，再读取训练稀疏模型中对应的针孔相机；不使用按名字排在前面的 `back` 面，也不把 ERP 的球面内参用于透视预览。

读取过程只保留相机及当前候选姿态，二进制二维观测通过 seek 跳过，文本观测行不整行分配；不扫描 Gaussian 文件。模型文件、JSON、名称和计数设有上限，错误不会阻止预览。

COLMAP 的 `R,t` 转换为相机中心 `-Rᵀt`、前方 `Rᵀ(0,0,1)`、上方 `Rᵀ(0,-1,0)`，随后应用与 Gaussian 一致的 PLY→引擎映射和编辑 Transform。保存完整朝向（含 roll）、视场、主点偏移和横纵焦距比例；不模拟镜头畸变。窗口宽高比不同，横向可见范围可以不同。

定位不再为三视图单独计算正交缩放，而是统一恢复真实透视相机，包括横纵焦距比例和主点偏移。轨道控制的观察距离仍采用缓存稳健中心的正深度，深度无效时以稳健包围盒半径尺度兜底；该距离只决定观察中心和交互尺度，不改变首帧相机位置、朝向或视场，不在定位时重新扫描 Gaussian。

缺少训练数据、损坏或清理过的项目禁用定位。外部导入模型即使附带 COLMAP，也不能证明 Gaussian 与相机坐标一致，因此禁用定位，不猜测视角或退化成包围盒适配。

## 平移倍率与兼容

右键平移使用 CSS 视口尺寸和当前投影尺度：正交为 `2 × orthoHeight / height`；透视为 `2 × distance × tan(fovY/2) / height`，横向同时考虑焦距比例。正交安全裁剪引起的相机后退不改变倍率，原网格大小和正近裁剪面保持不变。

本机视角继续使用原 version 1 存储键，新字段均可选；旧记录使用默认 roll／投影偏移和 52° FOV。视频捕获同步保留朝向与裁切后的视场，离线 HTML 的 Rust 校验和查看器同步接受新字段。不修改项目 schema、训练数据或配方。

## 按需渲染与侧倾调平（2026-10-25）

- 关闭预览默认的持续渲染，保留引擎输入／更新循环；相机、编辑、选区、操作轴、尺寸变化和排序完成时才请求 GPU 重绘。实际预览绘制以时间预算合并至最高 60 FPS，不减少点数、SH、DPI 或渲染分辨率。
- 刷新请求按预览实例及递增标记保存，避免 PlayCanvas 在 `postrender` 后清除标志时丢失最后一次更新。初始化及视频排序等待使用短期强制帧，成功、取消、超时和销毁时释放；后台强制帧仅用于这些受控等待，不是空闲轮询。
- 裁切轮廓和轨道线在实际 `prerender` 提交，不在被跳过的 update 中积累。轨道几何按状态复用。操作轴的隐藏手柄只隐藏网格并排除拾取，不反复改写引擎的实体朝向可见状态，消除其循环唤醒。
- 动画按实际提交帧之间的时间推进；隐藏期间停止普通绘制及动画计时，返回不补算隐藏时间。帧预算基于提交节奏而非绘制完成时间，避免将 60 Hz 人为降成 30 FPS。视频继续使用明确的时间点、分辨率与 30 FPS 输出，不受普通预览限帧阻挡。
- 相机侧倾只在手动旋转越过现有阈值后归零，采用最短角度的 180 毫秒过渡，同时继续处理拖动。固定视角、定位或其他导航接管时取消旧过渡；松手后尚未完成的调平继续结束并保存最终视角。旧记录和训练相机的侧倾保持可读取，不修改网格或模型。

### 真实生产预览验证

Windows 隔离 Edge 加载生产 React 预览和只读真实 PLY，只有 Tauri IPC 使用官方 mock；不等同于原生 WebView2 人工验收。普通项目为 297,294 点、约 66.9 MiB，全景项目使用已有 Ballintoy 结果，不重建或训练。

普通项目 WebGPU／中文／1280px：原持续渲染基线 500 毫秒约 30 次绘制；优化后默认调整、操作轴开启、球选择、盒选择各稳定观察 5 秒，均为 **0 次重绘**。三视图、动画／视频取消返回及调平后也停止重绘。动画样本 1.2003 秒提交 71 帧，约 59 FPS；真实视频已渲染并编码后取消，未调用发布。定位仍恢复真实相机，位置误差约 `1.55e-7`、方向点积为 1；旋转后相机右方向的 Y 分量由约 `-0.142697` 归零到约 `-6.72e-11`，缩放不变。

已查看 `gizmo.png`、`sphere.png`、`rolled-reference.png` 与 `leveled.png`，操作轴、裁切轮廓及网格均可见。这里的“调平”是世界坐标的相机侧倾归零，不自动将重建模型转成真实地面朝向。证据目录：`.cache/preview-navigation/1792863536045-webgpu-zh/`。

Ballintoy 的 209,467 点、SH3 结果通过 WebGL2／英文／1280px 复核，实际设备为 **RTX 3060 Ti / ANGLE D3D11**。基线 500 毫秒 31 次绘制；默认调整、操作轴、球／盒选择各稳定 5 秒均零重绘，三视图和返回调整也停绘。动画 shader 时钟差值 `1.2271s` 对应实际提交帧墙钟 `1.229s`，67 帧未超过 60 FPS；真实视频编码出帧后取消、零发布。调平后右方向 Y 分量由 `0.0154122` 归零到约 `3.10e-11`；已查看调平和盒选择截图。证据：`.cache/preview-navigation/1792863892279-webgl2-en/`。

额外复跑同一 WebGL2 场景，对比球／盒裁切在按需停绘与连续重绘 500 毫秒后的实际画布像素，两者平均字节差均为 **0**，没有靠漏画缓冲更新实现停绘；完整检查再次通过。证据：`.cache/preview-navigation/1792864088584-webgl2-en/`，包括裁切的按需／连续截图、5 秒静止计数及实际编码后取消记录。该检查比较渲染路径一致性，不将 Ballintoy 小样本身的画质视为本次优化效果。

第一轮真实检查发现操作轴开启时仍在 5 秒绘制 152 次，定位到隐藏手柄与原生朝向更新互相触发后修正；没有放宽空闲零重绘断言。WebGL2 的软件渲染检查曾使用有刷新节流的 HUD 估计动画时间，因 UI 提交滞后失败；已改为同一实际提交帧的 shader 时钟与墙钟比较，而非扩大 HUD 容差。

另一轮软件 GL 在固定等待 800 毫秒后仍收到一次异步刷新，因此不算通过。验证改为明确等待排序收敛且静止一秒，再保留完整 5 秒零重绘断言；最终通过的是上述显卡 WebGL2 路径，不将失败的软件路径声称为已验收。

### 验证边界

新增自动测试覆盖 60／120／144 Hz 刷新、最后一帧、postrender 请求、隐藏恢复、强制帧释放、实例隔离、辅助线停绘，以及正负侧倾、短路径、定位打断、近极点、自由正交和保存恢复。完整 `npm test` 为 **512 项前端 + 6 项诊断测试通过**；`npm run build`、TypeScript 和 `git diff --check` 通过。构建保留既有模块外置、WASM 默认 URL 与大 chunk 警告。本轮未修改 Rust，未重复运行 Rust 测试或 Clippy。

未测独立进程级 GPU 利用率、显存／功耗或峰值内存，不能把重绘归零等同于 GPU 整体占用必然为零；动画和持续操作仍有实际渲染成本。未完成原生桌面、macOS/Linux 及整段视频文件的人工画质验收。没有修改 Rust 接口、项目数据、训练配方、版本号或导出画质。

### English summary

Preview now redraws on demand with a 60 FPS rendering budget, preserving model detail and resolution. Idle auxiliary lines no longer accumulate; hidden gizmo meshes no longer fight native facing updates and repeatedly wake rendering. Ordinary animation pauses while hidden; scoped initialization/video sort waits retain forced frames and release them on every exit. Manual orbit smoothly removes camera roll over 180 ms, without modifying the grid/model or zoom; Locate and saved-view restore still retain the full original pose. Real production WebGPU checks confirmed zero redraws over five idle seconds with gizmos and sphere/box selection enabled, approximately 59 FPS animation, and successful video-frame encoding followed by cancellation. Tauri IPC was mocked; native desktop and non-Windows acceptance remain outstanding.

The real Ballintoy model also passed hardware WebGL2/ANGLE D3D11 on an RTX 3060 Ti, including zero redraws over five stable idle seconds, shader-clock/wall-clock agreement, camera leveling and video encoding followed by cancellation. A slow software-GL run was not accepted; settling now waits for actual sort convergence rather than a fixed delay. All 512 frontend and 6 diagnostic tests and the production build passed. Full-video visual acceptance and per-process GPU utilization/power measurements remain unverified.

To opt into the hardware WebGL2 verification path (test process only, not an application setting):

```powershell
$env:OOOSPLAT_PREVIEW_HARDWARE_GL = '1'
node scripts/verify-preview-navigation.mjs '<project>' '<reference-view.json>' webgl2 en 1280
```

## 历史验证记录（2026-10-19）

下列早期记录中的“三视图保持正交／居中原点／恢复参考缩放”是当时规则，已由当前统一恢复首帧透视相机的行为替代。

- 自动测试覆盖首次打开／恢复视角后连续放大再平移、正交裁剪退让、三视图极点、完整朝向恢复、WASD 方向及对角速度、Shift、失焦／取消／禁用、定位打断、旧记录与 HTML 参数兼容。
- Rust 对真实普通项目读取到 `frame_000002.jpg`，对已有真实全景结果读取到 `erp_0000000001/front.png`。两个项目均只读，没有重新生成或修改源数据。
- 使用真实完整 PLY 和生产 `GaussianViewer` 组件在隔离 Edge 中验证：全景 WebGL2 中文、全景 WebGPU 英文（含 1100px 最小窗口）、普通项目 WebGL2 英文 1280px 窗口。Tauri IPC 使用官方 mock；模型加载、GPU 渲染、相机控制、视角存储和界面布局为真实实现，不能等同于完整原生 WebView2 人工验收。
- 全景相机位置误差约 `2.74e-7`、普通项目约 `1.61e-7`；方向点积约 1。连续滚轮缩放后的实测平移与公式一致；三视图保持正交且原点居中，松键停止，英文窄窗口工具栏没有重叠。
- 证据：`.cache/preview-navigation/*/{proof.json,preview.png,browser.log}`。当前环境的浏览器 GPU 子进程需要沙箱外运行许可；未关闭浏览器自身的安全沙箱。
- 最终自动验证：399 项前端、6 项诊断接收端和 363 项常规 Rust 测试通过，前端构建、Rust 格式检查及 Clippy 通过。8 项需显式资源的 Rust 测试默认忽略，其中本次参考相机真实项目测试已另行执行；另有 2 项既有符号链接测试因当前 Windows 缺少创建符号链接权限而明确排除，未计入通过数。
- 仍需人工检查原生 Windows WebView2、真实鼠标／键盘长时间使用、不同 DPI 和更多模型；macOS／Linux 原生交互未实测。未重新执行生成或训练，也未把这次导航测试表述为训练效果验证。

### 同日降速与按钮调整复核（上一轮，降速已撤销）

- 漫游速度调整为可见画面高度的 15%／秒；自动测试覆盖缩放、FOV、窗口高度以及 30／60／144 FPS 下的速度，Shift 仍为三倍，右键平移倍率不变。
- 在真实全景 PLY 上重新执行生产预览检查：WebGPU 英文 1100px、WebGL2 中文 1280px 均通过。确认“自动定位视角／Auto locate view”为无图标纯文字按钮、工具栏无重叠、缩放后平移符合公式、漫游满足新速度上限、松键停止及正交视图不响应 WASD。
- 证据目录：`.cache/preview-navigation/1792378948112-webgpu-en/` 和 `.cache/preview-navigation/1792378948112-webgl2-zh/`，均包含 `proof.json`、`preview.png` 和 `browser.log`。本次自动回归为 416 项前端、6 项诊断及 365 项常规 Rust 测试，构建、格式检查、Clippy 通过；上述原生桌面及其他平台人工验证限制不变。

### 同日恢复倍率、焦点及参考缩放复核（当前实现）

- 恢复每秒一个可见画面高度的 WASD 倍率，实时读取距离与 FOV；定位过渡同时更新内部距离。修复按键前已排队的旧动画帧使漫游时钟倒退、随后重复计算位移的问题。自动测试覆盖缩放／旋转／恢复／定位后同倍率、30／60／144 FPS、Shift、失焦与输入隔离。
- 有效定位同步聚焦画布，动画期间可直接按 WASD 中断；动画完成不再聚焦，不抢走后来输入框的焦点。正交视图的提示保留但灰显，明确仅透视可用。
- 三视图改用缓存稳健中心，保留参考相机横纵焦距比例。含离群点、180° 坐标映射、旋转／缩放以及大范围网格安全后退的测试通过，后退不改变像素尺度；视角存储继续保留新的正交尺度与焦距比例。
- 真实普通项目 `20261011-234325_toyWithMask` 只读取得首张训练图 `frame_0000000000.png`，参考深度约 `7.99287`，三视图 `orthoHeight` 均约 `4.16745`；1280px WebGL2 中实测每单位约 `107.139984` 像素，与参考平面理论值 `107.139984` 一致。普通项目另通过 WebGPU 检查。
- 真实全景结果在 WebGPU／英文／精确 1100 CSS 像素视口、WebGL2／中文／1280px 下通过；两者均确认按钮点击后无需再点击画布即可漫游、松键停止、原点居中、提示未裁切。该全景 front 参考方向的稳健中心深度约 `−9.85203`，因此使用约定的稳健包围盒尺度兜底，而非声称精确匹配首帧主体占比。
- 最新截图与数值：`.cache/preview-navigation/1792381697060-webgpu-en/`（全景）、`1792381740304-webgl2-zh/`（全景）、`1792381742911-webgl2-en/`（普通）；普通 WebGPU 记录在 `1792381625421-webgpu-zh/`。验证脚本现在通过浏览器协议设置真实 CSS 视口，避免操作系统窗口边框使最小宽度截图被裁切。
- 最终单独执行的 `npm test` 为 427 项前端和 6 项诊断测试通过，构建通过。此前与两个软件渲染窗口并行执行的一轮出现超时及连带失败，渲染退出后的完整重跑通过，未修改测试超时来掩盖。另有 11 项 Rust HTML 导出测试与 1 项真实参考相机测试通过，1 项 HTML 浏览器夹具测试默认忽略；Rust 格式检查及 Clippy 通过。本轮没有修改 Rust 代码，也没有重新生成／训练项目。
- 生产预览和真实 GPU 渲染在隔离 Edge 中执行，Tauri IPC 为官方 mock；保存重开兼容由自动测试验证。原生 Windows WebView2 人工操作及 macOS／Linux 仍未验收。

### 提示精简复核

- 旋转提示统一为“左键 旋转”（矩形选择仍按实际操作显示“中键 旋转”），漫游合并为单项“图标 + WASD + 漫游”，移除展开的 Shift 加速和透视限制文字；正交灰显及实际操作规则不变。
- 427 项前端、6 项诊断测试及构建通过；实际生产预览在精确 1100px 视口下完成中英文图标／按键／功能顺序及无溢出检查。截图与断言证据：`.cache/preview-navigation/1792388890265-webgpu-zh/`、`1792388900006-webgpu-en/`。原生桌面人工验收限制不变。

## 当前定位规则复核：三视图返回首帧透视相机

- 正视／侧视／顶视点击“自动定位视角”，统一退出正交投影并恢复实际首帧训练相机，清除三视图选中；保留既有短时过渡、打断、焦点交还与视角保存。不修改模型或训练数据，历史保存的正交视角仍可按原样打开。
- 430 项前端、6 项诊断测试与构建通过；单元测试覆盖三视图即时／动画定位、坐标映射与模型变换、主点偏移／焦距比例、恢复保存状态，以及定位后直接漫游。
- 普通项目 WebGPU／中文／1100px 及全景项目 WebGL2／英文／1280px 的生产预览检查通过，三个入口相机位置误差分别约 `1.55e-7`、`2.74e-7`，方向点积为 1，FOV 分别恢复为约 `55.07466°` 和 `92°`。均确认定位前正交不响应 WASD，定位后无须点击画布即可漫游。
- 证据：`.cache/preview-navigation/1792394606311-webgpu-zh/`、`1792394624123-webgl2-en/`。首轮浏览器检查在对齐动画尚未结束时测量静止位置而失败，等待对齐结束后重跑通过；没有为此改变相机操作规则。隔离 Edge 的真实渲染不等同于原生 WebView2 人工验收，本轮未重新生成或训练、未执行新的 Rust 回归。

### 复现真实项目检查

```powershell
# 所有输出放在本地验证缓存，项目仅只读。
$env:OOOSPLAT_REFERENCE_PROJECT = '<已完成项目目录>'
$env:OOOSPLAT_REFERENCE_ARTIFACT = 'E:/Project/OOOSplat/.cache/panorama-v1/reference-view.json'
cargo test --manifest-path src-tauri/Cargo.toml --lib actual_training_reference_view -- --ignored --nocapture
node scripts/verify-preview-navigation.mjs '<已完成项目目录>' '.cache/panorama-v1/reference-view.json' webgl2 zh
node scripts/verify-preview-navigation.mjs '<已完成项目目录>' '.cache/panorama-v1/reference-view.json' webgpu en 1280
```

本地 Edge 验证脚本使用独立配置目录，不访问个人浏览器资料，不运行导出或编辑写入。

## English summary

Perspective-only navigation moves W/S along the current view direction, including pitch: W ascends when looking up and descends when looking down, while S reverses that direction. A/D strafe on the world-horizontal XZ plane, following camera yaw independently of pitch/roll, including vertical views. Movement translates the camera and target together without changing orientation. Speed is 15% of one visible height per second, zoom-relative, with Shift ×3. Clicking the canvas or the text-only “Auto locate view” button enables it directly in Adjust; locating returns focus synchronously, never at animation completion. Upper-right hints remain visible but disabled in orthographic views. Queued timestamps cannot move the flight clock backwards or double-count movement. Ordinary projects use their actual first valid Brush dataset image; panorama projects use the earliest registered ERP frame's front training face. Locate always restores that perspective camera, leaving any fixed/free orthographic view and clearing fixed-view selection. The cached robust depth (or bounds fallback) sets only the orbit target/distance, not a substitute orthographic zoom or camera pose. Imported/missing/unverified cameras disable locating. Right-button pan is unchanged. Roll, FOV and projection state remain compatible with saved views, video capture and offline HTML. Earlier horizontal-navigation, speed and orthographic-scale verification records describe superseded behavior, not new validation of this W/S change.

Earlier reference-view verification (before the horizontal-movement change) passed 427 frontend tests, 6 diagnostic tests, build, 11 Rust HTML-export tests, a separate real-project reference-camera test, formatting and Clippy. One optional HTML browser-fixture test remained ignored. A test run concurrent with software-rendered browser checks timed out and cascaded; the full isolated rerun passed without increasing timeouts. Real ordinary/panorama PLYs passed production Edge WebGL2/WebGPU checks for direct post-locate flight and orthographic pixel scale; the panorama sample exercised the invalid-depth fallback. The English narrow-layout check used an exact 1100px CSS viewport. Tauri IPC was mocked; native WebView2 manual and non-Windows platform checks remain outstanding. No generation/training was rerun.

The bottom-right Movement speed slider applies a 0.1×–5.0× multiplier in 0.1× steps, defaulting to 1.0× (the existing 15%-visible-height base speed). Its 2px track retains an 18px interaction height. It stores a device-local preference and is disabled in orthographic views. Completed edits, history actions, saves, explicit numeric Enter and pointer speed adjustment return canvas focus safely. Keyboard range adjustment retains focus until Enter. Pending editing/selection, inputs, dialogs, export, stale operations and old preview instances cannot trigger unwanted navigation or steal focus. See the [control validation record](PREVIEW_CONTROLS_VALIDATION.md).

Historical horizontal-navigation update (2026-10-19): 475 frontend tests (including 58 camera-control tests), 6 diagnostic tests and the production build passed. Coverage includes four ground-plane directions, pitch/roll and exact vertical views, unchanged camera height/orientation, 15% speed, zoom/FOV scaling, 30/60/144 FPS, normalized diagonals, opposing keys, Shift, focus isolation and immediate post-locate navigation. Movement translates the camera/target without recomputing lookAt, avoiding roll jitter at vertical views. This update did not rerun real-model browser/native rendering or generation/training; prior rendering results are not new validation of horizontal movement. Existing browser externalization/WASM URL/chunk-size build warnings remain.
