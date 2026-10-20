# stm32burn 设计说明

**[English](DESIGN.md) | 中文**

通过 USART 烧录 STM32 的命令行工具，走芯片出厂固化的系统存储器 Bootloader（ST **AN3155** 协议），不依赖 ST-Link 或任何专用调试器。

## 一、定位与范围

- **只用串口**：一根 USB-TTL（CH340 / CP2102 / FT232）接 PA9/PA10 即可烧录
- **纯 CLI**：没有图形界面，适合脚本、批量和远程场景
- **配置驱动**：芯片参数放在 TOML 里，类似 OpenOCD 的 target cfg
- **第一版目标**：STM32F103（以 C8T6 为基准），架构上能扩展到 F4/G0/L4

非目标：调试（断点、寄存器）、读保护设置、Intel HEX、GUI。

## 二、为什么不用自研协议

STM32 芯片内部固化了一个 bootloader，`BOOT0=1` 复位后从系统存储器启动，通过 USART1（PA9/PA10）走 AN3155 协议。我们要实现的只是**主机侧**，芯片侧的行为是 ST 保证的。这也是 FlyMcu / mcuisp / STM32CubeProgrammer 的 UART 模式所做的事。

## 三、架构

```
main.rs        CLI 参数、流程编排、进度输出、退出码
  └─ burn.rs   Burner：把协议操作编排成 擦除/写入/校验
       ├─ protocol.rs   帧构造与应答解析（纯函数，无 IO）
       └─ transport.rs  Transport trait + SerialTransport
config.rs      芯片配置解析、校验、页号换算
elf.rs         ELF32 / BIN 解析 → 烧写镜像列表
```

关键设计：**协议层不直接操作串口**，只依赖 `Transport` trait。这样没有硬件也能用脚本化传输层跑完整流程测试，抓字节序错误。

## 四、协议要点（含实际踩过的坑）

### 帧格式

| 帧 | 字节序列 |
|---|---|
| 命令 | `cmd, !cmd` |
| 地址 | `addr[4] 大端, addr[0]^addr[1]^addr[2]^addr[3]` |
| 写数据 | `N-1, data..., (N-1) ^ data[0] ^ ... ^ data[N-1]` |
| 读长度 | `N-1, !(N-1)` |
| 页擦除 | `页数-1, 页号...（每页 1 字节）, 校验` |
| 全片擦除 | `0xFF, 0x00` |

应答：`ACK = 0x79`，`NACK = 0x1F`。

### 必须记住的六个细节

1. **串口是 8E1**（偶校验），不是常见的 8N1。搞错会全程 NACK。
2. **擦除页号是 1 字节**，不是 2 字节。2 字节页号属于扩展擦除（0x44），只在 F4/G0/L4 上用。
3. **全片擦除帧只有两个字节** `0xFF 0x00`，不是 `0xFF 0xFF ...`。
4. **写数据必须补齐到 4 的倍数**，补 `0xFF`（擦除后的值）。
5. **读长度帧的校验是补码**（`!N`），而写数据帧的校验是异或——两者不一样。
6. **ELF 取 `p_paddr`（LMA）**，不是 `p_vaddr`。`.data` 段的 VMA 在 RAM、LMA 在 Flash，用错会把初始值写进内存。

### 流程

```
0x7F 同步（可重试）→ GET_ID 校验型号 → 擦除 → 逐块写入 → 可选读回校验
```

握手前先 `flush` 接收缓冲：串口刚打开时常见残留数据，会让第一次 `0x7F` 失败。

## 五、配置文件

```toml
[chip]
name       = "STM32F103C8"
pids       = [0x0410]        # GET_ID 读回值，不在列表内直接拒绝烧录
flash_base = 0x08000000
flash_size = 0x10000
page_size  = 0x400
erase_mode = "page"          # page | mass | extended
ram_base   = 0x20000000

[uart]
baud      = 115200
parity    = "even"
data_bits = 8
stop_bits = 1
```

`pids` 是防「烧错型号」的关键：`GET_ID` 读回 0x0410 才继续，避免把 F407 的固件烧进 F103。

查找顺序：`-c` 指定 → 固件同目录同名 `.toml` → `./stm32burn.toml` → 可执行文件同目录 `stm32burn.toml`。

## 六、命令行

```
stm32burn [OPTIONS] [FIRMWARE] [PORT]

  <FIRMWARE>            .elf 或 .bin
  <PORT>                COM3

  -c, --config <FILE>   芯片配置
  -b, --baud <BAUD>     波特率，默认 115200
  -e, --erase <MODE>    page | mass | extended
      --verify          写完读回比对
      --base <ADDR>     仅 BIN 需要：烧写起始地址
      --no-enter        跳过「拨 BOOT0」提示
  -y, --yes             不做交互询问
      --dry-run         只解析并打印计划
  -v, --verbose         打印收发字节
  -l, --list-ports      列出串口
```

### 退出码

| 码 | 含义 |
|---|---|
| 0 | 成功 |
| 1 | 参数/文件/配置错误 |
| 2 | 串口打不开 |
| 3 | 握手失败或通信超时 |
| 4 | PID 不匹配 |
| 5 | 擦除失败 |
| 6 | 写入失败 |
| 7 | 校验失败 |

## 七、测试策略

没有硬件时的验证手段：

- `protocol.rs` 的纯函数单测：帧构造、校验、边界（256 字节块、255 页分帧）
- `tests/protocol_flow.rs`：脚本化 `Transport` 断言实际发出的字节序列，覆盖单块写、4 字节补齐、擦除、全擦、PID 解析、NACK、握手重试、完整流程
- `--dry-run`：验证 ELF/BIN 解析、地址范围、擦除页数

硬件到位后需要人工验证的：真实握手时序、擦除耗时、verify 一致性。

## 八、已知限制

- 只实现了标准擦除（0x43）。扩展擦除（0x44）的帧构造已就绪，但 F4/G0/L4 的扇区表尚未配置。
- 超过 256 页的芯片（1 MB 型号，512 页）无法按页擦除，只能用 `-e mass` 全片擦除；标准擦除的页号只有 1 字节。
- 不自动跳转执行（`GO` 命令已实现但未启用）：手动 BOOT0 场景下拨回复位更直观。
- 波特率固定可配，不做自适应提速。
- 不支持 Intel HEX。

## 九、后续可做

1. **自动下载电路**：DTR→BOOT0、RTS→NRST，实现一键进入 bootloader（需硬件配合）
2. **F4/G0/L4 扇区表**：扩展擦除的页表配置
3. **HEX 支持**：Intel HEX 解析
4. **读保护检测**：擦除失败时提示是否开了 RDP
5. **批量/产线模式**：配置文件驱动、日志落盘
