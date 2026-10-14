//! `.glaux` フォルダを Godot の `FileAccess` で読み込む(書き出したゲームの .pck の中でも読める)。

use glaux_core::{PluginSource, Project};
use glaux_engine::data::{build_playback_data, load_wav_bytes, PlaybackData, SampleBank};
use glaux_engine::timeline::Timeline;
use godot::classes::FileAccess;

/// 読み込んだ曲。
pub struct LoadedSong {
    pub timeline: Timeline,
    pub data: PlaybackData,
    /// 曲(ゲームの中で表情・テンポなどを変えるとき、ここに Command を当てて組み直す。ファイルは変えない)
    pub project: Project,
    /// 読み込んだ音声・SoundFont(組み直すときに使い回す)
    pub bank: SampleBank,
    /// キーと小節ごとのコード(ノートからの推定。効果音の音程を曲に合わせるのに使う)
    pub harmony: glaux_core::harmony::HarmonyAnalysis,
    /// 読み込みで気づいた注意(CLAP の音源など)
    pub warnings: Vec<String>,
}

fn join(dir: &str, rel: &str) -> String {
    format!(
        "{}/{}",
        dir.trim_end_matches('/'),
        rel.trim_start_matches('/')
    )
}

fn read_bytes(path: &str) -> Result<Vec<u8>, String> {
    if !FileAccess::file_exists(path) {
        return Err(format!("ファイルがありません: {path}"));
    }
    Ok(FileAccess::get_file_as_bytes(path).to_vec())
}

/// `dir`(例 `res://songs/stage1.glaux`)の曲を、`sample_rate` で鳴らす準備をする。
pub fn load(dir: &str, sample_rate: f64) -> Result<LoadedSong, String> {
    let json_path = join(dir, "project.json");
    let bytes = read_bytes(&json_path)?;
    let json =
        String::from_utf8(bytes).map_err(|_| format!("UTF-8 ではありません: {json_path}"))?;
    let mut project =
        Project::from_json(&json).map_err(|e| format!("project.json を読めません: {e}"))?;
    let warnings = prepare(&mut project);

    let mut warnings = warnings;
    let mut bank = SampleBank::default();
    let (data, timeline, missing) = rebuild(dir, &project, &mut bank, sample_rate);
    warnings.extend(missing);
    Ok(LoadedSong {
        timeline,
        harmony: glaux_core::harmony::analyze(&project, None, None),
        data,
        project,
        bank,
        warnings,
    })
}

/// 曲から鳴らすデータと時間軸を作る(足りない音声・SoundFont は `dir` から読む)。
/// ゲームの中で曲を変えたとき(`GlauxPlayer.apply_command` など)も、これで組み直す。
/// 3 つ目は読めなかった音声・SoundFont の注意(その音だけ鳴らない。どのトラックのどのファイルか)
pub fn rebuild(
    dir: &str,
    project: &Project,
    bank: &mut SampleBank,
    sample_rate: f64,
) -> (PlaybackData, Timeline, Vec<String>) {
    let mut bad_wavs: Vec<(String, String)> = Vec::new();
    let mut bad_fonts: Vec<(String, String)> = Vec::new();
    bank.sync_with(
        project,
        &mut |rel| {
            let r = read_bytes(&join(dir, rel)).and_then(|b| load_wav_bytes(&b));
            if let Err(e) = &r {
                bad_wavs.push((rel.to_owned(), e.clone()));
            }
            r
        },
        // SoundFont は曲フォルダの soundfonts/ か res://soundfonts/ に置く
        &mut |file| {
            let local = join(dir, &format!("soundfonts/{file}"));
            let shared = format!("res://soundfonts/{file}");
            let path = if FileAccess::file_exists(&local) {
                local
            } else {
                shared
            };
            let r = read_bytes(&path).and_then(|b| glaux_engine::sf2::load_font_bytes(&b));
            if let Err(e) = &r {
                bad_fonts.push((file.to_owned(), e.clone()));
            }
            r
        },
    );
    (
        build_playback_data(project, sample_rate, bank),
        Timeline::from_project(project),
        missing_warnings(project, &bad_wavs, &bad_fonts),
    )
}

/// 読めなかった音声・SoundFont を、使っているトラックの名前つきの注意にする(#61)
fn missing_warnings(
    project: &Project,
    wavs: &[(String, String)],
    fonts: &[(String, String)],
) -> Vec<String> {
    let names = |hit: &dyn Fn(&glaux_core::Track) -> bool| {
        let v: Vec<String> = project
            .tracks
            .iter()
            .filter(|t| hit(t))
            .map(|t| format!("「{}」", t.name))
            .collect();
        if v.is_empty() {
            "曲".to_owned()
        } else {
            format!("トラック{}", v.join(""))
        }
    };
    let devices = |t: &glaux_core::Track| -> Vec<glaux_core::PluginSource> {
        t.device
            .iter()
            .chain(t.layers.iter().map(|l| &l.device))
            .map(|d| d.source.clone())
            .collect()
    };
    let mut out = Vec::new();
    for (rel, e) in wavs {
        let ids: Vec<&glaux_core::AssetId> = project
            .assets
            .iter()
            .filter(|(_, a)| a.path == *rel)
            .map(|(id, _)| id)
            .collect();
        let uses = |t: &glaux_core::Track| {
            let clip = t.clips.iter().any(|c| {
                matches!(&c.content, glaux_core::ClipContent::Audio { asset, .. } if ids.contains(&asset))
            });
            let dev = devices(t).iter().any(|s| {
                matches!(s, glaux_core::PluginSource::Sampler { asset } if ids.contains(&asset))
            });
            let params = t
                .device
                .iter()
                .chain(t.layers.iter().map(|l| &l.device))
                .flat_map(|d| d.params.values())
                .chain(t.effects.iter().flat_map(|e| e.params.values()))
                .any(|v| matches!(v, glaux_core::ParamValue::Enum(s) if ids.iter().any(|id| id.to_string() == *s)));
            clip || dev || params
        };
        out.push(format!(
            "{}の音声 {rel} を読めないため、その音は鳴りません({e})",
            names(&uses)
        ));
    }
    for (file, e) in fonts {
        let uses = |t: &glaux_core::Track| {
            devices(t).iter().any(
                |s| matches!(s, glaux_core::PluginSource::Sf2 { soundfont, .. } if soundfont == file),
            )
        };
        out.push(format!(
            "{}の SoundFont「{file}」を読めないため、その音は鳴りません。曲フォルダの soundfonts/ か res://soundfonts/ に置いてください({e})",
            names(&uses)
        ));
    }
    out
}

/// ゲームで鳴らせないもの(CLAP プラグイン)を外し、注意を返す(SFZ の音源も鳴らない)。
fn prepare(project: &mut Project) -> Vec<String> {
    let mut warnings = Vec::new();
    // ソロは「聴き方」(アプリで聴き比べるためのもの)。ソロのまま保存した曲でも、ゲームでは曲の全体を鳴らす(#60)。
    // ミュートは曲の一部として残す
    let soloed: Vec<String> = project
        .tracks
        .iter()
        .filter(|t| t.solo)
        .map(|t| format!("「{}」", t.name))
        .collect();
    if !soloed.is_empty() {
        for t in &mut project.tracks {
            t.solo = false;
        }
        warnings.push(format!(
            "トラック{}がソロのまま保存されていたので、ソロを外して曲の全体を鳴らします(ミュートはそのまま)",
            soloed.join("")
        ));
    }
    for t in &mut project.tracks {
        if let Some(d) = &t.device {
            if matches!(d.source, PluginSource::Clap { .. }) {
                // プラグインが無いと既定のシンセの音で鳴ってしまうので、無音にする
                t.mute = true;
                warnings.push(format!(
                    "トラック「{}」の音源は CLAP プラグインのため、ゲームでは鳴りません(Glaux の「書き出し → ゲーム用」で書き出すか、トラックの見出しを右クリック →「音声にする(フリーズ)」で音声にしてください)",
                    t.name
                ));
            }
        }
        // SFZ の音源は Glaux のライブラリのフォルダから読むので、ゲームを遊ぶパソコンには無い
        if t.device
            .iter()
            .chain(t.layers.iter().map(|l| &l.device))
            .any(|d| matches!(d.source, PluginSource::Sfz { .. }))
        {
            warnings.push(format!(
                "トラック「{}」の音源は SFZ のため、ゲームでは鳴りません(Glaux の「書き出し → ゲーム用」で書き出すと音声にします)",
                t.name
            ));
        }
        // CLAP エフェクトはゲームでは用意できないので素通しになる(つながりを保つため、消さずに残す)
        if t.effects
            .iter()
            .any(|e| matches!(e.source, PluginSource::Clap { .. }))
        {
            warnings.push(format!(
                "トラック「{}」の CLAP エフェクトはゲームでは掛かりません",
                t.name
            ));
        }
    }
    if project
        .master
        .effects
        .iter()
        .any(|e| matches!(e.source, PluginSource::Clap { .. }))
    {
        warnings.push("マスターの CLAP エフェクトはゲームでは掛かりません".to_owned());
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{AssetId, Track, TrackId, TrackKind};

    #[test]
    fn solo_saved_in_the_song_is_cleared_and_mute_is_kept() {
        let mut p = Project::new("t");
        let mut a = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        a.solo = true;
        let mut b = Track::new(TrackId::new(), "Pad", TrackKind::Midi);
        b.mute = true;
        p.tracks.push(a);
        p.tracks.push(b);
        let w = prepare(&mut p);
        assert!(p.tracks.iter().all(|t| !t.solo));
        assert!(p.tracks[1].mute, "ミュートは曲の一部として残す");
        assert!(
            w.iter()
                .any(|w| w.contains("「Lead」") && w.contains("ソロ")),
            "{w:?}"
        );
        // ソロが無ければ何も言わない
        assert!(prepare(&mut p).is_empty());
    }

    #[test]
    fn missing_files_name_the_tracks_that_use_them() {
        let mut p = Project::new("t");
        let mut keys = Track::new(TrackId::new(), "Piano", TrackKind::Midi);
        keys.device = Some(glaux_core::Device {
            source: PluginSource::Sf2 {
                soundfont: "GeneralUser.sf2".into(),
                bank: 0,
                preset: 0,
            },
            params: Default::default(),
        });
        p.tracks.push(keys);
        let id = AssetId::parse("sha256:ab12").unwrap();
        p.assets.insert(
            id.clone(),
            glaux_core::Asset {
                path: "audio/vox.wav".into(),
                sample_rate: 48000,
                channels: 1,
                frames: 48000,
            },
        );
        let mut vox = Track::new(TrackId::new(), "Vox", TrackKind::Audio);
        vox.clips.push(glaux_core::Clip::new_audio(
            glaux_core::ClipId::new(),
            "vox",
            glaux_core::Tick(0),
            glaux_core::Tick(3840),
            id,
        ));
        p.tracks.push(vox);
        let w = missing_warnings(
            &p,
            &[("audio/vox.wav".into(), "ファイルがありません".into())],
            &[("GeneralUser.sf2".into(), "ファイルがありません".into())],
        );
        assert_eq!(w.len(), 2);
        assert!(
            w[0].contains("「Vox」") && w[0].contains("audio/vox.wav"),
            "{w:?}"
        );
        assert!(
            w[1].contains("「Piano」") && w[1].contains("GeneralUser.sf2"),
            "{w:?}"
        );
    }
}
