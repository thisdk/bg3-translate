import { describe, expect, it } from "vitest";
import {
  TARGET_LANGUAGE,
  localizationWritePriority,
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
