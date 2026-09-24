//! `.glaux` フォルダを Godot の `FileAccess` で読み込む(書き出したゲームの .pck の中でも読める)。

use glaux_core::{PluginSource, Project};
use glaux_engine::data::{build_playback_data, load_wav_bytes, PlaybackData, SampleBank};
use glaux_engine::timeline::Timeline;
use godot::classes::FileAccess;

/// 読み込んだ曲。
pub struct LoadedSong {
    pub timeline: Timeline,
    pub data: PlaybackData,
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

    let mut bank = SampleBank::default();
    bank.sync_with(
        &project,
        &mut |rel| load_wav_bytes(&read_bytes(&join(dir, rel))?),
        // SoundFont は曲フォルダの soundfonts/ か res://soundfonts/ に置く
        &mut |file| {
            let local = join(dir, &format!("soundfonts/{file}"));
            let shared = format!("res://soundfonts/{file}");
            let path = if FileAccess::file_exists(&local) {
                local
            } else {
                shared
            };
            glaux_engine::sf2::load_font_bytes(&read_bytes(&path)?)
        },
    );
    let data = build_playback_data(&project, sample_rate, &bank);
    Ok(LoadedSong {
        timeline: Timeline::from_project(&project),
        data,
        warnings,
    })
}

/// ゲームで鳴らせないもの(CLAP プラグイン)を外し、注意を返す。
fn prepare(project: &mut Project) -> Vec<String> {
    let mut warnings = Vec::new();
    for t in &mut project.tracks {
        if let Some(d) = &t.device {
            if matches!(d.source, PluginSource::Clap { .. }) {
                // プラグインが無いと既定のシンセの音で鳴ってしまうので、無音にする
                t.mute = true;
                warnings.push(format!(
                    "トラック「{}」の音源は CLAP プラグインのため、ゲームでは鳴りません(Glaux で音声に書き出して音声クリップにしてください)",
                    t.name
                ));
            }
        }
        let before = t.effects.len();
        t.effects
            .retain(|e| !matches!(e.source, PluginSource::Clap { .. }));
        if t.effects.len() != before {
            warnings.push(format!(
                "トラック「{}」の CLAP エフェクトはゲームでは掛かりません",
                t.name
            ));
        }
    }
    let before = project.master.effects.len();
    project
        .master
        .effects
        .retain(|e| !matches!(e.source, PluginSource::Clap { .. }));
    if project.master.effects.len() != before {
        warnings.push("マスターの CLAP エフェクトはゲームでは掛かりません".to_owned());
    }
    warnings
}
