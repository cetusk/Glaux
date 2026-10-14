//! ゲーム用に書き出す(Glaux for Godot で鳴らすための曲の写しを作る)。
//!
//! ゲームの中では CLAP プラグインと SFZ の音源は鳴らず(遊ぶ人のパソコンに無い)、CLAP のエフェクトは素通しになる。
//! 写しの中でそれらのトラックを音声にし(フリーズと同じ。エフェクト・音量・パン・送り先のバスの響きまで込み)、
//! 使っている SoundFont を写しの `soundfonts/` に入れ、ソロを外す。元の曲は変えない。
//! バス・マスターに挿した CLAP のエフェクトは音声にできないので、知らせるだけにする。
//! 配るときに約束(ライセンス)を確かめられるよう、使ったプラグインと SoundFont も一覧にする。

use glaux_core::{PluginSource, Project, Track, TrackKind};
use std::path::{Path, PathBuf};

/// 書き出しの結果。
#[derive(Debug, Default, serde::Serialize)]
pub struct GameExport {
    /// 書き出した曲のフォルダ
    pub dir: PathBuf,
    /// 音声にしたトラック(元の名前)
    pub frozen: Vec<String>,
    /// 写しに入れた SoundFont
    pub soundfonts: Vec<String>,
    /// 見つからず入れられなかった SoundFont(ゲームでその音は鳴らない)
    pub missing_soundfonts: Vec<String>,
    /// 音声にできず、ゲームでは掛からない CLAP のエフェクト(バス・マスター)
    pub unbaked_effects: Vec<String>,
    /// ソロを外したトラック
    pub solo_cleared: Vec<String>,
    /// 使っていたプラグイン(CLAP の ID)。配るときの約束の確かめ用
    pub plugins: Vec<String>,
}

fn is_clap(s: &PluginSource) -> bool {
    matches!(s, PluginSource::Clap { .. })
}

/// ゲームで鳴らないので音声にするトラック: 音源が CLAP か SFZ(層も含む)、または CLAP のエフェクトを挿したもの
fn needs_freeze(t: &Track) -> bool {
    if t.kind == TrackKind::Bus {
        return false;
    }
    let dev = t
        .device
        .iter()
        .chain(t.layers.iter().map(|l| &l.device))
        .any(|d| is_clap(&d.source) || matches!(d.source, PluginSource::Sfz { .. }));
    dev || t.effects.iter().any(|e| is_clap(&e.source))
}

/// 曲のフォルダの名前に使える形にする(ファイル名に使えない文字を外す)
fn folder_name(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_owned();
    if s.is_empty() {
        "Song".to_owned()
    } else {
        s
    }
}

/// `project`(フォルダ `project_dir`)のゲーム用の写しを `dest` の中に `<曲名>.glaux` として作る。
/// 同じ名前の曲のフォルダが既にあれば作り直す(`project.json` の無いフォルダは消さずにエラー)。
/// `sf2_dir` は SoundFont のライブラリのフォルダ
pub fn export_for_game(
    project: &Project,
    project_dir: &Path,
    dest: &Path,
    sf2_dir: &Path,
) -> Result<GameExport, String> {
    let out = dest.join(format!("{}.glaux", folder_name(&project.meta.title)));
    if out.exists() {
        if !out.join("project.json").is_file() {
            return Err(glaux_core::tr!(
                "{} は曲のフォルダではないので上書きしません。別の場所を選んでください",
                "{} is not a song folder, so it won't be overwritten. Choose another location",
                out.display()
            ));
        }
        if out.canonicalize().ok() == project_dir.canonicalize().ok() {
            return Err(glaux_core::tr!(
                "元の曲と同じ場所には書き出せません",
                "Can't export over the original song"
            ));
        }
        std::fs::remove_dir_all(&out).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(out.join("audio")).map_err(|e| e.to_string())?;

    let mut report = GameExport {
        dir: out.clone(),
        ..Default::default()
    };
    let mut p = project.clone();

    // 使っている音声を写す(フリーズで描き出すときは元のフォルダから読む)
    for asset in p.assets.values() {
        let src = project_dir.join(&asset.path);
        if src.is_file() {
            let to = out.join(&asset.path);
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&src, &to).map_err(|e| e.to_string())?;
        }
    }

    // ソロは聴き方なので外す(ゲームでは全体を鳴らす)
    for t in &mut p.tracks {
        if t.solo {
            t.solo = false;
            report.solo_cleared.push(t.name.clone());
        }
    }

    // 使っているプラグイン(約束の確かめ用)
    let mut plugins: Vec<String> = p
        .tracks
        .iter()
        .flat_map(|t| {
            t.device
                .iter()
                .chain(t.layers.iter().map(|l| &l.device))
                .map(|d| &d.source)
                .chain(t.effects.iter().map(|e| &e.source))
        })
        .chain(p.master.effects.iter().map(|e| &e.source))
        .filter_map(|s| match s {
            PluginSource::Clap { plugin_id, .. } => Some(plugin_id.clone()),
            _ => None,
        })
        .collect();
    plugins.sort();
    plugins.dedup();
    report.plugins = plugins;

    // ゲームで鳴らないトラックを音声にする(元のトラックはミュートされて残る)
    let targets: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| needs_freeze(t))
        .map(|t| (t.id.clone(), t.name.clone()))
        .collect();
    if !targets.is_empty() {
        let bank = glaux_engine::SampleBank::for_offline(project, project_dir);
        for (id, name) in targets {
            let b = crate::bounce::bounce_track(&p, &out, &id, &bank)?;
            for c in &b.commands {
                p.apply(c).map_err(|e| e.to_string())?;
            }
            report.frozen.push(name);
        }
    }

    // バス・マスターの CLAP のエフェクトは音声にできない
    for t in p.tracks.iter().filter(|t| t.kind == TrackKind::Bus) {
        if t.effects.iter().any(|e| is_clap(&e.source)) {
            report
                .unbaked_effects
                .push(glaux_core::tr!("バス「{}」", "Bus \"{}\"", t.name));
        }
    }
    if p.master.effects.iter().any(|e| is_clap(&e.source)) {
        report
            .unbaked_effects
            .push(glaux_core::tr!("マスター", "Master").to_owned());
    }

    // 使っている SoundFont を写しに入れる(ゲームは曲のフォルダの soundfonts/ から読む)
    let mut fonts: Vec<String> = p
        .tracks
        .iter()
        .filter(|t| !t.mute)
        .flat_map(|t| t.device.iter().chain(t.layers.iter().map(|l| &l.device)))
        .filter_map(|d| match &d.source {
            PluginSource::Sf2 { soundfont, .. } => Some(soundfont.clone()),
            _ => None,
        })
        .collect();
    fonts.sort();
    fonts.dedup();
    for f in fonts {
        let src = sf2_dir.join(&f);
        if src.is_file() {
            let to = out.join("soundfonts").join(&f);
            if let Some(dir) = to.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&src, &to).map_err(|e| e.to_string())?;
            report.soundfonts.push(f);
        } else {
            report.missing_soundfonts.push(f);
        }
    }

    // 描き出しの途中のファイルは入れない
    let _ = std::fs::remove_dir_all(out.join("cache"));
    let json = p.to_json().map_err(|e| e.to_string())?;
    std::fs::write(out.join("project.json"), json).map_err(|e| e.to_string())?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Device, Effect, FxId, TrackId};

    #[test]
    fn folder_names_are_safe() {
        assert_eq!(folder_name("Stage 1: Boss?"), "Stage 1_ Boss_");
        assert_eq!(folder_name("  "), "Song");
        assert_eq!(folder_name("曲."), "曲");
    }

    #[test]
    fn exports_a_copy_with_solo_cleared_and_fonts_listed() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let fonts = tempfile::tempdir().unwrap();
        std::fs::write(fonts.path().join("Piano.sf2"), b"dummy").unwrap();
        let mut p = Project::new("Stage1");
        let mut lead = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        lead.solo = true;
        lead.device = Some(Device {
            source: PluginSource::Sf2 {
                soundfont: "Piano.sf2".into(),
                bank: 0,
                preset: 0,
            },
            params: Default::default(),
        });
        p.tracks.push(lead);
        let mut bus = Track::new(TrackId::new(), "Verb", TrackKind::Bus);
        let mut fx = Effect::builtin(FxId::new(), "reverb");
        fx.source = PluginSource::Clap {
            plugin_id: "org.example.verb".into(),
            state: None,
        };
        bus.effects.push(fx);
        p.tracks.push(bus);
        std::fs::write(src.path().join("project.json"), p.to_json().unwrap()).unwrap();

        let r = export_for_game(&p, src.path(), dest.path(), fonts.path()).unwrap();
        assert_eq!(r.dir, dest.path().join("Stage1.glaux"));
        assert_eq!(r.solo_cleared, vec!["Lead".to_owned()]);
        assert_eq!(r.soundfonts, vec!["Piano.sf2".to_owned()]);
        assert!(r.dir.join("soundfonts/Piano.sf2").is_file());
        assert_eq!(r.plugins, vec!["org.example.verb".to_owned()]);
        assert_eq!(r.unbaked_effects.len(), 1, "バスの CLAP は音声にできない");
        assert!(r.frozen.is_empty());
        let copy =
            Project::from_json(&std::fs::read_to_string(r.dir.join("project.json")).unwrap())
                .unwrap();
        assert!(copy.tracks.iter().all(|t| !t.solo));
        assert!(!r.dir.join("history.jsonl").exists());
        // 元の曲は変わらない
        assert!(p.tracks[0].solo);
        // もう一度書き出すと作り直す
        assert!(export_for_game(&p, src.path(), dest.path(), fonts.path()).is_ok());
        // CLAP のエフェクトを挿したトラックは音声にする(プラグインが無くても描き出せる。エフェクトは素通し)
        let mut q = p.clone();
        let mut keys = Track::new(TrackId::new(), "Keys", TrackKind::Midi);
        keys.device = Some(Device {
            source: PluginSource::Builtin {
                name: "subtractive".into(),
            },
            params: Default::default(),
        });
        let mut clip = glaux_core::Clip::new_midi(
            glaux_core::ClipId::new(),
            "k",
            glaux_core::Tick(0),
            glaux_core::Tick(3840),
        );
        clip.notes_mut().unwrap().push(glaux_core::Note {
            locked: false,
            id: glaux_core::NoteId::new(),
            pos: glaux_core::Tick(0),
            dur: glaux_core::Tick(960),
            pitch: 60,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
            vibrato: None,
            volume_curve: vec![],
            brightness_curve: vec![],
            condition: None,
        });
        keys.clips.push(clip);
        let mut cfx = Effect::builtin(FxId::new(), "eq");
        cfx.source = PluginSource::Clap {
            plugin_id: "org.example.eq".into(),
            state: None,
        };
        keys.effects.push(cfx);
        q.tracks.push(keys);
        let r = export_for_game(&q, src.path(), dest.path(), fonts.path()).unwrap();
        assert_eq!(r.frozen, vec!["Keys".to_owned()]);
        let copy =
            Project::from_json(&std::fs::read_to_string(r.dir.join("project.json")).unwrap())
                .unwrap();
        let audio = copy
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .expect("音声のトラック");
        assert!(!audio.clips.is_empty());
        assert!(
            copy.tracks.iter().find(|t| t.name == "Keys").unwrap().mute,
            "元はミュート"
        );
        assert!(
            copy.assets.values().all(|a| r.dir.join(&a.path).is_file()),
            "音声は写しの中に"
        );
        assert!(!r.dir.join("cache").exists());
        // 曲のフォルダでない同じ名前のフォルダは消さない
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(other.path().join("Stage1.glaux")).unwrap();
        assert!(export_for_game(&p, src.path(), other.path(), fonts.path()).is_err());
    }
}
