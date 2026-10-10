//! XDamage 监听:独立 X 连接,把 damage 矩形投递到共享脏区集合。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{anyhow, Result};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::damage;

use super::region::Rect;

/// 共享脏区状态:damage 任务写入,调度任务消费。
pub struct DirtyState {
    pub rects: Mutex<Vec<Rect>>,
    /// 使用 notify_one 的许可语义:无等待者时许可会被保存,不会丢事件
    pub notify: tokio::sync::Notify,
    pub last_damage_ms: AtomicU64,
    width: u16,
    height: u16,
    max_rects: usize,
}

impl DirtyState {
    pub fn new(width: u16, height: u16) -> Arc<Self> {
        Arc::new(Self {
            rects: Mutex::new(Vec::new()),
            notify: tokio::sync::Notify::new(),
            last_damage_ms: AtomicU64::new(0),
            width,
            height,
            max_rects: 4096,
        })
    }

    pub fn push(&self, rect: Rect) {
        {
            let mut guard = self.rects.lock().unwrap();
            if guard.len() >= self.max_rects {
                // 事件洪峰:退化为整屏刷新
                guard.clear();
                guard.push(Rect::full(self.width, self.height));
            }
            guard.push(rect);
        }
        self.last_damage_ms.store(now_ms(), Ordering::Relaxed);
        self.notify.notify_one();
    }

    pub fn push_full_screen(&self) {
        let mut guard = self.rects.lock().unwrap();
        guard.push(Rect::full(self.width, self.height));
        drop(guard);
        self.last_damage_ms.store(now_ms(), Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// 取走全部脏矩形。
    pub fn take(&self) -> Vec<Rect> {
        std::mem::take(&mut self.rects.lock().unwrap())
    }

    /// 限流等待期间把矩形放回,避免丢失。
    pub fn restore(&self, rects: Vec<Rect>) {
        if rects.is_empty() {
            return;
        }
        let mut guard = self.rects.lock().unwrap();
        guard.extend(rects);
        if guard.len() > self.max_rects {
            guard.clear();
            guard.push(Rect::full(self.width, self.height));
        }
    }
}

pub fn now_ms() -> u64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// damage 事件循环。自建 X 连接并创建 Damage 对象,阻塞收事件。
pub fn spawn_damage_task(dirty: Arc<DirtyState>) -> Result<()> {
    let (conn, screen_idx) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_idx].root;
    conn.extension_information(damage::X11_EXTENSION_NAME)?
        .ok_or_else(|| anyhow!("X server 无 XDamage 扩展"))?;
    // 扩展约定:使用前必须先 QueryVersion,否则 Create 会 BadRequest
    let ver = damage::query_version(&conn, 1, 1)?.reply()?;
    tracing::info!("XDamage 版本 {}.{}", ver.major_version, ver.minor_version);
    let damage = conn.generate_id()?;
    damage::create(&conn, damage, root, damage::ReportLevel::RAW_RECTANGLES)?.check()?;

    std::thread::Builder::new()
        .name("webdesk-damage".into())
        .spawn(move || {
            tracing::debug!("damage 任务启动");
            loop {
                match conn.wait_for_event() {
                    Ok(event) => {
                        if let x11rb::protocol::Event::DamageNotify(e) = event {
                            // RawRectangles:area 为本次脏矩形
                            let area = e.area;
                            dirty.push(Rect::new(area.x, area.y, area.width, area.height));
                        }
                    }
                    Err(e) => {
                        tracing::error!("damage 事件循环退出: {e}");
                        break;
                    }
                }
            }
        })?;
    Ok(())
}
