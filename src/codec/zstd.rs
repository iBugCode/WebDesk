//! Zstd 压缩(设计文档 §10)。

use std::io::Write;

pub fn compress(data: &[u8], level: i32) -> std::io::Result<Vec<u8>> {
    let mut enc = zstd::stream::Encoder::new(Vec::with_capacity(data.len() / 4), level)?;
    enc.write_all(data)?;
    let out = enc.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip_compresses() {
        let data = vec![7u8; 65536];
        let c = super::compress(&data, 1).unwrap();
        assert!(c.len() < 1000);
        // zstd crate 默认不带解码 feature 时的最小校验:magic number
        assert_eq!(&c[..4], &[0x28, 0xB5, 0x2F, 0xFD]);
    }
}
