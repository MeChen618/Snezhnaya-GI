# SnezhnayaGI

English | [中文](README.md)

An experimental Rust server targeting **Genshin Impact 7.2.0 release clients**. Includes Dispatch and the game server. Account login uses your existing AstaPS / hoyo-sdk service.

## Upstream and license

Direct upstream: [renyi89/nod-krai-gi](https://github.com/renyi89/nod-krai-gi), based on commit [`c218474a387ff43840f30f4c5863e34bc7fc6951`](https://github.com/renyi89/nod-krai-gi/commit/c218474a387ff43840f30f4c5863e34bc7fc6951). The upstream derives from [mavuika-rs](https://git.xeondev.com/mavuika-rs/mavuika-rs). Original copyright notices and the [AGPL-3.0 license](LICENSE) are retained.

Data comes from [MeChen618/AstaPS-Resource](https://github.com/MeChen618/AstaPS-Resource); neither resources nor the client are redistributed here.

## Build and run

Install Rust stable, Git and Python 3. On Ubuntu / Debian:

```bash
sudo apt-get update
sudo apt-get install -y build-essential libclang-dev protobuf-compiler python3 git
python3 scripts/setup_resources.py
cargo build --locked --workspace -j 2
cargo test --locked -p snezhnaya-gi-proto --lib -j 1
./target/debug/snezhnaya-gi-game-server --check-resources
bash scripts/start.sh
```

The setup script pins new resource checkouts to validated commit `6cfecfb55f889f8e3b7156adb12809608db4f74b`. To reuse resources, pass the local directory to the script. It links `BinOutput`, `ExcelBinOutput` and Lua directories and imports server tables. Use `--copy` to copy files, or `--refresh-custom` to replace previously imported tables. The bundled permanent banner is a development configuration, not the current official banner schedule.

Run from the repository root. First launch creates `game-server.toml` and `dispatch-server.toml`. Default endpoints:

| Service | Address |
| --- | --- |
| Account login | Your existing service; upstream hoyo-sdk example uses `127.0.0.1:21000` |
| Dispatch | `127.0.0.1:21001`, TCP/HTTP |
| Game | `127.0.0.1:21002`, UDP/KCP |

Configure the existing SDK / client to use this Dispatch URL; keep account endpoints on the existing login service. Account UIDs must remain stable. Account storage and game saves are separate. Upstream does not validate SDK tokens online; use a trusted test network.

For remote clients, update the TOML bind addresses and `dispatch_url`, `gateserver_ip` and `gateserver_port` in `assets/region/region_list.json`. Match HTTP/HTTPS and certificates to your existing proxy. The bundled upstream test keys must match the client's region/session setup. `hot_fix_data` is empty by default; resource download requires URLs, versions and hashes matching the exact client build.

Dispatch provides `/health`, `/query_region_list` and `/query_cur_region`. The launcher supervises both server processes. For release builds use `cargo build --locked --release --workspace -j 2`, then `bash scripts/start.sh release`.

## Validation

Regression tests cover native mask formulas, zero values, scene tokens, map-layer lists, 64-bit GUIDs, signed 64-bit properties, integer-key maps inside oneofs and malformed packets. To exercise a running local game server:

```bash
cargo run --locked -p snezhnaya-gi-game-server --example protocol_smoke
```

The synthetic client creates a test character, performs UDP/KCP and RSA/session negotiation, logs in, enters a scene and requests map data. Optionally append a game address and set a numeric `SMOKE_ACCOUNT_UID`. This does not replace testing on a real Android client.

The [upstream command handbook](handbook.md) is retained.
