//! `Project` からエンジン用の再生データを組み立てる。
//!
//! - **UI(非オーディオ)スレッドで**構築し、`ArcSwap` でオーディオスレッドに渡す。
//!   オーディオスレッドはこのデータを読むだけで、一切アロケーションしない。
//! - ノートは絶対サンプル位置に展開してソート済み。テンポは構築時に焼き込む
//!   (テンポ変更があればデータごと作り直す。再生位置はサンプルで保持しているため、
//!   再生中のテンポ変更では音楽的位置が僅かにずれる。Tick ベースの位置管理は将来課題)。
//! - 音声クリップは未対応(MVP は MIDI のみ。`glaux-dsp` 実装後に差し替える)。

use glaux_core::{ClipContent, Effect, Project, Tick};
use glaux_dsp::{EffectParams, InstrumentParams};

/// エフェクト状態プールのスロット数(エンジン起動時に固定確保)。
/// これを超えたエフェクトは無視される(構築時に警告)。
pub const MAX_EFFECT_SLOTS: usize = 64;
/// エフェクト・ミックス処理の対象になる最大トラック数。
/// 超過分のトラックはエフェクトなしで直接マスターへ送られる。
pub const MAX_TRACKS: usize = 64;

/// 焼き込み済みエフェクト + 状態プールのスロット番号。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakedEffect {
    pub params: EffectParams,
    pub slot: u32,
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
}

/// トラックのミックス設定(音量・パン・mute/solo を反映済み)。
#[derive(Clone, Debug, PartialEq)]
pub struct TrackMix {
    pub gain_l: f32,
    pub gain_r: f32,
    /// mute / solo 判定の結果。false なら発音しない
    pub audible: bool,
    /// 焼き込み済みの楽器パラメータ(glaux-dsp)
    pub instrument: InstrumentParams,
    /// エフェクトチェーン(bypass 除外・焼き込み済み)。楽器 → チェーン → 音量/パン の順
    pub effects: Vec<BakedEffect>,
}

/// オーディオスレッドが読む再生データ一式。イミュータブル。
#[derive(Clone, Debug, Default)]
pub struct PlaybackData {
    /// `start` 昇順
    pub events: Vec<NoteEvent>,
    pub tracks: Vec<TrackMix>,
    /// マスターバスのエフェクトチェーン
    pub master_effects: Vec<BakedEffect>,
    pub master_amp: f32,
    /// 最後のノートが終わるサンプル位置(自動停止に使う)
    pub end_sample: u64,
    pub sample_rate: f64,
}

pub fn db_to_amp(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub fn pitch_to_freq(pitch: u8) -> f32 {
    440.0 * 2.0_f32.powf((pitch as f32 - 69.0) / 12.0)
}

/// 等パワーパン。pan は -1.0(L) ..= 1.0(R)。
fn pan_gains(pan: f32) -> (f32, f32) {
    let t = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (t.cos(), t.sin())
}

/// エフェクトチェーンを焼き込み、状態プールのスロットを割り当てる。
fn bake_chain(
    effects: &[Effect],
    sample_rate: f32,
    next_slot: &mut u32,
    resolve_track: &dyn Fn(&str) -> Option<u32>,
) -> Vec<BakedEffect> {
    effects
        .iter()
        .filter(|e| !e.bypass)
        .filter_map(|e| {
            let params = glaux_dsp::bake_effect(e, sample_rate, resolve_track)?;
            if *next_slot as usize >= MAX_EFFECT_SLOTS {
                tracing::warn!(
                    "エフェクトが多すぎます({MAX_EFFECT_SLOTS} 超)。{} を無視",
                    e.id
                );
                return None;
            }
            let slot = *next_slot;
            *next_slot += 1;
            Some(BakedEffect { params, slot })
        })
        .collect()
}

/// プロジェクト全体を再生データに展開する。
pub fn build_playback_data(project: &Project, sample_rate: f64) -> PlaybackData {
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

    let mut next_slot: u32 = 0;
    let tracks: Vec<TrackMix> = project
        .tracks
        .iter()
        .map(|t| {
            let (pl, pr) = pan_gains(t.pan);
            let gain = db_to_amp(t.volume_db);
            let (_, instrument) = glaux_dsp::bake_instrument(t.device.as_ref());
            TrackMix {
                gain_l: gain * pl,
                gain_r: gain * pr,
                audible: !t.mute && (!any_solo || t.solo),
                instrument,
                effects: bake_chain(
                    &t.effects,
                    sample_rate as f32,
                    &mut next_slot,
                    &resolve_track,
                ),
            }
        })
        .collect();
    let master_effects = bake_chain(
        &project.master.effects,
        sample_rate as f32,
        &mut next_slot,
        &resolve_track,
    );

    let mut events = Vec::new();
    for (ti, track) in project.tracks.iter().enumerate() {
        for clip in &track.clips {
            let ClipContent::Midi { notes, .. } = &clip.content else {
                continue; // 音声クリップは MVP では鳴らさない
            };
            for note in notes {
                if note.pos >= clip.length {
                    continue;
                }
                // クリップ末尾をまたぐノートは切り詰める
                let dur = if note.pos + note.dur > clip.length {
                    clip.length - note.pos
                } else {
                    note.dur
                };
                let start_tick = clip.start + note.pos;
                let start = to_sample(start_tick);
                let end = to_sample(start_tick + dur).max(start + 1);
                events.push(NoteEvent {
                    start,
                    end,
                    freq: pitch_to_freq(note.pitch),
                    pitch: note.pitch,
                    amp: note.vel as f32 / 127.0,
                    track: ti as u32,
                });
            }
        }
    }
    events.sort_by_key(|e| e.start);
    let end_sample = events.iter().map(|e| e.end).max().unwrap_or(0);

    PlaybackData {
        events,
        tracks,
        master_effects,
        master_amp: db_to_amp(project.master.volume_db),
        end_sample,
        sample_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipId, NoteId, Track, TrackId, TrackKind};

    fn note(pos: u64, dur: u64, pitch: u8, vel: u8) -> glaux_core::Note {
        glaux_core::Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel,
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
        let data = build_playback_data(&project, 48_000.0);
        assert_eq!(data.events.len(), 1);
        let e = &data.events[0];
        assert_eq!(e.start, 24_000);
        assert_eq!(e.end, 48_000);
        assert!((e.freq - 440.0).abs() < 1e-3);
        assert!((e.amp - 1.0).abs() < 1e-6);
        assert_eq!(data.end_sample, 48_000);
    }

    #[test]
    fn truncates_notes_at_clip_end_and_sorts() {
        let project = project_with_notes(vec![
            note(3000, 5000, 60, 100), // クリップ長 3840 で切り詰め
            note(0, 480, 64, 100),
        ]);
        let data = build_playback_data(&project, 48_000.0);
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

        let data = build_playback_data(&project, 48_000.0);
        // T2 が solo なので T1 は聞こえない
        assert!(!data.tracks[0].audible);
        assert!(data.tracks[1].audible);

        project.tracks[1].solo = false;
        project.tracks[0].mute = true;
        let data = build_playback_data(&project, 48_000.0);
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

        let data = build_playback_data(&project, 48_000.0);
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

        let dry = render_project(&dry_project, 48_000.0).unwrap();
        let wet = render_project(&wet_project, 48_000.0).unwrap();
        assert!(
            wet.len() > dry.len(),
            "残響で音の長さが伸びるはず({} vs {})",
            wet.len(),
            dry.len()
        );
    }

    #[test]
    fn pan_and_volume_shape_gains() {
        let mut project = project_with_notes(vec![]);
        project.tracks[0].pan = -1.0; // full L
        project.tracks[0].volume_db = -6.0;
        let data = build_playback_data(&project, 48_000.0);
        let t = &data.tracks[0];
        assert!(t.gain_l > 0.0);
        assert!(t.gain_r.abs() < 1e-6);
        // -6dB ≒ 0.5012
        assert!((t.gain_l - db_to_amp(-6.0)).abs() < 1e-6);
    }
}
