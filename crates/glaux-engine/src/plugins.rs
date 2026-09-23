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

use glaux_clap::{ClapPlugin, ClapProcessor, PluginInfo};
use glaux_core::{PluginSource, Project, TrackId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

/// 同時に載せられるプラグイン数
pub const MAX_PLUGINS: usize = 16;

/// オーディオスレッドへ渡す処理窓口(どのインスタンスのものかを世代で見分ける)。
pub struct Processor {
    pub gen: u64,
    pub clap: ClapProcessor,
}

/// 処理窓口の受け渡し口(スロットごとに 1 つ)。
#[derive(Default)]
pub struct PluginSlot {
    incoming: AtomicPtr<Processor>,
    outgoing: AtomicPtr<Processor>,
    /// この世代の窓口を返してほしい(0 = 要求なし)
    remove_gen: AtomicU64,
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
        info: PluginInfo,
        state: Option<Vec<u8>>,
        sample_rate: f64,
    },
    Destroy {
        slot: usize,
        gen: u64,
    },
    LoadState {
        gen: u64,
        bytes: Vec<u8>,
    },
    SaveState {
        gen: u64,
        reply: mpsc::Sender<Result<Vec<u8>, String>>,
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
    plugin: ClapPlugin,
    dying: bool,
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
                info,
                state,
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
                    Ok((plugin, proc)) => {
                        live.push(Live {
                            gen,
                            plugin,
                            dying: false,
                        });
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
            Ok(HostCmd::LoadState { gen, bytes }) => {
                if let Some(l) = live.iter_mut().find(|l| l.gen == gen) {
                    if let Err(e) = l.plugin.load_state(&bytes) {
                        tracing::warn!("CLAP の状態を戻せません: {e}");
                    }
                }
            }
            Ok(HostCmd::SaveState { gen, reply }) => {
                let r = match live.iter_mut().find(|l| l.gen == gen) {
                    Some(l) => l.plugin.save_state().map_err(|e| e.to_string()),
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
        let mut wanted: HashMap<TrackId, (PluginInfo, Option<&str>)> = HashMap::new();
        for t in &project.tracks {
            let Some(d) = &t.device else { continue };
            let PluginSource::Clap { plugin_id, state } = &d.source else {
                continue;
            };
            match find(plugin_id) {
                Some(info) => {
                    wanted.insert(t.id.clone(), (info, state.as_deref()));
                }
                None => tracing::warn!("CLAP プラグインが見つかりません: {plugin_id}"),
            }
        }
        // 要らなくなった・中身が変わったものを片付ける
        let stale: Vec<TrackId> = assigned
            .iter()
            .filter(|(tid, a)| match wanted.get(*tid) {
                None => true,
                Some((info, _)) => info.id != a.plugin_id || a.sample_rate != sample_rate,
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
        for (tid, (info, state)) in wanted {
            let sig = hash_str(state);
            if let Some(a) = assigned.get_mut(&tid) {
                // 状態が外から変わった(取り消し・AI の操作など)なら読み込み直す
                if a.state_sig != sig {
                    a.state_sig = sig;
                    if let Some(bytes) = state.and_then(decode_state) {
                        let _ = self.tx.send(HostCmd::LoadState { gen: a.gen, bytes });
                    }
                }
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
                info: info.clone(),
                state: state.and_then(decode_state),
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

    /// トラックに載っているプラグインの今の状態(base64)。
    pub fn save_state(&self, track: &TrackId) -> Result<String, String> {
        let gen = self.gen_of(track)?;
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(HostCmd::SaveState { gen, reply })
            .map_err(|_| "プラグインのスレッドが止まっています".to_owned())?;
        let bytes = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "プラグインが応答しません".to_owned())??;
        Ok(encode_state(&bytes))
    }

    /// 保存した状態をプロジェクトに書いた後に呼ぶ(同じ状態を読み込み直さないように)。
    pub fn note_state_saved(&self, track: &TrackId, state: &str) {
        if let Some(a) = self
            .assigned
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(track)
        {
            a.state_sig = hash_str(Some(state));
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
        assert!(!state.is_empty());

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
}
