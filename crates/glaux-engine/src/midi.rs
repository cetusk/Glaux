//! MIDI キーボード入力。
//!
//! - [`LiveQueue`]: MIDI 受信スレッド → オーディオスレッドのロックフリーなイベント列。
//!   レンダラは毎ブロック頭で取り出して発音する(ライブ演奏。停止中でも鳴る)
//! - [`MidiConnection`]: midir で入力ポートに接続する。受信コールバックは
//!   ライブ発音キューへ積み、MIDI 録音中なら [`MidiTake`] にも記録する
//! - [`pair_notes`] / [`take_to_notes`]: 録音したイベント列をノートに組み立てる
//!
//! どのチャンネルの音も同じトラックで鳴らす(オムニ)。ピッチベンド・プレッシャー・音色(CC74)はチャンネルごとに持ち、
//! そのチャンネルで鳴っている音にだけ効かせる(MPE: 1 音ごとに別のチャンネルを使うコントローラー)。
//! 1 チャンネル目(MPE のマネージャー)の分は全部の音に効く。ベンド幅は 1 チャンネル目が 2 半音、ほかは 48 半音
//! (MPE の既定。RPN 0 で変えられる)。録音ではノートのピッチカーブ・明るさ・音量の曲線になる。
//! 送り先トラックは [`crate::EngineHandle`] 側で決め、トラック index を `Shared::live_track` に書いておく。

use glaux_core::{Articulation, Note, NoteId, TempoMap, Tick};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 送り先トラックなし(内蔵の既定音色でエフェクトなしに鳴らす)
pub const LIVE_NO_TRACK: u32 = 0x1FFF;
/// キュー容量(2 のべき)。1 ブロック(~20ms)でこれを超える入力は捨てる
const QUEUE_CAP: usize = 256;

/// ライブ演奏のイベント。`ch` は MIDI チャンネル(0〜15)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveEvent {
    NoteOn {
        track: u32,
        pitch: u8,
        vel: u8,
        ch: u8,
    },
    /// 送り先トラックを問わず、同じ音高・チャンネルの発音中ボイスを離す
    /// (押している間に送り先が変わっても音が残らないように)
    NoteOff { pitch: u8, ch: u8 },
    /// サステインペダル(CC64)
    Sustain(bool),
    /// 全ボイスを離す(CC120/123、送り先の切り替え時)
    AllOff,
    /// ピッチベンド(0..=16383、中央 8192)と、そのチャンネルのベンド幅(半音)
    PitchBend { ch: u8, v: u16, range: u8 },
    /// チャンネルプレッシャー(0..=127)
    Pressure { ch: u8, v: u8 },
    /// 音色(CC74。0..=127、中央 64)
    Timbre { ch: u8, v: u8 },
}

impl LiveEvent {
    /// bits: [63:61]=種類 [59:56]=チャンネル [44:32]=トラック [31:16]=pitch/値 [15:0]=vel/値
    fn pack(self) -> u64 {
        let k = |kind: u64, ch: u8, track: u32, a: u64, b: u64| {
            (kind << 61)
                | (((ch & 0xF) as u64) << 56)
                | (((track & 0x1FFF) as u64) << 32)
                | (a << 16)
                | b
        };
        match self {
            LiveEvent::NoteOff { pitch, ch } => k(0, ch, 0, pitch as u64, 0),
            LiveEvent::NoteOn {
                track,
                pitch,
                vel,
                ch,
            } => k(1, ch, track, pitch as u64, vel as u64),
            LiveEvent::Sustain(on) => k(2, 0, 0, on as u64, 0),
            LiveEvent::AllOff => k(3, 0, 0, 0, 0),
            LiveEvent::PitchBend { ch, v, range } => k(4, ch, 0, range as u64, (v & 0x3FFF) as u64),
            LiveEvent::Pressure { ch, v } => k(5, ch, 0, 0, v as u64),
            LiveEvent::Timbre { ch, v } => k(6, ch, 0, 0, v as u64),
        }
    }

    fn unpack(v: u64) -> Self {
        let ch = ((v >> 56) & 0xF) as u8;
        let a = ((v >> 16) & 0xFFFF) as u16;
        let b = (v & 0xFFFF) as u16;
        match v >> 61 {
            0 => LiveEvent::NoteOff { pitch: a as u8, ch },
            1 => LiveEvent::NoteOn {
                track: ((v >> 32) & 0x1FFF) as u32,
                pitch: a as u8,
                vel: b as u8,
                ch,
            },
            2 => LiveEvent::Sustain(a != 0),
            4 => LiveEvent::PitchBend {
                ch,
                v: b & 0x3FFF,
                range: a as u8,
            },
            5 => LiveEvent::Pressure { ch, v: b as u8 },
            6 => LiveEvent::Timbre { ch, v: b as u8 },
            _ => LiveEvent::AllOff,
        }
    }
}

/// 固定容量のリングバッファ。取り出し(オーディオスレッド)はロックフリー・
/// アロケーションなし。積む側は複数スレッドから呼べるよう短いロックで直列化する
/// (積む側は MIDI 受信スレッドや UI スレッドで、RT 制約はない)。
pub struct LiveQueue {
    buf: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    push_lock: Mutex<()>,
}

impl Default for LiveQueue {
    fn default() -> Self {
        LiveQueue {
            buf: (0..QUEUE_CAP).map(|_| AtomicU64::new(0)).collect(),
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

/// 時刻を指定して鳴らすノート(ゲームの効果音など)。時刻はレンダラの時計
/// ([`Renderer::clock`](crate::render::Renderer::clock)、再生・停止に関係なく進むサンプル数)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedNote {
    /// 鳴らし始める時計の位置(サンプル)。過ぎていれば次のブロックの頭ですぐ鳴らす
    pub at: u64,
    /// 鳴らすトラックの index(そのトラックの音源とエフェクトで鳴る)
    pub track: u16,
    pub pitch: u8,
    pub vel: u8,
    /// 鍵盤を押している長さ(サンプル)。その後はリリースで消える
    pub dur: u32,
}

const NOTE_QUEUE_CAP: usize = 512;

/// [`TimedNote`] の固定容量のリングバッファ。取り出し(オーディオスレッド)はロックフリー・
/// アロケーションなし。積む側は短いロックで直列化する([`LiveQueue`] と同じ作り。1 つのノートは
/// 2 つの 64 bit の枠に入れる)。
pub struct NoteQueue {
    buf: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    push_lock: Mutex<()>,
}

impl Default for NoteQueue {
    fn default() -> Self {
        NoteQueue {
            buf: (0..NOTE_QUEUE_CAP * 2).map(|_| AtomicU64::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            push_lock: Mutex::new(()),
        }
    }
}

impl NoteQueue {
    /// 積む。満杯なら false(そのノートは捨てる)。
    pub fn push(&self, n: TimedNote) -> bool {
        let _guard = self.push_lock.lock().unwrap_or_else(|e| e.into_inner());
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h.wrapping_sub(t) >= NOTE_QUEUE_CAP {
            return false;
        }
        let i = (h & (NOTE_QUEUE_CAP - 1)) * 2;
        let packed =
            (n.track as u64) << 48 | (n.pitch as u64) << 40 | (n.vel as u64) << 32 | n.dur as u64;
        self.buf[i].store(n.at, Ordering::Relaxed);
        self.buf[i + 1].store(packed, Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
        true
    }

    /// 取り出す(オーディオスレッド専用。単一の消費者から呼ぶこと)。
    pub fn pop(&self) -> Option<TimedNote> {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t == h {
            return None;
        }
        let i = (t & (NOTE_QUEUE_CAP - 1)) * 2;
        let at = self.buf[i].load(Ordering::Relaxed);
        let p = self.buf[i + 1].load(Ordering::Relaxed);
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        Some(TimedNote {
            at,
            track: (p >> 48) as u16,
            pitch: (p >> 40) as u8,
            vel: (p >> 32) as u8,
            dur: p as u32,
        })
    }
}

/// 受信した MIDI メッセージ。`ch` は MIDI チャンネル(0〜15)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMsg {
    On {
        pitch: u8,
        vel: u8,
        ch: u8,
    },
    Off {
        pitch: u8,
        ch: u8,
    },
    Sustain(bool),
    AllOff,
    /// ピッチベンド(0..=16383、中央 8192)
    PitchBend {
        ch: u8,
        v: u16,
    },
    /// ピッチベンドをセントにしたもの(録音用。受け側がそのチャンネルのベンド幅で換算して記録する)
    Bend {
        ch: u8,
        cents: i16,
    },
    /// チャンネルプレッシャー(0..=127)
    Pressure {
        ch: u8,
        v: u8,
    },
    /// 音色(CC74)
    Timbre {
        ch: u8,
        v: u8,
    },
    /// RPN の選択とデータ(CC 101・100・6。ベンド幅の設定に使う)
    Rpn {
        ch: u8,
        cc: u8,
        v: u8,
    },
}

/// MIDI バイト列を解釈する。対象外のメッセージは None。
pub fn parse_midi(bytes: &[u8]) -> Option<MidiMsg> {
    let (&status, rest) = bytes.split_first()?;
    let d1 = *rest.first()? & 0x7F;
    let d2 = rest.get(1).map(|v| v & 0x7F).unwrap_or(0);
    let ch = status & 0x0F;
    match status & 0xF0 {
        0x90 if d2 > 0 => Some(MidiMsg::On {
            pitch: d1,
            vel: d2,
            ch,
        }),
        0x90 | 0x80 => Some(MidiMsg::Off { pitch: d1, ch }),
        0xE0 => Some(MidiMsg::PitchBend {
            ch,
            v: ((d2 as u16) << 7) | d1 as u16,
        }),
        0xD0 => Some(MidiMsg::Pressure { ch, v: d1 }),
        0xB0 => match d1 {
            64 => Some(MidiMsg::Sustain(d2 >= 64)),
            74 => Some(MidiMsg::Timbre { ch, v: d2 }),
            100 | 101 | 6 => Some(MidiMsg::Rpn { ch, cc: d1, v: d2 }),
            120 | 123 => Some(MidiMsg::AllOff),
            _ => None,
        },
        _ => None,
    }
}

/// チャンネルごとのベンド幅(半音)。1 チャンネル目は 2、ほかは MPE の既定の 48。RPN 0 で変わる
#[derive(Clone, Copy, Debug)]
pub struct BendRanges {
    range: [u8; 16],
    /// 選んでいる RPN(MSB, LSB)
    rpn: [(u8, u8); 16],
}

impl Default for BendRanges {
    fn default() -> Self {
        let mut range = [48u8; 16];
        range[0] = 2;
        BendRanges {
            range,
            rpn: [(127, 127); 16],
        }
    }
}

impl BendRanges {
    pub fn range(&self, ch: u8) -> u8 {
        self.range[(ch & 0xF) as usize]
    }

    /// RPN のメッセージを受ける(RPN 0 のデータでベンド幅を変える)
    pub fn rpn(&mut self, ch: u8, cc: u8, v: u8) {
        let c = (ch & 0xF) as usize;
        match cc {
            101 => self.rpn[c].0 = v,
            100 => self.rpn[c].1 = v,
            6 if self.rpn[c] == (0, 0) => self.range[c] = v.clamp(1, 96),
            _ => {}
        }
    }

    /// ベンドの値(0..=16383)をセントに
    pub fn cents(&self, ch: u8, v: u16) -> i16 {
        let x = (v as f32 - 8192.0) / 8192.0;
        (x * self.range(ch) as f32 * 100.0).round() as i16
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

/// 組み立てたノート(絶対 tick の小数)。表現の曲線は (絶対 tick, 値)
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RecordedNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub vel: u8,
    /// 弾いたチャンネル
    pub ch: u8,
    /// ピッチベンド(セント。マネージャーのチャンネルとそのチャンネルの和)
    pub bend: Vec<(f64, f32)>,
    /// 音色(CC74 を −1〜1 に)
    pub bright: Vec<(f64, f32)>,
    /// プレッシャー(0〜1)
    pub press: Vec<(f64, f32)>,
}

/// note on/off を対にしてノートにする。サステインペダルを踏んでいる間の note off は
/// ペダルを離した時点まで延ばす。`stop` までに離されなかった音はそこで切る。
/// 同じ音高を離さずに打ち直した場合は前の音をそこで切る。
/// ベンド・音色・プレッシャーは、そのチャンネルで鳴っている音(1 チャンネル目の分は全部の音)の曲線にする
pub fn pair_notes(events: &[TakeEvent], stop: f64) -> Vec<RecordedNote> {
    // 音高ごとの鳴っている音。ペダルで保持中かどうか
    let mut open: Vec<Option<RecordedNote>> = vec![None; 128];
    let mut held = [false; 128];
    let mut pedal = false;
    let mut out = Vec::new();
    // チャンネルごとの今の値
    let mut bend = [0i16; 16];
    let mut bright: [Option<f32>; 16] = [None; 16];
    let mut press: [Option<f32>; 16] = [None; 16];
    let close =
        |open: &mut Vec<Option<RecordedNote>>, out: &mut Vec<RecordedNote>, p: usize, at: f64| {
            if let Some(mut n) = open[p].take() {
                n.end = at.max(n.start);
                out.push(n);
            }
        };
    // そのチャンネルの音に効く今の値
    let bend_of = |bend: &[i16; 16], ch: u8| -> f32 {
        bend[0] as f32
            + if ch != 0 {
                bend[ch as usize] as f32
            } else {
                0.0
            }
    };
    let pick = |v: &[Option<f32>; 16], ch: u8| v[ch as usize].or(v[0]);
    let affects = |note_ch: u8, ch: u8| ch == 0 || note_ch == ch;
    for e in events {
        match e.msg {
            MidiMsg::On { pitch, vel, ch } => {
                let p = pitch as usize & 0x7F;
                close(&mut open, &mut out, p, e.tick);
                let ch = ch & 0xF;
                let mut n = RecordedNote {
                    start: e.tick,
                    end: e.tick,
                    pitch: p as u8,
                    vel: vel.max(1),
                    ch,
                    ..Default::default()
                };
                let b = bend_of(&bend, ch);
                if b != 0.0 {
                    n.bend.push((e.tick, b));
                }
                if let Some(v) = pick(&bright, ch) {
                    n.bright.push((e.tick, v));
                }
                if let Some(v) = pick(&press, ch) {
                    n.press.push((e.tick, v));
                }
                open[p] = Some(n);
                held[p] = false;
            }
            MidiMsg::Off { pitch, .. } => {
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
            MidiMsg::Bend { ch, cents } => {
                let ch = ch & 0xF;
                bend[ch as usize] = cents;
                for n in open.iter_mut().flatten() {
                    if affects(n.ch, ch) {
                        n.bend.push((e.tick, bend_of(&bend, n.ch)));
                    }
                }
            }
            MidiMsg::Timbre { ch, v } => {
                let ch = ch & 0xF;
                bright[ch as usize] = Some(((v as f32 - 64.0) / 63.0).clamp(-1.0, 1.0));
                for n in open.iter_mut().flatten() {
                    if affects(n.ch, ch) {
                        if let Some(v) = pick(&bright, n.ch) {
                            n.bright.push((e.tick, v));
                        }
                    }
                }
            }
            MidiMsg::Pressure { ch, v } => {
                let ch = ch & 0xF;
                press[ch as usize] = Some(v as f32 / 127.0);
                for n in open.iter_mut().flatten() {
                    if affects(n.ch, ch) {
                        if let Some(v) = pick(&press, n.ch) {
                            n.press.push((e.tick, v));
                        }
                    }
                }
            }
            // 生のベンド値・RPN は受け側で換算済み(Bend)なので使わない
            MidiMsg::PitchBend { .. } | MidiMsg::Rpn { .. } => {}
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

/// 折れ線を `max` 点以下に減らす(形を一番保つ点から残す。両端は必ず残す)
fn reduce_points(pts: &[(f64, f32)], max: usize) -> Vec<(f64, f32)> {
    if pts.len() <= max {
        return pts.to_vec();
    }
    // 両端から始め、今の折れ線から一番離れた点を足していく
    let mut keep = vec![0usize, pts.len() - 1];
    while keep.len() < max {
        keep.sort_unstable();
        let mut best: Option<(f32, usize)> = None;
        for w in keep.windows(2) {
            let (a, b) = (pts[w[0]], pts[w[1]]);
            for (i, p) in pts.iter().enumerate().take(w[1]).skip(w[0] + 1) {
                let t = if b.0 > a.0 {
                    ((p.0 - a.0) / (b.0 - a.0)) as f32
                } else {
                    0.0
                };
                let d = (p.1 - (a.1 + (b.1 - a.1) * t)).abs();
                if best.is_none_or(|(bd, _)| d > bd) {
                    best = Some((d, i));
                }
            }
        }
        match best {
            Some((_, i)) => keep.push(i),
            None => break,
        }
    }
    keep.sort_unstable();
    keep.into_iter().map(|i| pts[i]).collect()
}

/// 録音した表現の曲線を、ノートの頭からの tick の点にする(同じ値の続きはまとめ、点数を上限に収める)。
/// 全部が `flat` なら空
fn note_curve(pts: &[(f64, f32)], start: f64, end: f64, max: usize, flat: f32) -> Vec<(u64, f32)> {
    if pts.iter().all(|p| (p.1 - flat).abs() < 1e-3) {
        return vec![];
    }
    let mut v: Vec<(f64, f32)> = Vec::new();
    for &(t, x) in pts {
        let t = (t.clamp(start, end) - start).max(0.0);
        match v.last_mut() {
            // 同じ tick なら後の値で上書き
            Some(last) if (last.0 - t).abs() < 0.5 => last.1 = x,
            _ => v.push((t, x)),
        }
    }
    reduce_points(&v, max)
        .into_iter()
        .map(|(t, x)| (t.round() as u64, x))
        .collect()
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
            let pitch_curve =
                note_curve(&n.bend, n.start, n.end, glaux_core::MAX_PITCH_POINTS, 0.0)
                    .into_iter()
                    .map(|(t, c)| {
                        glaux_core::PitchPoint::new(
                            Tick(t),
                            c.clamp(-glaux_core::MAX_PITCH_CENTS, glaux_core::MAX_PITCH_CENTS),
                        )
                    })
                    .collect();
            let expr = |pts: &[(f64, f32)], flat: f32, map: &dyn Fn(f32) -> f32| {
                note_curve(pts, n.start, n.end, glaux_core::MAX_EXPR_POINTS, flat)
                    .into_iter()
                    .map(|(t, v)| {
                        glaux_core::CurvePoint::new(
                            Tick(t),
                            map(v),
                            glaux_core::CurveShape::default(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            // 音色(CC74)は明るさ、プレッシャーは押し込むほど大きく(0〜+6dB)
            let brightness_curve = expr(&n.bright, 0.0, &|v| v.clamp(-1.0, 1.0));
            let volume_curve = expr(&n.press, 0.0, &|v| (v * 6.0).clamp(0.0, 6.0));
            (end > pos).then(|| Note {
                id: NoteId::new(),
                pos: Tick(pos),
                dur: Tick(end - pos),
                pitch: n.pitch,
                vel: n.vel,
                articulation: Articulation::Normal,
                pitch_curve,
                glide_ms: None,
                vibrato: None,
                volume_curve,
                brightness_curve,
                condition: None,
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
    /// チャンネルごとのベンド幅(RPN 0 で変わる)
    pub bends: Mutex<BendRanges>,
}

impl MidiSink {
    fn handle(&self, bytes: &[u8]) {
        let Some(msg) = parse_midi(bytes) else {
            return;
        };
        let sh = &self.shared;
        self.last_seen
            .store(sh.epoch.elapsed().as_millis() as u64 + 1, Ordering::Release);
        let mut bends = self.bends.lock().unwrap_or_else(|e| e.into_inner());
        // 録音に残すもの(ベンドはセントに換算して残す)
        let mut record = Some(msg);
        let ev = match msg {
            MidiMsg::On { pitch, vel, ch } => Some(LiveEvent::NoteOn {
                track: sh.live_track.load(Ordering::Acquire),
                pitch,
                vel,
                ch,
            }),
            MidiMsg::Off { pitch, ch } => Some(LiveEvent::NoteOff { pitch, ch }),
            MidiMsg::Sustain(on) => Some(LiveEvent::Sustain(on)),
            MidiMsg::AllOff => Some(LiveEvent::AllOff),
            MidiMsg::PitchBend { ch, v } => {
                record = Some(MidiMsg::Bend {
                    ch,
                    cents: bends.cents(ch, v),
                });
                Some(LiveEvent::PitchBend {
                    ch,
                    v,
                    range: bends.range(ch),
                })
            }
            MidiMsg::Pressure { ch, v } => Some(LiveEvent::Pressure { ch, v }),
            MidiMsg::Timbre { ch, v } => Some(LiveEvent::Timbre { ch, v }),
            MidiMsg::Rpn { ch, cc, v } => {
                bends.rpn(ch, cc, v);
                record = None;
                None
            }
            MidiMsg::Bend { .. } => None,
        };
        drop(bends);
        if let Some(ev) = ev {
            sh.live.push(ev);
        }
        let Some(msg) = record else {
            return;
        };

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
                ch: 0,
            },
            LiveEvent::NoteOn {
                track: LIVE_NO_TRACK,
                pitch: 127,
                vel: 1,
                ch: 15,
            },
            LiveEvent::NoteOff { pitch: 0, ch: 0 },
            LiveEvent::NoteOff { pitch: 64, ch: 0 },
            LiveEvent::Sustain(true),
            LiveEvent::Sustain(false),
            LiveEvent::AllOff,
            LiveEvent::PitchBend {
                ch: 0,
                v: 0,
                range: 2,
            },
            LiveEvent::PitchBend {
                ch: 3,
                v: 8192,
                range: 48,
            },
            LiveEvent::PitchBend {
                ch: 15,
                v: 16383,
                range: 96,
            },
            LiveEvent::Pressure { ch: 2, v: 127 },
            LiveEvent::Timbre { ch: 9, v: 64 },
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
                pitch: (i % 128) as u8,
                ch: (i % 16) as u8,
            }));
        }
        assert!(!q.push(LiveEvent::AllOff), "満杯なら積めない");
        for i in 0..QUEUE_CAP {
            assert_eq!(
                q.pop(),
                Some(LiveEvent::NoteOff {
                    pitch: (i % 128) as u8,
                    ch: (i % 16) as u8,
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
                vel: 100,
                ch: 1
            })
        );
        // ベロシティ 0 の note on は note off
        assert_eq!(
            parse_midi(&[0x90, 60, 0]),
            Some(MidiMsg::Off { pitch: 60, ch: 0 })
        );
        assert_eq!(
            parse_midi(&[0x85, 61, 40]),
            Some(MidiMsg::Off { pitch: 61, ch: 5 })
        );
        assert_eq!(parse_midi(&[0xB0, 64, 127]), Some(MidiMsg::Sustain(true)));
        assert_eq!(parse_midi(&[0xB0, 64, 0]), Some(MidiMsg::Sustain(false)));
        assert_eq!(parse_midi(&[0xB0, 123, 0]), Some(MidiMsg::AllOff));
        assert_eq!(parse_midi(&[0xB0, 1, 30]), None); // モジュレーションは未対応
        assert_eq!(
            parse_midi(&[0xE3, 0, 64]),
            Some(MidiMsg::PitchBend { ch: 3, v: 8192 })
        );
        assert_eq!(
            parse_midi(&[0xE0, 0x7F, 0x7F]),
            Some(MidiMsg::PitchBend { ch: 0, v: 16383 })
        );
        // MPE の表現: プレッシャー・CC74・RPN
        assert_eq!(
            parse_midi(&[0xD2, 90]),
            Some(MidiMsg::Pressure { ch: 2, v: 90 })
        );
        assert_eq!(
            parse_midi(&[0xB4, 74, 100]),
            Some(MidiMsg::Timbre { ch: 4, v: 100 })
        );
        assert_eq!(
            parse_midi(&[0xB1, 101, 0]),
            Some(MidiMsg::Rpn {
                ch: 1,
                cc: 101,
                v: 0
            })
        );
        assert_eq!(parse_midi(&[0xF8]), None); // クロック
        assert_eq!(parse_midi(&[]), None);
    }

    #[test]
    fn bend_ranges_default_to_mpe_and_follow_rpn_0() {
        let mut b = BendRanges::default();
        assert_eq!((b.range(0), b.range(1), b.range(15)), (2, 48, 48));
        assert_eq!(b.cents(0, 16383), 200);
        assert_eq!(b.cents(1, 0), -4800);
        // RPN 0(ベンド幅)を 12 半音に
        b.rpn(1, 101, 0);
        b.rpn(1, 100, 0);
        b.rpn(1, 6, 12);
        assert_eq!(b.range(1), 12);
        // ほかの RPN のデータでは変わらない
        b.rpn(2, 101, 0);
        b.rpn(2, 100, 6);
        b.rpn(2, 6, 15);
        assert_eq!(b.range(2), 48);
    }

    fn ev(tick: f64, msg: MidiMsg) -> TakeEvent {
        TakeEvent { tick, msg }
    }

    #[test]
    fn pairs_notes_with_sustain_and_stop() {
        use MidiMsg::*;
        let on = |pitch: u8, vel: u8| On { pitch, vel, ch: 0 };
        let off = |pitch: u8| Off { pitch, ch: 0 };
        let events = [
            ev(0.0, on(60, 90)),
            ev(10.0, on(64, 80)),
            ev(100.0, off(60)),
            ev(150.0, Sustain(true)),
            ev(200.0, off(64)),    // ペダル中 → 300 まで延びる
            ev(250.0, on(67, 70)), // 離されない → stop で切る
            ev(300.0, Sustain(false)),
            ev(320.0, on(72, 60)),
            ev(330.0, on(72, 61)), // 打ち直し → 前は 330 で切る
            ev(340.0, off(72)),
        ];
        let notes = pair_notes(&events, 400.0);
        let find = |p: u8, s: f64| {
            notes
                .iter()
                .find(|n| n.pitch == p && n.start == s)
                .cloned()
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
    fn mpe_expression_becomes_note_curves() {
        use MidiMsg::*;
        // 2 チャンネル目の音だけを 1 半音しゃくり上げ、3 チャンネル目の音は動かさない。
        // 1 チャンネル目(マネージャー)のベンドは両方に効く
        let events = [
            ev(
                0.0,
                On {
                    pitch: 60,
                    vel: 90,
                    ch: 1,
                },
            ),
            ev(
                0.0,
                On {
                    pitch: 64,
                    vel: 90,
                    ch: 2,
                },
            ),
            ev(0.0, Bend { ch: 1, cents: -100 }),
            ev(480.0, Bend { ch: 1, cents: 0 }),
            ev(480.0, Timbre { ch: 1, v: 127 }),
            ev(600.0, Pressure { ch: 2, v: 127 }),
            ev(700.0, Bend { ch: 0, cents: 50 }),
            ev(960.0, Off { pitch: 60, ch: 1 }),
            ev(960.0, Off { pitch: 64, ch: 2 }),
        ];
        let notes = pair_notes(&events, 2000.0);
        let a = notes.iter().find(|n| n.pitch == 60).unwrap();
        let b = notes.iter().find(|n| n.pitch == 64).unwrap();
        assert_eq!(a.bend, vec![(0.0, -100.0), (480.0, 0.0), (700.0, 50.0)]);
        assert_eq!(a.bright, vec![(480.0, 1.0)]);
        assert!(a.press.is_empty());
        assert_eq!(b.bend, vec![(700.0, 50.0)]);
        assert_eq!(b.press, vec![(600.0, 1.0)]);
        // ノートの曲線にする(頭からの tick)
        let n = take_to_notes(&notes, Tick(0), 0);
        let na = n.iter().find(|x| x.pitch == 60).unwrap();
        let pc: Vec<(u64, f32)> = na.pitch_curve.iter().map(|p| (p.tick.0, p.cents)).collect();
        assert_eq!(pc, vec![(0, -100.0), (480, 0.0), (700, 50.0)]);
        assert_eq!(na.brightness_curve[0].value, 1.0);
        let nb = n.iter().find(|x| x.pitch == 64).unwrap();
        assert_eq!(nb.volume_curve[0].value, 6.0);
        assert!(glaux_core::check_pitch_curve(&na.pitch_curve).is_ok());
    }

    #[test]
    fn long_bends_are_reduced_to_the_point_limit() {
        // なめらかなベンド(200 点)を 16 点に減らしても、形(山の頂点)は残る
        let pts: Vec<(f64, f32)> = (0..200)
            .map(|i| {
                (
                    i as f64 * 5.0,
                    ((i as f32 / 199.0) * std::f32::consts::PI).sin() * 200.0,
                )
            })
            .collect();
        let r = reduce_points(&pts, 16);
        assert_eq!(r.len(), 16);
        assert_eq!(r[0], pts[0]);
        assert_eq!(*r.last().unwrap(), pts[199]);
        assert!(r.iter().any(|p| p.1 > 199.0));
    }

    #[test]
    fn take_to_notes_trims_count_in_and_quantizes() {
        let rec = |start: f64, end: f64, pitch: u8| RecordedNote {
            start,
            end,
            pitch,
            vel: 100,
            ..Default::default()
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
