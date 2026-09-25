# BG3 MOD 汉化工具 ✨

一个给《博德之门 3》MOD 用的桌面汉化工具：**打开 MOD → 调用大模型流式翻译 → 重新打包**。

只发布 **Windows x64**。这个工具的使用场景就是「在 Windows 上翻译 MOD，然后在 Windows 上玩游戏」。

---

## 下载

到 [Releases](../../releases) 下载：

| 文件 | 说明 |
| --- | --- |
| `*-Portable.zip` | **推荐**。解压后直接运行 `bg3-translate.exe`，配置与术语表放在 exe 同级 `config/`，随包带走、方便备份 |
| `*-Setup.exe` | NSIS 安装程序，简体中文界面，可选「仅当前用户」或「所有用户」 |
| `*-Installer.msi` | MSI 安装包，适合批量分发 |

`SHA256SUMS.txt` 是各产物的 SHA-256 校验和。

---

## 使用

1. 打开软件，选择 `.pak` 或 `.zip`（Nexus 上常见的 zip 包会被自动解开取里面的 pak）。
2. 在设置里填好大模型接口（OpenAI 兼容协议，默认对接 DeepSeek）。
3. 选中要翻译的文件，开始翻译。文本会边生成边显示。
4. 翻完后检查几条重点文本，可以直接在表格里改。
5. 保存并重新打包，输出 `<原名>_zh.pak`，丢进 MOD 管理器即可。

### 大列表也不卡

翻译表格是虚拟滚动的，两万行条目只渲染当前可见的十几行；可以用搜索框和
状态过滤 chips（全部 / 待翻译 / 翻译中 / 已翻译 / 出错）快速定位。
出错的条目可以单独「重试」，也可以一键「重试全部失败条目」。

### 给模型一点语境

如果某个 MOD 有特殊语境，可以在翻译界面里填一条提示，模型会按这个方向处理。

### 术语表

内置 102 条 BG3 官方核心译名（职业、种族、地名、角色、法术、机制……）开箱即用。
也可以导入从游戏里提取的完整官方术语表（2 万条级别），导入时会自动过滤噪音条目
（占位符模板、UI 内部标记、破折号碎片、`of`/`the` 这类会误匹配的短词）。

命中检测是在构造时一次性预处理好的，所以 2 万条术语表下逐条翻译依然是即时的。

---

## 配置放在哪里

按下面的顺序决定，日志里会写明实际用了哪个：

1. 环境变量 `BG3_TRANSLATE_HOME`
2. **便携模式**：exe 同级目录的 `config/`（能写就用它）
3. **系统模式**：`%APPDATA%\bg3-translate`

第 3 步是必要的：装到 `C:\Program Files` 时 exe 同级目录通常不可写。
配置文件是 `settings.json`（大模型连接信息）和 `glossary.json`（术语表），
写入采用「先写临时文件再改名」的原子方式，断电不会留下半个损坏文件。

---

## 开发

技术栈：**Tauri 2 + Rust 2024 + React 19 + TypeScript 7 + Tailwind CSS 4 + Vite 8**。

环境要求：Node/Bun、Rust 1.85+、以及 Tauri 对应的系统依赖（Windows 上装
[WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) 和 MSVC 构建工具即可）。

```bash
# 前端
bun install
bun run dev            # 只跑前端（vite）
bun run test           # vitest 单测
bun run build          # tsc 类型检查 + vite 构建

# 桌面应用
bun tauri dev
bun tauri build
```

### 工程结构

```
Cargo.toml                     # Cargo workspace：统一版本、依赖、release profile
crates/bg3-translate-core/     # 纯逻辑核心，零 GUI 依赖
  src/error.rs                 #   统一错误类型
  src/types.rs                 #   跨 IPC 的数据结构
  src/config.rs                #   数据目录解析 + 设置持久化
  src/pak.rs                   #   PAK / ZIP 解包与重打包
  src/formats/                 #   contentList XML / LSX / LOCA 三种格式
  src/glossary/                #   术语表数据、清洗、命中匹配
  src/translation/             #   LLM 流式翻译引擎
  tests/e2e_pak_flow.rs        #   真实 PAK 的端到端闭环测试
src-tauri/                     # Tauri 薄壳：命令层 + 事件桥 + 插件注册
src/                           # React 前端
docs/ARCHITECTURE.md           # 架构说明与冻结的 IPC 契约
```

**为什么要拆 crate**：核心逻辑不依赖任何 GUI 系统库，所以在没有
webkit2gtk / gtk / dbus 的机器（以及 ubuntu-latest 的 CI）上都能直接
`cargo test`，几十秒出结果；同时 Tauri 壳保持得很薄，命令层不做业务计算。

### 常用检查

```bash
# 核心逻辑：不需要任何 GUI 系统库
cargo test -p bg3-translate-core
cargo clippy -p bg3-translate-core --all-targets -- -D warnings
cargo fmt --all --check

# 前端
bun run test
bun run build
```

### 端到端测试

两层，都是真的读写 PAK：

- `crates/bg3-translate-core/tests/e2e_pak_flow.rs`：**造**一个 PAK 跑闭环
  ```
  造 MOD 目录树 → 打包成 .pak → 解包 → 识别文件类型 → 读条目 →
  翻译 → 写回 → 重新打包 → 再解包 → 校验 contentuid / 标签 / 占位符 / 未翻译内容
  ```
- `crates/bg3-translate-core/tests/real_mod_sample.rs`：拿仓库里那个**真实 Nexus MOD**
  （`samples/Appearance Edit Enhanced-*.zip`）跑同样一遍，并校验未翻译的
  lua 脚本逐字节没被动过。

合成样本只能验证「我以为格式是这样的」，真实样本才能验证「格式实际就是这样」。
`meta.lsx` 里 `Name` 是模块内部标识符（`GustavDev`）而类型同样是 `LSString`
这件事，就是真实样本测出来的——它绝不能进翻译白名单。

### 跨层契约检查

```bash
python3 scripts/check_ipc_contract.py
```

只依赖 Python 标准库，几秒出结果，核对：前端 `invoke` 的命令 ⊆ 后端注册的命令、
后端注册的命令 == `docs/ARCHITECTURE.md` 命令表，以及 `TranslationEvent` /
`TranslationStatus` / `PakFileKind` 三组枚举的 serde 名称与前端联合类型一致。
`src-tauri` 依赖 GUI 系统库、在很多机器上编不了，这个脚本就是补上的那道防线。

---

## CI / 发布

- `.github/workflows/ci.yml`：4 个并行 job —— 版本号一致性、核心库
  （fmt + clippy + 单测）、前端（单测 + 构建）、Windows 上的 Tauri 壳编译检查。
- `.github/workflows/release.yml`：只用 `windows-latest` 构建 NSIS + MSI + 便携版，
  整理成 4 个文件上传到 Actions 制品；打 tag 时同时创建 GitHub Release。
  手动触发时留空 `tag_name` 就只构建、不发布。

---

## 备注

工具会尽量保留标签、占位符、`contentuid` 和 `version`（这三样是游戏查表的句柄，
改了就会失效），但打包前最好还是抽几条关键文本进游戏里看一眼。

`.lsx` 里只翻译白名单字段（`Description` / `DisplayName` / `Title` / `Tooltip` /
`TooltipDescription`）且类型必须是 `LSString` / `LSWString`。

`Name` **故意不在白名单里**：`Mods/<mod>/meta.lsx` 的 `Name` 是模块内部标识符
（`GustavDev`），类型同样是 `LSString`，翻译它会让 MOD 直接失效。
`TranslatedString` 类型存的是 contentuid 句柄，也不会被翻译。

## 许可

MIT
