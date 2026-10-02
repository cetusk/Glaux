//! Glaux のテスト用の CLAP エフェクト(配布しない。使い方は Cargo.toml の先頭)。
//! - 出力 = 入力 + 0.001(処理されたかが分かる)。トランスポートを受け取ったら、さらにテンポ × 1e-6
//!   (再生中ならさらに 1e-5)を足す(ホストがテンポ・再生中を渡したかが分かる)
//! - 入力が無音なら Sleep を返す
//! - 遅延 = 10 × そのインスタンスの起動回数。インスタンスごとに、最初の process で 1 回だけ再起動を頼む
//! - パラメータ `probe`(id 7、0〜1、変調できる・1 音ごとにも変調できる): 出力に 値 × 0.01 + 変調 × 0.1 +
//!   1 音ごとの変調 × 1.0 を足す(ホストが値と変調を分けて送ったかが分かる。値は変調で変わらない)
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::events::Match;
use clack_plugin::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const PROBE: u32 = 7;

pub struct TestFx;

pub struct Shared<'a> {
    host: HostSharedHandle<'a>,
    /// このインスタンスの起動回数と、再起動を頼んだか
    activations: AtomicU32,
    asked: AtomicBool,
    /// probe の値・変調・1 音ごとの変調(f32 のビット)
    value: AtomicU32,
    modulation: AtomicU32,
    note_mod: AtomicU32,
}

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::SeqCst))
}
fn store(a: &AtomicU32, v: f64) {
    a.store((v as f32).to_bits(), Ordering::SeqCst);
}

impl Shared<'_> {
    fn apply(&self, events: &InputEvents) {
        for e in events {
            match e.as_core_event() {
                Some(CoreEventSpace::ParamValue(v)) if v.param_id().map(|i| i.get()) == Some(PROBE) => {
                    store(&self.value, v.value());
                }
                Some(CoreEventSpace::ParamMod(m)) if m.param_id().map(|i| i.get()) == Some(PROBE) => {
                    match m.note_id() {
                        Match::Specific(_) => store(&self.note_mod, m.amount()),
                        _ => store(&self.modulation, m.amount()),
                    }
                }
                _ => {}
            }
        }
    }
}
impl<'a> PluginShared<'a> for Shared<'a> {}

pub struct Main<'a> {
    shared: &'a Shared<'a>,
}
impl<'a> PluginMainThread<'a, Shared<'a>> for Main<'a> {}

impl PluginLatencyImpl for Main<'_> {
    fn get(&self) -> u32 {
        10 * self.shared.activations.load(Ordering::SeqCst)
    }
}

impl PluginMainThreadParams for Main<'_> {
    fn count(&self) -> u32 {
        1
    }
    fn get_info(&self, index: u32, info: &mut ParamInfoWriter) {
        if index == 0 {
            info.set(&ParamInfo {
                id: ClapId::new(PROBE),
                flags: ParamInfoFlags::IS_AUTOMATABLE
                    | ParamInfoFlags::IS_MODULATABLE
                    | ParamInfoFlags::IS_MODULATABLE_PER_NOTE_ID,
                cookie: Default::default(),
                name: b"probe",
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: 0.0,
            });
        }
    }
    fn get_value(&self, id: ClapId) -> Option<f64> {
        (id.get() == PROBE).then(|| load(&self.shared.value) as f64)
    }
    fn value_to_text(
        &self,
        _id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        use std::fmt::Write;
        write!(writer, "{value:.3}")
    }
    fn text_to_value(&self, _id: ClapId, text: &std::ffi::CStr) -> Option<f64> {
        text.to_str().ok()?.trim().parse().ok()
    }
    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply(input);
    }
}

impl PluginAudioProcessorParams for Proc<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        self.shared.apply(input);
    }
}

impl PluginAudioPortsImpl for Main<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }
    fn get(&self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl Plugin for TestFx {
    type AudioProcessor<'a> = Proc<'a>;
    type Shared<'a> = Shared<'a>;
    type MainThread<'a> = Main<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&Shared<'_>>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginLatency>()
            .register::<PluginParams>();
    }
}

impl DefaultPluginFactory for TestFx {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;
        PluginDescriptor::new("dev.glaux.testfx", "Glaux Test FX")
            .with_features([AUDIO_EFFECT, STEREO])
    }
    fn new_shared(host: HostSharedHandle<'_>) -> Result<Shared<'_>, PluginError> {
        Ok(Shared {
            host,
            activations: AtomicU32::new(0),
            asked: AtomicBool::new(false),
            value: AtomicU32::new(0),
            modulation: AtomicU32::new(0),
            note_mod: AtomicU32::new(0),
        })
    }
    fn new_main_thread<'a>(
        _host: HostMainThreadHandle<'a>,
        shared: &'a Shared<'a>,
    ) -> Result<Main<'a>, PluginError> {
        Ok(Main { shared })
    }
}

pub struct Proc<'a> {
    shared: &'a Shared<'a>,
}

impl<'a> PluginAudioProcessor<'a, Shared<'a>, Main<'a>> for Proc<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main: &Main<'a>,
        shared: &'a Shared<'a>,
        _cfg: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        shared.activations.fetch_add(1, Ordering::SeqCst);
        Ok(Proc { shared })
    }

    fn process(
        &mut self,
        p: Process,
        mut audio: Audio,
        e: Events,
    ) -> Result<ProcessStatus, PluginError> {
        self.shared.apply(e.input);
        let extra = p.transport.map_or(0.0, |t| {
            let playing = t
                .flags
                .contains(clack_plugin::events::event_types::TransportFlags::IS_PLAYING);
            t.tempo as f32 * 1e-6 + if playing { 1e-5 } else { 0.0 }
        }) + load(&self.shared.value) * 0.01
            + load(&self.shared.modulation) * 0.1
            + load(&self.shared.note_mod);
        if !self.shared.asked.swap(true, Ordering::SeqCst) {
            self.shared.host.request_restart();
        }
        let mut silent = true;
        for mut port in &mut audio {
            let Some(pairs) = port.channels()?.into_f32() else {
                continue;
            };
            for pair in pairs {
                match pair {
                    ChannelPair::InputOutput(i, o) => {
                        for (a, b) in i.iter().zip(o.iter_mut()) {
                            if *a != 0.0 {
                                silent = false;
                            }
                            *b = a + 0.001 + extra;
                        }
                    }
                    ChannelPair::InPlace(b) => {
                        for s in b.iter_mut() {
                            if *s != 0.0 {
                                silent = false;
                            }
                            *s += 0.001 + extra;
                        }
                    }
                    ChannelPair::OutputOnly(b) => b.fill(0.001),
                    ChannelPair::InputOnly(_) => {}
                }
            }
        }
        Ok(if silent {
            ProcessStatus::Sleep
        } else {
            ProcessStatus::Continue
        })
    }
}

clack_export_entry!(SinglePluginEntry<TestFx>);
