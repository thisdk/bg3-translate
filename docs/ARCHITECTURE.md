# 架构说明

## 为什么拆 crate

原来的工程是一个单体的 Tauri 应用：所有业务逻辑都写在 `src-tauri/src/`，
编译它需要 webkit2gtk / dbus / gtk 等一堆 GUI 系统库。结果是：

- 在**没装**这些系统库的 Linux 开发机上 `cargo test` 直接失败
- CI 上想跑单测就必须装一整套 GUI 依赖
- 核心逻辑（PAK 解析、术语表匹配、翻译引擎）无法独立复用

现在拆成 workspace：

```
Cargo.toml                    # workspace 根：统一版本、依赖与 release profile
crates/bg3-translate-core/    # 纯逻辑，零 GUI 依赖 → 本地/CI 都能秒测
src-tauri/                    # 薄壳：Tauri 命令 + Channel 事件桥
```

**铁律：`bg3-translate-core` 不允许依赖 `tauri`、不允许依赖任何需要
`pkg-config` 的系统库。** 一旦破坏这条，本地 `cargo test` 立刻失效。

## 模块职责

### `bg3-translate-core`

| 模块 | 职责 |
| --- | --- |
| `error` | 统一错误 `AppError`；实现 `Serialize` 以便直接作为 Tauri 命令错误返回 |
| `types` | 跨 IPC 的数据结构，字段名即 TypeScript 字段名 |
| `config` | 数据目录解析（便携/系统/环境变量）+ 设置读写（原子写） |
| `pak` | PAK / ZIP 解包、类型识别、重打包 |
| `formats::content_list` | `<contentList>` 本地化 XML |
| `formats::lsx` | `.lsx` 元数据里的可翻译字段 |
| `formats::loca` | `.loca` 二进制本地化 |
| `glossary` | 术语表数据、清洗、命中匹配 |
| `translation` | LLM 流式翻译引擎（prompt 构造、任务规划、SSE 解析） |

### `src-tauri`

只做三件事：

1. 把前端 `invoke` 映射到 core 的调用
2. 把 core 的 `TranslationEvent` 桥接到 Tauri `Channel`
3. 注册插件、权限、窗口

命令层不做业务计算，最多做参数归一化。

## 数据目录策略

优先级（见 `config::resolve_data_dir`）：

1. `BG3_TRANSLATE_HOME` 环境变量
2. **便携模式**：exe 同级 `config/`，可写就用（解压即用、方便备份）
3. **系统模式**：`%APPDATA%\bg3-translate`、`~/.config/bg3-translate`
4. 兜底：系统临时目录

第 3 步是必须的：MSI/NSIS 安装到 `C:\Program Files` 时 exe 同级目录只读。

## 冻结的 IPC 契约

> 改这里 = 同时改 `src/lib/types.ts`，否则前端会静默收不到数据。

### Tauri 命令

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `open_mod` | `filePath` | `ExtractResult` |
| `extract_mod` | `filePath`, `outputDir` | `PakFile[]` |
| `read_file_entries` | `workDir`, `fileName` | `TranslationEntry[]` |
| `write_file_entries` | `workDir`, `fileName`, `entries` | `void` |
| `repack_mod` | `workDir`, `outputPath` | `void` |
| `close_mod` | — | `void` |
| `translate_entries` | `workDir`, `entries`, `styleHint`, `onEvent` | `void` |
| `cancel_translation` | — | `void` |
| `save_llm_settings` | `settings` | `void` |
| `load_llm_settings` | — | `LlmSettings` |
| `list_glossary` | — | `Glossary` |
| `add_glossary_entry` | `entry` | `Glossary` |
| `update_glossary_entry` | `oldSource`, `entry` | `Glossary` |
| `delete_glossary_entry` | `source` | `Glossary` |
| `reset_glossary` | — | `Glossary` |
| `import_glossary` | `jsonStr` | `Glossary` |
| `app_info` | — | `AppInfo` |

### 事件（`TranslationEvent`，serde `tag = "type"`）

```jsonc
{"type":"progress","entryId":"...","status":"translating"}
{"type":"delta","entryId":"...","text":"..."}
{"type":"done","entryId":"...","text":"..."}
{"type":"error","entryId":"...","message":"..."}
{"type":"all_done","total":100,"failed":2}
```

事件顺序：`progress` → `delta`* → `done`（失败则 `error`，最后一定有一条 `all_done`）。

**每次尝试开始时都会重发一次 `progress`**（网络退避重试、结构纠错重试各算一次新尝试）：
前端据此把该条目的流式文本清零、重新累积，否则两轮 delta 会拼成
`坏译文 + 好译文`（`useTranslationRun.ts` 已按此实现）。`Series` 组不推 `delta`，
所以重试时也不额外发 `progress`；取消生效后不再发任何事件。

### `LlmSettings`

```jsonc
{
  "baseUrl": "https://api.deepseek.com",
  "apiKey": "",
  "model": "deepseek-chat",
  "concurrency": 6,
  "temperature": 0.3
}
```

旧配置里的 `batchSize` 字段已被忽略（批量 JSON 模式早已移除）。

## `translation` 模块的公开 API（`bg3-translate-core`）

```rust
/// 事件出口：真实运行时是 Tauri Channel，单测里是收集器。
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: TranslationEvent);
}

/// 可克隆的取消令牌。
#[derive(Clone, Default)]
pub struct CancelToken(/* Arc<AtomicBool> */);
impl CancelToken {
    pub fn new() -> Self;
    pub fn cancel(&self);
    pub fn is_cancelled(&self) -> bool;
    pub fn reset(&self);
}

pub struct TranslationSummary {
    pub total: usize,
    pub translated: usize,
    pub failed: usize,
    pub cancelled: bool,
}

pub struct RunOptions<'a> {
    pub style_hint: &'a str,
    pub sink: &'a dyn EventSink,
    pub cancel: CancelToken,
}

pub struct TranslationEngine { /* reqwest::Client + LlmSettings */ }
impl TranslationEngine {
    pub fn new(client: reqwest::Client, settings: LlmSettings) -> Self;
    pub async fn run(
        &self,
        entries: &[TranslationEntry],
        matcher: &GlossaryMatcher,
        options: RunOptions<'_>,
    ) -> Result<TranslationSummary>;
}
```

### `translation` 的内部子模块拆分

`engine.rs` 原本 1365 行（实现 + 测试混在一起），现在按职责拆开：

| 文件 | 职责 |
| --- | --- |
| `translation/engine.rs` | 并发调度（`Semaphore` + `FuturesUnordered`）、事件推送、job 级 panic 隔离 |
| `translation/engine/tests.rs` | 引擎级测试（事件序列 / 并发 / 取消 / 结构校验接线） |
| `translation/events.rs` | `EventSink` / `CollectingSink` / `CancelToken` / 进度事件 |
| `translation/translator.rs` | `TranslateRequest` / `TextTranslator` / `HttpTranslator`（reqwest + SSE；请求体用 types-only 的协议类型构造，见下节） |
| `translation/retry.rs` | 网络退避重试 + 结构纠错重试 + 结构校验接入点 |
| `translation/fidelity.rs` | 译文结构保真校验（纯函数，见下节） |
| `translation/test_support.rs` | 单测共享的假翻译器（仅 `cfg(test)`） |

**公开路径与签名不变**：`engine` 把 `events` / `translator` 里的名字再导出一次，
所以 `translation::{CancelToken, CollectingSink, EventSink, RunOptions, TextTranslator,
TranslateRequest, TranslationEngine, TranslationSummary}` 以及
`translation::engine::{...}` 这些拆分前的路径继续成立，`src-tauri` 无需改动。

### LLM 协议层：**类型**借第三方，**实现**自己写

OpenAI 兼容协议的「结构」与「传输」是分开的，改代码前先认清这条边界：

| 层 | 谁实现 | 位置 |
| --- | --- | --- |
| 请求体 / 流式 chunk 的**协议类型** | `async-openai 0.42` 的 **types-only 特性**（`default-features = false` + `chat-completion-types`） | `Cargo.toml`（workspace） |
| HTTP 客户端、超时、发请求、读流 | 本仓库（`reqwest`） | `translation/translator.rs` |
| SSE 帧解析（跨 chunk 切断 / CRLF / 多行 data / `[DONE]`） | 本仓库（纯状态机） | `translation/sse.rs` |
| 重试、退避、取消、结构纠错 | 本仓库 | `translation/{retry,events}.rs` |

- **为什么只借类型**：`chat-completion-types` 不含 `_api`，所以不会带进 `reqwest` /
  `tower` / `eventsource-stream` / `tracing`。本仓库依赖图的增量是 **7 个包**
  （`async-openai` + `derive_builder` 一脉 + `darling` 一脉）；改开自带 HTTP 客户端的
  `chat-completion` 特性集则是 35 个包。
- **线上格式有测试钉住**：`build_chat_request_matches_the_wire_format` 断言类型化请求
  序列化出来**恰好**是 `{model, stream, temperature, messages}` 四个字段，与拆分前手写的
  `json!` 逐字段一致（async-openai 的请求结构有几十个字段，不能被填成 `null` 一起发出去）。
  比对方式是 `to_string` 后再 `from_str`，**不能直接 `to_value`** —— 后者会把 f32 拓宽成
  f64，报出一个永远上不了线的数字（`0.30000001192092896`，正是旧实现线上真实发的值）。
- **流式 chunk 是两级解析**（`sse::parse_chat_chunk`）：先按标准协议类型反序列化，失败再
  退回只认 `choices[0].delta.content` 的宽松结构。标准结构要求 `id`/`index`/`created`/
  `model`/`object` 齐全，而部分网关只发最小字段 —— 只留类型化一条路会把这类响应全丢掉。
- **三处协议级修复**（v1.0.0）：
  1. `LlmSettings::chat_completions_url()` 收口 baseUrl 的四种写法（`types.rs` 里有对照表）。
     过去填 `https://api.deepseek.com/v1` 会拼成 `/v1/v1/chat/completions` → 404。
  2. 流读完却没有任何文本 → 报错，而不是返回空译文。空译文对「原文没有占位符/标签」的条目
     能通过结构校验、被记成「已翻译」（界面一片空白），只在写回时因 target 为空退回原文；
     现在它变成一次可重试的失败（原文本身为空时不报错）。最常见成因是网关把错误包成了 200，
     或服务端没按 SSE 返回。
  3. **被截断的流不再当成功**（`translator::ensure_stream_complete`）。两种情况都算失败：
     `finish_reason ∈ {length, content_filter}`（服务端自报不完整），或者
     **既没有 `[DONE]` 也没有任何 `finish_reason`**（连接被中途掐断，拿不到「输出完整」的证据）。
     报的是 `llm` 错误 → 走既有网络重试（1 次 + 最多 3 次退避）→ 仍失败则条目落 `error`、
     写回退回原文。半截译文（`模型输出不完整（finish_reason=length），已丢弃这次半截译文…`）
     过去能一路通过结构校验被打进 PAK，现在不会。
- **已知限制（有意保留，不在本次范围）**：
  1. `429` 不读 `Retry-After` 头，仍按固定退避（500ms → 1000ms → 2000ms）重试。
  2. 服务端忽略 `stream: true`、直接返回完整 JSON 时没有非流式兜底。
  3. **完全不用 `[DONE]`、也不发 `finish_reason` 的第三方网关现在会被判失败**
     （重试 3 次后条目落 `error`、退回原文）。这是与上一条修复配套的**刻意取舍**：
     这类响应与「连接被中途掐断」在客户端不可区分，而「宁可失败也不要静默丢内容」——
     以前它们能用，是因为半截/未知完整性的译文被当成成功写进了 PAK。
     报错文案会明说原因与下一步（`流式响应结束，但既没有收到 [DONE] 也没有 finish_reason，无法确认输出完整（连接可能被中途掐断）；若在自定义网关上出现，请确认它按 OpenAI 协议发送 [DONE] 或 finish_reason，否则换用标准端点后重试`），
     用户可据此换用遵守 OpenAI 流式协议的端点，或点「重试」。

**重试分类**（`retry::is_retryable_failure`，纯函数，可单测）：
`408` / `425` / `429` / 全部 `5xx` → 可重试；**其余 4xx 直接失败**，不再退避重试 3 次；
消息里认不出状态码时**按可重试处理**（宁多重试一次，也不要把可恢复的错误当致命错误）。

### 译文结构保真校验（`translation::fidelity`）

系统 prompt 一直要求「占位符原样保留、富文本标签完整保留」，但从前的代码**从不校验**：
模型丢了占位符 / 标签也会被当成成功译文写出去。现在每个 job 拿到译文后都会比对
**结构签名**（只比结构，不比内容）：

| 检查项 | 抓什么 | 明确不抓什么（防误报） |
| --- | --- | --- |
| 占位符（花括号） | `{1}` / `{10}` / `{name}` / `{user_name}` 的多重集必须一致（缺失 / 多余 / 重复都报）；顺序不计（`{1} {2}` ↔ `{2} {1}` 保真） | `{}`、`{ }`、`{a b}`、`{"k": 1}`、`{#FFAA00}`、`{-1}` 都不算占位符 |
| 占位符（方括号） | `[1]` / `[10]`（游戏替换数值）、`[IE_PanelSelect]` / `[DRUID]`（内部 ID）的多重集必须一致，顺序同样不计。语料依据：官方术语表 163 条含 `[数字]` 的条目，**官方简中 163/163 全部原样保留**，其中 `, [1] from [2]` → `，从[2]处取走了[1]` 还换了位 | 混合大小写的 `[Note]` / `[Draft]` / `[todo]` 是方括号散文，不算占位符。判定与 `glossary::entry` 里过滤术语噪音的 `PLACEHOLDER_PATTERN` / `UI_MARKER_PATTERN` 保持一致 —— 两边对「什么算占位符」不能有分歧 |
| 占位符粘连 | 译文里两个占位符**零间隔相邻**而原文不是（`[1] [2]` → `[1][2]`）：游戏分别替换后中间没有分隔，会渲染成一个数 `12` | 判据是**非对称**的：只报「译文比原文更粘」，加分隔（空格 / 顿号 / 逗号）永远放行；占位符本身缺了 / 多了时不再叠加报粘连（那多半只是重复的副作用，会淹掉真正的原因） |
| 占位符周围的空格 | **不参与比对** | `deal [2] damage` 译成「造成[2]点伤害」是正确中文 —— 英文靠空格分词、中文不靠。官方简中里 `[N]` 两侧带空格只剩 6.9% / 3.4%（英文源是 61.3% / 38.7%），保留下来的都是载重分隔符（`[1] [2]` 之间、`+ [1]` 运算符之后），而分隔符种类不限 —— 所以只校验「不许粘连」 |
| 标签 | 写回白名单标签（`LSTag`/`font`/`i`/`b`/`u`/`br`/`span`/`em`/`strong`）的开 / 闭 / 空元素**多重集**必须一致；**开始标签的属性名多重集也必须一致**（大小写敏感、顺序不计）；标签内文本不参与比较 | **标签顺序完全不参与比对** —— 中英语序不同，标签跟着各自包住的正文换位是常态（真实例子：`Inflicts <A>Deadly Toxin</A> ... fails <B>Saving Throw</B>` 译成中文后条件从句提前，两个标签必然对调）。旧实现要求顺序一致，把这种**正确译文**判失败后退回英文，比不查糟糕得多。另外不抓：`< 5`、`a < b`、`a < b > c`、`<5>`、`x <y` 这类比较文本；白名单外的 `<name>`/`<color>`（写回时会转义成普通文本，没有配对义务）；属性名的顺序 |
| 属性**值** | **key 型属性的值必须逐字一致**。三道条件同时成立才查：属性名 ∈ `KEY_ATTRIBUTES`（`Tooltip` / `Type`，唯一扩展点）+ **原文**值是标识符形态（非空、仅 ASCII 字母数字下划线）+ 值带引号。依据：真实语料 1103 对 `<LSTag Tooltip="KEY">BODY</LSTag>` 里，`Tooltip="VENOMOUS_BARBS_CONDITION"` 包着 "Deadly Toxin"、`HitPoints` 包着 "hit points" —— key 是查表 ID，正文才是显示文本；翻了 key 游戏查不到表，tooltip 直接失效。比对是多重集单向包含；标签名多重集对不上时不叠加报（那种情况值没被动过，报它是误导） | 非 key 型属性（`Tag`/`color`/…）的值照旧自由；**key 属性里含空格的自然语言值照旧自由** —— 刻意的盲点：真实语料 279 个 `Tooltip` 取值一个空格都没有，把自然语言也收进来只增误报面，而误报会把整条正确译文退回英文。空值、含连字符的值同理放行 |
| 标签嵌套 | 只查**开闭不配对**：落单的闭合标签（栈下溢），或**不同名**标签之间交叉嵌套（`<LSTag><b></LSTag></b>`）。门槛是「原文本身合法」——原文自己就不配对时（MOD 本来就坏）不为难模型 | 合法的换位与重排：`<b></b><i></i>` ↔ `<i></i><b></b>`、`<b><i></i></b>` ↔ `<i><b></b></i>` 都放行；标签数量对不上由上面的多重集负责报。**同名标签之间不存在「交叉」**：`<b>x<b></b>y</b>` 就是合法内层嵌套（`</b>` 的语义是「关掉最内层那个」），所以当前语料（只有 `LSTag`、嵌套深度 1）里这条实际抓到的是落单的闭合标签。注意写文档举例必须用**白名单内**的标签名：`<a>` 不在白名单，放行它不是因为「嵌套合法」，而是它根本没进签名 |
| 空元素 | `<br/>`、`<br>`、`<br />` 等价，都不要求闭合；非空元素的 `<x/>` 与 `<x></x>` 也等价（XML 语义相同） | 空元素整个丢失仍然会报；代价是非空元素「空标签」与「包住文本的标签对」也分不出来（正文位置本来就不参与比对） |
| 实体 | `&lt;` / `&gt;` 先还原成字面尖括号再比较 | 模型把译文里的 `<` 重新转义成 `&lt;` 不算结构变化（解析层本来也还原过一次）。**这条容忍有代价**：校验会放行转义形态，只能靠**写回层**兜住 —— 见下面「转义形态由写回层兜底」 |

**可确定的占位符写法差异会在**校验之前**被修回原文形态**
（`fidelity::repair_placeholders`，由 `retry::translate_with_retry` 调用）：

| 修复 | 例子 | 只在什么时候动手 |
| --- | --- | --- |
| 全角 → 半角 | `【1】` / `［1］` → `[1]` | 内层是合法占位符形态才修，所以译文里作为标点用的 `【注意事项】` 不受影响 |
| 括号类型被模型改写 | `[1]` → `{1}`（或反向） | **只在原文只使用了一种括号类型时**。原文同时含 `{1}` 和 `[1]` 时不猜（两个 token 长得一样，分不清谁是谁改的），交给校验报错；只改内层名字在原文里出现过的 token，所以模型凭空多写的 `{2}` 不会被洗白 |

修的位置**必须在结构校验之前**：只在签名层归一化的话，`【1】` / `{1}` 会被判成保真、
然后原样写进 PAK —— 游戏替换不了它们，「报错可见」就变成了「静默损坏」。
放在 `translate_with_retry` 里还有个好处：纠错重试的比较、`Done` 事件与最终落盘的
文本看到的是同一个版本。

- **取舍**：上表「转义等价」是刻意容忍，**不等于写回安全** —— 模型若把真标签写成
  `&lt;LSTag&gt;`，校验判它保真，只能靠写回层的实体还原把它写对（见下面「转义形态由
  写回层兜底」）。这条防线拦的是「结构丢失」，不是「转义风格」；两者都拦会误伤
  大量正常译文。
- `Series` 组只翻了 base，所以校验跑在**合成后的完整译文**上（成员后缀里也可能带 `{1}`）。
- 校验不通过 → 把具体问题拼进请求**自动重试一次**（重试前重发一次 `progress`，
  前端据此丢弃上一轮被拒的流式文本）；仍不通过 → 按失败处理，
  发出 `Error`（message 形如 `大模型调用错误: 结构校验未通过：占位符 {1} 缺失（已重试 1 次）`），
  **绝不发 `Done`**，坏译文不会静默当成功。
- 纠错重试**不占用**网络重试额度（网络仍是 1 次 + 最多 3 次退避重试），
  仍在同一个 `Semaphore` 配额内、仍照常响应取消；事件顺序语义不变
  （`Progress` → `Delta`* → `Done` / `Error`，最后一条 `AllDone`）。

### 转义形态由**写回层**兜底（独立验证 D1 / D9）

上一节那条「转义等价」不是纸上谈兵，它是一条**可复现的静默损坏链**：

```
原文    <LSTag Tooltip="HitPoints">hit points</LSTag>
模型输出 &lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;
  → check_fidelity 返回 []（restore_entities 还原后签名一致，判**保真**）
  → 写回层再转义一次 → 落盘 &amp;lt;LSTag …&amp;gt;
  → 游戏读到字面量 &lt;LSTag …&gt;，tooltip 失效
  → 再解析回来文本是 &lt;LSTag …&gt;，**再校验仍判保真**（不会自愈）
```

`retry` 层只修占位符写法（`repair_placeholders`），`fidelity` 层**按设计放行**（改它会误伤大量正常译文）。
所以防线落在**写回层**：写回前先做一次实体还原再统一转义，让 `escape(decode(T))` 成为规范形。

```
模型输出      还原后        落盘          游戏读到
&lt;LSTag&gt;   →  <LSTag>   →  &lt;LSTag&gt;  →  <LSTag>（真标签）   ✅
HP &lt; 5     →  HP < 5     →  HP &lt; 5    →  HP < 5            ✅
&amp;         →  &          →  &amp;         →  &                 ✅
```

**风格分派是必须的**（`content_list::render_with_style`）：
`Escaped` 风格整条会被再转义、标签只是正文 → **全解**；
`Markup` 风格标签会被**裸写**，而 `parse` 故意保留属性值的原始转义 → **跳过真标签区间**不解码。
「一律跳过」在 Escaped 下会反向多出一层 `&amp;amp;` —— 这是实现时实测出来的。

**D9（T6 引入的回归，已修）**：第一版把 `decode_entities` 作用在整条文本（含标签内部），
于是**零译文写回**就能把合法的 `<LSTag Tooltip="a&amp;b">` 写成 `Tooltip="a&b"`，
产物不是合法 XML（Python expat 报 `not well-formed`），而且连写 3 轮字节相同 —— 坏文件不自愈。
修法是上面的风格分派，另加 `repair_tag_attributes`（裸写标签前把属性值规范成 `escape(decode(v))`，
只在值非规范时才重写，所以引号风格 / 属性顺序 / `Tooltip="a > b"` 全部逐字节不动）。
`docs/CORPUS-AUDIT.md` 的 D1 / D9 有完整复现与负对照。

**它能藏这么久的原因**：`samples/english.xml` 里 `&` 出现 **0 次**，98.68% 的往返哨兵看不见；
quick-xml 宽松，本仓库自己的 `parse` 照样接受非法产物；全仓原先没有严格 XML 校验器。
现在 `corpus_writeback.rs` 自带一条**严格校验**辅助（`check_end_names` + 属性值必须能反转义 +
属性值禁裸 `<` + 文本禁裸 `&`），并且独立验证用 Python expat 复验。

**两条已知取舍**：

- **`&amp;lt;` 每保存一次降一层**：源文件里真想显示 `&lt;` 的文本（文件写 `&amp;lt;`），
  每写回一次就少一层转义，直到没有实体形态为止（`&amp;amp;lt;` 要 3 次保存收敛）。
  接受它的理由：真实语料 `&` 0 处 → 影响面 0；方向对（**能自愈旧版本写坏的文件**）；
  会收敛不会无限恶化。
- **`&quot;` 残留盲点**：Markup 风格 + 模型把**整条标签**转义 + 属性里带 `&quot;` 时，
  解码出的裸引号让标签在扫描阶段就认不出来，整条降级成字面文本。
  三道条件同时成立才触发，产物**仍然合法**、文本内容不丢，只是标签没生效。
  要堵它得在解码前按「转义标签」语法解析属性，性价比不足，已钉成测试并留了升级路径。

**边界**：兜底只发生在**落盘**。`Done` 事件与条目内存里的文本仍是模型输出的转义形态，
所以界面上看到的和最终写进 PAK 的不完全一致 —— 这是刻意的（改动事件契约的代价更大）。

```rust
/// 一条结构保真问题；`Display` 输出简短中文原因。
pub enum FidelityIssue {
    MissingPlaceholder { token: String, count: usize },   // token 形如 `{1}` / `[2]`
    ExtraPlaceholder { token: String, count: usize },
    MissingTag { tag: String, count: usize },   // tag 形如 `<LSTag>` / `</LSTag>` / `<br/>`
    ExtraTag { tag: String, count: usize },
    AttributeValueChanged { name: String, value: String, count: usize },  // key 型属性值被改写
    TagNestingBroken,                           // 开闭不配对；标签**顺序**不参与比对
    GluedPlaceholders { count: usize },         // `[1] [2]` 被写成 `[1][2]`
}

pub fn check_fidelity(source: &str, target: &str) -> Vec<FidelityIssue>;
pub fn is_faithful(source: &str, target: &str) -> bool;
pub fn summarize(issues: &[FidelityIssue]) -> String;         // Error 事件 message 用
pub fn correction_hint(issues: &[FidelityIssue]) -> String;   // 纠错重试请求用

/// 占位符写法修复；由 `retry` 在校验**之前**调用（见上面的表）。
pub fn repair_placeholders(source: &str, target: &str) -> String;
pub fn repair_full_width_brackets(text: &str) -> String;      // 上面修复的第一类，单独可用

/// 结构纠错重试入口：默认实现忽略提示、退回 `translate`，既有实现不需要改。
pub trait TextTranslator {
    fn translate_with_correction<'a>(
        &'a self,
        request: TranslateRequest<'a>,
        correction: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>>>;
}
```

## `contentList` 的标记编码风格：写回**绝不翻转**（`formats::content_list`）

同一个「带标记的逻辑文本」在 contentList XML 里有两种等价写法，而**真实 BG3 文件
用的是转义形态**：

```xml
<!-- Escaped：真实语料 1971 条 / 1513 处标记，100% 是这种 -->
<content contentuid="h0a58…" version="2">regain half as many &lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;.</content>

<!-- Markup：标记是真 XML 元素（旧实现无条件写这种） -->
<content contentuid="h0a58…" version="2">regain half as many <LSTag Tooltip="HitPoints">hit points</LSTag>.</content>
```

两者**解析后的文本完全相同**，所以只比对文本的测试抓不到差异 —— 这正是这个缺陷
在仓库里藏了很久的原因：既有「真实样本」是个 41 条的捏脸 MOD，里面 `{`、`[`、`<`
一个都没有；e2e 夹具又全是手写的真元素形态，属于自证。

现在 `parse` 会记录本文件用的是哪种（`MarkupStyle::Escaped` / `Markup`：
`<content>` 内部真的解析出白名单子元素 → `Markup`，否则 `Escaped` 且为默认），
`write()` 重读磁盘时取出该风格交给 `render_with_style` —— **写回跟随源文件，绝不翻转**。
新建文件（磁盘上不存在）没有源可参考，默认 `Escaped`（真实形态；转义永不产出非法 XML）。

实测效果（判据 = 每个 `<content>` 元素的原始字节）：

| | 一致 | 比例 |
| --- | --- | --- |
| 修复前 | 1301 / 1971 | 66.01% |
| 修复后 | 1945 / 1971 | 98.68% |

修复前的 670 条差异 = 570 风格翻转 + 91 `'` 被写成 `&apos;` + 9 尾空格。前两类都已修：
`'` / `"` 改用 `quick-xml` 的 `partial_escape`（只转 `<` `>` `&` `\r`）原样输出。
剩余 26 条全部是**预先存在**的 `parse` 尾空格归一化（`raw.trim()`），与风格无关，刻意不动。

**已知取舍**：整文件字节不保证相等 —— `render` 不保留元素间空白与 CRLF，输出是单行。
这不影响游戏解析，写回第一次后就稳定。

## 写回不变量：`status == error` 的条目不写译文

结构校验失败（或网络中断）后条目状态是 `error`，但 `target` 里仍留着被拒译文 /
半截流式文本（前端要展示给用户看）。写回链路因此**不能只看「target 是否非空」**，
否则用户不点「重试失败」直接打包，坏译文照样进 PAK：

| 方法（`types::TranslationEntry`） | 语义 |
| --- | --- |
| `has_target()` | 仅表示「target 非空」，供展示 / 统计用，**不代表能写回** |
| `has_writable_target()` | `target` 非空 **且** `status != error`，写回的唯一闸门 |
| `effective_text()` | 有可写回译文用译文，否则保留原文（`error` 条目一律退回原文） |

- 三个格式都走这条闸门：`content_list` / `loca` 用 `effective_text()`，
  `lsx::plan_replacements` 用 `has_writable_target()`（`error` 条目不改写字段）。
- 人工编辑过的条目状态是 `edited`，照常写回；`error` 条目被用户手工救回后也会变成
  `edited`，所以「翻译失败 → 手动改好 → 打包」这条路仍然通。
- 设计取舍：`error` 状态是「这条译文不可信」的唯一信号，不引入新的字段
  （IPC 契约不变）；代价是 `translating`（取消后残留的半截译文）仍会被写回 ——
  该路径由前端把关：`useTranslationRun.ts` 在取消 / 收尾时把未完成条目回滚成
  `pending` + 空 `target`。

## `glossary` 模块的公开 API

```rust
pub struct GlossaryEntry { source, target, category, source_kind, enabled, ambiguous, whole_word, case_sensitive, count }
pub struct Glossary { pub terms: Vec<GlossaryEntry> }

impl Glossary {
    pub fn seeded() -> Self;                        // 内置官方种子
    pub fn load() -> Result<Self>;                  // 从当前数据目录读
    pub fn load_from(dir: &Path) -> Result<Self>;   // 指定目录（测试用）
    pub fn save(&self) -> Result<()>;
    pub fn save_to(&self, dir: &Path) -> Result<()>;
    pub fn reset() -> Result<Self>;
    pub fn from_json(json_str: &str) -> Result<Self>;   // 纯解析 + 清洗，不落盘
    pub fn import_json(json_str: &str) -> Result<Self>; // 解析 + 清洗 + 落盘
    pub fn add(&mut self, entry: GlossaryEntry) -> Result<()>;
    pub fn update(&mut self, old_source: &str, entry: GlossaryEntry) -> Result<()>;
    pub fn delete(&mut self, source: &str) -> Result<()>;
    pub fn matcher(&self) -> GlossaryMatcher;
}

pub struct GlossaryMatcher { /* 预编译索引 */ }
impl GlossaryMatcher {
    pub fn new(glossary: &Glossary) -> Self;
    pub fn find_matches(&self, text: &str) -> Vec<MatchedTerm>;
}
pub struct MatchedTerm { pub source: String, pub target: String }
```

**`GlossaryMatcher` 存在的原因**：2 万条术语表下，旧实现对每条待翻译文本都要
遍历全部术语、并对每条术语调用一次 `to_lowercase()` 和（命中时）
`Regex::new()`，也就是每条文本约 2 万次堆分配。匹配器把 lowercase 与正则
在构造时一次性预处理，`find_matches` 只做查表 + 校验。

## 测试策略

- `bg3-translate-core`：单元测试与实现同文件（`#[cfg(test)] mod tests`），
  需要临时目录时用 `tempfile`；需要 HTTP 时把请求层抽象成 trait 注入 fake。
  翻译引擎的测试放在 `translation/engine/tests.rs`（`#[cfg(test)] mod tests;`），
  假翻译器与断言辅助集中在 `translation/test_support.rs`。
- 真实样本用例（`tests/real_mod_sample.rs`、`glossary::store`、`glossary::matcher`）
  在样本缺失时**直接失败**并提示 `git checkout -- samples/`，不再 `eprintln!` 后跳过：
  `samples/` 随仓库提交（非 Git LFS），跳过只会让真实数据用例静默变空、而 `cargo test`
  依然全绿。`missing_sample_fails_loudly_instead_of_skipping` 专门盯着这条防线本身。
- 端到端：`crates/bg3-translate-core/tests/e2e_pak_flow.rs` 真的造一个 PAK，跑
  「解包 → 识别类型 → 读条目 → 翻译 → 写回 → 重打包 → 再解包」的完整闭环。
- 前端：`vitest` 测纯逻辑（路径改写、store reducer、过滤统计、虚拟滚动）。
- **跨层契约**：`scripts/check_ipc_contract.py`（Python 标准库，秒级）核对
  命令注册 / 前端 `invoke` / 本文件命令表三处集合相等，并核对
  `TranslationEvent`、`TranslationStatus`、`PakFileKind` 三组枚举的 serde 名称
  与 `src/lib/types.ts` 的联合类型一致。它在 CI 的 `meta` job 里跑。
- `src-tauri` 没有独立目录的集成测试，但**命令层有单元测试**：
  `cargo test -p bg3-translate --lib` 覆盖 `work_dir` / `file_name` 的越权校验
  （见下节）与纯逻辑；壳层刻意做得很薄（只做参数转发与事件桥接）。
  **能不能在本机编译，取决于 GUI 系统库是否就位**：装好 webkit2gtk / gtk / dbus 的
  Linux 与 Windows 都可以直接跑
  `cargo check -p bg3-translate --all-targets` 与
  `cargo clippy -p bg3-translate --all-targets -- -D warnings`；ubuntu-latest 的
  CI runner 历史上没有这些库，所以 CI 里这两条固定在 Windows `tauri-shell` job 上跑。
  **同一 job 还会真正执行这些单元测试**（`cargo test -p bg3-translate --lib`）：
  `--all-targets` 只**编译**测试不执行，少了这一步，越权校验的回归防线就只是静默失效。
  这三条命令**故意不进** `scripts/verify.sh`：门禁要在所有开发机与 ubuntu CI 上都能过，
  把「本机装了 GUI 库」当成必要条件会误伤没装的机器。

## 前端可控参数的信任边界（`src-tauri` 命令层）

`read_file_entries` / `write_file_entries` / `repack_mod` 的 `work_dir` 与 `file_name`
都是**前端可控字符串**，core 里的 `pak::resolve_disk_path` 只做纯 `join`。因此命令层
必须先校验再交给 core：

- `file_name` 过 `pak::safe_output_path`（拒绝 `..`、绝对路径、盘符、`:`、Windows 保留名）；
- `work_dir` 必须等于 `open_mod` 记在 `AppState` 里的那个目录（`is_same_dir`：
  先 `canonicalize`，失败退回词法比较），并且**用记录值而不是前端传的值**当读写根目录。

用户通过系统对话框选出来的路径（`extract_mod` 的 `outputDir`、`repack_mod` 的
`outputPath`）合法地可以指向任意位置，**不**做限制。

## 运行

```bash
# 核心逻辑（不需要任何 GUI 系统库）
cargo test -p bg3-translate-core

# 前端
bun install && bun run test && bun run build

# 桌面应用（需要 GUI 系统库；Windows 上直接可用）
bun tauri dev
```
