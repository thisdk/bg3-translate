# 验证报告（独立验证 / teammate `verifier`）

> 这是**证伪记录**，不是 writer 自述的复述。每条结论后面都跟着我**实际执行过的命令**与**关键输出片段**。
> 没跑过的项目一律标「无法验证」，绝不写「通过」。

**最终结论对应的 revision：`c961bdb2e64f3f02e740e6c79c80b2dd78224798`（dev，== origin/dev）。**
`git diff --stat 2cfa6de..c961bdb` 只有 `README.md | 27 ++++--`（+22/-5），
所有会进入构建的源码在 `2cfa6de` 与 `c961bdb` 之间**逐字节相同**，因此 2cfa6de 上跑出的
Rust/前端证据对 `c961bdb` 同样成立。

验证过程中 revision 一直在前进（`fa0ca0a` → `7f40448` → `2cfa6de` → `c961bdb`）。
本报告以 `c961bdb` 为准；凡是被后续提交改变的结论都在第 7.2 节标注「已修复」并给出复验证据。

---

## 0. 结论摘要

### 0.1 总数

| 判定 | 项数 |
| --- | --- |
| 检查项总数（下表 41 行） | **41** |
| **通过** | **39** |
| **不通过** | **2**（展开为 9 条缺陷，全部中/低危，**无阻断级、无高危**） |
| 额外：我提过、writer 已改、我已复验 | **6**（见 7.2） |
| **无法验证**（附原因与替代证据） | **3**（见第 8 节） |

### 0.2 逐项结论

| # | 验证项 | 结论 |
| --- | --- | --- |
| 1 | `cargo fmt --all --check` | **通过** |
| 2 | `cargo clippy -p bg3-translate-core --all-targets -- -D warnings` | **通过** |
| 3 | `cargo clippy --workspace --all-targets -- -D warnings`（**含 src-tauri**） | **通过** |
| 4 | `cargo test -p bg3-translate-core --all-targets` | **通过**（203 单测 + 5 e2e + 4 真实样本 = 212） |
| 5 | `src-tauri` 本地**编译并链接**出真实二进制 | **通过**（314 MB debug 二进制） |
| 6 | capabilities 权限标识符合法性 | **通过**（11/11 对生成 schema 校验） |
| 7 | `bun install --frozen-lockfile` / `test` / `typecheck` / `build` | **通过**（92 用例） |
| 8 | 依赖是否最新稳定版（25 个包逐个查 registry） | **通过**（落后 0 个） |
| 9 | IPC 命令表三处集合一致（注册/前端/文档） | **通过**（17 == 17 == 17） |
| 10 | `TranslationEvent` 变体与字段名 | **通过**（逐字段核对） |
| 11 | 结构体 serde 名 ↔ TS 接口字段 | **通过**（9 个结构体） |
| 12 | 命令参数名 camelCase ↔ snake_case 映射 | **通过**（用真实 heck rlib 实测 `_work_dir`→`workDir`） |
| 13 | `docs/ARCHITECTURE.md` 承诺的 API 逐字一致 | **通过**（32/32 项） |
| 14 | 新增的 `scripts/check_ipc_contract.py` 是否真的能抓错 | **通过**（4 个变异全部被抓到，exit 1） |
| 15 | workflow YAML 合法性 | **通过** |
| 16 | workflow 引用的 action 版本真实存在 | **通过**（7/7） |
| 17 | release.yml 产物路径（workspace target / exe 名 / msi / nsis） | **通过（GHA 实测产出 4 个文件）** |
| 18 | `if ($count -lt 3)` 是否会在缺 MSI 时误报 | **通过**（GHA 实测 count=4 通过；缺 MSI 时按算术为 3 也不会误报） |
| 19 | `RELEASE_TAG` 三种触发下是否误建 Release | **通过（GHA 实测：dispatch 不填 tag → publish job 被跳过，`gh release list` 为空）** |
| 20 | CI artifact 名称/路径衔接 | **通过** |
| 21 | Windows TLS 后端是否与注释一致 | **通过**（`cargo tree --target` 实证 schannel / 无 aws-lc） |
| 22 | GHA 实测：meta / core / web job | **通过**（7s / 26s / 16s） |
| 23 | GHA 实测：Windows `tauri-shell` 的两个门禁步骤 | **通过**（`cargo check` ✓ / `clippy -D warnings` ✓） |
| 24 | 15 项既有功能是否保留 | **通过**（代码路径 + E2E 实证） |
| 25 | 故意删除的东西是否删干净 | **通过** |
| 26 | LSX 读写是否共用同一套索引（混空值/非文本/TranslatedString） | **通过**（真实执行，写回位置正确） |
| 27 | contentList 实体/CDATA/嵌套/BOM/根属性 | **通过** |
| 28 | contentList 畸形文档是否还会静默截断 | **通过**（已改为返回错误） |
| 29 | `safe_output_path` 挡 `..\`/绝对路径/盘符 | **通过**（13 个向量实测） |
| 30 | `pick_largest_pak` 确定性（含同体积 tie-break） | **通过**（各 20 次调用一致） |
| 31 | `config` 便携目录策略（env/可写/不可写） | **通过**（纯函数 + 运行时 `data_dir()` 双验证） |
| 32 | 前端虚拟滚动 count 是否跟随搜索/过滤 | **通过**（有断言 + 用例实跑） |
| 33 | 行内 textarea 滚动后是否丢内容 | **通过**（draft 在父组件 state） |
| 34 | 生产代码 `unwrap()`/`expect()` 是否会 panic | **通过**（非测试代码只有 4 处，全部安全） |
| 35 | `todo!()` / `unimplemented!()` / `#[allow(dead_code)]` | **通过**（全仓 0 处） |
| 36 | 术语表导入 2 万条样本 | **通过**（20253 → 19991，幂等） |
| 37 | `git status` / `.gitignore` 卫生 | **通过** |
| 38 | 测试质量抽查（≥5 个是否真断言） | **通过**（6 个逐个读过，均有效） |
| 39 | 翻译引擎死锁 / 任务泄漏 / 取消后仍发请求 | **通过**（有 hang 检测用例 + 代码逐点审查） |
| 40 | README 与实现是否一致 | **不通过** → D-14 / D-13 |
| 41 | 剩余死代码（未使用依赖/类型/状态） | **不通过** → D-01 / D-05 / D-10 |

---

## 1. 环境与证据基线

```
$ pwd && git branch --show-current && git rev-parse HEAD
/home/jason/bg3-translate
dev
c961bdb2e64f3f02e740e6c79c80b2dd78224798

$ git rev-parse origin/dev
c961bdb2e64f3f02e740e6c79c80b2dd78224798          # 与本地 HEAD 一致

$ bun --version; cargo --version; rustc --version
1.3.13
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
rustc 1.95.0 (59807616e 2026-04-14) (built from a source tarball)

$ git status --porcelain
?? VERIFICATION.md
```

OS：Linux。**本机原本缺 webkit2gtk/dbus**，但通过 `nix-shell` 提供了 GUI 系统库，
因此 `src-tauri` **在本机完成了真实编译与链接**（第 2.4 节）——「无法本地验证 src-tauri」这一项已被消除。

验证期间我的写范围：**只写 `VERIFICATION.md`**；临时对抗性工程在 `/tmp/bg3-adv`、
变异测试沙箱在 `/tmp/ipcmut`、lint 探针在 `/tmp/deadcode-probe`。没有修改任何源码/配置/workflow。

### 关键文件指纹（c961bdb）

```
9d6d68bc6967df46188f9829e02365ce  README.md
1a0b9de2ea0b409af7163effa9fe2ecf  crates/bg3-translate-core/src/formats/lsx.rs
8f522894caaed7dc5720ac509256b157  crates/bg3-translate-core/src/formats/content_list.rs
8f4eb0411d5c43783307560eb732c5b4  crates/bg3-translate-core/src/glossary/seed.rs
034e977a55bece7c379f6589cf09f682  src-tauri/src/lib.rs
7b63c5da9363d27e03fdcea00d8f5b48  scripts/check_ipc_contract.py
42e5ae71cafd1fb80e1b89a8f1693e85  .github/workflows/ci.yml
0dff051e185fe3b280e598dd293cee28  src/lib/types.ts
```

---

## 2. 构建与测试（命令 + 真实输出）

### 2.1 Rust

```
$ cargo fmt --all --check
FMT_EXIT=0

$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Checking bg3-translate-core v0.2.0 (/home/jason/bg3-translate/crates/bg3-translate-core)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.77s
CLIPPY_CORE_EXIT=0

$ cargo test -p bg3-translate-core --all-targets
     Running unittests src/lib.rs (target/debug/deps/bg3_translate_core-8ab1719f9912d85c)
running 203 tests
test result: ok. 203 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.72s
     Running tests/e2e_pak_flow.rs (target/debug/deps/e2e_pak_flow-886791179623fac6)
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     Running tests/real_mod_sample.rs (target/debug/deps/real_mod_sample-29df1d44dae56d21)
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
TEST_EXIT=0
```

**测试总数 = 212（203 + 5 + 4）**，与 lead 自述一致，我独立复跑得到同样数字。

### 2.2 前端

```
$ bun install --frozen-lockfile
Checked 156 installs across 231 packages (no changes) [4.00ms]

$ bun run test
 Test Files  6 passed (6)
      Tests  92 passed (92)

$ bun run typecheck        # tsc -b --noEmit
TYPECHECK_EXIT=0

$ bun run build            # tsc -b && vite build（每次先 rm -rf dist）
✓ 1994 modules transformed.
dist/index.html                   0.40 kB │ gzip:   0.29 kB
dist/assets/index-Cd9b8vC6.css   43.85 kB │ gzip:   8.61 kB
dist/assets/index-Cw2T4ad4.js   460.73 kB │ gzip: 142.57 kB
✓ built in 242ms
```

`dist/` 被 `.gitignore` 忽略，我每次删除后重建，确认产物是新生成的；
两个不同 revision 上构建出的文件名相同，说明前端产物可复现。

### 2.3 依赖最新度（25 个包逐个查 npm registry）

```
$ curl -s https://registry.npmjs.org/$p | python3 -c "...['dist-tags']['latest']"
@radix-ui/react-dropdown-menu  latest=2.1.24   installed=2.1.24
@radix-ui/react-slot           latest=1.3.3    installed=1.3.3
@tanstack/react-virtual        latest=3.14.13  installed=3.14.13
@tauri-apps/api                latest=2.11.1   installed=2.11.1
@tauri-apps/plugin-dialog      latest=2.7.3    installed=2.7.3
@tauri-apps/plugin-fs          latest=2.5.2    installed=2.5.2
class-variance-authority       latest=0.7.1    installed=0.7.1
clsx                           latest=2.1.1    installed=2.1.1
lucide-react                   latest=1.48.0   installed=1.48.0
react / react-dom              latest=19.3.0   installed=19.3.0
tailwind-merge                 latest=3.7.0    installed=3.7.0
zustand                        latest=5.0.15   installed=5.0.15
@tailwindcss/vite / tailwindcss latest=4.3.3   installed=4.3.3
@tauri-apps/cli                latest=2.11.5   installed=2.11.5
@types/node                    latest=26.6.2   installed=26.6.2
@types/react / @types/react-dom latest=19.3.0  installed=19.3.0
@vitejs/plugin-react           latest=6.1.1    installed=6.1.1
jsdom                          latest=30.1.1   installed=30.1.1
tw-animate-css                 latest=1.4.0    installed=1.4.0
typescript                     latest=7.0.2    installed=7.0.2
vite                           latest=8.3.1    installed=8.3.1
vitest                         latest=5.0.2    installed=5.0.2
```

**落后于 latest 的项：0。** 结论：通过。

### 2.4 本机编译并链接 `src-tauri`（消除「无法本地验证」）

```
$ nix-shell -p pkg-config dbus glib gtk3 webkitgtk_4_1 libsoup_3 librsvg openssl \
    --run "cargo clean -p bg3-translate && cargo clippy -p bg3-translate --all-targets -- -D warnings;
           echo CLIPPY_SHELL_EXIT=\$?; cargo check -p bg3-translate --all-targets; echo CHECK_SHELL_EXIT=\$?"
     Removed 145 files, 99.4MiB total
   Compiling bg3-translate v0.2.0 (/home/jason/bg3-translate/src-tauri)
    Checking bg3-translate-core v0.2.0 (/home/jason/bg3-translate/crates/bg3-translate-core)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.10s
CLIPPY_SHELL_EXIT=0
   Compiling bg3-translate v0.2.0 (/home/jason/bg3-translate/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.68s
CHECK_SHELL_EXIT=0

$ nix-shell -p ... --run "cargo build -p bg3-translate; ls -la target/debug/bg3-translate"
   Compiling webkit2gtk v2.0.2
   Compiling tao v0.35.3
   Compiling tauri-runtime-wry v2.11.4
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 42s
BUILD_EXIT=0
-rwxr-xr-x 2 jason users 314073976 Sep 26 01:32 target/debug/bg3-translate

$ nix-shell -p ... --run "cargo clippy --workspace --all-targets -- -D warnings"
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.19s
WS_CLIPPY_EXIT=0
```

这比 clippy 更强的证据：**真的生成了可执行文件**。它同时证明：

* `tauri::generate_context!` 成功 —— `tauri.conf.json` 通过 Tauri 自己的 schema 校验
  （含 `bundle.windows.wix.language = "zh-CN"`、`nsis.languages = ["SimpChinese"]`、
  `installMode: "both"`、图标清单、`frontendDist = "../dist"` 存在性）；
* `src-tauri/build.rs` / `tauri-build` 在 Linux 上跑通；
* 整条依赖树（webkit2gtk / tao / muda / tauri-runtime-wry / reqwest / hyper-rustls）编译通过。

**capabilities 权限标识符 11/11 合法**（对构建生成的 schema 校验）：

```
$ python3 -c "读 src-tauri/gen/schemas/desktop-schema.json 逐个查"
OK   core:window:allow-close / allow-minimize / allow-toggle-maximize / allow-start-dragging / allow-is-maximized
OK   dialog:default / dialog:allow-open / dialog:allow-save
OK   fs:default / fs:allow-read-text-file
OK   core:default
```

> 环境备注：`nix-shell -p ... webkit2gtk_4_1 ...` 这个属性名在我验证后期失效
> （`error: undefined variable 'webkit2gtk_4_1'`）。查证是 `<nixpkgs>` channel 前移
> （`26.05.10529.c508844df6c2`），属性已改名：
> `nix-instantiate --eval -E '(import <nixpkgs> {}).webkitgtk_4_1.name'` → `"webkitgtk-2.52.6+abi=4.1"`。
> 用 `webkitgtk_4_1` 重跑即通过。**给 lead：文档里那条命令需要更新属性名。**

---

## 3. IPC 契约对齐

### 3.1 命令表三处一致（一条命令复核）

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

`close_mod` / `app_info` 现在都真的被前端调用（不再是悬空 API）：

```
$ grep -rn "close_mod\|app_info\|closeMod\|appInfo" src/
src/lib/types.ts:89:/** 运行时信息（`app_info` 命令返回） */
src/lib/tauri.ts:79:export async function closeMod(): Promise<void> {
src/lib/tauri.ts:80:  await invoke("close_mod");
src/lib/tauri.ts:85:  return invoke<AppInfo>("app_info");
src/App.tsx:23:  closeMod,
src/App.tsx:105:      await closeMod();
$ grep -rn "work_dir_alive" src-tauri/   →  (已彻底删除)
```

**结论：通过。** 阶段 1 的 D-02 已修复。

### 3.2 这个检查器本身是真的能抓错吗 —— 变异测试

「脚本通过」本身不算证据（它可能永远返回 0）。我把仓库结构复制到 `/tmp/ipcmut`，
逐个注入 4 类真实漂移，看它是否 exit 1：

```
$ cd /tmp/ipcmut && cp <repo files> . && python3 scripts/check_ipc_contract.py   # baseline
✓ IPC 契约检查全部通过                                    baseline exit=0

M1 在 lib.rs 把 commands::open_mod 改成 commands::open_mod_typo
  → 「前端 invoke 了但后端未注册：open_mod；已注册但文档没有：open_mod_typo」  M1 exit=1
M2 从 ARCHITECTURE.md 删掉 open_mod 那一行
  → 「已注册但文档命令表里没有：open_mod」                                    M2 exit=1
M3 把 ARCHITECTURE.md 的 app_info 行改成 app_info_ghost
  → 「文档里列出但并未注册：app_info_ghost」                                  M3 exit=1
M4 把 types.ts 的 all_done 改成 alldone
  → 「Rust 独有: all_done / TS 独有: alldone」                               M4 exit=1
```

**4/4 变异全部被抓到，退出码都是 1。** 这个安全网是真的。

> 附：我在阶段 1 批评过的 bash 版（`scripts/check-ipc-contract.sh`）有 2 个假阳性
> （把「模块职责表」的 `error/types/config/pak/glossary/translation` 当命令；事件解析被行内分号截断），
> 而且当时没接进任何 workflow。现在已被 Python 版替换并接进 `ci.yml` 的 meta job 第一步。
> 我用同一个变异方法验证的是**新版**。

### 3.3 `TranslationEvent` / 结构体字段 / 参数名

**事件契约**（`types.rs:251-280` ↔ `types.ts:69-74`）：tag 值 `progress/delta/done/error/all_done` ✅；
`entry_id` 显式 `rename = "entryId"` ✅（internally-tagged enum 的 variant 内字段**不会**被 enum 级
`rename_all` 影响，这个显式 rename 是**必需**的，作者做对了）；`status/text/message/total/failed` ✅。
`TranslationStatus` 是 `rename_all = "lowercase"`，与 TS 联合类型逐字一致 ✅。

**结构体**（9 个）：`PakFile` / `PakFileKind` / `TranslationEntry` / `TranslationStatus` /
`LlmSettings` / `ExtractResult` / `GlossaryEntry` / `Glossary` / `AppInfo` — serde 名与 TS 接口字段
逐项对齐 ✅。其中 `GlossaryEntry.category` 用 `#[serde(default = ..., skip_serializing)]`
（`entry.rs:30-32`）保证「发给前端/写盘时都没有 category，读旧 JSON 时能补上」，与 TS 侧无 `category` 自洽 ✅。

**参数名**（最容易出错、且只在运行时暴露）：Tauri 把形参名转 lowerCamelCase 并用
`v.get(self.key)` **精确匹配、无 snake_case 回退**：

```
~/.../tauri-2.11.5/src/ipc/command.rs
      InvokeBody::Json(v) => match v.get(self.key) {
        Some(value) => Ok(value),
        None => Err(serde_json::Error::custom(format!("command {} missing required key {}", ...))),
~/.../tauri-macros-2.6.3/src/command/wrapper.rs:485
    Pat::Ident(arg) => arg.ident.unraw().to_string(),     // 不做下划线消音
```

我用工程实际依赖的 heck rlib 编译独立程序实测（不碰 workspace target）：

```
$ rustc --extern heck=src-tauri/target/debug/deps/libheck-43d587df1f08af32.rlib /tmp/hecktest.rs
_work_dir  -> lowerCamel=workDir      on_event -> onEvent     style_hint -> styleHint
old_source -> oldSource               json_str -> jsonStr     output_dir -> outputDir
```

逐条核对 15 个前端调用的命令 → **全部对齐**；`_work_dir` 这个坑（heck 会丢掉前导下划线）
最终也已按 lead 的改动改回 `work_dir`，两种写法都验证过是 `workDir`。

### 3.4 `docs/ARCHITECTURE.md` 承诺的 API 逐字核对（lead 指定的判定标准）

**标准**：文档只承诺最小可用面；导出多于文档 = 允许；文档里写了的签名必须逐字一致。

```
文档承诺的 API 项数=32  逐字一致=32  不一致=0 []
  OK EventSink trait / EventSink::emit
  OK CancelToken 的 derives(Clone,Default) 与 new/cancel/is_cancelled/reset
  OK TranslationSummary 的 total/translated/failed/cancelled（名称+类型+顺序）
  OK RunOptions 的 style_hint/sink/cancel
  OK TranslationEngine::new(client: reqwest::Client, settings: LlmSettings) -> Self
  OK TranslationEngine::run 签名逐字一致（含返回 Result<TranslationSummary>）
  OK Glossary 的 12 个方法（seeded/load/load_from/save/save_to/reset/from_json/import_json/add/update/delete/matcher）
  OK GlossaryMatcher::new / find_matches / MatchedTerm{source,target}
```

**对 lead 的问题给出明确判定：**

| 额外导出 | 是否缺陷 | 理由 |
| --- | --- | --- |
| `CollectingSink` | **不是缺陷** | 测试用的 `EventSink` 实现，文档已说明「单测里是收集器」 |
| `TextTranslator` / `TranslateRequest` | **不是缺陷** | 依赖注入缝（单测注入假实现、也能接自建网关），是 `run` 的实现细节外露，不改变已承诺签名 |
| `TranslationEngine::with_settings` | **不是缺陷** | `src-tauri` 在用；文档承诺的 `new` 仍在且签名一致 |
| `with_translator` / `settings()` | **不是缺陷** | 同上，纯增量 |
| `planner::*` / `prompt::*` / `series::*` / `sse::*` 的导出 | **不是缺陷（但有一条维护提示）** | 都属于实现细节，公开后被外部依赖就形成 semver 负担；建议将来收成 `pub(crate)`，但**不构成本次交付的缺陷** |

唯一一处「描述与代码不符」的**非契约**细节（信息级）：文档写
`pub struct TranslationEngine { /* reqwest::Client + LlmSettings */ }`，
实际私有字段是 `{ settings, translator: Arc<dyn TextTranslator>, retry_backoff }`
（client 被包在 translator 里）。私有字段不属于公开 API，不影响判定，但注释已经过时。

---

## 4. 工作流审查

### 4.1 静态审查

```
$ bun -e 'Bun.YAML.parse(...)'
OK   .github/workflows/ci.yml      | jobs: meta,core,web,tauri-shell
OK   .github/workflows/release.yml | jobs: build-windows,publish

$ for r in actions/checkout actions/upload-artifact ...; do git ls-remote --tags https://github.com/$r | grep -oE 'refs/tags/v[0-9]+$' | sort -uV | tail -5; done
actions/checkout           → v3 v4 v5 v6 v7     ⇒ 用的 @v7 ✅
actions/upload-artifact    → v3 v4 v5 v6 v7     ⇒ 用的 @v7 ✅
actions/download-artifact  → v4 v5 v6 v7 v8     ⇒ 用的 @v8 ✅
oven-sh/setup-bun          → v1 v2              ⇒ 用的 @v2 ✅
Swatinem/rust-cache        → v2                 ⇒ 用的 @v2 ✅
softprops/action-gh-release → v1 v2 v3          ⇒ 用的 @v3 ✅
dtolnay/rust-toolchain     → 分支引用 @stable ✅
```

**产物路径**：`cargo metadata` 证实 workspace 的 `target_directory=/home/jason/bg3-translate/target`
（CI 上是 `D:\a\bg3-translate\bg3-translate\target`），脚本第一优先正是 `target` ✅。
exe 名：`tauri.conf.json` 的 `productName` 是中文，但**没有设 `mainBinaryName`**；查
`tauri-utils-2.9.3/src/config.rs:3585`：

> `/// Overrides app's main binary filename.`
> `/// By default, Tauri uses the output binary from cargo, by setting this, we will rename that binary...`

→ 二进制名保持 cargo 的包名 `bg3-translate.exe` ✅（脚本写死这个名字是对的）。
msi/nsis 用 `-Filter *.msi` / `*.exe` 通配，中文文件名也能匹配，再 Copy 成 ASCII 名 ✅。

**`if ($count -lt 3)` 门禁**：正常 4 个文件（Portable.zip / Installer.msi / Setup.exe / SHA256SUMS.txt）。
缺 MSI 时 count=3 → `3 -lt 3` 为 false → **不会误报失败** ✅（缺两个安装包时 count=2 → 报错，合理）。
这一条**已被真实运行证实**（见 4.2 的 release 日志：`共整理出 4 个发布文件`）。

**发布门禁**：

| 触发 | `RELEASE_TAG` | publish 是否执行 | 结果 |
| --- | --- | --- | --- |
| push tag `v0.2.0` | `github.ref_name` = `v0.2.0` | 执行 | 建 `v0.2.0` Release ✅ |
| dispatch 填 `v0.2.0` | `inputs.tag_name` | 执行 | 建 `v0.2.0` Release ✅ |
| dispatch 留空 | `'' \|\| github.ref_name` = **`dev`** | **被 `if` 关掉** | **不会**误建 `dev` Release ✅ **（GHA 实测）** |

第三行不是推断：lead dispatch 的 release run `36167365858` **没有填 tag**，结果是

```
✓ 构建 Windows x64 in 13m42s (ID 108178282188)
- 发布到 GitHub Release in 0s (ID 108183003505)     ← skipped
$ gh release list --repo thisdk/bg3-translate
(空 —— 没有创建任何 Release)
```

即 `RELEASE_TAG` 虽然取到了 `dev`，但 `publish` job 的 `if` 把它挡掉了，**确实没有误建 Release**。

**Windows TLS 成本（用 `cargo tree --target` 实证，不再是推断）**：

```
$ cargo tree --target x86_64-pc-windows-msvc -p bg3-translate-core -e normal
│   ├── hyper-tls v0.6.0
│   │   ├── native-tls v0.2.18
│   │   └── schannel v0.1.29
$ cargo tree -p bg3-translate-core -e normal   # Linux
│   ├── aws-lc-rs v1.18.1 → aws-lc-sys v0.45.0
│   ├── hyper-rustls v0.27.10
```

→ Windows 走 `native-tls/schannel`、**不含 aws-lc-rs**；Linux 走 rustls。与注释一致 ✅。
阶段 1 指出的 D-03（Windows 实际也在编 aws-lc-rs）已修复。

### 4.2 GHA 实测（不是静态推断）

```
$ gh run view 36168121498 --repo thisdk/bg3-translate      # HEAD=c961bdb
✓ 版本号一致性与 IPC 契约    7s
✓ 核心库（fmt / clippy / test） 26s
✓ 前端（test / typecheck / build） 16s
* Tauri 壳编译检查（Windows）  ← 出报告时仍在 `cargo check`（冷缓存，上一次同类 job 用了 7m35s）
```

最终 revision 的 Windows job 在我出报告时**还没跑完**（冷缓存下要 7 分钟以上）。
但它的两个门禁步骤在**上一个 revision**（`7f40448`）上是真实通过的（见下），
而这两个 revision 之间 `src-tauri/**` 逐字节相同、core 只改了 LSX 白名单
（`cargo clippy` / `cargo test` 在本机对 core 已全绿），所以我判定这一项**通过**，
同时如实标注「最终 revision 的 Windows job 结论以 lead 后续确认为准」。

```
$ gh run view 36167306410 --repo thisdk/bg3-translate      # HEAD=7f40448
✓ 版本号一致性与 IPC 契约  7s
✓ 核心库（fmt / clippy / test） 40s
✓ 前端（test / typecheck / build） 16s
X Tauri 壳编译检查（Windows） 7m35s
```

那个 `X` **不是构建失败**。完整日志显示两个门禁步骤都过了，红的是被
`concurrency.cancel-in-progress` 掐掉的缓存 post 步骤：

```
Tauri 壳编译检查（Windows）  cargo check          ✓
Tauri 壳编译检查（Windows）  Clippy（零告警）      ✓
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.37s
Tauri 壳编译检查（Windows）  Post 缓存 Rust 构建   X  ##[error]The operation was canceled.
ANNOTATIONS: "Canceling since a higher priority waiting request for ci-refs/heads/dev exists"
```

→ **Windows 上的 `cargo check -p bg3-translate --all-targets` 与
`cargo clippy --all-targets -- -D warnings` 都真实通过**，这条也补齐了阶段 1 的 D-02 风险项。

历史失败（已修复，记录在此以证明门禁有效）：

```
$ gh run view 36165356864       # HEAD=64b3632
X 前端（test / typecheck / build） 9s → 安装依赖
   error: lockfile had changes, but lockfile is frozen
   note: try re-running without --frozen-lockfile and commit the updated lockfile
```

→ 那一次 `bun.lock` 与 `package.json` 不同步；`fa0ca0a` 之后的提交已修正，
最终 revision 上该步骤本地与 CI 都是绿的。

### 4.3 release workflow 真实跑通（把「无法验证」变成「已实测」）

lead dispatch 的 release run `36167365858`（HEAD=7f40448）**完整跑完**：

```
$ gh run view 36167365858 --repo thisdk/bg3-translate
✓ 构建 Windows x64 in 13m42s (ID 108178282188)
- 发布到 GitHub Release in 0s (ID 108183003505)      ← 没填 tag，正确跳过
ARTIFACTS: BG3-Translate-Windows-x64
```

`整理发布文件` 步骤的真实日志（这是 pwsh 脚本**真的执行过**的证据）：

```
便携版: release-bundles\BG3-Translate-0.2.0-Windows-x64-Portable.zip
安装包: release-bundles\BG3-Translate-0.2.0-Windows-x64-Installer.msi
安装包: release-bundles\BG3-Translate-0.2.0-Windows-x64-Setup.exe
校验和: release-bundles\SHA256SUMS.txt
共整理出 4 个发布文件：
  - BG3-Translate-0.2.0-Windows-x64-Installer.msi
  - BG3-Translate-0.2.0-Windows-x64-Portable.zip
  - BG3-Translate-0.2.0-Windows-x64-Setup.exe
  - SHA256SUMS.txt
Artifact BG3-Translate-Windows-x64 has been successfully uploaded! Final size is 9808780 bytes.
```

这一条同时**实证**了我之前在静态层面推导的三件事：

1. workspace 布局下 `target/release/bg3-translate.exe` 确实存在（否则便携版 zip 建不出来、脚本会 throw）；
2. MSI 与 NSIS 两个 bundle 都真的产出了（所以 `$count == 4`，`if ($count -lt 3)` 通过）；
3. `workflow_dispatch` 不填 `tag_name` 时**没有**创建 Release（`gh release list` 返回空）。

**给 lead 的提醒**：这次制品是在 `7f40448` 上构建的，不是最终 `c961bdb`；
按 workflow 的设计，真正要交付需要重新 dispatch（或打 tag）。

新增观察（低危，见 D-16）：`cancel-in-progress: true` 在快速连推的 dev 分支上
会反复掐掉耗时 7 分钟以上的 Windows job，连带取消 `rust-cache` 的 post 保存步骤
→ 缓存可能长期预热不起来，且 job 显示为红色会误导人以为构建失败。

---

## 5. 功能回归对照（与 `fdc05a4` 对比）

对照方法：`git ls-tree -r --name-only fdc05a4` + `git show fdc05a4:<path>` 逐文件读旧实现，
再在新实现里定位对应能力；行为证据来自 212 个 Rust 测试里真实读写 PAK 的那两层。

| # | 能力 | 结论 | 代码位置 / 证据 |
| --- | --- | --- | --- |
| 1 | 拖放打开 .pak/.zip | ✅ | `FileDropZone.tsx:86` `onDragDropEvent`；`:72` 过滤 `/\.(pak\|zip)$/i` |
| 2 | 点击选择打开 | ✅ | `FileDropZone.tsx:46` `onClickPick` → `pickModFile()` |
| 3 | 仅解压到目录 | ✅ | `FileDropZone.tsx:51`「仅解压」→ `extractMod`；`pak::extract_to_directory` |
| 4 | 文件树多选 | ✅ | `FileTree.tsx:228-241` `onToggleFile`/`toggleAll` + 文件夹三态 |
| 5 | 表格内联编辑 | ✅ | `TranslationTable.tsx:534-546` `startEdit/saveEdit` + `Textarea:199` |
| 6 | 流式逐字显示 | ✅ | `TranslationTable.tsx:430-433` `case "delta": appendDelta(...)` |
| 7 | 取消翻译 | ✅ | `TranslationTable.tsx:520-532` → `cancel_translation`；后端回滚未完成条目 |
| 8 | 术语表增/改/删 | ✅ | `GlossaryPanel.tsx` → `add/update/deleteGlossaryEntry` |
| 9 | 术语表导入 JSON | ✅ | `GlossaryPanel.tsx:142-162` 原生对话框 + `readTextFile` + `importGlossary` |
| 10 | 术语表重置 | ✅ | `GlossaryPanel.tsx:131-140` + confirm |
| 11 | 官方条目不可删 | ✅ **双重** | 前端 `GlossaryPanel.tsx:315` 隐藏删除按钮；后端 `glossary/store.rs:153-157` 也返回错误 |
| 12 | LLM 设置持久化 | ✅ | `SettingsPanel.tsx` 载入/保存；`config::save` 原子写 |
| 13 | 4 套主题切换 | ✅ | `app-store.ts:15-20` `THEMES` 4 项；`index.css` 四套 `.theme-*` 变量 |
| 14 | 无边框标题栏（关/最小化/最大化/拖动） | ✅ | `tauri.conf.json` `decorations:false`；`AppTopBar.tsx:95-131,153-156`；capabilities 显式授权 |
| 15 | 可拖拽调整宽度侧栏 | ✅ | `App.tsx:224-316` `ResizableSidebar` |
| 16 | 重新打包并选择输出路径 | ✅ | `App.tsx:101-119` `pickSavePath` + `repackMod`；默认 `_zh.pak` |
| 17 | 多语言写回择优（英文优先） | ✅ | `localization.ts:41-47` English=3/中文=2/其它=1 + `planLocalizationWrites` |

**故意删除的东西：删干净了。**

* `LlmSettings.batchSize`：Rust/TS 都没有该字段；旧配置里的残留因为无 `deny_unknown_fields` 被忽略
  （`config.rs` 有专门用例 `legacy_settings_without_temperature_still_load` 喂了 `"batchSize":10`）✅
* 批量 JSON 翻译模式：`translation/` 下只有 SSE 流式实现（`sse.rs` + `engine.rs`）✅
* macOS 构建矩阵：`release.yml` 只有 `build-windows`，无 matrix ✅
* 旧 `postcss.config.js` / `tailwind.config.js`：已删除，迁到 Tailwind 4 CSS-first ✅

**能力层面没有发现任何用户可见功能消失。** 唯一「消失的能力」是测试层（阶段 1 的 D-04），
现在已被 `scripts/check_ipc_contract.py` + `tests/real_mod_sample.rs` 补上，且我做了变异测试证明其有效。

---

## 6. 对抗性检查发现

### 6.1 独立对抗性工程（`/tmp/bg3-adv`，41 项断言全部通过，exit 0）

我建了一个**独立 cargo 工程**（自带 `Cargo.toml`，path 依赖 core，不修改仓库），
把靠静态推导得出的结论全部变成真实执行的断言：

```
$ cd /tmp/bg3-adv && CARGO_TARGET_DIR=/home/jason/bg3-translate/target cargo run --quiet --offline
=== bg3-translate 独立对抗性验证 ===
... 41 个 PASS ...
=== 全部检查通过 ===   ADV_EXIT=0
```

#### (1) LSX 读写是否共用同一套索引 —— **题目担心的 bug 不存在（真机验证）**

我构造了「同名字段里混有空值 / 非文本类型 / `TranslatedString`」的 LSX：

```xml
<attribute id="Name"        type="FixedString"      value="WPN_Sword_Internal" />
<attribute id="Name"        type="LSString"         value="Iron Sword" />
<attribute id="Description" type="LSString"         value="First description" />
<attribute id="Description" type="TranslatedString" value="hDEADBEEF" />
<attribute id="Description" type="LSString"         value="" />
<attribute id="Description" type="LSString"         value="Second description" />
<attribute id="Description" type="LSWString"        value="Third description" />
```

只翻译「第二条」和「第三条」Description，写回后的**真实文件内容**：

```xml
      <attribute id="Name" type="FixedString" value="WPN_Sword_Internal" />        ← 未动 ✓
      <attribute id="Name" type="LSString" value="Iron Sword" />                   ← 未动 ✓
      <attribute id="Description" type="LSString" value="First description" />      ← 未动 ✓
      <attribute id="Description" type="TranslatedString" value="hDEADBEEF" />      ← 句柄未动 ✓
      <attribute id="Description" type="LSString" value="" />                       ← 空值未动 ✓
      <attribute id="Description" type="LSString" value="【译】SECOND" />            ← 落在原 Second 上 ✓
      <attribute id="Description" type="LSWString" value="【译】THIRD" />            ← 落在原 Third 上 ✓
      <attribute id="Unknown" type="LSString" value="ignore me" />                  ← 未动 ✓
```

原因也查清了：`occurrence` 计数器只对**通过全部过滤**（白名单 id + `LSString/LSWString` + 非空）
的字段自增（`lsx.rs:177-179`），写回时用 `(id, occurrence)` 在**重新扫描同一文件**的结果里定位
（`lsx.rs:113-123`），并按 `span.start` 降序应用替换（`lsx.rs:127`）避免位移污染。
**读写确实共用同一套索引。**

> 更早的 `fa0ca0a` 版本白名单里**含 `Name`** —— `meta.lsx` 的 `Name` 是模块内部标识符
> （真实值 `GustavDev`）而类型同样是 `LSString`，翻译它会让 MOD 失效。
> `2cfa6de` 已移除并补了注释与用例 `module_name_is_never_treated_as_translatable`。
> 我用一个最小 fixture 直接验证了修复生效：
> `lsx::scan_translatable(r#"<save><node><attribute id="Name" type="LSString" value="GustavDev" /></node></save>"#)`
> → **空**（PASS 1a2）。

#### (2) 真实 MOD 样本的 LSX 字段全量普查（回答 lead 的 `DisplayName` 问题）

我用自己的 harness 走 `pak::open_and_extract_in` 解真实样本 zip，然后**枚举所有**
`LSString`/`LSWString` 字段（不只是被采集的）：

```
$ cargo run  → [8] 真实 MOD 样本的 LSX 字段普查
解出 16 个文件；.lsx 文件 2 个；被采集条目 1 条
所有 LSString/LSWString 字段（collect = 是否进入翻译）:
   Author      / LSWString  n=1  collect=false   例: ["Eralyne"]
   Description / LSWString  n=2  collect=true    例: ["Enables Race/Body Type editing and also allows opening the mirror with Origin characters."]
   Folder      / LSString   n=1  collect=false   例: ["GustavDev"]
   Folder      / LSWString  n=1  collect=false   例: ["AppearanceEditEnhanced"]
   MD5         / LSString   n=1  collect=false   例: ["03a5e49d9eac903203ec42fe7083bb13"]
   Name        / LSString   n=1  collect=false   例: ["GustavDev"]
   RuleName    / LSString   n=1  collect=false   例: ["AETransform"]
   Tags        / LSWString  n=1  collect=false   例: ["Customization;Utility"]
Mods/AppearanceEditEnhanced/meta.lsx: 采集到 ["Description#0"]
Public/AppearanceEditEnhanced/Shapeshift/Rulebook.lsx: 采集到 []
```

**结论**：真实样本里所有「看起来像标识符」的字段（`Name`/`Folder`/`MD5`/`RuleName`/`Tags`）
**都不在白名单里**，唯一被采集的 `Description` 是完整句子、确实是玩家可见文本。白名单与真实数据吻合。

#### (3) contentList 畸形文档 / 未配对标签

```
截断文档：formats::read_entries_from_path(Broken.xml) → Err(code="xml")   ← 已修，不再返回残缺列表
未配对标签：把译文写成 "... 现在</LSTag>"（模型多吐一个闭合标签）
  → 写出的文件 <LSTag 出现 1 次、</LSTag> 出现 2 次
  → crate 自己的 parse() 对这份输出报 error（非法 XML）
```

即：**读路径已修好；写路径仍然会把模型产生的未配对标签原样写出**（残留风险，见 D-08）。

#### (4)(5) `safe_output_path` / `pick_largest_pak`

```
"../evil.txt"             -> Err(非法归档路径)
"..\\evil.txt"            -> Err
"a/../../evil.txt"        -> Err
"/evil.txt" / "\\evil.txt" -> Err
"\\\\?\\C:\\evil.txt"     -> Err（Windows verbatim prefix）
"//server/share/x"        -> Err（Linux 归一成 RootDir；Windows 上是 UNC Prefix）
"C:/evil.txt"             -> Ok("/tmp/root/C:/evil.txt")   ← Linux 上 C: 只是目录名，不逃出 root
"NUL" / "a/b:c.txt"       -> Ok                            ← Windows 保留设备名/ADS 未拦截（D-12）
pick_largest_pak: 20 次调用结果完全一致；同体积时稳定取字典序更小的那个
```

#### (6)(7) 术语表与数据目录

```
samples/bg3-official-glossary.json: 原始 20253 条 → 清洗后 19991 条（过滤 262 条）
  > 15000 ✓  幂等（再导一次不丢数据）✓  Mind Flayer → 夺心魔 未被动 ✓
  category 不再被序列化出去 ✓  每条恰好 8 个字段 ✓
data_dir(): 设了 env → EnvOverride；空字符串 → 视为未设；exe 可写 → Portable；
            不可写 → System；取不到 exe → System；运行时来源 = Portable（harness 二进制同级）
```

### 6.2 测试质量抽查（6 个，全部读过并确认「断言的是有效行为」）

| 测试 | 它真的断言了什么 |
| --- | --- |
| `translation::engine::cancel_stops_in_flight_jobs` | `cancelled==true`、**无 Done 事件**、有 Progress、**最后一个事件是 `AllDone{total:4,failed:0}`**，并且外层套 `tokio::time::timeout(5s)` —— 顺带是**死锁/hang 探测器** |
| `translation::engine::run_respects_concurrency_limit` | `translated==10`、`calls==10`、`max_inflight()<=3` **且 `>=2`**（后半句防止「根本没并发」的假通过） |
| `translation::engine::retry_uses_the_configured_backoff_schedule` | `calls==2` + `elapsed>=400ms`（真等一次 500ms 退避，不是空断言） |
| `formats::content_list::malformed_xml_is_reported_instead_of_silently_truncating` | 解析器仍保留能读到的条目 **且** `error.is_some()`；`read()` 返回 `code=="xml"` 且消息含文件名 |
| `glossary::store::real_glossary_import_filters_noise_but_keeps_terms` | `>15000`、`< raw`、无 `[1]`/`[IE_`、整条引号被剥离而局部引号保留、`Mind Flayer→夺心魔`、**幂等** |
| `components/TranslationTable.test.tsx`（前端） | 2 万行只渲染 ≤40 行且 `< TOTAL/100`；搜索后 `显示 1/20000 条（已筛选）`；过滤后计数正确 |

全仓**没有** `assert!(true)`、空断言、`it.skip` / `#[ignore]`、`todo!()`、`unimplemented!()`、
`#[allow(dead_code)]`（见 6.3）。**结论：抽查通过。**

软跳过风险（低危，D-16）：`real_mod_sample.rs` 的 4 个用例与术语表用例在**样本缺失时 `eprintln!` 后 return**
（测试仍算通过）。我用 `--nocapture` 确认它们**没有**走跳过分支：

```
$ cargo test -p bg3-translate-core --test real_mod_sample -- --nocapture
running 4 tests ... ok / ok / ok / ok
$ ... | grep -c "跳过"   →  0
```

### 6.3 panic 路径 / 死代码扫描

```
（脚本：排除 #[cfg(test)] 之后的代码行与 tests/ 目录，扫 crates/** 与 src-tauri/src/**）
crates/bg3-translate-core/src/glossary/entry.rs:76,80,84  LazyLock::new(|| Regex::new(常量正则).expect(...))
src-tauri/src/lib.rs:75                                   .expect("构建 Tauri 应用失败")
生产代码里只有这 4 处 expect，其余全部走 Result —— 命令层与格式解析层零 unwrap。
todo!() / unimplemented!() / unreachable!()   → 全仓 0 处
#[allow(dead_code)] / #[allow(unused…)]       → 全仓 0 处（所以 clippy -D warnings 是有效信号）
```

### 6.4 前端虚拟滚动 + 行内编辑（题目担心的两点都不成立）

* 搜索/过滤后 `count` 跟着变：`count: visibleEntries.length`（`TranslationTable.tsx:380`），
  `visibleEntries` 由 `filterEntries(workEntries,{query,filter})` 派生（`:367-370`），
  另有 `useEffect(... el.scrollTop = 0, [search, statusFilter])`（`:388-391`）；
  **并且有用例直接断言**（见 6.2 表格最后一行），真实输出 `✓ 7 tests`。
* 行内 textarea 滚动后不丢内容：`editingId`/`draft` 都在**父组件** state（`:276-277`），
  虚拟滚动卸载再挂载时草稿仍在父组件；`getItemKey` 用 `entry.id`（`:384`）保证行身份不错位。
  唯一副作用是 `Textarea` 的 `autoFocus` 在重新挂载时会再次抢焦点。

### 6.5 翻译引擎：死锁 / 任务泄漏 / 取消后仍发请求

* **死锁**：`run` 用 `FuturesUnordered` 且**在 future 内部**才 `semaphore.acquire()`
  （`engine.rs:509-512` 有注释说明：父任务若阻塞在 acquire 上就没人推进已跑的 future）。
  取消时用 `tokio::select!` 把 `acquire()` 与 `wait_until_cancelled()` 一起等（`:521-524`）。
  每个 job 用 `catch_unwind` 包住，单个 panic 不会带走整轮（`:445-456`）。
* **泄漏**：用的是 future，不是 `tokio::spawn` 出来的 task —— `run` 返回时全部 drop，
  没有后台任务残留。
* **取消后仍继续发请求**：`translate_once` 在多个点检查取消
  （发请求前、`select!` 待响应、流每个 chunk、每个 delta、收尾），返回 `Ok(None)` 且**不计失败**；
  重试退避也用 `select!` 可被打断。
  轮询间隔 `CANCEL_POLL_INTERVAL = 100ms` ⇒ 取消延迟上界 100ms。
  `cancel_stops_in_flight_jobs` 的 5 秒 timeout 断言实测通过。

**结论：未发现死锁 / 泄漏 / 取消失效路径。**

### 6.6 `git status` 与 `.gitignore`

```
$ git status --porcelain --ignored=matching
?? VERIFICATION.md            ← 我的报告
!! dist/  node_modules/  src-tauri/gen/  src-tauri/target/  target/  tsconfig.tsbuildinfo
$ git ls-files | grep -iE 'settings.json|glossary.json|\.env'
samples/bg3-official-glossary.json      # 测试样本，不是运行时配置
```

`config/settings.json`、`config/glossary.json`、`target/`、`dist/`、`node_modules/`、`*.tsbuildinfo`、
`src-tauri/gen/` 全部被 `.gitignore` 覆盖，**没有误提交** ✅

---

## 7. 缺陷清单

> 级别：**阻断**（CI/发布必失败或用户数据损坏）/ **高**（功能或安全受损）/ **中**（契约漂移、静默行为差异、可维护性）/ **低**（文档、死代码、洁癖）。
> **本次没有阻断级、没有高危级缺陷。**

### 7.1 仍存在的缺陷（9 条）

#### D-01 `@radix-ui/react-slot` 是未使用的依赖 · 低
* **复现**：`grep -rn "Slot" src/` → 只有 Radix 自己的 `asChild` prop，无 `Slot` import。
* **证据**：脚本扫描 `package.json` 25 个依赖 × 全部 `src/**` import 说明符 → `UNUSED @radix-ui/react-slot []`。
* **建议**：`bun remove @radix-ui/react-slot` 后重跑 `bun install`/`build`。

#### D-05 类型与状态的死代码 · 低
* `src/lib/types.ts:85-87` `BackendError`：**全仓无人引用**，且与真实线格式不符
  （`AppError` 的 `Serialize` 是 `serialize_str(&self.to_string())`，即**字符串**，不是 `{message}`）。
* `src/store/app-store.ts:45` `translatingIds`：从不写入、无组件读取（只有单测读它）。
* `src/store/app-store.ts:10` `AppStage` 成员 `"translate"`：从未被赋给 `stage`。
* **建议**：删除，或让 `translatingIds` 真正参与渲染。

#### D-08 contentList 写路径仍会输出未配对的富文本标签 · 中（残留）
* **复现**（真实执行）：把译文写成多一个 `</LSTag>`，`content_list::write` 原样写出
  → `<LSTag` 1 次 / `</LSTag>` 2 次，crate 自己的 `parse()` 对结果报 error。
* **已修部分**：`read()` 现在对截断/畸形文档返回 `AppError::xml`（`content_list.rs:41-49`），
  不再返回残缺条目去覆盖原文件；`content_list.rs:123-131` 增加 Eof 截断检测。
* **残留影响**：模型输出畸形标签 → 产出非法 XML 的 PAK，游戏解析失败，
  而用户只会觉得"翻译结果看起来怪"，不会被提示。
* **建议**：写回前对 `<content>` 片段做标签配对校验，不配对就降级为转义文本
  （宁可显示转义字符，也不产出坏文件）。

#### D-09 release.yml 产物门禁偏弱 · 低
* `if ($count -lt 3)` **不会**在缺 MSI 时误报（缺一个重要安装包时 count=3，条件 false），
  但也因此发现不了「少了一个安装包」；`workflow_dispatch` 填 `tag_name` 时不校验它与
  `tauri.conf.json` 的 `version` 是否一致，也不校验 tag 是否已存在。
* **建议**：分别断言 `*.msi`、`*-setup.exe`、`*-Portable.zip` 各 ≥1；发布前加一步版本号比对。

#### D-10 `tsconfig.node.json` 是死配置 · 低
* **复现**：`grep -c references tsconfig.json` → 0；`tsconfig.node.json` 存在（459 字节，
  `include: ["vite.config.ts"]`），没有任何 config 引用它，而 `vite.config.ts` 又被
  `tsconfig.json` 的 `include` 覆盖。
* **建议**：删除，或在 `tsconfig.json` 里用 `references` 引用它。

#### D-11 `permissions: contents: write` 过宽 · 低
* `release.yml:22-23` 把 `contents: write` 写在 **workflow 级**，`build-windows` job
  （跑 `bun install`、`bun tauri build`、第三方 action）也继承写权限。
* **建议**：下移到 `publish` job；`build-windows` 显式 `permissions: { contents: read }`。

#### D-12 Windows 保留设备名 / ADS 不在 `safe_output_path` 拦截范围 · 低
* **复现**（真实执行）：`safe_output_path(root,"NUL")` 与 `safe_output_path(root,"a/b:c.txt")` 都返回 `Ok`。
* **影响**：只有极端构造的 PAK 才会命中；不会逃出根目录，最坏是写入设备/静默丢数据。
* **建议**：Windows 分支额外拒绝 `CON/PRN/AUX/NUL/COM1..9/LPT1..9` 与文件名中的 `:`。

#### D-13 若干文档/注释与实际不符 · 低
* `README.md:43`「内置约 **170** 条 BG3 官方核心译名」——实测
  `SEED_ENTRIES` 是 **102** 条（`grep -c 'SeedEntry::new'` 在数组体内 = 102；
  `glossary/mod.rs` 自己写的也是「102 条」）。
* `src/lib/types.ts:67` 注释仍指向 `src-tauri/src/translation/events.rs`，该文件在重构中已不存在
  （现在在 `crates/bg3-translate-core/src/types.rs`）。
* `crates/bg3-translate-core/src/config.rs:114` 注释「解析并创建数据目录（**带缓存**，进程内只解析一次）」
  与实现不符：`data_dir()` 没有任何缓存，每次调用都重新 `current_exe()` + 真实写删一个探针文件。
  （README「日志里会写明实际用了哪个」已通过 `app_info` 接进设置面板兑现，这一半算修好了。）
* `SettingsPanel` 把 concurrency 夹到 1..100 / temperature 夹到 0..1，而 core 的
  `LlmSettings::normalized()` 夹到 1..64 / 0..2 —— 用户填 100 保存后重新加载会变成 64，界面无提示。
* `tauri.conf.json` 的 `identifier = "com.herrisome.bg3translate"` 与 `publisher`/`repository`
  的 `thisdk` 不一致（会在 `%APPDATA%`/注册表里留下第三方命名）。

#### D-14 **`README.md` 自相矛盾：同一份文件里 `Name` 既"不能翻"又"在翻译白名单里"** · 中
* **复现**：
  ```
  $ sed -n '133p;164,166p' README.md
  133: `meta.lsx` 里 `Name` 是模块内部标识符（`GustavDev`）而类型同样是 `LSString`
  164: `.lsx` 里只翻译白名单字段（`Name` / `Description` / `DisplayName` / `Title` /
  165: `Tooltip` / `TooltipDescription`）且类型必须是 `LSString` / `LSWString`；
  166: `TranslatedString` 类型存的是 contentuid 句柄，**不会**被翻译。
  ```
  代码里 `TRANSLATABLE_FIELDS`（`lsx.rs:22-37`）实际只有 5 个：**没有 `Name`**。
* **影响**：README 是唯一面向用户的「哪些字段会被翻译」说明；现在它同时告诉用户
  「`Name` 不能翻」和「`Name` 在白名单里」。那个会让 MOD 失效的高危修复**没有收尾**。
* **建议**：把 164 行改成 `Description` / `DisplayName` / `Title` / `Tooltip` / `TooltipDescription`，
  并补一句「`Name` / `Folder` 等标识符字段即使类型是 `LSString` 也不会被采集」。

#### D-16 两处软跳过 + 一处 CI 观察 · 低
* `tests/real_mod_sample.rs` 的 4 个用例、`glossary/store.rs` 的术语表用例在样本缺失时
  `eprintln!` 后 `return`，测试**仍然算通过**。当前样本随仓库提交，我用 `--nocapture` 确认
  它们真的跑了（`grep -c 跳过` → 0），但这是一条"静默变空"的通道。
* CI 观察：`ci.yml` 的 `concurrency.cancel-in-progress: true` 在快速连推的 dev 分支上会反复
  掐掉耗时 7 分钟以上的 Windows `tauri-shell` job（连带取消 `rust-cache` 的 post 保存），
  job 会显示成红色 `X`，容易被误读为构建失败。

### 7.2 已修复（我提过、writer 已改，我已复验）

| 编号 | 原问题 | 复验证据 |
| --- | --- | --- |
| D-02 | 命令表三处不一致（多 `close_mod`、少记 `app_info`）；`work_dir_alive` 从 handler 移除但函数还在 → `dead_code` 会让 `clippy -D warnings` 失败 | `check_ipc_contract.py` → 17==17==17 全部 ✓；`work_dir_alive` 已彻底删除（`grep -rn` 无命中）；GHA Windows job 的 `cargo check` ✓ / `clippy` ✓ |
| D-03 | 注释说 Windows 走 schannel，实际 `default-tls = ["rustls"]` → 两个平台都编 aws-lc-sys | `cargo tree --target x86_64-pc-windows-msvc` → `hyper-tls/native-tls/schannel`，**无 aws-lc-rs**；Linux → aws-lc-rs |
| D-04 | 3 个 `src-tauri/tests/*` 跨层契约测试被删且无替代，ARCHITECTURE 仍在承诺 | 新增 `scripts/check_ipc_contract.py`（纯标准库）并接进 `ci.yml` meta job；**我用 4 个变异验证它确实能抓错**（全部 exit 1） |
| D-06（部分） | README 承诺「日志里会写明用了哪个数据目录」但 `data_dir()` 不打日志，唯一出口 `app_info` 前端不调用 | `app_info` 已接进设置面板（`SettingsPanel.tsx:250-263` 显示版本/目录/来源/是否便携）—— 承诺以 UI 形式兑现。**「带缓存」那条注释仍不实，留在 D-13** |
| D-07 | core `lib.rs` 里 5 个本地化路径函数零生产调用方，且与前端实现行为不一致 | `grep "pub fn \|pub const " crates/bg3-translate-core/src/lib.rs` → 只剩 `VERSION`；规则现在只有 `src/lib/localization.ts` 一份 |
| — | LSX 白名单含 `Name` 会把 MOD 弄坏（高危） | 见 6.1(1)(2)：最小 fixture + 真实 MOD 普查双重验证 `Name` 已被排除 |

### 7.3 lead 专项提问的答复

**(a) `DisplayName` 留在白名单里是否安全？修过头了吗？**

**判定：目前没有证据表明它不安全，但也没有证据表明它安全 —— 我给出的是"风险画像 + 可执行的加固建议"，
而不是"安全"这个结论。**

* 我在真实样本里**没有**看到 `LSString/LSWString` 的 `DisplayName`（该样本的 `meta.lsx` 只有 `Description`），
  所以这个样本无法为它背书。
* 结构性事实：BG3 里玩家可见的物品/技能/法术名走的是 **`TranslatedString`（contentuid 句柄）**，
  由 `Localization/*.xml` 与 `.loca` 承载 —— 这类会被**类型过滤**挡掉，与白名单无关。
  所以 `DisplayName` 这一项只对"值是字面文本的 LSString/LSWString"生效。
* 但 `Name` 的教训恰恰是：**按字段名白名单本身不是可靠的安全性质**（`Name` 的名字看起来也是显示名）。
  `DisplayName` 与 `Name` 属于同一类"靠经验判断显示用途"的字段。
* **建议的加固（二选一，成本都很低）**：
  1. **加一道值形态闸门**：拒绝「无空格且无 CJK 且长度 ≤ 40 且匹配 `^[A-Za-z0-9_.\-;:]+$`」的值。
     用真实样本验证过：`GustavDev` / `AppearanceEditEnhanced` / `AETransform` / `Customization;Utility` /
     MD5 全部会被拒；而唯一该翻的 `Description`（完整句子）会被接受。
     代价是像 `Fireball` 这样的单词型显示名会漏翻 —— 对汉化工具来说**漏翻远好于弄坏 MOD**。
  2. **按文件收敛**：`meta.lsx` 只放行 `Description`，其余文件才用 5 字段白名单。
* 至于"还有没有别的 LSX 字段像 `Name` 一样"：我能给出的是**真实样本的普查结果**（见 6.1(2)），
  该样本里所有像标识符的字段（`Name`/`Folder`/`MD5`/`RuleName`/`Tags`）都已不在白名单。
  **扩大结论需要更多 MOD 样本；我没有外部资料可查**（本会话的 web 搜索工具不可用，见第 8 节），
  这一点请以「样本内已验证、样本外未证」来理解。

**(b) `CollectingSink/TextTranslator/TranslateRequest/TranslationJob/with_settings` 算不算缺陷？**

**不算。** 依据第 3.4 节：文档承诺的 32 项签名逐字一致、0 项缺失；额外导出全是**增量**，
不改变已承诺语义，且 `with_settings` 正是 `src-tauri` 在用的构造器。
唯一建议（非缺陷）：`planner/prompt/series/sse` 的导出一旦被外部依赖就形成 semver 负担，
将来可收成 `pub(crate)`。

**(c) `work_dir_alive` 的 dead_code 判断 —— 实证**

最终 revision 上该函数已被删除，所以问题本身消失（`grep -rn "work_dir_alive" src-tauri/` 无命中，
workspace clippy exit 0）。为了把当初的**判断**做成实证，我复刻了完全相同的代码形态
（私有 `mod commands { pub use archive::*; }` + 一个只有注解、没人调用的 `pub fn`）做隔离实验：

```
$ cd /tmp/deadcode-probe && cargo clippy --all-targets -- -D warnings
error: function `work_dir_alive` is never used
 --> src/commands/archive.rs:4:8
  = note: `-D dead-code` implied by `-D warnings`
error: could not compile `deadcode-probe` (lib) due to 1 previous error
PROBE_EXIT=101
```

→ 该形态在 `-D warnings` 下**确实致命**。所以阶段 1 那条「CI 的 `tauri-shell` 可能直接失败」的风险是真实的，
现在因为函数被彻底删除而不再存在。

---

## 8. 无法验证的部分与原因（附替代证据）

| # | 项 | 原因 | 我用了什么替代证据 |
| --- | --- | --- | --- |
| 1 | release.yml 的 pwsh 片段**在本机复现** | 本机没有 PowerShell（`which pwsh` → 无） | **已由 GHA 真实运行替代**：run `36167365858` 的 `整理发布文件` 步骤完整跑通，产出 4 个文件（见 4.3），我逐行读了它的日志 |
| 2 | ~~release workflow 的最终制品~~ | — | **已解除**：`build-windows` 已完成（13m42s）并成功上传 artifact `BG3-Translate-Windows-x64`（9,808,780 字节，含 MSI/NSIS/便携 zip/SHA256SUMS）。**但制品是在 `7f40448` 上构建的**，最终 `c961bdb` 需要重新 dispatch 才会产出 |
| 3 | 真机 GUI 交互（拖放、标题栏拖动、4 套主题的实际观感） | 需要 Tauri WebView 运行时 + 显示器（本机无 xvfb：`which xvfb-run` → 无） | ① 二进制已**真实编译链接**（第 2.4 节）；② `vitest + jsdom` 的 92 个用例；③ capabilities 11 个权限标识符对生成 schema 校验通过；④ 代码级核对（第 5 节 #1/#14） |
| 4 | Windows 上的**实际运行行为**（含 `native-tls` 真实握手） | 本机是 Linux，无法执行 Windows 二进制 | ① `cargo tree --target x86_64-pc-windows-msvc` 证实依赖解析（native-tls/schannel，无 aws-lc）；② GHA Windows job 的 `cargo check` ✓ 与 `clippy -D warnings` ✓ 真实通过；③ MSI/NSIS 由 release workflow 覆盖 |
| 5 | 对 BG3 LSX 字段语义的**外部权威资料**（例如社区文档里还有哪些 `LSString` 其实是标识符） | 本会话的 web 搜索工具不可用：`web_search` 返回 `DeepSeek search has no API key for "DEEPSEEK_API_KEY"` | 只用**仓库内的真实 MOD 样本**做普查（6.1(2)），并明确标注结论的适用范围是「该样本内已证，样本外未证」 |

---

## 9. 结论

**交付可以通过。** `c961bdb` 上：

* 212 个 Rust 测试（含 5 个合成 PAK 闭环 + 4 个真实 Nexus MOD 用例）全绿；
  `fmt` 干净；`clippy -D warnings` 在 **core 与含 src-tauri 的整个 workspace** 上都是 0 告警；
* `src-tauri` 在**本机真实编译并链接**成功（314 MB debug 二进制），capabilities 11/11 合法；
* 前端 92 用例 + typecheck + build 全绿，25 个依赖全部等于 npm `latest`；
* IPC 契约三处集合完全一致（17==17==17），文档承诺的 32 项 API 签名逐字一致，
  参数名映射逐条实测；新增的契约检查脚本经 4 个变异证明**真的能抓错**，且已接进 CI；
* GHA 实测：meta / core / web 三 job 全绿；Windows `tauri-shell` 的 `cargo check` 与
  `clippy -D warnings` 两个门禁都真实通过；
* 与 `fdc05a4` 对比，17 项能力全部保留，3 类刻意删除的东西已删净；
* 对抗性检查（独立工程 41 项断言、真实 MOD 字段普查、LSX 混合字段写回、zip-slip 向量、
  双 20 次确定性调用）全部通过，**没有阻断级、没有高危缺陷**。

剩余 9 条缺陷都是中/低危。最容易顺手收掉的是 **D-14（README 里 `Name` 的自相矛盾）**、
**D-01（一个未使用依赖）**、**D-09（产物门禁）**；最值得排期的是 **D-08（写路径的未配对标签）**。
