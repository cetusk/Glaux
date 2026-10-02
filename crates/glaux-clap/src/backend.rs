//! 外から使うプラグインの型。中身は CLAP か VST3 のどちらかで、公開のメソッドはすべてそこへ振り分ける
//! (エンジン・MCP・アプリはどちらの形式かを意識しない)。VST3 のプラグイン ID は `vst3:<クラス ID の 16 進>`。
//!
//! - [`ClapPlugin`]: 生成・起動(activate)・状態の保存と復元・画面。`Send` ではないので、
//!   作ったスレッド(= プラグインのメインスレッド)から動かさないこと
//! - [`ClapProcessor`]: `activate` で得る音声処理の窓口。オーディオスレッドへ渡して `process` を呼ぶ。
//!   止めるときはメインスレッドへ戻して [`ClapPlugin::deactivate`] に渡す

use crate::plugin::{ClapImpl, ClapProcImpl};
use crate::vst3host::{Vst3Plugin, Vst3Processor};
use crate::{ClapError, GuiEvent, HostTransport, NoteMsg, ParamInfo};

/// VST3 のプラグイン ID の頭
pub const VST3_PREFIX: &str = "vst3:";

/// プラグイン ID が VST3 のものか
pub fn is_vst3_id(id: &str) -> bool {
    id.starts_with(VST3_PREFIX)
}

enum Inner {
    Clap(Box<ClapImpl>),
    Vst3(Box<Vst3Plugin>),
}

/// メインスレッド側のプラグイン(CLAP か VST3)。
pub struct ClapPlugin {
    pub id: String,
    inner: Inner,
}

enum ProcInner {
    Clap(Box<ClapProcImpl>),
    Vst3(Box<Vst3Processor>),
}

/// オーディオスレッド側の処理窓口(CLAP か VST3)。
pub struct ClapProcessor {
    inner: ProcInner,
}

macro_rules! each {
    ($self:ident, $p:ident => $e:expr) => {
        match &$self.inner {
            Inner::Clap($p) => $e,
            Inner::Vst3($p) => $e,
        }
    };
    (mut $self:ident, $p:ident => $e:expr) => {
        match &mut $self.inner {
            Inner::Clap($p) => $e,
            Inner::Vst3($p) => $e,
        }
    };
}

macro_rules! each_proc {
    ($self:ident, $p:ident => $e:expr) => {
        match &$self.inner {
            ProcInner::Clap($p) => $e,
            ProcInner::Vst3($p) => $e,
        }
    };
    (mut $self:ident, $p:ident => $e:expr) => {
        match &mut $self.inner {
            ProcInner::Clap($p) => $e,
            ProcInner::Vst3($p) => $e,
        }
    };
}

impl ClapPlugin {
    /// `path`(`.clap` か `.vst3`)から `id` のプラグインを作る。呼んだスレッドをメインスレッドとして扱う。
    pub fn new(path: &std::path::Path, id: &str) -> Result<Self, ClapError> {
        let inner = if is_vst3_id(id) {
            Inner::Vst3(Box::new(Vst3Plugin::new(path, id)?))
        } else {
            Inner::Clap(Box::new(ClapImpl::new(path, id)?))
        };
        Ok(ClapPlugin {
            id: id.to_owned(),
            inner,
        })
    }

    /// VST3 のプラグインか
    pub fn is_vst3(&self) -> bool {
        matches!(self.inner, Inner::Vst3(_))
    }

    pub fn activate(&mut self, sample_rate: f64) -> Result<ClapProcessor, ClapError> {
        let inner = match &mut self.inner {
            Inner::Clap(p) => ProcInner::Clap(Box::new(p.activate(sample_rate)?)),
            Inner::Vst3(p) => ProcInner::Vst3(Box::new(p.activate(sample_rate)?)),
        };
        Ok(ClapProcessor { inner })
    }

    /// オーディオスレッドから戻ってきた処理窓口を受け取って止める。
    pub fn deactivate(&mut self, processor: ClapProcessor) {
        match (&mut self.inner, processor.inner) {
            (Inner::Clap(p), ProcInner::Clap(q)) => p.deactivate(*q),
            (Inner::Vst3(p), ProcInner::Vst3(q)) => p.deactivate(*q),
            // 形式の違う窓口は来ない(来たらそのまま捨てる)
            _ => {}
        }
    }

    pub fn is_active(&self) -> bool {
        each!(self, p => p.is_active())
    }

    pub fn param_infos(&mut self) -> Vec<ParamInfo> {
        each!(mut self, p => p.param_infos())
    }

    pub fn param_values(&mut self, ids: &[u32]) -> Vec<(u32, f64, String)> {
        each!(mut self, p => p.param_values(ids))
    }

    pub fn save_state(&mut self) -> Result<Vec<u8>, ClapError> {
        each!(mut self, p => p.save_state())
    }

    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), ClapError> {
        each!(mut self, p => p.load_state(bytes))
    }

    pub fn can_load_presets(&self) -> bool {
        match &self.inner {
            Inner::Clap(p) => p.can_load_presets(),
            Inner::Vst3(_) => false,
        }
    }

    pub fn load_preset_file(
        &mut self,
        path: &std::path::Path,
        load_key: Option<&str>,
    ) -> Result<(), ClapError> {
        match &mut self.inner {
            Inner::Clap(p) => p.load_preset_file(path, load_key),
            Inner::Vst3(_) => Err(vst3_no_presets()),
        }
    }

    pub fn load_preset(
        &mut self,
        location: &crate::PresetLocation,
        load_key: Option<&str>,
    ) -> Result<(), ClapError> {
        match &mut self.inner {
            Inner::Clap(p) => p.load_preset(location, load_key),
            Inner::Vst3(_) => Err(vst3_no_presets()),
        }
    }

    pub fn take_restart_requested(&self) -> bool {
        each!(self, p => p.take_restart_requested())
    }

    pub fn take_dirty(&mut self) -> bool {
        each!(mut self, p => p.take_dirty())
    }

    pub fn has_gui(&self) -> bool {
        each!(self, p => p.has_gui())
    }

    pub fn is_gui_open(&self) -> bool {
        each!(self, p => p.is_gui_open())
    }

    pub fn open_gui(&mut self, title: &str) -> Result<(), ClapError> {
        each!(mut self, p => p.open_gui(title))
    }

    /// テスト用: 埋め込んだ画面の中身を (幅, 高さ, RGB) で読む(Linux のみ)
    #[cfg(target_os = "linux")]
    #[doc(hidden)]
    pub fn capture_gui_for_test(&self) -> Option<(u32, u32, Vec<u8>)> {
        each!(self, p => p.capture_gui_for_test())
    }

    pub fn close_gui(&mut self) {
        each!(mut self, p => p.close_gui())
    }

    pub fn gui_tick(&mut self) -> GuiEvent {
        each!(mut self, p => p.gui_tick())
    }

    pub fn poll(&mut self) {
        each!(mut self, p => p.poll())
    }
}

fn vst3_no_presets() -> ClapError {
    ClapError::State("VST3 のプリセットの読み込みにはまだ対応していません".into())
}

impl ClapProcessor {
    pub fn set_transport(&mut self, t: Option<HostTransport>) {
        each_proc!(mut self, p => p.set_transport(t))
    }

    pub fn process(&mut self, frames: usize, notes: &[NoteMsg]) {
        each_proc!(mut self, p => p.process(frames, notes))
    }

    pub fn output(&self) -> Option<(&[f32], &[f32])> {
        each_proc!(self, p => p.output())
    }

    pub fn stop(&mut self) {
        each_proc!(mut self, p => p.stop())
    }

    pub fn input_mut(&mut self) -> Option<(&mut [f32], Option<&mut [f32]>)> {
        each_proc!(mut self, p => p.input_mut())
    }

    pub fn latency(&self) -> u32 {
        each_proc!(self, p => p.latency())
    }

    pub fn can_modulate(&self, id: u32) -> bool {
        each_proc!(self, p => p.can_modulate(id))
    }

    pub fn can_modulate_per_note(&self, id: u32) -> bool {
        each_proc!(self, p => p.can_modulate_per_note(id))
    }

    #[doc(hidden)]
    pub fn set_latency_for_test(&mut self, samples: u32) {
        each_proc!(mut self, p => p.set_latency_for_test(samples))
    }

    pub fn frames_processed(&self) -> u64 {
        each_proc!(self, p => p.frames_processed())
    }

    pub fn clear_input(&mut self, frames: usize) {
        each_proc!(mut self, p => p.clear_input(frames))
    }

    pub fn accepts_audio(&self) -> bool {
        each_proc!(self, p => p.accepts_audio())
    }

    pub fn accepts_notes(&self) -> bool {
        each_proc!(self, p => p.accepts_notes())
    }

    pub fn has_failed(&self) -> bool {
        each_proc!(self, p => p.has_failed())
    }
}

// ClapProcessor はオーディオスレッドへ渡して使う(同時に 2 つのスレッドから触らない)
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ClapProcessor>();
};
