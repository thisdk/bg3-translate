# 红队基线报告（T5 / 独立对抗式缺陷发现）

> 作者：`verifier`（独立红队，只读）。**本报告只针对冻结基线 HEAD 的内容**，
> 不参考也不评判四位 writer 的报告。
> 所有结论都附**实际执行过的命令与真实输出**；没跑过的一律进入 §4「无法验证」。
> **文中所有 `文件:行号` 都是 HEAD 的行号**（写作期间工作区已被 writer 改动，
> 行号可能已经漂移；引用时请用 `git show HEAD:<path>` 复核）。

---

## §1 被验证的 revision 与 worktree hash

| 项 | 值 | 采集命令 |
| --- | --- | --- |
| revision | `422f667a0d5d3be26190c0f5b55b0796a67a7b76` | `git rev-parse HEAD` |
| 开工时工作区 | 干净（`git status --porcelain` 无输出） | `git status --porcelain` |
| 开工时 worktree hash | `5219e03ea871dcc2d5fd836abbf65b74` | 见下方命令 |
| 写报告时 worktree hash | `90575e24462396aa84ab72b5dc15f86b`（writer 已在改代码） | 同上 |

```console
$ cd /home/jason/bg3-translate
$ git rev-parse HEAD
422f667a0d5d3be26190c0f5b55b0796a67a7b76
$ find src crates src-tauri scripts .github docs README.md package.json Cargo.toml -type f -not -path '*/target/*' | sort | xargs md5sum | md5sum
5219e03ea871dcc2d5fd836abbf65b74  -
```

**与 lead 记录的基线 `ad8d3219378a5b89059a3558138ec7d1` 不一致。** 差异原因未查明
（可能是 `sort`/文件集合/计算时机不同）。为了不让这个不一致影响结论，我改用
**逐文件与 HEAD 比对**来证明「我的探针跑在哪个内容上」：

```console
$ cd /home/jason/bg3-translate
$ diffcount=0; total=0
$ for f in $(git ls-files | grep -E '^(crates|src|src-tauri|scripts|samples|docs|README.md|package.json|Cargo.toml|Cargo.lock|index.html|vite.config.ts|components.json|tsconfig.json)'); do
    if [ -f "/tmp/rt/$f" ]; then total=$((total+1));
      if ! git show "HEAD:$f" | cmp -s - "/tmp/rt/$f"; then diffcount=$((diffcount+1)); echo "DIFF: $f"; fi
    fi
  done; echo "对比文件数=$total 与 HEAD 不一致数=$diffcount"
对比文件数=166 与 HEAD 不一致数=0
```

结论：**本报告的全部探针都跑在 `/tmp/rt`、`/tmp/rtB`、`/tmp/rtf` 三份副本上，
这三份副本与 `HEAD` 的 166 个受版本控制文件逐字节一致**，即完全等于冻结基线。
写作期间 writer 已开始修改工作区（`git status` 出现 15 个 M 与 4 个 ??），
**这些改动未被本报告验证**。特别地：写报告时再跑一次 grep 会发现
`translation/sse.rs` 与 `translation/translator.rs` **已经在改 R-01 这块**
（新增了 `is_truncated` 之类的东西）——那是 writer 的改动，不属于基线，
本报告对 R-01 的判定只针对 HEAD。凡是要「对着基线」验证的 grep，
本报告一律用 `git show HEAD:<path> | grep` 的形式给出。

探针文件（可复跑，md5 见 §5）：
`/tmp/redteam-r3/rt_probe{,2,3,4,5,6}.rs`、`/tmp/redteam-r3/rt-loc.ts`。

### 基线自检（证明探针环境本身是健康的）

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline 2>&1 | grep -E '^test result|^running'
running 263 tests
test result: ok. 263 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.69s
running 6 tests
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 6 tests
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
running 10 tests   ← 我加的 rt_probe
running 1 test     ← 我加的 rt_probe2
running 3 tests    ← 我加的 rt_probe4
running 7 tests    ← 我加的 rt_probe5
running 2 tests    ← 我加的 rt_probe6
```

即基线自身 `263 + 6 + 6` 全绿，与 lead 给的口径一致。

---

## §2 发现清单

| 编号 | 严重度 | 一句话 | 可复现 |
| --- | --- | --- | --- |
| R-01 | **高** | 流式响应被截断（`finish_reason=length` 或服务端不发 `[DONE]` 直接断流）时，半截译文被当成成功译文落库/写进 PAK | 是（端到端，含真实 reqwest 链路） |
| R-02 | **中高** | `content_list` 写回在特定合法输入下产出**非法 XML**，会让游戏放弃解析整份本地化文件（丢的是整份译文） | 是 |
| R-03 | 中 | `write_file_entries` 的 `file_name` 完全不校验路径：绝对路径与 `../` 都能写到工作目录之外 | 是 |
| R-04 | 中 | `extract_zip_to_find_pak` 对解压**没有任何大小/数量上限**：65 KB 的 zip → 64 MiB（1027×），磁盘可被写满 | 是 |
| R-05 | 中 | 同时勾选 English 与已有中文文件时，**未翻译条目会用英文原文覆盖目标文件里已有的中文**（整份文件的已有译文被回退） | 是 |
| R-06 | 低 | `<content>` 缺 `version` 属性时，写回会补出一个 `version=""`（原文件没有该属性） | 是 |
| R-07 | 低 | 源码里的未知实体（如 `&nbsp;`）读进来是字面量，写回被二次转义成 `&amp;nbsp;`；即使零译文也会改写文件字节 | 是 |
| R-08 | 低 | 429/401 等确定性失败一律重试 4 次、忽略 `Retry-After`；错误信息被双重前缀 | 是（端到端） |
| R-09 | 低 | SSE 解码器缓冲无上限：服务端只要一直不发换行就能持续吃内存 | 是（组件级） |
| R-10 | 低 | 结构保真校验忽略标签属性：互换两个属性不同的同类标签、丢掉全部属性，都不会被判失败 | 是 |
| R-11 | 信息 | `safe_output_path` 放行含 NUL 字节与尾随点/空格的文件名（不构成逃逸，但在 Windows 上不可写/被静默改名） | 是 |
| R-12 | 信息 | `.loca` 的 version 解析失败时静默退化为 1（有注释、有测试，属已知取舍，此处仅记录行为） | 是 |

---

### R-01（高）截断的流式响应被当成成功译文

**位置**：`crates/bg3-translate-core/src/translation/translator.rs:134-182`（流循环与
`ensure_stream_produced_text`）、`src/translation/sse.rs:161-177`（`parse_chat_chunk`）。

**现象**：服务端返回 `HTTP 200` + SSE，先给一部分 `delta.content`，随后发一个
`finish_reason:"length"` 的收尾 chunk 并**直接关闭连接（不发 `data: [DONE]`）**。
引擎把已收到的半截文本当成完整译文：发 `Done`、`summary.translated == 1`、
`failed == 0`。用户界面上这条是「已翻译」，打包时会被写进 PAK。

**复现**（端到端：本地 127.0.0.1 假服务端 + 真实 `reqwest` + 真实 `SseDecoder`；
不发任何外部请求）：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe4 g1 -- --nocapture
running 1 test
test g1_truncated_stream_is_accepted_as_success ... G1 请求数=1 耗时=992.012µs
G1 summary = TranslationSummary { total: 1, translated: 1, failed: 0, cancelled: false }
G1 events = Progress(Localization/English/a.xml#h1) Delta(...,"Appearance Editing") Delta(...," allows you to change") Done(...,"Appearance Editing allows you to change") AllDone(total=1,failed=0)
G1 原文（133 字符）= Appearance Editing allows you to change every aspect of your character's body, face and hair in ways that the base game never allowed
G1 最终落库译文 = Some("Appearance Editing allows you to change")
G1 译文长度 39 / 原文长度 133 —— 半截译文是否被当成成功 = true
```

服务端发的字节（探针 `rt_probe4.rs` 内联）：

```
data: {"choices":[{"delta":{"content":"Appearance Editing"},"finish_reason":null}]}

data: {"choices":[{"delta":{"content":" allows you to change"},"finish_reason":null}]}

data: {"choices":[{"delta":{},"finish_reason":"length"}]}

```

**根因**（三处叠加）：
1. `translator.rs:142-148`：`stream.next()` 返回 `None`（连接结束）时只做
   `decoder.finish()`，**不检查是否收到过 `[DONE]`**；`finish()` 为空就 `break`，
   视作正常结束。
2. `sse.rs:161-177`：`parse_chat_chunk` 的返回类型是 `Option<String>`，
   `finish_reason` 被整条丢掉，调用方拿不到「被截断」这个信息。
   针对 **HEAD 内容**的 grep（务必用 `git show HEAD:`，因为写作期间工作区已被改）：

   ```console
   $ cd /home/jason/bg3-translate
   $ git show HEAD:crates/bg3-translate-core/src/translation/sse.rs | grep -n "finish_reason"
   369:            r#"{"id":"1","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#;
   376:        let chunk = r#"{"id":"chatcmpl-1",...,"finish_reason":null}]}"#;
   383:        let full = r#"{"id":"c1",...,"finish_reason":null}]}"#;
   396:        // 只有 finish_reason 的收尾 chunk
   398:            parse_chat_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#),
   $ git show HEAD:crates/bg3-translate-core/src/translation/translator.rs | grep -n "finish_reason"
   （无匹配）
   ```

   即 HEAD 上 `finish_reason` **只出现在测试代码里**，生产路径一次都没读它。
3. `translator.rs:241-249`：`ensure_stream_produced_text` 只拦「一个字都没有」的情况。

**影响**：原文里没有占位符/标签的条目（绝大多数普通文本）**不会被结构校验拦住**，
半截译文直接进入 PAK。这是「产出看起来正常、实际内容缺失」的静默数据损坏，
用户在下游很难发现。真实触发场景：网关默认 `max_tokens` 截断、服务端超时/负载均衡
切断连接、代理中断。

**建议方向**（不代改）：把 `finish_reason` 透出成显式信号（例如
`enum ChunkOutcome { Delta(String), Truncated }`），或至少记录「见过 `[DONE]`」，
未见过就按可重试失败处理。

---

### R-02（中高）`content_list` 写回会产出非法 XML

**位置**：`crates/bg3-translate-core/src/formats/content_list.rs:316-347`
（`write_text_fragment`）、`:353-387`（`is_tag_balanced`）、`:390-406`
（`is_allowed_inline_tag`）。

**现象**：`is_tag_balanced` / `write_text_fragment` 都用「第一个 `<` 到**第一个** `>`」
切标签，**不处理属性值里的 `>`**，也不校验结束标签 `</` 与名字之间是否合法。
两类输入都会让 `render()` 产出**非法 XML**：

**触发 A：属性值里有裸 `>`**（XML 规范允许属性值里出现字面 `>`，所以我们的
`parse` 能正常读进来；真实 MOD 与 LLM 输出都可能出现）：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe a3b -- --nocapture
A3b source = "Deal <LSTag Tooltip=\"a>b\">fire</LSTag> damage"
A3b rendered = <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">Deal <LSTag Tooltip="a>b&quot;&gt;fire</LSTag> damage</content></contentList>
A3b reparse error = Some("syntax error: attribute value not closed: `\"` not found before end of input")
A3b reparse source = None
```

**触发 B：结束标签写成 `</ b>`**（LLM 偶尔会吐出这种形态；`is_allowed_inline_tag`
把 `</ b>` 当成合法标签）：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h2 -- --nocapture
H2 is_tag_balanced(<LSTag>x</ LSTag>) = true
H2 is_allowed_inline_tag(</ LSTag>) = true
H2 rendered = <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">译文 <b>x</ b> 尾</content></contentList>
H2 reparse error = Some("ill-formed document: expected `</b>`, but `</ b>` was found")
```

**根因**：`is_allowed_inline_tag` 先 `trim_start_matches('/')` 再 `trim()`，把
`/ b` 规范成 `b`；`raw`/`write_text_fragment` 又把这串**原样**写进 XML。
等价地，`is_tag_balanced` 用「名字 trim 后相等」判配对，比 XML 语法宽松。

**影响**：写回后整个 `<contentList>` 文档变成非法 XML。`content_list::read` 对
非法文档是**直接报错中止**的（这是对的），但如果游戏侧也这样处理，**丢的是整份
译文**（同一文件里所有条目），而不只是出问题的那一条。注意触发 A 在**零译文、
纯读一次再写回**的情况下就会发生（文件被「无意义地」改坏）。

**复现文件**：`/tmp/redteam-r3/rt_probe.rs`（`a3b_raw_gt_in_attr_value`）、
`/tmp/redteam-r3/rt_probe5.rs`（`h2_space_after_end_tag_slash_produces_invalid_xml`）。

---

### R-03（中）`write_file_entries` 的 `file_name` 不校验路径，可写到工作目录之外

**位置**：`src-tauri/src/commands/entries.rs:26-36` → `formats::write_entries`
（`crates/bg3-translate-core/src/formats/mod.rs:47-58`）→
`pak::resolve_disk_path`（`crates/bg3-translate-core/src/pak.rs:111-113`）。
对比：解包路径用的是 `safe_output_path`（`pak.rs:137`），**写回路径完全没用它**。

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe6 i1 -- --nocapture
I1 逃逸文件是否出现在工作目录之外: true -> /tmp/nix-shell-455-255726294/rtprobe6-i1-34219/evil.xml
I1 内容 = Some("<?xml version=\"1.0\" encoding=\"utf-8\"?><contentList><content contentuid=\"h1\" version=\"1\">pwned-outside-workdir</content></contentList>")
I1 绝对路径逃逸: true 大小=Ok(88)
```

`PathBuf::join` 遇到绝对路径会**整条替换**，所以
`resolve_disk_path(work, "/tmp/rt-i1-absolute.loca")` 就等于那个绝对路径：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe b1 -- --nocapture
B1 file_name="../../evil.loca" -> /tmp/.../work/unpacked/../../evil.loca
B1 file_name="/tmp/rt-absolute-escape.loca" -> /tmp/rt-absolute-escape.loca
B1 file_name="..\\..\\evil.loca" -> /tmp/.../work/unpacked/../../evil.loca
```

**触达性（诚实评估）**：正常前端只会把解包列表里的名字回传，而那些名字在解包时
已经过 `safe_output_path` 过滤（见 §3-3，`..`/绝对路径都会被拒），所以**仅靠恶意
MOD 文件本身不足以触发**；触发需要「前端被控」（恶意/被篡改的 webview 内容、
IPC 调用方）或将来某个新调用方。因此我定级为「中」而不是「高」：这是一个
**信任边界缺失**（同一份代码里两个 API 一个校验一个不校验），修起来很便宜。

**同类问题**：`repack_mod(work_dir, output_path)`（`src-tauri/src/commands/archive.rs:47`）
的 `output_path` 也是前端可控、直接写盘；`extract_mod(file_path, output_dir)`
的 `output_dir` 同理。它们都**没有**限制在某个根目录内（若产品上「让用户选保存
位置」是有意设计，那 `write_file_entries` 的 `file_name` 更应该校验 —— 它是唯一
一个「文件名」而不是「用户选的路径」的参数）。

---

### R-04（中）zip 解压没有任何大小/数量上限

**位置**：`crates/bg3-translate-core/src/pak.rs:278-311`（`extract_zip_to_find_pak`）。

```console
$ python3 -c "
import zipfile
data = b'\0' * (64*1024*1024)
with zipfile.ZipFile('/tmp/rt-bomb.zip','w',zipfile.ZIP_DEFLATED,compresslevel=9) as z:
    z.writestr('payload.pak', data)
import os; print('zip size', os.path.getsize('/tmp/rt-bomb.zip'))"
zip size 65352

$ cd /tmp/rtB && RT_BOMB=/tmp/rt-bomb.zip cargo test -p bg3-translate-core --offline --test rt_probe3 f2 -- --nocapture
F2 zip 大小 = 65352 字节；解包耗时 = 24.070576ms；落地字节 = 67108864；结果 = Err(Pak("打开 PAK 失败: not a valid PAK file: no valid signature found"))
F2 膨胀倍率 = 1026.8830946260252
```

**现象**：65 KB 的输入在 **24 毫秒**内写出 64 MiB；没有任何单文件大小上限、
总大小上限、条目数上限。放大到 20 MB 的 zip 就能写满几十 GB 磁盘。
`entry.enclosed_name()` 挡住了 zip-slip（见 §3-3），但**挡不住炸弹**。

**影响**：用户「打开一个 MOD」是产品的主入口，输入就是一个下载来的 `.zip`。
磁盘写满会让应用与用户机器同时进入异常状态；失败路径 `open_and_extract`
（`pak.rs:192-202`）虽然会清理工作目录，但那是**解压完之后**，磁盘已经被写满。

---

### R-05（中）多语言同时勾选时，未翻译条目会用英文覆盖目标文件里已有的中文

**位置**：`src/lib/localization.ts:41-47`（优先级 英文3 > 中文2 > 其它1）、
`:66-90`（`planLocalizationWrites`）、`src/App.tsx:48-62`（写回闸门后逐份写）。
`src/components/FileTree.tsx:261-263` 提供了一键「全选」，所以这个路径很好走到。

**复现**：

```console
$ cd /tmp/rtf && bun run rt-loc.ts
target: Localization/Chinese/x.xml sourceFile: Localization/English/x.xml priority: 3
  实际写进文件的内容: ["Fireball","Ice"]
只勾中文时: [["火球","寒冰"]]
```

（探针构造：MOD 同时带 `Localization/English/x.xml`（原文 Fireball/Ice）与
`Localization/Chinese/x.xml`（已有中文 火球/寒冰），两个文件都被勾选。
`write_entries` 写的是 `entry.effective_text()`，未翻译条目 = 原文。）

**影响**：MOD 自带官方/人工中文时，只要用户勾上英文文件（或点全选），
输出包里 `Localization/Chinese/*.xml` 中**未翻译的条目**会从中文退回英文。
对「已有中文、只想补翻一部分」的用户，这是**已有内容的净损失**。
现有测试只覆盖了 English vs Polish（`src/lib/localization.test.ts:145-161`），
**没有覆盖 English vs Chinese** 这个方向。

**设计歧义**：注释说「英文原文最权威」——对*源文本*成立；但目标文件名是
`Localization/Chinese/...`，把英文写进去与「产出中文包」的产品目标冲突。
（这条我按「行为导致已有译文丢失」定级为缺陷；也接受 lead 判定为有意取舍，
但它至少应该有一条测试把行为钉住。）

---

### R-06（低）`<content>` 缺 `version` 属性 → 写回补出 `version=""`

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe a2 -- --nocapture
A2 version = ""
A2 out = <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="">Hello</content></contentList>
```

原文件 `<content contentuid="h1">Hello</content>`（无 version）被改成 `version=""`。
`content_identity`（`content_list.rs:258-269`）把缺失属性变成空串，`render`
（`:200-203`）无条件写 `version`。若游戏按整数解析 version，这会让整份文件解析失败。
真实 BG3 文件都带 version，所以触发概率低，但「无意义的属性被凭空添加」本身
就是保真缺口（读-写不是恒等）。

---

### R-07（低）未知实体被二次转义

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe a1 -- --nocapture
A1 parsed source = "A&nbsp;B & C \u{a0} D&nbsp;"
A1 written  = <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">A&amp;nbsp;B &amp; C   D&amp;nbsp;</content></contentList>
A1 reparsed = "A&nbsp;B & C \u{a0} D&nbsp;"
A1 原文件与写回文件的 XML 文本是否逐字节一致: false
```

`resolve_reference`（`content_list.rs:272-284`）对未知实体**原样保留** `&nbsp;`，
写回时 `BytesText` 又把 `&` 转义成 `&amp;`。逻辑语义在「宽松解析」下等价
（两级解码都得到字面 `&nbsp;`），但**文件字节变了**，且对「认识 `&nbsp;` 的解析器」
语义不同。同一份文件里**未翻译的条目**也会被改写。定级「低」是因为我没能证明
游戏侧语义确实变化（见 §4）。

---

### R-08（低）错误分类与错误信息

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe4 -- --nocapture
G2 请求数=4 耗时=3.507674165s summary=TranslationSummary { total: 1, translated: 0, failed: 1, cancelled: false }
G2 events = Progress(...)×4 Error(...,"大模型调用错误: 大模型调用错误: 流式响应结束但没有任何文本（网关可能把错误包成了 200，或服务端未按 SSE 返回）") AllDone(total=1,failed=1)
G3 请求数=4 总耗时=3.506712801s summary=TranslationSummary { total: 1, translated: 0, failed: 1, cancelled: false }
G3 events = Progress(...)×4 Error(...,"大模型调用错误: 大模型调用错误: API 返回 429 Too Many Requests: ") AllDone(total=1,failed=1)
```

三个可确认的问题：
1. **不区分可重试性**：`retry.rs:136-149` 对所有 `Err` 一视同仁重试
   `MAX_ATTEMPTS = 4` 次。API Key 错误（401）、模型名错误（404）也会白等 3.5 秒并
   发 4 次请求。G3 显示 429 也是 4 次请求、3.5 秒。
2. **忽略 `Retry-After`**：`grep -rn "Retry-After\|retry_after" crates/bg3-translate-core/src`
   无匹配；G3 里服务端给了 `Retry-After: 60`，客户端在 3.5 秒内打了 4 次。
3. **错误信息双重前缀**：`error.rs:29` 的 Display 是 `大模型调用错误: {0}`，
   而 `retry.rs:151` 用 `AppError::Llm(last_err)` 再包一层（`last_err` 已经是
   `err.to_string()`），于是用户看到「大模型调用错误: 大模型调用错误: …」。

对 1、2 我倾向性能/礼貌问题而非正确性问题；3 是确定的显示缺陷（前端错误横幅直接展示）。

---

### R-09（低）SSE 解码器缓冲无上限

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h4 -- --nocapture
H4 喂入 32MiB 无换行数据后，decoder 仍未产出任何事件（缓冲无上限，无背压）
```

`sse.rs:57-67` 的 `push` 无条件 `self.buffer.extend_from_slice(chunk)`，
`take_line`（`:128-148`）在遇到换行前一直返回 `None`。服务端（或中间网关）只要持续
发送不带 `\n`/`\r` 的字节，内存就会一直涨。触发者是用户自己配置的 base URL，
所以定级「低」；缓解方式也很简单（缓冲区上限 + 报错）。

---

### R-10（低）结构保真校验忽略标签属性

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h6 -- --nocapture
H6 互换标签内容后的保真问题 = []（空 = 未被发现）
H6 丢掉全部属性的保真问题 = []
```

`fidelity.rs:314-336` 的 `render_tag` 把标签规范化成 `<LSTag>` / `</LSTag>`，
**属性完全不参与签名**。后果：
* 两个属性不同（`Tooltip="Fire"` / `Tooltip="Ice"`）的同类标签，**内容互换**不会被
  判失败 → 游戏里显示的是错的效果说明；
* 译文把 `<LSTag Type="Action">` 写成 `<LSTag>`（丢属性）也不会失败 → 富文本渲染
  语义丢失。

这不是 XML 层损坏（写回仍是合法 XML），所以定级「低」。但仓库里有一条注释
（`fidelity.rs` 的 `attribute_changes_are_ignored` 测试）说明「忽略属性」是有意的，
那就属于**已知取舍**——我把它列出来是因为它与「完整性校验」的宣称存在落差。

---

### R-11（信息）`safe_output_path` 放行 NUL 字节与尾随点/空格

```console
$ cd /tmp/rtB && cargo test -p bg3-translate-core --offline --test rt_probe3 f4 -- --nocapture
F4 "evil." -> Ok("/tmp/.../evil.")
F4 "evil " -> Ok("/tmp/.../evil ")
F4 "a\0b.txt" -> Ok("/tmp/.../a\0b.txt")
F4 "..%2f..%2fx.txt" -> Ok("/tmp/.../..%2f..%2fx.txt")
F4 "....//x.txt" -> Ok("/tmp/.../..../x.txt")
```

不构成逃逸（`..%2f` 只是普通文件名），但在 Windows 上：尾随点/空格会被文件系统
静默裁掉（`evil.` 与 `evil` 冲突），含 NUL 的名字不可写。属加固项。
同一命令里也可见 `../`、绝对路径、`C:/`、`CON`、`NUL.loca` 都被正确拒绝。

---

### R-12（信息）`.loca` version 静默退化

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h7 -- --nocapture
H7 写回后 version = "1"（原为 "not-a-number"）
```

`formats/loca.rs:51` 的 `entry.version.parse().unwrap_or(1)`。有注释、有测试
（`invalid_version_falls_back_to_one`），是明确的取舍；仅在脏数据场景下会改数据，
记录备查。

---

## §3 已尝试但**证伪**的攻击面（同样附真实命令与输出）

> 这一节与 §2 同等重要：下面每一条都是我**实际尝试攻过、并确认攻不动**的方向。

### 3-1 `walk_files` 会不会在符号链接环上死循环？—— 不会

构造 `sub/loop -> <root>` 的符号链接环，5 秒超时观察（`rt_probe.rs::c1`）：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe c1 -- --nocapture
C1 walk_files 返回 42 个文件（未死循环）
```

原因：每次下降都会把路径拼长，最终超过 `PATH_MAX` 导致 `read_dir` 失败并
`continue`（`pak.rs:169-171`），循环自然终止。**不是无限循环**（但仍会做几百次
无用遍历，属可选加固）。

### 3-2 `repack` 会不会把 `zip_contents/`、`pak_meta.json` 打进包？—— 不会

```console
$ cd /tmp/rtB && cargo test -p bg3-translate-core --offline --test rt_probe3 f1 -- --nocapture
F1 解出 16 个文件（PakFile 列表 16 条）
F1 work_dir 顶层内容 = ["zip_contents", "unpacked", "pak_meta.json"]
F1 repacked.pak 大小 = 41110
F1 重打包后的条目 = [16 条，全部是 MOD 原始路径]
F1 *** 泄漏进包的工作目录文件 = [] ***
```

进一步做**逐字节**等价性检查（`rt_probe6::i2`）：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe6 i2 -- --nocapture
I2 重打包条目数 = 16 / 解包文件数 = 16，逐字节比较了 16 条
I2 内容不一致 = []
```

即：真实样本 MOD 解包 → 原样重打包后，**16 个条目的内容逐字节一致**，
没有多余条目。`repack` 只 `add_directory(&unpacked)`（`pak.rs:427-430`）。

### 3-3 `safe_output_path` 的 zip-slip / pak-slip 绕过？—— 全部被拒

见 R-11 的命令输出：`../x.txt`、`a/../../x.txt`、`/etc/x.txt`、`..\x.txt`、
`C:/x.txt`、`a:b.txt`、`CON`、`con.txt`、`NUL.loca` 全部返回 `Err`。
剩下能通过的 `evil.` / `evil ` / 含 NUL / `..%2f` / `....//` 都不逃逸。

### 3-4 `meta.lsx` 的 `Name` 会不会被翻译？—— 不会（独立复核了 writer 的既有结论）

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h1 -- --nocapture
H1 Mods/AppearanceEditEnhanced/meta.lsx 扫描出的可翻译字段 = ["Description#0"]
H1 文件里有 id="Name" = true；是否被扫描为可翻译 = false
H1 Mods/AppearanceEditEnhanced/meta.lsx 无译文写回逐字节一致 = true
H1 Public/AppearanceEditEnhanced/Shapeshift/Rulebook.lsx 扫描出的可翻译字段 = []
H1 Public/AppearanceEditEnhanced/Shapeshift/Rulebook.lsx 无译文写回逐字节一致 = true
```

### 3-5 「HTTP 200 但 body 是错误 JSON」会不会被当成空译文成功？—— 会被拦下

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe4 g2 -- --nocapture
G2 请求数=4 耗时=3.507674165s summary=TranslationSummary { total: 1, translated: 0, failed: 1, cancelled: false }
G2 events = Progress(...)×4 Error(...,"…流式响应结束但没有任何文本（网关可能把错误包成了 200，或服务端未按 SSE 返回）") AllDone(total=1,failed=1)
```

### 3-6 SSE 被任意切分（含多字节字符被切断）会不会错乱？—— 不会

除仓库自带的「所有二段切分 + 逐字节喂」测试外，我自己做了**三段切分**穷举：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h3 -- --nocapture
H3 三段切分组合数 = 3240，不一致数 = 0
```

（负载含 CRLF 事件、多字节 UTF-8「你」、`[DONE]`。）

### 3-7 LOCA 二进制往返会不会丢语义？—— 真实样本与极端输入都守恒

真实样本（`rt_probe3::f3`）：把 10 条全改成译文写回，再读回来，`contentuid` 与
`version` 全部保持 `1`，文本正确。极端输入（`rt_probe5::h5`）：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h5 -- --nocapture
H5 写入成功，读回 4 条
H5   key=h1 ver=1 len=0 前20=""
H5   key=h1 ver=1 len=3 前20="dup"
H5   key=h2 ver=7 len=100000 前20="xxxxxxxxxxxxxxxxxxxx"
H5   key=h3 ver=1 len=7 前20="含\0NUL"
```

空文本、重复 key、10 万字符、NUL、version=7 全部原样往返。
另：真实样本 `.loca` **不改译文时写回字节完全一致**（`rt_probe2` 的
「往返后有变化的文件」只列出两个 XML，两个 `.loca` 都没进列表）。

### 3-8 真实样本 XML 读-写-读会不会丢条目？—— 条目不丢（只有缩进被规范化）

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe2 -- --nocapture
E1 Localization/English/AppearanceEditEnhanced.xml -> 10 条条目
E1 *** ... 写回后字节数变化 1271 -> 1227 ***
E1 Localization/Polish/AppearanceEditEnhanced.xml -> 10 条条目
E1 *** ... 写回后字节数变化 1314 -> 1279 ***
E1 往返后有变化的文件 = ["Localization/English/AppearanceEditEnhanced.xml", "Localization/Polish/AppearanceEditEnhanced.xml"]
```

字节变化来自「缩进/换行被规范化 + `<content>` 内文本被 `trim()`」，
条目数、`contentuid`、`version`、`source` 逐条比较**全部一致**（我的探针在
任何一条不一致时都会打印 `*** 条目内容变化 ***`，输出里没有）。
注意 `raw.trim()`（`content_list.rs:119`）会吃掉条目文本首尾空白——真实样本里没有
因此变化的条目，但「首尾空格有意义的文本」会被改（未构造出真实用例，列入 §4）。

### 3-9 畸形 XML 会不会静默丢条目？—— 会报错，不会静默

`content_list::read`（`content_list.rs:41-51`）在 `parse().error.is_some()` 时返回
`Err`。我用 A3b/H2 两次「写坏的文件」反证了这条防线确实生效
（`A3b reparse error = Some(...)`、`H2 reparse error = Some(...)`）。
仓库自带的 `malformed_xml_is_reported_instead_of_silently_truncating` 也覆盖。

---

## §4 无法验证项与原因

| 项 | 原因 |
| --- | --- |
| Windows 上的路径语义（尾随点/空格的静默裁剪、保留设备名、大小写不敏感碰撞、`..%2f` 在 NTFS/资源管理器里的行为） | 本机是 Linux（`uname -a` → `Linux nixos 6.18.33.2-microsoft-standard-WSL2 ... x86_64 GNU/Linux`，WSL2 下的 Linux 语义）；R-11 只能给出 Linux 侧行为 |
| 「非法 XML / `version=""` / 半截译文」在**真机 BG3** 里的具体表现（是整份文件被丢弃，还是逐条容错） | 没有游戏本体，无法验证加载器行为；§2 的影响描述基于代码里 `read` 的失败语义与常见 XML 解析器行为，**属推断，已在正文里标注** |
| 真实 LLM 网关的截断比例、是否发 `[DONE]`、`max_tokens` 默认值 | 无 API key，且不允许发外部请求；R-01 用本地假服务端复现，只能证明「这种响应一定会被当成成功」，不能给出线上发生率 |
| 代理/中间设备把连接切断的具体形态（半截 UTF-8、半截 SSE 帧之后直接 RST） | 只覆盖了「正常 FIN 结束」这一种；RST/超时的组合未测 |
| `.zip` 内含重复 `.pak`、损坏 PAK、声明大小与实际不符（4 GB 声明）时 `Package::open`/`read_file` 的行为 | 未构造成功；`bg3rustpaklib` 的边界行为超出本仓库，且构造需要大量时间。见 §5 未做项 |
| PAK 内**同名重复条目**时「最后一个可读副本」是否与游戏一致 | 未构造出带重复条目的真实 PAK（`PackageBuilder` 我只用它做了打包，没做重复注入），列为未验证 |
| `content_list` 对「条目文本首尾空白有意义」的场景是否保真 | 真实样本里没有这种条目；我可以构造（`raw.trim()`），但没有证据证明游戏需要它，故只记录机制不判缺陷 |
| 前端（React/zustand）侧的异步竞态、delta 批处理、虚拟滚动 | 本轮红队时间用在了 Rust 核心与真实数据上；前端只覆盖了 `localization.ts`（R-05）。**前端其余部分本轮未验证**，建议 lead 明示由 T3 的报告 + 我的 T6 复核来补 |

---

## §5 复现清单（探针文件的 md5）

```console
$ md5sum /tmp/redteam-r3/*
e8a8466b98a0517e5300921afa44de2d  rt_probe2.rs
04f74a9715885c44c81573a1242fb65a  rt_probe3.rs
7eacfeea0fd5119760527b67b1019e7a  rt_probe4.rs
28abdcee809ca2f7cf1279cd8e5ac8d0  rt_probe5.rs
3e072ed35943f4ec2583c41db15b8aea  rt_probe6.rs
ec20b1bfa1c96864643165e2346ffbb0  rt_probe.rs
```

复跑方式（把文件放进 /tmp 副本的 `crates/bg3-translate-core/tests/`）：

```console
$ cp /tmp/redteam-r3/rt_probe*.rs /tmp/rt/crates/bg3-translate-core/tests/
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe  -- --nocapture   # R-02/R-07/R-06/§3-1/§3-3
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe2 -- --nocapture   # §3-8
$ cd /tmp/rtB && RT_BOMB=/tmp/rt-bomb.zip cargo test -p bg3-translate-core --offline --test rt_probe3 -- --nocapture  # R-04/§3-2/§3-3/R-11
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe4 -- --nocapture   # R-01/R-08/§3-5
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 -- --nocapture   # R-02b/R-09/R-10/§3-4/§3-6/§3-7
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe6 -- --nocapture   # R-03/§3-2
$ cd /tmp/rtf && bun run rt-loc.ts                                                          # R-05
```

**本轮未做（留给 T6 或后续轮次）**：PAK 同名重复条目语义、损坏 PAK/超大声明、
前端异步竞态与流式渲染、`glossary` 2 万条术语匹配、`config.rs` 数据目录回退、
CI/脚本漂移。

---

## §6 给 lead 的优先级建议

1. **R-01** 必须先修：它直接产出「内容缺失但显示成功」的 PAK，且用户无从察觉。
2. **R-02** 次之：会把整份本地化文件变成非法 XML，代价是整份译文。
3. **R-04** 一行上限就能挡（单文件 + 总量 + 条目数），性价比最高。
4. **R-03 / R-05** 需要在产品语义上拍板：R-03 是补一次 `safe_output_path`
   （与解包侧对齐）；R-05 需要决定「目标文件已有译文时的回退策略」。
5. R-06 ~ R-12 属加固/体验，可进 backlog。

---
---

# 追加轮次（T6 准备期，未改动上面 §1~§6 的任何结论）

> 本节是 lead 在 T6 之前安排的准备性工作，全部在 `/tmp` 完成，**仓库里只动本文件**。
> §7/§8 是工具与快照，§9 是**新发现**（编号续 R-13 起）。

## §7 复跑框架：`/tmp/redteam-r3/verify/`

统一用法：`bash /tmp/redteam-r3/verify/<id>.sh <工作区路径>`
（不给路径则默认 `/home/jason/bg3-translate`；一键全跑：`bash .../verify/all.sh`）

脚本会把传入的工作区 **rsync 到沙箱**（`/tmp/redteam-r3/ws/after`，node_modules 走符号链接、
`--delete` 保证忠实）再灌探针跑，**绝不改动原工作区**；每份都跑两侧：

* `BEFORE` = `/tmp/rt`（内容与 HEAD `422f667` 逐字节一致，见 §1）
* `AFTER`  = 传入的工作区

判定：`红` = 缺陷复现，`绿` = 缺陷未复现，`未判定(编译失败)` = 探针没跑到
（**未判定不是绿**）。每份脚本会打印两侧的**完整原始输出**。

| id | 覆盖 | 判定依据（与修复手法无关） |
| --- | --- | --- |
| `R-01.sh` | 截断流：`finish_reason=length` 断流 + 无 `[DONE]` 断流 | 不得出现 `Done`（后一种只打印观察结果、不做断言） |
| `R-02.sh` | 属性值含裸 `>` / `</ b>` | `render` 产物必须能再解析且条目数不变 |
| `R-03.sh` | `file_name` 相对 + 绝对逃逸 | 工作目录之外不得出现被写入的文件 |
| `R-04.sh` | zip 炸弹（脚本现场用 python3 生成 64 MiB→65 KB） | 落地字节必须 < 64 MiB |
| `R-05.sh` | 多语言覆盖（前端） | 最终写进目标文件的文本里不得出现英文原文 |
| `R-10.sh` | 保真校验的属性（`Type=`→`类型=`） | `check_fidelity` 必须报出问题 |

**快照（采集时刻的工作区，仅供 T6 起点参考，不是 T6 结论）**：

```
R-01   BEFORE=红(缺陷复现)   AFTER=绿(缺陷未复现)
R-02   BEFORE=红(缺陷复现)   AFTER=绿(缺陷未复现)
R-03   BEFORE=红(缺陷复现)   AFTER=绿(缺陷未复现)
R-04   BEFORE=红(缺陷复现)   AFTER=红(缺陷复现)
R-05   BEFORE=红(缺陷复现)   AFTER=红(缺陷复现)
R-10   BEFORE=红(缺陷复现)   AFTER=绿(缺陷未复现)
```

⚠️ writer 当时仍在改代码（期间我还撞到过一次 `pak.rs` 编译不过的中间态，
框架如实判成 `未判定`），所以 **T6 必须重新跑一遍**，不能引用上面的快照。

两条 AFTER 输出的关键片段（供 T6 做对照基线）：

```
R-01 AFTER: Error{..., "大模型调用错误: 模型输出不完整（finish_reason=length），已丢弃这次半截译文…"}
            Error{..., "大模型调用错误: 流式响应结束，但既没有收到 [DONE] 也没有 finish_reason，无法确认输出完整（连接可能被中途掐断）"}
R-02 AFTER: R02A 产物 = …<LSTag Tooltip="a>b">fire</LSTag>…  再解析 error = None，条目数 = 1
            R02B 产物 = …译文 &lt;b&gt;x&lt;/ b&gt; 尾…        再解析 error = None，条目数 = 1
R-10 AFTER: issues = [MissingTag { tag: "<LSTag Type>" }, ExtraTag { tag: "<LSTag 类型>" }]
```

## §8 变异测试机制与负向自检

```
bash /tmp/redteam-r3/mut/revert.sh <工作区> <仓库相对路径>...   # 同步沙箱 + 把指定生产文件还原成 HEAD
bash /tmp/redteam-r3/mut/run.sh <test-bin> [测试名过滤]        # 在变异沙箱里跑探针
```

负向自检（证明「还原修复 ⇒ 探针确实变红」，即机制有效、探针敏感）：

```console
$ bash /tmp/redteam-r3/mut/revert.sh /home/jason/bg3-translate crates/bg3-translate-core/src/formats/content_list.rs
工作区     = /home/jason/bg3-translate
变异沙箱   = /tmp/redteam-r3/mut/ws
HEAD       = 422f667a0d5d3be26190c0f5b55b0796a67a7b76
还原 crates/bg3-translate-core/src/formats/content_list.rs                    -> HEAD 版本（25721 字节）
OK：变异沙箱已就绪。下一步：bash /tmp/redteam-r3/mut/run.sh <test-bin> [测试名过滤]

$ bash /tmp/redteam-r3/mut/run.sh v_r02
MUT=红(缺陷复现)
```

即：同一份探针，在**打了修复的工作区**上是绿（§7 的 `R-02 AFTER=绿`），
把 `content_list.rs` 还原成 HEAD 之后立刻变红。**机制可用**，T6 可以放心用它
逐个击穿 writer 的新回归测试。

## §9 追加发现：系列变体合成（`series.rs`）与任务分组（`planner.rs`）

这是 T5 未覆盖的角度。探针：`/tmp/redteam-r3/rt_probe7_series_planner.rs`
（11 个用例，全部走公开 API：`plan_jobs` / `TranslationEngine::with_translator`）。

```console
$ cp /tmp/redteam-r3/rt_probe7_series_planner.rs /tmp/rt/crates/bg3-translate-core/tests/rt_probe7.rs
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe7 -- --nocapture --test-threads=1
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
```

（这些用例是「观察型」的：结论来自打印出来的真实行为，所以我把原始输出直接贴在下面。
同一份探针在当前工作区上也跑过一遍，**11 条行为逐条相同**。）

| 编号 | 严重度 | 一句话 |
| --- | --- | --- |
| R-13 | 低 | 同一系列 base 的**独立条目**与**变体组**会各发一次请求，同一原文被请求两次 |
| R-14 | 低 | 后缀形态判定过宽：任何以 `{n}` / 纯数字 / `(9b)` 结尾的文本都被当成系列变体，普通句子被拆成「片段 + 后缀」翻译 |
| R-15 | 低 | 一致性记忆的 base 会被污染（`银发 9b 9b` → base 记成 `银发 9b`）；后缀大小写不一致时记忆直接丢失 |
| R-16 | 信息 | 合成用的分隔符是 ASCII 空格，会原样写进 PAK（`银发 9b`） |

### R-13（低）同一 base 被请求两次

```console
test p4_base_entry_plus_variants ... P4 请求原文 = ["Silver Hair", "Silver Hair"]
P4 Done = [("…#h1", "银发 9b"), ("…#h2", "银发 10"), ("…#h0", "银发")]
```

条目列表 = `Silver Hair`（base 本身待翻译）+ `Silver Hair 9b` + `Silver Hair 10`。
base 条目走 `Single` 任务、变体走 `Series` 任务，两者**请求的是同一段原文**。
影响：多一次 API 计费；更实际的风险是两次独立采样（`temperature` 默认 0.3）
可能给出**不同译名**，于是同一个 MOD 里出现「银发 9b」与「银色头发」并存。
（`plan_jobs` 的 `used_ids` 只保证条目不被重复覆盖，不保证原文不被重复请求；
`p9` 已验证覆盖数 = 待翻译数、无重复无遗漏。）

### R-14（低）后缀判定过宽：以 `{n}` 结尾的普通句子也会被合并

```console
test p7_suffix_shape_edge_cases ... P7 split_series_variant("Damage 10") = Some(SeriesVariant { base: "Damage", suffix: "10" })
P7 split_series_variant("Silver Hair {1}") = Some(SeriesVariant { base: "Silver Hair", suffix: "{1}" })
P7 split_series_variant("Silver Hair (9b)") = Some(SeriesVariant { base: "Silver Hair", suffix: "(9b)" })
P7 split_series_variant("Silver Hair #9b") = Some(SeriesVariant { base: "Silver Hair", suffix: "#9b" })
P7 split_series_variant("Plate Armor 2") = Some(SeriesVariant { base: "Plate Armor", suffix: "2" })
```

`is_variant_suffix` 把 `{1}` 这类占位符也算合法后缀（`{`/`}` 被 trim 掉后剩 `1`，
含数字、纯 ASCII 字母数字）。于是**只要两条文本共享前缀、且都以 `{n}` 结尾**，
就会被合并成一次请求，而模型只被要求翻译那个**残缺片段**：

```console
test p10_placeholder_ended_sentences_are_treated_as_series ... P10 计划 = TranslationPlan { total: 2, skipped: 0, jobs: 1 }
P10 分组 = Series(src="Deal {1} damage to"; …#h1←{2}, …#h2←{3})
P10 请求原文 = ["Deal {1} damage to"]
P10 Done = [("…#h1", "对目标造成{1}点伤害{2}"), ("…#h2", "对目标造成{1}点伤害{3}")]
```

`Deal {1} damage to {2}` / `Deal {1} damage to {3}` 这两条正常句子被拆成
「`Deal {1} damage to` + `{2}`」。因为合成是 `译文 + 后缀`，**后缀被强制留在句尾**，
中文语序调整的自由就没了：

```console
test p11_forced_suffix_position_costs_word_order_freedom ... P11 请求原文 = ["Talk to"]
P11 Done = [("…#h1", "与…交谈{1}"), ("…#h2", "与…交谈{2}")]
P11 对照：若整句交给模型，自然译文是「与{1}交谈」
```

（p11 的 base 译文「与…交谈」是我为说明问题手选的假译文；`P11 请求原文 = ["Talk to"]`
与「后缀被追加到句尾」这两件事是真实行为。）
影响：不弄坏 XML（`p5` 证明合成后仍逐成员过保真校验，占位符齐全），
但会静默降低译文质量，且用户无法关闭或察觉。

### R-15（低）一致性记忆的 base 会被污染 / 大小写不符即丢失

```console
P7 strip_target_suffix("银发 9b", "9b") = Some("银发")
P7 strip_target_suffix("银发 9B", "9b") = None
P7 strip_target_suffix("银发9b", "9b") = Some("银发")
P7 strip_target_suffix("银发 - 9b", "9b") = Some("银发")
P7 strip_target_suffix("9b", "9b") = None
P7 strip_target_suffix("银发 9b 9b", "9b") = Some("银发 9b")
```

* `银发 9b 9b` → base 记成 `银发 9b`：一旦某条译文多写了后缀，**污染会传播**给
  之后所有复用该 base 的变体（合成出「银发 9b 10」）。
* `银发 9B`（大小写与后缀不一致）→ `None` → base 不入记忆 → 后续变体全部重新请求
  （只损效率，不损数据）。

### R-16（信息）合成分隔符是 ASCII 空格，会原样进 PAK

```console
P6 compose("银发", "9b") = "银发 9b"（含 ASCII 空格 = true）
P6 compose("银发", "(9b)") = "银发(9b)"
P6 写进 XML 的文本 = <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">银发 9b</content></contentList>
P6 再解析 error = None
```

中文译文里会多出一个半角空格（以 ASCII 标点开头的后缀则不会）。纯排版问题。

### §9 附：同一批探针**证伪**的怀疑点（同样有真实输出）

| 怀疑 | 结论 | 证据 |
| --- | --- | --- |
| 变体↔后缀配对会错位（「串味」） | 证伪 | `P1 Done = [("…#h1", "银发 10"), ("…#h2", "银发 9b")]`，且 `P1 请求原文 = ["Silver Hair"]`（两个变体只发一次请求） |
| 条目顺序错乱会让记忆复用失效 | 证伪 | `P2 请求原文（应为空）= []`，`P2 Done = [("…#h3", "银发 10"), ("…#h2", "银发 9a")]` |
| 只有一个变体时会被错误地按系列合并 | 证伪 | `P3 请求原文 = ["Silver Hair 10"]`（整条翻译，`Series` 分支要求 ≥2 成员） |
| 合成后的译文不再过保真校验 | 证伪 | `P5A Done = ["银发{1}", "银发{2}"]`（正常）；`P5B` 让 base 多出 `{0}` → 两条都 `Error(占位符 {0} 多出（已重试 1 次）)`，即**逐成员校验生效** |
| 同 contentuid 的中英双语 base 会被当成两个系列 | 证伪 | `P8 请求原文（期望为空）= []`，`P8 Done = [("…#h1","银发 9b"), ("…#h2","银发 10")]`（别名归并 + 记忆复用都生效） |
| 计划会漏条目或重复覆盖 | 证伪 | `P9 覆盖条数 = 5 / 待翻译 = 5 / 有重复 = false` |

### §9 附：一条「只修了一半」的观察（供 T6 用）

`R-10` 的 AFTER 已经绿，但 `v_r10.rs` 里我加的两条**不计入判定**的观察用例显示，
T2 的修复只把属性**名**纳入签名，属性的**值**仍然不参与比较：

```console
R10B(不计入判定) 丢掉全部属性时 issues = [MissingTag { tag: "<LSTag Tooltip Type>", count: 1 }, ExtraTag { tag: "<LSTag>", count: 1 }]
R10C(不计入判定) 只互换属性值时 issues = []
```

`R10C` 用的是「两个同类标签互换 `Tooltip="Fire"` / `Tooltip="Ice"`」——
签名里只剩 `<LSTag Tooltip>`，互换检测不到（与我在 §2 R-10 里描述的情形一致，
只是范围缩小到属性值）。这不算新缺陷，但**T6 复核 writer 的「已修」声明时必须写上这个边界**。

## §10 追加（第二轮准备）：R-05 探针口径修正、T6 判定口径、R-14 定量

### §10.1 R-05 的探针原来会误判「未修」——已改

lead 指出：frontend 的 R-05 修复**刻意没有改 `planLocalizationWrites` 的返回语义**
（plan 仍让英文胜出，这是设计），真正的合并发生在
`src/App.tsx::onGoPack` → `readFileEntries(workDir, plan.fileName)` 取底稿 →
`mergeWithExistingTarget(incoming, existing)`（`src/lib/localization.ts:115`）。
我原来的 `R-05.sh` 只直调 `planLocalizationWrites`，**在修复后的工作区上仍然是红**，
会造成 T6 误判。现在 `R-05.sh` 改成三部分：

| 部分 | 探针 | 作用 | 是否判定 |
| --- | --- | --- | --- |
| PART1 | `probes/v_r05_app.test.tsx`（App 级 vitest，驱动 `onGoPack` 并检查 `write_file_entries` 的真实 payload） | **判定依据** | 是 |
| PART2 | `probes/v_r05_merge.ts`（纯函数 `mergeWithExistingTarget` 的 8 条对抗式检查） | 附证 | 否（单独打印） |
| PART3 | `probes/v_r05.ts`（原 `planLocalizationWrites` 直调） | **对照项，不再代表最终行为** | 否 |

PART2 的 8 条检查（`mergeWithExistingTarget` 的真实输出）：

```console
$ cd <工作区> && bun run v_r05_merge.ts
PASS C1 未翻译保留底稿中文 :: 落盘 = "火球"（期望 火球）
PASS C2 已翻译用新译文 :: 落盘 = "火球术"（期望 火球术）
PASS C3 error 不覆盖底稿 :: 落盘 = "火球"（期望 火球，且不含被拒译文）
PASS C4 底稿独有条目保留 :: contentuid = ["uid-1","uid-9"]（期望含 uid-9）
PASS C5 底稿为空时退回 incoming :: 落盘 = "Fireball"（期望 Fireball）
PASS C6 translating 半截不覆盖底稿 :: 落盘 = "火球"（期望 火球）
PASS C7 底稿重复 uid 取第一条 :: 落盘 = "甲"（期望 甲）
PASS C8 顺序 incoming 在前 :: 顺序 = uid-1,uid-2,uid-9
INFO C9 底稿里没有的新条目落盘 = "Brand New"（无底稿可选，属预期）
R05-MERGE 判定: 绿（全部检查通过）
```

PART1 的真实判定（App 级）：

```console
$ bash /tmp/redteam-r3/verify/R-05.sh /home/jason/bg3-translate
BEFORE=红(缺陷复现)            ← /tmp/rt = HEAD：payload = ["Fireball","Ice"]
BEFORE(纯函数)=未判定(探针无法运行)   ← HEAD 上 mergeWithExistingTarget 不存在
BEFORE(对照-plan)=红
AFTER(纯函数)=绿
AFTER=绿(缺陷未复现)           ← 当前工作区：payload = ["火球","寒冰"]
AFTER(对照-plan)=红            ← 对照项，按 lead 裁定不算缺陷
```

HEAD 侧 5 个 App 用例里 4 红 1 绿（绿的那条是只断言文件名的信息项），
即 App 级探针在修复前确实是红的 —— 判定口径正确。

一条 **INFO（降级取舍，不是缺陷）**：当目标文件确实存在于 MOD、但 `read_file_entries`
读取失败时，`onGoPack` 会退回 `plan.entries` 并只打一条 `console.warn`，
此时未翻译条目会以英文原文写回中文文件：

```console
R05-APP [底稿读失败] texts = ["Fireball","Ice"] （读到失败时退回 plan.entries = 英文原文；这是 App 注释里写明的降级取舍）
```

行为与注释一致，我按「已声明的取舍」记录，供 T6 决定是否需要更严的策略。

### §10.2 T6 判定口径（按 lead 裁定，避免误读）

* **R-04（zip 炸弹）**：format-auditor 仍在改 `pak.rs`，我最近一次跑仍是红 —— 那是**预期中间态**。
  T6 重跑时若仍红才算缺陷；若变绿，按 §8 的变异机制还原 `pak.rs` 复核「修复前红」。
* **R-10 的属性值**：**属性名被改**与**丢属性**已能检出；**属性值互换仍判保真**是
  **lead 明确裁定的口径（属性值放宽）**，在 T6 报告里记「按设计」，**不算 writer 的未修项**。
  `R10C` 保留为观察用例，只作边界说明。
* 其余按原判据：`红=缺陷复现 / 绿=缺陷未复现 / 未判定(编译失败) 不算绿`。

### §10.3 R-14 定量：真实 MOD 文本里到底有多常见

探针 `/tmp/redteam-r3/rt_probe8_r14_census.rs`：把仓库里**唯一一份真实 MOD 样本**
（`samples/Appearance Edit Enhanced-…zip`）全部可翻译条目取出来，逐条喂给**真正的**
`split_series_variant`：

```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe8 -- --nocapture
R14 ===== 真实样本 MOD 的原文总数 = 41 =====
R14 以 {n} 占位符结尾的原文 = 0 / 41（0.0%）
R14 被判为系列变体的原文 = 0 / 41（0.0%）
R14 会被真正合并（同 base ≥2 成员）的组数 = 0
```

**结论：在这份真实样本里，R-14 的触发次数是 0。** 我把 41 条原文全部打印出来核对过
（`R14   [文件] "原文"`，见探针输出），它们都是短 UI 文案，既没有以 `{n}` 结尾的，
也没有以裸数字结尾的，因此系列变体分支**根本没被走到**。

诚实说明与风险边界：
* 这份样本只有 41 条、且是纯 UI MOD；官方剧情/物品文本（动辄上千条、占位符密集）
  **不在样本里**，所以不能据此断言线上频率同样是 0。
* 触发 R-14 需要**两个条件同时成立**：①同一文件里有 ≥2 条文本共享同一个 base 前缀；
  ②它们各自以不同的短后缀（`{n}` / 纯数字 / `#x` / `(x)`）结尾。这是较强的巧合条件。
* 弱信号（**合成语料，不是真实数据**）：仓库自带的测试语料里存在
  `"Silver's Hair {1}"` / `"Silver's Hair {2}"` 这种成对的、以 `{n}` 结尾的字符串
  （`grep -rhoE '"[^"]{6,}[^"]*\{[0-9]+\}"' crates src | sort -u` 共 20 条）。
  一旦真实 MOD 出现这种成对文本，就会被合并。

后果边界也已验证——**不会弄坏 XML**（占位符完整、文件仍可解析）：

```console
R14 合成译文 = "对目标造成{1}点伤害{2}"
R14 XML = <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">对目标造成{1}点伤害{2}</content></contentList>
R14 再解析 error = None，读回 source = Some("对目标造成{1}点伤害{2}")
R14 占位符是否完整保留 = true
```

所以 R-14 的真实代价是**译文质量**（残缺片段被单独翻译 + 后缀被强制留在句尾），
不是数据损坏。**维持「低」**，但建议 T6 在报告里注明「频率未能在真实数据上得到正例，
只有构造样例；触发需要同前缀成对文本」。

### §10.4 本轮新增/改动的探针文件（都在 /tmp）

| 文件 | 用途 |
| --- | --- |
| `/tmp/redteam-r3/verify/probes/v_r05_app.test.tsx` | R-05 PART1，App 级判定 |
| `/tmp/redteam-r3/verify/probes/v_r05_merge.ts` | R-05 PART2，纯函数 8 条检查 |
| `/tmp/redteam-r3/verify/probes/v_r05.ts` | R-05 PART3，对照项（已加「不再代表最终行为」标注） |
| `/tmp/redteam-r3/rt_probe8_r14_census.rs` | R-14 真实样本普查 + 后果边界 |
| `/tmp/redteam-r3/verify/R-05.sh` | 已改写为三部分结构 |

（§1~§6 与 §7~§9 的原有结论一律未改动。）
