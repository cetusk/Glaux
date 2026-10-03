//! RT セーフなレンダラ。オーディオスレッド(cpal コールバック)から呼ばれる。
//!
//! 絶対条件(CLAUDE.md): このモジュールの `process` 内では
//! **アロケーション・ロック・ブロッキング I/O をしない**。
//! - 制御は [`Shared`] のアトミックと `ArcSwap` 経由(どちらもロックフリー)
//! - ボイス配列は起動時に確保した固定容量(`MAX_VOICES`)を使い回す
//! - `PlaybackData` の解放はオーディオスレッドでは起きない
//!   (ハンドル側が旧データを graveyard に保持してから捨てる)
//!
//! 音源は glaux-dsp の内蔵楽器(subtractive / drum)。トラックの device 設定から
//! 焼き込まれたパラメータ([`crate::data::TrackMix::instrument`])で発音する。

use crate::data::{
    balance_gains, db_to_amp, pan_gains, AutoPoint, PlaybackData, MAX_EFFECT_SLOTS, MAX_TRACKS,
};
use crate::midi::{LiveEvent, LiveQueue, LIVE_NO_TRACK};
use crate::plugins::{PluginSlot, Processor, MAX_PLUGINS};
use arc_swap::ArcSwap;
use glaux_clap::{NoteMsg, MAX_EVENTS, MAX_FRAMES};
use glaux_core::Curve;
use glaux_dsp::{EffectParams, EffectState, VoiceState};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// マスターの最後のクリップ防止。振幅 0.9(-0.9dBFS)までは素通しし、それを超えた分だけ
/// 1.0 に向けてなめらかに抑える。以前は全体に tanh を掛けていて、普通の音量でも潰れと歪みが
/// 付いていた(-6dBFS で -0.7dB)
#[inline]
pub(crate) fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.9;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh()).copysign(x)
    }
}

/// 再生中の編集・シークで、発音内容が変わったトラックの音を消すフェードと、鳴らし直す音のフェードイン(秒)
const SWAP_FADE_SECS: f32 = 0.005;
/// 差し替え・シークの位置をまたいでいるノートを探すとき、遡るイベント数の上限(オーディオスレッドで
/// 使う時間を抑える)
const MAX_REPLAY_SCAN: usize = 2048;

/// 音色・エフェクトのオートメーションを評価する間隔(フレーム。48kHz で約 2.7ms)
const AUTO_FRAMES: usize = 128;

/// 処理単位の頭で評価するオートメーション(音色・エフェクト)があるか。
/// 音量・パンはサンプルごとに評価するので含めない
fn has_block_automation(data: &PlaybackData) -> bool {
    !data.master_fx_auto.is_empty()
        || data
            .tracks
            .iter()
            .any(|t| !t.device_auto.is_empty() || !t.fx_auto.is_empty())
}

/// 同時発音数(全トラック合計)。超えると古い音を短いフェードで奪う(ボイススティール)
pub const MAX_VOICES: usize = 256;
/// 奪われてフェードアウト中のボイスのための予備枠(この分も起動時に確保しておく)。
/// 層のある音色は 1 音で 4 つ奪うことがあるので、同じ瞬間の 32 音 × 4 層ぶん
const STEAL_RESERVE: usize = 128;
/// 奪うときのフェードの長さ(秒)。短すぎるとクリック、長すぎると予備枠を食う
const STEAL_FADE_SECS: f32 = 0.003;
/// 同時に再生する音声クリップ数
pub const MAX_AUDIO_VOICES: usize = 16;
/// 同時プレビュー(試聴)ボイス数
pub const MAX_PREVIEW_VOICES: usize = 8;
/// MIDI キーボードのライブ発音ボイス数
pub const MAX_LIVE_VOICES: usize = 32;
/// 1 ブロックで取り出すライブイベントの上限(暴走した入力で処理が伸びないように)
const MAX_LIVE_EVENTS_PER_BLOCK: usize = 256;
/// 鳴らし始めを待っている時刻指定のノートの上限(超えた分は捨てる)
const MAX_TIMED_NOTES: usize = 256;
/// ライブ演奏の後、停止中でもエフェクトの残響を鳴らし切る時間(秒)
const LIVE_TAIL_SECS: f32 = 4.0;
/// シーク要求なしを表す番兵値
pub const NO_SEEK: u64 = u64::MAX;
/// リリースが終わらないボイスの強制解放(秒)。スタック防止の保険
const VOICE_HARD_LIMIT_SECS: f32 = 8.0;
/// 最後のノートが終わってから自動停止するまでの余韻(秒)
const TAIL_SECS: f64 = 2.0;

/// ハンドル(UI 側)とレンダラ(オーディオ側)が共有する制御データ。
pub struct Shared {
    pub playing: AtomicBool,
    /// 現在の再生位置(サンプル)。レンダラが書き、UI が読む
    pub pos: AtomicU64,
    /// シーク要求(サンプル)。`NO_SEEK` なら要求なし。UI が書き、レンダラが消費する
    pub seek: AtomicU64,
    /// ノート試聴要求(パック形式)。UI が書き、レンダラがカウンタ変化で検出する。
    /// bits: [63:48]=カウンタ [47:32]=トラック index [31:16]=長さ(ms) [15:8]=pitch [7:0]=vel
    pub preview: AtomicU64,
    /// ループ区間(サンプル)。`loop_end <= loop_start` ならループなし。
    /// 2 つのアトミックに分かれているため一瞬だけ不整合になり得るが、
    /// 影響は 1 ブロックのジャンプ位置に限られる(実害なし)
    pub loop_start: AtomicU64,
    pub loop_end: AtomicU64,
    /// 予約した位置で飛ぶ(ゲームの曲の展開の切り替え)。再生位置が `jump_at` に来たら `jump_to` へ。
    /// `NO_SEEK` なら予約なし。飛ぶときに `jump_loop_end > jump_loop_start` ならループ区間もそれに替える
    /// (`jump_loop_end` が `NO_SEEK` ならループはそのまま、0 ならループを外す)。
    /// 書く側は `jump_to` → ループ → `jump_at` の順に書く(`jump_at` が予約の合図)
    pub jump_at: AtomicU64,
    pub jump_to: AtomicU64,
    pub jump_loop_start: AtomicU64,
    pub jump_loop_end: AtomicU64,
    /// 直近に位置が飛んだ記録(ループの折り返し・予約した飛び先)。[`JumpRecord`] で読む
    pub last_jump: JumpLog,
    /// トラックごとの追加の音量(リニアの f32 のビット。既定 1.0)。フェーダー・オートメーションに掛ける。
    /// プロジェクトを変えずに外から動かす(ゲームの場面で楽器を足し引きする)
    pub live_gain: [AtomicU32; MAX_TRACKS],
    /// アプリから鳴る音の音量(リニアの f32 のビット。既定 1.0)。出力の最後(クリップ防止の後)に掛ける。
    /// 聴く音量だけで、曲(マスター音量)・メーター・書き出しには入らない
    pub output_gain: AtomicU32,
    /// メトロノーム(拍ごとのクリック)を鳴らすか
    pub metronome: AtomicBool,
    /// 録音中(曲末の自動停止を抑止する)
    pub recording: AtomicBool,
    /// メトロノームだけ鳴らす(遅延の較正中。ノート・音声クリップを発音しない)
    pub click_only: AtomicBool,
    /// マスターの最後のクリップ防止(soft_clip)を通さない。トラックを音声にする(フリーズ)ときの
    /// 描き出し用(後でミックスするので、大きい音でも潰さずにそのまま残す)
    pub no_master_clip: AtomicBool,
    pub data: ArcSwap<PlaybackData>,
    /// 音量をそろえた A/B の聴き比べ(書き出した 2 つの音)と、どちらを鳴らすか([`crate::ab::AbSide`] の番号)
    pub ab: arc_swap::ArcSwapOption<crate::ab::AbClip>,
    pub ab_side: std::sync::atomic::AtomicU8,
    /// 負荷の統計(オーディオスレッドが書き、UI が読む)。[`DspStats`] 参照
    pub stats: StatsCounters,
    /// MIDI キーボードのライブ演奏イベント(MIDI 受信スレッドが積み、レンダラが取り出す)
    pub live: LiveQueue,
    /// 時刻指定のノート(ゲームの効果音など。積む側は任意のスレッド、取り出しはレンダラ)
    pub notes: crate::midi::NoteQueue,
    /// ライブ演奏の送り先トラック index(`LIVE_NO_TRACK` なら既定音色)
    pub live_track: AtomicU32,
    /// 時刻の基準(`pos_nanos` や MIDI 受信時刻の起点)
    pub epoch: std::time::Instant,
    /// 直前ブロックのフレーム数と、`pos` を書いた時刻(`epoch` からの ns)。
    /// MIDI 録音で「いま聞こえている位置」をブロック内まで推定するのに使う
    pub block_frames: AtomicU32,
    pub pos_nanos: AtomicU64,
    /// CLAP プラグインの処理窓口の受け渡し口([`crate::plugins`])
    pub plugin_slots: Arc<[PluginSlot; MAX_PLUGINS]>,
    /// トラック・マスターの直近のピーク(ミキサーのメーター用)。[`Levels`] 参照
    pub levels: Levels,
    /// 聴き方の切り替えと、相関・ゴニオメーター([`crate::monitor`])
    pub monitor: crate::monitor::MonitorShared,
}

/// 位置が飛んだ記録: レンダラの時計 `clock` のサンプルで、再生位置が `from` から `to` へ飛んだ。
/// 「いま聞こえている位置」を、出力の遅れの間に飛んだ場合も正しく求めるのに使う
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JumpRecord {
    /// 何回目の飛びか(0 = まだ飛んでいない)
    pub seq: u64,
    pub clock: u64,
    pub from: u64,
    pub to: u64,
}

/// [`JumpRecord`] の受け渡し(書くのはレンダラだけ。読む側は `seq` を前後で確かめる)
#[derive(Default)]
pub struct JumpLog {
    seq: AtomicU64,
    clock: AtomicU64,
    from: AtomicU64,
    to: AtomicU64,
}

impl JumpLog {
    fn write(&self, clock: u64, from: u64, to: u64) {
        // 奇数 = 書いている途中
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq + 1, Ordering::Release);
        self.clock.store(clock, Ordering::Release);
        self.from.store(from, Ordering::Release);
        self.to.store(to, Ordering::Release);
        self.seq.store(seq + 2, Ordering::Release);
    }

    /// 直近の記録(書いている途中なら少し待って読み直す)
    pub fn read(&self) -> JumpRecord {
        loop {
            let a = self.seq.load(Ordering::Acquire);
            if a % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let r = JumpRecord {
                seq: a / 2,
                clock: self.clock.load(Ordering::Acquire),
                from: self.from.load(Ordering::Acquire),
                to: self.to.load(Ordering::Acquire),
            };
            if self.seq.load(Ordering::Acquire) == a {
                return r;
            }
        }
    }
}

/// トラック(フェーダー・パンの後)とマスター(マスターの音量の後、クリップ防止の前)の直近のピーク。
/// オーディオスレッドが振幅の最大を書き(`fetch_max`)、UI が読んでリセットする。
/// 値は振幅(0 以上)の f32 のビット。0 以上の f32 はビットの大小と値の大小が同じなので `fetch_max` で比べられる
pub struct Levels {
    pub tracks: [AtomicU32; MAX_TRACKS],
    pub master: AtomicU32,
}

impl Default for Levels {
    fn default() -> Self {
        Levels {
            tracks: std::array::from_fn(|_| AtomicU32::new(0)),
            master: AtomicU32::new(0),
        }
    }
}

impl Levels {
    fn note(slot: &AtomicU32, peak: f32) {
        if peak > 0.0 {
            slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// 最初の `ntracks` 本とマスターのピーク(dBFS。無音は -120)を読み出してリセットする
    pub fn take(&self, ntracks: usize) -> (Vec<f32>, f32) {
        let db = |slot: &AtomicU32| {
            let v = f32::from_bits(slot.swap(0, Ordering::Relaxed));
            if v > 1e-6 {
                20.0 * v.log10()
            } else {
                -120.0
            }
        };
        let tracks = self
            .tracks
            .iter()
            .take(ntracks.min(MAX_TRACKS))
            .map(db)
            .collect();
        (tracks, db(&self.master))
    }
}

/// オーディオ処理の負荷統計(アトミック。オーディオスレッドからロックなしで更新)。
pub struct StatsCounters {
    /// 直近の集計区間の処理時間・予算の合計(ns)と、ブロック負荷の最大(0.1% 単位)
    busy_ns: AtomicU64,
    budget_ns: AtomicU64,
    max_permille: AtomicU64,
    /// 起動からの累計: 処理が予算(ブロック長)を超えた回数
    pub overruns: AtomicU64,
    /// 起動からの累計: 再生中、前回のコールバックからブロック長の 1.8 倍以上
    /// 空いて呼ばれた回数(OS / 他プロセスに CPU を奪われた)
    pub late: AtomicU64,
    /// 起動からの累計: 再生中の再生データ差し替え(発音中の音が切り直される)
    pub swaps: AtomicU64,
    /// 起動からの累計: OS のオーディオが知らせてきた音切れ(バッファの不足。対応している環境だけ)
    pub xruns: AtomicU64,
    /// OS がオーディオスレッドのリアルタイム優先度を認めなかった
    pub realtime_denied: std::sync::atomic::AtomicBool,
    /// トラックごと・マスターの処理時間(ns)と、その間の予算(ns)。[`StatsCounters::take_loads`] で読んでリセット
    track_ns: [AtomicU64; MAX_TRACKS],
    master_ns: AtomicU64,
    load_budget_ns: AtomicU64,
}

/// UI に渡す負荷の要約。
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct DspStats {
    /// 直近区間の平均負荷(%、処理時間 / ブロック長)
    pub avg_pct: f32,
    /// 直近区間で最も重かったブロックの負荷(%)
    pub max_pct: f32,
    pub overruns: u64,
    pub late: u64,
    pub swaps: u64,
    pub xruns: u64,
    pub realtime_denied: bool,
}

impl Default for StatsCounters {
    fn default() -> Self {
        StatsCounters {
            busy_ns: AtomicU64::new(0),
            budget_ns: AtomicU64::new(0),
            max_permille: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            late: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
            xruns: AtomicU64::new(0),
            realtime_denied: AtomicBool::new(false),
            track_ns: std::array::from_fn(|_| AtomicU64::new(0)),
            master_ns: AtomicU64::new(0),
            load_budget_ns: AtomicU64::new(0),
        }
    }
}

impl StatsCounters {
    /// 最初の `ntracks` 本とマスターの、前回からの処理の重さ(%。処理時間 / 音の長さ)を読み出してリセットする。
    /// 音源の発音はトラックをまたいで 1 サンプルずつ回すので、その時間は鳴らした声の数で按分した目安
    pub fn take_loads(&self, ntracks: usize) -> (Vec<f32>, f32) {
        let budget = self.load_budget_ns.swap(0, Ordering::AcqRel).max(1) as f32;
        let pct = |ns: u64| ((ns as f32 / budget * 1000.0).round() / 10.0).min(999.0);
        let tracks = self
            .track_ns
            .iter()
            .take(ntracks.min(MAX_TRACKS))
            .map(|a| pct(a.swap(0, Ordering::AcqRel)))
            .collect();
        (tracks, pct(self.master_ns.swap(0, Ordering::AcqRel)))
    }

    /// 直近区間の平均・最大を読み出してリセットする(累計カウンタはそのまま)。
    pub fn take(&self) -> DspStats {
        let busy = self.busy_ns.swap(0, Ordering::AcqRel);
        let budget = self.budget_ns.swap(0, Ordering::AcqRel);
        let max = self.max_permille.swap(0, Ordering::AcqRel);
        DspStats {
            avg_pct: if budget > 0 {
                busy as f32 / budget as f32 * 100.0
            } else {
                0.0
            },
            max_pct: max as f32 / 10.0,
            overruns: self.overruns.load(Ordering::Acquire),
            late: self.late.load(Ordering::Acquire),
            swaps: self.swaps.load(Ordering::Acquire),
            xruns: self.xruns.load(Ordering::Acquire),
            realtime_denied: self.realtime_denied.load(Ordering::Acquire),
        }
    }
}

/// この(オーディオ)スレッドでデノーマル数を 0 に丸める(x86_64 は FTZ / DAZ、aarch64 は FZ)。
/// 残響やフィルタが減衰しきる直前の極小値は桁違いに遅い演算になり、
/// 音の消え際で処理落ちやノイズを起こすことがある。DAW では標準的な対策。
/// 設定はスレッドごとで子スレッドに受け継がれないので、処理するスレッドで毎ブロック呼ぶ。
#[inline]
pub fn flush_denormals() {
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    // SAFETY: MXCSR の FTZ(bit 15)と DAZ(bit 6)を立てるだけ。このスレッドの
    // 浮動小数演算の丸め方が変わるが、オーディオ処理では望ましい挙動
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: FPCR の FZ(bit 24)を立てるだけ(Apple Silicon などの ARM)。
    // このスレッドの浮動小数演算でデノーマル数が 0 になる
    unsafe {
        let fpcr: u64;
        std::arch::asm!("mrs {}, fpcr", out(reg) fpcr, options(nomem, nostack, preserves_flags));
        if fpcr & (1 << 24) == 0 {
            std::arch::asm!("msr fpcr, {}", in(reg) fpcr | (1 << 24), options(nomem, nostack, preserves_flags));
        }
    }
}

impl Shared {
    pub fn new(data: PlaybackData) -> Self {
        Shared {
            ab: arc_swap::ArcSwapOption::empty(),
            ab_side: std::sync::atomic::AtomicU8::new(0),
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0),
            seek: AtomicU64::new(NO_SEEK),
            preview: AtomicU64::new(0),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            jump_at: AtomicU64::new(NO_SEEK),
            jump_to: AtomicU64::new(0),
            jump_loop_start: AtomicU64::new(0),
            jump_loop_end: AtomicU64::new(NO_SEEK),
            last_jump: JumpLog::default(),
            live_gain: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
            output_gain: AtomicU32::new(1.0f32.to_bits()),
            metronome: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            click_only: AtomicBool::new(false),
            no_master_clip: AtomicBool::new(false),
            data: ArcSwap::from_pointee(data),
            stats: StatsCounters::default(),
            live: LiveQueue::default(),
            notes: Default::default(),
            live_track: AtomicU32::new(LIVE_NO_TRACK),
            epoch: std::time::Instant::now(),
            block_frames: AtomicU32::new(0),
            pos_nanos: AtomicU64::new(0),
            plugin_slots: Arc::new(crate::plugins::new_slots()),
            levels: Levels::default(),
            monitor: Default::default(),
        }
    }

    /// いま聞こえている(と推定される)再生位置(サンプル、小数)。
    /// レンダラは 1 ブロック先まで書いてから `pos` を更新するので、1 ブロック分戻し、
    /// 前回の書き込みからの経過時間ぶん進める(1 ブロック以内に制限)。概算であり、
    /// デバイス固有の出力遅延までは含まない
    pub fn audible_pos(&self) -> f64 {
        let pos = self.pos.load(Ordering::Acquire) as f64;
        if !self.playing.load(Ordering::Acquire) {
            return pos;
        }
        let block = self.block_frames.load(Ordering::Acquire) as f64;
        let sr = self.data.load().sample_rate;
        let written = self.pos_nanos.load(Ordering::Acquire);
        let now = self.epoch.elapsed().as_nanos() as u64;
        let elapsed = now.saturating_sub(written) as f64 * 1e-9 * sr;
        (pos - block + elapsed.min(block)).max(0.0)
    }
}

struct Voice {
    end: u64,
    track: u32,
    released: bool,
    /// ループ折り返しを何回またいだか。リリースの長い音が周回ごとに世代累積して
    /// ボイスプールを食い潰さないよう、2 回またいだら強制解放する
    wraps: u8,
    /// レガートのつなぎ(サンプル数。0 = なし)と鳴り始めからの経過
    fade_in: u32,
    fade_out: u32,
    age: u32,
    /// 同時発音数の上限で奪われ、フェードアウト中
    stolen: bool,
    /// 鳴らし始めたときのトラックの識別子・発音内容(`TrackMix::ident` / `content`)。
    /// データを差し替えたとき、同じなら鳴らし続ける
    ident: u64,
    content: u64,
    state: VoiceState,
    /// ノートの音量・明るさの曲線(出口にかける)
    shape: glaux_dsp::NoteShape,
    /// 0 = 本体の音源、1 以上 = `TrackMix::layers` の番号 + 1
    layer: u8,
}

/// 同時発音数の上限に達したとき、奪うボイスを選ぶ(奪われ中のものは除く)。
/// リリース中(鍵盤を離した余韻)の音を優先し、その中で最も古いもの。無ければ最も古いもの
fn steal_victim(voices: &[Voice]) -> Option<usize> {
    voices
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.stolen)
        .max_by_key(|(_, v)| (v.released, v.age))
        .map(|(i, _)| i)
}

/// 再生中の音声クリップ。波形は `data.audio_events[idx]` を参照する
/// (データ差し替え時は resync で作り直すので添字が古くなることはない)。
#[derive(Clone, Copy)]
struct AudioVoice {
    idx: usize,
    /// 波形内の再生位置(ネイティブレートのフレーム、小数)
    pos: f64,
}

/// メトロノームのクリック(減衰するサイン波。小節頭は高い音)。
#[derive(Clone, Copy, Default)]
struct Click {
    active: bool,
    age: u32,
    downbeat: bool,
}

impl Click {
    fn next(&mut self, sr: f32) -> f32 {
        if !self.active {
            return 0.0;
        }
        let t = self.age as f32 / sr;
        if t > 0.06 {
            self.active = false;
            return 0.0;
        }
        self.age += 1;
        let freq = if self.downbeat { 1500.0 } else { 1000.0 };
        let env = (-t / 0.012).exp();
        0.35 * env * (t * freq * std::f32::consts::TAU).sin()
    }
}

/// UI からの試聴用ボイス。停止中でも鳴り、トラックエフェクトは通さない。
struct PreviewVoice {
    /// note_off までの残りサンプル数
    remaining: u32,
    released: bool,
    gain_l: f32,
    gain_r: f32,
    instrument: glaux_dsp::InstrumentParams,
    state: VoiceState,
}

/// MIDI キーボードのライブ発音ボイス。送り先トラックの楽器で鳴らし、
/// トラックのエフェクト・音量・パンを通す(停止中でも鳴る)。
struct LiveVoice {
    /// 送り先トラック index(`LIVE_NO_TRACK` なら既定音色でマスター直行)
    track: u32,
    pitch: u8,
    /// 弾いた MIDI チャンネル(MPE ではチャンネルごとのベンド・音色・押し込みがこの音にだけ効く)
    ch: u8,
    /// 音色(CC74)・押し込みを明るさ・音量としてかける
    shape: glaux_dsp::NoteShape,
    released: bool,
    /// 時刻指定のノート: この時計の位置で離す(MIDI キーボードの音は u64::MAX = 鍵盤を離すまで)
    off_at: u64,
    /// ペダルで保持中(ペダルを離したらリリース)
    sustained: bool,
    instrument: glaux_dsp::InstrumentParams,
    state: VoiceState,
}

/// プラグインへ送ったノートの note off 待ち。
#[derive(Clone, Copy)]
struct PendingOff {
    slot: u8,
    key: u8,
    /// この位置で離す。`seq` なら再生位置(サンプル)、そうでなければ `clock` 基準。
    /// `u64::MAX` は鍵盤を離すまで(ライブ演奏)
    end: u64,
    seq: bool,
    /// ピッチカーブのあるノート: `data.events` の添字(無ければ `u32::MAX`)と、
    /// 送ったノート ID・最後に送った音程(半音)
    ev: u32,
    note_id: u32,
    last_semi: f32,
    /// 最後に送った音量(倍率)・明るさ(0〜1)
    last_gain: f32,
    last_bright: f32,
    /// ライブ演奏の音: 弾いた MIDI チャンネル
    ch: u8,
}

impl PendingOff {
    fn simple(slot: usize, key: u8, end: u64, seq: bool) -> Self {
        PendingOff {
            slot: slot as u8,
            key,
            end,
            seq,
            ev: u32::MAX,
            note_id: 0,
            last_semi: 0.0,
            last_gain: f32::NAN,
            last_bright: f32::NAN,
            ch: 0,
        }
    }
}

/// 1 トラックで扱う CLAP パラメータのオートメーションレーン数
pub const MAX_PLUGIN_LANES: usize = 32;
/// プラグインのパラメータ・ピッチカーブを送る間隔(サンプル)
const PLUGIN_CTRL_STEP: usize = 64;

/// プラグインの遅延補正で遅らせられる上限(サンプル)。48kHz で約 0.17 秒
const MAX_PDC: usize = 8192;

/// 送り先ごとの「先に書いておく」輪状バッファの長さ。遅延補正の上限 + 1 ブロック
const RING: usize = MAX_PDC + MAX_FRAMES;
/// エフェクトの分岐の遅れを揃えるのに使える輪状バッファの数(全トラック・マスターで共有)
const BRANCH_RINGS: usize = 16;
/// 分岐の揃えを使わない印
const NO_RING: u8 = u8::MAX;

/// 輪状バッファの `start` から(折り返して)左右の音を `amp` 倍して足す
fn ring_add(ring: &mut [Vec<f32>; 2], start: usize, l: &[f32], r: &[f32], amp: f32) {
    let n = ring[0].len();
    let start = start % n;
    let first = l.len().min(n - start);
    for (side, src) in [(0, l), (1, r)] {
        let (a, b) = src.split_at(first);
        for (d, s) in ring[side][start..start + first].iter_mut().zip(a) {
            *d += s * amp;
        }
        for (d, s) in ring[side][..b.len()].iter_mut().zip(b) {
            *d += s * amp;
        }
    }
}

/// 輪状バッファの `start` から `l.len()` 分を取り出し(上書き)、その区間を 0 に戻す
fn ring_take(ring: &mut [Vec<f32>; 2], start: usize, l: &mut [f32], r: &mut [f32]) {
    let n = ring[0].len();
    let start = start % n;
    let first = l.len().min(n - start);
    for (side, dst) in [(0, l), (1, r)] {
        let (a, b) = dst.split_at_mut(first);
        a.copy_from_slice(&ring[side][start..start + first]);
        ring[side][start..start + first].fill(0.0);
        let rest = b.len();
        b.copy_from_slice(&ring[side][..rest]);
        ring[side][..rest].fill(0.0);
    }
}

/// つながりのノードの入口: 入ってくる線の音を足す。`ring` があれば、各線を「入口の時刻 − 線の元の出口の時刻」
/// だけ先に書いてから読み出し、遅れの違う枝を揃える
fn gather(
    inputs: &[(u16, f32)],
    done: &[[Vec<f32>; 2]],
    ol: &mut [f32],
    or: &mut [f32],
    ring: Option<(&mut [Vec<f32>; 2], usize)>,
    times: &[u32],
    in_time: u32,
) {
    let frames = ol.len();
    match ring {
        Some((ring, pos)) => {
            for &(src, amp) in inputs {
                let Some([sl, sr]) = done.get(src as usize) else {
                    continue;
                };
                let off = in_time.saturating_sub(times[src as usize]) as usize;
                ring_add(
                    ring,
                    pos + off.min(MAX_PDC - 1),
                    &sl[..frames],
                    &sr[..frames],
                    amp,
                );
            }
            ring_take(ring, pos, ol, or);
        }
        None => {
            ol.fill(0.0);
            or.fill(0.0);
            for &(src, amp) in inputs {
                let Some([sl, sr]) = done.get(src as usize) else {
                    continue;
                };
                for f in 0..frames {
                    ol[f] += sl[f] * amp;
                    or[f] += sr[f] * amp;
                }
            }
        }
    }
}

/// ノードの入口の時刻: 入ってくる線の元(`limit` 以下の番号)の出口の時刻の最大
fn in_time_of(times: &[u32], inputs: &[(u16, f32)], limit: usize) -> u32 {
    inputs
        .iter()
        .filter(|(src, _)| (*src as usize) <= limit)
        .map(|(src, _)| times[*src as usize])
        .max()
        .unwrap_or(0)
}

/// トラックを処理する順(先頭 `n` 個)。再生データの順が使えなければ「通常のトラック → バス」
fn process_order(data: &PlaybackData, ntracks: usize) -> ([u32; MAX_TRACKS], usize) {
    let mut order = [0u32; MAX_TRACKS];
    let ok = data.order.len() == ntracks && data.order.iter().all(|&i| (i as usize) < ntracks);
    if ok {
        order[..ntracks].copy_from_slice(&data.order);
    } else {
        let mut k = 0;
        for bus in [false, true] {
            for (i, m) in data.tracks.iter().take(ntracks).enumerate() {
                if m.is_bus == bus {
                    order[k] = i as u32;
                    k += 1;
                }
            }
        }
    }
    (order, ntracks)
}

/// 送っている CLAP のつまみの変調の上限(全トラック)
const MAX_MOD_SENT: usize = 256;

/// 送っている CLAP のつまみの変調 1 つ
#[derive(Clone, Copy, Debug)]
struct ModSent {
    slot: u16,
    id: u32,
    /// 最後に送った量(変調ならずれ、値として送るなら値)
    last: f32,
    /// 値として送っている(プラグインが変調を受けない)なら、外したときに戻す元の値
    restore: Option<f32>,
    /// このブロックで送り直したか(送らなかったものは外れたので戻す)
    seen: bool,
}

/// 同時に鳴らしておけるプラグインのノート数
const MAX_PENDING_OFFS: usize = 1024;

pub struct Renderer {
    shared: Arc<Shared>,
    voices: Vec<Voice>,
    audio_voices: Vec<AudioVoice>,
    /// `data.audio_events` の次に開始するイベントの添字
    next_audio: usize,
    preview_voices: Vec<PreviewVoice>,
    /// 最後に消費した試聴要求のカウンタ
    last_preview: u64,
    live_voices: Vec<LiveVoice>,
    /// MIDI チャンネルごとの今のベンド(セント)・音色(−1〜1)・押し込み(0〜1)。1 チャンネル目は全部の音に効く
    mpe_bend: [f32; 16],
    mpe_bright: [Option<f32>; 16],
    mpe_press: [Option<f32>; 16],
    /// サステインペダルを踏んでいるか
    sustain: bool,
    /// ライブ演奏の残響を停止中にも鳴らす残りサンプル数
    live_tail: u32,
    /// CLAP プラグインの処理窓口(スロット別。受け取りは [`PluginSlot`] 経由)
    plugins: Vec<Option<Box<Processor>>>,
    /// このブロックでプラグインへ送るノート(スロット別、時刻順)
    plugin_notes: Vec<Vec<NoteMsg>>,
    /// プラグインの出力(スロット別の左右。このブロックの分)
    plugin_out: Vec<[Vec<f32>; 2]>,
    plugin_pending: Vec<PendingOff>,
    /// このブロックでプラグインが鳴らすトラック → スロット
    track_plugin: [Option<usize>; MAX_TRACKS],
    /// 再生と無関係に進むサンプル時計(試聴・ライブ演奏の長さに使う)
    clock: u64,
    /// プラグインのオートメーションで最後に送った値(トラック × レーン。NaN = 未送信)
    plugin_auto_last: Vec<[f32; MAX_PLUGIN_LANES]>,
    /// CLAP のつまみに送っている変調(起動時に確保): 外れたら変調を 0 に戻す・値を元に戻すため
    mod_sent: Vec<ModSent>,
    /// ピッチカーブ付きのノートに振るノート ID
    next_note_id: u32,
    /// 時刻指定のノートで、まだ鳴らし始めていないもの(容量は起動時に確保)
    timed: Vec<crate::midi::TimedNote>,
    /// 曲のノートで最近離した鍵盤(スロット, 鍵盤)。レガート・ポルタメントでその余韻を切る(choke)のに使う
    plugin_released: Vec<(u8, u8)>,
    /// スロットごとの「この位置で余韻を切る」(u64::MAX = 予定なし)
    plugin_choke_at: [u64; MAX_PLUGINS],
    /// エフェクト状態プール(リバーブのバッファ込みで起動時に確保)
    effect_states: Vec<EffectState>,
    /// このブロックの頭の曲の位置(tick)と 1 サンプルあたりの tick(停止中は 0)。テンポに合わせるエフェクト用
    blk_tick: f64,
    blk_tps: f64,
    /// このブロックで CLAP エフェクトとして使うプラグインのスロット(音源と分けて処理する)
    slot_is_fx: [bool; MAX_PLUGINS],
    /// このブロックで処理したエフェクトのスロット(処理しなかったものは最後に無音で処理する)
    slot_done: [bool; MAX_PLUGINS],
    /// ブロック用バッファ(起動時に MAX_FRAMES で確保): トラックごとのエフェクト前の合算、
    /// エフェクトを通さずマスターへ行く分(左右)、クリック、各フレームの再生位置
    blk_mono: Vec<Vec<f32>>,
    /// トラックごとのステレオ素材の左右差成分(エフェクト前)
    blk_side: Vec<Vec<f32>>,
    blk_direct: [Vec<f32>; 2],
    blk_click: Vec<f32>,
    blk_pos: Vec<u64>,
    /// エフェクトチェーンの作業用(左右)と、マスター前の合算(左右)
    fx_l: Vec<f32>,
    /// エフェクトのつながり(分岐・合流)用: 入力と各エフェクトの出力(ステレオ)。起動時に確保
    graph_bufs: Vec<[Vec<f32>; 2]>,
    fx_r: Vec<f32>,
    mix_l: Vec<f32>,
    mix_r: Vec<f32>,
    /// バスの入力(このブロックで受けた音。まだ処理していないバスは前のブロックの音。サイドチェインのキーにも使う)
    bus_l: Vec<Vec<f32>>,
    bus_r: Vec<Vec<f32>>,
    /// 遅延補正(PDC)。各トラックの音が出口に届く時刻(入力の遅れ + 自分の遅れ)と、入口に揃える時刻
    /// (流れ込む音のうち一番遅いもの)。送る音は「受け側の入口 − 送り側の出口」だけ先の位置に書いておく
    pdc_out: [u32; MAX_TRACKS],
    pdc_in: [u32; MAX_TRACKS],
    pdc_master_in: u32,
    /// テスト用: 各トラックの出力先(バスかマスター)へ書くときの遅らせる量
    pdc_delay: [u32; MAX_TRACKS],
    /// 送り先(バス)ごとの輪状バッファと、マスター前の合算の輪状バッファ。位置は全部で共通
    route_ring: Vec<[Vec<f32>; 2]>,
    master_ring: [Vec<f32>; 2],
    ring_pos: usize,
    /// エフェクトの分岐の遅れ揃え: つながりごと(トラック + マスター)の各ノードの出口の時刻と、
    /// 入口で揃えるのに使う輪状バッファの番号(NO_RING なら揃えない)。末尾が出口
    graph_time: Vec<[u32; crate::data::MAX_GRAPH_NODES + 2]>,
    graph_ring: Vec<[u8; crate::data::MAX_GRAPH_NODES + 2]>,
    branch_rings: Vec<[Vec<f32>; 2]>,
    /// トラックごとの無音連続サンプル数(残響が消えたらエフェクト処理を省く)
    track_silence: [u32; MAX_TRACKS],
    /// オートメーション評価カーソル(vol, pan)。単調前進、resync でリセット
    auto_cursors: [(usize, usize); MAX_TRACKS],
    /// マスター音量オートメーションの評価カーソル
    master_cursor: usize,
    /// トラックの音量・パンの左右ゲインを、目標へ約 5ms でなめらかに寄せた値(NaN = まだ無い → 最初は目標そのまま)。
    /// フェーダーやパンを動かしたとき・ブロックごとのオートメーションの段差で「プチッ」と鳴らないように
    gain_smooth: [(f32, f32); MAX_TRACKS],
    /// アプリの音量(`Shared::output_gain`)のなめらかな追従
    out_smooth: f32,
    /// マスター音量のなめらかにした値(同上)
    master_smooth: f32,
    /// A/B の聴き比べ: 聴き比べの音の混ぜ具合(0 = ふつうの再生、1 = 聴き比べ)と、B の混ぜ具合(0 = A、1 = B)
    ab_on: f32,
    ab_mix: f32,
    /// このブロックでトラックごとに鳴らした声のサンプル数(発音の時間の按分用)
    voice_samples: [u32; MAX_TRACKS],
    /// 相関・ゴニオメーターの測定と、聴き方の切り替え([`crate::monitor`])
    monitor: crate::monitor::MonitorState,
    /// device オートメーション適用済みの楽器パラメータ(トラック別スクラッチ)。
    /// レーンのあるトラックだけブロック頭でベースからコピーして値を上書きする。
    /// 起動時に確保し、以後アロケーションしない(clone は Arc 参照カウントのみ)
    inst_scratch: Vec<glaux_dsp::InstrumentParams>,
    /// fx オートメーション適用済みのエフェクト定義(状態スロット別)。None なら焼いたまま使う
    fx_scratch: Vec<Option<EffectParams>>,
    /// `data.events` の次に発音するイベントの添字
    next_event: usize,
    /// 直前に見ていた `PlaybackData` のアドレス(差し替え検出用)
    last_data: usize,
    /// 直前のデータのトラックごとの (識別子, 発音内容)(差し替えで何が変わったかを知る)
    prev_tracks: [(u64, u64); MAX_TRACKS],
    prev_len: usize,
    /// 差し替え・シークで発音内容が変わった(= またいでいる音を鳴らし直す)トラック
    retrig: [bool; MAX_TRACKS],
    /// `data.events` のこの添字より前は「鳴らし直し」の範囲(差し替え・シークの位置をまたぐ音だけ鳴らす)
    replay_until: usize,
    pos: u64,
    /// 直前ブロックで再生中だったか(再開時に音声クリップを途中から鳴らし直す)
    was_playing: bool,
    /// メトロノーム: 次のクリック位置と発音中のクリック
    next_beat: Option<crate::data::Beat>,
    click: Click,
    /// `pos` に対応する音楽的位置(tick)。データ差し替え(テンポ変更)時に
    /// この tick を保ったままサンプル位置を換算し直す
    last_tick: f64,
    /// 前回のコールバック時刻(呼び出し遅延の検出用)
    last_call: Option<std::time::Instant>,
}

/// 音量・パンのなめらかさ(1 サンプルで目標へ寄せる割合。時定数 5ms)
fn gain_smooth_coef(sr: f32) -> f32 {
    1.0 - (-1.0 / (0.005 * sr.max(1.0))).exp()
}

/// オートメーション点列を区分補間で評価する(core の `AutomationLane::value_at` と同義)。
/// `cursor` は「sample <= pos の最後の点」の添字で、単調に前進させる。
fn eval_auto(points: &[AutoPoint], cursor: &mut usize, pos: u64) -> f32 {
    while *cursor + 1 < points.len() && points[*cursor + 1].sample <= pos {
        *cursor += 1;
    }
    let a = points[*cursor];
    if pos < a.sample {
        return a.value; // 最初の点より前
    }
    let Some(b) = points.get(*cursor + 1) else {
        return a.value; // 最後の点より後
    };
    let span = (b.sample - a.sample) as f32;
    if span <= 0.0 {
        return a.value;
    }
    let t = (pos - a.sample) as f32 / span;
    match a.curve {
        Curve::Hold => a.value,
        Curve::Linear => a.value + (b.value - a.value) * t,
        Curve::Exponential => a.value + (b.value - a.value) * t * t,
    }
}

/// オートメーションの値を CLAP エフェクトのパラメータ(`clap:<id>`)として積む(ブロック頭の 1 回)。
fn push_plugin_param(notes: &mut Vec<NoteMsg>, name: &str, value: f32) {
    let Some(id) = crate::plugins::parse_param_key(name) else {
        return;
    };
    if notes.len() < MAX_EVENTS {
        notes.push(NoteMsg::Param {
            time: 0,
            id,
            value: value as f64,
        });
    }
}

impl Renderer {
    /// 差し替え・シークで発音内容が変わったトラックか(MAX_TRACKS 超のトラックは常に鳴らし直す)
    fn retrig_track(&self, track: u32) -> bool {
        self.retrig.get(track as usize).copied().unwrap_or(true)
    }

    pub fn new(shared: Arc<Shared>) -> Self {
        // 撥弦の弦の置き場(最初の 1 回だけ確保する。オーディオスレッドより前のここで)
        glaux_dsp::string_pool::init();
        Renderer {
            shared,
            voices: Vec::with_capacity(MAX_VOICES + STEAL_RESERVE),
            audio_voices: Vec::with_capacity(MAX_AUDIO_VOICES),
            next_audio: 0,
            preview_voices: Vec::with_capacity(MAX_PREVIEW_VOICES),
            last_preview: 0,
            live_voices: Vec::with_capacity(MAX_LIVE_VOICES),
            mpe_bend: [0.0; 16],
            mpe_bright: [None; 16],
            mpe_press: [None; 16],
            sustain: false,
            live_tail: 0,
            plugins: (0..MAX_PLUGINS).map(|_| None).collect(),
            plugin_notes: (0..MAX_PLUGINS)
                .map(|_| Vec::with_capacity(MAX_EVENTS))
                .collect(),
            plugin_out: (0..MAX_PLUGINS)
                .map(|_| [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]])
                .collect(),
            plugin_pending: Vec::with_capacity(MAX_PENDING_OFFS),
            track_plugin: [None; MAX_TRACKS],
            clock: 0,
            plugin_auto_last: vec![[f32::NAN; MAX_PLUGIN_LANES]; MAX_TRACKS],
            mod_sent: Vec::with_capacity(MAX_MOD_SENT),
            next_note_id: 1,
            timed: Vec::with_capacity(MAX_TIMED_NOTES),
            plugin_released: Vec::with_capacity(MAX_PENDING_OFFS),
            plugin_choke_at: [u64::MAX; MAX_PLUGINS],
            // clone で複製すると 0 のバッファを実際に書き写してしまう(64 × 512KB)。1 つずつ確保すれば
            // ディレイ系を使うまでページは実体化しない
            blk_tick: 0.0,
            blk_tps: 0.0,
            effect_states: (0..MAX_EFFECT_SLOTS)
                .map(|_| EffectState::default())
                .collect(),
            slot_is_fx: [false; MAX_PLUGINS],
            slot_done: [false; MAX_PLUGINS],
            blk_mono: (0..MAX_TRACKS).map(|_| vec![0.0; MAX_FRAMES]).collect(),
            blk_side: (0..MAX_TRACKS).map(|_| vec![0.0; MAX_FRAMES]).collect(),
            blk_direct: [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]],
            blk_click: vec![0.0; MAX_FRAMES],
            blk_pos: vec![0; MAX_FRAMES],
            fx_l: vec![0.0; MAX_FRAMES],
            graph_bufs: (0..=crate::data::MAX_GRAPH_NODES)
                .map(|_| [vec![0.0; MAX_FRAMES], vec![0.0; MAX_FRAMES]])
                .collect(),
            fx_r: vec![0.0; MAX_FRAMES],
            mix_l: vec![0.0; MAX_FRAMES],
            mix_r: vec![0.0; MAX_FRAMES],
            bus_l: (0..MAX_TRACKS).map(|_| vec![0.0; MAX_FRAMES]).collect(),
            bus_r: (0..MAX_TRACKS).map(|_| vec![0.0; MAX_FRAMES]).collect(),
            pdc_out: [0; MAX_TRACKS],
            pdc_in: [0; MAX_TRACKS],
            pdc_master_in: 0,
            pdc_delay: [0; MAX_TRACKS],
            route_ring: (0..MAX_TRACKS)
                .map(|_| [vec![0.0; RING], vec![0.0; RING]])
                .collect(),
            master_ring: [vec![0.0; RING], vec![0.0; RING]],
            ring_pos: 0,
            graph_time: vec![[0; crate::data::MAX_GRAPH_NODES + 2]; MAX_TRACKS + 1],
            graph_ring: vec![[NO_RING; crate::data::MAX_GRAPH_NODES + 2]; MAX_TRACKS + 1],
            branch_rings: (0..BRANCH_RINGS)
                .map(|_| [vec![0.0; RING], vec![0.0; RING]])
                .collect(),
            track_silence: [u32::MAX; MAX_TRACKS],
            auto_cursors: [(0, 0); MAX_TRACKS],
            master_cursor: 0,
            gain_smooth: [(f32::NAN, f32::NAN); MAX_TRACKS],
            out_smooth: f32::NAN,
            master_smooth: f32::NAN,
            ab_on: 0.0,
            ab_mix: 1.0,
            voice_samples: [0; MAX_TRACKS],
            monitor: Default::default(),
            inst_scratch: vec![glaux_dsp::InstrumentParams::default(); MAX_TRACKS],
            fx_scratch: vec![None; MAX_EFFECT_SLOTS],
            next_event: 0,
            last_data: 0,
            prev_tracks: [(0, 0); MAX_TRACKS],
            prev_len: 0,
            retrig: [true; MAX_TRACKS],
            replay_until: 0,
            pos: 0,
            was_playing: false,
            next_beat: None,
            click: Click::default(),
            last_tick: 0.0,
            last_call: None,
        }
    }

    /// 音声クリップの再生状態を現在位置に合わせて作り直す。
    /// 位置をまたいでいるクリップは途中から鳴らす(シーク・ループ折返し・
    /// 編集によるデータ差し替えのどれでも音が途切れない)。
    fn resync_audio(&mut self, data: &PlaybackData) {
        self.audio_voices.clear();
        self.next_audio = data.audio_events.partition_point(|e| e.start < self.pos);
        for (idx, ev) in data.audio_events[..self.next_audio].iter().enumerate() {
            if ev.end <= self.pos || self.audio_voices.len() >= MAX_AUDIO_VOICES {
                continue;
            }
            if !data
                .tracks
                .get(ev.track as usize)
                .is_some_and(|m| m.audible)
            {
                continue;
            }
            self.audio_voices.push(AudioVoice {
                idx,
                pos: ev.offset + (self.pos - ev.start) as f64 * ev.rate,
            });
        }
    }

    /// `out` はインターリーブされた出力バッファ。
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        flush_denormals();
        let t0 = std::time::Instant::now();
        let was_playing = self.was_playing;
        // プラグインのバッファ長を超えないよう、長いブロックは分けて処理する。
        // 音色・エフェクトのオートメーションは処理単位の頭で評価するので、それがある曲では
        // 細かく分ける(以前は書き出しで 85ms 刻みの階段になり、再生とも音が違っていた)
        let step = if has_block_automation(&self.shared.data.load()) {
            AUTO_FRAMES
        } else {
            MAX_FRAMES
        };
        for chunk in out.chunks_mut(step * channels.max(1)) {
            self.process_inner(chunk, channels);
        }

        // 負荷統計(ロック・アロケーションなし)
        let sr = self.shared.data.load().sample_rate.max(1.0);
        let frames = out.len() / channels.max(1);
        let budget = (frames as f64 / sr * 1e9) as u64;
        let busy = t0.elapsed().as_nanos() as u64;
        let st = &self.shared.stats;
        st.busy_ns.fetch_add(busy, Ordering::Relaxed);
        st.budget_ns.fetch_add(budget, Ordering::Relaxed);
        st.load_budget_ns.fetch_add(budget, Ordering::Relaxed);
        if let Some(permille) = (busy * 1000).checked_div(budget) {
            st.max_permille.fetch_max(permille, Ordering::Relaxed);
        }
        if busy > budget {
            st.overruns.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(prev) = self.last_call {
            let gap = t0.duration_since(prev).as_nanos() as u64;
            if was_playing && self.was_playing && gap > budget * 18 / 10 {
                st.late.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.last_call = Some(t0);
    }

    fn process_inner(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        if channels == 0 {
            return;
        }

        let guard = self.shared.data.load();
        let data: &PlaybackData = &guard;
        let data_addr = std::sync::Arc::as_ptr(&guard) as usize;

        // データ差し替え・シークのどちらでも発音状態を作り直す
        let mut resync = false;
        let mut swapped = false;
        if data_addr != self.last_data {
            swapped = self.last_data != 0;
            self.last_data = data_addr;
            resync = true;
        }
        let seek = self.shared.seek.swap(NO_SEEK, Ordering::AcqRel);
        if swapped && self.was_playing {
            self.shared.stats.swaps.fetch_add(1, Ordering::Relaxed);
        }
        if seek != NO_SEEK {
            self.pos = seek;
            resync = true;
        } else if swapped && !data.tempo.is_empty() {
            // テンポが変わっていても音楽的位置(tick)を保つ
            self.pos = data.tick_to_sample(self.last_tick);
        }
        if resync {
            // 以前は鳴っている音を全部消していた(AI の編集やつまみの操作のたびに音が切れ、途中の音も戻らなかった)。
            // 発音内容が変わっていないトラックの音は鳴らし続け、変わったトラック(シークなら全部)の音だけ
            // 短いフェードで消して、位置をまたいでいる音を途中から鳴らし直す
            let seeking = seek != NO_SEEK;
            let fade = ((data.sample_rate as f32 * SWAP_FADE_SECS) as u32).max(1);
            for (j, m) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
                let same = !seeking
                    && self.prev_tracks[..self.prev_len]
                        .iter()
                        .any(|&(i, c)| i == m.ident && c == m.content);
                self.retrig[j] = !same;
            }
            for v in self.voices.iter_mut().filter(|v| !v.stolen) {
                let kept = (!seeking)
                    .then(|| {
                        data.tracks
                            .iter()
                            .position(|m| m.ident == v.ident && m.content == v.content)
                    })
                    .flatten();
                match kept {
                    Some(j) => v.track = j as u32,
                    None => {
                        // 同じトラックがあればそこを通して消す(消えたトラックの音は次のサンプルで外す)
                        v.track = data
                            .tracks
                            .iter()
                            .position(|m| m.ident == v.ident)
                            .map_or(u32::MAX, |j| j as u32);
                        v.stolen = true;
                        v.released = false;
                        v.end = self.pos;
                        v.fade_out = fade;
                    }
                }
            }
            self.prev_len = data.tracks.len().min(MAX_TRACKS);
            for (j, m) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
                self.prev_tracks[j] = (m.ident, m.content);
            }
            self.next_beat = None; // シークで戻ったら拍を取り直す
            self.auto_cursors = [(0, 0); MAX_TRACKS];
            self.master_cursor = 0;
            self.next_event = data.events.partition_point(|e| e.start < self.pos);
            // 位置をまたいでいる音(鳴らし直すトラックのもの)の最初の添字まで戻す
            let lim = self.next_event.saturating_sub(MAX_REPLAY_SCAN);
            let mut first = self.next_event;
            for k in (lim..self.next_event).rev() {
                let e = &data.events[k];
                if e.end > self.pos && self.retrig_track(e.track) {
                    first = k;
                }
            }
            self.replay_until = self.next_event;
            self.next_event = first;
            self.resync_audio(data);
            // 旧データの Arc(サンプル波形等)を掴んだままにしないようスクラッチを戻す
            for s in self.inst_scratch.iter_mut() {
                *s = glaux_dsp::InstrumentParams::default();
            }
            for s in self.fx_scratch.iter_mut() {
                *s = None;
            }
            // スロットの中身が変わっていたらエフェクト状態を作り直す(アロケーションなし)
            for fx in data
                .tracks
                .iter()
                .flat_map(|t| t.effects.iter())
                .chain(data.master_effects.iter())
            {
                self.effect_states[fx.slot as usize].ensure_kind(&fx.params);
            }
        }

        let sr = data.sample_rate as f32;

        // CLAP プラグイン: 窓口の受け取り・返却と、このブロックのノート列の準備
        self.exchange_plugins();
        for n in self.plugin_notes.iter_mut() {
            n.clear();
        }
        // プロジェクト(AI・取り消し)からのパラメータ変更
        for (i, slot) in self.shared.plugin_slots.clone().iter().enumerate() {
            if self.plugins[i].is_none() {
                continue;
            }
            while let Some((id, value)) = slot.params.pop() {
                let notes = &mut self.plugin_notes[i];
                if notes.len() < MAX_EVENTS {
                    notes.push(NoteMsg::Param { time: 0, id, value });
                }
            }
        }
        self.refresh_track_plugins(data);
        self.slot_is_fx = [false; MAX_PLUGINS];
        self.slot_done = [false; MAX_PLUGINS];
        for fx in data
            .tracks
            .iter()
            .flat_map(|t| t.effects.iter())
            .chain(data.master_effects.iter())
        {
            if let Some((ps, _)) = fx.plugin {
                if let Some(f) = self.slot_is_fx.get_mut(ps as usize) {
                    *f = true;
                }
            }
        }
        if resync {
            self.plugins_all_off(true);
            for l in self.plugin_auto_last.iter_mut() {
                *l = [f32::NAN; MAX_PLUGIN_LANES];
            }
            for m in self.mod_sent.iter_mut() {
                m.last = f32::NAN;
            }
        }

        // ノート試聴要求(カウンタ変化で 1 回だけ発音)
        let preview_req = self.shared.preview.load(Ordering::Acquire);
        if preview_req != self.last_preview && preview_req != 0 {
            self.last_preview = preview_req;
            let track = ((preview_req >> 32) & 0xFFFF) as usize;
            let dur_ms = ((preview_req >> 16) & 0xFFFF) as u32;
            let pitch = ((preview_req >> 8) & 0xFF) as u8;
            let vel = (preview_req & 0xFF) as u8;
            self.refresh_track_plugins(data);
            let plugin_slot = self.track_plugin.get(track).copied().flatten();
            if let Some(slot) = plugin_slot {
                // プラグインのトラック: ノートを送り、長さぶん後に離す
                self.plugin_note_on(slot, pitch, vel as f32 / 127.0, 0, None);
                self.push_pending(PendingOff::simple(
                    slot,
                    pitch,
                    self.clock + (dur_ms as f64 / 1000.0 * data.sample_rate) as u64,
                    false,
                ));
            }
            let (instrument, gain_l, gain_r) = match data.tracks.get(track) {
                Some(mix) => (mix.instrument.clone(), mix.gain_l, mix.gain_r),
                None => (glaux_dsp::InstrumentParams::default(), 0.8, 0.8),
            };
            if plugin_slot.is_none() && self.preview_voices.len() < MAX_PREVIEW_VOICES {
                let freq = crate::data::pitch_to_freq(pitch);
                let state = VoiceState::start(
                    &instrument,
                    freq,
                    pitch,
                    vel as f32 / 127.0,
                    glaux_core::Articulation::Normal,
                    sr,
                );
                self.preview_voices.push(PreviewVoice {
                    remaining: (dur_ms as f32 / 1000.0 * sr) as u32,
                    released: false,
                    gain_l,
                    gain_r,
                    instrument,
                    state,
                });
            }
        }

        // MIDI キーボードのライブ演奏(ブロック頭でまとめて反映。遅れは最大 1 ブロック)
        self.consume_live(data, sr);
        // 時刻指定のノートを受け取る(鳴らし始めはフレーム単位で正確に)
        while self.timed.len() < MAX_TIMED_NOTES {
            let Some(n) = self.shared.notes.pop() else {
                break;
            };
            self.timed.push(n);
        }

        let playing = self.shared.playing.load(Ordering::Acquire);
        if playing && !self.was_playing && !resync {
            self.resync_audio(data);
        }
        if !playing && self.was_playing {
            // 止めたらプラグインで鳴っている曲のノートを離す
            self.plugins_all_off(true);
        }
        self.was_playing = playing;
        let any_plugin = self.plugins.iter().any(Option::is_some);
        if !playing {
            self.voices.clear();
            self.audio_voices.clear();
            if self.preview_voices.is_empty()
                && self.live_voices.is_empty()
                && self.timed.is_empty()
                && self.live_tail == 0
                && !any_plugin
            {
                self.last_tick = data.sample_to_tick(self.pos);
                self.clock += (out.len() / channels) as u64;
                self.publish_pos(out.len() / channels);
                return;
            }
        }
        let block_frames = out.len() / channels;
        if self.live_voices.is_empty() {
            self.live_tail = self.live_tail.saturating_sub(block_frames as u32);
        } else {
            self.live_tail = (LIVE_TAIL_SECS * sr) as u32;
        }

        let hard_limit = (VOICE_HARD_LIMIT_SECS * sr) as u64;
        let frames = out.len() / channels;
        // テンポに合わせるエフェクト(トランスゲート・音量シェイパーなど)に渡す曲の位置
        self.blk_tick = data.sample_to_tick(self.pos);
        self.blk_tps = if playing && frames > 0 {
            (data.sample_to_tick(self.pos + frames as u64) - self.blk_tick) / frames as f64
        } else {
            0.0
        };
        // 声ごとのモジュレーター(テンポに合わせる揺れ)に今のテンポと曲の位置を渡す
        if let Some(spb) = data.secs_per_beat(self.pos) {
            glaux_dsp::tone::set_beat_secs(spb as f32);
        }
        glaux_dsp::tone::set_song_beat(self.blk_tick / glaux_core::PPQ as f64);

        // ループ区間(このブロックの間は固定値として扱う。予約した飛びでだけ替わる)
        let mut loop_start = self.shared.loop_start.load(Ordering::Acquire);
        let mut loop_end = self.shared.loop_end.load(Ordering::Acquire);
        let mut looping = loop_end > loop_start;
        // 予約した飛び先(`jump_at` を先に読み、合図が立っていれば残りを読む)
        let mut jump_at = self.shared.jump_at.load(Ordering::Acquire);

        // CLAP プラグインへ渡す曲の進み具合(テンポ同期する LFO・アルペジエーター・ディレイ用)。
        // ブロックの頭の値を渡し、プラグインの側でチャンクごとに進める
        let transport =
            data.transport_at(self.pos, playing, looping.then_some((loop_start, loop_end)));
        for p in self.plugins.iter_mut().flatten() {
            p.clap.set_transport(transport);
        }

        // メトロノーム: このブロックで最初に来る拍を求める(resync 後も自然に追従)
        let metronome = self.shared.metronome.load(Ordering::Acquire);
        let click_only = self.shared.click_only.load(Ordering::Acquire);
        if metronome && playing {
            if self.next_beat.is_none_or(|b| b.sample < self.pos) {
                self.next_beat = data.next_beat(self.pos);
            }
        } else {
            self.next_beat = None;
        }

        // 音色パラメータのオートメーション: ブロック頭で評価してスクラッチに適用する
        // (1 ブロック ≈ 数 ms なので聴感上は連続。発音中のボイスにも効く =
        //  フィルタスイープ等が鳴る)
        // エフェクトのパラメータのオートメーションも同様(スロット別の作業用コピー)
        for mix in data.tracks.iter().take(MAX_TRACKS) {
            for (slot, name, points) in &mix.fx_auto {
                let Some(fx) = mix.effects.iter().find(|f| f.slot == *slot) else {
                    continue;
                };
                if let Some((ps, _)) = fx.plugin {
                    // CLAP エフェクトのつまみ(clap:<id>)はプラグインへ送る(変調を値として送るつまみは変調の側で)
                    if crate::plugins::parse_param_key(name)
                        .is_some_and(|id| self.mod_as_value(mix, Some(*slot), ps as usize, id))
                    {
                        continue;
                    }
                    let cursor = points.partition_point(|pt| pt.sample <= self.pos);
                    let v = eval_auto(points, &mut cursor.saturating_sub(1), self.pos);
                    push_plugin_param(&mut self.plugin_notes[ps as usize], name, v);
                    continue;
                }
                let s = *slot as usize;
                if s >= self.fx_scratch.len() {
                    continue;
                }
                let mut p = self.fx_scratch[s].unwrap_or(fx.params);
                let mut cursor = points.partition_point(|pt| pt.sample <= self.pos);
                cursor = cursor.saturating_sub(1);
                let v = eval_auto(points, &mut cursor, self.pos);
                p.set_continuous(name, v, sr);
                self.fx_scratch[s] = Some(p);
            }
        }
        for (slot, name, points) in &data.master_fx_auto {
            let Some(fx) = data.master_effects.iter().find(|f| f.slot == *slot) else {
                continue;
            };
            if let Some((ps, _)) = fx.plugin {
                let cursor = points.partition_point(|pt| pt.sample <= self.pos);
                let v = eval_auto(points, &mut cursor.saturating_sub(1), self.pos);
                push_plugin_param(&mut self.plugin_notes[ps as usize], name, v);
                continue;
            }
            let s = *slot as usize;
            if s >= self.fx_scratch.len() {
                continue;
            }
            let mut p = self.fx_scratch[s].unwrap_or(fx.params);
            let cursor = points.partition_point(|pt| pt.sample <= self.pos);
            let v = eval_auto(points, &mut cursor.saturating_sub(1), self.pos);
            p.set_continuous(name, v, sr);
            self.fx_scratch[s] = Some(p);
        }
        for (ti, mix) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
            if mix.device_auto.is_empty() {
                continue;
            }
            self.inst_scratch[ti] = mix.instrument.clone();
            for (name, points) in &mix.device_auto {
                let mut cursor = points.partition_point(|p| p.sample <= self.pos);
                cursor = cursor.saturating_sub(1);
                let v = eval_auto(points, &mut cursor, self.pos);
                self.inst_scratch[ti].set_continuous(name, v);
            }
        }

        // CLAP プラグイン: このブロックのノートを集めて、先にブロック単位で処理しておく
        if any_plugin {
            self.collect_plugin_notes(
                data,
                frames,
                playing && !click_only,
                looping,
                loop_start,
                loop_end,
            );
            self.collect_plugin_automation(data, frames, playing);
            for notes in self.plugin_notes.iter_mut() {
                // 同じ時刻は「離す → パラメータ → 鳴らす → 表現」の順(並べ替えはアロケーションなし)
                notes.sort_unstable_by_key(|n| (n.time(), n.order()));
            }
            for slot in 0..MAX_PLUGINS {
                // エフェクトはトラックの音が揃ってから通す(process_track_chains)
                if self.slot_is_fx[slot] {
                    continue;
                }
                let Some(p) = self.plugins[slot].as_mut() else {
                    continue;
                };
                // 音源(やバイパス中のエフェクト)には音声を入れない
                let t = std::time::Instant::now();
                p.clap.clear_input(frames);
                p.clap.process(frames, &self.plugin_notes[slot]);
                let ns = t.elapsed().as_nanos() as u64;
                if let Some(ti) = self.track_plugin.iter().position(|s| *s == Some(slot)) {
                    self.shared.stats.track_ns[ti].fetch_add(ns, Ordering::Relaxed);
                }
                let [ol, or] = &mut self.plugin_out[slot];
                match p.clap.output() {
                    Some((l, r)) => {
                        ol[..frames].copy_from_slice(&l[..frames]);
                        or[..frames].copy_from_slice(&r[..frames]);
                    }
                    None => {
                        ol[..frames].fill(0.0);
                        or[..frames].fill(0.0);
                    }
                }
            }
        }

        // 1. フレームごとに発音し、トラックごとの合算(エフェクト前)をブロック用のバッファに溜める
        let ntracks = data.tracks.len().min(MAX_TRACKS);
        // トラックごとのモノ合算(エフェクト前)。毎フレーム消すのは使っているトラックの分だけ
        // (それより後ろのトラックの値は写さないので消さなくてよい)
        let mut track_mono = [0.0f32; MAX_TRACKS];
        let mut track_side = [0.0f32; MAX_TRACKS];
        let t_voices = std::time::Instant::now();
        self.voice_samples = [0; MAX_TRACKS];
        for frame in 0..frames {
            // ループ終端に達したら区間頭へ。発音中の音は note_off でリリースに回し
            // (ぶつ切りのクリックを避ける)、イベント・オートメーションのカーソルを再同期。
            // 旧世代のボイスは 1 周分のリリース猶予の後に解放する(無限に世代が
            // 積み重なって CPU が漸増するのを防ぐ)
            let wrap_to = if playing && jump_at != NO_SEEK && self.pos >= jump_at {
                // 予約した飛び: ループの折り返しより先に見る(ループの終わりちょうどに予約しても飛べる)
                let to = self.shared.jump_to.load(Ordering::Acquire);
                let ls = self.shared.jump_loop_start.load(Ordering::Acquire);
                let le = self.shared.jump_loop_end.load(Ordering::Acquire);
                if le != NO_SEEK {
                    loop_start = if le > ls { ls } else { 0 };
                    loop_end = if le > ls { le } else { 0 };
                    looping = loop_end > loop_start;
                    self.shared.loop_start.store(loop_start, Ordering::Release);
                    self.shared.loop_end.store(loop_end, Ordering::Release);
                }
                // 飛んでいる間に次の予約が書かれていたら消さない
                let _ = self.shared.jump_at.compare_exchange(
                    jump_at,
                    NO_SEEK,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                jump_at = NO_SEEK;
                Some(to)
            } else if playing && looping && self.pos >= loop_end {
                Some(loop_start)
            } else {
                None
            };
            if let Some(to) = wrap_to {
                self.shared
                    .last_jump
                    .write(self.clock + frame as u64, self.pos, to);
                self.pos = to;
                self.voices.retain_mut(|v| {
                    // 奪われてフェード中の音は、折り返しを機に消す
                    if v.stolen {
                        return false;
                    }
                    if !v.released {
                        v.state.note_off();
                        v.released = true;
                    }
                    v.wraps = v.wraps.saturating_add(1);
                    v.wraps < 2
                });
                self.auto_cursors = [(0, 0); MAX_TRACKS];
                self.master_cursor = 0;
                self.next_event = data.events.partition_point(|e| e.start < self.pos);
                self.replay_until = 0;
                self.resync_audio(data);
                if metronome {
                    self.next_beat = data.next_beat(self.pos);
                }
            }

            // メトロノーム: 拍の位置でクリックを鳴らす
            if let Some(b) = self.next_beat {
                if playing && self.pos >= b.sample {
                    self.click = Click {
                        active: true,
                        age: 0,
                        downbeat: b.downbeat,
                    };
                    self.next_beat = data.next_beat(self.pos + 1);
                }
            }

            // このサンプル位置で始まる音声クリップを開始
            while playing
                && !click_only
                && self.next_audio < data.audio_events.len()
                && data.audio_events[self.next_audio].start <= self.pos
            {
                let idx = self.next_audio;
                self.next_audio += 1;
                let ev = &data.audio_events[idx];
                let audible = data
                    .tracks
                    .get(ev.track as usize)
                    .is_some_and(|m| m.audible);
                if audible && self.audio_voices.len() < MAX_AUDIO_VOICES {
                    self.audio_voices.push(AudioVoice {
                        idx,
                        pos: ev.offset,
                    });
                }
            }

            // このサンプル位置で始まるノートを発音(上限を超えたら古い音を奪う)。
            // 生きている声の数は、このサンプルで最初に鳴らすときに 1 回だけ数え、以後は足し引きで追う
            let mut live_count: Option<usize> = None;
            while playing
                && !click_only
                && self.next_event < data.events.len()
                && data.events[self.next_event].start <= self.pos
            {
                let idx = self.next_event;
                let e = data.events[self.next_event];
                self.next_event += 1;
                // 鳴らし直しの範囲: 位置をまたいでいる、鳴らし直すトラックの音だけ
                let replay = idx < self.replay_until;
                if replay && !(e.end > self.pos && self.retrig_track(e.track)) {
                    continue;
                }
                let mix = data.tracks.get(e.track as usize);
                // プラグインのトラックは collect_plugin_notes で送る
                if let Some(mix) = mix.filter(|m| m.audible && m.plugin.is_none()) {
                    // 本体と、範囲に合う層の数だけ空きを作る(上限を超えたら古い音を奪う)
                    let vel_midi = (e.amp * 127.0).round().clamp(1.0, 127.0) as u8;
                    let needed = 1 + mix
                        .layers
                        .iter()
                        .filter(|l| l.plays(e.pitch, vel_midi))
                        .count();
                    let mut live = *live_count
                        .get_or_insert_with(|| self.voices.iter().filter(|v| !v.stolen).count());
                    let before = self.voices.len();
                    while live + needed > MAX_VOICES {
                        let Some(i) = steal_victim(&self.voices) else {
                            break;
                        };
                        let v = &mut self.voices[i];
                        v.stolen = true;
                        v.released = false;
                        v.end = self.pos;
                        v.fade_out = ((sr * STEAL_FADE_SECS) as u32).max(1);
                        live -= 1;
                    }
                    if self.voices.len() < MAX_VOICES + STEAL_RESERVE {
                        // 声ごとのモジュレーターが曲の拍に合わせられるよう、この音の頭の位置(拍)を渡す
                        glaux_dsp::tone::set_song_beat(
                            data.sample_to_tick(self.pos) / glaux_core::PPQ as f64,
                        );
                        // 発音時パラメータ(pluck 等)にもスイープ中の値を反映する
                        let ti = e.track as usize;
                        let inst = if !mix.device_auto.is_empty() && ti < MAX_TRACKS {
                            &self.inst_scratch[ti]
                        } else {
                            &mix.instrument
                        };
                        let mut state = VoiceState::start_variant(
                            inst,
                            e.freq,
                            e.pitch,
                            e.amp,
                            e.articulation,
                            sr,
                            e.variant,
                        );
                        // チョーク: 同じトラック・同じ層の、このグループで止まる音(オープンハイハットなど)を止める
                        for group in state.choke_groups() {
                            if group == 0 {
                                continue;
                            }
                            for v in self.voices.iter_mut() {
                                if v.track == e.track && v.layer == 0 {
                                    v.state.choke_if(group);
                                }
                            }
                        }
                        let x = data.expr(&e);
                        if !x.curve.is_empty() {
                            state.set_curve(&x.curve);
                        }
                        if x.vibrato.is_active() {
                            state.set_vibrato(&x.vibrato);
                        }
                        // 鳴らし直す音は途中からなので、立ち上がりを飛ばして短くフェードイン
                        let fade_in = if replay {
                            e.fade_in.max(((sr * SWAP_FADE_SECS) as u32).max(1))
                        } else {
                            e.fade_in
                        };
                        if fade_in > 0 {
                            state.skip_attack(inst);
                        }
                        // レガート・ポルタメント: 同じトラックで先に離された音の余韻を
                        // つなぎ目の長さで消す(押さえたままの音には触れない)
                        if e.choke > 0 && !replay {
                            for v in self.voices.iter_mut() {
                                if v.track == e.track && v.released {
                                    v.released = false;
                                    v.end = self.pos;
                                    v.fade_out = e.choke;
                                }
                            }
                        }
                        self.voices.push(Voice {
                            end: e.end,
                            track: e.track,
                            released: false,
                            wraps: 0,
                            fade_in,
                            fade_out: e.fade_out,
                            age: 0,
                            stolen: false,
                            ident: mix.ident,
                            content: mix.content,
                            state,
                            shape: data.expr(&e).shape,
                            layer: 0,
                        });
                        // 重ねる音源: 元のノートの音程・強さで範囲を判定し、移調して鳴らす
                        for (li, layer) in mix.layers.iter().enumerate() {
                            if !layer.plays(e.pitch, vel_midi)
                                || self.voices.len() >= MAX_VOICES + STEAL_RESERVE
                            {
                                continue;
                            }
                            let pitch =
                                (e.pitch as i32 + layer.transpose as i32).clamp(0, 127) as u8;
                            let freq = e.freq * 2f32.powf(layer.transpose as f32 / 12.0);
                            let mut state = VoiceState::start_variant(
                                &layer.instrument,
                                freq,
                                pitch,
                                e.amp,
                                e.articulation,
                                sr,
                                e.variant,
                            );
                            for group in state.choke_groups() {
                                if group == 0 {
                                    continue;
                                }
                                for v in self.voices.iter_mut() {
                                    if v.track == e.track && v.layer == li as u8 + 1 {
                                        v.state.choke_if(group);
                                    }
                                }
                            }
                            let x = data.expr(&e);
                            if !x.curve.is_empty() {
                                state.set_curve(&x.curve);
                            }
                            if x.vibrato.is_active() {
                                state.set_vibrato(&x.vibrato);
                            }
                            if fade_in > 0 {
                                state.skip_attack(&layer.instrument);
                            }
                            self.voices.push(Voice {
                                end: e.end,
                                track: e.track,
                                released: false,
                                wraps: 0,
                                fade_in,
                                fade_out: e.fade_out,
                                age: 0,
                                stolen: false,
                                ident: mix.ident,
                                content: mix.content,
                                state,
                                shape: data.expr(&e).shape,
                                layer: li as u8 + 1,
                            });
                        }
                    }
                    // 鳴らした分を足す(奪った分は live から引いてある)
                    live_count = Some(live + (self.voices.len() - before));
                }
            }

            track_mono[..ntracks].fill(0.0);
            track_side[..ntracks].fill(0.0);
            let mut direct_l = 0.0f32; // MAX_TRACKS 超のトラックはエフェクトなしで直行
            let mut direct_r = 0.0f32;
            let mut i = 0;
            while i < self.voices.len() {
                let v = &mut self.voices[i];
                // resync 直後以外で track が範囲外になることはない
                let Some(mix) = data.tracks.get(v.track as usize) else {
                    self.voices.swap_remove(i);
                    continue;
                };
                // レガートでつながれた音は end で離さず、つなぎ目の長さで消す
                let mut fade = 1.0f32;
                if v.fade_out > 0 && self.pos >= v.end && !v.released {
                    let k = self.pos - v.end;
                    if k >= v.fade_out as u64 {
                        self.voices.swap_remove(i);
                        continue;
                    }
                    fade = 1.0 - k as f32 / v.fade_out as f32;
                } else if self.pos >= v.end && !v.released {
                    v.state.note_off();
                    v.released = true;
                }
                if v.age < v.fade_in {
                    fade *= v.age as f32 / v.fade_in as f32;
                }
                v.age = v.age.saturating_add(1);
                // device オートメーションのあるトラックはスクラッチ(適用済み)を読む。層は層の音源
                let ti = v.track as usize;
                let layer = match v.layer {
                    0 => None,
                    n => match mix.layers.get(n as usize - 1) {
                        Some(l) => Some(l),
                        None => {
                            self.voices.swap_remove(i);
                            continue;
                        }
                    },
                };
                let inst = match layer {
                    Some(l) => &l.instrument,
                    None if !mix.device_auto.is_empty() && ti < MAX_TRACKS => {
                        &self.inst_scratch[ti]
                    }
                    None => &mix.instrument,
                };
                // 鳴り終わったボイスはノート終了を待たずに解放する
                // (減衰しきったピアノ・読み切ったワンショット等が
                //  スロットと CPU を占有し続けないように)
                if v.state.finished(inst) || self.pos >= v.end + hard_limit {
                    self.voices.swap_remove(i);
                    continue;
                }
                // 左右に広がる音源(ユニゾンの広がり・パンの LFO)は左右の差も出す
                let (mut sample, mut vside) = v.state.next_stereo(inst);
                sample *= fade;
                vside *= fade;
                if v.shape.is_active() {
                    (sample, vside) = v.shape.process_stereo(sample, vside, v.age as f32, sr);
                }
                if let Some(c) = self.voice_samples.get_mut(v.track as usize) {
                    *c += 1;
                }
                // 層は層の音量・パン(中央成分と左右差成分)を掛ける。声の左右の差は層の音量で
                let (mid, side) = match layer {
                    Some(l) => (sample * l.mid, sample * l.side + vside * l.mid),
                    None => (sample, vside),
                };
                match track_mono.get_mut(v.track as usize) {
                    Some(acc) => {
                        *acc += mid;
                        track_side[v.track as usize] += side;
                    }
                    None => {
                        direct_l += (mid + side) * mix.gain_l;
                        direct_r += (mid - side) * mix.gain_r;
                    }
                }
                i += 1;
            }

            // 音声クリップ(線形補間で読み出し、フェードを掛けてトラックへ合算)
            let mut i = 0;
            while i < self.audio_voices.len() {
                let v = &mut self.audio_voices[i];
                let ev = &data.audio_events[v.idx];
                let frames = &ev.data.frames;
                let i0 = v.pos as usize;
                if self.pos >= ev.end || i0 + 1 >= frames.len() {
                    self.audio_voices.swap_remove(i);
                    continue;
                }
                let frac = (v.pos - i0 as f64) as f32;
                let mut amp = ev.gain;
                let since_start = self.pos - ev.start;
                if ev.fade_in > 0 && since_start < ev.fade_in {
                    amp *= since_start as f32 / ev.fade_in as f32;
                }
                let until_end = ev.end - self.pos;
                if ev.fade_out > 0 && until_end < ev.fade_out {
                    amp *= until_end as f32 / ev.fade_out as f32;
                }
                let sample = glaux_dsp::hermite(frames, i0, frac) * amp;
                // ステレオ素材の左右差成分(L = M + S、R = M − S)
                let side = match &ev.data.side {
                    Some(sd) if i0 + 1 < sd.len() => glaux_dsp::hermite(sd, i0, frac) * amp,
                    _ => 0.0,
                };
                v.pos += ev.rate;
                if let Some(c) = self.voice_samples.get_mut(ev.track as usize) {
                    *c += 1;
                }
                match track_mono.get_mut(ev.track as usize) {
                    Some(acc) => {
                        *acc += sample;
                        track_side[ev.track as usize] += side;
                    }
                    None => {
                        if let Some(mix) = data.tracks.get(ev.track as usize) {
                            direct_l += (sample + side) * mix.gain_l;
                            direct_r += (sample - side) * mix.gain_r;
                        }
                    }
                }
                i += 1;
            }

            // 時刻指定のノート: 時計が来たものを鳴らし始める
            let now = self.clock + frame as u64;
            if !self.timed.is_empty() {
                let mut k = 0;
                while k < self.timed.len() {
                    if self.timed[k].at <= now {
                        let n = self.timed.swap_remove(k);
                        self.start_timed(n, now, data, sr);
                    } else {
                        k += 1;
                    }
                }
            }

            // MIDI キーボードのライブ発音・時刻指定のノート(送り先トラックのエフェクトを通す)
            let mut i = 0;
            while i < self.live_voices.len() {
                let v = &mut self.live_voices[i];
                if !v.released && now >= v.off_at {
                    v.state.note_off();
                    v.released = true;
                }
                if v.released && v.state.finished(&v.instrument) {
                    self.live_voices.swap_remove(i);
                    continue;
                }
                let (mut sample, mut vside) = v.state.next_stereo(&v.instrument);
                if v.shape.is_active() {
                    (sample, vside) = v.shape.process_stereo(sample, vside, 0.0, sr);
                }
                match data.tracks.get(v.track as usize) {
                    Some(mix) => match track_mono.get_mut(v.track as usize) {
                        Some(acc) => {
                            *acc += sample;
                            track_side[v.track as usize] += vside;
                        }
                        None => {
                            direct_l += (sample + vside) * mix.gain_l;
                            direct_r += (sample - vside) * mix.gain_r;
                        }
                    },
                    None => {
                        direct_l += (sample + vside) * 0.8;
                        direct_r += (sample - vside) * 0.8;
                    }
                }
                i += 1;
            }

            // 試聴ボイス(停止中でも鳴る。トラックエフェクトはバイパスしてマスターへ)
            let mut i = 0;
            while i < self.preview_voices.len() {
                let v = &mut self.preview_voices[i];
                if v.remaining == 0 && !v.released {
                    v.state.note_off();
                    v.released = true;
                }
                if v.released && v.state.finished(&v.instrument) {
                    self.preview_voices.swap_remove(i);
                    continue;
                }
                v.remaining = v.remaining.saturating_sub(1);
                let (sample, vside) = v.state.next_stereo(&v.instrument);
                direct_l += (sample + vside) * v.gain_l;
                direct_r += (sample - vside) * v.gain_r;
                i += 1;
            }

            // ブロック用のバッファへ(エフェクトはこの後ブロック単位で通す)
            for (ti, m) in track_mono.iter().enumerate().take(ntracks) {
                self.blk_mono[ti][frame] = *m;
                self.blk_side[ti][frame] = track_side[ti];
            }
            self.blk_direct[0][frame] = direct_l;
            self.blk_direct[1][frame] = direct_r;
            // クリックはマスターエフェクト・マスター音量を通さず直接足す
            self.blk_click[frame] = self.click.next(sr);
            self.blk_pos[frame] = self.pos;
            if playing {
                self.pos += 1;
            }
        }

        // 発音にかかった時間を、トラックごとに鳴らした声の数で按分する
        let voice_ns = t_voices.elapsed().as_nanos() as u64;
        let total: u64 = self.voice_samples.iter().map(|c| *c as u64).sum();
        for (ti, c) in self.voice_samples.iter().enumerate().take(ntracks) {
            if let Some(ns) = (voice_ns * *c as u64)
                .checked_div(total)
                .filter(|ns| *ns > 0)
            {
                self.shared.stats.track_ns[ti].fetch_add(ns, Ordering::Relaxed);
            }
        }

        // 2. トラックごとに エフェクトチェーン → 音量/パン(ブロック単位。CLAP エフェクトもここで通す)
        self.process_track_chains(data, frames, ntracks, sr);
        // 3. マスターのエフェクト → マスター音量 → ソフトクリップ
        let t_master = std::time::Instant::now();
        self.process_master(data, frames, sr, out, channels);
        self.shared
            .stats
            .master_ns
            .fetch_add(t_master.elapsed().as_nanos() as u64, Ordering::Relaxed);
        // 4. 鳴っていないトラックのエフェクトも毎ブロック無音で処理する(処理を呼ばれないと、
        //    画面を開くときに音声処理側の応答を待って固まるプラグインがある: Surge XT Effects)
        self.keep_effects_alive(frames);

        // 曲が終わって余韻も消えたら自動停止(ループ中・録音中は止めない)
        let tail = (TAIL_SECS * data.sample_rate) as u64;
        let recording = self.shared.recording.load(Ordering::Acquire);
        if playing
            && !looping
            && !recording
            && data.end_sample > 0
            && self.pos > data.end_sample + tail
        {
            self.shared.playing.store(false, Ordering::Release);
        }

        self.last_tick = data.sample_to_tick(self.pos);
        self.clock += frames as u64;
        self.publish_pos(frames);
    }

    // ---- CLAP プラグイン ----

    // ---- エフェクトチェーン(ブロック単位) ----

    /// トラックごとに 入力(楽器 + プラグイン出力)→ エフェクトチェーン → 音量/パン を通し、
    /// 出力先(マスター前の合算か、グループのバス)とセンド先のバスへ送る。送る側を受ける側より先に
    /// 処理する順([`PlaybackData::order`])で回し、遅延補正は送り先の輪状バッファの書く位置で揃える。
    /// マスター前の合算は MAX_TRACKS 超のトラックと試聴の直行分から始める
    fn process_track_chains(
        &mut self,
        data: &PlaybackData,
        frames: usize,
        ntracks: usize,
        sr: f32,
    ) {
        let mut mix_l = std::mem::take(&mut self.mix_l);
        let mut mix_r = std::mem::take(&mut self.mix_r);
        let mut fl = std::mem::take(&mut self.fx_l);
        let mut fr = std::mem::take(&mut self.fx_r);
        self.compute_pdc(data, ntracks);
        let pos = self.ring_pos;
        ring_add(
            &mut self.master_ring,
            pos + self.pdc_master_in as usize,
            &self.blk_direct[0][..frames],
            &self.blk_direct[1][..frames],
            1.0,
        );
        let (order, n) = process_order(data, ntracks);
        for &ti in &order[..n] {
            let ti = ti as usize;
            let mix = &data.tracks[ti];
            // 入力: 通常のトラックは楽器 + プラグイン出力、バスは送りと出力で受けた音
            let pslot = if mix.is_bus {
                None
            } else {
                self.track_plugin[ti]
            };
            let any = if mix.is_bus {
                ring_take(
                    &mut self.route_ring[ti],
                    pos,
                    &mut self.bus_l[ti][..frames],
                    &mut self.bus_r[ti][..frames],
                );
                // ミュートしたバス(バスには発音が無いので、ここで止める)
                if !mix.audible {
                    continue;
                }
                fl[..frames].copy_from_slice(&self.bus_l[ti][..frames]);
                fr[..frames].copy_from_slice(&self.bus_r[ti][..frames]);
                fl[..frames].iter().chain(&fr[..frames]).any(|v| *v != 0.0)
            } else {
                let mut any = false;
                for f in 0..frames {
                    let mono = self.blk_mono[ti][f];
                    let side = self.blk_side[ti][f];
                    let (el, er) = match pslot {
                        Some(s) => (self.plugin_out[s][0][f], self.plugin_out[s][1][f]),
                        None => (0.0, 0.0),
                    };
                    fl[f] = mono + side + el;
                    fr[f] = mono - side + er;
                    any |= mono != 0.0 || side != 0.0 || el != 0.0 || er != 0.0;
                }
                any
            };
            let t = std::time::Instant::now();
            self.track_block(data, ti, mix, pslot, any, frames, sr, &mut fl, &mut fr);
            self.shared.stats.track_ns[ti]
                .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        ring_take(
            &mut self.master_ring,
            pos,
            &mut mix_l[..frames],
            &mut mix_r[..frames],
        );
        // 読まれなかった送り先(バスでなくなった添字など)の区間を消しておく(古い音が後で出ないように)
        for ring in self.route_ring.iter_mut() {
            for side in ring.iter_mut() {
                let start = pos % RING;
                let first = frames.min(RING - start);
                side[start..start + first].fill(0.0);
                side[..frames - first].fill(0.0);
            }
        }
        self.ring_pos = (pos + frames) % RING;
        self.mix_l = mix_l;
        self.mix_r = mix_r;
        self.fx_l = fl;
        self.fx_r = fr;
    }

    /// 1 トラック分: チェーン → 音量/パン → 出力先(マスター前の合算かバス)とセンド先のバスへ。
    /// 長く無音(残響も消えた)で省いたら false。
    #[allow(clippy::too_many_arguments)]
    fn track_block(
        &mut self,
        data: &PlaybackData,
        ti: usize,
        mix: &crate::data::TrackMix,
        pslot: Option<usize>,
        any: bool,
        frames: usize,
        sr: f32,
        fl: &mut [f32],
        fr: &mut [f32],
    ) -> bool {
        // 残響テールが確実に消えるまでの猶予(これを超えて無音ならチェーンごと省く)
        // (畳み込みリバーブの IR は最長 8 秒なので、それがあるトラックは長めに待つ)
        let long_tail = mix
            .effects
            .iter()
            .any(|e| matches!(e.params, EffectParams::Convolution(_)));
        let tail_limit = ((if long_tail { 9.0 } else { 4.0 }) * sr) as u32;
        let silent_before = self.track_silence[ti];
        self.track_silence[ti] = if any {
            0
        } else {
            self.track_silence[ti].saturating_add(frames as u32)
        };
        if !any && (mix.effects.is_empty() || silent_before > tail_limit) {
            // 鳴っていない(エフェクトの残響も消えた)トラックは省く(CPU 節約)
            return false;
        }
        match &mix.fx_graph {
            Some(g) => self.run_graph(&mix.effects, g, ti, data, fl, fr, frames),
            None if !mix.effects.is_empty() => self.run_chain(&mix.effects, data, fl, fr, frames),
            None => {}
        }
        // 遅延補正: 受け側の入口の時刻に合わせて、送り先の輪状バッファの先の位置に書く
        let out_t = self.pdc_out[ti];
        let at = |target_in: u32| {
            self.ring_pos + (target_in.saturating_sub(out_t) as usize).min(MAX_PDC - 1)
        };
        // フェーダー前のセンド(チェーンの後、音量・パンの前)
        for snd in mix.sends.iter().filter(|s| s.pre_fader) {
            let t = snd.target as usize;
            let start = at(self.pdc_in[t]);
            ring_add(
                &mut self.route_ring[t],
                start,
                &fl[..frames],
                &fr[..frames],
                snd.amp,
            );
        }
        // ステレオの音(CLAP 音源・バス・ステレオの音声)はパンを左右のバランスとして掛ける
        // (中央 0dB、反対側だけを下げる)。モノラルの音は等パワーのパン。
        // 以前は等パワーのパンを √2 倍していたため、振った側が +3dB 大きくなっていた
        let balance = pslot.is_some() || mix.is_bus || mix.stereo;
        let pan_law = if balance { balance_gains } else { pan_gains };
        let static_gains = if balance {
            let (bl, br) = balance_gains(mix.base_pan);
            (mix.base_amp * bl, mix.base_amp * br)
        } else {
            (mix.gain_l, mix.gain_r)
        };
        let post: &[crate::data::SendMix] = &mix.sends;
        let mut peak = 0.0f32;
        let k = gain_smooth_coef(sr);
        // 外から動かす追加の音量(ゲームの場面で楽器を足し引きする)。なめらかさは音量と同じ追従で
        let live = self
            .shared
            .live_gain
            .get(ti)
            .map_or(1.0, |g| f32::from_bits(g.load(Ordering::Relaxed)));
        let mut gs = self
            .gain_smooth
            .get(ti)
            .copied()
            .unwrap_or((f32::NAN, f32::NAN));
        for f in 0..frames {
            let pos = self.blk_pos[f];
            // ループで位置が戻ったらオートメーションのカーソルを戻す
            if f > 0 && pos < self.blk_pos[f - 1] {
                self.auto_cursors[ti] = (0, 0);
            }
            // 音量・パン: オートメーションレーンがあればフェーダーより優先
            let (gl, gr) = if mix.vol_db_auto.is_empty() && mix.pan_auto.is_empty() {
                static_gains
            } else {
                let (vol_cur, pan_cur) = &mut self.auto_cursors[ti];
                let amp = if mix.vol_db_auto.is_empty() {
                    mix.base_amp
                } else {
                    db_to_amp(eval_auto(&mix.vol_db_auto, vol_cur, pos))
                };
                let pan = if mix.pan_auto.is_empty() {
                    mix.base_pan
                } else {
                    eval_auto(&mix.pan_auto, pan_cur, pos).clamp(-1.0, 1.0)
                };
                let (pl, pr) = pan_law(pan);
                (amp * pl, amp * pr)
            };
            let (gl, gr) = (gl * live, gr * live);
            if gs.0.is_nan() {
                gs = (gl, gr);
            } else {
                gs.0 += (gl - gs.0) * k;
                gs.1 += (gr - gs.1) * k;
            }
            let (gl, gr) = gs;
            let (ol, or) = (fl[f] * gl, fr[f] * gr);
            peak = peak.max(ol.abs()).max(or.abs());
            fl[f] = ol;
            fr[f] = or;
        }
        // 出力先(グループのバスかマスター前の合算)と、フェーダー後のセンド(トラックの音量・パンに追従)
        let out_t = self.pdc_out[ti];
        let at = |target_in: u32| {
            self.ring_pos + (target_in.saturating_sub(out_t) as usize).min(MAX_PDC - 1)
        };
        match mix.output {
            Some(b) => {
                let b = b as usize;
                let start = at(self.pdc_in[b]);
                ring_add(
                    &mut self.route_ring[b],
                    start,
                    &fl[..frames],
                    &fr[..frames],
                    1.0,
                );
            }
            None => {
                let start = at(self.pdc_master_in);
                ring_add(
                    &mut self.master_ring,
                    start,
                    &fl[..frames],
                    &fr[..frames],
                    1.0,
                );
            }
        }
        for snd in post.iter().filter(|s| !s.pre_fader) {
            let t = snd.target as usize;
            let start = at(self.pdc_in[t]);
            ring_add(
                &mut self.route_ring[t],
                start,
                &fl[..frames],
                &fr[..frames],
                snd.amp,
            );
        }
        if ti < MAX_TRACKS {
            self.gain_smooth[ti] = gs;
            Levels::note(&self.shared.levels.tracks[ti], peak);
        }
        true
    }

    /// エフェクトチェーンをブロック単位で通す。内蔵エフェクトはサンプルごと、CLAP エフェクトは
    /// 入力口に書いてブロックごと処理する(まだ届いていないプラグインは素通し)。
    fn run_chain(
        &mut self,
        chain: &[crate::data::BakedEffect],
        data: &PlaybackData,
        fl: &mut [f32],
        fr: &mut [f32],
        frames: usize,
    ) {
        for fx in chain {
            self.run_effect(fx, data, fl, fr, frames);
        }
    }

    /// つながり(分岐・合流・線の音量)の順にエフェクトを通す。各エフェクトの入口で、入ってくる線の音を
    /// 足し合わせてから処理し、最後に出口へ入る線の音を足して `fl` / `fr` に返す。確保はしない。
    /// 遅れの違う枝が合流するところは、早い枝を輪状バッファで遅らせて揃える(`owner` はトラックの添字、
    /// マスターは MAX_TRACKS)
    #[allow(clippy::too_many_arguments)]
    fn run_graph(
        &mut self,
        chain: &[crate::data::BakedEffect],
        plan: &crate::data::FxGraphPlan,
        owner: usize,
        data: &PlaybackData,
        fl: &mut [f32],
        fr: &mut [f32],
        frames: usize,
    ) {
        let mut bufs = std::mem::take(&mut self.graph_bufs);
        let n = chain.len().min(bufs.len() - 1);
        let exit = crate::data::MAX_GRAPH_NODES + 1;
        let times = self.graph_time[owner];
        let rings = self.graph_ring[owner];
        let pos = self.ring_pos;
        bufs[0][0][..frames].copy_from_slice(&fl[..frames]);
        bufs[0][1][..frames].copy_from_slice(&fr[..frames]);
        for (i, fx) in chain.iter().enumerate().take(n) {
            let (done, rest) = bufs.split_at_mut(i + 1);
            let [ol, or] = &mut rest[0];
            let inputs = plan.inputs.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
            let ring = rings[i + 1];
            gather(
                inputs,
                done,
                &mut ol[..frames],
                &mut or[..frames],
                (ring != NO_RING).then(|| (&mut self.branch_rings[ring as usize], pos)),
                &times,
                in_time_of(&times, inputs, i),
            );
            self.run_effect(fx, data, &mut ol[..frames], &mut or[..frames], frames);
        }
        let ring = rings[exit];
        let outs: &[(u16, f32)] = &plan.output;
        let in_time = in_time_of(&times, outs, n);
        let valid = &bufs[..=n];
        gather(
            outs,
            valid,
            &mut fl[..frames],
            &mut fr[..frames],
            (ring != NO_RING).then(|| (&mut self.branch_rings[ring as usize], pos)),
            &times,
            in_time,
        );
        self.graph_bufs = bufs;
    }

    /// エフェクト 1 つを通す(内蔵はサンプルごと、CLAP はブロックごと)
    fn run_effect(
        &mut self,
        fx: &crate::data::BakedEffect,
        data: &PlaybackData,
        fl: &mut [f32],
        fr: &mut [f32],
        frames: usize,
    ) {
        {
            if let Some((ps, gen)) = fx.plugin {
                let ps = ps as usize;
                let notes = &self.plugin_notes[ps];
                let Some(p) = self
                    .plugins
                    .get_mut(ps)
                    .and_then(|p| p.as_mut())
                    .filter(|p| p.gen == gen)
                else {
                    return;
                };
                if let Some((il, ir)) = p.clap.input_mut() {
                    match ir {
                        Some(ir) => {
                            il[..frames].copy_from_slice(&fl[..frames]);
                            ir[..frames].copy_from_slice(&fr[..frames]);
                        }
                        None => {
                            for f in 0..frames {
                                il[f] = (fl[f] + fr[f]) * 0.5;
                            }
                        }
                    }
                }
                p.clap.process(frames, notes);
                self.slot_done[ps] = true;
                if let Some((ol, or)) = p.clap.output() {
                    fl[..frames].copy_from_slice(&ol[..frames]);
                    fr[..frames].copy_from_slice(&or[..frames]);
                }
                return;
            }
            let slot = fx.slot as usize;
            let params = self.fx_scratch[slot].unwrap_or(fx.params);
            // 畳み込みリバーブは再生データの本体でブロックごとに通す
            if let EffectParams::Convolution(c) = &params {
                if let Some(eng) = data.conv.get(c.index as usize) {
                    eng.process_block(&mut fl[..frames], &mut fr[..frames], c.mix, c.wet);
                }
                return;
            }
            // サイドチェイン・ダイナミック EQ の検出信号: ソーストラックの生ミックス(エフェクト前)。
            // CLAP 音源のトラックは音源の出力も足し、バスはそのバスが受けた音(送る側より後に処理するので
            // 同じブロックの音。輪になって先に処理するときは 1 ブロック前の音)
            let key_track = params
                .key_source()
                .map(|t| t as usize)
                .filter(|t| *t < data.tracks.len().min(MAX_TRACKS));
            let key_bus = key_track.filter(|t| data.tracks[*t].is_bus);
            let key_plugin = key_track
                .filter(|t| !data.tracks[*t].is_bus)
                .and_then(|t| self.track_plugin[t]);
            let state = &mut self.effect_states[slot];
            state.set_clock(self.blk_tick, self.blk_tps);
            for f in 0..frames {
                let key = match (key_track, key_bus) {
                    (_, Some(b)) => (self.bus_l[b][f] + self.bus_r[b][f]) * 0.5,
                    (Some(t), None) => {
                        self.blk_mono[t][f]
                            + key_plugin.map_or(0.0, |s| {
                                (self.plugin_out[s][0][f] + self.plugin_out[s][1][f]) * 0.5
                            })
                    }
                    (None, None) => 0.0,
                };
                (fl[f], fr[f]) = state.process(&params, fl[f], fr[f], key);
            }
        }
    }

    /// 遅延補正の量を決める(CLAP の申告と、内蔵エフェクトの遅れ)。処理の順に、各トラックの出口の時刻
    /// (入口の時刻 + 音源とエフェクトの遅れ)と、各バス・マスターの入口の時刻(流れ込む音の出口の最大)を求める。
    /// エフェクトの分岐は、遅れの違う枝が合流するノードに輪状バッファを割り当てて揃える
    fn compute_pdc(&mut self, data: &PlaybackData, ntracks: usize) {
        let lat_of = |slot: usize, gen: Option<u64>, plugins: &[Option<Box<Processor>>]| -> u32 {
            plugins
                .get(slot)
                .and_then(|p| p.as_ref())
                .filter(|p| gen.is_none_or(|g| p.gen == g))
                .map(|p| p.clap.latency())
                .unwrap_or(0)
        };
        let fx_lat = |fx: &crate::data::BakedEffect, plugins: &[Option<Box<Processor>>]| {
            fx.plugin
                .map(|(s, g)| lat_of(s as usize, Some(g), plugins))
                .unwrap_or_else(|| fx.params.latency())
        };
        const NODES: usize = crate::data::MAX_GRAPH_NODES;
        let mut next_ring = 0usize;
        // つながり 1 つ分: 各ノードの出口の時刻を求め、揃えが要る入口に輪状バッファを割り当てる。出口までの遅れを返す
        let mut plan_graph = |owner: usize,
                              effects: &[crate::data::BakedEffect],
                              graph: Option<&crate::data::FxGraphPlan>,
                              this: &mut Self|
         -> u32 {
            let mut times = [0u32; NODES + 2];
            let mut rings = [NO_RING; NODES + 2];
            let total = match graph {
                Some(g) => {
                    let n = effects.len().min(NODES);
                    let mut assign =
                        |times: &[u32], inputs: &[(u16, f32)], limit: usize| -> (u32, u8) {
                            let t = in_time_of(times, inputs, limit);
                            let uneven = inputs
                                .iter()
                                .filter(|(src, _)| (*src as usize) <= limit)
                                .any(|(src, _)| times[*src as usize] != t);
                            let ring = if uneven && next_ring < BRANCH_RINGS {
                                next_ring += 1;
                                (next_ring - 1) as u8
                            } else {
                                NO_RING
                            };
                            (t, ring)
                        };
                    for (i, fx) in effects.iter().enumerate().take(n) {
                        let inputs = g.inputs.get(i).map(|v| v.as_slice()).unwrap_or(&[]);
                        let (t, ring) = assign(&times, inputs, i);
                        rings[i + 1] = ring;
                        times[i + 1] = t + fx_lat(fx, &this.plugins);
                    }
                    let (t, ring) = assign(&times, &g.output, n);
                    rings[NODES + 1] = ring;
                    t
                }
                None => effects.iter().map(|fx| fx_lat(fx, &this.plugins)).sum(),
            };
            times[NODES + 1] = total;
            // 割り当てが変わった輪状バッファは、前の持ち主が先に書いた音を消してから使う
            for (k, r) in rings.iter().enumerate() {
                if *r != NO_RING && this.graph_ring[owner][k] != *r {
                    for side in this.branch_rings[*r as usize].iter_mut() {
                        side.fill(0.0);
                    }
                }
            }
            this.graph_time[owner] = times;
            this.graph_ring[owner] = rings;
            total
        };
        let mut lat = [0u32; MAX_TRACKS];
        for (ti, mix) in data.tracks.iter().take(ntracks).enumerate() {
            let inst = self.track_plugin[ti]
                .filter(|_| !mix.is_bus)
                .map(|s| lat_of(s, None, &self.plugins))
                .unwrap_or(0);
            lat[ti] = inst + plan_graph(ti, &mix.effects, mix.fx_graph.as_ref(), self);
        }
        plan_graph(
            MAX_TRACKS,
            &data.master_effects,
            data.master_fx_graph.as_ref(),
            self,
        );
        let mut inn = [0u32; MAX_TRACKS];
        let mut master_in = 0u32;
        let (order, n) = process_order(data, ntracks);
        for &ti in &order[..n] {
            let ti = ti as usize;
            let mix = &data.tracks[ti];
            let out = inn[ti] + lat[ti];
            self.pdc_out[ti] = out;
            for b in mix.output.iter().chain(mix.sends.iter().map(|s| &s.target)) {
                if let Some(v) = inn.get_mut(*b as usize) {
                    *v = (*v).max(out);
                }
            }
            if mix.output.is_none() {
                master_in = master_in.max(out);
            }
        }
        self.pdc_in = inn;
        self.pdc_master_in = master_in.min(MAX_PDC as u32 - 1);
        for &ti in &order[..n] {
            let ti = ti as usize;
            let target = match data.tracks[ti].output {
                Some(b) => inn[b as usize],
                None => master_in,
            };
            self.pdc_delay[ti] = target
                .saturating_sub(self.pdc_out[ti])
                .min(MAX_PDC as u32 - 1);
        }
    }

    /// テスト用: このブロックでプラグインのスロットへ積んだメッセージ。
    #[doc(hidden)]
    pub fn plugin_msgs_for_test(&self, slot: usize) -> &[NoteMsg] {
        self.plugin_notes.get(slot).map_or(&[], |v| v.as_slice())
    }

    /// テスト用: トラックの遅延補正の量(サンプル)。
    #[doc(hidden)]
    pub fn pdc_delay(&self, track: usize) -> u32 {
        self.pdc_delay.get(track).copied().unwrap_or(0)
    }

    /// テスト用: スロットのプラグインの遅延の申告を差し替える。
    #[doc(hidden)]
    pub fn set_plugin_latency_for_test(&mut self, slot: usize, samples: u32) {
        if let Some(Some(p)) = self.plugins.get_mut(slot) {
            p.clap.set_latency_for_test(samples);
        }
    }

    /// このブロックで処理しなかった CLAP エフェクトに無音を通す(出力は捨てる)。
    fn keep_effects_alive(&mut self, frames: usize) {
        for slot in 0..MAX_PLUGINS {
            if !self.slot_is_fx[slot] || self.slot_done[slot] {
                continue;
            }
            let notes = &self.plugin_notes[slot];
            if let Some(p) = self.plugins[slot].as_mut() {
                p.clap.clear_input(frames);
                p.clap.process(frames, notes);
            }
        }
    }

    /// テスト用: スロットのプラグインがこれまでに処理したフレーム数。
    #[doc(hidden)]
    pub fn plugin_frames_processed(&self, slot: usize) -> Option<u64> {
        self.plugins
            .get(slot)?
            .as_ref()
            .map(|p| p.clap.frames_processed())
    }

    /// マスターのエフェクト → マスター音量 → ソフトクリップ → 出力。
    fn process_master(
        &mut self,
        data: &PlaybackData,
        frames: usize,
        sr: f32,
        out: &mut [f32],
        channels: usize,
    ) {
        let clip = !self.shared.no_master_clip.load(Ordering::Relaxed);
        let mut l = std::mem::take(&mut self.mix_l);
        let mut r = std::mem::take(&mut self.mix_r);
        match &data.master_fx_graph {
            Some(g) => self.run_graph(
                &data.master_effects,
                g,
                MAX_TRACKS,
                data,
                &mut l,
                &mut r,
                frames,
            ),
            None if !data.master_effects.is_empty() => {
                self.run_chain(&data.master_effects, data, &mut l, &mut r, frames)
            }
            None => {}
        }
        let mut peak = 0.0f32;
        let k = gain_smooth_coef(sr);
        let (mon_mode, xfeed) = self.shared.monitor.get();
        let speaker = self.shared.monitor.speaker();
        let monitoring = mon_mode != crate::monitor::MonitorMode::Stereo
            || xfeed
            || speaker != crate::monitor::Speaker::Off;
        let out_target = f32::from_bits(self.shared.output_gain.load(Ordering::Relaxed));
        // A/B の聴き比べ(再生中だけ。止めたらふつうの再生に戻す)。切り替えは約 10ms のクロスフェード
        let ab_guard = self.shared.ab.load();
        let ab_clip = ab_guard.as_deref();
        let ab_side = crate::ab::AbSide::from_code(self.shared.ab_side.load(Ordering::Relaxed));
        let ab_target_on = if ab_clip.is_some()
            && ab_side != crate::ab::AbSide::Off
            && self.shared.playing.load(Ordering::Relaxed)
        {
            1.0
        } else {
            0.0
        };
        let ab_target_b = if ab_side == crate::ab::AbSide::A {
            0.0
        } else {
            1.0
        };
        let ab_k = 1.0 - (-1.0 / (0.01 * sr)).exp();
        for f in 0..frames {
            let pos = self.blk_pos[f];
            if f > 0 && pos < self.blk_pos[f - 1] {
                self.master_cursor = 0;
            }
            let master_amp = if data.master_vol_auto.is_empty() {
                data.master_amp
            } else {
                db_to_amp(eval_auto(
                    &data.master_vol_auto,
                    &mut self.master_cursor,
                    pos,
                ))
            };
            self.master_smooth = if self.master_smooth.is_nan() {
                master_amp
            } else {
                self.master_smooth + (master_amp - self.master_smooth) * k
            };
            let master_amp = self.master_smooth;
            let click = self.blk_click[f];
            let base = f * channels;
            let (mut ol, mut or) = (l[f] * master_amp, r[f] * master_amp);
            // A/B の聴き比べ: 書き出した音(マスター込み)を今の位置で鳴らす。メーターも聴いている音で測る
            let mut ab_full = false;
            if ab_target_on > 0.0 || self.ab_on > 1e-5 {
                self.ab_on += (ab_target_on - self.ab_on) * ab_k;
                self.ab_mix += (ab_target_b - self.ab_mix) * ab_k;
                let (al, ar) = match ab_clip.and_then(|c| c.frame(pos)) {
                    Some(((al, ar), (bl, br))) => {
                        (al + (bl - al) * self.ab_mix, ar + (br - ar) * self.ab_mix)
                    }
                    None => (0.0, 0.0),
                };
                ol += (al - ol) * self.ab_on;
                or += (ar - or) * self.ab_on;
                ab_full = self.ab_on > 0.999;
            }
            // メーター・相関はミックスそのもの(聴き方の切り替えの前)で測る
            self.monitor.measure(&self.shared.monitor, ol, or, sr);
            peak = peak.max((ol + click).abs()).max((or + click).abs());
            let (ol, or) = if monitoring {
                self.monitor.apply(mon_mode, xfeed, speaker, ol, or, sr)
            } else {
                (ol, or)
            };
            let (ol, or) = (ol + click, or + click);
            // 聴き比べの音は書き出しでクリップ防止を通してあるので、二重に掛けない
            let (ol, or) = if clip && !ab_full {
                (soft_clip(ol), soft_clip(or))
            } else {
                (ol, or)
            };
            // アプリの音量(聴く音量だけ。メーター・書き出しには入らない)
            self.out_smooth = if self.out_smooth.is_nan() {
                out_target
            } else {
                self.out_smooth + (out_target - self.out_smooth) * k
            };
            let g = self.out_smooth;
            out[base] = ol * g;
            if channels >= 2 {
                out[base + 1] = or * g;
            }
        }
        Levels::note(&self.shared.levels.master, peak);
        self.monitor.publish(&self.shared.monitor);
        self.mix_l = l;
        self.mix_r = r;
    }

    /// 窓口の受け取り(新しい世代)と返却(外すよう頼まれた・置き換えられた世代)。
    fn exchange_plugins(&mut self) {
        let slots = self.shared.plugin_slots.clone();
        for (i, slot) in slots.iter().enumerate() {
            let remove = slot.remove_requested();
            if self.plugins[i].as_ref().is_some_and(|p| p.gen == remove) && slot.outgoing_free() {
                if let Some(mut p) = self.plugins[i].take() {
                    p.clap.stop();
                    if let Err(back) = slot.put_outgoing(p) {
                        self.plugins[i] = Some(back); // 返却口が空いたら次のブロックで
                    }
                }
            }
            // 置き換え: 今の窓口を返せるときだけ新しい窓口を受け取る
            if self.plugins[i].is_none() || slot.outgoing_free() {
                if let Some(new) = slot.take_incoming() {
                    if let Some(mut old) = self.plugins[i].take() {
                        old.clap.stop();
                        let _ = slot.put_outgoing(old);
                    }
                    self.plugins[i] = Some(new);
                }
            }
        }
    }

    /// 各トラックのプラグインが使える状態か(窓口が届いていて世代が合う)を調べる。
    fn refresh_track_plugins(&mut self, data: &PlaybackData) {
        self.track_plugin = [None; MAX_TRACKS];
        for (ti, mix) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
            if let Some((slot, gen)) = mix.plugin {
                let slot = slot as usize;
                if self
                    .plugins
                    .get(slot)
                    .and_then(|p| p.as_ref())
                    .is_some_and(|p| p.gen == gen)
                {
                    self.track_plugin[ti] = Some(slot);
                }
            }
        }
    }

    fn plugin_note_on(
        &mut self,
        slot: usize,
        key: u8,
        velocity: f32,
        time: u32,
        note_id: Option<u32>,
    ) {
        let notes = &mut self.plugin_notes[slot];
        if notes.len() < MAX_EVENTS {
            notes.push(NoteMsg::On {
                time,
                key,
                velocity: velocity.clamp(0.0, 1.0),
                note_id,
            });
        }
    }

    /// MIDI キーボードの送り先がプラグインのトラックなら、そのスロット。
    fn live_plugin_slot(&self) -> Option<usize> {
        let t = self.shared.live_track.load(Ordering::Acquire) as usize;
        self.track_plugin.get(t).copied().flatten()
    }

    /// CLAP パラメータのオートメーション: 一定間隔で値を評価し、変わったときだけ送る。
    /// 変調(LFO)は、プラグインが受けるなら非破壊の変調(ParamMod)でずれだけを、受けないなら値として送る
    fn collect_plugin_automation(&mut self, data: &PlaybackData, frames: usize, playing: bool) {
        for m in self.mod_sent.iter_mut() {
            m.seen = false;
        }
        let sr = data.sample_rate as f32;
        for (ti, mix) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
            let Some(slot) = self.track_plugin[ti] else {
                if !mix.plugin_mods.is_empty() {
                    self.collect_plugin_mods(data, ti, frames, playing, sr);
                }
                continue;
            };
            if mix.plugin_auto.is_empty() && mix.plugin_mods.is_empty() {
                continue;
            }
            let mut f = 0;
            while f < frames {
                let pos = if playing {
                    self.pos + f as u64
                } else {
                    self.pos
                };
                for (li, (id, points)) in mix.plugin_auto.iter().take(MAX_PLUGIN_LANES).enumerate()
                {
                    // 変調を受けないプラグインで、変調の付いたつまみは変調の側が値を送る
                    if self.mod_as_value(mix, None, slot, *id) {
                        continue;
                    }
                    let mut cursor = points
                        .partition_point(|p| p.sample <= pos)
                        .saturating_sub(1);
                    let v = eval_auto(points, &mut cursor, pos);
                    let last = self.plugin_auto_last[ti][li];
                    if last.is_nan() || (v - last).abs() > 1e-6 * (1.0 + v.abs()) {
                        self.plugin_auto_last[ti][li] = v;
                        let notes = &mut self.plugin_notes[slot];
                        if notes.len() < MAX_EVENTS {
                            notes.push(NoteMsg::Param {
                                time: f as u32,
                                id: *id,
                                value: v as f64,
                            });
                        }
                    }
                }
                if !playing {
                    break;
                }
                f += PLUGIN_CTRL_STEP;
            }
            if !mix.plugin_mods.is_empty() {
                self.collect_plugin_mods(data, ti, frames, playing, sr);
            }
        }
        // 外れた変調: 変調は 0 に、値として送っていたものは元の値に戻す
        let mut i = 0;
        while i < self.mod_sent.len() {
            let m = self.mod_sent[i];
            if m.seen {
                i += 1;
                continue;
            }
            if let Some(notes) = self.plugin_notes.get_mut(m.slot as usize) {
                if notes.len() < MAX_EVENTS {
                    notes.push(match m.restore {
                        Some(v) => NoteMsg::Param {
                            time: 0,
                            id: m.id,
                            value: v as f64,
                        },
                        None => NoteMsg::ParamMod {
                            time: 0,
                            id: m.id,
                            amount: 0.0,
                            note: None,
                        },
                    });
                }
            }
            self.mod_sent.swap_remove(i);
        }
    }

    /// そのつまみの変調を、変調ではなく値として送るか(プラグインが変調を受けない)
    fn mod_as_value(
        &self,
        mix: &crate::data::TrackMix,
        fx_slot: Option<u32>,
        ps: usize,
        id: u32,
    ) -> bool {
        mix.plugin_mods
            .iter()
            .any(|m| m.fx_slot == fx_slot && m.id == id)
            && self
                .plugins
                .get(ps)
                .and_then(|p| p.as_ref())
                .is_some_and(|p| !p.clap.can_modulate(id))
    }

    /// 1 トラック分の CLAP のつまみの変調を送る(変わったときだけ)
    fn collect_plugin_mods(
        &mut self,
        data: &PlaybackData,
        ti: usize,
        frames: usize,
        playing: bool,
        sr: f32,
    ) {
        let mix = &data.tracks[ti];
        for m in &mix.plugin_mods {
            let ps = match m.fx_slot {
                None => self.track_plugin[ti],
                Some(s) => mix
                    .effects
                    .iter()
                    .find(|f| f.slot == s)
                    .and_then(|f| f.plugin)
                    .map(|(p, _)| p as usize),
            };
            let Some(ps) = ps else {
                continue;
            };
            let Some((can_mod, can_note)) =
                self.plugins.get(ps).and_then(|p| p.as_ref()).map(|p| {
                    (
                        p.clap.can_modulate(m.id),
                        p.clap.can_modulate_per_note(m.id),
                    )
                })
            else {
                continue;
            };
            // 表に載せる(満杯なら送らない)
            let k = match self
                .mod_sent
                .iter()
                .position(|x| x.slot as usize == ps && x.id == m.id)
            {
                Some(k) => k,
                None if self.mod_sent.len() < MAX_MOD_SENT => {
                    self.mod_sent.push(ModSent {
                        slot: ps as u16,
                        id: m.id,
                        last: f32::NAN,
                        restore: None,
                        seen: false,
                    });
                    self.mod_sent.len() - 1
                }
                None => continue,
            };
            self.mod_sent[k].seen = true;
            self.mod_sent[k].restore = (!can_mod).then_some(m.fixed);
            let mut f = 0;
            while f < frames {
                let pos = if playing {
                    self.pos + f as u64
                } else {
                    self.pos
                };
                let mut cursor = m
                    .offsets
                    .partition_point(|p| p.sample <= pos)
                    .saturating_sub(1);
                let mut off = if m.offsets.is_empty() {
                    0.0
                } else {
                    eval_auto(&m.offsets, &mut cursor, pos)
                };
                // 音ごとの変調を受けないつまみは、曲の頭からの時間でトラック全体を揺らす
                if !(can_mod && can_note) {
                    let secs = pos as f32 / sr;
                    off += m.per_note.iter().map(|l| l.at(secs)).sum::<f32>();
                }
                let v = if can_mod {
                    off
                } else {
                    let base = if m.base.is_empty() {
                        m.fixed
                    } else {
                        let mut c = m
                            .base
                            .partition_point(|p| p.sample <= pos)
                            .saturating_sub(1);
                        eval_auto(&m.base, &mut c, pos)
                    };
                    (base + off).clamp(m.lo, m.hi)
                };
                let last = self.mod_sent[k].last;
                if last.is_nan() || (v - last).abs() > 1e-6 * (1.0 + v.abs()) {
                    self.mod_sent[k].last = v;
                    let notes = &mut self.plugin_notes[ps];
                    if notes.len() < MAX_EVENTS {
                        notes.push(if can_mod {
                            NoteMsg::ParamMod {
                                time: f as u32,
                                id: m.id,
                                amount: v as f64,
                                note: None,
                            }
                        } else {
                            NoteMsg::Param {
                                time: f as u32,
                                id: m.id,
                                value: v as f64,
                            }
                        });
                    }
                }
                if !playing {
                    break;
                }
                f += PLUGIN_CTRL_STEP;
            }
        }
    }

    fn plugin_note_off(&mut self, slot: usize, key: u8, time: u32) {
        let notes = &mut self.plugin_notes[slot];
        if notes.len() < MAX_EVENTS {
            notes.push(NoteMsg::Off { time, key });
        }
    }

    fn push_pending(&mut self, p: PendingOff) {
        if self.plugin_pending.len() < MAX_PENDING_OFFS {
            self.plugin_pending.push(p);
        }
    }

    /// プラグインの音を離す(`seq_only` なら曲のノートだけ、そうでなければライブ演奏も)。
    fn plugins_all_off(&mut self, seq_only: bool) {
        self.plugin_choke_at = [u64::MAX; MAX_PLUGINS];
        self.plugin_released.clear();
        let mut i = 0;
        while i < self.plugin_pending.len() {
            let p = self.plugin_pending[i];
            if !seq_only || p.seq {
                self.plugin_note_off(p.slot as usize, p.key, 0);
                self.plugin_pending.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// 鍵盤を離した(`key` が None なら全部)。
    fn release_live_plugin_notes(&mut self, key: Option<u8>) {
        let mut i = 0;
        while i < self.plugin_pending.len() {
            let p = self.plugin_pending[i];
            if p.end == u64::MAX && key.is_none_or(|k| k == p.key) {
                self.plugin_note_off(p.slot as usize, p.key, 0);
                self.plugin_pending.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// このブロックでプラグインのトラックに送るノートを集める(本処理と同じ規則で
    /// 位置・ループ折り返しをなぞる)。試聴の note off もここで出す。
    fn collect_plugin_notes(
        &mut self,
        data: &PlaybackData,
        frames: usize,
        playing: bool,
        looping: bool,
        loop_start: u64,
        loop_end: u64,
    ) {
        let sr = data.sample_rate as f32;
        // 試聴(時計基準)の note off
        let block_end = self.clock + frames as u64;
        let mut i = 0;
        while i < self.plugin_pending.len() {
            let p = self.plugin_pending[i];
            if !p.seq && p.end != u64::MAX && p.end < block_end {
                let t = p.end.saturating_sub(self.clock) as u32;
                self.plugin_note_off(p.slot as usize, p.key, t);
                self.plugin_pending.swap_remove(i);
            } else {
                i += 1;
            }
        }
        if !playing || self.track_plugin.iter().all(Option::is_none) {
            return;
        }
        let mut pos = self.pos;
        let mut cursor = self.next_event;
        let mut curve_started = false;
        // 次に離す音・次に余韻を切るスロットの位置(これより手前のフレームでは保留の一覧を見ない)。
        // どちらも「これより前には来ない」下限で、見たときに求め直す
        let min_off = |pending: &[PendingOff]| {
            pending
                .iter()
                .filter(|p| p.seq)
                .map(|p| p.end)
                .min()
                .unwrap_or(u64::MAX)
        };
        let mut next_off = min_off(&self.plugin_pending);
        let mut next_choke = self
            .plugin_choke_at
            .iter()
            .copied()
            .min()
            .unwrap_or(u64::MAX);
        for f in 0..frames {
            let t = f as u32;
            if looping && pos >= loop_end {
                pos = loop_start;
                cursor = data.events.partition_point(|e| e.start < pos);
                self.plugin_choke_at = [u64::MAX; MAX_PLUGINS];
                next_choke = u64::MAX;
                self.plugin_released.clear();
                let mut i = 0;
                while i < self.plugin_pending.len() {
                    let p = self.plugin_pending[i];
                    if p.seq {
                        self.plugin_note_off(p.slot as usize, p.key, t);
                        self.plugin_pending.swap_remove(i);
                    } else {
                        i += 1;
                    }
                }
                next_off = u64::MAX;
            }
            // 先に離してから鳴らす(同じ音の連打で新しい音を消さないように)
            if pos >= next_off {
                let mut i = 0;
                while i < self.plugin_pending.len() {
                    let p = self.plugin_pending[i];
                    if p.seq && p.end <= pos {
                        self.plugin_note_off(p.slot as usize, p.key, t);
                        self.plugin_pending.swap_remove(i);
                        // 余韻を後で切れるよう覚えておく(一杯なら古いものから捨てる)
                        if self.plugin_released.len() >= MAX_PENDING_OFFS {
                            self.plugin_released.remove(0);
                        }
                        self.plugin_released.push((p.slot, p.key));
                    } else {
                        i += 1;
                    }
                }
                next_off = min_off(&self.plugin_pending);
            }
            // レガート・ポルタメントの余韻切り: 予定の位置に来たスロットで、離した鍵盤(押さえ直していないもの)を止める
            for slot in 0..MAX_PLUGINS {
                if next_choke > pos {
                    break;
                }
                if self.plugin_choke_at[slot] > pos {
                    continue;
                }
                self.plugin_choke_at[slot] = u64::MAX;
                let mut i = 0;
                while i < self.plugin_released.len() {
                    let (s, key) = self.plugin_released[i];
                    if s as usize != slot {
                        i += 1;
                        continue;
                    }
                    let held = self
                        .plugin_pending
                        .iter()
                        .any(|p| p.slot == s && p.key == key);
                    if !held {
                        let notes = &mut self.plugin_notes[slot];
                        if notes.len() < MAX_EVENTS {
                            notes.push(NoteMsg::Choke { time: t, key });
                        }
                    }
                    self.plugin_released.swap_remove(i);
                }
            }
            if next_choke <= pos {
                next_choke = self
                    .plugin_choke_at
                    .iter()
                    .copied()
                    .min()
                    .unwrap_or(u64::MAX);
            }
            while cursor < data.events.len() && data.events[cursor].start <= pos {
                let e = data.events[cursor];
                cursor += 1;
                let ti = e.track as usize;
                let Some(slot) = self.track_plugin.get(ti).copied().flatten() else {
                    continue;
                };
                if !data.tracks[ti].audible {
                    continue;
                }
                // レガート・ポルタメント: つなぎ目の後で、先に離した音の余韻を切る
                if e.choke > 0 {
                    self.plugin_choke_at[slot] = pos + e.choke as u64;
                    next_choke = next_choke.min(pos + e.choke as u64);
                }
                // 奏法をプラグインで近づける: アクセントは強く、パームミュートは短く弱く
                // (ビブラート・ベンドは下で音程の変化として送る。スタッカートは長さに反映済み)
                let (amp, end) = match e.articulation {
                    glaux_core::Articulation::Accent => ((e.amp * 1.25).min(1.0), e.end),
                    glaux_core::Articulation::PalmMute => {
                        (e.amp * 0.85, e.start + (e.end - e.start).div_ceil(2))
                    }
                    // レガートでつながれた音は次の音と重ねて離す
                    _ => (e.amp, e.end + e.fade_out as u64),
                };
                let x = data.expr(&e);
                let per_note_mod = data.tracks[ti]
                    .plugin_mods
                    .iter()
                    .any(|m| m.fx_slot.is_none() && !m.per_note.is_empty());
                if x.curve.is_empty()
                    && !x.vibrato.is_active()
                    && !x.shape.is_active()
                    && !glaux_dsp::articulation_moves_pitch(e.articulation)
                    && !per_note_mod
                {
                    self.plugin_note_on(slot, e.pitch, amp, t, None);
                    self.push_pending(PendingOff::simple(slot, e.pitch, end.max(pos + 1), true));
                    next_off = next_off.min(end.max(pos + 1));
                } else {
                    // ピッチカーブ・ビブラート・ベンド: ノート ID を付けて鳴らし、音程の変化を後から送る
                    curve_started = true;
                    let note_id = self.next_note_id;
                    self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
                    self.plugin_note_on(slot, e.pitch, amp, t, Some(note_id));
                    self.push_pending(PendingOff {
                        slot: slot as u8,
                        key: e.pitch,
                        end: end.max(pos + 1),
                        seq: true,
                        ev: (cursor - 1) as u32,
                        note_id,
                        last_semi: f32::NAN,
                        last_gain: f32::NAN,
                        last_bright: f32::NAN,
                        ch: 0,
                    });
                    next_off = next_off.min(end.max(pos + 1));
                }
            }
            // ピッチカーブの音程を一定間隔で送る(変わったときだけ。鳴らした瞬間にも送る)
            if f % PLUGIN_CTRL_STEP == 0 || curve_started {
                curve_started = false;
                for i in 0..self.plugin_pending.len() {
                    let p = self.plugin_pending[i];
                    if !p.seq || p.ev == u32::MAX {
                        continue;
                    }
                    let Some(e) = data.events.get(p.ev as usize) else {
                        continue;
                    };
                    let age = pos.saturating_sub(e.start) as f32;
                    let x = data.expr(e);
                    // ノートのビブラートがあれば奏法のビブラートの代わりに(内蔵音源と同じ)
                    let art = if x.vibrato.is_active()
                        && e.articulation == glaux_core::Articulation::Vibrato
                    {
                        0.0
                    } else {
                        glaux_dsp::articulation_cents(e.articulation, age, sr)
                    };
                    let semi = (x.curve.cents_at(age) + art + x.vibrato.cents_at(age, sr)) / 100.0;
                    // 音量・明るさの曲線(変わったときだけ)
                    if x.shape.is_active() {
                        let gain = x.shape.gain_at(age).min(4.0);
                        let bright = 0.5 + 0.5 * x.shape.brightness_at(age);
                        let notes = &mut self.plugin_notes[p.slot as usize];
                        if !x.shape.volume.is_empty()
                            && (p.last_gain.is_nan() || (gain - p.last_gain).abs() > 0.002)
                            && notes.len() < MAX_EVENTS
                        {
                            self.plugin_pending[i].last_gain = gain;
                            notes.push(NoteMsg::Volume {
                                time: t,
                                key: p.key,
                                note_id: p.note_id,
                                gain: gain as f64,
                            });
                        }
                        if !x.shape.bright.is_empty()
                            && (p.last_bright.is_nan() || (bright - p.last_bright).abs() > 0.002)
                            && notes.len() < MAX_EVENTS
                        {
                            self.plugin_pending[i].last_bright = bright;
                            notes.push(NoteMsg::Brightness {
                                time: t,
                                key: p.key,
                                note_id: p.note_id,
                                value: bright as f64,
                            });
                        }
                    }
                    // 音ごとの変調(プラグインが 1 音ごとに受けるつまみだけ)
                    if let Some(mix) = data.tracks.get(e.track as usize) {
                        let can = |id: u32| {
                            self.plugins
                                .get(p.slot as usize)
                                .and_then(|x| x.as_ref())
                                .is_some_and(|x| {
                                    x.clap.can_modulate(id) && x.clap.can_modulate_per_note(id)
                                })
                        };
                        for m in &mix.plugin_mods {
                            if m.fx_slot.is_some() || m.per_note.is_empty() || !can(m.id) {
                                continue;
                            }
                            let secs = age / sr;
                            let amount: f32 = m.per_note.iter().map(|l| l.at(secs)).sum();
                            let notes = &mut self.plugin_notes[p.slot as usize];
                            if notes.len() < MAX_EVENTS {
                                notes.push(NoteMsg::ParamMod {
                                    time: t,
                                    id: m.id,
                                    amount: amount as f64,
                                    note: Some((p.key, p.note_id)),
                                });
                            }
                        }
                    }
                    if p.last_semi.is_nan() || (semi - p.last_semi).abs() > 0.005 {
                        self.plugin_pending[i].last_semi = semi;
                        let notes = &mut self.plugin_notes[p.slot as usize];
                        if notes.len() < MAX_EVENTS {
                            notes.push(NoteMsg::Tuning {
                                time: t,
                                key: p.key,
                                note_id: p.note_id,
                                semitones: semi as f64,
                            });
                        }
                    }
                }
            }
            pos += 1;
        }
    }

    /// プラグインの窓口を 1 つでも持っているか。
    pub fn has_plugins(&self) -> bool {
        self.plugins.iter().any(Option::is_some)
    }

    /// オフライン用: 持っている窓口を止めて返す(呼び出し側がメインスレッドで片付ける)。
    pub fn take_plugins(&mut self) -> Vec<Box<Processor>> {
        let mut out = Vec::new();
        for p in self.plugins.iter_mut() {
            if let Some(mut p) = p.take() {
                p.clap.stop();
                out.push(p);
            }
        }
        out
    }

    /// 再生位置を UI・MIDI 受信側へ公開する(書いた時刻とブロック長も添える)。
    fn publish_pos(&self, frames: usize) {
        let sh = &self.shared;
        sh.pos.store(self.pos, Ordering::Release);
        sh.block_frames.store(frames as u32, Ordering::Release);
        sh.pos_nanos
            .store(sh.epoch.elapsed().as_nanos() as u64, Ordering::Release);
    }

    /// ライブ演奏キューを取り出して発音・消音する(アロケーションなし)。
    /// レンダラの時計(再生・停止に関係なく、処理したサンプル数だけ進む)。
    /// 時刻指定のノート([`crate::midi::TimedNote::at`])はこの時計で指す
    pub fn clock(&self) -> u64 {
        self.clock
    }

    /// 時刻指定のノートを鳴らし始める(送り先トラックの音源で。CLAP のトラックは鳴らさない)。
    fn start_timed(&mut self, n: crate::midi::TimedNote, now: u64, data: &PlaybackData, sr: f32) {
        let Some(mix) = data.tracks.get(n.track as usize) else {
            return;
        };
        if mix.plugin.is_some() {
            return;
        }
        if self.live_voices.len() >= MAX_LIVE_VOICES {
            // 満杯なら離した音を優先して(無ければ先頭を)捨てる
            let victim = self
                .live_voices
                .iter()
                .position(|v| v.released)
                .unwrap_or(0);
            self.live_voices.swap_remove(victim);
        }
        let instrument = mix.instrument.clone();
        let state = VoiceState::start(
            &instrument,
            crate::data::pitch_to_freq(n.pitch),
            n.pitch,
            n.vel.min(127) as f32 / 127.0,
            glaux_core::Articulation::Normal,
            sr,
        );
        self.live_voices.push(LiveVoice {
            track: n.track as u32,
            pitch: n.pitch,
            ch: 0,
            shape: glaux_dsp::NoteShape::NONE,
            released: false,
            off_at: now + n.dur.max(1) as u64,
            sustained: false,
            instrument,
            state,
        });
    }

    fn consume_live(&mut self, data: &PlaybackData, sr: f32) {
        for _ in 0..MAX_LIVE_EVENTS_PER_BLOCK {
            let Some(ev) = self.shared.live.pop() else {
                break;
            };
            match ev {
                LiveEvent::NoteOn {
                    track,
                    pitch,
                    vel,
                    ch,
                } if self
                    .track_plugin
                    .get(track as usize)
                    .copied()
                    .flatten()
                    .is_some() =>
                {
                    // プラグインのトラック: ノート ID を付けて送り(MPE の 1 音ごとの表現のため)、鍵盤を離すまで保持
                    let slot = self.track_plugin[track as usize].unwrap_or(0);
                    let note_id = self.next_note_id;
                    self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
                    self.plugin_note_on(slot, pitch, vel as f32 / 127.0, 0, Some(note_id));
                    let mut p = PendingOff::simple(slot, pitch, u64::MAX, false);
                    p.note_id = note_id;
                    p.ch = ch;
                    self.push_pending(p);
                    if ch != 0 {
                        self.send_live_expression(slot, pitch, note_id, ch);
                    }
                }
                LiveEvent::NoteOn {
                    track,
                    pitch,
                    vel,
                    ch,
                } => {
                    // 同じ音高を打ち直したら前の音はリリースへ
                    for v in self.live_voices.iter_mut() {
                        if v.pitch == pitch && v.ch == ch && !v.released {
                            v.state.note_off();
                            v.released = true;
                            v.sustained = false;
                        }
                    }
                    if self.live_voices.len() >= MAX_LIVE_VOICES {
                        // 満杯なら離した音を優先して(無ければ先頭を)捨てる
                        let victim = self
                            .live_voices
                            .iter()
                            .position(|v| v.released)
                            .unwrap_or(0);
                        self.live_voices.swap_remove(victim);
                    }
                    let (instrument, track) = match data.tracks.get(track as usize) {
                        Some(mix) if track != LIVE_NO_TRACK => (mix.instrument.clone(), track),
                        _ => (glaux_dsp::InstrumentParams::default(), LIVE_NO_TRACK),
                    };
                    let state = VoiceState::start(
                        &instrument,
                        crate::data::pitch_to_freq(pitch),
                        pitch,
                        vel as f32 / 127.0,
                        glaux_core::Articulation::Normal,
                        sr,
                    );
                    self.live_voices.push(LiveVoice {
                        track,
                        pitch,
                        ch,
                        shape: glaux_dsp::NoteShape::NONE,
                        released: false,
                        off_at: u64::MAX,
                        sustained: false,
                        instrument,
                        state,
                    });
                    let i = self.live_voices.len() - 1;
                    self.apply_live_expression(i);
                }
                LiveEvent::NoteOff { pitch, ch } => {
                    self.release_live_plugin_notes(Some(pitch));
                    for v in self.live_voices.iter_mut() {
                        if v.pitch == pitch && v.ch == ch && !v.released {
                            if self.sustain {
                                v.sustained = true;
                            } else {
                                v.state.note_off();
                                v.released = true;
                            }
                        }
                    }
                }
                LiveEvent::PitchBend { ch, v, range } => {
                    let c = (ch & 0xF) as usize;
                    self.mpe_bend[c] = (v as f32 - 8192.0) / 8192.0 * range as f32 * 100.0;
                    // 1 チャンネル目は今までどおり MIDI のベンドとしてプラグインへ(幅はプラグインの設定)
                    if c == 0 {
                        self.live_plugin_midi([0xE0, (v & 0x7F) as u8, ((v >> 7) & 0x7F) as u8]);
                    }
                    self.live_expression_changed(ch);
                }
                LiveEvent::Timbre { ch, v } => {
                    let c = (ch & 0xF) as usize;
                    self.mpe_bright[c] = Some(((v as f32 - 64.0) / 63.0).clamp(-1.0, 1.0));
                    if c == 0 {
                        self.live_plugin_midi([0xB0, 74, v & 0x7F]);
                    }
                    self.live_expression_changed(ch);
                }
                LiveEvent::Pressure { ch, v } => {
                    let c = (ch & 0xF) as usize;
                    self.mpe_press[c] = Some(v as f32 / 127.0);
                    if c == 0 {
                        self.live_plugin_midi([0xD0, v & 0x7F, 0]);
                    }
                    self.live_expression_changed(ch);
                }
                LiveEvent::Sustain(on) => {
                    self.live_plugin_midi([0xB0, 64, if on { 127 } else { 0 }]);
                    self.sustain = on;
                    if !on {
                        for v in self.live_voices.iter_mut() {
                            if v.sustained && !v.released {
                                v.state.note_off();
                                v.released = true;
                                v.sustained = false;
                            }
                        }
                    }
                }
                LiveEvent::AllOff => {
                    self.release_live_plugin_notes(None);
                    self.sustain = false;
                    for v in self.live_voices.iter_mut() {
                        if !v.released {
                            v.state.note_off();
                            v.released = true;
                            v.sustained = false;
                        }
                    }
                }
            }
        }
    }

    /// MIDI キーボードの送り先がプラグインのトラックなら、MIDI メッセージをそのまま送る
    fn live_plugin_midi(&mut self, data: [u8; 3]) {
        if let Some(slot) = self.live_plugin_slot() {
            let notes = &mut self.plugin_notes[slot];
            if notes.len() < MAX_EVENTS {
                notes.push(NoteMsg::Midi { time: 0, data });
            }
        }
    }

    /// チャンネル `ch` の音に効く今の (ベンドのセント, 音色, 押し込み)。1 チャンネル目の分は全部の音に効く
    fn live_expression(&self, ch: u8) -> (f32, Option<f32>, Option<f32>) {
        let c = (ch & 0xF) as usize;
        let bend = self.mpe_bend[0] + if c != 0 { self.mpe_bend[c] } else { 0.0 };
        let bright = if c != 0 { self.mpe_bright[c] } else { None }.or(self.mpe_bright[0]);
        let press = if c != 0 { self.mpe_press[c] } else { None }.or(self.mpe_press[0]);
        (bend, bright, press)
    }

    /// 内蔵音源のライブの音に、今の表現をかける(ベンドは音程、音色は明るさ、押し込みは 0〜+6dB)
    fn apply_live_expression(&mut self, i: usize) {
        let (bend, bright, press) = self.live_expression(self.live_voices[i].ch);
        let v = &mut self.live_voices[i];
        let curve = |x: f32| glaux_dsp::PitchCurve::from_points(&[(0.0, x)]);
        v.state.set_curve(&if bend != 0.0 {
            curve(bend)
        } else {
            glaux_dsp::PitchCurve::EMPTY
        });
        v.shape.bright = bright.map_or(glaux_dsp::PitchCurve::EMPTY, curve);
        v.shape.volume = press.map_or(glaux_dsp::PitchCurve::EMPTY, |p| curve(p * 6.0));
    }

    /// プラグインのライブの音 1 つに、そのチャンネルの表現を 1 音ごとの表現として送る(MPE)
    fn send_live_expression(&mut self, slot: usize, key: u8, note_id: u32, ch: u8) {
        let (bend, bright, press) = self.live_expression(ch);
        // 1 チャンネル目のベンドは MIDI のベンドで送っているので、ここではそのチャンネルの分だけ
        let own = bend - self.mpe_bend[0];
        let notes = &mut self.plugin_notes[slot];
        if notes.len() + 3 > MAX_EVENTS {
            return;
        }
        notes.push(NoteMsg::Tuning {
            time: 0,
            key,
            note_id,
            semitones: own as f64 / 100.0,
        });
        if let Some(b) = bright {
            notes.push(NoteMsg::Brightness {
                time: 0,
                key,
                note_id,
                value: (0.5 + 0.5 * b) as f64,
            });
        }
        if let Some(p) = press {
            notes.push(NoteMsg::Pressure {
                time: 0,
                key,
                note_id,
                value: p as f64,
            });
        }
    }

    /// チャンネル `ch` の表現が変わった: その音(1 チャンネル目なら全部の音)に反映する
    fn live_expression_changed(&mut self, ch: u8) {
        let affects = |note_ch: u8| ch == 0 || note_ch == ch;
        for i in 0..self.live_voices.len() {
            if affects(self.live_voices[i].ch) {
                self.apply_live_expression(i);
            }
        }
        // プラグインの音は MPE のチャンネル(2 チャンネル目以降)の分だけ 1 音ごとに送る
        if ch == 0 {
            return;
        }
        for i in 0..self.plugin_pending.len() {
            let p = self.plugin_pending[i];
            if p.end == u64::MAX && !p.seq && p.note_id != 0 && p.ch == ch {
                self.send_live_expression(p.slot as usize, p.key, p.note_id, p.ch);
            }
        }
    }
}

/// 出力デバイスの切り替えでレンダラが作り直されるとき、プラグインの窓口を受け渡し口へ戻し、
/// 新しいレンダラがそのまま引き継げるようにする(この時点で古いストリームは止まっている)。
impl Drop for Renderer {
    fn drop(&mut self) {
        let slots = self.shared.plugin_slots.clone();
        for (i, p) in self.plugins.iter_mut().enumerate() {
            let Some(mine) = p.take() else { continue };
            let my_gen = mine.gen;
            let Some(waiting) = slots[i].put_incoming(mine) else {
                continue;
            };
            // 別の窓口が既に置かれていた: 新しい世代を残し、古い方は返却して片付けてもらう
            let mut older = if waiting.gen > my_gen {
                match slots[i].put_incoming(waiting) {
                    Some(m) => m,
                    None => continue,
                }
            } else {
                waiting
            };
            older.clap.stop();
            let _ = slots[i].put_outgoing(older);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_writes_ahead_and_reads_back_once() {
        let mut ring = [vec![0.0f32; 16], vec![0.0f32; 16]];
        let l: Vec<f32> = (1..=6).map(|v| v as f32).collect();
        let r: Vec<f32> = l.iter().map(|v| -v).collect();
        // 位置 12 から 3 先(折り返す)に書き、同じ位置に 0 遅れで足す
        ring_add(&mut ring, 12 + 3, &l, &r, 1.0);
        ring_add(&mut ring, 12, &l, &r, 0.5);
        let (mut ol, mut or) = (vec![0.0f32; 6], vec![0.0f32; 6]);
        ring_take(&mut ring, 12, &mut ol, &mut or);
        assert_eq!(ol, vec![0.5, 1.0, 1.5, 3.0, 4.5, 6.0]);
        assert_eq!(or[3], -3.0);
        // 読んだ区間は消え、続きは次のブロックで出る
        let (mut ol2, mut or2) = (vec![0.0f32; 3], vec![0.0f32; 3]);
        ring_take(&mut ring, 18, &mut ol2, &mut or2);
        assert_eq!(ol2, vec![4.0, 5.0, 6.0]);
        let (mut ol3, mut or3) = (vec![0.0f32; 6], vec![0.0f32; 6]);
        ring_take(&mut ring, 12, &mut ol3, &mut or3);
        assert!(ol3.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn gather_aligns_branches_with_different_latency() {
        // 入力(0)と、遅れ 2 のノード(1)が合流する。早い方(入力)を 2 遅らせて揃える
        let done = [
            [vec![1.0f32, 0.0, 0.0, 0.0], vec![0.0f32; 4]],
            [vec![0.0f32, 0.0, 1.0, 0.0], vec![0.0f32; 4]],
        ];
        let times = [0u32, 2];
        let mut ring = [vec![0.0f32; 32], vec![0.0f32; 32]];
        let (mut ol, mut or) = (vec![0.0f32; 4], vec![0.0f32; 4]);
        gather(
            &[(0, 1.0), (1, 1.0)],
            &done,
            &mut ol,
            &mut or,
            Some((&mut ring, 0)),
            &times,
            2,
        );
        assert_eq!(ol, vec![0.0, 0.0, 2.0, 0.0]);
    }

    use crate::data::{NoteEvent, TrackMix};
    use glaux_dsp::{InstrumentParams, SubtractiveParams, Waveform};

    /// テスト用: リリースの短いサイン波 subtractive(旧サイン波シンセ相当)
    fn test_instrument() -> InstrumentParams {
        InstrumentParams::Subtractive(SubtractiveParams {
            waveform: Waveform::Sine,
            cutoff: 12_000.0,
            resonance: 0.0,
            attack: 0.001,
            decay: 1.0,
            sustain: 1.0,
            release: 0.01,
            filter_env: 0.0,
            unison: 1,
            detune_cents: 0.0,
            sub: 0.0,
            noise: 0.0,
            noise_color: glaux_dsp::NoiseColor::White,
            osc_level: 1.0,
            crackle: 0.0,
            gain: 1.0,
            tone: Default::default(),
        })
    }

    fn data_with_note(start: u64, end: u64, audible: bool) -> PlaybackData {
        PlaybackData {
            events: Arc::new(vec![NoteEvent {
                articulation: Default::default(),
                expr: crate::data::NO_EXPR,
                fade_in: 0,
                fade_out: 0,
                glide: 0.0,
                choke: 0,
                variant: 0,
                start,
                end,
                freq: 440.0,
                pitch: 69,
                amp: 1.0,
                track: 0,
            }]),
            tracks: vec![TrackMix {
                gain_l: 1.0,
                gain_r: 1.0,
                audible,
                base_amp: 1.0,
                base_pan: 0.0,
                vol_db_auto: vec![],
                pan_auto: vec![],
                device_auto: vec![],
                fx_auto: vec![],
                instrument: test_instrument(),
                layers: vec![],
                effects: vec![],
                fx_graph: None,
                plugin: None,
                plugin_auto: vec![],
                plugin_mods: vec![],
                is_bus: false,
                sends: vec![],
                output: None,
                stereo: false,
                ident: 1,
                // 実際の構築(build_playback_data)と同じく、ノートが違えば発音内容も違う
                content: start.wrapping_mul(31) ^ end,
            }],
            exprs: Default::default(),
            audio_events: vec![],
            master_effects: vec![],
            master_fx_graph: None,
            master_amp: 1.0,
            master_vol_auto: vec![],
            master_fx_auto: vec![],
            end_sample: end,
            sample_rate: 48_000.0,
            tempo: vec![],
            sigs: vec![],
            conv: vec![],
            order: vec![],
        }
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    #[test]
    fn timed_notes_start_on_the_exact_sample_while_stopped() {
        // 曲は止めたまま(ゲームの効果音用)。時計 5000 から 2400 サンプル押す
        let shared = Arc::new(Shared::new(data_with_note(10_000_000, 10_000_001, true)));
        let mut r = Renderer::new(shared.clone());
        assert!(shared.notes.push(crate::midi::TimedNote {
            at: 5000,
            track: 0,
            pitch: 69,
            vel: 100,
            dur: 2400,
        }));
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; 1024 * 2];
        for _ in 0..16 {
            r.process(&mut buf, 2);
            out.extend(buf.chunks(2).map(|c| c[0]));
        }
        assert_eq!(r.clock(), 16 * 1024);
        let first = out.iter().position(|v| v.abs() > 1e-6).expect("鳴る");
        // 指定したサンプルちょうどで鳴り始める(ブロックの途中でも)。発振器は位相 0(sin 0 = 0)から
        // 始まるので、0 でない最初の値は 1〜2 サンプル後になる
        assert!((5000..=5002).contains(&first), "{first}");
        assert!(out[..5000].iter().all(|v| *v == 0.0), "それより前は無音");
        assert!(rms(&out[5200..7000]) > 0.01, "押している間は鳴る");
        // 離した後はリリースで消えていく(十分後にはほぼ無音)
        assert!(rms(&out[14_000..16_000]) < rms(&out[5200..7000]) * 0.05);
        // 過ぎた時刻を指定したら次のブロックの頭ですぐ鳴る
        assert!(shared.notes.push(crate::midi::TimedNote {
            at: 0,
            track: 0,
            pitch: 72,
            vel: 100,
            dur: 480,
        }));
        r.process(&mut buf, 2);
        let head: Vec<f32> = buf[..800].chunks(2).map(|c| c[0]).collect();
        assert!(rms(&head) > 0.01, "過ぎた時刻なら次のブロックの頭で鳴る");
    }

    fn render_block(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        r.process(&mut buf, 2);
        buf
    }

    // ---- 再生中の編集・シークで音を切らない ----------------------------------------

    /// 2 トラック(どちらも 4 小節伸ばしっぱなしの音)の曲
    fn two_pads() -> glaux_core::Project {
        use glaux_core::{
            Articulation, Clip, ClipContent, ClipId, Note, NoteId, Tick, Track, TrackId, TrackKind,
        };
        let mut p = glaux_core::Project::new("t");
        for (name, pitch) in [("A", 57u8), ("B", 64u8)] {
            let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
            let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 4));
            if let ClipContent::Midi { notes, .. } = &mut c.content {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(0),
                    dur: Tick(3840 * 4),
                    pitch,
                    vel: 100,
                    articulation: Articulation::Normal,
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                    condition: None,
                });
            }
            let mut d = glaux_core::Device::builtin("subtractive");
            d.params.insert("sustain".into(), 1.0.into());
            t.device = Some(d);
            t.clips.push(c);
            p.tracks.push(t);
        }
        p
    }

    fn build(p: &glaux_core::Project) -> Arc<PlaybackData> {
        Arc::new(crate::data::build_playback_data(
            p,
            48_000.0,
            &Default::default(),
        ))
    }

    /// 差し替えの直後 5ms の音量が、直前 5ms の何倍か(1 に近いほど途切れていない)
    fn level_across_swap(before: &glaux_core::Project, after: &glaux_core::Project) -> f32 {
        let shared = Arc::new(Shared::new((*build(before)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 48_000 / 2);
        let pre = rms(&render_block(&mut r, 240));
        shared.data.store(build(after));
        let post = rms(&render_block(&mut r, 240));
        post / pre
    }

    #[test]
    fn levels_follow_each_track_and_reset_when_read() {
        // ミキサーのメーター: トラックごと(フェーダーの後)とマスターのピーク。読むと 0 に戻る
        let mut p = two_pads();
        p.tracks[1].volume_db = -20.0;
        p.tracks[1].mute = true;
        let shared = Arc::new(Shared::new((*build(&p)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 4800);
        let (tracks, master) = shared.levels.take(2);
        assert!(tracks[0] > -30.0, "鳴っているトラック {tracks:?}");
        assert!(
            tracks[1] <= -100.0,
            "ミュートしたトラックは振れない {tracks:?}"
        );
        assert!(master > -30.0 && master <= 6.0, "マスター {master}");
        let (again, m2) = shared.levels.take(2);
        assert!(again[0] <= -100.0 && m2 <= -100.0, "読むとリセットされる");
    }

    #[test]
    fn knob_changes_keep_the_sounding_notes() {
        // 以前はつまみを 1 回動かすたびに、鳴っている音がすべて切れていた
        let before = two_pads();
        let mut after = before.clone();
        after.tracks[0].volume_db = -1.0;
        if let Some(d) = &mut after.tracks[0].device {
            d.params.insert("cutoff".into(), 3000.0.into());
        }
        let ratio = level_across_swap(&before, &after);
        assert!(ratio > 0.7, "途切れた: {ratio:.3}");
    }

    #[test]
    fn convolution_reverb_uses_the_ir_asset_and_survives_rebuilds() {
        use glaux_core::{Asset, AssetId, Effect, FxId, ParamValue};
        let mut p = two_pads();
        let asset = AssetId::from_sha256_hex(&"ab".repeat(32)).unwrap();
        p.assets.insert(
            asset.clone(),
            Asset {
                path: "audio/ir.wav".into(),
                sample_rate: 48_000,
                channels: 1,
                frames: 24_000,
            },
        );
        let mut fx = Effect::builtin(FxId::new(), "convolution");
        fx.params
            .insert("ir".into(), ParamValue::Enum(asset.to_string()));
        fx.params.insert("mix".into(), ParamValue::Float(1.0));
        p.tracks[0].effects.push(fx);
        // IR: 0.5 秒の減衰する雑音
        let mut bank = crate::data::SampleBank::default();
        bank.sync_with(
            &p,
            &mut |_| {
                let mut r: u32 = 3;
                Ok(glaux_dsp::SampleData::mono(
                    (0..24_000)
                        .map(|i| {
                            r ^= r << 13;
                            r ^= r >> 17;
                            r ^= r << 5;
                            (r as f32 / u32::MAX as f32 - 0.5) * (-(i as f32) / 4000.0).exp()
                        })
                        .collect(),
                    48_000.0,
                ))
            },
            &mut |_| Err("なし".into()),
        );
        let data = crate::data::build_playback_data(&p, 48_000.0, &bank);
        assert_eq!(data.conv.len(), 1);
        // 作り直しても同じ IR なら同じ本体(響きが途切れない)
        let again = crate::data::build_playback_data(&p, 48_000.0, &bank);
        assert!(Arc::ptr_eq(&data.conv[0], &again.conv[0]));
        // 鳴らすと音が出て、ほかのトラックは畳み込みの遅れぶん遅らせる
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let out = render_block(&mut r, 48_000);
        assert!(rms(&out) > 1e-4);
        assert_eq!(r.pdc_delay(1), glaux_dsp::convolver::PART as u32);
        // IR が無ければ素通し(本体は作らない)
        let mut none = p.clone();
        none.assets.clear();
        let bank2 = crate::data::SampleBank::default();
        assert!(crate::data::build_playback_data(&none, 48_000.0, &bank2)
            .conv
            .is_empty());
    }

    #[test]
    fn per_track_loads_are_measured() {
        // リバーブを挿したトラックは、何も挿していないトラックより重い
        let mut p = two_pads();
        for _ in 0..4 {
            p.tracks[1].effects.push(glaux_core::Effect::builtin(
                glaux_core::FxId::new(),
                "reverb",
            ));
        }
        let shared = Arc::new(Shared::new((*build(&p)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = shared.stats.take_loads(2);
        for _ in 0..20 {
            let _ = render_block(&mut r, 480);
        }
        let (tracks, master) = shared.stats.take_loads(2);
        assert_eq!(tracks.len(), 2);
        assert!(tracks[0] > 0.0 && tracks[1] > tracks[0], "{tracks:?}");
        assert!(master >= 0.0);
        // 読むとリセット
        let (again, _) = shared.stats.take_loads(2);
        assert!(again.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn builtin_limiter_latency_is_compensated() {
        // 先読みのリミッタを挿したトラックの遅れぶん、ほかのトラックを遅らせてそろえる
        let mut p = two_pads();
        p.tracks[0].effects.push(glaux_core::Effect::builtin(
            glaux_core::FxId::new(),
            "limiter",
        ));
        let shared = Arc::new(Shared::new((*build(&p)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 480);
        let lat = glaux_dsp::limiter::LimiterParams::new(0.0, -1.0, 100.0, 48_000.0).latency();
        assert_eq!(r.pdc_delay(0), 0);
        assert_eq!(r.pdc_delay(1), lat);
    }

    /// 2 トラックの曲に、空のバスを `n` 本足す(末尾に並ぶ)
    fn with_buses(n: usize) -> glaux_core::Project {
        let mut p = two_pads();
        for i in 0..n {
            p.tracks.push(glaux_core::Track::new(
                glaux_core::TrackId::new(),
                format!("Bus{i}"),
                glaux_core::TrackKind::Bus,
            ));
        }
        p
    }

    fn render_project(p: &glaux_core::Project, blocks: usize) -> Vec<f32> {
        let shared = Arc::new(Shared::new((*build(p)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared);
        (0..blocks)
            .flat_map(|_| render_block(&mut r, 480))
            .collect()
    }

    #[test]
    fn group_bus_sums_its_tracks_and_its_fader_controls_them() {
        let plain = render_project(&with_buses(1), 20);
        let mut grouped = with_buses(1);
        let bus = grouped.tracks[2].id.clone();
        for t in &mut grouped.tracks[..2] {
            t.output = Some(bus.clone());
        }
        // バスが素通し(0dB・中央)なら、マスターへ直接送ったときと同じ音
        let g = render_project(&grouped, 20);
        let diff = plain
            .iter()
            .zip(&g)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(rms(&plain) > 1e-3);
        assert!(diff < 1e-5, "{diff}");
        // バスのフェーダーを下げると、まとめた 2 本が下がる
        grouped.tracks[2].volume_db = -60.0;
        assert!(rms(&render_project(&grouped, 20)) < rms(&plain) * 0.01);
        // バスをミュートしても同じ
        grouped.tracks[2].volume_db = 0.0;
        grouped.tracks[2].mute = true;
        assert!(rms(&render_project(&grouped, 20)) < 1e-6);
    }

    #[test]
    fn bus_can_feed_another_bus() {
        // トラック → バス0 → バス1 → マスター。バス1 を下げると全部下がる
        let mut p = with_buses(2);
        let (b0, b1) = (p.tracks[2].id.clone(), p.tracks[3].id.clone());
        for t in &mut p.tracks[..2] {
            t.output = Some(b0.clone());
        }
        p.tracks[2].output = Some(b1.clone());
        let open = render_project(&p, 20);
        assert!(rms(&open) > 1e-3);
        p.tracks[3].volume_db = -60.0;
        assert!(rms(&render_project(&p, 20)) < rms(&open) * 0.01);
        // バス0 からバス1 へのセンド(フェーダー前)でも届く
        let mut q = with_buses(2);
        let b0 = q.tracks[2].id.clone();
        for t in &mut q.tracks[..2] {
            t.output = Some(b0.clone());
        }
        let b1 = q.tracks[3].id.clone();
        q.tracks[2].volume_db = -90.0;
        q.tracks[2].sends.push(glaux_core::Send {
            target: b1,
            level_db: 0.0,
            pre_fader: true,
        });
        assert!(rms(&render_project(&q, 20)) > rms(&open) * 0.5);
    }

    #[test]
    fn latency_is_compensated_across_bus_levels() {
        // トラック0 → (リミッタの遅れ) バス0(リミッタ) → マスター、トラック1 → マスター
        let lat = glaux_dsp::limiter::LimiterParams::new(0.0, -1.0, 100.0, 48_000.0).latency();
        let mut p = with_buses(1);
        let bus = p.tracks[2].id.clone();
        p.tracks[0].output = Some(bus);
        for i in [0, 2] {
            p.tracks[i].effects.push(glaux_core::Effect::builtin(
                glaux_core::FxId::new(),
                "limiter",
            ));
        }
        let shared = Arc::new(Shared::new((*build(&p)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared);
        let _ = render_block(&mut r, 480);
        // トラック0 はバスの入口にそのまま(バスへ入るのは自分だけ)、バスは 2 倍遅れてマスターへ。
        // トラック1 はその 2 倍ぶん遅らせて揃える
        assert_eq!(r.pdc_delay(0), 0);
        assert_eq!(r.pdc_delay(2), 0);
        assert_eq!(r.pdc_delay(1), 2 * lat);
    }

    #[test]
    fn fader_moves_ramp_instead_of_jumping() {
        // フェーダーを 0 → -40dB に一気に下げても、音量は約 5ms かけて下がる(段差でプチッと鳴らない)
        let mut before = two_pads();
        before.tracks[1].mute = true;
        let mut after = before.clone();
        after.tracks[0].volume_db = -40.0;
        after.master.volume_db = -6.0;
        let shared = Arc::new(Shared::new((*build(&before)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 48_000 / 2);
        let pre = rms(&render_block(&mut r, 48));
        shared.data.store(build(&after));
        let first = rms(&render_block(&mut r, 48));
        let _ = render_block(&mut r, 2400);
        let settled = rms(&render_block(&mut r, 480));
        assert!(first / pre > 0.3, "直後に飛び下がった: {:.3}", first / pre);
        let db = 20.0 * (settled / pre).log10();
        assert!(db < -40.0, "50ms 後には下がりきる: {db:.1} dB");
    }

    #[test]
    fn editing_another_track_keeps_this_track_sounding() {
        let before = two_pads();
        let mut after = before.clone();
        // B の音を消す(A は変わらない)
        if let glaux_core::ClipContent::Midi { notes, .. } = &mut after.tracks[1].clips[0].content {
            notes.clear();
        }
        let mut only_a = before.clone();
        only_a.tracks[1].mute = true;
        // A だけを聴く: B をミュートした曲どうしで、B のノートだけ消した場合
        let mut only_a_after = after.clone();
        only_a_after.tracks[1].mute = true;
        let ratio = level_across_swap(&only_a, &only_a_after);
        assert!(ratio > 0.7, "A が途切れた: {ratio:.3}");
    }

    #[test]
    fn seeking_into_a_long_note_plays_it() {
        // 以前はシーク位置より前に始まった音は鳴らなかった(パッドの途中へシークすると無音)
        let shared = Arc::new(Shared::new((*build(&two_pads())).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        shared.seek.store(48_000 * 3, Ordering::Release);
        let _ = render_block(&mut r, 480); // フェードイン
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.05, "シーク先で鳴るはず: {}", rms(&block));
    }

    #[test]
    fn editing_the_same_track_replays_the_long_note() {
        // 同じトラックに音を足すと発音内容が変わるので、鳴っている音はいったん短く消えて、
        // 途中から鳴らし直される(鳴り続ける)
        let before = two_pads();
        let mut after = before.clone();
        if let glaux_core::ClipContent::Midi { notes, .. } = &mut after.tracks[0].clips[0].content {
            let mut n = notes[0].clone();
            n.id = glaux_core::NoteId::new();
            n.pos = glaux_core::Tick(3840 * 3);
            n.dur = glaux_core::Tick(960);
            n.pitch = 69;
            notes.push(n);
        }
        let shared = Arc::new(Shared::new((*build(&before)).clone()));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 48_000 / 2);
        let pre = rms(&render_block(&mut r, 4800));
        shared.data.store(build(&after));
        let _ = render_block(&mut r, 480);
        let post = rms(&render_block(&mut r, 4800));
        assert!(post > pre * 0.7, "鳴り続けるはず: {pre:.3} → {post:.3}");
        // 声が二重に積み上がっていない(トラックごとに 1 音ずつ)
        assert_eq!(r.voices.iter().filter(|v| !v.stolen).count(), 2);
    }

    #[test]
    fn produces_sound_during_note_and_silence_after() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.1, "ノート区間で音が出るはず");

        // リリース(10ms)より十分先まで進める
        let _ = render_block(&mut r, 4800);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-3, "ノート終了後はほぼ無音のはず");
        assert_eq!(shared.pos.load(Ordering::Acquire), 4800 * 3);
    }

    #[test]
    fn queued_jump_fires_at_the_sample_and_can_switch_the_loop() {
        // ノート 0..4800。9600 で 24000 へ飛び、飛んだ先は 24000..28800 をループ
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        shared.jump_to.store(24_000, Ordering::Release);
        shared.jump_loop_start.store(24_000, Ordering::Release);
        shared.jump_loop_end.store(28_800, Ordering::Release);
        shared.jump_at.store(9600, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 4800);
        assert_eq!(shared.last_jump.read().seq, 0, "まだ飛んでいない");
        let _ = render_block(&mut r, 4800 + 100);
        // 9600 のサンプルちょうどで飛び、そこから 100 サンプル進んでいる
        assert_eq!(shared.pos.load(Ordering::Acquire), 24_100);
        let rec = shared.last_jump.read();
        assert_eq!(
            (rec.seq, rec.clock, rec.from, rec.to),
            (1, 9600, 9600, 24_000)
        );
        assert_eq!(
            shared.jump_at.load(Ordering::Acquire),
            NO_SEEK,
            "予約は消える"
        );
        assert_eq!(shared.loop_start.load(Ordering::Acquire), 24_000);
        assert_eq!(shared.loop_end.load(Ordering::Acquire), 28_800);
        // 飛んだ先のループで折り返す(記録も増える)
        let _ = render_block(&mut r, 4800);
        let pos = shared.pos.load(Ordering::Acquire);
        assert!((24_000..28_800).contains(&pos), "{pos}");
        let rec = shared.last_jump.read();
        assert_eq!((rec.seq, rec.from, rec.to), (2, 28_800, 24_000));
        // 曲の終わりを過ぎてもループ中は止まらない
        assert!(shared.playing.load(Ordering::Acquire));
    }

    #[test]
    fn output_gain_scales_only_the_output() {
        let shared = Arc::new(Shared::new(data_with_note(0, 48_000, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 2400);
        let full = rms(&render_block(&mut r, 2400));
        let (_, master_full) = shared.levels.take(1);
        shared
            .output_gain
            .store(0.25f32.to_bits(), Ordering::Relaxed);
        let _ = render_block(&mut r, 2400);
        let _ = shared.levels.take(1);
        let quiet = rms(&render_block(&mut r, 2400));
        let (_, master_quiet) = shared.levels.take(1);
        assert!((quiet / full - 0.25).abs() < 0.03, "{}", quiet / full);
        // メーター(曲の音量)は変わらない
        assert!(
            (master_quiet - master_full).abs() < 0.5,
            "{master_full} → {master_quiet}"
        );
    }

    #[test]
    fn live_gain_scales_a_track_smoothly() {
        let shared = Arc::new(Shared::new(data_with_note(0, 48_000, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 2400);
        let full = rms(&render_block(&mut r, 2400));
        shared.live_gain[0].store(0.5f32.to_bits(), Ordering::Relaxed);
        let _ = render_block(&mut r, 2400);
        let half = rms(&render_block(&mut r, 2400));
        let ratio = half / full;
        assert!((ratio - 0.5).abs() < 0.08, "{ratio}");
    }

    #[test]
    fn loop_region_wraps_and_retriggers_notes() {
        // ノート 0..4800、ループ区間 0..9600
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        shared.loop_end.store(9600, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        // 1 周目: 前半は鳴り、後半は無音に向かう
        let first = render_block(&mut r, 4800);
        assert!(rms(&first) > 0.1);
        let _ = render_block(&mut r, 4800); // pos = 9600 → 次のブロック頭で巻き戻る

        // 2 周目に入った直後: ノートが再トリガされて再び鳴る
        let wrapped = render_block(&mut r, 4800);
        assert!(rms(&wrapped) > 0.1, "ループ 2 周目でも音が鳴るはず");
        let pos = shared.pos.load(Ordering::Acquire);
        assert!(pos <= 9600, "再生位置がループ区間内に戻るはず: {pos}");
        assert!(shared.playing.load(Ordering::Acquire));
    }

    #[test]
    fn loop_wraps_do_not_accumulate_released_voices() {
        // リリースの非常に長い音 + 短いループ。折り返しごとに旧世代を解放しないと
        // ボイスが際限なく積み重なって CPU が漸増する(実機で「段々カクつく」報告の原因)
        let mut data = data_with_note(0, 4700, true);
        if let InstrumentParams::Subtractive(p) = &mut data.tracks[0].instrument {
            p.release = 100.0;
            p.sustain = 1.0;
        }
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        shared.loop_end.store(4800, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        for _ in 0..30 {
            let _ = render_block(&mut r, 4800); // 1 ブロック = ちょうど 1 周
        }
        assert!(
            r.voices.len() <= 3,
            "旧世代ボイスが解放されず {} 個残っている",
            r.voices.len()
        );
        // 音は出続けている(現行世代は生きている)
        let block = render_block(&mut r, 2400);
        assert!(rms(&block) > 0.05);
    }

    #[test]
    fn looping_disables_auto_stop() {
        // 曲の終端(4800)+ 余韻 2 秒を大きく超える位置までループ区間を広げても
        // 自動停止しないこと
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        shared.loop_end.store(48_000 * 4, Ordering::Release); // 4 秒
        let mut r = Renderer::new(shared.clone());

        // end_sample + tail = 4800 + 96000 = 100800 を超えるまで進める
        for _ in 0..30 {
            let _ = render_block(&mut r, 4800);
        }
        assert!(
            shared.playing.load(Ordering::Acquire),
            "ループ中は自動停止しないはず"
        );

        // ループを解除すると従来どおり自動停止する
        shared.loop_end.store(0, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        let mut stopped = false;
        for _ in 0..40 {
            let _ = render_block(&mut r, 4800);
            if !shared.playing.load(Ordering::Acquire) {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "ループ解除後は曲末で自動停止するはず");
    }

    #[test]
    fn soft_clip_passes_normal_levels_and_limits_peaks() {
        for x in [0.0f32, 0.1, -0.5, 0.9, -0.9] {
            assert_eq!(soft_clip(x), x, "0.9 までは素通し");
        }
        let mut prev = 0.9f32;
        for i in 1..200 {
            let y = soft_clip(0.9 + i as f32 * 0.05);
            assert!(y >= prev && y <= 1.0, "単調に 1.0 へ近づき、超えない");
            prev = y;
        }
        assert_eq!(soft_clip(-3.0), -soft_clip(3.0));
    }

    #[test]
    fn layered_voices_stay_within_the_limit() {
        // 本体 + 3 層の音色で 300 音: 生きている声は上限を超えず、新しいノートは捨てられない
        let mut data = data_with_note(0, 96_000, true);
        let layer = crate::data::LayerMix {
            instrument: test_instrument(),
            mid: 1.0,
            side: 0.0,
            transpose: 0,
            key: (0, 127),
            vel: (1, 127),
        };
        data.tracks[0].layers = vec![layer.clone(), layer.clone(), layer];
        let base = data.events[0];
        data.events = Arc::new(
            (0..300u64)
                .map(|i| NoteEvent {
                    start: i * 10,
                    pitch: 40 + (i % 40) as u8,
                    ..base
                })
                .collect(),
        );
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let capacity = r.voices.capacity();
        let _ = render_block(&mut r, 3000);
        let live = r.voices.iter().filter(|v| !v.stolen).count();
        assert!(live <= MAX_VOICES, "live={live}");
        assert!(live >= MAX_VOICES - 3, "live={live}");
        // 最後のノートは 4 声とも鳴っている
        assert_eq!(
            r.voices.iter().filter(|v| !v.stolen && v.age < 15).count(),
            4
        );
        assert_eq!(
            r.voices.capacity(),
            capacity,
            "オーディオスレッドで確保しない"
        );
    }

    #[test]
    fn voices_over_the_limit_steal_the_oldest_instead_of_dropping_new_notes() {
        // 以前は 64 声で頭打ちになり、超えたノートは黙って捨てられていた
        // (20 トラック × 4 声で後ろの 4 トラックが無音)
        let mut data = data_with_note(0, 96_000, true);
        let base = data.events[0];
        data.events = Arc::new(
            (0..300u64)
                .map(|i| NoteEvent {
                    start: i * 10,
                    pitch: 40 + (i % 40) as u8,
                    ..base
                })
                .collect(),
        );
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let capacity = r.voices.capacity();

        // 300 音すべてが鳴り始めた直後: 生きている声は上限ちょうど、奪った音はフェード中
        let _ = render_block(&mut r, 3000);
        let live = r.voices.iter().filter(|v| !v.stolen).count();
        assert_eq!(live, MAX_VOICES, "新しいノートが捨てられていないこと");
        assert!(r.voices.len() <= MAX_VOICES + STEAL_RESERVE);
        // 奪われたのは古い音(最後に始まった音は残っている)
        assert!(r.voices.iter().any(|v| !v.stolen && v.age < 20));

        // フェード(3ms)が終われば、奪った音は解放される
        let _ = render_block(&mut r, 480);
        assert_eq!(r.voices.len(), MAX_VOICES);
        assert_eq!(
            r.voices.capacity(),
            capacity,
            "オーディオスレッドで再確保しない"
        );
    }

    #[test]
    fn finished_voices_are_released_before_note_end() {
        // ワンショット(ドラム)が鳴り終わったら、ノートが長くても
        // スロットを占有し続けない(SF2 ピアノ等の CPU 漸増対策の回帰テスト)
        let mut data = data_with_note(0, 480_000, true); // 10 秒のノート
        data.tracks[0].instrument = InstrumentParams::Drum(glaux_dsp::DrumParams {
            gain: 1.0,
            ..Default::default()
        });
        Arc::make_mut(&mut data.events)[0].pitch = 42; // クローズドハット(短い)
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        let _ = render_block(&mut r, 512);
        assert_eq!(r.voices.len(), 1, "発音直後はボイスがあるはず");
        // 2 秒後(ハットは鳴り終わっている。ノートはまだ 8 秒残っている)
        for _ in 0..20 {
            let _ = render_block(&mut r, 4800);
        }
        assert_eq!(
            r.voices.len(),
            0,
            "鳴り終わったボイスはノート終了前に解放されるはず"
        );
    }

    #[test]
    fn metronome_clicks_on_beats_and_recording_blocks_auto_stop() {
        // 無音のデータ(ノートは可聴でない)+ 120bpm のテンポ表 → 拍位置でだけ音が出る
        let mut data = data_with_note(0, 1, false);
        data.tempo = vec![crate::data::TempoSeg {
            sample: 0,
            tick: 0,
            samples_per_tick: 25.0,
        }];
        data.sigs = vec![(0, 4, 4)];
        data.end_sample = 1;
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        shared.metronome.store(true, Ordering::Release);
        shared.recording.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        // 0.5 秒 = 1 拍。拍頭直後 30ms は鳴り、拍の後半は無音
        let block = render_block(&mut r, 1440); // 0..30ms
        assert!(rms(&block) > 0.05, "拍頭でクリックが鳴るはず");
        let _ = render_block(&mut r, 24_000 - 1440 - 4800);
        let quiet = render_block(&mut r, 4800); // 拍の直前 100ms
        assert!(rms(&quiet) < 1e-4, "拍の間は無音");
        let next = render_block(&mut r, 1440); // 2 拍目の頭
        assert!(rms(&next) > 0.05, "次の拍でも鳴るはず");
        // end_sample + 余韻 2 秒を大きく超えても録音中は止まらない
        for _ in 0..40 {
            let _ = render_block(&mut r, 4800);
        }
        assert!(
            shared.playing.load(Ordering::Acquire),
            "録音中は自動停止しない"
        );
    }

    #[test]
    fn metronome_clicks_again_after_seeking_back() {
        let mut data = data_with_note(0, 1, false);
        data.tempo = vec![crate::data::TempoSeg {
            sample: 0,
            tick: 0,
            samples_per_tick: 25.0,
        }];
        data.sigs = vec![(0, 4, 4)];
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        shared.metronome.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        // 1.5 拍ぶん進めてから先頭へ戻す → 先頭の拍がもう一度鳴る
        let _ = render_block(&mut r, 36_000);
        shared.seek.store(0, Ordering::Release);
        let block = render_block(&mut r, 1440);
        assert!(rms(&block) > 0.05, "戻った小節でもクリックが鳴るはず");
    }

    #[test]
    fn paused_renderer_outputs_silence_and_holds_position() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        let block = render_block(&mut r, 512);
        assert!(rms(&block) < 1e-9);
        assert_eq!(shared.pos.load(Ordering::Acquire), 0);
    }

    #[test]
    fn muted_track_is_silent() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, false)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-9);
    }

    #[test]
    fn seek_replays_note_and_data_swap_resyncs() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        // ノートを過ぎるまで再生
        let _ = render_block(&mut r, 4800 * 3);
        // 先頭にシーク → もう一度鳴る
        shared.seek.store(0, Ordering::Release);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.1, "シーク後に再発音するはず");

        // データ差し替え(ノート位置が先) → 現位置より前のイベントは飛ばす
        shared
            .data
            .store(Arc::new(data_with_note(96_000, 100_000, true)));
        // 変わったトラックの音は切らずに 5ms で消える
        let _ = render_block(&mut r, 480);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-6, "差し替え後、未来のノートはまだ鳴らない");
    }

    #[test]
    fn preview_note_sounds_while_paused() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        // 停止中に試聴要求(counter=1, track=0, 100ms, pitch 69, vel 127)
        let packed = (1u64 << 48) | (100u64 << 16) | (69u64 << 8) | 127;
        shared.preview.store(packed, Ordering::Release);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.05, "停止中でも試聴は鳴るはず");
        // note_off(100ms)後は減衰して消える
        let _ = render_block(&mut r, 9600);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-3, "試聴は終わるはず");
        // 再生位置は動かない
        assert_eq!(shared.pos.load(Ordering::Acquire), 0);
    }

    #[test]
    fn live_midi_notes_sound_while_paused_and_release() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        assert!(rms(&render_block(&mut r, 4800)) < 1e-6, "何も無ければ無音");
        shared.live.push(LiveEvent::NoteOn {
            track: 0,
            pitch: 60,
            vel: 120,
            ch: 0,
        });
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.05, "停止中でもライブ演奏は鳴るはず");
        // 押している間は鳴り続ける
        assert!(rms(&render_block(&mut r, 4800)) > 0.05);
        shared.live.push(LiveEvent::NoteOff { pitch: 60, ch: 0 });
        let _ = render_block(&mut r, 4800);
        assert!(rms(&render_block(&mut r, 4800)) < 1e-3, "離したら消える");
        assert_eq!(shared.pos.load(Ordering::Acquire), 0, "再生位置は動かない");
    }

    #[test]
    fn live_sustain_pedal_holds_until_released() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        shared.live.push(LiveEvent::Sustain(true));
        shared.live.push(LiveEvent::NoteOn {
            track: 0,
            pitch: 64,
            vel: 100,
            ch: 0,
        });
        let _ = render_block(&mut r, 480);
        shared.live.push(LiveEvent::NoteOff { pitch: 64, ch: 0 });
        let _ = render_block(&mut r, 4800);
        assert!(
            rms(&render_block(&mut r, 4800)) > 0.05,
            "ペダル中は鳴り続ける"
        );
        shared.live.push(LiveEvent::Sustain(false));
        let _ = render_block(&mut r, 4800);
        assert!(
            rms(&render_block(&mut r, 4800)) < 1e-3,
            "ペダルを離したら消える"
        );
    }

    #[test]
    fn live_voices_are_capped_and_all_off_silences() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        for p in 0..100u8 {
            shared.live.push(LiveEvent::NoteOn {
                track: LIVE_NO_TRACK,
                pitch: p,
                vel: 10,
                ch: 0,
            });
        }
        let _ = render_block(&mut r, 480);
        assert!(r.live_voices.len() <= MAX_LIVE_VOICES);
        assert!(r.live_voices.capacity() <= MAX_LIVE_VOICES, "再確保しない");
        shared.live.push(LiveEvent::AllOff);
        for _ in 0..8 {
            let _ = render_block(&mut r, 48_000); // 既定音色のリリースを待つ
        }
        assert!(r.live_voices.is_empty());
    }

    #[test]
    fn mpe_bend_moves_only_the_notes_on_its_channel() {
        // 左チャンネルのゼロ交差から音の高さを測る
        let freq = |buf: &[f32]| -> f32 {
            let l: Vec<f32> = buf.chunks(2).map(|c| c[0]).collect();
            let crossings = l.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
            crossings as f32 / (l.len() as f32 / 48_000.0)
        };
        let run = |bend_ch: u8| -> f32 {
            let shared = Arc::new(Shared::new(data_with_note(0, 1, false)));
            let mut r = Renderer::new(shared.clone());
            // 2 チャンネル目で A3(220Hz)だけを鳴らす
            shared.live.push(LiveEvent::NoteOn {
                track: LIVE_NO_TRACK,
                pitch: 57,
                vel: 100,
                ch: 1,
            });
            let _ = render_block(&mut r, 4800);
            // そのチャンネルを +12 半音(幅 48 半音で 1/4 だけ倒す)
            shared.live.push(LiveEvent::PitchBend {
                ch: bend_ch,
                v: 8192 + 2048,
                range: 48,
            });
            let _ = render_block(&mut r, 960);
            freq(&render_block(&mut r, 9600))
        };
        let own = run(1);
        let other = run(5);
        assert!(
            (own - 440.0).abs() < 15.0,
            "自分のチャンネルのベンドで 1 オクターブ上: {own}"
        );
        assert!(
            (other - 220.0).abs() < 10.0,
            "ほかのチャンネルのベンドは効かない: {other}"
        );
    }

    #[test]
    fn ab_listening_plays_the_matched_side_at_the_current_position() {
        // 曲は無音。聴き比べの A は 0.2、B は 0.4(B を半分に下げてそろえる)
        let shared = Arc::new(Shared::new(data_with_note(10_000_000, 10_000_001, true)));
        shared.playing.store(true, Ordering::Release);
        shared.ab.store(Some(Arc::new(crate::ab::AbClip {
            start: 0,
            a: vec![0.2; 48_000 * 2],
            b: vec![0.4; 48_000 * 2],
            gain_a: 1.0,
            gain_b: 0.5,
        })));
        let mut r = Renderer::new(shared.clone());
        let last = |b: &[f32]| b[b.len() - 2];
        shared
            .ab_side
            .store(crate::ab::AbSide::A.code(), Ordering::Release);
        assert!((last(&render_block(&mut r, 4800)) - 0.2).abs() < 1e-3);
        // B に切り替えても同じ大きさ(そろえてある)。途中はクロスフェード
        shared
            .ab_side
            .store(crate::ab::AbSide::B.code(), Ordering::Release);
        let b = render_block(&mut r, 4800);
        assert!((last(&b) - 0.2).abs() < 1e-3);
        // 範囲の外は無音、やめればふつうの再生(無音)
        shared
            .ab_side
            .store(crate::ab::AbSide::Off.code(), Ordering::Release);
        assert!(last(&render_block(&mut r, 4800)).abs() < 1e-3);
    }

    #[test]
    fn audible_pos_trails_written_pos_by_up_to_a_block() {
        let shared = Arc::new(Shared::new(data_with_note(0, 48_000, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 960);
        let _ = render_block(&mut r, 960);
        let a = shared.audible_pos();
        assert!((960.0..=1920.0).contains(&a), "{a}");
    }

    #[test]
    fn auto_stops_after_end_plus_tail() {
        let shared = Arc::new(Shared::new(data_with_note(0, 480, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        // 480 + 96000(tail 2 秒) を超えるまで回す
        for _ in 0..25 {
            let _ = render_block(&mut r, 4800);
        }
        assert!(
            !shared.playing.load(Ordering::Acquire),
            "終端で自動停止するはず"
        );
    }
}
