//! WebDesk Agent 入口。

mod auth;
mod capture;
mod clipboard;
mod codec;
mod config;
mod input;
mod protocol;
mod server;
mod session;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let mut cfg = config::Config::parse();

    // --generate-jwt-keypair:生成后退出,不启动服务
    if cfg.generate_jwt_keypair {
        let (priv_pem, pub_pem, pub_hex) = auth::generate_keypair()?;
        println!("{priv_pem}\n");
        println!("{pub_pem}\n");
        println!("公钥 hex(--jwt-public-key 参数用):\n  {pub_hex}");
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 版本号一律来自 package.json(全局规则),不手写常量
    let version = config::load_version();

    // DISPLAY 由启动参数/环境决定,不接受客户端指定
    if let Some(d) = &cfg.display {
        std::env::set_var("DISPLAY", d);
    }
    let display = std::env::var("DISPLAY").unwrap_or_default();

    let token = cfg.token.clone().unwrap_or_else(gen_token);
    cfg.token = Some(token.clone());

    let listen = server::parse_listen(&cfg.listen)?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let app = rt.block_on(async { session::App::build(&cfg) })?;

    let startup = format!(
        "WebDesk v{version} 启动:DISPLAY={display} 屏幕={}x{} 监听=ws://{listen}(仅 WebSocket,无 HTTP 页面)",
        app.screen_w, app.screen_h
    );
    tracing::info!("{}", startup);
    println!();
    println!("  WebDesk v{version}");
    println!("  WebSocket:  ws://{listen}/ws?token={token}");
    println!();

    rt.block_on(server::run_server(app, listen))?;
    Ok(())
}

fn gen_token() -> String {
    use rand::distributions::Alphanumeric;
    use rand::Rng;
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}
