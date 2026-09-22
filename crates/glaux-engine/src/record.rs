//! 録音: オーディオ入力デバイス → モノラル WAV。
//!
//! 入力コールバック(オーディオスレッド)はロックフリーのリングバッファに
//! 書くだけで、ファイル書き込みは専用スレッドが行う(CLAUDE.md の RT 条件)。
//! 入力ストリームは録音スレッドが所有し、停止要求で drop → WAV を確定して
//! 結果を返す。cpal の Stream はスレッドをまたげないので、生成から破棄まで
//! 同じスレッドで完結させている。

use crate::output::EngineError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// リングバッファ容量(サンプル)。48kHz で約 21 秒ぶん。書き込みスレッドは
/// 20ms ごとに空けるので、これが溢れるのは書き込み側が固まったときだけ
const RING_CAP: usize = 1 << 20;

/// SPSC リングバッファ(f32 をビット列として AtomicU32 に入れる。unsafe なし)。
pub struct Ring {
    buf: Box<[AtomicU32]>,
    /// 書き込んだ総サンプル数(producer が進める)
    head: AtomicUsize,
    /// 読み出した総サンプル数(consumer が進める)
    tail: AtomicUsize,
    /// 溢れて捨てたサンプル数
    dropped: AtomicU64,
    /// 直近のピーク(絶対値、f32 のビット列)。正の f32 はビット列の大小と値の大小が
    /// 一致するので fetch_max で最大を取れる
    peak_bits: AtomicU32,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Ring {
            buf: (0..cap).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            peak_bits: AtomicU32::new(0),
        }
    }

    /// ピークを記録する(オーディオスレッド)。
    pub fn note_peak(&self, v: f32) {
        self.peak_bits
            .fetch_max(v.abs().to_bits(), Ordering::Relaxed);
    }

    /// ピーク(dBFS)を読み出してリセットする。
    pub fn take_peak_db(&self) -> f32 {
        let p = f32::from_bits(self.peak_bits.swap(0, Ordering::AcqRel));
        20.0 * p.max(1e-6).log10()
    }

    /// producer(オーディオスレッド)。満杯なら捨てて false。
    pub fn push(&self, v: f32) -> bool {
        let cap = self.buf.len();
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h - t >= cap {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.buf[h % cap].store(v.to_bits(), Ordering::Relaxed);
        self.head.store(h + 1, Ordering::Release);
        true
    }

    /// consumer(書き込みスレッド)。溜まっている分を全部 `out` に移す。
    pub fn drain_into(&self, out: &mut Vec<f32>) {
        let cap = self.buf.len();
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        for i in t..h {
            out.push(f32::from_bits(self.buf[i % cap].load(Ordering::Relaxed)));
        }
        self.tail.store(h, Ordering::Release);
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// 録音結果。
#[derive(Clone, Debug)]
pub struct RecordResult {
    pub path: PathBuf,
    pub frames: u64,
    pub sample_rate: u32,
    /// 入力がフルスケールを超えた(WAV 上でクリップした)サンプル数
    pub clipped: u64,
    /// リングバッファ溢れで失ったサンプル数(0 が正常)
    pub dropped: u64,
}

/// 進行中の録音(または入力テスト)。`stop` で確定する。
pub struct Recording {
    stop: Arc<AtomicBool>,
    done: mpsc::Receiver<Result<RecordResult, EngineError>>,
    pub path: Option<PathBuf>,
    pub sample_rate: u32,
    ring: Arc<Ring>,
}

impl Recording {
    /// 入力レベルのピーク(dBFS)を読み出してリセットする。
    pub fn take_peak_db(&self) -> f32 {
        self.ring.take_peak_db()
    }

    /// 録音を止めて WAV を確定する。
    pub fn stop(self) -> Result<RecordResult, EngineError> {
        self.stop.store(true, Ordering::Release);
        self.done
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| EngineError::Stream("録音スレッドが応答しません".into()))?
    }
}

fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| "unknown".into())
}

/// 名前で入力デバイスを探す。
pub fn find_input_device(name: &str) -> Option<cpal::Device> {
    cpal::default_host()
        .input_devices()
        .ok()?
        .find(|d| device_name(d) == name)
}

/// OS 既定の入力デバイス名。
pub fn default_input_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .map(|d| device_name(&d))
}

/// 入力を開始する。`path` があればモノラル 16bit WAV に録音し、None なら
/// レベル測定だけ(入力テスト)。`device` は入力デバイス名(None で OS 既定)。
pub fn start_input(path: Option<PathBuf>, device: Option<&str>) -> Result<Recording, EngineError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, Arc<Ring>), EngineError>>();
    let stop_flag = stop.clone();
    let out_path = path.clone();
    let device = device.map(str::to_owned);

    std::thread::Builder::new()
        .name("glaux-record".into())
        .spawn(move || {
            let (stream, sample_rate, ring) = match open_input(device.as_deref()) {
                Ok(v) => v,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(EngineError::Stream(e.to_string())));
                return;
            }
            let _ = ready_tx.send(Ok((sample_rate, ring.clone())));

            let result = match &out_path {
                Some(p) => write_loop(p, sample_rate, &ring, &stop_flag),
                None => discard_loop(sample_rate, &ring, &stop_flag),
            };
            drop(stream);
            let _ = done_tx.send(result);
        })
        .map_err(|e| EngineError::Stream(e.to_string()))?;

    let (sample_rate, ring) = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| EngineError::Stream("入力デバイスの初期化がタイムアウト".into()))??;
    Ok(Recording {
        stop,
        done: done_rx,
        path,
        sample_rate,
        ring,
    })
}

/// 入力テスト用: リングを空け続けるだけ(ピークはコールバック側で記録済み)。
fn discard_loop(
    sample_rate: u32,
    ring: &Ring,
    stop: &AtomicBool,
) -> Result<RecordResult, EngineError> {
    let mut chunk = Vec::with_capacity(8192);
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(20));
        ring.drain_into(&mut chunk);
        chunk.clear();
    }
    Ok(RecordResult {
        path: PathBuf::new(),
        frames: 0,
        sample_rate,
        clipped: 0,
        dropped: ring.dropped(),
    })
}

fn open_input(name: Option<&str>) -> Result<(cpal::Stream, u32, Arc<Ring>), EngineError> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => find_input_device(n)
            .ok_or_else(|| EngineError::Stream(format!("入力デバイスが見つかりません: {n}")))?,
        None => host.default_input_device().ok_or(EngineError::NoDevice)?,
    };
    let config = device
        .default_input_config()
        .map_err(|e| EngineError::Stream(e.to_string()))?;
    let sample_rate = config.sample_rate();
    let channels = config.channels().max(1) as usize;
    let ring = Arc::new(Ring::new(RING_CAP));
    let err_fn = |e| tracing::error!("録音ストリームエラー: {e}");
    let stream_config = config.config();
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let ring = ring.clone();
            device.build_input_stream(
                stream_config,
                move |data: &[f32], _| push_mono(&ring, data, channels, |v| v),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let ring = ring.clone();
            device.build_input_stream(
                stream_config,
                move |data: &[i16], _| push_mono(&ring, data, channels, |v| v as f32 / 32768.0),
                err_fn,
                None,
            )
        }
        other => return Err(EngineError::UnsupportedFormat(other.to_string())),
    }
    .map_err(|e| EngineError::Stream(e.to_string()))?;
    let name = device_name(&device);
    tracing::info!("録音入力: {name} / {sample_rate} Hz / {channels} ch");
    Ok((stream, sample_rate, ring))
}

/// 入力フレームをモノラル化してリングへ(オーディオスレッド。アロケーションなし)。
fn push_mono<T: Copy>(ring: &Ring, data: &[T], channels: usize, conv: impl Fn(T) -> f32) {
    let mut peak = 0.0f32;
    for frame in data.chunks(channels) {
        let sum: f32 = frame.iter().map(|v| conv(*v)).sum();
        let v = sum / channels as f32;
        peak = peak.max(v.abs());
        ring.push(v);
    }
    ring.note_peak(peak);
}

/// 停止要求まで 20ms ごとにリングを WAV へ書き出す(書き込みスレッド)。
fn write_loop(
    path: &PathBuf,
    sample_rate: u32,
    ring: &Ring,
    stop: &AtomicBool,
) -> Result<RecordResult, EngineError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| EngineError::Stream(e.to_string()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).map_err(|e| EngineError::Stream(e.to_string()))?;
    let mut chunk: Vec<f32> = Vec::with_capacity(8192);
    let mut frames = 0u64;
    let mut clipped = 0u64;
    {
        let mut flush = |chunk: &mut Vec<f32>| -> Result<(), EngineError> {
            for &v in chunk.iter() {
                if v.abs() > 1.0 {
                    clipped += 1;
                }
                let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
                writer
                    .write_sample(s)
                    .map_err(|e| EngineError::Stream(e.to_string()))?;
            }
            frames += chunk.len() as u64;
            chunk.clear();
            Ok(())
        };
        loop {
            std::thread::sleep(Duration::from_millis(20));
            ring.drain_into(&mut chunk);
            flush(&mut chunk)?;
            if stop.load(Ordering::Acquire) {
                // 停止後にまだ入力が残っているかもしれないので、もう一度だけ回収
                std::thread::sleep(Duration::from_millis(30));
                ring.drain_into(&mut chunk);
                flush(&mut chunk)?;
                break;
            }
        }
    }
    writer
        .finalize()
        .map_err(|e| EngineError::Stream(e.to_string()))?;
    Ok(RecordResult {
        path: path.clone(),
        frames,
        sample_rate,
        clipped,
        dropped: ring.dropped(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_roundtrips_and_wraps() {
        let ring = Ring::new(8);
        for i in 0..6 {
            assert!(ring.push(i as f32));
        }
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        // 容量をまたいで書き続けても順序が保たれる
        for i in 6..12 {
            assert!(ring.push(i as f32));
        }
        out.clear();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
        // 満杯なら捨てる
        for i in 0..8 {
            assert!(ring.push(i as f32));
        }
        assert!(!ring.push(99.0));
        assert_eq!(ring.dropped(), 1);
    }

    #[test]
    fn write_loop_produces_readable_wav_and_counts_clipping() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rec/take.wav");
        let ring = Arc::new(Ring::new(1024));
        let stop = Arc::new(AtomicBool::new(false));
        // 入力の代わりに 300 サンプル(うち 2 つはクリップ)を積んでから停止
        for i in 0..300 {
            let v = if i == 10 || i == 20 { 1.5 } else { 0.25 };
            ring.push(v);
        }
        stop.store(true, Ordering::Release);
        let r = write_loop(&path, 48_000, &ring, &stop).unwrap();
        assert_eq!(r.frames, 300);
        assert_eq!(r.clipped, 2);
        assert_eq!(r.dropped, 0);
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.duration(), 300);
    }

    #[test]
    fn peak_is_tracked_and_reset() {
        let ring = Ring::new(64);
        push_mono(&ring, &[0.1f32, -0.5, 0.25], 1, |v| v);
        let db = ring.take_peak_db();
        assert!((db - 20.0 * 0.5f32.log10()).abs() < 0.01, "{db}");
        // 読み出すとリセットされる
        assert!(ring.take_peak_db() < -100.0);
    }

    #[test]
    fn push_mono_downmixes_channels() {
        let ring = Ring::new(16);
        push_mono(&ring, &[1.0f32, 0.0, 0.5, 0.5], 2, |v| v);
        let mut out = Vec::new();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![0.5, 0.5]);
        push_mono(&ring, &[16384i16, -16384], 1, |v| v as f32 / 32768.0);
        out.clear();
        ring.drain_into(&mut out);
        assert_eq!(out, vec![0.5, -0.5]);
    }
}
