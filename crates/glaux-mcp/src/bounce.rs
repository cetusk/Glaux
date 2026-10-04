//! トラックを音声にする(フリーズ / バウンス)。
//!
//! 対象のトラックを、自分のエフェクト・音量・パン・オートメーションと、センドしているバスの響きまで込みで
//! 描き出し(マスターのエフェクトとクリップ防止は通さない)、新しい音声トラックとして直後に置いて、
//! 元のトラックはミュートする。全体で 1 件の履歴(1 回の undo で戻る)。
//!
//! 用途: CLAP プラグインの音源はゲーム(Godot)で鳴らないので音声にしておく / 重いトラックを軽くする /
//! 音声として切り貼りする。
//!
//! [`resample_to_sampler`] は同じ描き出しを、内蔵の sampler の音源にする(リサンプリング:
//! 作った音を鍵盤で弾き直す・スライスして並べ替える・ループして伸ばす)。

use crate::assets::{audio_clip_commands, import_wav};
use glaux_core::{ClipId, Command, Project, Tick, Track, TrackId, TrackKind, TrackProp};
use std::path::Path;

/// 描き出しのサンプルレート
const RATE: u32 = 48_000;

/// 音声にした結果。
pub struct Bounced {
    /// 適用するコマンド(音声トラックの追加・音声の登録とクリップ・元のトラックのミュート)
    pub commands: Vec<Command>,
    pub new_track: TrackId,
    pub seconds: f64,
    pub label: String,
}

/// 描き出す用のプロジェクト: 対象のトラックと、それがセンドしているバスだけを残す。
/// マスターのエフェクト・オートメーションは外し、音量は 0dB にする
pub fn stem_project(project: &Project, track: &Track) -> Project {
    let buses: Vec<TrackId> = track.sends.iter().map(|s| s.target.clone()).collect();
    let mut p = project.clone();
    p.tracks
        .retain(|t| t.id == track.id || buses.contains(&t.id));
    for t in &mut p.tracks {
        t.solo = false;
        if t.id == track.id {
            t.mute = false;
        }
    }
    p.master.effects.clear();
    p.master.fx_links = None;
    p.master.automation.clear();
    p.master.volume_db = 0.0;
    p
}

/// `track_id` のトラックを音声にするコマンドを作る(描き出した WAV はプロジェクトの audio/ に取り込む)。
pub fn bounce_track(
    project: &Project,
    project_dir: &Path,
    track_id: &TrackId,
    bank: &glaux_engine::SampleBank,
) -> Result<Bounced, String> {
    let index = project
        .tracks
        .iter()
        .position(|t| &t.id == track_id)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let track = &project.tracks[index];
    if track.kind == TrackKind::Bus {
        return Err("バスは音声にできません(送っているトラックを音声にしてください)".to_owned());
    }

    let stem = stem_project(project, track);
    let stereo = glaux_engine::render_stem(&stem, RATE as f64, bank)
        .map_err(|e| format!("「{}」を描き出せません: {e}", track.name))?;
    let imported = import_stereo(project_dir, track_id, &stereo)?;

    let new_id = TrackId::new();
    let name = format!("{}(音声)", track.name);
    let mut new_track = Track::new(new_id.clone(), name.clone(), TrackKind::Audio);
    new_track.color = track.color.clone();
    // 左右が同じ音(中央に置いたモノラルの音源)は、読み込み時にモノラルとして扱われ、中央でもパンの
    // 計算で -3dB になる。描き出した音には元のトラックの中央の -3dB が既に入っているので、音量を √2 倍
    // (+3.01dB)にして二重に下がるのを打ち消す(読み込み側の判定と同じ条件)
    let is_mono = stereo
        .as_chunks::<2>()
        .0
        .iter()
        .all(|c| (c[0] - c[1]).abs() < 1e-6);
    if is_mono {
        new_track.volume_db = 20.0 * std::f32::consts::SQRT_2.log10();
    }
    let add_track = Command::AddTrack {
        track: new_track,
        index: Some(index + 1),
    };
    // 音声クリップのコマンドは、置き先のトラックがある状態で作る
    let mut with_track = project.clone();
    with_track.apply(&add_track).map_err(|e| e.to_string())?;
    let mut commands = vec![add_track];
    commands.extend(audio_clip_commands(
        &with_track,
        &new_id,
        &imported,
        ClipId::new(),
        Tick::ZERO,
        &name,
    )?);
    if !track.mute {
        commands.push(Command::SetTrackProp {
            id: track_id.clone(),
            prop: TrackProp::Mute(true),
        });
    }
    Ok(Bounced {
        commands,
        new_track: new_id,
        seconds: stereo.len() as f64 / 2.0 / RATE as f64,
        label: format!("「{}」を音声にする(元のトラックはミュート)", track.name),
    })
}

/// 32bit 浮動小数のステレオ WAV にして取り込む(0dBFS を超える音も潰さずに残す)
fn import_stereo(
    project_dir: &Path,
    track_id: &TrackId,
    stereo: &[f32],
) -> Result<crate::assets::ImportedSample, String> {
    let tmp_dir = project_dir.join("cache");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
    let tmp = tmp_dir.join(format!("bounce-{track_id}.wav"));
    {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&tmp, spec).map_err(|e| e.to_string())?;
        for s in stereo {
            w.write_sample(*s).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    let imported = import_wav(project_dir, &tmp);
    let _ = std::fs::remove_file(&tmp);
    imported
}

/// リサンプリングの指定
pub struct ResampleRequest {
    /// 描き出す範囲(tick)。終わりを省略すると、鳴り終わり(無音になる所)まで
    pub start: Tick,
    pub end: Option<Tick>,
    /// 置き先の MIDI トラック。None なら元のトラックの直後に新しく作る
    pub target: Option<TrackId>,
    /// サンプラーの root(この鍵盤で元の高さ)
    pub root: u8,
    /// スライス数(0 = スライスしない)
    pub slices: u32,
    /// 元のトラックをミュートする
    pub mute_source: bool,
}

/// リサンプリングの結果
pub struct Resampled {
    pub commands: Vec<Command>,
    /// サンプラーにしたトラック
    pub track: TrackId,
    pub asset: glaux_core::AssetId,
    pub seconds: f64,
    pub label: String,
}

/// 鳴り終わりとみなす音量(約 -80dBFS)
const SILENCE: f32 = 1e-4;

/// `track_id` のトラックの範囲を(自分のエフェクト・センド先のバス込みで)描き出し、
/// 内蔵の sampler の音源にするコマンドを作る(描き出した WAV はプロジェクトの audio/ に取り込む)。
pub fn resample_to_sampler(
    project: &Project,
    project_dir: &Path,
    track_id: &TrackId,
    req: &ResampleRequest,
    bank: &glaux_engine::SampleBank,
) -> Result<Resampled, String> {
    let index = project
        .tracks
        .iter()
        .position(|t| &t.id == track_id)
        .ok_or_else(|| format!("トラックが見つかりません: {track_id}"))?;
    let track = &project.tracks[index];
    if track.kind == TrackKind::Bus {
        return Err("バスは描き出せません(送っているトラックを指定してください)".to_owned());
    }
    if let Some(target) = &req.target {
        let t = project
            .track(target)
            .ok_or_else(|| format!("置き先のトラックが見つかりません: {target}"))?;
        if t.kind != TrackKind::Midi {
            return Err(format!(
                "置き先「{}」は MIDI トラックではありません(サンプラーは MIDI トラックの音源)",
                t.name
            ));
        }
    }
    if req.end.is_some_and(|e| e <= req.start) {
        return Err("end_tick は start_tick より後にしてください".to_owned());
    }

    let stem = stem_project(project, track);
    let stereo = glaux_engine::render_stem(&stem, RATE as f64, bank)
        .map_err(|e| format!("「{}」を描き出せません: {e}", track.name))?;
    let frames = stereo.len() / 2;
    let to_frame = |t: Tick| {
        ((project.tempo_map.tick_to_seconds(t) * RATE as f64).round() as usize).min(frames)
    };
    let from = to_frame(req.start);
    let to = match req.end {
        Some(e) => to_frame(e),
        None => {
            // 鳴り終わり(最後に SILENCE を超える所)+ 10ms
            let last = stereo
                .chunks(2)
                .rposition(|c| c[0].abs() > SILENCE || c[1].abs() > SILENCE)
                .unwrap_or(0);
            (last + RATE as usize / 100).min(frames)
        }
    };
    if to <= from + 64 {
        return Err(
            "描き出した範囲が無音か短すぎます(範囲とトラックの中身を確かめてください)".into(),
        );
    }
    let part = &stereo[from * 2..to * 2];
    if part.iter().all(|v| v.abs() <= SILENCE) {
        return Err("描き出した範囲が無音です(範囲とトラックの中身を確かめてください)".into());
    }
    let imported = import_stereo(project_dir, track_id, part)?;
    let is_mono = part
        .as_chunks::<2>()
        .0
        .iter()
        .all(|c| (c[0] - c[1]).abs() < 1e-6);

    let mut params = glaux_core::ParamMap::new();
    params.insert(
        "root".to_owned(),
        glaux_core::ParamValue::Int(req.root.min(127) as i64),
    );
    if is_mono {
        // 左右が同じ音は中央に置いたモノラルの音源。描き出した音には中央の -3dB が既に入っていて、
        // サンプラーで鳴らすとまた中央の -3dB が掛かるので、ゲインを √2 倍(+3.01dB)にして打ち消す
        params.insert(
            "gain_db".to_owned(),
            glaux_core::ParamValue::Float(20.0 * std::f64::consts::SQRT_2.log10()),
        );
    } else {
        // ステレオの音は左右のまま鳴らす(パンは左右バランス。中央 0dB)
        params.insert("stereo".to_owned(), glaux_core::ParamValue::Bool(true));
    }
    if req.slices > 0 {
        params.insert(
            "slices".to_owned(),
            glaux_core::ParamValue::Int(req.slices.min(64) as i64),
        );
    }
    let device = glaux_core::Device {
        source: glaux_core::PluginSource::Sampler {
            asset: imported.id.clone(),
        },
        params,
    };

    let mut commands = Vec::new();
    if !project.assets.contains_key(&imported.id) {
        commands.push(Command::AddAsset {
            id: imported.id.clone(),
            asset: imported.asset.clone(),
        });
    }
    let target = match &req.target {
        Some(t) => {
            commands.push(Command::SetDevice {
                track: t.clone(),
                device: Some(device),
            });
            t.clone()
        }
        None => {
            let id = TrackId::new();
            let mut t = Track::new(
                id.clone(),
                format!("{}(サンプル)", track.name),
                TrackKind::Midi,
            );
            t.color = track.color.clone();
            t.device = Some(device);
            commands.push(Command::AddTrack {
                track: t,
                index: Some(index + 1),
            });
            id
        }
    };
    if req.mute_source && !track.mute {
        commands.push(Command::SetTrackProp {
            id: track_id.clone(),
            prop: TrackProp::Mute(true),
        });
    }
    Ok(Resampled {
        commands,
        track: target,
        asset: imported.id,
        seconds: (to - from) as f64 / RATE as f64,
        label: format!("「{}」をサンプラーの音源にする(リサンプリング)", track.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Articulation, Clip, ClipContent, Note, NoteId};

    #[test]
    fn bounced_audio_sounds_like_the_track() {
        // 中央(左右が同じ = モノラル扱い)とパンを振った場合(ステレオ扱い)の両方
        for pan in [0.0f32, -0.6] {
            check_bounce(pan);
        }
    }

    fn check_bounce(pan: f32) {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Project::new("b");
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "Lead", TrackKind::Midi);
        t.volume_db = -6.0;
        t.pan = pan;
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
        if let ClipContent::Midi { notes, .. } = &mut c.content {
            notes.push(Note {
                locked: false,
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(1920),
                pitch: 60,
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
        t.clips.push(c);
        p.tracks.push(t);
        // マスターの音量はフリーズに入れない
        p.master.volume_db = -20.0;

        let b = bounce_track(&p, tmp.path(), &tid, &Default::default()).unwrap();
        let mut after = p.clone();
        after
            .apply(&Command::batch(b.label.clone(), b.commands))
            .unwrap();
        // 音声トラックが直後にでき、元はミュート
        assert_eq!(after.tracks.len(), 2);
        assert_eq!(after.tracks[1].id, b.new_track);
        assert_eq!(after.tracks[1].kind, TrackKind::Audio);
        assert!(after.tracks[0].mute);
        assert!(b.seconds > 1.0);

        // 元のトラック(ミュート前)と、音声にしたトラックの鳴り方が同じ
        let bank = glaux_engine::SampleBank::load(&after, tmp.path());
        let original = glaux_engine::render_project(&p, 48_000.0, &Default::default()).unwrap();
        let bounced = glaux_engine::render_project(&after, 48_000.0, &bank).unwrap();
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let n = original.len().min(bounced.len()).min(48_000);
        for ch in 0..2 {
            let pick = |x: &[f32]| {
                x[..n]
                    .iter()
                    .skip(ch)
                    .step_by(2)
                    .copied()
                    .collect::<Vec<f32>>()
            };
            let (a, b2) = (rms(&pick(&original)), rms(&pick(&bounced)));
            assert!(
                (a / b2 - 1.0).abs() < 0.05,
                "pan {pan} ch {ch}: 元 {a} / 音声 {b2}"
            );
        }
    }

    #[test]
    fn bus_cannot_be_bounced() {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Project::new("b");
        let tid = TrackId::new();
        p.tracks
            .push(Track::new(tid.clone(), "Rev", TrackKind::Bus));
        assert!(bounce_track(&p, tmp.path(), &tid, &Default::default()).is_err());
    }

    fn note_at(pos: u64, pitch: u8) -> Note {
        Note {
            locked: false,
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(960),
            pitch,
            vel: 100,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        }
    }

    /// トラックの最初のクリップ(無ければ 4 小節の MIDI クリップを作る)
    fn c_of(t: &mut Track) -> &mut Clip {
        if t.clips.is_empty() {
            t.clips
                .push(Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(7680)));
        }
        &mut t.clips[0]
    }

    #[test]
    fn resampled_range_plays_from_a_sampler() {
        // 中央(モノラル扱い)とパンを振った場合(ステレオで鳴らす)の両方
        for pan in [0.0f32, -0.6] {
            check_resample(pan);
        }
    }

    fn check_resample(pan: f32) {
        let tmp = tempfile::tempdir().unwrap();
        let mut p = Project::new("r");
        let tid = TrackId::new();
        let mut t = Track::new(tid.clone(), "Pad", TrackKind::Midi);
        t.pan = pan;
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(7680));
        if let ClipContent::Midi { notes, .. } = &mut c.content {
            // 2 小節目の頭に 1 音だけ
            notes.push(note_at(1920, 57));
        }
        t.clips.push(c);
        p.tracks.push(t);
        let req = ResampleRequest {
            start: Tick(1920),
            end: None,
            target: None,
            root: 57,
            slices: 0,
            mute_source: false,
        };
        let r = resample_to_sampler(&p, tmp.path(), &tid, &req, &Default::default()).unwrap();
        // 頭(1 小節目の無音)は入れず、鳴り終わりまで(0.5 秒の音 + リリース)
        assert!(r.seconds > 0.4 && r.seconds < 2.5, "{}", r.seconds);
        let mut after = p.clone();
        after
            .apply(&Command::batch(r.label.clone(), r.commands))
            .unwrap();
        assert_eq!(after.tracks.len(), 2);
        let st = &after.tracks[1];
        assert_eq!(st.id, r.track);
        assert!(!after.tracks[0].mute, "元のトラックは残す(既定)");
        let Some(glaux_core::Device {
            source: glaux_core::PluginSource::Sampler { asset },
            params,
        }) = &st.device
        else {
            panic!("サンプラーのはず");
        };
        assert_eq!(asset, &r.asset);
        assert_eq!(params.get("root"), Some(&glaux_core::ParamValue::Int(57)));
        // 取り込んだ素材は頭から音がある(範囲の頭 = 音の頭)
        let bank = glaux_engine::SampleBank::load(&after, tmp.path());
        let data = bank.get(&r.asset).expect("読み込める");
        let head = data.frames[..2400]
            .iter()
            .map(|v| v.abs())
            .fold(0.0f32, f32::max);
        assert!(head > 0.01, "頭から音がある: {head}");
        // 新しいトラックを root で弾くと、元のトラックとほぼ同じ大きさで鳴る
        let mut play = after.clone();
        play.tracks[0].mute = true;
        play.tracks[1].pan = 0.0;
        // 描き出した音には強さ(100)が入っているので、サンプラーは最強(127)で弾いて比べる
        if let ClipContent::Midi { notes, .. } = &mut c_of(&mut play.tracks[1]).content {
            notes.push(Note {
                vel: 127,
                ..note_at(1920, 57)
            });
        }
        let rms = |x: &[f32], ch: usize| {
            let v: Vec<f32> = x[96_000..144_000]
                .iter()
                .skip(ch)
                .step_by(2)
                .copied()
                .collect();
            (v.iter().map(|v| v * v).sum::<f32>() / v.len() as f32).sqrt()
        };
        let a = glaux_engine::render_project(&p, 48_000.0, &bank).unwrap();
        let b = glaux_engine::render_project(&play, 48_000.0, &bank).unwrap();
        for ch in 0..2 {
            let (ra, rb) = (rms(&a, ch), rms(&b, ch));
            assert!(
                (ra / rb - 1.0).abs() < 0.1,
                "pan {pan} ch {ch}: 元 {ra} / サンプラー {rb}"
            );
        }

        // 無音の範囲はエラー
        let silent = ResampleRequest {
            start: Tick(0),
            end: Some(Tick(1800)),
            ..req
        };
        assert!(resample_to_sampler(&p, tmp.path(), &tid, &silent, &Default::default()).is_err());
    }
}
