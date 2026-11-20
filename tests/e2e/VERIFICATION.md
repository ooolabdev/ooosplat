# Lifecycle matrix verification — 2026-11-18

**验收未完成：最终两轮完整套件均有严格失败；第三轮因磁盘空间门槛未启动。“连续三轮完整通过”尚未达成。** 报告标识与执行时间使用 UTC，日期标题使用本地 Asia/Shanghai。

现有分支 `test/playwright-recipe-e2e`，本地 `main`/HEAD 基线 `5f1df30fd67e22f5869cee92eed0a34e59c50463`。按后续指示未使用新分支。原有 44 例保留，扩展至 268 例；[覆盖矩阵](LIFECYCLE_MATRIX.md)、[缺陷证据](ISSUES.md)、[既有忽略测试及原因](IGNORED_TESTS.md)。不提交、不推送、不更新远端、不安装打包、不运行真实生成引擎或 GPU 探测，不上传数据。

## 补拍准备清单断言补验 — 2026-11-20

历史失败的 145 例在 B1 至 B4 修复后独立复跑，143 例通过，剩余两例均为补拍特征尚未保存完成检查点时中断。任务均完成并发布，归档和恢复后的四个基础文件 SHA-256 相同。修改时间变化触发准备清单身份重绑，原 E2E 对 ID 的严格相等断言因此失败。原报告和磁盘核对保存在 `.cache/e2e/lifecycle-validation/20261120-historical-failures-rerun/`。

新增断言仅在这两种中断边界验证有证据的重绑。它核对新归档、冻结快照身份、四个基础文件的实际 SHA-256、其余文件记录，以及新特征到匹配的身份链。缓存复用、原策略、预算和发布断言仍执行。负例验证错误快照、基础文件内容变化、普通文件修改时间变化和缺少新归档都会被拒绝。

定向运行 `20261120062918289-8fdad4b1-7172-4deb-bb72-1bd60811e355` 共 6 例，全部通过，单 worker、零跳过、零重跑，93.45 秒。两例实际发生重绑，三个缓存对照的准备清单 ID 保持一致，断言负例通过。专用 GUI 的 SHA-256 与 145 例复跑完全一致。TypeScript、文档和差异检查均通过。独立报告、清单及核对记录在 `.cache/e2e/lifecycle-validation/20261120-prepared-rebind-assertion/`。本轮覆盖六个定向案例。

## B3／B4 文件操作修复与定向验收 — 2026-11-20

B3 对关键 JSON 的 Windows OS 2 打开失败设置最多四次短等待，读完同一个句柄，沿用现有保存和解析逻辑。B4 对完成清单后的快照目录改名设置最多三次可取消等待，目标冲突按原身份规则处理，改名成功后只读封存。两处等待均属于文件操作，不消耗任务的自动补救额度。根因复现、错误范围与剩余风险见 [ISSUES.md](ISSUES.md)。

独立证据目录为 `.cache/e2e/lifecycle-validation/20261120-json-snapshot-fixes/`。`fixed-code.json` 保存核验文件的 SHA-256，`case-manifest.json` 保存八个选定原生案例。既有完整套件和诊断报告保留原结果。每项 Rust 检查和原生验证命令记录 C、E 盘执行前容量及 2 GiB、8 GiB 最低要求。

| 检查 | 最终结果 |
| --- | --- |
| 普通 Rust 定向回归 | 213 passed / 0 failed / 10 既有 ignored |
| E2E 特性 Rust 定向回归 | 213 passed / 0 failed / 10 既有 ignored |
| 普通／E2E 全目标 Clippy `-D warnings` | 均通过 |
| E2E TypeScript、Rust 格式、文档和差异检查 | 均通过 |
| Windows 原生 E2E，单 worker，`retries: 0` | 8 passed / 0 failed / 0 skipped / 0 flaky，119.49 秒 |

两种 Rust 构建均通过 5,000 次真实保存压力测试、确定性 OS 2 恢复和真实 Windows 子文件句柄冲突测试。持久占用、取消、并发目标以及改名后封存回归均通过。10 项既有 ignored 的名称和依赖见 `rust-final-ordinary.log` 与 `rust-final-e2e.log`，包括手动基准和使用真实引擎、GPU 或已有本地素材的检查。

原生执行 `20261120035451571-6aa74197-185d-47f0-987a-bfaf1393292d` 通过三条连续补拍链、同一补拍三次中断、任务策略重启持久化、取消重启继续、Prepared 和 Committed 恢复。专用 GUI／CLI 构建与原生验证命令合计 187.20 秒。GUI SHA-256 为 `27ee8223606781c11c35c6fa0e3b7ceff8a1c077fa7323fd768b76a012c8dcc1`。HTML 在对应 `.cache/e2e/artifacts/<runId>/report/index.html`，验证目录中另有 `native-report.json`、`native-cases.json`、`snapshot-audit.json` 和 `verification.json`。

磁盘证据核对 17 个任务的策略记录、9 份快照及 1,503 项冻结资产。文件长度、SHA-256 和只读属性均符合清单，9 份初始化 PLY 摘要与各自父项目一致，9 份祖先检查全部不变。发布恢复通过已有的零额外引擎调用断言。验收范围是这八个原生案例和相关 Rust 测试，完整 268 例及三轮稳定性仍待验证，真实引擎、画质和硬件验收仍待实机完成。

## B2 Mask 复制修复与定向验收 — 2026-11-18

已按后续授权修复联合补拍从只读快照复制 Mask 时传播只读属性的问题。私有复制函数只处理当前项目工作 Mask：校验目标及父路径，解除已有工作文件项，再调用原有可取消字节复制器创建可写副本。兼容旧失败任务留下的只读文件及硬链接；不改变冻结快照／祖先的属性、配方、检查点、缓存校验、通用复制器或原子保存器。具体原因与历史失败现场见 ISSUES B2。

独立证据目录：`.cache/e2e/lifecycle-validation/20261118022200567-mask-copy-fix/`。`baseline.json` 保存本次修改前摘要；`mask-copy-fix.patch` 为相对本次开始时的实现／断言差异；`fixed-code.json` 记录原生执行前固定的代码摘要；`case-manifest.json` 是 20 例清单，`native-cases.json` 包含逐例任务／运行身份及证据目录，`policy-audit.json` 保存 Mask、策略和祖先核对，`verification.json` 汇总结果。之前所有失败及 B1 验证报告均保留。

| 检查 | B2 本次结果 |
| --- | --- |
| 普通相关 Rust 回归 | 131 passed、0 failed、2 既有 ignored；`rust-related-ordinary.log` |
| E2E 特性相关 Rust 回归 | 131 passed、0 failed、2 既有 ignored；`rust-related-e2e.log` |
| 普通／E2E 全目标 Clippy | 两种均 `-D warnings` 通过；`clippy-ordinary.log`、`clippy-e2e.log` |
| 专用 E2E GUI／CLI 构建 | Passed；`native.log`；离线依赖，无真实引擎或安装包 |
| 20 个定向原生案例 | 20 passed、0 failed、0 skipped、0 flaky；单 worker、retries=0；284.965 秒（约 4.7 分钟），含构建及启动约 5.8 分钟 |
| E2E TypeScript／格式／文档／差异 | 均 Passed；`e2e-types.log`、`fmt.log`、`docs.log`、`diff-whitespace.log` |

Rust 范围为联合输入、冻结快照、已有可取消复制、视频恢复及 TaskService；其中新增七项 B2 测试全部通过，覆盖可写与重复复制、旧只读目标、只读硬链接不修改祖先、预取消、越界／目录／父文件目标、符号链接／重解析点、真实 Windows 共享冲突。保留的既有忽略项为 `pipeline::runner::inputs_runner::tests::native_joint_erp_and_ordinary_reconstruction_from_real_pixels`（需要真实 COLMAP 与 Ballintoy 素材）和 `tasks::tests::real_activity_journal_count_redaction_read_only_regression`（需要用户已有任务记录），均未启用。

开发首轮 `rust-mask-ordinary.log` 因新增函数误用模块路径 `super::artifacts` 而编译失败，已纠正为 `crate::pipeline::artifacts`，该日志保持原样。之后两种相关 Rust 回归通过，并在最终固定代码上执行唯一一轮 20 例原生测试；没有失败重跑或测试执行期间的实现变更。Clippy 保留既有 Cargo manifest redundant_homepage 提示。

| 原生覆盖组 | 数量 | 结果 |
| --- | ---: | --- |
| 新增来源 sourcePrepared 检查点三种中断方式 | 3 | 全部通过 |
| 冻结祖先 sourcePrepared 检查点三种中断方式 | 3 | 全部通过 |
| sourcePlan／sourcePrepared／prepared 写完、检查点未保存的强杀窗口 | 3 | 全部通过 |
| 图片与 Mask 部分写入取消 | 2 | 全部通过 |
| 视频抽帧执行中取消及完成写入后强杀 | 2 | 全部通过 |
| 同一补拍在准备、特征、训练三阶段反复中断 | 1 | 四次运行后完成，策略／初始化／额度检查通过 |
| B1 已通过的六个策略与恢复回归 | 6 | 全部通过 |
| 合计 | 20 | 无跳过、预期失败或重跑 |

原生 runId：`20261118023455616-fddde331-9699-4abc-a616-fc17e300ffb1`。HTML 位于 `.cache/e2e/artifacts/20261118023455616-fddde331-9699-4abc-a616-fc17e300ffb1/report/index.html`；原始 JSON 同目录上层的 `report.json` 已复制为证据目录的 `native-report.json`。代码摘要 `c9cf769dd2732412ffda3c0e8755b493c289ecca776bf088c1b4e6636f15bc0d`，GUI 二进制摘要 `6cff2372e994ac4dea0ba5fa4f154e62f1978ce1a7d934ab16787a8f7105479c`。

核对了 450 份继承 Mask：工作内容与冻结清单／源文件 SHA-256 一致，工作副本可写，冻结副本仍只读。15 份祖先检查全部 unchanged=true；35 个实际任务的内存策略、actual_configuration、request_parameters、tasks.json 及项目 state 一致。34 个任务 Completed，唯一 Failed 是关闭补救后故意注入 OOM 的预期业务终态；总训练调用 38 次，包括三项已声明的训练中断／继续，不代表额外自动补救。发布与可信候选恢复、完成任务重启均未重复训练。

本次 B2 定向验收通过；没有在修复后重跑 268 例完整矩阵或三轮稳定性验证。B3、B4 保持原实现与历史失败记录，真实引擎兼容性、画质和硬件验收仍未完成。每次编译及原生执行前均重新检查容量，未清理用户素材或缓存；没有提交或推送。

## B1 后续补测完成 — 2026-11-18

E 盘空间恢复后，使用相同 B1 实现与用例补跑之前受阻的检查。证据目录 `.cache/e2e/lifecycle-validation/20261118012907935-policy-fix-resumed/`，其中 `verification.json` 汇总实际结果，`native-report.json` 保存原始 Playwright JSON，`native-cases.json` 包含逐案例任务／运行身份及附件，`policy-audit.json` 核对真实持久化记录，`case-manifest.json` 保存六例清单。首次实施时的受阻记录和修复前两轮完整报告均保持原样。

| 检查 | 后续补测结果 |
| --- | --- |
| 普通 TaskService Rust | 79 passed、0 failed、1 既有 ignored；`rust-ordinary.log` |
| E2E 特性 TaskService Rust | 79 passed、0 failed、1 既有 ignored；`rust-e2e.log` |
| 普通／E2E 全目标 Clippy | 两种均 `-D warnings` 通过；`clippy-ordinary.log`、`clippy-e2e.log` |
| 专用 E2E GUI／CLI 构建 | Passed；`native.log`；本地离线依赖，无真实引擎或安装包 |
| 六个定向原生 E2E | 6 passed、0 failed、0 skipped、0 flaky；单 worker、retries=0；59.909 秒，含构建及启动约 94.5 秒 |
| E2E TypeScript／格式／文档／差异 | 均 Passed；`e2e-types.log`、`fmt.log`、`docs.log`、`diff-whitespace.log` |

Rust 唯一既有忽略项为 `tasks::tests::real_activity_journal_count_redaction_read_only_regression`，需要 `OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY` 指向用户已有任务记录；本次没有启用。其余 TaskService 测试包括新增的六种策略状态与旧运行配置隔离均通过。Clippy 保留既有 Cargo manifest 的 redundant_homepage 提示，没有修改清单压制它。

原生 runId 为 `20261118013132010-c8b7f281-308b-47b1-8fdf-a10ddae41cc7`，HTML 报告位于 `.cache/e2e/artifacts/20261118013132010-c8b7f281-308b-47b1-8fdf-a10ddae41cc7/report/index.html`。代码摘要 `1c2cd4cafae7b8480d1a42703a33489d04a62d042a2b51bfb99c4c7e8d0a5106`，测试 GUI 摘要 `d62ec05619fc054d5b65aaf29c257753af594792cf3c284821d634c409a0b77a`；补测期间实现与案例未改变。

| 原生定向案例 | 结果与实际执行 |
| --- | --- |
| 关闭补救后取消／重启／继续并遇 OOM | Passed；两次训练分别为首次中断及继续后的 OOM，最终任务按预期 Failed；策略 false，未自动补救 |
| 普通特征检查点取消／重启／继续 | Passed；仅训练一次 |
| 普通 OutputInstalled 强杀／重启恢复 | Passed；仅训练一次，发布恢复不重新训练 |
| 普通训练执行中取消／继续 | Passed；两次训练，沿用原配方及初始化 |
| 补拍可信候选取消／重启／继续 | Passed；父任务与补拍任务各训练一次；祖先摘要 unchanged=true |
| 完成后重启核对策略 | Passed；原任务／运行记录及唯一训练调用保持不变 |

对上述七个实际任务额外核对内存 generation_policy、actual_configuration、request_parameters、磁盘 tasks.json 与项目 state，全部等于独立维护的原策略；未观察到策略丢失。该轮完成 B1 的六例定向验收。B2、B3、B4 修复以及 145 个历史失败案例的复验见前面的独立记录。

## B1 首次实施与空间阻断记录 — 2026-11-18

本次仅按后续授权修复任务策略快照遗漏，保留全部已有修改。`checkpoint_snapshot` 明确输出保存的策略或 null，不改变策略、配方及恢复语义，不批量回填历史记录。以下为修复后的独立检查，之前的固定代码完整轮次保持原样。

证据目录：`.cache/e2e/lifecycle-validation/20261118012000749-policy-fix/`。`baseline.json` 保存修改前摘要及容量；各检查的 JSON／日志记录实际命令和状态，`verification.json` 为汇总，`case-manifest.json` 为六个选定案例，`policy-fix.patch` 为相对此次开始时的实现／用例差异。

| 检查 | 本次状态 |
| --- | --- |
| E2E TypeScript | Passed；`e2e-types.log` |
| 格式 | Passed；`fmt-final.log`；初次新增 Rust 测试的换行不符已修正，保留 `fmt-initial.log` |
| 差异／文档 | Passed；`diff-whitespace.log`、`docs.log` |
| 选定原生清单 | 已枚举六例，workers=1、retries=0；`native-selection.log`；仅发现测试，没有执行 |
| 普通 TaskService Rust | 磁盘检查阻断，未编译或执行；`rust-ordinary.json` |
| E2E 特性 TaskService Rust | 磁盘检查阻断，未编译或执行；`rust-e2e.json` |
| 普通／E2E 全目标 Clippy | 磁盘检查阻断，未执行；`clippy-ordinary.json`、`clippy-e2e.json` |
| 六个定向原生案例 | 磁盘检查阻断，未构建、未启动应用；`native.json`；没有本次原生运行报告 |

启动前 E 盘约 7,354,978,304 字节（6.85 GiB），低于 8,589,934,592 字节门槛；C 盘约 9.14 GiB，满足 2 GiB 门槛。不降低门槛、不清理缓存，不使用修复前二进制冒充修复验证。新增 Rust 测试覆盖六种策略状态在配置回调、磁盘和重新加载中的保留，以及旧运行配置更新隔离；尚未获得它们的编译／运行结果。

选定原生案例：完成后重启核对策略；关闭补救后取消重启并遇到 OOM；普通训练 partial/cancel；普通 features checkpoint/cancel-restart；普通 publication:outputInstalled checkpoint/crash；补拍 candidate checkpoint/cancel-restart。原生总清单仍为 268 例，本次不重复完整套件或三轮稳定性验证。

当时记录的待执行定向命令如下，后续补测使用这些范围并离线执行 Cargo（执行前仍须检查磁盘）：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib tasks::tests::
cargo test --manifest-path src-tauri/Cargo.toml --lib --features e2e tasks::tests::
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features e2e -- -D warnings
npm run test:e2e -- --grep "durable task policy snapshot|disabled retry authorization continuation|lifecycle ordinary training partial cancel |lifecycle ordinary features checkpoint cancel-restart |lifecycle ordinary publication:outputInstalled checkpoint crash |lifecycle reshoot candidate checkpoint cancel-restart "
```

该次修复已实施，但运行验收当时受阻；后续补测结果见上节。原始全量失败报告及逐例归类见下文。

## 固定实现的检查记录

| 检查 | 结果／本地证据（`.cache/e2e/lifecycle-validation/` 下） |
| --- | --- |
| 新适配器、屏障及真实成果验证 | 最终 13 passed；`adapter-execution-scope.log`（其中 12 项写入作用域检查另见 `adapter-scoped-writers.log`） |
| 前端完整回归及诊断协议 | 隔离执行 88 files / 976 tests + 7 协议测试 passed；`frontend-isolated-final.log`；之前的一次超时仍保留如下 |
| 普通 Rust 全目标 | 最终 1123 库 + 1 CLI passed；48 库 + 1 CLI 既有 ignored；`rust-final2-ordinary.log` |
| E2E 特性 Rust 全目标 | 最终 1136 库 passed；两份 CLI 各 1 passed；48 库及每份 CLI 的 1 项既有 ignored；`rust-final2-e2e.log` |
| E2E TypeScript | Passed；`tsc -p tests/e2e/tsconfig.json`；`e2e-types-final2.log` |
| 前端生产构建 | Passed；`frontend-build-final.log`；保留既有 bundle/WASM/browser-external 提示 |
| 普通 GUI／CLI 构建 | Passed；`ordinary-build-final2.log` |
| 专用 E2E GUI／CLI 构建 | Passed；`native-scoped-writers-probe.log` 中的构建输出；无安装包 |
| 普通／E2E 全目标 Clippy | 均 `-D warnings` 通过；`clippy-final2-ordinary.log`、`clippy-final2-e2e.log`；既有 Cargo manifest 的 redundant_homepage 提示仍存在 |
| 格式／空白 | `cargo fmt --check`、`git diff --check` Passed；`fmt-final2.log`、`diff-final2.log` |
| 文档 | 42 个 Markdown Passed（已将测试说明纳入原文档检查器）；`docs-delivery.log` |

前端检查初轮 88 files / 976 tests 及诊断协议 7 项通过；并发开发校验的一次完整前端回归为 975 passed / 1 timed out：`App live log > keeps all 90 ordinary photo groups on one global image count without labelling each group as a retry`，5000ms；保留 `frontend-final.log`，没有延长超时或修改原测试。当前原生开发轮结束后单独完整复核通过，见上表；不将其解释为已证明超时的根因。

普通 Rust 的第一次扩展全量检查曾出现 JSON 5000 次保存读者 NotFound，1122 passed / 1 failed / 48 ignored；保存于 `20261117114719681-checks/rust-ordinary.log`。最终普通全目标通过不抹去该历史失败，原因未确立，见 ISSUES B3。初期 E2E CLI 未初始化场景的框架错误、受限桌面 CDP 启动失败及修改中开发轮均保留为开发证据，不计入三轮验收。

开发轮 `20261117114932336-efd08477-30de-42c2-893f-694085c6ccf2` 执行 230 例：160 passed / 70 failed / 0 skipped / 0 flaky，约 61.6 分钟，`native-development-full.log`。期间修改过框架和案例清单，包含旧观察接口死锁与一项开发中列表变动错误，不能用作固定实现的验收轮。随后一次接口检查启动曾卡在测试自有 PowerShell 的 Conda profile，Node 尚未执行；只终止已核对归属的该进程树后用无 profile 模式启动，没有终止其他应用。

最终接口开发检查 `20261117125759000-18992eae-f5f8-4f00-9f06-a44a007a9f10`：13 例，4 passed / 9 failed / 0 skipped / 0 flaky，155.5 秒。四个通过项为 Warm 链、Cold 额度耗尽、Bridge 补帧及模型中断；失败包含七项任务策略、一个早期补拍恢复，以及一个已按原 prepared 信任语义校准的断言误报。后续 `20261117131108653-7a404ecb-98af-4dd3-aad6-d7a973ffb200` 两例均严格捕获任务策略为 null，未出现来源复用误报；`20261117132001749-f6bc2343-fb84-49dc-a0b8-05307d017be5` 的 Warm 链通过、祖先冻结准备恢复失败，两例的 teardown 祖先摘要检查均完成且不变。上述全部为独立开发报告，不计入最终三轮。

随后完整开发轮 `20261117132207514-a93bed17-27f4-44f1-a4f5-8d7cc299644b` 执行 268 例：109 passed / 159 failed / 0 skipped / 0 flaky，47.85 分钟。报告定位到图片／Mask 屏障挂在旧入口、模型屏障可能先落在祖先快照写入，以及 58 项等待函数读到旧终态的竞态。另有 86 项策略记录失败、10 项任务真实失败、4 项未命中图片／Mask 屏障和 1 项早期补拍失败阻断后续屏障。修正测试适配器后，代码摘要守卫阻止该计划进入下一轮；这不是最终三轮之一。完整报告及清单保留于 `20261117132206624-native-rounds/`。

写入作用域修正后 `20261117142422016-731d5091-8837-49fc-b653-735ccc92b15a`：13 例，6 passed / 7 failed / 0 skipped / 0 flaky，142.1 秒。实际命中联合图片／Mask 写入屏障并读取第一个真实 PNG，模型屏障确认当前 Mapper 正在执行；继续操作先等待新 runId 获准启动。显卡、OOM 额度、Cold、Warm 链及原有取消重启通过；失败只剩 5 项策略为 null、2 项早期补拍恢复 Failed。文件、参数、运行及祖先摘要断言仍严格执行。

## 最终固定代码的完整原生结果

证据目录：`.cache/e2e/lifecycle-validation/20261117143037964-native-rounds/`。`case-manifest.json` 为全部 268 例的独立清单，保留原有 44 例，新增 224 例。单 worker、`retries: 0`，两轮均无筛选、无跳过、无预期失败。每个结果的实际任务／运行身份与隔离目录见 `native-1-cases.json`、`native-2-cases.json`。

| 轮次 | 独立报告标识 | 实际执行结果 | 用例执行耗时 |
| --- | --- | --- | --- |
| 1 | `20261117143038853-70a3437b-f935-4366-8ae8-48cd20253c09` | 268 执行；124 passed / 144 failed / 0 skipped / 0 flaky | 47.62 分钟 |
| 2 | `20261117151825888-be766c08-ac52-4faa-b204-27377feef0d6` | 268 执行；123 passed / 145 failed / 0 skipped / 0 flaky | 48.89 分钟 |
| 3 | 无原生 runId；`native-3-blocked.json` | **未启动**：E 盘 7,357,841,408 bytes（约 6.85 GiB），低于脚本要求的 8 GiB；C 盘约 8.88 GiB，满足 2 GiB 门槛 | 不适用 |

两轮均使用代码 SHA-256 `040257551cf93e275b3effaccdb5c01db6866921946f217ae4abbe19a7336ff4`，GUI 测试二进制 SHA-256 `561e16bb8f6b255eb730ee1719a36b3a046c2d230fad65f19083edd44e637179`。执行后的代码摘要复核一致。前两轮开始前 E 盘分别为 21,421,731,840 与 14,432,014,336 bytes，均通过门槛；第三轮启动前明确阻断，没有降低门槛、清理用户素材／缓存或伪造第三轮报告。

独立 HTML 位于 `.cache/e2e/artifacts/<报告标识>/report/index.html`，JSON 为同目录 `report.json`，trace／失败截图／实际执行附件在 `results/`。验证目录另保存两轮 JSON 副本、日志、案例索引及 `verification.json`；第三轮只有启动前环境记录 `native-3-blocked.json` 和 `native-3-preflight.log`，没有执行结果，不能计为跳过或完成的一轮。

| 严格失败归类 | 第 1 轮 | 第 2 轮 |
| --- | ---: | ---: |
| B1：任务 generation_policy 为 null | 128 | 128 |
| 补拍特征中断后准备清单身份断言 | 2 | 2 |
| B2：早期补拍继续后 OS5 / Failed | 13 | 13 |
| B2：同一补拍反复中断被 OS5 阻断，未到达下一屏障 | 1 | 1 |
| B4：无中断 A→B→C→D，在 D 准备原项目文件时 OS5 | 0 | 1 |
| 合计 | 144 | 145 |

只读证据审计 `evidence-audit.json` 核对了全部失败及其实际磁盘记录：每轮 124 项祖先文件摘要检查全部保持不变；12 个 Bridge 定向执行的来源身份、冻结初始／候选选帧、补帧数组、预算及存在的全局冻结计划均保持一致，初始帧数独立期望为 `[35,35]`，最大为 `[42,42]`。审计通过表示证据一致、已归类，不代表原生套件通过。两轮均没有发现未归类失败；所有原有 44 例均通过。

通过的定向案例包含显卡保持、迟到事件／重复操作／队列隔离、混合 FPS 与 PTS、大图片集独立回答、ERP 六面、Bridge 六个边界、兄弟派生、Warm→Cold→Warm、全量后增量、首次恢复后补拍、OOM／Cold 额度耗尽、部分采用／发布恢复后下一轮及全部新增零注册。A→B→C→D 第一轮通过、第二轮 D 失败，不宣称稳定通过。反复中断同一补拍的复合案例在准备阶段恢复时失败，后续特征／训练停点未触达，必须保留为未完成验收。

## 未完成的实机验收

真实 COLMAP／Brush／FFmpeg 兼容、视觉几何与注册质量、画质、PSNR、显存及真实 OOM、多型号显卡和远端 Windows 工作流未运行。本地模拟原生控制流不能代替这些验收。历史旧策略／旧快照、显卡消失、任意断电／文件损坏及所有素材×质量×硬件组合未穷举。

B1 至 B4 的后续定向验收独立记录在本文前部。三轮全量计划仍待按原门槛完成。历史报告使用执行当时的代码和结果。

以下为扩展前的历史记录，其中 44 例三轮通过仅属于旧套件，不能代表当前 268 例通过。

# Historical native recipe E2E verification — updated 2026-11-17

Branch: `test/playwright-recipe-e2e`, based on `main` at `64c98c2`. Local Windows/WebView2 only; no commit, push, installer, real generation engine, GPU training or online test upload.

## Automated checks

| Check | Result |
| --- | --- |
| Frontend Vitest | 88 files, 976 tests passed |
| Existing diagnostic protocol tests | 7 passed |
| Frontend production build | Passed; existing bundle-size/WASM/browser-external warnings remain |
| E2E TypeScript check | Passed |
| Ordinary Rust all-target regression | 1123 library + 1 CLI tests passed; 48 library + 1 CLI tests intentionally ignored |
| E2E adapter Rust tests | 7 passed: path/field isolation, positional dataset, selected device, fail-closed executable, valid tiny DB/model/PLY, full-path input identity and no invented source frames |
| E2E-feature shared saver tests | 28 passed, including actual open-reader replacement, real sharing/readonly refusal and 5000 consecutive saves |
| Clippy | Ordinary and `--features e2e`, all targets, `-D warnings` passed |
| rustfmt | Passed |
| Documentation links | Passed |

The sandbox initially denied three existing Windows symlink fixtures (OS1314). Re-running the complete Rust suite with the required local privileges passed; no test was changed or silently skipped to hide these permission failures.

## Windows state-save follow-up — 2026-11-17

The two 42/44 baseline failures share the state-save chain, not a recipe or geometry failure:

- `Alpha pixels true`: Brush completed; `final.ply` contained 64 valid Splats and its SHA-256 matched the **Committed** transaction. A subsequent view save failed.
- `reshoot appearance`: the ordinary source project's creation/preparation save failed **before any engine call or reshoot**.

An isolated comparison reproduced `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` failing with OS5 while an ordinary reader, including delete sharing, held the old target open. The old primitive succeeded 2000 times without a reader but failed on the first replacement with a reader. Rust 1.100's standard rename compatibility path passed 5000 replacements with 946079 concurrent reads, zero invalid records and zero read errors. True non-delete-sharing locks and read-only targets still failed and preserved the original file. This proves this contention mechanism, not the cause of every historical OS5 report.

The production JSON saver now uses that standard-library path on Windows, on the existing blocking executor. Serialization, unique same-directory staging, `write_all`, `flush`, `sync_all`, exact length verification and handle closure are retained. No generic OS5 sleep/retry, ACL/read-only alteration, target deletion or copy-overwrite fallback was introduced. Large-file publication and non-Windows replacement behavior are unchanged. I/O details retain the original OS error plus fixed save-operation/substep/target-role labels; private paths are not included.

New ordinary Rust regressions prove held readers retain complete old snapshots while new opens see complete new JSON; 5000 real public-saver calls with concurrent reads never expose missing/partial records. Actual sharing/read-only errors preserve old JSON. Creation failure/restart reuses the same project and owner intent. Failed views with an intact Commit reconcile through the real TaskService with normal readers held open, unchanged run history/training options/model bytes and a generation executor that would panic if invoked.

Alpha, all four reshoot-answer cases and Prepared/Committed recovery now set `sharedJsonReaders: true`: a test-only read-only handle is held across each **real** save. Polling frequency, assertions and `retries: 0` are unchanged. This observer does not manufacture a successful write, pause readers or retry failures.

Frontend tests/protocol tests/build and test TypeScript check passed. Ordinary/e2e all-target Clippy with `-D warnings`, rustfmt and documentation checks passed. Ordinary GUI/CLI binaries were rebuilt without the test feature; the tested E2E control/environment/error markers are absent, while the new JSON diagnostic marker is present.

The first restricted native attempt could not connect to WebView2's loopback CDP endpoint; it was stopped after two startup timeouts, not counted as a green run. Native verification requires the desktop/loopback permissions subsequently granted. No assertion or test was skipped to compensate.

### Three consecutive complete native runs

| Run identity | Result | Duration |
| --- | --- | --- |
| `20261117015210863-f1baf47d-2ce6-4de5-a5ba-f03eaace3e31` | 44 passed, 0 failed, 0 skipped | 4.6 minutes |
| `20261117015740950-43acc85b-4542-4666-9d90-573ab2a720f3` | 44 passed, 0 failed, 0 skipped | 4.1 minutes |
| `20261117020236848-75216382-9113-4ab5-b3b0-709f971ff0cf` | 44 passed, 0 failed, 0 skipped | 4.1 minutes |

All 132 scenario executions passed with one worker and zero retries, including both original failure contexts and Prepared/Commit recovery with deterministic shared readers. Each report and actual execution snapshot remains under its invocation's `.cache/e2e/artifacts/<run>/` directory. No assertions were weakened, sources reordered or polling delayed. These are local Windows results, not remote CI, real-engine or production-success-rate evidence.

## Historical native UI runs — 2026-11-16 baseline

44 scenarios are implemented, including the assertion negative control. Read the newest invocation's report using `npm run test:e2e:report`; each invocation is retained independently under `.cache/e2e/artifacts/<run>/`. The earlier fixed report locations `.cache/e2e/report` and `.cache/e2e/results` retain pre-migration evidence.

Complete 44-scenario invocation: **43 passed, 1 failed**, in about 5 minutes. Report identity: `20261116131720551-0c405bb9-b820-4929-a4a5-09a6fa80c873`. Failure was the ordinary base-generation portion of `reshoot geometry`, with real `replace_state` OS5, **before the reshoot started**. This is not a green-suite result.

Final complete re-run with correct native file/folder button routing and strict PTS/source-frame checks: **42 passed, 2 failed**, in about 6 minutes (`20261116133042118-5a143eeb-1744-461f-9402-b5d47741623b`). Both failures were real `replace_state` OS5, in `Alpha pixels true` and the base-generation portion of `reshoot appearance`. All 44 cases have passed in at least one run, but the suite is **not stably green**. This distinction must be retained.

Afterwards, a targeted run of crash classification, reshoot geometry, ordinary fast video and 29.97/VFR passed **4/4**. It does not invalidate the full-run failure. Saved-GPU preferences, partial reshoot adoption and initial-Mapper geometry/I/O also passed a separate **4/4** run.

The video/ERP/Bridge/29.97 targeted re-run passed **10/10**. After tightening the explicit command whitelist (including COLMAP help), a three-case run passed Alpha and fast video, while the ordinary base generation in `reshoot appearance` again failed with real `replace_state` OS5 (**2/3**). No assertion or retry was relaxed in response.

Dedicated test executables (`ooo-splat-e2e.exe`, `splatstudio-e2e.exe`) then passed real GUI/MCP/CLI startup and recipe checks **3/3**. Ordinary debug executables were rebuilt without the feature after the earlier prototype builds; neither contains the tested E2E control/error markers. Future test runs build only the dedicated test executables.

Final publication-count contrast check passed **4/4** (JPEG, Prepared recovery, Commit recovery, ordinary fast): simulated progress reports **128** Splats while the actual validated PLY contains **64**; persisted publication remains **64**. The final negative-control check also passed **1/1**, including missing and duplicate command arguments.

Representative targeted runs passed for MCP/CLI, all four reshoot answers, per-source Bridge, transparency, mixed ERP, saved GPU settings, partial-source adoption and initial-Mapper geometric failure. These are not substitutes for a complete green invocation.

## Historical failure evidence — before the state-save fix

Earlier complete 40-scenario runs ended at 39 passed / 1 failed. The failing scenario moved between ordinary preparation, publication and the base-generation portion of a reshoot. The last reproduced message retained:

```text
文件读写失败：拒绝访问。 (os error 5)
e2e_save_substep=replace_state
```

The annotation identified a real replacement failure, not a simulated engine error or recipe assertion; by itself it did not distinguish JSON from large-file replacements. At that point the exact failed file and cause were still unconfirmed. The follow-up above records the subsequently verified JSON contexts and reader-contention fix. The test framework still does not add retries, serialize away production writes or replace the saver with a fake one.

Failed run directories and local traces are retained. New green runs do not erase the original failures or establish that unrelated production I/O errors have all been resolved.

## Contract evidence

- Independent constants: ordinary tiers use reviewed caps 6192 / 12385 / 15481 for 30×96×64 images and 32 sparse points; ERP high uses 1280. Exact Brush option sets also reject unexpected learning-rate/noise overrides.
- Wrong iteration count, FPS, Gaussian initialization presence and duplicate Brush launch fail the same assertions used by positive tests; the mock does not rewrite the recipe.
- Real image preparation, database/feature/model validation, Gaussian inspection and publication remain active. Final output count is independently validated as 64, not taken from a progress estimate.
- Warm-to-Cold numerical fallback archives initialization outside the dataset; export OS112 and process crash do not consume algorithm fallback. A Warm initialization's SHA-256 is compared with the source Gaussian hash.
- Candidate and Commit recovery tests use durable real transactions and require zero additional engine calls after restart.
- Normal frontend production output contains no dialog adapter markers; Rust test commands/module are feature-gated with default features empty and a release-build prohibition.

## Not certified / not yet native-matrix coverage

- Manual GitHub Windows workflow has been created, not dispatched or remotely verified. No PR/release workflow or merge gate was added.
- No real COLMAP, Brush, FFmpeg, GPU compatibility, registration quality, rendering quality, PSNR, VRAM usage or online success-rate certification.
- Old-strategy permutations, GPU disappearance, queue/late-event race combinations, Bridge-mid-restart combinations and every historical snapshot are still principally covered by the existing Rust/Vitest suites, not exhaustively repeated through native windows.
- The native toolchain is Windows-only. No Linux/macOS native E2E result is claimed.
