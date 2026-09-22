//! cpal 出力ストリームと、UI 側から操作するための [`EngineHandle`]。
//!
//! `cpal::Stream` は `Send` ではないので、専用スレッドを立ててそこでストリームを
//! 生成・保持する。UI 側には Send + Sync な [`EngineHandle`] だけを渡す。

use crate::data::{build_playback_data, PlaybackData, SampleBank};
use crate::render::{Renderer, Shared, NO_SEEK};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glaux_core::{Project, TempoMap, Tick};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

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
    sample_rate: f64,
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
        self.sample_rate
    }

    /// プロジェクトから再生データを構築して差し替える(UI スレッドで呼ぶ)。
    /// `project_dir` はサンプラー音源の WAV 解決に使う。
    pub fn set_project(&self, project: &Project, project_dir: &std::path::Path) {
        let data = {
            let mut bank = self.bank.lock().expect("bank lock");
            bank.sync(project, project_dir);
            Arc::new(build_playback_data(project, self.sample_rate, &bank))
        };
        let old = self.shared.data.swap(data);
        *self.tempo.lock().expect("tempo lock") = project.tempo_map.clone();
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
        let sample = (seconds * self.sample_rate) as u64;
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
        let s = (tempo.tick_to_seconds(start) * self.sample_rate) as u64;
        let e = (tempo.tick_to_seconds(end) * self.sample_rate) as u64;
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
        if slot.is_some() {
            return Err(EngineError::Stream("既に録音中です".into()));
        }
        let now = self.playhead_tick();
        let clip_start = now + count_in_ticks;
        let count_in_secs = {
            let tempo = self.tempo.lock().expect("tempo lock");
            tempo.tick_to_seconds(clip_start) - tempo.tick_to_seconds(now)
        };
        let rec = crate::record::start_recording(path)?;
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

    /// オーディオ処理の負荷統計(直近区間の平均・最大はリセットされる)。
    pub fn take_stats(&self) -> crate::render::DspStats {
        self.shared.stats.take()
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
        let seconds = pos as f64 / self.sample_rate;
        self.tempo
            .lock()
            .expect("tempo lock")
            .seconds_to_tick(seconds)
    }
}

/// オーディオスレッドを起動してハンドルを返す。
/// デバイスが無い環境ではエラーを返す(アプリ側は再生なしで動作を続ける)。
pub fn start_engine() -> Result<EngineHandle, EngineError> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<EngineHandle, EngineError>>();

    std::thread::Builder::new()
        .name("glaux-audio".into())
        .spawn(move || {
            let result = open_stream();
            match result {
                Ok((stream, handle)) => {
                    let _ = tx.send(Ok(handle));
                    // ストリームを生かしたままスレッドを維持する
                    if let Err(e) = stream.play() {
                        tracing::error!("ストリーム開始に失敗: {e}");
                        return;
                    }
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        })
        .expect("audio thread spawn");

    rx.recv().unwrap_or(Err(EngineError::NoDevice))
}

fn open_stream() -> Result<(cpal::Stream, EngineHandle), EngineError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(EngineError::NoDevice)?;
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
    let device_name = device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "unknown".into());
    tracing::info!("オーディオ出力: {device_name} / {sample_rate} Hz / {channels} ch");

    let shared = Arc::new(Shared::new(PlaybackData {
        sample_rate,
        ..PlaybackData::default()
    }));
    let mut renderer = Renderer::new(shared.clone());

    // バッファは大きめ(1024 フレーム ≒ 21ms @48k)を要求してスパイク耐性を稼ぐ。
    // ドライバが拒否したらデフォルトにフォールバック
    let mut stream_config = config.config();
    stream_config.buffer_size = cpal::BufferSize::Fixed(1024);
    let err_fn = |e| tracing::error!("オーディオストリームエラー: {e}");
    let stream = match device.build_output_stream(
        stream_config,
        {
            let shared = shared.clone();
            let mut r = Renderer::new(shared);
            move |out: &mut [f32], _| r.process(out, channels)
        },
        err_fn,
        None,
    ) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("バッファ 1024 での起動に失敗({e})。既定バッファで再試行");
            device
                .build_output_stream(
                    config.config(),
                    move |out: &mut [f32], _| renderer.process(out, channels),
                    err_fn,
                    None,
                )
                .map_err(|e| EngineError::Stream(e.to_string()))?
        }
    };

    let handle = EngineHandle {
        shared,
        sample_rate,
        tempo: Arc::new(Mutex::new(TempoMap::default())),
        loop_ticks: Arc::new(Mutex::new(None)),
        graveyard: Arc::new(Mutex::new(Vec::new())),
        bank: Arc::new(Mutex::new(SampleBank::default())),
        recording: Arc::new(Mutex::new(None)),
    };
    Ok((stream, handle))
}
