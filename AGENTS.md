# AGENTS.md

给 AI 助手的说明。人类读者请直接看 [README.md](README.md)。

## 项目结构

| 路径 | 作用 |
|---|---|
| `src/protocol.rs` | AN3155 帧构造/解析，纯函数，无 IO |
| `src/burn.rs` | 擦除/写入/校验流程编排 |
| `src/elf.rs` | ELF/BIN 加载，取 LMA |
| `src/config.rs` | 芯片 TOML 解析 |
| `src/transport.rs` | 串口封装 |
| `src/main.rs` | CLI（clap） |
| `gui/` | Tauri 桌面界面，直接调 `src` 的库 |

## 用户说「帮我适配 XXX 芯片」时

读 `.claude/skills/add-stm32-chip/SKILL.md`（DSH 用户读 `.dsh/skills/add-stm32-chip/SKILL.md`，两份内容相同），按其中的流程执行：

1. 从型号名解析出 Flash 大小、页大小、PID
2. 生成 `chips/<型号小写>.toml`
3. 用 `--dry-run` 验证页数与地址范围
4. 告诉用户真机验证步骤

## 三条最容易出错的规则

- **页大小按容量分档**：F103 高密度（256/512 KB）是 **2 KB**，其余是 1 KB。填错不报错，但擦除页号会错位。
- **PID**：小容量 `0x412` / 中容量 `0x410` / 高密度 `0x414` / 互联 `0x418` / XL `0x430`。
- **F1 系列一律 `erase_mode = "page"`**，`extended`（0x44）是 F4/G0/L4 的命令。

## 协议实现要点（都是踩过的坑）

- **`MAX_PAGES_PER_ERASE = 255`，不是 256。** 擦除帧里页数字段是 `页数-1`，所以 N=256 时字段值为 `0xFF`，与整片擦除标记 `0xFF 0x00` 撞车，芯片会误执行 mass erase。512 KB 的 VET6 正好 256 页，必须拆成 255+1 两次。
- **ELF 一律取 `p_paddr`（LMA），不要用 `p_vaddr`。** 带 `.data` 初始值的固件里两者不同，用 vaddr 会写错地址。
- 写数据帧的长度字段是 `(N-1)`，校验是 `XOR(N-1, data...)`；读命令的长度字段是 `(N-1)` 但校验是 `XOR(N-1, !(N-1))`。
- 地址帧是 4 字节大端 + 单字节 XOR。
- 默认 8E1、115200，握手最多重试 20 次（每次 500 ms）。

## 构建

### CLI

```bash
cargo build --release
```

发布版要静态链接 C 运行库，否则用户机器缺 `VCRUNTIME140.dll` 会打不开：

```bash
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release
```

构建后可用 `dumpbin /DEPENDENTS` 确认只剩 `kernel32/ntdll/setupapi/cfgmgr32/advapi32/bcryptprimitives` 这类系统 DLL。

### GUI

```bash
cd gui
npx tauri build        # 生成 exe + NSIS 安装包
```

**不要用 `cargo build --release` 构建 GUI。** `tauri-build` 在 build script 里用 `cfg!(debug_assertions)` 判断模式，而 build script 本身恒按 debug 编译，结果会把 `devUrl`（`http://localhost:1420`）嵌进 release exe，双击就是 `ERR_CONNECTION_REFUSED`。`tauri build` 会设置 `TAURI_ENV_DEBUG=false` 来规避。

**便携版 exe 必须取完整 `tauri build` 的产物（带 bundle 类型标记）。** `--no-bundle` 直出的裸 exe 在部分机器上会闪退，而安装包里那份同样代码却能正常运行——差异就是 NSIS 打包时 patch 进去的 bundle 类型资源。

`build.rs` 已声明 `cargo:rerun-if-changed=../dist`，前端产物变化会触发重编。若仍怀疑 exe 没更新，`touch` 一下 `src-tauri/src/lib.rs`。

## 发版产物构成

| 资产 | 内容 |
|---|---|
| `stm32burn-vX.Y.Z-windows-x64.zip` | 只有 CLI：`stm32burn.exe` + `chips/` + 两份 README |
| `stm32burn-gui-vX.Y.Z-windows-x64.zip` | GUI + CLI 都在：`stm32burn-gui.exe` + `stm32burn.exe` + `chips/` + 两份 README |
| `stm32burn_X.Y.Z_x64-setup.exe` | NSIS 安装包，只装 GUI，**不带 CLI** |

版本号要同时改三处：`Cargo.toml`、`gui/src-tauri/Cargo.toml`、`gui/src-tauri/tauri.conf.json`。

发版命令：

```bash
git tag -a vX.Y.Z -m "stm32burn vX.Y.Z"
git push origin vX.Y.Z
gh release create vX.Y.Z <三个资产> --title "..." --notes-file <notes.md>
```

`--notes-file` 必须用文件，命令行里内联多行 markdown 会被 shell 转义搞坏。

## 文档约定

- 英文为主，中文并存：`README.md` ⇄ `README_zh.md`，`DESIGN.md` ⇄ `DESIGN_zh.md`，两两之间顶部互相链接。
- CLI 帮助：`--help` 输出英文（clap 自动生成），`--chelp` 输出中文（`print_chinese_help()` 手写）。加参数时**两处都要改**。
- `#[command(bin_name = "stm32burn")]` 不能删，否则 Usage 行会显示 `stm32burn.exe`。

## 已知环境问题

- **crates.io 代理**：全局配置的 `127.0.0.1:10808` 断开时 `cargo build` 会卡住。项目内 `.cargo/config.toml` 写 `[http] proxy = ""` 可绕过（该文件已 gitignore）。
- **WebView2 `0x8000FFFF`**：WebView2 无法在数据目录创建配置时出现，常见于终端防护软件拦截。设 `WEBVIEW2_USER_DATA_FOLDER` 指向可写目录可绕过。GUI 的 panic 日志写在 `%TEMP%\stm32burn-gui-crash.log` 和 exe 同级。
- **git push**：`http.proxy` 指向未启动的 v2rayN 时连接被拒。可先试 `git -c http.proxy= push` 直连；仍失败再用 `gh api` 的 Git Data API 逐块提交。

## 代码约定

- `src/protocol.rs` 是纯函数层，不引入任何 IO；新增协议处理请保持可单测。
- 新增芯片只加 `chips/*.toml`，不要改代码。
- 改动后跑 `cargo test`（29 个测试，无需硬件）和 `cargo clippy --all-targets`。
- 提交前用 `./target/release/stm32burn --dry-run` 验证配置能正确加载。
- GUI 前端改了样式或逻辑，记得 `npx tauri build` 后再验证，别直接双击旧的 `target/release` 产物。
