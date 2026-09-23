//! ホスト側のコールバック(プラグインから Glaux への問い合わせ・通知)。
//!
//! CLAP はスレッドの役割を厳密に分ける(`[main-thread]` / `[audio-thread]`)。Glaux では
//! プラグインを作ったスレッドを「メインスレッド」、`process` を呼ぶスレッドを
//! 「オーディオスレッド」として [`mark_main_thread`] / [`mark_audio_thread`] で印を付け、
//! thread-check 拡張でプラグインに答える。

use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPortsImpl, PluginAudioPorts};
use clack_extensions::gui::{GuiSize, HostGuiImpl, PluginGui};
use clack_extensions::latency::{HostLatencyImpl, PluginLatency};
use clack_extensions::log::{HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{
    HostNotePortsImpl, NoteDialects, NotePortRescanFlags, PluginNotePorts,
};
use clack_extensions::params::{
    HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags, PluginParams,
};
use clack_extensions::preset_discovery::preset_data::Location;
use clack_extensions::preset_discovery::{HostPresetLoadImpl, PluginPresetLoad};
use clack_extensions::state::{HostStateImpl, PluginState};
use clack_extensions::thread_check::HostThreadCheckImpl;
use clack_host::prelude::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

thread_local! {
    static IS_MAIN: Cell<bool> = const { Cell::new(false) };
    static IS_AUDIO: Cell<bool> = const { Cell::new(false) };
}

/// このスレッドをプラグインの「メインスレッド」として扱う(生成・状態の保存などを呼ぶスレッド)。
pub fn mark_main_thread() {
    IS_MAIN.with(|c| c.set(true));
}

/// このスレッドをプラグインの「オーディオスレッド」として扱う(`process` を呼ぶスレッド)。
pub fn mark_audio_thread() {
    IS_AUDIO.with(|c| c.set(true));
}

pub struct GlauxHost;

impl HostHandlers for GlauxHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMain<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<clack_extensions::log::HostLog>()
            .register::<clack_extensions::thread_check::HostThreadCheck>()
            .register::<clack_extensions::state::HostState>()
            .register::<clack_extensions::audio_ports::HostAudioPorts>()
            .register::<clack_extensions::note_ports::HostNotePorts>()
            .register::<clack_extensions::params::HostParams>()
            .register::<clack_extensions::latency::HostLatency>()
            .register::<clack_extensions::gui::HostGui>()
            .register::<clack_extensions::preset_discovery::HostPresetLoad>();
    }
}

/// どのスレッドからも触れるホスト側の状態(プラグインからの要求フラグと、プラグインの拡張)。
#[derive(Default)]
pub struct HostShared {
    pub callback_requested: AtomicBool,
    pub restart_requested: AtomicBool,
    pub process_requested: AtomicBool,
    pub state: OnceLock<Option<PluginState>>,
    pub audio_ports: OnceLock<Option<PluginAudioPorts>>,
    pub note_ports: OnceLock<Option<PluginNotePorts>>,
    pub gui: OnceLock<Option<PluginGui>>,
    pub params: OnceLock<Option<PluginParams>>,
    pub preset_load: OnceLock<Option<PluginPresetLoad>>,
    pub latency: OnceLock<Option<PluginLatency>>,
    /// プラグインが頼んだ画面の大きさ(幅 << 32 | 高さ。0 = なし)
    pub requested_size: AtomicU64,
    /// プラグインが自分の(浮動)ウィンドウを閉じた
    pub gui_closed: AtomicBool,
}

impl<'a> SharedHandler<'a> for HostShared {
    fn initializing(&self, instance: InitializingPluginHandle<'a>) {
        let _ = self.state.set(instance.get_extension());
        let _ = self.audio_ports.set(instance.get_extension());
        let _ = self.note_ports.set(instance.get_extension());
        let _ = self.gui.set(instance.get_extension());
        let _ = self.params.set(instance.get_extension());
        let _ = self.preset_load.set(instance.get_extension());
        let _ = self.latency.set(instance.get_extension());
    }

    fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::Release);
    }

    fn request_process(&self) {
        self.process_requested.store(true, Ordering::Release);
    }

    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::Release);
    }
}

impl HostLogImpl for HostShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        // オーディオスレッドから呼ばれることもあるが、tracing は既定で出力先が無ければ軽い
        match severity {
            LogSeverity::Error | LogSeverity::Fatal | LogSeverity::HostMisbehaving => {
                tracing::warn!("[CLAP] {message}")
            }
            _ => tracing::debug!("[CLAP] {message}"),
        }
    }
}

impl HostThreadCheckImpl for HostShared {
    fn is_main_thread(&self) -> bool {
        IS_MAIN.with(|c| c.get())
    }

    fn is_audio_thread(&self) -> bool {
        IS_AUDIO.with(|c| c.get())
    }
}

impl HostGuiImpl for HostShared {
    fn resize_hints_changed(&self) {}

    fn request_resize(&self, new_size: GuiSize) -> Result<(), HostError> {
        let packed = ((new_size.width as u64) << 32) | new_size.height as u64;
        self.requested_size.store(packed.max(1), Ordering::Release);
        Ok(())
    }

    fn request_show(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn request_hide(&self) -> Result<(), HostError> {
        Ok(())
    }

    fn closed(&self, _was_destroyed: bool) {
        self.gui_closed.store(true, Ordering::Release);
    }
}

impl HostParamsImplShared for HostShared {
    fn request_flush(&self) {
        self.process_requested.store(true, Ordering::Release);
    }
}

/// メインスレッド専用のホスト側の状態。
pub struct HostMain<'a> {
    pub _shared: std::marker::PhantomData<&'a HostShared>,
    /// プラグインが「状態が変わった」と知らせた(画面での操作など)
    pub dirty: Cell<bool>,
    /// 音声ポート・ノートポートの構成が変わった(作り直しが必要)
    pub ports_changed: Cell<bool>,
    /// プリセットの読み込み結果(読み込めた / 失敗の理由)
    pub preset_result: std::cell::RefCell<Option<Result<(), String>>>,
}

impl<'a> MainThreadHandler<'a> for HostMain<'a> {
    fn initialized(&self, _instance: InitializedPluginHandle<'a>) {}
}

impl HostStateImpl for HostMain<'_> {
    fn mark_dirty(&self) {
        self.dirty.set(true);
    }
}

impl HostPresetLoadImpl for HostMain<'_> {
    fn on_error(
        &self,
        _location: Location,
        _load_key: Option<&std::ffi::CStr>,
        os_error: i32,
        message: Option<&std::ffi::CStr>,
    ) {
        let msg = message
            .map(|m| m.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("エラー {os_error}"));
        *self.preset_result.borrow_mut() = Some(Err(msg));
    }

    fn loaded(&self, _location: Location, _load_key: Option<&std::ffi::CStr>) {
        *self.preset_result.borrow_mut() = Some(Ok(()));
    }
}

impl HostAudioPortsImpl for HostMain<'_> {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }

    fn rescan(&self, _flags: AudioPortRescanFlags) {
        self.ports_changed.set(true);
    }
}

impl HostNotePortsImpl for HostMain<'_> {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }

    fn rescan(&self, _flags: NotePortRescanFlags) {
        self.ports_changed.set(true);
    }
}

impl HostParamsImplMainThread for HostMain<'_> {
    fn rescan(&self, _flags: ParamRescanFlags) {}

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostLatencyImpl for HostMain<'_> {
    fn changed(&self) {}
}
