# 生命周期扩展发现记录

首次生命周期扩展按授权仅修改测试适配器及 `e2e` 隔离的观测和屏障。B1 至 B4 的后续修复和独立验收见各节与 [VERIFICATION.md](VERIFICATION.md)。严格用例保留原断言，Playwright 使用单 worker 和零失败重跑。历史完整轮次、空间阻断和定向验收分别记录。

## B1：TaskService 丢失已保存生成策略

最小用例：`durable task policy snapshot > completed task records retain the independently saved generation policy`。普通 30 张图片、Fast、默认自动补救，完成后独立期望 `{version:7, autoRetryEnabled:true, frameSamplingRatio:null, frameSamplingFps:null}`。项目 state 和 task.request_parameters 保留策略，task.generation_policy 为 null。

证据：开发轮 `20261117114932336-efd08477-30de-42c2-893f-694085c6ccf2`；隔离目录 `.cache/e2e/runs/3-129d920d-b99e-42ca-8ccd-e5996b53a04a`；taskId `b3d95fe2-4c40-4a48-9eaa-7c57d70e8cd9`，末次 runId `b7367cf2-2541-4ff2-be7a-b37d2f9f977b`。查看边界快照、app-data/tasks.json 和项目 state.json 可交叉核对。

代码证据：`tasks::checkpoint_snapshot` 未输出 generation_policy；`on_runner_update` 的 Configuration 分支从该快照读取缺失字段并赋值 None。保存请求与项目状态未同步丢失，不应把它描述成所有配方字段丢失。影响为任务持久化记录与原策略不一致，不能证明继续记录完整保持原策略。主矩阵在执行恢复、引擎次数、缓存及事务断言后保留此严格检查；提前失败的其他问题会单独记录。

2026-11-18 修复：检查点配置快照现在输出保存的 `state.generation_policy`；没有策略时明确输出 null，保留旧项目的旧行为。没有在消费者中使用默认值兜底，不修改策略版本、采样、补救额度、恢复流程或历史任务文件。新增 Rust 回归覆盖默认、关闭补救、手动比例、手动 FPS、v5 和历史无策略，并经过真实 Configuration 回调、任务落盘、重新加载；另验证旧 runId 的配置回调不能覆盖当前记录。原有最小原生用例增加磁盘记录、重启后策略、运行与训练调用不增加的断言。

首次实施状态：实现及回归用例已写入，TypeScript／格式／文档／差异检查结果见独立验证记录；普通／E2E Rust、两种 Clippy 及六个定向原生案例当时因 E 盘约 6.85 GiB 低于 8 GiB 门槛而未启动。该次受阻证据保留于 `.cache/e2e/lifecycle-validation/20261118012000749-policy-fix/`，原始复现及两轮完整报告保持不变。

后续补测：磁盘空间恢复至约 21.4 GiB 后，普通／E2E 特性 TaskService 回归各 79 passed、0 failed、1 既有 ignored，两种全目标 Clippy `-D warnings` 通过。原生运行 `20261118013132010-c8b7f281-308b-47b1-8fdf-a10ddae41cc7` 为 6 passed、0 failed／skipped／flaky，单 worker、retries=0，约 59.9 秒。七个实际任务在内存策略、actual_configuration、request_parameters、tasks.json 与项目 state 中均与独立期望一致；补拍祖先摘要不变。关闭补救的 OOM 案例按预期 Failed，策略始终为 false，未新增自动补救尝试。训练中断后重新训练；发布／可信候选恢复及完成后重启不重复训练。证据目录 `.cache/e2e/lifecycle-validation/20261118012907935-policy-fix-resumed/`。该轮仅覆盖六个定向案例，145 个历史失败案例的后续复验见验证记录。

## B2：冻结祖先准备后继续发生 OS5

复现：A 完成；B 新增两段视频与透明图片，在真实 `sourcePrepared` checkpoint/sourceId=`__source` 中断，再继续。开发轮三种方式（取消、取消重启、强杀重启）均到达继续运行后 Failed。取消方式证据目录 `.cache/e2e/runs/30-761757ea-4929-4789-9184-4e899a891562`，taskId `ecb11a43-49f5-4c88-9697-bd208e635f96`，继续 runId `d86d2c0a-5a57-405d-9be6-7f48a0bc5a29`；另两方式目录 `31-df9649de-4c99-47d8-886c-bdf749983786`、`32-828327fe-6ed0-47b6-b7a3-e5a192bdb594`。

任务错误：pipeline_failed / extractingFrames / ffmpeg，文件读写失败、拒绝访问（OS5）；retry-preparation 日志已完成恢复状态、工作数据库检查及 reuse barrier。state.stage=failed，inputs.prepared=false。最初只定位到原项目准备链路；后续逐例复查确认 13 个主矩阵失败和一个反复中断复合案例，真实 activity 均停在“正在准备原项目文件：0/30”，不能据 engine 标签断言真实 FFmpeg 失败。

主矩阵改为明确绑定本轮新增 first.mp4；上述内部祖先停点保留为三个额外严格回归。没有用新增来源的成功替换祖先停点失败，也未修改冻结快照或恢复语义。

后续根因：冻结快照文件本来应当只读，但 `inputs_runner` 对继承 Mask 使用 `tokio::fs::copy`，连只读属性一起复制到工作目录。联合 prepared 清单尚未可信时中断，现有恢复规则清除来源完成记录并重新准备；首次复制留下的工作 Mask 仍只读，第二次覆盖便触发 Windows OS5。失败现场的 30 个继承 Mask 均只读；最小现场中第一张 frames／colmap_frames 已被新运行完整重写，Mask 仍保留旧时间和只读属性。

2026-11-18 修复：仅该联合补拍路径改用私有 `copy_frozen_reshoot_mask`。校验当前项目 masks 目标和父路径，拒绝越界、非普通目标、符号链接／重解析点；先解除工作文件项，再复用既有可取消字节复制器生成可写副本。旧只读目标通过固定工具链的标准 unlink 处理，不清除源属性、不改 ACL、不添加重试；真实共享冲突原样报错。冻结快照、祖先文件和通用复制／原子保存实现保持不变，取消后成果仍按现有未完成规则恢复。

新增七项 Rust 回归覆盖可写副本与重复复制、旧只读目标、只读硬链接保护、预取消、越界／目录／父文件目标、符号链接／重解析点及真实共享冲突。普通与 E2E 特性相关回归均为 131 passed、0 failed、2 既有 ignored。原生沿用原有 13 个失败停点、1 个反复中断案例和 6 个 B1 回归，新增核对冻结 Mask 摘要、内容、工作副本写权限与快照只读属性。最终原生及 Clippy 结果见 VERIFICATION.md；独立证据目录 `.cache/e2e/lifecycle-validation/20261118022200567-mask-copy-fix/`，历史失败证据不变。

最终状态：B2 已通过本次定向验收。原生 `20261118023455616-fddde331-9699-4abc-a616-fc17e300ffb1`：20 passed、0 failed／skipped／flaky，单 worker、retries=0，约 4.7 分钟；包括历史受阻的反复中断案例在四次运行后完成。450 份 Mask 的内容及读写属性正确，15 份祖先摘要不变，35 个任务策略一致；两种全目标 Clippy 通过。没有降低断言或修改完成标记，没有修复 B3／B4，没有把该结果当作 268 例或三轮全量通过。

## B3：并发替换期间 JSON 路径短暂无法打开

第一次普通 Rust 全量测试中，原有 `project::manager::tests::concurrent_readers_never_see_partial_or_missing_json_during_5000_saves` 的 reader 记录 `NotFound`（OS2），1122 passed / 1 failed / 48 ignored。日志：`.cache/e2e/lifecycle-validation/20261117114719681-checks/rust-ordinary.log`。同次 E2E-feature 库测试的此项通过。

生产原子保存器与此压力测试未修改；没有调整断言、保存次数、重试或读者行为。该结果证明本次观察到读失败，不证明每次都可复现，也不能用后续通过擦除它。最终固定代码全量检查另列结果。

写入作用域修正期间的 E2E 特性完整回归又出现此失败：1134 passed / 1 failed / 48 ignored，读者捕获 10 次 OS2 NotFound；`rust-scoped-e2e.log`。最终新增运行上下文观测后的全目标回归为 1136 passed / 48 ignored，但并未修复保存器，两个失败证据均保留。

2026-11-20 定位：在隔离目录用 Rust 1.100 和 Tokio 1.53.1 保留同目录临时文件、flush、sync、关句柄和单写者替换流程，把读路径拆成 `File::open`、读取和解析。按 Rust 标准库的分支顺序调用 `MoveFileExW` 和 `FileRenameInfoEx`，75,000 次写入全部成功，其中一次 OS 2 发生在成功的 `MoveFileExW` 替换期间。单独调用 `FileRenameInfoEx` 的 75,000 次写入也全部成功，其中三次 OS 2 均发生在同一次成功的替换调用期间。错误都来自并发 `File::open`。成功打开的文件都能完整解析，最终版本与文件数符合预期。使用标准库 `rename` 的同轮持读对照完成了 75,000 次替换，期间读者全部成功。两个 Windows 替换入口均已观察到短暂的路径打开失败，历史报错具体走了哪个分支、Windows 内部如何产生这个窗口仍缺少现场记录。源码与日志保存在 `.cache/e2e/lifecycle-validation/20261120-b3-b4-diagnosis/`。

2026-11-20 修复。任务注册、项目元数据与状态、项目索引和发布事务的应用读者使用统一的打开逻辑。Windows 仅对打开时的 OS 2 加入 5、20、75、200 ms 的等待，最多尝试五次。成功打开后通过该句柄读取，其他 I/O 和解析错误沿用调用方处理，等待耗尽后返回原始错误。应用继续使用同目录临时文件和原子替换写入。5,000 次保存的生产读写压力测试保留完整记录断言，另有确定性用例覆盖恢复、耗尽和其他错误直返。直接读取磁盘文件的外部读者仍可能观察到诊断中复现的短暂路径缺失。

## B4：未中断的第四轮 Warm 补拍发生 OS5

最终第 2 轮 `20261117151825888-be766c08-ac52-4faa-b204-27377feef0d6` 中，`repeated warm-chain` 的 A、B、C 均完成，D 在准备 C 的文件时失败。相同代码的第 1 轮该链通过；没有对失败用例重跑。场景没有 faults，屏障命中列表为空，因此不是预声明的故障注入或取消结果。

证据目录 `.cache/e2e/runs/143-4c4f33bd-812d-49ad-bf85-b69fabb3169f`。D taskId `ef9fb7aa-b6ae-4286-a81a-84db9f60d7a6`、runId `19343542-6cb5-4b9a-a363-3dc6a201ea2d`；来源为当前 C 项目 `f37e1fe8-0b64-48ac-82c1-884d1cfdce60`，对应 C taskId `a3fc8672-4b41-4c36-9839-6bb4346c5b4b`。真实 activity 最后记录“正在准备原项目文件：277/277”，随后 pipeline_failed / extractingFrames / OS5。D 尚未开始训练。

2026-11-20 定位：暂存目录保留了完整 `snapshot.json`，277 项资产的长度和 SHA-256 全部匹配，初始化 PLY 摘要与 C 的最终 PLY 一致。正式目录 `reshoot-snapshot-v3` 缺失，`snapshotV3Ready=false`，祖先摘要全部一致。按 `ReshootSnapshotBuilder` 的调用顺序，失败点定位到暂存目录改名为正式目录时的 `tokio::fs::rename`。在隔离目录持有一个子文件的普通读句柄，即使包含 `FILE_SHARE_DELETE`，同样返回 OS 5，保留暂存目录和清单，关闭句柄后改名成功。复现确认子文件句柄足以触发 B4 的失败形态，历史运行中具体哪个文件被哪个进程占用仍缺少句柄记录。B2 发生在重建工作 Mask 时覆盖旧的只读文件，B4 发生在首次发布整份冻结快照时。审计和复现日志位于 `.cache/e2e/lifecycle-validation/20261120-b3-b4-diagnosis/`。

2026-11-20 修复。Windows 发布使用同卷且禁止覆盖的目录改名。OS 5、32、33 的等待限于已写完清单的暂存目录，依次为 50、150、350 ms。等待前后检查取消和目标状态，已出现的正式快照按父项目、坐标系及内容身份核对。持续失败保留暂存证据，并记录 `publish_reshoot_snapshot`、尝试次数和原始 OS 错误。改名成功后完成只读封存，再由原流程写入 `snapshotV3Ready`。Rust 回归用真实打开的子文件复现冲突，验证释放后发布、持续占用、取消、并发目标及改名后封存。

B3／B4 定向验收已通过。普通和 E2E 特性 Rust 各通过 213 项，原生八例全部通过，包括历史出现 B4 的四轮 Warm 链。诊断与历史失败报告单独保留。 [独立验证记录](VERIFICATION.md)

## 最终两轮失败与环境阻断

逐例对照原始报告后，两轮各有 128 项 B1 策略断言失败、2 项补拍准备清单身份断言失败、13 项早期补拍 OS5，以及 1 项被同一 OS5 阻断的反复中断案例。第二轮额外出现 B4，总结果仍为 124/144 与 123/145（通过／失败）。所有原有 44 例均通过。

第三轮在启动前因 E 盘约 6.85 GiB 低于 8 GiB 检查门槛而未执行。记录见最终验证目录 `native-3-blocked.json`，没有清理用户素材或缓存，没有降低门槛，不能宣称“三轮完整通过”。

## 已修复的测试框架问题

- 两个补拍特征中断案例的准备清单断言已按既有重绑规则核验。继续运行须产生新的 COLMAP 工作库归档，四个基础文件仅允许修改时间变化，归档及恢复后的 SHA-256 均须与中断前一致。新特征和匹配清单须绑定恢复后的身份。普通文件和其他恢复分支继续使用原严格断言。两例与三个缓存对照及断言负例均通过。 [独立验证记录](VERIFICATION.md)
- 模拟器原先各轮相同 PLY、忽略 pair 文件和按全局首行截断注册；已使用逐任务／尝试不同 PLY、实际配对和按来源注册。
- 模拟 SQLite 缺少 COLMAP 4 rigs/frames，真实 direct_input 校验拒绝 Bridge 发布恢复。补齐 trivial rig/frame 关系，并用生产 inspect 的补拍准入断言验证模拟成果；生产校验未放宽。
- 发布末尾屏障持有真实 TaskService gate，旧 snapshot 接口也进入 gate，导致观察死锁。E2E-only 快照读取真实 registry；已完成任务的取消先排队、释放屏障后等返回。不改生产锁或事务。
- 联合素材图片准备未经过旧 image_sequence 入口。现将图片／Mask 屏障放在 `prepare_images_with_preparation` 的真实写入循环并传入实际 material ID，读取第一个真实 PNG 证明已写入；旧入口钩子保留给其调用链。
- 通用 Model.write 可先用于冻结祖先。现用测试进程写入作用域限定模型屏障，证明命中时同 task/run 的 Mapper 正在运行；作用域在错误返回后恢复，防止跨调用串值。生成中的分析进程也通过当前执行上下文获得真实 task/run，准入前调用继续明确无运行。
- UI 继续点击返回时，旧 Cancelled 仍可能可见。等待终态之前现先确认该任务的新 runId 或原事务已经 Completed，不将旧运行终态当作本轮结果；不增加休眠、重跑或放宽终态期望。
- 图片顺序回答会自动切换当前素材，旧 check 等待已卸载控件；改为 click 后验证对应 material ID 的 ready 和最终独立回答。旧质量／GPU／提示框定位也改为准确可访问名称或任务 ID。
- Rust 库单元测试未初始化测试场景时，不再进入原生适配器；专用 CLI 单元准入测试显式建立自有场景。没有跳过既有测试。`cfg(test)` 资源采样不启动实际 nvidia-smi。
- 来源复用断言曾把联合 prepared manifest 提交前的逐来源记录当作可信像素。复读 `artifacts::normalize_collection_inner`、`invalidate_prepared` 与 `prepared_recovery::restore_frozen_video_source` 后确认这些记录仅是恢复计划，现有实现按冻结计划重抽。断言改为只对已提交有效 prepared 成果要求零额外抽帧；未提交窗口仍严格检查原选帧、实际来源路径和预算。该断言误报不列为业务缺陷。

开发轮允许修改框架后重新验证，均与最终固定代码的三轮完整报告分开保留。断言负例不是被放行的业务错误，而是确认错误来源、PTS、预算、初始化和不可信成果确实被拒绝。
