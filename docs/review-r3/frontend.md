# 前端审计报告（task-3）—— store / 流式渲染 / 虚拟滚动 / 组件

**审计者**：`frontend-auditor`
**写范围**：`src/**` + 本报告
**基线**：`git rev-parse HEAD` = `422f667a0d5d3be26190c0f5b55b0796a67a7b76`
（lead 给出的基线 worktree hash `ad8d3219378a5b89059a3558138ec7d1`；我审计的是同一 HEAD）

**开工前基线**：`bun run test` → 13 文件 / 155 用例全绿；`bun run build` 通过。
**收工时**：`src/**` 全量 md5 = `113f2897ba3fc1024db1980e7a252394`（`find src -type f | sort | xargs md5sum | md5sum`），
`bun run test` → **20 文件 / 190 用例全绿**，`bun run build` 通过（真实输出见 §6）。
本轮**没有删除或削弱任何既有测试**：既有 155 条用例逐条保留，新增 35 条。

> 贯穿本轮前端的核心跨层不变量（lead 定向提示 + `docs/ARCHITECTURE.md`「写回不变量」）：
> 后端 `TranslationEntry::has_writable_target()`（`crates/bg3-translate-core/src/types.rs:223`）
> **只在 `status == "error"` 时退回原文**；`status == "translating"` 的非空 `target`
> 会被 `effective_text()` 当成真译文写进 PAK。**「不留半截译文」这条只能由前端保证**，
> 所以下面 F-20 ~ F-23、F-25 都是围绕它来找反例的。

---

## §1 结论摘要表

| 编号 | 严重度 | 状态 | 一句话 |
|---|---|---|---|
| **F-20** | **高** | 已修 | 翻译进行中点「完成翻译，去打包」会把 `translating` + 半截译文写回 PAK（无任何闸门） |
| **F-21** | **高** | 已修 | 一轮结束后（取消/正常结束/命令已返回）迟到的 `delta` 会把条目重新点成 `translating` + 半截文本 —— 幽灵译文照样进 PAK |
| **F-22** | 中 | 已修 | 上一轮的迟到 `delta` 落到新一轮：已完成条目变成「权威译文 + 旧轮尾巴」，`translating` 状态被写回 |
| **F-23** | 中 | 已修 | 卸载工作台（返回首页 / 切 MOD）后，旧一轮的迟到 `progress` 与收尾回滚命中新 MOD 里**同 id** 的条目 |
| **F-24** | 中 | 已修 | `useEntryLoading` cleanup 无条件剔除已加载文件：每次新增勾选都重读全部文件（O(N²) IPC），并用磁盘内容**丢掉用户编辑与已翻译结果** |
| **F-25** | 中 | 已修 | 用户在流式中手工保存译文后，模型的 `delta`/`done`/`error` 仍会追加/覆盖人工成果，取消回滚还会把它清空 |
| **F-26** | 低 | 已修 | 文件对话框 reject 变成 unhandled Promise rejection：用户看不到任何反馈 |
| **F-27** | 低 | 已修 | 进度条没有 `role="progressbar"` / `aria-value*`，屏幕阅读器读不出翻译进度 |
| **F-28（= R-05）** | 中 | 已修 | 已有中文文件被英文回退覆盖（写回目标已存在时缺底稿合并）——详见 §2.9 专节 |
| F-29 | 信息 | 已确认未修 | `GlossaryPanel` 的 `refresh`/`onImport`/`onReset` 与 FileDropZone 同型（async onClick 未捕获 reject）。风险与 F-26 同级，未改（见 §5） |

统计：**高危 2 条、中危 4 条 + R-05、低危 2 条**，全部「修复前红 → 修复后绿」并有变异测试记录（§4）。

---

## §2 逐条缺陷

### 2.1 F-20（高）翻译进行中仍可写回：半截译文进 PAK

**现象**：点「翻译 N 条」开始流式翻译后，底部「完成翻译，去打包」**没有任何 running 判断**。
`translating` 条目的 `target` 里已经有半截流式文本，打包时 `planLocalizationWrites` 直接把它交给
`write_file_entries`；后端按 `status != error` 判定「有可写回译文」，半截句子就这样写进 PAK。

**复现**（真实输出，`src/App.write-guard.test.tsx`，修复前）：

```console
$ bunx vitest run src/App.write-guard.test.tsx
 ❯ 写回闸门（翻译进行中不得写回） (2 tests | 1 failed)
     × 翻译仍在进行时点「完成翻译，去打包」不得写回半截译文
AssertionError: expected "vi.fn()" to not be called at all, but actually been called 1 times
  1st vi.fn() call:
    Array [
      "/tmp/work",
      "Localization/Chinese/guard.xml",
      Array [
        Object { "contentuid": "uid-g-0", "status": "translating", "target": "半截译文", … },
        Object { "contentuid": "uid-g-1", "status": "translating", "target": "", … },
      ],
    ]
 Tests  1 failed | 1 passed (2)
```

**根因**：翻译运行态只存在于 `useTranslationRun` 的组件局部 state 里，`FilesPage`（打包按钮所在处）
完全看不到，所以两个 UI 之间没有任何同步点；`onGoPack` 直接写 store 里的条目。

**改动**：
- `src/store/app-store.ts`：新增 `runToken: number | null` + `beginRun()` / `endRun(token)`；
  `reset()` / `setModOpened()` 归零。用 **token 而不是 boolean**：只有登记它的那一轮才能关闭闸门
  （否则旧工作台的收尾会误关新一轮的闸门，见 F-23）。
- `src/components/translation-table/hooks/useTranslationRun.ts`：`runTranslation` 开始 `beginRun()`、
  `finally` 里 `endRun(runToken)`。
- `src/App.tsx`：`onGoPack` 开头硬校验 `useAppStore.getState().runToken !== null` → 弹错误横幅并返回；
  同时给按钮加 `title` 提示（按钮保持可点，点了会明确告诉你为什么不行，而不是静默失效）。
- `src/lib/entries.ts` + `src/lib/localization.ts`：纵深防御 —— `toWritableEntry()` 把任何残留在
  `translating` 的条目降级为 `pending` + 空 target（后端于是保留原文），`planLocalizationWrites`
  出口统一过一遍。

**回归测试**：`src/App.write-guard.test.tsx`
- `翻译仍在进行时点「完成翻译，去打包」不得写回半截译文`
- `取消后 all_done 已到但命令还没返回：打包仍被拦住，返回后条目全部回滚`
- `翻译结束后可以正常写回（闸门不能挡住合法流程）`（防过度拦截）
- `error 条目：打包时 status 必须保持 error（后端据此退回原文，不写坏 PAK）`
`src/lib/localization.test.ts` → `planLocalizationWrites 写回闸门（半截译文不得进 PAK）`（2 例）

---

### 2.2 F-21（高）一轮结束后的迟到 delta 复活半截译文

**现象**：`await translateEntries(...)` settle 之后（取消/正常结束都算），IPC 通道里仍可能有
`delta` / `progress` 迟到。此时 `cancelRequestedRef` 已在 `finally` 里被重置为 `false`、
`settledIdsRef` 里的信息也不再拦住它（条目从没收到过 `done`），于是迟到的 delta 被接受：
条目被改回 `status: "translating"` + 半截文本。收尾回滚**已经跑过**，之后没有任何代码会再清它 ——
用户此时打包，半截文本进 PAK（`translating` + 非空 target 是可写回的）。

**复现**（修复前，`src/components/translation-table/TranslationTable.late-events.test.tsx`）：

```console
$ bunx vitest run src/components/translation-table/TranslationTable.late-events.test.tsx
     × 取消并收尾之后到达的 delta 不得复活半截译文（否则会被写回 PAK）
     × 正常结束之后到达的 progress/delta 不得把 pending 条目重新点着
     × 上一轮的迟到 delta 不得污染新一轮里已完成的条目
 Tests  3 failed (3)
   Received: { "status": "translating", "target": "权威译文幽灵尾巴" }   ← 已完成的条目被追加了尾巴
```

**根因**：事件回调只在「取消窗口内」有守卫（`cancelRequestedRef`），**没有「这一轮还活着吗」的判定**；
`runningRef` 只在收尾时被置 false，回调却完全不看它。

**改动**：`useTranslationRun` 引入每轮唯一 `runId`（`runSeqRef` + `activeRunRef`）：
回调第一行 `if (activeRunRef.current !== runId) return;`，`finally` 里把 `activeRunRef` 置 `null`。
一个判定同时解决「本轮已结束」与「上一轮的事件落到新一轮」（F-22）。

**回归测试**：`TranslationTable.late-events.test.tsx` 三条用例（`取消并收尾之后到达的 delta …`、
`正常结束之后到达的 progress/delta …`、`上一轮的迟到 delta 不得污染新一轮里已完成的条目`）。

---

### 2.3 F-22（中）上一轮的迟到 delta 污染新一轮（与 F-21 同一修复点）

**现象**：第一轮 `done` 已写好的条目 A（`translated` + 权威译文）。第二轮（用户点翻译/重试）开始时
`settledIdsRef`/`attemptedIdsRef` 被整体重置，A 的「已定稿」信息丢失 → 第一轮的迟到 delta 到达时被接受：
`target = "权威译文 A（上一轮的尾巴）"`、`status = "translating"`。随后第二轮收尾回滚又会把 A 清成
`pending` + 空 target（用户丢译文）。

**复现/证据**：见 2.2 的第 3 条用例输出（`Received: {"target": "权威译文 A（上一轮的尾巴）"}`）。

**改动**：同 F-21（runId 失配即丢弃）。

---

### 2.4 F-23（中）卸载后旧一轮仍能改动新 MOD 的同 id 条目

**现象**：条目 id 是 `{PAK 内路径}#{contentuid}`（`crates/bg3-translate-core/src/types.rs:193`），
**同一个 MOD 重新打开后 id 完全相同**。用户在工作台翻译途中点「返回」回首页（工作台卸载，但后端命令
还在跑），再打开同一份 MOD → 旧一轮的迟到 `progress` 会把新条目改成 `translating`（会被写回），
旧一轮的收尾回滚会把新条目清成 `pending` + 空 target（丢新译文）。

**复现**（修复前，真实输出）：

```console
$ bunx vitest run src/components/translation-table/TranslationTable.mod-switch.test.tsx
 ❯ 翻译途中切换 MOD > 旧工作台的迟到事件与收尾回滚不得改动新 MOD 的同 id 条目
AssertionError: expected 'translating' to be 'translated'
Expected: "translated"
Received: "translating"
 Tests  1 failed (1)
```

**根因**：`useTranslationRun` 只知道「组件还活着」，没有「组件已卸载」的概念；
而 store 是全局的，事件回调拿到 id 就能继续写。

**改动**：`aliveRef` + 挂载时复位（兼容 StrictMode 的挂载→卸载→挂载）：
- 卸载后 `activeRunRef.current = null` → 迟到事件全部丢弃；
- `finally` 里 `if (!aliveRef.current)` 跳过取消/未完成回滚（否则回滚会误伤新 MOD 同 id 条目）；
- `endRun(runToken)` 仍然调用，但因为带 token 校验，旧一轮不会关掉新一轮的闸门。

**回归测试**：`TranslationTable.mod-switch.test.tsx` 1 例（同时断言 `runToken` 不被旧一轮清空）。

---

### 2.5 F-24（中）勾选新文件会重读全部已加载文件，并丢掉用户成果

**现象**：`useEntryLoading` 的 effect cleanup 对 `toLoad` **无条件**执行
`loadedRef.current.delete(name)` —— 包括那些**已经加载成功**的文件。于是每次新增勾选都会重读之前
已加载的全部文件（勾选 N 个文件共 O(N²) 次 IPC），并且 `setFileEntries` 会用磁盘内容**整体替换** store
里的条目：用户手工编辑过的译文、本轮已流式翻译出来的结果被静默丢弃。

**复现**（修复前，真实输出）：

```console
$ bunx vitest run src/components/translation-table/hooks/useEntryLoading.test.tsx
     × 新增勾选第二个文件时不会重读已经加载过的文件
      - Expected  + Received
        [ "Localization/English/a.xml",
      +   "Localization/English/a.xml",
          "Localization/English/b.xml" ]
     × 新增勾选不会用磁盘内容覆盖用户已编辑的译文
      AssertionError: expected { id: 'a-1', … } to match object { target: '手工译好的译文', status: 'edited' }
      -   "status": "edited",   +   "status": "pending",
      -   "target": "手工译好的译文",  +   "target": "",
 Tests  2 failed | 1 passed (3)
```

**根因**：cleanup 无法区分「请求还没回来（需要放回待加载集合，否则漏加载）」与「已经加载完成
（必须保持已加载）」。

**改动**：`useEntryLoading.ts` 增加 per-effect `settled` 集合，在每个请求的 `.finally()` 里登记；
cleanup 只剔除 `!settled.has(name)` 的文件名。

**回归测试**：`src/components/translation-table/hooks/useEntryLoading.test.tsx`
- `新增勾选第二个文件时不会重读已经加载过的文件`
- `新增勾选不会用磁盘内容覆盖用户已编辑的译文`
- `快速取消勾选时未完成的加载会被放回待加载集合（不会漏加载）`（守住原有语义）

---

### 2.6 F-25（中）手工保存的译文会被模型文本覆盖 / 追加，也会被收尾回滚清掉

**现象**：用户在流式过程中点「编辑 → 保存」（`status: "edited"`）之后：
1. 后续 `delta` 继续追加到人工译文上，状态被改回 `translating`；
2. `done` 直接用模型文本覆盖人工译文（**这是最常见的一条：`done` 几乎必然到来**）；
3. 取消/未完成回滚把人工译文清成 `pending` + 空 target；
4. `error` 会把人工译文标成 `error` —— 写回时后端退回原文，等于人工成果丢失。

**复现**（修复前，真实输出）：

```console
$ bunx vitest run src/components/translation-table/TranslationTable.edit-race.test.tsx
     × 保存之后到达的模型 delta 不得追加到用户译文上
     × 保存之后到达的 done 不得覆盖用户译文
        -   "status": "edited",   +   "status": "translated",
        -   "target": "用户手工译文", +   "target": "模型的权威译文"
     × 取消收尾回滚不得清掉用户已经保存的译文
        -   "status": "edited",   +   "status": "pending",
        -   "target": "用户手工译文", +   "target": ""
 Tests  3 failed (3)
```

（用例走**真实 UI**：`[data-index]` 行内点「编辑」→ 写 textarea → 点「保存」。）

**改动**：确立「人工成果优先」规则：
- `src/store/app-store.ts`：`applyDeltas` 跳过 `status === "edited"` 的条目；
- 新增 `getEntryById(id)`（O(1)，用现有索引）；
- `useTranslationRun`：`delta` / `done` / `error` 三个分支在写 store 前检查当前 `status === "edited"`
  （`done`/`error` 仍照常做 `settled/completed/inFlight` 记账，保证计数与收尾一致）；
- 两个收尾回滚循环都跳过 `edited` 条目。

**回归测试**：`TranslationTable.edit-race.test.tsx` 三条用例。

---

### 2.7 F-26（低）对话框 reject 变成 unhandled rejection

**现象**：`onClickPick` / `onClickExtract` 是 async 函数直接当 `onClick` 用，
`pickModFile()`/`pickExtractDirectory()` reject 时（Tauri IPC 不可用、权限问题）没有任何 catch：
用户看不到反馈，运行器把它报成 unhandled rejection。

**复现**（修复前，真实输出 —— 注意 vitest 直接报了两个 Unhandled Rejection）：

```console
$ bunx vitest run src/components/FileDropZone.test.tsx
 Tests  2 failed | 7 passed (9)
 Errors  2 errors
⎯⎯⎯ Unhandled Rejection ⎯⎯⎯
Error: 对话框不可用
```

**改动**：`FileDropZone.tsx` 两个 handler 用 try/catch 包住对话框调用 → `setError(String(e))`；
`onClickExtract` 保持原有状态时序（先拿两个对话框结果，再置 `extracting`），只增加错误通道。

**回归测试**：`src/components/FileDropZone.test.tsx` → `文件对话框 reject 时给出错误提示，而不是未捕获的
Promise rejection`、`仅解压时对话框 reject 也要有错误提示`。

---

### 2.8 F-27（低）进度条缺无障碍属性

**现象**：`src/components/ui/progress.tsx` 只渲染两个 div，没有 `role="progressbar"` 与
`aria-valuenow/valuemax/valuemin`，屏幕阅读器读不出翻译进度（这是长任务里唯一的进度信号）。

**改动**：`progress.tsx` 加 `role="progressbar"` + `aria-valuemin={0}` + `aria-valuemax={max}` +
`aria-valuenow={value}`；`TranslationToolbar.tsx` 传 `aria-label="翻译进度"`。
**回归测试**：`src/components/ui/progress.test.tsx`（2 例，含宽度钳制 0–100%）。

---

### 2.9 F-28 = R-05（中）已有中文文件被英文回退覆盖 → 写回底稿合并

**现象**（红队独立发现，lead 裁定必修）：MOD 同时带
`Localization/English/x.xml`（Fireball / Ice）与 `Localization/Chinese/x.xml`（火球 / 寒冰）。
两者都映射到写回路径 `Localization/Chinese/x.xml`，`planLocalizationWrites` 按优先级
（英文 3 > 中文 2 > 其它 1）让英文胜出；英文文件里**未翻译**的条目写回时 `effective_text()` 退回英文原文，
把文件里已有的中文覆盖成英文。`FileTree` 还有一键「全选」，这条路非常好走。

**复现**（修复前，真实输出）：

```console
$ bunx vitest run src/App.localization-merge.test.tsx
     × 英文 + 中文都勾选：未翻译条目保留文件里已有的中文
        -   "火球",   +   "Fireball",
        -   "寒冰",   +   "Ice",
     × 只勾英文、MOD 自带中文：同样走合并（更常见的路径）
     × 已翻译的条目用新译文，未翻译的条目保留底稿中文
     × error 条目不得把底稿中文覆盖掉（也不得写回被拒译文）
     × 底稿有、英文文件里没有的 contentuid 要保留
     ✓ MOD 里不存在目标中文文件时行为不变（首次生成中文文件）
 Tests  5 failed | 1 passed (6)
```

**改动**（按 lead 指定口径，不动后端 `write_file_entries` 语义）：
- `src/lib/localization.ts` 新增纯函数
  `mergeWithExistingTarget(incoming, existing)`：contentuid 级合并 ——
  ① incoming 有可写回译文 → 用 incoming；② incoming 没有可写回译文（target 空 / `error` /
  `translating`）→ 底稿同 contentuid 且文本非空时**保留底稿文本与状态**；
  ③ 底稿有、incoming 没有的 contentuid 保留；④ 顺序 = incoming 在前、底稿独有在后。
  入口先过 `toWritableEntry` 归一化（`translating` 的半截文本永远赢不过底稿）。
- `src/App.tsx` `onGoPack`：仅当 **目标路径确实存在于 `files`**（= MOD 自带该文件）时，
  先 `readFileEntries(workDir, plan.fileName)` 取底稿再合并；**读失败不阻断写回**，
  退回「直接用计划条目」的既有行为，并 `console.warn` 留痕。

**回归测试**：
- 端到端（App 级，7 例）：`src/App.localization-merge.test.tsx` —— 含红队场景、
  **只勾英文**（更常见路径）、新译文优先、`error` 不覆盖底稿、底稿独有 contentuid、
  **目标文件不存在时行为不变（护栏）**、**底稿读取失败不阻断写回**。
- 纯函数（6 例）：`src/lib/localization.test.ts` → `mergeWithExistingTarget（写回已有文件的底稿合并）`。

---

## §3 被证伪的怀疑点（都跑过命令）

| 怀疑点 | 结论 | 命令与证据 |
|---|---|---|
| 过滤后虚拟列表 index 与原始数组错位 | **证伪** | 新增 `src/components/TranslationTable.test.tsx` → `过滤后虚拟列表的行与条目一一对应（index 不错位）`：切「出错」后逐行校对 `Entry number 4k+3` 与 `uid-4k+3`，`✓ 8 tests`。 |
| 取消时「未 flush 的 delta 会在回滚之后迟到写回」（lead 疑问 2） | **证伪**：`finally` 里 `batcher.discardAll()` 先 `cancelFrame()` 再清空，且它跑在任何后续 flush 之前 | 既有 `TranslationTable.streaming.test.tsx` → `取消后未完成的条目回滚为待翻译，缓存的 delta 不会写回` 仍绿；`cancel 后`的迟到 delta 由 F-21 的 runId 校验兜住。 |
| 后端 `all_done` 可能在命令 Promise 之后才到达（那 F-21 的「返回后一律丢弃」就会丢 `run_summary`） | **证伪** | `sed -n '150,225p' crates/bg3-translate-core/src/translation/engine.rs`：`plan.total == 0` 分支与正常分支都是 `sink.emit(AllDone{..})` 后才 `return Ok(..)`，即命令返回前 channel 一定已发出。 |
| `localization.ts` 只按 `/` 切分，Windows 反斜杠路径会绕过改写（可能写到错误文件） | **证伪**：后端在构造 `PakFile.name` 前已归一化 | `grep -n "normalize_entry_name" crates/bg3-translate-core/src/pak.rs` → `pub fn normalize_entry_name(name: &str) -> String { name.replace('\\\\', "/") }`，且 `to_pak_file()` 里 `name: normalize_entry_name(file.name())`。 |
| delta 批处理在组件卸载时泄漏 rAF/setTimeout | **证伪** | `src/lib/delta-batcher.ts`：`flush()` 先 `cancelFrame()`；`dispose()` → `flush()`；`useTranslationRun` 卸载 effect 调 `batcher.dispose()`。既有 `delta-batcher.test.ts`（13 例）覆盖调度器注入与取消。 |
| F-20 的修复会挡住「取消后正常打包」的合法流程 | **证伪** | `App.write-guard.test.tsx` → `取消后 all_done 已到但命令还没返回…`：命令返回后 `runToken === null`，再点击打包成功写出 `[["uid-g-0","","pending"],["uid-g-1","","pending"]]` 且 `stage === "done"`。 |
| 「error 条目写回必须退回原文」这条闸门被前端破坏 | **证伪**（前端原样保留 `status: "error"` 与半截 target） | `App.write-guard.test.tsx` → `error 条目：打包时 status 必须保持 error…`：payload = `[["uid-g-0","error","半截译文"],["uid-g-1","translated","完整译文 2"]]`。 |
| 2 万条下 `applyDeltas` 是 O(N) 扫描 | **证伪** | 既有 `app-store.test.ts` → `updateEntry / appendDelta / applyDeltas 不再线性扫描数组`（`findIndex` 调用数 0）；`streaming.perf.test.ts` 打印 `[perf] locate: scan=… index=…`。 |

---

## §4 变异测试记录（我自己先做了一轮）

方法：用 python 就地把修复改回原样 → 跑指定用例 → `cp` 还原（**没有**用任何 git 命令）。

| # | 变异 | 结果 |
|---|---|---|
| M1 | 删掉 `useTranslationRun` 的 `if (activeRunRef.current !== runId) return;` | `late-events` 3/3 红 ✅ |
| M2 | 让卸载失效逻辑不生效（`aliveRef` 恒 true） | `mod-switch` 红（`expected 'translating' to be 'translated'`）✅ |
| M3 | 删掉 `onGoPack` 的 `runToken` 闸门 | `App.write-guard` 2/4 红 ✅ |
| M4 | `toWritableEntry` 直接 `return entry`（不降级 translating） | `localization.test.ts` 1 例红（`+ "半截流式文本"`）✅ |
| M5 | `useEntryLoading` cleanup 无条件 delete | `useEntryLoading` 2/3 红 ✅ |
| M6 | `endRun` 不校验 token（无条件清 `runToken`） | `mod-switch` 红（`expected null not to be null`）✅ |
| M7 | `App.tsx` 不合并底稿（`const entries = plan.entries`） | `App.localization-merge` 6/7 红 ✅ |
| M8 | `mergeWithExistingTarget` 不保留底稿分支 | `localization.test.ts` 4 例红 ✅ |

> 说明：M2/M6 的用例都做了「旧一轮 / 新一轮」双实例构造，所以它们不是「同义反复」的断言。

---

## §5 无法验证项与遗留风险

**无法在本机验证**
1. **真实 Tauri IPC 的消息投递顺序**：本机不跑 Tauri 后端，`delta`/`all_done` 与命令 Promise 的相对顺序
   用的是受控 mock + `engine.rs` 的代码路径论证。若真实 Channel 之间存在**乱序**（例如 `all_done` 真在
   命令 Promise 之后到达），F-21 的「一轮结束后一律丢弃」会顺带丢掉 `run_summary`（界面少一行「上次任务 N 条」），
   不会造成数据损坏。**建议 T6 在 Windows/macOS 真机上抽查一次 run_summary 是否仍显示。**
2. **概率性窗口宽度**：F-21/F-23 都是「迟到消息」类缺陷，真实发生率取决于 IPC 延迟与用户手速；
   我用确定性用例证明了缺陷的存在与修复的有效性，但没有量化线上概率。
3. **Windows 路径行为**（反斜杠、大小写、8.3 短名）无法在本机验证；前端侧只用到 PAK 内路径
   （后端已归一化），故未改。

**已确认未修（低危，记录在案）**
4. F-29：`GlossaryPanel` 的 `refresh` / `onImport` / `onReset`（`GlossaryPanel.tsx:68,142,131`）与
   `onSave` 一样是 async onClick，但对话框/文件读取的 reject 未被捕获 —— 与 F-26 完全同型。
   影响面是「术语表面板的对话框失败静默」，不会碰 MOD 数据，故本轮不改（留作已知取舍）。
   `onSave`/`onDelete`/`onReset` 已有 try/catch，只有 `openDialog`/`readTextFile` 那两处 `await` 裸露。
5. `splitErrorLines("   ")` 返回 `[""]` 会让 `ErrorBanner` 渲染一个空行（`error-banner.test.ts` 已固化该行为）：
   纯展示问题，未改以免动既有断言。

**性能**
6. `src/store/streaming.perf.test.ts` 与 `TranslationTable.streaming.test.tsx` 的 `[perf]` 数字本轮波动明显
   （`direct store_ms` 252 → 1012ms），原因是同时有 4 个 writer 在跑 cargo build / test，机器负载高 —— 
   **结构性断言未变**（batched 通知数 30 vs 3000、端到端 1 次通知、`notifications=1`）。
   我的改动不在热路径上（`applyDeltas` 多了一次 `status === "edited"` 判断；`entryIdToIndex` 索引仍是 O(1)）。
7. `planLocalizationWrites` 现在对每个 plan 做一次 `entries.map(toWritableEntry)`（20 万条量级下是一次
   O(N) 数组分配）。实测打包是一次性动作，可接受；若后续要省，可改成 `some()` 预检后再 map。

---

## §6 收工门禁（真实输出）

```console
$ cd /home/jason/bg3-translate && bun run test && bun run build
 ✓ src/lib/localization.test.ts (28 tests) 38ms
 ✓ src/store/app-store.test.ts (33 tests) 99ms
 ✓ src/lib/utils.test.ts (8 tests) 50ms
 ✓ src/lib/delta-batcher.test.ts (13 tests) 35ms
 ✓ src/lib/entries.test.ts (29 tests) 50ms
 ✓ src/components/translation-table/hooks/useEntryLoading.test.tsx (3 tests) 208ms
 ✓ src/components/ui/progress.test.tsx (2 tests) 183ms
 ✓ src/components/FileTree.test.tsx (7 tests) 709ms
 ✓ src/components/translation-table/TranslationTable.mod-switch.test.tsx (1 test) 463ms
 ✓ src/components/FileDropZone.test.tsx (9 tests) 588ms
 ✓ src/components/SettingsPanel.test.tsx (8 tests) 761ms
 ✓ src/components/translation-table/TranslationTable.late-events.test.tsx (3 tests) 664ms
 ✓ src/components/translation-table/TranslationTable.edit-race.test.tsx (3 tests) 1115ms
 ✓ src/components/ui/error-banner.test.ts (5 tests) 20ms
 ✓ src/components/TranslationTable.test.tsx (8 tests) 1630ms
 ✓ src/App.write-guard.test.tsx (4 tests) 1446ms
 ✓ src/App.localization-merge.test.tsx (7 tests) 1522ms
 ✓ src/components/GlossaryPanel.test.tsx (11 tests) 1738ms
 ✓ src/components/translation-table/TranslationTable.streaming.test.tsx (7 tests) 3125ms
 ✓ src/store/streaming.perf.test.ts (1 test) 5488ms

 Test Files  20 passed (20)
      Tests  190 passed (190)
   Start at  16:33:00
   Duration  8.42s (environment 51%, tests 25%, transform 13%, import 10%, setup 1%)

=== BUILD ===
vite v8.3.1 building client environment for production...
✓ 2007 modules transformed.
dist/index.html                   0.40 kB │ gzip:   0.29 kB
dist/assets/index-DSUdqI8X.css   44.12 kB │ gzip:   8.63 kB
dist/assets/index-DyFj7FAk.js   466.61 kB │ gzip: 144.43 kB
✓ built in 288ms
```

（`bun run build` = `tsc -b && vite build`；无 `any` / `@ts-ignore` / `eslint-disable` /
`as unknown as`；未新增依赖；未改 `package.json` / `vite.config.ts` / `tsconfig.json`。）

**改动文件清单**（全部在 `src/**`）
- 生产代码：`src/App.tsx`、`src/lib/entries.ts`、`src/lib/localization.ts`、`src/store/app-store.ts`、
  `src/components/translation-table/hooks/useTranslationRun.ts`、
  `src/components/translation-table/hooks/useEntryLoading.ts`、`src/components/FileDropZone.tsx`、
  `src/components/ui/progress.tsx`、`src/components/translation-table/TranslationToolbar.tsx`
- 新增测试：`src/App.write-guard.test.tsx`、`src/App.localization-merge.test.tsx`、
  `src/components/translation-table/TranslationTable.late-events.test.tsx`、
  `src/components/translation-table/TranslationTable.mod-switch.test.tsx`、
  `src/components/translation-table/TranslationTable.edit-race.test.tsx`、
  `src/components/translation-table/hooks/useEntryLoading.test.tsx`、
  `src/components/ui/progress.test.tsx`
- 追加用例（未删改既有用例）：`src/lib/localization.test.ts`、`src/components/FileDropZone.test.tsx`、
  `src/components/TranslationTable.test.tsx`

**跨范围发现（写给 lead / T6）**
1. 后端 `TranslationEntry::has_writable_target()` 把「半截译文是否进 PAK」的责任完全交给前端；
   本轮前端补了 4 道闸门（runId 校验、alive 失效、token 写回闸门、写回前 `toWritableEntry` 降级）。
   **若 T1/T4 将来改动 `write_entries` 的 status 语义，请同步告知前端**：目前前端依赖
   「只有 `error` 会退回原文」这一条。
2. `docs/ARCHITECTURE.md` 的「写回不变量」只写了「取消/收尾回滚」一条路径；
   实际还需要「迟到事件」「卸载/切 MOD」「手工编辑优先」「打包闸门」四条，建议在 T7 汇总时补进文档。
3. R-05 的修复需要在写回前**多一次 `read_file_entries` IPC 调用**（仅当 MOD 自带目标文件时）。
   这是前端新增的一次 IPC，`scripts/check_ipc_contract.py` 不涉及，但 T6 若核对 IPC 调用次数需知悉。
