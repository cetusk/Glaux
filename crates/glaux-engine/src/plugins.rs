//! CLAP プラグイン(外部の音源)をエンジンで鳴らすための管理。
//!
//! スレッド構成(CLAP のスレッド規約に合わせる):
//! - **プラグインのメインスレッド**(`glaux-plugins`): インスタンスの生成・起動・状態の
//!   読み書き・破棄をすべてここで行う([`glaux_clap::ClapPlugin`] は Send でない)
//! - **オーディオスレッド**: レンダラが処理窓口([`Processor`])を受け取って `process` する
//! - **UI スレッド**: [`PluginManager::sync`] がプロジェクトを見て、どのトラックにどの
//!   プラグインを載せるか(スロットと世代)を決め、メインスレッドへ指示する
//!
//! 処理窓口の受け渡しは [`PluginSlot`] の `AtomicPtr` 1 個ずつ(ロック・アロケーションなし)。
//! オーディオスレッドは受け取った窓口を使い終えたら `outgoing` に戻し、メインスレッドが
//! 止めて破棄する(オーディオスレッドでは解放しない)。

use glaux_clap::{ClapPlugin, ClapProcessor, ParamInfo, PluginInfo};
use glaux_core::{PluginSource, Project, TrackId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

/// 同時に載せられるプラグイン数
pub const MAX_PLUGINS: usize = 16;

/// オーディオスレッドへ渡す処理窓口(どのインスタンスのものかを世代で見分ける)。
pub struct Processor {
    pub gen: u64,
    pub clap: ClapProcessor,
}

/// パラメータ変更の受け渡し(プラグインのスレッド → オーディオスレッド)。
/// 積むのはプラグインのスレッドだけ、取り出すのはオーディオスレッドだけ(SPSC、ロックなし)。
pub struct ParamQueue {
    ids: Box<[AtomicU32]>,
    values: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

const PARAM_QUEUE_CAP: usize = 512;

impl Default for ParamQueue {
    fn default() -> Self {
        ParamQueue {
            ids: (0..PARAM_QUEUE_CAP).map(|_| AtomicU32::new(0)).collect(),
            values: (0..PARAM_QUEUE_CAP).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }
}

impl ParamQueue {
    /// 積む(満杯なら false)。
    pub fn push(&self, id: u32, value: f64) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= PARAM_QUEUE_CAP {
            return false;
        }
        let i = h % PARAM_QUEUE_CAP;
        self.ids[i].store(id, Ordering::Relaxed);
        self.values[i].store(value.to_bits(), Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    /// 取り出す(オーディオスレッド)。
    pub fn pop(&self) -> Option<(u32, f64)> {
        let t = self.tail.load(Ordering::Relaxed);
        if t == self.head.load(Ordering::Acquire) {
            return None;
        }
        let i = t % PARAM_QUEUE_CAP;
        let v = (
            self.ids[i].load(Ordering::Relaxed),
            f64::from_bits(self.values[i].load(Ordering::Relaxed)),
        );
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

/// 処理窓口の受け渡し口(スロットごとに 1 つ)。
#[derive(Default)]
pub struct PluginSlot {
    incoming: AtomicPtr<Processor>,
    outgoing: AtomicPtr<Processor>,
    /// この世代の窓口を返してほしい(0 = 要求なし)
    remove_gen: AtomicU64,
    /// AI・画面以外(プロジェクト)からのパラメータ変更
    pub params: ParamQueue,
}

impl PluginSlot {
    fn swap(ptr: &AtomicPtr<Processor>, p: Option<Box<Processor>>) -> Option<Box<Processor>> {
        let new = p.map_or(std::ptr::null_mut(), Box::into_raw);
        let old = ptr.swap(new, Ordering::AcqRel);
        // SAFETY: このスロットに入るのは Box::into_raw した窓口だけで、swap で所有権ごと取り出す
        (!old.is_null()).then(|| unsafe { Box::from_raw(old) })
    }

    /// メインスレッド: 新しい窓口を置く(前に置いてまだ取られていないものがあれば返す)。
    pub fn put_incoming(&self, p: Box<Processor>) -> Option<Box<Processor>> {
        Self::swap(&self.incoming, Some(p))
    }

    /// オーディオスレッド: 置かれた窓口を受け取る。
    pub fn take_incoming(&self) -> Option<Box<Processor>> {
        if self.incoming.load(Ordering::Acquire).is_null() {
            return None;
        }
        Self::swap(&self.incoming, None)
    }

    /// オーディオスレッド: 使い終えた窓口を返す。返却口が埋まっていれば受け取らずに返す。
    pub fn put_outgoing(&self, p: Box<Processor>) -> Result<(), Box<Processor>> {
        let raw = Box::into_raw(p);
        match self.outgoing.compare_exchange(
            std::ptr::null_mut(),
            raw,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            // SAFETY: 置けなかったので所有権はこちらに残っている
            Err(_) => Err(unsafe { Box::from_raw(raw) }),
        }
    }

    pub fn outgoing_free(&self) -> bool {
        self.outgoing.load(Ordering::Acquire).is_null()
    }

    /// メインスレッド: 返された窓口を受け取る。
    pub fn take_outgoing(&self) -> Option<Box<Processor>> {
        if self.outgoing.load(Ordering::Acquire).is_null() {
            return None;
        }
        Self::swap(&self.outgoing, None)
    }

    pub fn request_remove(&self, gen: u64) {
        self.remove_gen.store(gen, Ordering::Release);
    }

    pub fn remove_requested(&self) -> u64 {
        self.remove_gen.load(Ordering::Acquire)
    }
}

impl Drop for PluginSlot {
    fn drop(&mut self) {
        drop(Self::swap(&self.incoming, None));
        drop(Self::swap(&self.outgoing, None));
    }
}

pub fn new_slots() -> [PluginSlot; MAX_PLUGINS] {
    std::array::from_fn(|_| PluginSlot::default())
}

// ---- プラグインの一覧 ----

fn catalog_cell() -> &'static Mutex<Option<Vec<PluginInfo>>> {
    static CATALOG: OnceLock<Mutex<Option<Vec<PluginInfo>>>> = OnceLock::new();
    CATALOG.get_or_init(|| Mutex::new(None))
}

/// 探す場所: CLAP の標準パス + `GLAUX_CLAP_PATH`(区切りは OS の PATH と同じ)。
pub fn search_paths() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(p) = std::env::var_os("GLAUX_CLAP_PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    dirs.extend(glaux_clap::default_search_paths());
    dirs
}

/// インストール済みの CLAP プラグイン(初回だけ探す。探し直すなら [`rescan`])。
pub fn catalog() -> Vec<PluginInfo> {
    let mut c = catalog_cell().lock().unwrap_or_else(|e| e.into_inner());
    c.get_or_insert_with(|| glaux_clap::scan(&search_paths()))
        .clone()
}

/// プラグインを探し直す(インストールした後など)。
pub fn rescan() -> Vec<PluginInfo> {
    let list = glaux_clap::scan(&search_paths());
    *catalog_cell().lock().unwrap_or_else(|e| e.into_inner()) = Some(list.clone());
    list
}

pub fn find(id: &str) -> Option<PluginInfo> {
    catalog().into_iter().find(|p| p.id == id)
}

/// プロジェクトに保存する状態(base64)をバイト列に戻す。
pub fn decode_state(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

pub fn encode_state(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---- パラメータの情報と今の値(AI・UI 向け) ----

/// プロジェクトに書くパラメータのキー(`device/clap:<id>` の `clap:<id>` 部分)。
pub fn param_key(id: u32) -> String {
    format!("clap:{id}")
}

/// `clap:<id>` → id。
pub fn parse_param_key(key: &str) -> Option<u32> {
    key.strip_prefix("clap:")?.parse().ok()
}

/// デバイスの params から CLAP パラメータの上書き値を取り出す。
fn overrides(params: &glaux_core::ParamMap) -> HashMap<u32, f64> {
    params
        .iter()
        .filter_map(|(k, v)| {
            let id = parse_param_key(k)?;
            let v = match v {
                glaux_core::ParamValue::Float(f) => *f,
                glaux_core::ParamValue::Int(i) => *i as f64,
                glaux_core::ParamValue::Bool(b) => *b as i64 as f64,
                _ => return None,
            };
            Some((id, v))
        })
        .collect()
}

fn param_info_cache() -> &'static Mutex<HashMap<String, Arc<Vec<ParamInfo>>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Vec<ParamInfo>>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// プラグインのパラメータの一覧。まだ誰も読み込んでいなければ、呼んだスレッドで一時的に
/// インスタンスを作って調べる(以後はキャッシュ)。
pub fn param_infos(plugin_id: &str) -> Option<Arc<Vec<ParamInfo>>> {
    if let Some(v) = param_info_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(plugin_id)
    {
        return Some(v.clone());
    }
    let info = find(plugin_id)?;
    glaux_clap::mark_main_thread();
    let mut p = ClapPlugin::new(&info.path, &info.id).ok()?;
    let list = Arc::new(p.param_infos());
    param_info_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(plugin_id.to_owned(), list.clone());
    Some(list)
}

/// AI に見せる・動かせるパラメータか(自動化でき、隠し・読み取り専用でない)
pub fn is_public_param(p: &ParamInfo) -> bool {
    p.automatable && !p.hidden && !p.readonly
}

/// パラメータ ID → (今の値, 表示用の文字列)
pub type ParamValues = HashMap<u32, (f64, String)>;
/// 状態の保存結果: (状態のバイト列, 上書きしているパラメータの今の値)
type SavedState = (Vec<u8>, Vec<(u32, f64)>);

fn live_values_cell() -> &'static Mutex<HashMap<TrackId, ParamValues>> {
    static C: OnceLock<Mutex<HashMap<TrackId, ParamValues>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// トラックのプラグインの今の値(と表示用の文字列)。読み込み・変更のたびにプラグインの
/// スレッドが更新する。まだ無ければ None
pub fn live_values(track: &TrackId) -> Option<ParamValues> {
    live_values_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(track)
        .cloned()
}

/// テスト用: 共有表に「プラグインの今の値」を置く。
#[doc(hidden)]
pub fn set_live_values_for_test(track: &TrackId, id: u32, value: f64, text: &str) {
    live_values_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(track.clone())
        .or_default()
        .insert(id, (value, text.to_owned()));
}

fn hash_str(s: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---- メインスレッド ----

enum HostCmd {
    Create {
        slot: usize,
        gen: u64,
        track: TrackId,
        info: PluginInfo,
        state: Option<Vec<u8>>,
        params: Vec<(u32, f64)>,
        sample_rate: f64,
    },
    /// 状態を読み込み直してから上書き値を送る(取り消しで上書きが消えたときなど)
    Reload {
        gen: u64,
        state: Option<Vec<u8>>,
        params: Vec<(u32, f64)>,
    },
    SetParams {
        gen: u64,
        params: Vec<(u32, f64)>,
    },
    Destroy {
        slot: usize,
        gen: u64,
    },
    /// 状態と、指定したパラメータの今の値
    SaveState {
        gen: u64,
        ids: Vec<u32>,
        reply: mpsc::Sender<Result<SavedState, String>>,
    },
    OpenGui {
        gen: u64,
        title: String,
        reply: mpsc::Sender<Result<(), String>>,
    },
    CloseGui {
        gen: u64,
    },
}

/// プラグインのスレッドから UI への知らせ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginEvent {
    /// 画面での操作などで状態が変わった(プロジェクトへ保存するとよい)
    Dirty,
    /// 利用者が画面を閉じた
    GuiClosed,
}

struct Live {
    gen: u64,
    slot: usize,
    track: TrackId,
    plugin: ClapPlugin,
    dying: bool,
    /// この時刻を過ぎたら今の値を読み直す(変更がオーディオスレッドで反映されてから)
    refresh_at: Option<std::time::Instant>,
}

impl Live {
    /// パラメータの変更をオーディオスレッドへ送る。
    fn send_params(&mut self, slots: &[PluginSlot; MAX_PLUGINS], params: &[(u32, f64)]) {
        for &(id, v) in params {
            if !slots[self.slot].params.push(id, v) {
                tracing::warn!("パラメータ変更が多すぎるため一部を捨てました");
                break;
            }
        }
        self.schedule_refresh();
    }

    fn schedule_refresh(&mut self) {
        self.refresh_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(120));
    }

    /// 今の値を読み直して共有の表に書く(AI・UI が読む)。
    fn refresh_values(&mut self) {
        self.refresh_at = None;
        let Some(infos) = param_info_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&self.plugin.id)
            .cloned()
        else {
            return;
        };
        let ids: Vec<u32> = infos
            .iter()
            .filter(|p| is_public_param(p))
            .map(|p| p.id)
            .collect();
        let values: HashMap<u32, (f64, String)> = self
            .plugin
            .param_values(&ids)
            .into_iter()
            .map(|(id, v, t)| (id, (v, t)))
            .collect();
        live_values_cell()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.track.clone(), values);
    }
}

fn host_thread(
    rx: mpsc::Receiver<HostCmd>,
    slots: Arc<[PluginSlot; MAX_PLUGINS]>,
    events: Arc<Mutex<Vec<(u64, PluginEvent)>>>,
) {
    glaux_clap::mark_main_thread();
    let mut live: Vec<Live> = Vec::new();
    let retire = |live: &mut Vec<Live>, p: Box<Processor>| {
        if let Some(i) = live.iter().position(|l| l.gen == p.gen) {
            live[i].plugin.deactivate(p.clap);
            if live[i].dying {
                live.remove(i);
            }
        }
    };
    loop {
        // 画面を開いている間はウィンドウのメッセージをこまめに処理する(固まらないように)
        let any_gui = live.iter().any(|l| l.plugin.is_gui_open());
        glaux_clap::pump_gui_events();
        let wait = if any_gui { 8 } else { 30 };
        match rx.recv_timeout(std::time::Duration::from_millis(wait)) {
            Ok(HostCmd::Create {
                slot,
                gen,
                track,
                info,
                state,
                params,
                sample_rate,
            }) => {
                let made = (|| -> Result<(ClapPlugin, ClapProcessor), glaux_clap::ClapError> {
                    let mut plugin = ClapPlugin::new(&info.path, &info.id)?;
                    if let Some(bytes) = &state {
                        if let Err(e) = plugin.load_state(bytes) {
                            tracing::warn!("{}: 保存された状態を戻せません: {e}", info.name);
                        }
                    }
                    let proc = plugin.activate(sample_rate)?;
                    Ok((plugin, proc))
                })();
                match made {
                    Ok((mut plugin, proc)) => {
                        param_info_cache()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .entry(info.id.clone())
                            .or_insert_with(|| Arc::new(plugin.param_infos()));
                        let mut l = Live {
                            gen,
                            slot,
                            track,
                            plugin,
                            dying: false,
                            refresh_at: None,
                        };
                        // 上書き値は窓口と一緒に届くよう、窓口を置く前に積む
                        l.send_params(&slots, &params);
                        l.schedule_refresh();
                        live.push(l);
                        if let Some(old) =
                            slots[slot].put_incoming(Box::new(Processor { gen, clap: proc }))
                        {
                            // 前に置いた窓口がまだ使われていなかった
                            retire(&mut live, old);
                        }
                        tracing::info!("CLAP を読み込みました: {} (slot {slot})", info.name);
                    }
                    Err(e) => tracing::warn!("CLAP を読み込めません: {e}"),
                }
            }
            Ok(HostCmd::Reload { gen, state, params }) => {
                if let Some(l) = live.iter_mut().find(|l| l.gen == gen) {
                    if let Some(bytes) = &state {
                        if let Err(e) = l.plugin.load_state(bytes) {
                            tracing::warn!("CLAP の状態を戻せません: {e}");
                        }
                    }
                    l.send_params(&slots, &params);
                }
            }
            Ok(HostCmd::SetParams { gen, params }) => {
                if let Some(l) = live.iter_mut().find(|l| l.gen == gen) {
                    l.send_params(&slots, &params);
                }
            }
            Ok(HostCmd::Destroy { slot, gen }) => {
                if let Some(l) = live.iter_mut().find(|l| l.gen == gen) {
                    l.dying = true;
                }
                slots[slot].request_remove(gen);
                // まだオーディオスレッドが受け取っていなければ、ここで取り戻して片付ける
                if let Some(p) = slots[slot].take_incoming() {
                    if p.gen == gen {
                        retire(&mut live, p);
                    } else if let Some(other) = slots[slot].put_incoming(p) {
                        retire(&mut live, other);
                    }
                }
                // 一度も起動していない(窓口を渡していない)ものはそのまま消す
                live.retain(|l| !(l.dying && !l.plugin.is_active()));
            }
            Ok(HostCmd::SaveState { gen, ids, reply }) => {
                let r = match live.iter_mut().find(|l| l.gen == gen) {
                    Some(l) => l
                        .plugin
                        .save_state()
                        .map(|bytes| {
                            let values = l
                                .plugin
                                .param_values(&ids)
                                .into_iter()
                                .map(|(id, v, _)| (id, v))
                                .collect();
                            (bytes, values)
                        })
                        .map_err(|e| e.to_string()),
                    None => Err("プラグインが見つかりません".to_owned()),
                };
                let _ = reply.send(r);
            }
            Ok(HostCmd::OpenGui { gen, title, reply }) => {
                let r = match live.iter_mut().find(|l| l.gen == gen) {
                    Some(l) => l.plugin.open_gui(&title).map_err(|e| e.to_string()),
                    None => Err("プラグインがまだ読み込まれていません".to_owned()),
                };
                let _ = reply.send(r);
            }
            Ok(HostCmd::CloseGui { gen }) => {
                if let Some(l) = live.iter_mut().find(|l| l.gen == gen) {
                    l.plugin.close_gui();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        // オーディオスレッドから返ってきた窓口を片付ける
        for s in slots.iter() {
            if let Some(p) = s.take_outgoing() {
                retire(&mut live, p);
            }
        }
        let mut ev = Vec::new();
        for l in live.iter_mut() {
            l.plugin.poll();
            if l.plugin.gui_tick() == glaux_clap::GuiEvent::Closed {
                ev.push((l.gen, PluginEvent::GuiClosed));
            }
            if l.plugin.take_dirty() {
                ev.push((l.gen, PluginEvent::Dirty));
                l.schedule_refresh();
            }
            if l.refresh_at.is_some_and(|t| std::time::Instant::now() >= t) {
                l.refresh_values();
            }
        }
        if !ev.is_empty() {
            events.lock().unwrap_or_else(|e| e.into_inner()).extend(ev);
        }
    }
}

// ---- UI スレッド側 ----

#[derive(Clone)]
struct Assigned {
    slot: usize,
    gen: u64,
    plugin_id: String,
    state_sig: u64,
    sample_rate: f64,
    /// 送ってあるパラメータの上書き値
    params: HashMap<u32, f64>,
}

/// どのトラックにどのプラグインを載せているかを管理する(UI スレッドから使う)。
pub struct PluginManager {
    tx: mpsc::Sender<HostCmd>,
    events: Arc<Mutex<Vec<(u64, PluginEvent)>>>,
    assigned: Mutex<HashMap<TrackId, Assigned>>,
    next_gen: AtomicU64,
}

impl PluginManager {
    pub fn start(slots: Arc<[PluginSlot; MAX_PLUGINS]>) -> Self {
        let (tx, rx) = mpsc::channel();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        std::thread::Builder::new()
            .name("glaux-plugins".into())
            .spawn(move || host_thread(rx, slots, ev))
            .expect("plugin thread spawn");
        PluginManager {
            tx,
            events,
            assigned: Mutex::new(HashMap::new()),
            next_gen: AtomicU64::new(1),
        }
    }

    /// プロジェクトの CLAP トラックに合わせてプラグインを用意・破棄し、
    /// トラック → (スロット, 世代) の対応を返す(再生データの構築に使う)。
    pub fn sync(&self, project: &Project, sample_rate: f64) -> HashMap<TrackId, (u32, u64)> {
        let mut assigned = self.assigned.lock().unwrap_or_else(|e| e.into_inner());
        // トラック → (プラグイン, 状態, 上書き値)
        type Wanted<'a> = (PluginInfo, Option<&'a str>, HashMap<u32, f64>);
        let mut wanted: HashMap<TrackId, Wanted> = HashMap::new();
        for t in &project.tracks {
            let Some(d) = &t.device else { continue };
            let PluginSource::Clap { plugin_id, state } = &d.source else {
                continue;
            };
            match find(plugin_id) {
                Some(info) => {
                    wanted.insert(t.id.clone(), (info, state.as_deref(), overrides(&d.params)));
                }
                None => tracing::warn!("CLAP プラグインが見つかりません: {plugin_id}"),
            }
        }
        // 要らなくなった・中身が変わったものを片付ける
        let stale: Vec<TrackId> = assigned
            .iter()
            .filter(|(tid, a)| match wanted.get(*tid) {
                None => true,
                Some((info, _, _)) => info.id != a.plugin_id || a.sample_rate != sample_rate,
            })
            .map(|(tid, _)| tid.clone())
            .collect();
        for tid in stale {
            if let Some(a) = assigned.remove(&tid) {
                let _ = self.tx.send(HostCmd::Destroy {
                    slot: a.slot,
                    gen: a.gen,
                });
            }
        }
        let as_vec = |m: &HashMap<u32, f64>| -> Vec<(u32, f64)> {
            let mut v: Vec<(u32, f64)> = m.iter().map(|(k, v)| (*k, *v)).collect();
            v.sort_by_key(|(k, _)| *k);
            v
        };
        for (tid, (info, state, params)) in wanted {
            let sig = hash_str(state);
            if let Some(a) = assigned.get_mut(&tid) {
                let removed = a.params.keys().any(|k| !params.contains_key(k));
                if a.state_sig != sig || removed {
                    // 状態が外から変わった(取り消し・AI の操作)か、上書きが消えた:
                    // 状態を読み込み直してから残りの上書き値を送る
                    a.state_sig = sig;
                    let _ = self.tx.send(HostCmd::Reload {
                        gen: a.gen,
                        state: state.and_then(decode_state),
                        params: as_vec(&params),
                    });
                } else {
                    let changed: Vec<(u32, f64)> = params
                        .iter()
                        .filter(|(k, v)| a.params.get(k) != Some(v))
                        .map(|(k, v)| (*k, *v))
                        .collect();
                    if !changed.is_empty() {
                        let _ = self.tx.send(HostCmd::SetParams {
                            gen: a.gen,
                            params: changed,
                        });
                    }
                }
                a.params = params;
                continue;
            }
            let used: Vec<usize> = assigned.values().map(|a| a.slot).collect();
            let Some(slot) = (0..MAX_PLUGINS).find(|s| !used.contains(s)) else {
                tracing::warn!("CLAP プラグインは同時に {MAX_PLUGINS} 個までです");
                continue;
            };
            let gen = self.next_gen.fetch_add(1, Ordering::Relaxed);
            let _ = self.tx.send(HostCmd::Create {
                slot,
                gen,
                track: tid.clone(),
                info: info.clone(),
                state: state.and_then(decode_state),
                params: as_vec(&params),
                sample_rate,
            });
            assigned.insert(
                tid,
                Assigned {
                    slot,
                    gen,
                    plugin_id: info.id.clone(),
                    state_sig: sig,
                    sample_rate,
                    params,
                },
            );
        }
        assigned
            .iter()
            .map(|(tid, a)| (tid.clone(), (a.slot as u32, a.gen)))
            .collect()
    }

    fn gen_of(&self, track: &TrackId) -> Result<u64, String> {
        self.assigned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(track)
            .map(|a| a.gen)
            .ok_or_else(|| "このトラックに CLAP プラグインは載っていません".to_owned())
    }

    /// プラグインの画面を開く(`title` はウィンドウのタイトル)。
    pub fn open_gui(&self, track: &TrackId, title: &str) -> Result<(), String> {
        let gen = self.gen_of(track)?;
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(HostCmd::OpenGui {
                gen,
                title: title.to_owned(),
                reply,
            })
            .map_err(|_| "プラグインのスレッドが止まっています".to_owned())?;
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "プラグインが応答しません".to_owned())?
    }

    pub fn close_gui(&self, track: &TrackId) {
        if let Ok(gen) = self.gen_of(track) {
            let _ = self.tx.send(HostCmd::CloseGui { gen });
        }
    }

    /// プラグインのスレッドからの知らせを取り出す(トラックに直して、重複はまとめる)。
    pub fn take_events(&self) -> Vec<(TrackId, PluginEvent)> {
        let raw: Vec<(u64, PluginEvent)> =
            std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()));
        if raw.is_empty() {
            return vec![];
        }
        let assigned = self.assigned.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(TrackId, PluginEvent)> = Vec::new();
        for (gen, ev) in raw {
            if let Some((tid, _)) = assigned.iter().find(|(_, a)| a.gen == gen) {
                if !out.iter().any(|(t, e)| t == tid && *e == ev) {
                    out.push((tid.clone(), ev));
                }
            }
        }
        out
    }

    /// トラックに載っているプラグインの今の状態(base64)と、上書きしているパラメータの今の値。
    pub fn save_state(&self, track: &TrackId) -> Result<(String, Vec<(u32, f64)>), String> {
        let (gen, ids) = {
            let assigned = self.assigned.lock().unwrap_or_else(|e| e.into_inner());
            let a = assigned
                .get(track)
                .ok_or_else(|| "このトラックに CLAP プラグインは載っていません".to_owned())?;
            (a.gen, a.params.keys().copied().collect::<Vec<u32>>())
        };
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(HostCmd::SaveState { gen, ids, reply })
            .map_err(|_| "プラグインのスレッドが止まっています".to_owned())?;
        let (bytes, values) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "プラグインが応答しません".to_owned())??;
        Ok((encode_state(&bytes), values))
    }

    /// 保存した状態(と上書き値)をプロジェクトに書く前に呼ぶ(同じものを送り直さないように)。
    pub fn note_state_saved(&self, track: &TrackId, state: &str, params: &[(u32, f64)]) {
        if let Some(a) = self
            .assigned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(track)
        {
            a.state_sig = hash_str(Some(state));
            for (id, v) in params {
                a.params.insert(*id, *v);
            }
        }
    }
}

// ---- オフライン(書き出し・解析)用 ----

/// 書き出しなどのために、呼んだスレッドでプラグインを作って起動する。
/// 戻り値のプラグインは、レンダリングが終わったら [`OfflinePlugins::finish`] で片付ける。
pub struct OfflinePlugins {
    plugins: Vec<(u64, ClapPlugin)>,
}

impl OfflinePlugins {
    /// プロジェクトの CLAP トラックのプラグインを作り、`slots` に窓口を置く。
    pub fn create(
        project: &Project,
        sample_rate: f64,
        slots: &[PluginSlot; MAX_PLUGINS],
    ) -> (Self, HashMap<TrackId, (u32, u64)>) {
        glaux_clap::mark_main_thread();
        glaux_clap::mark_audio_thread();
        let mut plugins = Vec::new();
        let mut map = HashMap::new();
        for t in &project.tracks {
            if plugins.len() >= MAX_PLUGINS {
                break;
            }
            let Some(d) = &t.device else { continue };
            let PluginSource::Clap { plugin_id, state } = &d.source else {
                continue;
            };
            let Some(info) = find(plugin_id) else {
                continue;
            };
            let made = (|| -> Result<(ClapPlugin, ClapProcessor), glaux_clap::ClapError> {
                let mut p = ClapPlugin::new(&info.path, &info.id)?;
                if let Some(bytes) = state.as_deref().and_then(decode_state) {
                    p.load_state(&bytes)?;
                }
                let proc = p.activate(sample_rate)?;
                Ok((p, proc))
            })();
            match made {
                Ok((p, proc)) => {
                    let slot = plugins.len();
                    let gen = slot as u64 + 1;
                    // プロジェクトの上書き値も最初のブロックで送る
                    for (id, v) in overrides(&d.params) {
                        slots[slot].params.push(id, v);
                    }
                    let _ = slots[slot].put_incoming(Box::new(Processor { gen, clap: proc }));
                    map.insert(t.id.clone(), (slot as u32, gen));
                    plugins.push((gen, p));
                }
                Err(e) => tracing::warn!("書き出し用に CLAP を用意できません: {e}"),
            }
        }
        (OfflinePlugins { plugins }, map)
    }

    /// レンダラから回収した窓口を渡して片付ける。
    pub fn finish(mut self, processors: Vec<Box<Processor>>) {
        for p in processors {
            if let Some((_, plugin)) = self.plugins.iter_mut().find(|(g, _)| *g == p.gen) {
                plugin.deactivate(p.clap);
            }
        }
    }
}

/// 実プラグインを使うテスト。`GLAUX_TEST_CLAP` に音源の `.clap`(例: Surge XT)を
/// 指定したときだけ動く(無ければ何もせず成功扱い)。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{build_playback_data, SampleBank};
    use crate::render::{Renderer, Shared};
    use glaux_core::{Clip, ClipContent, ClipId, Device, Note, NoteId, Tick, Track, TrackKind};

    /// テスト用プラグインのある場所を探し先に加え、音源の ID を返す
    fn setup() -> Option<String> {
        let path = PathBuf::from(std::env::var_os("GLAUX_TEST_CLAP")?);
        let dir = path.parent()?.to_owned();
        std::env::set_var("GLAUX_CLAP_PATH", &dir);
        rescan()
            .into_iter()
            .find(|p| p.is_instrument())
            .map(|p| p.id)
    }

    fn project_with_plugin(plugin_id: &str) -> Project {
        let mut project = Project::new("t");
        let mut track = Track::new(TrackId::new(), "Synth", TrackKind::Midi);
        track.device = Some(Device {
            source: PluginSource::Clap {
                plugin_id: plugin_id.to_owned(),
                state: None,
            },
            params: Default::default(),
        });
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
        if let ClipContent::Midi { notes, .. } = &mut clip.content {
            // 0.5 秒目から 0.5 秒の C4(120BPM: 960 tick = 0.5 秒)
            notes.push(Note {
                id: NoteId::new(),
                pos: Tick(960),
                dur: Tick(960),
                pitch: 60,
                vel: 110,
                articulation: Default::default(),
                pitch_curve: vec![],
            });
        }
        track.clips.push(clip);
        project.tracks.push(track);
        project
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn offline_render_plays_the_plugin() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let project = project_with_plugin(&id);
        let out = crate::export::render_project(&project, 48_000.0, &SampleBank::default())
            .expect("書き出せる");
        // ステレオ interleaved。0.1〜0.4 秒は無音、0.6〜0.9 秒は鳴っている
        let sec = |a: f64, b: f64| &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
        assert!(
            rms(sec(0.1, 0.4)) < 1e-4,
            "ノート前は無音: {}",
            rms(sec(0.1, 0.4))
        );
        assert!(
            rms(sec(0.6, 0.9)) > 1e-3,
            "プラグインで鳴る: {}",
            rms(sec(0.6, 0.9))
        );
    }

    #[test]
    fn realtime_path_receives_processor_plays_and_releases() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let mut project = project_with_plugin(&id);
        let tid = project.tracks[0].id.clone();
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        assert!(bank.plugin_slots.contains_key(&tid));
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));

        // オーディオスレッド役: 窓口が届くまで停止中に回す → 再生して 1 秒ぶん鳴らす
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 960 * 2];
        for _ in 0..300 {
            r.process(&mut buf, 2);
            if shared.plugin_slots[0].remove_requested() == 0 && r_has_plugin(&mut r) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(r_has_plugin(&mut r), "窓口がオーディオスレッドに届く");
        shared
            .playing
            .store(true, std::sync::atomic::Ordering::Release);
        let mut loud = 0.0f32;
        for _ in 0..50 {
            r.process(&mut buf, 2);
            loud = loud.max(rms(&buf));
        }
        assert!(loud > 1e-3, "再生で鳴る: {loud}");

        // 状態を保存できる
        let state = manager.save_state(&tid).expect("状態を保存できる");
        assert!(!state.0.is_empty());

        // 音源を外すと窓口が返却される
        project.tracks[0].device = None;
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        assert!(bank.plugin_slots.is_empty());
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        for _ in 0..20 {
            r.process(&mut buf, 2);
        }
        assert!(!r_has_plugin(&mut r), "外したら窓口を手放す");
    }

    fn r_has_plugin(r: &mut Renderer) -> bool {
        r.has_plugins()
    }

    /// ゼロ交差から周波数を推定(ステレオ interleaved の左)
    fn freq_of(stereo: &[f32], sr: f32) -> f32 {
        let l: Vec<f32> = stereo.iter().step_by(2).copied().collect();
        let c: Vec<usize> = (1..l.len())
            .filter(|&i| l[i - 1] < 0.0 && l[i] >= 0.0)
            .collect();
        if c.len() < 2 {
            return 0.0;
        }
        (c.len() - 1) as f32 * sr / (c[c.len() - 1] - c[0]) as f32
    }

    /// 名前に `needle` を含む、操作できる連続値のパラメータ
    fn find_param(plugin_id: &str, needle: &str) -> Option<ParamInfo> {
        param_infos(plugin_id)?
            .iter()
            .find(|p| is_public_param(p) && !p.stepped && p.name.to_lowercase().contains(needle))
            .cloned()
    }

    #[test]
    fn project_param_override_reaches_plugin_and_live_values() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let target = find_param(&id, "volume").expect("音量のパラメータがある");
        eprintln!(
            "対象: {} / {} [{}..{}]",
            target.module, target.name, target.min, target.max
        );
        let mut project = project_with_plugin(&id);
        let tid = project.tracks[0].id.clone();
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 480 * 2];
        let mut run = |r: &mut Renderer, ms: u64| {
            let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < until {
                r.process(&mut buf, 2);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        run(&mut r, 600);
        let before = live_values(&tid).and_then(|m| m.get(&target.id).cloned());
        eprintln!("変更前: {before:?}");
        assert!(before.is_some(), "読み込み後に今の値が共有される");

        // AI の set_param 相当: 上書き値を書いて同期 → オーディオスレッド経由でプラグインへ
        let want = target.min;
        project.tracks[0]
            .device
            .as_mut()
            .unwrap()
            .params
            .insert(param_key(target.id), glaux_core::ParamValue::Float(want));
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        run(&mut r, 600);
        let after = live_values(&tid)
            .and_then(|m| m.get(&target.id).cloned())
            .unwrap();
        eprintln!("変更後: {after:?}");
        assert!((after.0 - want).abs() < 1e-6, "上書き値がプラグインに届く");

        // 保存すると上書き値の今の値も返る
        let (_, values) = manager.save_state(&tid).unwrap();
        assert_eq!(values.len(), 1);
        assert!((values[0].1 - want).abs() < 1e-6);
    }

    #[test]
    fn automation_lane_moves_plugin_param_in_export() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let target = find_param(&id, "volume").expect("音量のパラメータがある");
        let mut project = project_with_plugin(&id);
        // 1 小節まるごと鳴らし、音量を 最大 → 最小 に下げていく
        if let ClipContent::Midi { notes, .. } = &mut project.tracks[0].clips[0].content {
            notes[0].pos = Tick(0);
            notes[0].dur = Tick(3840);
        }
        project.tracks[0]
            .automation
            .push(glaux_core::AutomationLane {
                target: glaux_core::ParamPath::device(param_key(target.id)),
                points: vec![
                    glaux_core::AutomationPoint {
                        tick: Tick(0),
                        value: target.max,
                        curve: glaux_core::Curve::Linear,
                    },
                    glaux_core::AutomationPoint {
                        tick: Tick(3840),
                        value: target.min,
                        curve: glaux_core::Curve::Linear,
                    },
                ],
            });
        let out =
            crate::export::render_project(&project, 48_000.0, &SampleBank::default()).unwrap();
        let sec = |a: f64, b: f64| &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
        let (head, tail) = (rms(sec(0.2, 0.5)), rms(sec(1.5, 1.9)));
        eprintln!("head {head} tail {tail}");
        assert!(
            head > tail * 3.0,
            "オートメーションで音量が下がる: {head} {tail}"
        );
    }

    #[test]
    fn pitch_curve_bends_plugin_note() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        // A4 を 1 小節。後半はピッチカーブで +12 半音(1 オクターブ上)
        let mut project = project_with_plugin(&id);
        if let ClipContent::Midi { notes, .. } = &mut project.tracks[0].clips[0].content {
            notes[0].pos = Tick(0);
            notes[0].dur = Tick(3840);
            notes[0].pitch = 69;
            notes[0].pitch_curve = vec![
                glaux_core::PitchPoint {
                    tick: Tick(0),
                    cents: 0.0,
                },
                glaux_core::PitchPoint {
                    tick: Tick(1800),
                    cents: 0.0,
                },
                glaux_core::PitchPoint {
                    tick: Tick(1920),
                    cents: 1200.0,
                },
            ];
        }
        let out =
            crate::export::render_project(&project, 48_000.0, &SampleBank::default()).unwrap();
        let sec = |a: f64, b: f64| &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
        let (f1, f2) = (
            freq_of(sec(0.3, 0.8), 48_000.0),
            freq_of(sec(1.3, 1.8), 48_000.0),
        );
        eprintln!("前半 {f1} Hz / 後半 {f2} Hz");
        assert!(
            f2 > f1 * 1.8,
            "ピッチカーブで 1 オクターブ上がる: {f1} → {f2}"
        );
    }

    #[test]
    fn live_pitch_bend_reaches_plugin() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let project = project_with_plugin(&id);
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        shared
            .live_track
            .store(0, std::sync::atomic::Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 960 * 2];
        for _ in 0..300 {
            r.process(&mut buf, 2);
            if r.has_plugins() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(r.has_plugins());
        let mut listen = |r: &mut Renderer| {
            let mut all = Vec::new();
            for _ in 0..25 {
                r.process(&mut buf, 2);
                all.extend_from_slice(&buf);
            }
            freq_of(&all[all.len() / 2..], 48_000.0)
        };
        shared.live.push(crate::midi::LiveEvent::NoteOn {
            track: 0,
            pitch: 69,
            vel: 110,
        });
        let f1 = listen(&mut r);
        shared.live.push(crate::midi::LiveEvent::PitchBend(16383));
        let f2 = listen(&mut r);
        eprintln!("ベンド前 {f1} Hz / 最大ベンド {f2} Hz");
        assert!(f2 > f1 * 1.05, "ピッチベンドで音が上がる: {f1} → {f2}");
    }

    #[test]
    fn live_sustain_pedal_holds_plugin_notes() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let project = project_with_plugin(&id);
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        shared
            .live_track
            .store(0, std::sync::atomic::Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 960 * 2];
        for _ in 0..300 {
            r.process(&mut buf, 2);
            if r.has_plugins() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut level = |r: &mut Renderer| {
            let mut m = 0.0f32;
            for _ in 0..40 {
                r.process(&mut buf, 2);
                m = rms(&buf);
            }
            m
        };
        use crate::midi::LiveEvent;
        shared.live.push(LiveEvent::Sustain(true));
        shared.live.push(LiveEvent::NoteOn {
            track: 0,
            pitch: 60,
            vel: 110,
        });
        let _ = level(&mut r);
        shared.live.push(LiveEvent::NoteOff { pitch: 60 });
        let held = level(&mut r);
        shared.live.push(LiveEvent::Sustain(false));
        let released = level(&mut r);
        eprintln!("ペダル中 {held} / 離した後 {released}");
        assert!(held > 1e-3, "ペダル中は鳴り続ける: {held}");
        assert!(released < held * 0.1, "ペダルを離すと消える: {released}");
    }
}
