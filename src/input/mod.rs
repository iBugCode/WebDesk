//! 输入注入:XTest 鼠标 / 键盘 / 滚轮(设计文档 §12)。

pub mod keyboard;
pub mod mouse;

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xfixes;
use x11rb::protocol::xproto;
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::protocol::input::MouseMsg;

/// 输入注入器:独立 X 连接。
pub struct Injector {
    pub(crate) conn: RustConnection,
    pub(crate) root: xproto::Window,
    pub(crate) keymap: keyboard::KeyMap,
    /// 客户端当前按住的修饰键 keysym
    pub(crate) client_mods: HashSet<u32>,
    /// 因需要 shift 而被注入器自动包裹的按键 keycode(按下中)
    pub(crate) wrapped_keys: HashSet<u8>,
}

impl Injector {
    pub fn connect() -> Result<Self> {
        let (conn, screen_idx) =
            x11rb::connect(None).map_err(|e| anyhow!("连接 X server 失败: {e}"))?;
        conn.extension_information(xtest::X11_EXTENSION_NAME)?
            .ok_or_else(|| anyhow!("X server 无 XTEST 扩展,无法注入输入"))?;
        // XTEST 约定:先协商版本
        let _ = xtest::get_version(&conn, 2, 4)?.reply()?;
        let root = conn.setup().roots[screen_idx].root;
        let mut inj = Self {
            conn,
            root,
            keymap: keyboard::KeyMap::empty(),
            client_mods: HashSet::new(),
            wrapped_keys: HashSet::new(),
        };
        inj.keymap.reload(&inj.conn)?;
        inj.hide_cursor()?;
        Ok(inj)
    }

    /// 隐藏 X 光标:客户端渲染本地指针,远端画面不再包含光标,
    /// 也消除光标移动产生的无效 damage。XFixes 的隐藏计数随本连接
    /// 断开自动撤销,agent 退出后 Xvfb 光标自动恢复。
    /// 本连接与 App 同生命周期,保证隐藏持续生效。
    fn hide_cursor(&self) -> Result<()> {
        self.conn
            .extension_information(xfixes::X11_EXTENSION_NAME)?
            .ok_or_else(|| anyhow!("无 XFixes 扩展"))?;
        xfixes::query_version(&self.conn, 5, 0)?.reply()?;
        xfixes::hide_cursor(&self.conn, self.root)?.check()?;
        tracing::info!("已隐藏 X 光标(客户端使用本地指针)");
        Ok(())
    }

    pub fn mouse(&self, msg: &MouseMsg) -> Result<()> {
        mouse::apply(&self.conn, self.root, msg)
    }

    /// 键盘映射变化(如布局切换)时重建。
    pub fn reload_keymap(&mut self) -> Result<()> {
        self.keymap.reload(&self.conn)
    }
}
