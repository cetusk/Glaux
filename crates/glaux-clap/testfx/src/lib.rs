//! Glaux のテスト用の CLAP エフェクト(配布しない。使い方は Cargo.toml の先頭)。
//! - 出力 = 入力 + 0.001(処理されたかが分かる)
//! - 入力が無音なら Sleep を返す
//! - 遅延 = 10 × そのインスタンスの起動回数。インスタンスごとに、最初の process で 1 回だけ再起動を頼む
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::latency::{PluginLatency, PluginLatencyImpl};
use clack_plugin::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub struct TestFx;

pub struct Shared<'a> {
    host: HostSharedHandle<'a>,
    /// このインスタンスの起動回数と、再起動を頼んだか
    activations: AtomicU32,
    asked: AtomicBool,
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
            .register::<PluginLatency>();
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
        _p: Process,
        mut audio: Audio,
        _e: Events,
    ) -> Result<ProcessStatus, PluginError> {
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
                            *b = a + 0.001;
                        }
                    }
                    ChannelPair::InPlace(b) => {
                        for s in b.iter_mut() {
                            if *s != 0.0 {
                                silent = false;
                            }
                            *s += 0.001;
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
