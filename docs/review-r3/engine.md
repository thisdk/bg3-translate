# T2 审查报告：Rust 翻译引擎与 LLM 协议层

- 审查人：`engine-auditor`（task-2）
- 基线 revision：`422f667a0d5d3be26190c0f5b55b0796a67a7b76`（本轮所有 writer 共享）
- 写范围：`crates/bg3-translate-core/src/translation/**`、`crates/bg3-translate-core/src/types.rs`
- 报告路径：`docs/review-r3/engine.md`
- 结论：**修了 8 条**（高 1 / 中 1 / 低 4 / 信息 2），另有 3 条明确不改或跨范围，记录在 §4/§5。
- 未经真实 LLM API：全程零外部网络请求，所有网络行为用「直接喂字节给 SSE 解析器」验证。

---

## §1 结论摘要表

| 编号 | 严重度 | 状态 | 一句话 |
| --- | --- | --- | --- |
| F-01 | 中 | **已修** | 保真校验忽略标签**属性名**，而写回会把模型输出的标签原样写进 PAK → 坏标签静默落盘 |
| F-02 | 高 | **已修** | 流式响应被截断（`finish_reason=length` / 无任何收尾证据）仍被当成成功译文写回 |
| F-03 | 低 | **已修** | 用户看到的错误信息双重前缀「大模型调用错误: 大模型调用错误: …」 |
| F-04 | 低 | **已修** | 401/403/404 这类确定性失败也退避重试 4 次，白等 3.5 秒 |
| F-05 | 低 | **已修** | SSE 解码器缓冲无上限，服务端不发换行即可持续吃内存 |
| F-06 | 低 | **已修** | baseUrl 带 query 时拼出废地址（`…/v1?api-version=1/v1/chat/completions`） |
| F-07 | 信息 | **已修** | `temperature: NaN` 穿过 `normalized()`，线上序列化成 `"temperature": null` |
| F-08 | 信息 | **已修** | `contains_cjk` 漏 CJK 扩展 F/G/H/I 区，生僻字系列名不会被归到中文一侧 |
| F-09 | 信息 | 已确认未修 | job 级 panic 只计失败、不发 `Error` 事件（前端有兜底回滚，见 §5） |
| F-10 | 信息 | 已确认未修 | `429` 不读 `Retry-After`（要结构化状态码，跨 `error.rs`，见 §5） |
| F-11 | 信息 | 已确认未修 | prompt 注入无边界（原文里可写指令文本；影响限于该条译文，见 §5） |

`F-01` / `F-02` 与红队 `R-10` / `R-01`、lead 的定向提示同源，已按 lead 的裁定（保真校验口径 `(b)`）落地。

---

## §2 逐条缺陷

### F-01（中）保真校验忽略标签属性名 → 坏标签静默进 PAK

**现象**：模型把 `<LSTag Type="Spell">` 写成 `<LSTag 类型="Spell">`、`<LSTag Typ=...>` 或整个
`<LSTag>` 时，`is_faithful()` 全部返回 `true`。而写回链路拿的是**模型输出的那段文本**
（`entry.effective_text()` → `content_list::render` → `write_text_fragment` 直接 `write_all(tag)`），
原文的属性**不会**被恢复 —— 于是产物里就是坏标签。产物仍是合法 XML（`类型` 是合法 XML 属性名），
所以 `is_tag_balanced` 也拦不住。`fidelity.rs` 原注释「写回时属性原样保留」是**假承诺**，已一并删除。

**复现（修复前，临时探针 + 真实 `render`）**：

```
$ cargo test -p bg3-translate-core --lib probe_attribute_mangling -- --nocapture
PROBE source="<LSTag Type=\"Spell\" Tooltip=\"Fireball\">Fireball</LSTag>"
      target="<LSTag 类型=\"Spell\" Tooltip=\"火球术\">火球术</LSTag>"
      issues=[]
PROBE source="<LSTag Type=\"Spell\" Tooltip=\"Fireball\">Fireball</LSTag>"
      target="<LSTag>火球术</LSTag>"                       → issues=[]
PROBE source="<LSTag Type=\"Spell\" Tooltip=\"Fireball\">Fireball</LSTag>"
      target="<LSTag Typ=\"Spell\" Tooltip=\"Fireball\">火球术</LSTag>" → issues=[]
PROBE source="<font color=\"red\">红</font>" target="<font>红</font>" → issues=[]
PROBE rendered=<?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1"><LSTag 类型="Spell">火球术</LSTag></content></contentList>
```

最后一行是**根因证据**：写回确实把模型输出的 `<LSTag 类型="Spell">` 原样写进了文件。

**根因**：`Signature` 的标签 token 只保留标签名与开/闭/空元素形态，属性整段丢弃；
`render_tag()` 里 `tail` 只用于判断「是不是自闭合」，属性名从不参与比较。

**改动**（`src/translation/fidelity.rs`）：

1. 新增 `find_tag_end()`：找标签结束的 `>` 时**跳过引号内的 `>`**（`<LSTag Tooltip="a > b">` 是合法
   XML），`<` 之后再见 `<` 仍判为非标签 → 抗误报；
2. 新增 `scan_attributes()`：解析开始标签的属性名（支持双/单引号值、未加引号值、多余空白、
   `name` 无值），语法不合法返回 `None`（整段当普通文本，不做半吊子比较）；
3. `render_tag()` 的 token 变成 **`<标签名 属性名…>`（属性名排序后空格分隔，属性值丢弃）**；
   自闭合与空元素写成 `<name attrs/>`；
4. `normalize_self_closing()` 按 token 里的首词取标签名，展开出的闭合标签不带属性；
5. 模块文档 + 删除「写回时属性原样保留」的假注释，改成事实描述。

语义边界：**属性名多重集必须一致**（顺序不计、大小写敏感）；**属性值一律不参与比较**
（Tooltip 这类玩家可见文本允许翻译）。

**回归测试**：
- `translation::fidelity::tests::attribute_names_are_part_of_the_signature`（改名 / 拼错 / 大小写 /
  增删属性 → 不保真；属性顺序调换、多余空白、单引号、值里带 `>`/`/`、自闭合 → 保真）
- `translation::fidelity::tests::attribute_values_are_free_but_names_must_match`（值改写保真、
  删属性不保真）
- `translation::fidelity::tests::mangled_attribute_names_reach_the_written_file_when_not_caught`
  （端到端：真实 `render()` 证明坏标签会落盘、`error` 条目退回原文）
- 既有 `realistic_translations_are_never_flagged_by_structure_check`（真实语料零误报）保持绿；
  format-auditor 的 `real_mod_sample::mangled_tag_attribute_names_are_not_faithful` 由红转绿。

**修复前红 / 修复后绿（变异测试，自跑）**：把 `render_tag` 退回「属性名不进 token」：

```
$ cargo test -p bg3-translate-core --all-targets
test translation::fidelity::tests::attribute_names_are_part_of_the_signature ... FAILED
test translation::fidelity::tests::attribute_values_are_free_but_names_must_match ... FAILED
test translation::fidelity::tests::mangled_attribute_names_reach_the_written_file_when_not_caught ... FAILED
test translation::fidelity::tests::non_void_self_closing_equals_an_empty_pair ... FAILED
test result: FAILED. 261 passed; 4 failed; 0 ignored; 0 measured
$ cargo test -p bg3-translate-core --test real_mod_sample
---- mangled_tag_attribute_names_are_not_faithful stdout ----
属性名被改坏必须判为不保真: "<LSTag 类型=\"Spell\" Tooltip=\"Deals {1} damage\">Fireball</LSTag>"（问题: []）
test result: FAILED. 6 passed; 1 failed
# 还原后
$ cargo test -p bg3-translate-core --all-targets
test result: ok. 265 passed; 0 failed   /   6 passed   /   7 passed
```

**对既有测试的两处断言收紧**（都是**加强**，不是削弱，按 lead 裁定）：
- `attribute_changes_are_ignored` → 改名为 `attribute_values_are_free_but_names_must_match`：
  保留「只改属性值 → 保真」，把 `<font color="red">` → `<font>` 由「保真」翻成「不保真」。
- `non_void_self_closing_equals_an_empty_pair` 里 `assert!(is_faithful("<LSTag Type=\"Spell\"/>",
  "<LSTag>火球</LSTag>"))` 改为「属性一致时仍保真（保留空标签/标签对的盲点）+ 属性丢掉时不保真」。

---

### F-02（高）被截断的流式响应被当成成功译文

**现象**：服务端吐出半截 delta 后（带 `finish_reason: "length"`，甚至什么都不带给直接断流），
旧实现把它当成功：`summary.translated=1, failed=0`，`Done` 里是半句话，随后会被写进 PAK。
原文没有占位符/标签时结构校验完全拦不住 → **静默内容缺失**。根因之一是 `finish_reason` 在
HEAD 上被彻底丢弃：

```
$ git show HEAD:crates/bg3-translate-core/src/translation/translator.rs | grep -c finish_reason
0
```

**复现（修复前红，变异回「截断不拦」后的真实输出）**：

```
$ cargo test -p bg3-translate-core --lib translation::translator
test translation::translator::tests::lenient_gateway_truncation_is_also_rejected ... FAILED
test translation::translator::tests::truncated_stream_is_rejected_instead_of_written_back ... FAILED
thread ... panicked at translator.rs:566:
截断的流必须报错，不能返回半截译文: Some("造成")
test result: FAILED. 13 passed; 2 failed; 0 ignored
```

`Some("造成")` 就是「半截译文被当成成品」的原样证据。

**改动**：

1. `sse.rs` 新增 `ChatStreamChunk { content, finish_reason }` + `parse_chat_chunk_parts()`（`pub(crate)`，
   **公开的 `parse_chat_chunk` 签名与语义逐字不变**，改为委托实现）+ `truncation_reason()`
   （`length` / `content_filter` 才是不完整）；
2. `translator.rs` 把流消费循环抽成可注入字节流的 `consume_chat_stream()`（纯重构，行为不变），
   记录 `truncation`、`saw_done`、`saw_finish_reason` 三个信号；
3. 新增 `ensure_stream_complete()`：
   - 见过 `finish_reason ∈ {length, content_filter}` → 报错（可重试）；
   - 既没有 `[DONE]` 也没有任何 `finish_reason` → 报错（连接被掐断，无法确认完整）；
   - 判据放在 `ensure_stream_produced_text` **之前**，这样「被截断且没有文本」时报的是真正的原因。

失败会走既有网络重试（3 次退避），最终条目落 `error`（`has_writable_target()==false`，
写回退回原文），前端拿到 `Error` 事件而不是 `Done`。

**回归测试**：
- `translation::translator::tests::truncated_stream_is_rejected_instead_of_written_back`
  （复刻红队形态：发一半 + `finish_reason:"length"` + **不发 `[DONE]`**）
- `content_filtered_stream_is_rejected`、`lenient_gateway_truncation_is_also_rejected`（最小字段网关）
- `consume_stream_rejects_a_stream_that_never_terminates`（无 `[DONE]` 无 `finish_reason`）
- **正向防误报**：`stopped_stream_is_still_a_success`、`consume_stream_accepts_finish_reason_without_done_marker`、
  `consume_stream_assembles_deltas_and_stops_at_done`、`unknown_finish_reason_counts_as_a_terminator`
  （网关自造 `eos` 等未知收尾标记按「服务端自报收尾」处理，避免误杀）
- `completeness_helper_covers_both_truncation_signals`（纯函数边界）
- `translation::sse::tests::parts_expose_finish_reason_on_both_parse_paths`（两条解析路径都带出
  `finish_reason`，且 `content` 与旧 API 逐字一致）

**修复前红 / 修复后绿（变异测试，自跑）**：去掉「无收尾证据」分支：

```
test translation::translator::tests::completeness_helper_covers_both_truncation_signals ... FAILED
test translation::translator::tests::consume_stream_rejects_a_stream_that_never_terminates ... FAILED
test result: FAILED. 151 passed; 4 failed
# 还原后
test result: ok. 155 passed; 0 failed
```

**行为变更（重要）**：完全不用 `[DONE]`、也不发 `finish_reason` 的网关，现在会被判失败。
它与「连接被掐断」在协议上无法区分，按后者处理是刻意的取舍（见 §5）。

---

### F-03（低）错误信息双重前缀

**现象**：`error.rs` 的 `AppError::Llm` Display 是 `大模型调用错误: {0}`，而
`translate_attempts` 里 `last_err = err.to_string()` 后又包一层 `AppError::Llm(last_err)` →
用户看到 `大模型调用错误: 大模型调用错误: 流式响应结束但没有任何文本…`（红队 R-08b 同源）。

**复现（修复前）**：

```
$ grep -n 'API 返回 429' docs/review-r3/redteam-baseline.md
G3 events = Progress(...)×4 Error(...,"大模型调用错误: 大模型调用错误: API 返回 429 Too Many Requests: ")
```

**改动**（`src/translation/retry.rs`）：保留最后一次的 `AppError` **原样抛出**（不再重新包装），
并新增 `error_message()` 给结构纠错分支拼接用（避免内嵌第二个前缀）。顺带修好一个副作用：
非 `Llm` 类别的错误不再被强行改写成 LLM 错误。

**回归测试**：`translation::retry::tests::unauthorized_fails_after_a_single_attempt_without_double_prefix`
断言 `messages[0].1.matches("大模型调用错误").count() == 1`。

**变异证据**：把结尾改回 `Err(AppError::Llm(last_err.to_string()))` → 该测试 FAILED（自跑）。

---

### F-04（低）确定性 4xx 也在退避重试

**现象**：`401`（密钥错）、`403`、`404` 会照网络抖动一样重试 4 次，用户白等 3.5 秒才看到同一个错误。

**改动**（`src/translation/retry.rs` + `translator.rs`）：

- `translator.rs` 新增 `API_STATUS_PREFIX` 与 `api_status_error()`：状态码错误消息**单点产出**为
  `API 返回 {status}: {body}`（`body` 仍按字符截断到 500，中文不会被切半）；
- `retry.rs` 新增可单测的纯函数 `is_retryable_failure()` / `http_status_of()` /
  `is_retryable_status()`：`408` / `425` / `429` / 5xx 重试，其余 4xx 直接失败；
  **认不出来的一律当可重试**（绝不把网络抖动变成不可重试）；
- `translate_attempts` 在不可重试时立即返回，不再睡退避。

**回归测试**：
- `translation::retry::tests::http_status_codes_are_classified_for_retry`（400/401/403/404/405/413/422
  不重试；408/425/429/500/502/503/504 重试；302 不重试）
- `unrecognized_errors_stay_retryable`（网络错误、流读取失败、`Io`、`Cancelled`、以及消息里
  恰好出现「API 返回 401」但不是我们格式的字符串 → 全部仍可重试）
- `http_status_is_parsed_from_our_own_message_shape`
- `unauthorized_fails_after_a_single_attempt_without_double_prefix`（端到端接线：1 次调用、
  1 条 Error、`failed=1`、无 `Done`）
- `rate_limited_requests_are_still_retried`（429 重试到 `MAX_ATTEMPTS`，防「顺手收紧过头」）

**变异证据**（自跑）：① 去掉调用点 → `unauthorized_…` FAILED；② 把 `is_retryable_status` 改成恒 `true`
→ `http_status_codes_are_classified_for_retry` 与 `unauthorized_…` 双双 FAILED。

**未修**：`Retry-After` 仍被忽略（需要结构化状态码，跨 `error.rs`，见 §5）。

---

### F-05（低）SSE 解码器缓冲无上限

**现象**：服务端只要一直不发换行，`SseDecoder.buffer` 就无限增长（`data_lines` 同理）。

**改动**（`src/translation/sse.rs`）：新增 `MAX_LINE_BYTES = 1 MiB`、`MAX_EVENT_BYTES = 4 MiB`，
以及 `pub(crate) push_checked()` / `finish_checked()`；超限**返回错误**（不静默截断）。
公开的 `push()` / `finish()` 保持原语义（无上限），生产路径 `consume_chat_stream` 改走 `_checked` 版本。
解析循环抽成 `drain_lines()` 供两者共用。

**回归测试**：`translation::sse::tests::oversized_line_is_rejected_instead_of_buffered_forever`、
`oversized_event_accumulation_is_rejected`、`long_but_terminated_lines_are_still_fine`
（长但正常的行不受影响，防误报）。

**变异证据**（自跑）：删掉两处上限检查 → 前两条 FAILED。

---

### F-06（低）baseUrl 带 query 时拼出废地址

**现象（修复前探针输出）**：

```
$ cargo test -p bg3-translate-core --lib probe_tmp -- --nocapture
PROBE base="https://gw.test/v1?api-version=2024-02-01" -> https://gw.test/v1?api-version=2024-02-01/v1/chat/completions
PROBE base="https://gw.test/openai?x=1"               -> https://gw.test/openai?x=1/v1/chat/completions
PROBE base="https://gw.test/v1/chat/completions?api-version=1" -> https://gw.test/v1/chat/completions?api-version=1/v1/chat/completions
```

Azure 风格 / 自建网关常见这种写法，结果 100% 404。

**改动**（`src/types.rs::chat_completions_url`）：先把 `?`/`#` 之后的部分摘出来，路径部分做原有的
`/chat/completions`、`/v1`、补 `/v1` 三级归一，再把 query 接回末尾；文档表格补一行。

**回归测试**：`types::tests::chat_completions_url_keeps_the_query_string_at_the_end`
（三种 query 写法 + 尾斜杠 + query + 「无 query 的老行为逐字不变」）。

**变异证据**（自跑）：退回不拆 query → FAILED：
`left: "https://gw.test/v1?api-version=2024-02-01/v1/chat/completions"` /
`right: "https://gw.test/v1/chat/completions?api-version=2024-02-01"`。

---

### F-07（信息）NaN 温度穿过归一化，线上变成 `null`

**现象（修复前探针输出）**：

```
PROBE nan.temperature=NaN is_nan=true
PROBE wire={...,"temperature":null}
```

`f32::clamp` 对 NaN 无效（与任何数比较都是 false），序列化时 serde_json 把 NaN 写成 `null`，
部分网关直接 400。可达路径仅限进程内构造（JSON/IPC 传不了 NaN，`null` 会在反序列化阶段就报错）。

**改动**（`src/types.rs::normalized`）：NaN 退回默认温度（0.3），其他非有限值仍走 clamp（`∞→2.0`、
`-∞→0.0`）。

**回归测试**：`types::tests::nan_temperature_falls_back_to_the_default`。
**变异证据**（自跑）：退回 `clamp` → FAILED（`NaN 必须被归一化，实际: NaN`）。

---

### F-08（信息）`contains_cjk` 漏 CJK 扩展 F/G/H/I

**现象（修复前探针输出）**：

```
PROBE 扩展F U+2CEB0 contains_cjk=false
PROBE 扩展G U+30000 contains_cjk=false
PROBE 扩展H U+31350 contains_cjk=false
PROBE 扩展I U+2EBF0 contains_cjk=false
```

后果：用生僻字写的系列名不会被归到中文一侧，同一系列可能被拆成两组分别翻译（译名不一致）。

**改动**（`src/translation/series.rs::contains_cjk`）：按 Unicode 15.1 补齐
`2CEB0..=2EBEF` / `2EBF0..=2EE5F` / `30000..=3134F` / `31350..=323AF`。

**回归测试**：`translation::series::tests::contains_cjk_covers_the_extended_planes`（含边界：
扩展 I 末字算 CJK、紧邻下一码位不算）。
**变异证据**（自跑）：删掉新区间 → FAILED（`扩展F首字 应识别为 CJK`）。

---

## §3 被证伪的怀疑点（同样是结论，附实际命令）

以下都是**跑过之后判定「不是缺陷」**的项，命令与结果都来自本机 `cargo test`。

| 怀疑点 | 验证方式 | 结论 |
| --- | --- | --- |
| SSE 事件被切在两个分片中间会错乱 | `translation::sse::tests::survives_every_possible_chunk_split`（在两个事件 + `[DONE]` 的字节流上遍历**所有**切点）、`survives_byte_by_byte_feeding`、`consume_stream_survives_split_chunks`（字节级喂给真实 `consume_chat_stream`） | **证伪**：三种喂法结果一致（红队另用 3240 种组合独立得到同一结论，见其 §3-6） |
| UTF-8 字符被切成两半会乱码/panic | `keeps_multibyte_characters_split_across_chunks`（字节缓冲保留原始字节，只在整行处 `from_utf8_lossy`） | **证伪** |
| `data:` 有无空格、CRLF/LF/CR、注释行、`event:`/`id:`/`retry:` | `parses_event_without_space_after_colon`、`parses_crlf_events`、`parses_lone_cr_terminators`、`ignores_comments_and_other_fields`、`ignores_blank_lines_without_pending_data` | **证伪**（行为符合 SSE 规范） |
| 多行 `data:` 拼接 | `joins_multi_line_data_payloads` | **证伪**（按规范用 `\n` 拼接） |
| 服务端不发 `[DONE]` 但发 `finish_reason` | 新增 `consume_stream_accepts_finish_reason_without_done_marker` | **证伪**（仍然成功，F-02 的正向对照） |
| HTTP 200 + 错误 JSON / HTML 被当空译文成功 | 新增 `http_200_with_a_non_sse_body_is_an_error`（HTML、错误 JSON、`data:` 错误体 + `[DONE]` 三种） | **证伪**（全部报错，红队 §3-5 结论一致） |
| `choices` 空数组 / `delta.content` 为 null / 非 JSON 心跳 / 空 content | `parse_chat_chunk_tolerates_missing_pieces`、`parse_chat_chunk_returns_empty_string_for_empty_content` | **证伪** |
| 一次流里多个 choices 会串味 | 新增 `multiple_choices_take_the_first_one`（取第一个；请求从不设 `n>1`） | **证伪**（行为符合预期） |
| 网络重试会让 delta 拼成「坏译文+好译文」 | `network_retry_re_emits_progress_before_the_second_attempt`、`structural_retry_re_emits_progress_before_the_correction_attempt`、`every_failed_attempt_re_emits_progress_then_error`、`series_retries_do_not_emit_extra_progress` | **证伪**（每轮尝试前重发 `Progress`，前端据此清空累积） |
| 结构纠错重试会重复计费/占并发额度 | `structural_retry_does_not_change_network_retry_budget`、`structural_retries_stay_within_the_concurrency_limit` | **证伪**（纠错在同一 job 内，不额外占许可） |
| 取消在退避等待中不生效 | `cancel_during_backoff_emits_no_further_events`（`select!` 包住 sleep） | **证伪** |
| 并发上限被突破 | `run_respects_concurrency_limit`、`structural_retries_stay_within_the_concurrency_limit`（许可在 future 内部获取，避免 `FuturesUnordered` 死锁） | **证伪** |
| `all_done` 会漏发/多发（零条目、取消、全失败、panic） | `empty_input_only_emits_all_done`、`cancel_before_start_only_emits_all_done`、`failures_are_reported_per_entry`、`panicking_job_is_counted_as_failure_and_run_finishes` | **证伪**（每条路径恰好一次） |
| 已有译文的条目会被重复请求 | `memory_completed_entries_count_as_translated`、`non_pending_entries_are_ignored`、`plan_skips_entries_with_existing_target` | **证伪** |
| 规划会静默丢条目 | `planned_entry_count` + `identical_sources_share_one_request`、`series_members_merge_into_one_job`、`single_member_series_is_not_merged`，engine 里还有覆盖不完整的 `log::warn` 断言式兜底 | **证伪** |
| `style_hint` 截断会把中文切成半个字符 | `normalize_style_hint_trims_and_limits`、`build_user_prompt_truncates_long_style_hint`、`truncate_chars_never_splits_multibyte`（全部用 `chars()`） | **证伪** |
| 占位符顺序/重复/`{{1}}`/属性值里的占位符 | `placeholder_order_is_not_part_of_the_signature`、`duplicated_placeholder_is_reported`、`doubled_braces_are_scanned_by_the_inner_token`、`placeholders_inside_tag_markup_are_still_compared` | **证伪** |
| 比较用的尖括号（`HP < 5`、`a < b > c`、`<5>`）被误判成标签 | `comparison_text_is_not_a_tag`、`unbalanced_angle_bracket_is_plain_text`，外加 F-01 新增的 `find_tag_end` 引号处理 | **证伪**（真实的 `<LSTag Tooltip="a > b">` 现在也能正确识别） |
| `<i/>` 与 `<i></i>` 互相判失败 | `non_void_self_closing_equals_an_empty_pair` | **证伪**（已规范化） |
| `&lt;i&gt;` 被模型重新转义 → 误报 | `restored_entities_are_not_reported` | **证伪**（两边都还原再比较，与 `real_mod_sample` 的「转义尖括号」语料一致） |
| `chat_completions_url` 常见写法归一 | `chat_completions_url_accepts_every_common_base_url_shape`（尾斜杠 / `/v1` / 路径前缀 / 完整端点 / localhost 带端口） | **证伪**（query 那一种除外，已修 F-06） |
| `normalized()` 的 clamp 边界（concurrency=0、超长 model） | `llm_settings_clamps_temperature_and_fills_blanks`、`llm_settings_defaults_and_normalization`（`concurrency=999→64`、`0→1`） | **证伪**（NaN 除外，已修 F-07） |
| 事件/字段名与前端契约漂移 | `types::tests::translation_event_matches_frontend_contract`、`translation_entry_serializes_camel_case`、`translation_status_serializes_lowercase` + `scripts/check_ipc_contract.py`（见 §2 门禁） | **证伪** |

---

## §4 无法验证项与原因

1. **真实 LLM API 的服务端行为**：没有 key，也刻意不发任何外部请求。`finish_reason`、
   `Retry-After`、各网关的 SSE 形态只能按协议文档 + 手工构造的字节流验证。
2. **真机 BG3 对坏标签的渲染表现**：`<LSTag 类型="Spell">` 会被游戏读成什么样（忽略标签、
   显示原文、还是显示错乱文本）无法在本机验证 —— 能证明的只是「写进 PAK 的标签已经不是
   游戏预期的那一个」（见 F-01 的 `render()` 输出）。
3. **Windows 平台**：`native-tls` 分支、路径行为未在本机（Linux）验证。
4. **`Retry-After` 的真实收益**：未实现，无法评估。
5. **网关是否普遍发送 `[DONE]`/`finish_reason`**：没有真实网关可测，F-02 的行为变更影响面
   依赖用户反馈（见 §5 第 1 条）。

---

## §5 遗留风险与已知取舍

1. **F-02 的行为变更**：完全不用 `[DONE]`、也不发 `finish_reason` 的网关，现在每次请求都会
   被判失败（重试 3 次后条目落 `error`）。这类网关与「连接被掐断」不可区分，按后者处理是
   刻意选择：静默写半句译文进 PAK 的代价高于显式报错。如果确实遇到这种网关，需要改成
   「配置项开关」或「只依赖 finish_reason」。
2. **长文本撞输出上限会稳定失败**：目前没有自动拆分/续写机制，超长条目会重试 3 次后落 `error`，
   用户只能手工补或换模型。错误信息里已写清原因。
3. **`content_filter` 现在算失败**：被内容过滤的响应即使有文本也不接受。
4. **属性名收紧的误报面**：真实 MOD 若出现 `<LSTag>` 属性名大小写/拼写与模型输出不同，
   现在会判失败并触发一次纠错重试。format-auditor 已确认真实样本里没有 `<LSTag>`，
   误报风险主要来自合成语料；`attribute_names_are_part_of_the_signature` 覆盖了合法写法。
5. **F-09（已确认未修）**：job 级 panic 只计入 `summary.failed`、**不发 `Error` 事件**，
   前端靠「命令返回后回滚仍在飞的条目」兜底（`useTranslationRun.ts` 的收尾分支），
   所以不会卡在「翻译中」，但用户看不到失败原因。生产路径几乎不可达（`HttpTranslator`
   没有 panic 点），改动需要新的 `FidelityIssue`/错误传播设计，本轮不动。
6. **F-10（已确认未修）**：`429` 不读 `Retry-After`。要做得对，需要错误类型携带结构化状态码
   —— `AppError` 在 `error.rs`（shell-auditor 范围）。本轮的分类函数已把 `429` 归为可重试，
   只是退避时间固定 500ms/1s/2s。
7. **F-11（已确认未修）**：原文被直接拼进 user prompt（`原文：\n{source}`），没有分隔符防御，
   恶意 MOD 文本理论上可以注入指令。影响限于该条译文质量（结构保真校验仍然生效），
   修法（转义/加定界符）收益不确定，本轮不改。
8. **`ExactGroup` 的语义**：`normalize_consistency_key` 会把 `"Fireball"`、`"fireball."`、
   `" fireball "` 归一成同一组共享一份译文（含大小写与首尾标点差异）。这是既有设计
   （`identical_sources_share_one_request` 覆盖），但对「仅差一个句点」的条目会造成译名同化，
   记录为已知取舍。
9. **`parse_chat_chunk` 只取 `choices[0]`**：请求从不设 `n>1`，行为已用测试钉住。

---

## §6 门禁（真实输出，修复全部完成后）

```
$ cd /home/jason/bg3-translate
$ cargo fmt --all --check
fmt OK

$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Checking bg3-translate-core v1.0.0 (/home/jason/bg3-translate/crates/bg3-translate-core)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.22s

$ cargo test -p bg3-translate-core --all-targets
running 326 tests
test result: ok. 326 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.70s
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

$ cargo check -p bg3-translate --all-targets
    Checking bg3-translate v1.0.0 (/home/jason/bg3-translate/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.68s
```

基线对照：`263 + 6 + 6 = 275` → 收尾时 `326 + 7 + 7 = 340`（这个数字会随其他 writer 继续加测试而变动；
本次自测范围单独跑是 `translation::` 160 条 + `types::` 16 条）。新增/收紧的回归测试见 §2 各条。
公开 API 只做了**新增**（`SseDecoder::push_checked`/`finish_checked` 为 `pub(crate)`；
`parse_chat_chunk`、`SseDecoder::push/finish`、`TextTranslator`、事件语义均未改签名/语义）。

## §7 改动文件清单

| 文件 | 内容 |
| --- | --- |
| `crates/bg3-translate-core/src/translation/fidelity.rs` | F-01：属性名进 token、引号感知的 `find_tag_end`、`scan_attributes`、文档与假注释修正、测试收紧 |
| `crates/bg3-translate-core/src/translation/sse.rs` | F-02/F-05：`ChatStreamChunk`/`parse_chat_chunk_parts`/`truncation_reason`、`push_checked`/`finish_checked`/`drain_lines`、内存上限 |
| `crates/bg3-translate-core/src/translation/translator.rs` | F-02/F-04：`consume_chat_stream` 抽取、`ensure_stream_complete`、`API_STATUS_PREFIX`/`api_status_error` |
| `crates/bg3-translate-core/src/translation/retry.rs` | F-03/F-04：错误原样抛出 + `error_message`、`is_retryable_failure` 分类 |
| `crates/bg3-translate-core/src/translation/series.rs` | F-08：`contains_cjk` 补齐扩展区 |
| `crates/bg3-translate-core/src/types.rs` | F-06/F-07：query 归一、NaN 温度 |
| `docs/review-r3/engine.md` | 本报告 |
