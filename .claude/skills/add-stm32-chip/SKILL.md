---
name: add-stm32-chip
description: Use when the user names an STM32 chip model and wants stm32burn to support it (for example "适配 STM32F103RCT6"、"给 VET6 加个配置"、"我的芯片是 XXX"), or when an existing chips/*.toml fails the PID check or erases the wrong pages. Generates or fixes chips/<model>.toml with the correct PID, flash size, page size and erase mode, then verifies the result with --dry-run.
---

# 为 stm32burn 适配新芯片

用户报一个型号、要一份能用的 `chips/*.toml` 时用这份流程。目标是**一次给对**，不要让用户烧上去才发现页大小错了。

## 配置文件由四项决定

其余字段都是推导出来的，不要凭空猜：

| 字段 | 来源 | 备注 |
|---|---|---|
| `flash_size` | 型号里的容量字母 | 见下表 |
| `page_size` | **按容量分档** | 最容易错的一项 |
| `pids` | `GET_ID` 返回值 | 防烧错型号 |
| `erase_mode` | 芯片系列 | F1 系列一律 `page` |

## 从型号名读参数

`STM32F103` + **引脚** + **容量** + **封装** + **温度**

- 引脚：T=36, C=48, R=64, V=100, Z=144, I=176
- 容量：4=16K, 6=32K, 8=64K, B=128K, C=256K, D=384K, E=512K, F=768K, G=1M
- 封装：T=LQFP, H=BGA, U=QFN
- 温度：6=-40~85℃, 7=-40~105℃

## F103 速查表

| 型号 | Flash | RAM | PID | 页大小 | 页数 |
|---|---|---|---|---|---|
| C4T6 / C6T6 | 16 / 32 KB | 6 / 10 KB | 0x412 | 1 KB | 16 / 32 |
| C8T6 / C8T6A / R8T6 | 64 KB | 20 KB | 0x410 | 1 KB | 64 |
| CBT6 / RBT6 | 128 KB | 20 KB | 0x410 | 1 KB | 128 |
| RCT6 / VCT6 / ZCT6 | 256 KB | 48 KB | 0x414 | 2 KB | 128 |
| RET6 / VET6 / ZET6 | 512 KB | 64 KB | 0x414 | 2 KB | 256 |
| VGT6 / ZGT6 | 1 MB | 96 KB | 0x430 | 2 KB | 512 |

PID 分档规律：

- **小容量**（16~32 KB）→ `0x412`
- **中容量**（64~128 KB）→ `0x410`
- **高密度**（256~512 KB）→ `0x414`
- **互联型** → `0x418`
- **XL 容量**（768 KB~1 MB）→ `0x430`

## 页大小分界线（关键）

**高密度（256/512 KB）和 XL 的页大小是 2 KB，其余 F103 是 1 KB。**

填错不会报错，但擦除页号会全部错位——表现为「想擦一页，实际擦了两页」，或者擦除后写入校验失败。这是这套配置里唯一一个「错了也能跑一半」的地方，所以必须按容量分档确认。

## 生成步骤

1. 按型号推出四项参数
2. 写 `chips/<型号小写>.toml`，例如 `stm32f103vet6.toml`
3. 跑 dry-run 验证（见下）
4. 告诉用户真机验证要点：BOOT0=1 → 复位 → 烧录 → BOOT0=0 → 复位

## 模板

```toml
# <型号>（<容量> Flash，<密度描述>）

[chip]
name       = "<型号>"
pids       = [<PID>]
flash_base = 0x08000000
flash_size = <容量，十六进制>
page_size  = <1KB 或 2KB 的十六进制>
erase_mode = "page"
ram_base   = 0x20000000

[uart]
baud      = 115200
parity    = "even"
data_bits = 8
stop_bits = 1
```

## 必须做的验证

```bash
cargo build --release

# Windows
./target/release/stm32burn.exe <任意固件> COM1 -c chips/<model>.toml --dry-run
# Linux / macOS
./target/release/stm32burn <任意固件> /dev/ttyUSB0 -c chips/<model>.toml --dry-run
```

检查三处输出：

1. `(<型号>, XX KiB Flash)` —— 容量与型号一致
2. `擦除: Page 模式，覆盖 N 页` —— N 与「固件大小 ÷ 页大小」吻合
3. `段 0: 0x08000000..` —— 起始地址正确

如果手上没有该型号的固件，用任意非空文件配合 `--base 0x08000000` 也能验证布局计算。

## 边界与陷阱

- **256 页上限**：标准擦除的页号只有 1 字节。VET6 正好 256 页，代码会自动拆成 `255 + 1` 两批，因为「页数-1 = 0xFF」是全片擦除的特殊标记，一帧塞 256 页会误抹整片。
- **超过 256 页**（1 MB 型号，512 页）：标准擦除无法按页寻址，这类芯片只能用 `-e mass` 全片擦除。**不要**写进配置就假设它能按页擦。
- **PID 变体**：国产兼容片（GD32 / APM32 / CH32）的 PID 不同，`GET_ID` 校验会失败并返回退出码 4。处理方式是询问用户实测值后加进 `pids`，或临时 `pids = []` 跳过校验。
- **接线固定 USART1**：PA9 / PA10，与型号、封装无关。100 脚以上的片子虽然还有 USART2/3，但 bootloader 只认 USART1。
- **擦除 NACK 不一定是配置错**：开了读保护（RDP）时擦除必然失败，先提示用户用工具或 ST-Link 解除。

## 不要做的事

- 不要给 F1 写 `erase_mode = "extended"`（`0x44` 是 F4/G0/L4 的命令）
- 不要照抄别的型号的 `page_size`，必须按容量分档重新确认
- 不要跳过 dry-run 直接让用户上板子
- 不要修改 `[uart]` 的 `parity = "even"` 和 `data_bits = 8`，这是 bootloader 的硬性要求
