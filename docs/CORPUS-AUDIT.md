# 独立验证报告（T4）：语料、写回风格与三道防线的证伪性核验

> 验证者：teammate `verifier`（独立于 T1/T2/T3/T5 的作者）
> 任务：`task-4`（依赖 task-1/2/3/5 全部 completed）
> 日期：本轮末尾；工作树状态见下
> 立场：**不复述结论，只找反例**。所有数字都在本机重新算过，作者的测试只当作"待证伪的对象"。

---

## 0. 基线与方法

**基线（验证开始前实测，未改动任何人的文件）：**

```
$ bash scripts/verify.sh
✓ 全部 6 道门禁通过（总耗时 10.0s）

$ cargo test -p bg3-translate-core --all-targets
unittests src/lib.rs      : 356 passed
tests/corpus_regression.rs:  16 passed
tests/corpus_writeback.rs :   7 passed
tests/e2e_pak_flow.rs     :   8 passed
tests/real_mod_sample.rs  :   8 passed
```

**方法：**

1. 自己写探针（临时文件 `tests/zz_verifier_probe.rs`，跑完**已删除**），所有统计逻辑独立实现，不复用 T1 的测试辅助函数，也不复用 `fidelity` 的内部扫描器；
2. 语料常量用**独立的 Python 实现**逐个复算（见 §3.3）；
3. 用**变异测试**验证测试台不是自证（改坏 → 必须变红 → 立即还原 → `sha256sum -c` 自查）；
4. 用**还原修复前行为**的变异复现 T5 报告的 66.01%。

**工作树自证（验证结束时）：**

```
$ git status --short
 M README.md
 M crates/bg3-translate-core/src/formats/content_list.rs
 M crates/bg3-translate-core/src/formats/mod.rs
 M crates/bg3-translate-core/src/translation/fidelity.rs
 M crates/bg3-translate-core/src/translation/mod.rs
 M crates/bg3-translate-core/src/translation/prompt.rs
 M crates/bg3-translate-core/src/translation/retry.rs
 M crates/bg3-translate-core/tests/e2e_pak_flow.rs
 M crates/bg3-translate-core/tests/real_mod_sample.rs
 M docs/ARCHITECTURE.md
?? crates/bg3-translate-core/tests/corpus_regression.rs
?? crates/bg3-translate-core/tests/corpus_writeback.rs
?? samples/english.xml
```

与验证开始前**逐条一致**；被临时改动的两个文件用备份 sha256 校验通过（见 §6）。

---

## 1. 结论速览

| # | 发现 | 分类 | 严重度 |
|---|------|------|--------|
| D1 | 模型输出已转义文本 → `restore_entities` 判保真 → 写回二次转义 → 落盘 `&amp;lt;`，且再读回仍保真。**T5 报告"会被保真度校验拦下"是错的**（既有测试 `restored_entities_are_not_reported` 断言的正是相反结论） | **确认为缺陷**（代码层可达、用户可见）；但 README/ARCHITECTURE 已把它记为已知取舍，靠提示词兜底 | 中 |
| D2 | `samples/english.xml` **未入库**（untracked，HEAD 里没有）；测试自身的救命提示 `git checkout -- samples/english.xml` 实际报 pathspec 错。本地全绿 ≠ CI 绿：干净 clone 上 18 个用例会红 | **确认为缺陷**（交付/流程） | 高 |
| D3 | 混合风格文件被统一成 Markup；真元素文件里 `<br>` → `<br/>`；Markup 文件里不配对的那条降级成转义文本 | **确认为刻意取舍**（有文档 + 有测试 + 真实语料不受影响） | 低 |
| D4 | T2 闸门刻意收窄：非标识符形态的 key 值（含空格/连字符/点/空值/`&`）、非白名单标签上的 key、无引号属性值一律不查 | **确认为刻意取舍**（语料内 0 处命中，未找到反例） | 低（设计） |
| D5 | 提示词「`&lt;` 游戏不认，会显示成字面标签」 | **无法验证**（游戏侧行为在仓库内无证据） | — |
| D6 | 「真元素形态其实也被游戏接受」 | **无法验证**；且即使游戏两种都认，写回翻转文件格式本身仍是缺陷，不构成不修的理由 | — |
| D7 | 三处文案精度问题（"降级成纯文本"的机制归属、ARCHITECTURE 用非白名单标签举例、legacy 测试复现的不是完整修复前行为） | 非缺陷（建议顺手订正） | 极低 |
| D8 | `content_list::write` 忽略 `parsed.error`：磁盘文件解析残缺时会把整份文件重写成残缺集合（实测被写成空 `<contentList/>`） | 低危观察（**当前不可达**：`read` 会先返回 Err） | 低 |

---

## 2. 发现详情

### D1（最高优先级，T5 报告中已确认的错误推理）转义形态的双重转义链

**这条链真实存在，且没有任何代码拦它。**

最小用例（`fidelity` 的公开 API，无需改代码即可复现）：

```rust
is_faithful(
    r#"Cast <LSTag Tooltip="HitPoints">hit points</LSTag>."#,
    r#"施放 &lt;LSTag Tooltip="HitPoints"&gt;生命值&lt;/LSTag&gt;。"#,
) // => true, issues == []
```

实际结果（探针输出，节选）：

```
is_faithful(source, escaped_model_output) = true
issues = []
write-out = ...<content contentuid="h1" version="1">施放 &amp;lt;LSTag Tooltip="HitPoints"&amp;gt;生命值&amp;lt;/LSTag&amp;gt;。</content>...
[double-escape-out] &amp;lt;=2 | 真标签 <LSTag=0 | &lt;LSTag=0
replayed text = "施放 &lt;LSTag Tooltip=\"HitPoints\"&gt;生命值&lt;/LSTag&gt;。"
is_faithful(source, replayed) = true          ← 落盘后再校验，仍然"保真"
```

期望结果：模型把真标签写成 `&lt;LSTag&gt;` 时必须被判**不保真**（或在校验前被修回字面尖括号），不能落盘成 `&amp;lt;` —— 游戏 XML 解码一次后玩家看到的是字面量 `&lt;LSTag Tooltip="HitPoints"&gt;生命值&lt;/LSTag&gt;`。

**为什么 T5 报告的判断是错的（有既有测试直接反证）：**

`crates/bg3-translate-core/src/translation/fidelity.rs:2001` 的既有测试 `restored_entities_are_not_reported`：

```rust
// 真正的标签也不会因为转义写法被判失败
assert!(is_faithful(
    r#"<LSTag Type="Spell">Fireball</LSTag>"#,
    r#"&lt;LSTag Type="Spell"&gt;火球术&lt;/LSTag&gt;"#
));
```

机制在 `fidelity.rs:594` 的 `restore_entities`：签名前把 `&lt;` / `&gt;` 还原成字面尖括号再比对，所以转义形态与真标签形态**签名完全相同**。T1 的测试台还把这件事钉成"合法译文"：`tests/corpus_regression.rs:732` 的 A3 `escaped_angle_brackets_are_still_faithful` 对**全部 1971 条**的转义变体断言必须保真。也就是说：**仓库里有两处绿灯在主动要求"转义形态必须放行"**，与 T3 提示词规则 4（禁止 `&lt;` 输出）在语义上相互矛盾。这是我这一轮找到的最实质的"绿灯下的缺陷"。

**是否只靠提示词兜着？——是，且只有提示词。**

- 校验层：放行（上文）；
- 修复层：`retry.rs:76` 只调 `repair_placeholders`（全角括号 / 括号类型），**不处理实体**；
- 写回层：`content_list.rs:380` 的 `escaped_text` 只做 `partial_escape`，`&` 必然被转义成 `&amp;`；
- 提示词：`prompt.rs:32` 规则 4 明确"必须用**字面尖括号**输出，不要写成 `&lt;LSTag ...&gt;`" —— 这是唯一防线。

**判断：确认为缺陷（不是"刻意取舍"就没事了）。** 理由有三：

1. 后果是**用户可见的损坏**（译文里出现字面标签），不是"字节不同但语义相同"；
2. 兜底成本全压在一次性的提示词上：真实语料 1971 条里 570 条含标记（29%），一旦模型某次"顺手转义"，这 570 条会**静默**变成字面量，且校验、重试、`Done` 事件、落盘全绿；
3. 修法很便宜（例如在 `retry` 层对"原文含真白名单标签、译文含 `&lt;标签名`"直接判不合格并给出纠错提示，或先 unescape 再校验），不需要动"转义等价"这条既有取舍本身。

**同时确认：这不是 T5 引入的回归** —— 修复前 `write` 走 `render`（Markup 风格）+ `BytesText::new`，同样会把模型输出的 `&` 转义成 `&amp;`。这是**预先存在**的缺陷，只是本轮首次把它连同"校验放行"一起说清楚。

---

### D2 `samples/english.xml` 没有入库（本地全绿 ≠ CI 绿）

```
$ git ls-files --error-unmatch samples/english.xml
error: pathspec 'samples/english.xml' did not match any file(s) known to git
$ git cat-file -e HEAD:samples/english.xml
fatal: path 'samples/english.xml' exists on disk, but not in 'HEAD'
$ git checkout -- samples/english.xml          # 测试自己给的救命指令
error: pathspec 'samples/english.xml' did not match any file(s) known to git
$ git check-ignore -v samples/english.xml      # 也不是被 .gitignore 挡了
(exit 1)
```

实际结果：

- `tests/corpus_regression.rs:110-118` 的 panic 文案写着「该文件**随仓库提交**（非 Git LFS），缺失说明 checkout 不完整；请执行 `git checkout -- samples/english.xml`」—— **这句话现在是错的**，那条命令必然失败；
- `.github/workflows/ci.yml` 的 core job 是 `actions/checkout` + `scripts/verify.sh`，其中 `cargo test -p bg3-translate-core --all-targets` 会跑到这批用例。按代码判定：`corpus_regression` 有 15/16 个用例走 `load_sources()`（只有"缺样本必须响亮失败"那条不走），`corpus_writeback` 有 3/7 个用例调 `corpus()`，**共 18 个用例在干净 clone 上会 panic/expect 失败**。

期望结果：`git add samples/english.xml`（343 KB，仓库已经在跟踪 5.8 MB 的官方术语表与 31 KB 的样本 zip，体积无压力）；退一步至少把 panic 文案改成"语料未入库/需从发布页获取"。

> 诚实标注：这 18 个是**按代码判定**得出的（我没有实跑无样本环境，因为纪律禁止改动 `samples/`）。"缺失时响亮失败"这条机制本身我验证过：`missing_corpus_fails_loudly_instead_of_skipping` 在基线里是通过的。

---

### D3 混合风格 / 空元素规范化（确认为刻意取舍）

实测（探针）：

| 输入 | `parse` 判定的风格 | 写回结果 | 判断 |
|------|-------------------|----------|------|
| 同文件 h1 转义、h2 真元素 | `Markup` | h1 **被翻成真元素**；再写一次稳定 | 刻意取舍，`content_list.rs:90-99` 有明文说明理由（真元素是 XML 层硬证据） |
| 真元素 `<br>`（不闭合） | 解析失败（非法 XML） | — | `read` 返回 Err，读数被拦下有明确报错 |
| 真元素 `<br/>` | `Markup` | 保持 `<br/>` | ✓ |
| 转义 `&lt;br&gt;` | `Escaped` | 保持 `&lt;br&gt;` | ✓ |
| 无标记 / 裸 `&lt;` / 空 content | `Escaped` | 逐字节不变 | ✓ |
| 真元素 + 裸 `&lt;` 同条 | `Markup` | 真标签保持、裸 `<` 转义 | ✓ |

`<br>` → `<br/>` 的量化：**只发生在真元素风格文件里**，且是必须的（不闭合的 `<br>` 会让整份文件非法 XML，`content_list.rs:639-651` 的 `normalize_void_tag`）。真实语料的**原始文件字节**里 `&lt;br&gt;` 410 处、字面 `<br>` 0 处、`<br/>` 0 处（纯转义风格），所以这条规范化对真实语料**零影响**（实测：写回后 `&lt;br&gt;` 仍 410 处、真 `<br` 仍 0 处）。

---

### D4 T2 闸门的收口与盲点（确认为刻意取舍，语料内无反例）

**闸门非空转，我独立复现了它的强度：**

| 检验 | 我的独立结果 |
|------|--------------|
| 语料 279 个 `Tooltip` 取值逐个改成中文 | 0 个漏报（279/279 拦下） |
| 语料 3 个 `Type` 取值逐个改成中文 | 0 个漏报 |
| 含 key 属性值的条目逐条变异（全部 key 值改中文） | **552 条全部报出，missed = 0**（与 T2 声称的 552 一致） |
| 语料里"不是标识符形态"的 `Tooltip` 取值 | **0 个**（所以语料内不存在这条盲点的命中样本） |
| 形态覆盖：`A_B` / `HP2` / `ALLCAPS` / `camelCase` / `_leading` / `1234` | 6/6 全部拦下 |
| 零误报：1971 条"标记逐字节照抄、只翻正文" | **1971/1971 保真，0 误报**；再加"把尖括号重新转义"变体，仍 0 误报 |

**刻意盲点（实测全部放行，确认存在且不会意外收紧）：**
`Tooltip="a natural sentence"`（含空格）、`Tooltip="Hit-Points"`（连字符）、`Tooltip="a.b"`（点）、`Tooltip=""`（空值）、`Tooltip="A&amp;B"`（实体）、`<foo Tooltip="HitPoints">`（非白名单标签）、`<LSTag Tag="Fire">`（刻意不收 `Tag`）、`<LSTag Tooltip=HitPoints>`（无引号）。

**结论：在真实语料上我找不到任何能绕过三道条件的 key 形态。** 能绕过的只有"非标识符形态"这一类，而这正是模块文档写明"宁可漏报，不可误报"的取舍，且语料里 0 处命中。**分类：确认为刻意取舍**；T2 的报告若把范围说成"属性值"而不限定"标识符形态"，需要收紧措辞。

---

### D5 / D6 两条"无法验证"

- **D5**：提示词说转义形态"游戏不认，会显示成字面标签"。仓库里没有任何游戏侧实测证据（既无 BG3 解析行为记录，也无用户报障样本）。方向正确、措辞合理，但**无法验证**。
- **D6**：任务允许我怀疑"真元素形态其实也被游戏接受"。我没有找到任何证据支持或反对。**但即便两种形态游戏都认，T5 的修复依然必要**：写回不变量是"零译文写回不该改变文件字节"，这是用户可直接 diff 的；66.01% → 98.68% 的字节一致率提升与"格式翻转 570 条"是可复现的客观事实（§3.1）。所以我不建议用"游戏可能都认"来降级这条修复的严重度。

---

### D7 三处文案精度问题（非缺陷）

1. **README.md:72 / fidelity.rs 模块头**：「写回层会把整条**降级成纯文本**」——这只在 **Markup 风格**路径成立（`render_with_style` 的 `Markup` 分支才调 `is_tag_balanced`）。真实语料是 **Escaped 风格**，那条分支根本不会被走到：写回是"整条文本统一转义"，不是"降级"。玩家看到字面标签这个**结论**没问题，机制描述不精确。
2. **ARCHITECTURE.md:243**（标签嵌套行）用 `<a></a><b></b>` ↔ `<b></b><a></a>`、`<a><b></b></a>` ↔ `<b><a></a></b>` 举例说明"合法换位都放行"。但 `<a>` **不在写回白名单**里，进不了签名，放行的原因不是"嵌套合法"而是"根本没被当成标签"——`fidelity.rs:1915` 的单测注释专门提醒过这一点（"注意必须用白名单标签"）。建议把例子换成 `<b>`/`<i>`。
3. **`tests/corpus_writeback.rs:259`** 的 `legacy_markup_render_still_flips_the_style_which_is_the_regression` 注释说"修复前的行为必须继续可复现"。实测它打印 **70.62%**（1392/1971），而修复前的真实数字是 **66.01%**（1301/1971，见 §3.1）—— 因为它现在跑的是"Markup 风格 + 已修好的 `escaped_text`"，只复现了**风格翻转**这一条，没有复现修复前的过度转义。测试只断言 `legacy_rt.identical > rt.identical`，**不构成假绿**，但口径容易被误读。

另：66.01% / 98.68% / 670 这三个数字**只被 `println!` 打印，没有任何断言**。文档数字与测试之间没有守卫，本轮它们恰好是对的（我复算了），但下次会漂移。

---

### D8 低危观察：`content_list::write` 忽略 `parsed.error`（当前不可达）

实测：对一个解析失败的磁盘文件调 `write(&path, &parsed.entries)`（`parsed.error = Some(...)`、`entries` 为空），文件被重写成：

```xml
<?xml version="1.0" encoding="utf-8"?><contentList></contentList>
```

即"解析到哪算哪"的残缺条目集合会覆盖整份文件（`merge_missing_entries` 拿到的 `on_disk` 也是残缺的）。

**当前不可达**：`read`（以及 `formats::read_entries*`）在 `parsed.error.is_some()` 时返回 Err，实测报文清晰：

```
read_entries_from_path -> Err(Xml("t.xml 解析失败，已中止读取以避免写回时丢失条目: ill-formed document: expected `</br>`, but `</content>` was found"))
```

所以 UI 拿不到条目、不会走到 `write`。属于**纵深防御缺口**（`write` 自己不复核 `parsed.error`），本轮不必修，但值得记一笔。

---

## 3. 对四个任务的逐项核验

### 3.1 T5：格式保真修复 —— **数字与生产链路全部复现，未找到反例**

**（a）66.01% 与 98.68% 都是真的，我独立重算了两个端点：**

98.68%（当前工作树，独立实现的区域提取）：

```
[in ] &lt;LSTag=1103 &lt;/LSTag=1103 &lt;br&gt;=410 <LSTag=0 <br>=0 &apos;=0
[out] &lt;LSTag=1103 &lt;/LSTag=1103 &lt;br&gt;=410 <LSTag=0 <br>=0 &apos;=0
identical=1945 / 1971 = 98.68%
26 条差异**全部**是"条目内尾随空格被 trim"，apostrophe=0 other=0
text_diff（逐条重新解析后的文本）= 0
```

66.01%：我用**还原修复前行为**的变异（把 `escaped_text` 换回 `BytesText::new`，即修复前的 `Event::Text` 语义）重跑，得到 **1301/1971 = 66.01%**，与文档逐位一致。

**（b）670 条的分解也逐项对得上（独立 Python 复算语料）：**

```
tagged（含 &lt; 的条目）= 570      → 风格翻转 570
plain 且正文含 ' 的条目 = 91      → &apos; 91
plain 且正文尾部带空白 = 9        → 尾空格 9
三者互不重叠（overlap = 0），570 + 91 + 9 = 670，1971 - 670 = 1301 ✓
（另有 17 条 tagged 条目也带尾随空白，但它们已计入 570）
```

**（c）生产链路确认走"跟随风格"那条：**

```
src/lib/tauri.ts:75  invoke("write_file_entries", …)
  → src-tauri/src/commands/entries.rs:80  formats::write_entries_to_path(...)
    → crates/.../src/formats/mod.rs:78    content_list::write(path, entries)
      → crates/.../src/formats/content_list.rs:218  write()
        → :237  render_with_style(&merged, &root_attributes, style)   ← 风格来自磁盘
```

我不满足于读代码，用**真实语料走了一遍生产 API**（`formats::read_entries` + `formats::write_entries`，临时工作目录）：

```
[prod-in ] &lt;LSTag=1103 <LSTag=0 &apos;=0
[prod-out] &lt;LSTag=1103 <LSTag=0 &apos;=0
production path identical=1945/1971 = 98.68%
翻译后写进**新建**的中文文件：&lt;LSTag=1104（新增的那条），真标签 = 0
```

结论：**生产链路与 `write` 同一条，风格跟随成立；"只有 render/write 被验证过"这条担心不成立。**

**（d）我构造的反例（全部没推翻修复）：** 混合风格（h1 转义 + h2 真元素）→ 统一成 Markup，与文档一致；无标记文件 → 不变；只有 `&lt;br&gt;` → 不变；真元素 + 裸 `&lt;` 同条 → 真标签保持、裸 `<` 保持转义；真元素文件 → 不反向翻转；空 `<content>` → 不变。`<br>` → `<br/>`、`'` → `&apos;` 在真实语料里的增量都是 **0**。

### 3.2 T2：属性值闸门 —— **非空转，552 条全部报出，无漏报反例**

见 §2 D4 的表。补充两条：闸门确实接在生产路径上（`retry.rs:258` `job_fidelity_issues` → `check_fidelity`），并且 `M5` 变异（把 `KEY_ATTRIBUTES` 清空）会让 T2 的反向用例立刻变红 —— 说明那些用例不是自证。语料证据我也逐条核过：`VENOMOUS_BARBS_CONDITION`→"Deadly Toxin"、`CAUSE_FEARED`→"Frighten"、`Projectile_MagicStoneThrow`→"Throw Magic Stone"、`ID_INSINUATION`→"Incapacitated"、`HitPoints`→"hit points"，`Type` 取值闭集 = {Spell, Status, Passive}。

### 3.3 T1：测试台 —— **数字全真、变异有效，但有两处交付/口径问题**

**（a）9088 个用例**：矩阵求和 `1971×3 + 288 + 303 + 552 + 349 + 303×3 + 303 + 54 + 129 + 288 = 9088` ✓（与 16 个 `#[test]` 的 `assert_enough` 常量逐一对应）。

**（b）"语料计数断言不是从被测代码推出来的"——成立，而且我复算得更严：** 计数走测试文件里**自己写的朴素扫描器**（`corpus_regression.rs:207/340`），只有"条目文本"来自 `formats::read_entries_from_path`（这是被测量对象本身，属于交叉校验）。我用**完全独立的 Python 实现**（正则 + 实体还原 + trim）复算了全部 19 个常量，**逐个命中**：

```
entries 1971 ✓  no_tag 1401 ✓  LSTag 开/闭/块 1103/1103/1103 ✓  br 410 ✓
entries_with_lstag 552 ✓  lstag_only 441 ✓  ge2_lstag 288 ✓  entries_with_br 129 ✓
two_adjacent_br 127 ✓  tooltip_attrs 1103 ✓  type_attrs 625 ✓  entries_with_type 349 ✓
placeholder_entries 303 ✓  placeholders 417 ✓  ph_with_tags 183 ✓  ph_without_tags 120 ✓
separable_ph_pairs 54 ✓  total_chars 158006 ✓  max_chars 581 ✓  tooltip_distinct 279 ✓
```

**（c）"样本缺失响亮失败"——成立**（`sample_path_at` 的 `assert!` + 专门的 `missing_corpus_fails_loudly_instead_of_skipping`，基线通过）；但**配套的救命指令是错的**（D2）。

**（d）"零误报不是自证"——成立**：我用自己的"保留标记、只翻正文"实现（独立于 T1 的 `translate_body_keep_markup`）跑全量 1971 条 → 0 误报；再叠一层"尖括号重新转义" → 仍 0 误报。

**（e）变异测试记录（三个检查点全红，全部已还原，见 §6）：**

| 变异 | 改了什么 | 期望 | 实测 |
|------|----------|------|------|
| M2 | `fidelity::is_well_nested` 开头插 `if true { return true; }` | B7 交叉嵌套用例变红 | `cross_nested_tags_are_reported ... FAILED`（exit 101）✓ |
| M3 | `render_tag` 里属性名不再进 token（`format!("<{name} {}", names.join(" "))` → `format!("<{name}")`） | B2 属性名用例变红 | `renaming_a_tag_attribute_reports_missing_and_extra_tag ... FAILED` ✓ |
| M4 | 停报"占位符缺失"（`if found < expected` → `if false && found < expected`） | B3 占位符用例变红 | `placeholder_loss_change_or_invention_is_reported ... FAILED` ✓ |
| M5 | `KEY_ATTRIBUTES = &[]` | T2 反向用例变红 | `key_attribute_values_are_not_faithful_when_translated ... FAILED` ✓ |
| M1 | `escaped_text` 换回 `BytesText::new`（还原修复前） | 复现 66.01%；`corpus_writeback` 多条变红 | legacy = **1301/1971（66.01%）**；`real_corpus_write_back…` / `plain_text_entries…` / `real_markup_file…` 三条 FAILED ✓ |

**（f）T1 测试台本身的两个问题：** ① 依赖的语料没入库（D2）；② A3 `escaped_angle_brackets_are_still_faithful` 把"转义形态"钉成**合法译文**，而 T3 提示词规则 4 明确禁止该形态（D1）——两处绿灯在语义上打架，需要 Lead 决定是"校验层补拦"还是"提示词降级为建议"。

### 3.4 T3：提示词审查 —— **没找到"诱导改 key"的措辞；发现 1 处判据比提示词宽**

逐条对照结果：

| 提示词主张 | 与代码/语料是否一致 |
|------------|--------------------|
| 规则 3「本游戏语料里的占位符是 `[数字]` 形态」 | ✓ 语料 417 处占位符**全是** `[N]`，`{…}` 0 处 |
| 规则 3「方括号和花括号不能互换」「编号不可改」「顺序可调」 | ✓ 与 `check_fidelity` 的多重集判据 + `repair_swapped_bracket_style` 一致 |
| 规则 3「相邻占位符必须保留分隔」 | ✓ 与 `GluedPlaceholders`（非对称：只报更粘）一致 |
| 规则 3「不要照抄英文词间空格」 | ✓ 空格本来就不参与比对（`glued` 只看"有没有东西隔着"），提示词更严 = 安全方向 |
| 规则 4「本语料里只有 `<LSTag ...>` 与 `<br>` 两种」 | ✓ 语料标签名集合确实只有 `LSTag`（1103 对）与 `br`（410） |
| 规则 4「属性名逐字照抄（`Type` 不能写成 `类型`）、大小写敏感」 | ✓ `render_tag` 的属性名多重集判据，大小写敏感 |
| 规则 4「属性值是查表 key，一律逐字照抄」 | ✓ 方向一致；校验器更宽（只收标识符形态的 `Tooltip`/`Type`），提示词更严 = 安全方向 |
| 规则 4「标签位置可随语序调整」 | ✓ 与"只查嵌套、不查顺序"一致 |
| 规则 4「开闭成对、不许交叉」 | ✓ 与 `is_well_nested` 一致（含"同名标签不存在交叉"这一条，见 §4） |
| **规则 4「`&lt;` 游戏不认」** | **无法验证**（D5）；但"禁止转义输出"这个要求本身是对的（D1） |

**判据比提示词宽的一处**：`fidelity` 还保护 `[全大写ID]`、`[IE_*]`、`{name}`/`{user_name}` 形态（`fidelity.rs:892-906`），而规则 3 只点名了 `[数字]` 与 `{1}`。总则"原文里的参数占位符必须**逐字照抄**"覆盖了它们，所以不构成"按提示词做却被拒"，但点名的形态与判据的形态不完全对齐，属低危提示词精度问题。

**"会诱导模型改 key 属性值"的措辞：没有。** 规则 4 三处都在说"逐字照抄""不是显示文本""不许翻译"，没有反向诱导；旧提示词里"标签内的英文内容（如 Tag 属性值）不要翻译"这种把属性值当作可翻文本的含糊说法已被替换。

---

## 4. Lead 三处改动的复核（`TagNestingBroken` 文案 / `is_well_nested` 文档 / README / ARCHITECTURE）

**结论：三处文案与代码实际行为一致，无夸大、无错报。** 逐条：

1. **`TagNestingBroken` 的 `Display` / `hint`**（`fidelity.rs:271`、`:229`）：
   - `Display` = "标签开闭不配对，嵌套不合法"，`hint` = "…（有落单的闭合标签，或不同标签名之间交叉嵌套），请让每个开始标签与它对应的闭合标签成对嵌套"；
   - 这两句话**恰好覆盖了可达的两种情形**。论证：这条检查的门槛是 `issues.is_empty()`（标签多重集已经相等）；多重集相等而非良构，只可能是①栈下溢（落单闭合）②弹出时名字对不上（不同名交叉）。"少一个闭合标签"必然先被多重集报掉，所以 hint 不提它是正确的，不是遗漏。
   - 两句文案都被 `fidelity.rs:1902-1910` 的断言逐字钉住。

2. **"同名标签之间不存在交叉"**（`fidelity.rs:531-534`、ARCHITECTURE.md:243）——**我实测确认成立**：
   - `<LSTag>a<LSTag>b</LSTag>c</LSTag>` 这类同名序列：我把第二个 `</LSTag>` 前后移动、把正文换位，只要开闭数量相等，**除了"闭标签出现在任何开标签之前"（栈下溢）之外没有别的失败形态**；同名标签之间**构造不出"交叉"**（交叉的定义要求两个不同的名字）；
   - 真正可检测的交叉必须是**不同名**标签：`<LSTag><b></LSTag></b>` → `check_fidelity` 返回 `[TagNestingBroken]`（我实测）；
   - `tests/corpus_regression.rs:974` 的 B7 用例名虽然叫"交叉嵌套"，实际构造的是**栈下溢**（闭标签前移）—— 该文件的注释（`:668-676`）已经把这个取舍写清楚了，与 Lead 的文案一致，不算错，但用例名容易误读，可以在报告口径上说明。

3. **README / ARCHITECTURE 的新数字**：66.01% / 98.68% / 570+91+9=670 / 26 条尾空格 / 279 个 Tooltip 取值 / `Type` 闭集 / 1513 处标记（= 1103 个 `<LSTag>` 对 + 410 个 `<br>`）—— **全部独立复算命中**（§3.1、§3.2）。仅有 §2 D7 的三处精度问题。

---

## 5. 为什么这些缺陷能同时满足"全绿"

这是本轮最值得记录的问题。逐条对齐"缺陷"与"为什么门禁抓不到"：

| 缺陷 / 取舍 | 为什么六道门禁全绿 |
|-------------|-------------------|
| D1 双重转义链 | **测试主动要求它绿**：`fidelity.rs:2001` 与 `corpus_regression.rs:732`（A3，全量 1971 条）都断言"转义形态必须判保真"。没有任何用例断言"落盘后再读回来的文本不该含 `&lt;`"，也没有用例走过"模型预转义 → write → 落盘字节"这条完整链。 |
| D2 语料未入库 | 本地文件在，所以本地全绿；门禁里没有任何一步检查"测试依赖的样本是否被 git 跟踪"。`git status` 里的 `??` 不会被任何门禁看见。 |
| D3 混合风格统一 / `<br>` 规范化 | 都是**有意的**行为，测试断言的就是"统一后"的结果（`mixed` 用例、`real_markup_file_is_written_back_as_real_markup`），绿灯与行为一致。 |
| D4 闸门盲点 | 盲点是文档写明的取舍，且**语料里 0 处命中**（279 个 `Tooltip` 取值全为标识符形态），没有样本能触发它。 |
| D7 文档数字 | 66.01% / 98.68% / 670 只被 `println!`，**没有任何断言**；legacy 测试只断言"修复后一致率更高"。文档错了也不会红。 |
| D8 write 忽略 parse 错误 | `read` 先返回 Err，正常流程走不到；没有测试直接对"解析失败的磁盘文件"调 `write`。 |

一句话：**全绿说明的是"被测代码与其测试一致"，不说明"代码与真实链路/文档一致"。** 本轮暴露的两类盲区正是后两者——(1) 端到端语义（落盘字节 + 游戏可见结果）没有用例，(2) 交付物（语料是否入库）没有门禁。

---

## 6. 纪律、临时改动与还原自证

**我没有修改任何人的文件。** 工作树在验证前后逐条一致（§0 的 `git status`）。

**临时改动只发生过 5 次（变异测试），每次都是"改 → 跑 → 立刻还原 → sha256 校验"，全部还原：**

| 变异 | 改动文件 | 行内改动 | 还原证据 |
|------|----------|----------|----------|
| M1 | `content_list.rs` | `escaped_text` 用 `BytesText::new` | sha256 `41952df5…f77fb9` OK |
| M2–M5 | `fidelity.rs` | 见 §3.3(e) 表 | sha256 `005f87de…e83e43` OK |

备份与校验清单在 `/tmp/verifier-backup/`（探针源码、`probe1.txt`/`probe2.txt`、变异输出）；`/tmp` 之外没有留下任何临时文件：

```
$ ls crates/bg3-translate-core/tests/
corpus_regression.rs  corpus_writeback.rs  e2e_pak_flow.rs  real_mod_sample.rs
$ sha256sum -c /tmp/verifier-backup/HASHES.txt
crates/bg3-translate-core/src/formats/content_list.rs: OK
crates/bg3-translate-core/src/translation/fidelity.rs: OK
```

**验证结束时的门禁复跑：**

```
$ bash scripts/verify.sh
✓ 全部 6 道门禁通过
```

---

## 7. 对四个任务的整体可信度评价

| 任务 | 可信度 | 依据 | 扣分点 |
|------|--------|------|--------|
| **T5** 格式保真修复 | **高** | 66.01% / 98.68% / 670 分解全部独立复现；生产链路（`formats::write_entries`）实测同一条；四类反例都没推翻修复 | 报告里"模型输出的转义文本会被保真度校验拦下"是**错的**（既有测试断言相反）；legacy 回归用例复现的不是完整修复前行为（70.62% vs 66.01%） |
| **T2** key 值闸门 | **高** | 552/552 全部报出（独立重算）、279+3 个取值全部拦下、1971 条零误报、6 种形态全覆盖、语料证据逐条核实 | 报告若把范围说成"属性值"需限定为"标识符形态"；闸门对非标识符形态/`Tag`/无引号值刻意不设防（有理由、语料内无命中） |
| **T1** 语料测试台 | **中高** | 9088 用例求和成立；19 个语料常量用独立实现逐个复现；M2/M3/M4 三个检查点变异均变红（非自证）；零误报我用独立实现复算仍 0 | ① 语料未入库 → 干净 clone/CI 会红 18 个用例，且 panic 文案给的救命指令是错的（**高危交付问题**）；② A3 把转义形态钉成"合法译文"，与 T3 提示词规则 4 直接冲突 |
| **T3** 提示词 | **中高** | 与 `fidelity`/`content_list` 逐条对齐，没有"会诱导改 key"的措辞；语料断言全部与语料一致；double-escape 只能靠提示词这件事它自己在注释里说清楚了 | 「游戏不认 `&lt;`」无证据（无法验证）；点名的占位符形态（`[数字]`）比判据（`[全大写]`/`[IE_*]`/`{name}`）窄 |

**给 Lead 的三条行动建议（按优先级）：**

1. **`git add samples/english.xml`**（否则 CI 的 core job 必红，且测试里的提示语在骗人）；顺手把 panic 文案改成与仓库事实一致的说法。
2. **决定 D1 怎么收口**：要么在校验/重试层补一条"原文含真标签 + 译文含 `&lt;标签名` → 不合格"（同时收敛 A3 用例的口径），要么明确接受"只靠提示词"，并把 README/ARCHITECTURE 里"这是已知取舍，不是漏网"改成"无代码防线，仅提示词"。目前的措辞（"校验判它保真、写回再转义一次"）是对的，缺的是"没有任何代码兜底"这句。
3. 顺手订正 §2 D7 的三处文案精度（尤其 ARCHITECTURE.md:243 的 `<a>`/`<b>` 例子），以及让 66.01%/98.68% 至少有一条断言或注释来源。

---

# 第二轮独立验证（T7）：T6 的「实体还原 → 统一转义」防线

> 验证者：teammate `verifier`（同一人，独立于 T6 作者 format-smith）
> 任务：`task-7`（阻塞于 task-6，已解除）
> 方法：独立探针（`tests/zz_verifier_probe2.rs`，9 个用例，跑完**已删除**）+ 变异测试 + Python `expat` 严格校验
> 临时改动：1 次（`decode_entities` no-op，用于复现修复前行为）、1 次（decode/sanitize 顺序对调）；两次都已还原，sha256 校验通过

## 8.1 结论速览

| # | 结论 | 分类 |
|---|------|------|
| **D9** | **Markup 风格下，标签属性值里的实体会被解码成裸字符，随后标签被「裸写」→ 产出非法 XML（整个本地化文件在游戏里作废）；`&quot;` 变体则整条降级成纯文本、标签丢失。零译文写回即可触发，不需要模型参与** | **确认为缺陷（T6 引入的回归，高）** |
| D1 闭合 | 转义风格下「模型输出 `&lt;LSTag&gt;` → 落盘 `&amp;lt;`」这条链**已经闭合**：走生产 API 实测落盘是 `&lt;LSTag`、`&amp;lt;` 0 处、再解析文本与原文一致 | 验证通过（T6 有效） |
| 两路规则 | 我自己造的 37 条实体表喂给**事件级（真 XML parse）**与**字符串级（写回还原）**两条路，**解码规则零分歧**；仅有的两处不对称都有文档且不可达（见 §8.4） | 验证通过 |
| 无绕过 | `content_list` 只有**一个** `Writer::new`（:307，在 `render_with_style` 内）；`render` / `render_with_style` / `write` 三个公开出口**全部**经过 `decode_entities`；`src-tauri` → `formats::write_entries_to_path` → `content_list::write`；tests 直接调 `render` 也走同一条。**没有任何调用点能绕开** | 验证通过 |
| 顺序 | 「decode 必须早于 sanitize」是真要求：把顺序对调 → T6 自己的用例 `entity_decoding_happens_before_illegal_character_filtering` **变红**（exit 101） | 验证通过（有测试钉住） |
| 语料 | 我独立复算 **1945/1971 = 98.68%**，`&lt;LSTag` 1103、`&lt;br&gt;` 410、真标签 0、`&apos;` 0、`&amp;lt;` 0 | 验证通过（T6 未误伤语料） |
| `&amp;lt;` 取舍 | 作者披露的"解开一层"确认存在；我补充量化：**每写回一次恰好降一层**，N 层文件要 N 次保存才收敛（见 §8.5） | **确认为刻意取舍**（语料 0 `&`，且有自愈价值） |

T6 自测口径我也复现了：`cargo test --all-targets` = **366 + 16 + 8 + 8 + 8 = 406 全绿**，`clippy -D warnings` exit 0，`cargo fmt --all --check` OK。

## 8.2 D9（确认为缺陷·高）：Markup 路径的标签属性被解码 → 产出非法 XML

### 复现（最小用例，零译文写回，无需模型）

输入是**合法 XML**、Markup 风格（文件里有真元素 `<b>`）：

```xml
<contentList><content contentuid="h1" version="1">a <LSTag Tooltip="a&amp;b">x</LSTag> b <b>keep</b></content></contentList>
```

```rust
let entries = formats::read_entries_from_path(&path, "x.xml", PakFileKind::LocalizationXml)?;
formats::write_entries_to_path(&path, PakFileKind::LocalizationXml, &entries)?;   // 原样写回
```

**实际结果**（T6 代码）：

| 输入（合法） | 写回产物 | Python `expat` 严格校验 |
|---|---|---|
| `Tooltip="a&amp;b"` | `Tooltip="a&b"` | **PARSE ERROR: not well-formed (invalid token), column 109** |
| `Tooltip="a&lt;b"` | `Tooltip="a<b"` | **PARSE ERROR: not well-formed (invalid token), column 107** |
| `Tooltip="a&quot;b"` | 整条降级：`&lt;LSTag Tooltip="a"b"&gt;x&lt;/LSTag&gt; b &lt;b&gt;keep&lt;/b&gt;` | OK，但**标签全丢**（玩家看到字面量） |
| Escaped 风格同样输入 | 逐字节不变 | OK（未受影响） |

**期望结果**：零译文写回**逐字节不变**（这是 T5 立下的写回不变量），产物永远是合法 XML。

独立校验命令（与 Rust 侧完全无关的严格解析器；把写出的文件路径替换进去）：

```bash
python3 -c "import sys,xml.etree.ElementTree as ET; ET.fromstring(open(sys.argv[1],'rb').read()); print('OK')" /tmp/out.xml
# 实际：xml.etree.ElementTree.ParseError: not well-formed (invalid token): line 1, column 107 / 109
```

### 证明这是 T6 引入的回归（不是既有行为）

把 `decode_entities` 变异成 `if true { return Cow::Borrowed(text); }`（= 修复前行为），同一组用例：

```
[markup_amp_attr]  写回 = ...a <LSTag Tooltip="a&amp;b">x</LSTag> b <b>keep</b>...   ← 与输入逐字节相同
[markup_lt_attr]   写回 = ...a <LSTag Tooltip="a&lt;b">x</LSTag> b <b>keep</b>...   ← 同上
[markup_quot_attr] 写回 = ...a <LSTag Tooltip="a&quot;b">x</LSTag> b <b>keep</b>...  ← 同上
```

即：**修复前这四种输入全部字节不变且合法；修复后前两种产出非法 XML。**

### 机制（根因，供修法参考）

两个**有意为之**的契约被 T6 打破：

1. `parse` 的 `raw_start_tag`（`content_list.rs:561`）用 `attr.value.as_ref()`——**故意保留属性值的原始转义形态**，所以读进来的文本里是 `Tooltip="a&amp;b"`，不是 `a&b`；
2. `write_text_fragment` 的注释与实现（`:589-620`）——**故意把白名单标签裸写**，注释写明「属性值里的 `&`/`"` 都是原始转义形态」。

T6 把 `decode_entities` 作用在**整条文本**上（`:337`），包括标签内部的属性值；于是第 1 条留下的转义在第 2 条裸写时被还原成裸 `&` / 裸 `<` —— XML 里这两个字符在属性值中必须转义，产物立刻非法。`&quot;` 那一格更早失败：解码出的裸 `"` 让 `parse_single_tag` 认不出标签，整条按纯文本转义。

### 为什么 406 个绿灯一个都没抓到

- T6 的 Markup 用例 `decoded_tags_are_written_as_real_markup_in_markup_style` 只把 `&amp;` 放在**正文**（`与 A &amp; B`），没有放在**标签属性**里；
- `samples/english.xml` 是 **Escaped** 风格且 `&` 0 处，98.68% 那条哨兵看不见；
- 本仓库自己的 `parse` 对非法产物**不报错**（quick-xml 宽松），所以 `read → write → read` 一路"成功"；
- 全仓没有一步调用严格校验器（如 expat）。

### 影响与不自愈性

实测连续 3 轮 `read → write`，产物字节**完全相同**（坏文件被稳定地一直写下去），不是"写一次就好"。触发面：

- 任何 Markup 风格、且白名单标签属性里含 `&amp;` / `&lt;` / `&quot;` / `&#38;` / `&#60;` 的 MOD —— **打开 + 保存就损坏**；
- 模型输出同样能触发：模型（正确地）写 `<LSTag Tooltip="a&amp;b">` 时，落盘即非法。

**建议修法方向（由 Lead 决定谁改）**：`decode_entities` 只解码**标签之外**的文本，标签区间内的字节保持原样（与 `write_text_fragment` 的裸写契约对齐）；或在写标签前对属性值重新做一次属性级转义。两种都能保住 D1 的修复效果（D1 场景里 `&lt;LSTag&gt;` 在解码前根本不是标签，属于"标签之外"）。

## 8.3 验证通过的项（含证据）

**① 端到端（生产 API，不是只调 `render`）**：夹具 `Localization/English/x.xml` → `formats::read_entries` → `mark_translated("施放 &lt;LSTag Tooltip=\"HitPoints\"&gt;生命值&lt;/LSTag&gt;。")` → `formats::write_entries`：

```
落盘 = ...<content contentuid="h1" version="1">施放 &lt;LSTag Tooltip="HitPoints"&gt;生命值&lt;/LSTag&gt;。</content>...
&lt;LSTag=1  &amp;lt;=0  真 <LSTag=0
再 parse 文本 = 施放 <LSTag Tooltip="HitPoints">生命值</LSTag>。
```

**② 幂等**：8 组输入（转义标签 / `&amp;` / `&lt;` / `&nbsp;` / 裸 `&` / `&amp;lt;` / `&amp;amp;lt;` / 无实体）各跑两轮 `write → read → write → read`，**每组的 F1 与 F2 逐字节相同**。

**③ 两路实体规则一致性**（我自己造表，非采信作者的 `entity_rule_table_pins_both_paths`）。表里含 `&amp; &lt; &gt; &quot; &apos;`、`&#65;`、`&#x41;`、`&#X41;`、`&#43;`、`&#+43;`、`&#-1;`、`&#0;`、`&#;`、`&#x;`、`&#xZZ;`、`&#xD800;`、`&#1114112;`、`&#1;`、`&#x0C;`、`&amp;amp;`、`&#38;lt;`、`&unknown;`、`&nbsp;`、`&AMP;`、`&LT;`、`&am p;`、`&#60;…&#62;`，以及残缺组 `&lt`、`&amp`、`&`、`&&`、`&&amp;`、`&;`、`a & b`。**解码规则零分歧**，两处不对称：

- 残缺引用（`&lt`、裸 `&`、`&&`）在**解析侧直接报错**（`entity or character reference not closed`）→ `read` 大声失败、文档被拒；字符串级保留原文。这是**必须**的不对称（模型输出这一侧没有解析器可依），作者的注释已经写明；
- `&#1;` 解析侧给出 `\u{1}`、写回侧给出 `""`——差额来自**既有的** `sanitize_xml_chars`（丢弃 XML 非法字符），两条**解码规则本身**仍一致；`&#x0C;` 同理由 `parse` 的 `trim` 解释。两者都是 T6 之前就有的行为，不构成本轮缺陷。

**④ 无调用点绕过 `decode_entities`**（Lead 补问的那条）：全仓扫描结果——`Writer::new` 在 `content_list.rs` 里只有一处（`:307`），就在 `render_with_style` 内；`pub fn render` / `render_with_style` / `write` 三个出口全部经过它；`formats::write_entries_to_path`（`formats/mod.rs:78`）→ `content_list::write`；`src-tauri/commands/entries.rs:80` → `formats::write_entries_to_path`；测试里直接调 `render` 的地方也走同一条。**结论：不存在旁路。**

**⑤ 顺序是硬要求**：变异对调 `decode_entities` / `sanitize_xml_chars` → T6 的 `entity_decoding_happens_before_illegal_character_filtering` 变红（`FAILED`，exit 101）。同时确认该变异**不影响**语料一致率（仍 98.68%），说明语料哨兵对这条顺序无感、必须有专门用例——作者补了。

**⑥ Markup 路径的其他子例**：`&lt;LSTag Tag="Fire"&gt;火球&lt;/LSTag&gt;` → 写成真元素 `<LSTag Tag="Fire">火球</LSTag>`（D1 在 Markup 侧也修好了）；数字引用造出的标签 `&#60;LSTag …&#62;` → 识别为真标签并写成元素（合法）；非白名单/畸形标签仍按纯文本转义。**除 D9 外没有发现别的 Markup 侧问题。**

## 8.4 边界攻击结论

| 攻击 | 实际结果 | 判断 |
|------|----------|------|
| 源文本里合法 `&amp;`（逻辑文本 `&`）往返 | 文件 `x &amp; y` → 文本 `x & y` → 写回 `x &amp; y`，三轮稳定 | ✓ 符合要求 |
| 模型输出双重转义 `&amp;lt;LSTag&amp;gt;` | 落盘 `&amp;lt;LSTag&amp;gt;`（**没有被解成真标签**），再解析文本 = `&lt;LSTag&gt;`；不产生标签，也不产生非法 XML | 可接受（见 §8.5） |
| 正文想显示字面 `&lt;`（`HP &lt; 5`） | 文件 `HP &lt; 5`；游戏 XML 解码一次 → 玩家看到 `HP < 5`；写回后字节不变 | ✓ 与修复前一致 |
| 转义形态出现在**标签属性内部** | Escaped：`&lt;LSTag Tooltip="a&amp;b"&gt;` 正确保持；**Markup：非法 XML / 标签被毁** | ✗ **D9，确认为缺陷** |
| Markup 路径整体 | 除 D9 外均通过（§8.3 ⑥） | 部分通过 |
| 未定义实体 `&nbsp;` | 文本 `x &nbsp; y` → 写回 `x &amp;nbsp; y` → 稳定三轮；语义等价 | ✓（既有取舍 R-07） |

## 8.5 `&amp;lt;` 过度还原：确认为刻意取舍（附我的量化）

作者自陈的取舍我复现并确认，且要补一句更准的量化：

- 文件 `x &amp;lt; y`（意图显示 `&lt;`）：第一次保存 → `x &lt; y`（意图丢失，玩家看到 `<`），之后稳定；
- 文件 `x &amp;amp;lt; y`：**每保存一次降一层**（`&amp;amp;lt;` → `&amp;lt;` → `&lt;` → 稳定），三层要 3 次保存才收敛。

也就是说"自愈旧版写坏的文件"和"降解本来想显示实体的文本"是**同一个机制**：写回把实体层数减一。判断为**可接受的刻意取舍**，理由：① 真实语料 1971 条 `&` **0 处**，影响面为 0（我复算 98.68% 未变）；② 方向选对了——旧版本自己写坏的 `&amp;lt;` 文件能自愈，反向取舍则永远修不好；③ 它会收敛（每层一次），不会无限退化。

**但请把文档措辞收紧**：现在的说法是"会被解开一层"，读起来像一次性的；实际是"每次写回降一层，直到没有实体形态为止"。对"想显示 `&lt;`/`&amp;`"的 MOD 属于数据损坏（虽然罕见），建议在 README/ARCHITECTURE 里写成"每次保存降一层 + 真实语料 0 命中 + 自愈旧文件"三条并列。

**另一条观察（低危）**：修复只发生在写回层。`Done` 事件与条目内存文本仍是模型输出的转义形态（`&lt;LSTag&gt;`），界面上用户看到的和落盘的不完全一致。落盘正确是任务要求，这条只是观感，若在意可在 `retry` 层顺手归一化。

## 8.6 对 T6 的整体可信度评价

| 维度 | 评价 |
|------|------|
| 修复效果（D1 链） | **达成**：Escaped 风格端到端证据齐全，我独立复现；生产链路无旁路 |
| 自测口径 | **如实**：366+16+8+8+8、clippy/fmt、98.68% 全部复现 |
| 规则一致性 | **达成**：我自己造表验证两路零分歧；顺序要求有测试钉住且变异可证 |
| 回归安全 | **未达成**：D9 是本轮新引入的高危回归（零译文写回即可产出非法 XML），发生在 Markup 路径，被"语料是 Escaped + 无 `&`"和"quick-xml 宽松"两层掩盖 |
| 取舍披露 | 诚实且理由成立，但量化需收紧（见 §8.5） |

**给 Lead 的行动建议（按优先级）：**

1. **D9 必须修**：`decode_entities` 不得解码标签内部的字节（或写标签前重建属性转义）。修完请补一条用例：Markup 夹具里属性含 `&amp;` / `&lt;` / `&quot;`，零译文写回必须**逐字节不变**且通过 expat 校验。
2. 把 `&amp;lt;` 取舍的措辞按 §8.5 收紧。
3. 上一轮的行动项 2 现在可以结题：**校验层仍在放行 `&lt;`（D1 的判据没变），但写回层已经能把它写对**——所以 README/ARCHITECTURE 里"没有任何代码兜底"要改成"由写回层 `decode_entities` 兜底（仅限落盘；`Done` 事件仍是转义形态）"。

---

# 第三轮独立验证（T10）：T9 的 D9 修复 与 T8 的 lsx 实体规则

> 验证者：teammate `verifier`（独立于 T9/T8 作者）
> 任务：`task-10`｜方法：独立探针（10 个用例，产出 57 个产物）+ **Python expat 严格校验** + 变异测试 + 真实 MOD 提取
> 基线：`bash scripts/verify.sh` 6/6 通过；`cargo test --all-targets` = **374 + 16 + 11 + 8 + 8 = 417 全绿**（我实跑）
> 临时改动：1 次变异（把 `Markup` 的分派改回"整条解码"），已还原，sha256 `a8ae26c8…dc09` 校验通过；探针 `zz_verifier_probe3/4.rs` 已删除

## 9.1 结论速览

| # | 结论 | 分类 |
|---|------|------|
| **D9** | **已闭合**。我用 task-7 的原最小复现重跑：Markup 零译文写回，`&amp;` / `&lt;` / `&quot;` / `>` 四种属性形态的 `<content>` 区间**逐字节不变**；产出的 **57 个文件 100% 通过 Python expat**。变异回"整条解码"后我的用例立刻变红 → 用例有效 | **验证通过（D9 闭合）** |
| 风格分派 | 风格 × 模型输出形态 **4 组组合全部正确**（Escaped 全解 → `&lt;LSTag …&amp;…&gt;`；Markup 跳过真标签 → `<LSTag …&amp;…>`）。**没有"两边都不对"的一组**。作者对"一律跳过会反向出错"的理由成立 | 验证通过 |
| D1 | 未回退：生产 API 端到端落盘一层转义、无 `&amp;lt;`、再解析 == 原文；连续 3 轮字节相同；语料仍 **1945/1971 = 98.68%** | 验证通过 |
| **D10** | 新发现（低危）：`&apos;` / `&#38;` / `&#x26;` 出现在**真标签属性里**时，零译文写回会把它们规范化成 `'` / `&amp;`（**字节变、语义不变、产物合法、一次性**）；单引号属性会被 normalize 成双引号（这一条来自 `parse`，**既有**） | **确认为刻意取舍（低危）**，附收紧建议 |
| 快路径 | `repair_tag_attributes` 的"只在非规范时才重写"经攻击后成立：无属性 / 多空白 / 属性顺序 / 单引号 / `Tooltip="a > b"` / 自闭合 / 未知实体 → **逐字节原样** | 验证通过 |
| 残留盲点 | 作者的披露属实：Markup + 整条转义 + 属性含 `&quot;` → 标签降级成字面文本。产物**合法、不丢数据、且有 `log::warn!`**，但该条富文本**功能失效** | **确认为刻意取舍（残留）**，升级路径见 §9.7 |
| T8 读侧 | 收紧的**不是 3 个而是 4 个**：`&#X41;` / `&#+65;` / `&#0;` / `&#x0;`；都是非法 XML 引用，收紧合规。真实 MOD 的两个 LSX **`&` 总数 0、撇号 0** → **零影响** | 验证通过（补充一个） |
| T8 往返 | `meta.lsx` / `Rulebook.lsx` 零译文写回**逐字节相同**；写回修复 3 组正常；全仓只有**一份**实体规则（lsx 的本地 `decode_entity` 已删） | 验证通过 |

## 9.2 D9 是否真修好（我自己的严格判据）

**最小复现**（与 task-7 同一组；Markup 风格：文件里另有真元素 `<b>keep</b>`）：

```rust
let es = formats::read_entries_from_path(&path, "x.xml", PakFileKind::LocalizationXml)?;
formats::write_entries_to_path(&path, PakFileKind::LocalizationXml, &es)?;   // 零译文写回
```

| 输入（合法 XML） | `<content>` 区间 | expat |
|---|---|---|
| `Tooltip="a&amp;b"` | **逐字节不变** | OK |
| `Tooltip="a&lt;b"` | **逐字节不变** | OK |
| `Tooltip="a&quot;b"` | **逐字节不变**（task-7 里这条会被降级成纯文本） | OK |
| `Tooltip="a > b"` | **逐字节不变**（含空格） | OK |

**57 个产物全部通过严格校验**（含畸形标签、非白名单标签、残留盲点、lsx 产物）：

```bash
for f in /tmp/verifier3/artifacts/*.xml; do
  python3 -c "import sys,xml.etree.ElementTree as ET; ET.fromstring(open(sys.argv[1],'rb').read())" "$f" || echo "BAD $f"
done
# 严格校验失败数 = 0（共 57 个产物）
```

> 说明：我刻意**不用**作者自建的 `strict_xml_check`（那是自证）。上面用的是 Python 标准库 expat（`xml.etree`）——与 Rust 侧零共享代码。

**变异测试（证明用例有效）**：把 `render_with_style` 里 Markup 的分派改回 T6 行为

```rust
MarkupStyle::Markup => decode_entities(entry.effective_text()),   // ← 变异（改成这样）
```

结果：我的 `q1 [quot]` 立刻变红，产物退化成
`a &lt;LSTag Tooltip="a"b"&gt;x&lt;/LSTag&gt; b &lt;b&gt;keep&lt;/b&gt;`（标签被毁、整条降级），
同时 `amp` 仍逐字节不变 —— 说明 **`&amp;` 由"跳过标签区间"和"`repair_tag_attributes`"两道机制共同保护，而 `&quot;` 只靠前者，跳过是承重的**。还原后 sha256 校验通过。

## 9.3 风格分派：4 组组合逐一验证（作者理由成立）

| 风格 | 模型输出形态 | 落盘 | 判断 |
|---|---|---|---|
| Escaped | `&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;` | `&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;` | ✓ 一层 |
| Escaped | `<LSTag Tooltip="a&amp;b">x</LSTag>`（真标签） | `&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;` | ✓ 一层（全解 → 统一转义） |
| Markup | `&lt;LSTag …&gt;`（整条转义） | `<LSTag Tooltip="a&amp;b">x</LSTag>` | ✓ 真元素 + 属性规范 |
| Markup | `<LSTag Tooltip="a&amp;b">x</LSTag>`（真标签） | `<LSTag Tooltip="a&amp;b">x</LSTag>` | ✓ 逐字节不变 |

**没有"两边都不对"的组合。** 作者的论证（"一律跳过"在 Escaped 下会把 `&amp;` 变成两层 `&amp;amp;`）我实测确认：Escaped 分支必须全解，因为随后 `escaped_text` 会统一转义。

顺带订正一个容易误读的点：Markup 下**读回来的条目文本**会保留属性里的 `&amp;`（`raw_start_tag` 故意保留原始转义），这与 Escaped 的"全解码"不同 —— 这是既有设计，不是不一致。

## 9.4 D10（确认为刻意取舍·低危）：非规范写法会被一次性规范化

零译文写回、Markup 风格，`<content>` 区间的实测差异（**语义完全相同、产物合法**）：

| 输入 | 写回 | 说明 |
|---|---|---|
| `Tooltip="a&apos;b"` | `Tooltip="a'b"` | `'` 在双引号属性里合法，新 helper 刻意不转义 |
| `Tooltip="a&#38;b"` | `Tooltip="a&amp;b"` | 数字引用 → 命名实体 |
| `Tooltip="a&#x26;b"` | `Tooltip="a&amp;b"` | 同上 |
| `Tooltip='a&amp;b'` | `Tooltip="a&amp;b"` | **单引号被归一成双引号 —— 来源是 `parse` 的 `raw_start_tag`，T9 之前就如此**（既有行为） |

**一次性**：连写 3 轮，第 2、3 轮字节完全相同（我用 5 个 churn 用例 + `lt` 反例跑了两轮对比，全部稳定）→ 不是每轮 churn。

**判断：可接受**（合法 XML、解析后文本不变、只发生一次、规范形态与源文件里其他标签一致），但要让"零译文写回逐字节不变"这条不变量更硬，建议把修复判据从
`escape(decode(v)) != v`（"非规范"）
收紧为
`v` 里真的出现**裸 `&` 或裸 `<`**（"非法 XML"）。
这样 `&apos;` / 数字引用 / 单引号属性都能原样保留，而 D9 的非法产物照样被堵住（`repair_tag_attributes` 的存在意义就是那两种裸字符）。

## 9.5 `repair_tag_attributes` 快路径攻击（结果）

| 攻击 | 结果 | 判断 |
|---|---|---|
| 无属性 `<b>` / 多空白 `<LSTag   Tooltip = "a&amp;b"  >` / 属性顺序 / 单引号 `Tooltip='a&amp;b'` / `Tooltip="a > b"` / 自闭合 / 未知实体 `&nbsp;` / 空值 | **逐字节原样**（快路径或"值未变 → None"） | ✓ 没有意外改写 |
| 裸 `&` / 裸 `<`（模型输出） | 规范化成 `&amp;` / `&lt;` | ✓ 输入本来就非法 XML，必须修 |
| 数字引用 / `&quot;`（模型输出完整标签） | 规范化成命名实体 | 同 D10 |
| 属性值里带 `>` + 带 `&` | 跳过（`scan_tag_end` 的引号处理正确） | ✓ |

## 9.6 标签区间扫描的边界

| 输入（Escaped 夹具，均可被 expat 解析） | 跳过？ | 结果 |
|---|---|---|
| 畸形 `<LSTag Tooltip=a&amp;b>`（无引号）/ 未闭合 / 非白名单 `<name a="x&amp;y">` | 不跳过（`is_allowed_inline_tag` 拒绝） | `&` 被还原后整条按文本转义 → 产物合法，文本往返一致 ✓ |
| `Tooltip="a > b&amp;c"`（`>` 在属性里） | 正确跳过整段 | ✓ |
| 相邻标签 `<b>1</b><i>2</i>` / 嵌套 `<b><i>1</i></b>` / `br` + 实体混排 | 逐个跳过 | ✓ |
| 正文实体 + 属性实体**同一条** | 正文解码、属性保留 | ✓ |
| `&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;`（解码后**变成**真标签） | 解码前不是标签 → 整条解码 → 变成标签 → `repair_tag_attributes` 把裸 `&` 修回 `&amp;` | ✓ 两道机制组合正确（这是最容易出错的路径，我专门验了） |
| Markup 下模型输出 `<LSTag Tooltip="a&amp;b"/>` 自闭合 | 跳过 | ✓ |

## 9.7 残留盲点评估（作者自陈的那条）

复现：Markup 风格 + 模型输出**整条转义** + 属性含 `&quot;` → 落盘
`&lt;LSTag Tooltip="say "hi""&gt;x&lt;/LSTag&gt;`（标签降级成字面文本）。

**判断：部分无害，但不应说成"完全无害"。**
- 无害的部分我确认：产物**合法 XML**（expat OK）、**不丢数据**（再解析文本 == 模型的逻辑文本）、走的是 `Markup` + 不配对的分支，**有 `log::warn!("条目 … 的译文标签不配对，已按纯文本写入…")`**，不是静默；
- 有代价的部分：该条的富文本标签**在游戏里失效**（玩家看到字面量），属于 D1 同类的可见缺陷，只是触发面窄（需要 Markup + 整条转义 + 属性里有引号类实体）。

**升级路径成立且不复杂**：根因是"把整串解码后再指望标签仍然可解析"。正确做法是在**解码前**按标签形状识别 `&lt;Tag attr="…"&gt;` 段，**逐属性解码、再逐属性重新转义**（`Tooltip="say &quot;hi&quot;"` → 解码 `say "hi"` → 重转义 `say &quot;hi&quot;`），而不是把 `"` 直接吐进属性区。这样这条也能写成真元素。属于可排期项，不是本轮阻塞。

## 9.8 T8（lsx）：写回修复、单一规则、读侧收紧的真实影响

**写回修复（生产 `write` 路径）**：

| 模型输出 | 落盘 | 判断 |
|---|---|---|
| `a &amp; b` | `value="a &amp; b"` | ✓ 一层（修复前会 `&amp;amp;`） |
| `&lt;LSTag&gt;x&lt;/LSTag&gt;` | `value="&lt;LSTag&gt;x&lt;/LSTag&gt;"` | ✓ 一层 |
| `a & b`（裸） | `value="a &amp; b"` | ✓ |
| `&#65;&#x42;` | `value="AB"` | ✓ |
| `a&#1;b` | `value="ab"`（控制字符按既有规则丢弃） | ✓ 与 `content_list` 的 sanitize 行为一致 |
| `a &nbsp; b` | `value="a &amp;nbsp; b"` | ✓ 未知实体保留 |

**单一规则（无第二份解码器）**：全仓扫描 `resolve_char_ref` / `"apos" =>` / `fn decode_entity` → **只有 `content_list::decode_reference_body` 一份**；`lsx.rs` 现在 `use super::content_list::{decode_entities, decode_reference_body}`，本地 `decode_entity` 已被删除（`git diff` 可见）。✓

**真实 MOD 实测**：`samples/Appearance Edit Enhanced-*.zip` → 内含 `.pak`（非普通 ZIP，用仓库的 `pak::open_and_extract_in` 解开）→ 2 个 LSX：

```
Mods/AppearanceEditEnhanced/meta.lsx                entries=1  零译文写回字节相同=true  总 & = 0  撇号 = 0
Public/AppearanceEditEnhanced/Shapeshift/Rulebook.lsx entries=0  零译文写回字节相同=true  总 & = 0  撇号 = 0
```

**读侧收紧的真实影响 = 0**：两个真实文件里**一个实体引用都没有**。我另做了规则级 diff（按 diff 复刻旧 `decode_entity` 对照），差异**不是 3 个而是 4 个**：

| 引用 | 旧 | 新 | 是否合法 XML |
|---|---|---|---|
| `&#X41;` | `A` | 原样 `&#X41;` | 否（十六进制只认小写 `x`） |
| `&#+65;` | `A` | 原样 `&#+65;` | 否（不支持符号） |
| `&#0;` | `\0` | 原样 `&#0;` | 否（码位 0 非法） |
| **`&#x0;`** | `\0` | 原样 `&#x0;` | 否（同上，作者报告里没提这一个） |

其余（5 个命名实体、`&#65;`、`&#x41;`、`&#0000065;`、`&#x1f600;`、`&#-1;`、`&#xD800;`、`&#1114112;`、`&#xZZ;`、`&#;`、`&nbsp;`…）**新旧完全一致**，合法引用读侧零变化 ✓。

**`'` → `&apos;` 的决定**：
- lsx 侧 `escape_attribute` **会**把 `'` 写成 `&apos;`（既有行为，T8 未改），`unescape_attribute` 能解回来 → 可逆、合法，真实数据 `'` 0 处 → 无影响；
- `content_list` 新增的 `escape_attribute_value` **不**转义 `'` → 也安全（重建的标签用双引号包裹，`'` 在里面合法；若值里真有 `"` 它会被转义成 `&quot;`）。我实测 `Tooltip='a&amp;b'`（模型输出）**逐字节原样**保留。
- 结论：**两处都不需要改**。

## 9.9 结论（task-10 要求的收口）

**D9 是否已闭合：是。**
① 我自己那组最小复现（Markup 零译文写回、`&amp;`/`&lt;`/`&quot;`/`>` 四种属性形态）现在是**逐字节不变**；
② 探针产出的 **57 个文件 100% 通过 Python expat**（不经作者自建校验）；
③ 把新判据变异回"整条解码"后，我的用例立刻变红 → 用例不是摆设；
④ D1 未回退（生产 API 端到端一层转义、3 轮幂等、语料 98.68%）；
⑤ T6 引入的 `&amp;`/`&lt;` 非法 XML 路径在修复后不可达（含"实体造标签 + 属性裸 `&`"这种组合路径）。

**是否引入新的迁移风险：没有发现"产出非法 XML / 丢数据"级别的新风险**，但有三条需要记录在案的低危项：
1. **D10**：真标签属性里的 `&apos;` / `&#38;` / `&#x26;` 会在第一次写回时被规范化（字节变、语义不变、一次后稳定）；单引号属性归一成双引号来自既有的 `parse`。若想严守字节不变量，把修复判据收紧为"值里有裸 `&` 或裸 `<`"即可（§9.4）。
2. **T8 读侧**：`&#X41;` / `&#+65;` / `&#0;` / `&#x0;` 四种非法引用不再解码（合规）；真实 MOD 两个 LSX 里实体总数为 0，因此**对现有数据零影响**；但如果有第三方 MOD 依赖旧行为，读出来的文本会从"解出的字符"变成"字面引用"——这是**好方向**（旧行为会把 `&#0;` 解成 NUL）。
3. **残留盲点**（§9.7）：Markup + 整条转义 + 属性含引号类实体 → 标签降级成字面文本；合法、不丢数据、有 warn 日志，但该条富文本失效。有明确的升级路径，建议排期。
