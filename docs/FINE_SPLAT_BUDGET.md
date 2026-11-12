# 普通素材有效像素预算 / Ordinary-input effective-pixel budget

## 范围与公式

普通图片、普通视频和普通补拍共用以下公式。新任务使用生成策略版本7，快速／均衡／精细的系数分别为0.2／0.4／0.5；精细档公式从版本2起保持不变。

```text
k = Fast: 0.2, Balanced: 0.4, Detailed: 0.5
t_mean = valid observations / points with valid tracks
requested = round(k × sqrt(2462476 × 0.063362106 × X_eff / t_mean))
effective maximum = max(initial Gaussian count, requested)
```

这是先前讨论的 TangoGS 静态预算公式的应用侧改编，不实现其动态增密／剔除控制，也不保证最终点数或画质。本次仅修改普通快速／均衡的上限计算，分辨率、学习率、增密日程、训练步数及自动重试规则不变。全景及含ERP训练面的混合素材保持独立预算，不把六面像素与球面轨迹混算。

`X_eff` 为本次最终模型中已注册的实际训练图片有效像素总数。逐张统计，不抽样；完全透明的像素排除，Alpha 大于零的像素全部计入，包括半透明边缘。快速／均衡新增预算按真正传入 Brush 的加载分辨率及其缩图规则统计；旧精细档版本1预算的统计和序列化保持兼容。普通不透明图片的黑色背景不当作遮蔽，不使用 COLMAP 的 Alpha ≥ 0.5 特征蒙版代替训练有效区域。当前普通训练没有额外独立的 Brush 排除蒙版。

平均轨迹长度来自最终采用的普通 COLMAP 模型。只计入已注册图片中索引有效的轨迹，同一三维点对同一图片的重复引用只计一次；没有有效轨迹的点不参与平均值。只读取图片记录头并跳过2D观测载荷，不重新匹配或重建。

统计在重建后、训练启动前执行，不加入素材快速检测。透明图片逐张只读解码，单幅保持512 MiB安全限制；不透明图片只读取头部。支持8／16位 RGBA／灰度 Alpha、混合尺寸及多相机补拍。支持取消；错误、全零有效像素、无有效轨迹、溢出或缺失文件时明确失败，不猜测预算或静默恢复40倍。

## 保存与兼容

- 沿用可选 `brushTraining.fineSplatBudget` 字段名：保存预算版本、系数、注册数、有效像素数、稀疏点／轨迹／观测数、平均轨迹长度及请求上限。快速／均衡的新预算还记录档位和实际加载分辨率；旧精细档预算不增加空字段，避免改变已有候选身份。
- 失败、取消和中断统一通过“继续生成”保留原策略及已解析参数，不自动升级预算或刷新重试次数。生成策略版本1的普通精细档仍为40倍稀疏点；版本1–6的快速／均衡仍为各自原预算。新建任务统一使用版本7。
- 旧 PLY 初始化超过公式结果时上调至实际初始化点数，不删点。OOM重试仍仅在原授权及可信计数条件下，使用失败进程点数的80%，不得低于初始化数量。
- 不修改项目 schema、云端字段、版本号或已有模型；技术日志仅记录计算摘要，不输出逐图路径、GPS或像素数据。

## English

New ordinary-image/video tasks and ordinary reshoots use generation policy v7 and the formula above, with coefficients 0.2/0.4/0.5 for Fast/Balanced/Detailed. Detailed's v2 formula is unchanged. Only registered training views contribute. Every positive-alpha pixel counts, including semi-transparent edges; fully transparent pixels do not. Fast/Balanced statistics follow the actual Brush load dimensions and resizing rules. Opaque black pixels remain valid. COLMAP's thresholded feature masks are not treated as Brush training masks.

Valid sparse tracks determine mean track length; duplicate references within a track and invalid image/index references do not inflate it. The calculation runs after reconstruction, off the async executor, with sequential bounded image decoding and cancellation. It is not part of input metadata analysis and does not change SfM, training resolution, densification, learning rates, or steps. Panorama and ERP-containing mixed datasets keep their independent budgets.

The optional budget snapshot and resolved preset are saved locally. Continue generation preserves older policies and resolved parameters for failed, cancelled and interrupted tasks; it does not upgrade their recipe or renew retry allowances. New Fast/Balanced snapshots record quality and load resolution, while old Detailed snapshots retain their serialized identity. Initialization is never silently reduced. Authorized OOM retry remains unchanged. This is a maximum, not a trained point count or a PSNR guarantee.

## 验证 / Verification

### 当前版本7 / Current v7

2026-11-10 Windows：Rust 731项通过，36项资源／人工测试默认忽略，3项已知需要Windows符号链接权限的安全夹具未执行；前端741项、诊断协议7项及生产构建通过。`cargo fmt --check`、全目标Clippy（`-D warnings`）和差异空白检查通过。构建仍有既有第三方模块及大分块警告。

覆盖0.2／0.4系数、实际训练尺寸与Alpha缩放、JPEG尺寸、旧精细档序列化身份、旧1–6策略、初始化下限、保存恢复、解析后快照不重算、未知估算不伪造上限，以及抽帧／增密／全景参数不变。未重新训练、重新评测PSNR、原生桌面或硬件OOM验收。

Windows verification: 731 Rust tests passed (36 manual/resource tests ignored; three known Windows symlink-privilege fixtures not run), 741 frontend tests and seven diagnostic tests passed, along with the production build, formatting and strict all-target Clippy. No new training-quality, PSNR or hardware-OOM validation is claimed.

### 历史版本2 / Historical v2

以下为2026-10-27精细档首次接入记录。当时“失败后手动重试升级版本2”的行为已由统一继续流程替代；现在继续生成保留保存的策略与预算。下列测试数量和实测仅代表当时版本，不代表本轮三档配方已重新验收。

2026-10-27 Windows 开发环境：

- Rust 全量测试498项通过，27项资源／人工测试默认忽略。最初沙箱运行有两项符号链接夹具被 Windows 1314 拒绝；获得相应权限后全量复验通过，没有跳过这些安全检查。
- 前端600项测试、7项诊断协议测试、生产构建、Rust 1.99.0 全目标 Clippy（`-D warnings`）及稳定版 rustfmt 检查通过。构建仍有现有第三方模块／大分块警告。
- 单独启用只读真实数据测试：`20261027-085019_images` 的90张2560×3840训练图片，364,993,345个有效像素，66,218个有效轨迹点、191,175条有效观测；平均轨迹长度2.8870548793，公式上限2,220,679。全部训练图片逐张统计，调试构建耗时144.81秒；这不是发布构建性能测量，也不影响素材输入检测。原项目 `state.json` 前后字节一致。
- 覆盖不透明黑色、8／16位 RGBA及灰度 Alpha、半透明像素、混合尺寸／相机、未注册图片排除、无效及重复轨迹、二进制／文本模型、取消、缺失／损坏数据、初始化下限、预算保存与续跑、旧策略、普通补拍共用入口，以及全景／快速／均衡不变。
- JSON浮点读回允许极小的舍入差异；上限始终从整数像素及观测计数复核，不能因为平均轨迹长度差一个浮点 ULP 就拒绝恢复。

没有重新训练、重新计算PSNR或改写历史项目；不将只读公式核算视为画质验收。未执行原生桌面交互验收及 macOS／Linux 端到端训练验证。No new training or PSNR claim is made.
