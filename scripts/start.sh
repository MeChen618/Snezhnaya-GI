#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
profile="${1:-debug}"
component="${2:-all}"
case "$profile" in debug|release) ;; *) echo "用法：bash scripts/start.sh [debug|release] [all|game|dispatch]" >&2; exit 2 ;; esac
case "$component" in all|game|dispatch) ;; *) echo "启动项应为 all、game 或 dispatch" >&2; exit 2 ;; esac
binary_dir="${SNEZHNAYA_BIN_DIR:-${CARGO_TARGET_DIR:-target}/$profile}"
declare -a binaries=() children=()
[[ "$component" == dispatch ]] || binaries+=("$binary_dir/snezhnaya-gi-game-server")
[[ "$component" == game ]] || binaries+=("$binary_dir/snezhnaya-gi-dispatch-server")
for binary in "${binaries[@]}"; do
    if [[ ! -x "$binary" ]]; then
        echo "找不到可执行文件 $binary，请先编译或检查安装目录。" >&2
        exit 1
    fi
done
command -v flock >/dev/null || { echo "缺少 flock，请安装 util-linux。" >&2; exit 1; }
mkdir -p .run
if [[ "$component" != dispatch ]]; then
    exec 7>.run/game.lock
    flock -n 7 || { echo "游戏服已在运行。" >&2; exit 1; }
fi
if [[ "$component" != game ]]; then
    exec 8>.run/dispatch.lock
    flock -n 8 || { echo "Dispatch 已在运行。" >&2; exit 1; }
fi
cleanup() {
    trap - EXIT INT TERM
    if (("${#children[@]}")); then
        kill -TERM "${children[@]}" 2>/dev/null || true
        wait "${children[@]}" 2>/dev/null || true
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for binary in "${binaries[@]}"; do
    "$binary" &
    children+=("$!")
done
set +e
wait -n "${children[@]}"
status=$?
exit "$status"
