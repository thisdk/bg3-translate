# BG3 MOD 汉化工具

[![CI](https://github.com/thisdk/bg3-translate/actions/workflows/ci.yml/badge.svg)](https://github.com/thisdk/bg3-translate/actions/workflows/ci.yml)
[![发布](https://github.com/thisdk/bg3-translate/actions/workflows/release.yml/badge.svg)](https://github.com/thisdk/bg3-translate/actions/workflows/release.yml)
![平台](https://img.shields.io/badge/平台-Windows%20x64-0078d4)
![许可](https://img.shields.io/badge/许可-MIT-green)

给《博德之门 3》MOD 用的桌面汉化工具：**打开 MOD → 用大模型流式翻译 → 重新打包成游戏能加载的 `.pak`**。

它不是一个「把文本丢给大模型再原样贴回去」的脚本。翻译这类文本真正会坏的地方在于
**结构**——`{1}`/`[2]` 占位符、`<LSTag Tooltip="…">` 富文本标签、`Type="Spell"` 这类查表 key
一旦被模型改写或漏掉，轻则该条译文显示异常，重则整个本地化文件变成坏 XML、游戏直接读不了。
所以这个工具在**写回之前会真的把原文与译文的结构比一遍**，不合格的条目标成「出错」，
打包时退回原文（等于这条没翻），而不是把半成品写进 MOD。

## 特性

- **三种本地化文件**：`contentList` XML（`Localization/*/*.xml`）、`.loca` 二进制、
  `.lsx` 元数据里白名单字段（`Description` / `DisplayName` / `Title` / `Tooltip` /
  `TooltipDescription`，且类型必须是 `LSString` / `LSWString`）。
- **直接吃 Nexus 上的包**：`.pak` 直接打开；`.zip` 会先解开、自动取里面最大的 `.pak`
  （Nexus 包里通常还混着 README 和图片）。
- **流式翻译**：边生成边显示，2 万条条目也不卡（虚拟滚动 + 按帧批处理 delta）。
- **结构保真校验**：占位符、标签、属性名、key 型属性值、实体转义、流式截断，
  逐条比对；不合格自动带原因重试一次，仍不合格就标错并退回原文。
- **术语表**：内置 102 条 BG3 官方核心译名（职业 / 种族 / 地名 / 角色 / 法术 / 机制），
  可导入从游戏提取的完整官方术语表（2 万条级别），命中检测在构造时预处理，逐条翻译即时。
- **可人工校对**：表格里直接改、单条重试、一键重试全部失败条目；改动即时生效，
  写回只认「你确认过的译文」。
- **便携**：解压即用，配置与术语表放 exe 同级 `config/`，随包带走、方便备份。

## 下载

到 [Releases](../../releases) 下载最新版：

| 文件 | 说明 |
| --- | --- |
| `*-Portable.zip` | **推荐**。解压后直接运行 `bg3-translate.exe`，配置与术语表放在 exe 同级 `config/`，随包携带、方便备份 |
| `*-Setup.exe` | NSIS 安装程序（简体中文界面，可选「仅当前用户」或「所有用户」） |
| `*-Installer.msi` | MSI 安装包，适合企业批量分发 |
| `SHA256SUMS.txt` | 各产物的 SHA-256 校验和，下载后建议核对 |

只提供 **Windows x64**：这个工具的使用场景就是「在 Windows 上翻译 MOD，然后在 Windows 上玩游戏」。
需要 WebView2 运行时（Windows 11 自带；Windows 10 上安装程序会联网自动下载安装）。

## 快速上手

1. 打开软件，选择 `.pak` 或 `.zip`（也可以直接把文件拖进窗口）。
2. 打开「设置」，填大模型接口（OpenAI 兼容协议，默认对接 DeepSeek）：
   `https://api.deepseek.com`、`https://api.deepseek.com/v1` 或完整的
   `…/v1/chat/completions` 都可以，三种写法会落到同一个端点。
   默认模型 `deepseek-chat`，并发 6，温度 0.3。
3. 在左侧文件树里勾选要翻译的文件，点「开始翻译」。文本边生成边显示，
   可以随时搜索、按状态过滤（全部 / 待翻译 / 翻译中 / 已翻译 / 出错）。
4. 翻完后检查几条重点文本，不满意的可以直接在表格里改（也可单条重试）。
5. 点「保存并打包」，输出默认叫 `<原名>_zh.pak`，丢进 MOD 管理器即可。

> 第一次用建议拿一个小 MOD 试手：先把整条链路走通（解包 → 翻译 → 打包 → 进游戏看一眼），
> 再上大工程。MOD 里没被翻译的文件（Lua 脚本、其它资源）是**逐字节原样复制**的。

## 翻译质量：写回前会真的校验结构

### 校验什么

比的是原文与译文的**结构签名**，不是内容：

- **占位符的多重集**，两套写法各自独立比对（`{1}` 不能顶 `[1]`）：
  - 花括号：`{1}` / `{10}` / `{name}` / `{user_name}`
  - 方括号：`[1]` / `[10]`（游戏替换数值）、`[IE_PanelSelect]` / `[DRUID]`（内部 ID）。
    真实官方术语表里 163 条含 `[数字]` 的条目，简中 163/163 全部原样保留，
    其中 `, [1] from [2]` → `，从[2]处取走了[1]` 还交换了位置 —— 所以顺序随意、数量必须相等，
    缺一处 / 多一处 / 重复都算不合格。
- **富文本标签**：白名单只有 `<LSTag>` / `</LSTag>` / `<br/>`。比标签名的多重集与
  开始标签的**属性名**；标签之间的正文随便翻。
- **属性值里查表用的 key 必须逐字照抄**：`Tooltip="VENOMOUS_BARBS_CONDITION"` 是内部 ID，
  翻它等于把 key 改掉，游戏查不到表。`Type="Spell"` 这类标识符形态的 key 值被翻译会报错；
  非 key 型属性（`Tag`、`color`…）与含空格的自然语言值不受限。
- **标签顺序不算问题**：中英语序不同，标签跟着它包住的那段正文换位是常态。
  要查的是**开闭不配对**（落单的闭合标签、不同标签名交叉嵌套）。
- **占位符不许粘连**：原文分开的两个占位符被写成 `[1][2]` 会报错（游戏填完值会渲染成一个数
  `12`）；给它们之间**加**分隔（`[1] [2]`、`[1]、[2]`）永远放行。
- **占位符周围的空格不参与比对**：`deal [2] damage` 译成「造成[2]点伤害」是正确的。

### 会自动修回来，而不是判失败

- **全角括号 → 半角**：模型按中文排版写成 `【2】` 时修成 `[2]`；
  译文里当标点用的 `【注意事项】` 不会误伤。
- **括号类型被改写**：模型把少见的 `[1]`「归一化」成 `{1}` 时按原文修回 `[1]` ——
  这两套是游戏的两套替换机制。只在原文只用了一种括号时才修，原文两种都用时不猜，交给校验报错。

这些修复发生在校验**之前**，修好的形态既进界面也进 PAK。

### 不合格会怎样

1. 第一次拿到结构不合格的译文 → **带上具体原因**（例如「上一轮译文缺少占位符 `{1}`」）
   自动重试一次。这条纠错重试不占用网络重试的额度。
2. 仍不合格 → 该条目标记为**出错**，表格里显示原因，可以单条「重试」，也可以手工改好再保存。
   点「重试」不是原样再问一遍：失败原因会附在原文前面一起发给模型。
3. 写回 PAK 时只看一条规则：**译文非空且状态不是「出错」**。出错条目退回原文
   （该字段保持 MOD 原样），所以放着不管也不会把坏译文写进文件。

### 被截断的流不算成功

服务端自报 `finish_reason` 是 `length` / `content_filter`，或者**流结束了却既没有 `[DONE]`
也没有 `finish_reason`**，都判为失败（重试 3 次后标错、打包退回原文），不会把半句话当成品。
代价：**完全不用 `[DONE]` 也不发 `finish_reason` 的第三方网关会开始报错** ——
这类响应与「连接被掐断」在客户端无法区分，宁可失败也不静默丢内容。遇到就换一个遵守
OpenAI 流式协议的端点。

### 防误报

`HP < 5`、`a < b`、`<5>` 这类不良好的尖括号按普通文本处理；白名单之外的标签名
（`<name>`、`<color>`）不算标签；混合大小写的方括号散文（`[Note]`、`[Draft]`）不算占位符；
`&lt;` 会先还原成字面 `<` 再比较。模型输出转义形态（`&lt;LSTag&gt;`）时由写回层兜底：
先做实体还原再统一转义，落盘仍是游戏能读回的形态。

### 相同原文与系列变体

- 原文相同的条目只翻一次，译文在它们之间复用（省时间与额度）。
- 只差后缀的一组文本（`Silver's Hair` / `Silver's Hair 9b`）按系列处理：基础部分翻一次再拼后缀，
  结构校验在**合成后的完整译文**上逐成员做，拆到后缀里的占位符同样不会漏。

### 术语表

内置 102 条官方核心译名，开箱即用；也可以导入从游戏里提取的完整官方术语表（2 万条级别）。
导入时会自动过滤噪音条目（占位符模板、UI 内部标记、破折号碎片、`of`/`the` 这类会误匹配的短词），
官方条目不可删除（只能改译文或停用）。命中检测在构造时一次性预处理，
2 万条术语表下逐条翻译依然是即时的。

## 配置放在哪里

按下面的顺序决定，日志里会写明实际用了哪个：

1. 环境变量 `BG3_TRANSLATE_HOME`（相对路径按进程当前目录解析，建议写绝对路径）
2. **便携模式**：exe 同级目录的 `config/`（能写就用它）
3. **系统模式**：`%APPDATA%\bg3-translate\config`（Windows）、
   `~/.config/bg3-translate`（Linux）、`~/Library/Application Support/bg3-translate`（macOS）

第 3 步是必要的：装到 `C:\Program Files` 时 exe 同级目录通常不可写。
配置文件是 `settings.json`（大模型连接信息）与 `glossary.json`（术语表），
写入采用「先写临时文件再改名」的原子方式，断电不会留下半个损坏文件。

**文件损坏时不会静默丢数据**：JSON 语法坏了、被编辑器存成了非 UTF-8、读不出来，
原文件一律被改名成 `<文件名>.corrupt`（已存在则 `.corrupt.2`…）留在原位旁边，
应用回退到默认设置 / 官方种子术语，之后你还可以手工把 API Key 或术语捞回来。
「设置」面板里的「配置目录」就是实际生效的那个路径。

> API Key 以明文存在 `settings.json` 里（便携模式下就在 exe 旁边）。
> 它只会随请求发给你自己填的那个端点；请不要把 `config/` 目录连同 key 一起分享出去。

## 常见问题

**Q：表格里出现「结构校验未通过」怎么办？**
说明模型这一条的占位符/标签结构不对。点该条的「重试」——重试会把失败原因一起带给模型；
也可以手工把结构补回去再保存。放着不管也不会污染产物：打包时这条退回原文。

**Q：报错「流结束但既没有 [DONE] 也没有 finish_reason」？**
你的网关没有遵守 OpenAI 流式协议。换端点（或换网关），或在设置里把 base URL 指向别处。

**Q：429 / 超时 / 连接中断？**
网络类错误会自动退避重试（500ms → 1000ms → 2000ms，最多 3 次）。频繁 429 就把「并发」调小
（设置里 1–64），或换更宽松的端点。

**Q：翻译很慢？**
速度取决于端点。工具侧已经做了：并发请求、同原文复用、系列变体只翻基础部分、
增量只在帧边界刷新界面。条目多的时候界面也不卡。

**Q：打出来的 pak 在游戏里没中文？**
依次检查：①MOD 管理器里排序/覆盖关系（本工具的输出是**新 MOD**，要让它的本地化文件生效）；
②游戏语言是简体中文；③你翻译的文件确实是这个 MOD 用到的那个（有些 MOD 只用了 `.lsx` 描述字段，
正文在别的 MOD 里）。也可以先用一个小 MOD 验证整条链路。

**Q：写回/打包失败，提示文件被占用？**
先关掉游戏和 MOD 管理器（BG3 会锁住 pak），检查杀毒软件是否在扫这个目录。
写回与打包都是「先写临时文件再原子替换」，失败时不会把原文件截断成 0 字节。

**Q：怎么备份 / 换电脑？**
用便携版：整个目录拷走即可（`config/` 里就是设置与术语表）。

**Q：能翻别的游戏吗？**
只对 BG3 的本地化格式做过适配与验证。同一系列/同引擎的游戏格式相近，可能能用，
但没有测试覆盖，出问题请先备份 MOD。

## 已知限制

诚实清单（都是明确的取舍，不是待修的 bug）：

| 限制 | 说明 |
| --- | --- |
| contentList 写回会规范化格式 | 写回是按「解析成条目再重建」做的：注释、DOCTYPE、`<content>` 上的额外属性、元素之间的换行与缩进都不会保留（整份文件会写成一行，BG3 读得动，但你想手工看格式就没了）；文本首尾空白会被归一化。真实语料 1971 条里 1945 条（98.68%）`<content>` 逐字节往返，其余 26 条全是尾空格归一化 |
| 转义文本的「层数」 | 源文件里真想显示 `&lt;` 的文本（`&amp;lt;`）每保存一次会降一层；「真元素风格 + 整条标签被转义 + 属性里带 `&quot;`」三条件同时成立时，标签会降级成字面文本（产物仍合法、不丢内容） |
| 「还原」还原的是工作目录里的底稿 | 第二次打包时底稿 = 上一次写回的结果，所以「还原」会把条目还原成工作目录里的内容，而不是 MOD 的原始字节。要彻底重来请重新解包一次 |
| `.lsx` 写回是 O(n·k) | 4 万字段约 5 秒（release），写回不在 UI 线程，界面不会卡 |
| 取消翻译 | 未完成的条目回滚为「待翻译」并清空半截流式文本；已保存的人工译文不受影响 |
| 只翻译白名单内容 | Lua 脚本、其它资源原样复制（逐字节保留）；`.lsy`/`.txt` 等不翻译 |
| 勾选了但一条都没翻的文件也会被写回 | 因为写回是「解析 → 重建」的，这类文件的文本不变但格式会被规范化。想完全避免就只在文件树里勾选你真正要翻的文件 |
| `meta.lsx` 的 `Name` 不翻 | 它是模块内部标识符（`GustavDev`），类型同样是 `LSString`，翻它 MOD 直接失效 |
| 平台 | 只发布 Windows x64 产物 |

## 版本历史

### v1.5.0（当前）

发布前做了一轮**五阶段全面审查**：四位审计者分别负责归档与格式层、翻译引擎与协议层、
Tauri 壳层/脚本/CI 与发布链路、前端状态机，一位**独立验证者**全程只读仓库做对抗式发现，
再用变异实验逐条证伪（把修复撤回去，测试必须真变红）。

共修掉 **24 条缺陷（含 4 条高危）**，都是会造成真实损失的：会把用户的 MOD 打包坏、
把已有译文静默覆盖、把半截译文写进产物、把「失败」判成「成功」、或让防线可以悄悄失效。
高危 4 条：

1. **PAK 解包没有任何上限**（zip 那条路早就有了）：一个 257 KB 的恶意/损坏 PAK 能写出
   64 MiB 并整份进内存，可撑爆磁盘。现在按「声明大小预检 + 实际解包字节兜底」两层封顶。
2. **SSE 流开头的 UTF-8 BOM 让整条首帧被丢弃**：少一截的译文配上 `[DONE]` 会被判成功，
   静默写进 PAK。
3. **标准结构 chunk 里的 `{"error":…}` 被当心跳吞掉**：服务端明说「这条流没做完」被忽略，
   半截译文照样判成功。
4. **打包页点「返回继续编辑」会把已翻译/人工编辑的条目整体抹掉**，再点一次打包就把英文
   原文写回 `Localization/Chinese/…`，覆盖掉上一次写好的中文（不可逆）。这是正常回路上的
   数据损失，也是本轮最有价值的一条。

其余 20 条中低危里，成体系的几类（完整清单见发布说明）：

- **静默丢数据**：写回失败残留的临时文件会被打进用户产物；纯不可见字符（BOM/零宽空格）
  的「译文」会把原文覆盖成空白；第二次打包的底稿取「上次写回的结果」导致「还原」失效；
  一致性记忆恒为空导致跨文件专名/系列复用从未生效。
- **静默失效的防线**：契约脚本漏解析嵌套泛型 `invoke<Record<string, T[]>>`、命令表章节
  边界只认 `###`（迁移到 README 后可被静默绕过）；`verify.sh --list` 与模式标志顺序相关，
  `--list --core-only` 会真的去执行门禁；Channel 推送失败在 release 日志里完全无声；
  三条新回归测试经变异实验证明「拦不住自己防的那个回归」，已补齐为真哨兵。
- **误判与误合并**：属性语法错误被吞导致 `contentuid` 被改写成空串；没有 `contentuid` 的
  元素被凭空补空句柄并在合并中互相覆盖；PAK 目录条目让整个 MOD 报 `Not a directory`；
  系列别名「后缀重叠」误把无关的中文系列并进英文系列；`…save?` 的译文被贴到 `…save!`。
- **发布链路**：`tag push` 不触发 CI，发布链现在自己兜一道门禁（`fmt` / `clippy` /
  核心测试 + Windows 上真跑命令层单测）后再构建。

同时：`README` 重写（命令表从架构文档迁入，成为契约脚本的唯一事实来源）、新增 `LICENSE`、
用 [OSV](https://osv.dev) 把 753 个锁定依赖全量扫了一遍（结论与逐条裁定见上面「依赖与已知告警」）、
并把 AI 编码期间产生的过程性文档全部清理出仓库。

### v1.2.1

修掉「普通数字被写成占位符」的提示词诱导，并让重试带上上一次的失败原因。

### 更早

见 [Releases](../../releases)。

## 开发

技术栈：**Tauri 2 + Rust 2024 + React 19 + TypeScript 7 + Tailwind CSS 4 + Vite 8 + zustand 5**。

环境要求：Bun（或 Node 20+）、Rust 1.85+、Tauri 对应的系统依赖
（Windows 上装 [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) 与 MSVC 构建工具）。

```bash
bun install

# 前端
bun run dev            # 只跑前端（vite，端口 1420）
bun run test           # vitest
bun run build          # tsc -b 类型检查 + vite 构建

# 桌面应用
bun tauri dev
bun tauri build

# 一键质量门禁（CI 的 core / web job 走的也是它）
bun run verify         # = bash scripts/verify.sh
```

### 工程结构

```
Cargo.toml                     # Cargo workspace：统一版本、依赖、release profile
crates/bg3-translate-core/     # 纯逻辑核心，零 GUI 依赖
  src/error.rs                 #   统一错误类型（可序列化给前端）
  src/types.rs                 #   跨 IPC 的数据结构（字段名即 TS 字段名）
  src/config.rs                #   数据目录解析 + 设置持久化（原子写、损坏备份）
  src/pak.rs                   #   PAK / ZIP 解包与重新打包（含 zip-slip / 解压炸弹防线）
  src/formats/                 #   contentList XML / LSX / LOCA 三种格式
  src/glossary/                #   术语表数据、清洗、命中匹配
  src/translation/             #   LLM 流式翻译引擎（协议类型借用 types-only 依赖，HTTP/SSE 自研）
    sse.rs                     #     SSE 帧解析（跨 chunk / CRLF / 多行 data / [DONE]）
    fidelity.rs                #     译文结构保真校验（纯函数）
    retry.rs                   #     网络退避重试 + 结构纠错重试
  tests/e2e_pak_flow.rs        #   合成 PAK 的端到端闭环
  tests/real_mod_sample.rs     #   真实 Nexus MOD 样本闭环（含 Lua 逐字节未改动断言）
  tests/corpus_regression.rs   #   真实语料 1971 条 → 9088 个防误报 / 防漏报用例
  tests/corpus_writeback.rs    #   同一份语料的写回字节往返与标记编码风格回归
src-tauri/                     # Tauri 薄壳：命令层 + 事件桥 + 插件注册
src/                           # React 前端（store / 组件 / 纯函数 lib）
samples/english.xml            # 真实 BG3 contentList（1971 条），语料测试的只读输入
samples/bg3-official-glossary.json      # 从游戏提取的官方术语表（约 2 万条），术语表清洗/匹配测试的只读输入
samples/Appearance Edit Enhanced-*.zip  # 真实 Nexus MOD 样本，端到端测试的只读输入
scripts/check_ipc_contract.py  # 跨层 IPC 契约检查（CI meta job 调用）
scripts/verify.sh              # 一键跑完全部门禁（CI core / web job 也走它）
```

**为什么拆 crate**：核心逻辑不依赖任何 GUI 系统库，所以在没有 webkit2gtk / gtk / dbus 的机器
（以及 ubuntu-latest 的 CI runner）上都能直接 `cargo test`，几十秒出结果；
Tauri 壳保持很薄——命令层只做参数归一化与状态管理，不做业务计算。

### 质量门禁

```bash
bun run verify                        # 六道门禁依次跑，任一失败立刻非零退出
bash scripts/verify.sh --core-only    # 只跑 IPC + Rust 核心（跳过前端）
bash scripts/verify.sh --web-only     # 只跑 IPC + 前端（跳过 Rust 核心）
bash scripts/verify.sh --no-ipc       # 跳过 IPC 契约检查（CI meta job 已单独跑）
bash scripts/verify.sh --list         # 只打印门禁清单，不需要任何工具链
```

依次是：跨层 IPC 契约检查 → `cargo fmt --all --check` →
`cargo clippy -p bg3-translate-core --all-targets -- -D warnings` →
`cargo test -p bg3-translate-core --all-targets` → `bun run test` → `bun run build`。
每道门禁都有标题与耗时；工具链缺失时会一次性说清楚缺什么（退出码 2）。
脚本不写任何受版本控制的文件，产物只落在 `target/`、`dist/` 这些 gitignore 目录里。

Tauri 壳（`src-tauri`）依赖 webkit2gtk / gtk / dbus：装好这些库的 Linux 与 Windows 可以直接编译，
没装的机器（含 ubuntu-latest 的 CI runner）编不了，所以**不在** `verify.sh` 里——门禁得在所有
开发机上都能过。它的 `cargo check` / `clippy` / 命令层单测固定由 CI 的 Windows `tauri-shell`
job 负责；本机装了 GUI 系统库时可以自己跑：

```bash
cargo check -p bg3-translate --all-targets
cargo clippy -p bg3-translate --all-targets -- -D warnings
cargo test -p bg3-translate --lib     # 命令层的路径校验与状态机单测
```

### 测试体系

四层，都真的跑：

- **单元测试**：解析、校验、重试、匹配等纯逻辑（Rust 与前端各一套）。
- **端到端闭环**（`tests/e2e_pak_flow.rs`）：造一个 PAK 跑完整链路
  `造 MOD 目录树 → 打包 → 解包 → 识别类型 → 读条目 → 翻译 → 写回 → 重新打包 → 再解包 →
  校验 contentuid / 标签 / 占位符 / 未翻译内容`。
- **真实样本闭环**（`tests/real_mod_sample.rs`）：拿仓库里那个真实 Nexus MOD 跑同样一遍，
  并断言未翻译的 Lua 脚本逐字节没被动过。合成样本只能验证「我以为格式是这样的」，
  真实样本才能验证「格式实际就是这样」——`meta.lsx` 的 `Name` 不能翻就是真实样本测出来的。
- **真实语料回归**（`samples/english.xml`，1971 条真实 contentList）：
  结构校验的 9088 个用例，A 组零误报 6504（自反、标记逐字照抄换正文、尖括号重新转义、
  288 条标签整体换位、占位符空格增删），B 组零漏报 2584（丢标签对、属性名改坏、
  占位符丢/改号/凭空加、`[N]`→`{N}`、相邻粘连、丢 `<br>`、开闭不配对）。
  朴素标记扫描器**自己实现**，不复用被测代码，两边独立才谈得上互相验证。
  `corpus_writeback.rs` 另外钉住写回字节往返与标记编码风格不被翻转。

语料是**只读输入**，缺失时测试**响亮失败**而不是静默跳过；它和真实 MOD 样本一样随仓库提交。

### 跨层 IPC 契约

`src-tauri` 在没装 GUI 系统库的机器上编不了，于是「命令名写错」「参数名对不上」
「命令表腐烂了」这类问题原本只会在 Windows 构建甚至运行时才暴露。
`scripts/check_ipc_contract.py` 只用 Python 标准库，几秒出结果：

```bash
python3 scripts/check_ipc_contract.py
```

它核对：前端 `invoke` 的命令 ⊆ 后端注册的命令、后端注册的命令 == 下面「冻结的 IPC 契约」
里的命令表、**每条命令的参数名**在「后端形参 / 前端 `invoke` 字段 / 命令表」三处一致、
写了 `#[tauri::command]` 的函数都真的进了 `generate_handler!`（漏注册的命令前端一调就报错，
只从注册表出发的检查看不见它）、命令的**返回类型**与命令表一致、
`TranslationEvent` / `TranslationStatus` / `PakFileKind` 三组枚举的 serde 名称与前端联合类型一致、
结构体字段与 `src/lib/types.ts` 一致，以及版本号在 `package.json` / `tauri.conf.json` /
`Cargo.toml` / `Cargo.lock` 四处一致。

改协议时**命令表与 `src/lib/types.ts` 必须一起改**，否则前端会静默收不到数据。

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

### 事件

`TranslationEvent`（serde `tag = "type"`，经 Tauri Channel 推给前端）：

```jsonc
{"type":"progress","entryId":"...","status":"translating"}
{"type":"delta","entryId":"...","text":"..."}
{"type":"done","entryId":"...","text":"..."}
{"type":"error","entryId":"...","message":"..."}
{"type":"all_done","total":100,"failed":2}
```

顺序：`progress` → `delta`* → `done`（失败则 `error`，最后一定有一条 `all_done`）。
**每次尝试开始时都会重发一次 `progress`**（网络退避重试、结构纠错重试各算一次新尝试），
前端据此把该条目的流式文本清零、重新累积，否则两轮 delta 会拼成「坏译文 + 好译文」。
取消生效后不再发任何事件。

### LlmSettings

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

### 依赖与已知告警

发布前用 [OSV](https://osv.dev) 把 `Cargo.lock`（523 个包）与 `bun.lock`（230 个包）全量比对过一遍：
前端依赖 0 告警；Rust 侧的告警全部落在 Tauri 栈的传递依赖上，逐条评估如下（都不在我们的输入面上）：

| 包 | 告警 | 位置与裁定 |
| --- | --- | --- |
| `time 0.3.45` | RUSTSEC-2026-0009：RFC 2822 日期解析栈耗尽 | 唯一进入**发行产物**的一条（`cookie ← tauri`）。修复版 `0.3.47` 要求 Rust 1.88，高于本仓库 MSRV 1.85；触发条件是「解析攻击者给的 RFC 2822 日期」，而本应用的 WebView 只加载本地打包资源（CSP 仅 `self`），没有这种输入。接受并记录 |
| `quick-xml 0.38.4` | RUSTSEC-2026-0194 / -0195：属性重名二次方耗时、命名空间无界分配 | 经 `plist ← tauri-utils ← tauri-build`，**只在构建期**；核心库自己用的是 `quick-xml 0.42`，不受影响 |
| `serde_with 3.17.0` | GHSA-7gcf-g7xr-8hxj：`KeyValueMap` 空条目 panic | 构建期依赖（`tauri-utils`），且本项目没有用到 `KeyValueMap` |
| `glib 0.18.5`、`proc-macro-error`、`unic-*` | 不健全 / 无人维护 | `glib` 只出现在 Linux GTK 路径（`--target x86_64-pc-windows-msvc` 下不参与编译），另两个是构建期传递依赖 |

### CI 与发布

- `.github/workflows/ci.yml`：4 个并行 job。
  - `meta`：版本号一致（`package.json` / `Cargo.toml` / `tauri.conf.json`，并断言两个 crate
    都用 `version.workspace = true` 继承，不留第四处版本号；`Cargo.lock` 由契约脚本一并核对）、
    跨层 IPC 契约检查，以及「`scripts/verify.sh` 的门禁没被删改」的一致性断言：默认清单逐字符
    比对，`--core-only --no-ipc` 与 `--web-only --no-ipc` 两个**模式**的清单也各自断言
    （只钉默认清单的话，给某道门禁加个 `if` 就能让某个 CI job 静默少跑一步）。
    几秒出结果，不需要 cargo / bun。
  - `core` / `web`：直接调用 `scripts/verify.sh --core-only --no-ipc` /
    `--web-only --no-ipc`，CI 与本地跑的是同一串命令，不会各写一份慢慢漂移。
  - `tauri-shell`：Windows 上复用 `web` 的 `dist` 制品做 `cargo check` + `clippy`，
    并真正执行命令层单测（`cargo test -p bg3-translate --lib`）。
- `.github/workflows/release.yml`：只用 `windows-latest` 构建 NSIS + MSI + 便携版，
  整理成 4 类文件上传到 Actions 制品；打 `v*` 标签时同时创建 GitHub Release。
  手动触发时留空 `tag_name` 就只构建、不发布。发布链路上的闸门：

  - 构建前校验标签与 `tauri.conf.json` 版本一致（`v1.5.0` ⇔ 版本 `1.5.0`）；
  - 整理产物时按类型分别断言 `*-Portable.zip` / `*.msi` / `*-Setup.exe` / `SHA256SUMS.txt`
    各 ≥1，并拒绝 0 字节产物；
  - 发布前查标签指向：不存在则在本次构建的提交上创建，已存在但指向别的提交直接失败；
  - 权限最小化：workflow 级 `contents: read`，只有 `publish` job 拿 `contents: write`。

- **版本号规则**：`Cargo.toml` 的 `[workspace.package]`、`package.json`、
  `src-tauri/tauri.conf.json` 三处必须一致（`Cargo.lock` 里的 crate 版本由 cargo 自动同步），
  发布标签为 `v` + 版本号。

## 许可

MIT，全文见 [LICENSE](LICENSE)。
