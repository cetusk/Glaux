//! 音声クリップの音源分離(ステム分離)→ パートごとの音声トラックに置くコマンド組み立て。
//! UI(Tauri)と MCP ツールが共用する。
//!
//! - `builtin`: 内蔵の HPSS(`glaux_engine::separate`)。打楽器 / 音程楽器の 2 本。常に使える
//! - `demucs`: 外部の Demucs(Meta、`pip install demucs`)があれば呼び出す。
//!   ボーカル / ドラム / ベース / その他 の 4 本。高品質だが重い(CPU で曲の長さの半分〜数倍)
//!
//! 分けた音は新しい音声トラック(元トラックの直後)に、元クリップと同じ位置・長さ・音量・
//! テンポ追従で置き、元のトラックはミュートする(すべて 1 回の取り消しで戻る)。

use glaux_core::{ClipContent, ClipId, Command, Project, Track, TrackId, TrackKind, TrackProp};
use std::path::{Path, PathBuf};

/// 分離の方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeparateMethod {
    Builtin,
    Demucs,
}

impl SeparateMethod {
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        match s.unwrap_or("builtin") {
            "builtin" | "" => Ok(SeparateMethod::Builtin),
            "demucs" => Ok(SeparateMethod::Demucs),
            other => Err(format!("method は builtin か demucs です: {other}")),
        }
    }
}

pub struct Separated {
    pub commands: Vec<Command>,
    /// 作ったトラック(パート名, ID)
    pub tracks: Vec<(String, TrackId)>,
}

/// Demucs を呼び出すコマンド(見つからなければ None)。`demucs` → `python -m demucs` の順に探す。
pub fn find_demucs() -> Option<Vec<String>> {
    let candidates: &[&[&str]] = &[
        &["demucs"],
        &["python", "-m", "demucs"],
        &["python3", "-m", "demucs"],
        &["py", "-m", "demucs"],
    ];
    candidates.iter().find_map(|c| {
        let ok = command(c)
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        ok.then(|| c.iter().map(|s| s.to_string()).collect())
    })
}

fn command(argv: &[impl AsRef<std::ffi::OsStr>]) -> std::process::Command {
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // コンソールウィンドウを出さない(CREATE_NO_WINDOW)
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// `clip_id`(音声クリップ)を分離し、パートごとの音声トラックに置くコマンドを作る。
pub fn separate_clip_commands(
    project: &Project,
    project_dir: &Path,
    clip_id: &ClipId,
    method: SeparateMethod,
) -> Result<Separated, String> {
    let (src_index, src_track, clip) = project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(i, t)| t.clips.iter().find(|c| &c.id == clip_id).map(|c| (i, t, c)))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let ClipContent::Audio {
        asset,
        offset_samples,
        gain_db,
        fade_in_ms,
        fade_out_ms,
        stretch,
    } = &clip.content
    else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    // ステレオの素材は左右のまま分ける(M と S に同じマスク)
    let data = glaux_engine::load_wav(&project_dir.join(&meta.path))?;
    // クリップが参照している範囲だけを分ける
    let secs = stretch
        .follow_seconds(clip.length.0 as f64)
        .unwrap_or_else(|| {
            project.tempo_map.tick_to_seconds(clip.start + clip.length)
                - project.tempo_map.tick_to_seconds(clip.start)
        });
    let from = (*offset_samples as usize).min(data.frames.len());
    let to = (from + (secs * data.sample_rate as f64) as usize).min(data.frames.len());
    let range = &data.frames[from..to];
    if range.is_empty() {
        return Err("クリップの範囲に音声がありません".to_owned());
    }
    let side_range = data.side.as_ref().map(|s| &s[from..to]);

    let work = project_dir.join("cache").join("stems");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    // (名前, M, S)
    let stems: Vec<Stem> = match method {
        SeparateMethod::Builtin => {
            let mut chans: Vec<&[f32]> = vec![range];
            if let Some(sr) = side_range {
                chans.push(sr);
            }
            let mut r = glaux_engine::separate::hpss_channels(&chans);
            let side = (r.len() > 1).then(|| r.remove(1));
            let mid = r.remove(0);
            let (sp, sh) = match side {
                Some(s) => (Some(s.percussive), Some(s.harmonic)),
                None => (None, None),
            };
            vec![
                ("打楽器".to_owned(), mid.percussive, sp),
                ("音程楽器".to_owned(), mid.harmonic, sh),
            ]
        }
        SeparateMethod::Demucs => run_demucs(range, side_range, data.sample_rate, &work)?,
    };

    let mut commands = Vec::new();
    let mut tracks = Vec::new();
    let mut view = project.clone();
    for (i, (label, samples, side)) in stems.into_iter().enumerate() {
        let tmp = work.join(format!("{}_{i}.wav", clip.id));
        write_wav_ms(&tmp, &samples, side.as_deref(), data.sample_rate as u32)?;
        let imported = crate::assets::import_wav(project_dir, &tmp);
        let _ = std::fs::remove_file(&tmp);
        let imported = imported?;

        let tid = TrackId::new();
        let track = Track::new(
            tid.clone(),
            format!("{} {label}", src_track.name),
            TrackKind::Audio,
        );
        let add_track = Command::AddTrack {
            track,
            index: Some(src_index + 1 + i),
        };
        view.apply(&add_track).map_err(|e| e.to_string())?;
        commands.push(add_track);
        let cmds = crate::assets::audio_clip_commands(
            &view,
            &tid,
            &imported,
            ClipId::new(),
            clip.start,
            &format!("{} ({label})", clip.name),
        )?;
        for mut c in cmds {
            // 元クリップと同じ長さ・音量・フェード・テンポ追従で置く
            if let Command::AddClip { clip: new, .. } = &mut c {
                new.length = clip.length;
                if let ClipContent::Audio {
                    gain_db: g,
                    fade_in_ms: fi,
                    fade_out_ms: fo,
                    stretch: st,
                    ..
                } = &mut new.content
                {
                    *g = *gain_db;
                    *fi = *fade_in_ms;
                    *fo = *fade_out_ms;
                    *st = stretch.clone();
                }
            }
            view.apply(&c).map_err(|e| e.to_string())?;
            commands.push(c);
        }
        tracks.push((label, tid));
    }
    if !src_track.mute {
        commands.push(Command::SetTrackProp {
            id: src_track.id.clone(),
            prop: TrackProp::Mute(true),
        });
    }
    Ok(Separated { commands, tracks })
}

fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for s in samples {
        w.write_sample(*s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// 分けた 1 パート: (名前, モノラル成分 M, 左右差成分 S(ステレオのとき))
type Stem = (String, Vec<f32>, Option<Vec<f32>>);

/// M(と S)を WAV に書く。S があればステレオ(L = M + S、R = M − S)。
fn write_wav_ms(
    path: &Path,
    mid: &[f32],
    side: Option<&[f32]>,
    sample_rate: u32,
) -> Result<(), String> {
    let Some(side) = side else {
        return write_wav(path, mid, sample_rate);
    };
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for (m, s) in mid.iter().zip(side) {
        w.write_sample(m + s).map_err(|e| e.to_string())?;
        w.write_sample(m - s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// Demucs(htdemucs)で 4 パートに分ける(ステレオの素材はステレオのまま渡して受け取る)。
fn run_demucs(
    range: &[f32],
    side: Option<&[f32]>,
    sample_rate: f32,
    work: &Path,
) -> Result<Vec<Stem>, String> {
    let argv = find_demucs().ok_or_else(|| {
        "Demucs が見つかりません。Python 環境で `pip install demucs` を実行してから再度お試しください\
         (内蔵の分離(打楽器 / 音程楽器)は method: builtin で使えます)"
            .to_owned()
    })?;
    let input = work.join("demucs_input.wav");
    write_wav_ms(&input, range, side, sample_rate as u32)?;
    let out_dir: PathBuf = work.join("demucs_out");
    let _ = std::fs::remove_dir_all(&out_dir);
    let output = command(&argv)
        .args(["-n", "htdemucs", "--filename", "{stem}.{ext}", "-o"])
        .arg(&out_dir)
        .arg(&input)
        .output()
        .map_err(|e| format!("Demucs を起動できません: {e}"))?;
    let _ = std::fs::remove_file(&input);
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: String = err.lines().rev().take(5).collect::<Vec<_>>().join(" / ");
        return Err(format!("Demucs が失敗しました: {tail}"));
    }
    let mut stems = Vec::new();
    for (file, label) in [
        ("vocals", "ボーカル"),
        ("drums", "ドラム"),
        ("bass", "ベース"),
        ("other", "その他"),
    ] {
        let path = out_dir.join("htdemucs").join(format!("{file}.wav"));
        let data = glaux_engine::load_wav(&path)
            .map_err(|e| format!("Demucs の出力を読めません({file}): {e}"))?;
        // Demucs は 44.1kHz で書き出す。元のレートに合わせる
        let fit = |x: Vec<f32>| {
            if (data.sample_rate - sample_rate).abs() > 0.5 {
                glaux_ml::resample(&x, data.sample_rate, sample_rate)
            } else {
                x
            }
        };
        // 元がモノラルなら左右差は捨てる(Demucs は常にステレオで書き出す)
        let s = if side.is_some() {
            data.side.clone().map(fit)
        } else {
            None
        };
        stems.push((label.to_owned(), fit(data.frames.clone()), s));
    }
    let _ = std::fs::remove_dir_all(&out_dir);
    Ok(stems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::Tick;

    #[test]
    fn builtin_separation_keeps_stereo() {
        let tmp = tempfile::tempdir().unwrap();
        // 左にクリック(打楽器)、右に持続音(音程楽器)
        let sr = 22_050u32;
        let wav = tmp.path().join("st.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..sr {
            let t = i as f32 / sr as f32;
            let click = if i % (sr / 4) < 100 { 0.6 } else { 0.0 };
            let tone = 0.3 * (std::f32::consts::TAU * 220.0 * t).sin();
            w.write_sample((click * 20_000.0) as i16).unwrap();
            w.write_sample((tone * 20_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let imported = crate::assets::import_wav(tmp.path(), &wav).unwrap();
        let mut project = Project::new("t");
        let track = Track::new(TrackId::new(), "Mix", TrackKind::Audio);
        let src_tid = track.id.clone();
        project.tracks.push(track);
        let clip_id = ClipId::new();
        for c in crate::assets::audio_clip_commands(
            &project,
            &src_tid,
            &imported,
            clip_id.clone(),
            Tick(0),
            "mix",
        )
        .unwrap()
        {
            project.apply(&c).unwrap();
        }
        let s = separate_clip_commands(&project, tmp.path(), &clip_id, SeparateMethod::Builtin)
            .unwrap();
        project.apply(&Command::batch("sep", s.commands)).unwrap();
        // 分けたパートの素材はステレオで、打楽器は左、音程楽器は右に寄っている
        let energy_lr = |ti: usize| {
            let glaux_core::ClipContent::Audio { asset, .. } = &project.tracks[ti].clips[0].content
            else {
                panic!()
            };
            let meta = &project.assets[asset];
            assert_eq!(meta.channels, 2);
            let d = glaux_engine::load_wav(&tmp.path().join(&meta.path)).unwrap();
            let (l, r) = d.left_right();
            let e = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
            (e(&l), e(&r))
        };
        let (pl, pr) = energy_lr(1);
        let (hl, hr) = energy_lr(2);
        assert!(pl > pr * 3.0, "打楽器は左: {pl} / {pr}");
        assert!(hr > hl * 3.0, "音程楽器は右: {hl} / {hr}");
    }

    #[test]
    fn builtin_separation_adds_two_tracks_and_mutes_source() {
        let tmp = tempfile::tempdir().unwrap();
        // 持続音 + クリックの WAV
        let sr = 22_050u32;
        let wav = tmp.path().join("mix.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..sr {
            let t = i as f32 / sr as f32;
            let mut x = 0.3 * (std::f32::consts::TAU * 220.0 * t).sin();
            if i % (sr / 4) < 100 {
                x += 0.5;
            }
            w.write_sample((x * 20_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let imported = crate::assets::import_wav(tmp.path(), &wav).unwrap();
        let mut project = Project::new("t");
        let track = Track::new(TrackId::new(), "Mix", TrackKind::Audio);
        let src_tid = track.id.clone();
        project.tracks.push(track);
        let clip_id = ClipId::new();
        for c in crate::assets::audio_clip_commands(
            &project,
            &src_tid,
            &imported,
            clip_id.clone(),
            Tick(0),
            "mix",
        )
        .unwrap()
        {
            project.apply(&c).unwrap();
        }

        let s = separate_clip_commands(&project, tmp.path(), &clip_id, SeparateMethod::Builtin)
            .unwrap();
        let batch = Command::batch("sep", s.commands);
        project.apply(&batch).unwrap();
        assert!(project.is_valid(), "{:?}", project.validate());
        assert_eq!(project.tracks.len(), 3);
        assert_eq!(project.tracks[1].name, "Mix 打楽器");
        assert_eq!(project.tracks[2].name, "Mix 音程楽器");
        assert!(project.tracks[0].mute, "元のトラックはミュート");
        let src_len = project.tracks[0].clips[0].length;
        for t in &project.tracks[1..] {
            assert_eq!(t.clips.len(), 1);
            assert_eq!(t.clips[0].length, src_len, "元と同じ長さ");
        }
        assert_eq!(s.tracks.len(), 2);
    }

    #[test]
    fn method_parses() {
        assert_eq!(SeparateMethod::parse(None), Ok(SeparateMethod::Builtin));
        assert_eq!(
            SeparateMethod::parse(Some("demucs")),
            Ok(SeparateMethod::Demucs)
        );
        assert!(SeparateMethod::parse(Some("spleeter")).is_err());
    }
}
