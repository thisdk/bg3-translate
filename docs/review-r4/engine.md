# T2 审查报告（第四轮）：Rust 翻译引擎与 LLM 协议层

- 审查人：`auditor-engine`（task-2）
- 基线 revision：`d8295da`（= tag v1.1.5），开工时工作树干净、代码 hash `ad513e30c1e335f2a1f9cd669f6aa7ed`
- 写范围：`crates/bg3-translate-core/src/translation/**`、`crates/bg3-translate-core/src/types.rs`、
  `crates/bg3-translate-core/src/error.rs`、本文件
- 结论：**修了 3 条**（中 2 / 信息 1）；另有 **1 条被证伪的怀疑**、**2 条定量后决定不修**、
  **6 条已确认未修 + 理由**、**5 条无法验证**。
- 全程零外部网络请求：所有网络行为都用「直接喂字节给生产消费函数」验证。

基线自检（开工时，`docs/REVIEW-ROUND3.md` 冻结的口径）：

```
$ cd /home/jason/bg3-translate && find src crates src-tauri scripts .github README.md package.json Cargo.toml \
    -type f -not -path '*/target/*' -not -path '*/__pycache__/*' | sort | xargs md5sum | md5sum
ad513e30c1e335f2a1f9cd669f6aa7ed  -

$ bash scripts/verify.sh
✓ 全部 6 道门禁通过（总耗时 7.1s）

$ cargo test -p bg3-translate-core --all-targets
running 374 tests ... test result: ok. 374 passed
running 16 tests  ... test result: ok.  16 passed
running 11 tests  ... test result: ok.  11 passed
running 8 tests   ... test result: ok.   8 passed
running 8 tests   ... test result: ok.   8 passed
# 前端：Test Files 20 passed (20) / Tests 190 passed (190)
```

---

## §1 结论摘要表

| 编号 | 级别 | 状态 | 一句话 |
| --- | --- | --- | --- |
| E-R4-01 | 中 | **已修** | SSE 流中错误载荷（`data: {"error":…}`）被当心跳跳过 → 半截译文 + `[DONE]` 被判成功 |
| E-R4-02 | 中 | **已修** | 「相同原文合并」用归一化 key 分组 → `…save?` 与 `…save!` 共享同一份译文 |
| E-R4-03 | 信息 | **已修** | `流读取失败` 文案不含「连接中断 / 总超时」两个成因，用户不知道下一步 |
| E-R4-04 | — | **已证伪** | 模型输出 `&lt;LSTag …&gt;` 会被二次转义进 PAK —— 写回层已是不动点，产物正确 |
| E-R4-05 | 低 | 定量后不修 | 一致性参考的「词中命中」：真实语料 59/1225，**多数是复数屈折**，加词边界净损失 |
| E-R4-06 | 信息 | 定量后不修 | 单字 CJK key 通过 `key.len() >= 3`（数的是字节）；收紧会砍掉 2 字 CJK 系列基名复用 |
| E-R4-07 | 低 | 已确认未修 | R-14 系列变体误合并：**现有测试把 `Silver's Hair {1}/{2}` 必须合并写死**，收紧会变红 |
| E-R4-08 | 信息 | 已确认未修 | 归一化 key 用于**已确定译名复用**（memory）时仍会把 `…save?` 的译文补给 `…save!` |
| E-R4-09 | 信息 | 已确认未修 | `Retry-After` 仍未实现（评估见 §4） |
| E-R4-10 | 信息 | 已确认未修 | prompt 注入无边界（R3 遗留，评估见 §4） |
| E-R4-11 | 信息 | 已确认未修 | job 级 panic / 信号量错误只计 1 条失败、不发 `Error` 事件（R3 遗留） |
| E-R4-12 | 信息 | 已确认未修 | 后端写回仍只认 `status == error`，对 `translating` 不设防（R3 遗留） |
| E-R4-13 | 信息 | 证伪（无需改动） | `types.rs` ↔ `src/lib/types.ts` 逐字段核对：7 个结构 + 5 个事件变体无漂移 |

---

## §2 逐条缺陷

### E-R4-01（中）SSE 流中错误载荷被当心跳跳过 → 半截译文被当成成功译文

**现象**：OpenAI 兼容网关在上游超时 / 限流时，会在**流中间**插一条错误对象
（`data: {"error":{"message":"upstream timeout","type":"server_error"}}`），随后照常结束连接
—— 很多代理的 `[DONE]` 是在 `finally` 里发的，所以错误载荷后面**跟着 `[DONE]`**。

本仓库的解析链把这条载荷当成「解析不出来的心跳」丢掉了：

1. `parse_chat_chunk_parts` 的标准结构解析失败（缺 `id`/`created`/…）；
2. 宽松结构 `LenientChunk` 里只有 `choices`，错误对象根本不在结构里 → `choices` 为空；
3. `let Some(chunk) = parse_chat_chunk_parts(&payload) else { continue; }` —— 直接跳过；
4. 后面的 `[DONE]` 于是成了「输出完整」的证据（`saw_done = true`），
   而 `ensure_stream_complete(None, true, false)` 认为这就够了；
5. `ensure_stream_produced_text` 也拦不住：**半截译文非空**。

结果与第三轮 R-01 是同一类（半截译文静默进 PAK），只是触发信号从
`finish_reason` / 无收尾证据换成了「服务端明说的错误载荷」。

**后果**：真·半截译文发 `Done` → 前端 `target` → 写回 PAK。原文没有占位符 / 标签时，
结构保真校验完全拦不住 → **静默内容缺失**。

**证据（修复前红）**：

```
$ cargo test -p bg3-translate-core --lib mid_stream_error -- --nocapture
thread 'translation::translator::tests::mid_stream_error_payload_is_rejected_instead_of_returning_half_text' panicked at translator.rs:693:
流中错误载荷必须判失败，不能把半截译文当成功: Some("造成")
thread 'translation::translator::tests::mid_stream_error_payload_is_rejected_before_finish_reason' panicked at translator.rs:704:
错误载荷必须判失败: Some("造成")
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 377 filtered out
```

`Some("造成")` 就是「半截译文被当成成品」的原样证据。

**证据（修复后绿）**：

```
$ cargo test -p bg3-translate-core --lib mid_stream_error
test translation::translator::tests::mid_stream_error_payload_is_rejected_instead_of_returning_half_text ... ok
test translation::translator::tests::mid_stream_error_payload_is_rejected_before_finish_reason ... ok
test translation::translator::tests::a_null_error_field_does_not_break_a_normal_stream ... ok
test result: ok. 3 passed; 0 failed; 0 ignored
```

**修法**（`translation/sse.rs` + `translation/translator.rs`，公开 API 不变）：

1. `LenientChunk` 增加 `error: Option<serde_json::Value>`（用 `Value` 接住，形状不定也不会
   把同一条载荷里的 delta 一起带崩；`"error": null` → `None`）；
2. `ChatStreamChunk` 增加 `error: Option<String>`，新增 `lenient_error_message()`：
   取 `error.message`，取不到退回整段 JSON；
3. **错误判定必须早于 `choices.into_iter().next()?`** —— 否则 `{"choices":[],"error":{…}}`
   会被 `?` 先吞掉（测试里专门有这条形状）；
4. `consume_chat_stream` 收到 `chunk.error` 立即返回**可重试**的 `AppError::Llm`
   （不带 `API 返回 ` 前缀，所以 `is_retryable_failure` 判它可重试），
   服务端消息用既有 `truncate_chars(.., ERROR_BODY_LIMIT)` 截到 500 字符；
5. 公开的 `parse_chat_chunk` **行为逐字不变**（错误载荷仍然返回 `None`）。

**回归测试**：
- `translation::translator::tests::mid_stream_error_payload_is_rejected_instead_of_returning_half_text`
  （半截 + error + `[DONE]`）
- `translation::translator::tests::mid_stream_error_payload_is_rejected_before_finish_reason`
  （半截 + error + `finish_reason: "stop"`）
- `translation::translator::tests::a_null_error_field_does_not_break_a_normal_stream`（正向防误杀）
- `translation::sse::tests::error_payloads_are_exposed_without_changing_the_public_api`
  （两种线上形状 + `{"choices":[],"error":…}` + 取不到 message 的退化 + 公开 API 不变）

**变异实验**（把检查关掉：`chunk.error.clone().filter(|_| false)`）：

```
$ cargo test -p bg3-translate-core --lib mid_stream_error
thread '...mid_stream_error_payload_is_rejected_instead_of_returning_half_text' panicked at translator.rs:704:
流中错误载荷必须判失败，不能把半截译文当成功: Some("造成")
failures:
    mid_stream_error_payload_is_rejected_before_finish_reason
    mid_stream_error_payload_is_rejected_instead_of_returning_half_text
test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 397 filtered out
# 还原后
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 397 filtered out
```

**残留风险**：
1. `event: error` + **非 `error` 键**的载荷（例如顶层 `{"message":…}` / `{"type":"error",…}`）
   仍会被跳过 —— 解码器不保留 `event:` 字段名，要覆盖它得改 `SseDecoder` 的公开语义；
   本机没有真实网关样本可验证这类形状，故本轮不动（见 §4）。
2. 若一条载荷同时含**结构完整**的 chunk 与 `error`（协议上自相矛盾），typed 路径先成功，
   错误仍会被忽略（未见过真实样本）。
3. 「网关真会在错误后补 `[DONE]`」这一点**无法在本机验证**（§4 第 4 条）；
   即便不补，旧行为也已经是错的（错误载荷被静默丢弃，用户只看到「没有任何文本」）。

---

### E-R4-02（中）「相同原文合并」用归一化 key 分组 → 不同原文共享同一份译文

**现象**：`plan_jobs` 的第二段用 `normalize_consistency_key`（折空白、去**首尾 ASCII 标点**、
转小写）当分组键，把「看起来像」的原文并成一个 `ExactGroup`，只发**一次**请求
（`source` 取第一条），组内每条都拿到**同一份**译文。

```
$ cargo test -p bg3-translate-core --lib probe_ -- --nocapture     # 修复前探针（临时模块，已删）
PROBE plan = TranslationPlan { total: 4, skipped: 0, jobs: 1 }
PROBE job source="Fireball" output=ExactGroup { entry_ids: ["…#u1", "…#u2", "…#u3", "…#u4"] }
# 四条原文分别是 "Fireball" / "Fireball." / "Fireball?" / "FIREBALL!"
```

```
PROBE2 plan = TranslationPlan { total: 2, skipped: 0, jobs: 1 }
PROBE2 source = "Are you sure you want to delete this save?"
PROBE2 → 不同原文共享一份译文: ["test.loca#u1", "test.loca#u2"]
# u2 的原文是 "Are you sure you want to delete this save!"
```

**后果**：疑问句被写成陈述句 / 感叹句被写成陈述句 —— **语义被静默改写**，
而结构校验不可能发现（占位符与标签完全一致、正文本来就允许改写）。
附带还有一处：`matches`（术语命中）也是按第一条原文算的，会贴到另一条原文上
（原文不同时，注入的术语可能与本条无关）。

**根因**：模块文档写的是「**完全相同的原文**并成一次请求（`ExactGroup`）」，
实现却复用了「一致性 key」—— 那个 key 的设计用途是「已确定译名的模糊复用」
（`Fireball` / `fireball` / ` fireball ` 视为同一个），不是「两份译文可以互换」的判据。

**证据（修复前红，变异：把分组键改回一致性 key）**：

```
$ cargo test -p bg3-translate-core --lib punctuation_only_differences
assertion `left == right` failed: 原文不同的条目必须各自成 job: [
  TranslationJob { source: "Are you sure you want to delete this save?", …,
                   output: ExactGroup { entry_ids: ["test.loca#u1", "test.loca#u2"] } },
  TranslationJob { source: "DELETE SAVE", …,
                   output: ExactGroup { entry_ids: ["test.loca#u3", "test.loca#u4"] } }]
  left: 2  right: 4
test result: FAILED. 0 passed; 1 failed
```

**证据（修复后绿）**：

```
$ cargo test -p bg3-translate-core --lib punctuation_
test translation::engine::tests::punctuation_variants_each_get_their_own_translation ... ok
test translation::planner::tests::punctuation_only_differences_do_not_share_one_translation ... ok
test result: ok. 2 passed; 0 failed
```

**修法**（`translation/planner.rs`，一处）：分组键从 `key`（归一化）改成 `entry.source.clone()`
（原文本身）；一致性 key **继续只用于**「已确定译名复用」（`memory.get(&key)`）那条路径。
原文逐字相同的条目**仍然合并**（既有测试 `identical_sources_share_one_request` 保持绿）。

**回归测试**：
- `translation::planner::tests::punctuation_only_differences_do_not_share_one_translation`
  （`…save?` vs `…save!`、`DELETE SAVE` vs `Delete save` → 4 个 job；
   并同时断言「逐字相同的两条仍然合并成一个 `ExactGroup`」，防止修过头）
- `translation::engine::tests::punctuation_variants_each_get_their_own_translation`
  （端到端：两条各自拿到 `译:<自己那句>`，请求数 = 2）

**变异实验**：见上「修复前红」——把 `let group_key = entry.source.clone();` 改回 `key.clone()`，
两条用例立刻变红（`left: 2 right: 4`，并打印出被误并的 `ExactGroup`）；还原后全绿。

**取舍与残留风险（必须写清）**：
1. **代价**：大小写 / 首尾空白不同、但语义相同的 pending 条目，现在各发一次请求
   （多花一点额度与一次采样），换来的是「不会有条目拿到别人的译文」。
   这是**有意的**：`ExactGroup` 的语义是「共享一份译文」，只有原文逐字相同才成立。
2. **残留（E-R4-08）**：`memory.get(&key)` 那条「已确定译名复用」路径**仍然是模糊 key**：
   MOD 里若已经有一句 `…save?` 的译文，pending 的 `…save!` 仍会被它直接补上
   （既有「同 MOD 一致性」设计，本轮按最小改动原则未动）。它比本条轻
   —— 复用对象是**用户在 MOD 里已经确认过的译法**，而 `ExactGroup` 复用的是
   **同一次运行里刚生成的、没经过任何人确认的**译文。
3. **上游影响**：本轮没有改 `normalize_consistency_key` 本身（它是 `pub`，
   且被 memory / 别名表共用），所以 `"Quoted Name"` ≡ `quoted name` 等既有行为逐字不变。

---

### E-R4-03（信息）`流读取失败` 文案不含成因，用户不知道下一步

**现象**：流读取出错时只拼 reqwest 的底层错误：

```rust
Some(Err(err)) => return Err(AppError::Llm(format!("流读取失败: {err}")))
```

用户看到的是 `大模型调用错误: 流读取失败: error decoding response body` —— 一句英文，
既不知道发生了什么，也不知道下一步。两个真实成因在客户端不可区分：
连接真的断了，或者这次流式响应撞上**总超时**（`reqwest` 的 `timeout` 覆盖到响应体读完，
`REQUEST_TIMEOUT = 60s`；慢模型 / 长条目会在这里稳定失败）。

**后果**：纯 UX —— 用户反复重试同一个长条目，4 次失败后条目落 `error`
（写回退回英文），但错误信息没有给出任何可执行的下一步。

**证据**（变异实验：把文案退回旧的那一行）：

```
$ cargo test -p bg3-translate-core --lib consume_stream_reports_mid_stream_errors
thread '...consume_stream_reports_mid_stream_errors' panicked at translator.rs:625:
实际: 大模型调用错误: 流读取失败: 大模型调用错误: 连接被重置
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 403 filtered out
# 还原后
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 403 filtered out
```

（`实际:` 里那串嵌套前缀只出现在**单测**里：那条用例为了造流错误注入了一个 `AppError`，
它的 Display 自带「大模型调用错误: 」。生产路径上 `{err}` 是 `reqwest::Error`，
Display 是一句英文描述，不会被再包一层 —— 这正是本条要修的东西。）

**修法**：文案补上两个成因与下一步，**不改任何控制流 / 重试语义**：

```
流读取失败（连接中断，或流式响应超过 60 秒总超时）: {err}；可减小单条文本长度或换用更快的模型后重试
```

**回归测试**：`translation::translator::tests::consume_stream_reports_mid_stream_errors`
（**加强而不是替换**：原有 `contains("流读取失败")` 断言保留，另加三条）。

**残留风险**：60 秒总超时的**真实触发**无法在本机验证（没有网络 / mock 服务器），
所以文案里写的是「或」（两个成因之一），不宣称具体是哪一个。

---

## §3 被证伪的怀疑点（同样是结论，附实际命令与输出）

### E-R4-04（已证伪）模型把标签写成 `&lt;LSTag …&gt;` 会被二次转义进 PAK

**怀疑**：`check_fidelity` 的 `restore_entities` 认为 `&lt;` ≡ `<`（两边都还原再比对），
而写回层拿的是**模型输出的原文**，直觉上会把 `&lt;` 再转义一次 → 产物里是
`&amp;lt;LSTag …`，游戏显示成字面标签（链接 / tooltip 失效）——即「坏译文被判为成功」。

**实测：证伪。** 写回层 `content_list::render_with_style` 在标签扫描**之前**先做
`decode_entities`（`escape(decode(T))` 是刻意的**不动点**），所以模型的转义写法会被
还原成真标签再写出去：

```
$ cargo test -p bg3-translate-core --lib probe_escaped_tags -- --nocapture   # 临时探针，已删
PROBE 校验 issues = []  (faithful=true)
PROBE Markup 写回 = <?xml …?><contentList><content contentuid="h1" version="1"><LSTag Type="Spell">火球术</LSTag></content></contentList>
PROBE Escaped 写回 = <?xml …?><contentList><content contentuid="h1" version="1">&lt;LSTag Type="Spell"&gt;火球术&lt;/LSTag&gt;</content></contentList>
PROBE 现有修复后 = &lt;LSTag Type="Spell"&gt;火球术&lt;/LSTag&gt;
---
PROBE 校验 issues = []  (faithful=true)
PROBE Markup 写回 = …<content contentuid="h1" version="1">使用 &lt;i&gt; 表示斜体</content>…
PROBE Escaped 写回 = …使用 &lt;i&gt; 表示斜体…
---
PROBE 校验 issues = []  (faithful=true)
PROBE Markup 写回 = …<content contentuid="h1" version="1">生命值 &lt; 50%</content>…
```

- Markup 风格：`&lt;LSTag …&gt;` **被写成真元素**（产物与源文件同形）；
- Escaped 风格：写成**单层**转义（`&lt;` 而不是 `&amp;lt;`），游戏解码后拿到的正是
  `<LSTag …>`；
- `&lt;i&gt;`（标签不成对）走 `is_tag_balanced` 的降级分支，同样只写**单层**转义。

`&amp;lt;` 一次都没有出现 → 产物是「游戏解码后仍然正确的」文本。
**不修**（改了反而会与写回层的既有不变量打架）。

### E-R4-13（已证伪）IPC 契约字段漂移

逐字段核对 `crates/bg3-translate-core/src/types.rs` ↔ `src/lib/types.ts`：
`PakFile`（name/size/kind/language）、`PakFileKind`（6 个 kebab-case 取值）、
`TranslationEntry`（id/sourceFile/source/target/contentuid/version/status/error）、
`TranslationStatus`（5 个小写取值）、`LlmSettings`（baseUrl/apiKey/model/concurrency/temperature）、
`TranslationEvent`（5 个变体：`progress{entryId,status}` / `delta{entryId,text}` /
`done{entryId,text}` / `error{entryId,message}` / `all_done{total,failed}`）、
`ExtractResult`（workDir/files）—— **无漂移**，本轮不需要改 `types.rs`。
`error.rs` 的 `Serialize`（纯字符串、不带 `code`）已有锁死测试，未动。

---

## §4 已确认未修 + 理由

| 编号 | 项 | 为什么没修 |
| --- | --- | --- |
| E-R4-05 | 一致性参考的「词中命中」（`End` ⊂ `Legendary`） | **定量后决定不修**。真实语料（`samples/english.xml`，1971 条，记忆用自身构造 = 「已翻过一轮的 MOD」最坏情况）实测：`至少命中一条参考的原文 = 793/1971`、`参考总条数 = 1225`、其中「词中命中」`= 59`（4.8%）。逐条看这 59 条，**绝大多数是复数 / 屈折形态的正确提示**：`Moonblade ⊂ "Moonblades"`、`Unarmed Strike ⊂ "unarmed strikes"`、`Funeral Rite ⊂ "funeral rites"`、`Vortex Warp ⊂ "Vortex Warped"`；真正误导的是少数（`Sleep ⊂ "falls asleep"`、`Snare ⊂ "Ensnare"`、`Bite ⊂ "Frostbite"`）。加词边界会把**正确**的复数提示一起砍掉（如 `unarmed strikes` 会失去 `Unarmed Strike` 的译名约束），净收益为负 → 维持现状，记录为已知取舍。 |
| E-R4-06 | 单字 CJK key 通过 `key.len() >= 3` 闸门 | 闸门注释写的是「key 长度 ≥3 才值得记（短词会误命中）」，但 `String::len()` 数的是**字节**：一个汉字 3 字节，于是 `剑`、`盾` 这类单字 key 会被记进记忆，并被注入任何含该字的条目（实测：`记忆 keys = ["剑","盾"]`，`「长剑与短剑」的参考 = [ConsistencyTerm { source: "剑", target: "剑" }]`）。**但没有改成 `chars().count()`**：那会连带砍掉 **2 字 CJK 系列基名**（`发色9b`/`发色10` 这种极常见写法的 base「发色」）的记忆复用 —— 那是既有功能，代价（多一条无关提示）小于收益损失。记录为已知取舍。 |
| E-R4-07 | R-14 系列变体误合并（`deal {1} damage to {2}` 被判成系列） | **被现有测试挡住**：`engine/tests.rs:741/772/931` 三处把 `Silver's Hair {1}` / `{2}` **必须合并成系列**写死了（`series_with_placeholder_suffixes_passes_and_composes` 还断言 `done` 是 `银发{1}`/`银发{2}`）。收紧后缀形态判定（不再把 `{n}` 当变体后缀）会让这些既有用例变红 —— 按「禁止删除或削弱任何现有测试」的纪律不动，维持 R3 §4.2 的裁定（低，真实样本 41 条里 0 例，不弄坏 XML）。**要改就得先由 lead 裁定改这几条既有用例的语义**。 |
| E-R4-08 | `memory` 复用仍是模糊 key | 见 E-R4-02 取舍第 2 条。要一并收紧，等于改 `normalize_consistency_key`（`pub`、被 memory / 系列别名 / 测试共用），并会让「大小写不同视为同一专名」这条既有承诺失效 → 独立议题。 |
| E-R4-09 | `Retry-After` 未实现 | **评估结论：能低风险实现，但接线无法验证**。可行方案：`translate_once` 读 `response.headers()["retry-after"]` → 解析 delta-seconds（HTTP-date 形态忽略）→ 归一化后附到 `api_status_error` 的消息尾部（保持 `API 返回 {status}: ` 前缀不变，`retry` 的分类不受影响）→ `retry.rs` 新增纯函数 `retry_after_of(err)` 取用、clamp；纯函数与「退避改用该值」都能用假 translator 单测。**但「从响应头到消息」这一段需要真实 HTTP 响应**，本机既无网络也无 mock 服务器（新增 mock 服务器 = 新依赖，被禁），只能靠代码审查 —— 按「没跑过的一律标未验证」的纪律，本轮不做，留给需要实测的独立议题。收益也有限（仅 429）。 |
| E-R4-10 | prompt 注入无边界（R3 遗留） | 原文直接拼进 user prompt（`原文：\n{source}`）。**低成本缓解评估**：加定界符 / 转义指令文本属于「改了也没法验证」的改动（没有真实 LLM API，无法证明注入成功率下降），且结构保真校验仍然生效（影响限于该条译文的措辞）。维持 R3 裁定，不动。 |
| E-R4-11 | job 级 panic / 信号量错误只计 1 条失败、不发 `Error` 事件 | 与 R3 F-09 同源：`run()` 的 `Err(panic)` / `Ok(Err(..))` 分支 `failed += 1`（而不是按该 job 覆盖的条目数），且不发 `Error` 事件。前端有收尾回滚兜底（不会卡在「翻译中」），但用户看不到失败原因。生产路径几乎不可达（`HttpTranslator` 没有 panic 点），补事件要动 IPC 事件语义 → 维持 R3 裁定。 |
| E-R4-12 | 后端对 `translating` 不设防 | `has_writable_target() = has_target() && status != Error`：取消后留在 `translating` 的半截流式文本在后端看来「可写」。前端有 `toWritableEntry` 等闸门（R3 验证者已复核两条路径），后端加防线要改 `has_writable_target()` 语义、会改变「人工救回」路径 → 维持 R3 裁定。本轮**新测**：取消语义本身没有新问题（见 §5 第 3 条）。 |

---

## §5 无法验证项（如实列出）

1. **真实 LLM API 的行为**：没有 key，也刻意不发任何外部请求。错误载荷、`finish_reason`、
   `Retry-After`、各网关的 SSE 形态只能按协议文档 + 手工构造的字节流验证。
2. **「网关会在错误载荷之后补 `[DONE]`」**（E-R4-01 的触发前提）：没有真实网关可测。
   但要说明：即便网关**不**补 `[DONE]`，旧行为也已经错了 —— 错误载荷被静默丢弃，
   用户看到的是「流式响应结束但没有任何文本（网关可能把错误包成了 200）」，
   真正的服务端原因（`upstream timeout` 等）被吞掉。
3. **取消语义**（无新发现，记录为「未验证而非已确认」）：全局单个取消令牌 + `reset()`
   复用、取消后仍在途的事件是否可能污染下一轮 —— 本次只做了静态推理：
   `run()` 的 `FuturesUnordered` 循环会**等所有 job future 结束**才返回，
   每个 job 在取消后最多迟 `CANCEL_POLL_INTERVAL = 100ms` 返回 `Ok(None)`，
   且 `EventSink` 由调用方按次注入（壳层每次命令一个 Channel），
   所以「上一轮的迟到事件打进下一轮」在 core 侧没有通路；**没有真机壳层可验证**，
   维持 R3「理论竞态」的定性。
4. **60 秒总超时的真实触发**（E-R4-03）：没有网络 / mock 服务器，只能验证文案。
5. **真机 BG3 对产物的渲染**：无法运行游戏，只能证明写出的字节与真实样本往返一致。

另记一条**跨范围观察**（不是我的缺陷，留给 lead 集成时看）：
`fidelity.rs` 通过 `formats::content_list::is_allowed_inline_tag` 判定「这是不是白名单标签」，
而本文件正在被 auditor-formats 改写 `parse_single_tag`（严格 XML 1.0 文法）。
两边对「什么算标签」的判定必须继续一致，否则保真校验与写回层会分叉
（校验认它、写回不认，或反过来）。建议 verifier 在最终冻结树上专门跑一次
`corpus_regression` + `real_mod_sample` + `corpus_writeback`（本轮我跑它们时是
auditor-formats 的中间态，见 §7）。

---

## §6 改动文件清单与门禁

| 文件 | 内容 |
| --- | --- |
| `crates/bg3-translate-core/src/translation/sse.rs` | E-R4-01：`LenientChunk.error` / `ChatStreamChunk.error` / `lenient_error_message` / 错误判定早于取 choices；新增 1 条测试 |
| `crates/bg3-translate-core/src/translation/translator.rs` | E-R4-01：`consume_chat_stream` 收到错误载荷立即失败；E-R4-03：流读取失败文案补成因；新增 3 条测试、加强 1 条既有测试 |
| `crates/bg3-translate-core/src/translation/planner.rs` | E-R4-02：`ExactGroup` 分组键改用原文本身；新增 1 条测试 |
| `crates/bg3-translate-core/src/translation/engine/tests.rs` | E-R4-02：端到端用例（每条拿到自己那句译文）；新增 1 条测试 |
| `docs/review-r4/engine.md` | 本报告 |

新增回归测试 **6 条**（`translation::` 子集 `191 passed`）；未删除、未削弱任何既有测试
（唯一的既有测试改动是**加强** `consume_stream_reports_mid_stream_errors`）。
未新增依赖、未改 `Cargo.toml` / `package.json`、未改 `types.rs` / `error.rs`、
未在生产路径新增 `unwrap/expect/panic/#[allow]`、未 commit / tag / 改版本号。

本轮范围内精确跑（全绿）：

```
$ cargo test -p bg3-translate-core --lib translation::
test result: ok. 191 passed; 0 failed; 0 ignored; 0 measured; 206 filtered out

$ cargo test -p bg3-translate-core --lib types::
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 384 filtered out

$ cargo test -p bg3-translate-core --lib error::
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 394 filtered out

$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.29s

$ cargo fmt --all --check      # 我的四个文件：无差异（工作区其余差异见下）
```

---

## §7 收尾时的全量门禁（工作区含其他人的中间态，如实标注）

本节记录「我自己已经不再改文件」时的全量重跑。**跑的时候工作区里有别人的中间态**，
与 `docs/REVIEW-ROUND3.md` 冻结的基线不同，逐条标注归属。
所有失败项都在**别人的写范围**里，逐条列出了文件归属（没有一条在 `translation/**`）。

```
$ cargo test -p bg3-translate-core --all-targets --no-fail-fast
running 404 tests  ... test result: ok. 404 passed; 0 failed
running  16 tests  ... test result: ok.  16 passed; 0 failed
running  11 tests  ... test result: ok.  11 passed; 0 failed
running   8 tests  ... test result: ok.   8 passed; 0 failed
running   8 tests  ... test result: ok.   8 passed; 0 failed
# 合计 404 + 16 + 11 + 8 + 8 = 447（基线 374 + 16 + 11 + 8 + 8 = 417；我加 6 条，
# 其余由 auditor-formats / auditor-shell / auditor-web 增加）

$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.31s

$ python3 scripts/check_ipc_contract.py
✓ 版本号三处一致：1.1.5（package.json / tauri.conf.json / Cargo.toml）
✓ Cargo.lock 中 bg3-translate、bg3-translate-core 的版本与 workspace 一致：1.1.5
✓ IPC 契约检查全部通过

$ bun run test
Test Files  21 passed (21)
     Tests  206 passed (206)

$ bun run build
✓ built in 274ms

$ bash scripts/verify.sh
✗ 门禁未通过：Rust 代码格式（cargo fmt --check）（core-fmt）
命令：cargo fmt --all --check
   ✗ 失败：core-fmt 退出码 1（172ms）
已跑 2/6 道，总耗时 254ms，退出码 1

$ cargo fmt --all --check   # 差异文件（逐条标注归属，translation/ 下为 0）
Diff in .../src/formats/loca.rs                   ← auditor-formats 的中间态
Diff in .../src-tauri/src/commands/entries.rs     ← auditor-shell 的中间态
Diff in .../src-tauri/src/commands/terminology.rs ← auditor-shell 的中间态
Diff in .../src-tauri/src/commands/translate.rs   ← auditor-shell 的中间态
# （我最早观察到 8 个文件，config.rs / content_list.rs / glossary/store.rs / pak.rs
#   已被各自 owner 格式化掉，现在只剩这 4 个）
# 我的四个文件（translation/{sse,translator,planner}.rs、translation/engine/tests.rs）
# 用 rustfmt --edition 2024 单独校验过：无差异。
```

**结论**：`cargo fmt --all --check` 这一道门禁目前整树红，原因**全在别人的写范围**（我没有权限、
也不应该去格式化他们的文件）。除它之外，第 4~6 道门禁与 clippy / ipc 都是绿的。
本轮范围内（`translation::` / `types::` / `error::`）全绿：

```
$ cargo test -p bg3-translate-core --lib translation::
test result: ok. 191 passed; 0 failed; 0 ignored; 0 measured
$ cargo test -p bg3-translate-core --lib types::     → ok. 16 passed
$ cargo test -p bg3-translate-core --lib error::     → ok.  6 passed
```

### §7.1 诚实说明：收尾这段时间里工作区在四种状态之间跳

我跑全量时其他 writer 仍在编辑，同一个命令在几十分钟内给出过四种结果，逐条记录
（**都不是我的文件**）：

| 时刻 | `cargo test -p bg3-translate-core --all-targets` | 归属 |
| --- | --- | --- |
| T1 | `402 + 16 + 11 + 8 + 8 = 445` **全绿** | — |
| T2 | 402 里 1 红：`formats::loca::tests::lying_entry_count_is_rejected_without_a_giant_allocation`（`布局非法的目标按既有取舍整体重写: Err(Loca(InvalidFormat(...)))`） | `src/formats/loca.rs`（auditor-formats 中间态） |
| T3 | 编译失败：`error[E0282]: type annotations needed for Vec<_> --> src/formats/loca.rs:47:9` | 同上 |
| T4（收尾时） | `404 + 16 + 11 + 8 + 8 = 447` **全绿**；clippy ✓；ipc ✓；前端 21 文件 / 206 用例 ✓；build ✓ | `bash scripts/verify.sh` 仍卡在第 2 道 fmt（4 个别人的文件） |

同一时段 `cargo fmt --all --check` 一直红在别人的文件上（最多时 8 个，收尾时 4 个），
`translation/**` 下自始至终 **0 处**差异。

所以：**最终权威数字请以 lead 在冻结树上的重跑为准**；本报告引用的 447 是 T4（我停止改文件那一刻）
的真实输出，不是推算；445 是同一天更早一次全绿的输出。需要我在别人收工后再跑一次，随时叫我（我在此之前不再改任何文件）。

**另一条给 lead 的提醒**（不是我的缺陷）：`config.rs` 里还留着别人的临时探针
（`// 临时探针（取证用，取证后删除）` + `println!("PROBE system_data_dir=…")`），
收尾前应当删掉；`src/components/AppTopBar.test.tsx` 也是未跟踪文件。
