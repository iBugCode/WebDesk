//! Axum WS 服务:只负责 WebSocket 会话,不提供任何 HTML/JS 静态资源。
//! 页面与 SDK 由仓库独立分发(web/dist/sdk.js + web/demo.html,任意静态服务器托管)。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::session::App;

#[derive(Clone)]
struct WebState {
    app: Arc<App>,
}

pub async fn run_server(app: Arc<App>, listen: SocketAddr) -> Result<()> {
    let state = WebState { app };
    // 仅 WebSocket:其余路径一律 404
    let router = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("监听 {listen} 失败"))?;
    axum::serve(listener, router).await?;
    Ok(())
}

async fn ws_handler(
    State(st): State<WebState>,
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    // 最小安全模型:未通过认证(静态 token 或 JWT)的连接一律拒绝
    let presented = params.get("token").map(String::as_str).unwrap_or("");
    if !st.app.auth.authorize(presented) {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    ws.on_upgrade(move |socket| run_session(st.app, socket))
}

/// 单个浏览器会话:HELLO → 接管输入/设置 → 转发帧。
async fn run_session(app: Arc<App>, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
    let id = app.next_session_id();

    // 先发 HELLO(能力协商),再接管会话
    if sink
        .send(Message::Binary(app.hello().encode().into()))
        .await
        .is_err()
    {
        return;
    }
    app.attach_session(id, tx.clone());
    tracing::info!("会话 #{id} 已连接");

    let writer = tokio::spawn(async move {
        // WS 协议层心跳:画面静止时 WebDesk 不发任何帧(核心原则),
        // 而 Cloudflare 等反代对空闲 WS 约 100s 读超时。每 25s 发一次 Ping
        // (浏览器协议栈自动回 Pong),反代/NAT 就不会掐掉空闲连接。
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(m) => {
                        if sink.send(Message::Binary(m.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                },
                _ = ping.tick() => {
                    if sink.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 2026-10-04:不再有「被新会话接管即退出」的单会话逻辑——多开标签页互踢正是
    // 「连接断开(1006),3s 后重连…」永不停歇的根因。这里只等对端断开/出错。
    loop {
        match stream.next().await {
            Some(Ok(Message::Binary(b))) => {
                app.handle_client_msg(&b, &tx).await;
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => {
                tracing::debug!("会话 #{id} WS 错误: {e}");
                break;
            }
            None => break,
        }
    }
    writer.abort();
    app.detach_session(id);
    tracing::info!("会话 #{id} 已断开");
}

pub fn parse_listen(s: &str) -> Result<SocketAddr> {
    s.parse::<SocketAddr>()
        .map_err(|_| anyhow!("listen 地址无效: {s}(示例 127.0.0.1:9527)"))
}
