# AGENTS.md

给 AI 助手的说明。人类读者请直接看 [README.md](README.md)。

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

## 代码约定

- `src/protocol.rs` 是纯函数层，不引入任何 IO；新增协议处理请保持可单测。
- 新增芯片只加 `chips/*.toml`，不要改代码。
- 改动后跑 `cargo test`（29 个测试，无需硬件）和 `cargo clippy --all-targets`。
- 提交前用 `./target/release/stm32burn --dry-run` 验证配置能正确加载。

## GUI（gui/）

- **构建只用 `npx tauri build`，不要用 `cargo build --release`。** 后者会让 `tauri-build` 误判为 dev 模式，把 `devUrl`（`http://localhost:1420`）写进 exe，双击就是 `ERR_CONNECTION_REFUSED`。
- 便携版 exe 必须取完整 `tauri build` 的产物（带 bundle 类型标记）。`--no-bundle` 直出的裸 exe 在部分机器上会闪退，安装包里的那份却能正常运行。
- `build.rs` 已声明 `cargo:rerun-if-changed=../dist`，前端产物变化会自动触发重编。若仍怀疑 exe 没更新，先 `touch` 一下 `src-tauri/src/lib.rs`。
- 双击无反应时先看崩溃日志：`%TEMP%\stm32burn-gui-crash.log` 或 exe 同级的同名文件。
- `chips/*.toml` 通过 `bundle.resources` 打进安装包；便携版则放在 exe 同级的 `chips/`。
