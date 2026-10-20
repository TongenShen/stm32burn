//! stm32burn —— 通过 USART 烧录 STM32（AN3155 系统存储器 Bootloader）的命令行工具。
//!
//! 模块划分：
//! - [`protocol`]：协议帧的构造与应答解析，纯字节变换，可脱离硬件单测
//! - [`transport`]：串口读写抽象，便于用模拟实现替换
//! - [`burn`]：把协议操作编排成完整的烧录流程
//! - [`config`]：芯片配置文件（TOML）的解析与查找
//! - [`elf`]：ELF / BIN 固件解析

pub mod burn;
pub mod config;
pub mod elf;
pub mod protocol;
pub mod transport;
