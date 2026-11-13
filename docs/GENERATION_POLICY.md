# 默认生成策略与自动重试 / Generation defaults and automatic retry

本说明对应当前源码的新策略，不代表旧安装包或已存在的中断快照已经改变。

## SfM 与 0.7.1 对齐及初始化保护（2026-11-11）

- 基线为 `0.7.1`（`663de39a76b9672f33449922ccaaa823002ea1ea`）。普通单来源恢复原生 `exhaustive_matcher`／`sequential_matcher`；顺序匹配沿用窗口10、原生指数邻距及每10张查询的闭环规则。多来源批内采用相同候选规则，跨来源仍使用已有有界检索，不把不同视频当作连续序列；不承诺显式图片对与原生命令的RANSAC执行顺序逐字节相同。
- 普通三档 SfM 长边／特征上限为 `1200/4096`、`1600/8192`、`3200/16384`；ERP 的特征长边恢复为不超过 `2048/3072/4096`，Bridge 共用同一规则，不放大小图。训练分辨率、增密和Splat预算不变。
- 保留独立的 Alpha 预乘 RGB 与8位灰度 Alpha Mask，原始RGBA供训练。它有意不同于0.7.1直接使用RGBA及二值Mask，因此不承诺特征或最终模型逐字节相同。已完成且验证有效的旧资产不强制重算；新旧任务仍保留自己的相机分组、配方、增量约束、GPS先验及已消耗补救次数。
- 未改变像素坐标的普通PNG派生图同时原样保留`eXIf`与标准XMP `iTXt`相机元数据，防止正常焦距先验因RGB重编码而丢失；旋转规范化后不套用原坐标标定。按文件span使用64KiB固定缓冲校验CRC并在一次RGB编码中注入，不限制元数据总大小、不解压XMP、不复制ICC或任意文本、不重读IDAT像素载荷。原图和Mask不改。临时PNG分数补丁仍限定已确认的运行库和字段优先级，不因此变成一般焦距推断系统。
- 普通冷启动 Mapper 只有同时出现低于80%注册率和异常内参时，才进行应用侧有界初始化恢复。最多三个首图候选由已验证匹配图确定：先验／总对应数优先，然后贪心覆盖不同闭邻域，以稳定ID处理并列；不硬编码素材、相机、焦距或图片编号。每次只固定首图和随机种子，不固定第二张，不限制全程BA线程，不放宽内参或几何安全阈值。
- 只接纳内参有效且注册更多的模型，原始和替代结果均保留；仍无有效改进则停止，不把异常小模型送入Brush。这是对已确认失败模式的应用侧兼容处理，并非重编修复COLMAP底层并发实现，也不保证所有低注册任务都会提高注册率。
- 已有输入模型、固定相机／位姿、显式起点、ERP和GPS位置先验执行不改成冷启动；普通全量补拍的`fix_existing_frames=0`不误判为固定约束。取消、进程崩溃、OOM、启动和I/O错误保持原分类，不伪装成几何失败。Caspar一旦回退Ceres，后续起点继续保持Ceres。

The shared SfM implementation restores the 0.7.1 native single-input matchers and candidate rules, while retaining multi-input/reshoot/GPS constraints and saved valid caches. Ordinary cold Mapper results with both low registration and invalid intrinsics can check at most three graph-diverse deterministic first images. This application-side compatibility path preserves geometric thresholds, camera sharing, BA parallelism and all previous outputs; it is not a rebuilt COLMAP fix or a guarantee of full registration. Premultiplied RGB and grayscale Alpha masks remain intentional differences from the old pixel preparation. Standard EXIF/XMP camera chunks are preserved byte-for-byte when the pixel coordinate map is unchanged, using fixed-memory streaming rather than extending focal inference or rewriting source metadata.

验证：隔离复用原90张快速档失败数据库及真实3图坏模型，由匹配图自然选出候选，在2次Mapper执行、约72.6秒后恢复为90/90；零特征重提取、零重新匹配、零Brush，原数据库及模型摘要不变。另使用相同素材的快速档预乘RGB／灰度Mask特征，在新数据库重做原生穷举匹配和共享Mapper，约117.7秒得到90/90，抽查源图片／Mask及原数据库摘要不变。首版仅取前三个邻近seed的实测失败已保留，因此改用图覆盖候选而非把测试素材编号写入生产代码。这里只验证SfM，不冒充新Brush训练、PSNR或其他平台原生验收。

最终自动验证：Rust 982通过、43项资源／人工测试默认忽略；3项Windows符号链接安全测试在获准权限下复测并进入完整通过结果。前端972通过、诊断协议7通过；前端构建、`cargo fmt --check`、全目标Clippy `-D warnings`通过。默认带完整调试信息的首次编译因磁盘不足失败，仅清理本次生成的失败对象／working目录，再以关闭增量编译和仅本包关闭调试信息的命令行配置验证；未修改Cargo配置、应用版本或引擎二进制，未清理已有缓存／素材。

## 默认配置

原自动优化的重建、闭环和自动补帧成为默认行为；“自动重试”只授权改变预期方案的重试，默认开启。关闭不会关闭补帧或后端兼容回退。

| 档位 | 默认保留帧率 | 普通素材训练 | 全景 ERP／六面 |
|---|---:|---|---|
| 快速 | 6 FPS | 现有优化配方，最大长边1600 | 2048／512上限，不放大 |
| 均衡 | 8 FPS | 现有优化配方，最大长边1920 | 3072／768上限，不放大 |
| 精细 | 12 FPS | 原始分辨率 | 原始 ERP，六面边长 `floor(宽度/4)` |

策略版本6的视频滑轨显示并选择抽帧后的帧率。最低值为 `min(源帧率, max(1 FPS, 30 / 视频时长))`，最高值为源帧率并保留全部帧；界面中的帧率、首次保留比例和范围均来自正式帧规划结果。内部仍按每秒去除帧数保存，保留全部帧对应 `frameRemovalFps = 0`。快速／均衡／精细默认分别保留6／8／12 FPS，并受上述动态范围约束；因此正常情况下精细档保留的画面最多。设置按素材ID分别保存；切换质量重置全部视频，恢复默认只重置当前视频。每段仍使用独立时间轴，源帧不足30帧时全部采用。即使容器时长与帧数存在小误差，30 FPS素材的最高端仍严格表示全部源帧、30 FPS和100%。已创建或续跑的版本5任务继续使用0.1 FPS去除量下限和原范围；尚未提交的版本5 GUI草稿迁移至版本6。旧版本4的全局 `frameSamplingFps` 继续解释为目标保留帧率。历史快照不改写，图片和COLMAP直接训练不显示该设置。

策略版本3中，仅手动抽帧改为总量最多1.2倍：首次实际抽取`N`帧、源视频可用`T`帧时，最多新增`min(N / 5 向下取整, T − N)`帧。100%不新增。当时的默认档位仍使用9／12／15 FPS候选预算。初始注册率低于80%、存在有效初始模型时，按未注册区间优先级从源帧范围按需选取，不再受旧候选池密度限制；不提前枚举或抽取全部源帧。只尝试一轮，保存预算及选择，续跑不重复扩大。

候选范围与允许新增量分开记录，不代表全部抽取。策略版本5起，用户手动修改某段视频的设置后，该段沿用20%规则，按首次实际帧数冻结，不跨段挪用；未修改的默认配置仍走正常Bridge路径。界面显示的保留比例仅描述首次计划，不包含可能发生的Bridge补帧。整个多素材任务至多一轮联合Bridge，恢复不增加预算；统一补拍仅为新增视频来源执行同一Bridge，不修改旧画面或转移预算。视频保存真实PTS索引并复用，不按平均FPS伪造时间。GUI、CLI和MCP的新任务默认使用版本7，抽帧规则与版本6相同；显式旧全局FPS继续按兼容目标帧率解释。旧任务中断续跑保持保存策略。1.2倍仅表示帧数预算，不保证耗时或成功率。[旧单素材验证](MANUAL_BRIDGE_V3.md)及新输入说明。

精细档不按显存分档：普通30000步，阈值0.0015、选择比例0.15、截止24000；全景45000步，阈值0.001、比例0.30、截止38000；Refine均为200步。策略版本2的普通素材采用[有效像素预算](FINE_SPLAT_BUDGET.md)：`round(0.5 × sqrt(2462476 × 0.063362106 × X_eff / meanTrackLength))`；仅统计已注册训练图片，扣除 Alpha 为零的像素，半透明像素仍计入。全景继续使用最终有效球面 COLMAP 稀疏点数×40，不乘六，不保留原1000万硬上限。旧 PLY 初始化超过预算时仅提高至初始化点数，不丢点。上限不是增密目标；数量溢出和引擎数值范围仍校验。旧版本任务续跑保持原快照及原预算。

策略版本7将同一公式扩展到普通素材快速／均衡档，系数分别为0.2／0.4；普通精细档仍为0.5。快速／均衡按实际 Brush 加载尺寸统计有效像素，Alpha 大于零才计入；不改变训练分辨率、步数、学习率或增密参数。全景及含ERP训练面的混合素材保持独立预算。版本1–6的快速／均衡任务及已保存的实际训练配置继续按原快照运行，不因继续生成升级预算。

原分辨率仅针对训练，不取消 SfM 特征尺寸限制。保留512 MiB单图解码等安全保护；无法处理时失败，不暗中缩小。

所有档位仅在实际图片具有非不透明像素时启用两视图三角化，透明视频抽帧也适用；全不透明的 RGBA 不启用。

## BA 后端与素材提示

新任务保存独立的可选 `baPolicyVersion=1`，不改变抽帧策略版本或训练配方。普通生成、兼容的普通补拍及 Bridge 共用选择器；仅支持经验证的 `PINHOLE`／`SIMPLE_RADIAL` 相机及约束，且关键点和几何验证匹配总数均达到保存阈值（默认各200万）、实际GPU探测通过时，使用局部Ceres／全局Caspar。ERP／含ERP混合、GPS位置先验、未知或不兼容相机及传感器约束均使用Ceres，不丢弃观测或先验。GPS仅辅助匹配而实际使用普通Mapper时可以进入同一选择。

旧记录缺字段保持旧适用范围，创建前失败的旧任务也不注入新范围；有效重建、训练候选及Commit不因新增字段失效。Caspar兼容回退保留所有原约束，只重置本次Mapper输出；GPS数据库／先验、视觉模型和补拍原模型不清除。实际后端及原因写日志，SIFT使用GPU不等于BA使用GPU。

所有任务的素材统计不再独立展示，素材信息保留在输入框。Alpha仅按每项保存的通道信息显示“检测到 Alpha 通道”，不等于有透明像素或已生成Mask；历史缺失信息不重新扫描。

New tasks freeze an optional, independent BA scope version. Compatible ordinary/reshoot/Bridge mapping uses local Ceres and global Caspar only after data-size, GPU, camera and constraint checks. ERP and GPS position priors retain Ceres without dropping observations or priors. Older tasks retain their scope; caches and publication recovery are not invalidated by the added field. Alpha hints now belong to each input and report channel presence only, not transparent pixels or mask creation.

本轮自动／原生验证及未完成项目见验证记录。

## 自动重试与兼容

- 明确 GPU OOM 后，保存的执行链最多自动重试一次，上限为当前失败进程最后一次可信点数的 `floor(N×4/5)`。不使用配置上限或历史进程点数猜测；再次点击继续生成不重新获得额度。
- 没有计数、无法降低、或新上限低于初始化点数时不重试。分辨率、训练步数、学习率及增密参数不变，从原初始化重新训练，不恢复旧优化器。
- 补拍的全量重建／重新训练回退也受此开关约束，保留明确原因和各类一次的原限制。OOM不触发算法回退；取消、崩溃、DeviceLost、资源限制和文件错误不冒充 OOM。
- 完成详情仅在实际执行了受控重试时显示“已自动重试”；只有勾选或发生 Caspar→Ceres 兼容回退不显示。
- 草稿保存独立重试开关和比例；任务保存策略、抽帧、真实参数和重试消耗。失败、取消和中断统一为“继续生成”：新增 `continue` 运行记录，但保留原策略、实际路线和配方、OOM降低后的上限及各项已消耗次数，不恢复初始方案、不自动升级。旧 `resume`／`retry` 历史不改写。
- 继续生成先恢复发布事务，再校验成果；有效候选只需校验发布。未完成训练从本次初始化重新启动，不恢复优化器。剩余补救次数为零仍可主动继续未完成步骤，但失败后不再自动降点或换方案。缺失／损坏只失效相关成果及下游，权限、磁盘和数据库占用保持真实错误；已发布模型和编辑成果不归档或覆盖。
- 工作单继续保存在现有 `state.json`，不新增 Ticket 文件。首次、补拍、导入重建和继续生成共用公共处理流程，根据有效成果决定下一步；补拍只增加源快照、增量约束和新增素材成功条件。
- 点击继续生成关闭本机自动队列，只执行当前任务。卡片和详情采用相同准入规则；纯发布恢复不要求全套引擎或原素材存在，真正需要外部素材时仍执行权限及身份检查。
- 旧任务不因缺少策略版本单独被拒绝；只有实际配方、初始化和坐标可证明时恢复，否则安全停止。新任务Alpha解释不再按补拍身份硬编码，旧任务保留原训练选项；自动定位按捆绑Brush参考视场作针孔近似，不改训练相机。
- GUI、MCP `generation_policy` 和 CLI `--auto-retry false` / `--frame-sampling-ratio 0.5` 使用同一策略。未提供新选项的新任务使用默认值；旧接口显式禁止补拍回退时仍尊重该拒绝，旧 `plannerEnabled` 仅解释历史行为。MCP `resume_task` 接受失败、取消和中断；`start_task.retry_failed_run_id`保留为相同继续行为的兼容入口。旧运行标识不能重复启动新一轮，重启软件不会自动继续。

## English

Previous optimization behavior is standard, independently of Automatic retry. Policy v6 displays and selects the post-sampling frame rate. Its lower bound is `min(source FPS, max(1 FPS, 30 / duration))`; its upper bound is the source frame rate and keeps every frame. The displayed rate, initial retention percentage and endpoints come from the production frame planner. Values are still stored internally as frames removed per second, with `frameRemovalFps = 0` meaning keep all frames. Fast/Balanced/Detailed retain 6/8/12 FPS by default, clamped to this dynamic range, so Detailed normally keeps the most frames. Even when container duration and frame count differ slightly, the upper endpoint for a 30 FPS source means every source frame, exactly 30 FPS and 100% retention. Created or resumed v5 runs keep their 0.1-FPS removal floor and original range; unsubmitted v5 GUI drafts migrate to v6. Saved v4 `frameSamplingFps` remains a retained-target FPS and is not reinterpreted. Images and direct COLMAP training do not show this setting.

Policy v3 changes only manual sampling: for `N` initially selected frames from `T` source frames, add at most `min(floor(N/5), T-N)`. The initial indices remain unchanged. Default sampling retains the 9/12/15 FPS candidate pools. Below 80% initial registration with a valid initial model, one bridge attempt searches unregistered intervals lazily across source indices, independently of the old pool. It does not extract all candidates, and resume cannot renew the frozen budget. Panorama bridges reuse the bounded real PTS index already probed during generation, including VFR and nonzero start times. Candidate availability is not the extraction budget or the number of training views.

New admission uses v7 with the same frame rules as v6; continuing failed, cancelled or interrupted work preserves its saved version and snapshot. A manually changed rate freezes at most 20% extra frames for that clip, with one joint Bridge attempt and no cross-clip transfer. The retention percentage shown before generation describes the initial plan only. Unmodified defaults retain the normal Bridge path. The 1.2× ceiling controls frame counts, not time or success. Legacy global FPS/ratio fields keep their prior semantics. See the unified input boundaries and validation; the previous [v3 native record](MANUAL_BRIDGE_V3.md) remains historical.

Detailed uses source-resolution training, including native ERP and width/4 cubemap faces at 92°. SfM feature limits and the 512 MiB image decode guard remain. Policy version 2 uses the [effective-pixel budget](FINE_SPLAT_BUDGET.md) for ordinary media: `round(0.5 × sqrt(2462476 × 0.063362106 × X_eff / meanTrackLength))`. Only registered training views contribute; zero-alpha pixels are excluded, while positive semi-transparent pixels count. Panoramas retain 40× the final valid spherical sparse point count, without VRAM tiers or the old 10M cap. A larger trusted PLY initialization raises the effective ceiling without discarding points. Saved older runs retain their original policy. This is a ceiling, not a promised final count.

Policy v7 extends the same ordinary-media formula to Fast and Balanced with coefficients 0.2 and 0.4; Detailed remains at 0.5. Fast/Balanced count positive-alpha pixels at the actual Brush load dimensions without changing resolution, steps, learning rates or densification. Panorama and ERP-containing mixed datasets retain their separate budgets. Existing v1–6 Fast/Balanced tasks and saved resolved recipes are not upgraded by Continue generation.

Only verified GPU OOM may retry once at 80% of the failed process's last trustworthy reported count. Missing counts, ineffective reductions, or ceilings below initialization fail without guessing. Other training settings remain unchanged. Reshoot algorithm fallbacks also require this switch; compatibility fallbacks and backfill remain standard. Continuing preserves consumed allowances instead of renewing them. Completed details mark actual controlled retries, not permission alone.

Failed, cancelled and interrupted tasks use one Continue generation flow and a new `continue` run record, preserving the saved policy, actual route, training parameters, OOM-lowered cap and consumed remedy allowances. Historical `resume`/`retry` records are unchanged. Zero remaining allowances do not prevent an explicit continuation of unfinished work; they prevent further automatic quality/route changes. Valid candidates recover publication without training again, and incomplete training restarts from its recorded initialization rather than restoring optimizer state. Restarting the app does not automatically continue a task.

The work order remains in the existing `state.json`, not a new Ticket file. First generation, reshoots, imported reconstruction and continuation share the same artifact-driven processing flow. Invalid caches are archived behind a durable reuse barrier; operational I/O errors are not treated as cache corruption. Source files, published models and edits remain protected. All preset tiers enable two-view triangulation only for actual transparent training images, including video frames. MCP `resume_task` accepts failed/cancelled/interrupted tasks; `start_task.retry_failed_run_id` remains a compatibility alias with the same preservation rules.

Explicit continuation turns off the local automatic queue and runs only that task. Cards and details use the same admission rules. Proven publication-only recovery does not require all engines or the original media; any actual external-media access still checks permission and identity. Missing policy versions alone do not reject old work, but incomplete recipe/initialization/coordinate evidence does. New tasks share the runtime-default Alpha interpretation; saved old options remain unchanged. Auto locate uses a pinhole approximation of the bundled Brush reference FOV rather than changing training intrinsics. Unified reshoot Bridge applies only to newly added videos, with the same per-source frozen budgets.

## 验证记录 / Verification

2026-11-10 普通快速／均衡公式预算（策略版本7）：Rust731项、前端741项、诊断协议7项通过，构建、格式检查、差异空白检查与全目标Clippy（`-D warnings`）通过。Rust另有36项资源／人工测试默认忽略，3项已知Windows符号链接权限夹具未执行；既有第三方构建警告保留。覆盖系数、实际Brush尺寸、Alpha、旧快照身份、初始化保护、恢复及抽帧／增密／全景参数不变。未重新训练或评测PSNR，不将预算计算视为实际点数、画质或显存验收。Fast/Balanced v7 passed automated regression and build checks; no new training or PSNR claim is made. See [budget verification](FINE_SPLAT_BUDGET.md#验证--verification).

2026-11-09 统一继续入口前端回归：失败、取消、中断的项目行与共享详情使用同一操作；不依赖详情读取成功才显示入口；无项目记录可由后台验证原请求后继续；运行标识、历史继承配置及中英文按钮已覆盖。命令、结果及未验收项见统一继续验证记录，不以界面自动测试代替完整训练、硬件OOM或原生桌面恢复验收。

2026-10-28 手动补帧版本3验证见[独立记录](MANUAL_BRIDGE_V3.md)。下方版本1／2记录保留为历史，不代表重新执行完整训练。

2026-10-27 普通精细档预算更新为策略版本2、系数0.5；全景保持原策略。498项 Rust、600项前端及7项诊断测试、构建、Clippy与格式检查通过，真实90张图片只读核算得到2,220,679上限。[实现与验证详情](FINE_SPLAT_BUDGET.md)。以下记录保留为此前默认策略实现的历史验证，不代表重新完成了训练画质测试。

自动测试与原生硬件验收分开记录。本文不代表原生桌面、高分辨率完整训练或硬件 OOM 已验收；不通过故意耗尽显存来制造成功案例。

2026-10-27 Windows 开发环境：

- 前端：600 项 Vitest、6 项诊断服务测试及生产构建通过。覆盖比例滑轨、精度与重置、缓存元数据重规划、重试开关不重检测、队列、补拍问答、历史配置和真实重试标记。
- Rust：479 项单元／接入测试通过，26 项原生或大模型手动测试保持忽略。符号链接安全夹具在沙箱外的相应权限下验证；不跳过安全检查冒充全部通过。
- Rust 1.99.0 全目标 Clippy（`-D warnings`）、本机 Rust 1.97.0 格式检查和差异空白检查通过；1.99.0 工具链未安装 rustfmt，因此未用该版本执行格式检查。测试进程使用临时 `TAURI_CONFIG={"bundle":{"resources":[]}}` 避免开发客户端占用 DLL；正式资源及打包配置未改动。
- 新增覆盖：三档普通／全景预算、显存无关精细档、原始 ERP 尺寸／大于1024的六面配置、点数溢出和初始化保护、80%点数及单次重试、真实 Brush 报告识别、Alpha 与不透明 RGBA 区分、缓存归档失败的持久化屏障、结果保护和手动重试运行隔离。
- 前端构建仍输出第三方模块／大分块等既有警告；不属于构建失败。原生桌面布局、真实高分辨率训练、硬件显存不足后的完整重试，以及 macOS／Linux 端到端验收未在本次执行，不将单测视作这些验收的替代。
