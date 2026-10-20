//! stm32burn 命令行入口。

use anyhow::{anyhow, Result};
use clap::{Parser, ValueEnum};
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use stm32burn::burn::{Burner, Stage, StageError};
use stm32burn::config::{Config, EraseMode};
use stm32burn::elf::{Firmware, Format};
use stm32burn::transport::{self, SerialTransport};

#[derive(Parser, Debug)]
#[command(
    name = "stm32burn",
    version,
    about = "通过 USART 烧录 STM32（AN3155 系统存储器 Bootloader）",
    after_help = "示例:\n  stm32burn app.elf COM3\n  stm32burn app.bin COM3 --base 0x08000000 --verify\n  stm32burn app.elf COM3 -c chips/stm32f103c8.toml -b 230400\n  stm32burn --list-ports"
)]
struct Cli {
    /// 固件文件（.elf 或 .bin）
    #[arg(value_name = "FIRMWARE")]
    firmware: Option<PathBuf>,

    /// 串口端口，例如 COM3
    #[arg(value_name = "PORT")]
    port: Option<String>,

    /// 芯片配置文件（TOML）
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// 波特率，覆盖配置文件（默认 115200）
    #[arg(short, long, value_name = "BAUD")]
    baud: Option<u32>,

    /// 擦除模式，覆盖配置文件
    #[arg(short, long, value_enum)]
    erase: Option<EraseArg>,

    /// 写入后读回校验
    #[arg(long)]
    verify: bool,

    /// BIN 文件的烧写起始地址，如 0x08000000
    #[arg(long, value_name = "ADDR", value_parser = parse_addr)]
    base: Option<u32>,

    /// 跳过「拨 BOOT0 + 复位」提示（已经在 bootloader 里时用）
    #[arg(long)]
    no_enter: bool,

    /// 不做任何交互询问
    #[arg(short, long)]
    yes: bool,

    /// 只解析固件并打印计划，不连接串口
    #[arg(long)]
    dry_run: bool,

    /// 打印每一步收发的字节
    #[arg(short, long)]
    verbose: bool,

    /// 列出可用串口后退出
    #[arg(short = 'l', long)]
    list_ports: bool,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum EraseArg {
    Page,
    Mass,
    Extended,
}

impl From<EraseArg> for EraseMode {
    fn from(v: EraseArg) -> Self {
        match v {
            EraseArg::Page => EraseMode::Page,
            EraseArg::Mass => EraseMode::Mass,
            EraseArg::Extended => EraseMode::Extended,
        }
    }
}

fn parse_addr(s: &str) -> std::result::Result<u32, String> {
    let t = s.trim();
    let r = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u32::from_str_radix(h, 16)
    } else {
        t.parse::<u32>()
    };
    r.map_err(|e| format!("无效地址「{s}」: {e}"))
}

/// 携带退出码的错误包装。
#[derive(Debug)]
struct ExitError {
    code: i32,
    err: anyhow::Error,
}

impl ExitError {
    fn new(code: i32, err: impl Into<anyhow::Error>) -> Self {
        Self {
            code,
            err: err.into(),
        }
    }
}

impl fmt::Display for ExitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.err)
    }
}

impl std::error::Error for ExitError {}

impl From<anyhow::Error> for ExitError {
    fn from(err: anyhow::Error) -> Self {
        Self { code: 1, err }
    }
}

fn main() {
    enable_utf8_console();
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("错误: {e}");
            e.code
        }
    };
    std::process::exit(code);
}

fn run() -> std::result::Result<(), ExitError> {
    let cli = Cli::parse();

    if cli.list_ports {
        let ports = transport::list_ports().map_err(|e| ExitError::new(2, e))?;
        if ports.is_empty() {
            println!("未发现可用串口");
        } else {
            for p in ports {
                println!("{p}");
            }
        }
        return Ok(());
    }

    let firmware_path = cli
        .firmware
        .ok_or_else(|| ExitError::new(1, anyhow!("缺少固件文件参数，用法: stm32burn <FIRMWARE> <PORT>")))?;
    let port = cli
        .port
        .ok_or_else(|| ExitError::new(1, anyhow!("缺少串口端口参数，例如 COM3")))?;

    let cfg_path =
        Config::locate(cli.config.as_deref(), &firmware_path).map_err(|e| ExitError::new(1, e))?;
    let mut cfg = Config::load(&cfg_path).map_err(|e| ExitError::new(1, e))?;
    if let Some(baud) = cli.baud {
        cfg.uart.baud = baud;
    }
    let erase_mode = cli.erase.map(EraseMode::from).unwrap_or(cfg.chip.erase_mode);

    let mut fw = Firmware::load(&firmware_path, cli.base).map_err(|e| ExitError::new(1, e))?;
    fw.retain_flash(cfg.chip.flash_base, cfg.chip.flash_size)
        .map_err(|e| ExitError::new(1, e))?;

    print_banner(&cfg_path, &cfg, &fw, &port);

    if cli.dry_run {
        print_plan(&cfg, &fw, erase_mode);
        return Ok(());
    }

    if !cli.no_enter && !cli.yes && io::stdin().is_terminal() {
        prompt("请将 BOOT0 拨到 1，按一下复位键，然后按回车继续... ")?;
    }

    let serial = SerialTransport::open(&port, &cfg.uart).map_err(|e| ExitError::new(2, e))?;
    let mut burner = Burner::new(serial, cli.verbose);

    let started = Instant::now();

    burner.sync(20).map_err(|e| ExitError::new(3, e))?;
    println!("[1/4] 握手        OK");

    let pid = burner.get_id().map_err(|e| ExitError::new(3, e))?;
    if !cfg.chip.pids.is_empty() && !cfg.chip.pids.contains(&(pid as u32)) {
        let expect = cfg
            .chip
            .pids
            .iter()
            .map(|p| format!("0x{p:04X}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(ExitError::new(
            4,
            anyhow!(
                "芯片不匹配：读到 PID 0x{pid:04X}，但 {} 配置期望 {expect}",
                cfg.chip.name
            ),
        ));
    }
    println!("[2/4] 识别芯片    OK  PID=0x{pid:04X}");

    let mut current: Option<Stage> = None;
    let mut last_pct = usize::MAX;
    let mut progress = |stage: Stage, done: usize, total: usize| {
        if current != Some(stage) {
            if current.is_some() {
                println!();
            }
            current = Some(stage);
            last_pct = usize::MAX;
        }
        let pct = (done * 100).checked_div(total).unwrap_or(100);
        if pct != last_pct {
            last_pct = pct;
            print!("\r      {:<7} {:>3}%", stage.label(), pct);
            let _ = io::stdout().flush();
        }
    };

    let result = burner.program(&cfg.chip, &fw, erase_mode, cli.verify, &mut progress);
    println!();

    result.map_err(|se: StageError| {
        let code = match se.stage {
            Stage::Erase => 5,
            Stage::Write => 6,
            Stage::Verify => 7,
        };
        ExitError::new(code, se.err)
    })?;

    let (start, end) = fw.span();
    let pages = cfg.chip.pages_covering(start, end).len();
    let verify_note = if cli.verify { "，校验通过" } else { "" };
    println!(
        "烧录完成：擦除 {pages} 页，写入 {}{verify_note}，耗时 {:.1} s",
        kib(fw.byte_len()),
        started.elapsed().as_secs_f64()
    );
    println!("请将 BOOT0 拨回 0，按一下复位键运行程序。");
    Ok(())
}

fn print_banner(cfg_path: &Path, cfg: &Config, fw: &Firmware, port: &str) {
    println!("stm32burn {}", env!("CARGO_PKG_VERSION"));
    println!(
        "config  : {}  ({}, {} Flash)",
        cfg_path.display(),
        cfg.chip.name,
        kib(cfg.chip.flash_size as usize)
    );
    let kind = match fw.format {
        Format::Elf => "ELF",
        Format::Bin => "BIN",
    };
    println!(
        "firmware: {kind}  {}  ->  0x{:08X}..0x{:08X}",
        kib(fw.byte_len()),
        fw.start(),
        fw.end()
    );
    println!(
        "port    : {port} @ {} {}{}{}",
        cfg.uart.baud, cfg.uart.data_bits, parity_char(cfg), cfg.uart.stop_bits
    );
    println!();
}

fn print_plan(cfg: &Config, fw: &Firmware, mode: EraseMode) {
    println!("dry-run：只解析，不连接串口");
    for (i, img) in fw.images.iter().enumerate() {
        println!(
            "  段 {i}: 0x{:08X}..0x{:08X}  {}",
            img.addr,
            img.end(),
            kib(img.data.len())
        );
    }
    let (start, end) = fw.span();
    let pages = cfg.chip.pages_covering(start, end);
    println!(
        "  擦除: {:?} 模式，覆盖 {} 页（0x{:08X}..0x{:08X}）",
        mode,
        pages.len(),
        start,
        end
    );
    println!("  入口: 0x{:08X}", fw.entry);
}

fn parity_char(cfg: &Config) -> &'static str {
    match cfg.uart.parity {
        stm32burn::config::Parity::None => "N",
        stm32burn::config::Parity::Even => "E",
        stm32burn::config::Parity::Odd => "O",
    }
}

fn kib(n: usize) -> String {
    if n >= 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn prompt(msg: &str) -> Result<()> {
    print!("{msg}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(())
}

/// Windows 控制台默认代码页是 GBK，中文输出会乱码，这里切到 UTF-8。
#[cfg(windows)]
fn enable_utf8_console() {
    extern "system" {
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleCP(code_page: u32) -> i32;
    }
    const CP_UTF8: u32 = 65001;
    unsafe {
        SetConsoleOutputCP(CP_UTF8);
        SetConsoleCP(CP_UTF8);
    }
}

#[cfg(not(windows))]
fn enable_utf8_console() {}
