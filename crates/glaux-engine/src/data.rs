//! `Project` からエンジン用の再生データを組み立てる。
//!
//! - **UI(非オーディオ)スレッドで**構築し、`ArcSwap` でオーディオスレッドに渡す。
//!   オーディオスレッドはこのデータを読むだけで、一切アロケーションしない。
//! - ノートは絶対サンプル位置に展開してソート済み。テンポは構築時に焼き込む
//!   (テンポ変更があればデータごと作り直す)。再生位置はサンプルで保持するが、
//!   テンポ区間表([`TempoSeg`])も焼き込んであり、レンダラは差し替え時に
//!   音楽的位置(tick)を保ったままサンプル位置を換算し直す。

use glaux_core::{AssetId, ClipContent, Curve, Effect, ParamPath, Project, Tick};
use glaux_dsp::{EffectParams, InstrumentParams, SampleData};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// エフェクト状態プールのスロット数(エンジン起動時に固定確保)。
/// これを超えたエフェクトは無視される(構築時に警告)。
pub const MAX_EFFECT_SLOTS: usize = 64;
/// エフェクト・ミックス処理の対象になる最大トラック数。
/// 超過分のトラックはエフェクトなしで直接マスターへ送られる。
pub const MAX_TRACKS: usize = 64;

/// 焼き込み済みエフェクト + 状態プールのスロット番号。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakedEffect {
    /// CLAP エフェクトは `EffectParams::External`
    pub params: EffectParams,
    pub slot: u32,
    /// CLAP エフェクト: プラグインの (スロット, 世代)。ブロック単位で処理する
    pub plugin: Option<(u32, u64)>,
}

/// サンプル位置に焼き込んだオートメーション点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoPoint {
    pub sample: u64,
    pub value: f32,
    pub curve: Curve,
}

/// 1 ノート分の再生イベント。サンプル位置は曲頭からの絶対値。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    pub start: u64,
    pub end: u64,
    /// 周波数(Hz)。ピッチから変換済み
    pub freq: f32,
    /// MIDI ノート番号(ドラムシンセの音色選択に使う)
    pub pitch: u8,
    /// ベロシティ由来の振幅 0.0..=1.0
    pub amp: f32,
    /// `PlaybackData::tracks` への添字
    pub track: u32,
    /// 奏法(パームミュート等)。ボイス起動時に音色へ反映する
    pub articulation: glaux_core::Articulation,
    /// 連続ピッチカーブ(ノート先頭からのサンプル数, セント)。空なら無し
    pub curve: glaux_dsp::PitchCurve,
    /// レガートのつなぎ: 鳴り始めをこのサンプル数かけて立ち上げる(0 = そのまま)
    pub fade_in: u32,
    /// レガートのつなぎ: end で離さずにこのサンプル数かけて消す(0 = 通常のリリース)。
    /// CLAP 音源へは end + fade_out で離す(次の音と重ねて送り、プラグインのレガートを効かせる)
    pub fade_out: u32,
    /// ノート個別のポルタメントの滑る時間(秒。0 ならトラックの設定)
    pub glide: f32,
    /// レガート・ポルタメント: 同じトラックで先に離された音の余韻を、このサンプル数で消す(0 = 消さない)。
    /// 押さえたままの音(和音の伴奏など)には触れない
    pub choke: u32,
}

/// トラックごとのつなぎの設定(秒)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegatoSettings {
    /// つなぎ目の長さ
    pub xfade: f64,
    /// ポルタメントで滑る時間
    pub glide: f64,
}

impl Default for LegatoSettings {
    fn default() -> Self {
        LegatoSettings {
            xfade: LEGATO_XFADE_SEC,
            glide: PORTAMENTO_SEC,
        }
    }
}

/// レガートのつなぎ目の長さの既定(秒)。前の音が消え、次の音が立ち上がる
pub const LEGATO_XFADE_SEC: f64 = 0.03;
/// レガートでつなぐ直前の音を探す範囲(前の音の終わりから、この秒数までの隙間なら「つながっている」)
pub const LEGATO_GAP_SEC: f64 = 0.3;
/// 直前の音が無いポルタメント(フレーズの頭)は、この半音数だけ下から滑り込む
pub const PORTAMENTO_SCOOP_SEMITONES: f32 = 2.0;
/// ポルタメントで滑る時間の既定(秒。音がそれより短ければ音の終わりまでかけて滑る)
pub const PORTAMENTO_SEC: f64 = 0.15;

/// レガート・ポルタメントのノートを同じトラックの直前の音とつなぐ(`events` は開始順)。
/// 直前の音 = そのノートより前に始まり、終わりが開始の手前 `LEGATO_GAP_SEC` 以内(または次の音の
/// 前半に少し重なるまで)のもののうち、最も後に始まったもの(同時なら音程の近いもの)。
/// 次の音より長く鳴り続ける音(押さえたままの伴奏)はつながない。前の音はつなぎ目で消え、次の音は立ち上がりを消す。
/// ポルタメントはさらに前の音の高さから滑らせる(ノートに自前のピッチカーブがあればそちらを優先)。
/// 直前の音が無いポルタメントは全音下から滑り込む(レガートは直前の音が無ければ普通に鳴る)。
/// `settings(track)` がそのトラックのつなぎの設定、None のトラック(ドラム)はつながない。
/// 滑る時間はノート個別の `glide` があればそちらを使う。
pub fn link_legato(
    events: &mut [NoteEvent],
    sample_rate: f64,
    settings: &dyn Fn(u32) -> Option<LegatoSettings>,
) {
    use glaux_core::Articulation as A;
    if !events
        .iter()
        .any(|e| matches!(e.articulation, A::Legato | A::Portamento))
    {
        return;
    }
    let gap = (LEGATO_GAP_SEC * sample_rate) as u64;
    let mut by_track: std::collections::HashMap<u32, Vec<usize>> = Default::default();
    for (i, e) in events.iter().enumerate() {
        by_track.entry(e.track).or_default().push(i);
    }
    for (track, idx) in by_track {
        let Some(set) = settings(track) else {
            continue;
        };
        let xf = ((set.xfade * sample_rate) as u64).max(1);
        for (k, &i) in idx.iter().enumerate() {
            let e = events[i];
            if !matches!(e.articulation, A::Legato | A::Portamento) {
                continue;
            }
            let mut prev: Option<usize> = None;
            for &j in idx[..k].iter().rev() {
                let p = &events[j];
                // 直前の音 = 開始の手前 gap 以内に終わる音か、少しだけ重なって終わる音(次の音の半分まで)。
                // それより長く鳴り続ける音(押さえたままの伴奏など)はつながない
                let overlap_limit = e.start + (e.end - e.start) / 2;
                if p.start >= e.start || p.end + gap < e.start || p.end > overlap_limit {
                    continue;
                }
                prev = match prev {
                    None => Some(j),
                    Some(q) => {
                        let pq = &events[q];
                        let closer = (p.pitch as i32 - e.pitch as i32).abs()
                            < (pq.pitch as i32 - e.pitch as i32).abs();
                        if p.start > pq.start || (p.start == pq.start && closer) {
                            Some(j)
                        } else {
                            Some(q)
                        }
                    }
                };
                // 開始順に並んでいるので、十分前(1 分以上)まで遡ったら打ち切る
                if e.start.saturating_sub(p.start) > (60.0 * sample_rate) as u64 {
                    break;
                }
            }
            // 滑り始めの高さ: 直前の音があればその音程、無ければ(フレーズの頭のポルタメント)全音下から
            let from_pitch = match prev {
                Some(j) => {
                    // 前の音はつなぎ目から消え始め、次の音の立ち上がりと同じ長さで入れ替わる
                    // (CLAP 音源へは fade_out の分だけ離すのを遅らせ、重ねて送る)
                    events[j].end = e.start;
                    events[j].fade_out = xf as u32;
                    events[i].fade_in = xf as u32;
                    Some(events[j].pitch as f32)
                }
                None if e.articulation == A::Portamento => {
                    Some(e.pitch as f32 - PORTAMENTO_SCOOP_SEMITONES)
                }
                None => None,
            };
            let Some(from_pitch) = from_pitch else {
                continue;
            };
            // 先に離された音の余韻(リリースの長いパッド・弦など)がつながった音に重ならないよう消す
            events[i].choke = xf as u32;
            if e.articulation == A::Portamento && e.curve.is_empty() && from_pitch != e.pitch as f32
            {
                let secs = if e.glide > 0.0 {
                    e.glide as f64
                } else {
                    set.glide
                };
                let glide = ((secs * sample_rate) as u64).min(e.end - e.start).max(1) as f32;
                let cents = (from_pitch - e.pitch as f32) * 100.0;
                // 減速しながら到達する形(前半で 7 割進む)
                events[i].curve = glaux_dsp::PitchCurve::from_points(&[
                    (0.0, cents),
                    (glide * 0.4, cents * 0.3),
                    (glide, 0.0),
                ]);
            }
        }
    }
}

/// 音声クリップ 1 つ分の再生イベント。サンプル位置は曲頭からの絶対値。
#[derive(Clone, Debug)]
pub struct AudioEvent {
    pub start: u64,
    pub end: u64,
    /// `PlaybackData::tracks` への添字
    pub track: u32,
    /// モノラル化済みの波形(ネイティブレート)
    pub data: Arc<SampleData>,
    /// 波形内の再生開始位置(ネイティブレートのフレーム)
    pub offset: f64,
    /// 1 出力サンプルあたりの進み(ネイティブレート / エンジンレート)
    pub rate: f64,
    /// リニアゲイン(clip.gain_db 由来)
    pub gain: f32,
    /// フェードイン / アウト(出力サンプル数)
    pub fade_in: u64,
    pub fade_out: u64,
}

impl PartialEq for AudioEvent {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.end == other.end
            && self.track == other.track
            && Arc::ptr_eq(&self.data, &other.data)
            && self.offset == other.offset
    }
}

/// トラックのミックス設定(音量・パン・mute/solo を反映済み)。
#[derive(Clone, Debug, PartialEq)]
pub struct TrackMix {
    pub gain_l: f32,
    pub gain_r: f32,
    /// mute / solo 判定の結果。false なら発音しない
    pub audible: bool,
    /// 静的な音量(リニア)とパン。オートメーションと組み合わせるときに使う
    pub base_amp: f32,
    pub base_pan: f32,
    /// track/volume_db のオートメーション(dB 値)。空ならフェーダー値を使う。
    /// レーンがあるときは**フェーダーより優先**(一般的な DAW と同じ)
    pub vol_db_auto: Vec<AutoPoint>,
    /// track/pan のオートメーション(-1..1)。空ならフェーダー値を使う
    pub pan_auto: Vec<AutoPoint>,
    /// device/<param> のオートメーション(raw 値)。ブロックレートで
    /// `InstrumentParams::set_continuous` に流し込む(フィルタスイープ等)
    pub device_auto: Vec<(String, Vec<AutoPoint>)>,
    /// fx/<id>/<param> のオートメーション(エフェクトの状態スロット, パラメータ名, 点列)。
    /// ブロックレートで `EffectParams::set_continuous` に流し込む
    pub fx_auto: Vec<(u32, String, Vec<AutoPoint>)>,
    /// 焼き込み済みの楽器パラメータ(glaux-dsp)
    pub instrument: InstrumentParams,
    /// エフェクトチェーン(bypass 除外・焼き込み済み)。楽器 → チェーン → 音量/パン の順
    pub effects: Vec<BakedEffect>,
    /// CLAP プラグインで鳴らすトラック: (スロット, 世代)。内蔵楽器の代わりにプラグインへ
    /// ノートを送り、その出力(ステレオ)をエフェクト → 音量/パンに通す
    pub plugin: Option<(u32, u64)>,
    /// CLAP プラグインのパラメータのオートメーション(`device/clap:<id>`、プラグインの単位)
    pub plugin_auto: Vec<(u32, Vec<AutoPoint>)>,
    /// バス(リターン)トラック: 自分の音は持たず、センドで受けた音をチェーン → 音量/パンに通す
    pub is_bus: bool,
    /// センド(送り先のバスの添字・量(リニア)・フェーダー前か)
    pub sends: Vec<SendMix>,
    /// ステレオの素材(音声クリップ)を含む: パンは左右バランスとして掛ける
    pub stereo: bool,
}

/// 焼き込み済みのセンド。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SendMix {
    /// `PlaybackData::tracks` への添字(バス)
    pub target: u32,
    pub amp: f32,
    pub pre_fader: bool,
}

/// テンポ区間(サンプル位置 ⇔ tick の相互変換用)。`sample` 昇順。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoSeg {
    pub sample: u64,
    pub tick: u64,
    pub samples_per_tick: f64,
}

/// オーディオスレッドが読む再生データ一式。イミュータブル。
#[derive(Clone, Debug, Default)]
pub struct PlaybackData {
    /// `start` 昇順
    pub events: Vec<NoteEvent>,
    /// 音声クリップ。`start` 昇順
    pub audio_events: Vec<AudioEvent>,
    pub tracks: Vec<TrackMix>,
    /// マスターバスのエフェクトチェーン
    pub master_effects: Vec<BakedEffect>,
    pub master_amp: f32,
    /// マスター音量のオートメーション(dB)。空ならフェーダー値(`master_amp`)
    pub master_vol_auto: Vec<AutoPoint>,
    /// マスターのエフェクトのオートメーション(スロット, パラメータ名, 点列)
    pub master_fx_auto: Vec<(u32, String, Vec<AutoPoint>)>,
    /// 最後のノートが終わるサンプル位置(自動停止に使う)
    pub end_sample: u64,
    pub sample_rate: f64,
    /// テンポマップの焼き込み。データ差し替え時に「音楽的位置(tick)」を
    /// 保ったままサンプル位置を換算し直すために使う(空なら換算しない)
    pub tempo: Vec<TempoSeg>,
    /// 拍子イベント(tick, 分子, 分母)。メトロノームの拍・小節頭の判定に使う
    pub sigs: Vec<(u64, u8, u8)>,
}

/// メトロノームの次のクリック。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    pub sample: u64,
    pub downbeat: bool,
}

impl PlaybackData {
    /// サンプル位置 → tick(小数)。区分線形。
    pub fn sample_to_tick(&self, sample: u64) -> f64 {
        let idx = self.tempo.partition_point(|s| s.sample <= sample);
        let Some(seg) = idx.checked_sub(1).and_then(|i| self.tempo.get(i)) else {
            return 0.0;
        };
        seg.tick as f64 + (sample - seg.sample) as f64 / seg.samples_per_tick
    }

    /// `pos`(サンプル)以降で最初に来る拍。テンポ・拍子が無ければ None。
    pub fn next_beat(&self, pos: u64) -> Option<Beat> {
        if self.tempo.is_empty() {
            return None;
        }
        let tick = self.sample_to_tick(pos);
        let (sig_tick, num, den) = self
            .sigs
            .iter()
            .rev()
            .find(|(t, _, _)| (*t as f64) <= tick)
            .copied()
            .or_else(|| self.sigs.first().copied())
            .unwrap_or((0, 4, 4));
        let beat_len = (3840.0 / den.max(1) as f64).max(1.0);
        let idx = ((tick - sig_tick as f64) / beat_len).floor().max(0.0);
        // 今の拍がちょうど pos 以上なら今の拍、そうでなければ次の拍
        for k in 0..2 {
            let i = idx + k as f64;
            let beat_tick = sig_tick as f64 + i * beat_len;
            let sample = self.tick_to_sample(beat_tick);
            if sample >= pos {
                return Some(Beat {
                    sample,
                    downbeat: (i as u64).is_multiple_of(num.max(1) as u64),
                });
            }
        }
        None
    }

    /// tick(小数)→ サンプル位置。
    pub fn tick_to_sample(&self, tick: f64) -> u64 {
        let idx = self.tempo.partition_point(|s| (s.tick as f64) <= tick);
        let Some(seg) = idx.checked_sub(1).and_then(|i| self.tempo.get(i)) else {
            return 0;
        };
        seg.sample + ((tick - seg.tick as f64) * seg.samples_per_tick).round() as u64
    }
}

/// 読み込み済みサンプルの置き場(サンプラー音源用)。
/// UI(非オーディオ)スレッドで構築し、`Arc` で `PlaybackData` に焼き込む。
/// 編集のたびに WAV をデコードし直さないよう、アセット ID ごとにキャッシュする。
#[derive(Clone)]
pub struct SampleBank {
    map: HashMap<AssetId, Arc<SampleData>>,
    /// SoundFont ライブラリフォルダ(既定は `sf2::default_dir()`)
    sf2_dir: PathBuf,
    /// パース済み SoundFont(ファイル名 → フォント)
    fonts: HashMap<String, Arc<rustysynth::SoundFont>>,
    /// 構築済みゾーン列((ファイル名, bank, preset) → zones)
    multis: HashMap<(String, u16, u16), Arc<Vec<glaux_dsp::Zone>>>,
    /// テンポ追従クリップの伸縮済み波形(クリップ ID → (条件のハッシュ, 波形))。
    /// 波形はクリップ先頭から末尾までで、素材のサンプルレートのまま
    stretched: HashMap<glaux_core::ClipId, (u64, Arc<SampleData>)>,
    /// CLAP プラグインの持ち主(トラック・エフェクト)→ (スロット, 世代)。[`crate::plugins`] が決める
    pub plugin_slots: HashMap<crate::plugins::PluginOwner, (u32, u64)>,
}

impl Default for SampleBank {
    fn default() -> Self {
        SampleBank {
            map: HashMap::new(),
            sf2_dir: crate::sf2::default_dir(),
            fonts: HashMap::new(),
            multis: HashMap::new(),
            stretched: HashMap::new(),
            plugin_slots: HashMap::new(),
        }
    }
}

impl SampleBank {
    pub fn get(&self, id: &AssetId) -> Option<&Arc<SampleData>> {
        self.map.get(id)
    }

    /// テンポ追従クリップの伸縮済み波形(条件が変わっていれば None)。
    pub fn get_stretched(
        &self,
        project: &Project,
        clip: &glaux_core::Clip,
    ) -> Option<&Arc<SampleData>> {
        let key = stretch_key(project, clip)?;
        self.stretched
            .get(&clip.id)
            .filter(|(k, _)| *k == key)
            .map(|(_, d)| d)
    }

    /// テスト・特殊環境用: SoundFont ライブラリフォルダを差し替える。
    pub fn with_sf2_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.sf2_dir = dir.into();
        self
    }

    pub fn get_multi(
        &self,
        soundfont: &str,
        bank: u16,
        preset: u16,
    ) -> Option<&Arc<Vec<glaux_dsp::Zone>>> {
        self.multis.get(&(soundfont.to_owned(), bank, preset))
    }

    /// プロジェクトの全アセットと SoundFont プリセットを読み込む
    /// (読み込み済みは再利用、使われなくなったものは破棄)。
    pub fn sync(&mut self, project: &Project, project_dir: &Path) {
        let sf2_dir = self.sf2_dir.clone();
        self.sync_with(
            project,
            &mut |rel| load_wav(&project_dir.join(rel)),
            &mut |file| crate::sf2::load_font(&sf2_dir.join(file)),
        );
    }

    /// [`sync`](Self::sync) の読み込み方を差し替えられる版。`wav` はプロジェクト内の相対パス
    /// (`audio/xxx.wav`)、`font` は SoundFont のファイル名を受け取る。ゲームエンジンのパック内の
    /// ファイルなど、OS のパスで開けない場所から読むときに使う。
    pub fn sync_with(
        &mut self,
        project: &Project,
        wav: &mut dyn FnMut(&str) -> Result<SampleData, String>,
        font: &mut dyn FnMut(&str) -> Result<Arc<rustysynth::SoundFont>, String>,
    ) {
        self.map.retain(|id, _| project.assets.contains_key(id));
        for (id, asset) in &project.assets {
            if self.map.contains_key(id) {
                continue;
            }
            match wav(&asset.path) {
                Ok(data) => {
                    self.map.insert(id.clone(), Arc::new(data));
                }
                Err(e) => {
                    tracing::warn!("サンプルを読み込めません({}): {e}", asset.path);
                }
            }
        }

        // テンポ追従クリップ: 条件(テンポ・位置・長さ・元テンポ等)が変わったものだけ伸縮し直す
        let mut used_clips = std::collections::HashSet::new();
        for clip in project.tracks.iter().flat_map(|t| t.clips.iter()) {
            let Some(key) = stretch_key(project, clip) else {
                continue;
            };
            used_clips.insert(clip.id.clone());
            if self.stretched.get(&clip.id).is_some_and(|(k, _)| *k == key) {
                continue;
            }
            let ClipContent::Audio {
                asset,
                offset_samples,
                stretch,
                ..
            } = &clip.content
            else {
                continue;
            };
            let Some(src) = self.map.get(asset) else {
                continue;
            };
            let data = render_follow(project, clip, src, *offset_samples, stretch);
            self.stretched
                .insert(clip.id.clone(), (key, Arc::new(data)));
        }
        self.stretched.retain(|id, _| used_clips.contains(id));

        // SoundFont: プロジェクトが参照しているプリセットのゾーンを構築
        let mut used: std::collections::HashSet<(String, u16, u16)> =
            std::collections::HashSet::new();
        for t in &project.tracks {
            if let Some(d) = &t.device {
                if let glaux_core::PluginSource::Sf2 {
                    soundfont,
                    bank,
                    preset,
                } = &d.source
                {
                    used.insert((soundfont.clone(), *bank, *preset));
                }
            }
        }
        self.multis.retain(|k, _| used.contains(k));
        let used_fonts: std::collections::HashSet<&String> =
            used.iter().map(|(f, _, _)| f).collect();
        self.fonts.retain(|f, _| used_fonts.contains(f));
        for (file, bank, preset) in used {
            if self.multis.contains_key(&(file.clone(), bank, preset)) {
                continue;
            }
            let font = match self.fonts.get(&file) {
                Some(f) => f.clone(),
                None => match font(&file) {
                    Ok(f) => {
                        self.fonts.insert(file.clone(), f.clone());
                        f
                    }
                    Err(e) => {
                        tracing::warn!("{e}");
                        continue;
                    }
                },
            };
            match crate::sf2::build_zones(&font, bank, preset) {
                Some(zones) => {
                    self.multis.insert((file.clone(), bank, preset), zones);
                }
                None => {
                    tracing::warn!(
                        "SoundFont にプリセットがありません: {file} bank={bank} preset={preset}"
                    );
                }
            }
        }
    }

    /// 使い捨て(オフラインレンダ・解析用)に全アセットを読み込む。
    pub fn load(project: &Project, project_dir: &Path) -> SampleBank {
        let mut bank = SampleBank::default();
        bank.sync(project, project_dir);
        bank
    }
}

/// テンポ追従クリップの伸縮条件のハッシュ。伸縮が要らない(追従しない、または
/// クリップ全体でテンポが元テンポと同じ)なら None。
fn stretch_key(project: &Project, clip: &glaux_core::Clip) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let ClipContent::Audio {
        asset,
        offset_samples,
        stretch: glaux_core::Stretch::Follow { original_bpm },
        ..
    } = &clip.content
    else {
        return None;
    };
    let tm = &project.tempo_map;
    let end = clip.start + clip.length;
    let same_tempo = tm.bpm_at(clip.start) == *original_bpm
        && tm
            .events()
            .iter()
            .all(|e| e.tick <= clip.start || e.tick >= end || e.bpm == *original_bpm);
    if same_tempo {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    asset.as_str().hash(&mut h);
    offset_samples.hash(&mut h);
    clip.start.0.hash(&mut h);
    clip.length.0.hash(&mut h);
    original_bpm.to_bits().hash(&mut h);
    for e in tm.events() {
        e.tick.0.hash(&mut h);
        e.bpm.to_bits().hash(&mut h);
    }
    Some(h.finish())
}

/// テンポ追従クリップを伸縮する(WSOLA、音程は保つ)。戻り値はクリップ先頭から末尾までの
/// 波形(素材のサンプルレート)。出力の各時刻 → テンポマップで tick → 元テンポで素材の位置。
fn render_follow(
    project: &Project,
    clip: &glaux_core::Clip,
    src: &SampleData,
    offset_samples: u64,
    stretch: &glaux_core::Stretch,
) -> SampleData {
    let tm = &project.tempo_map;
    let sr = src.sample_rate as f64;
    let start_sec = tm.tick_to_seconds(clip.start);
    let end_sec = tm.tick_to_seconds(clip.start + clip.length);
    let out_len = ((end_sec - start_sec) * sr).max(0.0) as usize;
    let start_tick = clip.start.0 as f64;
    let src_pos = |i: usize| {
        let rel = tm.seconds_to_tick_f64(start_sec + i as f64 / sr) - start_tick;
        offset_samples as f64 + stretch.follow_seconds(rel).unwrap_or(0.0) * sr
    };
    // ステレオは左右差成分も同じ位置で伸縮する(位置は M で決める)
    let mut channels: Vec<&[f32]> = vec![&src.frames];
    if let Some(side) = &src.side {
        channels.push(side);
    }
    let mut out = glaux_dsp::stretch::wsola_channels(&channels, src.sample_rate, out_len, src_pos);
    let side = (out.len() > 1).then(|| out.remove(1));
    let frames = out.remove(0);
    SampleData {
        frames,
        sample_rate: src.sample_rate,
        side,
    }
}

/// 波形の縮小表示用ピーク列: 区間ごとの (最小, 最大)。`buckets` 個に分ける。
pub fn wave_peaks(frames: &[f32], buckets: usize) -> Vec<(f32, f32)> {
    if frames.is_empty() || buckets == 0 {
        return vec![];
    }
    let per = frames.len() as f64 / buckets as f64;
    (0..buckets)
        .map(|b| {
            let s = (b as f64 * per) as usize;
            let e = (((b + 1) as f64 * per) as usize).clamp(s + 1, frames.len());
            frames[s..e]
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)))
        })
        .collect()
}

/// WAV をモノラル f32 に読み込む(ステレオは平均で合算)。
pub fn load_wav_mono(path: &Path) -> Result<SampleData, String> {
    let mut d = load_wav(path)?;
    d.side = None;
    Ok(d)
}

/// WAV を読む。ステレオ(2 ch)なら左右差成分(`side`)も持つ。3 ch 以上はモノラルに合算する。
pub fn load_wav(path: &Path) -> Result<SampleData, String> {
    decode_wav(hound::WavReader::open(path).map_err(|e| e.to_string())?)
}

/// バイト列の WAV を読む([`load_wav`] と同じ規則。パスで開けない場所のファイル用)。
pub fn load_wav_bytes(bytes: &[u8]) -> Result<SampleData, String> {
    decode_wav(hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?)
}

fn decode_wav<R: std::io::Read>(mut reader: hound::WavReader<R>) -> Result<SampleData, String> {
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    let sr = spec.sample_rate as f32;
    if channels == 2 {
        let (l, r): (Vec<f32>, Vec<f32>) =
            raw.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).unzip();
        // 実はモノラル(左右が同じ)なら side を持たない
        if l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1e-6) {
            return Ok(SampleData::mono(l, sr));
        }
        return Ok(SampleData::stereo(&l, &r, sr));
    }
    let frames = raw
        .chunks_exact(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect();
    Ok(SampleData::mono(frames, sr))
}

/// トラックの音源を焼き込む(サンプラーは SampleBank から波形を解決)。
fn bake_track_instrument(
    t: &glaux_core::Track,
    bank: &SampleBank,
    sample_rate: f32,
) -> InstrumentParams {
    if let Some(d) = &t.device {
        match &d.source {
            glaux_core::PluginSource::Sampler { asset } => {
                if let Some(data) = bank.get(asset) {
                    return InstrumentParams::Sampler(glaux_dsp::bake_sampler(
                        &d.params,
                        data.clone(),
                        sample_rate,
                    ));
                }
                tracing::warn!("サンプル未読込のため subtractive で代用: {asset}");
            }
            glaux_core::PluginSource::Sf2 {
                soundfont,
                bank: b,
                preset,
            } => {
                if let Some(zones) = bank.get_multi(soundfont, *b, *preset) {
                    return InstrumentParams::Sf2(glaux_dsp::bake_sf2(&d.params, zones.clone()));
                }
                tracing::warn!("SoundFont 未読込のため subtractive で代用: {soundfont}");
            }
            _ => {}
        }
    }
    glaux_dsp::bake_instrument(t.device.as_ref()).1
}

pub fn db_to_amp(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub fn pitch_to_freq(pitch: u8) -> f32 {
    440.0 * 2.0_f32.powf((pitch as f32 - 69.0) / 12.0)
}

/// 等パワーパン。pan は -1.0(L) ..= 1.0(R)。
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let t = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (t.cos(), t.sin())
}

/// エフェクトチェーンを焼き込み、状態プールのスロットを割り当てる。
/// 各エフェクトの ID も返す(オートメーションのスロット解決用)。
fn bake_chain_with_ids(
    effects: &[Effect],
    sample_rate: f32,
    next_slot: &mut u32,
    resolve_track: &dyn Fn(&str) -> Option<u32>,
    plugin_slots: &HashMap<crate::plugins::PluginOwner, (u32, u64)>,
) -> Vec<(glaux_core::FxId, BakedEffect)> {
    effects
        .iter()
        .filter(|e| !e.bypass)
        .filter_map(|e| {
            // CLAP エフェクトは用意できたものだけ(見つからないプラグインは素通し = 焼かない)
            let plugin = match &e.source {
                glaux_core::PluginSource::Clap { .. } => {
                    Some(*plugin_slots.get(&crate::plugins::PluginOwner::Effect(e.id.clone()))?)
                }
                _ => None,
            };
            let params = match plugin {
                Some(_) => EffectParams::External,
                None => glaux_dsp::bake_effect(e, sample_rate, resolve_track)?,
            };
            if *next_slot as usize >= MAX_EFFECT_SLOTS {
                tracing::warn!(
                    "エフェクトが多すぎます({MAX_EFFECT_SLOTS} 超)。{} を無視",
                    e.id
                );
                return None;
            }
            let slot = *next_slot;
            *next_slot += 1;
            Some((
                e.id.clone(),
                BakedEffect {
                    params,
                    slot,
                    plugin,
                },
            ))
        })
        .collect()
}

/// プロジェクト全体を再生データに展開する。
pub fn build_playback_data(project: &Project, sample_rate: f64, bank: &SampleBank) -> PlaybackData {
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let to_sample =
        |tick: Tick| -> u64 { (project.tempo_map.tick_to_seconds(tick) * sample_rate) as u64 };

    // サイドチェインの source(トラック ID)→ index 解決
    let resolve_track = |id: &str| -> Option<u32> {
        project
            .tracks
            .iter()
            .position(|t| t.id.as_str() == id)
            .map(|i| i as u32)
    };

    // track/volume_db・track/pan のレーンをサンプル位置に焼き込む
    let bake_lane = |t: &glaux_core::Track, name: &str| -> Vec<AutoPoint> {
        let mut points: Vec<AutoPoint> = t
            .automation
            .iter()
            .find(|lane| matches!(&lane.target, ParamPath::Track { name: n } if n == name))
            .map(|lane| {
                lane.points
                    .iter()
                    .map(|p| AutoPoint {
                        sample: to_sample(p.tick),
                        value: p.value as f32,
                        curve: p.curve,
                    })
                    .collect()
            })
            .unwrap_or_default();
        points.sort_by_key(|p| p.sample);
        points
    };

    // device/<param> のレーンをまとめて焼き込む(値は raw のまま。適用は再生時)
    let bake_device_lanes = |t: &glaux_core::Track| -> Vec<(String, Vec<AutoPoint>)> {
        t.automation
            .iter()
            .filter_map(|lane| {
                let ParamPath::Device { name } = &lane.target else {
                    return None;
                };
                // CLAP プラグインのパラメータは別に扱う(plugin_auto)
                if lane.points.is_empty() || name.starts_with("clap:") {
                    return None;
                }
                let mut points: Vec<AutoPoint> = lane
                    .points
                    .iter()
                    .map(|p| AutoPoint {
                        sample: to_sample(p.tick),
                        value: p.value as f32,
                        curve: p.curve,
                    })
                    .collect();
                points.sort_by_key(|p| p.sample);
                Some((name.clone(), points))
            })
            .collect()
    };

    // fx/<id>/<param> のレーンを、焼いたエフェクトのスロットに解決する
    // (バイパス中・未知のエフェクトのレーンは鳴らさない)
    let bake_fx_lanes = |lanes: &[glaux_core::AutomationLane],
                         chain: &[(glaux_core::FxId, BakedEffect)]|
     -> Vec<(u32, String, Vec<AutoPoint>)> {
        lanes
            .iter()
            .filter_map(|lane| {
                let ParamPath::Effect { id, name } = &lane.target else {
                    return None;
                };
                let (_, baked) = chain.iter().find(|(fid, _)| fid == id)?;
                if lane.points.is_empty() {
                    return None;
                }
                let mut points: Vec<AutoPoint> = lane
                    .points
                    .iter()
                    .map(|p| AutoPoint {
                        sample: to_sample(p.tick),
                        value: p.value as f32,
                        curve: p.curve,
                    })
                    .collect();
                points.sort_by_key(|p| p.sample);
                Some((baked.slot, name.clone(), points))
            })
            .collect()
    };

    let mut next_slot: u32 = 0;
    let mut tracks: Vec<TrackMix> = project
        .tracks
        .iter()
        .map(|t| {
            let (pl, pr) = pan_gains(t.pan);
            let gain = db_to_amp(t.volume_db);
            let instrument = bake_track_instrument(t, bank, sample_rate as f32);
            let chain = bake_chain_with_ids(
                &t.effects,
                sample_rate as f32,
                &mut next_slot,
                &resolve_track,
                &bank.plugin_slots,
            );
            let fx_auto = bake_fx_lanes(&t.automation, &chain);
            TrackMix {
                gain_l: gain * pl,
                gain_r: gain * pr,
                // バスはソロの影響を受けない(ソロにしたトラックのリバーブが消えないように)
                audible: !t.mute && (!any_solo || t.solo || t.kind == glaux_core::TrackKind::Bus),
                base_amp: gain,
                base_pan: t.pan,
                vol_db_auto: bake_lane(t, "volume_db"),
                pan_auto: bake_lane(t, "pan"),
                device_auto: bake_device_lanes(t),
                fx_auto,
                instrument,
                effects: chain.into_iter().map(|(_, b)| b).collect(),
                is_bus: t.kind == glaux_core::TrackKind::Bus,
                stereo: false,
                sends: if t.kind == glaux_core::TrackKind::Bus {
                    vec![]
                } else {
                    t.sends
                        .iter()
                        .filter_map(|snd| {
                            let target = project.tracks.iter().position(|x| {
                                x.id == snd.target && x.kind == glaux_core::TrackKind::Bus
                            })?;
                            (target < MAX_TRACKS).then_some(SendMix {
                                target: target as u32,
                                amp: db_to_amp(snd.level_db),
                                pre_fader: snd.pre_fader,
                            })
                        })
                        .collect()
                },
                plugin: bank
                    .plugin_slots
                    .get(&crate::plugins::PluginOwner::Track(t.id.clone()))
                    .copied(),
                plugin_auto: t
                    .automation
                    .iter()
                    .filter_map(|lane| {
                        let ParamPath::Device { name } = &lane.target else {
                            return None;
                        };
                        let id = crate::plugins::parse_param_key(name)?;
                        let mut points: Vec<AutoPoint> = lane
                            .points
                            .iter()
                            .map(|p| AutoPoint {
                                sample: to_sample(p.tick),
                                value: p.value as f32,
                                curve: p.curve,
                            })
                            .collect();
                        points.sort_by_key(|p| p.sample);
                        (!points.is_empty()).then_some((id, points))
                    })
                    .take(crate::render::MAX_PLUGIN_LANES)
                    .collect(),
            }
        })
        .collect();
    let master_chain = bake_chain_with_ids(
        &project.master.effects,
        sample_rate as f32,
        &mut next_slot,
        &resolve_track,
        &bank.plugin_slots,
    );
    let master_fx_auto = bake_fx_lanes(&project.master.automation, &master_chain);
    let master_vol_auto: Vec<AutoPoint> = {
        let mut points: Vec<AutoPoint> = project
            .master
            .automation
            .iter()
            .find(|l| matches!(&l.target, ParamPath::Track { name } if name == "volume_db"))
            .map(|l| {
                l.points
                    .iter()
                    .map(|p| AutoPoint {
                        sample: to_sample(p.tick),
                        value: p.value as f32,
                        curve: p.curve,
                    })
                    .collect()
            })
            .unwrap_or_default();
        points.sort_by_key(|p| p.sample);
        points
    };
    let master_effects: Vec<BakedEffect> = master_chain.into_iter().map(|(_, b)| b).collect();

    let mut events = Vec::new();
    let mut audio_events = Vec::new();
    for (ti, track) in project.tracks.iter().enumerate() {
        for clip in &track.clips {
            let ClipContent::Midi { .. } = &clip.content else {
                if let ClipContent::Audio {
                    asset,
                    offset_samples,
                    gain_db,
                    fade_in_ms,
                    fade_out_ms,
                    ..
                } = &clip.content
                {
                    let Some(data) = bank.get(asset) else {
                        tracing::warn!("音声クリップの波形が未読込のため鳴らしません: {asset}");
                        continue;
                    };
                    // テンポ追従: 伸縮済み波形をクリップ先頭から鳴らす
                    let (data, offset) = match bank.get_stretched(project, clip) {
                        Some(stretched) => (stretched, 0.0),
                        None => (data, *offset_samples as f64),
                    };
                    let start = to_sample(clip.start);
                    let end = to_sample(clip.start + clip.length).max(start + 1);
                    audio_events.push(AudioEvent {
                        start,
                        end,
                        track: ti as u32,
                        data: data.clone(),
                        offset,
                        rate: data.sample_rate as f64 / sample_rate,
                        gain: db_to_amp(*gain_db),
                        fade_in: (*fade_in_ms as f64 * 0.001 * sample_rate) as u64,
                        fade_out: (*fade_out_ms as f64 * 0.001 * sample_rate) as u64,
                    });
                }
                continue;
            };
            // クリップ外の切り捨て・ループの繰り返し展開は core の playback_notes に任せる
            for note in &clip.playback_notes() {
                let dur = note.dur;
                let start_tick = clip.start + note.pos;
                let start = to_sample(start_tick);
                let mut end = to_sample(start_tick + dur).max(start + 1);
                // スタッカートは音価の半分で切る(歯切れの表現)
                if note.articulation == glaux_core::Articulation::Staccato {
                    end = start + ((end - start) / 2).max(1);
                }
                // ピッチカーブ: 相対 tick → ノート先頭からのサンプル数(テンポ考慮)
                let curve_pts: Vec<(f32, f32)> = note
                    .pitch_curve
                    .iter()
                    .map(|p| {
                        let at = to_sample(start_tick + p.tick).saturating_sub(start) as f32;
                        (at, p.cents)
                    })
                    .collect();
                events.push(NoteEvent {
                    start,
                    end,
                    freq: pitch_to_freq(note.pitch),
                    pitch: note.pitch,
                    amp: note.vel as f32 / 127.0,
                    track: ti as u32,
                    articulation: note.articulation,
                    curve: glaux_dsp::PitchCurve::from_points(&curve_pts),
                    fade_in: 0,
                    fade_out: 0,
                    glide: note.glide_ms.map_or(0.0, |ms| ms / 1000.0),
                    choke: 0,
                });
            }
        }
    }
    events.sort_by_key(|e| e.start);
    link_legato(&mut events, sample_rate, &|t| {
        let drum = tracks
            .get(t as usize)
            .is_some_and(|m| matches!(m.instrument, glaux_dsp::InstrumentParams::Drum(_)));
        let track = project.tracks.get(t as usize)?;
        (!drum).then(|| LegatoSettings {
            xfade: track
                .legato_ms
                .map_or(LEGATO_XFADE_SEC, |ms| ms as f64 / 1000.0),
            glide: track
                .glide_ms
                .map_or(PORTAMENTO_SEC, |ms| ms as f64 / 1000.0),
        })
    });
    audio_events.sort_by_key(|e| e.start);
    for ev in &audio_events {
        if ev.data.side.is_some() {
            if let Some(t) = tracks.get_mut(ev.track as usize) {
                t.stereo = true;
            }
        }
    }
    let end_sample = events
        .iter()
        .map(|e| e.end)
        .chain(audio_events.iter().map(|e| e.end))
        .max()
        .unwrap_or(0);

    let tempo = project
        .tempo_map
        .events()
        .iter()
        .map(|ev| TempoSeg {
            sample: to_sample(ev.tick),
            tick: ev.tick.0,
            samples_per_tick: sample_rate * 60.0 / (ev.bpm * project.ppq as f64),
        })
        .collect();

    let sigs = project
        .time_sig_map
        .iter()
        .map(|e| (e.tick.0, e.num, e.den))
        .collect();

    PlaybackData {
        events,
        audio_events,
        tracks,
        master_effects,
        master_amp: db_to_amp(project.master.volume_db),
        master_vol_auto,
        master_fx_auto,
        end_sample,
        sample_rate,
        tempo,
        sigs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipId, NoteId, Track, TrackId, TrackKind};

    fn note(pos: u64, dur: u64, pitch: u8, vel: u8) -> glaux_core::Note {
        glaux_core::Note {
            articulation: Default::default(),
            pitch_curve: vec![],
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel,
            glide_ms: None,
        }
    }

    fn project_with_notes(notes: Vec<glaux_core::Note>) -> Project {
        let mut project = Project::new("t");
        let mut track = Track::new(TrackId::new(), "T1", TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
        if let ClipContent::Midi { notes: n, .. } = &mut clip.content {
            *n = notes;
        }
        track.clips.push(clip);
        project.tracks.push(track);
        project
    }

    #[test]
    fn expands_notes_to_samples_at_120bpm() {
        // 120bpm: 960 tick = 0.5s = 24000 samples @48k
        let project = project_with_notes(vec![note(960, 960, 69, 127)]);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.events.len(), 1);
        let e = &data.events[0];
        assert_eq!(e.start, 24_000);
        assert_eq!(e.end, 48_000);
        assert!((e.freq - 440.0).abs() < 1e-3);
        assert!((e.amp - 1.0).abs() < 1e-6);
        assert_eq!(data.end_sample, 48_000);
    }

    #[test]
    fn loop_clip_repeats_in_playback() {
        use glaux_core::ClipContent;
        // 1 拍ぶんのパターン(頭に 1 音)を 1 小節のクリップでループ → 4 回鳴る
        let mut project = project_with_notes(vec![note(0, 240, 60, 100)]);
        if let ClipContent::Midi {
            looped, loop_len, ..
        } = &mut project.tracks[0].clips[0].content
        {
            *looped = true;
            *loop_len = Some(Tick(960));
        }
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let starts: Vec<u64> = data.events.iter().map(|e| e.start).collect();
        // 120bpm: 960 tick = 24000 サンプル。クリップ長 3840 の中で 4 回
        assert_eq!(starts, vec![0, 24_000, 48_000, 72_000]);
    }

    #[test]
    fn staccato_halves_note_length() {
        let mut n = note(960, 960, 69, 127);
        n.articulation = glaux_core::Articulation::Staccato;
        let data = build_playback_data(&project_with_notes(vec![n]), 48_000.0, &Default::default());
        let e = &data.events[0];
        assert_eq!(e.start, 24_000);
        assert_eq!(e.end, 36_000, "音価の半分で切れるはず");
        assert_eq!(e.articulation, glaux_core::Articulation::Staccato);
    }

    #[test]
    fn truncates_notes_at_clip_end_and_sorts() {
        let project = project_with_notes(vec![
            note(3000, 5000, 60, 100), // クリップ長 3840 で切り詰め
            note(0, 480, 64, 100),
        ]);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.events.len(), 2);
        assert!(data.events[0].start <= data.events[1].start);
        // 3840 tick = 2.0s = 96000 samples が終端
        assert_eq!(data.end_sample, 96_000);
    }

    #[test]
    fn mute_and_solo_control_audibility() {
        let mut project = project_with_notes(vec![note(0, 480, 60, 100)]);
        let mut t2 = Track::new(TrackId::new(), "T2", TrackKind::Midi);
        t2.solo = true;
        project.tracks.push(t2);

        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        // T2 が solo なので T1 は聞こえない
        assert!(!data.tracks[0].audible);
        assert!(data.tracks[1].audible);

        project.tracks[1].solo = false;
        project.tracks[0].mute = true;
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert!(!data.tracks[0].audible);
        assert!(data.tracks[1].audible);
    }

    #[test]
    fn bakes_effect_chains_with_slots() {
        use glaux_core::{Effect, FxId};
        let mut project = project_with_notes(vec![note(0, 480, 60, 100)]);
        let mut rv = Effect::builtin(FxId::new(), "reverb");
        rv.params.insert("mix".into(), 0.5.into());
        let mut bypassed = Effect::builtin(FxId::new(), "eq");
        bypassed.bypass = true;
        project.tracks[0].effects = vec![rv, bypassed];
        project
            .master
            .effects
            .push(Effect::builtin(FxId::new(), "compressor"));

        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        // bypass は除外され、スロットは通しで振られる
        assert_eq!(data.tracks[0].effects.len(), 1);
        assert_eq!(data.tracks[0].effects[0].slot, 0);
        assert_eq!(data.master_effects.len(), 1);
        assert_eq!(data.master_effects[0].slot, 1);
    }

    #[test]
    fn reverb_effect_extends_render_tail() {
        use crate::export::render_project;
        use glaux_core::{Effect, FxId};
        let dry_project = project_with_notes(vec![note(0, 480, 72, 100)]);
        let mut wet_project = dry_project.clone();
        let mut rv = Effect::builtin(FxId::new(), "reverb");
        rv.params.insert("mix".into(), 0.6.into());
        rv.params.insert("size".into(), 0.9.into());
        wet_project.tracks[0].effects.push(rv);

        let dry = render_project(&dry_project, 48_000.0, &Default::default()).unwrap();
        let wet = render_project(&wet_project, 48_000.0, &Default::default()).unwrap();
        assert!(
            wet.len() > dry.len(),
            "残響で音の長さが伸びるはず({} vs {})",
            wet.len(),
            dry.len()
        );
    }

    #[test]
    fn fm_instrument_renders_and_inharmonic_ratio_sounds_metallic() {
        use crate::export::render_project;
        let render = |ratio: f64| {
            let mut p = project_with_notes(vec![note(0, 1920, 60, 110)]);
            let mut d = glaux_core::Device::builtin("fm");
            d.params.insert("ratio".into(), ratio.into());
            d.params.insert("index".into(), 4.0.into());
            d.params.insert("index_sustain".into(), 1.0.into());
            d.params.insert("sustain".into(), 1.0.into());
            p.tracks[0].device = Some(d);
            let st = render_project(&p, 48_000.0, &Default::default()).unwrap();
            let mono: Vec<f32> = st.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
            mono
        };
        let harmonic = render(1.0);
        let metallic = render(3.5);
        let peak = harmonic.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.02, "鳴る: {peak}");
        let dh = crate::timbre::describe(&harmonic[..48_000], 48_000.0, None);
        let dm = crate::timbre::describe(&metallic[..48_000], 48_000.0, None);
        let (ih, im) = (
            dh.harmonics
                .as_ref()
                .map(|h| h.inharmonicity)
                .unwrap_or(0.0),
            dm.harmonics
                .as_ref()
                .map(|h| h.inharmonicity)
                .unwrap_or(1.0),
        );
        eprintln!("非調和性: 比 1 = {ih:.3} / 比 3.5 = {im:.3}");
        assert!(im > ih, "非整数比の方が非調和: {im} vs {ih}");
    }

    fn with_art(mut n: glaux_core::Note, a: glaux_core::Articulation) -> glaux_core::Note {
        n.articulation = a;
        n
    }

    #[test]
    fn legato_links_to_the_previous_note_on_the_same_track() {
        use glaux_core::Articulation as A;
        let project = project_with_notes(vec![
            note(0, 1920, 60, 100),
            with_art(note(1920, 960, 64, 100), A::Portamento),
            // 0.3 秒より大きく空いた音はつながない
            with_art(note(3840 - 480, 480, 67, 100), A::Legato),
        ]);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let e = &data.events;
        assert_eq!(e.len(), 3);
        let xf = (LEGATO_XFADE_SEC * 48_000.0) as u64;
        assert_eq!(e[0].end, e[1].start, "前の音はつなぎ目から消える");
        assert_eq!(e[0].fade_out as u64, xf);
        assert_eq!(e[1].fade_in as u64, xf);
        // ポルタメント: 4 半音下(-400 セント)から滑る
        assert_eq!(e[1].curve.cents_at(0.0), -400.0);
        assert_eq!(e[1].curve.cents_at(48_000.0), 0.0);
        // 2 つ目(1.5 秒で終わる)と 3 つ目(1.75 秒から)は 0.25 秒差なのでつながる
        assert_eq!(e[2].fade_in as u64, xf);
        assert!(e[2].curve.is_empty(), "レガートは音程を滑らせない");
    }

    #[test]
    fn portamento_without_a_previous_note_scoops_from_a_whole_step_below() {
        use glaux_core::Articulation as A;
        let project = project_with_notes(vec![
            // フレーズの頭のポルタメント(直前の音なし)
            with_art(note(0, 960, 64, 100), A::Portamento),
            // 直前の音から 0.3 秒より離れたポルタメントも直前の音なし扱い
            with_art(note(2880, 960, 67, 100), A::Portamento),
            // 直前の音が無いレガートは普通に鳴る
            // (1.0〜1.125 秒。前後とも 0.3 秒より離れている)
            with_art(note(1920, 240, 60, 100), A::Legato),
        ]);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let e = &data.events;
        let head = e.iter().find(|x| x.pitch == 64).unwrap();
        assert_eq!(head.curve.cents_at(0.0), -200.0, "全音下から");
        assert_eq!(head.curve.cents_at(7200.0), 0.0, "既定 0.15 秒で到達");
        assert_eq!(head.fade_in, 0, "フレーズの頭は普通に立ち上がる");
        let late = e.iter().find(|x| x.pitch == 67).unwrap();
        assert_eq!(late.curve.cents_at(0.0), -200.0);
        let leg = e.iter().find(|x| x.pitch == 60).unwrap();
        assert!(leg.curve.is_empty());
        assert_eq!(leg.fade_in, 0);
    }

    #[test]
    fn portamento_cuts_release_tails_but_keeps_held_notes() {
        use crate::export::render_project;
        use glaux_core::Articulation as A;
        // リリース 2 秒のサイン波。C4 を 0.25 秒弾いて離し、1 秒から G4 をポルタメント(直前の音なし → 全音下から)。
        // 伴奏の C3 は 0〜2 秒ずっと押さえたまま
        let render = |a: A| {
            let mut p = project_with_notes(vec![
                note(0, 480, 60, 100),
                note(0, 3840, 48, 100),
                with_art(note(1920, 960, 67, 100), a),
            ]);
            let mut d = glaux_core::Device::builtin("subtractive");
            d.params.insert(
                "waveform".into(),
                glaux_core::ParamValue::Enum("sine".into()),
            );
            d.params.insert("sustain".into(), 1.0.into());
            d.params.insert("release".into(), 2.0.into());
            p.tracks[0].device = Some(d);
            let st = render_project(&p, 48_000.0, &Default::default()).unwrap();
            st.chunks(2).map(|c| c[0] + c[1]).collect::<Vec<f32>>()
        };
        let bin = |x: &[f32], f: f32| {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in x.iter().enumerate() {
                let w = std::f64::consts::TAU * f as f64 * i as f64 / 48_000.0;
                re += *v as f64 * w.cos();
                im += *v as f64 * w.sin();
            }
            ((re * re + im * im).sqrt() * 2.0 / x.len() as f64) as f32
        };
        // 1.1〜1.3 秒(G4 が鳴っている間)
        let win = |x: &[f32]| x[52_800..62_400].to_vec();
        let normal = win(&render(A::Normal));
        let porta = win(&render(A::Portamento));
        let (tail_n, tail_p) = (bin(&normal, 261.6), bin(&porta, 261.6));
        let (held_n, held_p) = (bin(&normal, 130.8), bin(&porta, 130.8));
        eprintln!("C4 の余韻: 通常 {tail_n:.4} → ポルタメント {tail_p:.4} / 伴奏 C3: {held_n:.4} → {held_p:.4}");
        assert!(tail_n > 0.05, "通常は余韻が残る: {tail_n}");
        assert!(
            tail_p < tail_n * 0.05,
            "ポルタメントでは余韻が消える: {tail_p}"
        );
        assert!(
            (held_p - held_n).abs() < held_n * 0.05,
            "押さえたままの伴奏は変わらない"
        );
    }

    #[test]
    fn legato_settings_come_from_the_track_and_the_note() {
        use glaux_core::Articulation as A;
        let mut slow = with_art(note(1920, 960, 64, 100), A::Portamento);
        slow.glide_ms = Some(400.0);
        let mut project = project_with_notes(vec![
            note(0, 960, 60, 100),
            with_art(note(960, 960, 62, 100), A::Portamento),
            slow,
        ]);
        project.tracks[0].glide_ms = Some(80.0);
        project.tracks[0].legato_ms = Some(60.0);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let e = &data.events;
        assert_eq!(e[1].fade_in, 2880, "つなぎ目 60ms");
        // トラックの 80ms で到達(その手前ではまだ滑っている)
        assert_eq!(e[1].curve.cents_at(3840.0), 0.0);
        assert!(e[1].curve.cents_at(3000.0) < 0.0);
        // ノート個別の 400ms が優先
        assert!(e[2].curve.cents_at(3840.0) < 0.0);
        assert_eq!(e[2].curve.cents_at(19_200.0), 0.0);
    }

    /// 8 分音符(0.25 秒)の上行ポルタメントで、つなぎ目から `at` 秒後の高さ(Hz)
    fn porta_freq_at(glide_ms: f32, at: f64) -> f32 {
        use crate::export::render_project;
        use glaux_core::Articulation as A;
        let mut p = project_with_notes(vec![
            note(0, 480, 60, 100),
            with_art(note(480, 480, 72, 100), A::Portamento),
        ]);
        p.tracks[0].glide_ms = Some(glide_ms);
        let mut d = glaux_core::Device::builtin("subtractive");
        d.params.insert(
            "waveform".into(),
            glaux_core::ParamValue::Enum("sine".into()),
        );
        d.params.insert("sustain".into(), 1.0.into());
        p.tracks[0].device = Some(d);
        let st = render_project(&p, 48_000.0, &Default::default()).unwrap();
        let x: Vec<f32> = st.chunks(2).map(|c| c[0] + c[1]).collect();
        let s = 12_000 + (at * 48_000.0) as usize;
        let w = &x[s..s + 960];
        let c: Vec<usize> = (1..w.len())
            .filter(|&i| w[i - 1] < 0.0 && w[i] >= 0.0)
            .collect();
        (c.len() - 1) as f32 * 48_000.0 / (c[c.len() - 1] - c[0]) as f32
    }

    #[test]
    fn glide_time_is_audible_on_short_notes() {
        // 8 分音符でも 80ms と 400ms で滑り方がはっきり違う(以前は音の長さの半分で頭打ちだった)
        let fast = porta_freq_at(80.0, 0.12);
        let slow = porta_freq_at(400.0, 0.12);
        eprintln!("0.12 秒後: 80ms → {fast:.0}Hz / 400ms → {slow:.0}Hz(C5 = 523Hz)");
        assert!((fast - 523.3).abs() < 15.0, "{fast}");
        // 400ms は音の終わり(0.25 秒)までかけて滑るので、まだ 2 半音以上低い
        assert!(slow < 466.0, "{slow}");
    }

    #[test]
    fn legato_removes_the_attack_and_portamento_glides() {
        use crate::export::render_project;
        use glaux_core::Articulation as A;
        let render = |a: A| {
            let mut p = project_with_notes(vec![
                note(0, 1920, 60, 100),
                with_art(note(1920, 1920, 64, 100), a),
            ]);
            let mut d = glaux_core::Device::builtin("subtractive");
            d.params.insert("sustain".into(), 0.4.into());
            d.params.insert("decay".into(), 0.2.into());
            p.tracks[0].device = Some(d);
            let st = render_project(&p, 48_000.0, &Default::default()).unwrap();
            st.chunks(2).map(|c| c[0] + c[1]).collect::<Vec<f32>>()
        };
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let joint = 48_000usize;
        // つなぎ目直後の山(新しい音の立ち上がり)÷ 落ち着いた後の音量
        let bump = |x: &[f32]| {
            let peak = (0..30)
                .map(|k| rms(&x[joint + k * 480..joint + (k + 1) * 480]))
                .fold(0.0f32, f32::max);
            peak / rms(&x[joint + 38_400..joint + 43_200])
        };
        let normal = render(A::Normal);
        let legato = render(A::Legato);
        let (bn, bl) = (bump(&normal), bump(&legato));
        eprintln!("立ち上がりの山: 通常 {bn:.2} / レガート {bl:.2}");
        assert!(bn > 1.5, "通常は弾き直しの山がある: {bn}");
        assert!(bl < 1.2, "レガートは山がない: {bl}");
        // ポルタメント: つなぎ目の直後は前の音(C4 = 261.6Hz)寄り、0.3 秒後には E4(329.6Hz)
        let porta = render(A::Portamento);
        let freq = |x: &[f32]| {
            let c: Vec<usize> = (1..x.len())
                .filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0)
                .collect();
            (c.len() - 1) as f32 * 48_000.0 / (c[c.len() - 1] - c[0]) as f32
        };
        // つなぎ目の入れ替わり(30ms)の後、滑っている途中(約 30〜60ms)
        let early = freq(&porta[joint + 1500..joint + 2900]);
        let late = freq(&porta[joint + 14_400..joint + 24_000]);
        eprintln!("ポルタメント: {early:.1}Hz → {late:.1}Hz");
        assert!(early > 262.0 && early < 315.0, "{early}");
        assert!((late - 329.6).abs() < 4.0, "{late}");
    }

    #[test]
    fn wavetable_position_changes_brightness_and_lfo_moves_it() {
        use crate::export::render_project;
        let render = |position: f64, lfo_depth: f64| {
            let mut p = project_with_notes(vec![note(0, 3840, 48, 110)]);
            let mut d = glaux_core::Device::builtin("wavetable");
            d.params.insert("position".into(), position.into());
            d.params.insert("lfo_depth".into(), lfo_depth.into());
            d.params.insert("lfo_rate".into(), 2.0.into());
            p.tracks[0].device = Some(d);
            let st = render_project(&p, 48_000.0, &Default::default()).unwrap();
            let mono: Vec<f32> = st.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
            crate::timbre::describe(&mono[..96_000], 48_000.0, None)
        };
        let dark = render(0.0, 0.0);
        let bright = render(1.0, 0.0);
        eprintln!(
            "重心: {} → {}",
            dark.spectrum.centroid_hz, bright.spectrum.centroid_hz
        );
        assert!(bright.spectrum.centroid_hz > dark.spectrum.centroid_hz * 2.0);
        // LFO で揺らすと明るさが時間とともに上下する
        let spread = |d: &crate::timbre::SoundDescriptors| {
            let c = &d.spectrum.centroid_curve_hz;
            c.iter().cloned().fold(f32::MIN, f32::max) - c.iter().cloned().fold(f32::MAX, f32::min)
        };
        let still = render(0.5, 0.0);
        let wobble = render(0.5, 0.8);
        assert!(
            spread(&wobble) > spread(&still) * 2.0 + 50.0,
            "{} vs {}",
            spread(&wobble),
            spread(&still)
        );
    }

    #[test]
    fn sends_feed_a_shared_reverb_bus() {
        use crate::export::render_project;
        use glaux_core::{Effect, FxId, Send, Track, TrackId, TrackKind};
        // 短い音 + リバーブを挿したバス(リバーブ 100% wet)
        let mut project = project_with_notes(vec![note(0, 240, 72, 110)]);
        let bus_id = TrackId::new();
        let mut bus = Track::new(bus_id.clone(), "Reverb", TrackKind::Bus);
        let mut rv = Effect::builtin(FxId::new(), "reverb");
        rv.params.insert("mix".into(), 1.0.into());
        rv.params.insert("size".into(), 0.9.into());
        bus.effects.push(rv);
        project.tracks.push(bus);
        let sr = 48_000.0;
        // 音が消えた後(0.5〜1.5 秒)のエネルギー
        let tail = |x: &[f32]| -> f32 {
            x.chunks(2)
                .skip((0.5 * sr) as usize)
                .take(sr as usize)
                .map(|c| c[0] * c[0] + c[1] * c[1])
                .sum()
        };
        let render = |p: &Project| render_project(p, sr, &Default::default()).unwrap();
        let dry = render(&project);
        let with_send = |db: f32, pre: bool, vol: f32| {
            let mut p = project.clone();
            p.tracks[0].volume_db = vol;
            p.tracks[0].sends.push(Send {
                target: bus_id.clone(),
                level_db: db,
                pre_fader: pre,
            });
            p
        };
        let wet = render(&with_send(0.0, false, 0.0));
        assert!(
            tail(&wet) > tail(&dry) * 100.0 + 1e-3,
            "{} vs {}",
            tail(&wet),
            tail(&dry)
        );
        // フェーダー後のセンドは元の音量に追従: 元を -60dB にするとほぼ消える
        let post_quiet = render(&with_send(0.0, false, -60.0));
        assert!(tail(&post_quiet) < tail(&wet) * 1e-2);
        // フェーダー前なら元を絞ってもバスには同じだけ届く
        let pre_quiet = render(&with_send(0.0, true, -60.0));
        assert!(
            tail(&pre_quiet) > tail(&wet) * 0.2,
            "{} vs {}",
            tail(&pre_quiet),
            tail(&wet)
        );
        // 元のトラックをソロにしてもバスは鳴る(ソロの影響を受けない)
        let mut solo = with_send(0.0, false, 0.0);
        solo.tracks[0].solo = true;
        assert!((tail(&render(&solo)) - tail(&wet)).abs() < tail(&wet) * 0.01);
        // バスをミュートすれば消える
        let mut muted = with_send(0.0, false, 0.0);
        muted.tracks[1].mute = true;
        let m = tail(&render(&muted));
        assert!(
            (m - tail(&dry)).abs() <= tail(&dry) * 0.01 + 1e-9,
            "{m} vs {}",
            tail(&dry)
        );
    }

    #[test]
    fn volume_automation_fades_and_overrides_fader() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};

        // フェーダーは -60dB(ほぼ無音)だが、レーンが -60 → 0dB のフェードインを描く
        let mut project = project_with_notes(vec![note(0, 3840, 69, 127)]);
        project.tracks[0].volume_db = -60.0;
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::track("volume_db"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -60.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(1920), // 1 秒でフェード完了、残り 1 秒は 0dB
                    value: 0.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let rms = |sl: &[f32]| (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt();
        // ステレオ interleaved: 秒 → サンプル対
        let sec = |a: f64, b: f64| &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
        let head = rms(sec(0.0, 0.4));
        let tail = rms(sec(1.2, 1.8)); // フェード完了後・ノート内
        assert!(
            tail > head * 4.0,
            "フェードインするはず: head={head} tail={tail}"
        );
        assert!(
            tail > 0.06,
            "レーンがフェーダー(-60dB)より優先されるはず: {tail}"
        );
    }

    #[test]
    fn device_automation_lane_is_baked() {
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};
        let mut project = project_with_notes(vec![note(0, 480, 60, 100)]);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::device("cutoff"),
            points: vec![AutomationPoint {
                tick: Tick(960),
                value: 200.0,
                curve: Curve::Linear,
            }],
        });
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.tracks[0].device_auto.len(), 1);
        let (name, points) = &data.tracks[0].device_auto[0];
        assert_eq!(name, "cutoff");
        assert_eq!(points[0].sample, 24_000); // 120bpm: 960 tick = 0.5s
        assert!((points[0].value - 200.0).abs() < 1e-6);
    }

    #[test]
    fn cutoff_automation_sweeps_brightness() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};

        // 低カットオフ → 高カットオフのスイープ。後半ほど高域(隣接差分)が増える
        let mut project = project_with_notes(vec![note(0, 3840, 45, 110)]);
        let mut device = glaux_core::Device::builtin("subtractive");
        device.params.insert("sustain".into(), 1.0.into());
        device.params.insert("release".into(), 0.05.into());
        device.params.insert("filter_env".into(), 0.0.into()); // スイープはレーンだけで
        project.tracks[0].device = Some(device);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::device("cutoff"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: 150.0,
                    curve: Curve::Hold, // 1 秒間こもったまま
                },
                AutomationPoint {
                    tick: Tick(1920), // 1 秒で全開に
                    value: 9000.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        // 左 ch の「隣接差分 RMS / RMS」= 音量に依らない高域の割合で前半と後半を比較
        let brightness = |a: f64, b: f64| -> f32 {
            let s = (a * 48_000.0) as usize;
            let e = (b * 48_000.0) as usize;
            let (mut dd, mut ss) = (0.0f32, 0.0f32);
            for i in s..e {
                let d = out[(i + 1) * 2] - out[i * 2];
                dd += d * d;
                ss += out[i * 2] * out[i * 2];
            }
            (dd / ss.max(1e-12)).sqrt()
        };
        let head = brightness(0.2, 0.6);
        let tail = brightness(1.4, 1.9);
        assert!(
            tail > head * 2.0,
            "スイープで高域の割合が増えるはず: head={head} tail={tail}"
        );
    }

    #[test]
    fn effect_param_automation_changes_output() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, Effect, FxId, ParamPath};
        // 歪みの出力レベルを -24dB → +6dB に上げていく。後半ほど大きくなるはず
        let mut project = project_with_notes(vec![note(0, 3840, 57, 110)]);
        let fx_id = FxId::new();
        let mut dist = Effect::builtin(fx_id.clone(), "distortion");
        dist.params.insert("level_db".into(), (-24.0).into());
        project.tracks[0].effects.push(dist);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::effect(fx_id, "level_db"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -24.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(3840),
                    value: 6.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.tracks[0].fx_auto.len(), 1);
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let rms = |a: f64, b: f64| {
            let sl = &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
            (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt()
        };
        let head = rms(0.1, 0.4);
        let tail = rms(1.5, 1.9);
        assert!(
            tail > head * 5.0,
            "レベルが上がっていくはず: head={head} tail={tail}"
        );

        // バイパスしたエフェクトのレーンは無視される
        project.tracks[0].effects[0].bypass = true;
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert!(data.tracks[0].fx_auto.is_empty());
    }

    #[test]
    fn master_automation_fades_volume_and_moves_effect_param() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, Effect, FxId, ParamPath};
        let lin = |tick: u64, value: f64| AutomationPoint {
            tick: Tick(tick),
            value,
            curve: Curve::Linear,
        };
        // マスター音量: -60 → 0dB のフェードイン
        let mut project = project_with_notes(vec![note(0, 3840, 69, 127)]);
        project.master.automation.push(AutomationLane {
            target: ParamPath::track("volume_db"),
            points: vec![lin(0, -60.0), lin(1920, 0.0)],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let rms = |out: &[f32], a: f64, b: f64| {
            let sl = &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
            (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt()
        };
        let (head, tail) = (rms(&out, 0.0, 0.4), rms(&out, 1.2, 1.8));
        assert!(
            tail > head * 4.0,
            "マスターでフェードインするはず: {head} {tail}"
        );

        // マスターのエフェクトのパラメータ: 歪みの出力を -24 → +6dB
        let mut project = project_with_notes(vec![note(0, 3840, 57, 110)]);
        let fx_id = FxId::new();
        let mut dist = Effect::builtin(fx_id.clone(), "distortion");
        dist.params.insert("level_db".into(), (-24.0).into());
        project.master.effects.push(dist);
        project.master.automation.push(AutomationLane {
            target: ParamPath::effect(fx_id, "level_db"),
            points: vec![lin(0, -24.0), lin(3840, 6.0)],
        });
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.master_fx_auto.len(), 1);
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let (head, tail) = (rms(&out, 0.1, 0.4), rms(&out, 1.5, 1.9));
        assert!(
            tail > head * 5.0,
            "マスターの歪みが大きくなるはず: {head} {tail}"
        );
    }

    #[test]
    fn pan_automation_moves_left_to_right() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};

        let mut project = project_with_notes(vec![note(0, 3840, 69, 110)]);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::track("pan"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -1.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(3840),
                    value: 1.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let n = out.len() / 2;
        let energy = |range: std::ops::Range<usize>, ch: usize| -> f32 {
            range.map(|i| out[i * 2 + ch].powi(2)).sum()
        };
        let head_l = energy(0..n / 4, 0);
        let head_r = energy(0..n / 4, 1);
        let tail_l = energy(n / 2..n * 3 / 4, 0);
        let tail_r = energy(n / 2..n * 3 / 4, 1);
        assert!(head_l > head_r * 5.0, "冒頭は左寄りのはず");
        assert!(tail_r > tail_l * 2.0, "後半は右寄りのはず");
    }

    #[test]
    fn tempo_segments_convert_both_ways() {
        use glaux_core::{TempoEvent, TempoMap};
        // 0〜3840 tick は 120bpm(25 samples/tick)、以降 60bpm(50 samples/tick)
        let mut project = project_with_notes(vec![]);
        project.tempo_map = TempoMap::new(vec![
            TempoEvent {
                tick: Tick(0),
                bpm: 120.0,
            },
            TempoEvent {
                tick: Tick(3840),
                bpm: 60.0,
            },
        ])
        .unwrap();
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.tempo.len(), 2);
        assert_eq!(data.tempo[1].sample, 96_000);
        assert!((data.sample_to_tick(48_000) - 1920.0).abs() < 1e-9);
        assert!((data.sample_to_tick(96_000 + 50_000) - 4840.0).abs() < 1e-9);
        assert_eq!(data.tick_to_sample(1920.0), 48_000);
        assert_eq!(data.tick_to_sample(4840.0), 146_000);
    }

    #[test]
    fn next_beat_follows_time_signature() {
        use glaux_core::TimeSigEvent;
        let mut project = project_with_notes(vec![]);
        project.time_sig_map = vec![
            TimeSigEvent {
                tick: Tick(0),
                num: 4,
                den: 4,
            },
            TimeSigEvent {
                tick: Tick(3840),
                num: 7,
                den: 8,
            },
        ];
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        // 120bpm: 1 拍(4 分)= 24000 サンプル
        let b0 = data.next_beat(0).unwrap();
        assert_eq!((b0.sample, b0.downbeat), (0, true));
        let b1 = data.next_beat(1).unwrap();
        assert_eq!((b1.sample, b1.downbeat), (24_000, false));
        // 2 小節目(tick 3840 = 96000)からは 8 分拍 = 12000 サンプル、7 拍で小節
        let b = data.next_beat(96_001).unwrap();
        assert_eq!((b.sample, b.downbeat), (108_000, false));
        let bar3 = data.next_beat(96_000 + 12_000 * 7).unwrap();
        assert!(bar3.downbeat);
    }

    #[test]
    fn tempo_change_keeps_musical_position() {
        use crate::render::{Renderer, Shared};
        use glaux_core::{TempoEvent, TempoMap};
        use std::sync::atomic::Ordering;
        use std::sync::Arc;

        let mk = |bpm: f64| {
            let mut p = project_with_notes(vec![note(0, 3840 * 4, 60, 100)]);
            p.tempo_map = TempoMap::new(vec![TempoEvent { tick: Tick(0), bpm }]).unwrap();
            build_playback_data(&p, 48_000.0, &SampleBank::default())
        };
        let shared = Arc::new(Shared::new(mk(120.0)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 4800 * 2];
        for _ in 0..10 {
            r.process(&mut buf, 2); // 1 秒 = tick 1920 @120bpm
        }
        assert_eq!(shared.pos.load(Ordering::Acquire), 48_000);

        // 60bpm に差し替え: tick 1920 は 2 秒 = 96000 サンプルに相当する
        shared.data.store(Arc::new(mk(60.0)));
        r.process(&mut buf, 2);
        assert_eq!(
            shared.pos.load(Ordering::Acquire),
            96_000 + 4800,
            "テンポ変更後も音楽的位置(tick)が保たれるはず"
        );
    }

    /// 1 秒のサイン波 WAV を書き、それを音声クリップとして置いたプロジェクトを作る
    fn audio_clip_project(dir: &std::path::Path, clip_start: u64, clip_len: u64) -> Project {
        use glaux_core::{Asset, AssetId, TrackKind};
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(dir.join("audio/tone.wav"), spec).unwrap();
        for i in 0..48_000 {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin();
            w.write_sample((s * 20_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();

        let mut project = Project::new("a");
        let asset_id = AssetId::from_sha256_hex("abcd").unwrap();
        project.assets.insert(
            asset_id.clone(),
            Asset {
                path: "audio/tone.wav".into(),
                sample_rate: 48_000,
                channels: 1,
                frames: 48_000,
            },
        );
        let mut track = Track::new(TrackId::new(), "Gt", TrackKind::Audio);
        track.clips.push(Clip::new_audio(
            ClipId::new(),
            "take",
            Tick(clip_start),
            Tick(clip_len),
            asset_id,
        ));
        project.tracks.push(track);
        project
    }

    #[test]
    fn stereo_audio_clip_keeps_left_and_right() {
        use crate::export::render_project;
        use glaux_core::{Asset, AssetId, TempoEvent, TempoMap, TrackKind};
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        // 左だけ鳴るステレオの WAV(1 秒)
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(dir.join("audio/left.wav"), spec).unwrap();
        for i in 0..48_000 {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin();
            w.write_sample((s * 20_000.0) as i16).unwrap();
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
        let loaded = load_wav(&dir.join("audio/left.wav")).unwrap();
        assert!(loaded.side.is_some());
        assert!(load_wav_mono(&dir.join("audio/left.wav"))
            .unwrap()
            .side
            .is_none());

        let mut project = Project::new("st");
        let asset_id = AssetId::from_sha256_hex("abce").unwrap();
        project.assets.insert(
            asset_id.clone(),
            Asset {
                path: "audio/left.wav".into(),
                sample_rate: 48_000,
                channels: 2,
                frames: 48_000,
            },
        );
        let mut track = Track::new(TrackId::new(), "St", TrackKind::Audio);
        track.clips.push(Clip::new_audio(
            ClipId::new(),
            "take",
            Tick(0),
            Tick(1920),
            asset_id,
        ));
        project.tracks.push(track);
        let lr = |p: &Project| -> (f32, f32) {
            let bank = SampleBank::load(p, dir);
            let out = render_project(p, 48_000.0, &bank).unwrap();
            let e = |ch: usize| {
                out.chunks(2)
                    .take(40_000)
                    .map(|c| c[ch] * c[ch])
                    .sum::<f32>()
            };
            (e(0), e(1))
        };
        let (l, r) = lr(&project);
        assert!(l > 100.0 * r.max(1e-6), "左だけ鳴る: {l} / {r}");
        // パンは左右バランス: 中央で左の音量はモノラル素材と同じ(√2 補正)、右へ振ると左が下がる
        let mut right = project.clone();
        right.tracks[0].pan = 0.8;
        let (l2, _) = lr(&right);
        assert!(l2 < l * 0.5, "右へ振ると左が小さくなる: {l2} vs {l}");
        // テンポ追従で伸縮しても左右は保たれる
        let mut follow = project.clone();
        follow.tempo_map = TempoMap::new(vec![TempoEvent {
            tick: Tick(0),
            bpm: 100.0,
        }])
        .unwrap();
        if let glaux_core::ClipContent::Audio { stretch, .. } =
            &mut follow.tracks[0].clips[0].content
        {
            *stretch = glaux_core::Stretch::Follow {
                original_bpm: 120.0,
            };
        }
        let (l3, r3) = lr(&follow);
        assert!(l3 > 100.0 * r3.max(1e-6), "伸縮後も左だけ: {l3} / {r3}");
    }

    #[test]
    fn audio_clip_plays_in_its_region_only() {
        use crate::export::render_project;
        let tmp = tempfile::tempdir().unwrap();
        // クリップは 0.5 秒(tick 960)から 1 小節(2 秒)だが波形は 1 秒しかない
        let project = audio_clip_project(tmp.path(), 960, 3840);
        let bank = SampleBank::load(&project, tmp.path());
        let data = build_playback_data(&project, 48_000.0, &bank);
        assert_eq!(data.audio_events.len(), 1);
        assert_eq!(data.audio_events[0].start, 24_000);
        assert_eq!(data.end_sample, 24_000 + 96_000);

        let out = render_project(&project, 48_000.0, &bank).unwrap();
        let rms = |a: f64, b: f64| {
            let e = ((b * 96_000.0) as usize).min(out.len());
            let sl = &out[(a * 96_000.0) as usize..e];
            (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt()
        };
        assert!(rms(0.0, 0.4) < 1e-4, "クリップ前は無音");
        assert!(rms(0.6, 1.4) > 0.2, "クリップ区間は鳴る");
        assert!(rms(1.6, 1.9) < 1e-4, "波形を読み切ったら止まる");
    }

    #[test]
    fn follow_clip_stretches_with_tempo_and_keeps_pitch() {
        use crate::export::render_project;
        use glaux_core::{Stretch, TempoEvent, TempoMap};
        let tmp = tempfile::tempdir().unwrap();
        // 1 秒の 440Hz を 120BPM の素材(= 2 拍)として置き、プロジェクトを 60BPM にする
        // → 2 拍 = 2 秒に伸びる(音程は 440Hz のまま)
        let mut project = audio_clip_project(tmp.path(), 0, 1920);
        let clip = &mut project.tracks[0].clips[0];
        if let ClipContent::Audio { stretch, .. } = &mut clip.content {
            *stretch = Stretch::Follow {
                original_bpm: 120.0,
            };
        }
        project.tempo_map = TempoMap::new(vec![TempoEvent {
            tick: Tick(0),
            bpm: 60.0,
        }])
        .unwrap();
        let mut bank = SampleBank::load(&project, tmp.path());
        let data = build_playback_data(&project, 48_000.0, &bank);
        let ev = &data.audio_events[0];
        assert_eq!(ev.end, 96_000, "クリップは 2 秒");
        assert_eq!(ev.data.frames.len(), 96_000, "伸縮済み波形も 2 秒");
        assert_eq!(ev.offset, 0.0);

        let out = render_project(&project, 48_000.0, &bank).unwrap();
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let body = &left[4_800..86_400];
        let rms = (body.iter().map(|v| v * v).sum::<f32>() / body.len() as f32).sqrt();
        assert!(rms > 0.2, "伸ばした区間ずっと鳴る: {rms}");
        let crossings: Vec<usize> = (1..body.len())
            .filter(|&i| body[i - 1] < 0.0 && body[i] >= 0.0)
            .collect();
        let freq = (crossings.len() - 1) as f32 * 48_000.0
            / (crossings[crossings.len() - 1] - crossings[0]) as f32;
        assert!((freq - 440.0).abs() < 3.0, "音程は変わらない: {freq}");

        // 同じ条件なら作り直さない(Arc が同じ)/ テンポを元に戻すと伸縮しない
        let before = ev.data.clone();
        bank.sync(&project, tmp.path());
        let again = build_playback_data(&project, 48_000.0, &bank);
        assert!(Arc::ptr_eq(&before, &again.audio_events[0].data));
        project.tempo_map = TempoMap::new(vec![TempoEvent {
            tick: Tick(0),
            bpm: 120.0,
        }])
        .unwrap();
        bank.sync(&project, tmp.path());
        let same = build_playback_data(&project, 48_000.0, &bank);
        assert_eq!(
            same.audio_events[0].data.frames.len(),
            48_000,
            "元の素材をそのまま使う"
        );
    }

    #[test]
    fn audio_clip_resumes_mid_clip_after_seek_and_pause() {
        use crate::render::{Renderer, Shared};
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        let tmp = tempfile::tempdir().unwrap();
        let project = audio_clip_project(tmp.path(), 0, 3840);
        let bank = SampleBank::load(&project, tmp.path());
        let shared = Arc::new(Shared::new(build_playback_data(&project, 48_000.0, &bank)));
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 4800 * 2];
        let rms = |b: &[f32]| (b.iter().map(|s| s * s).sum::<f32>() / b.len() as f32).sqrt();

        // クリップ途中(0.5 秒)へシークして再生 → 鳴る
        shared.seek.store(24_000, Ordering::Release);
        shared.playing.store(true, Ordering::Release);
        r.process(&mut buf, 2);
        assert!(rms(&buf) > 0.2, "途中からでも鳴るはず");

        // 一時停止 → 無音、再開 → 続きから鳴る
        shared.playing.store(false, Ordering::Release);
        r.process(&mut buf, 2);
        assert!(rms(&buf) < 1e-6);
        shared.playing.store(true, Ordering::Release);
        r.process(&mut buf, 2);
        assert!(rms(&buf) > 0.2, "再開後も鳴るはず");
    }

    #[test]
    fn pan_and_volume_shape_gains() {
        let mut project = project_with_notes(vec![]);
        project.tracks[0].pan = -1.0; // full L
        project.tracks[0].volume_db = -6.0;
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let t = &data.tracks[0];
        assert!(t.gain_l > 0.0);
        assert!(t.gain_r.abs() < 1e-6);
        // -6dB ≒ 0.5012
        assert!((t.gain_l - db_to_amp(-6.0)).abs() < 1e-6);
    }
}
