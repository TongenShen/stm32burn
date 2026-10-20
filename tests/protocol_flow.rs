//! 协议交互测试：用脚本化的传输层断言「实际发出去的字节」是否正确。
//!
//! 没有硬件也能验证帧格式，这是最容易出错、也最难在真机上调试的部分。

#![allow(clippy::identity_op)] // 测试里刻意保留校验和的推导过程

use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::time::Duration;

use stm32burn::burn::Burner;
use stm32burn::config::{ChipConfig, EraseMode};
use stm32burn::elf::{Firmware, Format, Image};
use stm32burn::protocol::{ACK, NACK};
use stm32burn::transport::Transport;

/// 按预设脚本吐字节的传输层，同时记录所有写出的内容。
struct ScriptedTransport {
    reads: VecDeque<u8>,
    written: Vec<u8>,
}

impl ScriptedTransport {
    fn new(reads: &[u8]) -> Self {
        Self {
            reads: reads.iter().copied().collect(),
            written: Vec::new(),
        }
    }
}

impl Transport for ScriptedTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.written.extend_from_slice(data);
        Ok(())
    }

    fn read_byte(&mut self, _timeout: Duration) -> Result<u8> {
        self.reads.pop_front().ok_or_else(|| anyhow!("脚本已耗尽"))
    }

    fn flush_input(&mut self) -> Result<()> {
        Ok(())
    }
}

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

fn firmware(data: Vec<u8>) -> Firmware {
    Firmware {
        format: Format::Bin,
        images: vec![Image {
            addr: 0x0800_0000,
            data,
        }],
        entry: 0x0800_0000,
    }
}

#[test]
fn write_chunk_four_bytes() {
    let t = ScriptedTransport::new(&[ACK, ACK, ACK]);
    let mut b = Burner::new(t, false);
    b.write_chunk(0x0800_0000, &[0xAA, 0xBB, 0xCC, 0xDD]).unwrap();

    let t = b.into_inner();
    let checksum = 0x03 ^ 0xAA ^ 0xBB ^ 0xCC ^ 0xDD;
    assert_eq!(
        t.written,
        vec![
            0x31, 0xCE, // 写命令
            0x08, 0x00, 0x00, 0x00, 0x08, // 地址 + 校验
            0x03, 0xAA, 0xBB, 0xCC, 0xDD, checksum, // 数据 + 校验
        ]
    );
}

#[test]
fn write_chunk_pads_to_four_bytes() {
    // 3 字节应补 1 个 0xFF，N-1 变成 3
    let t = ScriptedTransport::new(&[ACK, ACK, ACK]);
    let mut b = Burner::new(t, false);
    b.write_chunk(0x0800_0000, &[0x11, 0x22, 0x33]).unwrap();

    let t = b.into_inner();
    let checksum = 0x03 ^ 0x11 ^ 0x22 ^ 0x33 ^ 0xFF;
    assert_eq!(
        t.written,
        vec![
            0x31, 0xCE, 0x08, 0x00, 0x00, 0x00, 0x08, 0x03, 0x11, 0x22, 0x33, 0xFF, checksum,
        ]
    );
}

#[test]
fn erase_single_page_bytes() {
    let t = ScriptedTransport::new(&[ACK, ACK]);
    let mut b = Burner::new(t, false);
    b.erase_pages(&[0x07]).unwrap();

    let t = b.into_inner();
    // 0x43 0xBC | N=0, page=7, checksum=0^7
    assert_eq!(t.written, vec![0x43, 0xBC, 0x00, 0x07, 0x07]);
}

#[test]
fn erase_mass_bytes() {
    let t = ScriptedTransport::new(&[ACK, ACK]);
    let mut b = Burner::new(t, false);
    b.erase_mass().unwrap();

    let t = b.into_inner();
    assert_eq!(t.written, vec![0x43, 0xBC, 0xFF, 0x00]);
}

#[test]
fn get_id_parses_pid() {
    let t = ScriptedTransport::new(&[ACK, 0x01, 0x04, 0x10, ACK]);
    let mut b = Burner::new(t, false);
    assert_eq!(b.get_id().unwrap(), 0x0410);
}

#[test]
fn nack_is_reported_as_error() {
    let t = ScriptedTransport::new(&[NACK]);
    let mut b = Burner::new(t, false);
    let err = b.get_id().unwrap_err();
    assert!(err.to_string().contains("NACK"), "实际错误: {err}");
}

#[test]
fn sync_retries_until_ack() {
    // 第一次读到垃圾字节，第二次才 ACK
    let t = ScriptedTransport::new(&[0x00, ACK]);
    let mut b = Burner::new(t, false);
    b.sync(5).unwrap();

    let t = b.into_inner();
    assert_eq!(t.written, vec![0x7F, 0x7F]);
}

#[test]
fn full_program_sequence_page_mode() {
    // 擦除 1 页（2 个 ACK）+ 写 4 字节（3 个 ACK）
    let t = ScriptedTransport::new(&[ACK, ACK, ACK, ACK, ACK]);
    let mut b = Burner::new(t, false);
    let c = chip();
    let fw = firmware(vec![0x01, 0x02, 0x03, 0x04]);

    let mut noop = |_: stm32burn::burn::Stage, _: usize, _: usize| {};
    b.program(&c, &fw, EraseMode::Page, false, &mut noop)
        .unwrap();

    let t = b.into_inner();
    let checksum = 0x03 ^ 0x01 ^ 0x02 ^ 0x03 ^ 0x04;
    assert_eq!(
        t.written,
        vec![
            0x43, 0xBC, 0x00, 0x00, 0x00, // 擦除第 0 页
            0x31, 0xCE, 0x08, 0x00, 0x00, 0x00, 0x08, // 写命令 + 地址
            0x03, 0x01, 0x02, 0x03, 0x04, checksum, // 数据
        ]
    );
}

#[test]
fn full_512k_chip_splits_erase_to_avoid_mass_erase_marker() {
    // STM32F103VET6：512 KB / 2 KB 页 = 整 256 页。
    // 若一帧塞 256 页，则「页数-1」= 0xFF，与全片擦除标记冲突，会误抹整片。
    // 期望：拆成 255 页 + 1 页两批，第二批从第 259 字节开始。
    let c = ChipConfig {
        name: "STM32F103VET6".into(),
        pids: vec![0x414],
        flash_base: 0x0800_0000,
        flash_size: 0x8_0000,
        page_size: 0x800,
        erase_mode: EraseMode::Page,
        ram_base: Some(0x2000_0000),
    };

    // 擦除 2 批 × 2 个 ACK + 512 KB 数据 2048 块 × 3 个 ACK
    let total_acks = 4 + 2048 * 3;
    let acks = vec![ACK; total_acks];
    let t = ScriptedTransport::new(&acks);
    let mut b = Burner::new(t, false);

    let data = vec![0x5Au8; 0x8_0000];
    let fw = Firmware {
        format: Format::Bin,
        images: vec![Image {
            addr: 0x0800_0000,
            data,
        }],
        entry: 0x0800_0000,
    };

    let mut noop = |_: stm32burn::burn::Stage, _: usize, _: usize| {};
    b.program(&c, &fw, EraseMode::Page, false, &mut noop)
        .unwrap();

    let t = b.into_inner();

    // 第一批：0x43 0xBC | N=254 | 页号 0..254 | 校验
    assert_eq!(t.written[0], 0x43);
    assert_eq!(t.written[1], 0xBC);
    assert_eq!(t.written[2], 254, "第一批 N 必须是 254，不能是 0xFF");
    assert_eq!(t.written[3], 0, "第一批从第 0 页开始");
    assert_eq!(t.written[3 + 254], 254, "第一批最后一页是 254");
    assert_eq!(t.written.len() > 259, true);

    // 第二批：从偏移 2 + 257 开始
    let second = 2 + 257;
    assert_eq!(t.written[second], 0x43);
    assert_eq!(t.written[second + 1], 0xBC);
    assert_eq!(t.written[second + 2], 0, "第二批只有 1 页，N = 0");
    assert_eq!(t.written[second + 3], 255, "第二批擦的是第 255 页");
}
