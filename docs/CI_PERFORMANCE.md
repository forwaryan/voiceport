# CI 耗时与优化

## 实测基线

2026-11-16 读取现有 `macOS CI` 最近 12 次已完成且成功的运行。按 GitHub 的 `createdAt → updatedAt` 计算，总耗时中位数为 **117 秒**，范围 **109–159 秒**；该口径包含排队和收尾时间，不等于纯编译时间。

重点样本：

| 运行 | 变更/分支 | 总耗时 | job 耗时 | 缓存恢复 | 原合并检查步骤 |
| --- | --- | ---: | ---: | ---: | ---: |
| [43890179432](https://github.com/forwaryan/voiceport/actions/runs/43890179432) | README 界面截图 PR | 126 秒 | 122 秒 | 20 秒 | 84 秒 |
| [43873748885](https://github.com/forwaryan/voiceport/actions/runs/43873748885) | main | 159 秒 | 155 秒 | 18 秒 | 120 秒 |
| [43890895017](https://github.com/forwaryan/voiceport/actions/runs/43890895017) | README 截图合入 main | 157 秒 | 153 秒 | 17 秒 | 122 秒 |

第一条运行的完整日志显示：恢复缓存约 **903 MB / 946,700,776 字节**；Rust test 编译 12.74 秒、clippy 4.05 秒、release 编译 9.02 秒，打包测试 40.769 秒。缓存精确命中后打印 `not saving cache`，因为原 key 只随平台、工具链和 Cargo.lock 改变，后续同 key 不会更新工作区产物。不能把命中旧 target 缓存等同于当前源码无需重编译。

这说明当前最明显的浪费是文档 PR 执行整条原生构建链，以及恢复整个 target 目录的成本。现有约两分钟的基线也不足以直接采用其他项目宣传的十倍加速数字。

## 已实施方案

1. **README / 界面图库 PR 快速路径。** 原 `Rust and packaging` job 保持存在并执行 CI 范围回归。仅 `README.md`、`docs/images/` 内的 Markdown 和常见图片文件可以走快速路径，另执行 `git diff --check` 并在运行摘要说明实际范围。其他 docs 默认全量，尤其 `docs/TRANSCRIPTION_PLUGINS.md` 是 CLI 的 `include_str!` 编译输入，修改或删除均须编译与测试。未使用 workflow 级 `paths-ignore`，避免必需检查一直 pending。
2. **完整检查的触发范围。** 每次 main push、任何代码/测试/脚本/工作流/配置/资源变更、空差异、未知事件或范围无法核实，仍执行全部原有 Node、Rust、Python、hook、release 与真实 bundle 测试。文档 PR 的快速结果不表示运行过 Rust 或打包测试；main 的全量结果另行接受。
3. **核对实际测试树。** `scripts/ci_scope.py` 比较事件中的 base SHA 与实际 checkout HEAD，通常后者是 GitHub 合成的 PR merge commit。checkout 深度设为 2，在其临时认证仍可用时获取 PR merge 的父提交，避免私有仓库关闭 persist-credentials 后无法再获取常见 base。若仍缺少 base，只尝试获取精确 base；获取失败走全量检查。使用 NUL 分隔文件名并关闭 rename detection，源码移动到 docs 仍会触发完整检查。
4. **精简 Rust 缓存。** 使用固定提交的 `Swatinem/rust-cache` 2.9.3。缓存键包含编译器、Cargo manifests/lockfiles 和相关环境，保存前清除工作区产物、增量数据与过时依赖。不缓存 Cargo bin，不跳过 `cargo fetch --locked`。依赖预取之后所有原有 Rust/打包检查继续离线执行。该 action 自动设置 `CARGO_INCREMENTAL=0`；仍会编译本工作区源码。
5. **取消过时 PR 运行。** 相同 PR 的新提交取消之前运行；main 用唯一 `github.run_id` 分组，不启用 cancel-in-progress。仅设置 false 仍会替换同组 pending run，不能保证每个 main 提交接受完整检查，因此 main 不共用并发组。被取消的旧运行不能当作验收通过。
6. **暴露实际耗时。** 将原本合在一个 shell 步骤中的 Rust/streaming 检查、release build、loop/hooks 和 packaging 拆成独立 Actions steps，便于下一轮从真实计时判断是否值得并行或进一步优化。

没有引入付费 runner、外部远程编译缓存或新凭据。没有把本地机器的测试速度当作 GitHub runner 的新速度。首次新缓存为空时可能较慢，最终效果需要分别记录冷缓存和热缓存运行。

## 调研依据与取舍

- [Swatinem/rust-cache 原始说明与固定版本](https://github.com/Swatinem/rust-cache/blob/d1527c8f67ef9af357290475d4af4b0fab8f12e0/README.md)：按编译器和依赖键控、清理工作区/增量产物，以及 macOS 缓存注意事项。采用依赖缓存方案，不声称它解决所有源码重编译。
- [GitHub actions/cache 原始说明](https://github.com/actions/cache#usage)：精确 key 命中后不创建新缓存；默认分支缓存可供其他分支恢复。更换方案后首次运行会创建独立缓存，旧缓存不手工删除。
- [GitHub workflow 并发文档](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency)：按 PR 分组并取消过时运行，不把取消当成功。
- [GitHub 必需检查排查文档](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks#handling-skipped-but-required-checks)：workflow 因路径筛选而不触发可能使必需检查停在 pending；保留原 job，显式执行适用范围。

公开讨论还常提到 sccache、自托管持久 runner、付费高速 runner 和测试分片。这些方案需要结合源码编译占比、缓存传输成本和 runner 启动成本另做实验。当前先消除已证实的文档构建浪费、缩小缓存，并保留下一轮可分析的分步骤计时。

本轮搜索了 Twitter/X 的 Rust、GitHub Actions、cache 和路径过滤相关关键词，没有获得可核实的原始帖子；未将第三方搜索摘要包装为推特实证。GitHub 原始 README 已通过 `gh api` 读取；文档站直接抓取受限，文档链接供后续核对，不声称本轮已逐段读取这两份网页。

## 验证与远端接受

本地 18 项行为回归通过，覆盖文档/源码混合、源码删除、源码重命名至 docs、异常文件名、配置与测试变化、main push、未知事件、空差异、缺失/非法 base、缺失 event、失败 base fetch、真实浅克隆成功获取精确 base、实际 merge checkout 引入的 base 变化、嵌入 CLI 的文档修改/删除，以及脚本入口与文档 whitespace 检查的通过和失败路径。另解析 workflow YAML，对照旧版本核对触发事件、只读权限、job 名称、runner 和所有原有检查命令均保留，并核对全量检查条件。

独立 worktree 基于 `13193d5d9f1d315c4c57bcf39a6adb78e8707460`，只包含本次四个 CI 文件的改动。其本地 Rust test/fmt/clippy、release build 和 64 项真实打包测试均通过；19 项 streaming、41 项 QQ Python、28 项 loop、202 项 hook、18 项 CI scope 回归及 QQ Node 入口通过。Rust 套件保留原有 3 项 ignored 测试，未将它们称为已执行通过。本机使用已安装的工具链别名 `1.95`，`rustc -vV` 证实其 release 为 1.95.0、commit 为 `59807616e1fa2540724bfbac14d7976d7e4a3860`。前次指定 `1.95.0` 的下载等待已停止，没有把它当作测试结果。独立复核未发现当前范围内剩余具体问题。

GitHub 上的新速度、缓存大小、required checks 与冷/热缓存行为仍需发布本分支后实测。本地耗时不用于声明远端提速。

远端验证应分别记录代码变更 PR、后续文档 PR 和 main push 的精确 SHA、run/attempt、各 step 实际 conclusion/耗时及缓存大小。代码变更 PR 和 main 必须执行完整测试；文档 PR 必须执行范围测试和 diff 检查，并明确显示文档范围。不能将预期节省的时间写成已达到的性能提升。

## 十次测试的统计口径

用户要求在最终候选版本上实际测试十次。计划固定同一 PR source SHA 和既有 run，执行首次运行加九次明确的性能采样 rerun，顺序执行，逐次保存 run/attempt、jobs、完整日志和检查执行情况。这里的 rerun 是用户要求的采样，不是掩盖失败的重试；任一次出现失败或未执行全量步骤，保留该样本并停止诊断，不删掉异常值凑十次成功。

主要比较指标采用 GitHub `job.completed_at - job.started_at`，避免 rerun 复用原 `created_at` 导致从第二次起总耗时算错。旧版本十二次样本在此口径下为：平均 **118.92 秒**、中位数 **113 秒**、范围 **106–155 秒**。前文 117 秒为含排队/收尾的 workflow 总耗时，两种口径不能混用。

十次结果报告每次耗时、结论和缓存情况，并给出整体平均/中位数/范围/样本标准差；另列首轮和后九轮，按实际缓存日志识别冷/热缓存。README 快速路径不混入全量 CI 统计。历史样本来自不同提交，和新版本比较只能作为工程参考，不能冒充严格配对的因果实验。

## 后续是否仍可能变慢

仍有可能。工具链/依赖变化会正常触发冷缓存；GitHub runner 的 CPU、磁盘和排队存在波动；测试和打包资源增长也会增加执行时间。缓存下载、解压、保存的成本如果超过省下的编译时间，缓存方案本身就需要重新评估。

本次措施降低重复工作的成本，并使原因可见，不能承诺永久不变慢。若后续构建开始读取 README 或图库文件，须同时收紧快速路径并添加反例；无法证实范围时继续回退到完整检查。后续提速应继续依据步骤耗时和实际输入关系，不能通过跳过需要的检查获得漂亮数字。
