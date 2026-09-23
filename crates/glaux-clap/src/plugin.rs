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
use clack_host::events::event_types::{MidiEvent, NoteOffEvent, NoteOnEvent};
use clack_host::events::Match;
use clack_host::prelude::*;
use std::cell::Cell;
use std::sync::atomic::Ordering;

/// 1 回の `process` で渡せる最大フレーム数。これを超える長さは分割して処理する
pub const MAX_FRAMES: usize = 4096;
/// 1 ブロックに積めるノートイベント数(超えた分は捨てる)
pub const MAX_EVENTS: usize = 512;

/// プラグインへ送るノートのイベント。`time` はブロック先頭からのフレーム位置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoteMsg {
    On {
        time: u32,
        key: u8,
        velocity: f32,
    },
    Off {
        time: u32,
        key: u8,
    },
    /// 発音中の音をすべて離す(シーク・停止時)
    AllOff {
        time: u32,
    },
}

impl NoteMsg {
    fn time(&self) -> u32 {
        match *self {
            NoteMsg::On { time, .. } | NoteMsg::Off { time, .. } | NoteMsg::AllOff { time } => time,
        }
    }
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
            },
            &entry,
            &c_id,
            &host_info()?,
        )
        .map_err(|e| ClapError::Load(format!("{id}: {e}")))?;
        Ok(ClapPlugin {
            instance,
            id: id.to_owned(),
        })
    }

    /// 音声処理を始められる状態にし、オーディオスレッドへ渡す処理窓口を返す。
    pub fn activate(&mut self, sample_rate: f64) -> Result<ClapProcessor, ClapError> {
        // 音声ポート(入出力とも、宣言された全ポートにバッファを用意する必要がある)
        let (inputs, outputs, main_out) = self.audio_port_layout();
        let dialect = self.note_dialect();
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
        Ok(ClapProcessor {
            processor: Some(processor.into()),
            in_ports: AudioPorts::with_capacity(total(&inputs), inputs.len()),
            out_ports: AudioPorts::with_capacity(total(&outputs), outputs.len()),
            in_bufs: alloc(&inputs),
            out_bufs: alloc(&outputs),
            main_out,
            dialect,
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
    fn audio_port_layout(&mut self) -> (Vec<u32>, Vec<u32>, usize) {
        let ext = self
            .instance
            .access_shared_handler(|h| h.audio_ports.get().copied().flatten());
        let Some(ext) = ext else {
            // 拡張が無いプラグインはステレオ出力 1 本とみなす
            return (vec![], vec![2], 0);
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
        (
            ins.iter().map(|(c, _)| *c).collect(),
            outs.iter().map(|(c, _)| *c).collect(),
            main_out,
        )
    }

    /// ノートを送る方式(ノート入力が無ければ None)。
    fn note_dialect(&mut self) -> Option<NoteDialect> {
        let ext = self
            .instance
            .access_shared_handler(|h| h.note_ports.get().copied().flatten())?;
        let handle = self.instance.plugin_handle();
        if ext.count(&handle, true) == 0 {
            return None;
        }
        let mut buf = NotePortInfoBuffer::new();
        let info = ext.get(&handle, 0, true, &mut buf)?;
        if info.preferred_dialect == Some(NoteDialect::Clap)
            || info
                .supported_dialects
                .contains(clack_extensions::note_ports::NoteDialects::CLAP)
        {
            Some(NoteDialect::Clap)
        } else {
            Some(NoteDialect::Midi)
        }
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

    /// プラグインから「状態が変わった」と知らされていたら true(読むと戻る)。
    pub fn take_dirty(&mut self) -> bool {
        self.instance.access_handler(|h| h.dirty.replace(false))
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

/// オーディオスレッド側の処理窓口。
pub struct ClapProcessor {
    processor: Option<PluginAudioProcessor<GlauxHost>>,
    in_ports: AudioPorts,
    out_ports: AudioPorts,
    /// [ポート][チャンネル][フレーム](起動時に確保)
    in_bufs: Vec<Vec<Vec<f32>>>,
    out_bufs: Vec<Vec<Vec<f32>>>,
    main_out: usize,
    dialect: Option<NoteDialect>,
    events: EventBuffer,
    steady: u64,
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
        let Some(dialect) = self.dialect else {
            return;
        };
        let t = msg.time().saturating_sub(base);
        match (dialect, msg) {
            (NoteDialect::Clap, NoteMsg::On { key, velocity, .. }) => {
                self.events.push(&NoteOnEvent::new(
                    t,
                    Pckn::new(0u16, 0u16, key as u16, Match::All),
                    velocity as f64,
                ))
            }
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
            .with_input_buffers(self.in_bufs.iter_mut().map(|port| {
                AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_input_only(
                        port.iter_mut()
                            .map(|ch| InputChannel::constant(&mut ch[..n])),
                    ),
                }
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
