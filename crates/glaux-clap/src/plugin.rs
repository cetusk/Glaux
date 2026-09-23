//! プラグインのインスタンス(メインスレッド側)と音声処理(オーディオスレッド側)。
//!
//! - [`ClapPlugin`]: 生成・起動(activate)・状態の保存と復元。`Send` ではないので、
//!   作ったスレッド(= プラグインのメインスレッド)から動かさないこと
//! - [`ClapProcessor`]: `activate` で得る音声処理の窓口。オーディオスレッドへ渡して
//!   [`process`](ClapProcessor::process) を呼ぶ。バッファは起動時に確保済みで、
//!   `process` 内ではアロケーションしない。止めるときはメインスレッドへ戻して
//!   [`ClapPlugin::deactivate`] に渡す(オーディオスレッドで解放しない)

use crate::host::{GlauxHost, HostMain, HostShared};
use crate::ClapError;
use clack_extensions::audio_ports::{AudioPortFlags, AudioPortInfoBuffer};
use clack_extensions::note_ports::{NoteDialect, NotePortInfoBuffer};
use clack_host::events::event_types::{
    MidiEvent, NoteExpressionEvent, NoteExpressionType, NoteOffEvent, NoteOnEvent, ParamValueEvent,
};
use clack_host::events::Match;
use clack_host::prelude::*;
use std::cell::Cell;
use std::sync::atomic::Ordering;

/// 1 回の `process` で渡せる最大フレーム数。これを超える長さは分割して処理する
pub const MAX_FRAMES: usize = 4096;
/// 1 ブロックに積めるノートイベント数(超えた分は捨てる)
pub const MAX_EVENTS: usize = 512;

/// プラグインへ送るイベント。`time` はブロック先頭からのフレーム位置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteMsg {
    /// `note_id` を付けると、その音だけに効く表現(ピッチ等)を後から送れる
    On {
        time: u32,
        key: u8,
        velocity: f32,
        note_id: Option<u32>,
    },
    Off {
        time: u32,
        key: u8,
    },
    /// 発音中の音をすべて離す(シーク・停止時)
    AllOff {
        time: u32,
    },
    /// パラメータの値(プラグイン固有の単位)
    Param {
        time: u32,
        id: u32,
        value: f64,
    },
    /// MIDI メッセージ(サステインペダル・ピッチベンド等)。MIDI を受けないプラグインには送らない
    Midi {
        time: u32,
        data: [u8; 3],
    },
    /// 1 音だけの音程の変化(半音単位、ノートの ID で指す)
    Tuning {
        time: u32,
        key: u8,
        note_id: u32,
        semitones: f64,
    },
}

impl NoteMsg {
    pub fn time(&self) -> u32 {
        match *self {
            NoteMsg::On { time, .. }
            | NoteMsg::Off { time, .. }
            | NoteMsg::AllOff { time }
            | NoteMsg::Param { time, .. }
            | NoteMsg::Midi { time, .. }
            | NoteMsg::Tuning { time, .. } => time,
        }
    }

    /// 同じ時刻のイベントの並び順(離す → パラメータ → 鳴らす → 表現)
    pub fn order(&self) -> u8 {
        match self {
            NoteMsg::Off { .. } | NoteMsg::AllOff { .. } => 0,
            NoteMsg::Param { .. } | NoteMsg::Midi { .. } => 1,
            NoteMsg::On { .. } => 2,
            NoteMsg::Tuning { .. } => 3,
        }
    }
}

/// プラグインのパラメータの情報。
#[derive(Clone, Debug, PartialEq)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    /// 所属(例 `A/Filter 1`)。無ければ空
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// 整数値だけを取る(選択肢・スイッチ)
    pub stepped: bool,
    pub automatable: bool,
    /// 画面にも出ない内部用 / 読み取り専用
    pub hidden: bool,
    pub readonly: bool,
}

const HOST_NAME: &str = "Glaux";

fn host_info() -> Result<HostInfo, ClapError> {
    HostInfo::new(
        HOST_NAME,
        "Glaux",
        "https://github.com/cetusk/Glaux",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|e| ClapError::Load(e.to_string()))
}

/// メインスレッド側のプラグイン。
pub struct ClapPlugin {
    instance: PluginInstance<GlauxHost>,
    pub id: String,
    /// 画面を開いているか(浮動ウィンドウならプラグイン自身のウィンドウ)
    gui_open: bool,
    /// 画面を入れているホスト側のウィンドウ(埋め込み方式のとき)
    #[cfg(windows)]
    window: Option<crate::window::HostWindow>,
}

/// 画面まわりで起きたこと([`ClapPlugin::gui_tick`] の戻り値)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiEvent {
    None,
    /// 利用者が画面を閉じた
    Closed,
}

impl ClapPlugin {
    /// `path` の `.clap` から `id` のプラグインを作る。呼んだスレッドをメインスレッドとして扱う。
    pub fn new(path: &std::path::Path, id: &str) -> Result<Self, ClapError> {
        crate::host::mark_main_thread();
        let entry = crate::load_entry(path)?;
        let c_id = std::ffi::CString::new(id).map_err(|e| ClapError::Load(e.to_string()))?;
        let instance = PluginInstance::<GlauxHost>::new(
            |_| HostShared::default(),
            |_| HostMain {
                _shared: std::marker::PhantomData,
                dirty: Cell::new(false),
                ports_changed: Cell::new(false),
                preset_result: Default::default(),
            },
            &entry,
            &c_id,
            &host_info()?,
        )
        .map_err(|e| ClapError::Load(format!("{id}: {e}")))?;
        Ok(ClapPlugin {
            instance,
            id: id.to_owned(),
            gui_open: false,
            #[cfg(windows)]
            window: None,
        })
    }

    /// 音声処理を始められる状態にし、オーディオスレッドへ渡す処理窓口を返す。
    pub fn activate(&mut self, sample_rate: f64) -> Result<ClapProcessor, ClapError> {
        // 音声ポート(入出力とも、宣言された全ポートにバッファを用意する必要がある)
        let (inputs, outputs, main_in, main_out) = self.audio_port_layout();
        let (dialect, midi_ok) = self.note_dialect();
        let config = PluginAudioConfiguration {
            sample_rate,
            min_frames_count: 1,
            max_frames_count: MAX_FRAMES as u32,
        };
        let processor = self
            .instance
            .activate(|_, _| (), config)
            .map_err(|e| ClapError::Activate(format!("{}: {e}", self.id)))?;
        let alloc = |layout: &[u32]| -> Vec<Vec<Vec<f32>>> {
            layout
                .iter()
                .map(|&ch| (0..ch).map(|_| vec![0.0f32; MAX_FRAMES]).collect())
                .collect()
        };
        let total = |layout: &[u32]| layout.iter().sum::<u32>() as usize;
        // 処理の遅延(サンプル)。起動後に問い合わせる(CLAP の決まり)
        let latency = self
            .instance
            .access_shared_handler(|h| h.latency.get().copied().flatten())
            .map(|ext| ext.get(&self.instance.plugin_handle()))
            .unwrap_or(0);
        Ok(ClapProcessor {
            latency,
            processor: Some(processor.into()),
            in_ports: AudioPorts::with_capacity(total(&inputs), inputs.len()),
            out_ports: AudioPorts::with_capacity(total(&outputs), outputs.len()),
            in_bufs: alloc(&inputs),
            out_bufs: alloc(&outputs),
            main_in,
            main_out,
            dialect,
            midi_ok,
            events: EventBuffer::with_capacity(MAX_EVENTS),
            steady: 0,
            failed: false,
        })
    }

    /// オーディオスレッドから戻ってきた処理窓口を受け取って止める。
    pub fn deactivate(&mut self, mut processor: ClapProcessor) {
        if let Some(p) = processor.processor.take() {
            self.instance.deactivate(p.into_stopped());
        }
    }

    pub fn is_active(&self) -> bool {
        self.instance.is_active()
    }

    /// (入力ポートごとのチャンネル数, 出力ポートごとのチャンネル数, メイン出力の添字)
    /// (入力ポートのチャンネル数, 出力ポートのチャンネル数, メイン入力, メイン出力)
    fn audio_port_layout(&mut self) -> (Vec<u32>, Vec<u32>, Option<usize>, usize) {
        let ext = self
            .instance
            .access_shared_handler(|h| h.audio_ports.get().copied().flatten());
        let Some(ext) = ext else {
            // 拡張が無いプラグインはステレオ出力 1 本とみなす
            return (vec![], vec![2], None, 0);
        };
        let handle = self.instance.plugin_handle();
        let mut buf = AudioPortInfoBuffer::new();
        let mut list = |is_input: bool| -> Vec<(u32, bool)> {
            (0..ext.count(&handle, is_input))
                .filter_map(|i| {
                    ext.get(&handle, i, is_input, &mut buf).map(|info| {
                        (
                            info.channel_count,
                            info.flags.contains(AudioPortFlags::IS_MAIN),
                        )
                    })
                })
                .collect()
        };
        let ins = list(true);
        let outs = list(false);
        let main_out = outs.iter().position(|(_, main)| *main).unwrap_or(0);
        let main_in = if ins.is_empty() {
            None
        } else {
            Some(ins.iter().position(|(_, main)| *main).unwrap_or(0))
        };
        (
            ins.iter().map(|(c, _)| *c).collect(),
            outs.iter().map(|(c, _)| *c).collect(),
            main_in,
            main_out,
        )
    }

    /// ノートを送る方式(ノート入力が無ければ None)と、MIDI メッセージを受けるか。
    fn note_dialect(&mut self) -> (Option<NoteDialect>, bool) {
        use clack_extensions::note_ports::NoteDialects;
        let Some(ext) = self
            .instance
            .access_shared_handler(|h| h.note_ports.get().copied().flatten())
        else {
            return (None, false);
        };
        let handle = self.instance.plugin_handle();
        if ext.count(&handle, true) == 0 {
            return (None, false);
        }
        let mut buf = NotePortInfoBuffer::new();
        let Some(info) = ext.get(&handle, 0, true, &mut buf) else {
            return (None, false);
        };
        let midi_ok = info.supported_dialects.contains(NoteDialects::MIDI);
        if info.preferred_dialect == Some(NoteDialect::Clap)
            || info.supported_dialects.contains(NoteDialects::CLAP)
        {
            (Some(NoteDialect::Clap), midi_ok)
        } else {
            (Some(NoteDialect::Midi), midi_ok)
        }
    }

    fn params_ext(&self) -> Option<clack_extensions::params::PluginParams> {
        self.instance
            .access_shared_handler(|h| h.params.get().copied().flatten())
    }

    /// パラメータの一覧(公開されている全部)。
    pub fn param_infos(&mut self) -> Vec<ParamInfo> {
        use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags};
        let Some(ext) = self.params_ext() else {
            return vec![];
        };
        let handle = self.instance.plugin_handle();
        let mut buf = ParamInfoBuffer::new();
        let text = |b: &[u8]| {
            let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).trim().to_owned()
        };
        (0..ext.count(&handle))
            .filter_map(|i| {
                let info = ext.get_info(&handle, i, &mut buf)?;
                Some(ParamInfo {
                    id: info.id.get(),
                    name: text(info.name),
                    module: text(info.module),
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    stepped: info.flags.contains(ParamInfoFlags::IS_STEPPED),
                    automatable: info.flags.contains(ParamInfoFlags::IS_AUTOMATABLE),
                    hidden: info.flags.contains(ParamInfoFlags::IS_HIDDEN),
                    readonly: info.flags.contains(ParamInfoFlags::IS_READONLY),
                })
            })
            .collect()
    }

    /// パラメータの今の値と表示用の文字列(例 `1200 Hz`)。読めないものは飛ばす。
    pub fn param_values(&mut self, ids: &[u32]) -> Vec<(u32, f64, String)> {
        let Some(ext) = self.params_ext() else {
            return vec![];
        };
        let handle = self.instance.plugin_handle();
        let mut buf = [0u8; 128];
        ids.iter()
            .filter_map(|&id| {
                let cid = ClapId::from_raw(id)?;
                let v = ext.get_value(&handle, cid)?;
                let text = ext
                    .value_to_text(&handle, cid, v, &mut buf)
                    .map(|b| String::from_utf8_lossy(b).trim().to_owned())
                    .unwrap_or_default();
                Some((id, v, text))
            })
            .collect()
    }

    /// プラグインの状態を保存する(不透明なバイト列)。
    pub fn save_state(&mut self) -> Result<Vec<u8>, ClapError> {
        let ext = self
            .instance
            .access_shared_handler(|h| h.state.get().copied().flatten())
            .ok_or_else(|| ClapError::State("状態の保存に対応していないプラグインです".into()))?;
        let mut out = Vec::new();
        ext.save(&self.instance.plugin_handle(), &mut out)
            .map_err(|e| ClapError::State(e.to_string()))?;
        self.instance.access_handler(|h| h.dirty.set(false));
        Ok(out)
    }

    /// 保存しておいた状態を戻す。
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), ClapError> {
        let ext = self
            .instance
            .access_shared_handler(|h| h.state.get().copied().flatten())
            .ok_or_else(|| ClapError::State("状態の復元に対応していないプラグインです".into()))?;
        let mut reader = std::io::Cursor::new(bytes);
        ext.load(&self.instance.plugin_handle(), &mut reader)
            .map_err(|e| ClapError::State(e.to_string()))?;
        self.instance.access_handler(|h| h.dirty.set(false));
        Ok(())
    }

    /// プリセットを読み込めるプラグインか(CLAP の preset-load 拡張)。
    pub fn can_load_presets(&self) -> bool {
        self.instance
            .access_shared_handler(|h| h.preset_load.get().copied().flatten())
            .is_some()
    }

    /// プリセットのファイルを読み込む(`load_key` はプリセット一覧が返した値。無ければ None)。
    pub fn load_preset_file(
        &mut self,
        path: &std::path::Path,
        load_key: Option<&str>,
    ) -> Result<(), ClapError> {
        self.load_preset(&crate::PresetLocation::File(path.to_owned()), load_key)
    }

    /// プリセットを読み込む([`crate::list_presets`] が返した在りかと load_key)。
    pub fn load_preset(
        &mut self,
        location: &crate::PresetLocation,
        load_key: Option<&str>,
    ) -> Result<(), ClapError> {
        use clack_extensions::preset_discovery::preset_data::Location;
        let ext = self
            .instance
            .access_shared_handler(|h| h.preset_load.get().copied().flatten())
            .ok_or_else(|| {
                ClapError::State("プリセットの読み込みに対応していないプラグインです".into())
            })?;
        let c_path = match location {
            crate::PresetLocation::File(p) => Some(
                std::ffi::CString::new(p.to_string_lossy().as_bytes())
                    .map_err(|e| ClapError::State(e.to_string()))?,
            ),
            crate::PresetLocation::Plugin => None,
        };
        let loc = match &c_path {
            Some(c) => Location::File { path: c },
            None => Location::Plugin,
        };
        let c_key = load_key
            .filter(|k| !k.is_empty())
            .map(std::ffi::CString::new)
            .transpose()
            .map_err(|e| ClapError::State(e.to_string()))?;
        self.instance
            .access_handler(|h| *h.preset_result.borrow_mut() = None);
        ext.load_from_location(&self.instance.plugin_handle(), loc, c_key.as_deref())
            .map_err(|_| ClapError::State(format!("プリセットを読み込めません: {location:?}")))?;
        // プラグインが失敗を知らせてきていればエラー
        match self
            .instance
            .access_handler(|h| h.preset_result.borrow_mut().take())
        {
            Some(Err(msg)) => Err(ClapError::State(msg)),
            _ => Ok(()),
        }
    }

    /// プラグインから「状態が変わった」と知らされていたら true(読むと戻る)。
    pub fn take_dirty(&mut self) -> bool {
        self.instance.access_handler(|h| h.dirty.replace(false))
    }

    pub fn has_gui(&self) -> bool {
        self.instance
            .access_shared_handler(|h| h.gui.get().copied().flatten())
            .is_some()
    }

    pub fn is_gui_open(&self) -> bool {
        self.gui_open
    }

    /// プラグインの画面を開く(開いていれば前面に出す)。
    pub fn open_gui(&mut self, title: &str) -> Result<(), ClapError> {
        #[cfg(windows)]
        if let Some(w) = &self.window {
            w.show();
            return Ok(());
        }
        if self.gui_open {
            return Ok(());
        }
        let gui = self
            .instance
            .access_shared_handler(|h| h.gui.get().copied().flatten())
            .ok_or_else(|| ClapError::Gui("このプラグインには画面がありません".into()))?;
        self.instance
            .access_shared_handler(|h| h.gui_closed.store(false, Ordering::Release));
        self.open_gui_platform(gui, title)?;
        self.gui_open = true;
        Ok(())
    }

    #[cfg(windows)]
    fn open_gui_platform(
        &mut self,
        gui: clack_extensions::gui::PluginGui,
        title: &str,
    ) -> Result<(), ClapError> {
        use clack_extensions::gui::{GuiApiType, GuiConfiguration, Window};
        let handle = self.instance.plugin_handle();
        let embedded = GuiConfiguration {
            api_type: GuiApiType::WIN32,
            is_floating: false,
        };
        if gui.is_api_supported(&handle, embedded) {
            gui.create(&handle, embedded)
                .map_err(|e| ClapError::Gui(format!("{e:?}")))?;
            let size = gui
                .get_size(&handle)
                .unwrap_or(clack_extensions::gui::GuiSize {
                    width: 800,
                    height: 500,
                });
            let resizable = gui.can_resize(&handle);
            let window = crate::window::HostWindow::new(title, size.width, size.height, resizable)
                .map_err(ClapError::Gui)?;
            // SAFETY: ウィンドウは画面を破棄する(close_gui)まで生かしておく
            let set = unsafe {
                gui.set_parent(
                    &handle,
                    Window::from_generic_ptr(GuiApiType::WIN32, window.hwnd()),
                )
            };
            if let Err(e) = set {
                gui.destroy(&handle);
                return Err(ClapError::Gui(format!("{e:?}")));
            }
            let _ = gui.show(&handle);
            window.show();
            self.window = Some(window);
            return Ok(());
        }
        let floating = GuiConfiguration {
            api_type: GuiApiType::WIN32,
            is_floating: true,
        };
        if gui.is_api_supported(&handle, floating) {
            gui.create(&handle, floating)
                .map_err(|e| ClapError::Gui(format!("{e:?}")))?;
            if let Ok(t) = std::ffi::CString::new(title) {
                gui.suggest_title(&handle, &t);
            }
            gui.show(&handle)
                .map_err(|e| ClapError::Gui(format!("{e:?}")))?;
            return Ok(());
        }
        Err(ClapError::Gui(
            "Windows の画面に対応していないプラグインです".into(),
        ))
    }

    #[cfg(not(windows))]
    fn open_gui_platform(
        &mut self,
        _gui: clack_extensions::gui::PluginGui,
        _title: &str,
    ) -> Result<(), ClapError> {
        Err(ClapError::Gui(
            "この OS ではまだプラグインの画面を開けません(Windows のみ対応)".into(),
        ))
    }

    /// プラグインの画面を閉じる。
    pub fn close_gui(&mut self) {
        if !self.gui_open {
            return;
        }
        if let Some(gui) = self
            .instance
            .access_shared_handler(|h| h.gui.get().copied().flatten())
        {
            gui.destroy(&self.instance.plugin_handle());
        }
        #[cfg(windows)]
        {
            self.window = None;
        }
        self.gui_open = false;
    }

    /// 画面まわりの定期処理(大きさの要求・閉じる操作)。メインスレッドでこまめに呼ぶ。
    pub fn gui_tick(&mut self) -> GuiEvent {
        if !self.gui_open {
            return GuiEvent::None;
        }
        let closed_by_plugin = self
            .instance
            .access_shared_handler(|h| h.gui_closed.swap(false, Ordering::AcqRel));
        #[cfg(windows)]
        let closed_by_user = self
            .window
            .as_ref()
            .is_some_and(|w| w.take_close_requested());
        #[cfg(not(windows))]
        let closed_by_user = false;
        if closed_by_plugin || closed_by_user {
            self.close_gui();
            return GuiEvent::Closed;
        }
        let requested = self
            .instance
            .access_shared_handler(|h| h.requested_size.swap(0, Ordering::AcqRel));
        #[cfg(windows)]
        {
            if requested != 0 {
                if let Some(w) = &self.window {
                    w.set_client_size((requested >> 32) as u32, (requested & 0xFFFF_FFFF) as u32);
                }
            }
            // 利用者がウィンドウの大きさを変えたらプラグインに伝える
            let resized = self.window.as_ref().and_then(|w| w.take_resized());
            if let Some((width, height)) = resized {
                if let Some(gui) = self
                    .instance
                    .access_shared_handler(|h| h.gui.get().copied().flatten())
                {
                    let handle = self.instance.plugin_handle();
                    if gui.can_resize(&handle) {
                        let size = clack_extensions::gui::GuiSize { width, height };
                        let size = gui.adjust_size(&handle, size).unwrap_or(size);
                        let _ = gui.set_size(&handle, size);
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = requested;
        GuiEvent::None
    }

    /// メインスレッドでの定期処理(プラグインが頼んだコールバックを呼ぶ)。
    pub fn poll(&mut self) {
        let requested = self
            .instance
            .access_shared_handler(|h| h.callback_requested.swap(false, Ordering::AcqRel));
        if requested {
            self.instance.call_on_main_thread_callback();
        }
    }
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        // 画面はインスタンスより先に片付ける
        self.close_gui();
    }
}

/// オーディオスレッド側の処理窓口。
pub struct ClapProcessor {
    processor: Option<PluginAudioProcessor<GlauxHost>>,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    /// [ポート][チャンネル][フレーム](起動時に確保)
    in_bufs: Vec<Vec<Vec<f32>>>,
    out_bufs: Vec<Vec<Vec<f32>>>,
    /// メイン入力のポート(エフェクト)。入力の無いプラグイン(音源)は None
    main_in: Option<usize>,
    main_out: usize,
    dialect: Option<NoteDialect>,
    /// MIDI メッセージを受けるか(ペダル・ピッチベンド)
    midi_ok: bool,
    events: EventBuffer,
    steady: u64,
    /// プラグインが申告した処理の遅延(サンプル)
    latency: u32,
    /// 処理に失敗した(以後は無音を返す)
    failed: bool,
}

impl ClapProcessor {
    /// `frames` フレームぶん処理する。`notes` は時刻順であること。
    /// 結果はメイン出力の左右([`output`](Self::output))。
    pub fn process(&mut self, frames: usize, notes: &[NoteMsg]) {
        crate::host::mark_audio_thread();
        let mut done = 0;
        let mut next_note = 0;
        while done < frames {
            let n = (frames - done).min(MAX_FRAMES);
            // このチャンクに入るノートを相対時刻に直して積む
            self.events.clear();
            let mut pushed = 0;
            while next_note < notes.len() && (notes[next_note].time() as usize) < done + n {
                if pushed < MAX_EVENTS {
                    self.push_note(notes[next_note], done as u32);
                    pushed += 1;
                }
                next_note += 1;
            }
            self.process_chunk(n, done);
            done += n;
        }
    }

    fn push_note(&mut self, msg: NoteMsg, base: u32) {
        let t = msg.time().saturating_sub(base);
        // パラメータはノート入力の無いプラグイン(エフェクト)にも送る
        if let NoteMsg::Param { id, value, .. } = msg {
            if let Some(cid) = ClapId::from_raw(id) {
                self.events
                    .push(&ParamValueEvent::new(t, cid, Pckn::match_all(), value));
            }
            return;
        }
        if let NoteMsg::Midi { data, .. } = msg {
            if self.midi_ok {
                self.events.push(&MidiEvent::new(t, 0, data));
            }
            return;
        }
        let Some(dialect) = self.dialect else {
            return;
        };
        let nid = |id: Option<u32>| -> Match<u32> { id.map_or(Match::All, Match::Specific) };
        match (dialect, msg) {
            (
                NoteDialect::Clap,
                NoteMsg::On {
                    key,
                    velocity,
                    note_id,
                    ..
                },
            ) => self.events.push(&NoteOnEvent::new(
                t,
                Pckn::new(0u16, 0u16, key as u16, nid(note_id)),
                velocity as f64,
            )),
            (NoteDialect::Clap, NoteMsg::Off { key, .. }) => self.events.push(&NoteOffEvent::new(
                t,
                Pckn::new(0u16, 0u16, key as u16, Match::All),
                0.0,
            )),
            (NoteDialect::Clap, NoteMsg::AllOff { .. }) => self.events.push(&NoteOffEvent::new(
                t,
                Pckn::new(0u16, Match::All, Match::All, Match::All),
                0.0,
            )),
            (
                NoteDialect::Clap,
                NoteMsg::Tuning {
                    key,
                    note_id,
                    semitones,
                    ..
                },
            ) => self.events.push(&NoteExpressionEvent::new(
                t,
                Pckn::new(0u16, 0u16, key as u16, note_id),
                NoteExpressionType::Tuning,
                semitones,
            )),
            (_, NoteMsg::On { key, velocity, .. }) => {
                let v = (velocity * 127.0).round().clamp(1.0, 127.0) as u8;
                self.events
                    .push(&MidiEvent::new(t, 0, [0x90, key & 0x7F, v]))
            }
            (_, NoteMsg::Off { key, .. }) => {
                self.events
                    .push(&MidiEvent::new(t, 0, [0x80, key & 0x7F, 0]))
            }
            (_, NoteMsg::AllOff { .. }) => self.events.push(&MidiEvent::new(t, 0, [0xB0, 123, 0])),
            // MIDI だけのプラグインには 1 音ごとの音程は送れない
            _ => {}
        }
    }

    fn process_chunk(&mut self, n: usize, offset: usize) {
        let _ = offset;
        if self.failed {
            self.clear_outputs(n);
            return;
        }
        let Some(proc) = self.processor.as_mut() else {
            self.clear_outputs(n);
            return;
        };
        let started = match proc.ensure_processing_started() {
            Ok(p) => p,
            Err(_) => {
                self.failed = true;
                self.clear_outputs(n);
                return;
            }
        };
        let inputs = self
            .in_ports
            .with_input_buffers(self.in_bufs.iter_mut().map(|port| AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(port.iter_mut().map(|ch| {
                    InputChannel {
                        buffer: &mut ch[..n],
                        is_constant: false,
                    }
                })),
            }));
        let mut outputs = self
            .out_ports
            .with_output_buffers(self.out_bufs.iter_mut().map(|port| AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    port.iter_mut().map(|ch| &mut ch[..n]),
                ),
            }));
        let result = started.process(
            &inputs,
            &mut outputs,
            &self.events.as_input(),
            &mut OutputEvents::void(),
            Some(self.steady),
            None,
        );
        self.steady += n as u64;
        if result.is_err() {
            self.failed = true;
            self.clear_outputs(n);
        }
    }

    fn clear_outputs(&mut self, n: usize) {
        for port in self.out_bufs.iter_mut() {
            for ch in port.iter_mut() {
                ch[..n].fill(0.0);
            }
        }
    }

    /// 直前の `process` のメイン出力(左, 右)。長さは `MAX_FRAMES`(先頭の処理した分が有効)。
    /// モノラル出力のプラグインは左右に同じものを返す。出力が無ければ None
    pub fn output(&self) -> Option<(&[f32], &[f32])> {
        let port = self.out_bufs.get(self.main_out)?;
        let l = port.first()?;
        let r = port.get(1).unwrap_or(l);
        Some((l, r))
    }

    /// 連続処理を止める(オーディオスレッドで呼ぶ。メインスレッドへ返す前に)。
    pub fn stop(&mut self) {
        crate::host::mark_audio_thread();
        if let Some(p) = self.processor.as_mut() {
            if p.is_started() {
                let _ = p.stop_processing();
            }
        }
    }

    /// メイン入力の左右(エフェクトに通す音をここに書いてから [`process`](Self::process) を呼ぶ)。
    /// 長さは `MAX_FRAMES`。モノラル入力のプラグインは左右に同じバッファを返せないので、右は None
    pub fn input_mut(&mut self) -> Option<(&mut [f32], Option<&mut [f32]>)> {
        let port = self.in_bufs.get_mut(self.main_in?)?;
        let (first, rest) = port.split_first_mut()?;
        Some((&mut first[..], rest.first_mut().map(|v| &mut v[..])))
    }

    /// プラグインが申告した処理の遅延(サンプル。起動時に取得)。
    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// テスト用: 遅延の申告を差し替える。
    #[doc(hidden)]
    pub fn set_latency_for_test(&mut self, samples: u32) {
        self.latency = samples;
    }

    /// これまでに処理したフレーム数(プラグインへ渡している時刻)。
    pub fn frames_processed(&self) -> u64 {
        self.steady
    }

    /// メイン入力を無音にする(入力を使わないブロックで古い音を渡さないように)。
    pub fn clear_input(&mut self, frames: usize) {
        if let Some(port) = self.main_in.and_then(|i| self.in_bufs.get_mut(i)) {
            for ch in port.iter_mut() {
                let n = frames.min(ch.len());
                ch[..n].fill(0.0);
            }
        }
    }

    /// 音声の入力を受けるか(エフェクトか)。
    pub fn accepts_audio(&self) -> bool {
        self.main_in.is_some()
    }

    /// ノートを受け付けるか(音源か)。
    pub fn accepts_notes(&self) -> bool {
        self.dialect.is_some()
    }

    /// 処理に失敗して止まっているか。
    pub fn has_failed(&self) -> bool {
        self.failed
    }
}

// ClapProcessor はオーディオスレッドへ渡して使う(同時に 2 つのスレッドから触らない)
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ClapProcessor>();
};
