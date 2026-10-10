//! 区域缩放(画面质量):Box/面积平均重采样。

/// RGBA8888 面积平均缩放:每个目标像素取其覆盖的源像素加权和。
/// 适合缩小(抗混叠),O(sw*sh) 量级。
pub fn resample_rgba(src: &[u8], sw: u16, sh: u16, dw: u16, dh: u16) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh * 4 {
        return out;
    }
    let xr = sw as f64 / dw as f64;
    let yr = sh as f64 / dh as f64;

    for dy in 0..dh {
        let y0 = dy as f64 * yr;
        let y1 = y0 + yr;
        let iy0 = y0.floor() as usize;
        let iy1 = (y1.ceil() as usize).min(sh);
        for dx in 0..dw {
            let x0 = dx as f64 * xr;
            let x1 = x0 + xr;
            let ix0 = x0.floor() as usize;
            let ix1 = (x1.ceil() as usize).min(sw);
            let mut acc = [0f64; 4];
            let mut wsum = 0f64;
            for sy in iy0..iy1 {
                let wy = (y1 - sy as f64).clamp(0.0, 1.0)
                    * (sy as f64 + 1.0 - y0).clamp(0.0, 1.0);
                if wy <= 0.0 {
                    continue;
                }
                for sx in ix0..ix1 {
                    let wx = (x1 - sx as f64).clamp(0.0, 1.0)
                        * (sx as f64 + 1.0 - x0).clamp(0.0, 1.0);
                    if wx <= 0.0 {
                        continue;
                    }
                    let wgt = wx * wy;
                    let off = (sy * sw + sx) * 4;
                    acc[0] += src[off] as f64 * wgt;
                    acc[1] += src[off + 1] as f64 * wgt;
                    acc[2] += src[off + 2] as f64 * wgt;
                    acc[3] += src[off + 3] as f64 * wgt;
                    wsum += wgt;
                }
            }
            if wsum > 0.0 {
                let off = (dy * dw + dx) * 4;
                out[off] = (acc[0] / wsum).round() as u8;
                out[off + 1] = (acc[1] / wsum).round() as u8;
                out[off + 2] = (acc[2] / wsum).round() as u8;
                out[off + 3] = (acc[3] / wsum).round() as u8;
            }
        }
    }
    out
}
