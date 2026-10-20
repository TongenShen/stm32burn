//! 传输层：把「串口读写」抽象成 trait，便于替换成模拟实现做测试。

use anyhow::{bail, Context, Result};
use serialport::{DataBits, Parity as SpParity, StopBits};
use std::io::{self, Read, Write};
use std::time::Duration;

use crate::config::{Parity, Uart};

pub trait Transport {
    fn write_all(&mut self, data: &[u8]) -> Result<()>;
    /// 读取一个字节；超时返回错误。
    fn read_byte(&mut self, timeout: Duration) -> Result<u8>;
    /// 清空接收缓冲，避免残留数据干扰握手。
    fn flush_input(&mut self) -> Result<()>;
}

pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    pub fn open(device: &str, uart: &Uart) -> Result<Self> {
        let parity = match uart.parity {
            Parity::None => SpParity::None,
            Parity::Even => SpParity::Even,
            Parity::Odd => SpParity::Odd,
        };
        let stop = if uart.stop_bits == 2 {
            StopBits::Two
        } else {
            StopBits::One
        };
        let port = serialport::new(device, uart.baud)
            .data_bits(DataBits::Eight)
            .parity(parity)
            .stop_bits(stop)
            .timeout(Duration::from_millis(1000))
            .open()
            .with_context(|| format!("无法打开串口 {device}（检查端口名与是否被占用）"))?;
        Ok(Self { port })
    }
}

impl Transport for SerialTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.port.write_all(data).context("串口写入失败")?;
        self.port.flush().context("串口刷新失败")?;
        Ok(())
    }

    fn read_byte(&mut self, timeout: Duration) -> Result<u8> {
        self.port
            .set_timeout(timeout)
            .context("设置串口超时失败")?;
        let mut buf = [0u8; 1];
        loop {
            match self.port.read(&mut buf) {
                Ok(0) => continue,
                Ok(_) => return Ok(buf[0]),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    bail!("等待响应超时（{} ms）", timeout.as_millis())
                }
                Err(e) => return Err(e).context("串口读取失败"),
            }
        }
    }

    fn flush_input(&mut self) -> Result<()> {
        self.port
            .clear(serialport::ClearBuffer::Input)
            .context("清空串口缓冲失败")?;
        Ok(())
    }
}

/// 列出可用串口，按端口名排序。
pub fn list_ports() -> Result<Vec<String>> {
    let mut names: Vec<String> = serialport::available_ports()
        .context("枚举串口失败")?
        .into_iter()
        .map(|p| p.port_name)
        .collect();
    names.sort();
    Ok(names)
}
