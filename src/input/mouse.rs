//! 鼠标事件 → XTest。

use anyhow::{Result};
use x11rb::protocol::xproto;
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::protocol::input::{MouseAction, MouseMsg};

/// 单条消息最多注入的滚轮格数,防刷屏。
const MAX_WHEEL_CLICKS: i16 = 10;

pub fn apply(conn: &RustConnection, root: xproto::Window, msg: &MouseMsg) -> Result<()> {
    tracing::debug!(?msg, "注入鼠标事件");
    let fake = |type_: u8, detail: u8, x: i16, y: i16| -> Result<()> {
        xtest::fake_input(conn, type_, detail, 0, root, x, y, 0)?.check()?;
        Ok(())
    };
    match msg.action {
        MouseAction::Move => fake(xproto::MOTION_NOTIFY_EVENT, 0, msg.x, msg.y),
        MouseAction::Down => {
            let button = msg.button.clamp(1, 3);
            // 某些应用依据事件坐标命中目标,附带位置更稳
            fake(xproto::MOTION_NOTIFY_EVENT, 0, msg.x, msg.y)?;
            fake(xproto::BUTTON_PRESS_EVENT, button, msg.x, msg.y)
        }
        MouseAction::Up => {
            let button = msg.button.clamp(1, 3);
            fake(xproto::BUTTON_RELEASE_EVENT, button, msg.x, msg.y)
        }
        MouseAction::Wheel => {
            fake(xproto::MOTION_NOTIFY_EVENT, 0, msg.x, msg.y)?;
            // X11: 按钮 4/5 = 上/下,6/7 = 左/右
            let dy = msg.wheel_dy.clamp(-MAX_WHEEL_CLICKS, MAX_WHEEL_CLICKS);
            let dx = msg.wheel_dx.clamp(-MAX_WHEEL_CLICKS, MAX_WHEEL_CLICKS);
            let mut clicks: Vec<u8> = Vec::new();
            for _ in 0..dy.unsigned_abs() {
                clicks.push(if dy > 0 { 5 } else { 4 });
            }
            for _ in 0..dx.unsigned_abs() {
                clicks.push(if dx > 0 { 7 } else { 6 });
            }
            for b in clicks {
                fake(xproto::BUTTON_PRESS_EVENT, b, msg.x, msg.y)?;
                fake(xproto::BUTTON_RELEASE_EVENT, b, msg.x, msg.y)?;
            }
            Ok(())
        }
    }
}
