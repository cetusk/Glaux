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
use glaux_core::{FxId, ParamMap, PluginSource, Project, TrackId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

/// プラグインを載せている所(トラックの音源、またはエフェクト)。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PluginOwner {
    Track(TrackId),
    Effect(FxId),
}

impl std::fmt::Display for PluginOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginOwner::Track(t) => write!(f, "{t}"),
            PluginOwner::Effect(e) => write!(f, "{e}"),
        }
    }
}

/// プロジェクトに載っている CLAP プラグイン(トラックの音源と、トラック・マスターのエフェクト。
/// バイパス中のエフェクトも含む = 切り替えで作り直さない)。(持ち主, プラグイン ID, 状態, パラメータ)
pub fn project_plugins(project: &Project) -> Vec<(PluginOwner, &str, Option<&str>, &ParamMap)> {
    let mut out = Vec::new();
    for t in &project.tracks {
        if let Some(d) = &t.device {
            if let PluginSource::Clap { plugin_id, state } = &d.source {
                out.push((
                    PluginOwner::Track(t.id.clone()),
                    plugin_id.as_str(),
                    state.as_deref(),
                    &d.params,
                ));
            }
        }
    }
    let effects = project
        .tracks
        .iter()
        .flat_map(|t| t.effects.iter())
        .chain(project.master.effects.iter());
    for e in effects {
        if let PluginSource::Clap { plugin_id, state } = &e.source {
            out.push((
                PluginOwner::Effect(e.id.clone()),
                plugin_id.as_str(),
                state.as_deref(),
                &e.params,
            ));
        }
    }
    out
}

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

// ---- プリセット ----

type PresetList = Arc<Vec<glaux_clap::PresetEntry>>;

fn preset_cache() -> &'static Mutex<HashMap<String, PresetList>> {
    static C: OnceLock<Mutex<HashMap<String, PresetList>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// プラグインのプリセット一覧(初回だけ探す。`rescan` で探し直す)。重いので UI スレッドで呼ばないこと。
pub fn presets(plugin_id: &str, rescan: bool) -> Result<Arc<Vec<glaux_clap::PresetEntry>>, String> {
    if !rescan {
        if let Some(v) = preset_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(plugin_id)
        {
            return Ok(v.clone());
        }
    }
    let info =
        find(plugin_id).ok_or_else(|| format!("CLAP プラグインが見つかりません: {plugin_id}"))?;
    let list = Arc::new(glaux_clap::list_presets(&info.path, &info.id).map_err(|e| e.to_string())?);
    preset_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(plugin_id.to_owned(), list.clone());
    Ok(list)
}

/// プロジェクトに書いてある状態(base64)にプリセットを読み込んだ後の状態(base64)を作る。
/// 呼んだスレッドで一時的にプラグインを作る(再生中のプラグインには触らない。
/// 結果をプロジェクトに書けば、再生中のプラグインは同期で読み込み直す)。
pub fn state_with_preset(
    plugin_id: &str,
    state: Option<&str>,
    preset_id: &str,
) -> Result<String, String> {
    let info =
        find(plugin_id).ok_or_else(|| format!("CLAP プラグインが見つかりません: {plugin_id}"))?;
    glaux_clap::mark_main_thread();
    let mut p = ClapPlugin::new(&info.path, &info.id).map_err(|e| e.to_string())?;
    if let Some(bytes) = state.and_then(decode_state) {
        p.load_state(&bytes).map_err(|e| e.to_string())?;
    }
    let (loc, key) = glaux_clap::PresetEntry::parse_id(preset_id);
    p.load_preset(&loc, key.as_deref())
        .map_err(|e| e.to_string())?;
    let bytes = p.save_state().map_err(|e| e.to_string())?;
    Ok(encode_state(&bytes))
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

fn live_values_cell() -> &'static Mutex<HashMap<PluginOwner, ParamValues>> {
    static C: OnceLock<Mutex<HashMap<PluginOwner, ParamValues>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// トラックのプラグインの今の値(と表示用の文字列)。読み込み・変更のたびにプラグインの
/// スレッドが更新する。まだ無ければ None
pub fn live_values(owner: &PluginOwner) -> Option<ParamValues> {
    live_values_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(owner)
        .cloned()
}

/// テスト用: 共有表に「プラグインの今の値」を置く。
#[doc(hidden)]
pub fn set_live_values_for_test(owner: &PluginOwner, id: u32, value: f64, text: &str) {
    live_values_cell()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(owner.clone())
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
        owner: PluginOwner,
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
    owner: PluginOwner,
    plugin: ClapPlugin,
    dying: bool,
    /// この時刻を過ぎたら今の値を読み直す(変更がオーディオスレッドで反映されてから)
    refresh_at: Option<std::time::Instant>,
    /// 起動したときのサンプルレート(再起動に使う)
    sample_rate: f64,
    /// 再起動を頼まれ、オーディオスレッドから処理窓口が返ってくるのを待っている
    restarting: bool,
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
            .insert(self.owner.clone(), values);
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
                return;
            }
            // 再起動: 止めた窓口を起動し直して(遅延を読み直す)、同じ世代のまま渡し直す
            if live[i].restarting {
                live[i].restarting = false;
                let l = &mut live[i];
                match l.plugin.activate(l.sample_rate) {
                    Ok(proc) => {
                        tracing::info!(
                            "CLAP を再起動しました(slot {}、遅延 {} サンプル)",
                            l.slot,
                            proc.latency()
                        );
                        slots[l.slot].request_remove(0);
                        if let Some(old) = slots[l.slot].put_incoming(Box::new(Processor {
                            gen: l.gen,
                            clap: proc,
                        })) {
                            l.plugin.deactivate(old.clap);
                        }
                    }
                    Err(e) => tracing::warn!("CLAP を再起動できません: {e}"),
                }
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
                owner,
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
                            owner,
                            plugin,
                            dying: false,
                            refresh_at: None,
                            sample_rate,
                            restarting: false,
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
        // 再起動の頼み(処理の遅延が変わったときなど): オーディオスレッドに窓口を返してもらう
        // (返ってきたら retire で起動し直す)。まだ受け取られていない窓口なら、ここで取り戻して直す
        let mut back = Vec::new();
        for l in live.iter_mut() {
            if !l.dying
                && !l.restarting
                && l.plugin.is_active()
                && l.plugin.take_restart_requested()
            {
                l.restarting = true;
                slots[l.slot].request_remove(l.gen);
                if let Some(p) = slots[l.slot].take_incoming() {
                    if p.gen == l.gen {
                        back.push(p);
                    } else if let Some(other) = slots[l.slot].put_incoming(p) {
                        // 入れ直す間に別の窓口が置かれた(ふつうは起きない)
                        back.push(other);
                    }
                }
            }
        }
        for p in back {
            retire(&mut live, p);
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
    assigned: Mutex<HashMap<PluginOwner, Assigned>>,
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
    pub fn sync(&self, project: &Project, sample_rate: f64) -> HashMap<PluginOwner, (u32, u64)> {
        let mut assigned = self.assigned.lock().unwrap_or_else(|e| e.into_inner());
        // 持ち主 → (プラグイン, 状態, 上書き値)
        type Wanted<'a> = (PluginInfo, Option<&'a str>, HashMap<u32, f64>);
        let mut wanted: HashMap<PluginOwner, Wanted> = HashMap::new();
        for (owner, plugin_id, state, params) in project_plugins(project) {
            match find(plugin_id) {
                Some(info) => {
                    wanted.insert(owner, (info, state, overrides(params)));
                }
                None => tracing::warn!("CLAP プラグインが見つかりません: {plugin_id}"),
            }
        }
        // 要らなくなった・中身が変わったものを片付ける
        let stale: Vec<PluginOwner> = assigned
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
                owner: tid.clone(),
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

    fn gen_of(&self, owner: &PluginOwner) -> Result<u64, String> {
        self.assigned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(owner)
            .map(|a| a.gen)
            .ok_or_else(|| "CLAP プラグインが載っていません".to_owned())
    }

    /// プラグインの画面を開く(`title` はウィンドウのタイトル)。
    pub fn open_gui(&self, owner: &PluginOwner, title: &str) -> Result<(), String> {
        let gen = self.gen_of(owner)?;
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

    pub fn close_gui(&self, owner: &PluginOwner) {
        if let Ok(gen) = self.gen_of(owner) {
            let _ = self.tx.send(HostCmd::CloseGui { gen });
        }
    }

    /// プラグインのスレッドからの知らせを取り出す(持ち主に直して、重複はまとめる)。
    pub fn take_events(&self) -> Vec<(PluginOwner, PluginEvent)> {
        let raw: Vec<(u64, PluginEvent)> =
            std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()));
        if raw.is_empty() {
            return vec![];
        }
        let assigned = self.assigned.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<(PluginOwner, PluginEvent)> = Vec::new();
        for (gen, ev) in raw {
            if let Some((tid, _)) = assigned.iter().find(|(_, a)| a.gen == gen) {
                if !out.iter().any(|(t, e)| t == tid && *e == ev) {
                    out.push((tid.clone(), ev));
                }
            }
        }
        out
    }

    /// 載っているプラグインの今の状態(base64)と、上書きしているパラメータの今の値。
    pub fn save_state(&self, owner: &PluginOwner) -> Result<(String, Vec<(u32, f64)>), String> {
        let (gen, ids) = {
            let assigned = self.assigned.lock().unwrap_or_else(|e| e.into_inner());
            let a = assigned
                .get(owner)
                .ok_or_else(|| "CLAP プラグインが載っていません".to_owned())?;
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
    pub fn note_state_saved(&self, owner: &PluginOwner, state: &str, params: &[(u32, f64)]) {
        if let Some(a) = self
            .assigned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(owner)
        {
            a.state_sig = hash_str(Some(state));
            for (id, v) in params {
                a.params.insert(*id, *v);
            }
        }
    }
}

// ---- オフライン(書き出し・解析)用 ----

/// プリセットを 1 音ずつ鳴らす設定(プリセット検索の索引作り用)。
#[derive(Clone, Copy, Debug)]
pub struct PresetRenderSpec {
    pub pitch: u8,
    pub velocity: f32,
    /// 鍵盤を押している秒数
    pub hold: f64,
    /// 1 プリセットあたりの長さ(秒。押している間 + 余韻)
    pub total: f64,
    pub sample_rate: f64,
}

/// プラグイン 1 つを持ち回り、状態やつまみを変えながら 1 音ずつ鳴らす(プリセット検索・つまみの自動合わせ用)。
/// 呼んだスレッドでプラグインを作るので、そのスレッドから動かさないこと(`Send` ではない)。
/// 再生中のプラグインには触らない。
pub struct PluginRenderer {
    /// プリセットの読み込み用(止まったまま使う。起動中のインスタンスに直接読ませると鳴らなくなる
    /// プラグインがある: Surge XT)
    loader: ClapPlugin,
    plugin: ClapPlugin,
    proc: Option<ClapProcessor>,
    sample_rate: f64,
}

const RENDER_BLOCK: usize = 512;

impl PluginRenderer {
    pub fn new(plugin_id: &str, sample_rate: f64) -> Result<Self, String> {
        let info = find(plugin_id)
            .ok_or_else(|| format!("CLAP プラグインが見つかりません: {plugin_id}"))?;
        glaux_clap::mark_main_thread();
        glaux_clap::mark_audio_thread();
        let loader = ClapPlugin::new(&info.path, &info.id).map_err(|e| e.to_string())?;
        let mut plugin = ClapPlugin::new(&info.path, &info.id).map_err(|e| e.to_string())?;
        let proc = plugin.activate(sample_rate).map_err(|e| e.to_string())?;
        Ok(PluginRenderer {
            loader,
            plugin,
            proc: Some(proc),
            sample_rate,
        })
    }

    /// 状態(`save_state` のバイト列)を読み込む。
    pub fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        self.plugin.load_state(state).map_err(|e| e.to_string())?;
        self.plugin.poll();
        Ok(())
    }

    /// プリセットを読み込む(読み込み用のインスタンスで読んで状態を移す)。読み込んだ状態を返す。
    pub fn load_preset(&mut self, preset: &glaux_clap::PresetEntry) -> Result<Vec<u8>, String> {
        self.loader
            .load_preset(&preset.location, preset.load_key.as_deref())
            .map_err(|e| e.to_string())?;
        let state = self.loader.save_state().map_err(|e| e.to_string())?;
        self.load_state(&state)?;
        Ok(state)
    }

    /// つまみの情報と今の値(公開されているもの)。
    pub fn params(&mut self) -> Vec<(ParamInfo, f64)> {
        let infos: Vec<ParamInfo> = self
            .plugin
            .param_infos()
            .into_iter()
            .filter(is_public_param)
            .collect();
        let ids: Vec<u32> = infos.iter().map(|p| p.id).collect();
        let values: HashMap<u32, f64> = self
            .plugin
            .param_values(&ids)
            .into_iter()
            .map(|(id, v, _)| (id, v))
            .collect();
        infos
            .into_iter()
            .map(|p| {
                let v = values.get(&p.id).copied().unwrap_or(p.default);
                (p, v)
            })
            .collect()
    }

    /// つまみ `params`(プラグインの単位)を送ってから 1 音鳴らす(モノラル)。前の音の余韻は消してから鳴らす。
    pub fn render(
        &mut self,
        params: &[(u32, f64)],
        spec: PresetRenderSpec,
    ) -> Result<Vec<f32>, String> {
        use glaux_clap::NoteMsg;
        let proc = self.proc.as_mut().ok_or("プラグインが止まっています")?;
        let sr = self.sample_rate;
        // 無音を流す(状態・つまみの反映と、前の音の余韻の消去)。最初のブロックでつまみを送る
        let blocks = ((3.0 * sr) as usize).div_ceil(RENDER_BLOCK);
        let mut quiet = 0;
        for i in 0..blocks {
            let mut msgs = Vec::new();
            if i == 0 {
                msgs.push(NoteMsg::AllOff { time: 0 });
                msgs.extend(params.iter().map(|&(id, value)| NoteMsg::Param {
                    time: 0,
                    id,
                    value,
                }));
            }
            proc.process(RENDER_BLOCK, &msgs);
            let peak = proc
                .output()
                .map(|(l, r)| {
                    l[..RENDER_BLOCK]
                        .iter()
                        .chain(&r[..RENDER_BLOCK])
                        .fold(0.0f32, |m, v| m.max(v.abs()))
                })
                .unwrap_or(0.0);
            quiet = if peak < 1e-4 { quiet + 1 } else { 0 };
            // 読み込み直後の数ブロックは必ず流す(プラグインが次の処理で音色を切り替えることがある)
            if i >= 8 && quiet >= 4 {
                break;
            }
        }
        let hold = (spec.hold * sr) as usize;
        let total = ((spec.total * sr) as usize).max(hold + RENDER_BLOCK);
        let mut out = Vec::with_capacity(total);
        let mut pos = 0;
        while pos < total {
            let n = RENDER_BLOCK.min(total - pos);
            let mut msgs = Vec::new();
            if pos == 0 {
                msgs.push(NoteMsg::On {
                    time: 0,
                    key: spec.pitch,
                    velocity: spec.velocity,
                    note_id: None,
                });
            }
            if hold >= pos && hold < pos + n {
                msgs.push(NoteMsg::Off {
                    time: (hold - pos) as u32,
                    key: spec.pitch,
                });
            }
            proc.process(n, &msgs);
            match proc.output() {
                Some((l, r)) => out.extend(l[..n].iter().zip(&r[..n]).map(|(a, b)| (a + b) * 0.5)),
                None => out.extend(std::iter::repeat_n(0.0, n)),
            }
            pos += n;
        }
        if proc.has_failed() {
            return Err("プラグインの処理に失敗しました".into());
        }
        Ok(out)
    }
}

impl Drop for PluginRenderer {
    fn drop(&mut self) {
        if let Some(mut proc) = self.proc.take() {
            proc.stop();
            self.plugin.deactivate(proc);
        }
    }
}

/// 同じプラグイン 1 つでプリセットを順に読み込み、1 音ずつ鳴らす(モノラル)。
/// 呼んだスレッドでプラグインを作る(再生中のプラグインには触らない。別スレッドで呼ぶこと)。
/// `each(何番目か, 結果)` が false を返したらそこでやめる。
pub fn render_presets(
    plugin_id: &str,
    presets: &[glaux_clap::PresetEntry],
    spec: PresetRenderSpec,
    mut each: impl FnMut(usize, Result<Vec<f32>, String>) -> bool,
) -> Result<(), String> {
    let mut r = PluginRenderer::new(plugin_id, spec.sample_rate)?;
    for (i, preset) in presets.iter().enumerate() {
        let result = r.load_preset(preset).and_then(|_| r.render(&[], spec));
        if !each(i, result) {
            break;
        }
    }
    Ok(())
}

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
    ) -> (Self, HashMap<PluginOwner, (u32, u64)>) {
        glaux_clap::mark_main_thread();
        glaux_clap::mark_audio_thread();
        let mut plugins = Vec::new();
        let mut map = HashMap::new();
        for (owner, plugin_id, state, params) in project_plugins(project) {
            if plugins.len() >= MAX_PLUGINS {
                break;
            }
            let Some(info) = find(plugin_id) else {
                continue;
            };
            let made = (|| -> Result<(ClapPlugin, ClapProcessor), glaux_clap::ClapError> {
                let mut p = ClapPlugin::new(&info.path, &info.id)?;
                if let Some(bytes) = state.and_then(decode_state) {
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
                    for (id, v) in overrides(params) {
                        slots[slot].params.push(id, v);
                    }
                    let _ = slots[slot].put_incoming(Box::new(Processor { gen, clap: proc }));
                    map.insert(owner, (slot as u32, gen));
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
                glide_ms: None,
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
        assert!(bank
            .plugin_slots
            .contains_key(&PluginOwner::Track(tid.clone())));
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
        let state = manager
            .save_state(&PluginOwner::Track(tid.clone()))
            .expect("状態を保存できる");
        assert!(!state.0.is_empty());

        // 音源を外すと窓口が返却される
        project.tracks[0].device = None;
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        assert!(bank.plugin_slots.is_empty());
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        // 外す指示はプラグインのスレッドを経由して届くので、少し待ちながら回す
        for _ in 0..100 {
            r.process(&mut buf, 2);
            if !r_has_plugin(&mut r) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
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
        let before =
            live_values(&PluginOwner::Track(tid.clone())).and_then(|m| m.get(&target.id).cloned());
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
        let after = live_values(&PluginOwner::Track(tid.clone()))
            .and_then(|m| m.get(&target.id).cloned())
            .unwrap();
        eprintln!("変更後: {after:?}");
        assert!((after.0 - want).abs() < 1e-6, "上書き値がプラグインに届く");

        // 保存すると上書き値の今の値も返る
        let (_, values) = manager
            .save_state(&PluginOwner::Track(tid.clone()))
            .unwrap();
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
    fn bend_and_vibrato_articulations_reach_plugin() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let render = |art: glaux_core::Articulation| {
            let mut project = project_with_plugin(&id);
            if let ClipContent::Midi { notes, .. } = &mut project.tracks[0].clips[0].content {
                notes[0].pos = Tick(0);
                notes[0].dur = Tick(3840);
                notes[0].pitch = 69;
                notes[0].articulation = art;
            }
            crate::export::render_project(&project, 48_000.0, &SampleBank::default()).unwrap()
        };
        // 音程は YIN で測る(零交差は発振器の位相で短い区間がぶれる)
        let track = |x: &[f32]| {
            let mono: Vec<f32> = x.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
            crate::timbre::pitch_track(&mono, 48_000.0)
        };
        let median_f0 = |tr: &[crate::timbre::PitchFrame], a: f32, b: f32| {
            let mut v: Vec<f32> = tr
                .iter()
                .filter(|p| p.time >= a && p.time < b && p.f0 > 0.0)
                .map(|p| p.f0)
                .collect();
            v.sort_by(f32::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.0)
        };
        // ベンド: 出だし(全音下から)は後半より低い
        let bend = track(&render(glaux_core::Articulation::Bend));
        let (early, late) = (median_f0(&bend, 0.04, 0.1), median_f0(&bend, 1.0, 1.5));
        eprintln!("ベンド: 出だし {early} Hz / 後半 {late} Hz");
        assert!(early < late * 0.97, "全音下から上がる: {early} → {late}");
        // ビブラート: 後半の音程の揺れ幅(セント)が通常の音より大きい
        let spread = |tr: &[crate::timbre::PitchFrame]| {
            let fs: Vec<f32> = tr
                .iter()
                .filter(|p| p.time >= 1.0 && p.time < 1.8 && p.f0 > 0.0)
                .map(|p| p.f0)
                .collect();
            let max = fs.iter().cloned().fold(0.0f32, f32::max);
            let min = fs.iter().cloned().fold(f32::MAX, f32::min);
            1200.0 * (max / min).log2()
        };
        let (vib, normal) = (
            spread(&track(&render(glaux_core::Articulation::Vibrato))),
            spread(&track(&render(glaux_core::Articulation::Normal))),
        );
        eprintln!("音程の揺れ幅: ビブラート {vib:.0} セント / 通常 {normal:.0} セント");
        assert!(vib > normal + 30.0, "ビブラートで揺れる: {vib} vs {normal}");
    }

    /// レガート・ポルタメントで、先に離した音のプラグイン側の余韻(長いリリース)が choke で切れる(`GLAUX_TEST_CLAP`)。
    #[test]
    fn portamento_chokes_plugin_release_tails() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        // アンプのリリースを最大(長い余韻)にする
        let release = {
            let mut r = PluginRenderer::new(&id, 48_000.0).unwrap();
            let params = r.params();
            params
                .into_iter()
                .find(|(p, _)| p.name.to_lowercase().contains("amp eg release"))
                .map(|(p, _)| p)
                .expect("Amp EG Release")
        };
        let render = |art: glaux_core::Articulation| {
            let mut project = project_with_plugin(&id);
            if let Some(d) = project.tracks[0].device.as_mut() {
                d.params.insert(
                    param_key(release.id),
                    glaux_core::ParamValue::Float(release.max),
                );
            }
            if let ClipContent::Midi { notes, .. } = &mut project.tracks[0].clips[0].content {
                // C4 を 0〜0.25 秒弾いて離し、1 秒から G4(直前の音なし → 全音下から滑り込む)
                notes[0].pos = Tick(0);
                notes[0].dur = Tick(480);
                notes[0].pitch = 60;
                let mut g = notes[0].clone();
                g.id = NoteId::new();
                g.pos = Tick(1920);
                g.dur = Tick(960);
                g.pitch = 67;
                g.articulation = art;
                notes.push(g);
            }
            let st =
                crate::export::render_project(&project, 48_000.0, &SampleBank::default()).unwrap();
            st.chunks(2).map(|c| c[0] + c[1]).collect::<Vec<f32>>()
        };
        let bin = |x: &[f32], f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in x.iter().enumerate() {
                let w = std::f64::consts::TAU * f as f64 * i as f64 / 48_000.0;
                re += *v as f64 * w.cos();
                im += *v as f64 * w.sin();
            }
            ((re * re + im * im).sqrt() * 2.0 / x.len() as f64) as f32
        };
        // 1.1〜1.3 秒の C4(261.6Hz)成分 = 前の音の余韻
        let tail = |x: &[f32]| bin(&x[52_800..62_400], 261.6);
        let (n, p) = (
            tail(&render(glaux_core::Articulation::Normal)),
            tail(&render(glaux_core::Articulation::Portamento)),
        );
        eprintln!("C4 の余韻(Surge): 通常 {n:.4} → ポルタメント {p:.4}");
        assert!(n > 0.01, "リリースが長ければ通常は余韻が残る: {n}");
        assert!(p < n * 0.2, "ポルタメントでは余韻が切れる: {p} vs {n}");
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

    #[test]
    fn renders_presets_one_note_each() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let list = presets(&id, false).unwrap();
        let pick: Vec<_> = list.iter().take(6).cloned().collect();
        let spec = PresetRenderSpec {
            pitch: 60,
            velocity: 0.8,
            hold: 1.0,
            total: 2.0,
            sample_rate: 48_000.0,
        };
        let t0 = std::time::Instant::now();
        let mut got = Vec::new();
        render_presets(&id, &pick, spec, |i, r| {
            let x = r.unwrap();
            let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            eprintln!(
                "{} {}: {} samples, peak {peak:.3}",
                i,
                pick[i].name,
                x.len()
            );
            got.push((x.len(), peak));
            true
        })
        .unwrap();
        eprintln!("{} プリセットを {:?}", pick.len(), t0.elapsed());
        assert_eq!(got.len(), pick.len());
        assert!(got.iter().all(|(n, _)| *n == 96_000));
        assert!(got.iter().filter(|(_, p)| *p > 0.01).count() >= pick.len() - 1);
    }

    /// エフェクトプラグイン(`GLAUX_TEST_CLAP_FX`、例: Surge XT Effects。既定は Delay)を
    /// トラック・マスターに挿して書き出すと、短い音の後ろにこだまが残る。
    #[test]
    fn clap_effect_on_track_and_master_is_rendered() {
        use glaux_core::{Effect, FxId};
        let Some(path) = std::env::var_os("GLAUX_TEST_CLAP_FX").map(PathBuf::from) else {
            eprintln!("GLAUX_TEST_CLAP_FX が未設定のためスキップ");
            return;
        };
        std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
        let fx_id = rescan()
            .into_iter()
            .find(|p| p.is_effect() && !p.is_instrument())
            .map(|p| p.id)
            .expect("エフェクトがある");
        // 内蔵 subtractive の短い音 1 つ
        let mut project = Project::new("t");
        let mut track = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
        if let ClipContent::Midi { notes, .. } = &mut clip.content {
            notes.push(Note {
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(120),
                pitch: 60,
                vel: 110,
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
            });
        }
        track.clips.push(clip);
        project.tracks.push(track);
        let bank = SampleBank::default();
        let sr = 48_000.0;
        // 1〜2 秒(音が消えた後)のエネルギー
        let tail = |x: &[f32]| -> f32 {
            x.chunks(2)
                .skip(sr as usize)
                .take(sr as usize)
                .map(|c| c[0] * c[0] + c[1] * c[1])
                .sum::<f32>()
        };
        let dry = crate::export::render_project(&project, sr, &bank).unwrap();
        let clap_fx = |id: FxId| Effect {
            id,
            source: PluginSource::Clap {
                plugin_id: fx_id.clone(),
                state: None,
            },
            bypass: false,
            params: Default::default(),
            ui: Default::default(),
        };
        let mut on_track = project.clone();
        on_track.tracks[0].effects.push(clap_fx(FxId::new()));
        let wet = crate::export::render_project(&on_track, sr, &bank).unwrap();
        let mut on_master = project.clone();
        on_master.master.effects.push(clap_fx(FxId::new()));
        let wet_master = crate::export::render_project(&on_master, sr, &bank).unwrap();
        let mut bypassed = on_track.clone();
        bypassed.tracks[0].effects[0].bypass = true;
        let byp = crate::export::render_project(&bypassed, sr, &bank).unwrap();
        eprintln!(
            "余韻のエネルギー: なし {:.5} / トラック {:.5} / マスター {:.5} / バイパス {:.5}",
            tail(&dry),
            tail(&wet),
            tail(&wet_master),
            tail(&byp)
        );
        // 既定のディレイは控えめだが、余韻がはっきり増える
        assert!(tail(&wet) > tail(&dry) * 1.4, "トラックのディレイが鳴る");
        assert!(
            tail(&wet_master) > tail(&dry) * 1.4,
            "マスターのディレイが鳴る"
        );
        // 音量・パンが中央なのでトラックとマスターで同じ効き方
        assert!((tail(&wet) - tail(&wet_master)).abs() < tail(&wet) * 0.05);
        let diff: f32 = byp.iter().zip(&dry).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff < 1e-3, "バイパスなら素通し: {diff}");
    }

    /// 鳴っていない(停止中・無音)トラックの CLAP エフェクトも、バイパス中のものも、毎ブロック処理される
    /// (処理を呼ばれないと画面を開くときに固まるプラグインがある)。
    #[test]
    fn silent_and_bypassed_clap_effects_keep_processing() {
        use glaux_core::{Effect, FxId};
        let Some(path) = std::env::var_os("GLAUX_TEST_CLAP_FX").map(PathBuf::from) else {
            eprintln!("GLAUX_TEST_CLAP_FX が未設定のためスキップ");
            return;
        };
        std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
        let fx_plugin = rescan()
            .into_iter()
            .find(|p| p.is_effect() && !p.is_instrument())
            .unwrap()
            .id;
        let clap_fx = |bypass: bool| Effect {
            id: FxId::new(),
            source: PluginSource::Clap {
                plugin_id: fx_plugin.clone(),
                state: None,
            },
            bypass,
            params: Default::default(),
            ui: Default::default(),
        };
        let mut project = Project::new("t");
        let mut track = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        let active = clap_fx(false);
        let bypassed = clap_fx(true);
        track.effects = vec![active.clone(), bypassed.clone()];
        project.tracks.push(track);
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        let slot_of =
            |fx: &Effect| bank.plugin_slots[&PluginOwner::Effect(fx.id.clone())].0 as usize;
        let (sa, sb) = (slot_of(&active), slot_of(&bypassed));
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 480 * 2];
        // 窓口が届くまで回す(停止中のまま)
        for _ in 0..300 {
            r.process(&mut buf, 2);
            if r.plugin_frames_processed(sa).is_some() && r.plugin_frames_processed(sb).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let before = (
            r.plugin_frames_processed(sa).unwrap(),
            r.plugin_frames_processed(sb).unwrap(),
        );
        for _ in 0..100 {
            r.process(&mut buf, 2);
        }
        let after = (
            r.plugin_frames_processed(sa).unwrap(),
            r.plugin_frames_processed(sb).unwrap(),
        );
        eprintln!("処理したフレーム数: {before:?} → {after:?}");
        assert_eq!(
            after.0 - before.0,
            48_000,
            "無音のトラックのエフェクトも処理される"
        );
        assert_eq!(
            after.1 - before.1,
            48_000,
            "バイパス中のエフェクトも処理される"
        );
    }

    /// プリセットのつまみを変えた音を目標にし、元のつまみから合わせると近づく(`GLAUX_TEST_CLAP`)。
    #[test]
    fn plugin_params_fit_moves_toward_the_target() {
        let Some(id) = setup() else {
            eprintln!("GLAUX_TEST_CLAP が未設定のためスキップ");
            return;
        };
        let sr = 44_100.0f32;
        let mut r = PluginRenderer::new(&id, sr as f64).unwrap();
        let params = r.params();
        let chosen = crate::sound_match::choose_plugin_params(&params, &[], 9);
        eprintln!(
            "選んだつまみ: {:?}",
            chosen.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
        assert!(chosen
            .iter()
            .any(|p| p.name.to_lowercase().contains("cutoff")));
        let find = |w: &str| {
            chosen
                .iter()
                .find(|p| p.name.to_lowercase().contains(w))
                .unwrap()
                .clone()
        };
        let (sustain, decay) = (find("amp eg sustain"), find("amp eg decay"));
        // 目標: 伸びる音をプラック(サスティン 0・短いディケイ)にした音
        let spec = PresetRenderSpec {
            pitch: 57,
            velocity: 0.8,
            hold: 0.8,
            total: 1.2,
            sample_rate: sr as f64,
        };
        let target_vals = [
            (sustain.id, sustain.min),
            (decay.id, decay.min + (decay.max - decay.min) * 0.3),
        ];
        let target = r.render(&target_vals, spec).unwrap();
        let start: Vec<(u32, f64)> = chosen.iter().map(|p| (p.id, p.start)).collect();
        let a = r.render(&start, spec).unwrap();
        let b = r.render(&start, spec).unwrap();
        eprintln!(
            "同じ値で 2 回: {:.3} / 目標との差: {:.3}",
            crate::sound_match::compare(&a, sr, &b, sr).total,
            crate::sound_match::compare(&a, sr, &target, sr).total
        );
        let fit =
            crate::sound_match::fit_plugin(&mut r, &target, sr, 57, 0.8, &chosen, 12.0, 1).unwrap();
        let got = fit.values.iter().find(|(i, _)| *i == sustain.id).unwrap().1;
        eprintln!(
            "{:.3} → {:.3}、{} 回、サスティン {:.3} → {got:.3}(目標 {:.3})",
            fit.initial_distance.total,
            fit.distance.total,
            fit.evaluations,
            sustain.start,
            sustain.min
        );
        assert!(fit.distance.total < fit.initial_distance.total * 0.5);
        assert!((got - sustain.min).abs() < (sustain.start - sustain.min).abs() * 0.5);
    }

    /// 遅延のあるエフェクトを挿したトラックに、ほかのトラックが揃う(`GLAUX_TEST_CLAP_FX`)。
    /// プラグインが再起動を頼んだら(遅延の変化)、窓口を取り戻して起動し直し、新しい遅延で遅延補正する。
    /// `GLAUX_TEST_CLAP_SLEEPY` のテスト用エフェクト(遅延 = 10 × そのインスタンスの起動回数、
    /// 最初の process で再起動を頼む)で確かめる
    #[test]
    fn restart_request_reactivates_and_updates_delay_compensation() {
        use glaux_core::{Effect, FxId};
        let Some(path) = std::env::var_os("GLAUX_TEST_CLAP_SLEEPY").map(PathBuf::from) else {
            eprintln!("GLAUX_TEST_CLAP_SLEEPY が未設定のためスキップ");
            return;
        };
        std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
        let fx_plugin = rescan()
            .into_iter()
            .find(|p| p.path == path)
            .expect("テスト用エフェクトが見つかる")
            .id;
        let mut project = Project::new("t");
        let mut a = Track::new(TrackId::new(), "A", TrackKind::Midi);
        a.effects.push(Effect {
            id: FxId::new(),
            source: PluginSource::Clap {
                plugin_id: fx_plugin,
                state: None,
            },
            bypass: false,
            params: Default::default(),
            ui: Default::default(),
        });
        project.tracks = vec![a, Track::new(TrackId::new(), "B", TrackKind::Midi)];
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        shared
            .playing
            .store(true, std::sync::atomic::Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 480 * 2];
        let mut seen: Vec<u32> = Vec::new();
        for _ in 0..400 {
            r.process(&mut buf, 2);
            let d = r.pdc_delay(1);
            if d > 0 && seen.last() != Some(&d) {
                seen.push(d);
            }
            if seen.len() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(seen, [10, 20], "起動時は 10、再起動で遅延を読み直して 20");
    }

    #[test]
    fn plugin_delay_compensation_aligns_other_tracks() {
        use glaux_core::{Effect, FxId};
        let Some(path) = std::env::var_os("GLAUX_TEST_CLAP_FX").map(PathBuf::from) else {
            eprintln!("GLAUX_TEST_CLAP_FX が未設定のためスキップ");
            return;
        };
        std::env::set_var("GLAUX_CLAP_PATH", path.parent().unwrap());
        let fx_plugin = rescan()
            .into_iter()
            .find(|p| p.is_effect() && !p.is_instrument())
            .unwrap()
            .id;
        // 2 本とも同じ短い音。0 本目にだけ CLAP エフェクト。1 本目をソロで聴く
        let mk = |name: &str| {
            let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
            let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
            if let ClipContent::Midi { notes, .. } = &mut clip.content {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(960),
                    dur: Tick(480),
                    pitch: 60,
                    vel: 110,
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                });
            }
            t.clips.push(clip);
            t
        };
        let mut project = Project::new("t");
        let mut a = mk("A");
        a.effects.push(Effect {
            id: FxId::new(),
            source: PluginSource::Clap {
                plugin_id: fx_plugin,
                state: None,
            },
            bypass: false,
            params: Default::default(),
            ui: Default::default(),
        });
        let mut b = mk("B");
        b.solo = true;
        project.tracks = vec![a, b];
        let shared = Arc::new(Shared::new(Default::default()));
        let manager = PluginManager::start(shared.plugin_slots.clone());
        let mut bank = SampleBank::default();
        bank.plugin_slots = manager.sync(&project, 48_000.0);
        let slot = *bank.plugin_slots.values().next().unwrap();
        shared
            .data
            .store(Arc::new(build_playback_data(&project, 48_000.0, &bank)));
        let onset = |latency: u32| {
            let mut r = Renderer::new(shared.clone());
            let mut buf = vec![0.0f32; 480 * 2];
            for _ in 0..300 {
                r.process(&mut buf, 2);
                if r.plugin_frames_processed(slot.0 as usize).is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            r.set_plugin_latency_for_test(slot.0 as usize, latency);
            shared.seek.store(0, std::sync::atomic::Ordering::Release);
            shared
                .playing
                .store(true, std::sync::atomic::Ordering::Release);
            let mut out = Vec::new();
            for _ in 0..200 {
                r.process(&mut buf, 2);
                out.extend(buf.chunks(2).map(|c| c[0]));
            }
            shared
                .playing
                .store(false, std::sync::atomic::Ordering::Release);
            eprintln!("遅延補正: {}", r.pdc_delay(1));
            let at = out.iter().position(|v| v.abs() > 1e-3).unwrap();
            drop(r);
            at
        };
        let base = onset(0);
        let shifted = onset(480);
        eprintln!("B の鳴り始め: 遅延なし {base} / A が 480 サンプル遅延 {shifted}");
        assert_eq!(shifted - base, 480);
    }
}
