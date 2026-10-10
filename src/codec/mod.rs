//! 画面编码:画面质量缩放 + WebP 有损 / zstd 无损压缩(V2 设计文档 §4/§5/§6)。

pub mod resample;
pub mod webp;
pub mod zstd;

use crate::capture::region::Rect;
use crate::protocol::{CODEC_RAW, CODEC_WEBP, CODEC_ZSTD};

/// 编码设置(客户端通过 SETTINGS 消息下发)。
#[derive(Clone, Copy, Debug)]
pub struct EncodeSettings {
    /// 画面质量:传输分辨率缩放百分比 10..=100(V2 §6)
    pub scale_pct: u8,
    /// 画面压缩:None = 无损(zstd/raw);Some(q) = WebP lossy quality q(V2 §5/§6)
    pub compression: Option<u8>,
    pub zstd_level: i32,
}

impl Default for EncodeSettings {
    fn default() -> Self {
        // V2 默认走 WebP 有损(§5);70 为初始值,待 benchmark 调整(§10/§16)
        Self {
            scale_pct: 100,
            compression: Some(70),
            zstd_level: 1,
        }
    }
}

/// 小于 512 bytes 直接发 raw,避免压缩开销(V2 §4:小区域不做多余处理)。
const RAW_THRESHOLD: usize = 512;

#[derive(Debug)]
pub struct EncodedRect {
    pub codec: u8,
    pub payload: Vec<u8>,
    /// payload 实际像素尺寸(缩放后)
    pub pw: u16,
    pub ph: u16,
}

impl EncodeSettings {
    pub fn scaled_dims(&self, w: u16, h: u16) -> (u16, u16) {
        if self.scale_pct >= 100 {
            (w, h)
        } else {
            let s = self.scale_pct.max(10) as u32;
            let dw = (w as u32 * s).div_ceil(100).max(1) as u16;
            let dh = (h as u32 * s).div_ceil(100).max(1) as u16;
            (dw.min(w), dh.min(h))
        }
    }
}

/// 编码一个 RGBA 区域。
pub fn encode_rect(
    rgba: &[u8],
    w: u16,
    h: u16,
    s: &EncodeSettings,
) -> Result<EncodedRect, String> {
    let (pw, ph) = s.scaled_dims(w, h);
    let pixels = if (pw, ph) != (w, h) {
        resample::resample_rgba(rgba, w, h, pw, ph)
    } else {
        rgba.to_vec()
    };

    if let Some(q) = s.compression {
        let payload = webp::encode(&pixels, pw, ph, q).map_err(|e| format!("webp 编码失败: {e}"))?;
        return Ok(EncodedRect {
            codec: CODEC_WEBP,
            payload,
            pw,
            ph,
        });
    }

    if pixels.len() < RAW_THRESHOLD {
        return Ok(EncodedRect {
            codec: CODEC_RAW,
            payload: pixels,
            pw,
            ph,
        });
    }
    match zstd::compress(&pixels, s.zstd_level) {
        Ok(c) if c.len() < pixels.len() => Ok(EncodedRect {
            codec: CODEC_ZSTD,
            payload: c,
            pw,
            ph,
        }),
        Ok(_) | Err(_) => Ok(EncodedRect {
            codec: CODEC_RAW,
            payload: pixels,
            pw,
            ph,
        }),
    }
}

// ---------------------------------------------------------------------------
// 滚动检测(Phase 6):用 COPY 替代可平移的重复像素传输
// ---------------------------------------------------------------------------

/// 滚动检测参数。
const SCROLL_MIN_DIM: u16 = 64;
const SCROLL_MAX_OFFSET: i32 = 240;
const SCROLL_MIN_SAVE_BYTES: usize = 16 * 1024;
const SCROLL_MATCH_RATIO: f32 = 0.95;

/// COPY 密集校验网格(每轴采样步数上限,步长 = 尺寸/64 且至少逐像素)。
const SCROLL_VERIFY_GRID: u16 = 64;
/// body 平移假设的不匹配率上限:cur(x,y) 必须几乎完全等于 prev(x,y+dy)。
const SCROLL_BODY_MAX_MISMATCH: f32 = 0.01;
/// strip「新暴露内容」占比下限:低于此值说明条带并无新内容,COPY 假设不成立。
const SCROLL_STRIP_MIN_CHANGED: f32 = 0.02;

pub enum ScrollPlan {
    None,
    /// dy > 0:内容向上移动 dy 像素(如向下滚动)
    Scroll {
        dy: i32,
        /// 无需重传的主体区域
        body: Rect,
        /// 暴露出来的新内容条带
        strip: Rect,
    },
}

/// 在 `rect` 内检测滚动:对比当前像素(rgba,即 rect 的内容)与上一帧缓冲 prev。
///
/// prev 为整屏 RGBA。返回计划;成功时 body 从 prev 拷贝即可还原,strip 需重传。
///
/// 判定流程:稀疏采样(3列×10行)找出候选 dy → 对 body/strip 做**密集校验**
/// (网格采样,见常量注释)→ 全部通过才返回 Scroll。大面积静止/纯色内容对
/// 任意 dy 都能通过稀疏匹配,密集校验用于把这类误判挡在发送之前:错误的 COPY
/// 会把客户端画面整体平移且无法自愈(0.6.0 起无整屏自愈帧)。
pub fn detect_scroll(rect: Rect, rgba: &[u8], prev: &[u8], screen_w: u16, screen_h: u16) -> ScrollPlan {
    if rect.w < SCROLL_MIN_DIM || rect.h < SCROLL_MIN_DIM {
        return ScrollPlan::None;
    }
    if rect.area() as usize * 4 < SCROLL_MIN_SAVE_BYTES * 2 {
        return ScrollPlan::None;
    }

    // rgba 为 rect 区域捕获(步长 rect.w,坐标相对 rect);prev 为整屏(步长 screen_w)。
    // 全部做边界检查,越界视为不匹配。
    let cur_px = |x: i32, y: i32| -> Option<[u8; 4]> {
        let (lx, ly) = (x - rect.x as i32, y - rect.y as i32);
        if lx < 0 || ly < 0 {
            return None;
        }
        let off = (ly as usize * rect.w as usize + lx as usize) * 4;
        if off + 4 > rgba.len() {
            return None;
        }
        Some([rgba[off], rgba[off + 1], rgba[off + 2], rgba[off + 3]])
    };
    let prev_px = |x: i32, y: i32| -> Option<[u8; 4]> {
        if x < 0 || y < 0 {
            return None;
        }
        let off = (y as usize * screen_w as usize + x as usize) * 4;
        if off + 4 > prev.len() {
            return None;
        }
        Some([prev[off], prev[off + 1], prev[off + 2], prev[off + 3]])
    };

    // 采样点:3 列 × 10 行
    let cols = [
        rect.x as i32 + rect.w as i32 / 6,
        rect.x as i32 + rect.w as i32 / 2,
        rect.x as i32 + rect.w as i32 * 5 / 6,
    ];
    let rows: Vec<i32> = (0..10)
        .map(|i| rect.y as i32 + 1 + (rect.h as i32 - 2) * i / 10)
        .collect();

    let match_for = |dy: i32| -> f32 {
        // 假设内容向上移动 dy:cur 行 r 应等于 prev 行 r+dy
        let mut hit = 0usize;
        let mut total = 0usize;
        for &cy in &rows {
            let py = cy + dy;
            if cy < rect.y as i32 || cy >= rect.y as i32 + rect.h as i32 {
                continue;
            }
            if py < 0 || py >= screen_h as i32 {
                continue;
            }
            for &cx in &cols {
                total += 1;
                if let (Some(a), Some(b)) = (cur_px(cx, cy), prev_px(cx, py)) {
                    if a == b {
                        hit += 1;
                    }
                }
            }
        }
        if total == 0 {
            0.0
        } else {
            hit as f32 / total as f32
        }
    };

    let mut best: Option<(i32, f32)> = None;
    for dy in 1..=SCROLL_MAX_OFFSET {
        let r = match_for(dy);
        if r >= SCROLL_MATCH_RATIO && best.map(|(_, br)| r > br).unwrap_or(true) {
            best = Some((dy, r));
        }
    }
    if best.is_none() {
        for dy in (-SCROLL_MAX_OFFSET)..=-1 {
            let r = match_for(dy);
            if r >= SCROLL_MATCH_RATIO && best.map(|(_, br)| r > br).unwrap_or(true) {
                best = Some((dy, r));
            }
        }
    }
    let (dy, _) = match best {
        Some(v) => v,
        None => return ScrollPlan::None,
    };

    let ady = dy.unsigned_abs() as u16;
    if rect.h <= ady + 8 {
        return ScrollPlan::None;
    }
    let body_h = rect.h - ady;
    if body_h as usize * rect.w as usize * 4 < SCROLL_MIN_SAVE_BYTES {
        return ScrollPlan::None;
    }

    let (body, strip) = if dy > 0 {
        (
            Rect::new(rect.x, rect.y, rect.w, body_h),
            Rect::new(rect.x, rect.y + body_h as i16, rect.w, ady),
        )
    } else {
        (
            Rect::new(rect.x, rect.y + ady as i16, rect.w, body_h),
            Rect::new(rect.x, rect.y, rect.w, ady),
        )
    };

    // ---- 密集校验(防误判,见常量注释)----
    // 稀疏采样(3列×10行)对「大面积静止/纯色 + 一小块动态」的脏区(进度条、
    // 光标闪烁、悬停效果等)几乎任意 dy 都能凑满匹配比例;误判发出的 COPY 会把
    // 客户端画面整体平移,而真实变化区域只在 strip 里补一条,漂移随帧累积,
    // 且 0.6.0 起无整屏自愈帧兜底 —— 必须在发送前证伪。任一校验不过即退回
    // 整矩形重传(正确性优先,最多损失带宽优化)。
    let step = |len: u16| (len / SCROLL_VERIFY_GRID).max(1);
    let mut mismatch = 0usize;
    let mut total = 0usize;
    let mut gy = 0u16;
    while gy < body.h {
        let y = body.y as i32 + gy as i32;
        let mut gx = 0u16;
        while gx < body.w {
            let x = body.x as i32 + gx as i32;
            total += 1;
            match (cur_px(x, y), prev_px(x, y + dy)) {
                (Some(a), Some(b)) if a == b => {}
                _ => mismatch += 1,
            }
            gx += step(body.w);
        }
        gy += step(body.h);
    }
    // 采样点太少时统计不可信,直接放弃 COPY
    if total < 256 || mismatch as f32 / total as f32 > SCROLL_BODY_MAX_MISMATCH {
        tracing::debug!(
            "滚动候选 dy={dy} 未通过 body 密集校验(不匹配 {mismatch}/{total}),退回整矩形编码"
        );
        return ScrollPlan::None;
    }

    let mut changed = 0usize;
    let mut total2 = 0usize;
    let mut gy = 0u16;
    while gy < strip.h {
        let y = strip.y as i32 + gy as i32;
        let mut gx = 0u16;
        while gx < strip.w {
            let x = strip.x as i32 + gx as i32;
            total2 += 1;
            match (cur_px(x, y), prev_px(x, y)) {
                (Some(a), Some(b)) if a != b => changed += 1,
                _ => {}
            }
            gx += step(strip.w);
        }
        gy += step(strip.h);
    }
    if total2 < 32 || (changed as f32 / total2 as f32) < SCROLL_STRIP_MIN_CHANGED {
        tracing::debug!(
            "滚动候选 dy={dy} 未通过 strip 密集校验(新内容 {changed}/{total2}),退回整矩形编码"
        );
        return ScrollPlan::None;
    }

    ScrollPlan::Scroll { dy, body, strip }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_dims() {
        let s = EncodeSettings {
            scale_pct: 66,
            ..Default::default()
        };
        assert_eq!(s.scaled_dims(1920, 1080), (1268, 713));
        let s100 = EncodeSettings::default();
        assert_eq!(s100.scaled_dims(1920, 1080), (1920, 1080));
    }

    #[test]
    fn encode_small_is_raw() {
        let px = vec![0u8; 64]; // 4x4
        let s = EncodeSettings {
            compression: None,
            ..Default::default()
        };
        let e = encode_rect(&px, 4, 4, &s).unwrap();
        assert_eq!(e.codec, CODEC_RAW);
        assert_eq!(e.payload.len(), 64);
    }

    #[test]
    fn encode_flat_zstd_and_webp() {
        // 大块纯色:zstd 应显著小于 raw
        let px = vec![0xA5u8; 100 * 100 * 4];
        let s = EncodeSettings {
            compression: None,
            ..Default::default()
        };
        let e = encode_rect(&px, 100, 100, &s).unwrap();
        assert_eq!(e.codec, CODEC_ZSTD);
        assert!(e.payload.len() < 1000);

        let s = EncodeSettings {
            compression: Some(80),
            ..Default::default()
        };
        let e = encode_rect(&px, 100, 100, &s).unwrap();
        assert_eq!(e.codec, CODEC_WEBP);
        assert_eq!(&e.payload[..4], b"RIFF"); // WebP(RIFF 容器)
        assert_eq!(&e.payload[8..12], b"WEBP");
    }

    #[test]
    fn resample_scales_down() {
        // 2x2 → 1x1 取平均
        let src = vec![10, 20, 30, 255, 20, 30, 40, 255, 30, 40, 50, 255, 40, 50, 60, 255];
        let out = resample::resample_rgba(&src, 2, 2, 1, 1);
        assert_eq!(out, vec![25, 35, 45, 255]);
    }

    #[test]
    fn scroll_detection_finds_vertical_shift() {
        // 200x200 屏幕:prev 与 cur 仅垂直平移 40px(内容上移)
        let sw = 200u16;
        let mut prev = vec![0u8; 200 * 200 * 4];
        let mut cur = vec![0u8; 200 * 200 * 4];
        for y in 0..200 {
            for x in 0..200 {
                let v = ((x / 20 * 37 + y / 20 * 61) % 251) as u8;
                for c in 0..3 {
                    prev[(y * 200 + x) * 4 + c] = v;
                    if y >= 40 {
                        cur[((y - 40) * 200 + x) * 4 + c] = v;
                    }
                }
                cur[(y * 200 + x) * 4 + 3] = 255;
                prev[(y * 200 + x) * 4 + 3] = 255;
            }
        }
        let rect = Rect::new(0, 0, 200, 200);
        match detect_scroll(rect, &cur, &prev, sw, 200) {
            ScrollPlan::Scroll { dy, body, strip } => {
                assert_eq!(dy, 40);
                assert_eq!((body.w, body.h), (200, 160));
                assert_eq!((strip.w, strip.h), (200, 40));
            }
            _ => panic!("should detect scroll"),
        }
    }

    #[test]
    fn progress_fill_is_not_scroll() {
        // 回归(0.6.1):进度条式动画 —— 大面积静止(白底+蓝色填充),只有竖直
        // 右边缘前移 30px。竖直边缘对垂直平移不变,稀疏采样能凑满任意 dy;
        // 无密集校验时此处会误发 COPY,把客户端画面整体平移且无法自愈。
        let sw = 400u16;
        let fill = [0x1a, 0x73, 0xe8];
        let mut prev = vec![255u8; 400 * 120 * 4];
        let mut cur = prev.clone();
        for y in 30..90 {
            for x in 0..100 {
                let o = (y * 400 + x) * 4;
                prev[o..o + 3].copy_from_slice(&fill);
            }
            for x in 0..130 {
                let o = (y * 400 + x) * 4;
                cur[o..o + 3].copy_from_slice(&fill);
            }
        }
        let rect = Rect::new(0, 0, 400, 120);
        assert!(matches!(
            detect_scroll(rect, &cur, &prev, sw, 120),
            ScrollPlan::None
        ));
    }

    #[test]
    fn caret_blink_is_not_scroll() {
        // 回归(0.6.1):静止白底上只出现一个 2×24 的黑色光标。变化太小,
        // body 平移假设的网格采样可能完全错过它,但暴露条带并无新内容 ——
        // strip 密集校验必须拒绝 COPY(整矩形重传由 RECT 路径覆盖)。
        let sw = 300u16;
        let prev = vec![255u8; 300 * 100 * 4];
        let mut cur = prev.clone();
        for y in 40..64 {
            for x in 150..152 {
                let o = (y * 300 + x) * 4;
                cur[o..o + 3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let rect = Rect::new(0, 0, 300, 100);
        assert!(matches!(
            detect_scroll(rect, &cur, &prev, sw, 100),
            ScrollPlan::None
        ));
    }
}
