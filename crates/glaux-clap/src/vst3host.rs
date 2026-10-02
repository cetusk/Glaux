//! VST3 のホスト(バインディングは `vst3` クレート。MIT OR Apache-2.0。VST3 SDK のヘッダーも MIT)。
//!
//! CLAP と同じ使い方になるよう、[`crate::ClapPlugin`] / [`crate::ClapProcessor`] の中身として使う。
//! - プラグイン ID は `vst3:<クラス ID の 32 桁の 16 進>`。パスは `.vst3` のバンドル(か中の実体)
//! - パラメータは正規化値(0〜1)をそのまま「プラグインの単位」として扱う
//! - 画面での操作(performEdit)はホストが処理側へ渡す決まりなので、ロックの無い輪状の列でオーディオスレッドへ送る
//! - オーディオスレッドでは確保しない(イベント列・パラメータの列・バッファは起動時に確保して使い回す)
//! - Linux の画面は X11 に埋め込み、プラグインが頼む fd とタイマー(Linux::IRunLoop)をプラグインのスレッドで回す

// VST3 の列挙の型は OS で違う(Windows は i32、ほかは u32)ので、同じ型への変換も残しておく
#![allow(clippy::unnecessary_cast)]

use crate::{
    ClapError, GuiEvent, HostTransport, NoteMsg, ParamInfo, PluginInfo, MAX_EVENTS, MAX_FRAMES,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(target_os = "linux")]
use vst3::ComRef;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};

// ---- 文字列 ----

fn cstr_field(b: &[char8]) -> String {
    let bytes: Vec<u8> = b
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_owned()
}

fn wstr(b: &[TChar]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..end]).trim().to_owned()
}

fn hex_id(cid: &TUID) -> String {
    cid.iter().map(|b| format!("{:02X}", *b as u8)).collect()
}

fn parse_id(id: &str) -> Option<TUID> {
    let h = id.strip_prefix(crate::backend::VST3_PREFIX)?;
    if h.len() != 32 {
        return None;
    }
    let mut out: TUID = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()? as i8;
    }
    Some(out)
}

fn err(what: &str) -> ClapError {
    ClapError::Load(format!("VST3: {what}"))
}

// ---- モジュール(.vst3 の読み込み) ----

struct Module {
    _lib: libloading::Library,
    factory: ComPtr<IPluginFactory>,
}

// SAFETY: モジュールは読み込んだまま使い回す(解放しない)。ファクトリは作るときにだけ使い、
// プラグインの作成はメインスレッドで行う
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

/// バンドルの中の実体のファイル(ファイルが渡されたらそのまま)
pub fn bundle_binary(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_owned();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let contents = path.join("Contents");
    #[cfg(target_os = "windows")]
    let candidates = [contents.join("x86_64-win").join(format!("{stem}.vst3"))];
    #[cfg(target_os = "macos")]
    let candidates = [contents.join("MacOS").join(&stem)];
    #[cfg(all(unix, not(target_os = "macos")))]
    let candidates = [
        contents
            .join(format!("{}-linux", std::env::consts::ARCH))
            .join(format!("{stem}.so")),
        contents.join("x86_64-linux").join(format!("{stem}.so")),
    ];
    for c in &candidates {
        if c.is_file() {
            return c.clone();
        }
    }
    // 名前が違うときは、その場所にある最初の実体
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Ok(rd) = std::fs::read_dir(contents.join(format!("{}-linux", std::env::consts::ARCH))) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "so") {
                return e.path();
            }
        }
    }
    candidates[0].clone()
}

fn load_module(path: &Path) -> Result<Arc<Module>, ClapError> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Module>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = map.get(path) {
        return Ok(m.clone());
    }
    let bin = bundle_binary(path);
    // SAFETY: ユーザーが入れた VST3 のプラグインを読み込む(プラグインのコードを実行する)
    let lib = unsafe { load_library(&bin) }
        .map_err(|e| err(&format!("{} を読み込めません: {e}", bin.display())))?;
    // SAFETY: VST3 の決まりの入口(GetPluginFactory はファクトリを参照 1 つ付きで返す)
    let factory = unsafe {
        let get: libloading::Symbol<unsafe extern "system" fn() -> *mut IPluginFactory> = lib
            .get(b"GetPluginFactory\0")
            .map_err(|e| err(&format!("GetPluginFactory がありません: {e}")))?;
        ComPtr::from_raw(get()).ok_or_else(|| err("ファクトリを作れません"))?
    };
    let m = Arc::new(Module { _lib: lib, factory });
    map.insert(path.to_owned(), m.clone());
    Ok(m)
}

/// 実体を読み込み、OS ごとの初期化(Linux は ModuleEntry、Windows は InitDll)を呼ぶ
unsafe fn load_library(bin: &Path) -> Result<libloading::Library, String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let lib = libloading::os::unix::Library::open(
            Some(bin),
            libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_LOCAL,
        )
        .map_err(|e| e.to_string())?;
        let handle = lib.into_raw();
        let lib = libloading::os::unix::Library::from_raw(handle);
        if let Ok(entry) = lib.get::<unsafe extern "C" fn(*mut c_void) -> bool>(b"ModuleEntry\0") {
            if !entry(handle) {
                return Err("ModuleEntry が失敗しました".into());
            }
        }
        Ok(lib.into())
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        let lib = libloading::Library::new(bin).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
            init();
        }
        Ok(lib)
    }
}

/// `.vst3` 1 つに入っているプラグイン(音声のモジュール)の一覧。
pub fn describe(path: &Path) -> Result<Vec<PluginInfo>, ClapError> {
    let m = load_module(path)?;
    let f = &m.factory;
    // SAFETY: ファクトリの読み取りだけ(構造体は 0 埋めで渡す)
    unsafe {
        let mut finfo: PFactoryInfo = std::mem::zeroed();
        let factory_vendor = if f.getFactoryInfo(&mut finfo) == kResultOk {
            cstr_field(&finfo.vendor)
        } else {
            String::new()
        };
        let f2 = f.cast::<IPluginFactory2>();
        let mut out = Vec::new();
        for i in 0..f.countClasses() {
            let (cid, category, name, vendor, version, subs) = match &f2 {
                Some(f2) => {
                    let mut c: PClassInfo2 = std::mem::zeroed();
                    if f2.getClassInfo2(i, &mut c) != kResultOk {
                        continue;
                    }
                    (
                        c.cid,
                        cstr_field(&c.category),
                        cstr_field(&c.name),
                        cstr_field(&c.vendor),
                        cstr_field(&c.version),
                        cstr_field(&c.subCategories),
                    )
                }
                None => {
                    let mut c: PClassInfo = std::mem::zeroed();
                    if f.getClassInfo(i, &mut c) != kResultOk {
                        continue;
                    }
                    (
                        c.cid,
                        cstr_field(&c.category),
                        cstr_field(&c.name),
                        String::new(),
                        String::new(),
                        String::new(),
                    )
                }
            };
            if category != "Audio Module Class" {
                continue;
            }
            out.push(PluginInfo {
                id: format!("{}{}", crate::backend::VST3_PREFIX, hex_id(&cid)),
                name,
                vendor: if vendor.is_empty() {
                    factory_vendor.clone()
                } else {
                    vendor
                },
                version,
                path: path.to_owned(),
                features: features_of(&subs),
            });
        }
        Ok(out)
    }
}

/// VST3 の subCategories(`Instrument|Synth`・`Fx|Delay`)を CLAP の特徴タグに
fn features_of(subs: &str) -> Vec<String> {
    let parts: Vec<String> = subs
        .split('|')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = vec!["vst3".to_owned()];
    if parts.iter().any(|p| p.starts_with("instrument")) {
        out.push("instrument".into());
    }
    if parts.iter().any(|p| p == "fx" || p.starts_with("fx")) {
        out.push("audio-effect".into());
    }
    if !out.iter().any(|f| f == "instrument" || f == "audio-effect") {
        out.push("audio-effect".into());
    }
    for p in parts {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

// ---- ホストとプラグインの間で共有する状態 ----

/// 画面での操作をオーディオスレッドへ送る列(ロック無し。積むのはメインスレッド、取るのはオーディオスレッド)
struct ParamRing {
    ids: Box<[AtomicU32]>,
    values: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

const RING: usize = 1024;

impl ParamRing {
    fn new() -> Self {
        ParamRing {
            ids: (0..RING).map(|_| AtomicU32::new(0)).collect(),
            values: (0..RING).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn push(&self, id: u32, v: f64) {
        let h = self.head.load(Ordering::Relaxed);
        if h.wrapping_sub(self.tail.load(Ordering::Acquire)) >= RING {
            return;
        }
        self.ids[h % RING].store(id, Ordering::Relaxed);
        self.values[h % RING].store(v.to_bits(), Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
    }

    fn pop(&self) -> Option<(u32, f64)> {
        let t = self.tail.load(Ordering::Relaxed);
        if t == self.head.load(Ordering::Acquire) {
            return None;
        }
        let id = self.ids[t % RING].load(Ordering::Relaxed);
        let v = f64::from_bits(self.values[t % RING].load(Ordering::Relaxed));
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some((id, v))
    }
}

struct Shared {
    edits: ParamRing,
    dirty: AtomicBool,
    restart: AtomicBool,
    /// プラグインが頼んだ画面の大きさ(幅 << 32 | 高さ。0 = なし)
    requested_size: AtomicU64,
}

// ---- ホスト側の COM のオブジェクト ----

#[cfg(target_os = "linux")]
#[derive(Default)]
struct RunLoop {
    fds: Vec<(ComPtr<Linux::IEventHandler>, i32)>,
    timers: Vec<(
        ComPtr<Linux::ITimerHandler>,
        std::time::Duration,
        std::time::Instant,
    )>,
}

/// ホストの窓口(IHostApplication・IComponentHandler・IPlugFrame)
struct HostContext {
    shared: Arc<Shared>,
}

impl Class for HostContext {
    type Interfaces = (IHostApplication, IComponentHandler, IPlugFrame);
}

/// イベントループを回すスレッド(エンジンのプラグインのスレッド)で作ったインスタンスの画面の枠:
/// IPlugFrame に加えて Linux::IRunLoop を出し、プラグインが頼む fd とタイマーをそのスレッドで回す。
/// 回さないスレッドのインスタンスには出さない(出すと JUCE 製などが自前のメッセージのスレッドを使わず、処理が止まる)
#[cfg(target_os = "linux")]
struct LoopFrame {
    shared: Arc<Shared>,
    run_loop: Mutex<RunLoop>,
}

#[cfg(target_os = "linux")]
impl Class for LoopFrame {
    type Interfaces = (IPlugFrame, Linux::IRunLoop);
}

/// プラグインが頼んだ画面の大きさを覚える(実際の大きさは gui_tick で変える)
fn request_resize(shared: &Shared, new_size: *mut ViewRect) -> tresult {
    // SAFETY: プラグインが渡した矩形を読むだけ
    let Some(r) = (unsafe { new_size.as_ref() }) else {
        return kInvalidArgument;
    };
    let w = (r.right - r.left).max(1) as u64;
    let h = (r.bottom - r.top).max(1) as u64;
    shared
        .requested_size
        .store((w << 32) | h, Ordering::Release);
    kResultOk
}

#[cfg(target_os = "linux")]
impl IPlugFrameTrait for LoopFrame {
    unsafe fn resizeView(&self, _view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        request_resize(&self.shared, new_size)
    }
}

fn copy_wstring(src: &str, dst: &mut [TChar]) {
    let mut n = 0;
    for (d, s) in dst.iter_mut().zip(src.encode_utf16()) {
        *d = s;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n.min(dst.len().saturating_sub(1))) {
        *z = 0;
    }
}

/// 作ったオブジェクトから、頼まれたインターフェースを取り出して渡す
unsafe fn hand_out(unk: ComPtr<FUnknown>, iid: *const TUID, obj: *mut *mut c_void) -> tresult {
    let p = unk.as_ptr();
    ((*(*p).vtbl).queryInterface)(p, iid, obj)
}

impl IHostApplicationTrait for HostContext {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        copy_wstring("Glaux", &mut *name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        let cid = *(cid as *const vst3::com_scrape_types::Guid);
        if cid == IMessage::IID {
            if let Some(u) = ComWrapper::new(HostMessage::new()).to_com_ptr::<FUnknown>() {
                return hand_out(u, iid, obj);
            }
        } else if cid == IAttributeList::IID {
            if let Some(u) = ComWrapper::new(HostAttributes::default()).to_com_ptr::<FUnknown>() {
                return hand_out(u, iid, obj);
            }
        }
        *obj = std::ptr::null_mut();
        kResultFalse
    }
}

impl IComponentHandlerTrait for HostContext {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        self.shared.edits.push(id, value);
        self.shared.dirty.store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        self.shared.dirty.store(true, Ordering::Release);
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        let f = flags as u32;
        let restart = (RestartFlags_::kLatencyChanged
            | RestartFlags_::kReloadComponent
            | RestartFlags_::kIoChanged) as u32;
        if f & restart != 0 {
            self.shared.restart.store(true, Ordering::Release);
        }
        if f & RestartFlags_::kParamValuesChanged as u32 != 0 {
            self.shared.dirty.store(true, Ordering::Release);
        }
        kResultOk
    }
}

impl IPlugFrameTrait for HostContext {
    unsafe fn resizeView(&self, _view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        request_resize(&self.shared, new_size)
    }
}

#[cfg(target_os = "linux")]
impl Linux::IRunLoopTrait for LoopFrame {
    unsafe fn registerEventHandler(
        &self,
        handler: *mut Linux::IEventHandler,
        fd: Linux::FileDescriptor,
    ) -> tresult {
        let Some(h) = ComRef::from_raw(handler) else {
            return kInvalidArgument;
        };
        let mut rl = self.run_loop.lock().unwrap_or_else(|e| e.into_inner());
        rl.fds.push((h.to_com_ptr(), fd));
        kResultOk
    }

    unsafe fn unregisterEventHandler(&self, handler: *mut Linux::IEventHandler) -> tresult {
        let mut rl = self.run_loop.lock().unwrap_or_else(|e| e.into_inner());
        rl.fds.retain(|(h, _)| h.as_ptr() != handler);
        kResultOk
    }

    unsafe fn registerTimer(
        &self,
        handler: *mut Linux::ITimerHandler,
        milliseconds: Linux::TimerInterval,
    ) -> tresult {
        let Some(h) = ComRef::from_raw(handler) else {
            return kInvalidArgument;
        };
        let period = std::time::Duration::from_millis(milliseconds.max(1));
        let mut rl = self.run_loop.lock().unwrap_or_else(|e| e.into_inner());
        rl.timers
            .push((h.to_com_ptr(), period, std::time::Instant::now() + period));
        kResultOk
    }

    unsafe fn unregisterTimer(&self, handler: *mut Linux::ITimerHandler) -> tresult {
        let mut rl = self.run_loop.lock().unwrap_or_else(|e| e.into_inner());
        rl.timers.retain(|(h, _, _)| h.as_ptr() != handler);
        kResultOk
    }
}

/// プラグインどうしのやりとり(IConnectionPoint)に使う伝言(IMessage)
struct HostMessage {
    id: Mutex<CString>,
    attrs: ComWrapper<HostAttributes>,
}

impl HostMessage {
    fn new() -> Self {
        HostMessage {
            id: Mutex::new(CString::default()),
            attrs: ComWrapper::new(HostAttributes::default()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.lock().unwrap_or_else(|e| e.into_inner()).as_ptr()
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        if !id.is_null() {
            *self.id.lock().unwrap_or_else(|e| e.into_inner()) = CStr::from_ptr(id).to_owned();
        }
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attrs
            .as_com_ref::<IAttributeList>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<u16>),
    Bin(Vec<u8>),
}

#[derive(Default)]
struct HostAttributes {
    map: Mutex<HashMap<CString, Attr>>,
}

impl Class for HostAttributes {
    type Interfaces = (IAttributeList,);
}

unsafe fn attr_key(id: IAttributeList_::AttrID) -> Option<CString> {
    (!id.is_null()).then(|| CStr::from_ptr(id).to_owned())
}

impl IAttributeListTrait for HostAttributes {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: int64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(k, Attr::Int(value));
        kResultOk
    }

    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut int64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.map.lock().unwrap_or_else(|e| e.into_inner()).get(&k) {
            Some(Attr::Int(v)) => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(k, Attr::Float(value));
        kResultOk
    }

    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.map.lock().unwrap_or_else(|e| e.into_inner()).get(&k) {
            Some(Attr::Float(v)) => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut v = Vec::new();
        let mut i = 0;
        while *string.add(i) != 0 {
            v.push(*string.add(i));
            i += 1;
        }
        v.push(0);
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(k, Attr::Str(v));
        kResultOk
    }

    unsafe fn getString(
        &self,
        id: IAttributeList_::AttrID,
        string: *mut TChar,
        size_in_bytes: uint32,
    ) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.map.lock().unwrap_or_else(|e| e.into_inner()).get(&k) {
            Some(Attr::Str(v)) => {
                let n = (size_in_bytes as usize / 2).min(v.len());
                if n == 0 {
                    return kResultFalse;
                }
                std::ptr::copy_nonoverlapping(v.as_ptr(), string, n);
                *string.add(n - 1) = 0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *const c_void,
        size_in_bytes: uint32,
    ) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        let bytes = if data.is_null() {
            vec![]
        } else {
            std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec()
        };
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(k, Attr::Bin(bytes));
        kResultOk
    }

    unsafe fn getBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.map.lock().unwrap_or_else(|e| e.into_inner()).get(&k) {
            // 中身は属性の一覧が生きている間そのまま(差し替えるまで動かない)
            Some(Attr::Bin(v)) => {
                *data = v.as_ptr() as *const c_void;
                *size_in_bytes = v.len() as u32;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

/// 状態の読み書きに使うメモリの流れ(IBStream)
#[derive(Default)]
struct MemStream {
    data: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_read: *mut int32) -> tresult {
        let data = self.data.borrow();
        let pos = self.pos.get().min(data.len());
        let n = (num_bytes.max(0) as usize).min(data.len() - pos);
        std::ptr::copy_nonoverlapping(data.as_ptr().add(pos), buffer as *mut u8, n);
        self.pos.set(pos + n);
        if !num_read.is_null() {
            *num_read = n as int32;
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_written: *mut int32,
    ) -> tresult {
        let n = num_bytes.max(0) as usize;
        let mut data = self.data.borrow_mut();
        let pos = self.pos.get();
        if data.len() < pos + n {
            data.resize(pos + n, 0);
        }
        std::ptr::copy_nonoverlapping(buffer as *const u8, data.as_mut_ptr().add(pos), n);
        self.pos.set(pos + n);
        if !num_written.is_null() {
            *num_written = n as int32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let len = self.data.borrow().len() as i64;
        let base = match mode as u32 {
            m if m == IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => self.pos.get() as i64,
            m if m == IBStream_::IStreamSeekMode_::kIBSeekEnd as u32 => len,
            _ => 0,
        };
        let p = (base + pos).max(0);
        self.pos.set(p as usize);
        if !result.is_null() {
            *result = p;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            *pos = self.pos.get() as int64;
        }
        kResultOk
    }
}

fn stream_with(bytes: &[u8]) -> ComWrapper<MemStream> {
    ComWrapper::new(MemStream {
        data: RefCell::new(bytes.to_vec()),
        pos: Cell::new(0),
    })
}

fn stream_ptr(s: &ComWrapper<MemStream>) -> *mut IBStream {
    s.as_com_ref::<IBStream>()
        .map_or(std::ptr::null_mut(), |r| r.as_ptr())
}

// ---- 処理に渡すイベント列・パラメータの列(オーディオスレッドで確保しない) ----

struct EventList {
    events: RefCell<Vec<Event>>,
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        self.events.borrow().len() as int32
    }

    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match self.events.borrow().get(index.max(0) as usize) {
            Some(ev) if !e.is_null() => {
                *e = *ev;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        let mut v = self.events.borrow_mut();
        if e.is_null() || v.len() >= v.capacity() {
            return kResultFalse;
        }
        v.push(*e);
        kResultOk
    }
}

const MAX_PARAM_QUEUES: usize = 64;
const MAX_POINTS: usize = 64;

struct ParamQueue {
    id: Cell<u32>,
    points: RefCell<Vec<(i32, f64)>>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.points.borrow().len() as int32
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        match self.points.borrow().get(index.max(0) as usize) {
            Some((o, v)) => {
                *offset = *o;
                *value = *v;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    unsafe fn addPoint(&self, offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        let mut p = self.points.borrow_mut();
        if p.len() >= MAX_POINTS {
            return kResultFalse;
        }
        // 位置の順に並べる(同じ位置なら後の値で上書き)
        match p.iter().position(|(o, _)| *o >= offset) {
            Some(i) if p[i].0 == offset => p[i].1 = value,
            Some(i) => p.insert(i, (offset, value)),
            None => p.push((offset, value)),
        }
        if !index.is_null() {
            *index = (p.len() - 1) as int32;
        }
        kResultOk
    }
}

struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    used: Cell<usize>,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl ParamChanges {
    fn new() -> Self {
        ParamChanges {
            queues: (0..MAX_PARAM_QUEUES)
                .map(|_| {
                    ComWrapper::new(ParamQueue {
                        id: Cell::new(0),
                        points: RefCell::new(Vec::with_capacity(MAX_POINTS)),
                    })
                })
                .collect(),
            used: Cell::new(0),
        }
    }

    fn clear(&self) {
        for q in &self.queues[..self.used.get()] {
            q.points.borrow_mut().clear();
        }
        self.used.set(0);
    }

    fn add(&self, id: u32, offset: i32, value: f64) {
        let n = self.used.get();
        let q = match self.queues[..n].iter().find(|q| q.id.get() == id) {
            Some(q) => q,
            None if n < self.queues.len() => {
                self.queues[n].id.set(id);
                self.used.set(n + 1);
                &self.queues[n]
            }
            None => return,
        };
        // SAFETY: 自分のキューに点を足すだけ
        unsafe {
            q.addPoint(offset, value.clamp(0.0, 1.0), std::ptr::null_mut());
        }
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.used.get() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if index < 0 || index as usize >= self.used.get() {
            return std::ptr::null_mut();
        }
        self.queues[index as usize]
            .as_com_ref::<IParamValueQueue>()
            .map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }

    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if id.is_null() {
            return std::ptr::null_mut();
        }
        let n = self.used.get();
        let i = match self.queues[..n].iter().position(|q| q.id.get() == *id) {
            Some(i) => i,
            None if n < self.queues.len() => {
                self.queues[n].id.set(*id);
                self.used.set(n + 1);
                n
            }
            None => return std::ptr::null_mut(),
        };
        if !index.is_null() {
            *index = i as int32;
        }
        self.getParameterData(i as int32)
    }
}

// ---- プラグイン(メインスレッド側) ----

/// バスの構成(各バスのチャンネル数と、メインのバス)
#[derive(Clone)]
struct Layout {
    ins: Vec<u32>,
    outs: Vec<u32>,
    main_in: Option<usize>,
    main_out: usize,
    events: bool,
}

pub struct Vst3Plugin {
    _module: Arc<Module>,
    /// プラグインが参照する窓口(画面の枠・Linux の run loop にも使う)
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    host: ComWrapper<HostContext>,
    shared: Arc<Shared>,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// 処理と編集が別のオブジェクトのとき、互いにつないだ点
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    separate_controller: bool,
    /// イベントループのスレッドで作ったときの画面の枠(Linux::IRunLoop を出す)
    #[cfg(target_os = "linux")]
    loop_frame: Option<ComWrapper<LoopFrame>>,
    active: bool,
    view: Option<ComPtr<IPlugView>>,
    #[cfg(windows)]
    window: Option<crate::window::HostWindow>,
    #[cfg(target_os = "linux")]
    window: Option<crate::window_x11::HostWindow>,
    gui_open: bool,
}

impl Vst3Plugin {
    pub fn new(path: &Path, id: &str) -> Result<Self, ClapError> {
        crate::host::mark_main_thread();
        let module = load_module(path)?;
        let cid = parse_id(id).ok_or_else(|| err(&format!("ID の形が違います: {id}")))?;
        let shared = Arc::new(Shared {
            edits: ParamRing::new(),
            dirty: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            requested_size: AtomicU64::new(0),
        });
        let host = ComWrapper::new(HostContext {
            shared: shared.clone(),
        });
        #[cfg(target_os = "linux")]
        let loop_frame = crate::host::runs_event_loop().then(|| {
            ComWrapper::new(LoopFrame {
                shared: shared.clone(),
                run_loop: Mutex::new(RunLoop::default()),
            })
        });
        let host_unk = host
            .as_com_ref::<FUnknown>()
            .ok_or_else(|| err("ホストの窓口を作れません"))?
            .as_ptr();
        // SAFETY: VST3 の生成の手順(参照の数は ComPtr が持つ)
        unsafe {
            let component: ComPtr<IComponent> =
                create(&module.factory, &cid).ok_or_else(|| err("音声の部分を作れません"))?;
            if component.initialize(host_unk) != kResultOk {
                return Err(err("音声の部分を初期化できません"));
            }
            let processor = component
                .cast::<IAudioProcessor>()
                .ok_or_else(|| err("IAudioProcessor がありません"))?;
            // 編集の部分: 同じオブジェクトか、別のクラス
            let mut separate = false;
            let controller = match component.cast::<IEditController>() {
                Some(c) => Some(c),
                None => {
                    let mut ccid: TUID = [0; 16];
                    if component.getControllerClassId(&mut ccid) == kResultOk {
                        match create::<IEditController>(&module.factory, &ccid) {
                            Some(c) if c.initialize(host_unk) == kResultOk => {
                                separate = true;
                                Some(c)
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
            };
            let mut connection = None;
            if let Some(ctrl) = &controller {
                if separate {
                    if let (Some(a), Some(b)) = (
                        component.cast::<IConnectionPoint>(),
                        ctrl.cast::<IConnectionPoint>(),
                    ) {
                        a.connect(b.as_ptr());
                        b.connect(a.as_ptr());
                        connection = Some((a, b));
                    }
                }
                if let Some(h) = host.as_com_ref::<IComponentHandler>() {
                    ctrl.setComponentHandler(h.as_ptr());
                }
                // 編集の部分に音声の部分の状態を伝える
                if separate {
                    let s = ComWrapper::new(MemStream::default());
                    if component.getState(stream_ptr(&s)) == kResultOk {
                        s.pos.set(0);
                        ctrl.setComponentState(stream_ptr(&s));
                    }
                }
            }
            Ok(Vst3Plugin {
                _module: module,
                host,
                shared,
                component,
                processor,
                controller,
                connection,
                separate_controller: separate,
                #[cfg(target_os = "linux")]
                loop_frame,
                active: false,
                view: None,
                #[cfg(any(windows, target_os = "linux"))]
                window: None,
                gui_open: false,
            })
        }
    }

    /// バスを調べ、メインのバスをステレオにする
    unsafe fn setup_buses(&self) -> Layout {
        let c = &self.component;
        let count = |m: u32, d: u32| c.getBusCount(m as MediaType, d as BusDirection).max(0);
        let info = |m: u32, d: u32, i: i32| {
            let mut b: BusInfo = std::mem::zeroed();
            (c.getBusInfo(m as MediaType, d as BusDirection, i, &mut b) == kResultOk).then_some(b)
        };
        let (audio, input, output) = (
            MediaTypes_::kAudio as u32,
            BusDirections_::kInput as u32,
            BusDirections_::kOutput as u32,
        );
        let n_in = count(audio, input);
        let n_out = count(audio, output);
        let main = BusTypes_::kMain as i32;
        let is_main = |b: &Option<BusInfo>| b.as_ref().is_some_and(|b| b.busType == main);
        let in_infos: Vec<Option<BusInfo>> = (0..n_in).map(|i| info(audio, input, i)).collect();
        let out_infos: Vec<Option<BusInfo>> = (0..n_out).map(|i| info(audio, output, i)).collect();
        let main_in = in_infos.iter().position(is_main);
        let main_out = out_infos.iter().position(is_main).unwrap_or(0);
        // メインをステレオ、ほかはプラグインの既定のまま
        let arr = |d: u32, i: i32| {
            let mut a: SpeakerArrangement = 0;
            c.cast::<IAudioProcessor>()
                .map(|p| p.getBusArrangement(d as BusDirection, i, &mut a))
                .filter(|r| *r == kResultOk)
                .map(|_| a)
                .unwrap_or(SpeakerArr::kStereo)
        };
        let mut ins: Vec<SpeakerArrangement> = (0..n_in).map(|i| arr(input, i)).collect();
        let mut outs: Vec<SpeakerArrangement> = (0..n_out).map(|i| arr(output, i)).collect();
        if let Some(m) = main_in {
            ins[m] = SpeakerArr::kStereo;
        }
        if let Some(o) = outs.get_mut(main_out) {
            *o = SpeakerArr::kStereo;
        }
        if self
            .processor
            .setBusArrangements(ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), n_out)
            != kResultOk
        {
            // 断られたらプラグインの既定のまま
            ins = (0..n_in).map(|i| arr(input, i)).collect();
            outs = (0..n_out).map(|i| arr(output, i)).collect();
        }
        let channels = |a: SpeakerArrangement| a.count_ones().max(1);
        // メインのバスと、既定で有効なバスを有効にする
        let default_active = BusInfo_::BusFlags_::kDefaultActive as u32;
        for (i, b) in in_infos.iter().enumerate() {
            let on =
                Some(i) == main_in || b.as_ref().is_some_and(|b| b.flags & default_active != 0);
            c.activateBus(
                audio as MediaType,
                input as BusDirection,
                i as i32,
                on as TBool,
            );
        }
        for (i, b) in out_infos.iter().enumerate() {
            let on = i == main_out || b.as_ref().is_some_and(|b| b.flags & default_active != 0);
            c.activateBus(
                audio as MediaType,
                output as BusDirection,
                i as i32,
                on as TBool,
            );
        }
        let events = count(MediaTypes_::kEvent as u32, input) > 0;
        if events {
            c.activateBus(
                MediaTypes_::kEvent as MediaType,
                input as BusDirection,
                0,
                1,
            );
        }
        Layout {
            ins: ins.into_iter().map(channels).collect(),
            outs: outs.into_iter().map(channels).collect(),
            main_in,
            main_out,
            events,
        }
    }

    pub fn activate(&mut self, sample_rate: f64) -> Result<Vst3Processor, ClapError> {
        // SAFETY: 起動の手順(バス → setupProcessing → setActive)
        unsafe {
            let layout = self.setup_buses();
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                maxSamplesPerBlock: MAX_FRAMES as int32,
                sampleRate: sample_rate,
            };
            if self.processor.setupProcessing(&mut setup) != kResultOk {
                return Err(ClapError::Activate(
                    "VST3: setupProcessing が失敗しました".into(),
                ));
            }
            if self.component.setActive(1) != kResultOk {
                return Err(ClapError::Activate("VST3: setActive が失敗しました".into()));
            }
            self.active = true;
            let latency = self.processor.getLatencySamples();
            // MIDI のコントローラー(ペダル・ベンドなど)が割り当てられたパラメータ
            let mut midi_map = [u32::MAX; 130];
            if let Some(mm) = self
                .controller
                .as_ref()
                .and_then(|c| c.cast::<IMidiMapping>())
            {
                for (cc, slot) in midi_map.iter_mut().enumerate() {
                    let mut pid: ParamID = 0;
                    if mm.getMidiControllerAssignment(0, 0, cc as CtrlNumber, &mut pid) == kResultOk
                    {
                        *slot = pid;
                    }
                }
            }
            Ok(Vst3Processor::new(
                self.processor.clone(),
                self.shared.clone(),
                layout,
                sample_rate,
                latency,
                midi_map,
            ))
        }
    }

    pub fn deactivate(&mut self, mut processor: Vst3Processor) {
        processor.stop();
        drop(processor);
        if self.active {
            // SAFETY: 起動していたものを止める
            unsafe {
                self.component.setActive(0);
            }
            self.active = false;
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn param_infos(&mut self) -> Vec<ParamInfo> {
        let Some(c) = &self.controller else {
            return vec![];
        };
        // SAFETY: 読み取りだけ
        unsafe {
            (0..c.getParameterCount())
                .filter_map(|i| {
                    let mut p: ParameterInfo = std::mem::zeroed();
                    if c.getParameterInfo(i, &mut p) != kResultOk {
                        return None;
                    }
                    let flags = p.flags as u32;
                    let has = |f: ParameterInfo_::ParameterFlags| flags & f as u32 != 0;
                    Some(ParamInfo {
                        id: p.id,
                        name: wstr(&p.title),
                        module: String::new(),
                        min: 0.0,
                        max: 1.0,
                        default: p.defaultNormalizedValue,
                        stepped: p.stepCount > 0,
                        automatable: has(ParameterInfo_::ParameterFlags_::kCanAutomate),
                        modulatable: false,
                        per_note: false,
                        hidden: has(ParameterInfo_::ParameterFlags_::kIsHidden),
                        readonly: has(ParameterInfo_::ParameterFlags_::kIsReadOnly),
                    })
                })
                .collect()
        }
    }

    pub fn param_values(&mut self, ids: &[u32]) -> Vec<(u32, f64, String)> {
        let Some(c) = &self.controller else {
            return vec![];
        };
        // SAFETY: 読み取りだけ
        unsafe {
            ids.iter()
                .map(|&id| {
                    let v = c.getParamNormalized(id);
                    let mut s: String128 = [0; 128];
                    let text = if c.getParamStringByValue(id, v, &mut s) == kResultOk {
                        wstr(&s)
                    } else {
                        format!("{v:.3}")
                    };
                    (id, v, text)
                })
                .collect()
        }
    }

    /// 状態: `GV3S` + 音声の部分の長さ(u32 LE)+ 中身 + 編集の部分の長さ + 中身
    pub fn save_state(&mut self) -> Result<Vec<u8>, ClapError> {
        // SAFETY: 自前の流れに書いてもらう
        unsafe {
            let comp = ComWrapper::new(MemStream::default());
            if self.component.getState(stream_ptr(&comp)) != kResultOk {
                return Err(ClapError::State("VST3: 状態を読めません".into()));
            }
            let ctrl = ComWrapper::new(MemStream::default());
            if let Some(c) = &self.controller {
                c.getState(stream_ptr(&ctrl));
            }
            let (a, b) = (comp.data.borrow(), ctrl.data.borrow());
            let mut out = b"GV3S".to_vec();
            out.extend_from_slice(&(a.len() as u32).to_le_bytes());
            out.extend_from_slice(&a);
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(&b);
            self.shared.dirty.store(false, Ordering::Release);
            Ok(out)
        }
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), ClapError> {
        let (comp, ctrl) = split_state(bytes);
        // SAFETY: 自前の流れから読んでもらう
        unsafe {
            let s = stream_with(comp);
            if self.component.setState(stream_ptr(&s)) != kResultOk {
                return Err(ClapError::State("VST3: 状態を戻せません".into()));
            }
            if let Some(c) = &self.controller {
                if self.separate_controller {
                    let s = stream_with(comp);
                    c.setComponentState(stream_ptr(&s));
                }
                if !ctrl.is_empty() {
                    let s = stream_with(ctrl);
                    c.setState(stream_ptr(&s));
                }
            }
        }
        self.shared.dirty.store(false, Ordering::Release);
        Ok(())
    }

    pub fn take_restart_requested(&self) -> bool {
        self.shared.restart.swap(false, Ordering::AcqRel)
    }

    pub fn take_dirty(&mut self) -> bool {
        self.shared.dirty.swap(false, Ordering::AcqRel)
    }

    pub fn has_gui(&self) -> bool {
        self.controller.is_some()
    }

    pub fn is_gui_open(&self) -> bool {
        self.gui_open
    }

    pub fn open_gui(&mut self, title: &str) -> Result<(), ClapError> {
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(w) = &self.window {
            w.show();
            return Ok(());
        }
        if self.gui_open {
            return Ok(());
        }
        let ctrl = self
            .controller
            .clone()
            .ok_or_else(|| ClapError::Gui("このプラグインには画面がありません".into()))?;
        self.open_gui_platform(&ctrl, title)?;
        self.gui_open = true;
        Ok(())
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn open_gui_platform(
        &mut self,
        ctrl: &ComPtr<IEditController>,
        title: &str,
    ) -> Result<(), ClapError> {
        #[cfg(target_os = "linux")]
        let kind = kPlatformTypeX11EmbedWindowID;
        #[cfg(windows)]
        let kind = kPlatformTypeHWND;
        // SAFETY: 画面の手順(作る → 枠を渡す → 親の窓に付ける)。窓は画面を外すまで生かす
        unsafe {
            let view = ComPtr::from_raw(ctrl.createView(vst3::Steinberg::Vst::ViewType::kEditor))
                .ok_or_else(|| ClapError::Gui("画面を作れません".into()))?;
            if view.isPlatformTypeSupported(kind) != kResultOk {
                return Err(ClapError::Gui(
                    "この OS の画面に対応していないプラグインです".into(),
                ));
            }
            let mut rect: ViewRect = std::mem::zeroed();
            let (w, h) = if view.getSize(&mut rect) == kResultOk {
                (
                    (rect.right - rect.left).max(1) as u32,
                    (rect.bottom - rect.top).max(1) as u32,
                )
            } else {
                (800, 500)
            };
            let resizable = view.canResize() == kResultOk;
            #[cfg(target_os = "linux")]
            let window = crate::window_x11::HostWindow::new(title, w, h, resizable)
                .map_err(ClapError::Gui)?;
            #[cfg(windows)]
            let window =
                crate::window::HostWindow::new(title, w, h, resizable).map_err(ClapError::Gui)?;
            #[cfg(target_os = "linux")]
            let frame = match &self.loop_frame {
                Some(l) => l.as_com_ref::<IPlugFrame>(),
                None => self.host.as_com_ref::<IPlugFrame>(),
            };
            #[cfg(not(target_os = "linux"))]
            let frame = self.host.as_com_ref::<IPlugFrame>();
            if let Some(f) = frame {
                view.setFrame(f.as_ptr());
            }
            #[cfg(target_os = "linux")]
            let parent = window.id() as *mut c_void;
            #[cfg(windows)]
            let parent = window.hwnd() as *mut c_void;
            if view.attached(parent, kind) != kResultOk {
                view.setFrame(std::ptr::null_mut());
                return Err(ClapError::Gui("画面を窓に付けられません".into()));
            }
            window.show();
            self.view = Some(view);
            self.window = Some(window);
        }
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn open_gui_platform(
        &mut self,
        _ctrl: &ComPtr<IEditController>,
        _title: &str,
    ) -> Result<(), ClapError> {
        Err(ClapError::Gui(
            "この OS ではまだ VST3 の画面を開けません(Windows と Linux のみ対応)".into(),
        ))
    }

    #[cfg(target_os = "linux")]
    pub fn capture_gui_for_test(&self) -> Option<(u32, u32, Vec<u8>)> {
        self.window.as_ref().and_then(|w| w.capture_rgb())
    }

    pub fn close_gui(&mut self) {
        if let Some(v) = self.view.take() {
            // SAFETY: 付けた画面を外してから手放す
            unsafe {
                v.removed();
                v.setFrame(std::ptr::null_mut());
            }
        }
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.window = None;
        }
        self.gui_open = false;
    }

    pub fn gui_tick(&mut self) -> GuiEvent {
        if !self.gui_open {
            return GuiEvent::None;
        }
        #[cfg(any(windows, target_os = "linux"))]
        {
            #[cfg(target_os = "linux")]
            let closed = self.window.as_mut().is_some_and(|w| {
                w.pump();
                w.take_close_requested()
            });
            #[cfg(windows)]
            let closed = self
                .window
                .as_ref()
                .is_some_and(|w| w.take_close_requested());
            if closed {
                self.close_gui();
                return GuiEvent::Closed;
            }
            let req = self.shared.requested_size.swap(0, Ordering::AcqRel);
            let mut new_size = None;
            if req != 0 {
                let (w, h) = ((req >> 32) as u32, (req & 0xFFFF_FFFF) as u32);
                if let Some(win) = self.window.as_mut() {
                    win.set_client_size(w, h);
                }
                new_size = Some((w, h));
            } else if let Some(s) = self.window.as_mut().and_then(|w| w.take_resized()) {
                new_size = Some(s);
            }
            if let (Some((w, h)), Some(view)) = (new_size, &self.view) {
                // SAFETY: 付いている画面に大きさを伝える
                unsafe {
                    let mut r = ViewRect {
                        left: 0,
                        top: 0,
                        right: w as i32,
                        bottom: h as i32,
                    };
                    view.checkSizeConstraint(&mut r);
                    view.onSize(&mut r);
                }
            }
        }
        GuiEvent::None
    }

    /// プラグインが頼んだタイマーと fd の見張り(Linux)を回す
    pub fn poll(&mut self) {
        #[cfg(target_os = "linux")]
        {
            use Linux::{IEventHandlerTrait, ITimerHandlerTrait};
            let Some(frame) = &self.loop_frame else {
                return;
            };
            let now = std::time::Instant::now();
            let (due, fds) = {
                let mut rl = frame.run_loop.lock().unwrap_or_else(|e| e.into_inner());
                let mut due = Vec::new();
                for (h, period, at) in rl.timers.iter_mut() {
                    if *at <= now {
                        due.push(h.clone());
                        *at = (*at + *period).max(now);
                    }
                }
                let fds: Vec<(ComPtr<Linux::IEventHandler>, i32)> = rl.fds.clone();
                (due, fds)
            };
            // SAFETY: プラグインのコールバックをメインスレッドで呼ぶ(ロックは外してから)
            unsafe {
                for h in due {
                    h.onTimer();
                }
                if !fds.is_empty() {
                    let mut polls: Vec<libc::pollfd> = fds
                        .iter()
                        .map(|(_, fd)| libc::pollfd {
                            fd: *fd,
                            events: libc::POLLIN,
                            revents: 0,
                        })
                        .collect();
                    if libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 0) > 0 {
                        for (p, (h, fd)) in polls.iter().zip(&fds) {
                            if p.revents != 0 {
                                h.onFDIsSet(*fd);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn split_state(bytes: &[u8]) -> (&[u8], &[u8]) {
    let read = |b: &[u8], at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    if bytes.starts_with(b"GV3S") {
        if let Some(a) = read(bytes, 4) {
            if let Some(comp) = bytes.get(8..8 + a) {
                let rest = &bytes[8 + a..];
                let ctrl = read(rest, 0)
                    .and_then(|b| rest.get(4..4 + b))
                    .unwrap_or(&[]);
                return (comp, ctrl);
            }
        }
    }
    (bytes, &[])
}

/// ファクトリからクラスを作る
unsafe fn create<I: Interface>(f: &ComPtr<IPluginFactory>, cid: &TUID) -> Option<ComPtr<I>> {
    let mut obj: *mut c_void = std::ptr::null_mut();
    let r = f.createInstance(
        cid.as_ptr() as FIDString,
        I::IID.as_ptr() as FIDString,
        &mut obj,
    );
    if r != kResultOk {
        return None;
    }
    ComPtr::from_raw(obj as *mut I)
}

impl Drop for Vst3Plugin {
    fn drop(&mut self) {
        self.close_gui();
        // SAFETY: 作ったときの逆の順で片付ける
        unsafe {
            if self.active {
                self.component.setActive(0);
            }
            if let Some((a, b)) = &self.connection {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(std::ptr::null_mut());
                if self.separate_controller {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

// ---- 処理窓口(オーディオスレッド側) ----

pub struct Vst3Processor {
    processor: ComPtr<IAudioProcessor>,
    shared: Arc<Shared>,
    layout: Layout,
    /// [バス][チャンネル][フレーム](起動時に確保)
    in_bufs: Vec<Vec<Vec<f32>>>,
    out_bufs: Vec<Vec<Vec<f32>>>,
    in_ptrs: Vec<Vec<*mut f32>>,
    out_ptrs: Vec<Vec<*mut f32>>,
    in_bus: Vec<AudioBusBuffers>,
    out_bus: Vec<AudioBusBuffers>,
    events: ComWrapper<EventList>,
    params: ComWrapper<ParamChanges>,
    context: ProcessContext,
    transport: Option<HostTransport>,
    sample_rate: f64,
    latency: u32,
    steady: u64,
    processing: bool,
    failed: bool,
    /// 鳴らしている鍵盤(すべて離すときに使う)
    held: [bool; 128],
    /// MIDI のコントローラー番号(0〜127、128 = アフタータッチ、129 = ピッチベンド)→ パラメータ(u32::MAX = なし)
    midi_map: [u32; 130],
}

// SAFETY: 処理窓口はオーディオスレッドへ渡して 1 つのスレッドからだけ使う(COM の参照の数は原子的に数える)
unsafe impl Send for Vst3Processor {}

impl Vst3Processor {
    fn new(
        processor: ComPtr<IAudioProcessor>,
        shared: Arc<Shared>,
        layout: Layout,
        sample_rate: f64,
        latency: u32,
        midi_map: [u32; 130],
    ) -> Self {
        let alloc = |chs: &[u32]| -> Vec<Vec<Vec<f32>>> {
            chs.iter()
                .map(|&c| (0..c).map(|_| vec![0.0f32; MAX_FRAMES]).collect())
                .collect()
        };
        let mut in_bufs = alloc(&layout.ins);
        let mut out_bufs = alloc(&layout.outs);
        let ptrs = |b: &mut Vec<Vec<Vec<f32>>>| -> Vec<Vec<*mut f32>> {
            b.iter_mut()
                .map(|bus| bus.iter_mut().map(|c| c.as_mut_ptr()).collect())
                .collect()
        };
        let mut in_ptrs = ptrs(&mut in_bufs);
        let mut out_ptrs = ptrs(&mut out_bufs);
        let bus = |p: &mut Vec<Vec<*mut f32>>| -> Vec<AudioBusBuffers> {
            p.iter_mut()
                .map(|chs| AudioBusBuffers {
                    numChannels: chs.len() as int32,
                    silenceFlags: 0,
                    __field0: AudioBusBuffers__type0 {
                        channelBuffers32: chs.as_mut_ptr(),
                    },
                })
                .collect()
        };
        let in_bus = bus(&mut in_ptrs);
        let out_bus = bus(&mut out_ptrs);
        // SAFETY: ProcessContext は C の構造体で、0 埋めが有効な初期値
        let context: ProcessContext = unsafe { std::mem::zeroed() };
        Vst3Processor {
            processor,
            shared,
            layout,
            in_bufs,
            out_bufs,
            in_ptrs,
            out_ptrs,
            in_bus,
            out_bus,
            events: ComWrapper::new(EventList {
                events: RefCell::new(Vec::with_capacity(MAX_EVENTS)),
            }),
            params: ComWrapper::new(ParamChanges::new()),
            context,
            transport: None,
            sample_rate,
            latency,
            steady: 0,
            processing: false,
            failed: false,
            held: [false; 128],
            midi_map,
        }
    }

    pub fn set_transport(&mut self, t: Option<HostTransport>) {
        self.transport = t;
    }

    fn push_event(&self, e: Event) {
        let mut v = self.events.events.borrow_mut();
        if v.len() < v.capacity() {
            v.push(e);
        }
    }

    fn note_event(offset: i32, kind: Event_::EventTypes, field: Event__type0) -> Event {
        Event {
            busIndex: 0,
            sampleOffset: offset,
            ppqPosition: 0.0,
            flags: Event_::EventFlags_::kIsLive as u16,
            r#type: kind as u16,
            __field0: field,
        }
    }

    fn expression(&self, offset: i32, type_id: NoteExpressionTypeIDs, note_id: u32, value: f64) {
        self.push_event(Self::note_event(
            offset,
            Event_::EventTypes_::kNoteExpressionValueEvent,
            Event__type0 {
                noteExpressionValue: NoteExpressionValueEvent {
                    typeId: type_id as NoteExpressionTypeID,
                    noteId: note_id as i32,
                    value: value.clamp(0.0, 1.0),
                },
            },
        ));
    }

    fn note_off(&mut self, offset: i32, key: u8) {
        self.held[key as usize & 0x7F] = false;
        self.push_event(Self::note_event(
            offset,
            Event_::EventTypes_::kNoteOffEvent,
            Event__type0 {
                noteOff: NoteOffEvent {
                    channel: 0,
                    pitch: key as i16,
                    velocity: 0.0,
                    noteId: -1,
                    tuning: 0.0,
                },
            },
        ));
    }

    /// ノート・パラメータを積み、ブロックをまとめて処理する(確保しない)
    pub fn process(&mut self, frames: usize, notes: &[NoteMsg]) {
        crate::host::mark_audio_thread();
        let frames = frames.min(MAX_FRAMES);
        if self.failed {
            self.clear_outputs(frames);
            return;
        }
        // SAFETY: 起動済みのプラグインの処理を、自前で確保した列とバッファで呼ぶ
        unsafe {
            if !self.processing {
                self.processor.setProcessing(1);
                self.processing = true;
            }
            self.events.events.borrow_mut().clear();
            self.params.clear();
            // 画面での操作
            while let Some((id, v)) = self.shared.edits.pop() {
                self.params.add(id, 0, v);
            }
            for msg in notes {
                let t = (msg.time() as usize).min(frames.saturating_sub(1)) as i32;
                match *msg {
                    NoteMsg::On {
                        key,
                        velocity,
                        note_id,
                        ..
                    } => {
                        self.held[key as usize & 0x7F] = true;
                        self.push_event(Self::note_event(
                            t,
                            Event_::EventTypes_::kNoteOnEvent,
                            Event__type0 {
                                noteOn: NoteOnEvent {
                                    channel: 0,
                                    pitch: key as i16,
                                    tuning: 0.0,
                                    velocity: velocity as f32,
                                    length: 0,
                                    noteId: note_id.map_or(-1, |i| i as i32),
                                },
                            },
                        ));
                    }
                    NoteMsg::Off { key, .. } | NoteMsg::Choke { key, .. } => self.note_off(t, key),
                    NoteMsg::AllOff { .. } => {
                        for k in 0..128u8 {
                            if self.held[k as usize] {
                                self.note_off(t, k);
                            }
                        }
                    }
                    NoteMsg::Param { id, value, .. } => self.params.add(id, t, value),
                    NoteMsg::ParamMod { .. } => {}
                    NoteMsg::Midi { data, .. } => {
                        // MIDI のコントローラーは、プラグインが割り当てたパラメータへ
                        let (slot, v) = match data[0] & 0xF0 {
                            0xB0 => (data[1] as usize & 0x7F, data[2] as f64 / 127.0),
                            0xD0 => (128, data[1] as f64 / 127.0),
                            0xE0 => (
                                129,
                                (((data[2] as u16 & 0x7F) << 7) | (data[1] as u16 & 0x7F)) as f64
                                    / 16383.0,
                            ),
                            _ => continue,
                        };
                        let pid = self.midi_map[slot];
                        if pid != u32::MAX {
                            self.params.add(pid, t, v);
                        }
                    }
                    // 1 音ごとの表現(VST3 の決まりの正規化: 音程は ±120 半音、音量は 0.25 = そのまま)
                    NoteMsg::Tuning {
                        note_id, semitones, ..
                    } => self.expression(
                        t,
                        NoteExpressionTypeIDs_::kTuningTypeID,
                        note_id,
                        0.5 + semitones / 240.0,
                    ),
                    NoteMsg::Volume { note_id, gain, .. } => self.expression(
                        t,
                        NoteExpressionTypeIDs_::kVolumeTypeID,
                        note_id,
                        gain / 4.0,
                    ),
                    NoteMsg::Brightness { note_id, value, .. } => self.expression(
                        t,
                        NoteExpressionTypeIDs_::kBrightnessTypeID,
                        note_id,
                        value,
                    ),
                    NoteMsg::Pressure { .. } => {}
                }
            }
            // 出力は前のブロックの音が残らないように消してから
            for bus in self.out_bufs.iter_mut() {
                for ch in bus.iter_mut() {
                    ch[..frames].fill(0.0);
                }
            }
            self.fill_context();
            let mut data = ProcessData {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                numSamples: frames as int32,
                numInputs: self.in_bus.len() as int32,
                numOutputs: self.out_bus.len() as int32,
                inputs: if self.in_bus.is_empty() {
                    std::ptr::null_mut()
                } else {
                    self.in_bus.as_mut_ptr()
                },
                outputs: if self.out_bus.is_empty() {
                    std::ptr::null_mut()
                } else {
                    self.out_bus.as_mut_ptr()
                },
                inputParameterChanges: self
                    .params
                    .as_com_ref::<IParameterChanges>()
                    .map_or(std::ptr::null_mut(), |r| r.as_ptr()),
                outputParameterChanges: std::ptr::null_mut(),
                inputEvents: if self.layout.events {
                    self.events
                        .as_com_ref::<IEventList>()
                        .map_or(std::ptr::null_mut(), |r| r.as_ptr())
                } else {
                    std::ptr::null_mut()
                },
                outputEvents: std::ptr::null_mut(),
                processContext: &mut self.context,
            };
            for b in self.out_bus.iter_mut().chain(self.in_bus.iter_mut()) {
                b.silenceFlags = 0;
            }
            let _ = self.processor.process(&mut data);
        }
        self.steady += frames as u64;
    }

    fn fill_context(&mut self) {
        use ProcessContext_::StatesAndFlags_ as F;
        let c = &mut self.context;
        c.sampleRate = self.sample_rate;
        c.continousTimeSamples = self.steady as i64;
        let Some(t) = self.transport else {
            c.state = 0;
            return;
        };
        let mut state =
            F::kTempoValid | F::kTimeSigValid | F::kProjectTimeMusicValid | F::kBarPositionValid;
        if t.playing {
            state |= F::kPlaying;
        }
        if let Some((a, b)) = t.loop_beats {
            state |= F::kCycleActive | F::kCycleValid;
            c.cycleStartMusic = a;
            c.cycleEndMusic = b;
        }
        c.state = state as u32;
        c.tempo = t.tempo;
        c.projectTimeMusic = t.beats;
        c.barPositionMusic = t.bar_start;
        c.projectTimeSamples = (t.seconds * self.sample_rate) as i64;
        c.timeSigNumerator = t.numerator as i32;
        c.timeSigDenominator = t.denominator as i32;
    }

    fn clear_outputs(&mut self, frames: usize) {
        for bus in self.out_bufs.iter_mut() {
            for ch in bus.iter_mut() {
                ch[..frames].fill(0.0);
            }
        }
    }

    pub fn output(&self) -> Option<(&[f32], &[f32])> {
        let bus = self.out_bufs.get(self.layout.main_out)?;
        let l = bus.first()?;
        let r = bus.get(1).unwrap_or(l);
        Some((l, r))
    }

    pub fn stop(&mut self) {
        if self.processing {
            // SAFETY: 処理を止めるだけ
            unsafe {
                self.processor.setProcessing(0);
            }
            self.processing = false;
        }
    }

    pub fn input_mut(&mut self) -> Option<(&mut [f32], Option<&mut [f32]>)> {
        let bus = self.in_bufs.get_mut(self.layout.main_in?)?;
        let (first, rest) = bus.split_first_mut()?;
        Some((&mut first[..], rest.first_mut().map(|v| &mut v[..])))
    }

    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// VST3 には値を書き換えない変調が無い
    pub fn can_modulate(&self, _id: u32) -> bool {
        false
    }

    pub fn can_modulate_per_note(&self, _id: u32) -> bool {
        false
    }

    pub fn set_latency_for_test(&mut self, samples: u32) {
        self.latency = samples;
    }

    pub fn frames_processed(&self) -> u64 {
        self.steady
    }

    pub fn clear_input(&mut self, frames: usize) {
        if let Some(bus) = self.layout.main_in.and_then(|i| self.in_bufs.get_mut(i)) {
            for ch in bus.iter_mut() {
                let n = frames.min(ch.len());
                ch[..n].fill(0.0);
            }
        }
    }

    pub fn accepts_audio(&self) -> bool {
        self.layout.main_in.is_some()
    }

    pub fn accepts_notes(&self) -> bool {
        self.layout.events
    }

    pub fn has_failed(&self) -> bool {
        self.failed
    }
}

impl Drop for Vst3Processor {
    fn drop(&mut self) {
        // 確保したポインタの持ち主(in_ptrs / out_ptrs)は構造体と一緒に消える
        let _ = (&self.in_ptrs, &self.out_ptrs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::ComRef;

    #[test]
    fn ids_roundtrip_and_state_splits() {
        let cid: TUID = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, -1, -128];
        let id = format!("vst3:{}", hex_id(&cid));
        assert_eq!(id, "vst3:0102030405060708090A0B0C0D0EFF80");
        assert_eq!(parse_id(&id), Some(cid));
        assert_eq!(parse_id("vst3:12"), None);
        let mut s = b"GV3S".to_vec();
        s.extend_from_slice(&3u32.to_le_bytes());
        s.extend_from_slice(b"abc");
        s.extend_from_slice(&2u32.to_le_bytes());
        s.extend_from_slice(b"xy");
        assert_eq!(split_state(&s), (&b"abc"[..], &b"xy"[..]));
        assert_eq!(split_state(b"raw"), (&b"raw"[..], &b""[..]));
    }

    #[test]
    fn features_from_sub_categories() {
        let f = features_of("Instrument|Synth");
        assert!(f.contains(&"instrument".to_owned()) && !f.contains(&"audio-effect".to_owned()));
        let f = features_of("Fx|Reverb");
        assert!(f.contains(&"audio-effect".to_owned()));
    }

    #[test]
    fn param_changes_sort_points_and_reuse_queues() {
        let pc = ParamChanges::new();
        pc.add(7, 100, 0.5);
        pc.add(7, 10, 0.2);
        pc.add(9, 0, 2.0);
        unsafe {
            assert_eq!(pc.getParameterCount(), 2);
            let q = ComRef::from_raw(pc.getParameterData(0)).unwrap();
            assert_eq!(q.getParameterId(), 7);
            let (mut o, mut v) = (0, 0.0);
            q.getPoint(0, &mut o, &mut v);
            assert_eq!((o, v), (10, 0.2));
            let q = ComRef::from_raw(pc.getParameterData(1)).unwrap();
            q.getPoint(0, &mut o, &mut v);
            assert_eq!(v, 1.0, "0〜1 に収める");
        }
        pc.clear();
        unsafe {
            assert_eq!(pc.getParameterCount(), 0);
        }
    }
}
