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

首次资源准备会克隆并固定到已验证的 AstaPS-Resource 提交 `f2800e32599f32cde76ae0b4b93ea30b75651e59`。也可以复用本地资源：

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

让现有 SDK／客户端的 Dispatch 地址指向本项目的 `http://127.0.0.1:21001`；账号接口仍指向现有登录服务。账号服务和本项目使用独立存储，账号 UID 必须稳定。现有登录服务不会自动共享角色存档。游戏服通过 `[sdk]` 校验游戏令牌，失败不会创建角色。AstaPS 使用 `provider = "astaps"`，`verify_url` 填 `server.dispatch.dispatchUrl`，两个密钥对应 `server.dispatch.dispatchKey` 和 `encryptionKey`（Base64），也支持 `SNEZHNAYA_SDK_DISPATCH_KEY`、`SNEZHNAYA_SDK_ENCRYPTION_KEY` 环境变量；该 Dispatch 服务需已启动。hoyo-sdk 使用 `provider = "hoyo_sdk"`，`verify_url` 填完整的 `/hk4e_cn/mdk/shield/api/verify` 地址。

远程连接时，修改两个 TOML 的监听地址，并同步修改 `assets/region/region_list.json` 的 `dispatch_url`、`gateserver_ip`、`gateserver_port` 为客户端可访问地址。HTTP、HTTPS 和证书需与现有登录服务／反向代理保持一致。随仓库提供的是上游测试密钥，客户端的区域与会话密钥配置必须匹配。`hot_fix_data` 默认留空；需要下载热更新时，填入与你的客户端完全对应的资源地址、版本及校验值。

启动脚本会同时管理 Dispatch 和游戏服，任一进程退出时清理另一个。正式构建可执行 `cargo build --locked --release --workspace -j 2`，然后 `bash scripts/start.sh release`。

可用 `bash scripts/start.sh release game` 或 `bash scripts/start.sh release dispatch` 单独启动对应服务。预编译环境包运行根目录的 `start.sh`，进入后自动读取环境，菜单可选择启动全部、单独启动、校验资源或进入终端。`SNEZHNAYA_BIN_DIR` 可指定预编译文件所在目录。启动时会拦截同一服务的重复运行，按 Ctrl+C 正常停止并保存。

## 验证

协议回归测试覆盖原生掩码、进场 token、地图分层、GUID、有符号属性和畸形包。资源检查读取实际配置；联网冒烟与压测工具仅在本地维护。

```bash
cargo test --locked -p snezhnaya-gi-proto --lib
./target/release/snezhnaya-gi-game-server --check-resources
```

[上游指令手册](handbook.md) 仍保留；部分内容随上游实现而变化。
