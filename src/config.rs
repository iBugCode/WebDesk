//! 启动配置(CLI)与版本号解析。

use clap::Parser;

/// WebDesk Agent 配置。
///
/// 安全边界(设计文档 §15):默认仅监听 localhost;DISPLAY 不接受客户端指定;
/// 所有 WS 连接必须携带随机 token。
#[derive(Parser, Debug, Clone)]
#[command(name = "webdesk", about = "WebDesk - Web-first 远程桌面 Agent")]
pub struct Config {
    /// HTTP/WS 监听地址
    #[arg(long, default_value = "127.0.0.1:9527")]
    pub listen: String,

    /// X11 DISPLAY(默认读环境变量 DISPLAY)
    #[arg(long)]
    pub display: Option<String>,

    /// 会话 token;不指定则随机生成
    #[arg(short = 'k', long = "token")]
    pub token: Option<String>,

    /// JWT(EdDSA/Ed25519)验签公钥:64 位 hex、32B base64、PEM SPKI 公钥或其文件路径;
    /// 与 -k 并存,任一认证通过即放行
    #[arg(long = "jwt-public-key")]
    pub jwt_public_key: Option<String>,

    /// 生成 Ed25519 密钥对(PEM 私钥 / PEM 公钥 / hex 公钥)后退出
    #[arg(long = "generate-jwt-keypair", default_value_t = false)]
    pub generate_jwt_keypair: bool,

    /// 交互模式最高 FPS(默认 20):作为连接建立后的初始 FPS 上限;
    /// 0.8.0 起 SETTINGS 的 fps 字节接管,运行中由前端动态调整
    #[arg(long, default_value_t = 20)]
    pub max_fps: u8,

    /// Damage 批量合并窗口(毫秒,设计文档 §7)
    #[arg(long, default_value_t = 12)]
    pub batch_ms: u64,

    /// [遗留] 空闲档 FPS:仅作为最后一个客户端离开后的回落兜底;
    /// 0.8.0 起 FPS 完全由前端 SETTINGS 控制,服务端不再有 idle/交互模式
    #[arg(long, default_value_t = 1)]
    pub idle_fps: u8,

    /// [遗留] 0.7.x 的空闲超时:0.8.0 起空闲倒计时移到前端 SDK
    /// (默认 3s,可用 WebDesk.window 的 idleTimeout 选项调整),此参数仅保留兼容
    #[arg(long, default_value_t = 3000)]
    pub idle_timeout_ms: u64,

    /// 初始画面质量(传输分辨率百分比)
    #[arg(long, default_value_t = 100)]
    pub scale_pct: u8,

    /// 初始画面压缩:0 = 无损(zstd/raw),1..100 = WebP 有损压缩质量(V2 设计文档 §5/§6)
    #[arg(long, default_value_t = 70)]
    pub compression: u8,
}

/// 读取项目根 package.json 的 version 字段。
///
/// 版本号一律以 package.json 为准,程序内不手写常量。
pub fn load_version() -> String {
    for path in version_candidates() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(ver) = v.get("version").and_then(|s| s.as_str()) {
                    return ver.to_string();
                }
            }
        }
    }
    "0.0.0-dev".to_string()
}

fn version_candidates() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("WEBDESK_PACKAGE_JSON") {
        out.push(std::path::PathBuf::from(p));
    }
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        // target/debug/webdesk → 项目根
        for anc in exe.ancestors().skip(1).take(4) {
            dirs.push(anc.to_path_buf());
        }
    }
    for d in dirs {
        out.push(d.join("package.json"));
    }
    out
}
