#!/usr/bin/env python3
"""跨层 IPC 契约检查。

为什么需要它
------------
`src-tauri` 依赖 webkit2gtk / gtk / dbus 等 GUI 系统库，在没有这些库的机器
（以及 ubuntu CI）上编译不了。于是「命令注册名写错」「前端调了一个不存在
的命令」「文档里的命令表腐烂了」这些问题，原本只会在 Windows 构建甚至运行
时才暴露。这个脚本只用标准库就能跑，放在 CI 最前面几秒出结果。

检查项
------
1. 前端 `invoke` 过的每个命令，后端都必须在 `generate_handler!` 里注册
2. 后端注册的每个命令，都必须在 `docs/ARCHITECTURE.md` 的命令表里出现
3. 文档命令表里不能有并未注册的命令
4. `TranslationEvent` / `TranslationStatus` / `PakFileKind` 三组枚举的
   serde 名称与前端 TypeScript 联合类型必须一致

退出码：0 全部通过，1 存在不一致。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

LIB_RS = "src-tauri/src/lib.rs"
API_TS = "src/lib/tauri.ts"
TYPES_TS = "src/lib/types.ts"
TYPES_RS = "crates/bg3-translate-core/src/types.rs"
ARCH = "docs/ARCHITECTURE.md"


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


def main() -> int:
    report = Report()

    # ── 1/2. 命令注册 vs 前端调用 vs 文档 ────────────────────────
    lib_rs = read(LIB_RS)
    handler = re.search(r"generate_handler!\[(.*?)\n\s*\]", lib_rs, re.S)
    if not handler:
        report.bad(f"没能从 {LIB_RS} 解析出 generate_handler!，检查写法是否变了")
        return 1
    registered = sorted(set(re.findall(r"commands::([a-z_]+)", handler.group(1))))

    api_ts = read(API_TS)
    used = sorted(set(re.findall(r'invoke(?:<[^>]*>)?\("([a-z_]+)"', api_ts)))

    arch = read(ARCH)
    section = re.search(r"### Tauri 命令(.*?)(?:\n### |\Z)", arch, re.S)
    documented = (
        sorted(set(re.findall(r"^\| `([a-z_]+)`", section.group(1), re.M))) if section else []
    )

    print(
        f"命令：注册 {len(registered)} 个，前端使用 {len(used)} 个，文档列出 {len(documented)} 个"
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
        report.bad(f"已注册但 {ARCH} 命令表里没有的命令：")
        for cmd in undocumented:
            report.detail(cmd)
    else:
        report.ok("后端注册的命令全部写进了架构文档")

    phantom = sorted(set(documented) - set(registered))
    if phantom:
        report.bad(f"{ARCH} 里列出但并未注册的命令（文档腐烂）：")
        for cmd in phantom:
            report.detail(cmd)
    else:
        report.ok("架构文档没有虚构的命令")

    unused = sorted(set(registered) - set(used))
    if unused:
        print(f"  （提示）后端注册但前端暂未调用：{' '.join(unused)}")

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
