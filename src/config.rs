//! 芯片配置文件的解析、校验与地址/页号换算。

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// 顶层配置。
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub chip: ChipConfig,
    #[serde(default)]
    pub uart: Uart,
}

/// 芯片描述：Flash 布局与擦除方式。
#[derive(Debug, Clone, Deserialize)]
pub struct ChipConfig {
    /// 人类可读的型号名，例如 "STM32F103C8"
    pub name: String,
    /// 该配置接受的 PID 列表；`GET_ID` 读回的值不在列表内则拒绝烧录。
    /// 留空表示不做校验。
    #[serde(default)]
    pub pids: Vec<u32>,
    pub flash_base: u32,
    pub flash_size: u32,
    pub page_size: u32,
    #[serde(default)]
    pub erase_mode: EraseMode,
    #[serde(default)]
    pub ram_base: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EraseMode {
    /// 标准擦除（0x43），页号 1 字节 —— F1 / F0 等
    #[default]
    Page,
    /// 标准擦除但一次性抹掉整片
    Mass,
    /// 扩展擦除（0x44），页号 2 字节 —— F4 / G0 / L4 等
    Extended,
}

/// 串口参数。默认 8E1 / 115200，这是 STM32 出厂 bootloader 的既定格式。
#[derive(Debug, Clone, Deserialize)]
pub struct Uart {
    pub baud: u32,
    pub parity: Parity,
    pub data_bits: u8,
    pub stop_bits: u8,
}

impl Default for Uart {
    fn default() -> Self {
        Self {
            baud: 115200,
            parity: Parity::Even,
            data_bits: 8,
            stop_bits: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Parity {
    None,
    #[default]
    Even,
    Odd,
}

impl Config {
    /// 从文件读取并校验。
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件 {}", path.display()))?;
        let cfg: Config = toml::from_str(&text)
            .with_context(|| format!("配置文件格式错误 {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let c = &self.chip;
        if c.name.trim().is_empty() {
            bail!("配置缺少 chip.name");
        }
        if c.page_size == 0 || !c.page_size.is_power_of_two() {
            bail!("chip.page_size 必须是 2 的幂，当前为 {}", c.page_size);
        }
        if c.flash_size == 0 {
            bail!("chip.flash_size 不能为 0");
        }
        if !c.flash_size.is_multiple_of(c.page_size) {
            bail!(
                "chip.flash_size ({}) 必须是 chip.page_size ({}) 的整数倍",
                c.flash_size,
                c.page_size
            );
        }
        if !c.flash_base.is_multiple_of(4) {
            bail!("chip.flash_base 必须 4 字节对齐");
        }
        if self.uart.data_bits != 8 {
            bail!("STM32 bootloader 只接受 8 数据位，当前配置 {}", self.uart.data_bits);
        }
        if !matches!(self.uart.stop_bits, 1 | 2) {
            bail!("串口停止位只能是 1 或 2");
        }
        Ok(())
    }

    /// 定位配置文件。显式给定时必须存在；否则按约定顺序探测。
    ///
    /// 探测顺序：
    /// 1. 固件同目录同名 `.toml`
    /// 2. 当前目录 `stm32burn.toml`
    /// 3. 可执行文件同目录 `stm32burn.toml`
    pub fn locate(explicit: Option<&Path>, firmware: &Path) -> Result<PathBuf> {
        if let Some(p) = explicit {
            if !p.is_file() {
                bail!("配置文件不存在: {}", p.display());
            }
            return Ok(p.to_path_buf());
        }

        let mut tried: Vec<PathBuf> = Vec::new();
        let mut candidates: Vec<PathBuf> = Vec::new();

        candidates.push(firmware.with_extension("toml"));
        candidates.push(PathBuf::from("stm32burn.toml"));
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("stm32burn.toml"));
            }
        }

        for c in candidates {
            if c.is_file() {
                return Ok(c);
            }
            tried.push(c);
        }

        let list = tried
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("找不到芯片配置文件，已尝试：\n{list}\n请用 -c <FILE> 指定");
    }
}

impl ChipConfig {
    pub fn end(&self) -> u32 {
        self.flash_base.saturating_add(self.flash_size)
    }

    pub fn page_count(&self) -> u32 {
        self.flash_size / self.page_size
    }

    pub fn contains(&self, addr: u32) -> bool {
        addr >= self.flash_base && addr < self.end()
    }

    /// 地址所在页号；越界返回 None。
    pub fn page_of(&self, addr: u32) -> Option<u32> {
        if !self.contains(addr) {
            return None;
        }
        Some((addr - self.flash_base) / self.page_size)
    }

    /// 校验 `[addr, addr+len)` 完全落在 Flash 范围内。
    pub fn ensure_in_flash(&self, addr: u32, len: usize) -> Result<()> {
        let end = addr as u64 + len as u64;
        if addr < self.flash_base || end > self.end() as u64 {
            bail!(
                "地址范围 0x{addr:08X}..0x{end:08X} 超出 {} 的 Flash 范围 0x{:08X}..0x{:08X}",
                self.name,
                self.flash_base,
                self.end()
            );
        }
        Ok(())
    }

    /// 覆盖 `[start, end)` 的所有页号（含两端所在页）。
    pub fn pages_covering(&self, start: u32, end: u32) -> Vec<u32> {
        let first = self.page_of(start).unwrap_or(0);
        let last = if end <= start {
            first
        } else {
            self.page_of(end - 1)
                .unwrap_or_else(|| self.page_count().saturating_sub(1))
        };
        (first..=last).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip() -> ChipConfig {
        ChipConfig {
            name: "STM32F103C8".into(),
            pids: vec![0x410],
            flash_base: 0x0800_0000,
            flash_size: 0x1_0000,
            page_size: 0x400,
            erase_mode: EraseMode::Page,
            ram_base: Some(0x2000_0000),
        }
    }

    #[test]
    fn page_count_and_contains() {
        let c = chip();
        assert_eq!(c.page_count(), 64);
        assert!(c.contains(0x0800_0000));
        assert!(c.contains(0x0800_FFFF));
        assert!(!c.contains(0x0801_0000));
        assert!(!c.contains(0x07FF_FFFF));
    }

    #[test]
    fn page_of_boundaries() {
        let c = chip();
        assert_eq!(c.page_of(0x0800_0000), Some(0));
        assert_eq!(c.page_of(0x0800_03FF), Some(0));
        assert_eq!(c.page_of(0x0800_0400), Some(1));
        assert_eq!(c.page_of(0x0800_FF00), Some(63));
        assert_eq!(c.page_of(0x0801_0000), None);
    }

    #[test]
    fn pages_covering_range() {
        let c = chip();
        assert_eq!(c.pages_covering(0x0800_0000, 0x0800_0400), vec![0]);
        assert_eq!(c.pages_covering(0x0800_0000, 0x0800_0800), vec![0, 1]);
        assert_eq!(c.pages_covering(0x0800_0200, 0x0800_0600), vec![0, 1]);
        // 正好覆盖第 2 页
        assert_eq!(c.pages_covering(0x0800_0800, 0x0800_0C00), vec![2]);
    }

    #[test]
    fn ensure_in_flash_rejects_overflow() {
        let c = chip();
        assert!(c.ensure_in_flash(0x0800_0000, 64 * 1024).is_ok());
        assert!(c.ensure_in_flash(0x0800_0000, 64 * 1024 + 1).is_err());
        assert!(c.ensure_in_flash(0x07FF_FFFF, 4).is_err());
    }
}
