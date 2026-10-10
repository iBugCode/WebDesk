//! 服务端 -> 客户端消息:HELLO 与 FRAME。

use crate::protocol::*;

/// HELLO:连接后的能力协商。
pub struct Hello {
    pub width: u16,
    pub height: u16,
}

impl Hello {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12);
        out.push(MSG_HELLO);
        out.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.push(PIXEL_FORMAT_RGBA8888);
        out.extend_from_slice(&(ENC_RAW | ENC_ZSTD | ENC_WEBP).to_le_bytes());
        out
    }
}

/// FRAME 内的单一操作。
#[derive(Debug, Clone)]
pub enum Op {
    /// 矩形新像素。x/y/w/h 为原始屏幕坐标;pw/ph 为 payload 实际尺寸(画面质量缩放后)。
    Rect {
        x: i16,
        y: i16,
        w: u16,
        h: u16,
        pw: u16,
        ph: u16,
        codec: u8,
        payload: Vec<u8>,
    },
    /// 复制客户端已有像素区域。
    Copy {
        src_x: i16,
        src_y: i16,
        dst_x: i16,
        dst_y: i16,
        w: u16,
        h: u16,
    },
    /// 远端光标位置(原始屏幕坐标)。协议保留:V1.0.1 起客户端用本地指针,
    /// 服务端默认不再发送此操作。
    #[allow(dead_code)]
    Cursor { x: i16, y: i16 },
}

impl Op {
    pub fn encoded_len(&self) -> usize {
        match self {
            Op::Rect { payload, .. } => 1 + 2 + 2 + 2 + 2 + 2 + 2 + 1 + 4 + payload.len(),
            Op::Copy { .. } => 1 + 2 + 2 + 2 + 2 + 2 + 2,
            Op::Cursor { .. } => 1 + 2 + 2,
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Op::Rect {
                x,
                y,
                w,
                h,
                pw,
                ph,
                codec,
                payload,
            } => {
                out.push(1);
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
                out.extend_from_slice(&w.to_le_bytes());
                out.extend_from_slice(&h.to_le_bytes());
                out.extend_from_slice(&pw.to_le_bytes());
                out.extend_from_slice(&ph.to_le_bytes());
                out.push(*codec);
                out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                out.extend_from_slice(payload);
            }
            Op::Copy {
                src_x,
                src_y,
                dst_x,
                dst_y,
                w,
                h,
            } => {
                out.push(2);
                out.extend_from_slice(&src_x.to_le_bytes());
                out.extend_from_slice(&src_y.to_le_bytes());
                out.extend_from_slice(&dst_x.to_le_bytes());
                out.extend_from_slice(&dst_y.to_le_bytes());
                out.extend_from_slice(&w.to_le_bytes());
                out.extend_from_slice(&h.to_le_bytes());
            }
            Op::Cursor { x, y } => {
                out.push(3);
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
        }
    }
}

/// FRAME:一次屏幕更新,包含若干操作。
pub struct FrameBuilder {
    pub sequence: u32,
    pub timestamp_ms: u64,
    pub ops: Vec<Op>,
}

impl FrameBuilder {
    pub fn new(sequence: u32, timestamp_ms: u64) -> Self {
        Self {
            sequence,
            timestamp_ms,
            ops: Vec::new(),
        }
    }

    pub fn push(&mut self, op: Op) {
        self.ops.push(op);
    }

    pub fn byte_len(&self) -> usize {
        15 + self.ops.iter().map(|o| o.encoded_len()).sum::<usize>()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.byte_len());
        out.push(MSG_FRAME);
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        out.extend_from_slice(&(self.ops.len() as u16).to_le_bytes());
        for op in &self.ops {
            op.write(&mut out);
        }
        out
    }
}

/// CLIPBOARD 服务端消息:UTF-8 文本(设计文档 §13)。
pub struct ClipboardMsg {
    pub text: String,
}

impl ClipboardMsg {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let bytes = self.text.as_bytes();
        let mut out = Vec::with_capacity(5 + bytes.len());
        out.push(MSG_CLIPBOARD);
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16at(buf: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([buf[off], buf[off + 1]])
    }
    fn u32at(buf: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    }

    #[test]
    fn hello_layout() {
        let h = Hello {
            width: 1920,
            height: 1080,
        };
        let b = h.encode();
        assert_eq!(b.len(), 12);
        assert_eq!(b[0], MSG_HELLO);
        assert_eq!(u16at(&b, 1), PROTOCOL_VERSION);
        assert_eq!(u16at(&b, 3), 1920);
        assert_eq!(u16at(&b, 5), 1080);
        assert_eq!(b[7], PIXEL_FORMAT_RGBA8888);
        assert_eq!(u32at(&b, 8), ENC_RAW | ENC_ZSTD | ENC_WEBP);
    }

    #[test]
    fn frame_roundtrip() {
        let mut f = FrameBuilder::new(7, 123456789);
        f.push(Op::Rect {
            x: 10,
            y: 20,
            w: 100,
            h: 50,
            pw: 80,
            ph: 40,
            codec: CODEC_ZSTD,
            payload: vec![0xAB; 33],
        });
        f.push(Op::Copy {
            src_x: 1,
            src_y: 2,
            dst_x: 3,
            dst_y: 4,
            w: 300,
            h: 200,
        });
        f.push(Op::Cursor { x: -1, y: 700 });
        let b = f.encode();
        assert_eq!(b.len(), f.byte_len());

        assert_eq!(b[0], MSG_FRAME);
        assert_eq!(u32at(&b, 1), 7);
        assert_eq!(u16at(&b, 13), 3);

        // op1: RECT
        let mut p = 15;
        assert_eq!(b[p], 1);
        assert_eq!(u16at(&b, p + 1) as i16, 10);
        assert_eq!(u16at(&b, p + 3) as i16, 20);
        assert_eq!(u16at(&b, p + 5), 100);
        assert_eq!(u16at(&b, p + 7), 50);
        assert_eq!(u16at(&b, p + 9), 80);
        assert_eq!(u16at(&b, p + 11), 40);
        assert_eq!(b[p + 13], CODEC_ZSTD);
        let len = u32at(&b, p + 14) as usize;
        assert_eq!(len, 33);
        assert_eq!(b[p + 18 + 32], 0xAB);
        p += 18 + len;

        // op2: COPY
        assert_eq!(b[p], 2);
        assert_eq!(u16at(&b, p + 1) as i16, 1);
        assert_eq!(u16at(&b, p + 5) as i16, 3);
        assert_eq!(u16at(&b, p + 9), 300);
        p += 13;

        // op3: CURSOR
        assert_eq!(b[p], 3);
        assert_eq!(u16at(&b, p + 1) as i16, -1);
        assert_eq!(u16at(&b, p + 3) as i16, 700);
    }
}
