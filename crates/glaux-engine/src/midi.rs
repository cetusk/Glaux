//! MIDI キーボード入力。
//!
//! - [`LiveQueue`]: MIDI 受信スレッド → オーディオスレッドのロックフリーなイベント列。
//!   レンダラは毎ブロック頭で取り出して発音する(ライブ演奏。停止中でも鳴る)
//! - [`MidiConnection`]: midir で入力ポートに接続する。受信コールバックは
//!   ライブ発音キューへ積み、MIDI 録音中なら [`MidiTake`] にも記録する
//! - [`pair_notes`] / [`take_to_notes`]: 録音したイベント列をノートに組み立てる
//!
//! MIDI チャンネルは区別しない(オムニ)。送り先トラックは [`crate::EngineHandle`] 側で
//! 決め、トラック index を `Shared::live_track` に書いておく。

use glaux_core::{Articulation, Note, NoteId, TempoMap, Tick};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 送り先トラックなし(内蔵の既定音色でエフェクトなしに鳴らす)
pub const LIVE_NO_TRACK: u32 = 0x1FFF;
/// キュー容量(2 のべき)。1 ブロック(~20ms)でこれを超える入力は捨てる
const QUEUE_CAP: usize = 256;

/// ライブ演奏のイベント。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveEvent {
    NoteOn {
        track: u32,
        pitch: u8,
        vel: u8,
    },
    /// 送り先トラックを問わず、同じ音高の発音中ボイスを離す
    /// (押している間に送り先が変わっても音が残らないように)
    NoteOff {
        pitch: u8,
    },
    /// サステインペダル(CC64)
    Sustain(bool),
    /// 全ボイスを離す(CC120/123、送り先の切り替え時)
    AllOff,
    /// ピッチベンド(0..=16383、中央 8192)。CLAP プラグインのトラックにだけ効く
    PitchBend(u16),
}

impl LiveEvent {
    /// bits: [31:29]=種類 [28:16]=トラック [15:8]=pitch/値の上位 [7:0]=vel/値の下位
    fn pack(self) -> u32 {
        match self {
            LiveEvent::NoteOn { track, pitch, vel } => {
                (1 << 29) | ((track & 0x1FFF) << 16) | ((pitch as u32) << 8) | vel as u32
            }
            LiveEvent::NoteOff { pitch } => (pitch as u32) << 8,
            LiveEvent::Sustain(on) => (2 << 29) | ((on as u32) << 8),
            LiveEvent::AllOff => 3 << 29,
            LiveEvent::PitchBend(v) => (4 << 29) | (v as u32 & 0x3FFF),
        }
    }

    fn unpack(v: u32) -> Self {
        let pitch = ((v >> 8) & 0xFF) as u8;
        match v >> 29 {
            0 => LiveEvent::NoteOff { pitch },
            1 => LiveEvent::NoteOn {
                track: (v >> 16) & 0x1FFF,
                pitch,
                vel: (v & 0xFF) as u8,
            },
            2 => LiveEvent::Sustain(pitch != 0),
            4 => LiveEvent::PitchBend((v & 0x3FFF) as u16),
            _ => LiveEvent::AllOff,
        }
    }
}

/// 固定容量のリングバッファ。取り出し(オーディオスレッド)はロックフリー・
/// アロケーションなし。積む側は複数スレッドから呼べるよう短いロックで直列化する
/// (積む側は MIDI 受信スレッドや UI スレッドで、RT 制約はない)。
pub struct LiveQueue {
    buf: Box<[AtomicU32]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    push_lock: Mutex<()>,
}

impl Default for LiveQueue {
    fn default() -> Self {
        LiveQueue {
            buf: (0..QUEUE_CAP).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            push_lock: Mutex::new(()),
        }
    }
}

impl LiveQueue {
    /// 積む。満杯なら false(そのイベントは捨てる)。
    pub fn push(&self, ev: LiveEvent) -> bool {
        let _guard = self.push_lock.lock().unwrap_or_else(|e| e.into_inner());
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= QUEUE_CAP {
            return false;
        }
        self.buf[h & (QUEUE_CAP - 1)].store(ev.pack(), Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    /// 取り出す(オーディオスレッド専用。単一の消費者から呼ぶこと)。
    pub fn pop(&self) -> Option<LiveEvent> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h {
            return None;
        }
        let v = self.buf[t & (QUEUE_CAP - 1)].load(Ordering::Relaxed);
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(LiveEvent::unpack(v))
    }
}

/// 受信した MIDI メッセージ(チャンネルは無視)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMsg {
    On {
        pitch: u8,
        vel: u8,
    },
    Off {
        pitch: u8,
    },
    Sustain(bool),
    AllOff,
    /// ピッチベンド(0..=16383、中央 8192)
    PitchBend(u16),
}

/// MIDI バイト列を解釈する。対象外のメッセージは None。
pub fn parse_midi(bytes: &[u8]) -> Option<MidiMsg> {
    let (&status, rest) = bytes.split_first()?;
    let d1 = *rest.first()? & 0x7F;
    let d2 = rest.get(1).map(|v| v & 0x7F).unwrap_or(0);
    match status & 0xF0 {
        0x90 if d2 > 0 => Some(MidiMsg::On { pitch: d1, vel: d2 }),
        0x90 | 0x80 => Some(MidiMsg::Off { pitch: d1 }),
        0xE0 => Some(MidiMsg::PitchBend(((d2 as u16) << 7) | d1 as u16)),
        0xB0 => match d1 {
            64 => Some(MidiMsg::Sustain(d2 >= 64)),
            120 | 123 => Some(MidiMsg::AllOff),
            _ => None,
        },
        _ => None,
    }
}

/// 録音したイベント(時刻は tick の小数)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TakeEvent {
    pub tick: f64,
    pub msg: MidiMsg,
}

/// 進行中の MIDI 録音。
#[derive(Debug, Default)]
pub struct MidiTake {
    pub events: Vec<TakeEvent>,
}

/// 組み立てたノート(絶対 tick の小数)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub vel: u8,
}

/// note on/off を対にしてノートにする。サステインペダルを踏んでいる間の note off は
/// ペダルを離した時点まで延ばす。`stop` までに離されなかった音はそこで切る。
/// 同じ音高を離さずに打ち直した場合は前の音をそこで切る。
pub fn pair_notes(events: &[TakeEvent], stop: f64) -> Vec<RecordedNote> {
    // pitch ごとの (開始, vel)。ペダルで保持中かどうか
    let mut open: [Option<(f64, u8)>; 128] = [None; 128];
    let mut held = [false; 128];
    let mut pedal = false;
    let mut out = Vec::new();
    let close =
        |open: &mut [Option<(f64, u8)>; 128], out: &mut Vec<RecordedNote>, p: usize, at: f64| {
            if let Some((start, vel)) = open[p].take() {
                out.push(RecordedNote {
                    start,
                    end: at.max(start),
                    pitch: p as u8,
                    vel,
                });
            }
        };
    for e in events {
        match e.msg {
            MidiMsg::On { pitch, vel } => {
                let p = pitch as usize & 0x7F;
                close(&mut open, &mut out, p, e.tick);
                open[p] = Some((e.tick, vel.max(1)));
                held[p] = false;
            }
            MidiMsg::Off { pitch } => {
                let p = pitch as usize & 0x7F;
                if pedal {
                    held[p] = open[p].is_some();
                } else {
                    close(&mut open, &mut out, p, e.tick);
                }
            }
            MidiMsg::Sustain(on) => {
                if pedal && !on {
                    for (p, h) in held.iter_mut().enumerate() {
                        if *h {
                            *h = false;
                            close(&mut open, &mut out, p, e.tick);
                        }
                    }
                }
                pedal = on;
            }
            // ピッチベンドは録音しない(ノートのピッチカーブへの変換は将来)
            MidiMsg::PitchBend(_) => {}
            MidiMsg::AllOff => {
                held = [false; 128];
                for p in 0..128 {
                    close(&mut open, &mut out, p, e.tick);
                }
            }
        }
    }
    for p in 0..128 {
        close(&mut open, &mut out, p, stop);
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    out
}

/// 録音ノートをクリップ相対のノートにする。`clip_start` より前(カウントイン中の
/// 食い気味の打鍵)は 16 分音符以内なら頭に寄せ、それより前は捨てる。
/// `quantize_ticks` > 0 なら開始位置をグリッドに丸める(長さは最低 1 グリッド)。
pub fn take_to_notes(notes: &[RecordedNote], clip_start: Tick, quantize_ticks: u64) -> Vec<Note> {
    let base = clip_start.0 as f64;
    let grace = (glaux_core::PPQ / 4) as f64;
    let mut out: Vec<Note> = notes
        .iter()
        .filter(|n| n.start >= base - grace)
        .filter_map(|n| {
            let mut pos = (n.start - base).max(0.0).round() as u64;
            let mut end = (n.end - base).max(0.0).round() as u64;
            if quantize_ticks > 0 {
                let q = quantize_ticks as f64;
                let len = end.saturating_sub(pos) as f64;
                pos = ((pos as f64 / q).round() * q) as u64;
                end = pos + ((len / q).round().max(1.0) * q) as u64;
            }
            (end > pos).then(|| Note {
                id: NoteId::new(),
                pos: Tick(pos),
                dur: Tick(end - pos),
                pitch: n.pitch,
                vel: n.vel,
                articulation: Articulation::Normal,
                pitch_curve: vec![],
            })
        })
        .collect();
    out.sort_by_key(|n| (n.pos, n.pitch));
    // 同じ音高の重なり(量子化で起きうる)は前を詰める。同じ位置なら後を残す
    let mut kept: Vec<Note> = Vec::with_capacity(out.len());
    for n in out {
        if let Some(prev) = kept
            .iter_mut()
            .rev()
            .find(|p| p.pitch == n.pitch && p.end() > n.pos)
        {
            if prev.pos == n.pos {
                *prev = n;
                continue;
            }
            prev.dur = Tick(n.pos.0 - prev.pos.0);
        }
        kept.push(n);
    }
    kept
}

/// 利用できる MIDI 入力ポート名の一覧。
pub fn list_midi_inputs() -> Vec<String> {
    let Ok(input) = midir::MidiInput::new("glaux-list") else {
        return vec![];
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

/// MIDI 受信時に呼ばれる処理(接続ごとにコールバックへ渡す)。
pub(crate) struct MidiSink {
    pub shared: Arc<crate::render::Shared>,
    pub tempo: Arc<Mutex<TempoMap>>,
    pub sample_rate: Arc<std::sync::atomic::AtomicU64>,
    pub take: Arc<Mutex<Option<MidiTake>>>,
    /// 最後に受信した時刻(UI の受信ランプ用、epoch からの ms)
    pub last_seen: Arc<std::sync::atomic::AtomicU64>,
}

impl MidiSink {
    fn handle(&self, bytes: &[u8]) {
        let Some(msg) = parse_midi(bytes) else {
            return;
        };
        let sh = &self.shared;
        self.last_seen
            .store(sh.epoch.elapsed().as_millis() as u64 + 1, Ordering::Release);
        let ev = match msg {
            MidiMsg::On { pitch, vel } => LiveEvent::NoteOn {
                track: sh.live_track.load(Ordering::Acquire),
                pitch,
                vel,
            },
            MidiMsg::Off { pitch } => LiveEvent::NoteOff { pitch },
            MidiMsg::Sustain(on) => LiveEvent::Sustain(on),
            MidiMsg::AllOff => LiveEvent::AllOff,
            MidiMsg::PitchBend(v) => LiveEvent::PitchBend(v),
        };
        sh.live.push(ev);

        let mut take = self.take.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = take.as_mut() {
            let sr = f64::from_bits(self.sample_rate.load(Ordering::Acquire));
            let sample = sh.audible_pos();
            let tick = self
                .tempo
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .seconds_to_tick(sample / sr)
                .0 as f64;
            t.events.push(TakeEvent { tick, msg });
        }
    }
}

/// 接続中の MIDI 入力。drop で切断する。
pub struct MidiConnection {
    pub name: String,
    _conn: midir::MidiInputConnection<()>,
}

impl MidiConnection {
    pub(crate) fn open(name: &str, sink: MidiSink) -> Result<Self, String> {
        let mut input = midir::MidiInput::new("glaux").map_err(|e| e.to_string())?;
        input.ignore(midir::Ignore::All);
        let port = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).ok().as_deref() == Some(name))
            .ok_or_else(|| format!("MIDI 入力が見つかりません: {name}"))?;
        let conn = input
            .connect(
                &port,
                "glaux-in",
                move |_stamp, bytes, _| sink.handle(bytes),
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(MidiConnection {
            name: name.to_owned(),
            _conn: conn,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_event_pack_roundtrip() {
        for ev in [
            LiveEvent::NoteOn {
                track: 5,
                pitch: 60,
                vel: 100,
            },
            LiveEvent::NoteOn {
                track: LIVE_NO_TRACK,
                pitch: 127,
                vel: 1,
            },
            LiveEvent::NoteOff { pitch: 0 },
            LiveEvent::NoteOff { pitch: 64 },
            LiveEvent::Sustain(true),
            LiveEvent::Sustain(false),
            LiveEvent::AllOff,
            LiveEvent::PitchBend(0),
            LiveEvent::PitchBend(8192),
            LiveEvent::PitchBend(16383),
        ] {
            assert_eq!(LiveEvent::unpack(ev.pack()), ev);
        }
    }

    #[test]
    fn queue_is_fifo_and_bounded() {
        let q = LiveQueue::default();
        assert_eq!(q.pop(), None);
        for i in 0..QUEUE_CAP {
            assert!(q.push(LiveEvent::NoteOff {
                pitch: (i % 128) as u8
            }));
        }
        assert!(!q.push(LiveEvent::AllOff), "満杯なら積めない");
        for i in 0..QUEUE_CAP {
            assert_eq!(
                q.pop(),
                Some(LiveEvent::NoteOff {
                    pitch: (i % 128) as u8
                })
            );
        }
        assert_eq!(q.pop(), None);
        // 周回しても壊れない
        for _ in 0..3 * QUEUE_CAP {
            assert!(q.push(LiveEvent::AllOff));
            assert_eq!(q.pop(), Some(LiveEvent::AllOff));
        }
    }

    #[test]
    fn parses_note_and_controller_messages() {
        assert_eq!(
            parse_midi(&[0x91, 60, 100]),
            Some(MidiMsg::On {
                pitch: 60,
                vel: 100
            })
        );
        // ベロシティ 0 の note on は note off
        assert_eq!(parse_midi(&[0x90, 60, 0]), Some(MidiMsg::Off { pitch: 60 }));
        assert_eq!(
            parse_midi(&[0x85, 61, 40]),
            Some(MidiMsg::Off { pitch: 61 })
        );
        assert_eq!(parse_midi(&[0xB0, 64, 127]), Some(MidiMsg::Sustain(true)));
        assert_eq!(parse_midi(&[0xB0, 64, 0]), Some(MidiMsg::Sustain(false)));
        assert_eq!(parse_midi(&[0xB0, 123, 0]), Some(MidiMsg::AllOff));
        assert_eq!(parse_midi(&[0xB0, 1, 30]), None); // モジュレーションは未対応
        assert_eq!(parse_midi(&[0xE3, 0, 64]), Some(MidiMsg::PitchBend(8192)));
        assert_eq!(
            parse_midi(&[0xE0, 0x7F, 0x7F]),
            Some(MidiMsg::PitchBend(16383))
        );
        assert_eq!(parse_midi(&[0xF8]), None); // クロック
        assert_eq!(parse_midi(&[]), None);
    }

    fn ev(tick: f64, msg: MidiMsg) -> TakeEvent {
        TakeEvent { tick, msg }
    }

    #[test]
    fn pairs_notes_with_sustain_and_stop() {
        use MidiMsg::*;
        let events = [
            ev(0.0, On { pitch: 60, vel: 90 }),
            ev(10.0, On { pitch: 64, vel: 80 }),
            ev(100.0, Off { pitch: 60 }),
            ev(150.0, Sustain(true)),
            ev(200.0, Off { pitch: 64 }), // ペダル中 → 300 まで延びる
            ev(250.0, On { pitch: 67, vel: 70 }), // 離されない → stop で切る
            ev(300.0, Sustain(false)),
            ev(320.0, On { pitch: 72, vel: 60 }),
            ev(330.0, On { pitch: 72, vel: 61 }), // 打ち直し → 前は 330 で切る
            ev(340.0, Off { pitch: 72 }),
        ];
        let notes = pair_notes(&events, 400.0);
        let find = |p: u8, s: f64| {
            notes
                .iter()
                .find(|n| n.pitch == p && n.start == s)
                .copied()
                .unwrap_or_else(|| panic!("{p}@{s} が無い: {notes:?}"))
        };
        assert_eq!(find(60, 0.0).end, 100.0);
        assert_eq!(find(64, 10.0).end, 300.0);
        assert_eq!(find(67, 250.0).end, 400.0);
        assert_eq!(find(72, 320.0).end, 330.0);
        assert_eq!(find(72, 330.0).end, 340.0);
        assert_eq!(find(72, 330.0).vel, 61);
        assert_eq!(notes.len(), 5);
    }

    #[test]
    fn take_to_notes_trims_count_in_and_quantizes() {
        let rec = |start: f64, end: f64, pitch: u8| RecordedNote {
            start,
            end,
            pitch,
            vel: 100,
        };
        let clip_start = Tick(3840);
        let notes = take_to_notes(
            &[
                rec(1000.0, 1200.0, 50), // カウントイン中 → 捨てる
                rec(3800.0, 4300.0, 60), // 食い気味 → 頭に寄せる
                rec(4810.0, 5270.0, 62),
            ],
            clip_start,
            0,
        );
        assert_eq!(notes.len(), 2);
        assert_eq!(
            (notes[0].pos, notes[0].dur, notes[0].pitch),
            (Tick(0), Tick(460), 60)
        );
        assert_eq!((notes[1].pos, notes[1].dur), (Tick(970), Tick(460)));

        let q = take_to_notes(&[rec(4810.0, 5270.0, 62)], clip_start, 240);
        assert_eq!((q[0].pos, q[0].dur), (Tick(960), Tick(480)));
    }
}
