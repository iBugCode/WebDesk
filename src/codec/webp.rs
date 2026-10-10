//! WebP 有损编码(V2 设计文档 §5)。
//!
//! libwebp 直接接收 RGBA 缓冲;quality 即「画面压缩」参数(1..=100)。
//! method 的 benchmark 范围为 0/2/4(§5),实时远程桌面取 2:
//! 压缩率与编码耗时的折中,最终默认值待 §10 benchmark 确定后再调。

/// WebP method(0 快/低压缩 … 6 慢/高压缩)
const WEBP_METHOD: i32 = 2;

pub fn encode(rgba: &[u8], w: u16, h: u16, quality: u8) -> Result<Vec<u8>, String> {
    let mut cfg = webp::WebPConfig::new().map_err(|_| "WebPConfig 初始化失败".to_string())?;
    cfg.quality = quality.clamp(1, 100) as f32;
    cfg.method = WEBP_METHOD;
    let mem = webp::Encoder::from_rgba(rgba, w as u32, h as u32)
        .encode_advanced(&cfg)
        .map_err(|e| format!("WebP 编码失败: {e:?}"))?;
    Ok(mem.to_vec())
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_riff_container() {
        // 16x16 纯色块
        let px = vec![0x3Au8; 16 * 16 * 4];
        let out = super::encode(&px, 16, 16, 70).unwrap();
        assert_eq!(&out[..4], b"RIFF");
        assert_eq!(&out[8..12], b"WEBP");
        assert!(out.len() < 16 * 16 * 4, "有损编码应显著小于 raw");
    }

    #[test]
    fn quality_scales_payload() {
        // 噪声图对压缩参数敏感:低 quality 产物应不大于高 quality
        let px: Vec<u8> = (0..64 * 64 * 4)
            .map(|i| ((i * 7919) % 251) as u8)
            .collect();
        let hi = super::encode(&px, 64, 64, 90).unwrap().len();
        let lo = super::encode(&px, 64, 64, 40).unwrap().len();
        assert!(lo <= hi + 64, "q40={lo} 应接近或小于 q90={hi}");
    }
}
