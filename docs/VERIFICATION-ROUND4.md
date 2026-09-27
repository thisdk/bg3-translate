# 第四轮独立验证报告（T6 / verifier）

> 立场：四位审计者（auditor-formats / auditor-engine / auditor-web / auditor-shell）的报告与自述
> **在本报告中一律只当作待验证的假设**。下面每一条结论都来自我自己在隔离环境里跑出来的命令与输出；
> 引用 writer 的地方都会写明「这是 writer 的声明」并给出我的复核结果。

---

## §0 被验证的 revision 与两次 hash

| 项 | 值 |
| --- | --- |
| `HEAD` | `d8295da5ca9fed71ee6217e16890defa31e42760`（= tag `v1.1.5`） |
| 阶段 B 起始 hash（H1） | `aebf22d2337e3ca498491f22216989af` |
| 写报告前 hash（H2） | `aebf22d2337e3ca498491f22216989af` |
| H1 == H2 | **是**（逐字相同） |
| 基线 hash（阶段 A 开工时，= HEAD） | `ad513e30c1e335f2a1f9cd669f6aa7ed` |

命令（逐字，H1/H2 各跑一次，中间未改任何文件）：

```console
$ cd /home/jason/bg3-translate && find src crates src-tauri scripts .github README.md package.json Cargo.toml \
    -type f -not -path '*/target/*' -not -path '*/__pycache__/*' | sort | xargs md5sum | md5sum
aebf22d2337e3ca498491f22216989af  -
```

其他静止性证据：`git status --porcelain` 在 H1 与 H2 两次都是 **32** 项（30 个 `M` + 2 个 `??`，两个
`??` 是应交交付物 `docs/review-r4/` 与 `src/components/AppTopBar.test.tsx`）；写本报告之后变为 33 项
（多出来的就是本文件，`docs/` 不在 hash 命令范围内，因此 hash 不变）；我另把冻结树 rsync 到 `/tmp/redteam-r4/frozen`
做全部变异实验，验证结束后逐文件对比 **`aebf22d2337e3ca498491f22216989af` 与仓库完全相同**
（即所有变异都已还原）。

### 0.1 方法学事故与处置（必须写清楚，否则前面所有数字都不可信）

阶段 A 期间我用 `CARGO_TARGET_DIR=/home/jason/bg3-translate/target`（共享 target）在 `/tmp` 沙箱里编译探针，
结果**沙箱测试二进制链接到了仓库那份 rlib**，而不是沙箱源码编译出来的那份：

- 沙箱源码 `crates/bg3-translate-core/src/config.rs` 里 `grep -c "隔离备份"` = **0**（= HEAD 版本）；
- 但同一份源码编出来的测试二进制 `target/debug/deps/redteam_r4_shell-fd713b17bef830a5` 里
  `grep -c "隔离备份\|quarantine"` = **11**，并且把损坏的 `settings.json` 改名成了 `settings.json.corrupt`
  —— 那正是 auditor-shell 16:26 才在仓库里新增的 `quarantine_corrupt_file`。
- 交叉污染机制已确认：两个源码树下 **crate 的 unit hash 相同**（我在两边分别构建 `-p bg3-translate --lib`，
  产物都叫 `bg3_translate_lib-a69ad7406582d390`），共享 target 时后构建的一方会覆盖前者。

处置：阶段 A 全部探针改用隔离 target（`/tmp/redteam-r4/iso-target`）**重跑一遍**并以此为准
（证据文件 `/tmp/redteam-r4/iso-all.txt`）；本报告 §2–§4 的数字**全部**来自隔离环境的重新执行。
仓库生产文件我一个都没改（唯一写入是 `docs/review-ROUND4.md` 即本文件，且 `docs/` 不在 hash 命令范围内）。

---

## §1 结论摘要表

| # | 验证项 | 结论 | 证据锚点 |
| --- | --- | --- | --- |
| 1 | `bash scripts/verify.sh`（冻结树，静止） | **6/6 通过**，总耗时 7.7s | §2.1 |
| 2 | Rust 核心（`--all-targets`） | **414 + 16 + 11 + 8 + 8 = 457 全绿** | §2.2 |
| 3 | Tauri 壳命令层（`cargo test -p bg3-translate --lib`） | **19/19 通过**（真实执行，非「本机编不了」） | §2.3 |
| 4 | `cargo clippy -p bg3-translate --all-targets -- -D warnings` | **0 告警**（退出码 0） | §2.3 |
| 5 | 前端（vitest） | **21 文件 / 208 用例全绿** | §2.1 |
| 6 | IPC 契约脚本 | **12 项断言全过**；17 命令 × 3 处（后端注册 / 前端 invoke / 文档）一致 | §2.4 |
| 7 | 防线是否被静默削弱 | **未发现削弱**：无新增 `#[allow]` / `#[ignore]` / `continue-on-error`；`verify.sh` 一字未改；`Cargo.toml`/`Cargo.lock`/`package.json`/`bun.lock` diff 为空；测试只增不减（Rust `#[test]` 387→426，前端 `it()` 179→208） | §2.5 |
| 8 | 红队 F-A1（读不出来的目标被当成新文件） | **已修**：我的验收探针 ACC-1 由红转绿（写回报错、文件逐字节不变） | §3、§4.1 |
| 9 | 红队 F-A2（畸形底稿被整体重写、尾部条目永久丢失） | **已修**：ACC-2 由红转绿；变异（忽略 `parsed.error`）立刻变红 | §3、§4.1 |
| 10 | 红队 F-A3（MOD 自带目标的大小写判定） | **判定层已修**（`normalizePakPath` + `findShippedFile`），我的 ACC-F33/ACC-F33b 通过；**物理覆盖仍无法在本机复现**（Windows 专属，见 §6） | §3、§4.1 |
| 11 | loca 布局自检「是否过严」（lead 最担心） | **不过严**：~6000 例差分 fuzz 中「上游读得动、自检拒绝」= **0**；收紧型变异（M11/M12）会让正向用例变红，说明防线两个方向都被钉住 | §3.4 |
| 12 | `Glossary::delete` official 不变量 + 只删第一条 | **成立**；旧语义（本地重实现）确实会连 official 一起删掉，变异后作者的 2 条新用例 + 我的 2 条探针全红 | §3.6 |
| 13 | F-37（重复 source 的行 key / busy）与跨范围移交闭环 | **闭环**：shell→web（行 key）与 web→shell（delete 语义）两边都有归属、都有落地、都能变红 | §3.7 |
| 14 | 文档断言与实测一致 | README/ARCHITECTURE 的语料与门禁数字**逐条实测一致**（唯一 nit：平均长度 80.1654 写作 80.16 是截断而非四舍五入） | §2.6 |
| 15 | 关键修复的变异有效性 | 我独立做了 **16 组变异**，全部按预期变红（含 1 组直接 SIGABRT）；**未发现无效回归测试** | §3.8 |

**总体判定：可以收尾**（阻断项 0 条；无法验证项见 §6，均已在 §5 标注级别且不阻断）。

---

## §2 门禁真实输出（冻结树、静止、我自己跑的）

### 2.1 `bash scripts/verify.sh`（6/6）

```console
── [1/6] 跨层 IPC 契约检查        $ python3 scripts/check_ipc_contract.py            ✓ 通过（47ms）
── [2/6] Rust 代码格式            $ cargo fmt --all --check                          ✓ 通过（173ms）
── [3/6] Rust Clippy 零告警       $ cargo clippy -p bg3-translate-core --all-targets -- -D warnings  ✓ 通过（220ms）
── [4/6] Rust 核心库测试          $ cargo test -p bg3-translate-core --all-targets   ✓ 通过（1.0s）
── [5/6] 前端单元测试             $ bun run test                                     ✓ 通过（5.4s）
── [6/6] 前端类型检查 + 构建      $ bun run build                                    ✓ 通过（661ms）

✓ 全部 6 道门禁通过（总耗时 7.7s）      # 退出码 0
```

前端明细：`Test Files 21 passed (21)` / `Tests 208 passed (208)`。

### 2.2 Rust 核心测试逐 target 计数（`--all-targets`）

| target | 结果 |
| --- | --- |
| `unittests src/lib.rs` | `414 passed; 0 failed` |
| `tests/corpus_regression.rs` | `16 passed` |
| `tests/corpus_writeback.rs` | `11 passed` |
| `tests/e2e_pak_flow.rs` | `8 passed` |
| `tests/real_mod_sample.rs` | `8 passed` |
| **合计** | **457**（基线是 374+16+11+8+8 = 417，本轮 +40） |

### 2.3 壳层（本机真实执行）

```console
$ cargo test -p bg3-translate --lib
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
# 基线是 16；+3（write_back_downgrades_entries_still_translating /
#              concurrent_mutations_are_serialized_and_lose_nothing / translate_request_log 相关）

$ cargo clippy -p bg3-translate --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.13s     # 退出码 0
```

### 2.4 IPC 契约（脚本自己的输出）

```console
命令：注册 17 个，前端使用 17 个，文档列出 17 个
✓ 前端 invoke 的每个命令都已在后端注册
✓ 后端注册的命令全部写进了架构文档
✓ 架构文档没有虚构的命令
✓ 17 个命令的参数名在后端 / 前端 invoke / 文档命令表三处一致
✓ 17 个 #[tauri::command] 全部已在后端注册
✓ 17 个命令的返回类型与架构文档命令表一致
✓ TranslationEvent 与前端联合类型一致：all_done delta done error progress
✓ TranslationStatus 与前端联合类型一致：edited error pending translated translating
✓ PakFileKind 与前端联合类型一致：data-txt localization-loca localization-xml metadata-lsx other script-lua
✓ 7 个结构体 + TranslationEvent 全部变体的字段与 types.ts 逐字段一致
✓ 版本号三处一致：1.1.5（package.json / tauri.conf.json / Cargo.toml）
✓ Cargo.lock 中 bg3-translate、bg3-translate-core 的版本与 workspace 一致：1.1.5
✓ IPC 契约检查全部通过
```

我另外**独立变异了两处输入**验证这些断言不是空转（见 §3.8 的 IPC-M1 / IPC-M2），两次都 `exit=1`。

### 2.5 防线是否被静默削弱（逐项裁定）

| 检查 | 结果 |
| --- | --- |
| `scripts/verify.sh` diff | **空**（一字未改） |
| `scripts/verify.sh --list` | 6 道，与基线逐字一致 |
| `--core-only --no-ipc --list` / `--web-only --no-ipc --list` | 3 道 / 2 道，与 CI 新增断言一致（我复跑） |
| `Cargo.toml` / `Cargo.lock` / `package.json` / `bun.lock` diff | **空**（无依赖变化） |
| 新增 `#[allow]` / `#[ignore]` / `continue-on-error` / `--no-fail-fast` / `exit 0` | **0 处** |
| `#[ignore]` 总数 | **0** |
| 生产路径 panic 面（`.unwrap()/.expect(/panic!/unreachable!`） | 41 → **41**（唯一的 +1 在 `translation/engine/tests.rs`，该文件整体是测试模块） |
| 前端新增 `@ts-ignore` / `@ts-expect-error` / `as any` / `eslint-disable` / `dangerouslySetInnerHTML` | 全部 **0** |
| 测试总数 | Rust `#[test]` **387 → 426**；前端 `it()` **179 → 208**（只增不减） |
| 删除的测试 | **只有 1 条**：`App.localization-merge.test.tsx` 里第三轮那条「底稿读取失败不阻断写回」被**反向断言**替换为「必须中止写回」（场景与 mock 保留）。这是本轮语义裁定，不是削弱；替换后的用例在本轮变异 M-F30 下会红 |
| `.github/workflows/ci.yml` | +28 行，**加强**（给 `--core-only --no-ipc` / `--web-only --no-ipc` 分别断言门禁清单，防止「给某道门禁加个 `if` 就静默少跑」） |

### 2.6 文档断言 vs 我的实测

| 文档断言 | 我实测的值 | 结论 |
| --- | --- | --- |
| `samples/english.xml` 条目 1971 | 1971 | ✓ |
| 平均长度 80.16（trim + 实体解码后） | 158006 / 1971 = **80.1654** | ✓（80.16 是截断；四舍五入为 80.17，属排版 nit，非缺陷） |
| 最长 581 | 581 | ✓ |
| `&lt;LSTag` 1103 / `&lt;br&gt;` 410 | 1103 / 410 | ✓ |
| 真标签 0 处 | 0（`<content>` 下无子元素） | ✓ |
| `[数字]` 占位符 303 条 / 417 处；`{...}` 0 处 | 303 / 417 / 0 | ✓ |
| 门禁 6 道 | `verify.sh --list` = 6 | ✓ |
| 版本号三处 1.1.5 | 脚本 + Cargo.lock 均一致 | ✓ |

---

## §3 逐条修复的复现与变异实验矩阵

「修复前红」我用两种方式独立复现：**(a)** 在冻结树上把修复改回原样（变异）后跑作者回归测试；
**(b)** 我自己写的行为级验收探针（见 §3.1/§3.2 的 ACC 系列）。所有变异都在
`/tmp/redteam-r4/frozen`（隔离 target `/tmp/redteam-r4/frozen-target`）里做，做完逐个把文件还原，
最终整树 hash 与冻结 revision 相同。

| # | 修复（来源） | 作者回归测试 | 我的变异（改回原样） | 变异后结果 | 判定 |
| --- | --- | --- | --- | --- | --- |
| M3 | `content_list::write`：只有 `NotFound` 才算新文件（F-R4-03 = 红队 F-A1） | `formats::content_list::tests::write_refuses_when_the_target_exists_but_cannot_be_read` | `Err(err) if err.kind()==NotFound` → `Err(_)` | 作者用例 **FAILED** + 我的 ACC-1 **FAILED** | 有效 |
| M4 | `content_list::write`：`parsed.error` 必须中止（F-R4-04 = 红队 F-A2） | `…write_refuses_when_the_target_is_only_partially_parsable` | 删掉 `if let Some(error) = parsed.error { return Err }` | 作者用例 **FAILED** + 我的 ACC-2 **FAILED** | 有效 |
| M6 | `.loca` 读前布局自检（F-R4-02） | `lying_entry_count_is_rejected_without_a_giant_allocation` | `check_layout` 改为 no-op | 测试进程 **SIGABRT**（上游 `Vec::with_capacity(0xFFFFFFFF)` → 137 GB） | 有效（且证明后果是进程级 abort） |
| M11 | 自检**刻意不要求** `texts_offset ≥ 表尾` | `texts_offset_below_the_table_is_still_readable` | 反过来收紧成「必须 ≥ 表尾」 | 该用例 **FAILED**（`texts_offset_beyond_the_table_is_skipped_and_still_readable` 仍 ok） | 有效（过严会被抓到） |
| M12 | 自检**刻意允许**空文本 / 空表 | `empty_text_and_empty_table_are_readable` | 收紧成「有 n 条就必须有文本」 | 该用例 **FAILED** | 有效（过严会被抓到） |
| M-S01 | `write_atomic` 临时名加进程内序号（S-01） | `config::tests::concurrent_atomic_writes_all_succeed_with_whole_content` | `temp_path_for` 改回 `with_extension("tmp<pid>")` | **FAILED**（多个线程 panic） | 有效 |
| M-S02 | 术语表命令层写锁（S-02） | `commands::terminology::tests::concurrent_mutations_are_serialized_and_lose_nothing` | 去掉 3 处 `let _guard = glossary_write_guard()` | **FAILED**：`「读盘 → 改 → 写盘」必须在写锁内串行，否则会互相覆盖` | 有效 |
| M-S03 | 命令层把 `translating` 条目净化（S-03） | `commands::entries::tests::write_back_downgrades_entries_still_translating` | 直接 `write_entries_to_path(path, kind, entries)` | **FAILED**：产物里出现 `半截译文` | 有效 |
| M-S09 | `Glossary::delete`：任一同名 official 即拒绝 + 只删第一条（S-09） | `delete_refuses_when_a_later_duplicate_is_official`、`delete_removes_only_one_of_two_duplicate_user_entries` | 改回「找第一条 → official 就拒，否则 `retain` 掉全部同名」 | 2 条作者用例 **FAILED** + 我的 2 条探针 **FAILED**（`delete_protects_official_entries` 仍 ok） | 有效 |
| M-E01 | SSE 流中错误载荷不得当心跳跳过（E-R4-01） | `mid_stream_error_payload_is_rejected_instead_of_returning_half_text`、`…_before_finish_reason` | 删掉 `if let Some(message) = chunk.error { return Err }` | 2 条 **FAILED**；正向对照 `a_null_error_field_does_not_break_a_normal_stream` 仍 **ok** | 有效 |
| M-E02 | 「相同原文合并」改用原文分组（E-R4-02） | `translation::planner::tests::punctuation_only_differences_do_not_share_one_translation`、`translation::engine::tests::punctuation_variants_each_get_their_own_translation` | `group_key` 改回一致性 key | 2 条 **FAILED** | 有效 |
| M-F30 | 底稿读失败必须中止写回（F-30 = 红队 F-A1 前端一侧） | `App.localization-merge.test.tsx：「底稿读取失败必须中止写回…」` | 改回 `console.warn` + `return plan.entries` | 作者用例 **FAILED** + 我的 ACC-F30 **FAILED** | 有效 |
| M-F33 | 自带目标判定大小写不敏感（F-33 = 红队 F-A3） | `localization.test.ts：「归一化：分隔符、重复斜杠、点段与大小写」「大小写不同也算 MOD 自带的文件」` | 去掉 `normalizePakPath` 的 `.toLowerCase()` | 4 条 **FAILED**（2 条作者 + 我的 ACC-F33/ACC-F33b） | 有效 |
| M-F37 | 术语行键 `source#序号` + 按行键置忙（F-37，跨范围移交） | `GlossaryPanel.test.tsx：「同一 source 的两行都要渲染，且不产生 React 重复 key 警告」「点第一行的删除：只有那一行转圈…」` | `rowKey` 退回 `t.source` | 2 条 **FAILED** | 有效 |
| IPC-M1 | 契约脚本的结构体字段检查 | — | `src/lib/types.ts` 里 `sourceFile` → `sourceFileRenamed` | 脚本 **exit=1**：`✗ serde 字段契约不一致` | 有效 |
| IPC-M2 | 契约脚本的「定义了但没注册」检查 | — | 在 `commands/app.rs` 加一个 `#[tauri::command]` 不进 `generate_handler!` | 脚本 **exit=1**：`✗ 命令层定义了但没在 … generate_handler! 里注册的命令` | 有效 |

### 3.1 我自己的行为级验收（红队判据，独立于作者测试）

| 探针 | 判据 | HEAD（阶段 A） | 冻结 revision（阶段 B） |
| --- | --- | --- | --- |
| ACC-1 | 目标存在但读不出来 → 写回**必须报错**且文件逐字节不变 | **RED**（`Ok(())`，165 → 115 字节） | **GREEN**（`Err`，165 → 165 字节） |
| ACC-2 | 目标被截断/畸形 → 写回**必须报错**且不改文件 | **RED**（`Ok(())`，213 → 186 字节，h3 被删） | **GREEN**（`Err`，213 → 213 字节） |
| ACC-3 | 实体往返不动点（信息级，本轮不要求修） | RED | **仍 RED**（见 §5-O1） |
| ACC-4 | 真实语料 1971 条原地零译文写回三元组守恒 + 根属性保留 | GREEN | **GREEN** |
| ACC-5 | 14 例对抗译文写回产物仍是合法 XML 且条目数不变 | GREEN | **GREEN** |
| ACC-F30 | 底稿读失败 → 不调用 `writeFileEntries`、错误进横幅、stage 停在 `files` | **RED**（实测 write 被调用、`stage=done`） | **GREEN** |
| ACC-F33 | MOD 自带 `Localization/CHINESE/` 时仍走合并、payload 保住中文 | **RED**（payload 退化成 `Fireball`/`Ice`） | **GREEN**（`h1 火球`、`h2 寒冰`） |

### 3.2 阶段 A 红队探针在冻结树上的复跑（隔离 target）

| 探针文件 | 结果 |
| --- | --- |
| `redteam_r4_acceptance.rs`（ACC-1..5） | 5 passed / 0 failed |
| `redteam_r4_probe2.rs`（zip-slip、重复条目 zip、语言识别、loca version 阈值） | 7 passed |
| `redteam_r4_probe3.rs`（畸形底稿、闭环重打包、LSX 边界、真实语料三元组） | 6 passed |
| `redteam_r4_dbg.rs`（PAK 穿越名 9 例 + `safe_output_path` 29 例 fuzz） | 2 passed |
| `redteam_r4_dbg2.rs`（临时名撞名） | 1 passed |
| `redteam_r4_loca.rs`（差分 fuzz / 写回拒绝 / 200 轮往返） | 3 passed |
| `redteam_r4_glossary_delete.rs`（official 不变量） | 4 passed |
| `redteam_r4_shell.rs`（config / write_atomic / glossary 语义） | 7 passed / 2 failed（见下） |
| `redteam_r4_probe.rs`（阶段 A 第 1 批） | 12 passed / 1 failed（= ACC-3 同一条：实体不动点） |

`redteam_r4_shell.rs` 的 2 条「失败」是**我的探针口径过时**，不是产品缺陷：S-05 之后损坏的
`settings.json`/`glossary.json` 会被**隔离备份**成 `*.corrupt`，我的探针仍在原路径上 `read().unwrap()`。
隔离目标下的行为已由作者的 S-05 用例与我逐条核对（见 §3.3）。

### 3.3 config / glossary 面（含 lead 点名的三条候选）

| 项 | 我的实测（隔离 target） | 裁定 |
| --- | --- | --- |
| 损坏 `settings.json` | `load_from` 返回默认值、**原文件被改名成 `settings.json.corrupt`**（第二次再损坏 → `.corrupt.2`，不顶掉上一份）；非 UTF-8 不再让加载失败 | writer S-05 的声明**属实**（我复跑了他们的用例名与行为） |
| 损坏 `glossary.json` | 同上（回退 102 条种子 + 隔离备份） | 属实 |
| `BG3_TRANSLATE_HOME` 非法值 | 指向文件 → 回退系统目录；全空格 → 视为未设置；深层不存在 → 创建；**相对路径原样接受（落点 = 进程 CWD）** | writer 判定「不修 + 钉住行为 + 文档说明」；我认同（三条候选里唯一保留的行为，已用 `relative_env_override_is_used_verbatim` 钉住） |
| `write_atomic` 覆盖只读文件 | Linux：成功且**只读位丢失**（644）；Windows 分支本机跑不了 | writer 判定「记录为已知跨平台差异 + 钉住 Linux 行为」，我认同（§6 标注无法验证 Windows 侧） |
| `Glossary::update` 可造重复 source | 复现：`update` 后表内出现两条同 source；真实官方术语表清洗后本来就有 **6 组**重复 source（19991 条里） | writer 判定「不修，因为 source 唯一不是不变量」成立；其**副作用**（行 key / busy）已由 web 侧 F-37 修掉并验证 |
| 20k 术语匹配性能 | matcher 构造 403 ms；单次命中 ≈146 µs；38k 字符文本 446 µs | 与 writer 声明量级一致 |
| 匹配语义 | 长术语优先 ✓、重叠命中 ✓、大小写不敏感/敏感 ✓、`Mindflayer` 不误命中 `Mind` ✓、CJK 词边界 `火焰弹` 不误命中 `火焰` ✓ | ✓ |

### 3.4 loca 布局自检的**差分**审计（lead 点名，最担心的一条）

判据：对同一个文件，比较 **上游 `LocaUtils::load`（真读路径）** 与 **本仓库 `loca::read`（自检 + 上游）**：

```console
[LOCA] 合法样本 297 字节，上游读得动 = true
[LOCA] 本仓库读得动 = true，条数 = Some(3)
[LOCA] 差分结论：不一致 0 条；因上游会 GB 级分配而跳过对比 67 例（这些只验证了自检拒绝）
[LOCA-RT] 200 轮往返失败 0 例
[LOCA-W] 我们 write = Err("…目标 .loca 布局不可信（条目数 4294967295 需要 300647710662 字节的索引表，
         但文件只有 297 字节），已中止写回以免丢掉磁盘上的译文；确认这个文件损坏、想用当前译文重建它时请先删除它…")
[LOCA-W] 文件是否被改动 = false
```

- 变异集：头部 `num_entries` 9 档 × `texts_offset` 7 档、索引条目 `length` 6 档 × 3 条、
  `version` 4 档 × 3 条（**version 与布局无关，我专门验证**）、截断 10 档、尾部追加 3 档、
  随机单字节 4000 轮、随机多字节 2000 轮。
- **「过严」= 0 例**（这正是 lead 担心的方向）；「过松」= 0 例。
- 67 例因 `num_entries` 离谱而**无法安全喂给上游**（会分配 68–137 GB）；我实测过两次：
  `memory allocation of 68719476704 bytes failed`（= 0xFFFFFFFF × 16）与
  `137438953440 bytes`（× 32），**两次都是 SIGABRT**。也就是说：自检把「进程直接死」
  变成了「一条可读错误」，这是修复而不是风险。
- 我另外独立读了上游源码（`bg3rustpaklib 0.1.5/src/loca/{reader,writer}.rs`）核对：
  条目恒 70 字节（key 64 + version u16 + length u32，与 `version` 取值无关）、
  `texts_offset > 12+70n` 才 skip（否则原地不动、不回退）、`length == 0` 直接返回空串。
  自检的五条判定与上游这些 `read_exact` **逐条等价**，常量直接 import 上游
  （`ENTRY_SIZE` / `HEADER_SIZE` / `LOCA_SIGNATURE`），不存在版本相关误判面。
- 结论：**自检不过严，也不漏放**；「合法文件不被拒」有正向用例（我自己的 200 轮随机往返 +
  差分 fuzz 中的全部「上游 ok」样本），「谎报必被拒且无大分配」有 M6 的 SIGABRT 证据。

### 3.5 跨模块集成面

| 面 | 检查方式 | 结果 |
| --- | --- | --- |
| IPC 四方对齐（core types ↔ types.ts ↔ ARCHITECTURE.md ↔ 脚本） | 跑脚本 + 我变异两处输入 | 12 项断言全过；两处变异 exit=1（§3.8） |
| 写回闸门前后端语义 | 前端 `hasWritableTarget`/`toWritableEntry`（`src/lib/entries.ts`）、命令层 `writable_entries`、core `has_writable_target`/`effective_text` 三处定义逐条比对 | 三处语义一致：`target` 非空 **且** `status != error` 才可写回；`translating` 在**前端**和**命令层**两道都被降级为原文（纵深防御），core 仍只认 `error`（E-R4-12 已知残留） |
| 写回底稿判定 | 前端 `findShippedFile`（大小写/分隔符归一、只遍历 `files`）+ 我自己的 ACC-F33 | 判定层已修；物理覆盖见 §6 |
| 文档 ↔ 实现 | IPC 脚本命令表 + 我实测语料数字/门禁数 | 一致（§2.6） |
| 跨范围移交闭环 | shell→web（F-37 行 key/busy）、web→shell（delete 不变量） | 两边都有落地代码 + 回归测试 + 我的变异变红 |

---

## §4 阶段 A 的独立发现与被证伪的怀疑点

### 4.1 已在 HEAD（`d8295da`）复现、且本轮已修的缺陷

| 编号 | 级别 | 现象 | HEAD 复现证据（隔离 target） | 本轮处置 |
| --- | --- | --- | --- | --- |
| F-A1 | 中 | `content_list::write` 把「目标存在但读不出来」当新文件 → 静默删条目且返回 `Ok` | 探针 `p1g`：chmod 000 后只提交 h1 → 文件 165→115 字节、h2 消失、`write` 返回 `Ok(())`；前端一侧 App 层探针实测 `writeFileEntries` 被调用、`stage=done` | 已修（F-R4-03 + F-30）；我的 ACC-1、ACC-F30 转绿，变异 M3/M-F30 变红 |
| F-A2 | **高** | `content_list::write` 忽略 `parse().error`，用残缺底稿整体重写 → 解析中断点之后的条目**永久删除**，且返回 `Ok` | 探针 `p10`：213→186 字节、h3 消失、返回 `Ok(())`；可达性由 App 层探针证明（底稿读失败时 HEAD 只 `console.warn` 就继续写回，实测 `writeFileEntries` 调用 1 次、`stage=done`、`error=null`） | 已修（F-R4-04 + F-30）；ACC-2 转绿，变异 M4 变红。**同时订正了 `docs/CORPUS-AUDIT.md` §D8「当前不可达」的论断** |
| F-A3 | 中低 | `shippedFiles.has(plan.fileName)` 逐字节比较 → 目录/文件名大小写不同时判定落空 → 不读底稿、payload 退化成英文；在大小写不敏感 FS 上就是覆盖自带中文 | JS 探针 A2/A3：`命中底稿 = false`，payload = `[["h1","Fireball"],["h2","Ice"]]` | 判定层已修（F-33：`normalizePakPath` + `findShippedFile`）；ACC-F33/ACC-F33b 转绿，变异 M-F33 变红。物理覆盖本机无法复现（§6） |

### 4.2 低危 / 信息级观察（本轮未修或已由 writer 记录）

| 编号 | 级别 | 现象 | 证据 | 状态 |
| --- | --- | --- | --- | --- |
| O1 | 信息 | `content_list` 写回对**双重转义**文本不是不动点：每写一次剥一层（`&amp;amp;` → 下一轮 `&amp;`） | 探针 `p1f`/ACC-3：第一轮 159 字节、第二轮 151 字节，逐字节不同 | 仍存在（第三轮已把「过度还原」记为刻意取舍；只影响源文件里本来就有双重转义的条目） |
| O2 | 信息 | `loca` 的 `version` 是 u16：> 65535 静默变成 1 | 探针 `p9`：65535 → 65535；65536 → 1；4294967295 → 1 | 仍存在（真实 loca version 是 1–3；已知 R-12） |
| O3 | 信息 | 零译文「原地」写回会重排版式：XML 声明补 `encoding="utf-8"`、1973 个换行 → 0（整份文件压成一行）；根属性 `xmlns:xsd`/`xmlns:xsi` 保留 | 探针 `p7`/`p14`：343396 → 335499 字节，**1971 条三元组完全一致** | 仍存在（文档化的取舍；不是数据损失） |
| O4 | 信息 | `lsx::read` 对截断的 `.lsx` 静默返回部分字段（不报错）；`lsx::write` 是原地区间替换，不丢数据 | 探针 `p13`：畸形 lsx 读出 1 条、写回 Ok、文件其余内容原样 | 仍存在（写回不丢数据；影响面是 UI 少显示几行） |
| O5 | 信息 | `write_atomic` 覆盖只读文件在 Linux 成功并**丢掉只读位**；Windows 上 `rename` 会失败 | 探针 C3：`Ok(())`、权限变 644 | writer 判定「不修 + 记录跨平台差异」；我认同 |
| O6 | 信息 | `Glossary::update` 允许把 source 改成已存在的 source（重复 source）；`from_json` 不清重 | 探针 C4b/C4c：3 条输入含 2 条同名 → 匹配器同时命中 2 条互相矛盾的术语 | 部分修（web 侧行 key/busy 已修）；「唯一性收紧」经真实数据（6 组重复来自官方语料清洗）判定不修 |
| O7 | 信息 | 平均长度文档写 80.16，实测 80.1654（四舍五入应为 80.17） | §2.6 | nit，不改代码亦可 |

### 4.3 被证伪的怀疑点（我构造了攻击但打不穿，附真实输出）

| 怀疑点 | 结论 | 我的证据 |
| --- | --- | --- |
| ZIP zip-slip（`..`、反斜杠、绝对路径、UNC、`....//`、尾随空格、`..%2f`） | **证伪**：14 种形态全部落在 `zip_contents/` 内，零逃逸 | 探针 `p5a`：每次都输出「目录外逃逸 = false」；python 造的重复条目 zip 里 `../evil.txt` 被直接跳过 |
| PAK 条目穿越名 / Windows 保留名 / NTFS 数据流 | **证伪**：9 种用 `add_file` 真造出来的条目全部报错，零落盘 | 探针 `p15`：`Err("非法归档路径…")` × 9，`/tmp/evil.xml` 不存在；`safe_output_path` 29 例 fuzz 与预期 100% 一致 |
| ZIP 炸弹防御（声明大小 / 实际写入 / 条目数 / 总量 / 默认值有界） | **证伪**：7 条防线用例我复跑全绿 | `cargo test -p bg3-translate-core --lib zip_` → `7 passed` |
| 对抗译文会产出非法 XML / 丢条目 | **证伪**：36 例 × 2 风格 + 14 例验收全部合法 XML、条目零丢失 | 探针 `p1c`「非法 XML 0 例 / 条目丢失 0 例」；ACC-5 |
| SSE 跨分片切断（含多字节被切断）会错乱 | **证伪**：75 个切分点 + 逐字节喂入，事件序列与一次性喂入完全一致 | 探针 `p4a`「错乱 0 个」、`p4b` 逐字节 2 事件正确 |
| 截断 SSE 被当成成功译文 | **证伪**（HEAD 已修）：`ensure_stream_complete` 要求 `[DONE]` 或 `finish_reason`，截断 → 可重试失败 | 探针 `p4c` + 作者用例；本轮再把「流中错误载荷」补上（E-R4-01，我变异验证） |
| 真实语料零译文写回会丢条目 | **证伪**：1971 条三元组完全守恒（字节变化只是排版） | 探针 `p14`/ACC-4 |
| 完整闭环（解包→翻译→写回→重打包→再解包）会破坏产物 | **证伪**：非本地化文件字节不变、条目与译文全部保留 | 探针 `p11` |
| 「畸形底稿被整体重写」不可达（CORPUS-AUDIT §D8） | **证伪**：可达（App 层探针 + 端到端 p10） | 见 §4.1 F-A2 |
| 前端可能在 `tauri.ts` 之外直接 `invoke` 绕过契约检查 | 证伪（shell 报告结论，我复核） | `grep -rln invoke src` 只命中 `src/lib/tauri.ts` |

---

## §5 本轮新缺陷（级别 + 是否阻断）

**阻断项：0 条。** 以下是我在阶段 B 独立发现/确认、且不属于「已修」清单的项（均在 §4.2 有证据锚点）：

| 编号 | 级别 | 是否阻断 | 说明 |
| --- | --- | --- | --- |
| O1 实体往返非不动点 | 信息 | 否 | 只在源文件本身含双重转义时生效；第三轮已判定为刻意取舍，本轮无新增证据改变结论 |
| O2 loca version > 65535 静默变 1 | 信息 | 否 | 真实语料 version 1–3；已知 R-12 |
| O3 写回压平版式 | 信息 | 否 | 条目守恒；属既有取舍 |
| O4 截断 `.lsx` 读侧不报错 | 信息 | 否 | 写回不丢数据 |
| O5 只读目标丢只读位（Linux） | 信息 | 否 | writer 已记录为跨平台差异 |
| O6 重复 source 的语义 | 低 | 否 | 行 key/busy 已修；唯一性经真实数据判定不收紧 |
| O7 文档 80.16 vs 80.1654 | 信息 | 否 | nit |
| （方法学）共享 `CARGO_TARGET_DIR` 会跨源码树串产物 | 中（工程风险，非产品缺陷） | 否 | 见 §0.1；本轮所有验证均已隔离 target |

---

## §6 无法验证项（如实列出）

| 项 | 原因 |
| --- | --- |
| F-A3 的**物理覆盖**（Windows/macOS 大小写不敏感 FS 上把自带中文覆盖掉） | 本机是 Linux（大小写敏感），这类文件系统语义无法在本机执行。**我验证的是判定逻辑**：`normalizePakPath`/`findShippedFile` 把大小写不同的路径认作同一文件（ACC-F33/ACC-F33b 绿 + 变异 M-F33 红）。物理覆盖仍是推理，不写成已复现 |
| Windows 专属行为 | 只读目标上的 `rename` 失败、`system_data_dir` 的 Windows 形态、保留设备名/尾随点的真实规整结果、`.corrupt` 备份在 Windows 上的可写性 —— 本机无法执行，只有静态证据 |
| `release.yml` 的 4 段 PowerShell | 本机无 `pwsh`；只能静态审查（未发现新缺陷） |
| 真实 GitHub Actions 运行 | 本机不能跑 runner；我把 CI 里 `run:` 的原文在本地真实执行（门禁清单 3/2 道逐字符一致） |
| 真实 Tauri 运行时下的窗口 API reject / 拖放并发 | 需要真实 GUI 运行时；web 侧只验证了「reject 时给错误提示」这条分支 |
| 上游库对恶意 `.loca` 的 GB 级分配 | 无法安全执行（会造成 SIGABRT）；我用两次真实 abort（68 GB / 137 GB 分配失败）作为反向证据 |
| 四位 writer 报告的**每一条**低危项 | 时间盒内我抽验了全部高/中危与 lead 点名项（§3 的 16 组变异 + §3.1 的 7 条验收 + §3.3/§3.4 专项）。低危项中未逐条复现的，本报告不为其背书（writer 报告 §4/§5/§6 自陈的残留与无法验证项我已逐条读过并要求 lead 在收尾文档中保留） |
| 集成编辑阶段（REVIEW-ROUND4.md、README 索引、版本号 1.1.5→1.2.0 + Cargo.lock） | 按 lead 要求**分开**做：§0–§7 只覆盖代码冻结 revision `aebf22d2…`；集成编辑的增量复核见 **§8**（独立 hash） |

---

## §7 对「本轮审查能否收尾」的独立意见

**可以收尾。** 理由：

1. 冻结 revision 上**全部门禁真实通过**（verify.sh 6/6、核心 457 全绿、壳层 19/19、clippy 双零、前端 208 全绿、IPC 12 项）；
2. 我在阶段 A 独立发现的三条真实缺陷（F-A1 中危、F-A2 高危、F-A3 中低）**全部修好并被我自己写的验收探针证明**，其中 F-A2 是「静默永久删除用户条目」，修复前后行为差异由变异实验钉死；
3. lead 最担心的「loca 布局自检过严」经 ~6000 例差分 fuzz **零误拒**，且收紧型变异会被正向用例抓住 —— 该修复是**降低**风险而非引入风险；
4. 16 组独立变异**没有发现一条无效回归测试**；防线没有被静默削弱（§2.5）；
5. 残留项全部是信息/低危级、有明确归属与理由，且没有一条会导致「静默损坏用户产物」；
6. H1 == H2，验证期间无人改代码。

唯一需要在收尾文档里如实保留的是 §6 那些**无法在本机执行**的项（Windows 专属语义、GitHub Actions、
PowerShell、真实 GUI 运行时）。

---

## §8 集成编辑（版本 1.2.0）的增量复核

> 本节对应的是**另一次** revision：§0–§7 验证的是代码冻结 revision `aebf22d2…`；
> 下面是 lead 完成集成编辑（`docs/REVIEW-ROUND4.md`、README 索引、版本号 1.1.5→1.2.0 三处 + Cargo.lock）
> 之后的**增量**复核。两次 hash 分别标注。

| 项 | 值 |
| --- | --- |
| 集成编辑后代码 hash | `5a8d082e0e3db583e944b00db9a18e69`（与 lead 报的一致） |
| `Cargo.lock` md5 | `dc5d208f012d8e86373ee1bf407fd811`（与 lead 报的一致） |
| 上一节（冻结 revision）hash | `aebf22d2337e3ca498491f22216989af` |

### 8.1 「只改了这些」的独立确认

我把验证期的冻结树（`/tmp/redteam-r4/frozen`，hash `aebf22d2…`）与当前树逐文件 diff，差异**只有**：

```console
$ (在 /tmp/redteam-r4 下) for d in src crates src-tauri scripts .github README.md package.json Cargo.toml Cargo.lock bun.lock; \
    do diff -rq frozen/$d /home/jason/bg3-translate/$d; done
Files frozen/src-tauri/tauri.conf.json and …/src-tauri/tauri.conf.json differ
Files frozen/README.md and …/README.md differ
Files frozen/package.json and …/package.json differ
Files frozen/Cargo.toml and …/Cargo.toml differ
Files frozen/Cargo.lock and …/Cargo.lock differ
$ diff -rq frozen/docs /home/jason/bg3-translate/docs
Only in …/docs: REVIEW-ROUND4.md
Only in …/docs: VERIFICATION-ROUND4.md      # 本报告
```

即：源码（`src/**`、`crates/**`、`src-tauri/src/**`、`scripts/**`、`.github/**`）**零改动**，
`docs/ARCHITECTURE.md` 与 `docs/CORPUS-AUDIT.md` 也未被再动过。唯一与 lead 描述有出入的地方是
**README 的改动量**：`git diff --numstat README.md` = `23 9`（约 7 处 hunk），多于「补三行索引 + 一处版本示例」——
多出来的是数据目录/损坏备份/IPC 契约/CI 说明等段落（见 8.4 的逐条核对），内容我逐条验过。

### 8.2 ① 版本一致性与继承方式

| 位置 | 值 | 判定 |
| --- | --- | --- |
| `package.json` | `1.2.0` | ✓ |
| `src-tauri/tauri.conf.json` | `1.2.0` | ✓ |
| `Cargo.toml` `[workspace.package]` | `1.2.0` | ✓ |
| `Cargo.lock` → `bg3-translate` | `1.2.0` | ✓ |
| `Cargo.lock` → `bg3-translate-core` | `1.2.0` | ✓ |
| `crates/bg3-translate-core/Cargo.toml:4` | `version.workspace = true` | ✓ 仍继承 |
| `src-tauri/Cargo.toml:4` | `version.workspace = true` | ✓ 仍继承 |

`python3 scripts/check_ipc_contract.py` 也自报「版本号三处一致：1.2.0」「Cargo.lock 与 workspace 一致：1.2.0」。

### 8.3 ② Cargo.lock 零漂移

```console
$ git diff --numstat -- Cargo.lock
2	2	Cargo.lock
$ git diff Cargo.lock | grep -E "^[+-]" | grep -vE "^(\+\+\+|---)"
-version = "1.1.5"
+version = "1.2.0"
-version = "1.1.5"
+version = "1.2.0"
```

只有 `bg3-translate` / `bg3-translate-core` 两行版本号变化，其余依赖**零漂移**（2 行增、2 行删）。
`cargo metadata --locked --offline` 退出码 **0** ⇒ 锁文件自洽、`--locked` 可行。

### 8.4 ③ 门禁重跑（集成编辑后，我自己跑）

| 门禁 | 结果 |
| --- | --- |
| `bash scripts/verify.sh` | **6/6 通过**，总耗时 7.6s（ipc 43ms / fmt 168ms / clippy 186ms / core 999ms / web 5.5s / build 654ms） |
| 核心 `--all-targets` | **414 + 16 + 11 + 8 + 8 = 457 全绿**（与冻结 revision 逐 target 相同） |
| `cargo test -p bg3-translate --lib` | **19 passed; 0 failed** |
| `cargo clippy -p bg3-translate --all-targets -- -D warnings` | **0 告警**（`Finished dev profile`，rc=0） |
| `cargo metadata --locked --offline` | rc=**0** |
| 前端 | `Test Files 21 passed (21)` / `Tests 208 passed (208)` |

### 8.5 ④ README 改动的逐条核对（含两处非阻断不准确）

| README 新增/改动 | 我的核对 | 判定 |
| --- | --- | --- |
| 索引三行 `docs/REVIEW-ROUND4.md` / `docs/VERIFICATION-ROUND4.md` / `docs/review-r4/` | 三者都真实存在；`docs/review-r4/` 下 4 份报告齐全 | ✓ 一一对应 |
| 版本示例 `v1.2.0 ⇔ 1.2.0` | `tauri.conf.json` = 1.2.0 | ✓ |
| `BG3_TRANSLATE_HOME` 相对路径按进程 CWD 解析 | 与实现一致（我的 C1 探针实测：相对路径原样接受、落点= CWD） | ✓ |
| 系统模式 `%APPDATA%\bg3-translate\config`（Windows） | `directories` 6.0.0 文档表：Windows `config_dir` = `{FOLDERID_RoamingAppData}\<project_path>\config`；应用名 = `bg3-translate` | ✓ |
| 系统模式 `~/.config/bg3-translate`（Linux/**macOS**） | Linux ✓（`$XDG_CONFIG_HOME/<app>`，我的 C1 实测路径就是 `<XDG_CONFIG_HOME>/bg3-translate`）；**macOS ✗** —— `ProjectDirs::config_dir()` 在 macOS 是 `$HOME/Library/Application Support/<project_path>`，即 `~/Library/Application Support/bg3-translate`，**不是** `~/.config/...` | **✗ 非阻断不准确**（同一句在 `docs/ARCHITECTURE.md:55` 就已存在，本次被复制进 README） |
| 损坏文件会被改名成 `settings.json.corrupt` / `glossary.json.corrupt`、回退默认 | 与 S-05 实现一致（我的 C2/C2b + 作者用例） | ✓ |
| 「**关于**」面板里的「配置目录」就是实际生效的路径 | 该行确实显示 `info.dataDir`（`src/components/SettingsPanel.tsx:258`），但它所在的侧栏标题是「**设置**」（`src/App.tsx:446 title="设置" subtitle="大模型与界面偏好"`）；全仓 `grep 关于`（`src/**`）**0 命中**，没有「关于」面板 | **✗ 非阻断不准确** |
| IPC 契约检查新增「返回类型 / 注册完整性」描述 | 我用两处变异实证：改 `types.ts` 字段 → exit 1；加一个未注册的 `#[tauri::command]` → exit 1 | ✓ |
| CI「模式清单各自断言」描述 | `.github/workflows/ci.yml` 新增 `check_mode`，我复跑 `--core-only --no-ipc --list` = 3 道、`--web-only --no-ipc --list` = 2 道 | ✓ |
| `tauri-shell` job「真正执行命令层单元测试」 | `.github/workflows/ci.yml:242` 确有 `run: cargo test -p bg3-translate --lib`（并带注释说明为何不放进 verify.sh） | ✓ |
| 其他机器可验证断言（语料 1971 / 80.16 / 581 / 1103 / 410 / 303 / 417 / 门禁 6 道） | §2.6 已逐条实测一致 | ✓ |

版本号残留扫描：全仓除**历史性文档**（四份 writer 报告、`docs/REVIEW-ROUND4.md`、本报告引用 v1.1.5 的地方）、
`Cargo.toml` 里 `aho-corasick = "1.1.5"`（**依赖版本**，不是应用版本）之外，没有指向旧应用版本的活引用。
另有 1 处 nit：`.github/workflows/release.yml:26` 的 `workflow_dispatch` 说明里 tag 示例仍是 `v1.1.5`
（纯示例文本，不影响任何校验逻辑）。

### 8.6 §8 判定

**集成编辑通过，无阻断项。** 版本四处一致且子 crate 仍继承 workspace、Cargo.lock 零漂移且 `--locked` 可用、
全部门禁与冻结 revision 逐 target 相同、README 索引与实际文件一一对应。

建议（**不阻断提交**）在 commit 前顺手改掉两处一句话就完的文档不准确，或明确记为发布后 nit：

1. README:143 与 `docs/ARCHITECTURE.md:55`：把 macOS 的 `~/.config/bg3-translate` 改成
   `~/Library/Application Support/bg3-translate`（Windows/Linux 两段是对的）；
2. README 新增句里的「关于」面板 → 应为「设置」面板（`src/App.tsx:446`）。

若采纳第 1/2 条，代码 hash 会再次变化（新 hash 预计只影响 README/ARCHITECTURE），请把新 hash 发我，
我按 §8 的同一套检查再跑一次增量确认（约 5 分钟：hash + 四处版本 + 门禁）。

### 8.7 文档订正后的最终确认（第二次增量，hash `45ef22260768727fc8757bec13b977fd`）

lead 采纳 §8.6 的 (a)(b) 后只改了 3 行 Markdown（README 数据目录第 3 条拆出 macOS、README「关于」→「设置」、
`docs/ARCHITECTURE.md:55` 同样拆出 macOS）。我做了三项确认：

**① 变更范围（可复算的证明）**：当前树 hash = `45ef2226…`（与 lead 报的一致）。
我把 README 的**那两处编辑逐字撤回**后重算聚合 hash，得到 **`5a8d082e0e3db583e944b00db9a18e69`**
（= §8 的 hash，逐字命中）——这证明 §8→now 的 hash 变化**完全由这两处 README 编辑解释**，
其余被 hash 的文件（`src/**`、`crates/**`、`src-tauri/**`、`scripts/**`、`.github/**`、
package.json、Cargo.toml）一个字节都没动。`docs/ARCHITECTURE.md` 不在 hash 命令内，相对 §8 的差异是：

```diff
-3. **系统模式**：`%APPDATA%\bg3-translate\config`（Windows）、`~/.config/bg3-translate`（Linux/macOS）
+3. **系统模式**：`%APPDATA%\bg3-translate\config`（Windows）、`~/.config/bg3-translate`（Linux）、
+   `~/Library/Application Support/bg3-translate`（macOS）
```

（1 个逻辑行折成 2 个物理行，与 lead 说的「第 55 行」一致。）订正后的 macOS 路径与我 §8.5 的
crate 证据一致（`$HOME/Library/Application Support/<project_path>`），Windows / Linux 两段未动、仍然正确；
README 里的「设置」面板也与 `src/App.tsx:446 title="设置"` 对齐。

**② 门禁重跑**：`bash scripts/verify.sh` → **6/6 通过（7.0s）**，核心 414+16+11+8+8、
前端 `Tests 208 passed (208)`；`python3 scripts/check_ipc_contract.py` → **全过**
（含「版本号三处一致 1.2.0」「Cargo.lock 与 workspace 一致 1.2.0」——ARCHITECTURE.md 被契约脚本解析，
本次改动未影响任何被解析段落）。

**③ 版本**：package.json / tauri.conf.json / Cargo.toml[workspace.package] / Cargo.lock（两个 crate）
全部 = **1.2.0**。

**结论：最终树通过**（无阻断项）。
