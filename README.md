# stm32burn

**[中文](README_zh.md) | English**

Flash STM32 microcontrollers over USART using the chip's built-in system memory bootloader (ST AN3155 protocol). No ST-Link required — a single USB-TTL adapter is enough.

## Quick start

```bash
# 1. Build
cargo build --release

# 2. List serial ports
./target/release/stm32burn --list-ports

# 3. Dry run first: verify firmware parsing and address layout
./target/release/stm32burn app.elf COM3 -c chips/stm32f103c8.toml --dry-run

# 4. Flash
./target/release/stm32burn app.elf COM3 -c chips/stm32f103c8.toml
```

Wiring (F103):

```
USB-TTL TX  →  PA10 (USART1_RX)
USB-TTL RX  →  PA9  (USART1_TX)
USB-TTL GND →  GND
```

Flashing steps:

1. Set **BOOT0 to 1** and press reset
2. Run the command; erase and write begin once the handshake succeeds
3. Set **BOOT0 back to 0** and press reset to run the application

## Common options

```bash
stm32burn app.elf COM3                      # 115200 8E1 by default
stm32burn app.elf COM3 -b 230400            # change baud rate
stm32burn app.elf COM3 --verify             # read back and verify
stm32burn app.elf COM3 -e mass              # mass erase
stm32burn app.bin COM3 --base 0x08000000    # BIN requires a start address
stm32burn app.elf COM3 --dry-run            # parse only, do not flash
stm32burn --list-ports                      # list serial ports
stm32burn --chelp                           # help in Chinese (--help is English)
```

## Configuration

Chip parameters live in TOML, for example `chips/stm32f103c8.toml`:

```toml
[chip]
name       = "STM32F103C8"
pids       = [0x0410]        # GET_ID check, prevents flashing the wrong chip
flash_base = 0x08000000
flash_size = 0x10000
page_size  = 0x400
erase_mode = "page"

[uart]
baud   = 115200
parity = "even"
```

When `-c` is omitted, the tool looks for a `.toml` next to the firmware with the same name, then `./stm32burn.toml`, then `stm32burn.toml` next to the executable.

## Exit codes

`0` success · `1` bad arguments · `2` cannot open port · `3` handshake failed · `4` PID mismatch · `5` erase failed · `6` write failed · `7` verify failed

## Supported

- **Chips**: STM32F103 family, shipping with three configs: C8T6 (64 KB / 1 KB pages), RCT6 (256 KB / 2 KB pages), VET6 (512 KB / 2 KB pages). Other parts only need a `chips/*.toml`; no code changes are required for devices with the same flash layout.
- **Firmware**: `.elf` (ARM, uses the LMA address) and `.bin` (requires `--base`).
- **Erase**: per page or mass.

Not supported yet: Intel HEX, extended sector erase for F4/G0/L4, page erase on chips with more than 256 pages (1 MB parts need `-e mass`), automatic BOOT0 control (requires a DTR/RTS circuit).

Design notes and protocol details: [DESIGN.md](DESIGN.md).

## Adding a new chip

The repo ships an **AI-assistant workflow** for chip adaptation. After cloning, just tell your AI "add support for STM32F103RCT6" and it will find and read the skill:

| Tool | Path |
|---|---|
| Claude Code | `.claude/skills/add-stm32-chip/SKILL.md` |
| DSH | `.dsh/skills/add-stm32-chip/SKILL.md` |
| Others | [`AGENTS.md`](AGENTS.md) at the repo root points to both |

The skill contains a lookup table for the whole F103 family, the page-size tiering rules, the 256-page boundary handling, and a list of common pitfalls. The AI derives flash size, page size and PID, generates `chips/*.toml`, and validates it with `--dry-run`.

To do it by hand, copy an existing config and change four fields: `pids`, `flash_size`, `page_size`, `erase_mode`. **Page size depends on the density tier** — high-density parts (256/512 KB) use 2 KB pages, the rest of the F103 family uses 1 KB. A wrong value shifts every page number during erase.

## Desktop GUI

A Tauri-based GUI lives in [`gui/`](gui/). It calls the same library directly — no CLI text parsing, progress arrives as structured events.

```bash
cd gui
npm install
npm run tauri dev      # development with hot reload
npm run tauri build    # release installer (NSIS)
```

The GUI looks for `chips/*.toml` in the resource directory (installed build), next to the executable (portable build), or in `chips/` relative to the working directory (development).

> If the window fails to start with `0x8000FFFF`, point `WEBVIEW2_USER_DATA_FOLDER` at a writable directory. Some endpoint-protection software blocks WebView2 from creating its profile under AppData.

## Development

```bash
cargo test        # 29 tests, no hardware needed
cargo build --release
```

The protocol layer is pure functions, so a scripted transport can validate frame formats without a board.

> If `cargo build` stalls while downloading dependencies, the environment probably has a local proxy configured but not running. Create `.cargo/config.toml` in the project with `[http] proxy = ""` to bypass it (that file is gitignored).
