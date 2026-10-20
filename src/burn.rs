//! 烧录流程编排：把 [`protocol`] 的帧通过 [`Transport`] 发送并处理应答。

use anyhow::{bail, Context, Result};
use std::fmt;
use std::time::Duration;

use crate::config::{ChipConfig, EraseMode};
use crate::elf::Firmware;
use crate::protocol::{self, Response};
use crate::transport::Transport;

/// 普通命令的应答超时
pub const ACK_TIMEOUT: Duration = Duration::from_millis(1000);
/// 擦除可能要十几秒，给足时间
pub const ERASE_TIMEOUT: Duration = Duration::from_secs(30);
/// 握手时每次等待的时间
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Erase,
    Write,
    Verify,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Erase => "erase",
            Stage::Write => "write",
            Stage::Verify => "verify",
        }
    }
}

/// 带阶段信息的错误，便于调用方按阶段映射退出码。
#[derive(Debug)]
pub struct StageError {
    pub stage: Stage,
    pub err: anyhow::Error,
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.err)
    }
}

impl std::error::Error for StageError {}

pub struct Burner<T: Transport> {
    transport: T,
    verbose: bool,
}

impl<T: Transport> Burner<T> {
    pub fn new(transport: T, verbose: bool) -> Self {
        Self { transport, verbose }
    }

    /// 取出底层传输层，便于测试断言已发送的字节。
    pub fn into_inner(self) -> T {
        self.transport
    }

    fn send(&mut self, data: &[u8]) -> Result<()> {
        if self.verbose {
            eprintln!("  -> {}", hex(data));
        }
        self.transport.write_all(data)
    }

    fn wait_ack(&mut self, timeout: Duration, what: &str) -> Result<()> {
        let b = self
            .transport
            .read_byte(timeout)
            .with_context(|| format!("{what}：等待应答失败"))?;
        if self.verbose {
            eprintln!("  <- 0x{b:02X}");
        }
        match Response::from_byte(b) {
            Some(Response::Ack) => Ok(()),
            Some(Response::Nack) => bail!("{what}：bootloader 返回 NACK"),
            None => bail!("{what}：收到意外字节 0x{b:02X}"),
        }
    }

    /// 反复发送 0x7F 直到得到 ACK。
    ///
    /// 发送前清空接收缓冲：串口刚打开时可能残留旧数据，这是首个同步失败的常见原因。
    pub fn sync(&mut self, attempts: usize) -> Result<()> {
        for attempt in 1..=attempts {
            let _ = self.transport.flush_input();
            self.transport.write_all(&[protocol::SYNC])?;
            match self.transport.read_byte(HANDSHAKE_TIMEOUT) {
                Ok(b) if b == protocol::ACK => {
                    if self.verbose {
                        eprintln!("  sync ok on attempt {attempt}");
                    }
                    return Ok(());
                }
                Ok(b) => {
                    if self.verbose {
                        eprintln!("  sync attempt {attempt}: unexpected 0x{b:02X}");
                    }
                }
                Err(_) => {
                    if self.verbose {
                        eprintln!("  sync attempt {attempt}: no response");
                    }
                }
            }
        }
        bail!(
            "bootloader 无响应。请确认：BOOT0=1、已按复位键、串口 TX/RX 交叉连接、端口选择正确"
        )
    }

    /// 读取芯片 PID。
    pub fn get_id(&mut self) -> Result<u16> {
        self.send(&protocol::frame_cmd(protocol::CMD_GET_ID))?;
        self.wait_ack(ACK_TIMEOUT, "GET_ID 命令")?;

        let n = self.transport.read_byte(ACK_TIMEOUT)? as usize;
        if n > 15 {
            bail!("GET_ID 返回的长度异常：{n}");
        }
        let mut pid = Vec::with_capacity(n + 1);
        for _ in 0..=n {
            pid.push(self.transport.read_byte(ACK_TIMEOUT)?);
        }
        self.wait_ack(ACK_TIMEOUT, "GET_ID 数据")?;

        if pid.len() < 2 {
            bail!("GET_ID 返回的 PID 太短");
        }
        Ok(u16::from_be_bytes([pid[0], pid[1]]))
    }

    /// 标准页擦除（0x43），页号为 1 字节。
    pub fn erase_pages(&mut self, pages: &[u8]) -> Result<()> {
        self.send(&protocol::frame_cmd(protocol::CMD_ERASE))?;
        self.wait_ack(ACK_TIMEOUT, "ERASE 命令")?;
        self.send(&protocol::frame_erase_pages(pages))?;
        self.wait_ack(ERASE_TIMEOUT, "擦除")
    }

    /// 标准全片擦除（0x43 + 0xFF 0x00）。
    pub fn erase_mass(&mut self) -> Result<()> {
        self.send(&protocol::frame_cmd(protocol::CMD_ERASE))?;
        self.wait_ack(ACK_TIMEOUT, "ERASE 命令")?;
        self.send(&protocol::frame_erase_mass())?;
        self.wait_ack(ERASE_TIMEOUT, "全片擦除")
    }

    /// 扩展页擦除（0x44），页号为 2 字节，供 F4/G0/L4 使用。
    pub fn erase_ext_pages(&mut self, pages: &[u16]) -> Result<()> {
        self.send(&protocol::frame_cmd(protocol::CMD_EXT_ERASE))?;
        self.wait_ack(ACK_TIMEOUT, "EXT_ERASE 命令")?;
        self.send(&protocol::frame_ext_erase_pages(pages))?;
        self.wait_ack(ERASE_TIMEOUT, "扩展擦除")
    }

    /// 写一块数据（<=256 字节）。不足 4 字节倍数时补 0xFF，这是 flash 擦除后的值。
    pub fn write_chunk(&mut self, addr: u32, data: &[u8]) -> Result<()> {
        assert!(!data.is_empty());
        assert!(data.len() <= protocol::MAX_CHUNK);

        let mut payload = data.to_vec();
        while !payload.len().is_multiple_of(4) {
            payload.push(0xFF);
        }

        self.send(&protocol::frame_cmd(protocol::CMD_WRITE))?;
        self.wait_ack(ACK_TIMEOUT, "WRITE 命令")?;
        self.send(&protocol::frame_addr(addr))?;
        self.wait_ack(ACK_TIMEOUT, "WRITE 地址")?;
        self.send(&protocol::frame_write_data(&payload))?;
        self.wait_ack(ACK_TIMEOUT, "WRITE 数据")
    }

    /// 读一块内存（<=256 字节）。
    pub fn read_chunk(&mut self, addr: u32, len: usize) -> Result<Vec<u8>> {
        assert!((1..=protocol::MAX_CHUNK).contains(&len));

        self.send(&protocol::frame_cmd(protocol::CMD_READ))?;
        self.wait_ack(ACK_TIMEOUT, "READ 命令")?;
        self.send(&protocol::frame_addr(addr))?;
        self.wait_ack(ACK_TIMEOUT, "READ 地址")?;
        self.send(&protocol::frame_read_len(len))?;
        self.wait_ack(ACK_TIMEOUT, "READ 长度")?;

        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(self.transport.read_byte(ACK_TIMEOUT)?);
        }
        self.wait_ack(ACK_TIMEOUT, "READ 数据")?;
        Ok(out)
    }

    /// 跳转到指定地址执行。
    pub fn go(&mut self, addr: u32) -> Result<()> {
        self.send(&protocol::frame_cmd(protocol::CMD_GO))?;
        self.wait_ack(ACK_TIMEOUT, "GO 命令")?;
        self.send(&protocol::frame_addr(addr))?;
        self.wait_ack(ACK_TIMEOUT, "GO 地址")
    }

    /// 完整烧录流程：擦除 → 写入 →（可选）校验。
    pub fn program(
        &mut self,
        chip: &ChipConfig,
        fw: &Firmware,
        erase: EraseMode,
        verify: bool,
        progress: &mut dyn FnMut(Stage, usize, usize),
    ) -> Result<(), StageError> {
        self.erase(chip, fw, erase, progress)
            .map_err(|err| StageError {
                stage: Stage::Erase,
                err,
            })?;
        self.write(fw, progress).map_err(|err| StageError {
            stage: Stage::Write,
            err,
        })?;
        if verify {
            self.verify(fw, progress).map_err(|err| StageError {
                stage: Stage::Verify,
                err,
            })?;
        }
        Ok(())
    }

    fn erase(
        &mut self,
        chip: &ChipConfig,
        fw: &Firmware,
        mode: EraseMode,
        progress: &mut dyn FnMut(Stage, usize, usize),
    ) -> Result<()> {
        match mode {
            EraseMode::Mass => {
                self.erase_mass()?;
                progress(Stage::Erase, 1, 1);
            }
            EraseMode::Page => {
                let (start, end) = fw.span();
                let pages = chip.pages_covering(start, end);
                let total = pages.len();
                let mut done = 0usize;
                for chunk in pages.chunks(protocol::MAX_PAGES_PER_ERASE) {
                    let bytes = page_bytes(chunk)?;
                    self.erase_pages(&bytes)?;
                    done += chunk.len();
                    progress(Stage::Erase, done, total);
                }
            }
            EraseMode::Extended => {
                let (start, end) = fw.span();
                let pages: Vec<u16> = chip
                    .pages_covering(start, end)
                    .iter()
                    .map(|p| *p as u16)
                    .collect();
                let total = pages.len();
                let mut done = 0usize;
                // 扩展擦除的页数字段是 2 字节，一帧最多 65536 页，实际不会触及
                for chunk in pages.chunks(1024) {
                    self.erase_ext_pages(chunk)?;
                    done += chunk.len();
                    progress(Stage::Erase, done, total);
                }
            }
        }
        Ok(())
    }

    fn write(&mut self, fw: &Firmware, progress: &mut dyn FnMut(Stage, usize, usize)) -> Result<()> {
        let total = fw.byte_len();
        let mut done = 0usize;
        for img in &fw.images {
            let mut off = 0usize;
            while off < img.data.len() {
                let n = (img.data.len() - off).min(protocol::MAX_CHUNK);
                let addr = img.addr + off as u32;
                self.write_chunk(addr, &img.data[off..off + n])
                    .with_context(|| format!("写入失败：地址 0x{addr:08X}"))?;
                off += n;
                done += n;
                progress(Stage::Write, done, total);
            }
        }
        Ok(())
    }

    fn verify(&mut self, fw: &Firmware, progress: &mut dyn FnMut(Stage, usize, usize)) -> Result<()> {
        let total = fw.byte_len();
        let mut done = 0usize;
        for img in &fw.images {
            let mut off = 0usize;
            while off < img.data.len() {
                let n = (img.data.len() - off).min(protocol::MAX_CHUNK);
                let addr = img.addr + off as u32;
                let got = self
                    .read_chunk(addr, n)
                    .with_context(|| format!("校验读取失败：地址 0x{addr:08X}"))?;
                let expect = &img.data[off..off + n];
                if let Some(pos) = got.iter().zip(expect.iter()).position(|(a, b)| a != b) {
                    let bad_addr = addr + pos as u32;
                    bail!(
                        "校验不一致：地址 0x{bad_addr:08X} 读到 0x{:02X}，期望 0x{:02X}",
                        got[pos],
                        expect[pos]
                    );
                }
                off += n;
                done += n;
                progress(Stage::Verify, done, total);
            }
        }
        Ok(())
    }
}

/// 把页号列表转成标准擦除用的 1 字节页号。
fn page_bytes(pages: &[u32]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(pages.len());
    for &p in pages {
        if p > 255 {
            bail!(
                "页号 {p} 超出标准擦除的 1 字节范围；该芯片请使用 extended 擦除模式"
            );
        }
        out.push(p as u8);
    }
    Ok(out)
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
