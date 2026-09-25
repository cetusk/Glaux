//! 書き出し(アプリの書き出し画面と MCP の export_audio で共通)。
//!
//! 曲全体(ミックス)か、トラックごと(ステム)を WAV に書く。形式・範囲・音量の目標を選べる。
//! 以前は 48kHz / 16bit / 曲全体 / プロジェクトの export/ 固定だった。

use glaux_core::{Project, Tick, TrackKind};
use glaux_engine::{ExportOptions, SampleBank};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 書き出しの依頼(JSON でも受け取る)。
#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ExportRequest {
    /// 書き出し先のファイル(ステムならフォルダ)。省略でプロジェクトの export/ に日時付きの名前
    #[serde(default)]
    pub path: Option<String>,
    /// 44100 か 48000(既定 48000)
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// 16 / 24 / 32(32 は浮動小数。既定 16)
    #[serde(default)]
    pub bits: Option<u16>,
    /// 範囲(tick)。両方省略で曲全体
    #[serde(default)]
    pub start_tick: Option<u64>,
    #[serde(default)]
    pub end_tick: Option<u64>,
    /// 音量の目標(統合ラウドネス LUFS。例: 配信 -14、放送 -23)。省略でそのまま。ステムでは使わない
    #[serde(default)]
    pub loudness_lufs: Option<f64>,
    /// true でトラックごと(ステム)に書き出す(マスターのエフェクトは通さない。センド先のバスの響きは含む)
    #[serde(default)]
    pub stems: Option<bool>,
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.trim_matches('_').is_empty() {
        "untitled".to_owned()
    } else {
        s
    }
}

fn options(project: &Project, req: &ExportRequest) -> Result<ExportOptions, String> {
    let sample_rate = req.sample_rate.unwrap_or(48_000);
    if !matches!(sample_rate, 44_100 | 48_000) {
        return Err("sample_rate は 44100 か 48000".to_owned());
    }
    let bits = req.bits.unwrap_or(16);
    if !matches!(bits, 16 | 24 | 32) {
        return Err("bits は 16 / 24 / 32".to_owned());
    }
    let range_secs = match (req.start_tick, req.end_tick) {
        (None, None) => None,
        (s, e) => {
            let s = s.unwrap_or(0);
            let e = e.unwrap_or_else(|| project.end().0);
            if e <= s {
                return Err("end_tick は start_tick より大きくすること".to_owned());
            }
            let tm = &project.tempo_map;
            Some((tm.tick_to_seconds(Tick(s)), tm.tick_to_seconds(Tick(e))))
        }
    };
    if let Some(l) = req.loudness_lufs {
        if !(-40.0..=-5.0).contains(&l) {
            return Err("loudness_lufs は -40〜-5".to_owned());
        }
    }
    Ok(ExportOptions {
        sample_rate,
        bits,
        range_secs,
        target_lufs: req.loudness_lufs,
        ..Default::default()
    })
}

/// 書き出す(時間がかかるので、呼び出し側でブロッキングのスレッドに載せる)。結果は書いたファイルと測定値
pub fn run(
    project: &Project,
    project_dir: &Path,
    req: &ExportRequest,
    bank: &SampleBank,
) -> Result<Value, String> {
    let opts = options(project, req)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let title = sanitize(&project.meta.title);
    if req.stems.unwrap_or(false) {
        let dir = req.path.as_ref().map(PathBuf::from).unwrap_or_else(|| {
            project_dir
                .join("export")
                .join(format!("{title}_{stamp}_stems"))
        });
        let mut files = Vec::new();
        for (i, t) in project.tracks.iter().enumerate() {
            if t.kind == TrackKind::Bus || t.clips.is_empty() {
                continue;
            }
            let stem = crate::bounce::stem_project(project, t);
            let stereo = match glaux_engine::render(
                &stem,
                opts.sample_rate as f64,
                bank,
                opts.range_secs,
                false,
            ) {
                Ok(s) => s,
                Err(glaux_engine::ExportError::Empty) => continue, // 鳴る音が無いトラック
                Err(e) => return Err(format!("「{}」を描き出せません: {e}", t.name)),
            };
            let path = dir.join(format!("{:02}_{}.wav", i + 1, sanitize(&t.name)));
            glaux_engine::write_wav(&path, &stereo, opts.sample_rate, opts.bits)
                .map_err(|e| e.to_string())?;
            files.push(json!({ "track": t.name, "path": path.to_string_lossy() }));
        }
        if files.is_empty() {
            return Err("書き出せるトラックがありません".to_owned());
        }
        return Ok(json!({ "stems": files, "folder": dir.to_string_lossy() }));
    }
    let path = req.path.as_ref().map(PathBuf::from).unwrap_or_else(|| {
        project_dir
            .join("export")
            .join(format!("{title}_{stamp}.wav"))
    });
    let report =
        glaux_engine::export_audio(project, &path, &opts, bank).map_err(|e| e.to_string())?;
    let mut v = serde_json::to_value(&report).map_err(|e| e.to_string())?;
    v["path"] = json!(path.to_string_lossy());
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Articulation, Clip, ClipContent, ClipId, Note, NoteId, Track, TrackId};

    fn song() -> Project {
        let mut p = Project::new("曲 1");
        for (name, pitch) in [("Bass", 40u8), ("Lead", 72u8)] {
            let mut t = Track::new(TrackId::new(), name, TrackKind::Midi);
            let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
            if let ClipContent::Midi { notes, .. } = &mut c.content {
                notes.push(Note {
                    id: NoteId::new(),
                    pos: Tick(0),
                    dur: Tick(1920),
                    pitch,
                    vel: 100,
                    articulation: Articulation::Normal,
                    pitch_curve: vec![],
                    glide_ms: None,
                });
            }
            t.clips.push(c);
            p.tracks.push(t);
        }
        p
    }

    #[test]
    fn exports_mix_and_stems() {
        let tmp = tempfile::tempdir().unwrap();
        let p = song();
        let v = run(
            &p,
            tmp.path(),
            &ExportRequest {
                bits: Some(24),
                ..Default::default()
            },
            &Default::default(),
        )
        .unwrap();
        let path = v["path"].as_str().unwrap();
        assert!(path.contains("export"), "{path}");
        assert_eq!(
            hound::WavReader::open(path).unwrap().spec().bits_per_sample,
            24
        );

        let v = run(
            &p,
            tmp.path(),
            &ExportRequest {
                stems: Some(true),
                ..Default::default()
            },
            &Default::default(),
        )
        .unwrap();
        let stems = v["stems"].as_array().unwrap();
        assert_eq!(stems.len(), 2);
        assert!(stems[0]["path"].as_str().unwrap().ends_with("01_Bass.wav"));

        assert!(run(
            &p,
            tmp.path(),
            &ExportRequest {
                sample_rate: Some(22_050),
                ..Default::default()
            },
            &Default::default()
        )
        .is_err());
    }
}
