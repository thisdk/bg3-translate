# 第三轮审查报告：Rust 归档与本地化格式层（pak / loca / lsx / content_list）

- 审查者：`format-auditor`（T1）
- 基线：`HEAD = 422f667a0d5d3be26190c0f5b55b0796a67a7b76`，开工时 worktree hash `ad8d3219378a5b89059a3558138ec7d1`
- 写范围：`crates/bg3-translate-core/src/pak.rs`、`src/formats/**`、`tests/e2e_pak_flow.rs`、`tests/real_mod_sample.rs`
- 结论一句话：**格式层有 4 条会把整份本地化文件变成非法 XML / 弄丢条目的高危缺陷，均已用「先写失败测试 → 再修」的方式修掉**；另有 10 条中低危缺陷修复，3 条已知取舍（其中 2 条用测试钉住行为）——共新增 29 条回归测试（27 条单元 + 2 条集成），20 项变异测试全部会变红。

---

## §1 结论摘要表

| 编号 | 严重度 | 状态 | 一句话 |
|---|---|---|---|
| F-01 | **高** | 已修 | `content_list` 把正文里的字面 `< br >` 当成真标签写出去 → 产物是非法 XML，**零译文、纯读一次再写回**就触发 |
| F-02 | **高** | 已修 | 标签属性值里的裸 `>` 会把标签从中间劈开 → 产物是非法 XML（红队 R-02A） |
| F-03 | **高** | 已修 | 伪结束标签 `</ b>`（斜杠后带空格）被当成合法结束标签 → 产物是非法 XML（红队 R-02B） |
| F-04 | **高** | 已修 | 不闭合空元素 `<br>` 被原样写出 → 产物是非法 XML（同类的第三条触发路径） |
| F-05 | 中 | 已修 | `content_list` 写回按提交列表**重建整个文档**，磁盘上已有但未提交的条目被永久删除 |
| F-06 | 中 | 已修 | `.loca` 写回整体重建索引表，磁盘上已有但未提交的 key 被永久删除（F-05 同源） |
| F-07 | 中 | 已修 | XML 非法控制字符（`\u{0}`-`\u{8}` 等）原样落盘 → 整个 `.xml`/`.lsx` 变非法文件 |
| F-08 | 中 | 已修 | `formats::read_entries/write_entries` 不校验 `file_name` → `../evil.loca` 写到 `unpacked/` 之外（红队 R-03） |
| F-09 | 中 | 已修 | `safe_output_path` 放行尾随点/空格片段（`".. "`）→ Windows 上被规整成 `..`，zip-slip 绕过（红队 R-11 上半） |
| F-10 | 中 | 已修 | zip 解压无任何上限：65 KB → 64 MiB（1027×），可写满磁盘（红队 R-04） |
| F-11 | 中 | 已修 | `.lsx` 重复条目产生两个相同替换区间 → 用失效偏移二次替换，把值写坏 |
| F-12 | 低 | 已修 | `<content>` 原本没有 `version` 属性，写回凭空补出 `version=""`（红队 R-06） |
| F-13 | 低 | 已修 | `safe_output_path` 放行含 NUL 的片段 → 半路抛 IO 错误、信息不清（红队 R-11 下半） |
| F-14 | 低 | 已修 | `walk_files` 跟随符号链接；`repack` 会把 `unpacked/` 里符号链接指向的**工作目录外文件**打进 PAK |
| F-15 | 信息 | 已确认未修 | 未知实体 `&nbsp;` 读成字面量、写回转义成 `&amp;nbsp;`（零译文也改字节）——用测试钉住（红队 R-07） |
| F-16 | 信息 | 已确认未修 | `contentList` 写回丢弃注释 / DOCTYPE / 声明细节 / `<content>` 额外属性 / 缩进 |
| F-17 | 信息 | 已确认未修 | `.loca` 的 `version` 解析失败静默退化为 1（有注释有测试，属既有取舍，本轮仅复核） |

修复前红 / 修复后绿的真实证据见 §2 每条，以及文末「附录 A：修复前失败清单」「附录 B：变异测试」。

---

## §2 缺陷详情

### F-01（高）正文里的字面 `< X >` 被当成真标签写出去，产物是非法 XML

**现象**：源文件里 `x &lt; br &gt; y`（合法 XML：`<`、`>` 都是转义过的正文）解析后
`source == "x < br > y"`。`is_allowed_inline_tag` 按「`<` 后面第一个词是不是白名单
名字」判定，把 `< br >` 认成 `<br>`；`is_tag_balanced` 也认为配对，于是 `render`
把这串**原样**写进 XML —— `<` 后面紧跟空格不是合法开始标签，产物直接变成非法文档。
**这条路径不需要任何译文**：未翻译条目写回原文时就会触发。

**复现（探针，修复前）**：
```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin cl
=== lt-br-gt
  src     : "x < br > y"
  rendered: <?xml ...?><contentList><content contentuid="h1" version="1">x < br > y</content></contentList>
  reparse error: Some("ill-formed document: expected `</>`, but `</content>` was found")
  ROUNDTRIP_STABLE: false
=== lt-i-gt
  src     : "a < i > b </i>"
  rendered: ...>a < i > b </i></content>...
  reparse error: Some("ill-formed document: expected `</>`, but `</i>` was found")
```

**修复后（同一探针，未改一行探针代码）**：
```console
  rendered: ...>x &lt; br &gt; y</content>...
  reparse error: None
  ROUNDTRIP_STABLE: true
```

**根因**：`is_allowed_inline_tag` / `is_tag_balanced` / `write_text_fragment` 三个函数
各有一份「朴素标签识别」，且都不校验候选串**是否真是合法 XML 标签**。

**改动**（`src/formats/content_list.rs`）：
- 新增 `scan_tag_end`：引号感知地找标签结束的 `>`（同时给 F-02 兜底），中途遇 `<` 直接放弃；
- 新增 `parse_single_tag` + `is_xml_name`：用 quick-xml 真解析一次，要求恰好一个
  Start/End/Empty 事件、属性逐个 `attributes()` 可解析；结束标签按 XML 语法校验名字；
- `is_allowed_inline_tag` / `is_tag_balanced` / `write_text_fragment` 全部改用这套判定，
  「配对检查通过」因此等价于「写出去的东西是合法 XML」；
- 扫描到「不是标签的 `<`」时只跳过这一个字符继续找（而不是吞到下一个 `>`），
  于是 `HP < 5 and <b>bold</b>` 里的 `<b>` 仍能保住。

**回归测试**：`formats::content_list::tests::literal_angle_bracket_text_is_never_written_as_markup`、
`unknown_or_malformed_tags_are_escaped`、`equivalent_tag_spellings_are_still_written_as_markup`

---

### F-02（高）属性值里的裸 `>` 把标签劈坏

**现象**：`<LSTag Tooltip="a > b">` 是合法 XML（属性值允许字面 `>`）。按「第一个 `>`
结束标签」切分会得到 `<LSTag Tooltip="a >` + 正文 ` b">fire`，写回后属性没闭合。

**复现（探针，修复前）**：
```console
=== gt-in-attr
  src     : "Deals <LSTag Tooltip=\"a > b\">fire</LSTag> damage"
  rendered: ...>Deals <LSTag Tooltip="a > b&quot;&gt;fire</LSTag> damage</content>...
  reparse error: Some("syntax error: attribute value not closed: `\"` not found before end of input")
  ROUNDTRIP_STABLE: false
```
修复后：`reparse error: None`，`ROUNDTRIP_STABLE: true`（探针输出见附录 C）。

**根因 / 改动**：同 F-01（`scan_tag_end` 引号感知）。

**回归测试**：`formats::content_list::tests::tag_attribute_value_containing_gt_survives_write_back`

---

### F-03（高）伪结束标签 `</ b>` 被当成合法结束标签

**现象**：`<b>x</ b> y` 里 `</ b>` 不是合法 ETag（XML 要求 `</` 与名字之间不能有空白）。
旧 `is_allowed_inline_tag` 先 `trim_start_matches('/')` 再 `trim()`，把 `/ b` 规范成 `b`；
又因为 `<b>` 与「`</ b>`」被算作配对，整段走了「原样写标签」分支 → 非法 XML。

**复现（红队探针原文，修复前）**：
```console
$ cd /tmp/rt && cargo test -p bg3-translate-core --offline --test rt_probe5 h2 -- --nocapture
H2 is_allowed_inline_tag(</ LSTag>) = true
H2 rendered = ...>译文 <b>x</ b> 尾</content>...
H2 reparse error = Some("ill-formed document: expected `</b>`, but `</ b>` was found")
```

**修正确认**：`is_xml_name` 只接受 `NameStartChar`+`NameChar`，`" b"` 首字符是空格 →
`parse_single_tag` 返回 `None` → 该 `<` 当正文转义；同时 `is_tag_balanced` 因为配对栈
里还留着 `b` 而返回 false，整段退化成纯文本，产物必然合法。

**回归测试**：`formats::content_list::tests::pseudo_end_tag_with_space_is_treated_as_text`
（用例特意写成 `<b>粗体</ b> 尾巴`：只有「伪结束标签被当真」时配对才会通过，才能真正打到缺陷）

---

### F-04（高）不闭合空元素 `<br>` 被原样写出

**现象**：模型按 XHTML 习惯输出 `<br>`（纯 XML 里必须写 `<br/>`）。`VOID_TAGS` 把
`br` 当空元素，配对检查通过，于是 `<br>` 原样落盘 → `expected </br>` 非法文档。
源文件不可能出现这种形态（我们的读取会先报错），所以这条只是**译文侧**触发，但后果同样是整份文件作废。

**复现（本轮新发现）**：未修前 `render` 输出 `...line1<br>line2<br >line3</content>...`，
重新 `parse` 报错。修复后统一规范成 `<br/>`：
```
assert!(!out.contains("<br>") && !out.contains("<br >"));
assert!(out.contains("line1<br/>line2<br />line3"));
```

**改动**：新增 `normalize_void_tag`，写真实标签前把不闭合的空元素补成自闭合（语义不变）。

**回归测试**：`formats::content_list::tests::unclosed_void_tag_is_normalized_to_self_closing`

---

### F-05（中）`content_list` 写回删掉磁盘上已有、但没提交的条目

**现象**：写回是「用条目列表重建整个文档」。前端 `planLocalizationWrites`
（`src/lib/localization.ts:70-93`）按「英文(3) > 中文(2) > 其它(1)」每个目标路径只保留
**一份**条目：MOD 自带 `Localization/Chinese/x.xml`（含英文原文没有的 contentuid）时，
提交的是英文那份 → 中文文件里多出来的条目被永久删除，游戏里对应文本变成原始句柄。

**复现（e2e，修复前）**：
```console
$ cargo test -p bg3-translate-core --test e2e_pak_flow --offline
---- existing_target_entries_survive_a_subset_write_back_and_repack stdout ----
panicked at tests/e2e_pak_flow.rs:576:
只属于中文文件的条目被删掉了: <?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h11111111g2222u3333i4444" version="1">【译】Hello, adventurer.</content></contentList>
```

**改动**：`content_list::write` 先解析磁盘上的旧文档，把「旧有新无」的条目按原样补回
写回列表（contentuid、version、文本都不变），并记警告日志。空列表写回因此也不会再清空文件。

**回归测试**：`formats::content_list::tests::write_keeps_entries_that_already_exist_on_disk`、
`writing_an_empty_list_does_not_wipe_an_existing_file`、
`tests/e2e_pak_flow.rs::existing_target_entries_survive_a_subset_write_back_and_repack`（含重打包后再解包校验）

---

### F-06（中）`.loca` 写回删掉磁盘上已有的 key

同 F-05，只是格式换成了二进制索引表：`entries_to_resource` + `save_with_format`
整体重建，未提交的 key 消失。

**复现（修复前）**：`formats::loca::tests::write_keeps_keys_that_already_exist_on_disk`
断言 `back.len() == 2`，修复前只有 1 条（`h_only_chinese` 丢失）。
**改动**：`loca::write` 用 `LocaUtils::load` 读回旧表，把未提交的 key 追加保留；
读不出来（空文件/非法文件）时按整体重写处理并记警告。
**回归测试**：`formats::loca::tests::write_keeps_keys_that_already_exist_on_disk`、
`writing_an_empty_list_does_not_wipe_an_existing_loca`

---

### F-07（中）XML 非法控制字符落盘

**现象**：译文里带 `\u{1}` 时，`content_list::render` 与 `lsx::escape_attribute`
都把控制字符原样写进文件。XML 1.0 的 `Char` 产生式不含它们，**连字符引用都不允许**
（`&#1;` 同样非法），所以产物对游戏侧解析器就是非法文件——丢的是整份译文/整份元数据。

**复现（探针，修复前）**：
```console
=== control-char target
  bytes: [1]           # 渲染结果里真的有 0x01
  rendered: ...>你好<0x01>世界</content>...
（LSX）控制字符 out bytes = [1, 7]
```
修复后：`bytes: []`、`控制字符 out bytes = []`。

**改动**：`content_list` 新增 `sanitize_xml_chars`（写入前丢弃非法字符并记警告）；
`lsx::escape_attribute` 在转义循环里丢弃非法字符并计数告警。合法空白 `\t\n\r` 保留
（LSX 仍按 `&#10;`/`&#9;` 写字符引用）。contentuid/version 是查表句柄，含非法字符时
**直接报错**而不是悄悄改写（新增 `reject_illegal_identity`）。

**回归测试**：`formats::content_list::tests::illegal_control_characters_are_never_written_into_the_xml`、
`identity_attributes_with_control_characters_are_rejected`、
`formats::lsx::tests::illegal_control_characters_never_reach_the_attribute_value`

---

### F-08（中）`formats::read_entries/write_entries` 不校验 `file_name`，可写到工作目录之外

**现象**：`file_name` 是前端可控字符串，旧实现用纯 `join`（`pak::resolve_disk_path`）。
`"../evil.loca"` → `work_dir/evil.loca`（`unpacked/` 之外）；再往前几层就能写到任意位置。

**复现（探针，修复前）**：
```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin paths
write_entries("../evil.loca") kind=LocalizationLoca -> Ok(())
  exists outside unpacked? .../work/evil.loca = true      # ← 真的落盘了
read_entries(../secret.xml) -> Err(Config("文件不存在: .../work/unpacked/../secret.xml"))  # 读路径同样没校验
```
修复后：
```console
write_entries("../evil.loca") kind=LocalizationLoca -> Err(Pak("非法归档路径: ../evil.loca"))
  exists outside unpacked? .../work/evil.loca = false
read_entries(../secret.xml) -> Err(Pak("非法归档路径: ../secret.xml"))
```

**改动**：`formats` 新增私有 `checked_disk_path`，读写统一走
`pak::safe_output_path(&pak::unpacked_dir(work_dir), file_name)`。
命令层（`src-tauri`，shell-auditor 范围）已在做同样校验，这里是 core 公共 API 的纵深防御——
`write_entries` 是 `pub`，测试与其它调用方都可能直接用它。

**回归测试**：`formats::tests::write_entries_rejects_file_names_that_escape_unpacked`、
`formats::tests::read_entries_rejects_file_names_that_escape_unpacked`

---

### F-09（中）`safe_output_path` 放行尾随点/空格，Windows 上等价于 `..`

**现象**：Windows 路径规整会去掉每个片段结尾的 `.` 与空格，于是 `".. "` 变成 `".."`——
经典 zip-slip 绕过；`"a. "`、`"COM1 "` 之类同样会被静默改名。这类名字在 Windows 上
根本创建不出来，合法 PAK 里不可能存在。

**复现（探针，修复前 / 修复后）**：
```console
（前）safe_output_path(.. ) -> Ok("root/.. ")
     safe_output_path(.. /evil.txt) -> Ok("root/.. /evil.txt")
     safe_output_path(a. ) -> Ok("root/a. ")
（后）safe_output_path(.. ) -> Err(Pak("非法归档路径（片段以点或空格结尾，Windows 上会被规整掉）: .. "))
     safe_output_path(...) / (a. ) / (COM1 ) 同样 Err
     safe_output_path(a.b c.txt) -> Ok(...)    # 不误伤名字中间的点与空格
```
**改动**：`safe_output_path` 的 `Component::Normal` 分支拒绝 `ends_with(['.', ' '])` 的片段。
**回归测试**：`pak::tests::safe_output_path_rejects_trailing_dots_and_spaces`
**未验证部分**：本机是 Linux，无法实测 Windows 归一化行为（见 §4）。

---

### F-10（中）zip 解压无任何上限（压缩炸弹）

**现象**：`extract_zip_to_find_pak` 不检查声明大小、不统计实际写入、不限条目数。
`open_and_extract` 的清理跑在解压**之后**，磁盘写满了才轮到它。

**复现（探针，修复前）**：
```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin zipx
zip size on disk = 66019 bytes
  .../zip_contents/bomb.bin (67108864 bytes)      # 1000× 膨胀，无任何报错
```

**改动**（`pak.rs`）：
- `ZipLimits { max_entry_bytes: 6 GiB, max_total_bytes: 16 GiB, max_entries: 200_000 }`
  —— 取值理由写在代码注释里：BG3 的 4K 材质 MOD 解包后可达数 GB（合法），
  所以上限必须宽松到不误伤真实 MOD，只挡「单文件 >6 GiB」「总量 >16 GiB」
  「条目数 >20 万」这类不可能是真实 MOD 的形态；
- 三层检查：① 条目**声明**大小（连文件都不创建）→ ② 累计声明总量 → ③ **实际写入**
  字节数（`Read::take(budget+1)` + 超限时删除半成品文件，防声明撒谎）；
- `extract_zip_to_find_pak_limited` 私有函数接受注入上限，便于用小样本测防线。

**回归测试**：`pak::tests::zip_bomb_is_refused_before_it_fills_the_disk`（单条目层）、
`zip_total_size_limit_is_enforced_across_entries`（总量层）、
`zip_with_a_lying_size_header_is_refused`（实际写入层：篡改本地头 +22 与中央目录 +24
把声明大小改成 1 字节）、`zip_entry_count_limit_is_enforced`（条目数层）、
`normal_zip_still_extracts_under_the_limits`（不误伤正常 zip）

---

### F-11（中）`.lsx` 重复条目导致重叠替换，把值写坏

**现象**：`entries` 里同一个 contentuid 出现两次（前端重读/合并出错时可能发生）时，
`plan_replacements` 会产生两个**完全相同**的区间；从后往前替换时第二次用的是已经失效的
偏移，把刚落盘的译文又切一刀。

**复现（探针，修复前）**：
```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin misc
plan (dup entries): [(85..88, "新一"), (85..88, "新二")]
  out = <attribute id="Description" type="LSString" value="新二一" />   # ← 串了
```
修复后：`plan (dup entries): [(85..88, "新一")]`，值 = `新一`。

**改动**：`plan_replacements` 按区间去重（保留第一条并记警告）；
`apply_replacements` 增加重叠护栏（后面的替换必须整体落在上一个已应用区间左侧），
防止调用方直接塞进重叠/重复区间。

**回归测试**：`formats::lsx::tests::duplicate_entries_are_applied_once_instead_of_corrupting_the_value`
（4 个断言：单条、重复条目、重复区间计划、重叠区间计划）

---

### F-12（低）`<content>` 缺 `version` 属性时写回补出 `version=""`

**现象**：原文件 `<content contentuid="h1">A</content>`（没有 version 属性），
读出来 `version == ""`，`render` 无条件 `push_attribute("version", "")`，
产物多出一个空版本号——「没有这个属性」和「版本为空」对游戏不是一回事，
而且零译文写回也会改变文件内容。

**复现**：`formats::content_list::tests::missing_version_attribute_is_not_invented_on_write_back`
（修复前 `out` 里出现 `version=""`）。
**改动**：`version` 为空串时不写该属性。
**回归测试**：同上（同时断言有 version 的条目照旧写 `version="3"`，属性缺失能原样读回）。

---

### F-13（低）`safe_output_path` 放行含 NUL 的片段

**现象**：`"evil\0.txt"` 通过校验，到 `fs::write` 才以
`file name contained an unexpected NUL byte` 失败（不可利用，但错误发生在半路、
信息与「路径非法」无关）。
**复现（探针）**：修复前 `safe_output_path(evil\0.txt) -> Ok("root/evil\0.txt")`；
修复后 `Err(Pak("非法归档路径（含 NUL 字节）: evil\0.txt"))`。
**改动**：`safe_output_path` 拒绝含 `'\0'` 的片段。**回归测试**：`pak::tests::safe_output_path_rejects_nul_bytes`

---

### F-14（低）符号链接：`walk_files` 跟随链接、`repack` 会把工作目录外文件打进 PAK

**现象**：`walk_files` 用 `path.is_dir()`（会跟随链接）判断目录：链接指向的外部文件会被
当成目录树里的文件（`pick_largest_pak` 可能选中解压目录之外的 `.pak`），指向祖先目录的
链接会让遍历永不结束。`repack` 更严重：底层 `PackageBuilder::add_directory` 跟随链接，
把链接指向的**工作目录之外**的文件原样打进 PAK。

**复现（探针，修复前）**：
```console
$ cd /tmp/fa-probe && cargo run --offline -q --bin pak2
walk_files sees link target? true
repack with symlink -> Ok(())
  repacked files: ["Localization/English/a.xml", "link_dir/evil.txt", "zero.bin"]   # ← 外部文件进包了
```
修复后：
```console
walk_files sees link target? false
repack with symlink -> Err(Pak("工作目录里存在符号链接，拒绝打包（它会把工作目录外的文件打进 PAK）: .../unpacked/link_dir"))
```
**改动**：`walk_files` 用 `DirEntry::file_type()`（不 follow）判断类型并跳过符号链接；
`repack` 打包前用新增的 `find_symlink` 扫一遍 `unpacked/`，发现链接直接报错。
解包本身不会产生符号链接（见 §3），所以这只是本机其它程序留下的链接的兜底。
**回归测试**：`pak::tests::walk_files_does_not_follow_symlinked_directories`、
`pak::tests::repack_refuses_symlinks_inside_unpacked`

---

### F-15（信息，已确认未修）未知实体二次转义

`<content>a &nbsp; b</content>` 里的 `&nbsp;`（XML 未定义实体）被 quick-xml 当
`GeneralRef` 事件、`resolve_reference` 保留成字面量 `&nbsp;`；写回时作为纯文本被转义成
`&amp;nbsp;`。语义等价（再次读回来仍是 `a &nbsp; b`），但**字节变了**，零译文写回也会改文件。

**探针证据（修复前后一致）**：
```console
=== nbsp
  src     : "a &nbsp; b"
  rendered: ...>a &amp;nbsp; b</content>...
  reparse error: None
  ROUNDTRIP_STABLE: true
```
**不修的理由**：要「原样写回 `&name;`」就必须只放行源文件里出现过的实体；模型/用户新写
一个未定义实体就会产出**非法 XML**——正是本轮在修的那类缺陷，风险大于收益。
**行为钉住**：`formats::content_list::tests::unknown_entities_are_pinned_as_literal_text`

---

### F-16（信息，已确认未修）`contentList` 写回丢弃文档级结构

`render` 是「按条目重建文档」，以下内容在写回后消失：注释、DOCTYPE、XML 声明里的
`standalone`、`<content>` 上的额外属性、元素之间的缩进/换行；`raw.trim()` 还会去掉
文本首尾空白。

**探针证据**：
```console
=== structure-loss
  orig: "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<!DOCTYPE contentList>\n<!-- hi -->\n<contentList xmlns:x=\"urn:x\">\n  <content contentuid=\"h1\" version=\"1\" extra=\"keepme\">A</content>\n</contentList>\n"
  out : "<?xml version=\"1.0\" encoding=\"utf-8\"?><contentList xmlns:x=\"urn:x\"><content contentuid=\"h1\" version=\"1\">A</content></contentList>"
```
（根元素属性、BOM 是保留的；contentuid/version 全部原样保留。）

**不修的理由**：BG3 的 `contentList` 由工具生成，没有注释/DOCTYPE 语义；
要逐字节保真需要把「解析成条目再重建」改成「记录文本区间后原地替换」（`lsx` 已是这种设计），
是整模块级重构，超出本轮风险预算。若后续要做，`lsx` 的 span 方案可直接复用。

---

### F-17（信息，已确认未修）`.loca` version 解析失败静默退化

`entries_to_resource` 对 `entry.version.parse().unwrap_or(1)`：非数字版本号会静默变成 1。
真实链路里 version 来自 `LocalizedText.version`（u16 的 `to_string()`），必然可解析；
只有手工构造的条目才会命中。红队 R-12 同样把它记为「既有取舍」，本轮复核后维持原状
（`formats::loca::tests::invalid_version_falls_back_to_one` 已有测试覆盖该行为）。

---

## §3 已排查但**证伪**的怀疑点

| 怀疑点 | 结论 | 证据（命令 + 输出） |
|---|---|---|
| zip-slip：`../`、绝对路径、`a/../../`、`..\..\` | **证伪**（`enclosed_name()` 拦住） | `cargo run --bin zipx`：恶意条目全部跳过，`evil1 exists outside? false`，解压目录里只有 `abs.txt`、`bomb.bin`、`etc/passwd`（都在 scratch 内） |
| zip 里的符号链接条目会写出符号链接 | **证伪**（写成普通文件） | 同上：`add_symlink` 条目被 `File::create` 写成普通文件，`symlink created? false` |
| 磁盘写满时 `repack` 静默成功（BufWriter drop 吞错） | **证伪**（返回 Err） | `cargo run --bin devfull`：`repack -> /dev/full = Err("PAK 处理错误: 打包失败: I/O error: No space left on device (os error 28)")`；正常输出 `repack -> ok = Ok(())`，`ok.pak size = 107` |
| `pick_largest_pak` 同体积时结果不稳定 | **证伪**（确定性取字典序最小） | `cargo run --bin pak2`：`pick same-size -> Some("a.pak")`（`a.pak`/`b.pak` 各 10 字节） |
| 0 字节文件 / 空目录在 pack→unpack 后出问题 | 0 字节**证伪**；空目录不保留（PAK 格式无目录项，BG3 不需要） | `cargo run --bin pak2`：`zero.bin exists=true len=0`；`empty dir exists=false` |
| LOCA 往返丢 version/contentuid/内嵌 NUL/换行/重复 key/空文本 | **证伪**（全部逐条保留） | `cargo run --bin misc`：`h1 ver=1`、`h2 ver=2 src="a\0b"`、`h3 src="line1\nline2"`、`h4 src=""`、`h5 ver=5 first` + `h5 ver=6 second`（重复 key 顺序保留） |
| `.lsx` 会翻 `TranslatedString` 句柄 / `meta.lsx` 的 `Name` | **证伪**（白名单+类型双重过滤） | 既有测试 `translated_string_handles_are_never_treated_as_text`、`module_name_is_never_treated_as_translatable`、`tests/real_mod_sample.rs::real_meta_lsx_name_is_not_translatable` 全绿（真实样本 `Name=GustavDev`、`Folder=AppearanceEditEnhanced` 都没进条目） |
| `.lsx` 属性名大小写不同（`ID=`/`Value=`）会漏字段 | **不构成缺陷**（lslib 固定写小写 `id`/`type`/`value`，大写形态不是合法 LSX） | `cargo run --bin misc`：`lsx uppercase = 0`、`lsx lowercase id = 0`（后者是字段白名单外的 `description`，同样按设计跳过） |
| `safe_output_path` 被 `..%2f`、超长路径绕过 | **证伪**（无 URL 解码；超长路径在系统调用层失败，不会逃逸） | `cargo run --bin paths`：`..%2fevil -> Ok("root/..%2fevil")`（就是个普通文件名）；8000 字符路径 `Ok(...)` 但落在 root 之内 |
| 空/空白 XML 被写回成空文档 | **证伪（已由 F-05 一并加固）** | `cargo run --bin misc`：`empty xml read = Ok(0)`、`blank xml read = Ok(0)`；写回时合并逻辑会把磁盘上的条目原样保留，空列表不再清空文件 |
| `contentList` 里嵌套 `<content>`/重复 contentuid 会丢条目 | **证伪** | `cargo run --bin cl`：`dup-empty` 用例 3 条（含两个同 uid）全部往返一致；`nested_content_like_tags_do_not_break_parsing` 既有测试 |
| CDATA / 实体 / BOM / CRLF 影响写回 | **证伪** | `cargo run --bin cl`：`ws-cdata` 用例（含 `<![CDATA[a < b & c]]>`、`&#x4E2D;`）`ROUNDTRIP_STABLE: true`；BOM 由既有测试 `write_preserves_bom_and_root_attributes` 覆盖 |

---

## §4 无法验证项与原因

| 项 | 原因 |
|---|---|
| Windows 上尾随点/空格的归一化行为（F-09 的成因） | 本机是 Linux（WSL2 内核）；结论基于 Win32 路径规整语义。**但修复本身是安全的**：这类名字在 Windows 上不可创建，合法 PAK 不会包含它们 |
| 游戏侧解析器对「非法 XML」的容忍度（F-01~F-04 的最终影响） | 需要真机 BG3。我们只能证明「产物对 quick-xml 是非法文档」，以及游戏大概率同样拒绝整份文件 |
| 游戏加载器对未知实体 `&nbsp;`（F-15）与缺失 `version`（F-12）的处理 | 需要真机 BG3 + 官方本地化工具链；本轮只按 XML 语义与「不凭空造属性」原则处理 |
| `extract_package_files` 的「最后一个可读副本」是否与游戏加载器一致 | 需要真机/官方加载器实现；代码注释与实现一致（取最后一个可读副本），未改 |
| 上限取值（6 GiB / 16 GiB / 20 万）对超大真实 MOD 是否够宽 | 无 >6 GiB 的真实样本可测；取值基于「4K 材质 MOD 解包后数 GB」的量级判断，且只在超限时报错（不静默截断） |
| 声明大小撒谎 + 非 deflate（stored/zip64）组合下的实际写入层 | 只测了 deflate + 篡改声明大小的组合；zip64 大文件路径未构造样本 |

---

## §5 遗留风险

1. **F-15/F-16 是「零译文写回也会改字节」的仅存两处**。语义等价，但如果将来要做
   「不翻译就一个字节都不动」的严格保真，需要把 `content_list` 改成 `lsx` 那样的
   区间原地替换方案（是整模块重构，建议单独立项）。
2. **zip 上限是粗略的数量级防线**：真实 MOD 若真的超过 16 GiB 会被拒绝（用户可反馈调整常量）；
   磁盘写满（非恶意，单纯空间不足）仍会在解压中途失败——此时 `open_and_extract` 会清掉工作目录，
   但 `open_and_extract_in` 的调用方（命令层）需要自己保证清理（见跨范围发现）。
3. **`content_list` 的宽容度边界**：白名单外标签、属性顺序变化、`<br />` 与 `<br/>` 的
   形态差异会在第一次写回时被规范化（属性顺序会重排？——不会，`raw_start_tag` 按源顺序输出；
   自闭合的空格会被 quick-xml 规范化成 `<br/>`）。语义不变，但不保证逐字节幂等（第一次写回后稳定）。
4. **`.lsx` 的字段白名单是产品判断**：`Description/DisplayName/Title/Tooltip/TooltipDescription`
   之外的可翻译字段（若有）会被漏掉。本轮未发现真实 LSX 里有白名单外的玩家可见 LSSTring 字段，
   但这依赖 lslib 的字段命名习惯。
5. **上游依赖行为**：`PackageBuilder::add_directory` 跟随符号链接（F-14 已在我们这层挡住）、
   `LocaWriter` 对 key 长度 >64 会返回错误（已有防线）。这些不在本仓库可改范围。

---

## 跨范围发现（已 `send_message` 给 lead，未自行修改）

1. **`open_and_extract_in` 失败时不在原地清理工作目录**（`pak.rs`，本范围内但语义归命令层）：
   `open_and_extract` 会自动删；`open_and_extract_in` 不会。命令层若在失败路径上漏删，
   临时目录会残留（含半个解包树）。证据：`zipx` 探针里 `open_and_extract_in` 返回 Err 后
   `work/bg3-translate-*/zip_contents/bomb.bin` 仍在磁盘上。
2. **`PackageBuilder`（bg3rustpaklib 0.1.5，上游）跟随符号链接**：F-14 的根因，我们已用
   `repack` 前置扫描挡住；若要彻底修需改依赖，本轮不做。
3. 前端 `planLocalizationWrites`（`src/lib/localization.ts:70-93`）的「英文 > 中文」优先级
   会让**已有中文文件里未翻译的条目退回英文原文**（红队 R-05）——core 侧我已用 F-05 的
   合并策略保证「条目不丢」，但「已有中文译文被英文覆盖」仍归 frontend-auditor 决定。

---

## 附录 A：修复前失败清单（红→绿的「红」）

新增测试全部先写、先在**未修复**的代码上跑，得到 15 条 lib 失败 + 1 条 e2e 失败：

```console
$ cd /home/jason/bg3-translate && cargo test -p bg3-translate-core --all-targets --offline
test result: FAILED. 272 passed; 15 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.68s
    formats::content_list::tests::identity_attributes_with_control_characters_are_rejected
    formats::content_list::tests::illegal_control_characters_are_never_written_into_the_xml
    formats::content_list::tests::literal_angle_bracket_text_is_never_written_as_markup
    formats::content_list::tests::tag_attribute_value_containing_gt_survives_write_back
    formats::content_list::tests::write_keeps_entries_that_already_exist_on_disk
    formats::content_list::tests::writing_an_empty_list_does_not_wipe_an_existing_file
    formats::loca::tests::write_keeps_keys_that_already_exist_on_disk
    formats::loca::tests::writing_an_empty_list_does_not_wipe_an_existing_loca
    formats::lsx::tests::duplicate_entries_are_applied_once_instead_of_corrupting_the_value
    formats::lsx::tests::illegal_control_characters_never_reach_the_attribute_value
    formats::tests::read_entries_rejects_file_names_that_escape_unpacked
    formats::tests::write_entries_rejects_file_names_that_escape_unpacked
    pak::tests::safe_output_path_rejects_nul_bytes
    pak::tests::safe_output_path_rejects_trailing_dots_and_spaces
    pak::tests::walk_files_does_not_follow_symlinked_directories

$ cargo test -p bg3-translate-core --test e2e_pak_flow --offline
test existing_target_entries_survive_a_subset_write_back_and_repack ... FAILED
  panicked at tests/e2e_pak_flow.rs:576:
  只属于中文文件的条目被删掉了: <?xml ...><content contentuid="h11111111g2222u3333i4444" version="1">【译】Hello, adventurer.</content></contentList>
```

（F-12/F-15/F-16 的用例是修复过程中补的，同样在修复前红：见各条正文与探针输出。
`F-04`/`F-03`/`F-01` 的用例在「修复 F-01 的第一版实现」之后才补齐，红态证据是 §2 的探针输出。）

lead 插入项（属性名回归）的中间态：
```console
$ cargo test -p bg3-translate-core --all-targets
test mangled_tag_attribute_names_are_not_faithful ... FAILED
  panicked at tests/real_mod_sample.rs:212:
  属性名被改坏必须判为不保真: "<LSTag 类型=\"Spell\" Tooltip=\"Deals {1} damage\">Fireball</LSTag>"（问题: []）
（engine-auditor 在 fidelity.rs 落地「属性名多重集一致」校验后 → ok）
```

## 附录 B：变异测试（把修复改回原样，测试必须变红）

脚本：`/tmp/fa-mutate*.py`（逐条替换生产代码里的关键行 → 跑指定测试 → 记录 → 还原）。
**20/20 全部变红**：

| 变异 | 变红测试 |
|---|---|
| `scan_tag_end` 退回「第一个 `>`」 | `tag_attribute_value_containing_gt_survives_write_back` |
| `is_allowed_inline_tag` 放行畸形标签（`None => true`） | `literal_angle_bracket_text_is_never_written_as_markup`、`unknown_or_malformed_tags_are_escaped`、`pseudo_end_tag_with_space_is_treated_as_text` |
| 结束标签名 `trim_end()` → `trim()`（老行为） | `pseudo_end_tag_with_space_is_treated_as_text` |
| 去掉 `merge_missing_entries` | `write_keeps_entries_that_already_exist_on_disk`、`writing_an_empty_list_does_not_wipe_an_existing_file` |
| 总是写 `version` 属性 | `missing_version_attribute_is_not_invented_on_write_back` |
| 不去除非法控制字符（content_list） | `illegal_control_characters_are_never_written_into_the_xml` |
| 不规范化不闭合空元素 | `unclosed_void_tag_is_normalized_to_self_closing` |
| 去掉 `.lsx` 区间去重 | `duplicate_entries_are_applied_once_instead_of_corrupting_the_value` |
| 去掉 `.lsx` 重叠护栏 | 同上 |
| 不去除非法控制字符（lsx） | `illegal_control_characters_never_reach_the_attribute_value` |
| 去掉 `.loca` 合并 | `write_keeps_keys_that_already_exist_on_disk`、`writing_an_empty_list_does_not_wipe_an_existing_loca` |
| 退回未校验的 `resolve_disk_path` | `write_entries_rejects_file_names_that_escape_unpacked`、`read_entries_rejects_file_names_that_escape_unpacked` |
| 放行尾随点/空格 | `safe_output_path_rejects_trailing_dots_and_spaces` |
| 放行 NUL | `safe_output_path_rejects_nul_bytes` |
| `walk_files` 跟随符号链接 | `walk_files_does_not_follow_symlinked_directories` |
| 去掉单条目声明上限 | `zip_bomb_is_refused_before_it_fills_the_disk` |
| 去掉声明总量上限 | `zip_total_size_limit_is_enforced_across_entries` |
| 去掉实际写入上限 | `zip_with_a_lying_size_header_is_refused` |
| 去掉条目数上限 | `zip_entry_count_limit_is_enforced` |
| 去掉 `repack` 符号链接检查 | `repack_refuses_symlinks_inside_unpacked` |

（自查过程中发现 3 条测试最初**不会**被变异激活——`.lsx` 去重、zip 总量、zip 条目数——
已按「断言直击缺陷本身」重写：`.lsx` 增加 `plan.len() == 1` 断言；总量层断言错误信息来自
「声明总量」这一层（`将超过上限`）而不是实际写入层；条目数层断言「一个文件都没解压出来」。）

## 附录 C：最终门禁真实输出

```console
$ cd /home/jason/bg3-translate && cargo fmt --all --check
（无输出）
$ cargo clippy -p bg3-translate-core --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.84s
$ cargo test -p bg3-translate-core --all-targets
test result: ok. 326 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.45s   # lib
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    # e2e_pak_flow
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s    # real_mod_sample

$ cargo check -p bg3-translate --all-targets        # Tauri 壳集成
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.79s   # exit 0
```

修复后同一批探针的关键输出（`/tmp/fa-probe`，探针源码一字未改）：
```console
$ cargo run --offline -q --bin cl      # gt-in-attr/lt-br-gt/lt-i-gt: reparse error=None, ROUNDTRIP_STABLE=true
$ cargo run --offline -q --bin paths   # 越界路径全部 Err(Pak(...))；work/evil.loca 不再产生
$ cargo run --offline -q --bin misc    # plan (dup entries): [(85..88, "新一")]；控制字符 out bytes = []
$ cargo run --offline -q --bin pak2    # walk_files sees link target? false；repack with symlink -> Err(Pak(...))
$ cargo run --offline -q --bin zipx    # （修复后仍无上限的探针只用于对照，防线由单元测试覆盖）
```

## 附录 D：改动文件清单（仅限 T1 写范围）

| 文件 | 改动 |
|---|---|
| `crates/bg3-translate-core/src/formats/content_list.rs` | 标签扫描/校验重写（F-01~F-04）、非法字符过滤（F-07）、写回合并（F-05）、version 属性（F-12）+ **12 条回归测试** |
| `crates/bg3-translate-core/src/formats/loca.rs` | 写回合并已有 key（F-06）+ **2 条回归测试** |
| `crates/bg3-translate-core/src/formats/lsx.rs` | 重复/重叠区间防护（F-11）、非法字符过滤（F-07）+ **2 条回归测试** |
| `crates/bg3-translate-core/src/formats/mod.rs` | 读写路径统一走 `safe_output_path`（F-08）+ **2 条回归测试** |
| `crates/bg3-translate-core/src/pak.rs` | `safe_output_path` 加固（F-09/F-13）、`walk_files` 不跟随链接（F-14）、`repack` 拒绝符号链接（F-14）、zip 三重上限（F-10）+ **9 条回归测试** |
| `crates/bg3-translate-core/tests/e2e_pak_flow.rs` | 端到端条目不丢用例（F-05）+ 1 条 |
| `crates/bg3-translate-core/tests/real_mod_sample.rs` | lead 插入项：`rewrite_attribute_values` 只改属性值（保属性名）、新增 `mangled_tag_attribute_names_are_not_faithful` + 1 条 |

无新增依赖、无新增文件（除本报告）；未改动 `Cargo.toml` / `package.json` / `.gitignore` / `lib.rs` / `types.rs` / `error.rs` / `config.rs` / `glossary/**` / `translation/**` / `src/**` / `src-tauri/**`。
