//! 固件加载：ELF32（ARM）程序头解析与裸 BIN 支持。
//!
//! 关键点：烧写地址必须取程序头的 **p_paddr（LMA）** 而不是 p_vaddr（VMA）。
//! `.data` 段的 VMA 在 RAM、LMA 在 Flash，用错就会把初始值写进内存。

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

const EM_ARM: u16 = 40;
const PT_LOAD: u32 = 1;
const ELF_HEADER_SIZE: usize = 52;
const PHDR_SIZE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Elf,
    Bin,
}

#[derive(Debug, Clone)]
pub struct Image {
    /// 烧写地址（ELF 取 LMA，BIN 取 --base）
    pub addr: u32,
    pub data: Vec<u8>,
}

impl Image {
    pub fn end(&self) -> u32 {
        self.addr.saturating_add(self.data.len() as u32)
    }
}

#[derive(Debug, Clone)]
pub struct Firmware {
    pub format: Format,
    pub images: Vec<Image>,
    pub entry: u32,
}

impl Firmware {
    /// 加载固件。`bin_base` 仅在输入是裸 BIN 时需要。
    pub fn load(path: &Path, bin_base: Option<u32>) -> Result<Self> {
        let raw = fs::read(path)
            .with_context(|| format!("无法读取固件 {}", path.display()))?;
        if raw.is_empty() {
            bail!("固件文件为空: {}", path.display());
        }

        if raw.starts_with(b"\x7fELF") {
            let (images, entry) = parse_elf(&raw)
                .with_context(|| format!("解析 ELF 失败 {}", path.display()))?;
            return Ok(Firmware {
                format: Format::Elf,
                images,
                entry,
            });
        }

        // 非 ELF：按裸 BIN 处理，必须有起始地址
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "hex" {
            bail!("暂不支持 Intel HEX，请先用 objcopy 转成 .bin 或直接用 .elf");
        }
        let base = bin_base.with_context(|| {
            format!(
                "{} 不是 ELF，需要提供起始地址（--base 0x08000000）",
                path.display()
            )
        })?;
        if base % 4 != 0 {
            bail!("--base 必须 4 字节对齐");
        }
        Ok(Firmware {
            format: Format::Bin,
            images: vec![Image { addr: base, data: raw }],
            entry: base,
        })
    }

    /// 只保留完全落在 Flash 范围内的镜像，并按地址排序。
    ///
    /// 典型场景：ELF 里 `.data` 的初始化副本可能被链接到别的地址段，
    /// 或者存在纯 RAM 的 PT_LOAD，这些都不该写给 bootloader。
    pub fn retain_flash(&mut self, flash_base: u32, flash_size: u32) -> Result<()> {
        let flash_end = flash_base as u64 + flash_size as u64;
        self.images.retain(|im| {
            let start = im.addr as u64;
            let end = start + im.data.len() as u64;
            start >= flash_base as u64 && end <= flash_end
        });
        if self.images.is_empty() {
            bail!("固件里没有落在 Flash 范围 0x{flash_base:08X}..0x{flash_end:08X} 内的数据段");
        }
        self.images.sort_by_key(|im| im.addr);
        Ok(())
    }

    /// 数据总字节数。
    pub fn byte_len(&self) -> usize {
        self.images.iter().map(|im| im.data.len()).sum()
    }

    pub fn start(&self) -> u32 {
        self.images.first().map(|im| im.addr).unwrap_or(0)
    }

    pub fn end(&self) -> u32 {
        self.images.last().map(|im| im.end()).unwrap_or(0)
    }

    /// 覆盖全部镜像的地址范围（用于计算需要擦除的页）。
    pub fn span(&self) -> (u32, u32) {
        let start = self.images.iter().map(|i| i.addr).min().unwrap_or(0);
        let end = self.images.iter().map(|i| i.end()).max().unwrap_or(0);
        (start, end)
    }
}

fn parse_elf(raw: &[u8]) -> Result<(Vec<Image>, u32)> {
    if raw.len() < ELF_HEADER_SIZE {
        bail!("文件太小，不是有效的 ELF");
    }
    if raw[4] != 1 {
        bail!("只支持 32 位 ELF（ei_class = {}）", raw[4]);
    }
    if raw[5] != 1 {
        bail!("只支持小端 ELF（ei_data = {}）", raw[5]);
    }
    let machine = u16::from_le_bytes([raw[18], raw[19]]);
    if machine != EM_ARM {
        bail!("不是 ARM 目标（e_machine = {machine}），请确认编译目标是 thumbv7m 等");
    }
    let entry = u32::from_le_bytes(raw[24..28].try_into().unwrap());
    let phoff = u32::from_le_bytes(raw[28..32].try_into().unwrap()) as usize;
    let phentsize = u16::from_le_bytes([raw[42], raw[43]]) as usize;
    let phnum = u16::from_le_bytes([raw[44], raw[45]]) as usize;

    if phnum == 0 {
        bail!("ELF 没有程序头（program headers）");
    }
    if phentsize < PHDR_SIZE {
        bail!("程序头大小异常：{phentsize}");
    }

    let mut images = Vec::new();
    for i in 0..phnum {
        let off = phoff
            .checked_add(i * phentsize)
            .context("程序头偏移溢出")?;
        let end = off.checked_add(PHDR_SIZE).context("程序头偏移溢出")?;
        if end > raw.len() {
            bail!("程序头 #{i} 越界");
        }
        let ph = &raw[off..end];
        let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
        if p_type != PT_LOAD {
            continue;
        }
        let p_offset = u32::from_le_bytes(ph[4..8].try_into().unwrap()) as usize;
        // 注意：取 p_paddr（偏移 12），不是 p_vaddr（偏移 8）
        let p_paddr = u32::from_le_bytes(ph[12..16].try_into().unwrap());
        let p_filesz = u32::from_le_bytes(ph[16..20].try_into().unwrap()) as usize;

        if p_filesz == 0 {
            // 纯 .bss 段，文件里没有内容
            continue;
        }
        let seg_end = p_offset
            .checked_add(p_filesz)
            .context("段长度溢出")?;
        if seg_end > raw.len() {
            bail!("段 #{i} 超出文件末尾");
        }
        images.push(Image {
            addr: p_paddr,
            data: raw[p_offset..seg_end].to_vec(),
        });
    }

    if images.is_empty() {
        bail!("ELF 里没有可加载的数据段");
    }
    images.sort_by_key(|im| im.addr);
    Ok((images, entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小 ELF32/ARM 头，带一个 PT_LOAD 段。
    fn synth_elf(paddr: u32, data: &[u8]) -> Vec<u8> {
        let mut f = vec![0u8; ELF_HEADER_SIZE + PHDR_SIZE];
        f[0..4].copy_from_slice(b"\x7fELF");
        f[4] = 1; // 32 位
        f[5] = 1; // 小端
        f[6] = 1; // EV_CURRENT
        f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        f[18..20].copy_from_slice(&EM_ARM.to_le_bytes());
        f[20..24].copy_from_slice(&1u32.to_le_bytes()); // e_version
        f[24..28].copy_from_slice(&0x0800_0100u32.to_le_bytes()); // entry
        f[28..32].copy_from_slice(&(ELF_HEADER_SIZE as u32).to_le_bytes()); // phoff
        f[40..42].copy_from_slice(&(ELF_HEADER_SIZE as u16).to_le_bytes()); // ehsize
        f[42..44].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes()); // phentsize
        f[44..46].copy_from_slice(&1u16.to_le_bytes()); // phnum
        f[46..48].copy_from_slice(&0u16.to_le_bytes()); // shentsize
        f[48..50].copy_from_slice(&0u16.to_le_bytes()); // shnum

        let ph = ELF_HEADER_SIZE;
        let data_off = ELF_HEADER_SIZE + PHDR_SIZE;
        f[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
        f[ph + 4..ph + 8].copy_from_slice(&(data_off as u32).to_le_bytes());
        f[ph + 8..ph + 12].copy_from_slice(&paddr.to_le_bytes()); // vaddr
        f[ph + 12..ph + 16].copy_from_slice(&paddr.to_le_bytes()); // paddr (LMA)
        f[ph + 16..ph + 20].copy_from_slice(&(data.len() as u32).to_le_bytes());
        f[ph + 20..ph + 24].copy_from_slice(&(data.len() as u32).to_le_bytes());
        f[ph + 24..ph + 28].copy_from_slice(&5u32.to_le_bytes()); // R+X
        f.extend_from_slice(data);
        f
    }

    #[test]
    fn parse_minimal_elf_uses_lma() {
        let data = [0xDE, 0xAD, 0xBE, 0xEF];
        let elf = synth_elf(0x0800_0000, &data);
        let (images, entry) = parse_elf(&elf).unwrap();
        assert_eq!(entry, 0x0800_0100);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].addr, 0x0800_0000);
        assert_eq!(images[0].data, data);
    }

    #[test]
    fn reject_non_elf_magic() {
        let err = parse_elf(&[0u8; 64]).unwrap_err();
        assert!(err.to_string().contains("太小") || err.to_string().contains("ELF"));
    }

    /// 测试用临时文件路径：放在项目 target 目录下，避免系统临时目录的权限问题。
    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-tmp");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn bin_requires_base() {
        // 走 Firmware::load 的 BIN 分支没有 base 时应报错
        let tmp = tmp_path("no_base.bin");
        std::fs::write(&tmp, [1u8, 2, 3, 4]).unwrap();
        let err = Firmware::load(&tmp, None).unwrap_err();
        assert!(err.to_string().contains("--base"));
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn bin_with_base_loads_whole_file() {
        let tmp = tmp_path("with_base.bin");
        std::fs::write(&tmp, [1u8, 2, 3, 4, 5]).unwrap();
        let fw = Firmware::load(&tmp, Some(0x0800_0000)).unwrap();
        assert_eq!(fw.format, Format::Bin);
        assert_eq!(fw.images.len(), 1);
        assert_eq!(fw.images[0].addr, 0x0800_0000);
        assert_eq!(fw.images[0].data.len(), 5);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn retain_flash_filters_out_of_range() {
        let mut fw = Firmware {
            format: Format::Bin,
            images: vec![
                Image { addr: 0x2000_0000, data: vec![0; 16] }, // RAM，应被剔除
                Image { addr: 0x0800_0000, data: vec![0; 16] },
            ],
            entry: 0x0800_0000,
        };
        fw.retain_flash(0x0800_0000, 0x10000).unwrap();
        assert_eq!(fw.images.len(), 1);
        assert_eq!(fw.images[0].addr, 0x0800_0000);
    }
}
