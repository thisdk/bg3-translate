#!/usr/bin/env python3
"""跨层 IPC 契约检查。

为什么需要它
------------
`src-tauri` 依赖 webkit2gtk / gtk / dbus 等 GUI 系统库，在没有这些库的机器
（以及 ubuntu CI）上编译不了；装了 GUI 系统库的 Linux 开发机可以真编。于是
「命令注册名写错」「前端调了一个不存在的命令」「参数名对不上」「文档里的
命令表腐烂了」这些问题，原本只会在 Windows 构建甚至运行时才暴露。这个脚本
只用标准库就能跑，放在 CI 最前面几秒出结果；本机 `bash scripts/verify.sh`
也会跑它。

检查项
------
1. 前端 `invoke` 过的每个命令，后端都必须在 `generate_handler!` 里注册
2. 后端注册的每个命令，都必须在 `README.md` 的命令表里出现
3. 命令表里不能有并未注册的命令
4. **命令参数名**三处一致：后端 `#[tauri::command]` 形参（Tauri 注入的
   `State` / `AppHandle` / `Window` 等不算）/ 前端 `invoke` 的对象字段
   （camelCase，经 Tauri 转 snake_case）/ 命令表的参数列。
   只查命令名会漏掉「`workDir` 写成 `workdir`」这类只有在运行期才炸的错误。
5. 命令层写了 `#[tauri::command]` 的每个函数都必须在 `generate_handler!` 里注册
   （只从注册表出发的检查看不见「定义了但没注册」的命令）
6. 命令的**返回类型**与命令表的返回列一致（`Result<Vec<PakFile>>` 归一成
   `PakFile[]`、`Result<()>` 归一成 `void` 再比对）
7. `TranslationEvent` / `TranslationStatus` / `PakFileKind` 三组枚举的
   serde 名称与前端 TypeScript 联合类型必须一致
8. 版本号一致：`package.json` / `src-tauri/tauri.conf.json` /
   `Cargo.toml [workspace.package]` / `Cargo.lock` 里两个工作区 crate
9. **检查器自检**：用合成输入钉住「命令表按标题级别截断」与「嵌套泛型的
   `invoke` 不被整条跳过」两条解析行为 —— 解析器一旦静默失效，上面 1~8
   都会变成绿色，所以它必须自己先证明自己还在工作

退出码：0 全部通过，1 存在不一致。
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

LIB_RS = "src-tauri/src/lib.rs"
COMMANDS_DIR = "src-tauri/src/commands"
API_TS = "src/lib/tauri.ts"
TYPES_TS = "src/lib/types.ts"
TYPES_RS = "crates/bg3-translate-core/src/types.rs"
# 冻结的命令表在 README 的「### Tauri 命令」一节。
# v1.5.0 起 `docs/` 不再随发布保留，命令表已从架构文档迁到 README；
# 本脚本只读 README，不再依赖 `docs/**` 下的任何文件。
ARCH = "README.md"
PACKAGE_JSON = "package.json"
TAURI_CONF = "src-tauri/tauri.conf.json"
CARGO_TOML = "Cargo.toml"
CARGO_LOCK = "Cargo.lock"

# 这些类型的形参由 Tauri 注入，不由前端传（不参与参数契约比较）。
INJECTED_PARAM_TYPES = {
    "State",
    "AppHandle",
    "Window",
    "WebviewWindow",
    "Webview",
    "Menu",
    "Runtime",
}


class Report:
    def __init__(self) -> None:
        self.failed = False

    def ok(self, message: str) -> None:
        print(f"\033[32m✓\033[0m {message}")

    def bad(self, message: str) -> None:
        self.failed = True
        print(f"\033[31m✗\033[0m {message}", file=sys.stderr)

    def detail(self, message: str) -> None:
        print(f"    {message}")


def read(relative: str) -> str:
    path = ROOT / relative
    if not path.is_file():
        print(f"缺少文件: {relative}", file=sys.stderr)
        sys.exit(1)
    return path.read_text(encoding="utf-8")


def snake_case(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def split_top_level(text: str) -> list[str]:
    """按**顶层**逗号切分，忽略 `<>` / `()` / `[]` / `{}` 内部与字符串里的逗号。"""
    parts: list[str] = []
    buf = ""
    depth = 0
    for char in text:
        if char in "<([{":
            depth += 1
        elif char in ">)]}":
            depth -= 1
        if char == "," and depth == 0:
            parts.append(buf)
            buf = ""
        else:
            buf += char
    if buf.strip():
        parts.append(buf)
    return parts


def matching_paren(source: str, open_index: int) -> int:
    """返回与 `source[open_index]`（`(`）配对的 `)` 下标；找不到返回 -1。"""
    depth = 0
    for index in range(open_index, len(source)):
        if source[index] == "(":
            depth += 1
        elif source[index] == ")":
            depth -= 1
            if depth == 0:
                return index
    return -1


def rust_command_signatures() -> dict[str, tuple[list[str], str]]:
    """`{命令名: (形参名, 返回类型原文)}`，扫 `src-tauri/src/commands/*.rs`。

    形参不含 Tauri 注入的 `State` / `AppHandle` / `Window` 等；返回类型是
    `->` 后面那一段原文（例如 `Result<Vec<TranslationEntry>>`），
    归一化交给 [`normalize_return_type`]。
    """
    commands: dict[str, tuple[list[str], str]] = {}
    for path in sorted((ROOT / COMMANDS_DIR).glob("*.rs")):
        source = path.read_text(encoding="utf-8")
        for hit in re.finditer(
            r"#\[tauri::command\]\s*pub\s+(?:async\s+)?fn\s+(\w+)\s*\(", source
        ):
            name = hit.group(1)
            start = hit.end() - 1
            end = matching_paren(source, start)
            if end < 0:
                continue
            params: list[str] = []
            for raw in split_top_level(source[start + 1 : end]):
                raw = raw.strip()
                if not raw:
                    continue
                param, colon, type_text = raw.partition(":")
                param = param.strip()
                if not colon:
                    # `self` 之类：命令函数不该有，原样记下来让契约检查报错
                    params.append(param)
                    continue
                type_name = type_text.strip().split("<")[0].split("::")[-1].strip()
                if type_name in INJECTED_PARAM_TYPES:
                    continue
                params.append(param)
            after = source[end + 1 :]
            returns = re.match(r"\s*->\s*([^{]+)\{", after, re.S)
            commands[name] = (params, returns.group(1).strip() if returns else "()")
    return commands


def normalize_return_type(text: str) -> str:
    """把 Rust 返回类型与文档写法归一成同一种表示。

    `Result<Vec<PakFile>>` → `PakFile[]`；`Result<()>` → `void`；
    `Result<Glossary>` → `Glossary`；文档里的 `\\`PakFile[]\\`` → `PakFile[]`。
    """
    clean = text.strip().strip("`").strip()
    result = re.fullmatch(r"Result\s*<\s*(.*?)\s*>", clean, re.S)
    if result:
        clean = result.group(1).strip()
    if clean in ("", "()"):
        return "void"
    vector = re.fullmatch(r"Vec\s*<\s*(.*?)\s*>", clean, re.S)
    if vector:
        return f"{normalize_type_name(vector.group(1))}[]"
    return normalize_type_name(clean)


def normalize_type_name(text: str) -> str:
    return text.strip().split("::")[-1].replace(" ", "")


def object_keys(body: str) -> list[str]:
    """对象字面量 `{ a, b: c }` 的字段名；解析不了的条目原样返回（会触发失败）。"""
    keys: list[str] = []
    for item in split_top_level(body):
        item = item.strip()
        if not item:
            continue
        key = item.split(":", 1)[0].strip()
        if re.fullmatch(r"[A-Za-z_$][A-Za-z0-9_$]*", key):
            keys.append(key)
        else:
            keys.append(f"<无法静态解析: {item}>")
    return keys


def js_invoke_args(source: str) -> dict[str, list[str]]:
    """`{命令名: [前端字段名]}`（保持 camelCase），扫前端 `invoke` 调用点。

    泛型实参用 `[^(]*` 而不是 `[^<>]*`：`invoke<Record<string, Entry[]>>(…)`
    这种**嵌套尖括号**的写法，旧正则匹配不上，于是整条调用被静默跳过 ——
    「前端 invoke 了但后端没注册」与「参数名对不上」两道检查同时失效。
    `[^(]*` 贪婪吃到 `(` 再回溯到最后一个 `>`，嵌套多少层都能跟上。
    没有第二个参数（`invoke("close_mod")`）时字段列表为空。
    """
    calls: dict[str, list[str]] = {}
    for hit in re.finditer(r'invoke\s*(?:<[^(]*>)?\s*\(\s*"([a-z_]+)"\s*(,?)', source):
        command = hit.group(1)
        keys: list[str] = []
        if hit.group(2):
            index = hit.end()
            while index < len(source) and source[index].isspace():
                index += 1
            if index < len(source) and source[index] == "{":
                depth = 0
                for end in range(index, len(source)):
                    if source[end] == "{":
                        depth += 1
                    elif source[end] == "}":
                        depth -= 1
                        if depth == 0:
                            keys = object_keys(source[index + 1 : end])
                            break
            else:
                # 传了第二个参数但不是对象字面量（变量/展开）：不猜，直接报错
                keys = ["<invoke 的第二个参数不是对象字面量>"]
        calls[command] = keys
    return calls


def doc_command_section(text: str) -> str:
    """取出 `### Tauri 命令` 一节的内容，到**下一个同级或更高级标题**为止。

    为什么必须按标题级别截断：命令表在 README 里（大节是 `##`），而
    `### Tauri 命令` 完全可能是某个 `##` 大节的最后一节。只认 `###` 的话，
    这一节的边界会一路吃到文件末尾，后面任意一张形状相同的表格都会被当成
    「命令表」—— 一条没写进命令表的命令，只要在 README 别处有一行同形状的
    表格就能让门禁变绿（静默绕过）。所以终止条件取 `#` ~ `###` 任意标题。
    """
    section = re.search(r"### Tauri 命令(.*?)(?:\n#{1,3}[ \t]|\Z)", text, re.S)
    return section.group(1) if section else ""


def doc_command_table(arch: str) -> dict[str, tuple[list[str], str]]:
    """`{命令名: (参数列字段, 返回列原文)}`，来自 `README.md` 的命令表。"""
    table: dict[str, tuple[list[str], str]] = {}
    for row in re.finditer(
        r"^\| `([a-z_]+)` \| (.*?) \| (.*?) \|\s*$", doc_command_section(arch), re.M
    ):
        params = re.findall(r"`([A-Za-z_][A-Za-z0-9_]*)`", row.group(2))
        table[row.group(1)] = (params, row.group(3).strip())
    return table


# ── 结构体字段契约的配对表：(Rust 文件, Rust 类型名, TypeScript 接口名) ──
#
# 「字段名即前端字段名」是这份契约里最容易静默腐烂的一环：改了 Rust 侧字段
# 却忘了改 types.ts，前端拿到的是 `undefined`（没有编译错误、没有运行时异常）。
STRUCT_CONTRACTS = [
    ("crates/bg3-translate-core/src/types.rs", "PakFile", "PakFile"),
    ("crates/bg3-translate-core/src/types.rs", "TranslationEntry", "TranslationEntry"),
    ("crates/bg3-translate-core/src/types.rs", "LlmSettings", "LlmSettings"),
    ("crates/bg3-translate-core/src/types.rs", "ExtractResult", "ExtractResult"),
    ("crates/bg3-translate-core/src/glossary/entry.rs", "GlossaryEntry", "GlossaryEntry"),
    ("crates/bg3-translate-core/src/glossary/store.rs", "Glossary", "Glossary"),
    ("src-tauri/src/commands/app.rs", "AppInfo", "AppInfo"),
]


def camel_case(name: str) -> str:
    head, *rest = name.split("_")
    return head + "".join(part[:1].upper() + part[1:] for part in rest)


def serde_fields(body: str, rename_all: str | None, require_pub: bool) -> list[str]:
    """结构体 / 枚举变体字段体 → **会上线**的 serde 字段名（按声明顺序）。

    - 容器级 `#[serde(rename_all = "camelCase")]` 展开；
    - 字段级 `#[serde(rename = "...")]` 优先；
    - `#[serde(skip)]` / `skip_serializing` / `skip_deserializing` 的字段**不算**
      （例如 `GlossaryEntry.category`，它只用于兼容旧导入数据）；
    - `skip_serializing_if` 不算「不上线」，字段仍在契约里。

    按**顶层逗号**切分而不是按行：`AllDone { total: usize, failed: usize }`
    这种一行里两个字段的写法必须都取到。
    """
    pattern = re.compile((r"pub (\w+)\s*:" if require_pub else r"(\w+)\s*:"))
    fields: list[str] = []
    for chunk in split_top_level(body):
        rename: str | None = None
        skipped = False
        code: list[str] = []
        for raw in chunk.splitlines():
            line = raw.strip()
            if not line or line.startswith("//"):
                continue
            if line.startswith("#["):
                if "serde" in line:
                    if re.search(r"\bskip(?:_serializing|_deserializing)?\s*[,)]", line):
                        skipped = True
                    hit = re.search(r'rename\s*=\s*"([^"]+)"', line)
                    if hit:
                        rename = hit.group(1)
                continue
            code.append(line)
        hit = pattern.match(" ".join(code))
        if not hit or skipped:
            continue
        rust_name = hit.group(1)
        if rename is not None:
            fields.append(rename)
        elif rename_all == "camelCase":
            fields.append(camel_case(rust_name))
        else:
            fields.append(rust_name)
    return fields


def rust_struct_fields(source: str, name: str) -> list[str] | None:
    """Rust 结构体的 serde 字段名；找不到结构体返回 None。"""
    hit = re.search(rf"((?:#\[[^\n]*\]\s*)*)pub struct {name} \{{(.*?)\n\}}", source, re.S)
    if not hit:
        return None
    rule = re.search(r'rename_all\s*=\s*"([^"]+)"', hit.group(1))
    return serde_fields(hit.group(2), rule.group(1) if rule else None, require_pub=True)


def ts_interface_fields(source: str, name: str) -> list[str] | None:
    """TypeScript 接口的字段名；找不到接口返回 None。"""
    hit = re.search(rf"export interface {name} \{{(.*?)\n\}}", source, re.S)
    if not hit:
        return None
    fields: list[str] = []
    for raw in hit.group(1).splitlines():
        line = raw.strip()
        if not line or line.startswith(("//", "/*", "*")):
            continue
        match = re.match(r"(\w+)\??\s*:", line)
        if match:
            fields.append(match.group(1))
    return fields


def rust_event_payloads(source: str) -> dict[str, list[str]]:
    """`{variant: [字段名]}`，来自 `pub enum TranslationEvent`。"""
    body = re.search(r"pub enum TranslationEvent \{(.*?)\n\}", source, re.S)
    if not body:
        return {}
    payloads: dict[str, list[str]] = {}
    for hit in re.finditer(r"(\w+) \{([^{}]*)\}", body.group(1), re.S):
        payloads[snake_case(hit.group(1))] = serde_fields(hit.group(2), None, require_pub=False)
    return payloads


def ts_event_payloads(source: str) -> dict[str, list[str]]:
    """`{variant: [字段名]}`，来自 `TranslationEvent` 的对象联合。"""
    payloads: dict[str, list[str]] = {}
    for hit in re.finditer(r'\{\s*type:\s*"([a-z_]+)"\s*;([^}]*)\}', _ts_block(source, "TranslationEvent")):
        fields: list[str] = []
        for part in hit.group(2).split(";"):
            match = re.match(r"\s*(\w+)\??\s*:", part)
            if match:
                fields.append(match.group(1))
        payloads[hit.group(1)] = fields
    return payloads


def rust_enum_variants(source: str, enum_name: str) -> list[str]:
    """取出 `pub enum X { A, B { .. }, C(..) }` 里的变体名（源文件顺序）。"""
    match = re.search(rf"pub enum {enum_name} \{{(.*?)\n\}}", source, re.S)
    if not match:
        return []
    body = match.group(1)
    # 变体的缩进层级固定；去掉文档注释与属性行
    names = []
    for line in body.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith(("//", "#[")):
            continue
        hit = re.match(r"([A-Z][A-Za-z0-9]*)", stripped)
        if hit:
            names.append(hit.group(1))
    return names


def ts_union_members(source: str, type_name: str) -> list[str]:
    """取出 `export type X = | "a" | "b";` 或对象联合里的字面量。"""
    lines = source.splitlines()
    start = next(
        (i for i, line in enumerate(lines) if line.startswith(f"export type {type_name} =")),
        None,
    )
    if start is None:
        return []
    body: list[str] = []
    for line in lines[start + 1 :]:
        # 声明块到此为止：出现顶格的新内容
        if line and not line.startswith((" ", "\t")):
            break
        body.append(line)
    return re.findall(r'"([A-Za-z_][A-Za-z0-9_-]*)"', "\n".join(body))


def self_check(report: Report) -> None:
    """用合成输入证明解析器还在工作（检查器的自检）。

    为什么值得写：这个脚本的价值全在「能发现不一致」，而它一旦解析失败，
    表现是**全绿**（少解析出一批命令 → 集合差为空 → 每条检查都通过）。
    历史上已经踩过两次，都是静默失效：
      1. 命令表边界只认 `###`：命令表迁到 README 后，若它是某个 `##` 大节的
         最后一节，正则会把后面所有章节都吞进「命令表」，一条没写进命令表的
         命令只要在别处有一行同形状的表格就能让门禁变绿；
      2. `invoke` 的泛型实参写成嵌套尖括号（`invoke<Record<string, T[]>>(…)`）
         时旧正则整条匹配不上，该命令既不参与「是否注册」也不参与「参数名」检查。
    自检用合成输入（不读仓库文件），所以它验证的是解析器本身的行为。
    """
    readme_like = (
        "### Tauri 命令\n\n"
        "| `real_cmd` | `someArg` | `void` |\n\n"
        "## 其它章节\n\n"
        "| `ghost_cmd` | `otherArg` | `void` |\n"
    )
    api_like = (
        'await invoke("real_cmd", { someArg });\n'
        'await invoke<Record<string, Entry[]>>("nested_cmd", { otherArg });\n'
    )

    problems: list[str] = []
    if sorted(doc_command_table(readme_like)) != ["real_cmd"]:
        problems.append(
            "命令表没有在 `##` 标题处截断（后面的表格被当成命令表，门禁可被静默绕过）"
        )
    if sorted(js_invoke_args(api_like)) != ["nested_cmd", "real_cmd"]:
        problems.append("嵌套泛型实参的 `invoke` 被整条跳过（命令名 / 参数名检查同时失效）")

    if problems:
        report.bad("契约检查器自检未通过 —— 解析器已失效，本次检查结果不可信：")
        for problem in problems:
            report.detail(problem)
        return
    report.ok("检查器自检通过（命令表按标题级别截断、嵌套泛型 invoke 可解析）")


def main() -> int:
    report = Report()

    # ── 0. 检查器自检 ───────────────────────────────────────────
    # 放在最前面：解析器静默失效时下面每条检查都会「通过」，所以先证明它还在工作。
    self_check(report)

    # ── 1/2. 命令注册 vs 前端调用 vs 命令表 ──────────────────────
    lib_rs = read(LIB_RS)
    handler = re.search(r"generate_handler!\[(.*?)\n\s*\]", lib_rs, re.S)
    if not handler:
        report.bad(f"没能从 {LIB_RS} 解析出 generate_handler!，检查写法是否变了")
        return 1
    registered = sorted(set(re.findall(r"commands::([a-z_]+)", handler.group(1))))

    api_ts = read(API_TS)
    # 命令名与参数都从**同一个**解析器取：两处各写一份正则时，`invoke` 的写法
    # 一变（例如泛型实参嵌套），两边会一起漏判，而且漏了也没人发现。
    js_args = js_invoke_args(api_ts)
    used = sorted(js_args)

    readme = read(ARCH)
    documented = sorted(set(re.findall(r"^\| `([a-z_]+)`", doc_command_section(readme), re.M)))

    print(
        f"命令：注册 {len(registered)} 个，前端使用 {len(used)} 个，命令表列出 {len(documented)} 个"
    )

    unregistered = sorted(set(used) - set(registered))
    if unregistered:
        report.bad("前端 invoke 了但后端未注册的命令（运行时会报错）：")
        for cmd in unregistered:
            report.detail(cmd)
    else:
        report.ok("前端 invoke 的每个命令都已在后端注册")

    undocumented = sorted(set(registered) - set(documented))
    if undocumented:
        report.bad(f"已注册但 {ARCH} 的命令表里没有的命令：")
        for cmd in undocumented:
            report.detail(cmd)
    else:
        report.ok(f"后端注册的命令全部写进了 {ARCH} 的命令表")

    phantom = sorted(set(documented) - set(registered))
    if phantom:
        report.bad(f"{ARCH} 命令表里列出但并未注册的命令（文档腐烂）：")
        for cmd in phantom:
            report.detail(cmd)
    else:
        report.ok(f"{ARCH} 命令表里没有虚构的命令")

    unused = sorted(set(registered) - set(used))
    if unused:
        print(f"  （提示）后端注册但前端暂未调用：{' '.join(unused)}")

    # ── 1b. 命令**参数**契约 ────────────────────────────────────
    # 命令名对得上、参数名写错，同样只会在运行期炸；三处（后端形参 /
    # 前端 invoke 字段 / 命令表）必须能互相转成同一组 snake_case 名字。
    signatures = rust_command_signatures()
    rust_params = {name: params for name, (params, _) in signatures.items()}
    doc_table = doc_command_table(readme)
    doc_params = {name: params for name, (params, _) in doc_table.items()}

    if not rust_params:
        report.bad(
            f"没能从 {COMMANDS_DIR}/*.rs 解析出任何 `#[tauri::command]` 签名，检查写法是否变了"
        )
    else:
        problems: list[str] = []
        for command in registered:
            rust = rust_params.get(command)
            if rust is None:
                problems.append(f"{command}: 命令层源码里找不到对应的 #[tauri::command] 签名")
                continue
            js = sorted(snake_case(key) for key in js_args.get(command, []))
            doc = sorted(snake_case(key) for key in doc_params.get(command, []))
            if sorted(rust) != js:
                problems.append(f"{command}: 后端形参 {sorted(rust)} != 前端 invoke 字段 {js}")
            if sorted(rust) != doc:
                problems.append(f"{command}: 后端形参 {sorted(rust)} != 文档命令表参数 {doc}")
        if problems:
            report.bad("命令参数契约不一致（前端字段名与后端形参必须能互转）：")
            for problem in problems:
                report.detail(problem)
        else:
            report.ok(f"{len(registered)} 个命令的参数名在后端 / 前端 invoke / 文档命令表三处一致")

    # ── 1c. **定义了但没注册**的命令 ────────────────────────────
    # 写了 `#[tauri::command]` 却忘了加进 `generate_handler!`：前端一 invoke
    # 就是「命令不存在」，而只从注册表出发的检查完全看不见它
    # （未注册 → 不在 registered 里 → 既不查文档也不查前端）。
    if rust_params:
        orphans = sorted(set(rust_params) - set(registered))
        if orphans:
            report.bad(f"命令层定义了但没在 {LIB_RS} 的 generate_handler! 里注册的命令：")
            for command in orphans:
                report.detail(command)
        else:
            report.ok(f"{len(rust_params)} 个 #[tauri::command] 全部已在后端注册")

    # ── 1d. 命令**返回类型**契约 ────────────────────────────────
    # 返回类型写错（文档说 `void`、实现返回结构体，或反过来）不会编译报错，
    # 只会让文档腐烂、让按文档写调用方的人拿到 undefined。
    if rust_params:
        return_problems: list[str] = []
        for command in registered:
            rust_ret = normalize_return_type(signatures.get(command, ([], "()"))[1])
            doc_ret = normalize_return_type(doc_table.get(command, ([], ""))[1])
            if rust_ret != doc_ret:
                return_problems.append(
                    f"{command}: 后端返回 {rust_ret}，文档命令表写的是 {doc_ret or '（空）'}"
                )
        if return_problems:
            report.bad(f"命令返回类型与 {ARCH} 命令表不一致：")
            for problem in return_problems:
                report.detail(problem)
        else:
            report.ok(f"{len(registered)} 个命令的返回类型与 {ARCH} 命令表一致")

    # ── 3. 枚举契约 ─────────────────────────────────────────────
    types_rs = read(TYPES_RS)
    types_ts = read(TYPES_TS)

    # TranslationEvent：Rust 用 snake_case tag，TS 用对象联合
    rust_event_variants = rust_enum_variants(types_rs, "TranslationEvent")
    rust_events = sorted({snake_case(name) for name in rust_event_variants})
    ts_events = sorted(set(re.findall(r'type:\s*"([a-z_]+)"', _ts_block(types_ts, "TranslationEvent"))))
    _compare(report, "TranslationEvent", rust_events, ts_events)

    # TranslationStatus / PakFileKind：都是小写 / kebab-case 字符串枚举
    rust_statuses = sorted({snake_case(name) for name in rust_enum_variants(types_rs, "TranslationStatus")})
    ts_statuses = sorted(set(_ts_literals(types_ts, "TranslationStatus")))
    _compare(report, "TranslationStatus", rust_statuses, ts_statuses)

    rust_kinds = sorted({_kebab(name) for name in rust_enum_variants(types_rs, "PakFileKind")})
    ts_kinds = sorted(set(_ts_literals(types_ts, "PakFileKind")))
    _compare(report, "PakFileKind", rust_kinds, ts_kinds)

    # ── 3b. 结构体 / 事件载荷的**字段**契约 ──────────────────────
    field_problems: list[str] = []
    for relative, rust_name, ts_name in STRUCT_CONTRACTS:
        rust_fields = rust_struct_fields(read(relative), rust_name)
        ts_fields = ts_interface_fields(types_ts, ts_name)
        if rust_fields is None or ts_fields is None:
            field_problems.append(
                f"{rust_name}: 解析失败（Rust={rust_fields}，TS={ts_fields}）"
            )
            continue
        if sorted(rust_fields) != sorted(ts_fields):
            only_rust = sorted(set(rust_fields) - set(ts_fields))
            only_ts = sorted(set(ts_fields) - set(rust_fields))
            field_problems.append(
                f"{rust_name}: Rust 独有 {only_rust} / types.ts 独有 {only_ts}"
            )

    rust_payloads = rust_event_payloads(types_rs)
    ts_payloads = ts_event_payloads(types_ts)
    if not rust_payloads or not ts_payloads:
        field_problems.append("TranslationEvent 载荷字段解析失败")
    else:
        for variant in sorted(set(rust_payloads) | set(ts_payloads)):
            rust_fields = sorted(rust_payloads.get(variant, []))
            ts_fields = sorted(ts_payloads.get(variant, []))
            if rust_fields != ts_fields:
                field_problems.append(
                    f"TranslationEvent::{variant}: Rust {rust_fields} != types.ts {ts_fields}"
                )

    if field_problems:
        report.bad("serde 字段契约不一致（字段名就是前端字段名，错了前端只会拿到 undefined）：")
        for problem in field_problems:
            report.detail(problem)
    else:
        report.ok(
            f"{len(STRUCT_CONTRACTS)} 个结构体 + TranslationEvent 全部变体的字段与 types.ts 逐字段一致"
        )

    # ── 4. 版本号一致性（含 Cargo.lock）──────────────────────────
    # Cargo.lock 是发布链路的硬约束：版本号改了却忘了更新 lock，
    # `cargo build --locked` 会直接失败，而普通 cargo 命令只会静默改写它。
    pkg_version = json.loads(read(PACKAGE_JSON)).get("version")
    tauri_version = json.loads(read(TAURI_CONF)).get("version")
    workspace_version = (
        tomllib.loads(read(CARGO_TOML)).get("workspace", {}).get("package", {}).get("version")
    )
    versions = {
        PACKAGE_JSON: pkg_version,
        TAURI_CONF: tauri_version,
        f"{CARGO_TOML} [workspace.package]": workspace_version,
    }
    if not pkg_version or len(set(versions.values())) != 1:
        report.bad("版本号三处不一致（必须同步更新）：")
        for where, value in versions.items():
            report.detail(f"{where} = {value!r}")
    else:
        report.ok(f"版本号三处一致：{pkg_version}（package.json / tauri.conf.json / Cargo.toml）")

    lock_versions = dict(
        re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', read(CARGO_LOCK))
    )
    workspace_crates = ["bg3-translate", "bg3-translate-core"]
    stale = {
        crate: lock_versions.get(crate)
        for crate in workspace_crates
        if lock_versions.get(crate) != workspace_version
    }
    if stale:
        report.bad(f"{CARGO_LOCK} 里工作区 crate 的版本号没跟上（--locked 构建会失败）：")
        for crate, value in stale.items():
            report.detail(f"{crate}: {CARGO_LOCK}={value!r}，{CARGO_TOML}={workspace_version!r}")
    else:
        report.ok(
            f"{CARGO_LOCK} 中 {'、'.join(workspace_crates)} 的版本与 workspace 一致：{workspace_version}"
        )

    if report.failed:
        print("\n契约检查未通过。", file=sys.stderr)
        return 1
    print()
    report.ok("IPC 契约检查全部通过")
    return 0


def _kebab(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "-", name).lower()


def _ts_block(source: str, type_name: str) -> str:
    lines = source.splitlines()
    start = next(
        (i for i, line in enumerate(lines) if line.startswith(f"export type {type_name} =")),
        None,
    )
    if start is None:
        return ""
    body: list[str] = []
    for line in lines[start + 1 :]:
        if line and not line.startswith((" ", "\t")):
            break
        body.append(line)
    return "\n".join(body)


def _ts_literals(source: str, type_name: str) -> list[str]:
    return re.findall(r'"([A-Za-z_][A-Za-z0-9_-]*)"', _ts_block(source, type_name))


def _compare(report: Report, name: str, rust: list[str], ts: list[str]) -> None:
    if not rust or not ts:
        report.bad(f"{name} 契约解析失败（Rust: {rust}，TS: {ts}）")
        return
    if rust == ts:
        report.ok(f"{name} 与前端联合类型一致：{' '.join(ts)}")
        return
    report.bad(f"{name} 契约不一致：")
    only_rust = sorted(set(rust) - set(ts))
    only_ts = sorted(set(ts) - set(rust))
    if only_rust:
        report.detail(f"Rust 独有: {' '.join(only_rust)}")
    if only_ts:
        report.detail(f"TS 独有:   {' '.join(only_ts)}")


if __name__ == "__main__":
    sys.exit(main())
