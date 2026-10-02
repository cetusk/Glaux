//! プラグインの画面を入れるホスト側のウィンドウ(Linux の X11)。
//!
//! Windows と同じく、プラグインのメインスレッドで素のトップレベルウィンドウを作り、その中に
//! `set_parent` で画面を入れてもらう(Wayland の環境でも XWayland で動く)。libX11 は実行時に読み込むので、
//! 入っていない環境でも Glaux 自体は動く(画面を開くときにエラーを返す)。
//!
//! ウィンドウのイベント(閉じる・大きさの変更)は [`HostWindow::pump`] で処理する。閉じるボタンは
//! 破棄せずフラグで知らせる(プラグインの画面の後片付けを先に済ませてからウィンドウを破棄するため)。

use std::ffi::CString;
use std::os::raw::{c_int, c_uint, c_ulong};
use x11_dl::xlib;

/// X のエラーを記録して続ける(Xlib の既定はプロセスを終わらせるので、プラグインの画面の小さな失敗で
/// Glaux ごと落ちてしまう。エラーの処理はプロセスで 1 つなので、最初に画面を開くときに入れる)
unsafe extern "C" fn on_x_error(_d: *mut xlib::Display, e: *mut xlib::XErrorEvent) -> c_int {
    if let Some(e) = e.as_ref() {
        tracing::debug!(
            "X のエラー(続行): code {} request {}.{}",
            e.error_code,
            e.request_code,
            e.minor_code
        );
    }
    0
}

pub struct HostWindow {
    xlib: xlib::Xlib,
    display: *mut xlib::Display,
    window: xlib::Window,
    wm_delete: xlib::Atom,
    close_requested: bool,
    /// 利用者がウィンドウの大きさを変えた(最後の大きさ)
    resized: Option<(u32, u32)>,
    size: (u32, u32),
}

impl HostWindow {
    pub fn new(title: &str, width: u32, height: u32, resizable: bool) -> Result<Self, String> {
        let xlib = xlib::Xlib::open().map_err(|e| format!("libX11 を読み込めません: {e}"))?;
        // SAFETY: Xlib の呼び出しはこのスレッドだけで行い、作ったものは Drop で片付ける
        unsafe {
            static HANDLER: std::sync::Once = std::sync::Once::new();
            HANDLER.call_once(|| {
                (xlib.XSetErrorHandler)(Some(on_x_error));
            });
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return Err("X のディスプレイに接続できません(DISPLAY を確かめてください)".into());
            }
            let screen = (xlib.XDefaultScreen)(display);
            let root = (xlib.XRootWindow)(display, screen);
            let black = (xlib.XBlackPixel)(display, screen);
            let (w, h) = (width.max(1), height.max(1));
            let window = (xlib.XCreateSimpleWindow)(display, root, 0, 0, w, h, 0, black, black);
            if window == 0 {
                (xlib.XCloseDisplay)(display);
                return Err("ウィンドウを作れません".into());
            }
            if let Ok(t) = CString::new(title) {
                (xlib.XStoreName)(display, window, t.as_ptr());
                // UTF-8 の題名(_NET_WM_NAME)
                let utf8 = (xlib.XInternAtom)(display, c"UTF8_STRING".as_ptr(), 0);
                let net_name = (xlib.XInternAtom)(display, c"_NET_WM_NAME".as_ptr(), 0);
                let bytes = t.as_bytes();
                (xlib.XChangeProperty)(
                    display,
                    window,
                    net_name,
                    utf8,
                    8,
                    xlib::PropModeReplace,
                    bytes.as_ptr(),
                    bytes.len() as c_int,
                );
            }
            let mut wm_delete = (xlib.XInternAtom)(display, c"WM_DELETE_WINDOW".as_ptr(), 0);
            (xlib.XSetWMProtocols)(display, window, &mut wm_delete, 1);
            (xlib.XSelectInput)(display, window, xlib::StructureNotifyMask);
            let mut win = HostWindow {
                xlib,
                display,
                window,
                wm_delete,
                close_requested: false,
                resized: None,
                size: (w, h),
            };
            win.set_hints(w, h, resizable);
            (win.xlib.XFlush)(display);
            Ok(win)
        }
    }

    /// 大きさの決まり(大きさを変えられないプラグインは最小 = 最大にする)
    fn set_hints(&mut self, w: u32, h: u32, resizable: bool) {
        // SAFETY: XSizeHints は C の構造体で、0 埋めが有効な初期値
        unsafe {
            let mut hints: xlib::XSizeHints = std::mem::zeroed();
            if !resizable {
                hints.flags = xlib::PMinSize | xlib::PMaxSize;
                hints.min_width = w as c_int;
                hints.max_width = w as c_int;
                hints.min_height = h as c_int;
                hints.max_height = h as c_int;
            }
            (self.xlib.XSetWMNormalHints)(self.display, self.window, &mut hints);
        }
    }

    /// プラグインに渡すウィンドウの ID
    pub fn id(&self) -> c_ulong {
        self.window
    }

    pub fn show(&self) {
        // SAFETY: 生きているウィンドウ
        unsafe {
            (self.xlib.XMapRaised)(self.display, self.window);
            (self.xlib.XFlush)(self.display);
        }
    }

    /// ウィンドウのイベントを処理する(メインスレッドでこまめに呼ぶ)
    pub fn pump(&mut self) {
        // SAFETY: 生きているディスプレイ。XEvent は共用体で、type で読み分ける
        unsafe {
            while (self.xlib.XPending)(self.display) > 0 {
                let mut ev: xlib::XEvent = std::mem::zeroed();
                (self.xlib.XNextEvent)(self.display, &mut ev);
                match ev.get_type() {
                    xlib::ClientMessage => {
                        let cm = ev.client_message;
                        if cm.data.get_long(0) as xlib::Atom == self.wm_delete {
                            self.close_requested = true;
                            (self.xlib.XUnmapWindow)(self.display, self.window);
                        }
                    }
                    xlib::ConfigureNotify => {
                        let c = ev.configure;
                        let size = (c.width.max(1) as u32, c.height.max(1) as u32);
                        if size != self.size {
                            self.size = size;
                            self.resized = Some(size);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn take_close_requested(&mut self) -> bool {
        std::mem::take(&mut self.close_requested)
    }

    pub fn take_resized(&mut self) -> Option<(u32, u32)> {
        self.resized.take()
    }

    /// プラグインが頼んだ大きさにする
    pub fn set_client_size(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        self.size = (w, h);
        // SAFETY: 生きているウィンドウ
        unsafe {
            (self.xlib.XResizeWindow)(self.display, self.window, w as c_uint, h as c_uint);
            (self.xlib.XFlush)(self.display);
        }
    }

    /// テスト用: ウィンドウの中身を (幅, 高さ, RGB) で読む(画面が描かれたかを確かめる)
    #[doc(hidden)]
    pub fn capture_rgb(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (w, h) = self.size;
        // SAFETY: 生きているウィンドウ。取った画像は読み終えたら破棄する
        unsafe {
            let img = (self.xlib.XGetImage)(
                self.display,
                self.window,
                0,
                0,
                w,
                h,
                !0 as c_ulong,
                xlib::ZPixmap,
            );
            if img.is_null() {
                return None;
            }
            let get = (*img).funcs.get_pixel?;
            let mut out = Vec::with_capacity((w * h * 3) as usize);
            for y in 0..h as c_int {
                for x in 0..w as c_int {
                    let p = get(img, x, y) as u32;
                    out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
                }
            }
            if let Some(destroy) = (*img).funcs.destroy_image {
                destroy(img);
            }
            Some((w, h, out))
        }
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // SAFETY: new で作ったものを 1 回だけ片付ける
        unsafe {
            (self.xlib.XDestroyWindow)(self.display, self.window);
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}
