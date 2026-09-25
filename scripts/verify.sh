#!/usr/bin/env bash
# =============================================================================
# scripts/verify.sh —— 本地一键质量门禁
#
# 一条命令跑完全部本地可跑的质量门禁，任一阶段失败立即非零退出：
#   1. ipc          跨层 IPC 契约检查（python3 标准库，几秒）
#   2. core-fmt     cargo fmt --all --check
#   3. core-clippy  cargo clippy -p bg3-translate-core --all-targets -- -D warnings
#   4. core-test    cargo test -p bg3-translate-core --all-targets
#   5. web-test     bun run test
#   6. web-build    bun run build（= tsc -b 类型检查 + vite build）
#
# 用法：
#   bash scripts/verify.sh                # 全部 6 道门禁
#   bash scripts/verify.sh --core-only    # 跳过前端（ipc + Rust 核心）
#   bash scripts/verify.sh --web-only     # 跳过 Rust 核心（ipc + 前端）
#   bash scripts/verify.sh --no-ipc       # 跳过 IPC 契约（CI 的 meta job 已单独跑）
#   bash scripts/verify.sh --list         # 只打印门禁清单，不执行任何命令、不需要任何工具链
#   bash scripts/verify.sh --help
#
# 退出码：
#   0  全部通过
#   非 0  某道门禁失败（原样透传该命令的退出码，例如 clippy 的 101）
#   2  用法错误或缺少必需工具
#
# 说明：
#   * 不新增任何依赖：只用 python3（标准库）、cargo（rustfmt/clippy 组件）、bun。
#   * 不改动受版本控制的文件：dist/、target/、*.tsbuildinfo 都是 .gitignore 里的构建产物。
#   * 为什么没有 src-tauri：Tauri 壳依赖 webkit2gtk/gtk/dbus 等 GUI 系统库，
#     在多数开发机与 ubuntu CI 上都编不了，它的 check/clippy 固定在 CI 的
#     Windows tauri-shell job 里跑（见 .github/workflows/ci.yml）。
#   * 在 GitHub Actions 里会自动用 ::group:: 折叠每个阶段、失败时打 ::error:: 注解。
# =============================================================================

set -euo pipefail

# 仓库根目录 = 本脚本所在目录的上一级；因此从任何 cwd 调用都可以。
# 只用 bash 内建展开取目录，不依赖外部的 dirname（--list 因此不需要任何工具链）。
case "$0" in
*/*) SCRIPT_DIR=${0%/*} ;;
*) SCRIPT_DIR=. ;;
esac
SCRIPT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
cd "$ROOT"

RUN_IPC=1
RUN_CORE=1
RUN_WEB=1
MODE=all

usage() {
	cat <<'EOF'
scripts/verify.sh —— 本地一键质量门禁

用法：
  bash scripts/verify.sh [选项]

选项：
  --core-only   只跑 IPC 契约 + Rust 核心库（fmt / clippy / test）
  --web-only    只跑 IPC 契约 + 前端（vitest / tsc + vite build）
  --no-ipc      跳过跨层 IPC 契约检查（CI 的 meta job 已经单独跑过）
  --list        只打印门禁清单（TAB 分隔的 <阶段名><命令>），不做任何执行
  -h, --help    显示本帮助

退出码：0 通过；非 0 某道门禁失败（透传退出码）；2 用法错误或缺少工具。
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
	-h | --help)
		usage
		exit 0
		;;
	--core-only)
		RUN_WEB=0
		MODE="core-only"
		;;
	--web-only)
		RUN_CORE=0
		MODE="web-only"
		;;
	--no-ipc)
		RUN_IPC=0
		;;
	--list)
		MODE="list"
		;;
	*)
		printf '未知参数：%s\n\n' "$1" >&2
		usage >&2
		exit 2
		;;
	esac
	shift
done

if [ "$RUN_CORE" = 0 ] && [ "$RUN_WEB" = 0 ]; then
	echo "错误：--core-only 与 --web-only 不能同时使用。" >&2
	exit 2
fi

# ---------------------------------------------------------------------------
# 门禁清单（唯一事实来源）
#
# 阶段用 "组|标题|命令" 表示；命令以 bash -euo pipefail -c 执行，
# 所以 --list 打印出来的命令与实际执行的字面完全一致，不会漂移。
# ---------------------------------------------------------------------------
PLAN=""
plan_add() {
	if [ -z "$PLAN" ]; then
		PLAN="$1|$2|$3"
	else
		PLAN="$PLAN"$'\n'"$1|$2|$3"
	fi
}

if [ "$RUN_IPC" = 1 ]; then
	plan_add "ipc" "跨层 IPC 契约检查" "python3 scripts/check_ipc_contract.py"
fi
if [ "$RUN_CORE" = 1 ]; then
	plan_add "core-fmt" "Rust 代码格式（cargo fmt --check）" "cargo fmt --all --check"
	plan_add "core-clippy" "Rust Clippy 零告警" "cargo clippy -p bg3-translate-core --all-targets -- -D warnings"
	plan_add "core-test" "Rust 核心库测试（含端到端 PAK 闭环）" "cargo test -p bg3-translate-core --all-targets"
fi
if [ "$RUN_WEB" = 1 ]; then
	plan_add "web-test" "前端单元测试（vitest）" "bun run test"
	plan_add "web-build" "前端类型检查 + 构建（tsc -b && vite build）" "bun run build"
fi

if [ -z "$PLAN" ]; then
	echo "错误：没有选中任何门禁（--no-ipc 与 --core-only/--web-only 组合无效）。" >&2
	exit 2
fi

# --list：机器可读输出（TAB 分隔），交给 CI 断言门禁没有被悄悄删掉。
if [ "$MODE" = "list" ]; then
	while IFS='|' read -r group title cmd; do
		[ -n "$group" ] || continue
		# 被 head 之类的消费者提前关掉管道时安静结束，不算失败
		printf '%s\t%s\n' "$group" "$cmd" 2>/dev/null || break
	done <<<"$PLAN"
	exit 0
fi

# ---------------------------------------------------------------------------
# 环境自检：只检查选中的门禁真正需要的工具，缺什么一次性说清楚。
# ---------------------------------------------------------------------------
HAVE_PYTHON=0
if command -v python3 >/dev/null 2>&1; then
	HAVE_PYTHON=1
fi

MISSING=""
need_tool() { # $1=命令名 $2=用途
	if ! command -v "$1" >/dev/null 2>&1; then
		MISSING="$MISSING  - $1（$2）"$'\n'
	fi
}
if [ "$RUN_IPC" = 1 ]; then
	need_tool python3 "IPC 契约检查"
fi
if [ "$RUN_CORE" = 1 ]; then
	need_tool cargo "Rust 核心库门禁（还需要 rustfmt + clippy 组件：rustup component add rustfmt clippy）"
fi
if [ "$RUN_WEB" = 1 ]; then
	need_tool bun "前端门禁（bun run test / bun run build）"
fi
if [ -n "$MISSING" ]; then
	printf '缺少必需工具：\n%s' "$MISSING" >&2
	echo "安装后重试；只想跑一部分可以加 --core-only / --web-only。" >&2
	exit 2
fi

# ---------------------------------------------------------------------------
# 输出与计时
# ---------------------------------------------------------------------------
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
	C_BOLD=$'\033[1m'
	C_DIM=$'\033[2m'
	C_GREEN=$'\033[32m'
	C_RED=$'\033[31m'
	C_RESET=$'\033[0m'
else
	C_BOLD= C_DIM= C_GREEN= C_RED= C_RESET=
fi

now_ms() {
	if [ "$HAVE_PYTHON" = 1 ]; then
		python3 -c 'import time; print(int(time.time() * 1000))'
	else
		# 没有 python3 时退回整秒精度（只影响显示，不影响判定）
		echo $((SECONDS * 1000))
	fi
}

fmt_ms() {
	ms=$1
	if [ "$ms" -lt 1000 ]; then
		printf '%sms' "$ms"
	else
		printf '%s.%ss' "$((ms / 1000))" "$(((ms % 1000) / 100))"
	fi
}

TOTAL=0
while IFS='|' read -r group title cmd; do
	[ -n "$group" ] || continue
	TOTAL=$((TOTAL + 1))
done <<<"$PLAN"

printf '%s\n' "${C_BOLD}== bg3-translate 一键校验 ==${C_RESET}"
printf '%s\n' "${C_DIM}仓库：$ROOT"
printf '模式：%s，共 %s 道门禁%s\n' "$MODE" "$TOTAL" "${C_RESET}"

RUN_TOTAL_START=$(now_ms)
IDX=0
FAILED_KEY=""
FAILED_TITLE=""
FAILED_CMD=""
FAILED_RC=0

while IFS='|' read -r group title cmd; do
	[ -n "$group" ] || continue
	IDX=$((IDX + 1))

	printf '\n%s\n' "${C_BOLD}── [$IDX/$TOTAL] $title${C_RESET}"
	printf '%s\n' "   ${C_DIM}\$ $cmd${C_RESET}"
	if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
		printf '::group::%s\n' "[$IDX/$TOTAL] $title"
	fi

	start=$(now_ms)
	rc=0
	bash -euo pipefail -c "$cmd" || rc=$?
	end=$(now_ms)
	elapsed=$((end - start))

	if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
		printf '::endgroup::\n'
	fi

	if [ "$rc" -eq 0 ]; then
		printf '%s\n' "   ${C_GREEN}✓ 通过${C_RESET}（$(fmt_ms "$elapsed")）"
	else
		printf '%s\n' "   ${C_RED}✗ 失败：$group 退出码 $rc（$(fmt_ms "$elapsed")）${C_RESET}" >&2
		if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
			printf '::error title=verify.sh %s::%s\n' "$group" "$cmd"
		fi
		FAILED_KEY="$group"
		FAILED_TITLE="$title"
		FAILED_CMD="$cmd"
		FAILED_RC="$rc"
		break
	fi
done <<<"$PLAN"

RUN_TOTAL_END=$(now_ms)
RUN_TOTAL_MS=$((RUN_TOTAL_END - RUN_TOTAL_START))

if [ -n "$FAILED_KEY" ]; then
	printf '\n%s\n' "${C_RED}✗ 门禁未通过：${FAILED_TITLE}（${FAILED_KEY}）${C_RESET}" >&2
	printf '  命令：%s\n' "$FAILED_CMD" >&2
	printf '  已跑 %s/%s 道，总耗时 %s，退出码 %s\n' "$IDX" "$TOTAL" "$(fmt_ms "$RUN_TOTAL_MS")" "$FAILED_RC" >&2
	exit "$FAILED_RC"
fi

printf '\n%s\n' "${C_GREEN}✓ 全部 $TOTAL 道门禁通过${C_RESET}（总耗时 $(fmt_ms "$RUN_TOTAL_MS")）"
