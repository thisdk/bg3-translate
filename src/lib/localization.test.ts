import { describe, expect, it } from "vitest";
import {
  TARGET_LANGUAGE,
  localizationWritePriority,
  mergeWithExistingTarget,
  planLocalizationWrites,
  toTargetLocalizationPath,
} from "./localization";
import type { PakFile, TranslationEntry } from "./types";

function pakFile(partial: Partial<PakFile> & { name: string }): PakFile {
  return {
    size: 0,
    kind: "localization-xml",
    language: null,
    ...partial,
  };
}

function entry(id: string, target = ""): TranslationEntry {
  return {
    id,
    sourceFile: "Localization/English/foo.xml",
    source: `source ${id}`,
    target,
    contentuid: id,
    version: "1",
    status: "pending",
    error: null,
  };
}

describe("toTargetLocalizationPath", () => {
  it("把 Localization/English/... 改写成 Localization/Chinese/...", () => {
    expect(toTargetLocalizationPath("Localization/English/foo.xml")).toBe(
      "Localization/Chinese/foo.xml",
    );
  });

  it("保留语言段之后的多级路径", () => {
    expect(
      toTargetLocalizationPath("Mods/MyMod/Localization/English/Sub/dir/a.loca"),
    ).toBe("Mods/MyMod/Localization/Chinese/Sub/dir/a.loca");
  });

  it("语言段大小写不敏感，输出统一为 Chinese", () => {
    expect(toTargetLocalizationPath("localization/english/a.xml")).toBe(
      "localization/Chinese/a.xml",
    );
    expect(toTargetLocalizationPath("LOCALIZATION/Polish/a.xml")).toBe(
      "LOCALIZATION/Chinese/a.xml",
    );
  });

  it("已是中文目录时保持幂等", () => {
    const once = toTargetLocalizationPath("Localization/English/a.xml");
    expect(toTargetLocalizationPath(once)).toBe(once);
  });

  it("没有 localization 段时原样返回", () => {
    expect(toTargetLocalizationPath("Mods/MyMod/meta.lsx")).toBe(
      "Mods/MyMod/meta.lsx",
    );
    expect(toTargetLocalizationPath("foo.xml")).toBe("foo.xml");
  });

  it("边界：Localization 段后不足两级时原样返回", () => {
    // 只有 Localization/<lang>，没有文件名 → 不改写
    expect(toTargetLocalizationPath("Localization/English")).toBe(
      "Localization/English",
    );
    // 只有 Localization 一级
    expect(toTargetLocalizationPath("Localization")).toBe("Localization");
    // localization 出现在最后一级
    expect(toTargetLocalizationPath("Mods/Foo/Localization")).toBe(
      "Mods/Foo/Localization",
    );
  });

  it("边界：Localization 出现在中间且后面正好两级 → 改写", () => {
    expect(toTargetLocalizationPath("Localization/English/x")).toBe(
      "Localization/Chinese/x",
    );
  });

  it("处理重复斜杠与首尾斜杠", () => {
    expect(toTargetLocalizationPath("/Localization//English//a.xml/")).toBe(
      "Localization/Chinese/a.xml",
    );
  });

  it("多个 localization 段时改写第一个", () => {
    expect(
      toTargetLocalizationPath("Localization/English/Localization/French/a.xml"),
    ).toBe("Localization/Chinese/Localization/French/a.xml");
  });

  it("目标语言常量为 Chinese", () => {
    expect(TARGET_LANGUAGE).toBe("Chinese");
  });
});

describe("localizationWritePriority", () => {
  it("英文优先级最高", () => {
    expect(localizationWritePriority(pakFile({ name: "a", language: "English" }))).toBe(3);
  });

  it("中文（含 ChineseSimplified）为次高", () => {
    expect(localizationWritePriority(pakFile({ name: "a", language: "Chinese" }))).toBe(2);
    expect(
      localizationWritePriority(pakFile({ name: "a", language: "ChineseSimplified" })),
    ).toBe(2);
  });

  it("其它语言与未知语言最低", () => {
    expect(localizationWritePriority(pakFile({ name: "a", language: "Polish" }))).toBe(1);
    expect(localizationWritePriority(pakFile({ name: "a", language: null }))).toBe(1);
  });
});

describe("planLocalizationWrites", () => {
  it("把不同语言目录映射到同一目标路径，英文胜出", () => {
    const english = pakFile({
      name: "Localization/English/foo.xml",
      language: "English",
    });
    const polish = pakFile({
      name: "Localization/Polish/foo.xml",
      language: "Polish",
    });
    const entriesByFile = {
      [english.name]: [entry("en-1")],
      [polish.name]: [entry("pl-1")],
    };

    const plans = planLocalizationWrites([polish, english], entriesByFile);

    expect(plans).toHaveLength(1);
    expect(plans[0].fileName).toBe("Localization/Chinese/foo.xml");
    expect(plans[0].priority).toBe(3);
    expect(plans[0].entries[0].id).toBe("en-1");
    expect(plans[0].sourceFile).toBe(english.name);
  });

  it("先出现低优先级、后出现高优先级时仍会被替换", () => {
    const english = pakFile({ name: "Localization/English/foo.xml", language: "English" });
    const polish = pakFile({ name: "Localization/Polish/foo.xml", language: "Polish" });
    const plans = planLocalizationWrites([polish, english], {
      [english.name]: [entry("en-1")],
      [polish.name]: [entry("pl-1")],
    });
    expect(plans).toHaveLength(1);
    expect(plans[0].entries[0].id).toBe("en-1");
  });

  it("先出现高优先级时不会被低优先级覆盖", () => {
    const english = pakFile({ name: "Localization/English/foo.xml", language: "English" });
    const polish = pakFile({ name: "Localization/Polish/foo.xml", language: "Polish" });
    const plans = planLocalizationWrites([english, polish], {
      [english.name]: [entry("en-1")],
      [polish.name]: [entry("pl-1")],
    });
    expect(plans[0].entries[0].id).toBe("en-1");
  });

  it("跳过没有条目或条目为空的文件", () => {
    const withEntries = pakFile({ name: "Localization/English/a.xml", language: "English" });
    const empty = pakFile({ name: "Localization/English/b.xml", language: "English" });
    const missing = pakFile({ name: "Localization/English/c.xml", language: "English" });
    const plans = planLocalizationWrites([withEntries, empty, missing], {
      [withEntries.name]: [entry("a-1")],
      [empty.name]: [],
    });
    expect(plans.map((p) => p.fileName)).toEqual(["Localization/Chinese/a.xml"]);
  });

  it("不同目标文件各自保留一份计划，顺序按首次出现", () => {
    const a = pakFile({ name: "Localization/English/a.xml", language: "English" });
    const b = pakFile({ name: "Localization/English/b.xml", language: "English" });
    const plans = planLocalizationWrites([b, a], {
      [a.name]: [entry("a-1")],
      [b.name]: [entry("b-1")],
    });
    expect(plans.map((p) => p.fileName)).toEqual([
      "Localization/Chinese/b.xml",
      "Localization/Chinese/a.xml",
    ]);
  });

  it("找不到 localization 段时目标路径与源路径一致", () => {
    const other = pakFile({ name: "Mods/Foo/meta.lsx", language: null });
    const plans = planLocalizationWrites([other], { [other.name]: [entry("m-1")] });
    expect(plans).toHaveLength(1);
    expect(plans[0].fileName).toBe("Mods/Foo/meta.lsx");
    expect(plans[0].priority).toBe(1);
  });
});

describe("mergeWithExistingTarget（写回已有文件的底稿合并）", () => {
  const en = "Localization/English/x.xml";
  const zh = "Localization/Chinese/x.xml";

  function incoming(contentuid: string, text: string, patch: Partial<TranslationEntry> = {}) {
    return {
      id: `${en}#${contentuid}`,
      sourceFile: en,
      source: text,
      target: "",
      contentuid,
      version: "1",
      status: "pending" as const,
      error: null,
      ...patch,
    };
  }

  function base(contentuid: string, text: string) {
    return {
      id: `${zh}#${contentuid}`,
      sourceFile: zh,
      source: text,
      target: "",
      contentuid,
      version: "1",
      status: "pending" as const,
      error: null,
    };
  }

  it("未翻译条目保留底稿文本，已翻译条目用新译文", () => {
    const merged = mergeWithExistingTarget(
      [
        incoming("uid-1", "Fireball", { target: "火球术", status: "translated" }),
        incoming("uid-2", "Ice"),
      ],
      [base("uid-1", "火球"), base("uid-2", "寒冰")],
    );

    expect(merged.map((e) => [e.contentuid, e.target || e.source])).toEqual([
      ["uid-1", "火球术"],
      ["uid-2", "寒冰"],
    ]);
  });

  it("error 条目不得覆盖底稿（被拒译文也不进 payload）", () => {
    const merged = mergeWithExistingTarget(
      [
        incoming("uid-1", "Fireball", {
          target: "被拒译文",
          status: "error",
          error: "结构校验未通过",
        }),
      ],
      [base("uid-1", "火球")],
    );

    expect(merged).toHaveLength(1);
    expect(merged[0].target).toBe("");
    expect(merged[0].source).toBe("火球");
    expect(merged[0].status).toBe("pending");
  });

  it("translating 条目（半截译文）同样保留底稿", () => {
    const merged = mergeWithExistingTarget(
      [incoming("uid-1", "Fireball", { target: "半截", status: "translating" })],
      [base("uid-1", "火球")],
    );
    expect(merged[0].source).toBe("火球");
    expect(merged[0].target).toBe("");
  });

  it("底稿有、incoming 没有的 contentuid 保留，顺序为 incoming 在前", () => {
    const merged = mergeWithExistingTarget(
      [incoming("uid-2", "Ice")],
      [base("uid-1", "火球"), base("uid-9", "只有底稿有")],
    );
    // uid-2 底稿里没有 → 保持 incoming 原文；uid-1 / uid-9 只有底稿有 → 原样保留
    expect(merged.map((e) => e.contentuid)).toEqual(["uid-2", "uid-1", "uid-9"]);
    expect(merged.map((e) => e.source)).toEqual(["Ice", "火球", "只有底稿有"]);
  });

  it("底稿里有同 contentuid 且 incoming 未翻译时保留底稿文本", () => {
    const merged = mergeWithExistingTarget(
      [incoming("uid-2", "Ice")],
      [base("uid-2", "寒冰")],
    );
    expect(merged.map((e) => [e.contentuid, e.source])).toEqual([["uid-2", "寒冰"]]);
  });

  it("底稿文本为空时退回 incoming（不写出空条目）", () => {
    const merged = mergeWithExistingTarget(
      [incoming("uid-1", "Fireball")],
      [base("uid-1", "")],
    );
    expect(merged).toHaveLength(1);
    expect(merged[0].source).toBe("Fireball");
  });

  it("底稿为空数组时原样返回 incoming", () => {
    const list = [incoming("uid-1", "Fireball")];
    expect(mergeWithExistingTarget(list, [])).toEqual(list);
  });
});

describe("planLocalizationWrites 写回闸门（半截译文不得进 PAK）", () => {
  it("translating 条目退回原文，target 不进入写回请求", () => {
    // 后端只在 status === "error" 时退回原文；translating 的非空 target
    // 会被当成真译文写进 PAK，所以写回计划必须自己把它降级。
    const file = pakFile({ name: "Localization/English/half.xml", language: "English" });
    const plans = planLocalizationWrites([file], {
      [file.name]: [
        entry("done-1", "已经翻好的译文"),
        {
          ...entry("half-1", "半截流式文本"),
          status: "translating",
        },
      ],
    });

    expect(plans).toHaveLength(1);
    expect(plans[0].entries.map((e) => [e.contentuid, e.target, e.status])).toEqual([
      ["done-1", "已经翻好的译文", "pending"],
      ["half-1", "", "pending"],
    ]);
  });

  it("不改动已翻译 / 已编辑 / error 条目的语义", () => {
    const file = pakFile({ name: "Localization/English/keep.xml", language: "English" });
    const translated = { ...entry("t-1", "译文"), status: "translated" as const };
    const edited = { ...entry("e-1", "手工译文"), status: "edited" as const };
    const failed = {
      ...entry("f-1", "被拒的半截文本"),
      status: "error" as const,
      error: "结构校验未通过",
    };

    const plans = planLocalizationWrites([file], {
      [file.name]: [translated, edited, failed],
    });

    // translated / edited 的 target 必须原样带走（后端据此写回译文）
    expect(plans[0].entries.map((e) => [e.contentuid, e.target, e.status])).toEqual([
      ["t-1", "译文", "translated"],
      ["e-1", "手工译文", "edited"],
      // error 条目必须保留 status === "error"：后端据此退回原文。
      // 若这里被改成 pending，被拒译文就会被写进 PAK。
      ["f-1", "被拒的半截文本", "error"],
    ]);
  });
});
