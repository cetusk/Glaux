//! プラグインの画面を入れるホスト側のウィンドウ(Windows)。
//!
//! 多くのプラグイン(JUCE 製など)は「ホストが用意したウィンドウに埋め込む」方式しか
//! 対応しないので、プラグインのメインスレッドで素のトップレベルウィンドウを作り、
//! その中に `set_parent` で画面を入れてもらう。ウィンドウのメッセージは同じスレッドで
//! [`pump_messages`] を回して処理する(回さないと画面が固まる)。
//!
//! 閉じるボタンは破棄せず隠すだけにして、フラグで知らせる(プラグインの画面の後片付けを
//! 先に済ませてからウィンドウを破棄するため)。

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetWindowLongPtrW, LoadCursorW, PeekMessageW, RegisterClassExW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, MSG, PM_REMOVE, SIZE_MINIMIZED, SWP_NOMOVE,
    SWP_NOZORDER, SW_HIDE, SW_SHOW, WM_CLOSE, WM_SIZE, WNDCLASSEXW, WS_CLIPCHILDREN,
    WS_MAXIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_THICKFRAME,
};

/// ウィンドウからホストへの知らせ(ウィンドウごとに 1 つ、ウィンドウより長生きする)。
#[derive(Default)]
struct Signals {
    close_requested: AtomicBool,
    /// 利用者がウィンドウの大きさを変えた(幅 << 32 | 高さ。0 = なし)
    resized: AtomicU64,
}

pub struct HostWindow {
    hwnd: HWND,
    signals: Box<Signals>,
    style: u32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const CLASS_NAME: &str = "GlauxPluginWindow";

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let signals = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Signals;
    match msg {
        WM_CLOSE => {
            if !signals.is_null() {
                (*signals).close_requested.store(true, Ordering::Release);
            }
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_SIZE => {
            if !signals.is_null() && wparam as u32 != SIZE_MINIMIZED {
                let w = (lparam as u32) & 0xFFFF;
                let h = ((lparam as u32) >> 16) & 0xFFFF;
                (*signals)
                    .resized
                    .store(((w as u64) << 32) | h as u64, Ordering::Release);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn register_class() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let name = wide(CLASS_NAME);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        RegisterClassExW(&wc);
    });
}

impl HostWindow {
    /// 中身(クライアント領域)が `width` × `height` のウィンドウを作る(まだ表示しない)。
    pub fn new(title: &str, width: u32, height: u32, resizable: bool) -> Result<Self, String> {
        register_class();
        let mut style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        if !resizable {
            style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
        }
        let (w, h) = outer_size(style, width, height);
        let signals = Box::new(Signals::default());
        let class = wide(CLASS_NAME);
        let title = wide(title);
        // SAFETY: 登録済みのクラスでトップレベルウィンドウを作る。USERDATA に知らせ用の
        // 構造体を結び付け、ウィンドウを破棄するまで(Drop で先に破棄)解放しない
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err("ウィンドウを作れませんでした".into());
        }
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*signals as *const Signals as isize);
        }
        Ok(HostWindow {
            hwnd,
            signals,
            style,
        })
    }

    pub fn hwnd(&self) -> *mut c_void {
        self.hwnd
    }

    pub fn show(&self) {
        unsafe {
            ShowWindow(self.hwnd, SW_SHOW);
            SetForegroundWindow(self.hwnd);
        }
    }

    /// 中身の大きさを変える(プラグインからの要求)。
    pub fn set_client_size(&self, width: u32, height: u32) {
        let (w, h) = outer_size(self.style, width, height);
        unsafe {
            SetWindowPos(
                self.hwnd,
                std::ptr::null_mut(),
                0,
                0,
                w,
                h,
                SWP_NOMOVE | SWP_NOZORDER,
            );
        }
    }

    /// 閉じるボタンが押されたか(読むと戻る)。
    pub fn take_close_requested(&self) -> bool {
        self.signals.close_requested.swap(false, Ordering::AcqRel)
    }

    /// 利用者が大きさを変えたなら、新しい中身の大きさ。
    pub fn take_resized(&self) -> Option<(u32, u32)> {
        let v = self.signals.resized.swap(0, Ordering::AcqRel);
        (v != 0).then_some(((v >> 32) as u32, (v & 0xFFFF_FFFF) as u32))
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
        }
    }
}

fn outer_size(style: u32, width: u32, height: u32) -> (i32, i32) {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width as i32,
        bottom: height as i32,
    };
    unsafe {
        AdjustWindowRectEx(&mut rect, style, 0, 0);
    }
    (rect.right - rect.left, rect.bottom - rect.top)
}

/// このスレッドのウィンドウメッセージを処理する(溜まっている分だけ。待たない)。
pub fn pump_messages() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
