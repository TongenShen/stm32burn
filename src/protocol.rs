//! AN3155（STM32 系统存储器 UART Bootloader）协议的帧构造与应答解析。
//!
//! 本模块只做字节变换，不涉及任何 IO，因此可以脱离硬件做完整单元测试。
//!
//! 协议要点（详见 ST AN3155）：
//! - 命令帧 = 命令字节 + 按位取反
//! - 地址帧 = 4 字节大端 + 四字节异或
//! - 写数据帧 = (N-1) + 数据 + ((N-1) 异或所有数据字节)
//! - 读长度帧 = (N-1) + 其余按位取反
//! - 标准擦除 = (页数-1) + 页号（每页 1 字节）+ 异或；全片擦除为 `0xFF 0x00`
//! - 应答 ACK = 0x79，NACK = 0x1F

/// 同步字节：让 bootloader 完成波特率测量并确认进入 UART 模式
pub const SYNC: u8 = 0x7F;
/// 肯定应答
pub const ACK: u8 = 0x79;
/// 否定应答
pub const NACK: u8 = 0x1F;

pub const CMD_GET: u8 = 0x00;
pub const CMD_GET_ID: u8 = 0x02;
pub const CMD_READ: u8 = 0x11;
pub const CMD_GO: u8 = 0x21;
pub const CMD_WRITE: u8 = 0x31;
pub const CMD_ERASE: u8 = 0x43;
pub const CMD_EXT_ERASE: u8 = 0x44;

/// 单次读写命令能携带的最大字节数（AN3155 规定 256）
pub const MAX_CHUNK: usize = 256;
/// 标准擦除一帧最多能带的页数。
///
/// 页号是 1 字节（0..=255），但「页数-1」不能取 0xFF——那是全片擦除的特殊标记，
/// bootloader 会忽略页号直接抹掉整片。所以一帧最多 255 页。
pub const MAX_PAGES_PER_ERASE: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Ack,
    Nack,
}

impl Response {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            ACK => Some(Response::Ack),
            NACK => Some(Response::Nack),
            _ => None,
        }
    }
}

/// 命令帧：`[cmd, !cmd]`
pub fn frame_cmd(cmd: u8) -> [u8; 2] {
    [cmd, !cmd]
}

/// 地址帧：4 字节大端 + 四字节异或校验
pub fn frame_addr(addr: u32) -> [u8; 5] {
    let b = addr.to_be_bytes();
    [b[0], b[1], b[2], b[3], b[0] ^ b[1] ^ b[2] ^ b[3]]
}

/// 写数据帧：`(N-1) + 数据 + 校验`
///
/// 校验为 `(N-1)` 与所有数据字节的异或。
pub fn frame_write_data(data: &[u8]) -> Vec<u8> {
    assert!(!data.is_empty(), "写数据帧不能为空");
    assert!(data.len() <= MAX_CHUNK, "单帧最多 {MAX_CHUNK} 字节");
    let n = (data.len() - 1) as u8;
    let mut checksum = n;
    for &b in data {
        checksum ^= b;
    }
    let mut out = Vec::with_capacity(data.len() + 2);
    out.push(n);
    out.extend_from_slice(data);
    out.push(checksum);
    out
}

/// 读长度帧：`(N-1) + !(N-1)`
pub fn frame_read_len(len: usize) -> [u8; 2] {
    assert!(
        (1..=MAX_CHUNK).contains(&len),
        "读取长度须在 1..={MAX_CHUNK} 之间"
    );
    let n = (len - 1) as u8;
    [n, !n]
}

/// 页擦除帧：`(页数-1) + 页号序列 + 校验`
///
/// 注意页号是 **1 字节**（0..=255），因此一次最多 256 页；
/// 更大的 Flash 需要由调用方分批。
pub fn frame_erase_pages(pages: &[u8]) -> Vec<u8> {
    assert!(!pages.is_empty(), "擦除页列表不能为空");
    assert!(
        pages.len() <= MAX_PAGES_PER_ERASE,
        "一次最多擦除 {MAX_PAGES_PER_ERASE} 页"
    );
    let n = (pages.len() - 1) as u8;
    let mut checksum = n;
    for &p in pages {
        checksum ^= p;
    }
    let mut out = Vec::with_capacity(pages.len() + 2);
    out.push(n);
    out.extend_from_slice(pages);
    out.push(checksum);
    out
}

/// 全片擦除帧：`0xFF 0x00`（N=0xFF 表示全局擦除，校验固定 0x00）
pub fn frame_erase_mass() -> [u8; 2] {
    [0xFF, 0x00]
}

/// 扩展擦除（0x44）的页擦除帧，供 F4 等支持该命令的系列使用。
///
/// 格式：`0x00 + 页数(2 字节) + 页号(每个 2 字节) + 校验`
pub fn frame_ext_erase_pages(pages: &[u16]) -> Vec<u8> {
    assert!(!pages.is_empty(), "擦除页列表不能为空");
    let count = (pages.len() as u16).wrapping_sub(1);
    let count_bytes = count.to_be_bytes();
    let mut out = Vec::with_capacity(3 + pages.len() * 2 + 1);
    out.push(0x00);
    out.push(count_bytes[0]);
    out.push(count_bytes[1]);
    let mut checksum = count_bytes[0] ^ count_bytes[1];
    for &p in pages {
        let b = p.to_be_bytes();
        out.push(b[0]);
        out.push(b[1]);
        checksum ^= b[0] ^ b[1];
    }
    out.push(checksum);
    out
}

#[cfg(test)]
#[allow(clippy::identity_op)] // 测试里刻意保留 XOR 推导过程
mod tests {
    use super::*;

    #[test]
    fn command_frame_is_complement() {
        assert_eq!(frame_cmd(CMD_GET_ID), [0x02, 0xFD]);
        assert_eq!(frame_cmd(CMD_WRITE), [0x31, 0xCE]);
        assert_eq!(frame_cmd(CMD_ERASE), [0x43, 0xBC]);
        assert_eq!(frame_cmd(CMD_GO), [0x21, 0xDE]);
    }

    #[test]
    fn address_frame_checksum() {
        // 0x08000000 -> 08 00 00 00, 校验 = 08
        assert_eq!(frame_addr(0x0800_0000), [0x08, 0x00, 0x00, 0x00, 0x08]);
        // 0x08004ABC -> 08 00 4A BC, 校验 = 08^00^4A^BC
        let cs = 0x08 ^ 0x00 ^ 0x4A ^ 0xBC;
        assert_eq!(frame_addr(0x0800_4ABC), [0x08, 0x00, 0x4A, 0xBC, cs]);
    }

    #[test]
    fn write_frame_single_byte() {
        // 1 字节：N-1 = 0，校验 = 0 ^ data
        let f = frame_write_data(&[0xAA]);
        assert_eq!(f, vec![0x00, 0xAA, 0xAA]);
    }

    #[test]
    fn write_frame_four_bytes() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let f = frame_write_data(&data);
        assert_eq!(f[0], 3);
        assert_eq!(&f[1..5], &data);
        assert_eq!(f[5], 3 ^ 1 ^ 2 ^ 3 ^ 4);
    }

    #[test]
    fn write_frame_max_chunk() {
        let data = vec![0x5A; MAX_CHUNK];
        let f = frame_write_data(&data);
        assert_eq!(f.len(), MAX_CHUNK + 2);
        assert_eq!(f[0], 0xFF);
    }

    #[test]
    fn read_len_frame_is_complement() {
        assert_eq!(frame_read_len(1), [0x00, 0xFF]);
        assert_eq!(frame_read_len(256), [0xFF, 0x00]);
        assert_eq!(frame_read_len(4), [0x03, 0xFC]);
    }

    #[test]
    fn erase_single_page() {
        // 单页：N-1 = 0，校验 = 0 ^ page
        let f = frame_erase_pages(&[0x07]);
        assert_eq!(f, vec![0x00, 0x07, 0x07]);
    }

    #[test]
    fn erase_multiple_pages() {
        let pages = [0x01u8, 0x02, 0x03];
        let f = frame_erase_pages(&pages);
        assert_eq!(f[0], 2);
        assert_eq!(&f[1..4], &pages);
        assert_eq!(f[4], 2 ^ 1 ^ 2 ^ 3);
    }

    #[test]
    fn erase_mass_frame() {
        assert_eq!(frame_erase_mass(), [0xFF, 0x00]);
    }

    #[test]
    fn response_parse() {
        assert_eq!(Response::from_byte(ACK), Some(Response::Ack));
        assert_eq!(Response::from_byte(NACK), Some(Response::Nack));
        assert_eq!(Response::from_byte(0x00), None);
    }

    #[test]
    fn ext_erase_mass_shape() {
        // 单扇区扩展擦除：0x00 + 00 00 + 扇区号 + 校验
        let f = frame_ext_erase_pages(&[0x0005]);
        assert_eq!(f[0], 0x00);
        assert_eq!(&f[1..3], &[0x00, 0x00]);
        assert_eq!(&f[3..5], &[0x00, 0x05]);
        assert_eq!(f[5], 0x00 ^ 0x00 ^ 0x00 ^ 0x05);
    }
}
