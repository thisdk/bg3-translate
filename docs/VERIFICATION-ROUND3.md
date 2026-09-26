# 第三轮独立验证报告（T6 / verifier）

> 立场：四位 writer 的报告在我这里是**待验证假设**，不是结论。本报告的每一条判定都来自
> 我**自己跑出来的**命令与输出；跑不出来的进 §6「无法验证」，绝不写「应该没问题」。
> 所有探针都跑在与下面 `CODE_HASH` 一致的 **rsync 沙箱**里（`/tmp/redteam-r3/ws/after`、
> `/tmp/rt`、`/tmp/redteam-r3/mut/ws`、`/tmp/redteam-r3/ipc-mut`），**没有改动仓库任何生产文件**。

---

## §0 被验证的 revision 与两次 hash

| 时点 | 值 |
| --- | --- |
| HEAD | `422f667a0d5d3be26190c0f5b55b0796a67a7b76` |
| 开工时 CODE_HASH（lead 第一次冻结） | `de9360167c91539997ead87ea1283bb6` |
| lead 重新冻结后的 CODE_HASH | `d6fc477215ca36f5cbeeade09fe2a104` |
| **写报告前重算 CODE_HASH** | `d6fc477215ca36f5cbeeade09fe2a104`（**一致 → 验证期间无人改代码**） |
| 含 docs 的 hash（写报告前） | `bf9ea1efdd9394ab6695bf7357ca3892` |
| `git diff --diff-filter=D --name-only` | 空（**本轮零删除文件**） |

命令（lead 指定的那条，逐字）：

```console
$ cd /home/jason/bg3-translate
$ find src crates src-tauri scripts .github README.md package.json Cargo.toml -type f \
    -not -path '*/target/*' -not -path '*/__pycache__/*' | sort | xargs md5sum | md5sum
d6fc477215ca36f5cbeeade09fe2a104  -
```

关于两次冻结：lead 在我开工后追加了 3 处集成编辑（`translator.rs` 错误文案、`ci.yml` 新增
`cargo test -p bg3-translate --lib`、`README.md` 文档索引）。我在拿到新 hash 后**重跑了门禁**
（§2）与受影响的验证（§3.4）；其中 `translator.rs` 那条在旧 hash 里已包含，`ci.yml`/`README.md`
不参与 Rust/前端编译，因此 §3 的结论不受影响。

---

## §1 结论摘要表

| # | 验证项 | 结论 | 证据 |
| --- | --- | --- | --- |
| 1 | 必跑门禁 `bash scripts/verify.sh`（6/6） | **通过** | §2.1 |
| 2 | `cargo check -p bg3-translate --all-targets` | **通过**（exit 0） | §2.2 |
| 3 | `cargo clippy -p bg3-translate --all-targets -- -D warnings` | **通过**（exit 0） | §2.2 |
| 4 | 红队 6 条基线缺陷「修复前红 / 修复后绿」 | **通过 6/6** | §3.1 |
| 5 | 三层「与磁盘已有内容合并」的语义一致性 | **通过**（幂等、不推翻用户意图、无重复/乱序） | §3.3 |
| 6 | 写回闸门（前端两条路径 + 后端语义） | **通过**（App 级实测） | §3.2 |
| 7 | lead 的 4 条集成声明 | **通过 3 / 无法验证 1（Windows runner）** | §3.4 |
| 8 | `check_ipc_contract.py` 未放宽 + 新增检查有效 | **通过**（6 组变异，含 HEAD 对照） | §4.2 |
| 9 | `verify.sh --list` / CI 门禁未被改弱 | **通过**（逐字符一致、无 `continue-on-error`、CI 只增不减） | §3.5 |
| 10 | writer 新回归测试的变异有效性 | **7/8 抓到**；`pak` 默认上限那条无覆盖 | §4.1 |
| 11 | 独立重试 writer「已证伪」声明（≥3 条） | **4 条全部复现其结论** | §3.6 |
| 12 | 本轮新缺陷 | **无 blocking**；4 条低/信息级见 §5 | §5 |

**通过 11 项、部分通过 1 项、失败 0 项。**

---

## §2 必跑门禁的真实输出

### 2.1 `bash scripts/verify.sh`（在 lead 重新冻结后重跑）

```console
── [1/6] 跨层 IPC 契约检查
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
   ✓ 通过（49ms）
── [2/6] Rust 代码格式（cargo fmt --check）      ✓ 通过（138ms）
── [3/6] Rust Clippy 零告警                      ✓ 通过（225ms）
── [4/6] Rust 核心库测试（含端到端 PAK 闭环）
test result: ok. 326 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored      ← e2e_pak_flow
test result: ok. 7 passed; 0 failed; 0 ignored      ← real_mod_sample
   ✓ 通过（957ms）
── [5/6] 前端单元测试（vitest）
 Test Files  20 passed (20)
      Tests  190 passed (190)
   ✓ 通过（5.3s）
── [6/6] 前端类型检查 + 构建（tsc -b && vite build）  ✓ 通过（693ms）

verify.sh exit=0
```

### 2.2 Tauri 壳层（本机真实编译）

```console
$ cargo check -p bg3-translate --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s
exit=0
$ cargo clippy -p bg3-translate --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s
exit=0
```

### 2.3 独立复核 IPC 契约（脚本本身的输出）

```console
$ python3 scripts/check_ipc_contract.py
命令：注册 17 个，前端使用 17 个，文档列出 17 个
…（10 条 ✓，见 2.1）
✓ IPC 契约检查全部通过
```

---

## §3 逐条复核

### 3.1 红队基线 6 条：修复前红 → 修复后绿（全部独立复现）

工具：`/tmp/redteam-r3/verify/<id>.sh <工作区>`（BEFORE = `/tmp/rt`，内容与 HEAD 逐字节一致）。

```console
$ bash /tmp/redteam-r3/verify/all.sh /home/jason/bg3-translate
R-01   BEFORE=红(缺陷复现)     AFTER=绿(缺陷未复现)
R-02   BEFORE=红(缺陷复现)     AFTER=绿(缺陷未复现)
R-03   BEFORE=红(缺陷复现)     AFTER=绿(缺陷未复现)
R-04   BEFORE=红(缺陷复现)     AFTER=绿(缺陷未复现)
R-05   BEFORE=红(缺陷复现)     AFTER=绿(缺陷未复现)
R-10   BEFORE=红(缺陷复现)     AFTER=绿(缺陷未复现)
```

| 红队编号 | 对应 writer | 我的独立判据（与修复手法无关） | 判定 |
| --- | --- | --- | --- |
| R-01 截断流被当成成功译文 | engine F-02 | 服务端发 `finish_reason=length` 后断流时，**不得出现 `Done`**；修复后实测 `translated: 0, failed: 1` + `Error(模型输出不完整（finish_reason=length）…)` | 真实已修 |
| R-02 写回产出非法 XML | formats F-01/F-02/F-03/F-04 | `render` 产物必须能被再解析且条目数不变；修复后 `再解析 error = None，条目数 = 1`（属性值含裸 `>` 与 `</ b>` 两种输入都过） | 真实已修 |
| R-03 `file_name` 路径逃逸 | formats F-08 | 工作目录之外**不得**出现被写入的文件；修复后相对/绝对都被 `Err(非法归档路径)` 拒绝 | 真实已修 |
| R-04 zip 解压无上限 | formats F-10 | 手工构造 ZIP64 归档（**声明**解压后 8 GiB、实际 232 字节）：必须在写盘前被拒 + 落地字节 < 8 MiB。修复后：`Err(Pak("zip 条目 payload.pak 声明解压后 8589934592 字节，超过单文件上限 6442450944，已中止解压"))`，179 µs、落地 0 字节 | 真实已修 |
| R-05 英文覆盖已有中文 | frontend F-28 | 见 §3.2 的 App 级 payload 断言 | 真实已修 |
| R-10 保真校验忽略标签属性 | engine F-01 | `check_fidelity(<LSTag Type="Action">, <LSTag 类型="Action">)` 必须非空；修复后 `[MissingTag{"<LSTag Type>"}, ExtraTag{"<LSTag 类型>"}]` | 真实已修 |

> **R-04 的口径更正（重要）**：我在准备期用的判据是「64 MiB 炸弹落地字节必须变小」，
> 那个判据**严于** writer 实际采用的政策（单文件 6 GiB / 总量 16 GiB），因此当时误报「仍红」。
> 现在改用「声明超限必须在写盘前被拒」这一与政策无关的判据，并用手工构造的 ZIP64 归档
> 让**两侧输入完全相同**，结论才是可信的。这是我自己探针的一次口径错误，记录在此。

### 3.2 写回闸门（前端两条路径 + 后端语义）

| 层次 | 我做的实验 | 真实输出 | 判定 |
| --- | --- | --- | --- |
| 前端（目标文件**非** MOD 自带） | App 级挂载 + 把条目置成 `translating` + `target="半截译文"` + `runToken === null`，点击「完成翻译，去打包」 | `T6-GATE fileName = Localization/Chinese/x.xml landed = ["Fireball","Ice"]` | **闸门成立**（半截译文没进 payload） |
| 前端（`runToken` 非空） | 同上但先 `beginRun()` | `是否被拦 = true`，提示「翻译仍在进行中…」 | 成立 |
| 前端（目标文件自带） | R-05 三条用例（未翻译/error/已翻译混合） | `payload = [["uid-1","火球"],["uid-2","寒冰"]]` 等 | 成立 |
| 后端 core | `write_entries_to_path(translating + 半截 target)` / `error + target` | translating：**落盘半截译文 = true**；error：落盘原文 = true | 后端**仍只认 `error``（设计） |

结论：**写回闸门确实成立**，但它的唯一执行点是前端 `planLocalizationWrites` 里的
`entries.map(toWritableEntry)`（`src/lib/localization.ts:86`）—— 命令层直接调用
`write_file_entries` 时，`translating` 条目的半截文本仍会落盘（§5 F-3，非阻塞，纵深防御建议）。

### 3.3 三层「与磁盘已有内容合并」是否语义打架（我新增的集成探针）

探针：`/tmp/redteam-r3/t6-probes/rt_t6_merge.rs`（7 个用例，全部走公开 API）。

```console
$ cargo test -p bg3-translate-core --offline --test v_t6_merge
A1 第一次写回（只提交 h1） 共 3 条 / 第二次写回与第一次逐字节一致 = true
A2 清空译文后落盘文本 = ["火球", "寒冰"] / 只提交 h1 后条目数 = 2
A3 error 条目退回原文 = true / 磁盘独有条目保留 = true
A4 第二次 uid 顺序 = ["h1","h2","h9"] / 有重复条目 = false / 两次写回逐字节一致 = true
A5 uid=h1 ver=1 text="火球" / uid=h3 ver=3 text="Bolt" / 两次写回逐字节一致 = true
test result: ok. 7 passed; 0 failed
```

| 担心（lead 提出） | 实测结论 |
| --- | --- |
| 同一文件写两次是否幂等 | **幂等**：`content_list` 与 `.loca` 两条路径第二次写回与第一次**逐字节一致**（A1/A4/A5） |
| 用户手工删掉一条条目会不会被 core 补回 | core **会补回**（`merge_missing_entries` 按 contentuid 补），但这是 formats F-05 的**有意设计**（防止只在中文文件里存在的 contentuid 被永久删除）；UI 没有「删除条目」入口，写回列表始终是整份文件，所以不会推翻用户意图 |
| 用户把某条译文清空会不会被还原成旧译文 | **不会**：清空后落盘仍是该条目自己的文本（`["火球","寒冰"]`），未被磁盘旧 target 还原（A2） |
| 两条合并路径叠加会不会产生重复条目/顺序错乱 | **不会**：`["h1","h2","h9"]` 无重复、顺序稳定、两次打包逐字节一致（A4，且 A4 用的是**逐行复刻** `mergeWithExistingTarget` 的语义，含「incoming 无译文→保留底稿」规则） |
| `error` 条目在两层合并下会不会把坏译文写回 | **不会**：core `effective_text()` 退回原文，且前端保留 `status:"error"`（A3 + writer 的 App 级用例） |

> **一次自我纠错**：A4 的第一版探针里我把前端合并语义**简化**成「incoming + 底稿独有」，
> 漏了「incoming 无译文时保留底稿」这条规则，于是误报「已有中文被英文覆盖」。
> 逐行对照 `src/lib/localization.ts:115-149` 重写后结论反转为通过。误报原因是我自己的探针 bug，
> 不是产品缺陷 —— 如实记录，避免被当成 writer 的问题。

### 3.4 lead 的 3+1 处集成编辑

| 编辑 | 我的验证 | 判定 |
| --- | --- | --- |
| ① `translator.rs::ensure_stream_complete` 文案 + `ARCHITECTURE.md` 引用 | 用本地假服务端复现「无 `[DONE]` 也无 `finish_reason`」：`Error(…流式响应结束，但既没有收到 [DONE] 也没有 finish_reason，无法确认输出完整（连接可能被中途掐断）)`，与文档一致 | 通过 |
| ② `ci.yml` 新增 `cargo test -p bg3-translate --lib` | 本机 `cargo test -p bg3-translate --lib` → **`15 passed; 0 failed`**；`git diff .github/workflows/ci.yml` 只有 `+8` 行、**零删除**、无 `continue-on-error`；`tauri-shell` job 的 step 数与该步骤位置与 lead 描述一致 | **本机通过；Windows runner 上是否通过 → §6（无法验证）** |
| ③ `README.md` 文档索引 | `README.md:140-143` 索引 4 项；`docs/REVIEW-ROUND3.md` 与 `docs/VERIFICATION-ROUND3.md` 目前**不存在**（后者由本报告创建、前者由 T7 创建） | 通过（本报告落地后只剩 `REVIEW-ROUND3.md` 待 T7 关闭） |
| ④ `ARCHITECTURE.md` 测试策略与 ci.yml 一致 | `docs/ARCHITECTURE.md:352-363`：「命令层有单元测试 `cargo test -p bg3-translate --lib`…**同一 job 还会真正执行这些单元测试**」与 `ci.yml:213-214` 的实际步骤一致 | 通过 |

### 3.5 `verify.sh` 清单与 CI 是否被改弱

```console
$ bash scripts/verify.sh --list
ipc	python3 scripts/check_ipc_contract.py
core-fmt	cargo fmt --all --check
core-clippy	cargo clippy -p bg3-translate-core --all-targets -- -D warnings
core-test	cargo test -p bg3-translate-core --all-targets
web-test	bun run test
web-build	bun run build
```

* 与 `ci.yml:86-92` 的 `expected="$(printf …)"` **逐字符一致**（6 行全同）。
* `grep -rn "continue-on-error" .github/workflows/` → **无匹配**。
* `git diff --stat -- .github/` → 只有 `ci.yml | 8 ++++++++`（**纯新增**，无删除）。
* `.github/**` 里 `-D warnings` 仍在（core job 经 verify.sh，tauri-shell job 显式）。

### 3.6 独立重试 writer「已证伪」的声明（我自己的方法）

| writer 的证伪结论 | 我的独立方法 | 我的真实输出 | 结论 |
| --- | --- | --- | --- |
| formats：「zip 里的符号链接条目会写出符号链接 → 证伪」 | 用 python 手工构造带 `S_IFLNK`（`external_attr=0o120777<<16`）且内容为 `/etc/passwd` 的 zip 条目，跑 `open_and_extract_in`，用 `symlink_metadata` 判定 | `条目 evil.pak … 是符号链接 = True`（输入确认是链接）→ 解出后 `evil.pak 是符号链接 = false 大小 = 11`（普通文件） | **复现其结论** |
| formats：「`pick_largest_pak` 同体积时结果不稳定 → 证伪」 | 自建 `a.pak`/`b.pak`（各 10 字节）+ `c.pak`（3 字节），连跑 5 次；再加入 20 字节的 `z.pak` | `连跑 5 次 = [Some("a.pak") × 5]`（取字典序最小）；`加入 z.pak 后 = Some("z.pak")`（体积优先） | **复现其结论** |
| engine：「不发 `[DONE]` 但发 `finish_reason` → 仍然成功」 | 本地假服务端只发内容 + `finish_reason:"stop"` 就断流 | `Done{text:"火球造成伤害"}`、`AllDone{total:1,failed:0}`、无 `Error` | **复现其结论**（同时反证 R-01 的修复没有把正常网关误杀） |
| shell：「README『内置 102 条官方术语』属实」 | 自己数 | `grep -c "    SeedEntry" …/glossary/seed.rs` → `102` | **复现其结论** |

### 3.7 其余 writer 声明的批量复核（可独立观察的那部分）

探针：`/tmp/redteam-r3/t6-probes/rt_t6_claims.rs`（8 个用例，`test result: ok. 8 passed`）。

| 声明 | 我的实测输出 | 判定 |
| --- | --- | --- |
| engine F-03 不再双重前缀 | `消息 = "大模型调用错误: API 返回 429 Too Many Requests: "`，前缀出现 **1** 次 | 真实已修 |
| engine F-04 401/404 不重试 | `401 → 请求数 = 1，耗时 933µs`；`404 → 请求数 = 1`；对照 `429 → 请求数 = 4` | 真实已修 |
| engine F-05 SSE 缓冲上限 | 2 MiB 无换行 → `Error("…SSE 数据行已超过 1048576 字节仍没有换行…已中止本次请求")` | 真实已修 |
| engine F-06 baseUrl 带 query | `https://h.test/v1/chat/completions?api-version=1` | 真实已修 |
| engine F-07 NaN 温度 | `NaN → 0.3` | 真实已修 |
| engine F-08 CJK 扩展区 | **未验证**：`contains_cjk` 未从 `translation` 模块导出，`app` 层拿不到（§6） | 未验证 |
| formats F-07 非法控制字符 | 产物 `带控制字符的译文`（`\0`/`\u{1}`/`\u{b}` 全部丢弃），再解析 `error = None` | 真实已修 |
| formats F-09/F-13 `".. "` 与 NUL | `".. "`、`"evil. "`、`" ..."` → `Err(非法归档路径（片段以点或空格结尾…）)`；`"a\0b.txt"` → `Err(非法归档路径（含 NUL 字节）)` | 真实已修 |
| formats F-08 写回路径校验 | `"../evil.xml"`/`"/tmp/evil-abs.xml"`/`"..\..\evil.xml"` 全部 `Err(非法归档路径)` | 真实已修 |
| types：只有 `error` 失去可写回译文 | `error 后 has_writable_target = false`；`translating → has_writable_target = true` | 与声明一致（后者即 §5 F-3） |

**逐条复核的覆盖说明（诚实边界）**：四份报告合计 30 余条 F-xx，我用上面 6 类实验
（红队 6 条 + 批量 8 条 + 集成 7 条 + 闸门 2 条 + lead 4 条 + 变异 8 组）覆盖了其中的
**高危与中危全部条目**，以及低危中的可观察条目。**没有**逐条重跑的低危/信息项
（如 `.lsx` 大小写、`glossary` 边界语义、`config` 目录回退）只在 §6 列为未覆盖。

---

## §4 变异测试记录

工具：`/tmp/redteam-r3/mut/revert.sh`（整文件还原）、`/tmp/redteam-r3/mut/targeted.sh` +
`/tmp/redteam-r3/mut/targeted2.sh`（精准变异）。`revert.sh` 的机制先用 R-02 做过负向自检
（把 `content_list.rs` 还原成 HEAD ⇒ `MUT=红(缺陷复现)`），所以下面每条「红」都可以归因到那次代码改动。

### 4.1 对 writer 新回归测试的变异（精准变异，保留测试本身）

> 说明：先试过「整文件还原成 HEAD」，但 writer 的新回归测试就写在**同一个文件**的
> `mod tests` 里，整文件还原会把测试一起删掉（`content_list` 还原后只剩 17 条旧测试、全绿）
> —— 这种变异**无效**。所以改用「只改修复点、保留测试」的精准变异。

| # | 精准变异 | 跑的测试 | 结果 |
| --- | --- | --- | --- |
| M-A | `content_list`：`if on_disk.is_empty()` → `if true \|\| on_disk.is_empty()`（关掉补回） | `cargo test -p bg3-translate-core content_list::` | **红** ✓ |
| M-B | `content_list`：`sanitize_xml_chars` 早退条件改成 `if true`（不清洗） | 同上 | **红** ✓ |
| M-C | `loca`：`merge_with_existing` 早退 `if !path.is_file()` → `if true` | `cargo test … loca::` | **红** ✓ |
| M-D | `pak`：`ZipLimits::default().max_entry_bytes` → `u64::MAX` | `cargo test … pak::` | **绿** ✗ 见 §5 F-1 |
| M-E | `formats`：`checked_disk_path(..)?` → `resolve_disk_path(..)`（跳过校验） | `cargo test … formats::` | **红** ✓ |
| M-F | shell：`checked_work_root` 的 `is_same_dir` 判定 → `if false` | `cargo test -p bg3-translate --lib` | **红** ✓ |
| M-G | 前端：`planLocalizationWrites` 去掉 `entries.map(toWritableEntry)` | `bunx vitest run`（全量 20 文件） | **红** ✓（`src/lib/localization.test.ts` → “translating 条目退回原文，target 不进入写回请求”） |
| M-H | 前端：`mergeWithExistingTarget` 的「保留底稿」规则 → `if (false)` | `bunx vitest run src/App.localization-merge.test.tsx` | **红** ✓ |

**有效回归测试 7/8。** M-G 我第一次只跑了 `App.write-guard.test.tsx`（名称最像的用例），
得到「绿」；改为全量前端测试后由 `localization.test.ts` 抓到 —— 教训：**变异必须跑全量测试，
不能凭文件名猜**。M-D 见 §5。

### 4.2 `scripts/check_ipc_contract.py` 的变异（HEAD 版 vs 现在版对照）

方法：把工作区 rsync 到 `/tmp/redteam-r3/ipc-mut`，施加变异后分别用**现在版**与
`git show HEAD:scripts/check_ipc_contract.py` 跑一遍，比较退出码。

```console
== 未变异时（基线）==          新脚本 exit=0    HEAD 脚本 exit=0
M1 命令名写错                  新脚本 exit=1    HEAD 脚本 exit=1
M2 参数名 camelCase            新脚本 exit=1    HEAD 脚本 exit=0
M3 TS 结构体字段名             新脚本 exit=1    HEAD 脚本 exit=0
M4 枚举变体多一个              新脚本 exit=1    HEAD 脚本 exit=1
M5 文档命令表改名              新脚本 exit=1    HEAD 脚本 exit=1
M6 Rust serde 字段改名         新脚本 exit=1    HEAD 脚本 exit=0
```

* **没有任何一项从「HEAD 能抓」变成「现在抓不到」** → 未放宽。
* 新增的参数名/字段检查确实有效（M2/M3/M6：HEAD 漏检、现在抓住）。
* `git diff` 显示该文件 `371 insertions(+), 4 deletions(-)`，被删的 4 行经逐行检查**全是文档字符串/注释**
  （“（以及 ubuntu CI）上编译不了…”“4. `TranslationEvent`…”等），**没有删除任何检查逻辑**。

---

## §5 我新发现的缺陷与观察

**没有任何 blocking（B-xx）级缺陷。** 以下 4 条为低/信息级；另有 1 条承接红队未修项。

| 编号 | 级别 | 现象 | 证据 | 建议归属 |
| --- | --- | --- | --- | --- |
| **F-1** | 低（测试覆盖缺口，非产品缺陷） | `pak` 的 zip 解压**默认上限**（单文件 6 GiB / 总量 16 GiB）**没有任何测试钉住**：把默认值改成 `u64::MAX`（等于彻底关掉防线），`cargo test … pak::` 仍然 **22 passed 全绿**。writer 的炸弹单测用的是**注入的小上限**（`max_entry_bytes: 1<<20`），只覆盖机制不覆盖默认值 | 变异 M-D（§4.1）；默认值行为由我的 R-04 端到端探针覆盖（`超过单文件上限 6442450944`） | formats-auditor（若还有一轮） |
| **F-2** | 信息（已声明的取舍） | 上限仍然宽松：单文件 6 GiB / 总量 16 GiB，恶意 zip 仍可在**小容量磁盘**上写出十几 GB 才中止（中止时会删掉半个文件）。writer 在代码注释里明确写了「4K 材质 MOD 可能好几 GB，上限必须宽松」 | `pak.rs:331-357` 注释 + 我的 8 GiB 声明归档在写盘前被拒 | lead 决定是否收紧 |
| **F-3** | 信息（纵深防御建议） | 命令层直接调 `write_file_entries` 时，`status="translating"` + 半截 target **会落盘**（core `effective_text()` 只对 `error` 退回原文）。当前唯一防线是前端 `planLocalizationWrites` 里的 `entries.map(toWritableEntry)`；我用 App 级点击实测该防线在**两条路径**（自带/非自带目标文件）都成立 | §3.2 表格；`rt_t6_merge` A6 | shell/engine（可选） |
| **F-4** | 信息（临时状态） | `README.md:140-143` 的文档索引里 `docs/REVIEW-ROUND3.md` 与 `docs/VERIFICATION-ROUND3.md` 当前**不存在**（悬空链接）。本报告落地后只剩 `REVIEW-ROUND3.md` 待 T7 创建 | §3.4 ③ | T7（写 REVIEW-ROUND3.md 即闭合） |
| **F-5** | 低（承接红队，未在本轮范围内） | 红队 R-13~R-16（系列/planner）**本轮无人修**：`plan_jobs` 对同一 base 会发两次请求、`is_variant_suffix` 把以 `{n}` 结尾的普通句子当变体、记忆 base 会被污染后缀、合成用半角空格。R-14 只构造样例能触发，**真实样本 41 条里 0 例** | `docs/review-r3/redteam-baseline.md` §9 | 下一轮 / backlog |

---

## §6 无法验证项与原因

| 项 | 原因 |
| --- | --- |
| **`ci.yml` 新增的 `cargo test -p bg3-translate --lib` 在 Windows runner 上是否通过** | 本机是 Linux（`uname` → WSL2 x86_64），只能跑 Linux 分支：`15 passed`。测试里用 `cfg!(windows)`/`#[cfg(unix)]` 做了分支，但 Windows 侧的路径语义（尾随点/空格、盘符、大小写不敏感）我无法在本机执行。**只能静态判断该步骤没有放宽任何东西 + YAML 结构正确**。 |
| 真机 BG3 能否加载本轮产出的 PAK | 没有游戏本体。我能验证到「解包→原样重打包→逐字节一致」（红队 §3-2）与「产物是合法 XML」为止。 |
| 真实 LLM 网关行为（`finish_reason=length` 的真实比例、是否发 `[DONE]`、`Retry-After`） | 无 API key 且不允许外部请求；R-01/engine F-03/F-04/F-05 都用 127.0.0.1 假服务端复现，只能证明「这类响应会/不会被正确处理」，不能给出线上发生率。 |
| `engine F-08`（`contains_cjk` 是否覆盖 CJK 扩展 F/G/H/I） | `contains_cjk` 是 `series` 模块的 `pub fn`，但没有从 `translation` 模块再导出，`app` 层拿不到 → 我无法在集成测试里直接调；只看到 writer 的单测通过。 |
| GitHub Actions 各 action 版本真实可用性（`checkout@v7` 等） | 本机不跑 CI（也无外网）。shell writer 声称用 `curl` 查过 tag，我**未复验**，故不作为我的结论。 |
| 红队/ writer 报告中标注「低危/信息」但我未逐条重跑的项 | `.lsx` 属性大小写、`glossary` 的 `\b` 边界语义（266/19524 条）、`config.rs` 数据目录四级回退、前端 2 万条性能断言的具体耗时数字。这些需要各自的专用探针，本轮时间用在了集成面与变异测试上。 |
| `docs/REVIEW-ROUND3.md` 的内容 | 属于 T7，本报告不预判。 |

---

## §7 我对「本轮审查能否收尾」的独立意见

**可以收尾。** 理由（全部有 §2~§4 的证据支撑）：

1. **门禁全绿且没有被放宽**：`verify.sh` 6/6、壳层 check/clippy `-D warnings` exit 0、
   326+7+7 Rust 测试、20 文件/190 前端测试、`build` 通过；`verify.sh --list` 与 CI 期望逐字符一致，
   `.github/**` 只有 8 行新增、零删除、无 `continue-on-error`；IPC 契约脚本 4 组对照变异证明
   **只增强未削弱**。
2. **高危项全部独立复现为「真实已修」**：我自己的 6 条红队探针在冻结版上全部 `BEFORE=红 / AFTER=绿`，
   其中 R-01（截断流）、R-02（非法 XML）、R-03（路径逃逸）、R-04（zip 上限）都用了与 writer 不同的
   判据与不同的输入。
3. **跨 writer 集成面没有打架**：三层合并语义实测**幂等、不推翻用户意图、不产生重复/乱序**；
   写回闸门在两条前端路径上都成立（App 级点击实测）；lead 的 4 条集成编辑中 3 条通过、
   1 条（Windows runner）如实列入无法验证。
4. **回归防线是真实有效的**：8 组精准变异里 7 组让 writer 的新测试变红。

需要 lead 在收尾时处理的两件事（都不构成阻塞）：

* **F-1**：给 `ZipLimits::default()` 补一条断言默认值的测试（或至少把默认值写进一条 e2e 断言）。
  现状是「默认值可以被改成无限而上限防线静默失效」，这是我唯一发现的**防线强度**缺口。
* **F-4**：T7 写完 `docs/REVIEW-ROUND3.md` 后，README 索引的悬空链接自动闭合；
  若 T7 决定不写，请改 README 措辞。

另外把 **F-5（红队 R-13~R-16）** 记入下一轮 backlog 或已知取舍：它们本轮无人认领，
且 R-14 在唯一的真实样本上 0/41 触发。

> **（T6 后增量复核见 §8：F-1 已闭环，收尾意见据此更新为「只剩 F-4」。§0~§7 的历史内容未改写。）**

---

## §8 T6 后增量复核（F-1）

**触发**：lead 采纳 §5 的 F-1（`pak` 的 zip 默认上限没有测试钉住），在
`crates/bg3-translate-core/src/pak.rs` 的 `#[cfg(test)] mod tests` 末尾新增测试
`zip_limits_default_stays_bounded`。新冻结值 `CODE_HASH = 1189d7aa17ba5302e75681ec411918c0`。
本节只做增量复核，**不重跑 T6**。

### 8.1 增量确认：生产代码零改动、只多了测试（我自己核对的，不依据 lead 描述）

方法：把当前工作区的 `pak.rs` 与我在 T6 验证时使用的副本
（`/tmp/redteam-r3/ws/after/…/pak.rs`、`/tmp/redteam-r3/ipc-mut/…/pak.rs`，两者同为 T6 冻结态）
做 `diff -u`。

```console
$ diff -u /tmp/redteam-r3/ws/after/crates/bg3-translate-core/src/pak.rs \
          /home/jason/bg3-translate/crates/bg3-translate-core/src/pak.rs
@@ -1074,4 +1074,45 @@
         assert!(pak.is_file());
         assert!(pak.ends_with("Inner.pak"));
     }
+
+    /// `ZipLimits::default()` 必须被钉住。
+    …（新增 41 行，全部位于 `mod tests` 内）
+    #[test]
+    fn zip_limits_default_stays_bounded() { … }
 }
```

**唯一一个 hunk、纯新增 41 行、全部落在 `mod tests` 内。** 再用「测试区之前的生产代码」
做逐字节比对，两侧 md5 完全相同：

```console
$ for f in /tmp/redteam-r3/ws/after/…/pak.rs  /home/jason/bg3-translate/…/pak.rs; do
    awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" | md5sum; done
333f1dbccf6de343b68c044e2f8134bc  （T6 副本）
333f1dbccf6de343b68c044e2f8134bc  （当前工作区）
```

`impl Default for ZipLimits` 仍是 **6 GiB / 16 GiB / 200_000**（`pak.rs:346-357`），未被改动。
→ **「只增加测试、生产代码一行未改」成立。**

### 8.2 复跑 M-D 变异：现在**红**

精准变异（只替换 `impl Default` 里那一行的值，`assert count == 1` 保证唯一命中），
**变异只发生在沙箱副本** `/tmp/redteam-r3/mut/ws`，工作区全程未被触碰：

```console
$ bash /tmp/redteam-r3/mut/md.sh /home/jason/bg3-translate
变异已应用：max_entry_bytes 6 GiB -> u64::MAX（impl Default 内，唯一一处）
--- 变异后的那一行 ---
350:            max_entry_bytes: u64::MAX,
…
test pak::tests::zip_limits_default_stays_bounded ... FAILED
exit=101
M-D 判定 = 红(测试抓到了变异)
```

失败断言原文：

```console
$ cargo test -p bg3-translate-core --offline zip_limits_default_stays_bounded
thread 'pak::tests::zip_limits_default_stays_bounded' panicked at crates/bg3-translate-core/src/pak.rs:1089:9:
单条目默认上限过大，压缩炸弹防线等于失效: 18446744073709551615
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 326 filtered out
```

→ **F-1 闭环**：T6 时同样的变异是「绿(测试没抓到变异)」，现在变红。

**恢复确认**（我没有改工作区，但按要求给出证据）：

```console
$ grep -n "max_entry_bytes: u64::MAX" crates/bg3-translate-core/src/pak.rs   → 无匹配
$ diff -u (T6 副本) (当前) | 新增行只有测试函数体内的断言/字符串          → 无生产代码残留
$ diff -q /tmp/redteam-r3/mut/ws/…/pak.rs  /home/jason/bg3-translate/…/pak.rs
（沙箱副本也已用工作区内容覆盖回原状）→ 逐字节一致 ✓
```

### 8.3 门禁重跑（真实尾部输出）

```console
$ cargo test -p bg3-translate-core --all-targets
running 327 tests
test result: ok. 327 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.69s
running 7 tests      test result: ok. 7 passed; 0 failed
running 7 tests      test result: ok. 7 passed; 0 failed
exit=0

$ bash scripts/verify.sh
── [1/6] 跨层 IPC 契约检查        ✓ 通过（44ms）
── [2/6] Rust 代码格式            ✓ 通过（134ms）
── [3/6] Rust Clippy 零告警       ✓ 通过（194ms）
── [4/6] Rust 核心库测试
test result: ok. 327 passed; 0 failed; 0 ignored; 0 measured
test result: ok. 7 passed; 0 failed        test result: ok. 7 passed; 0 failed
   ✓ 通过（873ms）
── [5/6] 前端单元测试   Test Files 20 passed (20) / Tests 190 passed (190)   ✓ 通过（5.1s）
── [6/6] 前端类型检查 + 构建          ✓ 通过（645ms）
exit=0
```

Rust 单测数 **326 → 327**（正好 +1，与新增 1 条测试一致）；其余数字不变。

### 8.4 CODE_HASH

```console
$ find src crates src-tauri scripts .github README.md package.json Cargo.toml -type f \
    -not -path '*/target/*' -not -path '*/__pycache__/*' | sort | xargs md5sum | md5sum
1189d7aa17ba5302e75681ec411918c0  -
```

与 lead 给的新冻结值**完全一致**；结合 8.2 的恢复确认，说明变异没有污染工作区、也没有人在这期间改代码。

### 8.5 更新后的收尾意见

**可以收尾，且 F-1 / F-4 均已闭环。**

* **F-1 已闭环**：`zip_limits_default_stays_bounded` 经精准变异证明能拦住「把默认上限改成无限」
  这种关掉防线的改动（8.2）。
* **F-4 也已闭环**：写这一节时复核发现 `docs/REVIEW-ROUND3.md` 已经落地，README 索引
  四项全部指向真实文件：

  ```console
  $ for f in docs/VERIFICATION-ROUND2.md docs/REVIEW-ROUND3.md docs/VERIFICATION-ROUND3.md docs/review-r3; do
      printf '%-32s %s\n' "$f" "$([ -e "$f" ] && echo 存在 || echo 不存在)"; done
  docs/VERIFICATION-ROUND2.md      存在
  docs/REVIEW-ROUND3.md            存在
  docs/VERIFICATION-ROUND3.md      存在
  docs/review-r3                   存在
  ```

  即 §5 F-4 描述的「悬空链接」已消失。

非阻塞项只剩 F-2（6 GiB/16 GiB 仍属宽松取舍）、F-3（后端对 `translating` 不设防，建议纵深防御）、
F-5（红队 R-13~R-16 记入下一轮 backlog）。这三条都不影响本轮收尾。
