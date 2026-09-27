# T4 审计报告（第四轮）：Tauri 壳层 / 配置 / 术语表 / 脚本与 CI

- 审计者：`auditor-shell`（共享任务 task-4）
- 基线 revision：`d8295da`（= 已发布 tag v1.1.5），起始工作树 hash `ad513e30c1e335f2a1f9cd669f6aa7ed`
- 环境：Linux（nix shell）+ cargo 1.95 / bun 1.3.13 / python3 3.13 / yq 4.53；本机装有 webkit2gtk，**可以真编并真跑 `src-tauri`**
- 写范围：`src-tauri/**`、`crates/bg3-translate-core/src/{config.rs,lib.rs,glossary/**}`、`scripts/**`、`.github/**`、`README.md` 与 `docs/` 相关段落、本文件
- 本轮并行：另外三位审计者（formats / engine / web）+ 独立 verifier。审计中期 `crates/` 里他人半成品导致的编译失败按实标注（见 §7）

> 所有结论都附「实际执行过的命令 + 真实输出」。**Windows 分支与 PowerShell 段本机跑不了，一律标「无法验证」**（§6）。
> 术语表 / 配置 / 脚本这些纯逻辑项，全部用真实执行的测试或变异实验钉住，不用静态推理代替证据。

---

## §0 摘要

| 项 | 结果 |
| --- | --- |
| 缺陷（已修） | **8 条**：中 4 / 低 4 |
| 已确认未修（含理由） | 5 条（含 1 条「真实数据证明不该修」） |
| 已排查但证伪 | 3 条（含 1 条真实数据上的零差异） |
| 删除的测试 / CI 步骤 | **0** |
| 新增/修改的测试 | **17 条**（core 14：config 8 + glossary/store 6；命令层 3：entries / terminology / translate 各 1） |
| 门禁 | `bash scripts/verify.sh` **6/6 通过**（7.4s）；Rust 413+16+11+8+8 = **456**；前端 21 文件 / **208** 用例；冻结 hash `f95b2364…`（连跑两次一致） |
| 命令层测试 | `cargo test -p bg3-translate --lib` → **19 passed**（基线 16） |
| 独立可复现 | 每条修复都给「修复前红 → 修复后绿」原始输出 + 变异实验；脚本/CI 用变异矩阵（M1–M9 / N1–N2） |

**一句话结论**：壳层这一轮的重点不是「再找几条路径校验」，而是**「防线可以被静默关掉」这一类问题**——
命令层的半截译文写回闸门、术语表的并发读改写、术语表删除的 first-match 语义、原子写的临时文件撞名、CI 只钉默认门禁清单、
契约脚本漏检「定义了没注册的命令」，六条都属于同一模式：**现状看起来没坏，但少了任何一道都不会有人知道**。
每一道都配了会自动变红的回归测试或变异实验。

---

## §1 缺陷总表（编号 / 级别 / 现象 / 后果 / 证据 / 修法 / 回归测试 / 变异实验 / 残留风险）

| 编号 | 级别 | 现象 | 后果 | 证据（命令 + 输出） | 修法 | 回归测试名 | 变异实验 | 残留风险 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| S-01 | 中 | `config::write_atomic` 的临时文件名只有 `tmp<pid>`（`with_extension` 还会吃掉扩展名），同进程并发写算出**同一个**临时文件 | 两个写者互相截断（先完成的一方 `rename` 走之后，另一方 `rename` 直接 ENOENT），原子性失效 | `cargo test -p bg3-translate-core --lib config::tests::`（还原旧实现）→ `assertion left != right failed`；并发用例 8 个写者里 **5 个** `Io(Os { code: 2, kind: NotFound })` | `temp_path_for`：主名 + pid + 进程内 `AtomicU64` 序号 | `atomic_write_temp_path_is_unique_per_call`、`concurrent_atomic_writes_all_succeed_with_whole_content` | 把 `temp_path_for` 还原成 `tmp<pid>` → 2 条用例立刻红 | 跨进程仍可能撞名（不同 pid，实际不会同目录写同一文件）；不 fsync，掉电语义未变 |
| S-02 | 中 | 术语表 `add/update/delete/reset/import` 全是「读盘 → 改 → 写盘」，命令层**没有任何串行化** | 并发调用互相覆盖：用户删 A 再删 B，B 的表覆盖 A 的表，**A 的删除静默丢失**（刷新后术语又回来） | 还原旧实现跑 `concurrent_mutations_are_serialized_and_lose_nothing` → 连跑 3 次 `left: 2 / right: 1`；跳过串行断言再看数据 → `并发写把 TermA 丢了：["TermB"]` | `terminology.rs` 加进程内 `Mutex`（`glossary_write_guard`），锁覆盖**整段**读改写；`reset`/`import` 同样进锁 | `concurrent_mutations_are_serialized_and_lose_nothing` | 删掉 `mutate_in` 里的 `_guard` → 立刻红；还原后 19/19 绿 | 多进程/多实例并发仍无防护（单实例桌面应用，暂不需要）；跨命令的「读改写」语义仍需前端配合 |
| S-03 | 中 | 命令层对 `translating` 条目不设防：core 只在 `status == error` 时退回原文，`translating` 的非空 `target` 会被原样写进产物（第三轮列为纵深防御建议） | 前端一旦有 bug / 调用方是旧前端，**半截流式文本静默进 PAK** | `cargo test -p bg3-translate --lib write_back_downgrades` → `半截流式文本不得写进产物: <content contentuid="c1" version="1">半截译文</content>` | `entries.rs::writable_entries`：写回前把 `translating` 条目清空 `target`、退回 `pending`、清 `error`，并打 warn 日志（与前端 `toWritableEntry` 逐字段一致） | `write_back_downgrades_entries_still_translating` | 删掉 `let writable = writable_entries(entries);` 那一行 → 红 | 语义是**净化**不是报错：界面上的半截文本仍在内存里，用户看到的是「这条没保存」而不是错误弹窗（与前端行为一致，已写入 ARCHITECTURE） |
| S-04 | 低 | `translate.rs` 把前端可控 `work_dir` **原样**写进日志（第三轮 F-07 明确遗留） | 含换行的 `work_dir` 可在日志里伪造一整行记录，排查时被日志带偏（日志同时落盘） | `cargo test -p bg3-translate --lib translate_request_log` → `前端可控字符串不得在日志里引入换行（可伪造日志行）: "翻译请求：work_dir=C:\\mods\\x\n[INFO] 翻译结束…"` | 抽出 `log_translate_request`，用 `{work_dir:?}`（Debug 转义换行与不可见字符） | `translate_request_log_cannot_be_forged_with_newlines`（自带捕获 logger，断言只筛含 `work_dir=` 的记录，不会被并行用例的日志打脆） | `{:?}` 改回 `{}` → 立刻红 | 只防御「换行伪造」；日志里的其它前端可控文本（条目数等）本来就是数值 |
| S-05 | 低 | 设置/术语表损坏时**静默重置且不留备份**；更糟的是**非 UTF-8 文件直接报错**（同一类损坏两种行为） | 用户重填一次设置、或随便改一条术语触发保存，原文件被静默覆盖（术语表可能是导入的两万条官方术语）；非 UTF-8 时 `load_llm_settings` 直接失败，设置面板打不开、界面上无法自救 | RED：`损坏的文件应被备份到 …/settings.json.corrupt`、`非 UTF-8 不应让设置加载直接失败: Io(Error { kind: InvalidData …})`、`应备份到 …/glossary.json.corrupt`、`非 UTF-8 不应让术语表加载失败: Io(InvalidData)` | 新增 `config::quarantine_corrupt_file`：读失败/解析失败一律「告警 + 把原文件**改名**成 `<名>.corrupt`（已存在则 `.corrupt.2`…）+ 回退默认值/种子」；`Glossary::load_from` 同样接上 | `corrupted_settings_file_is_kept_as_backup`、`quarantine_does_not_overwrite_an_existing_backup`、`non_utf8_settings_file_falls_back_to_defaults`、`corrupted_file_is_quarantined_before_seed_takes_over`、`non_utf8_glossary_falls_back_to_seed` | 去掉两处 `quarantine_corrupt_file(&path)` 调用 → 备份用例红；`read_to_string(..)?` 改回直接 `?` → 非 UTF-8 用例红 | 备份是尽力而为（改名失败只记日志）；10 份备份都存在时回退覆盖第一份（极端情况，代码里有注释）；**用户在界面上仍只看到默认值**，需要自己去数据目录捞 `.corrupt`（README 已写明） |
| S-06 | 低 | `scripts/check_ipc_contract.py` 有两处漏检：① `#[tauri::command]` 定义了但没进 `generate_handler!`；② 命令**返回类型**与文档命令表不一致 | ① 前端一调就「命令不存在」，而只从注册表出发的检查看不见它；② 文档腐烂没人管，按文档写调用方的人拿到 undefined | 变异矩阵见 §3：M1（未注册命令）旧脚本 exit=0 / 新脚本 exit=1；M2（`repack_mod` 文档返回 `void`→`Glossary`）旧 0 / 新 1；M3（返回列清空）旧 0 / 新 1；M4（后端返回类型改掉）旧 0 / 新 1 | 新增 `rust_command_signatures` / `normalize_return_type` / `doc_command_table`，加两项检查；**原有 4 类检查一行未删**（M5–M9 证明仍然会红） | 脚本自检（`IPC_EXIT=0` + 变异矩阵）| M1–M4「改前漏检、改后 exit=1」；M5–M9 两版都 exit=1（未放宽） | 返回类型只做**形状**归一（`Result<T>`/`Vec<T>`/`()`），不校验泛型参数内部的等价性；解析仍是正则，写法超出模式会明确报错而不是静默通过 |
| S-07 | 中 | CI `meta` job 只断言 `verify.sh --list`（**默认**清单），而 `core`/`web` job 实际跑的是 `--core-only --no-ipc` / `--web-only --no-ipc` | 给某道门禁套一个 `if [ "$RUN_IPC" = 1 ]`，默认清单逐字符不变，但某个 CI job 从此**静默少跑一步**（第三轮 §3.4 的同一类问题） | 变异 N1（`web-build` 只在 `RUN_IPC=1` 时进清单）旧步骤 exit=0 / 新步骤 exit=1；N2（`core-test` 同型）旧 0 / 新 1；未变异时提取 CI 步骤原文真实执行 → exit=0（§3） | 在同一个 step 里加 `check_mode`，把两个模式的门禁清单也逐字符断言（**只加断言，没删任何步骤**） | CI 步骤原文本地执行（`yq -r '.jobs.meta.steps[]…' > /tmp/r4-meta-step.sh && bash /tmp/r4-meta-step.sh`） | 见上（旧步骤对新变异盲、新步骤会红） | 断言文本仍与 `verify.sh` 的实现耦合：新增门禁时必须同步改三处（默认 + core + web），这是有意的摩擦；GitHub Actions 真实运行未验证 |
| S-08 | 低 | 文档把系统模式写成 `%APPDATA%\bg3-translate`，实际是 `%APPDATA%\bg3-translate\config`（`directories` v6 在 Windows 上多拼一层 `config`）；README 还漏写了 `tauri-shell` job 已真正执行命令层测试 | Windows 用户按文档路径去找 `settings.json` **找不到**；读者以为命令层回归测试没进 CI | 本机运行时 `system_data_dir=/home/jason/.config/bg3-translate`（Linux 侧一致）；Windows 侧证据是依赖源码：`directories-6.0.0/src/win.rs:75 let config_dir = app_data_roaming.join("config");` 与 `src/lib.rs` 的 `ProjectDirs::config_dir` 文档表（`{FOLDERID_RoamingAppData}\_project_path_\config`） | 改 README / ARCHITECTURE / `config.rs` 头部注释三处；新增形状断言把两个平台都钉住 | `system_data_dir_shape_matches_documented_paths`（Linux 分支已跑；Windows 分支见 §6） | 把 `APP_NAME` 改成别的、或把 `ProjectDirs` 换成别的 API → 断言红 | Windows 分支本机执行不了（§6）；`directories` 升级到 v7 若有变化会由该用例在 Windows CI 上暴露 |
| S-09 | 中 | `Glossary::delete` 的 official 检查只看 `find()` **第一条**，删除却用 `retain` 删**全部**同 source | ① 两条同 source 的用户术语点一次删除**两条都没了**；② user 在前、official 在后时检查被绕过，**官方条目被连带删除**（「官方术语不可删除」这条不变量失效） | RED：`删一次只能少一条: []`（`left: 0 / right: 1`）、`同 source 里有官方条目必须拒绝: ()`（旧实现返回 `Ok`） | official 检查改成「**任意**一条同 source 是 official 就拒绝」；删除只 `remove` **第一条**匹配（与 `update` 的 first-match 一致） | `delete_removes_only_one_of_two_duplicate_user_entries`、`delete_refuses_when_a_later_duplicate_is_official`、`duplicate_source_state_is_reachable_through_public_api` | M-A 全还原 → 3 条全红；M-B 只去掉「只删一条」→ ① 红；M-C 只去掉「任意 official」→ ②③ 红 | `update` 仍是 first-match 语义（同 source 多条时只能改到第一条），彻底解决要改成按稳定 id 定位（动 IPC 契约，backlog） |

---

## §2 逐条明细（关键原始输出）

### S-01 原子写临时文件撞名（`config.rs`）

修复前（把 `temp_path_for` 临时还原成旧实现）：

```
$ cargo test -p bg3-translate-core --lib config::tests::
test config::tests::atomic_write_temp_path_is_unique_per_call ... FAILED
test config::tests::concurrent_atomic_writes_all_succeed_with_whole_content ... FAILED
---- atomic_write_temp_path_is_unique_per_call stdout ----
  assertion `left != right` failed: 同一次进程内的两次写入不能共用同一个临时文件
---- concurrent_atomic_writes_all_succeed_with_whole_content stdout ----
thread '<unnamed>' panicked at crates/bg3-translate-core/src/config.rs:607:50:
并发原子写不应失败: Io(Os { code: 2, kind: NotFound, message: "No such file or directory" })   ← 共 5 个写者
test result: FAILED. 17 passed; 2 failed
```

修复后：

```
$ cargo test -p bg3-translate-core --lib config::tests::
test config::tests::atomic_write_temp_path_is_unique_per_call ... ok
test config::tests::concurrent_atomic_writes_all_succeed_with_whole_content ... ok
test result: ok. 19 passed; 0 failed
```

**真实可达性**（lead 直接点名的问题）：仓库里只有两个调用点 —— `config::save_to`（`settings.json`）与
`glossary::save_to`（`glossary.json`），**主名不同**，所以「`a.xml` / `a.loca` / `a.slx` 同主名撞车」这一半在当前调用点
不会被触发；但「同一路径并发写」这一半是**可达的**：`config::write_atomic` 是公开 API，而
`terminology::mutate` 的读改写（S-02）与设置保存都跑在各自的 `spawn_blocking` 上，两个并发 mutation
就会同时走 `glossary.json` 的原子写 —— 上面那 5 个 ENOENT 就是同一进程 8 个写者的真实结果。
所以：**修**（3 行改动 + 2 条测试），并且它同时是 S-02 那个修复的前置条件。

### S-02 术语表并发读改写丢数据（`src-tauri/src/commands/terminology.rs`）

可达性证据（前端）：`GlossaryPanel.tsx` 的「保存」有 `saving` 守卫，但

```
const onDelete = async (source: string) => {
    setBusySource(source);            // ← 只按**行**置忙
    const g = await deleteGlossaryEntry(source);
```

`onReset` / `onImport` 更是没有任何守卫 ⇒「先删 A 再删 B」两次命令可以同时在途。
修复前（去掉 `mutate_in` 里的 `_guard`）：

```
$ cargo test -p bg3-translate --lib concurrent_mutations        # 连跑 3 次，结果相同
thread '…concurrent_mutations_are_serialized_and_lose_nothing' panicked:
assertion `left == right` failed: 「读盘 → 改 → 写盘」必须在写锁内串行，否则会互相覆盖
  left: 2
 right: 1
test result: FAILED. 0 passed; 1 failed
```

把串行断言临时跳过，直接看数据：

```
并发写把 TermA 丢了：["TermB"]
```

修复后：

```
test commands::terminology::tests::concurrent_mutations_are_serialized_and_lose_nothing ... ok
test result: ok. 19 passed; 0 failed
```

副作用（正面）：`Glossary::load()` / `save()` 的调用点从「隐式走当前数据目录」变成
`data_dir()` + `load_from/save_to`，语义不变（`load()` 本来就是 `load_from(&data_dir()?.path)`），
换来的是 `mutate_in(dir, …)` 可被单测直接驱动。

### S-03 命令层写回闸门（`entries.rs`）

```
$ cargo test -p bg3-translate --lib write_back_downgrades        # 修复前
半截流式文本不得写进产物: <?xml version="1.0" encoding="utf-8"?><contentList>
  <content contentuid="c1" version="1">半截译文</content>
  <content contentuid="c2" version="1">世界</content></contentList>
test result: FAILED
```

修复后同一条用例：`test result: ok`（19 passed）。
处置语义与前端 `lib/entries.ts::toWritableEntry` 逐字段一致，报告里这一条与第三轮 §3.2
「后端仍只认 `status == error`」是同一件事的收口。

### S-05 损坏文件隔离备份（`config.rs` / `glossary/store.rs`）

修复前：

```
损坏的文件应被备份到 /tmp/.tmpi5SXXe/settings.json.corrupt          ← 文件根本不存在
非 UTF-8 不应让设置加载直接失败: Io(Error { kind: InvalidData, message: "stream did not contain valid UTF-8" })
应备份到 /tmp/.tmpi1g8X8/glossary.json.corrupt
非 UTF-8 不应让术语表加载失败: Io(Error { kind: InvalidData, … })
test result: FAILED. 14 passed; 3 failed      # config
test result: FAILED. 13 passed; 2 failed      # glossary::store
```

修复后：`config::tests` 20 passed、`glossary::store::tests` 16 passed。
备份语义：**改名**（不是复制也不是删除），`settings.json.corrupt` / `glossary.json.corrupt`，
已存在则 `.corrupt.2` … `.corrupt.9`，绝不顶掉上一份。README 已把这条用户可见行为写清楚
（「原文件会被改名成 ….corrupt 留在原位旁边…你也可以手工把 API Key 或术语捞回来」）。

> 「是否告知用户」的结论：**日志 + 备份，但没有弹窗**。理由：损坏发生在一个可能没有 UI 的时机
> （`translate_entries` 也会 `config::load`），弹窗要引入新的 IPC 事件（超出「最小改动」）；
> 而「文件内容还在」比「弹窗」更能救回数据。这一条与 lead 点名的「是否备份/是否告知」对应。

### S-08 文档与代码不一致（数据目录路径）

```
$ cargo test -p bg3-translate-core --lib config::tests::path_helpers -- --nocapture
PROBE system_data_dir=/home/jason/.config/bg3-translate       ← 探针已在收尾时删除，改成正式断言
$ cargo test -p bg3-translate-core --lib config::tests
test config::tests::system_data_dir_shape_matches_documented_paths ... ok
```

Windows 侧证据（静态，来自依赖源码，非本机执行）：

```
~/.cargo/registry/src/…/directories-6.0.0/src/win.rs:75:  let config_dir = app_data_roaming.join("config");
~/.cargo/registry/src/…/directories-6.0.0/src/lib.rs:451: | Windows | `{FOLDERID_RoamingAppData}`\`_project_path_`\config |
```


### S-09 术语表删除的 first-match 语义 / official 不变量（`glossary/store.rs`）

**lead 转来的推断我先独立核实过，三条全部成立**（以代码 + 真跑为准）：

- 旧实现确实是「`find()` 第一条做 official 检查 → `retain(|e| e.source != source)` 删全部」；
- 状态确实可达，而且**只用公开 API** 就能造出来：官方表里本就有 6 组同 source
  （`real_glossary_already_contains_duplicate_sources`），用户在「新增术语」填同名 source 时
  `add` 是**原地覆盖第一条**（保留位置、把 kind 换成 user），于是第一条 = user、第二条 = official。
  最小复现：`vec![official("Jaheira"), official("Jaheira")]` → `add(user_entry("Jaheira", …))`
  → `terms[0].source_kind == "user"`、`terms[1].source_kind == "official"`。

修复前（RED，隔离 target 目录）：

```
$ cargo test -p bg3-translate-core --lib glossary::store::tests
test delete_removes_only_one_of_two_duplicate_user_entries ... FAILED
  assertion `left == right` failed: 删一次只能少一条: []        ← 两条都被删光
    left: 0
   right: 1
test delete_refuses_when_a_later_duplicate_is_official ... FAILED
  同 source 里有官方条目必须拒绝: ()                            ← 旧实现返回 Ok（删成功了）
test duplicate_source_state_is_reachable_through_public_api ... FAILED
test delete_protects_official_entries ... ok                    ← 既有测试完全看不见这两条
test result: FAILED. 16 passed; 3 failed
```

修复后：

```
test delete_removes_only_one_of_two_duplicate_user_entries ... ok
test delete_refuses_when_a_later_duplicate_is_official ... ok
test duplicate_source_state_is_reachable_through_public_api ... ok
test delete_protects_official_entries ... ok
test result: ok. 19 passed; 0 failed
```

**变异实验（两半修复各自都要为真）**：

| 变异 | ① 只删一条 | ② 任意 official 就拒绝 | ③ 公开 API 可达性 | `delete_protects_official_entries` |
| --- | --- | --- | --- | --- |
| M-A 完全还原旧实现（`find` + `retain`） | **红** | **红** | **红** | 绿（旧测试抓不到） |
| M-B 保留「任意 official」检查，删除仍用 `retain` | **红** | 绿 | 绿 | 绿 |
| M-C 保留 first-match 删除，official 检查只看第一条 | 绿 | **红** | **红** | 绿 |
| 还原修复版 | 绿 | 绿 | 绿 | 绿 |

M-B / M-C 说明这不是「顺手改两行」：**两半都是承重的**，各自有专属的红用例。

**残留**：`update` 仍是 first-match（同 source 多条时只能改到第一条）；要把「界面上点哪一行 = 数据里改哪一行」
彻底做对，得让 `update` / `delete` 按稳定 id 定位（IPC 契约变更）—— 与 §4 里 `Glossary::update`
那条取舍同源，列 backlog。
---

## §3 变异实验矩阵（脚本与 CI：改坏必须 exit 1）

### §3.1 `scripts/check_ipc_contract.py`（在 `/tmp/r4-shell-base` 的 `git archive HEAD` 干净树上做，两版脚本同树对照）

| 变异 | 旧脚本（HEAD） | 新脚本 | 说明 |
| --- | --- | --- | --- |
| M1 新增 `#[tauri::command]` 但不注册 | exit=0 | **exit=1** | 新增覆盖 |
| M2 文档返回列 `void` → `Glossary`（`repack_mod`） | exit=0 | **exit=1** | 新增覆盖 |
| M3 文档返回列清空（`open_mod`） | exit=0 | **exit=1** | 新增覆盖 |
| M4 后端返回类型 `Result<Vec<TranslationEntry>>` → `Result<Glossary>` | exit=0 | **exit=1** | 新增覆盖 |
| M5 文档参数名 `workDir` → `workdir` | exit=1 | exit=1 | 原有检查未放宽 |
| M6 前端 `invoke` 字段 `{ workDir }` → `{ workdir: workDir }` | exit=1 | exit=1 | 原有检查未放宽 |
| M7 后端形参 `file_name` → `filename` | exit=1 | exit=1 | 原有检查未放宽 |
| M8 `Cargo.lock` 里 core 版本 `1.1.5` → `0.9.9` | exit=1 | exit=1 | 原有检查未放宽 |
| M9 `types.ts` 里 `TranslationEntry` 多一个字段 | exit=1 | exit=1 | 原有检查未放宽 |
| 基线（未变异） | exit=0 | exit=0 | —— |

新增的两项检查在真实树上也是绿的：

```
✓ 17 个 #[tauri::command] 全部已在后端注册
✓ 17 个命令的返回类型与架构文档命令表一致
✓ IPC 契约检查全部通过
IPC_EXIT=0
```

### §3.2 `.github/workflows/ci.yml` 的 meta 门禁断言

把 CI 步骤的 `run:` 原文抽出来本地真实执行（`yq` 提取，跑的就是那串 bash）：

```
$ yq -r '.jobs.meta.steps[] | select(.name == "校验一键脚本的门禁完整性") | .run' .github/workflows/ci.yml > /tmp/r4-meta-step.sh
$ bash /tmp/r4-meta-step.sh
verify.sh 门禁清单一致（6 道）
verify.sh --core-only --no-ipc 门禁清单一致（3 道）
verify.sh --web-only --no-ipc 门禁清单一致（2 道）
STEP_EXIT=0
```

| 变异（在 `/tmp/r4-ci-mut` 改 `verify.sh`） | 旧 CI 步骤 | 新 CI 步骤 |
| --- | --- | --- |
| N1 `web-build` 只在 `RUN_IPC=1` 时进清单 | exit=0（**盲**） | **exit=1**：`::error::scripts/verify.sh 在 --web-only --no-ipc 模式下的门禁清单与 CI 期望不一致` |
| N2 `core-test` 只在 `RUN_IPC=1` 时进清单 | exit=0（**盲**） | **exit=1** |
| 基线 | exit=0 | exit=0 |

两种变异都**不影响默认清单**（`RUN_IPC=1` 时逐字符不变），所以旧的断言完全看不见 —— 这正是第三轮
§3.4「防线可以被静默关掉而 CI 不报警」的同一个模式。

### §3.3 `Glossary::delete` 的两半修复（见 §2 S-09）

M-A 全还原 → 3 条新用例全红；M-B 只去掉「只删一条」→ ① 红；M-C 只去掉「任意 official 就拒绝」→ ②③ 红；
还原修复版 → 19/19 绿。既有 `delete_protects_official_entries` 在三种变异下**始终是绿的**，
说明「旧测试全绿」完全不能证明这条不变量成立。

---

## §4 已确认未修 + 理由（逐条对应 lead / verifier 点名的候选）

| 项 | 判定 | 理由与证据 |
| --- | --- | --- |
| `BG3_TRANSLATE_HOME` 是**相对路径**时原样接受，落点 = 进程 CWD | **不修**（钉住行为 + 文档说明） | 这是最高优先级的用户覆盖入口，用来给脚本/多套配置切目录。「相对路径就忽略」会**偷偷改变配置落点**，对已经在用相对路径的人比现状更糟；而且落点只影响「配置写在哪」，不影响 MOD 产物。已用 `relative_env_override_is_used_verbatim` 把语义钉死（谁改成判否会立刻红），README 里写明「相对路径按进程当前目录解析，建议写绝对路径」，ARCHITECTURE 里写明「刻意原样接受」。真实风险场景（Windows 快捷方式 CWD 不是 exe 目录）会由 `ensure_dir` 的「创建失败 → 临时目录 + 告警」兜住 |
| `write_atomic` 覆盖**只读**目标：Linux 成功且丢掉只读位，Windows `rename` 失败 | **不修，记录为已知跨平台差异**（钉住行为 + 文档说明） | 两个方向都要付代价：让 Linux 也失败 = 用户明明能保存却被拦；让 Windows 也成功 = 要替用户清只读位。影响面仅限「有人手工把 `settings.json` 设成只读」。已用 `write_atomic_over_read_only_target_differs_by_platform` 把 Linux 行为钉住、把 Windows 期望写进 `cfg!(windows)` 分支（本机跑不了，§6 标注） |
| `Glossary::update` 允许把 source 改成**已存在**的 source（重复 source） | **不修**（真实数据证明不该按「唯一性」收紧） | 关键证据：真实官方术语表 `samples/bg3-official-glossary.json` 清洗后**本来就有 6 组重复 source**（`Jaheira`、`Freedom`、`Sharess' Caress`、`Danthelon's Dancing Axe`、`Fraygo's Flophouse`、`Sword Coast Couriers`），每组 2 条、**译文完全相同**；`matcher::tests::duplicates_are_reported_once_per_entry` 也把重复条目当既定行为。所以「source 全局唯一」不是本项目的真实不变量，给 `update` 加拒绝会**让这 6 组真实条目无法编辑**。已加 `real_glossary_already_contains_duplicate_sources` 把这条证据固化（断言 6 组、每组 2 条、组内译文一致），将来谁要收紧唯一性会先看到它。真正的补救方向是把 `update`/`delete` 从「按 source 定位」改成按稳定 id 定位（要动 IPC 契约）→ backlog |
| 承接上一条的**真实后果**：重复 source 行在 React 里 `key={t.source}` 重键 | **跨范围移交**（不在我写范围） | 证据：`GlossaryPanel.tsx:290 key={t.source}` + 上面的 6 组真实重复数据 ⇒ 导入官方术语表后表格里会出现 6 组重复 key 的行（React 会告警，且「编辑/删除第二行」实际作用到第一行，因为 `store.rs::update/delete` 用 `position`/`find` 取第一条）。**已通过 lead 转给 auditor-web**，我不改 `src/**` |
| `open_mod` 解包后同步 `remove_dir_all` 上一个工作目录，可能与在途写回/打包并发（第三轮 F-09）；全局单个取消令牌（第三轮 F-08） | **不修**（维持第三轮结论） | 本轮没有新的可达性证据：前端 `runningRef` 挡住重入、写回前还有 `checked_work_root` 校验；要修需要引入「任务租约 + RAII」或引用计数，改动面大于收益。写进「下轮 backlog」 |
| 术语表命令的**多进程**并发（两个实例同时改同一个 `glossary.json`） | **不修** | 桌面单实例应用，没有多实例入口；S-02 的锁已覆盖进程内所有命令层入口。要防跨进程得引入文件锁，收益极低 |

---

## §5 已排查但证伪的怀疑点（同样附命令）

| 怀疑点 | 结论 | 证据 |
| --- | --- | --- |
| `matcher::is_word_char`（`is_alphanumeric() \|\| '_'`）与 regex 的 `\w` 定义不一致（如 `½`、`²` 这类 `No` 字符）→ 有一部分术语仍然永不命中 | **真实数据上零差异（证伪）** | 临时探针（取证后删除）用真实 19,524 条术语逐条比较首尾字符的两种判定：`PROBE 参与匹配 19524 条 / PROBE 首尾字符判定分歧 0 条 / PROBE 自然上下文命中不了 0 条`。合成数据上仍可能有分歧（`½ Cup` 之类），但没有真实语料支持，按「不为没有证据的输入改代码」不动它 |
| 「source 唯一」是术语表的不变量，`update` 造重复是缺陷 | **证伪**（见 §4） | 真实数据里本来就有 6 组重复 source；`duplicates_are_reported_once_per_entry` 明确把重复当既定行为 |
| 前端可能在 `src/lib/tauri.ts` 之外直接 `invoke`，而契约脚本只扫 `tauri.ts` | **证伪** | `grep -rln "invoke" src` → 只有 `src/lib/tauri.ts` 一个文件（组件全部走 `@/lib/tauri` 的封装） |
| README 里那些可机器验证的语料数字（1971 / 80.16 / 581 / 1103 / 410 / 303 / 417 / Tooltip 1103 / Type 625 / `{` 0 处 / 307 组） | **属实** | 独立用 python3 复算 `samples/english.xml`：条目 1971、trim+实体解码后 total_chars **158006**、平均 **80.1654**（文档写 80.16）、最长 **581**、`&lt;LSTag` 1103、`&lt;br&gt;` 410、真标签 0、`[N]` 303 条/417 处、`{` 0、Tooltip 1103、Type 625；种子 `grep -c "    SeedEntry" seed.rs` → 102 |
| `verify.sh --list` 与 CI 期望是否仍然漂移 | **不漂移** | §3.2 的本地真实执行：默认 6 道逐字符一致，且新增了两种模式各 3/2 道的断言 |
| 新增的 `writable_entries` 会不会误伤正常写回（把已完成条目也降级） | **证伪** | 用例同时断言「`translated` 条目照常写回 `世界`」与「`translating` 条目退回原文 `Hello`」，只有 `status == translating` 的那条被净化 |

---

## §6 无法验证项（如实列出）

| 项 | 原因 |
| --- | --- |
| `release.yml` 的 4 段 PowerShell（版本读取、标签校验、产物整理、标签冲突） | 本机没有 `pwsh`。**静态审查未发现新缺陷**：`$ErrorActionPreference = "Stop"`、按类型逐个断言 `*-Portable.zip` / `*.msi` / `*-Setup.exe` / `SHA256SUMS.txt`、拒绝 0 字节产物、tag 冲突用 404/200 + commit sha 判定、`publish` 只在发布意图下运行且单独拿 `contents: write`。`yq -e '.jobs \| keys'` 证明两个 workflow 的 YAML 仍可解析 |
| `system_data_dir_shape_matches_documented_paths` 的 **Windows 分支** | 本机是 Linux；Windows 侧结论来自 `directories` 6.0.0 的 `src/win.rs` 与 crate 文档表（静态证据），实际路径值等 Windows CI 跑出来才算数 |
| `write_atomic_over_read_only_target_differs_by_platform` 的 **Windows 分支** | 同上（Linux 分支已真跑：覆盖成功 + 只读位丢失） |
| GitHub Actions **真实运行**（含我加强的 meta 断言、`tauri-shell` 的 `cargo test -p bg3-translate --lib`） | 本机不能跑 Actions；已把 CI 步骤的 `run:` 原文抽出来在本地真实执行（§3.2），但 runner 环境本身未验证 |
| 真实 Tauri 运行时下的并发命令（两个 `delete_glossary_entry` 同时到） | 不跑真后端；S-02 用的是**同一段生产代码** + 两个真线程（`mutate_in`），IPC/事件层未覆盖 |
| `open_mod` 删除旧工作目录与在途写回的竞态窗口 | 需要注入可阻塞的写回钩子才能稳定复现（第三轮已记录，本轮无新证据） |

---

## §7 门禁真实结果（收尾时跑，静止树）

```
$ python3 scripts/check_ipc_contract.py            → IPC_EXIT=0（12 项全绿，含 2 项新增）
$ cargo test -p bg3-translate --lib                → 19 passed; 0 failed
$ cargo clippy -p bg3-translate --all-targets -- -D warnings   → Finished（无 warning）
$ cargo test -p bg3-translate-core --lib           → 413 passed; 0 failed（隔离 target）
$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings → Finished（无 warning）
$ cargo fmt --all --check                          → 零 Diff（全仓）
$ bash scripts/verify.sh
── [1/6] 跨层 IPC 契约检查                ✓ 通过（43ms）
── [2/6] Rust 代码格式（cargo fmt --check） ✓ 通过（167ms）
── [3/6] Rust Clippy 零告警                ✓ 通过（1.3s）
── [4/6] Rust 核心库测试（含端到端 PAK 闭环）✓ 通过（4.6s）
      413 + 16 + 11 + 8 + 8 = 456 passed; 0 failed
── [5/6] 前端单元测试（vitest）             ✓ 通过（5.2s）  Test Files 21 passed / Tests 208 passed
── [6/6] 前端类型检查 + 构建               ✓ 通过（652ms）
✓ 全部 6 道门禁通过（总耗时 7.4s）         VERIFY_EXIT=0（2026-09-27 二次收工前最后一次跑，代码 hash f95b2364…）
```

说明：
- 审计中期曾出现**他人半成品导致的共享 crate 编译失败**（`translation/sse.rs` 的 `ChatStreamChunk` 字段
  与 `LenientChunk.error`），我的文件不在其中；等对方收口后重跑即绿（如实记录，与第三轮 §5.4 同型）。
- 我自己范围的两条新测试曾在**错误的测试写法**下死锁 60s+（把 `Barrier` 放在写锁内部）。
  这不是产品缺陷，是我的测试 bug：已把同步点移到锁外面，并在注释里写明「锁里面互等会死锁」。
  如实保留这条记录，因为它正好说明了「并发测试要跑得出来才算数」。
- 收尾自检（生产路径，`#[cfg(test)]` 之前的部分）：
  `unwrap() / .expect( / panic! / unreachable! / #[allow` → 5 个文件**全部为空**；
  `Cargo.toml` / `package.json` / `Cargo.lock` 无改动（未新增依赖）；
  `.github/**` 只增断言、未删步骤、无 `continue-on-error`。
- 需要如实说明的一点：为了过 `cargo fmt --all --check`，我两次跑过 `cargo fmt -p bg3-translate-core`
  （crate 级、覆盖整个 core）。若当时正好有其它 writer 的半成品未格式化，它的文件也会被格式化到
  rustfmt 标准——**只是格式差异、不影响语义**，项目本来就要求 `cargo fmt --all --check` 干净。
- 我加过的两处临时探针（`matcher.rs` 的 19,524 条术语探针、`config.rs` 的 `PROBE system_data_dir`）
  **已全部删除**：前者删净（`git diff` 对该文件为空），后者换成正式断言
  `system_data_dir_shape_matches_documented_paths`。收尾复查：
  `grep -rn "PROBE|临时探针|dbg!" crates/ src/ src-tauri/src/ scripts/` → 无匹配
  （仓库里剩下的 `eprintln!("跳过：当前用户仍可读 chmod 000 的文件…")` 是 formats/pak 测试里
  「以 root 运行时跳过」的提示，属于 auditor-formats 的范围，不在我的改动里）。

---

## §8 交付清单与本轮新增测试

| 文件 | 改动 |
| --- | --- |
| `crates/bg3-translate-core/src/config.rs` | `temp_path_for`（唯一临时名）、`quarantine_corrupt_file` + `quarantine_path`、`load_from` 三种损坏统一处置、头部注释路径修正、**+8 条测试** |
| `crates/bg3-translate-core/src/glossary/store.rs` | `load_from` 接隔离备份 + 非 UTF-8 回退、`delete` 改成「任意 official 拒绝 + 只删第一条」、**+6 条测试**（含真实数据重复 source 固化与删除语义三条） |
| `src-tauri/src/commands/entries.rs` | `writable_entries` / `writable_entry`（写回前净化 `translating`）、**+1 条测试** |
| `src-tauri/src/commands/terminology.rs` | 进程内写锁（`glossary_write_guard` / `mutate_in`）、`reset`/`import` 进锁、**+1 条并发测试** |
| `src-tauri/src/commands/translate.rs` | `log_translate_request`（`{:?}` 转义）+ 捕获 logger 的**+1 条测试** |
| `scripts/check_ipc_contract.py` | 新增「未注册命令」「返回类型」两项检查（原有 4 类一行未删），注释与检查项列表同步 |
| `.github/workflows/ci.yml` | meta job 增加 `--core-only` / `--web-only` 两个模式的门禁清单断言（**未删任何步骤、未加 continue-on-error**） |
| `README.md` / `docs/ARCHITECTURE.md` | 系统模式实际路径、损坏文件备份行为、相对路径语义、契约脚本新增检查、`tauri-shell` 真正执行命令层测试、命令层两道新的并发/净化不变量 |

新增/修改的测试名（共 17 条）：

1. `config::tests::atomic_write_temp_path_is_unique_per_call`
2. `config::tests::concurrent_atomic_writes_all_succeed_with_whole_content`
3. `config::tests::corrupted_settings_file_is_kept_as_backup`
4. `config::tests::quarantine_does_not_overwrite_an_existing_backup`
5. `config::tests::non_utf8_settings_file_falls_back_to_defaults`
6. `config::tests::system_data_dir_shape_matches_documented_paths`
7. `config::tests::relative_env_override_is_used_verbatim`
8. `config::tests::write_atomic_over_read_only_target_differs_by_platform`
9. `glossary::store::tests::corrupted_file_is_quarantined_before_seed_takes_over`
10. `glossary::store::tests::non_utf8_glossary_falls_back_to_seed`
11. `glossary::store::tests::real_glossary_already_contains_duplicate_sources`
12. `commands::entries::tests::write_back_downgrades_entries_still_translating`
13. `commands::terminology::tests::concurrent_mutations_are_serialized_and_lose_nothing`
14. `commands::translate::tests::translate_request_log_cannot_be_forged_with_newlines`
15. `glossary::store::tests::delete_removes_only_one_of_two_duplicate_user_entries`
16. `glossary::store::tests::delete_refuses_when_a_later_duplicate_is_official`
17. `glossary::store::tests::duplicate_source_state_is_reachable_through_public_api`

（17 条：core 14 条、命令层 3 条。）

## §9 给下一轮的 backlog

1. `Glossary::update` / `delete` 改成按稳定 id 定位（IPC 契约变更）：`delete` 本轮已改成 first-match（与 `update` 一致，
   界面上删一行 = 数据里少一行），但同 source 多条时 `update` 仍只作用于第一条；顺带解决重复 source 行的
   React 重键问题（已移交 auditor-web）。
2. `open_mod` 删除旧工作目录与在途写回/打包之间加引用计数或租约（第三轮 F-09）。
3. `AppState.cancel` 改成「每任务一个令牌 + RAII 租约」（第三轮 F-08）。
4. `write_atomic` 可考虑 `fsync` 目录项（掉电语义，当前未承诺）。
5. `actions` 缓存的 `bun-version: latest` 未固定版本（可复现性，非缺陷）。
