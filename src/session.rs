//! 会话状态与事件驱动调度器(设计文档 §7/§17 Phase 7)。
//!
//! 调度策略(0.8.0 起 FPS 完全由客户端控制,服务端是纯执行器):
//! - 无画面变化 → 不发送(空闲带宽 ≈ 0)
//! - 有画面变化 → 按 SETTINGS 下发的当前 FPS 上限发送(前端交互时报 20、
//!   空闲倒计时到点报 1,服务端不解释语义、不做任何输入检测)
//! - 连续变化用 batch window(默认 12ms)批量合并

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::auth::Auth;
use crate::capture::damage::{now_ms, spawn_damage_task, DirtyState};
use crate::capture::region::{merge_rects_capped, Rect};
use crate::capture::x11::X11Capture;
use crate::clipboard::{spawn_clipboard_task, ClipboardHandle, ClientSlot};
use crate::codec::{detect_scroll, encode_rect, EncodeSettings, ScrollPlan};
use crate::input::Injector;
use crate::protocol::frame::{FrameBuilder, Hello, Op};
use crate::protocol::input::decode;
use crate::protocol::input::ClientMsg;
use crate::protocol::frame::ClipboardMsg;

/// 单帧最多矩形数(超过则继续合并)。
const MAX_RECTS_PER_FRAME: usize = 24;
/// 合并矩形总面积超过屏幕 70% 时直接整屏。
const FULL_SCREEN_AREA_RATIO: f64 = 0.7;
/// 脏区四周外扩像素数:覆盖 XDamage 边界少报,避免残留 1px 边框。
const RECT_PAD_PX: i16 = 2;

pub struct App {
    pub auth: Auth,
    pub screen_w: u16,
    pub screen_h: u16,
    pub batch_ms: u64,

    pub dirty: Arc<DirtyState>,
    pub client_slot: ClientSlot,
    pub clipboard: ClipboardHandle,
    pub input: tokio::sync::Mutex<Injector>,

    settings: RwLock<EncodeSettings>,
    pub last_frame_ms: AtomicU64,
    /// 当前 FPS 上限(0.8.0,唯一画面节流参数):由前端 SETTINGS 的 fps 字节
    /// 下发(交互时报 max_fps、空闲倒计时到点报 idle_fps),服务端纯执行,
    /// 不做任何输入检测、没有高/低档概念。无 SETTINGS 时取 --max-fps。
    fps_cap: AtomicU8,
    /// 无观众时的 FPS 兜底(最后一个客户端离开后回落,避免空转高帧编码)
    idle_fps_floor: u8,
    frame_seq: AtomicU32,
    session_seq: AtomicU64,
    current_session: AtomicU64,
    /// 每次新会话接管时递增:调度器据此作废上一帧缓冲(滚动检测基准)
    pub session_epoch: AtomicU64,
}

impl App {
    pub fn build(cfg: &crate::config::Config) -> Result<Arc<Self>> {
        // 第一个连接用于校验环境并读取屏幕尺寸
        let probe = X11Capture::connect().context("初始化 X11 捕获连接失败")?;
        let (w, h) = (probe.width, probe.height);
        tracing::info!("X11 屏幕: {}x{} depth {}", w, h, probe.depth);
        drop(probe);

        let dirty = DirtyState::new(w, h);
        let input = Injector::connect().context("初始化 XTest 输入注入失败")?;

        let client_slot: ClientSlot = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let clipboard = spawn_clipboard_task(client_slot.clone())?;

        spawn_damage_task(dirty.clone())?;

        let settings = EncodeSettings {
            scale_pct: cfg.scale_pct.clamp(10, 100),
            compression: if cfg.compression == 0 {
                None
            } else {
                Some(cfg.compression.clamp(1, 100))
            },
            zstd_level: 1,
        };

        let app = Arc::new(Self {
            auth: Auth::new(
                cfg.token.clone(),
                match &cfg.jwt_public_key {
                    Some(s) => Some(
                        crate::auth::parse_public_key(s)
                            .context("解析 --jwt-public-key 失败")?,
                    ),
                    None => None,
                },
            ),
            screen_w: w,
            screen_h: h,
            batch_ms: cfg.batch_ms,
            dirty,
            client_slot,
            clipboard,
            input: tokio::sync::Mutex::new(input),
            settings: RwLock::new(settings),
            last_frame_ms: AtomicU64::new(0),
            fps_cap: AtomicU8::new(cfg.max_fps.clamp(1, 60)),
            idle_fps_floor: cfg.idle_fps.max(1),
            frame_seq: AtomicU32::new(1),
            session_seq: AtomicU64::new(1),
            current_session: AtomicU64::new(0),
            session_epoch: AtomicU64::new(0),
        });

        // 调度器兜底:异常退出时重启并强制全屏刷新(重建滚动检测基准)
        let sched_app = app.clone();
        tokio::spawn(async move {
            loop {
                run_scheduler(sched_app.clone()).await;
                tracing::error!("调度器异常退出,3s 后重启");
                sched_app.session_epoch.fetch_add(1, Ordering::Relaxed);
                sched_app.dirty.push_full_screen();
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        });

        // 残影防护:XDamage 对部分绘制(窗口隐藏、抗锯齿边缘)可能少报边界,
        // 调度器对每个脏区四周外扩 RECT_PAD_PX 像素(见 run_scheduler)覆盖该情况,
        // 保持空闲零带宽原则:交互结束后不再额外推送整屏帧。
        Ok(app)
    }

    pub fn settings_snapshot(&self) -> EncodeSettings {
        *self.settings.read().unwrap()
    }

    fn apply_settings(&self, s: EncodeSettings) -> bool {
        let mut changed = false;
        let mut g = self.settings.write().unwrap();
        if g.scale_pct != s.scale_pct || g.compression != s.compression {
            changed = true;
        }
        *g = s;
        changed
    }

    pub fn next_session_id(&self) -> u64 {
        self.session_seq.fetch_add(1, Ordering::Relaxed)
    }

    /// 新会话接入。**不再踢掉旧会话**(2026-10-04 多会话):多开标签页互踢是
    /// 「连接断开(1006)反复重连」的根因,这里改为并存,帧广播给所有会话。
    pub fn attach_session(&self, id: u64, tx: mpsc::Sender<Vec<u8>>) {
        self.client_slot.write().unwrap().insert(id, tx);
        self.current_session.store(id, Ordering::Relaxed);
        self.session_epoch.fetch_add(1, Ordering::Relaxed);
        // 允许立即发送首帧
        self.last_frame_ms
            .store(now_ms().saturating_sub(10_000), Ordering::Relaxed);
        self.dirty.push_full_screen();
    }

    pub fn detach_session(&self, id: u64) {
        self.client_slot.write().unwrap().remove(&id);
        // 还有别人在看就继续推帧,没人看了才停
        if self.client_slot.read().unwrap().is_empty() {
            self.current_session.store(0, Ordering::Relaxed);
            // 最后一个客户端离开:FPS 回落到无观众兜底,避免空转高帧编码
            self.fps_cap.store(self.idle_fps_floor, Ordering::Relaxed);
        }
    }
    pub fn hello(&self) -> Hello {
        Hello {
            width: self.screen_w,
            height: self.screen_h,
        }
    }

    /// 处理一条客户端消息。服务端不做任何输入检测,只执行注入与设置。
    pub async fn handle_client_msg(&self, buf: &[u8], session_tx: &mpsc::Sender<Vec<u8>>) {
        let msg = match decode(buf) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("忽略非法消息: {e}");
                return;
            }
        };
        match msg {
            ClientMsg::Mouse(m) => {
                let inj = self.input.lock().await;
                if let Err(e) = inj.mouse(&m) {
                    tracing::debug!("鼠标注入失败: {e}");
                }
            }
            ClientMsg::Key(k) => {
                let mut inj = self.input.lock().await;
                if let Err(e) = inj.key(&k) {
                    tracing::debug!("键盘注入失败: {e}");
                }
            }
            ClientMsg::Settings(s) => {
                let settings = EncodeSettings {
                    scale_pct: s.scale_pct.clamp(10, 100),
                    compression: if s.compression == 0 {
                        None
                    } else {
                        Some(s.compression.clamp(1, 100))
                    },
                    zstd_level: 1,
                };
                // 当前 FPS 上限由前端决定(交互 20 / 空闲 1 等),服务端纯执行
                let prev = self.fps_cap.swap(s.fps.clamp(1, 60), Ordering::Relaxed);
                if self.apply_settings(settings) {
                    // 画面参数变化:推一帧全屏,让用户立即看到效果
                    self.dirty.push_full_screen();
                } else if prev != s.fps {
                    // 帧率切换(空闲→交互等):唤醒调度器按新 gap 立即发帧。
                    // 必须及时:点击后的 damage 若在 SETTINGS 之前到达,调度器
                    // 可能已按旧的 1 FPS 进入最长 1s 的限速等待,不唤醒会拖垮
                    // 点击→上画延迟。
                    self.dirty.notify.notify_one();
                }
            }
            ClientMsg::ClipboardSet(text) => {
                if let Err(e) = self.clipboard.set(&text).await {
                    tracing::warn!("设置剪贴板失败: {e}");
                }
            }
            ClientMsg::ClipboardGet => match self.clipboard.get().await {
                Ok(text) => {
                    let _ = session_tx.try_send(ClipboardMsg::new(text).encode());
                }
                Err(e) => tracing::warn!("读取剪贴板失败: {e}"),
            },
            // 链路保活:只为了让反代/NAT 一直看到客户端方向的数据,无任何语义
            ClientMsg::Keepalive => {}
        }
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 把捕获的 RGBA 区域合成进上一帧缓冲(滚动检测基准)。
fn composite(prev: &mut [u8], screen_w: u16, rect: Rect, rgba: &[u8]) {
    let stride = screen_w as usize * 4;
    let rw = rect.w as usize * 4;
    for row in 0..rect.h as usize {
        let dst = (rect.y as usize + row) * stride + rect.x as usize * 4;
        let src = row * rw;
        let end = (src + rw).min(rgba.len());
        if dst + rw <= prev.len() && src < end {
            prev[dst..dst + rw].copy_from_slice(&rgba[src..end]);
        }
    }
}

/// 事件驱动调度器:damage 驱动 + 动态 FPS 限速 + 批量合并。
pub async fn run_scheduler(app: Arc<App>) {
    let mut capture = match X11Capture::connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("调度器 X11 连接失败: {e}");
            return;
        }
    };

    let (w, h) = (app.screen_w, app.screen_h);
    let mut prev = vec![0u8; w as usize * h as usize * 4];
    let mut prev_epoch = 0u64;

    tracing::info!("调度器启动({w}x{h})");
    loop {
        // Phase A:等待画面变化(光标由客户端本地渲染,不参与调度)
        app.dirty.notify.notified().await;

        // Phase B:限速 + 发送循环
        let epoch = app.session_epoch.load(Ordering::Relaxed);
        let first_frame_of_session = epoch != prev_epoch;
        if first_frame_of_session {
            // 新会话:客户端画布已重建,上一帧基准作废,先用整屏重建
            prev_epoch = epoch;
            app.dirty.push_full_screen();
        }
        loop {
            let dirty = app.dirty.take();
            if dirty.is_empty() {
                break;
            }

            // 帧率上限完全由前端 SETTINGS 的 fps 字节决定(0.8.0)
            let fps_cap = app.fps_cap.load(Ordering::Relaxed).max(1) as u64;
            let gap_ms = 1000 / fps_cap;
            let last_dmg = app.dirty.last_damage_ms.load(Ordering::Relaxed);
            let due_ms = app
                .last_frame_ms
                .load(Ordering::Relaxed)
                .saturating_add(gap_ms)
                .max(last_dmg.saturating_add(app.batch_ms));
            let now_v = now_ms();
            if now_v < due_ms {
                app.dirty.restore(dirty);
                let wait = Duration::from_millis(due_ms - now_v);
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {},
                    _ = app.dirty.notify.notified() => {},
                }
                continue;
            }

            // Phase C:合并、捕获、编码、发送
            // 先把每个脏区四周外扩几像素再合并:XDamage 对部分绘制(窗口隐藏/
            // 抗锯齿/亚像素布局)可能按边界少报 1~2px,不外扩会残留细边框。
            let mut rects = merge_rects_capped(
                dirty
                    .iter()
                    .filter_map(|r| r.expand(RECT_PAD_PX).clamp_to(w, h))
                    .collect(),
                1.5,
                MAX_RECTS_PER_FRAME,
                w,
                h,
            );
            let total_area: u64 = rects.iter().map(|r| r.area()).sum();
            if total_area as f64 > FULL_SCREEN_AREA_RATIO * (w as f64 * h as f64) {
                rects = vec![Rect::full(w, h)];
            }

            let settings = app.settings_snapshot();
            let seq = app.frame_seq.fetch_add(1, Ordering::Relaxed);
            let mut frame = FrameBuilder::new(seq, unix_ms());
            let mut payload_bytes: usize = 0;
            let mut ops_count = 0usize;

            for rect in &rects {
                // 捕获与滚动检测/编码严格分步,避免同时持有两份 capture 借用
                let rgba = match capture.get_region_rgba(rect.x, rect.y, rect.w, rect.h) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("区域读取失败: {e}");
                        break;
                    }
                };

                // 滚动检测仅在基准有效时进行(整屏刷新本身跳过)
                let is_full = rect.w == w && rect.h == h && rect.x == 0 && rect.y == 0;
                let scroll = if is_full || first_frame_of_session {
                    ScrollPlan::None
                } else {
                    detect_scroll(*rect, rgba, &prev, w, h)
                };
                match &scroll {
                    ScrollPlan::Scroll { dy, body, .. } => {
                        // 内容向上移动 dy:cur 行 r = prev 行 r+dy
                        frame.push(Op::Copy {
                            src_x: body.x,
                            src_y: body.y + *dy as i16,
                            dst_x: body.x,
                            dst_y: body.y,
                            w: body.w,
                            h: body.h,
                        });
                        ops_count += 1;
                    }
                    ScrollPlan::None => match encode_rect(rgba, rect.w, rect.h, &settings) {
                        Ok(enc) => {
                            payload_bytes += enc.payload.len();
                            frame.push(Op::Rect {
                                x: rect.x,
                                y: rect.y,
                                w: rect.w,
                                h: rect.h,
                                pw: enc.pw,
                                ph: enc.ph,
                                codec: enc.codec,
                                payload: enc.payload,
                            });
                            ops_count += 1;
                        }
                        Err(e) => tracing::warn!("区域编码失败: {e}"),
                    },
                }

                // 先把本矩形合成进 prev(滚动检测基准),再处理暴露条带
                composite(&mut prev, w, *rect, rgba);

                if let ScrollPlan::Scroll { strip, .. } = &scroll {
                    if let Ok(srgba) = capture.get_region_rgba(strip.x, strip.y, strip.w, strip.h)
                    {
                        match encode_rect(srgba, strip.w, strip.h, &settings) {
                            Ok(enc) => {
                                payload_bytes += enc.payload.len();
                                frame.push(Op::Rect {
                                    x: strip.x,
                                    y: strip.y,
                                    w: strip.w,
                                    h: strip.h,
                                    pw: enc.pw,
                                    ph: enc.ph,
                                    codec: enc.codec,
                                    payload: enc.payload,
                                });
                                ops_count += 1;
                            }
                            Err(e) => tracing::warn!("条带编码失败: {e}"),
                        }
                        composite(&mut prev, w, *strip, srgba);
                    }
                }
            }

            if ops_count > 0 {
                let msg = frame.encode();
                // 多会话:同一帧广播给所有在线客户端
                let slot = app.client_slot.read().unwrap();
                for tx in slot.values() {
                    if tx.try_send(msg.clone()).is_err() {
                        tracing::debug!("客户端通道满/关闭,丢弃本帧");
                    }
                }
                tracing::trace!(
                    "frame seq={seq} ops={ops_count} rects={} bytes={payload_bytes} fps_cap={fps_cap}",
                    rects.len()
                );
            }
            app.last_frame_ms.store(now_ms(), Ordering::Relaxed);
        }
    }
}
