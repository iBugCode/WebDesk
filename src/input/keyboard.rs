//! 键盘事件 → XTest。
//!
//! 浏览器发来 keysym,这里通过 core keyboard mapping(keysym → keycode)
//! 找到对应 keycode 注入;若目标 keysym 在 shifted 列(如大写字母),
//! 且客户端未按住 Shift,则自动包裹合成 Shift 按键。


use anyhow::{anyhow, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::protocol::xtest;

use super::Injector;
use crate::protocol::input::KeyMsg;

pub const XK_SHIFT_L: u32 = 0xffe1;
pub const XK_SHIFT_R: u32 = 0xffe2;

#[derive(Debug, Default)]
pub struct KeyMap {
    pub min_keycode: u8,
    pub syms_per_keycode: u8,
    pub keysyms: Vec<u32>,
}

impl KeyMap {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn reload(&mut self, conn: &x11rb::rust_connection::RustConnection) -> Result<()> {
        let setup = conn.setup();
        let first = setup.min_keycode;
        let count = setup.max_keycode.saturating_sub(first).saturating_add(1);
        if count == 0 {
            return Err(anyhow!("keycode 范围为空"));
        }
        let reply = conn.get_keyboard_mapping(first, count)?.reply()?;
        self.min_keycode = first;
        self.syms_per_keycode = reply.keysyms_per_keycode.max(1);
        self.keysyms = reply.keysyms;
        Ok(())
    }

    fn keycode_syms(&self, kc: u8) -> &[u32] {
        if kc < self.min_keycode {
            return &[];
        }
        let idx = (kc - self.min_keycode) as usize;
        let per = self.syms_per_keycode as usize;
        let start = idx * per;
        if start + per > self.keysyms.len() {
            return &[];
        }
        &self.keysyms[start..start + per]
    }

    /// 查找 keysym 对应 keycode;返回 (keycode, 是否位于 shifted 列)。
    pub fn lookup(&self, keysym: u32) -> Option<(u8, bool)> {
        if keysym == 0 {
            return None;
        }
        let mut kc = self.min_keycode;
        loop {
            let syms = self.keycode_syms(kc);
            if syms.is_empty() {
                return None;
            }
            for (col, &s) in syms.iter().enumerate() {
                if s == keysym && col < self.syms_per_keycode as usize {
                    return Some((kc, col % 2 == 1));
                }
            }
            kc = kc.wrapping_add(1);
        }
    }

    pub fn shift_keycode(&self) -> Option<u8> {
        self.lookup(XK_SHIFT_L).map(|(kc, _)| kc)
    }
}

impl Injector {
    pub fn fake_key(&self, detail: u8, press: bool) -> Result<()> {
        let type_ = if press {
            xproto::KEY_PRESS_EVENT
        } else {
            xproto::KEY_RELEASE_EVENT
        };
        xtest::fake_input(&self.conn, type_, detail, 0, 0, 0, 0, 0)?.check()?;
        Ok(())
    }

    pub fn key(&mut self, msg: &KeyMsg) -> Result<()> {
        // 维护客户端修饰键状态
        if is_modifier(msg.keysym) {
            if msg.pressed {
                self.client_mods.insert(msg.keysym);
            } else {
                self.client_mods.remove(&msg.keysym);
            }
        }

        let (kc, needs_shift) = match self.keymap.lookup(msg.keysym) {
            Some(v) => v,
            None => {
                // 可能是布局切换导致映射过期,重建一次再试
                if self.reload_keymap_internal().is_some() {
                    if let Some(v) = self.keymap.lookup(msg.keysym) {
                        v
                    } else {
                        return self.key_missing(msg.keysym);
                    }
                } else {
                    return self.key_missing(msg.keysym);
                }
            }
        };

        tracing::debug!(keysym = msg.keysym, pressed = msg.pressed, kc, "注入键盘事件");
        if msg.pressed {
            let client_shift =
                self.client_mods.contains(&XK_SHIFT_L) || self.client_mods.contains(&XK_SHIFT_R);
            if needs_shift && !client_shift {
                if let Some(sk) = self.keymap.shift_keycode() {
                    self.fake_key(sk, true)?;
                    self.wrapped_keys.insert(kc);
                }
            }
            self.fake_key(kc, true)
        } else {
            self.fake_key(kc, false)?;
            // 若该键曾被合成 shift 包裹,释放 shift
            if self.wrapped_keys.remove(&kc) {
                if let Some(sk) = self.keymap.shift_keycode() {
                    // 仅当客户端自己也没按住 shift 时才释放
                    let client_shift = self.client_mods.contains(&XK_SHIFT_L)
                        || self.client_mods.contains(&XK_SHIFT_R);
                    if !client_shift {
                        self.fake_key(sk, false)?;
                    }
                }
            }
            Ok(())
        }
    }
}

impl Injector {
    fn reload_keymap_internal(&mut self) -> Option<()> {
        match Injector::reload_keymap(self) {
            Ok(()) => Some(()),
            Err(_) => None,
        }
    }

    fn key_missing(&self, keysym: u32) -> Result<()> {
        tracing::debug!("未找到 keysym {:#x} 对应 keycode,忽略", keysym);
        Ok(())
    }
}

fn is_modifier(keysym: u32) -> bool {
    // 0xffe1..=0xffee: Shift/Control/Alt/Meta/Super/Hyper L+R;0xff7f: Num_Lock
    matches!(keysym, 0xffe1..=0xffee | 0xff7f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_keymap() -> KeyMap {
        // keycode 38: 'a'/'A';keycode 39: 's'/'S';keycode 50: Shift_L
        KeyMap {
            min_keycode: 38,
            syms_per_keycode: 2,
            keysyms: vec![0x61, 0x41, 0x73, 0x53, 0xffe1, 0xffe1],
        }
    }

    #[test]
    fn lookup_lower_and_shifted() {
        let km = fake_keymap();
        assert_eq!(km.lookup(0x61), Some((38, false)));
        assert_eq!(km.lookup(0x41), Some((38, true)));
        assert_eq!(km.lookup(0x53), Some((39, true)));
        assert_eq!(km.lookup(0x0), None);
        assert_eq!(km.lookup(0xffff), None);
    }
}
