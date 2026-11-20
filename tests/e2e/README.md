# Native recipe E2E / 配方端到端测试

**Manual, slow suite.** Use `npm test` for the fast development checks. Start native E2E explicitly with `npm run test:e2e`, or manually dispatch **Native recipe E2E (manual, slow)** in GitHub Actions.

**耗时较长，需主动触发。** 日常开发使用 `npm test` 做快速检查。原生 E2E 通过 `npm run test:e2e` 按需启动，GitHub Actions 仅支持手动触发。

Windows + installed WebView2 + Node.js 22.12+ + the repository's pinned Rust toolchain are required. Run from the repository root after `npm ci`. No generation engines, CUDA, real GPU or Playwright Chromium download are required.

需要 Windows、已安装的 WebView2、Node.js 22.12+ 和仓库锁定的 Rust。先执行 `npm ci`；不需要下载生成引擎、CUDA 或 Playwright 浏览器。

```powershell
npm run test:e2e
npm run test:e2e -- --grep "ordinary images"
npm run test:e2e:debug -- --grep "cancel and restart"
npm run test:e2e:report
npx tsc -p tests/e2e/tsconfig.json
cargo test --manifest-path src-tauri/Cargo.toml --features e2e --lib testing::
```

For case discovery without starting the Windows application, run `npx playwright test --list --reporter=list`. Run the selected cases with `npm run test:e2e -- --grep "case name"`.

Use these scripts rather than opening `npx playwright test` directly: they build the debug-only application and own the isolated Vite process. `--list` lists tests without launching the application. One worker is intentional. Port 1429 must be free; CDP gets a separate available loopback port per application. The ordinary OOOSplat client may remain open.

Test binaries are `ooo-splat-e2e.exe` and `splatstudio-e2e.exe` in the existing Cargo target. They do not overwrite the ordinary development executables. The CLI test binary compiles the production CLI source directly, without a second lifecycle implementation.

请通过上述脚本启动，脚本负责测试构建与前端服务；不要直接启动 Playwright。首版串行执行，1429 端口须空闲。CDP 每个测试窗口使用独立的本机端口；正常客户端可保持打开。

## What runs / 验证链路

```text
Playwright → real Tauri WebView2 UI → real IPC/MCP/CLI
  → material admission/questions → TaskService → production recipe resolver
  → real command assembly → simulated process boundary
  → real SQLite/model/PLY validation → checkpoints → durable publication
```

Only external process execution, engine capabilities, hardware facts, sampling and native dialog results are substituted. The frontend does **not** fabricate analysis, task or recipe responses. Real tiny PNG/JPEG fixtures exercise image headers, Alpha/Mask preparation and orientation grouping. Video metadata/PTS and engine output artifacts come from the scenario. The mock does not emulate visual geometry or Gaussian optimization.

仅替换外部执行、运行库能力、硬件事实、资源采样和原生选择框。素材分析、任务创建、配方解析、候选校验与发布不 mock。图片为真实小型 PNG/JPEG；视频为身份夹具，元数据、PTS 和抽取图片由模拟器提供并通过原映射校验。模拟器不模拟视觉几何或训练效果。

`e2e` defaults off. Release builds with this feature are compile-time errors. A test binary refuses startup without a scenario, owner token and an owned directory under `.cache/e2e/runs/`. Unknown executables/operations fail closed; there is no real-engine fallback. Update checks, telemetry delivery and diagnostic uploads are disabled. CPU/GPU resource collectors are not started. Test IPC only reads the current scenario/call/task snapshot and releases declared barriers; it cannot accept an arbitrary recipe or filesystem path.

`e2e` 默认关闭且禁止 release 构建。测试程序必须具有用例目录、所有权令牌及启动前绑定的场景；未知引擎调用直接失败。测试模式关闭更新、埋点发送和诊断上传，不启动实际硬件采样。控制命令不能传入最终配方或任意文件路径。

Publication fault hooks run **after** a real durable Prepared/Committed transition, never instead of a validator or state write. Native file dialogs return declared paths; all subsequent backend checks remain real.

发布故障点位于真实 Prepared/Committed 写入之后，不替代校验或状态保存。选择框仅返回声明的路径，后续后端校验照常执行。

Scenarios can set `sharedJsonReaders: true` to keep an ordinary read-only handle open across each real JSON replacement. Alpha, reshoot and publication-recovery cases use this deterministic Windows contention regression. It neither pauses polling nor substitutes/retries a state write. Old readers retain the old complete snapshot; new opens must read the new complete record. Real non-delete-sharing locks and read-only targets must still fail safely.

`sharedJsonReaders: true` 会在真实 JSON 替换期间持续持有普通只读句柄，用于确定性复现 Windows 读写重叠。透明素材、补拍及发布恢复场景已启用；它不暂停轮询、不替代保存器、不增加重试。旧读者保留完整旧记录，新读者获得完整新记录；真实占用和只读目标仍须安全失败。

## Scenario contract / 场景规范

- `fixtures/native.ts`: per-case app lifecycle, ownership, dialog adapter and observation. No production resolver import.
- `specs/`: user actions and explicit assertions.
- `expected/recipes.ts`: independently reviewed constants for steps, growth, feature limits and Splat budgets. Current small ordinary fixture: 30 × 96×64 images, 32 authoritative sparse points; caps 6192/12385/15481. These are ceilings, not the mock's output count.
- `expected/contracts.ts`: assertions shared by positive cases and deliberate negative controls. Wrong steps, FPS, init files or duplicate Brush starts are rejected.
- `src-tauri/src/testing/`: bounded fixture generation and process adapters. Successful Brush writes 64 valid SH0 Gaussians but reports 128 in progress; final publication must independently validate/count the file.
- Scenario `faults[]`: operation/occurrence, output/failure and optional named barrier. Occurrences are persisted across restarts. Release barriers explicitly; do not replace synchronization with fixed sleeps.

预期值必须人工审阅，不能调用被测解析器生成。比较路径／随机 ID 时可规范化，不能删除素材关系、策略、初始化身份和次数。修改生产配方后，先说明变更原因，再审核基线；禁止自动更新期望以“让测试通过”。

The engine adapter must follow actual arguments (including export filename, selected GPU and source indices), not repair a broken command. Unsupported adapters fail until implemented explicitly. Geometry fixtures represent declared outcomes only; they do not prove real registration accuracy.

模拟器须遵循实际命令，不能纠正错误参数。新增命令必须显式实现，否则失败；小型几何成果仅代表声明结果，不证明真实注册率。

## Coverage / 覆盖

Native scenarios cover ordinary images/videos and ERP at three qualities, mixed ERP recipes, exact frame mapping, manual per-video Bridge budgets, 29.97/VFR/nonzero PTS, short/all-frame inputs, opaque/transparent Alpha, JPEG, orientation groups, incompatible dimensions, 100/101-image questions, selected CUDA/non-CUDA GPUs, four reshoot answers, Warm/Cold fallback and export failures, zero new registration, OOM authorization, cancellation/restart, candidate/Commit recovery, plus representative real MCP/CLI calls.

Additional scenarios cover saved concrete-GPU preferences, durable partial reshoot adoption and geometric versus operational initial-Mapper failures. There are 44 scenarios including one assertion negative-control test.

生命周期扩展保留上述 44 例，总计 268 例。展开清单、公共停点及定向组合见 [LIFECYCLE_MATRIX.md](LIFECYCLE_MATRIX.md)，真实缺陷与复现证据见 [ISSUES.md](ISSUES.md)。新增覆盖取消／重启／强杀继续、真实部分成果、连续补拍、逐来源预算与 PTS、队列及旧运行隔离。

```powershell
node scripts/verify-e2e-lifecycle.mjs checks
node scripts/verify-e2e-lifecycle.mjs native-rounds
```

`checks` 执行前端、普通／E2E Rust 全目标测试、构建、格式、文档与两种 Clippy。`native-rounds` 在相同代码摘要上连续执行三轮完整套件，逐轮保存 JSON/HTML、日志、案例清单、磁盘空间及二进制摘要；失败仍继续收集后续完整轮次，不增加重跑。两者均检查磁盘，空间不足报告阻断，不清理用户缓存。

新增场景 `barriers[]` 声明 operation、boundary、task、source、occurrence；支持 entered、partial、written、checkpoint。账本先同步持久化再暴露命中，释放须同时匹配 hitId/taskId/runId。重启复用本案例目录，已消费屏障和引擎故障不能再次获得。`lateEvent` 只能投递预声明并捕获的真实旧运行回调。全局操作的 sourceId 为 null，具体来源操作记录实际 material ID。所有页面任务操作按实际 taskId，创建前按草稿 ID 定位。

模拟 Brush 每个任务／训练尝试输出不同且确定的合法 64 点 PLY；匹配使用真实 pair 文件，重建维持传入模型固定相机和位姿。未知操作或参数失败，不退回真实进程。`boundary-*.json`、`lifecycle.json` 和 `invocations.json` 连同最终任务／项目文件保留在隔离目录。详尽结果见 [VERIFICATION.md](VERIFICATION.md)；旧版 44 例三轮通过不能代表扩展套件通过。

原有 Rust／前端测试仍负责大量数值边界、旧快照、队列和迟到事件组合。首版并未将所有历史项目、旧策略、队列竞态和多卡设置组合重复成原生窗口用例；继续扩展时添加独立场景，不用模拟测试宣称这些实机验收全部完成。

## Evidence, isolation and CI / 报告、隔离与 CI

- `.cache/e2e/artifacts/<run>/report/`: HTML report; `results/` alongside it contains traces, screenshots and actual execution snapshots on failure. Each invocation has a unique directory, so later tests never overwrite failure evidence. `latest.json` points the report command to the latest invocation.
- `.cache/e2e/runs/<owner>/`: isolated settings/tasks/projects/locks/WebView/fixtures plus `scenario.json`, `invocations.json`, `app-output.log` and ownership marker. Restart cases reuse only their own directory.
- Successful cases remove only their owned WebView cache. Small recipes/states remain for inspection; failures retain local evidence. No user project/source/cache cleanup occurs. Browser downloads, if explicitly requested later, use `.cache/e2e/browsers/`.
- Only children launched by this framework are stopped. Occupied ports are reported, never cleared by killing unrelated processes.
- Manual workflow: `.github/workflows/playwright-e2e.yml`. No PR trigger, required check, engine download or installer packaging. Hosted Windows/WebView2 execution must be verified by a future manual run; local success is not remote CI evidence.

通过只表示**界面→配方→控制流→成果校验／发布**符合断言，不证明真实 COLMAP/Brush、画质、注册率、PSNR、显存或线上成功率。

Passing means the asserted **UI → recipe → control flow → validation/publication** contract holds. It is not real-engine, rendering-quality, registration-rate, PSNR, VRAM or production-success-rate certification.
