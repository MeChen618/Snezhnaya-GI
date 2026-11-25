#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
server_root="$PWD"
export SNEZHNAYA_BIN_DIR="${SNEZHNAYA_BIN_DIR:-$server_root/bin}"
export RUST_LOG="${RUST_LOG:-info}"
mkdir -p logs .run
for required in assets/ExcelBinOutput/AvatarExcelConfigData.json assets/BinOutput/Quest assets/lua/scene assets/proto/7.2.0/all.proto; do
    [[ -e "$required" ]] || { echo "缺少资源 $required，环境包不完整。" >&2; exit 1; }
done
for binary in snezhnaya-gi-game-server snezhnaya-gi-dispatch-server; do
    [[ -x "$SNEZHNAYA_BIN_DIR/$binary" ]] || { echo "缺少 $binary。" >&2; exit 1; }
done
show_environment() {
    echo
    echo "SnezhnayaGI 环境已加载"
    [[ ! -f VERSION ]] || cat VERSION
    echo "配置目录  $server_root"
    echo "日志目录  $server_root/logs"
    echo "存档目录  $server_root/game.db"
    echo
}
check_resources() {
    "$SNEZHNAYA_BIN_DIR/snezhnaya-gi-game-server" --check-resources 2>&1 | tee logs/resource-check.log
}
run_component() {
    local selected="$1"
    echo "启动 $selected，按 Ctrl+C 停止并保存。"
    set +e
    bash scripts/start.sh release "$selected" 2>&1 | tee -a "logs/$selected.log"
    local status="${PIPESTATUS[0]}"
    set -e
    echo "服务已结束，退出码 $status"
}
show_environment
case "${1:-menu}" in
    --check) check_resources; exit ;;
    --start) run_component all; exit ;;
    --shell) exec bash --noprofile --norc ;;
    menu) ;;
    *) echo "用法：console.sh [--check|--start|--shell]" >&2; exit 2 ;;
esac
# 非交互调用只完成环境读取，避免后台意外常驻。
if [[ ! -t 0 ]]; then exit 0; fi
while true; do
    cat <<'MENU'
1) 启动 Dispatch 和游戏服
2) 只启动游戏服
3) 只启动 Dispatch
4) 完整校验资源
5) 进入环境终端
0) 退出
MENU
    read -r -p "请选择 [0] " choice || break
    case "$choice" in
        1) run_component all ;;
        2) run_component game ;;
        3) run_component dispatch ;;
        4) check_resources || echo "资源检查失败，详情见 logs/resource-check.log。" ;;
        5) bash --noprofile --norc ;;
        0|"") break ;;
        *) echo "请输入菜单中的数字。" ;;
    esac
    echo
done
