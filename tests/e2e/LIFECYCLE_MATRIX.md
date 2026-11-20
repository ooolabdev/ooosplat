# 生命周期、补拍与多素材矩阵

实施分支为现有 `test/playwright-recipe-e2e`，本地基线 `5f1df30`。不更新远端、不提交、不推送。初次矩阵扩展的业务缺陷只记录，严格失败计入历史报告；后续单独授权的 B1、B2 修复及六例／二十例定向回归结果见 [VERIFICATION.md](VERIFICATION.md)，B3、B4 尚未修复。原生案例总数保持 268，B2 仅补强既有案例的 Mask 内容与权限断言。

## 案例分布

| 文件 | 数量 | 验证内容 |
| --- | ---: | --- |
| 原有 specs | 44 | 原生入口、配方、素材准入、失败与发布恢复，全部保留 |
| lifecycle-matrix.spec.ts | 170 | 普通／补拍主矩阵 166；冻结祖先准备恢复 3；任务策略持久化最小复现 1 |
| repeated-reshoot.spec.ts | 13 | 连续补拍、兄弟派生、路线转换、额度及采用结果 |
| material-combinations.spec.ts | 34 | 多来源采样、混合素材、ERP、Bridge 与准入编辑 |
| lifecycle-isolation.spec.ts | 6 | 显卡保持、迟到事件、重复操作与自动队列 |
| negative-controls.spec.ts 新增项 | 1 | 错误来源、预算、初始化、PTS、半成品的断言负例 |
| 合计 | 268 | 单 worker，`retries: 0`，无新增跳过或预期失败 |

每次验证自动保存 `case-manifest.json`，包含全部展开后的案例名称、文件、行号与代码摘要，作为具体案例清单。参数化用例在列表中各自计数。直接枚举（不启动窗口）可用 `node node_modules/@playwright/test/cli.js test --config=playwright.config.ts --list`；实际运行仍使用仓库启动脚本。

## 公共生命周期

普通任务使用两段参数不同的视频与透明横竖拍图片集。补拍先真实完成 A，再从冻结 A 派生，新增相同三组素材。以下 19 个完成停点分别执行取消后继续、取消后重启继续、强制结束后重启继续，两类任务共 114 例：

| 阶段 | 屏障操作 | 检查内容 |
| --- | --- | --- |
| 创建、配置 | creationIntent、project、configuration | 任务／运行／项目身份、创建意图、保存策略 |
| 素材准备 | sourcePrepared、prepared | 本轮具体来源、冻结计划与有效准备成果 |
| SfM | features、matching、reconstruction | 语义有效的特征／匹配／权威模型复用 |
| 训练准备、候选 | trainingDataset、candidate、candidateSummary | 数据集身份、可信成果；候选恢复不再训练 |
| 发布 | publication:prepared、publication:outputInstalled、publication:committed | 真实事务状态及实际安装文件 |
| 视图对账 | projectState、projectMetadata、publication:projectViewsReconciled、taskView、publication:reconciled | 项目状态、元数据、任务终态与事务最终对账 |

另有 26 个执行窗口 × 两类任务 = 52 例：创建与配置写入前的真实成果；素材分析 entered／written；sourcePlan written；单来源及联合 prepared written；图片／Mask partial；视频提取 entered／written；特征 partial／written；批内及跨来源匹配 partial、联合 matching written；模型 partial、Mapper written、重建校验 written；训练数据 partial／written；Brush partial／written；候选及候选摘要 written；安装文件后 OutputInstalled 保存前。执行中 partial 采用取消后直接继续，完整成果 written 通常采用强杀重启，避免额外笛卡尔积。

`sourcePrepared` 主矩阵明确绑定 `first.mp4`；written 窗口绑定 `alpha`。补拍内部冻结祖先 `__source` 的三种中断另列严格回归，不与本轮新增素材混淆。训练中断检查同一配方重新启动和初始化摘要，不声称优化器断点恢复。Commit 后接受原事务完成与对账，不强制 Cancelled。

## 连续补拍与隔离

| 定向案例 | 独立断言 |
| --- | --- |
| A→B→C→D；同 A 派生 B 与 C | 每轮 64 点合法 PLY 内容不同；实际训练初始化等于当前父项目快照摘要；祖先 project/state/final/work（含 DB、模型、图片、Mask、快照）摘要不变 |
| Warm→Warm；Warm→Cold→Warm；全量后增量 | 冻结父来源、实际路线和训练 seed；不借用 A 的旧初始化 |
| 首次恢复后补拍；同一补拍三次中断 | 新运行历史、素材与初始化保持 |
| OOM 降点后继续及第二次 OOM | 原降点参数与已用额度保留；额度耗尽必须失败 |
| Warm→Cold 已持久化后继续 | fallback 数量保留；Cold 不带 Gaussian 初始化；完成后可下一轮 Warm |
| 部分不采用→发布恢复→下一轮 | sourceAdoption、警告及当前父快照正确 |
| 部分低注册／不连接；全部新增零注册 | 逐来源统计；全部零注册不得训练或发布旧场景 |
| GPU 1；迟到事件；重复开始／继续／取消 | 实际参数、真实旧回调、STALE_RUN_ID、运行次数与进度隔离 |
| 禁用自动补救后取消、重启继续，再发生 OOM | 仍只允许两次训练执行；原禁用策略保存在请求与项目，任务持久化记录也必须相同 |
| 自动队列与重排 | 按真实草稿／任务 ID 定位；前驱结束才启动；取消禁用队列，继续不夹带等待任务 |

## 多素材

| 组合／停点 | 重点证据 |
| --- | --- |
| 三视频＋横竖拍图片 | 默认 29.97/VFR 非零 PTS、手动 7FPS、全部 15FPS；独立采样索引表、实际 `-i`、映射、来源 ID、相机组、配对 |
| 多尺寸图片、视频＋图片 | 来源身份和相机归属，规格隔离 |
| 普通＋ERP 的视频／图片交叉 | 实际配方、训练步数、六面准备 |
| 透明＋不透明、四组、不同目录同名文件 | 来源可区分、输出名称不冲突、透明重建参数 |
| 两个 151 张图片集 | 分别回答顺序问题；逐来源 loop 检索 50、跨来源检索 30，独立输入列表 |
| ERP 视频／图片 × 普通／补拍 | partial 三种中断，written 强杀；首面真实写入、后续六面重建，共 16 例 |
| 第一组完成、第二段 partial、Bridge 决策／发布 | 逐来源冻结 35 初始／42 最大；补帧不转移、不刷新；已发布 Bridge 不重执行引擎 |
| 重复路径、成员重叠、不支持格式、删除再添加 | 真实准入阻止任务；新顺序保留各来源关系 |

预期值位于 `expected/recipes.ts` 和 `expected/lifecycle.ts`，不调用生产解析器。模拟匹配遵守实际 pair 文件，SQLite 分批提交；重建按来源限制注册，固定输入模型的相机和位姿。未知操作、未知或重复参数直接失败。

## 屏障与证据约束

屏障绑定真实 taskId、runId、sourceId／全局空来源、操作与命中次数。账本在暴露屏障前原子写入并同步；一次性故障随执行记录保存，重启将未完成命中标为 interrupted。释放必须匹配当前命中身份；IPC 不接收任意路径、配方或完成状态。真实图片／Mask／ERP／SQLite／PLY 文件在屏障前已写入，不修改生产完成标记。

复用边界遵从现有信任语义：只有已提交且通过校验的联合 prepared manifest 才证明像素可直接复用；此前逐来源记录作为恢复计划，允许重新抽取，但来源路径、冻结选帧及预算必须保持。原 `normalize_collection`／`restore_frozen_video_source` 仍负责此判断。每次点击补拍前保护父项目，teardown 在停止应用后核对全部祖先并保存 `ancestor-checks.json`；业务用例提前失败也执行该检查。

`boundary-*.json` 保存命中快照，`lifecycle.json` 保存消费记录，`invocations.json` 保存实际参数、来源、运行、初始化 SHA-256 和匹配输入列表。每轮完整套件保存独立 HTML/JSON、日志、失败 screenshot/trace、任务与项目原始文件。验证脚本检查磁盘，空间不足停止且不清理用户素材或缓存。

真实引擎兼容性、几何注册质量、画质、PSNR、显存、OOM 硬件行为与远端 CI 仍须实机验收。模拟通过仅证明上述控制流与持久化合同。
