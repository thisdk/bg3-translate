# 前端审计报告（R4 / task-3）—— store / 流式渲染 / 虚拟滚动 / 组件 / 写回闸门

**审计者**：`auditor-web`
**写范围**：`src/**`（含测试）+ 本报告
**基线**：`git rev-parse HEAD` = `d8295da`（= tag `v1.1.5`），工作树干净；

```console
$ cd /home/jason/bg3-translate && find src crates src-tauri scripts .github README.md package.json Cargo.toml \
    -type f -not -path '*/target/*' -not -path '*/__pycache__/*' | sort | xargs md5sum | md5sum
ad513e30c1e335f2a1f9cd669f6aa7ed  -
```

**开工前基线**：`bun run test` → **20 文件 / 190 用例全绿**。
**收工时（含 §9 的 F-37 之后）**：`bun run test` → **21 文件 / 208 用例全绿**；`bun run build` 通过；
`src/**` 全量 md5（`find src -type f | sort | xargs md5sum | md5sum`）= `48ba9aa5e955e637196b505ba984a2d4`
（F-37 之前是 `2207ddb62e0e3bb9c45d569c1198ae24`）。

**测试增删**：既有 190 条逐条保留，新增 18 条（含 §9 移交来的 2 条）。唯一一处**断言反向**的改动是
`App.localization-merge.test.tsx` 里第三轮那条「底稿读取失败不阻断写回」——
本轮按 lead 裁定把它改成「必须中止写回」（场景与 mock 完全保留，只换断言方向，
见 §3.1 与 §7.1）。除它以外没有任何用例被删除、跳过或放宽。

---

## §1 结论摘要

| 编号 | 级别 | 状态 | 一句话 |
|---|---|---|---|
| **F-30** | 中 | 已修 | 写回底稿读取失败时只 `console.warn` 就继续写：payload 退化成英文原文，**在大小写不敏感的文件系统上把 MOD 自带的中文整体覆盖**（R-05 的兜底路径把数据损失放了回来），且用户看不到任何反馈 |
| **F-31** | 中 | 已修 | 重试开始时后端重发的 `progress(translating)` 会清空用户**已保存**的人工译文（F-25「人工成果优先」规则在 `progress` 重试分支漏了一条），清空后状态回到 `translating`，后续 delta/done 都能再覆盖它 |
| **F-32** | 中 | 已修 | 取消勾选一个仍在读取的文件后 `loading` 永不复位：表格永久显示「加载条目…」、不渲染任何条目、**翻译按钮永久禁用**（连清空勾选再重新勾选也救不回来） |
| **F-33**（= verifier F-A3） | 中 | 已修 | 判定「目标文件是不是 MOD 自带的」用的是逐字节比较 `shippedFiles.has(plan.fileName)`：MOD 里写 `Localization/CHINESE/x.xml` 时判定落空 → 不读底稿 → payload 退化成英文原文 → Windows/macOS 上覆盖自带中文 |
| **F-34** | 低 | 已修 | 拖放路径不看 busy：解包途中再拖一个文件进来会**并发两次 `open_mod`**，后端开新 MOD 时会删掉上一个工作目录 → store 的 workDir 与后端记录不一致，之后所有 `read_file_entries` 都被拒绝 |
| **F-35** | 低 | 已修 | `GlossaryPanel.onImport` 里 `await openDialog(...)` 裸露：对话框 reject 变成未捕获 Promise rejection + 用户零反馈（第三轮 F-29 记了 `refresh/onImport/onReset` 三个，实际裸露的只有这一个） |
| **F-36** | 低 | 已修 | `AppTopBar` 五处 `void win.minimize()/toggleMaximize()/close()/startDragging()`：`void` 只丢弃返回值、**不处理 rejection**，窗口控制失败时同样是无反馈 + unhandled rejection（第三轮只记了 GlossaryPanel 同型） |
| **F-37** | 低 | 已修 | 术语表行用 `key={t.source}`、忙碌态用 `busySource === t.source` 定位：真实术语表清洗后有 6 组重复 source → React 重复 key 警告 + 同 source 的两行**一起转圈**（详见 §9） |

统计：**中危 4 条、低危 4 条**，全部「修复前红 → 修复后绿」，并逐条做了变异实验（§3 / §9 每条末尾的 M 编号）。

---

## §2 逐条缺陷（编号 / 级别 / 现象 / 后果 / 证据 / 修法 / 回归测试名 / 变异实验 / 残留风险）

| 编号 | 级别 | 现象 | 后果 | 证据（命令+输出） | 修法 | 回归测试名 | 变异实验 | 残留风险 |
|---|---|---|---|---|---|---|---|---|
| F-30 | 中 | `App.tsx` 的 `mergeWithShippedTarget` 读底稿失败只 `console.warn` 后 `return plan.entries` | 未翻译条目退回英文原文、底稿独有 contentuid 丢失；大小写不敏感 FS 上就是**覆盖自带中文**；用户界面无任何提示 | `bunx vitest run src/App.localization-merge.test.tsx` → 见 §3.1（真实 payload = `Fireball`/`Ice`） | 读失败改为 `throw`，由 `onGoPack` 的 catch 弹错误横幅并中止写回（保留已写回的 plan 不回滚）；顺手把 `String(e)` 换成 Error 的 `message`，避免「Error: 读取…失败：Error: …」双重前缀 | `底稿读取失败必须中止写回（否则已有的中文会被英文原文整体覆盖）`（原 R-05 兜底用例断言反向） | M1：改回 `console.warn + return plan.entries` → 该用例红 | 打包被中止，用户需重试；错误横幅已写明「可取消勾选映射到它的文件后继续」 |
| F-31 | 中 | `useTranslationRun` 的 `progress` 重试分支 `updateEntry(target:"", status:"translating")` 没有 `edited` 判定 | 用户流式中保存的译文被清空，几秒后被模型文本覆盖；若此刻打包，`translating` + 非空半截文本是会被写进 PAK 的 | `bunx vitest run …/TranslationTable.edit-race.test.tsx` → 见 §3.2（`- "target": "用户手工译文" + "target": ""`） | 重试分支加 `if (getEntryById(id)?.status !== "edited")` 才清空（与 `delta`/`done`/`error` 三个分支同一规则）；记账（discard/settled/inFlight）照旧 | `重试开始（progress 重发）不得清掉用户已经保存的译文` | M2：删掉该判定 → 该用例红 | 该条目在这一轮里不再接收模型文本（人工成果优先，符合 F-25 既定语义） |
| F-32 | 中 | `useEntryLoading` 的 effect cleanup 取消请求后不复位 `loading`，而取消勾选后新 effect 因 `toLoad.length === 0` 直接 return | 表格永久「加载条目…」、条目行不渲染、翻译按钮永久禁用；清空勾选再重选也恢复不了（只有再勾一个未加载文件或重开 MOD 才行） | `bunx vitest run …/hooks/useEntryLoading.test.tsx` → 见 §3.3（`expected 'loading' to be 'idle'`） | cleanup 里补 `setLoading(false)`（卸载时是空操作） | `取消勾选正在读取的文件后 loading 必须收掉（否则界面永久卡在「加载中」）`；`取消勾选后表格恢复可用：不再卡在「加载条目…」，翻译按钮可点` | M3：删掉 cleanup 里的 `setLoading(false)` → 两条用例红 | cleanup 在每次依赖变化时都会复位一次 loading（同一提交内紧接着 `setLoading(true)`，不产生可见闪烁） |
| F-33 | 中 | `files.some(f => f.name === plan.fileName)` 逐字节比较 | MOD 自带 `Localization/CHINESE/`、`Chinese/`（大小写与改写结果不同）时判定落空 → 不读底稿 → payload 退化成英文；Windows/macOS 上写回路径与自带文件是同一个文件 → 覆盖自带中文 | `bunx vitest run src/App.localization-merge.test.tsx` → 见 §3.4（真实 payload = `Fireball`/`Ice`） | 新增纯函数 `normalizePakPath`（反斜杠归一 / 折叠分隔符 / 去 `.` 与空段 / **转小写**）+ `findShippedFile`（**只在 `files` 里找**、精确名优先）；App 用 `findShippedFile` 判定，读用**实际存在的那个名字**、写仍用改写后的目标名 | `MOD 自带的目录大小写不同（Localization/CHINESE）时同样走合并`；`同名文件以两种大小写同时存在时，底稿取精确匹配的那个`；`MOD 没列出目标文件时不去读它（不把用户新建/上次写回的文件当成自带底稿）`；纯函数 5 例见 `localization.test.ts` | M7a：退回逐字节比较 → 大小写用例红；M7b：去掉 `toLowerCase()` → 3 条红 | 误判（多合并一个同名不同大小写的文件）只影响 contentuid 取值，产物仍合法；反向风险由「只遍历 `files`」结构化排除（§7.2） |
| F-34 | 低 | `openDroppedPath` / `onDrop` 不检查 busy（点击路径被 `onClick={busy ? undefined : …}` 挡住，拖放路径没有） | 两次 `open_mod` 并发 → 后端删掉上一个工作目录 → store workDir 与后端记录不一致 → 所有读取被拒，用户必须重开 MOD | `bunx vitest run src/components/FileDropZone.test.tsx` → 见 §3.5（`expected "vi.fn()" to be called 1 times, but got 2 times`） | `handleOpen` 加 `openingRef` 守卫（覆盖点击 / Tauri 拖放 / HTML5 drop / input 四条入口），被忽略的那次给错误提示 | `打开还在进行中时再拖入文件不会并发发起第二次 openMod` | M5：删掉守卫 → 该用例红 | 「仅解压」进行中拖入文件仍会 openMod（`extracting` 不在守卫内），未复现，见 §5 |
| F-35 | 低 | `onImport` 的 `await openDialog(...)` 在 try 之外 | 对话框 reject → unhandled rejection，用户看不到任何反馈；测试运行器报错误 | `bunx vitest run src/components/GlossaryPanel.test.tsx` → 见 §3.6（`Unhandled Rejection`） | 对话框调用单独 try/catch → `setError`，其余流程不变 | `导入对话框 reject 时给出错误提示，而不是未捕获的 Promise rejection` | M4：去掉 try/catch → 该用例红 + `Errors 1 error` | 无（对话框失败只影响本次导入） |
| F-36 | 低 | `AppTopBar` 五处 `void win.X()`（最小化 / 最大化 ×2 / 关闭 / 拖动） | 窗口 API reject → unhandled rejection；用户点了没反应也没有任何提示 | `bunx vitest run src/components/AppTopBar.test.tsx` → 见 §3.7（3 条全红，error 为 null） | 新增 `runWindowAction(setError, label, action)`：`.catch` + 同步 try/catch 兜底，失败进顶部错误横幅 | `最小化失败时给出错误提示，而不是未捕获的 Promise rejection`、`关闭窗口失败时同样有错误提示`、`双击标题栏最大化失败时同样有错误提示` | M6：把 helper 改回 `void action()` → 3 条全红 | 真实 Tauri 运行时是否会真的 reject 本机无法验证（§6） |

---

## §3 逐条证据（命令与真实输出）

### 3.1 F-30 底稿读取失败继续写回（修复前红 → 修复后绿）

修复前（`bunx vitest run src/App.localization-merge.test.tsx`，尾部真实输出）：

```console
 FAIL  src/App.localization-merge.test.tsx > R-05 写回已存在的中文文件 > 底稿读取失败必须中止写回（否则已有的中文会被英文原文整体覆盖）
AssertionError: expected "vi.fn()" to not be called at all, but actually been called 1 times
    266|     expect(tauri.writeFileEntries).not.toHaveBeenCalled();
  1st vi.fn() call:
    Array [
      "/tmp/work",
      "Localization/Chinese/x.xml",
      Array [
        Object { "contentuid": "uid-1", "source": "Fireball", "status": "pending", "target": "" },
        Object { "contentuid": "uid-2", "source": "Ice",     "status": "pending", "target": "" },
      ],
    ]
 Test Files  1 failed (1)
      Tests  1 failed | 6 passed (7)
```

注意这不是「理论上危险」：`write_file_entries` 收到的就是 **`Localization/Chinese/x.xml` + 英文原文**，
而 core 侧在「文件存在但读不出来」时会按新文件整体重建（verifier F-A1）。

修复后：

```console
$ bunx vitest run src/App.localization-merge.test.tsx
 ✓ src/App.localization-merge.test.tsx (10 tests)
```

### 3.2 F-31 重试开始清空人工译文

修复前（`bunx vitest run src/components/translation-table/TranslationTable.edit-race.test.tsx`）：

```console
 FAIL  … > 手工编辑与流式翻译并发 > 重试开始（progress 重发）不得清掉用户已经保存的译文
AssertionError: expected { id: 'e-0', …(7) } to match object { target: '用户手工译文', status: 'edited' }
-   "status": "edited",
-   "target": "用户手工译文",
+   "status": "translating",
+   "target": "",
 Test Files  1 failed | 3 passed (4)
```

修复后：`✓ src/components/translation-table/TranslationTable.edit-race.test.tsx (4 tests)`。

这是「后端契约」内的正常事件序列：`retry.rs:239` 明确写着**每一次新的尝试开始时都会给流式条目重发一次
`Progress(translating)`**（F-09 就是靠它区分尝试边界），所以这条路径不是构造出来的边角。

### 3.3 F-32 取消勾选后 loading 卡死

修复前（`bunx vitest run src/components/translation-table/hooks/useEntryLoading.test.tsx`）：

```console
 FAIL  … > 取消勾选正在读取的文件后 loading 必须收掉（否则界面永久卡在「加载中」）
AssertionError: expected 'loading' to be 'idle' // Object.is equality
Expected: "idle"
Received: "loading"

 FAIL  … > 取消勾选后表格恢复可用：不再卡在「加载条目…」，翻译按钮可点
AssertionError: expected '翻译工作区1 个文件，1 条加载中…翻译 1 条已翻译 0/1（0%）待翻…' not to contain '加载条目…'
Received: "翻译工作区1 个文件，1 条加载中…翻译 1 条已翻译 0/1（0%）待翻译 1翻译中 0失败 0全部1待翻译1翻译中0已翻译0出错0加载条目…显示 1/1 条虚拟滚动渲染 1 行"
 Test Files  1 failed (1)
      Tests  2 failed | 3 passed (5)
```

修复后：`✓ …useEntryLoading.test.tsx (5 tests)`（含修复后重新渲染出 `Source a-1` 行、按钮 `disabled === false`）。

### 3.4 F-33 / F-A3 大小写不同的自带文件（修复前红 → 修复后绿）

修复前（`bunx vitest run src/App.localization-merge.test.tsx`）：

```console
 FAIL  … > MOD 自带的目录大小写不同（Localization/CHINESE）时同样走合并
AssertionError: expected [ …(2) ] to deeply equal [ [ 'uid-1', '火球', 'pending' ], …(1) ]
  [
    [ "uid-1",
-     "火球",
+     "Fireball",
      "pending",
    ],
    [ "uid-2",
-     "寒冰",
+     "Ice",
      "pending",
    ],
  ]
 Test Files  1 failed (1)
      Tests  1 failed | 9 passed (10)
```

修复后：

```console
$ bunx vitest run src/lib/localization.test.ts src/App.localization-merge.test.tsx
 ✓ src/lib/localization.test.ts (33 tests)
 ✓ src/App.localization-merge.test.tsx (10 tests)
      Tests  43 passed (43)
```

如实说明：本机是 Linux，**复现到的是「写回 payload 本身已经错了」**（英文原文），
不是「物理覆盖」。物理覆盖只在大小写不敏感的文件系统上发生，列入 §6 无法验证。

### 3.5 F-34 拖放并发 open_mod

修复前（`bunx vitest run src/components/FileDropZone.test.tsx`）：

```console
 FAIL  … > 打开还在进行中时再拖入文件不会并发发起第二次 openMod
AssertionError: expected "vi.fn()" to be called 1 times, but got 2 times
 Test Files  1 failed (1)
      Tests  1 failed | 9 passed (10)
```

修复后：`✓ src/components/FileDropZone.test.tsx (10 tests)`。

### 3.6 F-35 导入对话框 reject

修复前（`bunx vitest run src/components/GlossaryPanel.test.tsx`）：

```console
 FAIL  … > 导入对话框 reject 时给出错误提示，而不是未捕获的 Promise rejection
AssertionError: expected '2 条术语导入重置新增英文 (source)中文 (target)操作Sh…' to contain '对话框不可用'
⎯⎯⎯ Unhandled Errors ⎯⎯⎯
⎯⎯⎯ Unhandled Rejection ⎯⎯⎯
Error: 对话框不可用
 Test Files  1 failed (1)
      Tests  1 failed | 11 passed (12)
     Errors  1 error
```

修复后：`✓ src/components/GlossaryPanel.test.tsx (12 tests)`（无 unhandled error）。

### 3.7 F-36 窗口控制

修复前（`bunx vitest run src/components/AppTopBar.test.tsx`）：

```console
 FAIL  … > 最小化失败时给出错误提示，而不是未捕获的 Promise rejection
 FAIL  … > 关闭窗口失败时同样有错误提示
 FAIL  … > 双击标题栏最大化失败时同样有错误提示
AssertionError: the given combination of arguments (null and string) is invalid for this assertion.
 Test Files  1 failed (1)
      Tests  3 failed (3)
```

修复后：`✓ src/components/AppTopBar.test.tsx (3 tests)`。

### 3.8 变异实验汇总（把修复改回原样，测试是否变红）

方法：`cp` 备份 → python 就地把修复改回原样 → 跑指定用例 → `cp` 还原（**未使用任何 git 写操作**），
每次都核对还原后的 md5。

| # | 变异 | 结果 |
|---|---|---|
| M1 | F-30：`throw` 改回 `console.warn + return plan.entries` | `App.localization-merge` 1 例红 ✅ |
| M2 | F-31：删掉 progress 重试分支的 `edited` 判定 | `edit-race` 1 例红（`target: ""`）✅ |
| M3 | F-32：删掉 cleanup 里的 `setLoading(false)` | `useEntryLoading` 2 例红 ✅ |
| M4 | F-35：删掉对话框的 try/catch | `GlossaryPanel` 1 例红 + `Errors 1 error` ✅ |
| M5 | F-34：删掉 `openingRef` 守卫 | `FileDropZone` 1 例红（`got 2 times`）✅ |
| M6 | F-36：`action().catch(...)` 改回 `void action()` | `AppTopBar` 3 例红 ✅ |
| M7a | F-33：`findShippedFile` 退回 `f.name === plan.fileName` | `App.localization-merge` 1 例红（`Fireball`）✅ |
| M7b | F-33：`normalizePakPath` 去掉 `toLowerCase()` | `localization.test` 2 例 + `App.localization-merge` 1 例红 ✅ |

---

## §4 被证伪的怀疑点（都跑过命令）

| 怀疑点 | 结论 | 命令与证据 |
|---|---|---|
| FileTree 点文件行的 checkbox 会因冒泡被切换两次（净效果为零） | **证伪** | `src/components/ui/checkbox.tsx:22` 的 `onClick` 第一行是 `e.stopPropagation()`；`bunx vitest run src/components/FileTree.test.tsx` → `✓ 7 tests` |
| `useEntryLoading` 的 `loading` 是 store 全局标志，打包后回首页会把首页 `FileDropZone` 钉在「正在解包…」 | **证伪（我自己的假设被实验推翻）** | 临时探针显示 hook 用的是**局部** `useState`（`useEntryLoading.ts:17`），store 的 `loading` 只由 `FileDropZone.handleOpen` 置位：探针输出 `after render: loading= false text= loading`。我据此删掉了那条错误假设的用例，并据此重写 F-32 的后果描述 |
| 工具栏「翻译」按钮在翻译中仍可点，双击会走 `retranslateAll` 清空已完成译文 | **证伪** | `button.tsx:50` `disabled={disabled \|\| loading}`，`TranslationToolbar.tsx:75-76` 传 `loading={translating && !cancelling}` 且 `disabled` 含 `translating` → 翻译中恒禁用；`重试失败` 显式 `disabled={translating \|\| loading}`，行内重试 `disabled={busy}` |
| 注入面：`dangerouslySetInnerHTML` / `eval` / 把后端错误文本当 HTML | **证伪（零命中）** | `grep -rn "dangerouslySetInnerHTML\|innerHTML\|insertAdjacentHTML\|document.write\|eval(\|new Function\|srcdoc" src/ --include=*.ts --include=*.tsx` → 无输出；错误文本全部走文本节点（`error-banner.tsx`、`EntryRow.tsx`） |
| 本轮引入了 `any` / `@ts-ignore` / `eslint-disable` | **证伪（零命中）** | `git diff -U0 -- src/ \| grep -E "^\+" \| grep -E ": any\b\|as any\|@ts-ignore\|@ts-expect-error\|eslint-disable\|dangerouslySetInnerHTML"` → 无输出（既有的两处 `eslint-disable-next-line react-hooks/exhaustive-deps` 未动） |
| 改了依赖 / 门禁配置 / 版本号 | **证伪** | `git diff --stat -- package.json bun.lock vite.config.ts tsconfig*.json index.html` → 无输出；`git diff -- package.json \| wc -l` → `0` |
| 第三轮 F-29 记的 `GlossaryPanel.refresh` / `onReset` 也裸露 | **部分证伪** | `refresh` 自带 try/catch（`GlossaryPanel.tsx:71-78`），`onReset` 的 `resetGlossary` 在 try 内（`:135`）；真正裸露的只有 `onImport` 的 `openDialog` → F-35 |
| `translateAll/retryFailed/retryOne` 在 `runningRef` 守卫**之前**就改了 store（顺序缺陷） | **未复现（UI 三道禁用挡住，见 §5.1）** | 代码阅读结论，未构造出可达路径，因此不列为缺陷、只列 §5 |

---

## §5 已确认未修（含理由）

### 5.1 `translateAll` / `retryFailed` / `retryOne` 在守卫之前改 store（潜在顺序缺陷，低）

`useTranslationRun.ts:293-325`：三个入口都是「先 `updateEntry(...)` 把条目清成 `pending` + 空 target，
再 `await runTranslation(...)`」，而 `runningRef.current` 的守卫在 `runTranslation` 内部。
若能在翻译进行中触发，`retranslateAll` 分支会把**已经翻好的条目清空**且这一轮不会再补回来。
**未修理由**：当前 UI 的三道禁用（翻译按钮 / 重试失败 / 行内重试）把它挡在外面，我构造不出可达路径；
把「重置」挪进 `runTranslation` 会改变既有语义（`TranslationTable.streaming.test.tsx` 的
「重试失败条目只清空被重试的条目」依赖调用时机），属于应当单独评估的改动。**建议下一轮按
「入口只在 runningRef 内做状态修改」重构，并补一条直接调用 hook 的用例。**

### 5.2 第二次打包时底稿是「上一次写回的结果」（低；已复现，未修）

复现（临时探针，两次打包的真实输出）：

```console
第一次写回: [ [ 'uid-1', '火球术', 'translated' ] ]
第二次写回: [ [ 'uid-1', '火球术', 'pending' ] ]
```

用户流程：打包 → 「返回继续编辑」→ 把某条「还原」（清空译文）→ 再打包。
第二次的底稿是从**工作目录**里读的，而那里已经是上一次写回的结果，于是「还原」被上一次的译文
还原了回来。**未修理由**：要真正支持「还原成 MOD 原始文本」，必须在首次写回前保存原始字节
（新状态 + 新语义，会动 `read_file_entries` 与写回链路），属于设计决策；而且当前的合并方向是
「宁可保留底稿也不写英文」，比反过来安全。**建议 lead 决定是否列为下一轮议题。**

### 5.3 其余

| 项 | 理由 |
|---|---|
| 「仅解压」进行中拖入文件仍会 `openMod`（`extracting` 不在 `openingRef` 内） | 需要把 busy 的两个来源统一进 ref；未构造出真实可达路径（同一只手很难在解压期间再拖一次），未改 |
| `splitErrorLines("   ")` 返回 `[""]`，`ErrorBanner` 渲染一个空行 | 第三轮已记录；构造不出全空白的真实错误（所有 `setError` 调用都带前缀/上下文），且既有 `error-banner.test.ts` 固化了该行为 |
| 文件夹三态的 `aria-checked` 报 `false` 而不是 `"mixed"`（`checkbox.tsx:19`） | 无障碍语义可以更准，但 `FileTree.test.tsx:117` 明确断言 indeterminate 时是 `"false"`；改它要动既有断言，收益低 → 记入 backlog |
| 条目列表没有 table/list 语义（纯 div + 虚拟滚动） | 重做结构与视觉，属于 backlog，不属于「最小化修复」 |
| `planLocalizationWrites` 对每个 plan 做一次 O(N) `map(toWritableEntry)` | 第三轮已记录（打包是一次性动作）；本轮未动热路径 |

---

## §6 无法验证（如实列出）

| 项 | 原因 |
|---|---|
| 大小写不敏感文件系统上的**物理覆盖**（F-33/F-A3 的最终后果） | 本机是 Linux（ext4 大小写敏感），只能证明「写回 payload 已经退化成英文原文」；物理覆盖需要在 Windows/macOS 上验证，与 verifier 的结论一致 |
| 真实 Tauri 运行时里 `win.minimize()` / `openDialog()` 是否真的 reject（F-35/F-36 的实际发生概率） | 本机不跑 Tauri runtime，只能用「注入 reject 的 mock」复现缺陷与验证修复；概率无法量化 |
| 真实 IPC 的投递顺序（`all_done` 与命令 Promise 的先后） | 沿用第三轮：若乱序只会少一行 `run_summary`，不会数据损坏 |
| 真实 LLM API 的端到端行为 | 没有 key；所有网络行为用受控 mock |
| 真机 BG3 能否加载 repack 产物 | 无法运行游戏 |
| 20k 条下的墙钟性能 | 本轮有多位审计者并行跑 cargo，`[perf]` 数字波动大（`batched store_ms` 1.7~2.3ms）；只保结构性断言（一帧一次通知、文本零丢失）。我的改动都不在热路径（progress 重试分支多一次 O(1) `getEntryById`；写回/加载路径不在流式帧内） |

---

## §7 lead 交办的两条裁定

### 7.1 底稿读取失败的语义（verifier F-A1 的前端一侧）

**裁定：读不到底稿必须中止这次写回，并且要把原因展示给用户（不能降级继续、不能只 warn）。**

理由（按重要性排序）：

1. **合并的前提就是拿到这个文件里已有的内容。** 读不到还照常写回，就是把「保护自带中文」的
   那次合并整个跳过的同时**继续写这个文件**；在 core 侧「存在但读不出来 = 当新文件整体重建」
   （F-A1）的语义下，产出的 payload 就是英文原文 + 只剩本次提交的 contentuid。
2. **代价不对称。** 中止 = 用户重试一次（错误横幅里已写明补救动作：重试，或取消勾选映射到它的
   文件再打包）；继续 = 工作目录里已有的中文被不可逆地覆盖（要恢复只能重新解包整个 MOD，
   本轮会话的译文全丢）。
3. **第三轮「不阻断写回」的初衷是善意的，但它防的是「把可恢复的读取失败变成打包失败」，
   防不住「把可恢复的读取失败变成不可逆的覆盖」。** 本轮按后者取舍。
4. **后端即使改成「非 NotFound 的 IO 错误直接报错」（F-A1 主修），前端这条仍然要动**：
   前端是唯一知道「这次合并没能完成」的一层；让后端来兜底意味着用户看到的是后端错误
   （或者在后端只做 `exists()` 判断时根本不报错），而前端本来就能给出更清楚的「读取哪个文件、
   为什么中止、下一步做什么」。两层都做 = 纵深防御，互不替代。
5. 顺带修掉的**静默吞错**：原实现只有 `console.warn`，用户界面零反馈（task-3 清单第 3 条）。

### 7.2 自带目标文件的判定规则（verifier F-A3）

**归一化规则（`normalizePakPath`）**：反斜杠 → `/`；折叠重复分隔符；去掉空段与 `.` 段；
去掉首尾分隔符；**转小写**。判定入口 `findShippedFile(files, targetPath)`：先找**精确同名**
（两种大小写同时存在时读用户实际选中的那一个），再退化为归一化相等；找不到返回 `null`。

**大小写不敏感是全平台统一，不做平台分支。** 理由：

1. 同一个 MOD 在三个平台上必须给出**同一个判定结果**。按平台分支会让「是否合并自带中文」
   随平台变化，而差异恰好落在「有没有不可逆地覆盖自带中文」这件事上。
2. 第三轮 §3.5 的教训就是「把平台假设写进逻辑」：`trim_start_matches('/')` 在 Windows 上是
   空操作。这里如果按平台分支，Linux CI 永远测不到 Windows 走的那条路。
3. 误判的代价不对称：漏判（该合并没合并）会丢自带中文；误判（多读了一个同名不同大小写的
   文件当底稿）最多是多合并几个 contentuid 条目，产物依旧是合法、可加载的本地化文件。
   在两种错误里选可逆的那个。

**如何避免「把用户新建的同名文件误判成自带文件」这类反向风险**：
`findShippedFile` **只遍历 `files`**（= `open_mod` 返回的文件列表），绝不探测磁盘上是否存在该路径。
上一次写回在工作目录里造出来的 `Localization/Chinese/x.xml` 不在 `files` 里，因此无论大小写
如何都不可能被当成底稿。这条有专门的回归用例：
`MOD 没列出目标文件时不去读它（不把用户新建/上次写回的文件当成自带底稿）`（断言 `readFileEntries`
从未以该路径被调用，且 payload 仍是「首次生成」的英文兜底）。

**跨范围观察（给 lead / auditor-formats）**：`detect_language_from_path` 对 `Localization/Chinese/`
返回 `None` 会让 `localizationWritePriority` 把它当「其它语言」（优先级 1 而不是 2）。
本轮的前端修复**不依赖** `language` 字段（合并判定只看路径），所以不受影响；但如果后端要接受
`Chinese` 目录名，建议两侧的别名表（前端 `CHINESE_LANGUAGE_ALIASES = ["Chinese","ChineseSimplified"]`）
对齐一次。

---

## §8 task-3 必查清单 → 结论对照

| 清单项 | 结论 |
|---|---|
| 1. 竞态：切 MOD / 迟到 delta/done/error / 双击开始翻译 / 开始翻译与写回并发 | 迟到事件与切 MOD 由第三轮的 runId + alive + token + edited 四道闸门覆盖（本轮复核未发现新缺口）；**双击/并发入口**新增一条 F-34；「翻译与打包并发」闸门复核通过（工具按钮禁用 + `runToken`） |
| 2. 写回闸门（translating / error / 空译文 / 用户清空） | 新增 F-30（底稿读失败）与 F-33（大小写判定）两条；其余路径复核通过（`toWritableEntry` + `hasWritableTarget` 语义与后端一致） |
| 3. 未捕获 Promise rejection / 静默吞错 | F-30（静默吞错）、F-35、F-36 三条；全仓 async 处理器逐个过了一遍（§4 表） |
| 4. 虚拟滚动 + 过滤 + 编辑索引映射 | 既有用例（过滤后逐行核对、`data-index` 与条目一一对应）复核通过；F-32 的「加载态卡死」会让整个列表不可用，属于该链路 |
| 5. store 状态机 / delta 最后一帧 / 内存 / 派生重算 | `discardAll`（收尾）、`flush`（all_done）、`dispose`（卸载）三条落地路径复核通过；`applyDeltas` 仍是「一帧一次 set」；本轮未发现泄漏（`loadedRef`/索引随 MOD 切换重建） |
| 6. `lib/entries.ts` / `lib/localization.ts` / `lib/tauri.ts` | `localization.ts` 新增两个纯函数 + §5.2 的已知限制；`entries.ts` 与 `tauri.ts` 复核未发现新缺陷（Channel 每次翻译新建、旧回调由 runId 丢弃） |
| 7. a11y / 可用性 | 进度条 `role="progressbar"` + `aria-value*` 已在（`progress.tsx:16-19`）；键盘操作与 focus 复核通过（FileTree 行、ErrorBanner Esc、侧边栏 dialog）；仍存 §5.3 的三态 `aria-checked` |
| 8. 注入面 | 零命中（§4 表），后端错误文本全部以文本节点渲染 |

---

## §9 跨范围移交：重复 source 的行定位（F-37，低）

> auditor-shell 侦察到、按写范围移交给前端的一条小修。**IPC 契约未动、后端未动。**

### 9.1 既定事实（我自己核过，不引用二手结论）

用 python 复刻 `clean_entry` 的清洗规则（`trim` + `strip_full_wrapping_quotes`）后统计
`samples/bg3-official-glossary.json`：

```console
清洗后条数: 20253 | 重复组数: 6
  "Danthelon's Dancing Axe" [('丹瑟隆的飞斧', 'official'), ('丹瑟隆的飞斧', 'official')]
  "Fraygo's Flophouse"      [('弗雷戈招待所', 'official'), ('弗雷戈招待所', 'official')]
  'Freedom'                 [('自由', 'official'), ('自由', 'official')]
  'Jaheira'                 [('贾希拉', 'official'), ('贾希拉', 'official')]
  "Sharess' Caress"         [('夏芮丝的爱抚', 'official'), ('夏芮丝的爱抚', 'official')]
  'Sword Coast Couriers'    [('剑湾快递', 'official'), ('剑湾快递', 'official')]
```

即：**重复来自清洗**（`'Jaheira'` 与 `"Jaheira"` 这类带引号变体去掉引号后同名），每组 2 条、
组内译文相同 —— 与 Rust 侧 `real_glossary_already_contains_duplicate_sources`
（`crates/bg3-translate-core/src/glossary/store.rs:436`）钉住的一致。

**但要说清一个限定**：这 12 条全部是 `source_kind: "official"`，而删除按钮只在
`t.sourceKind !== "official"` 时渲染 —— 所以**这 6 组本身不会触发「两行一起转圈」**。
重复的 `user` 条目另有两条真实路径：

1. 导入任意 JSON（`from_json` 不清重，用户可以导入带重复 source 的 JSON）；
2. **把某条术语的 source 改成另一条已有的 source** —— 后端 `update` 只替换**第一条**匹配
   （`store.rs:147-158`），于是新改的这条与剩下那条同 source，且**组内译文可能不同**。

也就是说，「组内译文相同所以后果有限」对**随仓库发货的 6 组**成立，
对用户自己造出来的重复**不成立**。

### 9.2 缺陷与修复

| 编号 | 级别 | 现象 | 后果 | 证据 | 修法 | 回归测试名 | 变异实验 | 残留风险 |
|---|---|---|---|---|---|---|---|---|
| F-37 | 低 | 行用 `key={t.source}`、删除忙碌态用 `busySource === t.source` 定位（`GlossaryPanel.tsx:298/62/329`） | ① React 重复 key：`Encountered two children with the same key, 'Gith'`，增删/过滤时可能复用错行；② 点一行删除，**同 source 的两行一起转圈** | `bunx vitest run src/components/GlossaryPanel.test.tsx` → 见下 | 新增 `buildRowKeys()`：行键 = `source#组内序号`（序号在**整个术语表**范围内算）；`busySource` → `busyRow`，`onDelete(rowKey, source)` 把「行定位」与「后端主键」分开 | `同一 source 的两行都要渲染，且不产生 React 重复 key 警告`；`点第一行的删除：只有那一行转圈，重复的另一行不受影响` | M8a：`key` 退回 `t.source` → 警告用例红；M8b：busy 退回按 source 定位 → 转圈用例红 | 后端的 `update` / `delete` 仍按 source 命中（见 9.4），不是前端能修的 |

修复前（真实输出）：

```console
$ bunx vitest run src/components/GlossaryPanel.test.tsx
 FAIL  … > 同一 source 的两行都要渲染，且不产生 React 重复 key 警告
- Expected  - 0
+ Received  + 1
+   "Encountered two children with the same key, `%s`. Keys should be unique so that components
+    maintain their identity across updates. Non-unique keys may cause children to be duplicated
+    and/or omitted — the behavior is unsupported and could change in a future version.",
 FAIL  … > 点第一行的删除：只有那一行转圈，重复的另一行不受影响
AssertionError: expected 'true' to be null
 Test Files  1 failed (1)
      Tests  2 failed | 12 passed (14)
```

修复后：`✓ src/components/GlossaryPanel.test.tsx (14 tests)`。

### 9.3 行键怎么避免与过滤 / 排序串位

`buildRowKeys` 遍历的是 **`glossary.terms`（完整列表）**，按「同一 source 的第几次出现」编号，
因此：

- 搜索 / 过滤只改变**可见子集**，不改变任何一行的序号取值 → 过滤前后同一行的键不变；
- 「加载更多」（`limit + 200`）只扩大切片范围，同样不影响序号；
- 增删术语会换掉整个 `glossary` 对象 → `useMemo` 重算索引 → 键与新列表一致
  （列表顺序来自后端，索引按对象引用查表，重复 source 也不会互相顶掉）。

### 9.4 没覆盖什么（如实列出，含我自己核实的后端语义）

1. **本轮不改「按 source 当主键」这条链路**：`GlossaryEntry` 没有 id 字段，要加就得动 IPC 契约
   （`src/lib/types.ts` ↔ Rust 结构体 ↔ `scripts/check_ipc_contract.py`），属于跨范围改动，
   **已列 backlog**；前端只能在**展示层**用行键把它们区分开。
2. **后端 `update(old_source, entry)` 只替换第一条**（`store.rs:151-157`）：重复 source 下编辑
   第二行，实际改的是第一行 —— 这个语义没变，用户仍可能「改错行」。
3. **后端 `delete(source)` 比「命中第一条」更强**（`store.rs:161-174`）：先看**第一条**是不是
   `official`（是则整体拒绝），然后 `retain(|e| e.source != source)` —— **该 source 的所有重复
   条目会被一起删掉**。我的修复只让**按钮的忙碌态**不串行；「点一次删掉整组」是后端既有语义，
   未改。随仓库发货的那 6 组是 official（删除按钮根本不渲染），用户自造的重复则会一次删整组。
4. 删除 / 编辑按钮的 `aria-label` 仍含 source（重复时读屏文案相同）—— 未改，
   因为既有测试与用户习惯都依赖这个文案；记入 a11y backlog。

---

## §10 门禁真实输出与改动清单

### 10.1 收工门禁

```console
$ cd /home/jason/bg3-translate && bun run test
 ✓ src/components/GlossaryPanel.test.tsx (14 tests) 369ms
 ✓ src/components/AppTopBar.test.tsx (3 tests) 55ms
 …
 ✓ src/store/streaming.perf.test.ts (1 test) 3778ms
 Test Files  21 passed (21)
      Tests  208 passed (208)
   Duration  5.09s

$ bun run build
$ tsc -b && vite build
✓ 2007 modules transformed.
dist/index.html                   0.40 kB │ gzip:   0.29 kB
dist/assets/index-B9vOQbzi.css   44.39 kB │ gzip:   8.68 kB
dist/assets/index-C1OLynJC.js   467.64 kB │ gzip: 144.89 kB
✓ built in 274ms
```

（`206` 是 F-37 之前那次全量的数字；加上 §9 的 2 条用例后为 `208`。
两次的 `web-test` / `web-build` 均全绿。）

`bash scripts/verify.sh --web-only`（我的 scope，最终状态下重跑）：

```console
── [3/3] 前端类型检查 + 构建（tsc -b && vite build）
   $ bun run build
$ tsc -b && vite build
✓ 2007 modules transformed.
dist/index.html                   0.40 kB │ gzip:   0.29 kB
dist/assets/index-DZ4Lh-nh.css   44.21 kB │ gzip:   8.67 kB
dist/assets/index-C0lDwk_Q.js   467.43 kB │ gzip: 144.83 kB
✓ built in 651ms
   ✓ 通过（1.8s）
✓ 全部 3 道门禁通过（总耗时 14.4s）
```

`bash scripts/verify.sh`（全量，最终状态下重跑）：**在 gate 2/6 停在 `core-fmt`** ——

```console
Diff in /home/jason/bg3-translate/src-tauri/src/commands/terminology.rs:176:
Diff in /home/jason/bg3-translate/src-tauri/src/commands/translate.rs:153:
   ✗ 失败：core-fmt 退出码 1（330ms）
✗ 门禁未通过：Rust 代码格式（cargo fmt --check）（core-fmt）
  命令：cargo fmt --all --check
  已跑 2/6 道，总耗时 484ms，退出码 1
```

失败**不在我的写范围**：`cargo fmt --all --check` 报的 8 个文件全部是 `crates/**` 与
`src-tauri/**`（其他审计者收工前尚未 rustfmt 干净）：

```console
$ cargo fmt --all --check 2>&1 | grep '^Diff in' | sed 's/:[0-9]*:$//' | sort -u
crates/bg3-translate-core/src/config.rs
crates/bg3-translate-core/src/formats/content_list.rs
crates/bg3-translate-core/src/formats/loca.rs
crates/bg3-translate-core/src/glossary/store.rs
crates/bg3-translate-core/src/pak.rs
src-tauri/src/commands/entries.rs
src-tauri/src/commands/terminology.rs
src-tauri/src/commands/translate.rs
$ cargo fmt --all --check 2>&1 | grep -c '^Diff in .*/src/'   # 我的范围
0
```

因此 `core-test` / `web-test` / `web-build` 三道在全量脚本里**没有被跑到**；其中与我相关的两道
（`web-test` / `web-build`）已由 `--web-only` 单独跑通并记录在上。按 lead 的冻结流程，最终权威
结果以 lead 在冻结 revision 上重跑的 `verify.sh` 为准。

> 并发编辑说明（lead 广播第 4 条）：verifier 期间看到 `App.localization-merge.test.tsx` 一度红、
> 随后转绿，是我按「先写红测试 → 再改生产代码」推进时的正常过渡；收工状态下它是**绿**的
> （`✓ src/App.localization-merge.test.tsx (10 tests)`）。我的变异实验全部是「仓库内原地改 +
> md5 核对还原」，没有把 /tmp 副本的源码与仓库混跑。

### 10.2 改动清单（全部在 `src/**` + 本报告）

生产代码（7 个文件）：

- `src/App.tsx`：底稿读取失败中止写回 + 读用自带文件的真实路径 + 错误文案去 `Error:` 前缀
- `src/lib/localization.ts`：新增 `normalizePakPath` / `findShippedFile`
- `src/components/translation-table/hooks/useTranslationRun.ts`：progress 重试分支尊重 `edited`
- `src/components/translation-table/hooks/useEntryLoading.ts`：cleanup 复位 `loading`
- `src/components/FileDropZone.tsx`：`openingRef` 并发守卫
- `src/components/GlossaryPanel.tsx`：导入对话框 try/catch（F-35）+ `buildRowKeys` 行键 / `busyRow` 行定位（F-37，§9）
- `src/components/AppTopBar.tsx`：`runWindowAction` 统一处理窗口 API reject

测试（8 个文件，新增 18 条用例；`src/components/AppTopBar.test.tsx` 为新文件）：

- 新增用例：`App.localization-merge.test.tsx` ×3、`localization.test.ts` ×5、
  `TranslationTable.edit-race.test.tsx` ×1、`useEntryLoading.test.tsx` ×2、
  `GlossaryPanel.test.tsx` ×3（F-35 ×1 + F-37 ×2）、`FileDropZone.test.tsx` ×1、
  `AppTopBar.test.tsx` ×3（新文件）
- 断言反向（场景保留，见 §1）：`App.localization-merge.test.tsx` → `底稿读取失败必须中止写回（否则已有的中文会被英文原文整体覆盖）`

未改动：`src/lib/types.ts`（IPC 契约）、`package.json`、`vite.config.ts`、`tsconfig*.json`、
`index.html`、`crates/**`、`src-tauri/**`、`scripts/**`。

### 10.3 跨范围发现（写给 lead / 其他审计者）

1. **F-33/F-A3 与 `detect_language_from_path`**：见 §7.2 末尾。前端合并判定已不看 `language`，
   但如果后端要让 `Localization/Chinese/`（大写 C 以外的变体）也识别为中文，两侧别名表需对齐。
2. **F-30 与 F-A1**：前端现在会在读不到底稿时中止；后端若把「非 NotFound 的 IO 错误」也改成
   直接报错，两条防线**互不替代**（§7.1 第 4 点），建议都保留。
3. **`docs/ARCHITECTURE.md` 的「写回不变量」**建议补充第三轮已写的四条 + 本轮两条：
   「读不到自带底稿必须中止写回」「自带目标文件的判定按归一化路径（大小写不敏感）」。
4. **F-37 的后端语义**（给 auditor-shell / 下一轮）：`Glossary::delete(source)` 用
   `retain(|e| e.source != source)`，会删掉**该 source 的全部重复条目**，而 `update` 只替换第一条
   —— 两者不对称。真实那 6 组是 `official`（删除按钮不渲染），但用户导入带重复的 JSON、
   或把某条 source 改成已有 source 时会踩到。彻底解决要给 `GlossaryEntry` 加 id（动 IPC 契约），
   已列 backlog（§9.4）。
