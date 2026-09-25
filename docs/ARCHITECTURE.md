# 架构说明

## 为什么拆 crate

原来的工程是一个单体的 Tauri 应用：所有业务逻辑都写在 `src-tauri/src/`，
编译它需要 webkit2gtk / dbus / gtk 等一堆 GUI 系统库。结果是：

- 在 Linux 开发者机器上 `cargo test` 直接失败（缺系统库）
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
| `translation/translator.rs` | `TranslateRequest` / `TextTranslator` / `HttpTranslator`（reqwest + SSE） |
| `translation/retry.rs` | 网络退避重试 + 结构纠错重试 + 结构校验接入点 |
| `translation/fidelity.rs` | 译文结构保真校验（纯函数，见下节） |
| `translation/test_support.rs` | 单测共享的假翻译器（仅 `cfg(test)`） |

**公开路径与签名不变**：`engine` 把 `events` / `translator` 里的名字再导出一次，
所以 `translation::{CancelToken, CollectingSink, EventSink, RunOptions, TextTranslator,
TranslateRequest, TranslationEngine, TranslationSummary}` 以及
`translation::engine::{...}` 这些拆分前的路径继续成立，`src-tauri` 无需改动。

### 译文结构保真校验（`translation::fidelity`）

系统 prompt 一直要求「占位符原样保留、富文本标签完整保留」，但从前的代码**从不校验**：
模型丢了占位符 / 标签也会被当成成功译文写出去。现在每个 job 拿到译文后都会比对
**结构签名**（只比结构，不比内容）：

| 检查项 | 抓什么 | 明确不抓什么（防误报） |
| --- | --- | --- |
| 占位符 | `{1}` / `{10}` / `{name}` / `{user_name}` 的多重集必须一致（缺失 / 多余 / 重复都报）；顺序不计（`{1} {2}` ↔ `{2} {1}` 保真） | `{}`、`{ }`、`{a b}`、`{"k": 1}`、`{#FFAA00}`、`{-1}` 都不算占位符 |
| 标签 | 写回白名单标签（`LSTag`/`font`/`i`/`b`/`u`/`br`/`span`/`em`/`strong`）的开 / 闭 / 空元素序列必须一致；属性值与标签内文本不参与比较 | `< 5`、`a < b`、`a < b > c`、`<5>`、`x <y` 这类比较文本；白名单外的 `<name>`/`<color>`（写回时会转义成普通文本，没有配对义务）；属性值被改写 |
| 空元素 | `<br/>`、`<br>`、`<br />` 等价，都不要求闭合；非空元素的 `<x/>` 与 `<x></x>` 也等价（XML 语义相同） | 空元素整个丢失仍然会报；代价是非空元素「空标签」与「包住文本的标签对」也分不出来（正文位置本来就不参与比对） |
| 实体 | `&lt;` / `&gt;` 先还原成字面尖括号再比较 | 模型把译文里的 `<` 重新转义成 `&lt;` 不算结构变化（解析层本来也还原过一次） |

- **取舍**：上表「转义等价」是刻意容忍，**不等于写回安全**：模型若把真标签写成
  `&lt;LSTag&gt;`，写回层 `write_text_fragment` 会把 `&` 再转义一次，产物里是可见的
  `&amp;lt;LSTag&amp;gt;`（玩家看到字面量 `&lt;LSTag&gt;`）。这条防线拦的是「结构丢失」，
  不是「转义风格」；两者都拦会误伤大量正常译文。
- `Series` 组只翻了 base，所以校验跑在**合成后的完整译文**上（成员后缀里也可能带 `{1}`）。
- 校验不通过 → 把具体问题拼进请求**自动重试一次**（重试前重发一次 `progress`，
  前端据此丢弃上一轮被拒的流式文本）；仍不通过 → 按失败处理，
  发出 `Error`（message 形如 `大模型调用错误: 结构校验未通过：占位符 {1} 缺失（已重试 1 次）`），
  **绝不发 `Done`**，坏译文不会静默当成功。
- 纠错重试**不占用**网络重试额度（网络仍是 1 次 + 最多 3 次退避重试），
  仍在同一个 `Semaphore` 配额内、仍照常响应取消；事件顺序语义不变
  （`Progress` → `Delta`* → `Done` / `Error`，最后一条 `AllDone`）。

```rust
/// 一条结构保真问题；`Display` 输出简短中文原因。
pub enum FidelityIssue {
    MissingPlaceholder { token: String, count: usize },
    ExtraPlaceholder { token: String, count: usize },
    MissingTag { tag: String, count: usize },   // tag 形如 `<LSTag>` / `</LSTag>` / `<br/>`
    ExtraTag { tag: String, count: usize },
    TagStructureChanged,
}

pub fn check_fidelity(source: &str, target: &str) -> Vec<FidelityIssue>;
pub fn is_faithful(source: &str, target: &str) -> bool;
pub fn summarize(issues: &[FidelityIssue]) -> String;         // Error 事件 message 用
pub fn correction_hint(issues: &[FidelityIssue]) -> String;   // 纠错重试请求用

/// 结构纠错重试入口：默认实现忽略提示、退回 `translate`，既有实现不需要改。
pub trait TextTranslator {
    fn translate_with_correction<'a>(
        &'a self,
        request: TranslateRequest<'a>,
        correction: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>>>;
}
```

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
- `src-tauri` 本身**没有**集成测试：它依赖 webkit2gtk/gtk/dbus，本机与
  ubuntu CI 都编译不了。壳层刻意做得很薄（只做参数转发与事件桥接），
  验证手段是 CI 里 Windows runner 上的 `cargo check` + `cargo clippy -D warnings`，
  以及上面那个契约脚本。这是有意的取舍，不是遗漏。

## 运行

```bash
# 核心逻辑（不需要任何 GUI 系统库）
cargo test -p bg3-translate-core

# 前端
bun install && bun run test && bun run build

# 桌面应用（需要 GUI 系统库；Windows 上直接可用）
bun tauri dev
```
