#!/usr/bin/env python3
"""Prepare AstaPS-Resource for SnezhnayaGI without modifying the source."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
RESOURCE_URL = "https://github.com/MeChen618/AstaPS-Resource.git"
RESOURCE_REVISION = "f2800e32599f32cde76ae0b4b93ea30b75651e59"


def read_json(path):
    # 任务密钥表保留了注释，先按字符串边界移除注释。
    text = path.read_text(encoding="utf-8-sig")
    output = []
    i, quoted, escaped = 0, False, False
    while i < len(text):
        char = text[i]
        if quoted:
            output.append(char)
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
            i += 1
        elif char == '"':
            quoted = True
            output.append(char)
            i += 1
        elif text.startswith("//", i):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end
        elif text.startswith("/*", i):
            end = text.find("*/", i + 2)
            if end < 0:
                raise ValueError(f"Unclosed comment in {path}")
            output.append(" ")
            i = end + 2
        else:
            output.append(char)
            i += 1
    return json.loads("".join(output))


def mount(source, destination, copy):
    if not source.is_dir():
        raise FileNotFoundError(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.is_symlink():
        if destination.resolve() != source.resolve():
            raise ValueError(f"{destination} already points to another resource directory")
        return
    if destination.exists():
        if copy:
            shutil.copytree(source, destination, dirs_exist_ok=True)
        return
    if not copy:
        try:
            destination.symlink_to(source.resolve(), target_is_directory=True)
            return
        except OSError:
            pass
    shutil.copytree(source, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("resource_dir", nargs="?", type=Path, default=ROOT / "resources")
    parser.add_argument("--copy", action="store_true", help="Copy resources instead of creating links")
    parser.add_argument("--revision", default=RESOURCE_REVISION, help="Revision used when cloning new resources")
    parser.add_argument("--refresh-custom", action="store_true", help="Refresh tables copied from Server/")
    args = parser.parse_args()
    source = args.resource_dir.expanduser().resolve()
    if not source.exists():
        subprocess.run(["git", "clone", "--depth", "1", "--filter=blob:none", "--sparse", RESOURCE_URL, str(source)], check=True)
        subprocess.run(["git", "-C", str(source), "sparse-checkout", "set", "BinOutput", "ExcelBinOutput", "Scripts", "Server"], check=True)
        subprocess.run(["git", "-C", str(source), "fetch", "--depth", "1", "origin", args.revision], check=True)
        subprocess.run(["git", "-C", str(source), "checkout", "--detach", "FETCH_HEAD"], check=True)
    for directory in ("BinOutput", "ExcelBinOutput", "Server"):
        mount(source / directory, ROOT / "assets" / directory, args.copy)
    for directory in ("Scene", "Common", "Gadget", "Quest"):
        mount(source / "Scripts" / directory, ROOT / "assets/lua" / directory.lower(), args.copy)
    custom = ROOT / "assets/custom"
    custom.mkdir(parents=True, exist_ok=True)
    for name in ("DropTableExcelConfigData.json", "DropSubTableExcelConfigData.json", "GadgetMapping.json", "QuestEncryptionKeys.json"):
        target = custom / name
        if args.refresh_custom or not target.exists():
            value = read_json(source / "Server" / name)
            target.write_text(json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    if not (custom / "GachaBanners.json").is_file():
        raise FileNotFoundError("assets/custom/GachaBanners.json is missing from the server checkout")
    print(f"Resources ready: {source}")
    print("BinOutput, ExcelBinOutput, Lua and server tables are prepared; existing custom settings are retained.")


if __name__ == "__main__":
    main()
