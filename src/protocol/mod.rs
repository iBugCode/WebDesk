//! WebDesk 二进制协议常量与消息编解码。
//!
//! 所有整数一律小端(little-endian)。像素数据为 RGBA8888。

pub mod frame;
pub mod input;

/// 协议版本(V2:RECT codec=2 由 JPEG 改为 WebP)
pub const PROTOCOL_VERSION: u16 = 2;

// ---- 服务端 -> 客户端 ----
pub const MSG_HELLO: u8 = 0x01;
pub const MSG_FRAME: u8 = 0x02;
pub const MSG_CLIPBOARD: u8 = 0x15;

// ---- 客户端 -> 服务端 ----
pub const MSG_MOUSE: u8 = 0x10;
pub const MSG_KEY: u8 = 0x11;
pub const MSG_SETTINGS: u8 = 0x12;
pub const MSG_CLIPBOARD_SET: u8 = 0x13;
pub const MSG_CLIPBOARD_GET: u8 = 0x14;
/// 客户端链路保活(每 25s 一帧;服务端忽略,不计入输入活动)
pub const MSG_KEEPALIVE: u8 = 0x16;

// ---- RECT codec ----
pub const CODEC_RAW: u8 = 0;
pub const CODEC_ZSTD: u8 = 1;
/// 有损压缩:WebP lossy(V2 设计文档 §5,顶替 V1 的 JPEG,沿用 codec id 2)
pub const CODEC_WEBP: u8 = 2;

pub const ENC_RAW: u32 = 1 << 0;
pub const ENC_ZSTD: u32 = 1 << 1;
pub const ENC_WEBP: u32 = 1 << 2;

/// 像素格式: RGBA8888
pub const PIXEL_FORMAT_RGBA8888: u8 = 0;
