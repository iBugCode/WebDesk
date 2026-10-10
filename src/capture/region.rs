//! 脏矩形:定义、合并与限流。

/// 屏幕坐标下的矩形(x/y 为 i16 以容纳 X11 有符号坐标,来源数据均非负)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i16,
    pub y: i16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn new(x: i16, y: i16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub fn full(w: u16, h: u16) -> Self {
        Self { x: 0, y: 0, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }

    /// 裁剪到屏幕范围,空矩形返回 None。
    pub fn clamp_to(self, sw: u16, sh: u16) -> Option<Rect> {
        if self.x >= sw as i16 || self.y >= sh as i16 || self.is_empty() {
            return None;
        }
        let x0 = self.x.max(0);
        let y0 = self.y.max(0);
        let x1 = (self.x as i64 + self.w as i64).min(sw as i64);
        let y1 = (self.y as i64 + self.h as i64).min(sh as i64);
        if x1 <= x0 as i64 || y1 <= y0 as i64 {
            return None;
        }
        Some(Rect {
            x: x0,
            y: y0,
            w: (x1 - x0 as i64) as u16,
            h: (y1 - y0 as i64) as u16,
        })
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x as i32 + self.w as i32).max(other.x as i32 + other.w as i32);
        let y1 = (self.y as i32 + self.h as i32).max(other.y as i32 + other.h as i32);
        Rect {
            x: x0,
            y: y0,
            w: (x1 - x0 as i32) as u16,
            h: (y1 - y0 as i32) as u16,
        }
    }

    /// 四周外扩 pad 像素(可能越界/为空,调用方需再 clamp_to)。
    pub fn expand(&self, pad: i16) -> Rect {
        Rect {
            x: self.x.saturating_sub(pad),
            y: self.y.saturating_sub(pad),
            w: self.w.saturating_add(pad as u16 * 2),
            h: self.h.saturating_add(pad as u16 * 2),
        }
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < (other.x as i32 + other.w as i32) as i16
            && other.x < (self.x as i32 + self.w as i32) as i16
            && self.y < (other.y as i32 + other.h as i32) as i16
            && other.y < (self.y as i32 + self.h as i32) as i16
    }
}

fn clean(mut rects: Vec<Rect>, sw: u16, sh: u16) -> Vec<Rect> {
    rects.retain_mut(|r| match r.clamp_to(sw, sh) {
        Some(c) => {
            *r = c;
            true
        }
        None => false,
    });
    rects.sort_by_key(|r| std::cmp::Reverse(r.area()));
    rects.dedup();
    rects
}

/// 合并脏矩形。
///
/// 策略(设计文档 §6.2):两个矩形若相交,或合并后面积 <= 原面积总和 * `inflate`,
/// 则合并;反复迭代直到稳定,避免产生大量小消息,也避免合并出大片空白。
pub fn merge_rects(rects: Vec<Rect>, inflate: f32, sw: u16, sh: u16) -> Vec<Rect> {
    let mut rects = clean(rects, sw, sh);
    loop {
        let mut changed = false;
        'outer: for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let a = rects[i];
                let b = rects[j];
                let u = a.union(&b);
                if a.intersects(&b)
                    || (u.area() as f64) <= inflate as f64 * (a.area() + b.area()) as f64
                {
                    rects[i] = u;
                    rects.swap_remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
        if !changed {
            break;
        }
    }
    rects
}

/// 合并并限制矩形数量:超过 `max_rects` 时继续合并面积膨胀最小的组合。
pub fn merge_rects_capped(
    rects: Vec<Rect>,
    inflate: f32,
    max_rects: usize,
    sw: u16,
    sh: u16,
) -> Vec<Rect> {
    let mut rects = merge_rects(rects, inflate, sw, sh);
    while rects.len() > max_rects {
        // 找到合并后膨胀率最低的一对
        let mut best: Option<(usize, usize, f64)> = None;
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let u = rects[i].union(&rects[j]);
                let cost = u.area() as f64 / (rects[i].area() + rects[j].area()).max(1) as f64;
                if best.map(|(_, _, c)| cost < c).unwrap_or(true) {
                    best = Some((i, j, cost));
                }
            }
        }
        let (i, j, _) = match best {
            Some(v) => v,
            None => break,
        };
        let u = rects[i].union(&rects[j]);
        rects[i] = u;
        rects.swap_remove(j);
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i16, y: i16, w: u16, h: u16) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn doc_example_merge() {
        // 设计文档 §6.2 示例
        let merged = merge_rects(
            vec![r(10, 10, 100, 30), r(20, 35, 100, 30), r(15, 60, 100, 20)],
            1.5,
            1920,
            1080,
        );
        assert_eq!(merged, vec![r(10, 10, 110, 70)]);
    }

    #[test]
    fn distant_rects_stay_separate() {
        let merged = merge_rects(
            vec![r(0, 0, 10, 10), r(1000, 1000, 10, 10)],
            1.5,
            1920,
            1080,
        );
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn clamp_and_drop_empty() {
        let merged = merge_rects(
            vec![r(-5, -5, 20, 20), r(1900, 1070, 100, 100), r(50, 50, 0, 10)],
            1.5,
            1920,
            1080,
        );
        // 越界部分被裁掉:-5..15 → 0..15;1900..2000 → 1900..1920
        assert_eq!(
            merged,
            vec![r(0, 0, 15, 15), r(1900, 1070, 20, 10)]
        );
    }

    #[test]
    fn cap_limits_count() {
        let rects: Vec<Rect> = (0..10)
            .map(|i| r(i * 300, (i % 2) * 500, 50, 50))
            .collect();
        let merged = merge_rects_capped(rects, 1.5, 3, 1920, 1080);
        assert!(merged.len() <= 3);
    }

    #[test]
    fn expand_pads_and_clamps() {
        // 常规外扩
        assert_eq!(r(10, 10, 100, 100).expand(2), r(8, 8, 104, 104));
        // 越界后 clamp 回屏幕
        assert_eq!(r(0, 0, 10, 10).expand(2).clamp_to(1920, 1080), Some(r(0, 0, 12, 12)));
        // 负坐标饱和,不回绕
        assert_eq!(r(-32767, -32767, 10, 10).expand(2).x, -32768);
        // u16 尺寸饱和
        assert_eq!(r(0, 0, 65535, 65535).expand(2).w, 65535);
    }
}
