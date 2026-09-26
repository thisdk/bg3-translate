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
2. 在设置里填好大模型接口（OpenAI 兼容协议，默认对接 DeepSeek）。base URL 填
   `https://api.deepseek.com` 或 `https://api.deepseek.com/v1` 都可以，也可以直接
   粘完整的 `…/v1/chat/completions`，三种写法会落到同一个端点。
3. 选中要翻译的文件，开始翻译。文本会边生成边显示。
4. 翻完后检查几条重点文本，可以直接在表格里改。
5. 保存并重新打包，输出 `<原名>_zh.pak`，丢进 MOD 管理器即可。

### 大列表也不卡

翻译表格是虚拟滚动的，两万行条目只渲染当前可见的十几行；可以用搜索框和
状态过滤 chips（全部 / 待翻译 / 翻译中 / 已翻译 / 出错）快速定位。
出错的条目可以单独「重试」，也可以一键「重试全部失败条目」。

### 给模型一点语境

如果某个 MOD 有特殊语境，可以在翻译界面里填一条提示，模型会按这个方向处理。

### 占位符和标签：写回前会校验结构

提示词里要求模型原样保留 `{1}` 这类占位符和 `<LSTag ...>` 这类富文本标签，
现在拿到译文后还会**真的校验一遍结构**，不合格不会被当成成功译文：

- **比什么**：比的是原文与译文的**结构签名**——占位符（`{1}` / `{10}` / `{name}`）
  的多重集，加上白名单富文本标签（`<LSTag>` / `</LSTag>` / `<br/>`）的标签名序列
  与开始标签的**属性名**。属性值改了、标签之间的正文被翻译了都不算问题：只比结构，不比内容。
  占位符顺序不算问题（`{1} {2}` ↔ `{2} {1}` 保真）；缺一处 / 多一处 / 重复，
  以及标签的顺序或嵌套与原文不同，都算问题。
- **怎么处理**：第一次拿到结构不合格的译文，会带上具体原因（例如「上一轮译文缺少占位符
  `{1}`，请重新只输出译文……」）自动重试一次。这条纠错重试不占用网络重试的额度，
  网络失败仍然是 500ms → 1000ms → 2000ms 退避、最多 3 次，语义不变。
- **还是不合格**：该条目标记为**出错**，表格里直接显示原因
  （形如 `大模型调用错误: 结构校验未通过：占位符 {1} 缺失（已重试 1 次）`），
  可以单独「重试」，也可以手工改好再保存。
- **写回 PAK 时退回原文**：能不能写回只看这一条——译文非空**且**状态不是「出错」。
  出错条目在写回时退回原文（导出的 MOD 里该字段保持原样，等于这条没翻），
  所以不修也不会把坏译文写进文件；手工改好并保存后状态变成「已编辑」，
  才按你改的内容写回。（取消翻译时残留的半截流式文本，前端会回滚成「待翻译」
  并清空译文，同样退回原文。）
- **被截断的流不算成功**：服务端自报 `finish_reason` 是 `length` / `content_filter`，
  或者**流结束了却既没有 `[DONE]` 也没有 `finish_reason`**，都会被判为失败
  （重试 3 次后条目标成「出错」、打包时退回原文），不会把半句话当成品写进 PAK。
  代价：**完全不用 `[DONE]` 也不发 `finish_reason` 的第三方网关会开始报错** ——
  这类响应与「连接被掐断」在客户端无法区分，宁可失败也不静默丢内容；
  遇到这种报错请换用遵守 OpenAI 流式协议的端点（或点「重试」）。
- **防误报**：`HP < 5`、`a < b`、`<5>` 这类不良好的尖括号按普通文本处理；白名单之外的
  标签名（`<name>`、`<color>`）不算标签；`&lt;` 会先还原成字面 `<` 再比较，
  模型把 `<` 转义回去也不会误判——代价是这种转义写法会被原样写回，
  游戏里显示成字面 `&lt;`（这条防线拦的是结构丢失，不是转义风格）。
  标签的**属性值**是玩家可见文本（`Tooltip` 之类），允许翻译；**属性名**是结构，
  必须逐字保留（写回时模型输出的标签会被原样写进 PAK，原文标签不会被恢复）。
- 系列变体（只差后缀的一组文本，例如 `Silver's Hair` / `Silver's Hair 9b`，
  基础部分只翻一次再拼后缀）是在**合成后的完整译文**上逐成员校验的，
  所以拆到后缀里的占位符同样不会漏。

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
  src/translation/             #   LLM 流式翻译引擎（协议类型借用 types-only 依赖，HTTP/SSE 自研）
  tests/e2e_pak_flow.rs        #   真实 PAK 的端到端闭环测试
src-tauri/                     # Tauri 薄壳：命令层 + 事件桥 + 插件注册
src/                           # React 前端
docs/ARCHITECTURE.md           # 架构说明与冻结的 IPC 契约
docs/VERIFICATION.md           # 第一轮独立验证的完整记录（含已知限制与无法验证项）
docs/VERIFICATION-ROUND2.md    # 第二轮独立验证：对第一轮修复的证伪记录
docs/REVIEW-ROUND3.md          # 第三轮全面审查：结论摘要、修复清单、已知取舍
docs/VERIFICATION-ROUND3.md    # 第三轮独立验证：逐条复核 + 变异测试
docs/review-r3/                # 第三轮四位审计者的原始报告 + 独立红队基线发现
scripts/check_ipc_contract.py  # 跨层契约检查（CI meta job 调用）
scripts/verify.sh              # 一键跑完全部门禁（CI 的 core / web job 也走它）
```

**为什么要拆 crate**：核心逻辑不依赖任何 GUI 系统库，所以在没有
webkit2gtk / gtk / dbus 的机器（以及 ubuntu-latest 的 CI）上都能直接
`cargo test`，几十秒出结果；同时 Tauri 壳保持得很薄，命令层不做业务计算。

### 常用检查

一条命令跑完本地能跑的全部质量门禁（CI 的 core / web job 走的是同一个脚本）：

```bash
bun run verify              # = bash scripts/verify.sh，六道门禁依次执行
```

依次是：跨层 IPC 契约检查 → `cargo fmt --all --check` → `cargo clippy -p bg3-translate-core --all-targets -- -D warnings`
→ `cargo test -p bg3-translate-core --all-targets` → `bun run test` → `bun run build`（含 tsc 类型检查）。
每道门禁都有标题与耗时，任何一道失败立刻非零退出、后面的不再跑。

```bash
bash scripts/verify.sh --core-only   # 只跑 IPC + Rust 核心（跳过前端）
bash scripts/verify.sh --web-only    # 只跑 IPC + 前端（跳过 Rust 核心）
bash scripts/verify.sh --no-ipc      # 跳过 IPC 契约检查（CI meta job 已单独跑）
bash scripts/verify.sh --list        # 只打印门禁清单，不需要任何工具链
```

工具链缺失时会一次性说清楚缺什么（退出码 2）。脚本不写任何受版本控制的文件，
产物只落在 `target/`、`dist/` 这些 gitignore 目录里。

单独的某项检查：

```bash
# 核心逻辑：不需要任何 GUI 系统库
cargo test -p bg3-translate-core
cargo clippy -p bg3-translate-core --all-targets -- -D warnings
cargo fmt --all --check

# 前端
bun run test
bun run build
```

Tauri 壳（`src-tauri`）依赖 webkit2gtk / gtk / dbus：装好这些库的 Linux 与 Windows
可以直接编译，没装的机器（含 ubuntu-latest 的 CI runner）编不了，所以 **不在**
`verify.sh` 里——门禁得在所有开发机上都能过。它的 `cargo check` / `clippy` 固定由
CI 的 Windows `tauri-shell` job 负责；本机装了 GUI 系统库时可以自己跑：

```bash
cargo check -p bg3-translate --all-targets
cargo clippy -p bg3-translate --all-targets -- -D warnings
cargo test -p bg3-translate --lib   # 命令层的路径校验等单元测试
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
后端注册的命令 == `docs/ARCHITECTURE.md` 命令表、**每条命令的参数名**在
「后端形参 / 前端 `invoke` 字段 / 文档命令表」三处一致（命令名对了但参数名写错，
运行期一样报错），`TranslationEvent` / `TranslationStatus` / `PakFileKind` 三组枚举的
serde 名称与前端联合类型一致，以及版本号在 `package.json` / `tauri.conf.json` /
`Cargo.toml` / `Cargo.lock` 四处一致（版本改了却忘了更新 lock，`--locked` 构建会失败）。
装了 GUI 系统库时能真编 `src-tauri`，没装的机器上编不了——这个脚本就是补上的那道防线。

---

## CI / 发布

- `.github/workflows/ci.yml`：4 个并行 job。
  - `meta`：版本号一致（`package.json` / `Cargo.toml` / `tauri.conf.json`，
    并断言两个 crate 都用 `version.workspace = true` 继承，不留第四处版本号；
    `Cargo.lock` 由契约脚本一并核对）、
    跨层 IPC 契约检查（命令名 + 参数名 + 枚举 + 版本号），以及
    「`scripts/verify.sh` 的六道门禁没被删改」的一致性断言
    （`--list` 毫秒级、不需要工具链）。几秒出结果，不需要 cargo / bun。
  - `core` / `web`：直接调用 `scripts/verify.sh --core-only --no-ipc` /
    `--web-only --no-ipc`，所以 CI 和本地跑的是同一串命令，不会各写一份慢慢漂移。
  - `tauri-shell`：Windows 上复用 `web` 的 `dist` 制品做 `cargo check` + `clippy`。
- `.github/workflows/release.yml`：只用 `windows-latest` 构建 NSIS + MSI + 便携版，
  整理成 4 个文件上传到 Actions 制品；打 tag 时同时创建 GitHub Release。
  手动触发时留空 `tag_name` 就只构建、不发布。发布链路上的闸门：
  - 构建前校验标签与 `tauri.conf.json` 版本一致（`v1.0.0` ⇔ 版本 `1.0.0`）；
  - 整理产物时按类型分别断言 `*-Portable.zip` / `*.msi` / `*-Setup.exe` /
    `SHA256SUMS.txt` 各 ≥1（只看文件总数会漏掉「少打了一个安装包」），
    并拒绝 0 字节产物；
  - 发布前查标签指向：不存在则在本次构建的提交上创建，已存在但指向别的提交直接失败；
  - 权限最小化：workflow 级 `contents: read`，只有 `publish` job 拿 `contents: write`。

---

## 备注

`contentuid` 和 `version` 是游戏查表的句柄，改了就会失效，工具不会动它们。
标签与占位符除了在提示词里要求原样保留，拿到译文后还会再做一次结构校验
（见上面「占位符和标签：写回前会校验结构」）：校验不过的条目标成「出错」，
打包时退回原文而不是写回那条译文。不过打包前最好还是抽几条关键文本进游戏里看一眼。

`.lsx` 里只翻译白名单字段（`Description` / `DisplayName` / `Title` / `Tooltip` /
`TooltipDescription`）且类型必须是 `LSString` / `LSWString`。

`Name` **故意不在白名单里**：`Mods/<mod>/meta.lsx` 的 `Name` 是模块内部标识符
（`GustavDev`），类型同样是 `LSString`，翻译它会让 MOD 直接失效。
`TranslatedString` 类型存的是 contentuid 句柄，也不会被翻译。

## 许可

MIT
