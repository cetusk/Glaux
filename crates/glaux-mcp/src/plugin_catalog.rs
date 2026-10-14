//! 無料のプラグインの入手(音源を選ぶ画面の「無料のプラグインを入手」)。
//!
//! Glaux の配布物にはプラグインを入れず、使う人が選んだときに、作者の公式の配布(GitHub のリリース)から取得して、
//! 管理者の権限が要らないユーザー用の CLAP のフォルダ(`<CLAP>/Glaux/<id>/`)に入れる。版と SHA-256 を決めておき、
//! 取得したファイルが本物かを確かめてから入れる(プラグインは Glaux の中で動くプログラムなので)。
//! 新しい版が出たら、ここの版・URL・大きさ・SHA-256 を書き換える。
//! アカウントやインストーラーが要るもの・再配布や自動の取得を認めていないものは、配布ページを開くだけ([`How::Page`])。
//! 取得できる圧縮ファイルは Windows 用なので、ほかの OS では配布ページを開く。

use std::io::Read;
use std::path::{Path, PathBuf};

/// 入手のしかた
#[derive(Clone, Copy, Debug)]
pub enum How {
    /// 公式の圧縮ファイルを取得して入れる(Windows)
    Archive {
        version: &'static str,
        url: &'static str,
        bytes: u64,
        sha256: &'static str,
        /// 圧縮ファイルの中の、入れるものが入っているフォルダ("" なら一番上)
        prefix: &'static str,
        /// 入れるもの(prefix からの相対。"/" で終わるものはフォルダごと)
        files: &'static [&'static str],
    },
    /// 配布ページを開くだけ
    Page,
}

/// 入手できるプラグイン
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    /// "instrument" / "effect" / "both"
    pub kind: &'static str,
    pub author: &'static str,
    /// 約束(SPDX の名前か Freeware)
    pub license: &'static str,
    /// 配布ページ(約束の確かめ・Page のときに開く所)
    pub homepage: &'static str,
    pub about_ja: &'static str,
    pub about_en: &'static str,
    /// すでに入っているかを見分ける名前(プラグインの一覧の名前)
    pub names: &'static [&'static str],
    pub how: How,
}

pub const CATALOG: &[Entry] = &[
    Entry {
        id: "surge-xt",
        name: "Surge XT",
        kind: "both",
        author: "Surge Synth Team",
        license: "GPL-3.0",
        homepage: "https://surge-synthesizer.github.io/",
        about_ja: "定番の無料シンセ(減算・ウェーブテーブル・FM など)と、エフェクト集の Surge XT Effects。プリセット約 2,900 個・波形などの素材込み",
        about_en: "A popular free synth (subtractive, wavetable, FM and more) plus the Surge XT Effects suite. Includes about 2,900 presets and its wavetables",
        names: &["Surge XT", "Surge XT Effects"],
        how: How::Archive {
            version: "1.3.4",
            url: "https://github.com/surge-synthesizer/releases-xt/releases/download/1.3.4/surge-xt-win64-1.3.4-portable-install.zip",
            bytes: 301_759_660,
            sha256: "a5f2376702ad218c0382640ed0c7b9ebb222ba2410a210879899c0f726859d8e",
            prefix: "Surge Synth Team/",
            // プリセット・波形(SurgeXTData)は .clap と同じフォルダに置く(Surge の持ち運び用の置き方)
            files: &["Surge XT.clap", "Surge XT Effects.clap", "SurgeXTData/"],
        },
    },
    Entry {
        id: "dexed",
        name: "Dexed",
        kind: "instrument",
        author: "Digital Suburban (asb2m10)",
        license: "GPL-3.0",
        homepage: "https://asb2m10.github.io/dexed/",
        about_ja: "ヤマハ DX7 系の 6 オペレーター FM シンセ。エレピ・ベル・ブラス・ベースの 80 年代の FM の音",
        about_en: "A Yamaha DX7-style 6-operator FM synth: 80s FM electric pianos, bells, brass and basses",
        names: &["Dexed"],
        how: How::Archive {
            version: "1.0.1",
            url: "https://github.com/asb2m10/dexed/releases/download/v1.0.1/Dexed-1.0.1-win.zip",
            bytes: 9_742_538,
            sha256: "1f118445f2c5c411636bb41ddf8b456db09af74cd38f9ede8a93a01020505f05",
            prefix: "",
            files: &["Dexed.clap", "LICENSE"],
        },
    },
    Entry {
        id: "ob-xf",
        name: "OB-Xf",
        kind: "instrument",
        author: "Surge Synth Team",
        license: "GPL-3.0",
        homepage: "https://github.com/surge-synthesizer/OB-Xf",
        about_ja: "オーバーハイム系のアナログシンセ。太いパッド・ブラス・リード",
        about_en: "An Oberheim-style analog synth: fat pads, brass and leads",
        names: &["OB-Xf"],
        how: How::Archive {
            version: "1.0.3",
            url: "https://github.com/surge-synthesizer/OB-Xf/releases/download/v1.0.3/ob-xf-Windows-v1.0.3.zip",
            bytes: 9_222_959,
            sha256: "149fda0649daf8a3aa0851801f92515f0dbf6203acf33c0edc7d3c445e80b1e7",
            prefix: "obxf_products/",
            files: &["OB-Xf.clap", "LICENSE"],
        },
    },
    Entry {
        id: "six-sines",
        name: "Six Sines",
        kind: "instrument",
        author: "Paul Walker (baconpaul)",
        license: "MIT",
        homepage: "https://github.com/baconpaul/six-sines",
        about_ja: "6 オペレーターの FM・位相変調シンセ。金属的なベル・キラキラした音・アグレッシブなベース",
        about_en: "A 6-operator FM / phase modulation synth: metallic bells, glassy tones and aggressive basses",
        names: &["Six Sines"],
        how: How::Archive {
            version: "1.2.1",
            url: "https://github.com/baconpaul/six-sines/releases/download/v1.2.1/six-sines-Windows-2026-07-20-18ecb36.zip",
            bytes: 22_398_434,
            sha256: "9f7c7fa5cb1c5e11b53c0a9306ee443149cc7bc0a109380dbbe1578f3a7b6e5d",
            prefix: "installer_copy/",
            files: &["Six Sines.clap"],
        },
    },
    Entry {
        id: "dragonfly-reverb",
        name: "Dragonfly Reverb",
        kind: "effect",
        author: "Michael Willis",
        license: "GPL-3.0",
        homepage: "https://michaelwillis.github.io/dragonfly-reverb/",
        about_ja: "ホール・ルーム・プレート・初期反射の 4 つのリバーブ",
        about_en: "Four reverbs: hall, room, plate and early reflections",
        names: &[
            "Dragonfly Hall Reverb",
            "Dragonfly Room Reverb",
            "Dragonfly Plate Reverb",
            "Dragonfly Early Reflections",
        ],
        how: How::Archive {
            version: "3.2.10",
            url: "https://github.com/michaelwillis/dragonfly-reverb/releases/download/3.2.10/dragonfly-reverb-3.2.10-win64.zip",
            bytes: 22_447_365,
            sha256: "ca5f35f9dcd9a33f06490d3c92f04061a1a798cb81257d5ca5851bdda0824c66",
            prefix: "dragonfly-reverb-3.2.10/",
            files: &[
                "DragonflyHallReverb.clap",
                "DragonflyRoomReverb.clap",
                "DragonflyPlateReverb.clap",
                "DragonflyEarlyReflections.clap",
            ],
        },
    },
    Entry {
        id: "vital",
        name: "Vital",
        kind: "instrument",
        author: "Matt Tytel",
        license: "Freeware",
        homepage: "https://vital.audio/",
        about_ja: "スペクトル変形のウェーブテーブルシンセ(VST3)。取得にアカウント登録が要るので、配布ページから入れてください",
        about_en: "A spectral-warping wavetable synth (VST3). Downloading needs an account, so install it from its page",
        names: &["Vital"],
        how: How::Page,
    },
    Entry {
        id: "ott",
        name: "OTT",
        kind: "effect",
        author: "Xfer Records",
        license: "Freeware",
        homepage: "https://xferrecords.com/freeware",
        about_ja: "定番の 3 バンドの上下コンプ(VST3)。配布ページから入れてください",
        about_en: "The well-known 3-band upward/downward compressor (VST3). Install it from its page",
        names: &["OTT"],
        how: How::Page,
    },
    Entry {
        id: "valhalla-supermassive",
        name: "Valhalla Supermassive",
        kind: "effect",
        author: "Valhalla DSP",
        license: "Freeware",
        homepage: "https://valhalladsp.com/shop/reverb/valhalla-supermassive/",
        about_ja: "広大なリバーブ・ディレイ(VST3)。インストーラーで入れるので、配布ページから入れてください",
        about_en: "Huge reverbs and delays (VST3). It uses an installer, so install it from its page",
        names: &["ValhallaSupermassive", "Valhalla Supermassive"],
        how: How::Page,
    },
    Entry {
        id: "kontakt-player",
        name: "Kontakt Player",
        kind: "instrument",
        author: "Native Instruments",
        license: "Freeware",
        homepage: "https://www.native-instruments.com/products/kontakt-player",
        about_ja: "サンプル音源のプレイヤー(VST3)。Native Access で入れ、音源はプラグインの画面で選びます",
        about_en: "A sample-library player (VST3). Install it with Native Access and choose libraries in the plugin window",
        names: &["Kontakt", "Kontakt 8", "Kontakt 7"],
        how: How::Page,
    },
    Entry {
        id: "odin2",
        name: "Odin 2",
        kind: "instrument",
        author: "TheWaveWarden",
        license: "GPL-3.0",
        homepage: "https://thewavewarden.com/odin2",
        about_ja: "24 声のセミモジュラーシンセ。Windows はインストーラーなので、配布ページから入れてください",
        about_en: "A 24-voice semi-modular synth. Windows uses an installer, so install it from its page",
        names: &["Odin2", "Odin 2"],
        how: How::Page,
    },
    Entry {
        id: "airwindows",
        name: "Airwindows Consolidated",
        kind: "effect",
        author: "Chris Johnson (Airwindows)",
        license: "MIT",
        homepage: "https://www.airwindows.com/consolidated/",
        about_ja: "数百の小さなエフェクト(テープ・コンソール・サチュレーション・リバーブなど)をまとめたもの。配布ページから入れてください",
        about_en: "Hundreds of small effects (tape, console, saturation, reverb and more) in one plugin. Install it from its page",
        names: &["Airwindows Consolidated"],
        how: How::Page,
    },
];

/// その名前のプラグイン
pub fn entry(id: &str) -> Option<&'static Entry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// この OS で取得して入れられるか(取得する圧縮ファイルは Windows 用)
pub fn installable(e: &Entry) -> bool {
    cfg!(windows) && matches!(e.how, How::Archive { .. })
}

/// 入れる場所の根(ユーザー用の CLAP のフォルダの下の Glaux)。Glaux のプラグインの探し方はここも探す
pub fn install_root() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| {
            PathBuf::from(d)
                .join("Programs")
                .join("Common")
                .join("CLAP")
                .join("Glaux")
        })
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library/Audio/Plug-Ins/CLAP")
                .join("Glaux")
        })
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".clap").join("Glaux"))
    }
}

/// Glaux が入れたか(入れた場所に入れるものがそろっているか)
pub fn installed_by_glaux(e: &Entry, root: &Path) -> bool {
    match e.how {
        How::Archive { files, .. } => files
            .iter()
            .filter(|f| f.ends_with(".clap"))
            .all(|f| root.join(e.id).join(f).is_file()),
        How::Page => false,
    }
}

/// 取得して入れる(すでに入っていれば何もしない)。`progress(受信済み, 全体)`。入れたフォルダを返す
pub fn install(id: &str, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, String> {
    let e = entry(id).ok_or_else(|| format!("知らないプラグインです: {id}"))?;
    if !installable(e) {
        return Err(glaux_core::tr!(
            "{} はこの OS では取得して入れられません。配布ページから入れてください",
            "{} can't be installed automatically on this OS. Install it from its page",
            e.name
        ));
    }
    let How::Archive {
        url, bytes, sha256, ..
    } = e.how
    else {
        unreachable!("installable は Archive だけ");
    };
    let root = install_root().ok_or("プラグインのフォルダが分かりません")?;
    if installed_by_glaux(e, &root) {
        return Ok(root.join(e.id));
    }
    // 取得の途中のファイルは設定フォルダの cache/plugins に置き、入れ終えたら消す
    let cache = glaux_engine::sf2::default_dir()
        .with_file_name("cache")
        .join("plugins");
    let archive = cache.join(format!("{}.zip", e.id));
    crate::models::download_verified(url, &archive, bytes, sha256, progress)
        .map_err(|err| format!("{} を取得できません: {err}", e.name))?;
    let r = extract(e, &archive, &root);
    let _ = std::fs::remove_file(&archive);
    r
}

/// 取得した圧縮ファイルから、入れるものだけを `root/<id>/` に取り出す(途中で失敗したら片付ける)
pub fn extract(e: &Entry, archive: &Path, root: &Path) -> Result<PathBuf, String> {
    let How::Archive { prefix, files, .. } = e.how else {
        return Err("取得して入れるプラグインではありません".into());
    };
    let dest = root.join(e.id);
    let tmp = root.join(format!(".{}.part", e.id));
    let _ = std::fs::remove_dir_all(&tmp);
    let res = (|| -> Result<(), String> {
        let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
        let mut n = 0;
        for i in 0..z.len() {
            let mut item = z.by_index(i).map_err(|e| e.to_string())?;
            let Some(name) = item.enclosed_name() else {
                continue; // フォルダの外を指す名前は取り出さない
            };
            let name = name.to_string_lossy().replace('\\', "/");
            let Some(rel) = name.strip_prefix(prefix) else {
                continue;
            };
            let wanted = files.iter().any(|w| {
                if w.ends_with('/') {
                    rel.starts_with(w)
                } else {
                    rel == *w
                }
            });
            if !wanted || item.is_dir() {
                continue;
            }
            let to = tmp.join(rel);
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            let mut out = std::fs::File::create(&to).map_err(|e| e.to_string())?;
            let mut buf = Vec::with_capacity(item.size() as usize);
            item.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            std::io::Write::write_all(&mut out, &buf).map_err(|e| e.to_string())?;
            n += 1;
        }
        if n == 0 {
            return Err("圧縮ファイルの中に入れるものが見つかりません".into());
        }
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())
    })();
    if let Err(err) = res {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(glaux_core::tr!(
            "{} を入れられませんでした: {err}",
            "Couldn't install {}: {err}",
            e.name
        ));
    }
    Ok(dest)
}

/// Glaux が入れたプラグインを外す(ほかの場所に入っているものは触らない)
pub fn remove(id: &str) -> Result<(), String> {
    let e = entry(id).ok_or_else(|| format!("知らないプラグインです: {id}"))?;
    let root = install_root().ok_or("プラグインのフォルダが分かりません")?;
    let dir = root.join(e.id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|err| {
            glaux_core::tr!(
                "{} を外せませんでした(Glaux を閉じてからもう一度試してください): {err}",
                "Couldn't remove {} (close Glaux and try again): {err}",
                e.name
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_are_consistent() {
        let mut ids = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(ids.insert(e.id), "重複: {}", e.id);
            assert!(e.homepage.starts_with("https://"), "{}", e.id);
            if let How::Archive {
                url,
                sha256,
                files,
                bytes,
                ..
            } = e.how
            {
                assert!(
                    url.starts_with("https://github.com/"),
                    "公式の GitHub から: {}",
                    e.id
                );
                assert_eq!(sha256.len(), 64, "{}", e.id);
                assert!(bytes > 0);
                assert!(files.iter().any(|f| f.ends_with(".clap")), "{}", e.id);
            }
        }
    }

    #[test]
    fn extracts_only_the_wanted_files_into_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("Surge Synth Team/Surge XT.clap", "clap"),
                ("Surge Synth Team/Surge XT Effects.clap", "fx"),
                ("Surge Synth Team/Surge XT.exe", "exe"),
                ("Surge Synth Team/SurgeXTData/patches_factory/a.fxp", "p"),
                (
                    "Surge Synth Team/Surge XT.vst3/Contents/x86_64-win/Surge XT.vst3",
                    "v",
                ),
                ("README.txt", "r"),
            ] {
                w.start_file(name, o).unwrap();
                std::io::Write::write_all(&mut w, body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let root = dir.path().join("CLAP").join("Glaux");
        let e = entry("surge-xt").unwrap();
        let out = extract(e, &zip_path, &root).unwrap();
        assert_eq!(out, root.join("surge-xt"));
        assert!(out.join("Surge XT.clap").is_file());
        assert!(out.join("Surge XT Effects.clap").is_file());
        assert!(
            out.join("SurgeXTData/patches_factory/a.fxp").is_file(),
            "素材は .clap の隣"
        );
        assert!(!out.join("Surge XT.exe").exists());
        assert!(!out.join("Surge XT.vst3").exists());
        assert!(installed_by_glaux(e, &root));
        // 入れるものが無い圧縮ファイルは、片付けてエラー
        let other = entry("dexed").unwrap();
        assert!(extract(other, &zip_path, &root).is_err());
        assert!(!root.join(".dexed.part").exists());
        assert!(!installed_by_glaux(other, &root));
    }
}
