//! cpal 出力ストリームと、UI 側から操作するための [`EngineHandle`]。
//!
//! `cpal::Stream` は `Send` ではないので、専用スレッドを立ててそこでストリームを
//! 生成・保持する。UI 側には Send + Sync な [`EngineHandle`] だけを渡す。

use crate::data::{build_playback_data, PlaybackData, SampleBank};
use crate::midi::{MidiConnection, MidiSink, MidiTake, RecordedNote, LIVE_NO_TRACK};
use crate::render::{Renderer, Shared, NO_SEEK};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glaux_core::{Project, TempoMap, Tick, TrackId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("出力デバイスが見つかりません")]
    NoDevice,
    #[error("サンプルフォーマット {0} は未対応です(f32 のみ)")]
    UnsupportedFormat(String),
    #[error("出力ストリームを開けません: {0}")]
    Stream(String),
}

/// UI(Tauri)側から再生を操作するハンドル。クローン可能・Send + Sync。
#[derive(Clone)]
pub struct EngineHandle {
    shared: Arc<Shared>,
    /// 出力のサンプルレート(f64 のビット列)。出力デバイスを切り替えると変わりうる
    sample_rate: Arc<AtomicU64>,
    /// オーディオスレッドへの制御要求(出力デバイスの切り替え)
    ctl: mpsc::Sender<Ctl>,
    /// 使用中の出力デバイス名
    output_name: Arc<Mutex<String>>,
    /// 録音・入力テストに使う入力デバイス(None = OS の既定)
    input_device: Arc<Mutex<Option<String>>>,
    /// 入力テスト(録音せずレベルだけ見る)
    monitor: Arc<Mutex<Option<crate::record::Recording>>>,
    /// 再生ヘッドの tick 変換・シーク用に最新のテンポマップを持つ
    tempo: Arc<Mutex<TempoMap>>,
    /// ループ区間(tick)。テンポが変わったらサンプル位置を焼き直すために保持
    loop_ticks: Arc<Mutex<Option<(Tick, Tick)>>>,
    /// 差し替えた旧データの解放をオーディオスレッドで起こさないための退避場所
    graveyard: Arc<Mutex<Vec<Arc<PlaybackData>>>>,
    /// デコード済みサンプルのキャッシュ(サンプラー音源用)
    bank: Arc<Mutex<SampleBank>>,
    /// 進行中の録音(あれば)
    recording: Arc<Mutex<Option<RecordSession>>>,
    /// 接続中の MIDI 入力
    midi: Arc<Mutex<Option<MidiConnection>>>,
    /// MIDI 録音のイベント(受信コールバックが書く)と、その付帯情報
    midi_take: Arc<Mutex<Option<MidiTake>>>,
    midi_rec: Arc<Mutex<Option<MidiRecSession>>>,
    /// 最後に MIDI を受信した時刻(`Shared::epoch` からの ms + 1。0 = 未受信)
    midi_seen: Arc<AtomicU64>,
    /// ライブ演奏の送り先トラックと、index 解決用の直近のトラック順
    live_target: Arc<Mutex<Option<TrackId>>>,
    track_order: Arc<Mutex<Vec<TrackId>>>,
    /// CLAP プラグイン(トラックへの割り当てとプラグインのスレッド)
    plugins: Arc<crate::plugins::PluginManager>,
}

/// 進行中の MIDI 録音の付帯情報。
struct MidiRecSession {
    clip_start: Tick,
    metronome_auto: bool,
}

/// MIDI 録音停止の結果。
pub struct MidiRecordOutcome {
    /// クリップを置く位置(カウントイン後)
    pub clip_start: Tick,
    /// 停止位置(tick)
    pub stop_tick: Tick,
    /// 組み立てたノート(絶対 tick)
    pub notes: Vec<RecordedNote>,
}

/// 進行中の録音の付帯情報。
struct RecordSession {
    rec: crate::record::Recording,
    /// クリップを置く位置(カウントイン後)
    clip_start: Tick,
    /// 入力ストリームが動き出した時刻と、再生を始めた時刻(準備時間 = 差)
    ready_at: std::time::Instant,
    play_at: Option<std::time::Instant>,
    /// 波形の頭から捨てる秒数(カウントイン + レイテンシ補正)
    skip_secs: f64,
    /// 録音開始時にメトロノームを自動 ON にした(停止時に戻す)
    metronome_auto: bool,
}

/// 録音停止の結果。
pub struct RecordOutcome {
    pub result: crate::record::RecordResult,
    /// クリップを置く位置
    pub clip_start: Tick,
    /// 波形の頭から捨てるサンプル数(準備時間 + カウントイン + レイテンシ補正)
    pub offset_samples: u64,
}

impl EngineHandle {
    pub fn sample_rate(&self) -> f64 {
        f64::from_bits(self.sample_rate.load(Ordering::Acquire))
    }

    /// プロジェクトから再生データを構築して差し替える(UI スレッドで呼ぶ)。
    /// `project_dir` はサンプラー音源の WAV 解決に使う。
    pub fn set_project(&self, project: &Project, project_dir: &std::path::Path) {
        let data = {
            let mut bank = self.bank.lock().expect("bank lock");
            bank.sync(project, project_dir);
            bank.plugin_slots = self.plugins.sync(project, self.sample_rate());
            Arc::new(build_playback_data(project, self.sample_rate(), &bank))
        };
        let old = self.shared.data.swap(data);
        *self.tempo.lock().expect("tempo lock") = project.tempo_map.clone();
        *self.track_order.lock().expect("order lock") =
            project.tracks.iter().map(|t| t.id.clone()).collect();
        self.update_live_track();
        // テンポが変わっているかもしれないのでループ区間のサンプル位置を焼き直す
        if let Some((start, end)) = *self.loop_ticks.lock().expect("loop lock") {
            self.write_loop_samples(start, end);
        }
        let mut graveyard = self.graveyard.lock().expect("graveyard lock");
        graveyard.push(old);
        // オーディオスレッドは数ブロックで新データに移るので、少数残せば十分
        if graveyard.len() > 8 {
            let excess = graveyard.len() - 8;
            graveyard.drain(..excess);
        }
    }

    pub fn play(&self) {
        self.shared.playing.store(true, Ordering::Release);
    }

    pub fn pause(&self) {
        self.shared.playing.store(false, Ordering::Release);
    }

    /// 停止 = 一時停止 + 先頭へ。
    pub fn stop(&self) {
        self.pause();
        self.shared.seek.store(0, Ordering::Release);
        // 停止中はレンダラが seek を消費するまで pos が古いままなので、表示用に即反映
        self.shared.pos.store(0, Ordering::Release);
    }

    pub fn seek_tick(&self, tick: Tick) {
        let seconds = self.tempo.lock().expect("tempo lock").tick_to_seconds(tick);
        let sample = (seconds * self.sample_rate()) as u64;
        debug_assert_ne!(sample, NO_SEEK);
        self.shared.seek.store(sample, Ordering::Release);
        self.shared.pos.store(sample, Ordering::Release);
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }

    /// ループ区間を設定する(tick)。再生位置が終端に達すると区間頭へ戻る。
    pub fn set_loop(&self, start: Tick, end: Tick) -> Result<(), String> {
        if end <= start {
            return Err("ループ終端は始端より後にすること".to_owned());
        }
        *self.loop_ticks.lock().expect("loop lock") = Some((start, end));
        self.write_loop_samples(start, end);
        Ok(())
    }

    /// ループを解除する。
    pub fn clear_loop(&self) {
        *self.loop_ticks.lock().expect("loop lock") = None;
        // end を先に 0 にしてから start を消す(常に「無効」側へ倒れる)
        self.shared.loop_end.store(0, Ordering::Release);
        self.shared.loop_start.store(0, Ordering::Release);
    }

    /// 現在のループ区間(tick)。
    pub fn loop_region(&self) -> Option<(Tick, Tick)> {
        *self.loop_ticks.lock().expect("loop lock")
    }

    fn write_loop_samples(&self, start: Tick, end: Tick) {
        let tempo = self.tempo.lock().expect("tempo lock");
        let s = (tempo.tick_to_seconds(start) * self.sample_rate()) as u64;
        let e = (tempo.tick_to_seconds(end) * self.sample_rate()) as u64;
        drop(tempo);
        if e <= s {
            self.shared.loop_end.store(0, Ordering::Release);
            self.shared.loop_start.store(0, Ordering::Release);
            return;
        }
        // 旧区間との混合で end <= start にならないよう、end を後から書く
        self.shared.loop_start.store(s, Ordering::Release);
        self.shared.loop_end.store(e, Ordering::Release);
    }

    /// ノートを 1 音だけ試聴する(ピアノロールの編集フィードバック用)。
    /// 停止中でも鳴る。`track_index` のトラックの音源・音量・パンを使う。
    pub fn preview_note(&self, track_index: usize, pitch: u8, vel: u8, dur_ms: u16) {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed) & 0xFFFF;
        let packed = (counter.max(1) << 48)
            | (((track_index as u64) & 0xFFFF) << 32)
            | ((dur_ms as u64) << 16)
            | ((pitch as u64) << 8)
            | (vel as u64);
        self.shared.preview.store(packed, Ordering::Release);
    }

    /// 録音を開始する(既定の入力デバイス → `path` にモノラル WAV)。
    /// `count_in_ticks` ぶん先の位置にクリップを置き、その間はメトロノームで
    /// カウントインする(`metronome_on` なら録音中メトロノームを自動 ON)。
    /// `latency_secs` は出力レイテンシ補正(聴いて歌う分の遅れを前へ詰める)。
    /// 戻り値はクリップを置く位置(tick)。既に録音中ならエラー。
    /// 呼び出し側はこの直後に [`play`](Self::play) → [`mark_play_started`](Self::mark_play_started)。
    pub fn start_recording(
        &self,
        path: std::path::PathBuf,
        count_in_ticks: Tick,
        latency_secs: f64,
        metronome_on: bool,
    ) -> Result<Tick, EngineError> {
        let mut slot = self.recording.lock().expect("recording lock");
        if slot.is_some() || self.is_midi_recording() {
            return Err(EngineError::Stream("既に録音中です".into()));
        }
        let now = self.playhead_tick();
        let clip_start = now + count_in_ticks;
        let count_in_secs = {
            let tempo = self.tempo.lock().expect("tempo lock");
            tempo.tick_to_seconds(clip_start) - tempo.tick_to_seconds(now)
        };
        // 入力テスト中なら止める(同じデバイスを二重に開かない)
        if let Some(m) = self.monitor.lock().expect("monitor lock").take() {
            let _ = m.stop();
        }
        let device = self.input_device.lock().expect("input lock").clone();
        let rec = crate::record::start_input(Some(path), device.as_deref())?;
        let metronome_auto = metronome_on && !self.shared.metronome.load(Ordering::Acquire);
        if metronome_on {
            self.shared.metronome.store(true, Ordering::Release);
        }
        self.shared.recording.store(true, Ordering::Release);
        *slot = Some(RecordSession {
            rec,
            clip_start,
            ready_at: std::time::Instant::now(),
            play_at: None,
            skip_secs: count_in_secs + latency_secs,
            metronome_auto,
        });
        Ok(clip_start)
    }

    /// 再生を始めた時刻を記録する(録音開始の準備時間を波形の頭から差し引くため)。
    pub fn mark_play_started(&self) {
        if let Some(s) = self.recording.lock().expect("recording lock").as_mut() {
            s.play_at = Some(std::time::Instant::now());
        }
    }

    /// 録音を止めて WAV を確定する。
    pub fn stop_recording(&self) -> Result<RecordOutcome, EngineError> {
        let taken = self.recording.lock().expect("recording lock").take();
        self.shared.recording.store(false, Ordering::Release);
        let Some(s) = taken else {
            return Err(EngineError::Stream("録音していません".into()));
        };
        if s.metronome_auto {
            self.shared.metronome.store(false, Ordering::Release);
        }
        let result = s.rec.stop()?;
        let lead = s
            .play_at
            .map(|p| p.duration_since(s.ready_at).as_secs_f64())
            .unwrap_or(0.0);
        let offset_samples = ((lead + s.skip_secs).max(0.0) * result.sample_rate as f64) as u64;
        Ok(RecordOutcome {
            result,
            clip_start: s.clip_start,
            offset_samples,
        })
    }

    pub fn is_recording(&self) -> bool {
        self.recording.lock().expect("recording lock").is_some()
    }

    // ---- CLAP プラグイン ----

    /// トラックに載っている CLAP プラグインの今の状態(base64。プロジェクトへ保存する用)。
    /// 状態(base64)と、上書きしているパラメータの今の値を返す。
    pub fn save_plugin_state(&self, track: &TrackId) -> Result<(String, Vec<(u32, f64)>), String> {
        self.plugins.save_state(track)
    }

    /// プラグインの画面を開く。
    pub fn open_plugin_gui(&self, track: &TrackId, title: &str) -> Result<(), String> {
        self.plugins.open_gui(track, title)
    }

    pub fn close_plugin_gui(&self, track: &TrackId) {
        self.plugins.close_gui(track);
    }

    /// プラグインのスレッドからの知らせ(状態の変化・画面を閉じた)。
    pub fn take_plugin_events(&self) -> Vec<(TrackId, crate::plugins::PluginEvent)> {
        self.plugins.take_events()
    }

    /// 保存した状態をプロジェクトに書いたことを知らせる(読み込み直しを防ぐ)。
    pub fn note_plugin_state_saved(&self, track: &TrackId, state: &str, params: &[(u32, f64)]) {
        self.plugins.note_state_saved(track, state, params);
    }

    // ---- MIDI キーボード ----

    /// MIDI 入力に接続する(None = 切断)。以前の接続は切る。
    pub fn set_midi_input(&self, name: Option<String>) -> Result<(), EngineError> {
        let mut slot = self.midi.lock().expect("midi lock");
        *slot = None;
        // 押しっぱなしで切断されても音が残らないように
        self.shared.live.push(crate::midi::LiveEvent::AllOff);
        let Some(name) = name else {
            return Ok(());
        };
        let sink = MidiSink {
            shared: self.shared.clone(),
            tempo: self.tempo.clone(),
            sample_rate: self.sample_rate.clone(),
            take: self.midi_take.clone(),
            last_seen: self.midi_seen.clone(),
        };
        *slot = Some(MidiConnection::open(&name, sink).map_err(EngineError::Stream)?);
        Ok(())
    }

    /// 接続中の MIDI 入力名。
    pub fn midi_input(&self) -> Option<String> {
        self.midi
            .lock()
            .expect("midi lock")
            .as_ref()
            .map(|c| c.name.clone())
    }

    /// 最後に MIDI を受信してからの経過ミリ秒(未受信なら None)。受信ランプ用。
    pub fn midi_idle_ms(&self) -> Option<u64> {
        let seen = self.midi_seen.load(Ordering::Acquire);
        (seen > 0)
            .then(|| (self.shared.epoch.elapsed().as_millis() as u64).saturating_sub(seen - 1))
    }

    /// ライブ演奏(MIDI キーボード)の送り先トラック。None なら既定音色で鳴らす。
    /// 切り替えたら発音中の音は離す。
    pub fn set_live_target(&self, track: Option<TrackId>) {
        *self.live_target.lock().expect("live lock") = track;
        self.shared.live.push(crate::midi::LiveEvent::AllOff);
        self.update_live_track();
    }

    pub fn live_target(&self) -> Option<TrackId> {
        self.live_target.lock().expect("live lock").clone()
    }

    fn update_live_track(&self) {
        let target = self.live_target.lock().expect("live lock").clone();
        let index = target.and_then(|id| {
            self.track_order
                .lock()
                .expect("order lock")
                .iter()
                .position(|t| *t == id)
        });
        self.shared.live_track.store(
            index.map_or(LIVE_NO_TRACK, |i| (i as u32).min(LIVE_NO_TRACK - 1)),
            Ordering::Release,
        );
    }

    /// MIDI 録音を開始する。`count_in_ticks` ぶん先にクリップを置き、その間は
    /// カウントインする。戻り値はクリップを置く位置。呼び出し側はこの直後に
    /// [`play`](Self::play) する。
    pub fn start_midi_recording(
        &self,
        count_in_ticks: Tick,
        metronome_on: bool,
    ) -> Result<Tick, EngineError> {
        let mut rec = self.midi_rec.lock().expect("midi rec lock");
        if rec.is_some() || self.is_recording() {
            return Err(EngineError::Stream("既に録音中です".into()));
        }
        if self.midi_input().is_none() {
            return Err(EngineError::Stream(
                "MIDI 入力が接続されていません(設定で選んでください)".into(),
            ));
        }
        let clip_start = self.playhead_tick() + count_in_ticks;
        let metronome_auto = metronome_on && !self.shared.metronome.load(Ordering::Acquire);
        if metronome_on {
            self.shared.metronome.store(true, Ordering::Release);
        }
        self.shared.recording.store(true, Ordering::Release);
        *self.midi_take.lock().expect("take lock") = Some(MidiTake::default());
        *rec = Some(MidiRecSession {
            clip_start,
            metronome_auto,
        });
        Ok(clip_start)
    }

    /// MIDI 録音を止めてノートを組み立てる。
    pub fn stop_midi_recording(&self) -> Result<MidiRecordOutcome, EngineError> {
        let Some(s) = self.midi_rec.lock().expect("midi rec lock").take() else {
            return Err(EngineError::Stream("MIDI 録音していません".into()));
        };
        let stop_tick = {
            let sample = self.shared.audible_pos();
            let tempo = self.tempo.lock().expect("tempo lock");
            tempo.seconds_to_tick(sample / self.sample_rate())
        };
        let take = self
            .midi_take
            .lock()
            .expect("take lock")
            .take()
            .unwrap_or_default();
        self.shared.recording.store(false, Ordering::Release);
        if s.metronome_auto {
            self.shared.metronome.store(false, Ordering::Release);
        }
        Ok(MidiRecordOutcome {
            clip_start: s.clip_start,
            stop_tick,
            notes: crate::midi::pair_notes(&take.events, stop_tick.0 as f64),
        })
    }

    pub fn is_midi_recording(&self) -> bool {
        self.midi_rec.lock().expect("midi rec lock").is_some()
    }

    /// オーディオ処理の負荷統計(直近区間の平均・最大はリセットされる)。
    pub fn take_stats(&self) -> crate::render::DspStats {
        self.shared.stats.take()
    }

    // ---- オーディオデバイス ----

    /// 使用中の出力デバイス名。
    pub fn output_device(&self) -> String {
        self.output_name.lock().expect("output lock").clone()
    }

    /// 出力デバイスを切り替える(None = OS の既定)。再生位置は保つ。
    /// サンプルレートが変わりうるので、呼び出し側は続けて [`set_project`](Self::set_project)
    /// で再生データを作り直すこと。
    pub fn set_output_device(&self, name: Option<String>) -> Result<(), EngineError> {
        let (reply, rx) = mpsc::channel();
        self.ctl
            .send(Ctl::SwitchOutput { name, reply })
            .map_err(|_| EngineError::Stream("オーディオスレッドが停止しています".into()))?;
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| EngineError::Stream("出力デバイスの切り替えが応答しません".into()))?
    }

    /// 録音・入力テストに使う入力デバイス(None = OS の既定)。
    pub fn set_input_device(&self, name: Option<String>) -> Result<(), EngineError> {
        if let Some(n) = &name {
            if crate::record::find_input_device(n).is_none() {
                return Err(EngineError::Stream(format!(
                    "入力デバイスが見つかりません: {n}"
                )));
            }
        }
        *self.input_device.lock().expect("input lock") = name;
        // 入力テスト中なら新しいデバイスで開き直す
        let was = self.monitor.lock().expect("monitor lock").take();
        if let Some(m) = was {
            let _ = m.stop();
            self.set_input_monitor(true)?;
        }
        Ok(())
    }

    /// 選択中(未選択なら OS 既定)の入力デバイス名。
    pub fn input_device(&self) -> Option<String> {
        self.input_device
            .lock()
            .expect("input lock")
            .clone()
            .or_else(crate::record::default_input_name)
    }

    /// 入力テスト(録音せずに入力レベルだけ測る)の開始・停止。録音中は何もしない。
    pub fn set_input_monitor(&self, on: bool) -> Result<(), EngineError> {
        let mut slot = self.monitor.lock().expect("monitor lock");
        if !on {
            if let Some(m) = slot.take() {
                let _ = m.stop();
            }
            return Ok(());
        }
        if slot.is_some() || self.is_recording() {
            return Ok(());
        }
        let device = self.input_device.lock().expect("input lock").clone();
        *slot = Some(crate::record::start_input(None, device.as_deref())?);
        Ok(())
    }

    pub fn input_monitoring(&self) -> bool {
        self.monitor.lock().expect("monitor lock").is_some()
    }

    /// 入力レベルのピーク(dBFS)を読み出してリセットする。録音も入力テストも
    /// していなければ None。
    pub fn take_input_peak_db(&self) -> Option<f32> {
        if let Some(s) = self.recording.lock().expect("recording lock").as_ref() {
            return Some(s.rec.take_peak_db());
        }
        self.monitor
            .lock()
            .expect("monitor lock")
            .as_ref()
            .map(|m| m.take_peak_db())
    }

    /// 較正用: 楽曲を鳴らさずメトロノームだけにする。
    pub fn set_click_only(&self, on: bool) {
        self.shared.click_only.store(on, Ordering::Release);
    }

    pub fn set_metronome(&self, on: bool) {
        self.shared.metronome.store(on, Ordering::Release);
    }

    pub fn metronome(&self) -> bool {
        self.shared.metronome.load(Ordering::Acquire)
    }

    /// 再生ヘッド位置(tick)。
    pub fn playhead_tick(&self) -> Tick {
        let pos = self.shared.pos.load(Ordering::Acquire);
        let seconds = pos as f64 / self.sample_rate();
        self.tempo
            .lock()
            .expect("tempo lock")
            .seconds_to_tick(seconds)
    }
}

/// オーディオスレッドへの制御要求。
enum Ctl {
    SwitchOutput {
        name: Option<String>,
        reply: mpsc::Sender<Result<(), EngineError>>,
    },
}

/// 利用できるオーディオデバイスの一覧。
#[derive(Clone, Debug, serde::Serialize)]
pub struct DeviceList {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    pub default_output: Option<String>,
    pub default_input: Option<String>,
}

fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "unknown".into())
}

/// OS が認識しているオーディオデバイスを列挙する。
pub fn list_devices() -> DeviceList {
    let host = cpal::default_host();
    let outputs = host
        .output_devices()
        .map(|it| it.map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    let inputs = host
        .input_devices()
        .map(|it| it.map(|d| device_name(&d)).collect())
        .unwrap_or_default();
    DeviceList {
        outputs,
        inputs,
        default_output: host.default_output_device().map(|d| device_name(&d)),
        default_input: host.default_input_device().map(|d| device_name(&d)),
    }
}

/// オーディオスレッドを起動してハンドルを返す。
/// デバイスが無い環境ではエラーを返す(アプリ側は再生なしで動作を続ける)。
pub fn start_engine() -> Result<EngineHandle, EngineError> {
    let (tx, rx) = mpsc::channel::<Result<EngineHandle, EngineError>>();

    std::thread::Builder::new()
        .name("glaux-audio".into())
        .spawn(move || {
            let shared = Arc::new(Shared::new(PlaybackData {
                sample_rate: 48_000.0,
                ..PlaybackData::default()
            }));
            let (stream, sample_rate, name) = match open_stream(None, &shared) {
                Ok(v) => v,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let (ctl, ctl_rx) = mpsc::channel();
            let handle = EngineHandle {
                shared: shared.clone(),
                sample_rate: Arc::new(AtomicU64::new(sample_rate.to_bits())),
                ctl,
                output_name: Arc::new(Mutex::new(name)),
                input_device: Arc::new(Mutex::new(None)),
                monitor: Arc::new(Mutex::new(None)),
                tempo: Arc::new(Mutex::new(TempoMap::default())),
                loop_ticks: Arc::new(Mutex::new(None)),
                graveyard: Arc::new(Mutex::new(Vec::new())),
                bank: Arc::new(Mutex::new(SampleBank::default())),
                recording: Arc::new(Mutex::new(None)),
                midi: Arc::new(Mutex::new(None)),
                midi_take: Arc::new(Mutex::new(None)),
                midi_rec: Arc::new(Mutex::new(None)),
                midi_seen: Arc::new(AtomicU64::new(0)),
                live_target: Arc::new(Mutex::new(None)),
                track_order: Arc::new(Mutex::new(Vec::new())),
                plugins: Arc::new(crate::plugins::PluginManager::start(
                    shared.plugin_slots.clone(),
                )),
            };
            let _ = tx.send(Ok(handle.clone()));
            // ストリームはこのスレッドが持ち続ける(cpal::Stream は Send でない)
            let mut stream = Some(stream);
            while let Ok(req) = ctl_rx.recv() {
                match req {
                    Ctl::SwitchOutput { name, reply } => {
                        // 再生位置を保つため、新しいレンダラに現在位置へのシークを渡す
                        let pos = shared.pos.load(Ordering::Acquire);
                        drop(stream.take());
                        shared.seek.store(pos, Ordering::Release);
                        let result = match open_stream(name.as_deref(), &shared) {
                            Ok((s, sr, n)) => {
                                stream = Some(s);
                                handle.sample_rate.store(sr.to_bits(), Ordering::Release);
                                *handle.output_name.lock().expect("output lock") = n;
                                Ok(())
                            }
                            Err(e) => {
                                // 失敗したら既定デバイスに戻す
                                if let Ok((s, sr, n)) = open_stream(None, &shared) {
                                    stream = Some(s);
                                    handle.sample_rate.store(sr.to_bits(), Ordering::Release);
                                    *handle.output_name.lock().expect("output lock") = n;
                                }
                                Err(e)
                            }
                        };
                        let _ = reply.send(result);
                    }
                }
            }
        })
        .expect("audio thread spawn");

    rx.recv().unwrap_or(Err(EngineError::NoDevice))
}

/// 出力ストリームを開いて再生を始める。戻り値は (ストリーム, サンプルレート, デバイス名)。
fn open_stream(
    name: Option<&str>,
    shared: &Arc<Shared>,
) -> Result<(cpal::Stream, f64, String), EngineError> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => host
            .output_devices()
            .map_err(|e| EngineError::Stream(e.to_string()))?
            .find(|d| device_name(d) == n)
            .ok_or_else(|| EngineError::Stream(format!("出力デバイスが見つかりません: {n}")))?,
        None => host.default_output_device().ok_or(EngineError::NoDevice)?,
    };
    let config = device
        .default_output_config()
        .map_err(|e| EngineError::Stream(e.to_string()))?;
    if config.sample_format() != cpal::SampleFormat::F32 {
        return Err(EngineError::UnsupportedFormat(
            config.sample_format().to_string(),
        ));
    }
    let sample_rate = config.sample_rate() as f64;
    let channels = config.channels() as usize;
    let dev_name = device_name(&device);
    tracing::info!("オーディオ出力: {dev_name} / {sample_rate} Hz / {channels} ch");

    // バッファは大きめ(1024 フレーム ≒ 21ms @48k)を要求してスパイク耐性を稼ぐ。
    // ドライバが拒否したらデフォルトにフォールバック
    let mut stream_config = config.config();
    stream_config.buffer_size = cpal::BufferSize::Fixed(1024);
    let err_fn = |e| tracing::error!("オーディオストリームエラー: {e}");
    let stream = match device.build_output_stream(
        stream_config,
        {
            let mut r = Renderer::new(shared.clone());
            move |out: &mut [f32], _| r.process(out, channels)
        },
        err_fn,
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("バッファ 1024 での起動に失敗({e})。既定バッファで再試行");
            let mut r = Renderer::new(shared.clone());
            device
                .build_output_stream(
                    config.config(),
                    move |out: &mut [f32], _| r.process(out, channels),
                    err_fn,
                    None,
                )
                .map_err(|e| EngineError::Stream(e.to_string()))?
        }
    };
    stream
        .play()
        .map_err(|e| EngineError::Stream(e.to_string()))?;
    Ok((stream, sample_rate, dev_name))
}

// ハンドルは UI・MCP の複数スレッドで共有する(MIDI 接続を持っても Send + Sync を保つ)
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();
};
