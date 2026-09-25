# 第二轮独立验证报告（teammate `verifier`）

> 这是**证伪记录**，不是 writer 自述的复述。每条结论后面跟着我**实际执行过的命令**与真实输出片段。
> 没跑过的一律标「无法验证」，绝不写「通过」。三个 writer 的报告在本轮里只当作**待验证假设**。
>
> 验证者红线遵守情况：只读源码；唯一写入的仓库文件是本报告；所有探针、副本、变异实验都在 `/tmp/**`。
> 未执行 `git commit / checkout / reset / stash / clean`；`samples/` 的临时改名实验已原样恢复（见 §6）。

---

## 0. 被验证的确切 revision

```
$ git rev-parse HEAD
fb01d2e3e6314d2f4178f18acb304d28aa131cc5      # 基线，工作区在改造前是干净的

$ find src crates src-tauri scripts .github docs README.md package.json \
      -type f -not -path '*/target/*' | sort | xargs md5sum | md5sum
d85a3802e2ed0952f77e053fe8191389  -            # = Lead 冻结的 WORKTREE_HASH，开工前/报告前各算一次，两次一致

$ git status --porcelain | wc -l
30                                             # 14 改 + 16 新增；无 commit、无删除、无重命名
$ git diff --diff-filter=D --name-only
                                               # 空：本轮没有删除任何文件
```

环境：Linux；`cargo 1.95.0` / `rustc 1.95.0` / `bun 1.3.13` / `node v22.23.3` / `Python 3.13.15`；
`yq` 可用；**没有 `pwsh`**（release.yml 的 PowerShell 段只能静态审查，见 §9）。

复核方式说明：所有**基线对照**都在 `/tmp/bg3-probe`（HEAD 的 `tar` 副本 + `node_modules` 符号链接）里跑，
真实仓库零改动；被验证的实现通过 `rsync` 同步进同一个副本后再跑同一批探针。

---

## 1. 结论摘要

| # | 验证项 | 结论 |
| --- | --- | --- |
| 1 | `python3 scripts/check_ipc_contract.py` | **通过**（17 == 17 == 17，exit 0） |
| 2 | `cargo fmt --all --check` | **通过**（exit 0） |
| 3 | `cargo clippy -p bg3-translate-core --all-targets -- -D warnings` | **通过**（exit 0，零告警） |
| 4 | `cargo test -p bg3-translate-core --all-targets` | **通过**（244 + 5 + 6 = 255，全绿） |
| 5 | `bun run test` | **通过**（13 文件 / 151 用例） |
| 6 | `bun run build` | **通过**（tsc + vite，464.32 kB / gzip 143.65 kB） |
| 7 | `bash scripts/verify.sh` 全绿 + **失败路径** | **通过**（6/6 门禁；9 类变异全部被拦住并透传退出码，见 §7） |
| 8 | A 保真校验误报 / 漏报 | **通过**：24 例黑盒 + 55 行表驱动，**0 误报 0 漏报**；4 项已知取舍见 §8 |
| 9 | B 重试与纠错语义 | **通过**（10 个探针：1/2/4 次请求、Error+failed+1、无重复 Done、取消干净、Series 在合成文本上校验） |
| 10 | C 前端丢字 / 串条目 / 乱序 / 索引 | **通过**（56 个探针；2 万条下 0 丢字、0 串条目、0 乱序、迟到 delta 两道防线有效） |
| 11 | C2 复现性能数字 | **部分通过**：批处理收益复现（通知 5000→1、注入 436ms→2ms）；**「O(1) 索引」的收益复现不出来**（见 F-06） |
| 12 | D `git diff` 语义审查 | **通过**：无删除文件、无新增生产 `unwrap/expect/panic/#[allow]`、serde/IPC 字段零改动、测试名 0 删除；12 处生产改动逐条定性见 §5 |
| 13 | D2 `docs/ARCHITECTURE.md` 承诺逐字成立 | **通过**（命令表 17/17，含参数列；公开 API 编译探针逐条核对） |
| 14 | E 样本缺失不再算通过 | **通过**（真实仓库实测：lib 2 红 + 真实样本 5 红，exit 101；恢复后 `git status` 与实验前逐字一致） |
| 15 | 壳层 API 未被破坏 | **通过**（`cargo check -p bg3-translate --all-targets` 本机真实编译，exit 0） |
| — | 发现的缺陷 | **1 中 + 4 低 + 3 信息**（无阻断、无高危） |
| — | 无法验证 | **6 项**（见 §9） |

---

## 2. 必跑门禁（命令 + 真实输出）

### 2.1 IPC 契约（17 条命令三处一致）

```
$ python3 scripts/check_ipc_contract.py
命令：注册 17 个，前端使用 17 个，文档列出 17 个
✓ 前端 invoke 的每个命令都已在后端注册
✓ 后端注册的命令全部写进了架构文档
✓ 架构文档没有虚构的命令
✓ TranslationEvent 与前端联合类型一致：all_done delta done error progress
✓ TranslationStatus 与前端联合类型一致：edited error pending translated translating
✓ PakFileKind 与前端联合类型一致：data-txt localization-loca localization-xml metadata-lsx other script-lua
✓ IPC 契约检查全部通过
IPC_EXIT=0
```

### 2.2 Rust 门禁

```
$ cargo fmt --all --check
FMT_EXIT=0

$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s
CLIPPY_EXIT=0

$ cargo test -p bg3-translate-core --all-targets
     Running unittests src/lib.rs …            test result: ok. 244 passed; 0 failed
     Running tests/e2e_pak_flow.rs …           test result: ok. 5 passed; 0 failed
     Running tests/real_mod_sample.rs …        test result: ok. 6 passed; 0 failed
TEST_EXIT=0

$ cargo check -p bg3-translate --all-targets   # 壳层（本机有 webkit2gtk，能真编）
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s
CHECK_EXIT=0
```

对照基线（同一台机器、`/tmp` 副本）：`205 + 5 + 4 = 214`。本轮 `244 + 5 + 6 = 255`。

### 2.3 前端门禁

```
$ bun run test
 ✓ src/lib/utils.test.ts (8)   ✓ src/lib/entries.test.ts (29)   ✓ src/store/app-store.test.ts (?)
 ✓ src/components/FileDropZone.test.tsx (7)   ✓ src/components/FileTree.test.tsx (7)
 ✓ src/components/SettingsPanel.test.tsx (8)  ✓ src/components/GlossaryPanel.test.tsx (11)
 ✓ src/components/TranslationTable.test.tsx (7)  ✓ src/components/translation-table/TranslationTable.streaming.test.tsx (3)
 ✓ src/store/streaming.perf.test.ts (1)  …
 Test Files  13 passed (13)
      Tests  151 passed (151)
WEBTEST_EXIT=0

$ bun run build
dist/index.html                   0.40 kB │ gzip:   0.29 kB
dist/assets/index-DSUdqI8X.css   44.12 kB │ gzip:   8.63 kB
dist/assets/index-D4-pX5IF.js   464.32 kB │ gzip: 143.65 kB
BUILD_EXIT=0
```

基线对照：6 文件 / 92 用例 → 13 文件 / 151 用例。

### 2.4 `scripts/verify.sh` 全绿

```
$ bash scripts/verify.sh
── [1/6] 跨层 IPC 契约检查            ✓ 通过
── [2/6] Rust 代码格式                ✓ 通过
── [3/6] Rust Clippy 零告警           ✓ 通过
── [4/6] Rust 核心库测试              ✓ 通过
── [5/6] 前端单元测试（vitest）        ✓ 通过（8.9s）
── [6/6] 前端类型检查 + 构建           ✓ 通过（1.7s）
✓ 全部 6 道门禁通过（总耗时 13.2s）
VERIFY_EXIT=0
```

`bun run verify` 接线正确：`bun run verify -- --list` 会转发给 `bash scripts/verify.sh --list`。

---

## 3. A. 保真校验的误报与漏报（本轮核心新逻辑）

### 3.1 方法

两套**我自己写的**探针，都不在仓库里：

* `probe_fidelity_direct.rs`：直接对 `translation::check_fidelity` 做 55 行表驱动，逐行断言「必须保真 / 必须判违规」，并打印实际 issue 列表。
* `probe_fidelity.rs`：**黑盒**。假翻译器对每个 source 恒定返回候选译文，跑真实 `TranslationEngine::run`，用「是否出现 Error 事件」判定是否命中校验（假实现永不返回 `Err`，所以 Error 只可能来自结构校验）。

跑法：

```bash
bash /tmp/probes/run-probe-rust.sh probe_fidelity_direct
bash /tmp/probes/run-probe-rust.sh probe_fidelity
```

### 3.2 误报面：32 行全部「保真」，0 误报

| 类别 | 用例（source → target） | 实测 |
| --- | --- | --- |
| 数学/文本尖括号 | `Deal < 5 damage` → `造成 < 5 点伤害` | 保真 |
| | `HP > 50`、`a < b > c`、`<5> items`、`if (a>=b && c<d)`、`use x < y` | 全部保真 |
| 实体还原 | `Use &lt;i&gt; tag` → `使用 <i> 标签`（双向） | 保真 |
| 白名单外标签 | `<name>`、`<color=red>`、`<FONT size=1>`、`<?xml …?>` | 全部保真 |
| LSTag 属性 | 属性值被翻译、属性顺序变化、Tooltip 译成中文 | 全部保真 |
| br 形态 | `a<br>b<br/>c` ↔ `甲<br>乙<br/>丙`；`<br>` ↔ `<br/>`；`<br />` | 全部保真（规范化为 `<br/>`） |
| 嵌套/重复标签 | `<b><i>x</i></b>`、`<i>a</i> and <i>b</i>` | 保真 |
| 占位符合法保留 | `{0}`、`{10}`、`{user_name}`、换位 `{1} {2}` ↔ `{2} {1}` | 保真 |
| 非占位符花括号 | `{}`、`{ }`、`{"k": 1}`、`{#FFAA00}`、`{-1}`、未闭合 `{1` | 保真 |
| 双重花括号 | `Use {{1}} literally` ↔ `字面使用 {{1}}`（内层 `{1}` 两边一致） | 保真 |
| 标点/大小写 | `Fireball!` → `火球术。`、`FIREBALL` → `火球术` | 保真 |

黑盒面同样 10/10 保真，全部 `calls=1 → Done`（即**没有触发多余请求**）。

### 3.3 漏报面：15 行全部被抓住，0 漏报

| 反例（source → target） | 判定 |
| --- | --- |
| `Deal {1} damage` → `造成伤害` | 违规：`miss_ph:{1}x1` |
| `Deal damage` → `造成 {1} 点伤害` | 违规：`extra_ph:{1}x1` |
| `Deal {1} damage` → `造成 {1}{1} 点伤害` | 违规：`extra_ph:{1}x1` |
| `Deal {1} damage` → `造成 {2} 点伤害` | 违规：`miss_ph:{1}` + `extra_ph:{2}` |
| `{1} deals {2} to {3}` → `造成伤害` | 违规：三个全缺 |
| `Deal {1} damage` → `造成 ｛1｝ 点伤害`（全角） | 违规 |
| `Deal {1} damage` → `造成 { 1 } 点伤害`（加空格） | 违规 |
| `Hello {name}` → `你好 {名字}` | 违规 |
| `<LSTag Type="Spell">Fireball</LSTag>` → `火球术` | 违规：`<LSTag>` + `</LSTag>` 都缺 |
| `<i>Fireball</i>` → `<i>火球术` / `火球术</i>` | 违规（缺 `</i>` / 缺 `<i>`） |
| `Fireball` → `<i>火球术</i>` | 违规：多出标签 |
| `a<br>b` → `甲乙` | 违规：`miss_tag:<br/>` |

黑盒面 7/7 全部走到 `calls=2 → Error("…结构校验未通过…")`。

### 3.4 边界面（判定与设计一致，4 项属已知取舍）

| 用例 | 实测 | 评价 |
| --- | --- | --- |
| 标签顺序对调 / 嵌套改变 | `tag_order` 违规 | 合理 |
| `&lt;i&gt;x&lt;/i&gt;` ↔ `<i>x</i>` | 保真 | **有意取舍**，但写回会双重转义 → 见 F-02 |
| `<i/>x` ↔ `<i></i>x` | 违规（miss `<i/>` + extra `<i></i>`） | 潜在误报，风险低 → F-03 |
| `<LSTag Type="Spell" Tooltip="Fireball">` → 删掉 Tooltip | 保真 | 已知取舍 → F-04 |
| 空译文 `Fireball` → `""` | 保真，`Done("")` | 预存在 → F-05 |
| 整句回抄原文 | 保真 | 只比结构不比内容，符合文档 |
| `<i>Fireball</i>` → `<i></i>`（正文被删空） | 保真 | 同上（只比结构） |
| 属性里的 `{1}` | 需要保留，缺了会报 | 合理（属性原样写回） |

### 3.5 结论

**误报 0、漏报 0**（在我覆盖的 79 个用例上）。设计上「宁可漏报不可误报」的取舍被实测证实：所有合法译文都只发 1 次请求。

---

## 4. B. 重试与纠错语义

探针：`probe_retry.rs`（10 个用例，自写假翻译器，覆盖 `translate` 与 `translate_with_correction`）。

```
bash /tmp/probes/run-probe-rust.sh probe_retry
test result: ok. 10 passed; 0 failed
```

| 用例 | 实测（原文摘录） | 判定 |
| --- | --- | --- |
| B1 合法译文 | `calls=1 done=1 error=0 all_done=1 corrections=[]` | 通过：**不多发请求** |
| B2 网络一直失败 | `calls=4 done=0 error=1 all_done=1 elapsed=3.504s failed=1` | 通过：退避 500/1000/2000 语义未变 |
| B3 结构坏→好 | `calls=2 done=1 error=0 elapsed=99.7µs`；纠错提示=`上一轮译文缺少占位符 {1}，请重新只输出译文…` | 通过：**恰好 2 次请求**，且不等网络退避 |
| B4 结构一直坏 | `calls=2 error=1 done=0 all_done=1 failed=1`；message=`大模型调用错误: 结构校验未通过：占位符 {1} 缺失（已重试 1 次）` | 通过：Error + failed+1 + **无 Done** |
| B5 网络失败后成功 | `calls=2 done=1 error=0` | 通过：网络线路未被纠错线路挤掉 |
| B6 被拒尝试的 delta | `delta=4 streamed="造成伤害造成 {1} 点伤害" done="造成 {1} 点伤害"` | **记录**：坏译文已流到前端，Done 会覆盖 → F-01/F-09 |
| B7 退避中取消 | `calls=1 done=0 error=0 all_done=1 cancelled=true 213ms` | 通过：取消干净，0.6s 内返回 |
| B8 纠错请求在飞行中取消 | `calls=2 done=0 error=0 all_done=1 cancelled=true 169ms` | 通过：取消**不算**结构失败 |
| B9 已取消 | `calls=0` | 通过：连请求都不发 |
| B10 Series 合成校验 | 两个成员 `Silver's Hair {1}/{2}`：`calls=2 done=2 error=0 hints=[多出占位符 {1}]`；Done=`银色发型{1}` / `银色发型{2}` | 通过：**校验跑在合成后的完整译文上** |

补充：`translate_with_correction` 是 trait 的**默认方法**（默认忽略提示、退回 `translate`），既有自定义实现不需要改 —— 我在探针里只实现 `translate` 也能编译通过，符合文档承诺。

---

## 5. C. 前端高频路径（2 万条条目）

### 5.1 探针与结果：56 个用例全绿

```
bash /tmp/probes/run-probe-web.sh
 ✓ src/__probe__/batcher-probe.test.ts   (18 tests)
 ✓ src/__probe__/store-probe.test.ts     (19 tests)
 ✓ src/__probe__/delta-probe.test.tsx    (13 tests)
 ✓ src/__probe__/perf-probe.test.tsx     ( 1 test)
 ✓ src/__probe__/filetree-probe.test.tsx ( 5 tests)
 Test Files  5 passed (5)     Tests  56 passed (56)
```

**不丢字 / 不串条目 / 不乱序：**

| 场景 | 实测 |
| --- | --- |
| 同帧 1000 条 delta（单条目） | 期望 4890 字符 = 实际 4890 字符；store 通知 **1** 次（基线 1000） |
| 三条目交错 3000 条 delta | A/B/C 各 4890 字符，逐字节相等；未触碰的条目保持空 |
| 倒序注入 3 条 delta | 结果 = 到达顺序（`开头中间末尾`） |
| 跨 4 个真实帧到达 | `第一帧第二帧第三帧第四帧`（顺序不变） |
| 搜索过滤隐藏条目后注入 delta | 隐藏条目与可见条目都精确更新（**delta 路由不依赖可见列表**） |
| 1 万条 delta / 1 千条目（原语级） | 逐条比对每条目文本，0 不一致；恰好 1 次 commit |

**批量刷新边界（本轮最容易出错的地方）：**

| 场景 | 基线 HEAD | 本轮 |
| --- | --- | --- |
| `delta` 与 `done` 同帧 | 目标=`最终译文` | 目标=`最终译文`（`discard` 生效） |
| `done` **之后**再来 delta | 目标=`最终译文迟到碎片`（**污染**） | 目标=`最终译文`（`settledIdsRef` 生效，基线缺陷已修） |
| `all_done` 后未 flush 的 delta | 目标=`""`、状态 `pending` | 同左（`discardAll` + 回滚） |
| 取消后迟到的 delta | 目标=`""`、状态 `pending` | 同左 |
| 卸载时仍有未 flush 的 delta | 不抛错 | 不抛错，`dispose()` 先落地 |

**store 索引正确性：**

| 场景 | 实测 |
| --- | --- |
| 重复 `setFileEntries` 同一文件 | 旧 id 从 `entryIdToFile`/`entryIdToIndex` **一并清除**（基线会残留陈旧映射） |
| 整表替换后旧下标被新条目占用 | 旧 id 的 delta **不会写到新条目上**（`locateEntry` 的 `list[index].id !== id` 拒写） |
| 三个文件各 2 万条、同名 contentuid | 三条 delta 各自落到自己的文件（id 内嵌路径，天然不冲突） |
| 两文件含**同名 entry id** | 单值索引只命中最后 set 的文件 —— **但生产不可达**：真实样本 41 条 id 全局唯一、`PakFile.name` 唯一、`id == {source_file}#{contentuid}`（`probe_ids.rs`） |
| 同文件数组内重复 id | last-wins（基线是 first-wins）；真实样本 0 例，不可达 |
| 未知 id / 空批次 | no-op，且**不通知**订阅者（0 次） |
| 三文件同名 uid + 索引最大下标 | 落点全部正确 |

### 5.2 性能复现（我自己的基准，不引用 writer/Lead 的数字）

| 指标（2 万条条目） | 基线 HEAD | 本轮 | 说明 |
| --- | --- | --- | --- |
| 同帧注入 5000 条 delta | **436 ms** | **2 ms** | 纯 store 写入路径 |
| 5000 条 delta 的 store 通知 | **5000** | **1** | 批处理的核心收益 |
| 5000 条 delta（200 条目 × 25）通知 | 5000 | **1** | 与并发条目数无关 |
| 注入后内容错误条目 | 0 | 0 | 正确性未因批处理下降 |
| 渲染行数（虚拟滚动） | 14 | 14 | 未回退 |
| 单次 `appendDelta`（2 万条） | 0.093 ms | 0.080 ms | 约 14% |
| 2000 次 `appendDelta` 的通知 | 2000 | 2000 | 单元素路径仍是每次一通知（符合设计） |
| 一次 `applyDeltas`（5000 条 / 5000 条目） | — | 2.5 ms / 1 次通知 | |
| 2 万条下批量 1 万次 delta（末尾区域） | — | 1.9 ms | |

**结论：批处理收益复现（两个数量级的通知下降、注入耗时 436ms→2ms）；「O(1) 索引」的墙钟收益复现不出来** → F-06。

---

## 6. D. 行为保持与 `git diff` 审查

### 6.1 机械检查（可复算）

| 检查 | 方法 | 结果 |
| --- | --- | --- |
| 是否删除文件 | `git diff --diff-filter=D --name-only` + `git status` | **0 个** |
| 是否有测试被删除/改名 | 提取 HEAD 与当前的 `#[test]`/`#[tokio::test]` 函数名集合做差 | core `205 → 244`，**旧有新无 = 0** |
| 前端用例是否被删 | 提取 HEAD 与当前的 `it("…")` 标题集合做差 | `92 → 151`，**旧有新无 = 0** |
| 既有测试是否被弱化 | `src/components/TranslationTable.test.tsx` 与 HEAD 比对 md5 | **完全相同**（`fb3fe8fd…`），7 用例仍全过 |
| 生产代码新增 panic 点 | 提取非 `cfg(test)` 区域的 `unwrap()/.expect(/panic!/todo!/unimplemented!/#[allow` 行做多重集差 | 旧 3 → 新 3，**新增 0、消失 0** |
| 新增调试残留 | 生产文件里搜 `console.`/`debugger` | **0 处** |
| serde / IPC 字段 | `src/lib/types.ts`、`crates/.../types.rs` 是否在改动清单里 | **都不在**（零改动），且契约脚本通过 |
| 组件拆分是否丢逻辑 | 取 HEAD 版 `TranslationTable.tsx` 的 35 个声明，在新目录里逐一查找 | 3 个"找不到"全部是等价替换：`allEntries`→`useEntryDerivation` 一次遍历；`appendDelta`→`applyDeltas`；`onTranslated`→`done` 分支内联 |
| `#[cfg(test)]` 边界 | `engine.rs:53` 的 `mod tests;`、`mod.rs:32` 的 `test_support` | 均为 `cfg(test)`，不计入生产面 |

### 6.2 逐条定性（12 处生产改动）

| 改动 | 定性 | 依据 |
| --- | --- | --- |
| `engine.rs` 1365 → 342 行，拆出 `engine/tests.rs`/`events.rs`/`translator.rs`/`retry.rs`/`fidelity.rs`/`test_support.rs` | **等价重构 + 有意新行为** | 公开路径编译探针通过（§6.3）；生产 panic 点集合完全相同；重试/取消/事件语义由 10 个黑盒探针复算 |
| `translation/mod.rs`：新增 4 个私有 `mod` + 顶层再导出 | **等价重构** | `translation::engine::{…}` 等拆分前路径仍可解析；`mod fidelity` 为私有，同名函数在 `translation::` 下再导出 |
| `retry.rs`：网络重试 4 次 + 结构纠错 1 次（不占额度） | **有意的新行为** | B1/B2/B3/B4 实测 |
| `fidelity.rs`：新增结构签名校验 | **有意的新行为** | §3：0 误报 0 漏报 |
| `glossary/matcher.rs`、`glossary/store.rs`：样本缺失由「跳过」改为「失败」 | **有意的新行为（测试防线）** | 只改 `mod tests` 内的辅助函数；§7 → E |
| `tests/real_mod_sample.rs`：4 处跳过改硬失败 + 新增 2 个用例 | **有意的新行为** | E 实测 |
| `app-store.ts`：新增 `entryIdToIndex`/`applyDeltas`/`locateEntry` | **有意的新行为** | §5；`appendDelta` 保留为单元素路径 |
| `TranslationTable.tsx` → 7 行 barrel + `translation-table/` 目录 | **等价重构** | `@/components/TranslationTable` 导出不变；原测试文件逐字节未动且全过 |
| `FileTree.tsx`：新增 `onToggleFiles` 一次算完整组勾选 | **有意的新行为（修既有 bug）** | A/B 实测：旧实现 4/5 探针失败（「全选」只剩最后一个、「取消全选」留下 2 个），新实现 5/5 通过 |
| `app-store.test.ts`：+128/−1（只改了 1 行 import 加 `vi`，其余全是新增用例） | **只增不删** | 新用例覆盖索引错位拒写、零 `findIndex` 调用 |
| `.github/workflows/ci.yml`：core/web 改走 `verify.sh`、meta 增加门禁完整性断言 + tomllib 版本继承断言、加 `timeout-minutes` | **等价加固** | `--list` 逐字比对本地复现通过；`bash -n` 通过；`yq` 解析通过 |
| `.github/workflows/release.yml`：`RELEASE_TAG` 语义修正 + 0 字节产物门禁 + 标签冲突校验 + 权限最小化 | **有意加固** | `on.push.tags: v*` 限定下语义安全；`yq -e` 解析得到 `permissions=contents: read`（workflow 级）、`publish` 单独 `contents: write` |
| `README.md` / `docs/ARCHITECTURE.md` / `package.json` | **文档 + 脚本接线** | 见 §6.3、F-08 |

### 6.3 `docs/ARCHITECTURE.md` 承诺逐字成立

* **命令表**：脚本已经断言「注册 == 前端 == 文档 == 17」。我额外核对了**参数列**（脚本不查这列）：

  ```
  open_mod/filePath  extract_mod/filePath,outputDir  read_file_entries/workDir,fileName
  write_file_entries/workDir,fileName,entries  repack_mod/workDir,outputPath
  translate_entries/workDir,entries,styleHint,onEvent  save_llm_settings/settings
  add_glossary_entry/entry  update_glossary_entry/oldSource,entry  delete_glossary_entry/source
  import_glossary/jsonStr        # 11 条带参命令：文档 camelCase == 前端 invoke 键 == Rust snake_case，逐条一致
  close_mod / cancel_translation / load_llm_settings / list_glossary / reset_glossary / app_info
                                 # 6 条无参命令：文档写「—」，前端 invoke 不带参，Rust 只有注入的 State
  ```

* **公开 API**：自写编译探针 `probe_api.rs` 逐条核对文档里那段 Rust 代码，并同时引用**拆分前的旧路径**：

  ```
  bash /tmp/probes/run-probe-rust.sh probe_api
  test documented_api_signatures_are_verbatim ... ok
  ```

  覆盖：`EventSink::emit` 签名、`CancelToken::{new,cancel,is_cancelled,reset}`、`TranslationSummary` 四字段（Copy+PartialEq）、`RunOptions<'a>` 三字段、`TranslationEngine::{new,with_settings,with_translator,settings,run}`、`translation::engine::{…}` 旧路径类型别名与顶层再导出**是同一个类型**、`planner/prompt/series/sse` 全部公开项可取地址。

---

## 7. `scripts/verify.sh` 与 `check_ipc_contract.py` 的失败路径（变异测试）

全部在 `/tmp/bg3-probe` 副本里做，每个变异跑完立刻从仓库恢复原文件并逐文件 `cmp` 校验。

| 变异 | 期望 | 实测退出码 | 结论 |
| --- | --- | --- | --- |
| 未知参数 `--bogus` | 2 | **2**（打印用法） | 通过 |
| `--core-only --web-only` | 2 | **2** | 通过 |
| 缺工具（PATH 里没有 bun，`--web-only`） | 2 | **2**（列出缺什么） | 通过 |
| `--list` 在**空 PATH**（`env -i PATH=/nonexistent`） | 0，不执行任何命令 | **0**，TAB 分隔打印 6 道门禁 | 通过 |
| 代码格式改坏 | 非 0 | **1**（`✗ core-fmt 退出码 1`） | 通过 |
| Clippy 告警（`-D warnings`） | 非 0 | **101**（fmt 先通过，clippy 拦下） | 通过 |
| 改坏一个既有断言 | 非 0 | **101**（core-test） | 通过 |
| 改坏前端用例 | 非 0 | **1**（web-test） | 通过 |
| 引入 TS 类型错误 | 非 0 | **1**（web-build / tsc） | 通过 |
| 删掉后端注册的一个命令 | 1 | **1**（同时报「前端 invoke 了但未注册」与「文档列出但未注册」，计数 16 vs 17） | 通过 |
| 前端 `invoke` 一个不存在的命令 | 1 | **1** | 通过 |
| 文档命令表加一行虚构命令 | 1 | **1**（文档腐烂） | 通过 |
| 改 `TranslationEvent` 变体名（`Done`→`Finished`） | 1 | **1**（契约不一致） | 通过 |

> 说明：我第一次跑的「删注册命令」变异是**我的脚本 no-op**（匹配串带了尾逗号但文件里是行尾），当时脚本 exit 0；
> 修正变异后确认脚本能抓到。记录在这里以免混淆「工具漏检」与「我的变异没生效」。

---

## 8. E. 静默跳过（样本缺失不再算通过）

在**真实仓库**里执行，`trap` 保证任何情况下都恢复：

```bash
$ mv samples samples.bak
$ cargo test -p bg3-translate-core --all-targets
test glossary::matcher::tests::real_glossary_matching_is_fast_and_ordered ... FAILED
test glossary::store::tests::real_glossary_import_filters_noise_but_keeps_terms ... FAILED
真实术语表样本缺失或不可读：…/samples/bg3-official-glossary.json（No such file or directory …）
    请执行 `git checkout -- samples/` 或重新 clone
test result: FAILED. 242 passed; 2 failed
E1_EXIT=101

$ cargo test -p bg3-translate-core --test real_mod_sample
test real_localization_files_read_without_errors ... FAILED
test real_meta_lsx_name_is_not_translatable ... FAILED
test real_nexus_mod_unpacks_and_classifies_correctly ... FAILED
test real_mod_survives_translate_write_repack_roundtrip ... FAILED
test realistic_translations_are_never_flagged_by_structure_check ... FAILED
test missing_sample_fails_loudly_instead_of_skipping ... ok      # 这条是「防线的防线」，用假路径，故意不受影响
test result: FAILED. 1 passed; 5 failed
E2_EXIT=101
```

基线对照：HEAD 上这 5 个真实样本用例会 `eprintln!` 后 `return`，测试仍然全绿（静默变空）。
本轮 **7 个用例在样本缺失时真的红**，错误信息可操作（含 `git checkout -- samples/`）。

恢复证据：

```
$ find samples -type f -print0 | sort -z | xargs -0 md5sum
68d49afbc9bc9399d8224c8e98aa794b  samples/Appearance Edit Enhanced-899-3-1-3-1769898497.zip
1ee169f9f72ede3cbe55f50e0b63594a  samples/bg3-official-glossary.json

$ cargo test -p bg3-translate-core --test real_mod_sample   # 恢复后
test result: ok. 6 passed; 0 failed

$ git status --porcelain | wc -l
30                                   # 与实验前逐字一致（此处尚未写入本报告）
```

---

## 9. 缺陷清单

> 我**没有**修改任何源码。以下均由 Lead 决定是否返工。

### F-01（中）结构校验失败的坏译文仍会被导出写回 PAK —— 新防线可从导出路径绕过

> **复验状态（见 §10.1）：已修复。** task-5 引入 `TranslationEntry::has_writable_target()`
> （`target` 非空 **且** `status != error`），三个格式的写回全部走该闸门；我用三格式探针、
> `edited` 反例、真实样本重打包闭环复验通过。下面保留**修复前**的原始复现记录。

**链路**（三步都有实测支撑）：
1. `error` 分支只丢弃**未提交**的 delta、把状态置为 `error`，**不清 `target`**：
   `src/components/translation-table/hooks/useTranslationRun.ts:122-130`。
2. `App.onGoPack` 把 `entriesByFile` 原样交给 `write_file_entries`（`src/App.tsx:55`，`planLocalizationWrites`，**不看 status**）。
3. `content_list::write` / `loca::write` 用 `effective_text()`：`target` 非空就取 `target`（`types.rs:215`），`lsx::write` 只判 `has_target()`。

**最小复现**：

```bash
bash /tmp/probes/run-probe-web.sh src/__probe__/delta-probe.test.tsx
# [D7] {"beforeError":"造成伤害","target":"造成伤害","status":"error"}
#      → 结构校验失败后 target 里留着「造成伤害」（原文是 Deal {1} damage）

bash /tmp/probes/run-probe-rust.sh probe_writeback
# [错误条目导出产物]
# <content contentuid="h00000001" version="1">火球术 法术</content>      ← 漏了 <i> 标签的坏译文
# <content contentuid="h00000002" version="1">造成伤害</content>          ← 漏了 {1} 的坏译文
```

**加剧点**：结构纠错会先流一轮被拒的 delta（探针 B6：`streamed="造成伤害造成 {1} 点伤害"`），一直不合格时两轮文本都会留在 `target`（拼接），导出即写出一段"两轮拼接"的文本。

**影响**：用户不点「重试 N 条失败」直接导出时，恰好是本轮想拦住的坏译文进了 PAK。
**性质**：机制是预存在的（网络中断也能留下半截 target），但**可达性被本轮显著提高**（结构不合格 → 纠错 → 仍不合格 是常见终态）。
**建议**：`error` 分支同时 `target: ""`（`effective_text()` 会退回原文，游戏里表现为未翻译），或在导出前过滤 `status === "error"`。

### F-02（低）`&lt;i&gt;` 形式能通过保真校验，但写回后变成可见的转义文本

```
[保真校验] source="The <i>Fireball</i> spell" target="&lt;i&gt;火球术&lt;/i&gt; 法术" → is_faithful=true
[写回产物]  &amp;lt;i&amp;gt;火球术&amp;lt;/i&amp;gt; 法术
```

XML 解码后是字面 `&lt;i&gt;…`，游戏里会显示成转义文本而不是斜体。
这是 `restore_entities` 的**有意取舍**（避免把"原文本来就是转义文本"误判），但后果没写进文档 → 见 F-08。

### F-03（低）自闭合非空标签与开闭对被判为结构不一致（潜在误报）

```
check_fidelity("<i/>x", "<i></i>x") → [miss_tag:<i/>x1, extra_tag:<i>x1, extra_tag:</i>x1]
```

XML 语义等价。真实语料里 `<i/>` 罕见，误报代价是白重试 1 次 + 该条目报错，故判低。

### F-04（低）标签属性丢失不报

`<LSTag Type="Spell" Tooltip="Fireball">` → `<LSTag Type="Spell">` 判为保真；Tooltip（游戏内提示）丢失。ARCHITECTURE 明确写了「属性值不参与比较」，属已知取舍，此处只记录后果。

### F-05（低）空译文 / 回抄原文算成功（预存在）

`check_fidelity("Fireball", "")` 保真 → 引擎 `Done("")`，条目被标为「已翻译」但 `target` 为空。
下游 `effective_text()` 会退回原文，因此不会写空；影响是进度统计把这条算成完成。基线行为相同，非本轮引入。

### F-06（信息）「O(1) 索引」的性能收益复现不出来

* writer 自己的基准（`src/store/streaming.perf.test.ts` 的 `locate` 行）连跑两次：
  `scan=177.0ms index=177.6ms` / `scan=280.9ms index=341.0ms` —— 索引并不更快，有时更慢。
* 我的独立测量：2 万条下单次 `appendDelta` 0.093ms（基线）→ 0.080ms（本轮），约 14%。
* 原因：每次提交仍要 `list.slice()` 拷贝 2 万条数组，定位那一步不是瓶颈。
* 评价：`entryIdToIndex` + `locateEntry` 在**正确性**上有额外价值（错位拒写，实测有效），但对外表述应避免把「索引」与「批处理」的收益混在一起；真正的墙钟收益来自按帧批处理（通知 5000→1）。

### F-07（信息）`appendDelta` 已无生产调用方

组件走 `applyDeltas`；`appendDelta` 只出现在 `app-store.test.ts` 与 `streaming.perf.test.ts`（后者用它做基线对照）。保留有理，但属"仅测试使用"的公开 API。

### F-08（文档）README / ARCHITECTURE 的「不会把坏译文静默写出去」在导出路径上不成立

> **复验状态（见 §10.4）：已对齐。** README 已改为描述真实机制（写回退回原文 / 手工编辑成 `edited` 才写回），
> 并补上 F-02 的后果；ARCHITECTURE 新增「写回不变量」一节。逐句核对表见 §10.4。

README 写「不发出「完成」事件，也就不会把坏译文静默写出去」；ARCHITECTURE 写「绝不发 `Done`，坏译文不会静默当成功」。翻译路径的表述正确，但**导出路径**（F-01）仍是通道，措辞应收紧为「不会被当作成功译文写回」或先修 F-01。

### F-09（低）结构纠错期间 UI 会短暂显示两轮拼接文本

> **复验状态（见 §10.2）：已修复。** core 在每次新尝试开始前重发 `progress(translating)`
> （`retry.rs::emit_attempt_progress`），前端据此把该条目判为「重试开始」并清空上一轮文本；
> 我用 10 个自造事件序列探针覆盖网络退避两轮 / 结构纠错两轮 / 一直不合格 → error 三种场景。

探针 B6：被拒尝试的 delta 已经在事件流里，`done` 到达前 `target` 是 `造成伤害造成 {1} 点伤害`。终态由 `Done` 覆盖，属瞬时显示问题；但若最终失败，这段拼接文本会留下（→ F-01）。

---

## 10. 无法验证清单

| # | 项 | 原因 | 替代证据 |
| --- | --- | --- | --- |
| 1 | GitHub Actions 上 workflow 的真实运行 | 本会话不触发任何 GHA（Lead 也明确要求不要触发） | `yq -e` 真解析两个 workflow；meta job 的三段逻辑（`bash -n` + `--list` 逐字比对 + tomllib 版本继承）在本机**等价复现通过**；core/web job 改走 `verify.sh` 后，其门禁命令与本地 `verify.sh` 完全同源 |
| 2 | `release.yml` 里的 PowerShell 段 | 本机**没有 `pwsh`**（`command -v pwsh` 返回非 0） | 静态审查（`RELEASE_TAG` 语义、`on.push.tags: v*`、0 字节门禁、标签冲突分支）；`permissions` 用 `yq` 实测为 `contents: read` + publish job `contents: write` |
| 3 | Windows 上的 Tauri 壳编译 | Linux 环境 | `cargo check -p bg3-translate --all-targets` 本机真实编译通过（exit 0），比只比签名更强 |
| 4 | 真实 LLM API 的行为 | 不做真实网络调用 | 用可注入的假翻译器覆盖请求次数、纠错提示内容、取消、失败路径（§4）；prompt 文本本身未做端到端模型验证 |
| 5 | 真实游戏客户端对写回文本的渲染 | 无法运行 BG3 | 用写回后的 XML 产物 + XML 实体语义推断（F-01/F-02 的结论都建立在**实际产物字符串**上，不是猜测） |
| 6 | `requestAnimationFrame` 在真实 WebKit 隐藏窗口下的节流 | jsdom 无法模拟 | 原语级探针覆盖了「调度器不触发时数据留在 pending、`flush()` 能落地」这条路径（B9）；真实节流行为未测 |

---

## 11. 探针与复现清单（全部在 `/tmp`，不进仓库）

```bash
# 把仓库当前状态同步进 /tmp 探针副本（探针文件不常驻，避免污染 fmt/clippy/vitest/tsc）
bash /tmp/probes/sync.sh

# Rust 对抗性探针
bash /tmp/probes/run-probe-rust.sh probe_fidelity         # 黑盒 24 例：误报面/漏报面/边界面
bash /tmp/probes/run-probe-rust.sh probe_fidelity_direct  # 表驱动 55 行
bash /tmp/probes/run-probe-rust.sh probe_retry            # 重试/纠错/取消 10 例
bash /tmp/probes/run-probe-rust.sh probe_ids              # 真实样本 id 唯一性
bash /tmp/probes/run-probe-rust.sh probe_writeback        # 写回路径后果（含 F-01/F-02 证据）
bash /tmp/probes/run-probe-rust.sh probe_api              # ARCHITECTURE 公开 API 逐条核对

# 前端对抗性探针
bash /tmp/probes/run-probe-web.sh                         # batcher/store/delta/perf/filetree 共 56 例
bash /tmp/probes/run-probe-web.sh src/__probe__/delta-probe.test.tsx   # 单跑

# verify.sh + check_ipc_contract.py 变异测试
bash /tmp/probes/mutate.sh
```

基线与本轮的关键对照数字（同机、同探针）：5000 条 delta 注入 436ms → 2ms、store 通知 5000 → 1、
`done` 之后迟到 delta 的污染（基线复现、本轮修复）、FileTree 整组勾选 4/5 失败 → 5/5 通过。

---

# 10. 复验（F-01 / F-09 / F-08）

> 本节是 task-5 / task-6 / task-7 / task-9 落地后的**复验**。方法同上：只信我自己跑出来的东西。
> 我把上一轮验证的快照冻结在 `/tmp/bg3-prev`（指纹 `d85a3802e2ed0952f77e053fe8191389`），
> 因此本轮能做**逐文件精确 diff**，而不是靠 writer 的自述。

## 10.0 复验对应的 revision 与真实变更面

**指纹记账必须写清楚，因为 Lead 给的那条命令的输入集里包含我自己要改的文件**
（`docs/VERIFICATION-ROUND2.md` 在 `docs/` 下）。实测：

```
# ① 开工时（03:02 前后）用 Lead 的原命令，输入集 = 13 个改动文件 + 546 行的上一轮报告
$ find src crates src-tauri scripts .github docs README.md package.json \
      -type f -not -path '*/target/*' | sort | xargs md5sum | md5sum
54147be900a774a69493229c67d4e786  -      # 与 Lead 给的值逐字一致 → 我验证的就是被冻结的 revision

# ② 结束后用同一条原命令，输入集只多了我自己追加的报告章节
$ ...同一条命令...
33be0f68f520373d3ac22a09035668bd  -      # 差异 100% 来自我自己的报告（546 → 770 行），不是代码漂移

# ③ 因此按任务描述的要求，比较时**排除我的报告文件**，使输入集在开工/结束两侧一致：
$ prev=$(cd /tmp/bg3-prev && find src crates src-tauri scripts .github docs README.md package.json \
          -type f -not -path '*/target/*' -not -name 'VERIFICATION-ROUND2.md' | sort | xargs md5sum | md5sum)
$ now=$(find ...同一条... | sort | xargs md5sum | md5sum)
上一轮 d85a3802 树（无报告）: d85a3802e2ed0952f77e053fe8191389
本轮   54147be9 树（无报告）: bc9edbe9de1f70df72b56d96f1239848

# ④ 两侧的差异**恰好**是本轮那 13 个文件（文件级证据，不是推断）
$ diff -rq /tmp/bg3-prev <repo> | grep -c "differ\|Only in REPO"
13
# ⑤ 指纹输入集里最新的 mtime = 03:01:15（docs/ARCHITECTURE.md，writer 最后一次写入）；
#    我的全部验证（门禁、探针、变异）都在 03:03 之后，没有触碰输入集里的任何文件
$ find ... -not -name 'VERIFICATION-ROUND2.md' -printf '%TT %p\n' | sort | tail -1
03:01:15.5570150900 docs/ARCHITECTURE.md
```

**结论：验证对象没有漂移**。① 证明开工时冻结的 revision 与我核对的一致；④⑤ 证明结束后代码面
与「上一轮树 + 13 个文件」逐字相等，且这些文件全部在 03:01:15 之前写完。

```
$ git status --porcelain | wc -l
36                                        # 20 改 + 16 新增；无 commit、无删除

$ diff -rq /tmp/bg3-prev <repo>           # 本轮真正的源码级变更面
```

**实测变更面是 13 个文件，比 Lead 给的「6 条改动路径」多 5 个**：

| 文件 | 在 Lead 清单里？ | 内容 |
| --- | --- | --- |
| `crates/.../formats/content_list.rs` | ✔ | +19（新增 error 写回用例，**生产代码 0 改动**） |
| `crates/.../formats/loca.rs` | ✔ | +13（同上） |
| `crates/.../formats/lsx.rs` | ✔ | 生产闸门 `has_target()` → `has_writable_target()`，+测试 |
| `crates/.../translation/retry.rs` | ✔ | `emit_attempt_progress`（task-9） |
| `crates/.../tests/e2e_pak_flow.rs` | ✔ | +135（`error_entries_never_reach_the_packed_pak`） |
| `src/.../hooks/useTranslationRun.ts` | ✔ | task-6 重试清空语义 |
| `crates/.../src/types.rs` | ✘ | `has_writable_target()` + `effective_text` 改用它（**IPC 契约面文件**） |
| `crates/.../translation/fidelity.rs` | ✘ | F-03 `normalize_self_closing` |
| `crates/.../translation/engine/tests.rs` | ✘ | 2 个既有用例**追加**断言 + 新增用例 |
| `crates/.../translation/test_support.rs` | ✘ | 新增 `event_flow` 辅助 |
| `docs/ARCHITECTURE.md` | ✘ | progress 契约、写回不变量、F-03 取舍 |
| `README.md` | —（叙述里提过） | F-08 措辞 |
| `src/.../TranslationTable.streaming.test.tsx` | —（叙述里提过） | task-6 的 4 个新用例 |

`types.rs` 是 IPC 契约面却不在清单里，所以我单独做了 serde 面核对（见 10.6）。

## 10.1 F-01 判定：**已修复**

### 修复点（我读代码确认，不是听自述）

```rust
// types.rs
pub fn has_writable_target(&self) -> bool { self.has_target() && self.status != TranslationStatus::Error }
pub fn effective_text(&self) -> &str { if self.has_writable_target() { &self.target } else { &self.source } }
// lsx.rs::plan_replacements
if !entry.has_writable_target() { continue; }
```

三处唯一闸门：`content_list` / `loca` 走 `effective_text()`，`lsx` 走 `has_writable_target()`。

### 证据 1：三种格式各自的 error 写回与 edited 反例（我自己的探针）

```
$ bash /tmp/probes/run-probe-rust.sh probe_writeback
test content_list_error_reverts_to_source_and_edited_is_written ... ok
  [content_list 产物]
  <content contentuid="h00000001" version="1">The <i>Fireball</i> spell</content>   ← error：退回原文（标签完整）
  <content contentuid="h00000002" version="1">造成 {1} 点伤害</content>              ← edited：照常写回
test loca_error_reverts_to_source_and_edited_is_written ... ok
  [loca 读回] [("h0001", "1", "Hello {1}"), ("h0002", "3", "世界")]                  ← error 原文 / edited 译回写，version 1/3 保留
test lsx_error_leaves_field_alone_and_edited_is_written ... ok
  <attribute id="Description" type="LSString" value="A sturdy blade &amp; shield-breaker." />   ← error：字段原样
  <attribute id="DisplayName" type="LSString" value="第二段说明" />                              ← edited：改写
test real_sample_error_entry_roundtrips_back_to_source ... ok
  [真实样本] 10 条 → 10 条；第1条 (hbf6dc83dgcc9eg46a3g90ffg6a6c9dffcc28, ver=1) source="Appearance Editing"
test real_sample_repack_never_contains_rejected_translation ... ok
  [重打包] 文件数 16，条目 10 → 10；第1条 = "Appearance Editing"；第2条 = "【手工改好的译文】"
test f02_escaped_tags_still_pass_fidelity_and_double_escape ... ok
test result: ok. 6 passed; 0 failed
```

**真实样本 + 真实 PAK 闭环**（我自己写的，不依赖 writer 的 e2e）：解包 16 个文件 → 把第 1 条设成
`error` + target `【被拒的坏译文】`、第 2 条设成 `edited` + `【手工改好的译文】` → 写回 → `pak::repack`
→ 重新解包 → 重新解析：条目数 10 → 10、`contentuid`/`version` 原样、第 1 条是原文
`Appearance Editing`、重打包后的原始 XML **不含** `【被拒的坏译文】`、未触碰条目逐字不变。

### 证据 2：修过头检查（都通过）

| 反例 | 期望 | 实测 |
| --- | --- | --- |
| `edited` + 非空 target | 照常写回 | 三格式 + 真实样本 + 重打包全部写回 ✔ |
| `translated` + 非空 target | 照常写回 | e2e 用例 ③ + 我的 probe（`good` 分支）✔ |
| 取消后回滚 | target 清空、退回原文 | 上一轮 D4 探针复跑仍绿 ✔ |
| `translating` 中途（取消前） | 与改动前一致（有文本就写） | `writable_target_covers_all_statuses` 覆盖，`translating` 仍可写 ✔ |
| 只有空白字符的 target | 不算译文 | `has_target()` 不变 ✔ |
| 前端编辑保存路径 | 必须变成 `edited` | `useEntryEditing.saveEdit` 是唯一非空 target 写入者（`updateEntry(id,{target:draft,status:"edited"})`）✔ |

### 证据 3：前端状态与新的分工一致

上一轮 D7 探针复跑：`{"beforeError":"造成伤害","target":"造成伤害","status":"error"}` —— **target 仍然保留**，
这是**有意**的（给用户看 + 手工抢救），坏译文不进 PAK 改由写回侧兜底。
D7b（同一帧 delta + error）行为按 task-6 变更为 **`target="来不及落地的碎片"`**（新语义：error 前先 flush 保留最后一轮），
探针已同步更新并通过。

**判定：F-01 已修复，未修过头。**

## 10.2 F-09 判定：**已修复**（三种场景 + 附带伤害检查）

我自己的前端探针 `retrytext-probe.test.tsx`（10 例，全部手工构造 core 的真实事件序列）：

```
$ bash /tmp/probes/run-probe-web.sh src/__probe__/retrytext-probe.test.tsx
[F09-A]  {"midStream":"造成 {1} 点伤害","final":"造成 {1} 点伤害"}      网络退避两轮
[F09-A2] 重试 progress 之后: ""                                        旧文本被丢弃（不是追加）
[F09-A3] "迟到的旧碎片第二轮"                                          同帧旧 delta 无法区分（协议固有，见 N-04）
[F09-B]  {"afterCorrection":"造成 {1} 点伤害","final":"造成 {1} 点伤害"} 结构纠错两轮
[F09-C]  {"target":"第二轮仍坏","status":"error"}                       两轮都不合格 → 只留最后一轮
[F09-C2] {"target":"最后一轮","status":"error"}                          error 后迟到 delta 不污染
[F09-D]  重试请求 = ["…probe.xml#u11"] 已完成条目 = "已完成的译文"        重试失败条目不影响已完成条目
[F09-E]  {"target":"第一轮文本","status":"translating"}                  首次 progress 不清空 ✔
 Tests  10 passed (10)
```

* **网络退避两轮**：`[P, P, D, D, done]` → midStream 时 target 已经只对应最后一轮，最终不出现拼接。
* **结构纠错两轮**：被拒译文 `造成伤害` 已落地 → 纠错边界 progress → target 变 `""` → 新一轮 delta → `造成 {1} 点伤害`（不含 `造成伤害造成`）。
* **一直不合格 → error**：target = `第二轮仍坏`（不是两轮拼接），状态 `error`，且可人工编辑成 `edited`。
* **附带伤害**：点「重试 1 条失败」时 `translateEntries` 的请求只含 `#u11`，已完成条目 `#u10` 的译文与 `translated` 状态原封不动。

### core 侧的 progress 契约（我自己的假翻译器复算）

| 探针 | 实测事件流 | 结论 |
| --- | --- | --- |
| B3 结构纠错 | `[P#u1, P#u1, Done#u1, All(1,0)]` | 纠错边界恰好多一个 progress，Done 只有一条 |
| B7 退避中取消 | `[P#u1, All(2,0)]` | 取消后不再补发 progress/delta/done/error（取消检查在发 progress 之前） |
| B10 Series 重试 |`[P#u1, P#u2, Done#u1, Done#u2, All(2,0)]`| Series 不发额外 progress、不推 delta —— 与 ARCHITECTURE 新增表述一致 |

### Lead 的问题 3（首次 progress 会不会清空文本）：**不会**

`retrying = status === "translating" && attemptedIdsRef.current.has(id)`。首轮流程固定是
`progress` → `delta*`，首个 progress 到达时 `attemptedIdsRef` 里还没有这个 id（delta 分支才会补写它），
所以走 `setEntryStatus`（只改状态、不动 target）——探针 F09-E 实测 `target="第一轮文本"` 保留。
相反的顺序（先 delta 后首个 progress）确实会被当成重试边界清空，但**生产不可达**：
`src-tauri/src/commands/translate.rs` 的 `ChannelSink` 把一次运行的全部事件写进**同一个** `Channel`
（FIFO），而引擎对每个条目固定发 `[progress, (重试 progress)*, delta*, done]`（`run_job` 在
`translate_with_retry` 之前先发 progress）。见 N-02。

## 10.3 F-03（顺带修）与它的代价

`normalize_self_closing` 把非空元素的 `<x/>` 规范成 `<x>`+`</x>`。我加了 6 行探针：

```
[误报面] R78 非空元素 <i/> ≡ <i></i>（F-03 修复）   判定=保真
[误报面] R79 LSTag 自闭合 ≡ 空元素对（F-03）        判定=保真
[边界  ] R80 自闭合 vs 带正文（F-03 的代价/盲点）      判定=保真   ← 盲点确认
[误报面] R81 空元素 br 不受规范化影响                判定=保真
[漏报面] R82 多一个闭合标签仍要报                    判定=违规 extra_tag:</i>
[漏报面] R83 少一个闭合标签仍要报                    判定=违规 miss_tag:</i>
```

F-03 修复生效、`<br/>` 未被误伤、也没有规范化过头（真的多/少一个闭合标签照样报）。
代价是一个**新增盲点**（N-01）：`<LSTag Type="Spell"/>` 与 `<LSTag>火球</LSTag>` 判为保真 ——
writer 已在代码注释、`docs/ARCHITECTURE.md` 与 README 里主动声明，我也在 55 行表里固定住这条行为。

## 10.4 F-08：README / ARCHITECTURE 与实现逐句对齐

| 文档句子 | 代码依据 | 判定 |
| --- | --- | --- |
| 「能不能写回只看这一条——译文非空**且**状态不是「出错」」 | `has_writable_target()` | 准确 |
| 「出错条目在写回时退回原文（导出的 MOD 里该字段保持原样）」 | 三格式 + 真实重打包实测 | 准确 |
| 「手工改好并保存后状态变成「已编辑」，才按你改的内容写回」 | `useEntryEditing.saveEdit` + probe | 准确 |
| 「取消翻译时残留的半截流式文本…回滚成「待翻译」并清空译文，同样退回原文」 | D4 探针复跑 | 准确 |
| 「模型把 `<` 转义回去也不会误判——代价是这种转义写法会被原样写回，游戏里显示成字面 `&lt;`」 | F-02 探针（`&amp;lt;i&amp;gt;`） | 准确（**新增了后果说明**，F-08 的诉求已满足） |
| ARCHITECTURE「每次尝试开始时都会重发一次 progress…Series 组…不额外发 progress；取消生效后不再发任何事件」 | B3/B7/B10 事件流 | 准确 |
| ARCHITECTURE「`translating`（取消后残留的半截译文）仍会被写回 —— 该路径由前端把关」 | 取消回滚探针 D4 | 准确，且取舍已显式声明 |

绝对化措辞（「不会把坏译文静默写出去」）已删除，替换为可验证的机制描述。

## 10.5 无回归

```
$ bash scripts/verify.sh
── [1/6] IPC 契约 ✓  [2/6] fmt ✓  [3/6] clippy ✓  [4/6] core test ✓
── [5/6] web test ✓  [6/6] build ✓
✓ 全部 6 道门禁通过（总耗时 6.2s）      VERIFY_EXIT=0

$ cargo check -p bg3-translate --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s    CHECK_EXIT=0

core:  255 单测 + 6 e2e + 6 真实样本 = 267（上一轮 244 + 5 + 6 = 255）
web:   13 文件 / 155 用例（上一轮 151），dist JS 464.60 kB / gzip 143.70 kB
```

| 机械检查 | 上一轮 → 本轮 | 结论 |
| --- | --- | --- |
| core 测试名集合 | 255 → 267，**删除/改名 0** | 只增不减 |
| 前端 `it()` 集合 | 151 → 155，**删除/改名 0** | 只增不减 |
| 敏感用例仍在 | `failures_are_reported_per_entry`、`cancel_stops_in_flight_jobs`、`run_emits_progress_delta_done_then_all_done`、`structural_failure_is_retried_once_with_a_correction_hint`、`structural_retry_stops_when_cancelled` 全部在 | ✔ |
| 被删除的断言行 | 12 个改动文件里**没有任何 assert/expect 行被删**（仅 2 行生产闸门 + 注释被替换） | ✔ |
| 「既有测试被迫改动 0 个」 | `engine/tests.rs` 删除行数 = **1**（import 行），2 个用例是**追加**断言 | **自述属实** |
| `TranslationTable.test.tsx` | md5 `fb3fe8fd…` 与 HEAD 完全相同 | ✔ |
| `src/lib/types.ts` | 未改动（不在 `git status` 里） | ✔ |
| `types.rs` serde 面 | 相对 HEAD 与上一轮：serde 属性 / 字段名 / 变体名**新增 0、消失 0** | ✔ 契约零改动 |
| 生产 `unwrap/expect/panic/#[allow` | 3 → 3，新增 0、消失 0 | ✔ |
| 新增调试残留 | 生产前端 0 处 `console.`/`debugger` | ✔ |
| 上一轮的 56 个前端探针 | 复跑仍全绿（D7b 按新语义更新） | ✔ |
| 上一轮的 Rust 探针 | `probe_retry` 10/10、`probe_fidelity` 3/3、`probe_fidelity_direct`（55+6 行）、`probe_ids`、`probe_api` 全绿 | ✔ |

## 10.6 本轮缺陷清单

| # | 级别 | 结论 |
| --- | --- | --- |
| N-01 | **低** | F-03 规范化带来的新增盲点：`<LSTag Type="Spell"/>` 与 `<LSTag>火球</LSTag>` 判为保真（自闭合 vs 带正文）。影响面窄（源必须是自闭合非空标签，且正文本来就不参与比对）；writer 已在代码、ARCHITECTURE、README 三处声明，我也把行为固定进探针 R80。 |
| N-02 | **低（生产不可达）** | 若 `delta` 先于该条目首个 `progress` 到达，前端会把首个 progress 当成重试边界并清空 target（探针 F09-E1b 实测）。生产不可达：`ChannelSink` 单 Channel FIFO + 引擎固定 `progress → delta` 顺序。仅作为状态机边界记录。 |
| N-03 | **信息** | Lead 给的变更面是 6 条路径，实测 13 个文件（多出 `types.rs`、`fidelity.rs`、`engine/tests.rs`、`test_support.rs`、`ARCHITECTURE.md`）。其中 `types.rs` 是 IPC 契约面，已单独核对 serde 零改动。 |
| N-04 | **信息** | 重试边界 progress 与「迟到的上一轮 delta」在同一帧内无法区分（探针 F09-A3 实测 `迟到的旧碎片第二轮`）。协议下不可达（上一轮 delta 一定先于重试 progress 送达），且 `batcher.discard(id)` 已覆盖「上一轮 delta 仍在本批未提交」这一真实情形。 |
| N-05 | **信息** | `error` 条目保留 target 是**有意**的（展示 + 抢救），安全性完全依赖写回侧的 `has_writable_target()`。这一分工已在 ARCHITECTURE 里写成不变量；后续若有人新增写回路径（例如导出为其它格式），必须复用同一闸门。 |

**F-01：已修复。F-09：已修复。F-08：已对齐。无阻断、无高危、无中危。**

## 10.7 本轮复验的无法验证项（增量）

沿用 §9 的 6 项；本轮新增/仍然成立的：

| # | 项 | 原因 | 替代证据 |
| --- | --- | --- | --- |
| 7 | F-02 在真实 BG3 客户端里的显示效果 | 无法运行游戏 | 重打包产物字符串（`&amp;lt;i&amp;gt;`）已实测；README 用「游戏里显示成字面 `&lt;`」描述，属 XML 实体语义推断 |
| 8 | `edited` 状态在**界面点击**链路上的端到端 | 没有可运行的真实 Tauri 窗口（`@tauri-apps/api` 在 jsdom 下需 mock） | 代码路径核对（`saveEdit` 是唯一非空 target 写入者且置 `edited`）+ store 层探针把状态置成 `edited` 后写回实测 |
| 9 | GitHub Actions 上 verify.sh 的真实运行 | 未触发 GHA | 与 §9 第 1 项相同：本地 `verify.sh` 6/6 + `yq` 解析 + `--list` 断言本地复现 |
