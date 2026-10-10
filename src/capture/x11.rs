//! X11 屏幕读取:优先 MIT-SHM(XShm),不可用时回退 core GetImage。
//!
//! 输出统一转换为 RGBA8888(设计文档 §9)。

use anyhow::{anyhow, Result};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::shm;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::protocol::xproto::{self, ImageFormat};
use x11rb::rust_connection::RustConnection;

pub struct X11Capture {
    pub conn: RustConnection,
    pub root: xproto::Window,
    pub width: u16,
    pub height: u16,
    pub depth: u8,
    shm: Option<ShmCtx>,
    scratch: Vec<u8>,
}

struct ShmCtx {
    seg: u32,
    addr: usize,
    size: usize,
    shmid: i32,
}

// 共享内存指针仅在拥有本结构体的任务内解引用;跨线程只移动所有权。
unsafe impl Send for ShmCtx {}

impl X11Capture {
    /// 连接 DISPLAY(由 config 决定,不接受客户端指定)。
    pub fn connect() -> Result<Self> {
        let (conn, screen_idx) =
            x11rb::connect(None).map_err(|e| anyhow!("连接 X server 失败: {e}"))?;
        let screen = conn
            .setup()
            .roots
            .get(screen_idx)
            .ok_or_else(|| anyhow!("X server 无可用 screen"))?;
        let root = screen.root;
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;
        let depth = screen.root_depth;

        // V1 固定要求 TrueColor + 32bpp 布局
        let fmt_ok = conn
            .setup()
            .pixmap_formats
            .iter()
            .any(|f| f.depth == depth && f.bits_per_pixel == 32);
        let visual_ok = screen
            .allowed_depths
            .iter()
            .flat_map(|d| &d.visuals)
            .any(|v| {
                v.visual_id == screen.root_visual && v.class == xproto::VisualClass::TRUE_COLOR
            });
        if !fmt_ok || !visual_ok {
            return Err(anyhow!(
                "不支持的 root visual/depth(需要 TrueColor + 32bpp,depth={depth})"
            ));
        }

        let shm = match Self::setup_shm(&conn, width as usize * height as usize * 4) {
            Ok(ctx) => {
                tracing::info!("MIT-SHM 可用,启用共享内存区域读取");
                Some(ctx)
            }
            Err(e) => {
                tracing::warn!("MIT-SHM 不可用({e}),回退 core GetImage");
                None
            }
        };

        Ok(Self {
            conn,
            root,
            width,
            height,
            depth,
            shm,
            scratch: Vec::new(),
        })
    }

    fn setup_shm(conn: &RustConnection, size: usize) -> Result<ShmCtx> {
        conn.extension_information(shm::X11_EXTENSION_NAME)?
            .ok_or_else(|| anyhow!("无 MIT-SHM 扩展"))?;
        let ver = shm::query_version(conn)?.reply()?;
        if ver.major_version < 1 {
            return Err(anyhow!("MIT-SHM 版本过低"));
        }
        let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, size, libc::IPC_CREAT | 0o600) };
        if shmid < 0 {
            return Err(anyhow!("shmget 失败: {}", std::io::Error::last_os_error()));
        }
        let addr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
        if addr as isize == -1 {
            unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };
            return Err(anyhow!("shmat 失败: {}", std::io::Error::last_os_error()));
        }
        // 标记删除:所有段 detach 后自动回收
        unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };

        let seg = conn.generate_id()?;
        shm::attach(conn, seg, shmid as u32, false)?.check()?;
        Ok(ShmCtx {
            seg,
            addr: addr as usize,
            size,
            shmid,
        })
    }

    /// 读取屏幕区域,返回 RGBA8888 数据(行连续,无 padding)。
    ///
    /// 返回切片借用内部 scratch,下次调用即失效,调用方须立即消费。
    pub fn get_region_rgba(&mut self, x: i16, y: i16, w: u16, h: u16) -> Result<&[u8]> {
        let n = w as usize * h as usize * 4;
        self.scratch.clear();
        self.scratch.resize(n, 0);
        let depth = self.depth;
        match &self.shm {
            Some(s) => {
                shm::get_image(&self.conn, self.root, x, y, w, h, u32::MAX, u8::from(ImageFormat::Z_PIXMAP), s.seg, 0)?.reply()?;
                // 收到 reply 即代表 X server 已写入共享内存
                let src =
                    unsafe { std::slice::from_raw_parts(s.addr as *const u8, s.size.min(n)) };
                convert_bgrx_to_rgba(src, &mut self.scratch, depth);
            }
            None => {
                let reply = self
                    .conn
                    .get_image(ImageFormat::Z_PIXMAP, self.root, x, y, w, h, u32::MAX)?
                    .reply()?;
                convert_bgrx_to_rgba(&reply.data, &mut self.scratch, depth);
            }
        }
        Ok(&self.scratch)
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        if let Some(s) = self.shm.take() {
            let _ = shm::detach(&self.conn, s.seg);
            unsafe {
                libc::shmdt(s.addr as *const libc::c_void);
                libc::shmctl(s.shmid, libc::IPC_RMID, std::ptr::null_mut());
            }
        }
    }
}

/// X11 ZPixmap 32bpp 小端布局为 [B,G,R,X](大端为 [X,R,G,B]),转换为 RGBA。
fn convert_bgrx_to_rgba(src: &[u8], dst: &mut [u8], depth: u8) {
    let n = dst.len();
    if src.len() < n {
        return;
    }
    #[cfg(target_endian = "little")]
    {
        for i in (0..n).step_by(4) {
            dst[i] = src[i + 2];
            dst[i + 1] = src[i + 1];
            dst[i + 2] = src[i];
            dst[i + 3] = if depth == 32 { src[i + 3] } else { 255 };
        }
    }
    #[cfg(target_endian = "big")]
    {
        for i in (0..n).step_by(4) {
            dst[i] = src[i + 1];
            dst[i + 1] = src[i + 2];
            dst[i + 2] = src[i + 3];
            dst[i + 3] = if depth == 32 { src[i] } else { 255 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgrx_conversion() {
        // B=1 G=2 R=3 X=4 → RGBA = 3,2,1,255 (depth 24)
        let src = [1, 2, 3, 4];
        let mut dst = [0u8; 4];
        convert_bgrx_to_rgba(&src, &mut dst, 24);
        assert_eq!(dst, [3, 2, 1, 255]);
        let mut dst2 = [0u8; 4];
        convert_bgrx_to_rgba(&src, &mut dst2, 32);
        assert_eq!(dst2, [3, 2, 1, 4]);
    }
}
