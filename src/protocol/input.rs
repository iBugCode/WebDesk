//! 客户端 -> 服务端消息解码。

use crate::protocol::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Move = 0,
    Down = 1,
    Up = 2,
    Wheel = 3,
}

#[derive(Debug, Clone, Copy)]
pub struct MouseMsg {
    pub action: MouseAction,
    /// 1=左 2=中 3=右(仅 down/up)
    pub button: u8,
    pub x: i16,
    pub y: i16,
    /// 滚轮格数,正值向右/向下
    pub wheel_dx: i16,
    pub wheel_dy: i16,
}

#[derive(Debug, Clone, Copy)]
pub struct KeyMsg {
    pub pressed: bool,
    pub keysym: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SettingsMsg {
    /// 画面质量(传输分辨率缩放百分比),10..=100
    pub scale_pct: u8,
    /// 画面压缩:0 = 无损(zstd/raw),否则 WebP lossy quality 1..=100(V2 §5)
    pub compression: u8,
    /// 当前 FPS 上限(0.8.0,前端控制):SDK 交互时报 max_fps、空闲倒计时到点
    /// 报 idle_fps,服务端纯执行不解释语义。5 字节旧格式无此字节 → 回退 max_fps。
    pub fps: u8,
}

#[derive(Debug, Clone)]
pub enum ClientMsg {
    Mouse(MouseMsg),
    Key(KeyMsg),
    Settings(SettingsMsg),
    ClipboardSet(String),
    ClipboardGet,
    /// 链路保活:仅维持中间层(反代/NAT)上的双向流量,无任何语义
    Keepalive,
}

#[derive(Debug)]
pub struct ProtocolError(pub String);

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "protocol error: {}", self.0)
    }
}
impl std::error::Error for ProtocolError {}

fn err<T>(msg: &str) -> Result<T, ProtocolError> {
    Err(ProtocolError(msg.to_string()))
}

const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// 解码一条客户端消息。
pub fn decode(buf: &[u8]) -> Result<ClientMsg, ProtocolError> {
    if buf.is_empty() {
        return err("empty message");
    }
    let rd_u16 = |o: usize| -> u16 { u16::from_le_bytes([buf[o], buf[o + 1]]) };
    let rd_u32 = |o: usize| -> u32 {
        u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]])
    };
    match buf[0] {
        MSG_MOUSE => {
            if buf.len() < 11 {
                return err("mouse message too short");
            }
            let action = match buf[1] {
                0 => MouseAction::Move,
                1 => MouseAction::Down,
                2 => MouseAction::Up,
                3 => MouseAction::Wheel,
                _ => return err("bad mouse action"),
            };
            Ok(ClientMsg::Mouse(MouseMsg {
                action,
                button: buf[2],
                x: rd_u16(3) as i16,
                y: rd_u16(5) as i16,
                wheel_dx: rd_u16(7) as i16,
                wheel_dy: rd_u16(9) as i16,
            }))
        }
        MSG_KEY => {
            if buf.len() < 8 {
                return err("key message too short");
            }
            Ok(ClientMsg::Key(KeyMsg {
                pressed: buf[1] == 0,
                keysym: rd_u32(4),
            }))
        }
        MSG_SETTINGS => {
            if buf.len() < 5 {
                return err("settings message too short");
            }
            // 0.8.0 起追加第 6 字节 fps(当前帧率上限);5 字节旧格式回退 max_fps
            let fps = if buf.len() >= 6 { buf[5] } else { buf[4] };
            Ok(ClientMsg::Settings(SettingsMsg {
                scale_pct: buf[1],
                compression: buf[2],
                fps,
            }))
        }
        MSG_CLIPBOARD_SET => {
            if buf.len() < 5 {
                return err("clipboard set too short");
            }
            let len = rd_u32(1) as usize;
            if len > MAX_CLIPBOARD_BYTES {
                return err("clipboard too large");
            }
            if buf.len() < 5 + len {
                return err("clipboard payload truncated");
            }
            let text = String::from_utf8_lossy(&buf[5..5 + len]).into_owned();
            Ok(ClientMsg::ClipboardSet(text))
        }
        MSG_CLIPBOARD_GET => Ok(ClientMsg::ClipboardGet),
        MSG_KEEPALIVE => Ok(ClientMsg::Keepalive),
        _ => err("unknown message type"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_mouse() {
        let mut b = vec![MSG_MOUSE, 3, 0];
        b.extend_from_slice(&100u16.to_le_bytes());
        b.extend_from_slice(&200u16.to_le_bytes());
        b.extend_from_slice(&(-1i16 as u16).to_le_bytes());
        b.extend_from_slice(&2i16.to_le_bytes());
        let m = decode(&b).unwrap();
        match m {
            ClientMsg::Mouse(m) => {
                assert_eq!(m.action, MouseAction::Wheel);
                assert_eq!(m.x, 100);
                assert_eq!(m.y, 200);
                assert_eq!(m.wheel_dx, -1);
                assert_eq!(m.wheel_dy, 2);
            }
            _ => panic!("wrong msg"),
        }
    }

    #[test]
    fn decode_key_and_settings() {
        let mut b = vec![MSG_KEY, 0, 0, 0];
        b.extend_from_slice(&0xff1bu32.to_le_bytes());
        match decode(&b).unwrap() {
            ClientMsg::Key(k) => assert!(k.pressed && k.keysym == 0xff1b),
            _ => panic!(),
        }
        let s = decode(&[MSG_SETTINGS, 75, 80, 1, 20]).unwrap();
        match s {
            ClientMsg::Settings(s) => {
                assert_eq!(s.scale_pct, 75);
                assert_eq!(s.compression, 80);
                // 旧 5 字节格式:fps 回退到 max_fps
                assert_eq!(s.fps, 20);
            }
            _ => panic!(),
        }
        let s6 = decode(&[MSG_SETTINGS, 75, 80, 1, 20, 5]).unwrap();
        match s6 {
            ClientMsg::Settings(s) => {
                // 0.8.0 6 字节格式:第 6 字节为当前 FPS 上限
                assert_eq!(s.fps, 5);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn decode_clipboard() {
        let mut b = vec![MSG_CLIPBOARD_SET];
        b.extend_from_slice(&("你好a".len() as u32).to_le_bytes());
        b.extend_from_slice("你好a".as_bytes());
        match decode(&b).unwrap() {
            ClientMsg::ClipboardSet(t) => assert_eq!(t, "你好a"),
            _ => panic!(),
        }
        assert!(matches!(
            decode(&[MSG_CLIPBOARD_GET]).unwrap(),
            ClientMsg::ClipboardGet
        ));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[0x77]).is_err());
        assert!(decode(&[MSG_MOUSE, 0, 0, 0]).is_err());
    }

    #[test]
    fn decode_keepalive() {
        assert!(matches!(
            decode(&[MSG_KEEPALIVE]).unwrap(),
            ClientMsg::Keepalive
        ));
    }
}
