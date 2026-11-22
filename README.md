# SnezhnayaGI

[English](README.en.md) | 中文

基于 Rust 的某二次元动漫游戏正式服实验性服务端。包含 Dispatch 和游戏服，账号登录接已有的 AstaPS

## 上游与许可

直接上游：[renyi89/nod-krai-gi](https://github.com/renyi89/nod-krai-gi)，基线提交 [`c218474a387ff43840f30f4c5863e34bc7fc6951`](https://github.com/renyi89/nod-krai-gi/commit/c218474a387ff43840f30f4c5863e34bc7fc6951)。上游源自 [mavuika-rs](https://git.xeondev.com/mavuika-rs/mavuika-rs)。保留原作者版权和 [AGPL-3.0 许可](LICENSE)。

资源使用 [MeChen618/AstaPS-Resource](https://github.com/MeChen618/AstaPS-Resource)。资源与客户端不包含在本仓库内。

## 编译与资源

Linux 需要 Rust stable、Git、Python 3、C/C++ 编译器、libclang 和 protoc。Ubuntu／Debian：

```bash
sudo apt-get update
sudo apt-get install -y build-essential libclang-dev protobuf-compiler python3 git
# 安装 Rust 后执行
python3 scripts/setup_resources.py
cargo build --locked --workspace -j 2
cargo test --locked -p snezhnaya-gi-proto --lib -j 1
```

首次资源准备会克隆并固定到已验证的 AstaPS-Resource 提交 `6cfecfb55f889f8e3b7156adb12809608db4f74b`。也可以复用本地资源：

```bash
python3 scripts/setup_resources.py /path/to/AstaPS-Resource
# Windows 无法创建目录链接时自动复制，也可以显式使用 --copy
```

脚本链接 `BinOutput`、`ExcelBinOutput`、`Scripts`，并从 `Server` 转换必要的服务端表。已有自定义表默认保留，更新它们时加 `--refresh-custom`。开发用常驻卡池在 `assets/custom/GachaBanners.json`，并非当前官服卡池配置。

## 启动与连接

请在仓库根目录运行：

```bash
./target/debug/snezhnaya-gi-game-server --check-resources
bash scripts/start.sh
```

第一次运行会生成 `game-server.toml` 和 `dispatch-server.toml`。

让现有 SDK／客户端的 Dispatch 地址指向本项目的 `http://127.0.0.1:21001`；账号接口仍指向现有登录服务。账号服务和本项目使用独立存储，账号 UID 必须稳定。现有登录服务不会自动共享角色存档。上游游戏服未实现 SDK token 在线校验，请在可信测试网络中使用。

远程连接时，修改两个 TOML 的监听地址，并同步修改 `assets/region/region_list.json` 的 `dispatch_url`、`gateserver_ip`、`gateserver_port` 为客户端可访问地址。HTTP、HTTPS 和证书需与现有登录服务／反向代理保持一致。随仓库提供的是上游测试密钥，客户端的区域与会话密钥配置必须匹配。`hot_fix_data` 默认留空；需要下载热更新时，填入与你的客户端完全对应的资源地址、版本及校验值。

启动脚本会同时管理 Dispatch 和游戏服，任一进程退出时清理另一个。正式构建可执行 `cargo build --locked --release --workspace -j 2`，然后 `bash scripts/start.sh release`。

## 验证

协议回归测试覆盖原生掩码公式、零值、进场 token、地图分层列表、64 位 GUID、有符号 64 位属性、oneof 内数字键映射及畸形网络包。资源检查会读取实际配置；联网检查使用独立的合成客户端：

```bash
# 游戏服启动后，在另一终端运行；会创建测试角色
cargo run --locked -p snezhnaya-gi-game-server --example protocol_smoke
```

测试客户端默认连接 `127.0.0.1:21002`，可在命令后追加地址，通过 `SMOKE_ACCOUNT_UID` 指定隔离的数字测试账号。它验证 UDP/KCP、RSA／会话密钥、登录、进场和地图请求，不代替安卓客户端实机验收。

[上游指令手册](handbook.md) 仍保留；部分内容随上游实现而变化。
