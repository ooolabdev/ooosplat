# 埋点与实际 GPU 快照验证

## 2026-11-11：统一流水线统计与诊断可靠性修复

本轮在本地 `main` 工作区实施，不修改数据库、上传 DTO、事件名称、统计版本、训练配方或项目 schema 版本；不提交、不推送、不向线上发送测试数据。以下为当前实现，后面的 2026-10-27 记录保留为历史验证。

### 行为与兼容

- 既有统计配置损坏或不可读时，本会话停发，保留原文件，不生成新安装 ID；只有明确设置操作才重读。首次安装默认不变。关闭在等待磁盘前生效，即使保存失败仍停发；开启须保存成功。排队事件和心跳绑定授权代次，不能跨关闭／重开复活。已交付 HTTP 的请求无法保证撤回，保存失败也不承诺重启后继续关闭。
- 本地偏好响应仅新增可选 `settingsError`。错误复用隐私区域已有提示，不再覆盖或清除任务错误；旧初始化响应不能覆盖后来的开关选择。按 React 检查表复核异步回调、重复操作及卸载清理，没有新增弹窗或文案。
- 基础运行事件与 Planner 汇总准入分开。全视频／全图片可准确发送开始事件；真正的视频＋图片混合跳过无法表达的开始事件，但保留阶段、失败和完成事件。无法用现有字段准确表达的完整素材汇总仍不发送，不新增 `unknown` 类型或借用第一份素材，多素材时不伪造单一时长。
- 分析、准备和继续准入共用来源观测，按唯一新增来源 ID＋身份定位，排除补拍 `__source`。恢复只读保存的 `inputs.sources`／观测，不重新访问原素材；旧顶层记录保留兼容。视频请求 FPS、实际 FPS、最少30帧覆盖及候选范围分别记录；短视频覆盖标记不会改变抽帧计划。已知首次实际数量优先于旧估算，统计只提取标量，不为观测复制整份帧索引。
- 初始重建在 Bridge 前冻结，Bridge 后、GPS／跨源扩展前冻结补帧结果。预算和实际新增数按来源相加，不把不同视频的同编号帧去重，也不重复相加联合模型三维点。成功、无预算、无须执行、回滚均保留真实状态；旧初始证据不足时为空。可选本地观测不进入素材、候选或发布准入身份，额外保存失败只警告，不覆盖生成错误。
- 诊断日志改为当前 run／operation／engine／PID 的文件身份和字节区间。终止行完成写入后冻结64 KiB／200行尾部，内部最多32条执行记录；GPS、联合 Bridge、同文件重试不再按名称或 mtime 猜测。成功进程后的应用校验、启动、I/O、取消或终止信息不匹配时不借用上一进程日志。旧任务无可信归属时附件为空；#83 仍使用本次准备操作内存缓冲。
- 同一执行／同一已确认 GPU 的临时 N/A 或采样失败不清空已知显存及驱动；换设备、换进程、退出或同名多卡歧义仍隔离。歧义状态不会在下一次空采样时恢复旧值。MCP 设备字段按既有 snake_case Schema 显式映射，不改 Schema 或诊断上传字段。

### 本轮验证

| 命令／范围 | 结果 |
| --- | --- |
| Rust `cargo test --offline --all-targets`，显式排除下列3项既有 Windows 链接权限用例 | 库857通过、38忽略、3排除；CLI 1通过；主程序目标0项 |
| `npm test` | 84文件、895项前端测试及7项诊断协议测试通过 |
| `npm run build` | HTML viewer、TypeScript、Vite生产构建通过 |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --offline --all-targets -- -D warnings` | 通过 |
| `git diff --check` | 通过 |

排除项：`commands::model_export::tests::mcp_rejects_redirected_exports_and_holds_target_edit_lock`、`mcp::tests::default_input_directory_rejects_redirected_folders`、`tasks::tests::unauthorized_paths_and_log_symlinks_are_rejected`。未过滤的首轮确实遇到 Windows OS1314／链接权限限制，不能将排除计为通过；另发现并修复短视频最少帧覆盖统计断言，最终复测通过。

新增回归实际进入共享图片准备，再以不存在的引擎路径停止，检查录制报文及移走原图后的保存数据恢复；不是仅手填 Tracker。另覆盖真实无引擎 Bridge 无预算与发布恢复、候选／素材身份不受观测变化影响、隔离 Windows 子进程的退出日志顺序、配置读写故障与并发开关、旧日志较新和重复执行区间、冻结预览／重发、设备 N/A 及MCP字段一致性。录制传输与模拟数据库均不访问线上。

未重新执行完整 SfM／Brush、未故意触发硬件 OOM、未上传测试诊断。桌面自然交互、其他平台及线上服务部署仍须单独验收；既有浏览器模块外部化、WASM URL、大分块及Windows链接器提示保留。原素材、模型、已有实验和工作树未清理。

English: anonymous telemetry now fails closed for existing settings faults, preserves an immediate session opt-out when saving fails, and isolates queued requests by consent epoch. Basic lifecycle events no longer depend on scalar Planner-summary eligibility. Source and Bridge observations are restored from identity-bound local checkpoints without changing recipes or cache identities. Diagnostic attachments are frozen from the failed execution's proven byte interval rather than filename/mtime guesses; preparation failures keep their bounded memory context. Confirmed GPU metadata survives unavailable samples without crossing adapters or runs, and MCP uses its existing field names. Validation passed: 857 Rust library tests and one CLI test, 895 frontend tests, seven diagnostic protocol tests, production build, formatting and Clippy; 38 ignored and three Windows symlink-permission exclusions are not claimed as passed. No production upload or full-training/hardware acceptance was performed.

## 历史验证记录

日期：2026-10-27；分支：`main`。

## 采集口径

- 沿用匿名 `planner_evaluation` 汇总：一次任务运行最多一条，内部 OOM 重试及补拍算法回退不另建汇总；手动重试属于新运行。取消不发送失败统计。
- 补拍 V2 在分析、画面准备、最终采用的重建、训练配置解析和结果发布后更新现有 Tracker。失败保留已确认的数据；结果发布和持久化成功前不记录最终产物。
- Source 为本批新增素材的原始尺寸、图片数或视频总帧数；重建与产物为最终采用的新旧合并结果。全景使用原始 ERP 画面，不把六面数当作素材数。
- 恢复优先读取已有源素材快照；旧记录缺失的信息保持未知，不用合并后的训练图片数填充新增素材数量。
- 最后执行阶段独立于计时器；阶段结束后失败仍保留该阶段，日志、心跳及终态不覆盖它。
- 首次调用 Brush 前记录初始配置；只有进程实际启动才更新最终执行配置。未启动的回退不覆盖上一轮实际配置，旧快照无法证明初始配置时保持空值。

## 本地记录和现有上传字段

- 本地任务注册表 `TaskRun` 增加可选 `brush_executions` 和 `current_brush_execution`，记录每次 Brush 的操作、PID、起止、退出码、配置及设备；旧记录可直接读取，不升级项目 schema。
- 每次尝试从空设备状态开始。实际型号只来自本次 `RuntimeTracker` 解析的 Brush 输出；资源采样按进程隔离、唯一型号匹配，硬件缓存也必须唯一匹配。不能确定显存或驱动版本时留空，厂商名不当作驱动版本。
- 退出、取消或任务结束时冻结执行记录；界面清空监控不会清空历史。迟到操作、其他运行和已退出进程的事件不能改写快照。
- 用户确认上传的诊断报告继续使用版本 1：`environment.actualDevice` 为本次实际型号字符串，对应显存和驱动放入 `gpus`；型号已知时保证存在对应 GPU 条目。没有本次设备证据则为 `null`，不从日志尾部或首选显卡推断。
- 各次执行摘要按原顺序放入现有 `detail`，保留根因及最后尝试，遵守 16 KiB 详情和原有报告总大小限制。完整执行记录留在本地。
- 摘要白名单不含 PID、操作 ID、路径、GPU UUID、总线标识或 GPS；继续使用原有脱敏及诊断预览确认流程。匿名埋点不新增设备型号字段。
- 数据库结构、SQL、服务端校验、上传 DTO、事件名、统计版本及生成参数未修改。服务端仅增加现有协议的回归测试。

## 自动验证

- Rust `--lib`：490 通过、0 失败、26 个需要真实素材、引擎或硬件的测试保留忽略。
- 测试覆盖首次失败与重试配置、同 Profile 降点、旧快照未知初始配置、补拍源数量、发布失败不记产物、最后阶段、设备冻结、重启读取、双显卡／同名多卡、缺失驱动或显存、迟到事件、取消、长摘要大小与顺序。
- 现有 TaskService 录制传输回归覆盖新建／恢复／补拍、图片／视频、成功／失败、每运行去重；既有测试继续验证关闭统计、静默传输失败、诊断确认及重复上传保护。
- 诊断服务严格校验及 JSONB 组装测试：7 项通过，确认实际 GPU 与执行摘要仍可使用原字段保存，无需迁移。
- 前端 `npm test`：600 项通过，另含上述 7 项诊断服务测试；`npm run build` 通过。保留既有 PlayCanvas 外部化、资源 URL 和大包提示。
- `cargo +1.99.0 clippy --all-targets -- -D warnings`、稳定版 `cargo fmt --all -- --check`、`git diff --check` 通过。
- Windows 符号链接用例在普通沙箱中存在权限 1314，已在获准的本机测试权限下通过；未通过修改生成逻辑规避该检查。

运行 Rust 检查时仅临时设置 `TAURI_CONFIG={"bundle":{"resources":[]}}`，避免测试编译复制引擎资源；没有修改保存的配置或生成安装包。

## 未执行的实机验收

未故意耗尽 GPU 显存、未重跑完整训练、未向线上服务发送测试诊断。上述硬件与流程边界通过解析回放、隔离文件及录制传输测试验证，不代表新的真实多 GPU 训练或跨平台验收已经完成。

## English summary

Reshoot V2 now feeds the existing telemetry tracker as facts become available, retaining partial failure data and the last actual stage. Each run still emits at most one anonymous planner summary. Brush executions keep separate local configuration and device snapshots; diagnostics use frozen evidence through the existing `actualDevice`, `gpus`, and bounded `detail` fields. Unknown fields remain null. No database, SQL, wire schema, version, generation parameters, or consent flow changes were made. Native hardware OOM and full training were not exercised.
