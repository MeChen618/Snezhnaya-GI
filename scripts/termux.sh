#!/usr/bin/env bash
set -euo pipefail
bundle_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rootfs="$bundle_root/rootfs"
if [[ ! -x "$rootfs/bin/bash" ]]; then
    echo "找不到 Debian 环境，请完整解压 tar.xz。" >&2
    exit 1
fi
if [[ "$(uname -m)" != aarch64 && "$(uname -m)" != arm64 ]]; then
    echo "这个环境包需要 ARM64 设备。" >&2
    exit 1
fi
proot_path="$(command -v proot || true)"
if [[ -z "$proot_path" ]]; then
    echo "Termux 缺少 proot，先执行 pkg install proot。" >&2
    exit 1
fi
case "$bundle_root" in /sdcard/*|/storage/*)
    echo '请解压到 Termux 私有目录，例如 $HOME/SnezhnayaGI。共享存储不支持运行环境。' >&2
    exit 1
esac
# 每次进入只建立一次环境，菜单里的启动项共用当前 Debian 会话。
export PROOT_NO_SECCOMP=1
mkdir -p "$rootfs/tmp" "$rootfs/root"
exec "$proot_path" --link2symlink -0 -r "$rootfs" \
    -b /dev -b /proc -b /sys -b "$bundle_root:/bundle" \
    -w /opt/SnezhnayaGI \
    /usr/bin/env -i \
    HOME=/root USER=root TERM="${TERM:-xterm-256color}" \
    LANG=C.UTF-8 PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    /bin/bash /opt/SnezhnayaGI/scripts/console.sh "$@"
