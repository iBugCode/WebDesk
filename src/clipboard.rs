//! 剪贴板(设计文档 §13):UTF-8 文本双向同步。
//!
//! - Browser → X11:客户端发 CLIPBOARD_SET,本任务成为 CLIPBOARD 选择属主,
//!   响应 SelectionRequest;随后客户端发送的 Ctrl+V 即可粘贴新内容。
//! - X11 → Browser:XFixes 监听 CLIPBOARD 属主变化,自动读取并推送到客户端。

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::*;
use x11rb::x11_utils::Serialize;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::protocol::frame::ClipboardMsg;

const MAX_TEXT_BYTES: usize = 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(1);
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// 客户端消息发送槽:会话 id → 该会话的发送端。
///
/// 2026-10-04:原为 `Option<Sender>`(单会话,新连接接管旧连接)。浏览器里多开一个
/// demo 标签页就会互相把对方踢掉,表现为满屏「连接断开(1006),3s 后重连…」且永连不上。
/// 改为多会话后:所有连接的帧都广播出去,谁操作都注入到同一个真实桌面。
pub type ClientSlot = Arc<std::sync::RwLock<std::collections::HashMap<u64, tokio::sync::mpsc::Sender<Vec<u8>>>>>;

#[derive(Debug)]
pub enum ClipboardCmd {
    /// 存文本并成为选择属主
    Set(String, tokio::sync::oneshot::Sender<()>),
    /// 读取远端选择
    Get(tokio::sync::oneshot::Sender<String>),
}

#[derive(Clone, Copy, PartialEq)]
enum Pending {
    None,
    /// 显式 Get(拉取按钮 / Ctrl+V 流程外)
    Get,
    /// 属主变化触发的自动读取,完成后推送给客户端
    AutoPush,
}

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: u32,
    utf8_string: u32,
    string: u32,
    text: u32,
    targets: u32,
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<u32> {
    Ok(intern_atom(conn, false, name)?.reply()?.atom)
}

struct ClipboardCtx {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    text: String,
    /// 我们已推送给客户端的最后内容,避免重复推送
    known_text: String,
    cmd_rx: std::sync::mpsc::Receiver<ClipboardCmd>,
    pending: Pending,
    pending_tx: Option<tokio::sync::oneshot::Sender<String>>,
    pending_deadline: Instant,
    client_slot: ClientSlot,
    /// XFixes 不可用时轮询属主
    watch_owner: bool,
    last_seen_owner: u32,
}

/// 启动剪贴板任务(独立线程 + 独立 X 连接)。
pub fn spawn_clipboard_task(client_slot: ClientSlot) -> Result<ClipboardHandle> {
    let (conn, screen_idx) =
        x11rb::connect(None).map_err(|e| anyhow!("剪贴板任务连接 X 失败: {e}"))?;
    let root = conn.setup().roots[screen_idx].root;

    let atoms = Atoms {
        clipboard: intern(&conn, b"CLIPBOARD")?,
        utf8_string: intern(&conn, b"UTF8_STRING")?,
        string: intern(&conn, b"STRING")?,
        text: intern(&conn, b"TEXT")?,
        targets: intern(&conn, b"TARGETS")?,
    };

    let win = conn.generate_id()?;
    create_window(
        &conn,
        0, // InputOnly
        win,
        root,
        -1,
        -1,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT as Visualid,
        &CreateWindowAux::new(),
    )?
    .check()?;

    // XFixes 监听 CLIPBOARD 属主变化;不可用时回退轮询
    let mut watch_owner = false;
    match conn
        .extension_information(x11rb::protocol::xfixes::X11_EXTENSION_NAME)?
    {
        Some(_) => {
            if let Ok(ver) = x11rb::protocol::xfixes::query_version(&conn, 5, 0)?.reply() {
                tracing::info!("XFixes 版本 {}.{}", ver.major_version, ver.minor_version);
            }
            match x11rb::protocol::xfixes::select_selection_input(
                &conn,
                win,
                atoms.clipboard,
                x11rb::protocol::xfixes::SelectionEventMask::SET_SELECTION_OWNER
                    | x11rb::protocol::xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | x11rb::protocol::xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE,
            ) {
                Ok(c) => {
                    if c.check().is_ok() {
                        watch_owner = true;
                        tracing::info!("剪贴板变化监听:XFixes SelectionNotify");
                    }
                }
                Err(e) => tracing::warn!("XFixes select_selection_input 失败: {e}"),
            }
        }
        None => tracing::warn!("X server 无 XFixes 扩展"),
    }
    if !watch_owner {
        tracing::warn!("剪贴板变化监听回退为轮询属主(1.5s)");
    }
    conn.flush()?;

    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<ClipboardCmd>();

    std::thread::Builder::new()
        .name("webdesk-clipboard".into())
        .spawn(move || {
            run(ClipboardCtx {
                conn,
                win,
                atoms,
                text: String::new(),
                known_text: String::new(),
                cmd_rx,
                pending: Pending::None,
                pending_tx: None,
                pending_deadline: Instant::now(),
                client_slot,
                watch_owner,
                last_seen_owner: x11rb::NONE,
            })
        })?;

    Ok(ClipboardHandle { cmd_tx })
}

#[derive(Clone)]
pub struct ClipboardHandle {
    cmd_tx: std::sync::mpsc::Sender<ClipboardCmd>,
}

impl ClipboardHandle {
    /// 存文本并接管 X 剪贴板(等待剪贴板线程 ACK 后再注入 Ctrl+V,保证顺序)。
    pub async fn set(&self, text: &str) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(ClipboardCmd::Set(text.to_string(), tx))
            .map_err(|_| anyhow!("clipboard task closed"))?;
        rx.await.map_err(|_| anyhow!("clipboard task closed"))
    }

    pub async fn get(&self) -> Result<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.cmd_tx
            .send(ClipboardCmd::Get(tx))
            .map_err(|_| anyhow!("clipboard task closed"))?;
        rx.await.map_err(|_| anyhow!("clipboard task closed"))
    }
}

fn run(mut ctx: ClipboardCtx) {
    tracing::debug!("clipboard 任务启动");
    loop {
        // X 事件(非阻塞轮询)
        match ctx.conn.poll_for_event() {
            Ok(Some(event)) => {
                handle_event(&mut ctx, event);
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!("clipboard X 事件错误: {e}");
                break;
            }
        }

        // 命令
        match ctx.cmd_rx.try_recv() {
            Ok(cmd) => handle_cmd(&mut ctx, cmd),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }

        // 读取超时
        if ctx.pending != Pending::None && Instant::now() > ctx.pending_deadline {
            tracing::warn!("剪贴板读取超时");
            finish_read(&mut ctx, String::new());
        }

        // 回退:轮询 CLIPBOARD 属主变化
        if ctx.watch_owner && ctx.pending == Pending::None {
            let owner = match get_selection_owner(&ctx.conn, ctx.atoms.clipboard) {
                Ok(r) => r.reply().map(|rep| rep.owner).unwrap_or(x11rb::NONE),
                Err(_) => x11rb::NONE,
            };
            if owner != ctx.last_seen_owner {
                let prev = ctx.last_seen_owner;
                ctx.last_seen_owner = owner;
                if prev != x11rb::NONE && owner != ctx.win && owner != x11rb::NONE {
                    start_read(&mut ctx, Pending::AutoPush, None);
                }
            }
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

fn handle_cmd(ctx: &mut ClipboardCtx, cmd: ClipboardCmd) {
    match cmd {
        ClipboardCmd::Set(text, ack) => {
            if text.len() <= MAX_TEXT_BYTES {
                ctx.text = text;
                ctx.known_text = ctx.text.clone();
                if set_selection_owner(&ctx.conn, ctx.win, ctx.atoms.clipboard, x11rb::CURRENT_TIME)
                    .is_ok()
                {
                    let _ = ctx.conn.flush();
                }
            }
            let _ = ack.send(());
        }
        ClipboardCmd::Get(reply) => {
            start_read(ctx, Pending::Get, Some(reply));
        }
    }
}

fn start_read(
    ctx: &mut ClipboardCtx,
    kind: Pending,
    reply: Option<tokio::sync::oneshot::Sender<String>>,
) {
    let owner = match get_selection_owner(&ctx.conn, ctx.atoms.clipboard) {
        Ok(r) => r.reply().map(|rep| rep.owner).unwrap_or(x11rb::NONE),
        Err(_) => x11rb::NONE,
    };
    if owner == x11rb::NONE {
        // 无属主
        if kind == Pending::Get {
            if let Some(tx) = reply {
                let _ = tx.send(String::new());
            }
        } else {
            finish_read(ctx, String::new());
        }
        return;
    }
    if owner == ctx.win {
        // 自己拥有:直接用本地文本
        if kind == Pending::Get {
            if let Some(tx) = reply {
                let _ = tx.send(ctx.text.clone());
            }
        } else {
            finish_read(ctx, ctx.text.clone());
        }
        return;
    }
    let _ = convert_selection(
        &ctx.conn,
        ctx.win,
        ctx.atoms.clipboard,
        ctx.atoms.utf8_string,
        ctx.atoms.utf8_string, // property
        x11rb::CURRENT_TIME,
    );
    let _ = ctx.conn.flush();
    ctx.pending = kind;
    ctx.pending_tx = reply;
    ctx.pending_deadline = Instant::now() + READ_TIMEOUT;
}

fn finish_read(ctx: &mut ClipboardCtx, text: String) {
    let kind = ctx.pending;
    ctx.pending = Pending::None;
    if let Some(tx) = ctx.pending_tx.take() {
        let _ = tx.send(text.clone());
    }
    if kind == Pending::AutoPush && !text.is_empty() && text != ctx.known_text {
        ctx.known_text = text.clone();
        send_to_client(ctx, ClipboardMsg::new(text).encode());
    }
}

fn send_to_client(ctx: &ClipboardCtx, msg: Vec<u8>) {
    // 多会话:广播给所有在线客户端(谁在用剪贴板都能收到)
    let slot = ctx.client_slot.read().unwrap();
    for tx in slot.values() {
        let _ = tx.try_send(msg.clone());
    }
}

fn handle_event(ctx: &mut ClipboardCtx, event: Event) {
    match event {
        Event::SelectionRequest(req) => serve_selection_request(ctx, req),
        Event::SelectionNotify(_) => {
            let text = read_convert_result(ctx);
            finish_read(ctx, text);
        }
        Event::SelectionClear(clear) => {
            if clear.selection == ctx.atoms.clipboard {
                tracing::debug!("CLIPBOARD 属主被接管");
            }
        }
        Event::XfixesSelectionNotify(sel) => {
            if sel.selection != ctx.atoms.clipboard {
                return;
            }
            ctx.last_seen_owner = sel.owner;
            if sel.owner == ctx.win || sel.owner == x11rb::NONE {
                return;
            }
            start_read(ctx, Pending::AutoPush, None);
        }
        _ => {}
    }
}

fn read_convert_result(ctx: &mut ClipboardCtx) -> String {
    match get_property(
        &ctx.conn,
        false,
        ctx.win,
        ctx.atoms.utf8_string,
        x11rb::NONE, // AnyPropertyType
        0u32,
        (MAX_TEXT_BYTES / 4) as u32,
    ) {
        Ok(r) => match r.reply() {
            Ok(rep) => String::from_utf8_lossy(&rep.value).into_owned(),
            Err(_) => String::new(),
        },
        Err(_) => String::new(),
    }
}

fn serve_selection_request(ctx: &mut ClipboardCtx, req: SelectionRequestEvent) {
    let mut property = req.property;
    if property == x11rb::NONE {
        property = req.target;
    }
    let success;
    if req.target == ctx.atoms.targets {
        let mut bytes = Vec::with_capacity(16);
        for a in [
            ctx.atoms.targets,
            ctx.atoms.utf8_string,
            ctx.atoms.string,
            ctx.atoms.text,
        ] {
            bytes.extend_from_slice(&a.to_ne_bytes());
        }
        success = change_property(
            &ctx.conn,
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::ATOM,
            32,
            4,
            &bytes,
        )
        .is_ok();
    } else if req.target == ctx.atoms.utf8_string
        || req.target == ctx.atoms.string
        || req.target == ctx.atoms.text
    {
        let bytes = ctx.text.as_bytes();
        let ctype = if req.target == ctx.atoms.utf8_string {
            ctx.atoms.utf8_string
        } else {
            ctx.atoms.string
        };
        success = change_property(
            &ctx.conn,
            PropMode::REPLACE,
            req.requestor,
            property,
            ctype,
            8,
            bytes.len() as u32,
            bytes,
        )
        .is_ok();
    } else {
        property = x11rb::NONE;
        success = false;
    }

    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: if success { property } else { x11rb::NONE },
    };
    let mut buf = [0u8; 32];
    buf[..24].copy_from_slice(&notify.serialize());
    let _ = send_event(&ctx.conn, false, req.requestor, EventMask::NO_EVENT, buf);
    let _ = ctx.conn.flush();
}
