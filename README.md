# stm32burn

通过 USART 烧录 STM32 的命令行工具，使用芯片自带的系统存储器 Bootloader（ST AN3155 协议），**不需要 ST-Link**，一根 USB-TTL 就能烧。

## 快速开始

```bash
# 1. 编译
cargo build --release

# 2. 看有没有串口
./target/release/stm32burn --list-ports

# 3. 先干跑一遍，确认固件解析和地址规划没问题
./target/release/stm32burn app.elf COM3 -c chips/stm32f103c8.toml --dry-run

# 4. 正式烧录
./target/release/stm32burn app.elf COM3 -c chips/stm32f103c8.toml
```

接线（以 F103 为例）：

```
USB-TTL TX  →  PA10 (USART1_RX)
USB-TTL RX  →  PA9  (USART1_TX)
USB-TTL GND →  GND
```

烧录步骤：

1. **BOOT0 拨到 1**，按一下复位键
2. 运行命令，看到握手成功后自动擦除、写入
3. 结束后 **BOOT0 拨回 0**，再按复位运行程序

## 常用参数

```bash
stm32burn app.elf COM3                      # 默认 115200 8E1
stm32burn app.elf COM3 -b 230400            # 改波特率
stm32burn app.elf COM3 --verify             # 写完读回校验
stm32burn app.elf COM3 -e mass              # 整片擦除
stm32burn app.bin COM3 --base 0x08000000    # BIN 必须给地址
stm32burn app.elf COM3 --dry-run            # 只解析不烧
stm32burn --list-ports                      # 列出串口
```

## 配置文件

芯片参数写进 TOML，例如 `chips/stm32f103c8.toml`：

```toml
[chip]
name       = "STM32F103C8"
pids       = [0x0410]        # GET_ID 校验，防止烧错型号
flash_base = 0x08000000
flash_size = 0x10000
page_size  = 0x400
erase_mode = "page"

[uart]
baud   = 115200
parity = "even"
```

`-c` 省略时会依次查找：固件同目录同名 `.toml` → `./stm32burn.toml` → 可执行文件同目录。

## 退出码

`0` 成功 · `1` 参数错误 · `2` 串口打不开 · `3` 握手失败 · `4` PID 不匹配 · `5` 擦除失败 · `6` 写入失败 · `7` 校验失败

## 当前支持

- **芯片**：STM32F103 系列，已带 C8T6（64 KB / 1 KB 页）、RCT6（256 KB / 2 KB 页）、VET6（512 KB / 2 KB 页）三份配置。其它型号加一个 `chips/*.toml` 即可，Flash 布局一致的系列无需改代码。
- **固件**：`.elf`（ARM，取 LMA 地址）和 `.bin`（需 `--base`）。
- **擦除**：按页或整片。

暂不支持：Intel HEX、F4/G0/L4 的扩展扇区擦除、超过 256 页芯片的按页擦除（1 MB 型号需用 `-e mass`）、自动控制 BOOT0（需 DTR/RTS 电路）。

详细设计与协议笔记见 [DESIGN.md](DESIGN.md)。

## 添加新芯片

仓库里内置了一份**给 AI 助手用的适配流程**。clone 下来后，直接对 AI 说「帮我适配 STM32F103RCT6」即可，它会自己找到并读取：

| 工具 | 读取路径 |
|---|---|
| Claude Code | `.claude/skills/add-stm32-chip/SKILL.md` |
| DSH | `.dsh/skills/add-stm32-chip/SKILL.md` |
| 其它 | 根目录 [`AGENTS.md`](AGENTS.md) 会指向前两份 |

skill 里包含 F103 全系速查表、页大小分档规则、256 页边界处理和常见踩坑清单。AI 会按规则推导 Flash 大小、页大小和 PID，生成 `chips/*.toml`，并用 `--dry-run` 验证。

手动做的话，复制一份现有配置改四个字段即可：`pids`、`flash_size`、`page_size`、`erase_mode`。**页大小按容量分档**——高密度（256/512 KB）是 2 KB，其余 F103 是 1 KB，填错会导致擦除页号错位。


## 开发

```bash
cargo test        # 29 个测试，无需硬件
cargo build --release
```

协议层是纯函数，配合脚本化传输层可以在没有开发板的情况下验证帧格式。

> 如果 `cargo build` 卡在下载依赖，多半是环境里配了未运行的本地代理。可在项目下建 `.cargo/config.toml` 写 `[http] proxy = ""` 绕过（该文件已被 gitignore）。

