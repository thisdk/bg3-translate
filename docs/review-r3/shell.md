# T4 审计报告：Tauri 壳层 / 配置 / 术语表 / 构建脚本与 CI 文档一致性

- 审计者：`shell-auditor`（task-4）
- 基线 revision：`422f667a0d5d3be26190c0f5b55b0796a67a7b76`（worktree 起始 hash `ad8d3219378a5b89059a3558138ec7d1`）
- 环境：Linux + cargo 1.95.0 / rustc 1.95.0 / bun 1.3.13 / python3 3.13.15；本机装有 webkit2gtk，**可以真编译 Tauri 壳**
- 写范围：`src-tauri/src/**`、`core/src/{config.rs,error.rs,glossary/**}`、`scripts/**`、`.github/**`、`docs/ARCHITECTURE.md`、`README.md`、本文件

> 说明：本报告所有结论都附「实际跑过的命令 + 真实输出片段」。凡是没跑过的一律标「未验证」。
> 审计期间另外三位 writer 在同一工作区并行改动（`formats/**`、`pak.rs`、`translation/**`、`types.rs`、`src/**`），
> 因此中途出现过他们半成品导致的编译/格式失败，本报告如实标注。

---

## §1 结论摘要表

| 编号 | 严重度 | 位置 | 现象（一句话） | 状态 |
| --- | --- | --- | --- | --- |
| F-01 | **中** | `src-tauri/src/commands/entries.rs` + `archive.rs` | 前端可控 `work_dir`/`file_name` 可越出工作目录任意写盘（信任边界缺陷，MOD 内容触发不了） | **已修**（含 4 组回归测试 + 3 组变异验证） |
| F-02 | 低 | `glossary/matcher.rs` | 首尾带非 `\w` 字符的术语（如 `'Brake' Lever`）因 `\b` 永远不命中；真实术语表 19,524 条里 **266 条**受影响 | **已修**（3 个回归测试 + 变异验证） |
| F-03 | 低 | `config.rs::resolve_data_dir` | 纯空白 `BG3_TRANSLATE_HOME=" "` 被当合法目录，会在 CWD 下建一个名为空格的目录 | **已修**（红→绿） |
| F-04 | 低 | `config.rs::data_dir` | 「数据目录创建失败 → 回退临时目录」这条承诺此前没有可执行验证；`BG3_TRANSLATE_HOME` 指向文件时的行为未测 | **已修（补测试 + 抽出可测函数）** |
| F-05 | 低 | `scripts/check_ipc_contract.py` | 只查命令名，不查**参数名**；版本号一致性检查覆盖不到 `Cargo.lock` | **已修（加强检查，未放宽任何检查项）** |
| F-06 | 信息 | `error.rs::code()` | 文档说 `code()` 供前端做差异化处理，但 `Serialize` 输出纯字符串，前端拿不到 code | **已修（改注释 + 加形状锁定测试）** |
| F-07 | 低 | `src-tauri/src/commands/translate.rs:45` | 前端可控 `work_dir` 被原样写进日志（换行可伪造日志行）；未修 | **已确认未修**（无法先写失败测试） |
| F-08 | 低 | `state.rs` / `translate.rs` | `cancel` 是全局单令牌：并发的第二次 `translate_entries` 会 `reset()` 掉第一次的取消请求；前端有 `runningRef` 守卫，UI 触发不到 | **已确认未修**（附理由） |
| F-09 | 低 | `commands/archive.rs::open_mod` | 解包后同步 `remove_dir_all` 上一个工作目录，可能与上一次 `spawn_blocking` 写回/打包并发 | **已确认未修**（附理由） |
| F-10 | 信息 | `config.rs::write_atomic` | 临时文件名按 pid 命名：同进程并发写同一文件理论上可能交叉；实际入口只有一个 | **已确认未修**（附证据） |
| F-11 | 信息 | `docs/ARCHITECTURE.md` / `README.md` | engine-auditor 收口后 4 处文档漂移（4xx 重试分类、属性名参与保真校验、截断流判失败、不用 `[DONE]` 的网关会被判失败） | **已修（纯文档同步）** |

已排查但**证伪**的怀疑点见 §3（含 `actions/checkout@v7` 是否存在、CI 门禁清单是否漂移、脚本是否会漏检命令名等）。

---

## §2 逐条明细

### F-01（中）前端可控路径逃出工作目录 → 任意写盘

**现象**：`write_file_entries(work_dir, file_name, entries)` 把两个参数直接交给
`formats::write_entries`，后者用 `pak::resolve_disk_path`（纯 `join`）拼路径。
`file_name = "../../escaped-localization.xml"` 逃出 `work_dir`；
`file_name = "/tmp/absolute-localization.xml"`（绝对路径）直接无视 `work_dir`。
`work_dir` 本身也来自前端，所以只校验文件名还不够（`work_dir="/"` + 相对路径即可）。

**严重度校准（按 lead 要求）**：这是 **webview → 后端的信任边界缺陷**，需要前端主动传坏字符串；
MOD 内容触发不了它（解包时条目名已过 `safe_output_path`，能通过过滤的名字在裸 `join` 下落回同一路径）。
因此定级 **中**，不是高。

**复现（修复前，跑的是真实 `#[tauri::command] write_file_entries`）**：

```
$ cargo test -p bg3-translate --lib probe_document -- --nocapture
running 1 test
PROBE_relative_result=Ok(())
PROBE_relative_exists=true
PROBE_relative_content=Some("<?xml version=\"1.0\" encoding=\"utf-8\"?><contentList><content contentuid=\"c1\" version=\"1\">你好</content></contentList>")
PROBE_absolute_result=Ok(())
PROBE_absolute_exists=true
```

即：工作目录**之外**真的出现了文件，内容是前端给的条目（`contentuid` / 译文都可控）。
探针（`probe_document_escape_evidence`）取证后已删除，永久回归测试见下。

**根因**：`pak::resolve_disk_path` 只做 `unpacked_dir(work_dir).join(normalize_entry_name(file_name))`，
没有 `..` / 绝对路径 / 盘符 / `:` / Windows 保留名校验（`pak::safe_output_path` 有，但写回链路没用它）。

**改动**（全部在 `src-tauri`，未触碰 `pak.rs` / `formats/**`）：

- `src-tauri/src/commands/mod.rs`：新增 `checked_work_root`（校验前端回传的 `work_dir` 必须
  等于 `AppState` 记录值，并**返回记录值**作为读写根目录）、`unpacked_path`（用现成的
  `pak::safe_output_path` 校验归档路径）、`is_same_dir` / `lexical_parts`（先 `canonicalize`，
  失败退回词法比较；含非法 UTF-8 保守判否）。
- `src-tauri/src/commands/entries.rs`：`read_file_entries` / `write_file_entries` 改为
  「取 `AppState` 记录值 → 校验 → `formats::{read_entries_from_path, write_entries_to_path}`」。
- `src-tauri/src/commands/archive.rs`：`repack_mod` 的 `work_dir` 同样绑定到 `AppState`；
  **`output_path` 不做限制**（用户通过系统对话框选的保存位置合法地可以指向任意目录）。
- `src-tauri/src/state.rs`：新增 `current_work_dir()` 快照读取。

**回归测试**（`cargo test -p bg3-translate --lib`，15 个 shell 测试全绿）：

```
test commands::entries::tests::write_entries_rejects_parent_dir_escape ... ok
test commands::entries::tests::write_entries_rejects_absolute_path ... ok
test commands::entries::tests::write_entries_rejects_backslash_escape ... ok
test commands::entries::tests::write_entries_rejects_foreign_work_dir ... ok
test commands::entries::tests::write_entries_rejects_colon_and_reserved_names ... ok
test commands::entries::tests::read_entries_rejects_escape_and_keeps_missing_file_error ... ok
test commands::entries::tests::write_entries_still_writes_inside_unpacked ... ok
test commands::entries::tests::frontend_echoed_work_dir_passes_validation ... ok
test commands::tests::same_dir_accepts_exact_echo_and_trailing_separator ... ok
test commands::tests::same_dir_rejects_different_and_empty ... ok
test commands::tests::same_dir_case_sensitivity_matches_platform ... ok
test commands::tests::same_dir_falls_back_to_lexical_for_missing_paths ... ok
test commands::tests::same_dir_rejects_non_utf8_paths ... ok
test commands::tests::checked_work_root_returns_recorded_path ... ok
test commands::tests::unpacked_path_stays_under_root ... ok
test result: ok. 15 passed; 0 failed; 0 ignored
```

**正向证据（lead 要求 4）**：`frontend_echoed_work_dir_passes_validation` 模拟真实流程 ——
`AppState::replace_work_dir(open_mod 返回的目录)` → 前端把**同一个字符串**回传 →
`write_entries_checked` / `read_entries_checked` 校验通过、写回成功、读回 `contentuid`/`version`
逐字不变；`same_dir_accepts_exact_echo_and_trailing_separator` 另外覆盖「尾随 `/`、`./`、`//`、
反斜杠形态」都能通过。

**变异验证**（把修复改回原样，确认新测试真的会红）：

| 变异 | 结果 |
| --- | --- |
| `checked_path` 里换回 `pak::resolve_disk_path`（原始缺陷） | **5 个测试红**（parent_dir/absolute/backslash/colon-reserved/read-escape） |
| `is_same_dir` 退回裸 `a == b` 比较 | 红：`same_dir_accepts_exact_echo_and_trailing_separator` |
| `checked_work_root` 删掉目录校验 | 红：`write_entries_rejects_foreign_work_dir`、`checked_work_root_returns_recorded_path` |
| （还原后） | `test result: ok. 15 passed` |

**边界与取舍**：
- 路径比较用 `canonicalize` 优先；不存在时退回词法比较，并且**用记录值当根目录**，
  即使比较误判，落盘根目录也不会变成前端指定的目录（纵深防御）。
- 大小写：Windows 不敏感、其它平台敏感，与该平台文件系统一致（Linux 用例已跑）。
- 非 UTF-8 路径在词法回退里直接判否（IPC 层本来也传不了非 UTF-8；`open_mod` 返回的是 lossy 字符串）。
- 只锁「工作目录」，不锁用户选的 `outputDir` / `outputPath`（避免让「导出到桌面」失效）。

---

### F-02（低）术语表：首尾带标点的条目永远命中不了

**现象**：`compile_boundary` 一律编译 `\b{needle}\b`。`\b` 要求一侧是 `\w`、另一侧不是；
`'Brake' Lever` 的首字符是 `'`、`{1} damage` 的首字符是 `{`，它们与前面的空格之间**不是词边界**，
正则永不成立 —— 预筛命中了也会被边界校验丢掉，用户看到「这条术语怎么都不生效」。

**复现（修复前红）**：

```
$ cargo test -p bg3-translate-core --lib glossary::matcher
test glossary::matcher::tests::whole_word_terms_with_punctuation_edges_still_match ... FAILED
  assertion `left == right` failed
    left: []
   right: ["{1} damage", "+1 Sword"]
test glossary::matcher::tests::punctuation_edged_terms_still_respect_the_word_side ... FAILED
    left: []
   right: ["Sword+"]
test result: FAILED. 14 passed; 2 failed
```

**真实数据影响**（临时探针 `probe_count_terms_that_never_matched`，用 `samples/bg3-official-glossary.json`
跑真实的 `Glossary::from_json` + 同一个判定函数，取证后已删除）：

```
PROBE 参与匹配 19524 条，其中首尾含非 \w 字符（修复前永不命中）266 条
PROBE   "'Brake' Lever" -> "\"制动\"拉杆"
PROBE   "'Magic' Ring" -> "\"魔法\"戒指"
PROBE   "'Miracles' of the Outer City" -> "外城\"奇迹\""
```

**根因**：`\b` 只在紧邻 `\w` 的一侧有意义，旧实现两侧都加。

**改动**：`matcher.rs::compile_boundary` 按「首/尾字符是否属于 `\w`」决定要不要加对应的 `\b`；
新增 `is_word_char`（与 regex 的 Unicode `\w` 对齐）。词边界语义对单词形术语**完全不变**。

**回归测试**：`whole_word_terms_with_punctuation_edges_still_match`、
`punctuation_edged_terms_still_respect_the_word_side`（保证去掉 `\b` 的那一侧不会变成「任意后缀算命中」）、
`real_glossary_punctuation_edged_terms_now_match`（真实样本：`'Brake' Lever` 必须命中）。

**变异验证**：把两侧改回恒加 `\b` → 前两个测试红；还原后 17/17 绿。

**影响面说明**：术语命中只用于**提示词里的术语提示**（`planner.rs` 把 `matches` 挂到 job 上），
不会替换原文，所以这条不影响 MOD 正确性，定级低。

---

### F-03（低）纯空白 `BG3_TRANSLATE_HOME` 不被忽略

**现象**：空串被忽略（已有测试），但 `"   "` 被当成合法路径 → 在当前工作目录下建一个名字是
空格的目录，配置与术语表写到那里，用户完全找不到；Windows 上尾随空格还会被 Win32 静默改写。

**复现（修复前红）**：

```
$ cargo test -p bg3-translate-core --lib config::tests::whitespace
test config::tests::whitespace_only_env_override_is_ignored ... FAILED
  left: EnvOverride
 right: Portable
```

**改动**：`resolve_data_dir` 里把「trim 后为空」也按「没设置」处理。
**修复后**：`test result: ok. 14 passed`（config 模块）。

---

### F-04（低）数据目录回退路径此前不可测

**现象**：`data_dir()` 承诺「系统目录也创建不了就退回临时目录」，但这条分支没有测试，
`BG3_TRANSLATE_HOME` 指向**文件**（`create_dir_all` 报 `AlreadyExists`）时到底怎样没人验证过。

**改动**：把回退逻辑抽成 `ensure_dir(resolved: DataDir) -> Result<DataDir>`（语义不变，日志补上原因），
新增两个测试：

- `data_dir_that_is_actually_a_file_falls_back_to_temp`：路径是文件 → 回退到
  `<tmp>/bg3-translate`、`source` 变成「系统配置目录」、回退目录真实存在；
- `ensure_dir_keeps_a_usable_directory_untouched`：可创建目录原样返回，来源不变。

（这条是「补验证 + 可测性重构」，不是缺陷修复；行为与修复前一致。）

---

### F-05（低）契约脚本漏检参数名 / 版本号漏检 `Cargo.lock`

**现象**（修复前脚本只做命令名与枚举检查）：`invoke("read_file_entries", { workdir, fileName })`
这类「命令名对、参数名错」的调用只会在运行期炸；`Cargo.lock` 里工作区 crate 的版本号不在任何门禁里，
版本升了却忘更新 lock，普通 cargo 命令会静默改写它、`--locked` 构建才失败。

**改动**（`scripts/check_ipc_contract.py`，**只加强、不放宽**）：

1. 新增「命令参数契约」：解析 `src-tauri/src/commands/*.rs` 的 `#[tauri::command]` 形参
   （排除 `State` / `AppHandle` / `Window` 等注入类型），与前端 `invoke` 的字段（camelCase→snake_case）
   以及 `docs/ARCHITECTURE.md` 命令表逐条比对；
2. 新增版本号一致性：`package.json` / `tauri.conf.json` / `Cargo.toml [workspace.package]` /
   `Cargo.lock` 两个工作区 crate。

**修复前基线**（加强后第一次跑，说明解析是真在工作而不是空过）：
```
命令：注册 17 个，前端使用 17 个，文档列出 17 个
✓ 17 个命令的参数名在后端 / 前端 invoke / 文档命令表三处一致
✓ 版本号三处一致：1.0.0（package.json / tauri.conf.json / Cargo.toml）
✓ Cargo.lock 中 bg3-translate、bg3-translate-core 的版本与 workspace 一致：1.0.0
```
解析明细（抽样，证明每组参数非空且真实）：
```
read_file_entries    rust=['work_dir','file_name']      js=['workDir','fileName']      doc=['workDir','fileName']
translate_entries    rust=['work_dir','entries','style_hint','on_event'] js=['workDir','entries','styleHint','onEvent']
```

**变异验证**（在 /tmp 的仓库副本上做，避免碰其他 writer 的文件；每条变异都是真实失败）：

| 变异 | 脚本反应 |
| --- | --- |
| 前端 `{ workDir }` → `{ workdir }` | exit=1：命令参数契约不一致 |
| 后端形参 `file_name` → `filename` | exit=1：命令参数契约不一致 |
| 文档 `filePath` → `filepath` | exit=1：命令参数契约不一致 |
| `Cargo.lock` 里 `bg3-translate-core` 1.0.0 → 0.9.9 | exit=1：Cargo.lock 版本没跟上 |
| `package.json` 版本 → 1.0.1 | exit=1：版本号三处不一致 |
| 全部还原 | exit=0 通过 |

**未放宽**：原有 4 类检查（命令注册/文档/枚举）一字未删，`-D warnings` 等地门禁未动。

---

### F-06（信息）`AppError::code()` 的注释与 IPC 形状不符

**现象**：注释写「便于前端做差异化处理」，但 `Serialize` 把错误序列化成一条纯字符串，
前端 `String(e)` 展示（`src/App.tsx:68`、`useEntryLoading.ts:57` 等），**拿不到 code**；
`src/**` 里也没有任何 `.code` 的使用。

**证据**：
```
$ grep -rn "String(e)\|\.code\b" src/lib/*.ts src/store/*.ts   # 只有 String(e)，无 .code
$ grep -n "serialize_str" crates/bg3-translate-core/src/error.rs
serializer.serialize_str(&self.to_string())
```

**改动**：改注释说清楚「code 只在本进程内用；想按类别差异化必须在 Rust 侧判断，或者改 IPC 契约」，
并新增形状锁定测试 `serialized_form_carries_no_machine_readable_code`
（改 `Serialize` 成结构体会让它变红，防止前端拿到 `[object Object]` 时才发现）。
`error.rs` 中**没有** apiKey / 完整路径泄漏：`grep -rn "api_key" crates/... src-tauri/src` 只在
`translator.rs` 的 `bearer_auth`、`types.rs` 的字段定义与测试里出现，没有任何日志/错误信息拼接。

---

### F-07（低）前端可控字符串原样进日志 —— 已确认未修（见 §7 遗留风险）

`src-tauri/src/commands/translate.rs:45`：

```rust
log::debug!("翻译请求：work_dir={work_dir}，条目 {} 条", entries.len());
```

`work_dir` 来自前端，**含换行时可以伪造日志行**：日志同时写控制台与日志文件
（`tauri_plugin_log` 的 Stdout + LogDir），一个
`work_dir = "…\n[INFO] 翻译结束：成功 999 条"` 就能在日志里插进一条不存在的记录，
排查问题时会把日志当证据的人带偏。建议改成 `{work_dir:?}`（Debug 会转义换行与不可见字符）。
**为什么不改**：这条无法先写出失败测试（要跑真实 `translate_entries` 需要 Tauri `State` + `Channel`，
壳层没有 Tauri 测试运行时），按本任务的硬性要求「只有能先写失败测试时才改生产代码」，
保留为已确认未修。实际影响极低：`work_dir` 不参与任何 IO，且需要前端主动传坏值。

### F-08（低）全局单取消令牌 —— 已确认未修

`AppState.cancel` 是唯一令牌，`translate_entries` 开头 `state.cancel.reset()`。
两次并发调用时，第二次的 `reset()` 会吞掉第一次的取消请求，而 `cancel_translation` 会把两个都取消。

**为什么定低**：前端在 `useTranslationRun.ts:132` 用同步的 `runningRef.current` 挡住了重入
（`if (!workDir || request.length === 0 || runningRef.current) return;`），UI 触发不到并发。
**为什么不改**：需要给 `AppState` 加「当前任务租约 + Drop 释放」才能修干净；
一旦释放路径漏了，应用会永久无法再翻译（比现状更糟），而这条现有测试覆盖不到命令层。
建议后续按「`Mutex<Option<CancelToken>>` + RAII 租约 + 第二次调用快速失败」单独做。

### F-09（低）打开新 MOD 时删除旧工作目录与上一次写回并发 —— 已确认未修

`open_mod` 在 `replace_work_dir` 之后同步 `pak::remove_work_dir(previous)`。
若上一次的写回/打包还在 `spawn_blocking` 里跑，就可能删到正在用的目录。
F-01 的修复把「目录已经不是当前工作目录」的写回直接拒掉，窗口比修复前更小，
但「已经通过校验、正在写」的那一瞬间仍然存在竞态。
**为什么不定高、不改**：需要用户「点打包的同时立刻打开另一个 MOD」才可能命中，
且产物落在临时目录（可重来）；写一个不 flaky 的复现测试需要注入可阻塞的写回钩子，
改动面大于收益。另外这段删除是同步 IO，跑在 async 运行时线程上（大目录会短暂占住 worker）。

---

## §3 已排查但证伪的怀疑点（同样附命令）

| 怀疑点 | 结论 | 证据 |
| --- | --- | --- |
| `actions/checkout@v7` / `upload-artifact@v7` / `download-artifact@v8` / `action-gh-release@v3` 是否存在 | **存在，且都是当前最新 major** | `curl -sS https://api.github.com/repos/actions/checkout/tags?per_page=8` → `['v7.0.1','v7.0.0','v7','v6.1.0',…]`；upload-artifact → `['v7.0.1','v7.0.0','v7',…]`；download-artifact → `['v8.0.1','v8.0.0','v8',…]`；softprops/action-gh-release → `['v3.0.3','v3.0.2','v3',…]`；另查 `Swatinem/rust-cache`（v2.9.2）、`oven-sh/setup-bun`（v2.2.0）、`dtolnay/rust-toolchain`（v1）均存在 |
| workflow 里用到的 action **输入名**是否还合法 | 合法 | `curl …/actions/upload-artifact/v7.0.1/action.yml` → inputs 含 `name/path/if-no-files-found/retention-days`；`download-artifact/v8.0.1` → 含 `name/path`。ci.yml 只用了这几个 |
| `scripts/verify.sh --list` 与 ci.yml 里 hardcode 的清单是否漂移 | **不漂移**，6 条命令**逐字符相同** | 用 python 把 ci.yml 的 `expected=...` 与 `bash scripts/verify.sh --list` 的 `cut -f2` 逐行比对：`CI 期望 == verify.sh --list ? True`（6 行全 OK） |
| 契约脚本会不会漏检「前端调了后端没注册」 | 不会，且已在本机验证 | `python3 scripts/check_ipc_contract.py` → `✓ 前端 invoke 的每个命令都已在后端注册`；脚本对 `used - registered`、`registered - documented`、`documented - registered` 三个方向都做了差集 |
| `translate_entries` 使用前端传的 `work_dir` 读写文件 | 证伪：它只记日志，不参与任何 IO | `src-tauri/src/commands/translate.rs:45` 只有 `log::debug!`；条目自带 `source_file`，翻译不读盘 |
| `extract_mod` / `repack_mod` 的输出路径是否该收紧 | 不该：那是用户通过系统对话框选的保存位置 | `src/lib/tauri.ts:28-42` 的 `pickSavePath` / `pickExtractDirectory` 都走 `plugin-dialog`；且 lead 明确要求不锁 |
| `glossary` 的匹配会不会替换 `<LSTag>` 属性里的文本 | 证伪：匹配结果是**提示词素材**，不做任何替换 | `planner.rs:148/194` 只把 `matcher.find_matches(...)` 挂到 `TranslationJob.matches`；`grep -rn "find_matches" src-tauri/src` 只有 `translate.rs` 构造 matcher |
| 术语里的 `.`、`*`、`[`、`{1}` 会不会被当正则解释 | 证伪：`regex::escape` + Aho-Corasick 字面匹配，无模式解释 | `compile_boundary` 用 `regex::escape(needle)`；`build_automaton` 直接用字面量。真正的问题是 F-02 的边界语义，不是转义 |
| 空文件 / 非法 JSON 的术语表、设置文件会不会让应用起不来 | 证伪：都有回退 | `config::load_from` 损坏 → 默认值并告警（既有测试 `corrupted_settings_file_falls_back_to_defaults`）；`Glossary::load_from` 损坏 → 官方种子（既有测试 `corrupted_file_falls_back_to_seed`） |
| README「内置 102 条官方术语」是否属实 | 属实 | `grep -c "    SeedEntry" crates/bg3-translate-core/src/glossary/seed.rs` → `102` |
| `verify.sh --list` 是否真的不需要工具链 | 属实 | 脚本 `case "$0"` 取目录，无 dirname 依赖；`--list` 分支在工具自检之前 `exit 0` |

---

## §4 文档与代码漂移（含 lead 定向提示的环境假设类表述）

| 位置 | 原文 | 问题 | 处理 |
| --- | --- | --- | --- |
| `docs/ARCHITECTURE.md:335` | 「`src-tauri` 本身**没有**集成测试：它依赖 webkit2gtk/gtk/dbus，本机与 ubuntu CI 都编译不了」 | **本机是假的**（见下方证据）；且现在命令层已有单元测试 | 改写成「没有独立目录的集成测试，但命令层有单元测试；能否本机编译取决于 GUI 系统库是否就位；`verify.sh` 故意不纳入，避免误伤没装库的机器」 |
| `docs/ARCHITECTURE.md:8` | 「在 Linux 开发者机器上 `cargo test` 直接失败（缺系统库）」 | 绝对化表述（装了库就不会失败） | 改为「在**没装**这些系统库的 Linux 开发机上」 |
| `scripts/verify.sh:29-31` | 「在多数开发机与 ubuntu CI 上都编不了」 | 同类的过时环境假设 | 改为「在没装这些库的开发机与 ubuntu CI 上都编不了（装了 webkit2gtk 的 Linux 可以真跑），但门禁不能要求每台机器都有 GUI 系统库」；**门禁清单一行未动** |
| `scripts/check_ipc_contract.py:6-7` | 「在没有这些库的机器（以及 ubuntu CI）上编译不了」 | 同上 | 改为「没装的机器编不了；装了 GUI 库的 Linux 开发机可以真编」（同时更新检查项列表） |
| `README.md:175-177` | 「多数开发机上编不了」 | 同上 | 改为「装好库的 Linux/Windows 可以直接编译」，并给出本机可跑的三条命令 |
| `README.md:205` | 「在很多机器上编不了」 | 同上 | 与契约脚本描述一并改写，并补上新增的两类检查 |
| `README.md:211-215` | meta job 只提「版本号三处一致」 | 现在 `Cargo.lock` 也由契约脚本核对 | 文档同步 |
| `docs/VERIFICATION.md:96/245-249` | 历史验证记录里的 nix 属性名漂移 | 这是**一次已验证会话的历史记录**，不是当前承诺 | **不改**（改历史记录会让证据链失真），仅在此标注 |
| `docs/ARCHITECTURE.md:209` | 「4xx（除 408/429）仍跟网络错误一样退避重试 3 次才失败」 | 已被 `retry::is_retryable_status` 取代：**408/425/429/5xx 可重试，其余 4xx 直接失败，认不出的状态码按可重试处理** | 改写为新的分类表；`Retry-After` 仍未实现，继续留在已知限制 |
| `docs/ARCHITECTURE.md:221` | 保真校验「属性值与标签内文本不参与比较」 | 漏了关键一半：**开始标签的属性名**也参与比较（大小写敏感、顺序不计） | 改写表格行，并补上「属性值是玩家可见文本必须允许翻译；属性名是结构、写回会把模型标签逐字节写进 PAK，所以必须校验」的理由，防止以后被改回去 |
| `docs/ARCHITECTURE.md:202-208` | 「两处协议级修复」 | 新增了截断流判失败（`finish_reason ∈ {length, content_filter}` 或既无 `[DONE]` 也无 `finish_reason` → llm 错误 → 重试 → `error` → 退回原文） | 补成「三处」，并新增一段独立的「重试分类」说明 |
| `docs/ARCHITECTURE.md` 已知限制 / `README.md` | 未提及「不用 `[DONE]` 也不发 `finish_reason` 的第三方网关会被判失败」 | 这会让一部分以前能用的网关开始报错，属于**刻意取舍**（与「连接被掐断」不可区分；宁可失败也不静默丢内容） | 两处都写进「已知限制」，并给出用户可操作的建议（换用守协议的端点 / 点重试） |

**证据（lead 要求 1：独立复现）**：

```
$ cd /home/jason/bg3-translate && cargo check -p bg3-translate --all-targets; echo $?
    Checking bg3-translate v1.0.0 (/home/jason/bg3-translate/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.94s
0
```

（首次未缓存时 `real 0m1.027s`，check 成功；后续 `Finished` 走缓存。）
另外本机还真的跑了 `cargo clippy -p bg3-translate --all-targets -- -D warnings` 与
`cargo test -p bg3-translate --lib`（见 §5）。

**关于「要不要因此给 verify.sh 加门禁」**：**不加**。理由：CI 的 `core` / `web` job 跑在
ubuntu-latest 上，没有 webkit2gtk/gtk/dbus；把「本机必须装 GUI 库」写成门禁会让那些 job 直接失败，
而这与代码质量无关。keep 现状：本机可选跑，CI 由 Windows `tauri-shell` job 兜底。

---

## §5 门禁真实结果

命令（任务书要求的那串）：

```bash
python3 scripts/check_ipc_contract.py && cargo fmt --all --check &&
cargo clippy -p bg3-translate-core --all-targets -- -D warnings &&
cargo test -p bg3-translate-core --all-targets &&
cargo clippy -p bg3-translate --all-targets -- -D warnings
```

结果：**全绿**（§5.2）。另外完整的一键门禁也跑过：

```
$ bash scripts/verify.sh
…
   ✓ 通过（5.3s）        # 5/6 前端单元测试：Test Files 20 passed, Tests 190 passed
   ✓ 通过（660ms）       # 6/6 tsc -b && vite build
✓ 全部 6 道门禁通过（总耗时 7.3s）
VERIFY_EXIT=0
```

### §5.1 我写范围内的结果

- `python3 scripts/check_ipc_contract.py` → **exit 0**（9 项全绿，见 F-05 输出）
- `cargo test -p bg3-translate --lib`（壳层命令层 + 路径校验回归）→ **15 passed; 0 failed**
- `cargo test -p bg3-translate-core --lib config::` → **14 passed; 0 failed**
- `cargo test -p bg3-translate-core --lib error::` → **6 passed; 0 failed**
- `cargo test -p bg3-translate-core --lib glossary::` → **43 passed; 0 failed**
- `rustfmt --edition 2024 --check`（只对我改的 9 个 Rust 文件）→ **通过**（`MY_FILES_FMT_OK`）
- `cargo clippy -p bg3-translate --all-targets -- -D warnings` → 见 §5.2

### §5.2 集成后重跑：**全部通过**（其他 writer 收口后）

其他 writer 的半成品合上之后，在**当前工作树**上重跑任务书要求的那串门禁，全部通过：

```
$ python3 scripts/check_ipc_contract.py && cargo fmt --all --check \
  && cargo clippy -p bg3-translate-core --all-targets -- -D warnings \
  && cargo test -p bg3-translate-core --all-targets \
  && cargo clippy -p bg3-translate --all-targets -- -D warnings

=== 1) IPC 契约 ===
命令：注册 17 个，前端使用 17 个，文档列出 17 个
✓ 前端 invoke 的每个命令都已在后端注册
✓ 后端注册的命令全部写进了架构文档
✓ 架构文档没有虚构的命令
✓ 17 个命令的参数名在后端 / 前端 invoke / 文档命令表三处一致
✓ TranslationEvent 与前端联合类型一致：all_done delta done error progress
✓ TranslationStatus 与前端联合类型一致：edited error pending translated translating
✓ PakFileKind 与前端联合类型一致：data-txt localization-loca localization-xml metadata-lsx other script-lua
✓ 7 个结构体 + TranslationEvent 全部变体的字段与 types.ts 逐字段一致
✓ 版本号三处一致：1.0.0（package.json / tauri.conf.json / Cargo.toml）
✓ Cargo.lock 中 bg3-translate、bg3-translate-core 的版本与 workspace 一致：1.0.0
✓ IPC 契约检查全部通过
IPC_EXIT=0
=== 2) fmt ===
FMT_EXIT=0
=== 3) clippy core ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
CLIPPY_CORE_EXIT=0
=== 4) test core ===
test result: ok. 326 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.72s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
=== 5) clippy shell ===
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.61s
CLIPPY_SHELL_EXIT=0
```

### §5.3 隔离验证（排除其他 writer 的干扰，证明「我的改动本身干净」）

审计中期其他 writer 的半成品会让整仓门禁失败，为了把「我的改动」与「他们的改动」分开，
把仓库复制到 `/tmp/shell-gate`：**用 `git archive HEAD` 还原 `crates/` 与 `src-tauri/`，
只叠加我改过的文件**，再跑一遍：

```
### 1) fmt（隔离树全量）        FMT_OK
### 2) clippy core              Finished `dev` profile … in 1m 08s（无 warning）
### 3) test core                test result: ok. 270 passed; 0 failed
                                test result: ok. 6 passed; 0 failed
                                test result: ok. 6 passed; 0 failed
```

（隔离树里的 270 条 = HEAD 基线 + 我新增的测试；`src-tauri` 的 clippy 在本机需要
webkit2gtk 的 pkg-config 环境，隔离树用全新 target 目录时重建 `libdbus-sys`/`gdk-3.0` 失败，
这是**环境问题不是代码问题**——同一份代码在真实工作树（依赖已缓存）里
`cargo clippy -p bg3-translate --all-targets -- -D warnings` 是 exit 0，见 §5.2。）

### §5.4 中途失败记录（如实保留）

审计中期 `cargo fmt --all --check` 与 `cargo clippy` 曾失败，diff/错误全部落在
**其他 writer 正在改的文件**里（`formats/{content_list,loca,lsx}.rs`、`pak.rs`、
`translation/{fidelity,retry,sse,translator}.rs`、`types.rs`），我的文件不在其中：

```
$ cargo fmt --all --check | grep '^Diff in'
Diff in .../formats/content_list.rs
… （9 个文件，全部属于其他 writer）
$ rustfmt --edition 2024 --check <我改的 9 个文件>
MY_FILES_FMT_OK
```

另外说明：早期我用过一次 `cargo fmt --all`（写模式），它把全仓格式化到 rustfmt 标准；
若某位 writer 的半成品当时恰好被格式化，那只是格式差异、不影响语义，且项目本来就要求
`cargo fmt --all --check` 通过。

---

## §6 无法验证项与原因

1. **PowerShell 段（`release.yml` 的 4 个 `shell: pwsh` 步骤）**：本机没有 `pwsh`（也不允许装依赖），
   只能静态审查。静态审查未发现缺陷：`$ErrorActionPreference = "Stop"`、按类型逐个断言
   `*-Portable.zip` / `*.msi` / `*-Setup.exe` / `SHA256SUMS.txt`、拒绝 0 字节产物、
   标签冲突用 HTTP 404/200 + commit sha 判定。**标记：未验证**。
2. **CI 实际执行**：本机不能跑 GitHub Actions。action 版本存在性与输入名已用 GitHub API 验证（§3），
   但 job 之间的制品传递（`upload-artifact@v7` → `download-artifact@v8`）只能等真实 CI 跑。
   **标记：未验证**。
3. **Windows 专有路径行为**：`safe_output_path` 的 Windows 保留设备名 / NTFS 数据流拒绝逻辑、
   `canonicalize` 的 `\\?\` 前缀、大小写不敏感比较，都按代码 + `cfg!(windows)` 分支审查，
   Linux 上无法真跑。**标记：未验证**。
4. **真实 run 的 `translate_entries` 并发与取消**：需要 Tauri 运行时 + 真 API key，
   本机只做了静态审查（§2 F-08）。**标记：未验证**。
5. **`samples/` 真实 MOD 的端到端重打包**：属于 T1 的范围，本次未重复验证。

---

## §7 遗留风险

1. **F-07 日志注入**：`translate.rs:45` 把前端可控的 `work_dir` 原样写进日志（同时落盘）。
   恶意/异常前端可以用换行伪造日志行，后果是**排查问题时被日志误导**（日志是排查依据），
   不影响 MOD 产物。修法是一行的事（`{:?}`），但缺一条可自动化的失败测试，按纪律未改。
2. **F-08 全局取消令牌**：前端守卫挡着，但后端没有防线；一旦未来加了「并行翻译多个文件」的功能，
   必须同时改成「每任务一个令牌」。
3. **F-09 工作目录生命周期**：`open_mod` / `close_mod` / 退出钩子都会删临时目录，
   删除与「正在写回/打包」之间没有互斥。F-01 让越界与过期请求直接失败，
   但没有引入引用计数或锁；如果以后出现「后台批量打包」，需要补。
4. **F-10 原子写的同进程并发**：`write_atomic` 的临时文件名只带 pid（同一进程内并发写同一路径会共用
   临时文件）。当前只有「保存设置」一个入口，实际触发不到；将来若术语表也支持后台自动保存，建议改成
   `pid + 序号` 或 `tempfile::NamedTempFile`。
5. **`docs/VERIFICATION.md` 是历史记录**：它描述的是当时的机器状态（nix-shell 提供 GUI 库），
   本次刻意不改，避免证据链失真；读者应结合 `docs/VERIFICATION-ROUND2.md` 与 README 的当前表述。
6. **契约脚本的参数检查依赖正则解析 Rust/TS 源码**：写法一旦超出当前模式（例如 `invoke` 的第二个参数
   不是对象字面量、命令函数带非注入的自定义类型参数），脚本会明确报「无法静态解析」而不是静默通过；
   这是有意的失败姿态，但改动 IPC 写法时需要同步更新解析器。

---

## §8 待办与跨范围事项（交给 lead / 对应 writer）

1. **跨范围（engine-auditor，不要我自己改 `translation/**`）**：用户可见的报错文案
   `流式响应结束，但既没有收到 [DONE] 也没有 finish_reason，无法确认输出完整（连接可能被中途掐断）`
   （`translation/translator.rs::ensure_stream_complete`）**只说明原因、不含下一步动作**。
   对照同函数的截断文案（`…可减小单条文本长度或改用输出上限更高的模型后重试`），
   建议补一句可操作建议，例如「若你使用的是自定义网关，请确认它按 OpenAI 协议发送 `[DONE]`
   或 `finish_reason`；否则请改用标准端点后重试」。
2. **F-07（低）**：`translate.rs:45` 的日志注入，一行修复但缺可自动化测试（见 §7.1）。
3. **F-08（低）**：如果要支持「并行翻译多个文件」，必须先把 `AppState.cancel`
   改成每任务一个令牌 + RAII 租约（见 §2 F-08）。
4. **F-09（低）**：`open_mod` / `close_mod` / 退出钩子删除临时目录与「正在写回/打包」之间没有互斥；
   若将来引入后台批量打包，需要补引用计数或锁。
5. **给独立验证者**：本报告所有「已修」项都给了「修复前红 → 修复后绿」与变异验证的命令；
   建议至少复跑 §2 F-01 的三个变异（换回 `resolve_disk_path`、`is_same_dir` 裸比较、
   删掉 `checked_work_root` 的校验）来确认回归测试真的有效。
