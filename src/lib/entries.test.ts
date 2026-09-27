import { describe, expect, it } from "vitest";
import {
  buildRetryRequest,
  buildTranslationRequest,
  computeEntryStats,
  countByFilter,
  filterEntries,
  hasVisibleText,
  hasWritableTarget,
  isDoneStatus,
  isTranslatableNow,
  isTranslationWorkItem,
  matchesFilter,
  matchesQuery,
  progressPercent,
  shortSourcePath,
} from "./entries";
import type { TranslationEntry, TranslationStatus } from "./types";

function entry(
  id: string,
  patch: Partial<TranslationEntry> = {},
): TranslationEntry {
  return {
    id,
    sourceFile: "Localization/English/foo.xml",
    source: `Hello ${id}`,
    target: "",
    contentuid: `uid-${id}`,
    version: "1",
    status: "pending",
    error: null,
    ...patch,
  };
}

describe("isTranslationWorkItem / isTranslatableNow", () => {
  it("原文非空才算工作项", () => {
    expect(isTranslationWorkItem(entry("a"))).toBe(true);
    expect(isTranslationWorkItem(entry("b", { source: "   " }))).toBe(false);
    expect(isTranslationWorkItem(entry("c", { source: "" }))).toBe(false);
  });

  it("原文非空且译文为空才需要翻译", () => {
    expect(isTranslatableNow(entry("a"))).toBe(true);
    expect(isTranslatableNow(entry("b", { target: "你好" }))).toBe(false);
    expect(isTranslatableNow(entry("c", { target: "   " }))).toBe(true);
    expect(isTranslatableNow(entry("d", { source: " " }))).toBe(false);
  });

  it("translated 与 edited 都算完成", () => {
    expect(isDoneStatus("translated")).toBe(true);
    expect(isDoneStatus("edited")).toBe(true);
    expect(isDoneStatus("pending")).toBe(false);
    expect(isDoneStatus("error")).toBe(false);
    expect(isDoneStatus("translating")).toBe(false);
  });
});

describe("matchesQuery", () => {
  it("空查询命中所有条目", () => {
    expect(matchesQuery(entry("a"), "")).toBe(true);
    expect(matchesQuery(entry("a"), "   ")).toBe(true);
  });

  it("匹配原文、译文、contentuid，大小写不敏感", () => {
    const e = entry("a", { source: "Fireball", target: "火球术", contentuid: "h123" });
    expect(matchesQuery(e, "fire")).toBe(true);
    expect(matchesQuery(e, "FIREBALL")).toBe(true);
    expect(matchesQuery(e, "火球")).toBe(true);
    expect(matchesQuery(e, "H123")).toBe(true);
    expect(matchesQuery(e, "ice")).toBe(false);
  });

  it("忽略查询两端空白", () => {
    expect(matchesQuery(entry("a", { source: "Fireball" }), "  fire  ")).toBe(true);
  });
});

describe("matchesFilter", () => {
  it("all 命中所有状态", () => {
    for (const status of ["pending", "translating", "translated", "edited", "error"] as TranslationStatus[]) {
      expect(matchesFilter(entry("a", { status }), "all")).toBe(true);
    }
  });

  it("translated 维度包含 edited", () => {
    expect(matchesFilter(entry("a", { status: "translated" }), "translated")).toBe(true);
    expect(matchesFilter(entry("a", { status: "edited" }), "translated")).toBe(true);
    expect(matchesFilter(entry("a", { status: "pending" }), "translated")).toBe(false);
  });

  it("其它维度精确匹配", () => {
    expect(matchesFilter(entry("a", { status: "pending" }), "pending")).toBe(true);
    expect(matchesFilter(entry("a", { status: "translating" }), "translating")).toBe(true);
    expect(matchesFilter(entry("a", { status: "error" }), "error")).toBe(true);
    expect(matchesFilter(entry("a", { status: "translating" }), "error")).toBe(false);
  });
});

describe("filterEntries", () => {
  const list = [
    entry("1", { source: "Fireball", status: "pending" }),
    entry("2", { source: "Ice Storm", target: "冰风暴", status: "translated" }),
    entry("3", { source: "Shield", target: "护盾", status: "error" }),
    entry("4", { source: "Heal", target: "治疗", status: "edited" }),
    entry("5", { source: "Haste", status: "translating" }),
  ];

  it("无过滤条件时原样返回同一数组引用", () => {
    expect(filterEntries(list)).toBe(list);
    expect(filterEntries(list, { query: "", filter: "all" })).toBe(list);
  });

  it("按状态过滤", () => {
    expect(filterEntries(list, { filter: "pending" }).map((e) => e.id)).toEqual(["1"]);
    expect(filterEntries(list, { filter: "translating" }).map((e) => e.id)).toEqual(["5"]);
    expect(filterEntries(list, { filter: "translated" }).map((e) => e.id)).toEqual(["2", "4"]);
    expect(filterEntries(list, { filter: "error" }).map((e) => e.id)).toEqual(["3"]);
  });

  it("搜索 + 状态过滤同时生效", () => {
    expect(
      filterEntries(list, { filter: "translated", query: "冰" }).map((e) => e.id),
    ).toEqual(["2"]);
    expect(filterEntries(list, { filter: "pending", query: "冰" })).toEqual([]);
  });

  it("搜索可命中 contentuid", () => {
    expect(filterEntries(list, { query: "uid-3" }).map((e) => e.id)).toEqual(["3"]);
  });
});

describe("countByFilter", () => {
  it("translated 计数包含 edited", () => {
    const counts = countByFilter([
      entry("1", { status: "pending" }),
      entry("2", { status: "translating" }),
      entry("3", { status: "translated" }),
      entry("4", { status: "edited" }),
      entry("5", { status: "error" }),
      entry("6", { status: "error" }),
    ]);
    expect(counts).toEqual({
      all: 6,
      pending: 1,
      translating: 1,
      translated: 2,
      error: 2,
    });
    // 各维度之和 >= 总数（translated 覆盖 translated + edited）
    expect(counts.pending + counts.translating + counts.translated + counts.error).toBe(6);
  });

  it("空列表全为 0", () => {
    expect(countByFilter([])).toEqual({
      all: 0,
      pending: 0,
      translating: 0,
      translated: 0,
      error: 0,
    });
  });
});

describe("computeEntryStats", () => {
  it("统计各状态并计算 completed / percent", () => {
    const stats = computeEntryStats([
      entry("1", { status: "translated" }),
      entry("2", { status: "edited" }),
      entry("3", { status: "pending" }),
      entry("4", { status: "error" }),
    ]);
    expect(stats).toMatchObject({
      total: 4,
      pending: 1,
      translating: 0,
      translated: 1,
      edited: 1,
      error: 1,
      done: 2,
      percent: 50,
    });
  });

  it("空列表 percent 为 0", () => {
    expect(computeEntryStats([]).percent).toBe(0);
  });
});

describe("progressPercent", () => {
  it("四舍五入到整数", () => {
    expect(progressPercent(1, 3)).toBe(33);
    expect(progressPercent(2, 3)).toBe(67);
    expect(progressPercent(0, 10)).toBe(0);
    expect(progressPercent(10, 10)).toBe(100);
  });

  it("total <= 0 时返回 0", () => {
    expect(progressPercent(5, 0)).toBe(0);
    expect(progressPercent(5, -1)).toBe(0);
    expect(progressPercent(0, 0)).toBe(0);
  });

  it("越界输入被夹紧到 0–100", () => {
    expect(progressPercent(20, 10)).toBe(100);
    expect(progressPercent(-3, 10)).toBe(0);
  });
});

describe("shortSourcePath", () => {
  it("保留最后两级路径", () => {
    expect(shortSourcePath("Mods/Foo/Localization/English/x.xml")).toBe(
      "English/x.xml",
    );
    expect(shortSourcePath("Localization/English/x.xml")).toBe("English/x.xml");
  });

  it("不足两级时原样返回", () => {
    expect(shortSourcePath("x.xml")).toBe("x.xml");
    expect(shortSourcePath("English/x.xml")).toBe("English/x.xml");
  });

  it("忽略重复与首尾斜杠", () => {
    expect(shortSourcePath("/a//b/c.xml/")).toBe("b/c.xml");
  });
});

describe("buildTranslationRequest", () => {
  it("正常分支的 payload 是全部工作条目（含已有译文，供后端建一致性记忆）", () => {
    const plan = buildTranslationRequest([
      entry("1", { source: "A", target: "" }),
      entry("2", { source: "B", target: "乙", status: "translated" }),
      entry("3", { source: "", target: "" }),
      entry("4", { source: "D", target: "", status: "error" }),
      entry("5", { source: "E", target: "戊", status: "edited" }),
    ]);
    expect(plan.retranslateAll).toBe(false);
    // 原文为空的条目不是工作条目，永远不进 payload
    expect(plan.request.map((e) => e.id)).toEqual(["1", "2", "4", "5"]);
    // 已有译文的条目**原样**带上（target 不清、状态不改）：core 用整份 payload
    // 建 `planner::build_consistency_memory`（只认 has_writable_target 的条目）。
    // 只发 pending 的话这份记忆恒为空 —— 专名跨批次不一致、【本 MOD 已确定译名】
    // 段落从不出现、系列 base 复用失效。
    expect(plan.request.find((e) => e.id === "2")).toMatchObject({
      target: "乙",
      status: "translated",
    });
    expect(plan.request.find((e) => e.id === "5")).toMatchObject({
      target: "戊",
      status: "edited",
    });
    // 本轮真正会被后端翻译的只有这批（调用方的收尾回滚也必须只动它们）
    expect(plan.request.filter(isTranslatableNow).map((e) => e.id)).toEqual([
      "1",
      "4",
    ]);
  });

  it("全部已有译文时退回重新翻译全部，并清空译文", () => {
    const plan = buildTranslationRequest([
      entry("1", { source: "A", target: "甲", status: "translated" }),
      entry("2", { source: "B", target: "乙", status: "error", error: "boom" }),
    ]);
    expect(plan.retranslateAll).toBe(true);
    expect(plan.request.map((e) => e.target)).toEqual(["", ""]);
    expect(plan.request.map((e) => e.status)).toEqual(["pending", "pending"]);
    // 重译分支里每一条都是「本轮要翻的」（没有上下文条目，语义与旧实现一致）
    expect(plan.request.every(isTranslatableNow)).toBe(true);
    // 错误文案**保留**（本轮语义变更）：它是上一轮的结构诊断，后端会当纠错提示
    // 注入重试 prompt；界面上那份由调用方清掉。旧断言要求这里清成 null。
    expect(plan.request.map((e) => e.error)).toEqual([null, "boom"]);
  });

  it("空输入不触发重新翻译", () => {
    const plan = buildTranslationRequest([]);
    expect(plan.request).toEqual([]);
    expect(plan.retranslateAll).toBe(false);
  });

  it("不修改入参数组", () => {
    const list = [entry("1", { source: "A", target: "甲", status: "translated" })];
    buildTranslationRequest(list);
    expect(list[0].target).toBe("甲");
    expect(list[0].status).toBe("translated");
  });
});

describe("buildRetryRequest", () => {
  it("只返回错误条目并重置为待翻译", () => {
    const request = buildRetryRequest([
      entry("1", { status: "error", error: "timeout", target: "半截" }),
      entry("2", { status: "translated", target: "乙" }),
      entry("3", { status: "error", error: "boom" }),
      entry("4", { status: "error", error: "empty source", source: " " }),
    ]);
    expect(request.map((e) => e.id)).toEqual(["1", "3"]);
    expect(request[0]).toMatchObject({ target: "", status: "pending" });
  });

  /**
   * 上一轮的错误文案**必须**跟着 payload 一起发回后端：后端把它当纠错提示
   * 注入重试 prompt（`planner::previous_failure_of`），模型才知道上次错在哪。
   * 之前这里把它清成 null，于是「重试」永远只是原样再问一遍。
   */
  it("保留上一轮的错误文案，供后端注入纠错提示", () => {
    const reason =
      "大模型调用错误: 结构校验未通过：占位符 [1] 多出（已重试 1 次）";
    const request = buildRetryRequest([
      entry("1", { status: "error", error: reason, target: "半截译文" }),
    ]);

    expect(request[0].error).toBe(reason);
    expect(request[0].target).toBe("");
    expect(request[0].status).toBe("pending");
  });

  it("没有错误条目时返回空数组", () => {
    expect(buildRetryRequest([entry("1")])).toEqual([]);
  });
});

/**
 * `hasVisibleText` / `hasWritableTarget` 必须与 Rust 的
 * `crates/bg3-translate-core/src/types.rs::has_visible_text` **同义**。
 *
 * 为什么值得单独钉：这两个判据决定「MOD 自带的真实中文要不要被 incoming 挤掉」
 * （`localization::mergeWithExistingTarget`）。只有 BOM / 零宽空格的 target 会被
 * 后端当成「没有译文」而写回英文原文；前端若还按 `trim()` 判成「有译文」，
 * 自带中文就被静默覆盖了。跨层契约检查脚本覆盖不到这层语义。
 */
describe("hasVisibleText / hasWritableTarget", () => {
  it("只有空白 / 控制符 / 不可见零宽字符的译文不算可写回", () => {
    const invisible = [
      "",
      "   ",
      "\t\r\n",
      "\uFEFF", // BOM / 零宽不换行空格
      "\u200B", // 零宽空格
      "\u00AD", // 软连字符
      "\u2060", // word joiner
      "\u200E", // LRM
      "\u202E", // RLO
      "\uFE0F", // 变体选择符
      "\u0000", // C0 控制符
      "\u0085", // C1 控制符
      "\u200B\uFEFF\u00AD", // 全是不可见字符的组合
    ];
    for (const text of invisible) {
      expect(hasVisibleText(text), JSON.stringify(text)).toBe(false);
      expect(
        hasWritableTarget(entry(`x-${text.length}`, { target: text, status: "translated" })),
        JSON.stringify(text),
      ).toBe(false);
    }
  });

  it("零宽字符夹在正文中间仍算可见（变体选择符 / word joiner 是有意义的）", () => {
    const visible = [
      "火\u200B球\uFE0F术", // 零宽 + emoji 变体选择符夹在正文中间
      "普通中文",
      "a",
      "\u200B火",
      "火\u2060球",
      "1",
    ];
    for (const text of visible) {
      expect(hasVisibleText(text), JSON.stringify(text)).toBe(true);
      expect(
        hasWritableTarget(entry(`y-${text.length}`, { target: text, status: "translated" })),
        JSON.stringify(text),
      ).toBe(true);
    }
  });

  it("U+06DD 这类**可见**的 Cf 字符不能被当成空白（不按 Cf 类别一刀切）", () => {
    expect(hasVisibleText("\u06DD")).toBe(true);
    expect(hasWritableTarget(entry("z", { target: "\u06DD", status: "translated" }))).toBe(true);
  });

  it("error 状态一律不可写回，即便译文可见", () => {
    expect(hasWritableTarget(entry("e", { target: "火球", status: "error" }))).toBe(false);
    expect(hasWritableTarget(entry("e", { target: "火球", status: "pending" }))).toBe(true);
    expect(hasWritableTarget(entry("e", { target: "火球", status: "edited" }))).toBe(true);
  });
});
