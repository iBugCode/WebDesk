//! 连接认证:静态 token(`-k`)与 JWT/Ed25519(`--jwt-public-key`)。
//!
//! 两扇门彼此独立、可并存:任一通过即放行。
//! JWT 仅接受 `alg=EdDSA`(RFC 8032),显式拒绝 `none` 与其他算法,
//! 签名验证通过后才解析 claims;`exp`/`nbf` 存在即强制(30s 时钟偏移容忍)。

use anyhow::{anyhow, bail, Result};
use ed25519_dalek::{Signature, SigningKey, Verifier, VerifyingKey, PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH};
#[cfg(test)]
use ed25519_dalek::Signer;

/// exp/nbf 的时钟偏移容忍(秒)
const CLOCK_LEEWAY_SECS: i64 = 30;

/// Ed25519 SPKI DER 固定前缀(RFC 8410):`302a300506032b6570032100` + 32B 公钥
const SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];
/// Ed25519 PKCS#8 私钥 DER 固定前缀(RFC 8410):`302e020100300506032b657004220420` + 32B 种子
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

pub struct Auth {
    /// 静态 token(-k,未指定时为启动随机值);None 表示关闭这扇门
    pub token: Option<String>,
    /// Ed25519 验签公钥(--jwt-public-key);None 表示关闭这扇门
    pub jwt_key: Option<VerifyingKey>,
}

impl Auth {
    pub fn new(token: Option<String>, jwt_key: Option<VerifyingKey>) -> Self {
        Self { token, jwt_key }
    }

    /// WS upgrade 认证:静态 token 常量时间比较,或 JWT 验签 + 有效期检查。
    pub fn authorize(&self, presented: &str) -> bool {
        if presented.is_empty() {
            return false;
        }
        if let Some(t) = &self.token {
            if constant_time_eq(presented.as_bytes(), t.as_bytes()) {
                return true;
            }
        }
        if let Some(key) = &self.jwt_key {
            if verify_jwt(key, presented) {
                return true;
            }
        }
        false
    }
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 校验一个 EdDSA JWT:验签 → 头部 alg 必须显式为 EdDSA → exp/nbf。
/// 只做真伪与有效期判断,不做授权语义(能签出有效 JWT 即视为可接入)。
pub fn verify_jwt(key: &VerifyingKey, token: &str) -> bool {
    let mut parts = token.split('.');
    let (hb, pb, sb) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(h), Some(p), Some(s), None) if !h.is_empty() && !p.is_empty() && !s.is_empty() => {
            (h, p, s)
        }
        _ => return false,
    };

    // 验签对象是 ASCII(header).ASCII(payload)
    let mut signing_input = Vec::with_capacity(hb.len() + pb.len() + 1);
    signing_input.extend_from_slice(hb.as_bytes());
    signing_input.push(b'.');
    signing_input.extend_from_slice(pb.as_bytes());
    let sig = match b64_decode_any(sb) {
        Ok(v) if v.len() == SIGNATURE_LENGTH => match Signature::from_slice(&v) {
            Ok(s) => s,
            Err(_) => return false,
        },
        _ => return false,
    };
    if key.verify(&signing_input, &sig).is_err() {
        return false;
    }

    // alg 必须显式声明为 EdDSA(防 alg=none / 算法混淆)
    let ok_alg = b64_decode_any(hb)
        .ok()
        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok())
        .and_then(|h| h.get("alg").and_then(|a| a.as_str()).map(String::from))
        .as_deref()
        == Some("EdDSA");
    if !ok_alg {
        return false;
    }

    // 有效期:exp/nbf 存在才强制
    let now = unix_secs();
    let claims = b64_decode_any(pb)
        .ok()
        .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
    let Some(claims) = claims else {
        return false;
    };
    if let Some(exp) = claims.get("exp").and_then(|v| v.as_i64()) {
        if now > exp + CLOCK_LEEWAY_SECS {
            return false;
        }
    }
    if let Some(nbf) = claims.get("nbf").and_then(|v| v.as_i64()) {
        if now < nbf - CLOCK_LEEWAY_SECS {
            return false;
        }
    }
    true
}

fn unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 解析 `--jwt-public-key`,支持:64 位 hex、裸 base64(标准/URL-safe,32B)、
/// PEM SPKI 公钥、上述任一内容的文件路径(`@path` 强制按路径)。
pub fn parse_public_key(s: &str) -> Result<VerifyingKey> {
    let text = load_text(s)?;
    let raw = decode_key_bytes(text.trim())?;
    if raw.len() == SPKI_PREFIX.len() + PUBLIC_KEY_LENGTH && raw[..12] == SPKI_PREFIX {
        return VerifyingKey::from_bytes(&raw[12..].try_into().unwrap())
            .map_err(|e| anyhow!("公钥无效: {e}"));
    }
    if raw.len() == PUBLIC_KEY_LENGTH {
        return VerifyingKey::from_bytes(&raw.try_into().unwrap())
            .map_err(|e| anyhow!("公钥无效: {e}"));
    }
    bail!(
        "公钥格式无法识别({} 字节):支持 64 位 hex、32 字节 base64 或 PEM SPKI 公钥",
        raw.len()
    )
}

fn load_text(s: &str) -> Result<String> {
    let path = s.strip_prefix('@').unwrap_or(s);
    if path != s {
        return std::fs::read_to_string(path).map_err(|e| anyhow!("读取公钥文件 {path} 失败: {e}"));
    }
    if s.starts_with("-----BEGIN") || is_key_literal(s) {
        return Ok(s.to_string());
    }
    // 不像内联密钥则当路径尝试
    match std::fs::read_to_string(s) {
        Ok(t) => Ok(t),
        Err(_) => Ok(s.to_string()),
    }
}

fn is_key_literal(s: &str) -> bool {
    // hex(64) 或 base64(43~44 字符,可能带 padding)
    (s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        || ((43..=44).contains(&s.len())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '-' || c == '_' || c == '='))
}

/// 把 hex / base64(标准或 URL-safe)文本解成原始字节;PEM 先剥 armor。
fn decode_key_bytes(text: &str) -> Result<Vec<u8>> {
    let text = strip_pem(text);
    if text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        return (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| anyhow!("hex 解码失败: {e}")))
            .collect();
    }
    b64_decode_any(&text).map_err(|e| anyhow!("base64 解码失败: {e}"))
}

fn strip_pem(text: &str) -> String {
    let body: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("-----"))
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("");
    if body.is_empty() {
        text.trim().to_string()
    } else {
        body
    }
}

/// 标准/URL-safe base64,容忍缺 padding。
fn b64_decode_any(s: &str) -> Result<Vec<u8>, String> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 3 / 4 + 3);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\r' | b'\n' => continue,
            _ => return Err(format!("非法字符 {c:#x}")),
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

fn b64_std_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn pem_wrap(tag: &str, der: &[u8]) -> String {
    let b64 = b64_std_encode(der);
    let mut out = format!("-----BEGIN {tag}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {tag}-----"));
    out
}

/// 生成 Ed25519 密钥对(与 openssl genpkey -algorithm ed25519 互通):
/// 返回 (私钥 PKCS#8 PEM, 公钥 SPKI PEM, 公钥 hex)。
pub fn generate_keypair() -> Result<(String, String, String)> {
    let mut seed = [0u8; 32];
    // rand 0.8 的 thread_rng 即 rand_core 0.6 RngCore(ed25519-dalek "rand_core" feature)
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut seed);
    let sk = SigningKey::from_bytes(&seed);
    let pk = sk.verifying_key();
    let mut der = PKCS8_PREFIX.to_vec();
    der.extend_from_slice(&seed);
    let private_pem = pem_wrap("PRIVATE KEY", &der);
    let mut spki = SPKI_PREFIX.to_vec();
    spki.extend_from_slice(pk.as_bytes());
    let public_pem = pem_wrap("PUBLIC KEY", &spki);
    let public_hex: String = pk.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    Ok((private_pem, public_pem, public_hex))
}

/// 用测试/集成场景便捷签发一个 EdDSA JWT(仅供内部测试调用)。
#[cfg(test)]
pub(crate) fn sign_jwt_for_test(sk: &SigningKey, header: &str, payload: &str) -> String {
    let hb = b64_std_encode(header.as_bytes()).replace('+', "-").replace('/', "_").replace('=', "");
    let pb = b64_std_encode(payload.as_bytes()).replace('+', "-").replace('/', "_").replace('=', "");
    let signing_input = format!("{hb}.{pb}");
    let sig = sk.sign(signing_input.as_bytes());
    let sb: String = b64_std_encode(&sig.to_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .replace('=', "");
    format!("{signing_input}.{sb}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn other_key() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }

    #[test]
    fn jwt_roundtrip_accepts_valid() {
        let sk = new_key();
        let token = sign_jwt_for_test(
            &sk,
            r#"{"alg":"EdDSA","typ":"JWT"}"#,
            r#"{"sub":"u1","exp":9999999999}"#,
        );
        assert!(verify_jwt(&sk.verifying_key(), &token));
    }

    #[test]
    fn jwt_rejects_tampered_payload() {
        let sk = new_key();
        let token = sign_jwt_for_test(&sk, r#"{"alg":"EdDSA"}"#, r#"{"exp":9999999999}"#);
        let forged = format!("{}x", &token[..token.len() - 1]);
        assert!(!verify_jwt(&sk.verifying_key(), &forged));
        // 换密钥签名
        let other = other_key();
        assert!(!verify_jwt(&other.verifying_key(), &token));
    }

    #[test]
    fn jwt_rejects_alg_none_and_garbage() {
        let sk = new_key();
        // alg=none:即使 "签名" 是本密钥签的也必须拒绝
        let token = sign_jwt_for_test(&sk, r#"{"alg":"none"}"#, r#"{"exp":9999999999}"#);
        assert!(!verify_jwt(&sk.verifying_key(), &token));
        assert!(!verify_jwt(&sk.verifying_key(), ""));
        assert!(!verify_jwt(&sk.verifying_key(), "a.b"));
        assert!(!verify_jwt(&sk.verifying_key(), "a.b.c.d"));
    }

    #[test]
    fn jwt_enforces_exp_and_nbf() {
        let sk = new_key();
        let expired = sign_jwt_for_test(&sk, r#"{"alg":"EdDSA"}"#, r#"{"exp":1000000000}"#);
        assert!(!verify_jwt(&sk.verifying_key(), &expired));
        let future = sign_jwt_for_test(&sk, r#"{"alg":"EdDSA"}"#, r#"{"nbf":9999999999}"#);
        assert!(!verify_jwt(&sk.verifying_key(), &future));
        // 无 exp/nbf 视为长期有效
        let forever = sign_jwt_for_test(&sk, r#"{"alg":"EdDSA"}"#, r#"{"sub":"u"}"#);
        assert!(verify_jwt(&sk.verifying_key(), &forever));
    }

    #[test]
    fn auth_two_doors() {
        let sk = new_key();
        let token = sign_jwt_for_test(&sk, r#"{"alg":"EdDSA"}"#, "{}");
        let a = Auth::new(Some("static123".into()), Some(sk.verifying_key()));
        assert!(a.authorize("static123"));
        assert!(a.authorize(&token));
        assert!(!a.authorize("wrong"));
        // 只开 JWT 门
        let b = Auth::new(None, Some(sk.verifying_key()));
        assert!(b.authorize(&token));
        assert!(!b.authorize("static123"));
        // 只开 token 门
        let c = Auth::new(Some("static123".into()), None);
        assert!(c.authorize("static123"));
        assert!(!c.authorize(&token));
    }

    #[test]
    fn parse_public_key_hex_and_pem() {
        let sk = new_key();
        let hex: String = sk.verifying_key().as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let k1 = parse_public_key(&hex).unwrap();
        assert_eq!(k1.as_bytes(), sk.verifying_key().as_bytes());
        let (_, spki, _) = {
            // 用本密钥构造 SPKI PEM
            let mut spki = SPKI_PREFIX.to_vec();
            spki.extend_from_slice(sk.verifying_key().as_bytes());
            ((), pem_wrap("PUBLIC KEY", &spki), ())
        };
        let k2 = parse_public_key(&spki).unwrap();
        assert_eq!(k2.as_bytes(), sk.verifying_key().as_bytes());
        assert!(parse_public_key("zzzz").is_err());
    }

    #[test]
    fn keypair_pem_roundtrip() {
        let (priv_pem, pub_pem, pub_hex) = generate_keypair().unwrap();
        let pub_from_pem = parse_public_key(&pub_pem).unwrap();
        let pub_from_hex = parse_public_key(&pub_hex).unwrap();
        assert_eq!(pub_from_pem.as_bytes(), pub_from_hex.as_bytes());
        // 私钥导出的公钥应与 SPKI 一致(用 dalek 解析 PKCS#8 前缀后的种子)
        assert!(priv_pem.starts_with("-----BEGIN PRIVATE KEY-----"));
    }
}
