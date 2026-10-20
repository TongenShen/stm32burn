# stm32burn Design Notes

**[中文](DESIGN_zh.md) | English**

A command-line tool that flashes STM32 microcontrollers over USART through the factory-programmed system memory bootloader (ST **AN3155** protocol), without ST-Link or any dedicated debug probe.

## 1. Scope

- **USART only**: one USB-TTL adapter (CH340 / CP2102 / FT232) wired to PA9/PA10 is all it takes
- **CLI only**: no GUI, suitable for scripts, batch runs and remote workflows
- **Config-driven**: chip parameters live in TOML, similar to OpenOCD target configs
- **First release target**: STM32F103 (C8T6 as the baseline), architected to extend to F4/G0/L4

Non-goals: debugging (breakpoints, registers), readout protection settings, Intel HEX, GUI.

## 2. Why not a custom protocol

Every STM32 ships with a bootloader in system memory. With `BOOT0=1` the chip boots from it after reset and speaks AN3155 over USART1 (PA9/PA10). We only implement the **host side**; the device side is guaranteed by ST. This is exactly what the UART mode of FlyMcu / mcuisp / STM32CubeProgrammer does.

## 3. Architecture

```
main.rs        CLI parsing, flow orchestration, progress output, exit codes
  └─ burn.rs   Burner: turns protocol operations into erase / write / verify stages
       ├─ protocol.rs   frame construction and response parsing (pure functions, no IO)
       └─ transport.rs  Transport trait + SerialTransport
config.rs      chip config parsing, validation, page-number arithmetic
elf.rs         ELF32 / BIN parsing -> list of flash images
```

The key decision: **the protocol layer never touches the serial port**; it depends only on the `Transport` trait. That makes it possible to run the full flow against a scripted transport and catch byte-order bugs without hardware.

## 4. Protocol notes (including the traps we hit)

### Frame formats

| Frame | Byte sequence |
|---|---|
| Command | `cmd, !cmd` |
| Address | `addr[4] big-endian, addr[0]^addr[1]^addr[2]^addr[3]` |
| Write data | `N-1, data..., (N-1) ^ data[0] ^ ... ^ data[N-1]` |
| Read length | `N-1, !(N-1)` |
| Page erase | `page_count-1, page numbers... (1 byte each), checksum` |
| Mass erase | `0xFF, 0x00` |

Responses: `ACK = 0x79`, `NACK = 0x1F`.

### Six details worth memorizing

1. **The port is 8E1** (even parity), not the usual 8N1. Getting this wrong causes NACK on every command.
2. **Erase page numbers are 1 byte**, not 2. Two-byte page numbers belong to extended erase (0x44), used only on F4/G0/L4.
3. **The mass erase frame is only two bytes**: `0xFF 0x00`, not `0xFF 0xFF ...`.
4. **Write data must be padded to a multiple of 4**, using `0xFF` (the erased value).
5. **The read-length checksum is a complement** (`!N`), while the write-data checksum is XOR — they are not the same.
6. **ELF images must use `p_paddr` (LMA)**, not `p_vaddr`. For `.data`, the VMA is in RAM while the LMA is in flash; using the wrong one writes initial values into memory.

### Flow

```
0x7F sync (retryable) -> GET_ID model check -> erase -> chunked write -> optional read-back verify
```

Flush the receive buffer before the handshake: serial ports often carry stale bytes right after opening, which makes the first `0x7F` fail.

## 5. Configuration file

```toml
[chip]
name       = "STM32F103C8"
pids       = [0x0410]        # value returned by GET_ID; refuse to flash if not listed
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

`pids` is the guard against flashing the wrong part: the tool proceeds only if `GET_ID` returns a listed value, so F407 firmware cannot be flashed into an F103.

Lookup order: `-c` flag -> a `.toml` next to the firmware with the same name -> `./stm32burn.toml` -> `stm32burn.toml` next to the executable.

## 6. Command line

```
stm32burn [OPTIONS] [FIRMWARE] [PORT]

  <FIRMWARE>            .elf or .bin
  <PORT>                COM3

  -c, --config <FILE>   chip configuration
  -b, --baud <BAUD>     baud rate, 115200 by default
  -e, --erase <MODE>    page | mass | extended
      --verify          read back and compare after writing
      --base <ADDR>     BIN only: start address to flash at
      --no-enter        skip the "set BOOT0" prompt
  -y, --yes             do not ask for confirmation
      --dry-run         parse and print the plan only
  -v, --verbose         dump bytes sent and received
  -l, --list-ports      list serial ports
      --chelp           show help in Chinese (--help is English)
```

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Argument / file / config error |
| 2 | Cannot open the serial port |
| 3 | Handshake failed or communication timeout |
| 4 | PID mismatch |
| 5 | Erase failed |
| 6 | Write failed |
| 7 | Verify failed |

## 7. Testing strategy

How we validate without hardware:

- Unit tests on the pure functions in `protocol.rs`: frame construction, checksums, boundaries (256-byte chunks, 255-page batches)
- `tests/protocol_flow.rs`: a scripted `Transport` asserts the exact bytes emitted, covering single-chunk writes, 4-byte padding, page erase, mass erase, PID parsing, NACK handling, handshake retry and the full flow
- `--dry-run`: validates ELF/BIN parsing, address ranges and erase page counts

Still needing manual verification once hardware is available: real handshake timing, erase duration, verify consistency.

## 8. Known limitations

- Only standard erase (0x43) is implemented. Frame construction for extended erase (0x44) is ready, but sector tables for F4/G0/L4 are not configured.
- Chips with more than 256 pages (1 MB parts, 512 pages) cannot be erased page by page; they need `-e mass`. Standard erase reserves only one byte for page numbers.
- No automatic jump to the application (`GO` is implemented but not enabled): with manual BOOT0 control, telling the user to switch back and reset is clearer.
- Baud rate is configurable but there is no adaptive speed-up.
- No Intel HEX support.

## 9. Possible next steps

1. **Auto-download circuit**: DTR -> BOOT0, RTS -> NRST for one-key entry into the bootloader (needs hardware support)
2. **F4/G0/L4 sector tables**: page tables for extended erase
3. **Intel HEX support**: HEX parsing
4. **Readout protection detection**: hint at RDP when erase fails
5. **Batch / production mode**: config-driven runs, logging to disk
