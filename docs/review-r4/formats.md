# 第四轮审查报告：Rust 归档与本地化格式层（formats / pak / loca / content_list）

- 审查者：`auditor-formats`（R4-T1）
- 基线：`HEAD = d8295da`（= tag `v1.1.5`），开工时工作区 hash `ad513e30c1e335f2a1f9cd669f6aa7ed`（与 lead 冻结值一致）
- 写范围：`crates/bg3-translate-core/src/formats/**`、`crates/bg3-translate-core/src/pak.rs`、`crates/bg3-translate-core/tests/**`、`docs/review-r4/formats.md`
- 实际改动：`formats/content_list.rs`、`formats/loca.rs`、`pak.rs`（`formats/lsx.rs`、`formats/mod.rs`、`tests/**` 一字未改）
- 本轮新增回归测试：**20 条**（content_list 6 / loca 10 / pak 4），未删除、未削弱任何既有测试
- 结论一句话：**格式层有 2 条高危（会产出非法 XML / 让进程 abort）+ 4 条中危（静默丢条目、失败时摧毁已有产物）**，全部按「先写失败测试 → 再改代码」修掉；另 4 条低危收口；§4 列出 **8 条**已确认未修（含 3 条第三轮遗留取舍的复核）+ §5 列出 **6 类**无法验证。15 组变异实验全部按预期变红（含 1 组「撤回循环边界 → 测试挂死」）。

---

## §1 结论摘要表

| 编号 | 级别 | 状态 | 一句话 | 归属 |
|---|---|---|---|---|
| F-R4-01 | **高** | 已修 | 属性之间缺空白的开始标签（`<LSTag a="1"b="2">`）被当成合法标签**原样写出** → 产物对严格解析器是非法 XML（独立验证：python expat 拒绝整份文档） | 本轮新发现 |
| F-R4-02 | **高** | 已修 | 伪造 `.loca` 头（`num_entries=0xFFFFFFFF`）→ 上游 `Vec::with_capacity(42 亿)` → **137 GB 分配失败 → 进程 abort**（不可捕获） | 本轮新发现（lead 转来后确认） |
| F-R4-03 | 中 | 已修 | `content_list::write` 把「目标存在但读不出来」当「新文件」→ 静默删条目且返回 `Ok` | 红队 F-A1 |
| F-R4-04 | 中 | 已修 | `content_list::write` 忽略 `parsed.error`，用残缺底稿整体重写 → 出错点之后的条目被永久删除且返回 `Ok` | 红队 F-A2（= CORPUS-AUDIT §D8 订正） |
| F-R4-05 | 中 | 已修 | `loca::write` 非原子：`File::create` 先截断再校验 → 写失败把已有 `.loca` 从 160 字节变成 **0 字节** | 本轮新发现 |
| F-R4-06 | 中 | 已修 | `repack` 失败会摧毁 `output_path` 上已有的文件（`PackageBuilder::build` 第一步就截断），实测留下 0 字节 / 半个 pak | 本轮新发现 |
| F-R4-07 | 低 | 已修 | 属性值里的非法字符引用（`&#1;`）被解码成**裸控制字符**写进标签 → 产物非法 XML（F-07 在属性这条路径上的复现） | 本轮新发现 |
| F-R4-08 | 低 | 已修 | zip「实际写入」兜底层只按总量预算 `take` → 声明 1 字节的条目可写出 16 GiB，单文件上限形同虚设 | 本轮新发现 |
| F-R4-09 | 低 | 已修 | `loca::write` 对「存在但读不出来 / 布局不可信」的底稿默认整体重写 → 收口为中止报错（0 字节文件例外） | lead 要求（F-A1 同类） |
| F-R4-10 | 低 | 已修 | `detect_language_from_path` 不认 `Localization/Chinese/` → 前端 `CHINESE_LANGUAGE_ALIASES` 的 `"Chinese"` 分支是死代码，本工具自己产出的中文文件下一轮掉进「未知语言」（写回优先级 3 英文 > 1 未知） | lead 转来的观察 ④ |
| — | 信息 | 过程中自曝 | 我在实现 F-R4-01 的第一版里引入了**无界循环**（引号未闭合的候选串会死循环），被自己的标签探针 900s 超时抓到，当场修掉并补了守卫测试 | 见 §5 |

---

## §2 缺陷详情

### F-R4-01（高）属性间缺空白的标签被原样写出 → 产物非法 XML

**现象**：`parse_single_tag` 用 quick-xml 做「是不是合法标签」的判定，而 quick-xml 在**属性语法**上比 XML 1.0 宽松：`<LSTag a="1"b="2">`（属性之间没有空白）它能解析成两个属性，于是旧实现把它判成合法标签、由 `write_text_fragment` **裸写**进产物。XML 1.0 的 `STag ::= '<' Name (S Attribute)* S? '>'` 要求属性之间有 `S`，严格解析器（游戏侧 / python expat）会拒绝**整份文档** → 丢的是整份译文。触发面是**译文侧**（模型/用户编辑），源文件侧的同类形态会被 `raw_start_tag` 规范化掉。

**复现（修复前，探针 `/tmp/fa-probe/src/bin/r4_tags.rs`）**：

```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin r4_tags      # 修复前
## attr_no_space            allowed=true  balanced=true  raw_write=true  body="<LSTag a=\"1\"b=\"2\">x</LSTag>"
## attr_single_quote        allowed=true  balanced=true  raw_write=true  body="<LSTag a='1'>x</LSTag>"
...
$ cd /tmp/r4f_tags && python3 -c "import glob,xml.dom.minidom
for f in sorted(glob.glob('*.xml')):
    try: xml.dom.minidom.parse(f)
    except Exception as e: print('ILLEGAL',f,e)"
ILLEGAL attr_no_space.xml: not well-formed (invalid token): line 1, column 100
ILLEGAL attr_illegal_charref.xml: not well-formed (invalid token): line 1, column 98
```

**修复后（同一探针、探针源码一字未改）**：

```console
## attr_no_space       allowed=false balanced=false raw_write=false body="&lt;LSTag a=\"1\"b=\"2\"&gt;x&lt;/LSTag&gt;"
## attr_space_around_eq allowed=true  raw_write=true  body="<LSTag a = \"1\">x</LSTag>"   # 合法形态照旧
## attr_single_quote    allowed=true  raw_write=true  body="<LSTag a='1'>x</LSTag>"
## attr_newline         allowed=true  raw_write=true  body="<LSTag a=\"1\"\n    b=\"2\">x</LSTag>"
$ cd /tmp/r4f_tags && python3 …（expat 校验）
文件数= 21  非法数= 0
```

**修法**（`formats/content_list.rs::parse_single_tag`）：把「是不是合法标签」从 quick-xml 的宽松解析换成**自己按 XML 1.0 文法走一遍**：
`STag`/`EmptyElemTag`/`ETag` + `Attribute ::= Name Eq AttValue`，属性之间必须有空白、同名属性只能出现一次；名字用 `NameStartChar`/`NameChar` 全集（含非 ASCII），所以 `<LSTag 类型="x">` 这类合法标签**不会**被误伤。
只拒绝非法、不收紧宽容度：`<br>`、`<br />`、`<br  >`、单引号、`a = "1"`、换行分隔、属性换序、值里的 `>` 与实体全部照旧保留为真标签（见 `equivalent_tag_spellings_are_still_written_as_markup` 与新增用例的正向对照）。

**回归测试**：`formats::content_list::tests::tag_without_whitespace_between_attributes_is_never_written_as_markup`
**变异实验**：M1（去掉「必须有空白」判定）→ 变红。
**残留风险**：白名单外标签仍按纯文本转义（既有设计）；`&#0;` / 未定义实体在属性值里由 `repair_tag_attributes` 转义，行为未变。

---

### F-R4-02（高）伪造 `.loca` 头让整个进程 abort

**现象**：`.loca` 来自用户的 MOD，是不可信输入。上游 `LocaReader::read_entries` 直接 `Vec::with_capacity(num_entries)`。12 字节的伪造文件（签名 `LOCC` + `num_entries=0xFFFFFFFF` + `texts_offset=12`）让进程申请 **137 GB**；Rust 的分配失败是 `handle_alloc_error` → **abort**（不是可捕获的 `Err`），整个应用连同用户未保存的进度一起死。

**复现（修复前，探针 `/tmp/fa-probe/src/bin/r4_loca.rs`）**：

```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin r4_loca     # 修复前
## huge_num_entries: 开始 read（可能 abort）…
memory allocation of 137438953440 bytes failed
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

**修复后（同一探针）**：

```console
## huge_num_entries:  read = Err(Loca(InvalidFormat(".loca 布局非法（条目数 4294967295 需要 300647710662 字节的索引表，但文件只有 12 字节），拒绝解析: …/bomb.loca")))
## huge_texts_offset:  read = Err(Loca(InvalidFormat(".loca 布局非法（文本区偏移 4294967295 越界（文件只有 12 字节）），拒绝解析: …")))
## huge_entry_length:  read = Err(Loca(InvalidFormat(".loca 布局非法（文本区声称 4294967295 字节（起点 82），但文件只有 84 字节），拒绝解析: …")))
```

**修法**：`formats/loca.rs` 新增 `layout_problem()`，在把字节交给上游之前做布局自检（判定表见 §3）。所有算术都是 `u64` + `checked_*`，索引换算用 `usize::try_from`，无新增 `unwrap/expect/panic`。
**回归测试**：`formats::loca::tests::lying_entry_count_is_rejected_without_a_giant_allocation`、`lying_offsets_and_lengths_are_rejected`
**变异实验**：M6（去掉自检）→ 进程 abort（红）；M10（放宽：去掉索引表检查）→ 谎报用例变红；M11/M12（收紧自检）→ 正向用例变红。
**残留风险**：本机**没有真实游戏 `.loca` 语料**（见 §5），自检只按「上游 `read_exact` 是否可能成功」推导，采用保守策略（只挡物理上不可能）。

---

### F-R4-03（中）`content_list::write` 把「存在但读不出来」当新文件（红队 F-A1）

**现象**：`write` 里 `match std::fs::read(path) { Ok(..) => .., Err(_) => (false, Vec::new(), Escaped, Vec::new()) }`。
`rename` 只需目录写权限、读文件要文件读权限，两者可分离（Windows 上被占用 / 杀软扫描 / ACL 同样如此）。「文件在磁盘上、只是读不到」被当成新文件 → 没提交的条目被静默删掉，且返回 `Ok(())`。

**复现（修复前，探针 `/tmp/fa-probe/src/bin/r4_write.rs`）**：

```console
## locked: 当前用户仍可读? false（false 才能测）
## locked: write -> Ok(())
## locked: before_len=165 after_len=118 内容丢失? true
   after = "<?xml …?><contentList><content contentuid=\"h1\" version=\"1\">甲改</content></contentList>"
```

**修复后**：

```console
## locked: write -> Err(Io(Custom { kind: PermissionDenied, error: "写回目标存在但读不出来（Permission denied (os error 13)），已中止写回以免覆盖磁盘上的条目: …/locked.xml" }))
## locked: before_len=165 after_len=165 内容丢失? false
```

**修法**：只有 `ErrorKind::NotFound` 走「新建文件」语义；其它 IO 错误返回 `AppError::Io`（code 仍是 `io`，前端按 `String(e)` 展示，文案里写明「为什么中止」）。`loca` 的同类模式一并收口（F-R4-09）。
**回归测试**：`formats::content_list::tests::write_refuses_when_the_target_exists_but_cannot_be_read`、正向对照 `write_still_creates_a_target_that_does_not_exist`
**变异实验**：M3（把该分支改回 `Err(_)`）→ 变红。

---

### F-R4-04（中）`content_list::write` 忽略 `parsed.error`，用残缺底稿整体重写（红队 F-A2）

**现象**：`write` 读底稿时只看 `parsed.entries`，不看 `parsed.error`。底稿解析中断时拿到的是**残缺**列表 —— 正好是 `read` 的文档里警告过的那个场景（「带着残缺列表去写，畸形标签之后的所有原文都会被永久删掉」）。写回后出错点之后**完全合法**的条目消失，且返回 `Ok(())`。

**复现（修复前）**：

```console
## corrupt[raw_amp]: parse_error=Some("ill-formed document: entity or character reference not closed: `;` not found before end of input")
   parsed_ids=["h1"]   write=Ok(())      after_ids=["h1"]     ← h2/h3 被永久删除
## corrupt[raw_lt]:  parse_error=Some("ill-formed document: expected `</>`, but `</contentList>` was found")
   parsed_ids=["h1"]   write=Ok(())      after_ids=["h1"]
```

**修复后**：

```console
## corrupt[raw_amp]: write=Err(Xml("写回目标 …/corrupt.xml 解析失败，已中止写回以免丢失条目（请先修好这个文件，或删掉它重新生成）: ill-formed document: …"))
## corrupt[raw_lt]:  write=Err(Xml("… 解析失败，已中止写回以免丢失条目 …: ill-formed document: expected `</>`, but `</contentList>` was found"))
```

**关于 `parsed.error` 的语义**（lead 要求先弄清）：`parse()` 只在两处置 `error` —— ① `Eof` 时 `pending.is_some()`（文档在 `<content>` 内被截断）；② quick-xml 返回 `Err`。**两者都是硬失败**（文档没有完整读下来），不存在「容忍性 warning」混用；`&nbsp;` 这类未知实体走 `GeneralRef` 事件、`error` 保持 `None`（探针实测：`unknown_entity` 用例 `reparse_error=None`，写回稳定）。因此判定就是「硬失败才中止」，不会出现「MOD 有轻微怪癖就永远写不回去」。
**修法**：`parsed.error.is_some()` → 返回 `AppError::xml` 并**不落盘**（与 `read` 的既有政策一致）。
**回归测试**：`formats::content_list::tests::write_refuses_when_the_target_is_only_partially_parsable`
**变异实验**：M4（删掉该检查）→ 变红。
**文档订正**：`docs/CORPUS-AUDIT.md` §D8（lead 批准）已把「当前不可达」改为可达 + 已修，并写清两条新证据。

---

### F-R4-05（中）`loca::write` 非原子：写失败把已有 `.loca` 清成 0 字节

**现象**：`loca::write` 调上游 `LocaUtils::save_with_format`，而它的第一步是 `File::create(path)`（**截断**），随后 `LocaWriter::write` 才校验 key 长度并在写入过程中处理 IO 错误。任何失败（key > 64 字节、磁盘写满、进程被杀）都会留下 0 字节 / 半截文件 —— 用户整份已有译文没了。同模块的 `content_list` / `lsx` 早就走 `write_atomic`，只有 `.loca` 不是。

**复现（修复前）**：

```console
## long_key: write=Err(Loca(KeyTooLong { key: "kkk…k", length: 65 }))
   before_len=160 after_len=0 原译文是否被摧毁=true
```

**修复后**：

```console
## long_key: write=Err(Loca(KeyTooLong { … }))
   before_len=160 after_len=160 原译文是否被摧毁=false
```

**修法**：先 `LocaUtils::save_to_writer(&resource, &mut Vec<u8>, LocaFormat::Loca)` 完整序列化到内存（失败时磁盘一字未动），再 `config::write_atomic` 落盘（临时文件 + rename），与另外两种格式一致。
**回归测试**：`formats::loca::tests::failed_write_does_not_destroy_the_existing_loca`
**变异实验**：M5（改回 `save_with_format`）→ 变红（`写回失败时原文件必须逐字节不变`）。
**残留风险**：序列化会把整份 `.loca` 放进内存（真实文件量级 MB～几十 MB）。原实现是流式写盘，但代价是「失败即毁文件」；`.lsx` / `content_list` 已经是内存方案，取舍一致。

---

### F-R4-06（中）`repack` 失败摧毁 `output_path` 上已有的文件

**现象**：`PackageBuilder::build(output)` 的第一步就是 `File::create(output)`（截断），之后才逐个 `File::open` 源文件。任何一步失败（源文件读不出来 / 磁盘写满）都让用户原来放在那个路径上的产物变成 0 字节或**半个 pak**，而「导出覆盖上一次的结果」是最常见的用法。

**复现（修复前，探针 `/tmp/fa-probe/src/bin/r4_pak.rs`：`unpacked/` 里放一个 chmod 000 的文件让 `build` 中途失败，output 先放一份「用户已有产物」）**：

```console
## before: output_len=32
## repack -> Err(Pak("打包失败: I/O error: Permission denied (os error 13)"))
   after_len=0 内容仍是原来那份? false  变成 0 字节? true
```

（crate 内回归测试里 output 被写成了 54 字节的**半个 pak**：`left: [76, 83, 80, 75, …]` = `LSPK` 头 + 一个条目，`right:` 是原来的 32 字节用户数据。）

**修复后（同一探针）**：

```console
## repack -> Err(Pak("打包失败: I/O error: Permission denied (os error 13)"))
   after_len=36 内容仍是原来那份? true  变成 0 字节? false
## repack(ok) -> Ok(()) exists=true len=138
   tmp dir entries = ["Existing.pak", "Out.pak", "work"]      # 无临时文件残留
```

**修法**：先打到**同目录**下的隐藏临时文件（`.<名>.bg3-translate-<pid>-<seq>.tmp`），成功后 `std::fs::rename` 覆盖目标；失败/rename 失败都删掉临时文件并返回 `pak` 错误。同目录保证不跨文件系统（rename 原子）。
**回归测试**：`pak::tests::failed_repack_does_not_destroy_an_existing_output_file`（含成功路径的「无临时文件残留」与「产物可再次打开」断言）
**变异实验**：M9（改回直接 `build(output)`）→ 变红。
**残留风险**：rename 覆盖目标时 Windows 上依赖 `MOVEFILE_REPLACE_EXISTING`（Rust std 的行为，与既有 `write_atomic` 相同）；本机 Linux 无法实测 Windows 分支。

---

### F-R4-07（低）属性值里的非法字符引用解出裸控制字符

**现象**：`repair_tag_attributes` 会把属性值做一次「实体还原 → 转义」。`&#1;` / `&#xFFFF;` 这类**非法字符引用**（XML 1.0 的 `Char` 产生式不允许，连字符引用都不合法）会被解码规则解成字符（规则只拒绝「解析不出来」的引用），随后 `escape_attribute_value` 不处理控制字符 → 裸 `0x01` 落盘 → 整份文件非法。

**复现（修复前）**：`/tmp/r4f_tags/attr_illegal_charref.xml` 被 expat 拒绝（`not well-formed (invalid token): line 1, column 98`），探针 body 里直接是 `\u{1}`。
**修复后**：探针 body = `<LSTag a="">x</LSTag>`（非法字符按全文件统一政策丢弃），21 个产物文件 expat 全部通过。
**修法**：`repair_tag_attributes` 里解码后套一层既有的 `sanitize_xml_chars`（F-07 的政策不变）。
**回归测试**：`formats::content_list::tests::illegal_char_reference_in_tag_attribute_never_reaches_the_file`
**变异实验**：M2（删掉该行）→ 变红。
**残留风险**：触发面很窄（需要模型吐出 `&#1;` 这类引用）；`&#0;`、`&#xZZ;` 这类解不出来的引用本来就已被 `repair_tag_attributes` 转义（探针实测 `&amp;#xZZ;`，合法）。

---

### F-R4-08（低）zip 的「实际写入」兜底只按总量预算

**现象**：防线第 ③ 层（实际写入字节，防声明撒谎）用 `budget = max_total_bytes - written_total` 做 `Read::take`。声明大小撒谎时，单条目上限只对**声明值**生效 → 一个声明 1 字节的条目能把整份总量预算（默认 16 GiB）写进同一个文件，单文件上限（默认 6 GiB）形同虚设。

**复现（修复前，crate 内注入小上限的用例）**：`unwrap_err()` 拿到的是「zip 内未找到 .pak 文件」（解压"成功"），`bomb.bin` 留下 4 MiB。
**修复后**：`budget = min(剩余总量, max_entry_bytes)`，并分别用「单文件上限」与「总量上限」两条独立错误信息收尾（同时删除半成品文件）。
**回归测试**：`pak::tests::zip_entry_that_lies_about_its_size_cannot_exceed_the_per_entry_limit`（断言错误信息来自「单文件上限」这一层 + 磁盘上不留超过上限的文件）
**变异实验**：M8（只看总量并跳过单条目判定）→ 变红。

---

### F-R4-09（低）`loca::write` 对不可信底稿默认整体重写 → 收口为中止

**现象**（lead 要求「同类模式一并评估」）：旧 `merge_with_existing` 在 `LocaUtils::load` 失败时只 `log::warn!` 然后整体重写；读失败（EACCES）也走同一条分支。布局自检如果误判，重写就是不可逆的数据损失。
**修法**（与 F-R4-03 同一语义）：
1. `NotFound` → 真新文件；
2. 其它读失败 → `Err`（「写回目标存在但读不出来…已中止写回」）；
3. **0 字节文件** → 按新建处理（0 字节**没有任何可读内容**，这一点不需要推断，避免把用户卡死）；
4. 布局非法 / 上游解析失败 → `Err`（「…已中止写回以免丢掉磁盘上的译文；确认这个文件损坏、想用当前译文重建它时请先删除它」）。
**回归测试**：`formats::loca::tests::suspicious_target_aborts_but_zero_byte_target_is_rebuilt`（含 0 字节例外、非空垃圾文件、布局自洽但 key 非法 UTF-8 三种）
**变异实验**：M7（读失败降级成重写）、M14（布局不可信时重写）→ 均变红。
**反例分析（必须保留重写的场景）**：只剩「0 字节文件」与「不存在」两种，二者都不含任何可读内容；其它情况一律中止，代价是用户要先删掉损坏文件才能重建（错误文案里直接写了这一步）。

---

### F-R4-10（低）`Localization/Chinese/` 不被认成中文

**现象**：上游 `detect_language_from_path` 只认 BG3 官方 14 个语言名（`ChineseSimplified` / `ChineseTraditional`，**不含 `Chinese`**）。而本工具写回中文用的目录名就是 `Chinese`（前端 `TARGET_LANGUAGE`）：

```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin r4_lang
## Localization/English/a.xml              -> Some("English")
## Localization/Chinese/a.xml              -> None            ← 本工具自己的产物
## Localization/ChineseSimplified/a.xml    -> Some("ChineseSimplified")
## Localization/Polish/a.xml               -> Some("Polish")
## 真实样本（samples/Appearance Edit Enhanced…zip）的本地化文件：
   Localization/English/AppearanceEditEnhanced.xml   lang=Some("English")
   Localization/Polish/AppearanceEditEnhanced.xml    lang=Some("Polish")
```

后果：前端 `CHINESE_LANGUAGE_ALIASES` 里的 `"Chinese"` 分支**永远不可能命中**（死代码）；上一轮产出的 `Localization/Chinese/x.xml` 在下一轮被算成「未知语言」，`localizationWritePriority` 给出 1（与波兰语同级），同一目标路径上英文（3）会胜出。
**修法**（`pak.rs`，本范围内）：`to_pak_file` 改走新的 `detect_language()` —— 先调上游，再补一条本工具自己的目录约定（`Localization/Chinese/`，大小写不敏感、`\` 与 `/` 都认）。前端 `CHINESE_LANGUAGE_ALIASES` 原样就能命中，**不需要**改前端。
**回归测试**：`pak::tests::chinese_target_directory_is_recognized_as_chinese`、`build_pak_files_reports_chinese_for_our_target_directory`（后者走 `PackageBuilder → Package::open → build_pak_files`，把 `to_pak_file` 的接线也钉住）
**变异实验**：把 `to_pak_file` 改回只调上游函数 → 第二条用例变红。
**无法验证**：BG3 真实中文目录名（见 §5）。本修复只保证「我们自己产出的目录在下一轮被认成中文」这一内部一致性；若游戏实际只认 `ChineseSimplified`，那是前端 `TARGET_LANGUAGE` 的产品决策，需要 lead 裁定（跨范围）。
**残留风险**：如果某个 MOD 恰好有个 `Localization/Chinese/` 目录但不是本地化文件，它的语言字段会显示 `Chinese`（此前是 `None`）——只影响写回优先级排序，不影响读写路径。

---

## §3 `.loca` 布局自检：逐条判定表（lead 要求）

上游行为（`bg3rustpaklib 0.1.5/src/loca/reader.rs`，与 `/tmp/paklib` 那份逐字节一致）：头 12 字节（signature / num_entries / texts_offset）→ 逐条 `read_exact` **固定 70 字节**（key 64 + version u16 + length u32，`ENTRY_SIZE`/`HEADER_SIZE` 是常量，**与 version 字段取值无关**，全文件仅 `reader.rs:51` 与 `writer.rs:35` 两处使用）→ `if texts_offset > bytes_read { skip }`（**否则原地不动、不回退**）→ 逐条 `read_exact(length)`。

| # | 判定 | 依据 | 只拒绝「物理不可能」？ | 正向用例 | 反例（谎报）用例 |
|---|---|---|---|---|---|
| ① | `文件长度 ≥ 12` | 上游 `read_exact(12)` 必失败 | 是 | `layout_check_accepts_files_we_wrote` | `suspicious_target_aborts_but_zero_byte_target_is_rebuilt`（非空垃圾文件写回中止） |
| ② | `签名 == LOCA_SIGNATURE` | 上游第一个检查 | 是 | 同上 | 同上 |
| ③ | `12 + 70·n ≤ 文件长度` | 上游逐条 `read_exact(70)` 必失败 | 是 | `versions_do_not_affect_the_layout_check`（v1/v2/65535 各一条） | `lying_entry_count_is_rejected_without_a_giant_allocation`（`n=0xFFFFFFFF`，12 字节文件） |
| ④ | `texts_offset ≤ 文件长度` | 该偏移只用于 skip，越界时 skip 的 `read_exact` 必失败 | 是 | `texts_offset_below_the_table_is_still_readable`（**刻意不要求 ≥ 表尾**） | `lying_offsets_and_lengths_are_rejected` ① |
| ⑤ | `max(texts_offset, 12+70n) + Σlength ≤ 文件长度` | 文本区逐条 `read_exact(length)` 必失败 | 是 | `texts_offset_beyond_the_table_is_skipped_and_still_readable`（表尾与文本区之间留空隙）、`empty_text_and_empty_table_are_readable`（`length=0`、`n=0` 只有头） | `lying_offsets_and_lengths_are_rejected` ②③ |

**刻意不做的事**：不要求 `texts_offset ≥ 12 + 70n`（上游对更小的值原地不动、照读不误，`texts_offset = 0` 是**合法**文件）。
**实现约束**：全部算术在 `u64` 上做，乘法/加法一律 `checked_*`，表项下标用 `usize::try_from` —— 32 位平台或溢出都不可能绕过。
**无界分配都被压到文件大小以内**：`Vec::with_capacity(n)`（③ 后 `n ≤ 文件长度/70`）、`vec![0u8; texts_offset - bytes_read]`（④/⑤）、`vec![0u8; length]`（⑤ 的累加包含每一条）。
**写回路径**：布局不可信 ⇒ **中止报错**（不重写），唯一例外是 0 字节文件（可证没有内容可丢，见 F-R4-09）。

---

## §4 已确认未修 + 理由

| 项 | 级别 | 为什么本轮不修 |
|---|---|---|
| `content_list` 的 `raw.trim()` 会把**首尾的 NBSP / 全角空格**也吃掉（`'\u{a0}'.is_whitespace() == true`）：`<content>　Hello　</content>` 读出来是 `Hello`，零译文写回就改字节、丢的是**可见字符** | 低 | 探针实测（`## trim_nbsp: src="Hello"`、`## trim_ideographic: src="Hello"`）。改成只裁 ASCII 空白是一行改动，但它属于 F-16「写回不逐字节保真」那一族，且「文本首尾空白要不要留」是产品判断（有些文件用缩进包住正文）。**建议下一轮连同 F-16 一起做成「span 原地替换」时统一处理**，本轮不顺手改 |
| `content_list` 写回仍丢弃注释 / DOCTYPE / 声明细节 / `<content>` 额外属性 / 元素间缩进（第三轮 F-16） | 信息 | 与第三轮同因：要逐字节保真得把「解析成条目再重建」改成 `lsx` 那样的区间原地替换，属整模块重构。本轮实测 `<!-- keep me -->` 会连注释一起消失（探针 `## inner_comment: src="ab"`），一并记入 backlog |
| 未知实体 `&nbsp;` 二次转义（第三轮 F-15） | 信息 | 行为未变、已有测试钉住；本轮只复核了它是**不动点**（`&amp;nbsp;` 连写两轮稳定） |
| `&amp;amp;` 这类双层转义**不是不动点**，每次写回剥一层（lead 观察 ③） | 信息 | **确认存在**，实测：`Escaped in=&amp;amp; → w1=&amp;amp; → w2=&amp;`（不是不动点），磁盘上两轮后收敛。不修的理由：`decode_entities` 的「还原一层」正是第三轮为修「旧版本写出的双层转义」加的防线（`files_double_escaped_by_older_versions_are_repaired_on_write_back` 钉住），而「合法双层转义」与「旧 bug 产物」在信息上不可分；去掉还原会让旧文件永久坏掉。**维持已文档化的取舍**，代价写在这里 |
| `.loca` 的 `version > 65535` 被静默变成 1（lead 观察 ①） | 信息 | 实测 `65535→65535`、`65536→1`、`70000→1`、`-1→1`。触发面只有「手工构造的条目」：真实链路里 version 来自 `LocalizedText.version`（u16 的 `to_string()`），必然 ≤ 65535；改成显式报错会新增一条「本来能写回、现在写不回去」的失败路径，收益为负。**保持既有行为**（已有测试 `invalid_version_falls_back_to_one`） |
| `.lsx` 的 `apply_replacements` 是 `O(n·k)`：逐条 `String::replace_range`（从后往前）在长文件上退化成二次方 | 低 | 实测（release）：2 000 字段 7.7 ms / 10 000 字段 175 ms / 20 000 字段 857 ms / 40 000 字段（4.6 MB）**5.4 s**。真实 MOD 的 LSX 远小于此，且写回跑在 `blocking` 线程（不卡 UI）。修法是把区间合并成一次 O(n) 重建，属结构性改动，**记入 backlog**（顺带能彻底消除「失效偏移」这类风险） |
| `extract_to_directory` / `open_and_extract_in` 中途失败会在用户指定的输出目录 / 传入的工作根下留下**半成品**文件（第三轮已记为跨范围项） | 低 | `open_and_extract` 会自己清理；`open_and_extract_in` 的清理语义归调用方（命令层）。本轮未改：清理「调用方给的目录」有误删风险，需要 lead 与 shell 侧一起定语义 |
| zip / PAK 的其它形态：嵌套 zip 只解一层（找不到 `.pak` 会给出明确错误）、CP437 非 UTF-8 文件名（`enclosed_name()` 解码后仍是普通路径，且 `.pak` 靠 `walk_files` 找，与文件名编码无关）、空归档（`pick_largest_pak` 返回 `None` → 明确报错）、PAK v10/v18 差异（由上游库处理，本层只传 priority） | 信息 | 本轮逐条探过/推演过，未发现可复现缺陷；没有真实语料能覆盖的（v10 / 真机加载）见 §5 |

---

## §5 无法验证

| 项 | 原因 |
|---|---|
| 布局自检对**真实游戏 `.loca`** 是否过严 | 仓库里没有任何真实 `.loca` 语料（`samples/` 只有 contentList XML 与官方术语表 JSON，`.loca` 全是测试手搓的）。**因此采取保守策略**：只挡「上游 `read_exact` 物理上不可能成功」的布局（§3 逐条），并用 5 条正向用例（v1/v2/65535、`texts_offset=0`、`texts_offset>表尾`、`length=0`、`n=0`）兜住误判 |
| BG3 游戏侧解析器对「非法 XML」的容忍度 | 需要真机；本报告只用**独立解析器（python expat）**证明产物非法/合法，未证明游戏行为 |
| 游戏真实的中文本地化目录名（是 `Chinese` 还是 `ChineseSimplified`） | 无真机、本机无法联网检索（web_search 无 key）；真实样本 MOD 只带 English + Polish。F-R4-10 的修复只解决「我们自己产出的目录在下一轮被认成中文」这一内部一致性；`TARGET_LANGUAGE` 的取值需要 lead 裁定 |
| Windows 专有行为：`rename` 覆盖已存在文件、`chmod 000` 等价场景（文件被占用/ACL） | 本机是 Linux。F-R4-03/05/06/09 的失败注入用的是 `chmod 000`（`#[cfg(unix)]`），CI 在 Windows 上不会跑这几条；三个修复本身依赖的都是跨平台语义（`ErrorKind::NotFound`、`fs::rename`） |
| 「磁盘写满」时的真实表现 | 无法安全地在共享机器上制造满盘；F-R4-05/06 用「写入前必然失败的条件」（key 超长 / 源文件不可读）等价复现了「失败发生在截断之后」这条根因 |
| zip64 / stored / 非 deflate 组合下的实际写入层 | 只测了 deflate + 篡改声明大小（第三轮同口径）；本轮新增的单条目实际写入预算与压缩方法无关，但未构造 zip64 大样本 |

---

## §6 方法学与自查（含我自己引入并修掉的问题）

1. **每条修复都先有失败测试**：一次性跑出的红态是 `test result: FAILED. 390 passed; 9 failed`（9 条新用例全红，含 lead 转来的 F-A1/F-A2 两条）；改完后同一批全部转绿。原始输出见附录 A。
2. **15 组变异实验**：见附录 B。M2/M5 第一次自动跑时命中「别人正在改文件」导致的编译失败（探针输出为「没有 test result 行」），**已手工重跑确认变红**；M15 的预期红态是**挂死**（不是断言失败），用 `timeout 60` 记录。
3. **诚实记录一处自曝问题**：F-R4-01 的第一版严格文法实现里，属性值扫描写成了 `while bytes.get(i) != Some(&quote)` —— 越过末尾后 `get` 恒为 `None`，**循环永不退出**。`is_allowed_inline_tag` 是公开 API，`<LSTag a="x>` 这种候选串会直接挂死。这是我自己引入的缺陷，被标签探针 `timeout 900` 超时抓到（rc=124），已改成带 `i < bytes.len()` 边界的循环并补了守卫测试 `unterminated_tag_attribute_is_not_a_tag`（变异 M15：撤回边界 → 测试挂死）。
4. **工作区并发**：本轮期间有别的 writer 跑过 `cargo fmt`（我的 `content_list.rs` / `pak.rs` md5 在变异脚本运行中途变化，语义未变，已由重跑测试确认）；`cargo fmt --all --check` 收尾时仍有差异的 3 个文件（`src-tauri/src/commands/entries.rs`、`terminology.rs`、`translate.rs`）**不在我的写范围**，不属于本报告范围。
5. 我的三个文件最终 md5（供 verifier 钉版本；`content_list.rs` 在最后一次 rustfmt 后为 `3b29495f…`，与 `e0746a92…` **语义相同**、仅缩进变化）：
   - `formats/content_list.rs` = `3b29495fd5eba49402de3133a08f6284`（rustfmt 前 `e0746a9237828f67476c03086171d918`）
   - `formats/loca.rs` = `fe127105d8288370bc2527e0701987ad`
   - `pak.rs` = `ace940ec39e86651b6b80a41c3741c7a`
   - （`formats/lsx.rs` = `b688908739ed133590dae5f54ae830db`、`formats/mod.rs` = `aea81104604634a5c614f562c99aced8` 均为**未改**）

---

## 附录 A：修复前红 → 修复后绿（原始输出）

```console
$ cd /home/jason/bg3-translate && cargo test -p bg3-translate-core --lib --offline -- --skip lying_entry_count_is_rejected_without_a_giant_allocation
test result: FAILED. 390 passed; 9 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.70s
failures:
    formats::content_list::tests::illegal_char_reference_in_tag_attribute_never_reaches_the_file
    formats::content_list::tests::tag_without_whitespace_between_attributes_is_never_written_as_markup
    formats::content_list::tests::write_refuses_when_the_target_exists_but_cannot_be_read
    formats::content_list::tests::write_refuses_when_the_target_is_only_partially_parsable
    formats::loca::tests::failed_write_does_not_destroy_the_existing_loca
    formats::loca::tests::lying_offsets_and_lengths_are_rejected
    formats::loca::tests::write_refuses_when_the_target_exists_but_cannot_be_read
    pak::tests::failed_repack_does_not_destroy_an_existing_output_file
    pak::tests::zip_entry_that_lies_about_its_size_cannot_exceed_the_per_entry_limit
（`lying_entry_count_is_rejected_without_a_giant_allocation` 单跑时的红态是**进程 abort**：
 `memory allocation of 137438953440 bytes failed`，见 F-R4-02）
```

修复后（当前最终状态）：

```console
$ cargo test -p bg3-translate-core --all-targets --offline
test result: ok. 414 passed; 0 failed; …   # lib
test result: ok. 16 passed; …              # corpus_regression
test result: ok. 11 passed; …              # corpus_writeback
test result: ok. 8 passed; …               # e2e_pak_flow
test result: ok. 8 passed; …               # real_mod_sample
```

---

## 附录 B：变异实验（把修复改回原样，测试必须变红）

脚本：`/tmp/r4mut.py`（逐条替换生产代码关键行 → 跑对应测试 → 还原 → 校验 md5）。**15/15 按预期红**：

| 变异 | 目标测试 | 结果 |
|---|---|---|
| M1 去掉「属性之间必须有空白」 | `tag_without_whitespace_between_attributes_is_never_written_as_markup` | 变红 |
| M2 去掉属性值的非法字符过滤 | `illegal_char_reference_in_tag_attribute_never_reaches_the_file` | 变红（手工复跑确认） |
| M3 读不出来的目标当成新文件 | `write_refuses_when_the_target_exists_but_cannot_be_read` | 变红 |
| M4 忽略底稿解析中断 | `write_refuses_when_the_target_is_only_partially_parsable` | 变红 |
| M5 loca 直接写盘（非原子） | `failed_write_does_not_destroy_the_existing_loca` | 变红（手工复跑确认：`写回失败时原文件必须逐字节不变`） |
| M6 去掉 `.loca` 布局自检 | `lying_entry_count_is_rejected_without_a_giant_allocation` | 变红（**进程 abort**） |
| M7 loca 读失败降级成整体重写 | `write_refuses_when_the_target_exists_but_cannot_be_read` | 变红 |
| M8 zip 实际写入只看总量 | `zip_entry_that_lies_about_its_size_cannot_exceed_the_per_entry_limit` | 变红 |
| M9 repack 直接写目标文件 | `failed_repack_does_not_destroy_an_existing_output_file` | 变红 |
| M10（放宽自检）去掉索引表装不下检查 | `lying_entry_count_is_rejected_without_a_giant_allocation` | 变红 |
| M11（收紧自检）要求 `texts_offset ≥ 表尾` | `texts_offset_below_the_table_is_still_readable` | 变红 |
| M12（收紧自检）要求每条文本非空 | `empty_text_and_empty_table_are_readable` | 变红 |
| M13（放宽自检）去掉文本区总量检查 | `lying_offsets_and_lengths_are_rejected` | 变红 |
| M14 loca 布局不可信时整体重写 | `suspicious_target_aborts_but_zero_byte_target_is_rebuilt` | 变红 |
| M15 撤回引号扫描的循环边界 | `unterminated_tag_attribute_is_not_a_tag` | **挂死**（60 s 无 `test result`，`timeout` 124） |

---

## 附录 C：门禁真实输出（收工前最后一次）

```console
$ cd /home/jason/bg3-translate && cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.38s

$ cargo test -p bg3-translate-core --all-targets
test result: ok. 414 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.69s
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

`bash scripts/verify.sh` 的真实输出（收工前最后一次，全量 6 道）：

```console
$ cd /home/jason/bg3-translate && bash scripts/verify.sh
== bg3-translate 一键校验 ==
模式：all，共 6 道门禁
── [1/6] 跨层 IPC 契约检查                 ✓ 通过（44ms）
── [2/6] Rust 代码格式（cargo fmt --check） ✓ 通过（170ms）
── [3/6] Rust Clippy 零告警                 ✓ 通过（1.3s）
── [4/6] Rust 核心库测试（含端到端 PAK 闭环） ✓ 通过（972ms）
        test result: ok. 414 passed / 16 passed / 11 passed / 8 passed / 8 passed
── [5/6] 前端单元测试（vitest）             ✓ 通过（5.3s）  Test Files 21 passed / Tests 208 passed
── [6/6] 前端类型检查 + 构建                ✓ 通过（645ms）
✓ 全部 6 道门禁通过（总耗时 8.5s）
rc=0
```

（中途有一次 `core-fmt` 失败，是我自己的文件在最后一次编辑后没跑 rustfmt；已跑 `rustfmt --edition 2024` 并重跑全量门禁，
上表就是修好之后的真实结果。另：`src-tauri/src/commands/{entries,terminology,translate}.rs` 曾短暂出现格式差异，
那是别人的写范围，收尾前已由对应 writer 处理，**不在本报告范围**。）

### 收尾时的文件 md5（最终态，供 verifier 钉版本）

```console
$ md5sum crates/bg3-translate-core/src/formats/{content_list,loca,lsx,mod}.rs crates/bg3-translate-core/src/pak.rs
3b29495fd5eba49402de3133a08f6284  crates/bg3-translate-core/src/formats/content_list.rs   ← 本报告 §6 里的 e0746a92 是 rustfmt 之前的同一份语义
fe127105d8288370bc2527e0701987ad  crates/bg3-translate-core/src/formats/loca.rs
b688908739ed133590dae5f54ae830db  crates/bg3-translate-core/src/formats/lsx.rs            ← 未改
aea81104604634a5c614f562c99aced8  crates/bg3-translate-core/src/formats/mod.rs            ← 未改
ace940ec39e86651b6b80a41c3741c7a  crates/bg3-translate-core/src/pak.rs
```
