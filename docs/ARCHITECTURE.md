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
