//! 無料の SFZ 音源の取得(利用者の操作で。同梱はしない)。
//!
//! 取得元は sfzinstruments(GitHub)の各リポジトリの**決めたコミット**。まず .sfz(と `#include` の中身)を読み、
//! ふつうに弾いて鳴る region の波形だけを取る(リリース音・別のキースイッチ・ペダルを踏んだときの波形は取らない)。
//! 置き場所は SFZ ライブラリの `<id>/` で、中の並びはリポジトリのまま。作者とライセンスは `CREDITS.txt` に書く。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 取得できる音源 1 つ
#[derive(Clone, Copy, Debug)]
pub struct SfzPack {
    /// ライブラリの中のフォルダ名
    pub id: &'static str,
    pub name: &'static str,
    /// 楽器の種類(日本語)
    pub kind: &'static str,
    pub author: &'static str,
    pub license: &'static str,
    /// GitHub のリポジトリ(owner/name)と、取得するコミット
    pub repo: &'static str,
    pub commit: &'static str,
    /// 入れる .sfz(リポジトリの中のパス)
    pub programs: &'static [&'static str],
    /// 取得するおおよその大きさ(MB)
    pub approx_mb: u32,
    /// 選んだときに付ける調整つまみ(CC 番号, 値)。音源の既定より良く聞こえる設定
    pub cc: &'static [(u8, u8)],
}

/// 取得できる音源の一覧。ライセンスは CC0 か CC-BY(作者名の表示が要る。CREDITS.txt と README に書く)
pub const PACKS: &[SfzPack] = &[
    SfzPack {
        id: "osiris-piano",
        name: "Osiris Piano",
        kind: "ピアノ",
        author: "Versilian Studios・Karoryfer Samples",
        license: "CC0-1.0",
        repo: "sfzinstruments/Osiris_Piano",
        commit: "18c6afccb60cff458edbf7c394571783e074e1e9",
        programs: &["Programs/01-natural.sfz"],
        approx_mb: 84,
        cc: &[],
    },
    SfzPack {
        id: "e-pianos",
        name: "E-Pianos(CP80・Pianet T・Wurlitzer)",
        kind: "エレピ",
        author: "Greg Sullivan",
        license: "CC-BY-3.0",
        repo: "sfzinstruments/GregSullivan.E-Pianos",
        commit: "8c3e581acda3594b553948ff0222d4f84a698376",
        programs: &[
            "CP80/CP80.sfz",
            "Pianet T/Pianet T.sfz",
            "Wurlitzer EP200/Wurlitzer EP200.sfz",
        ],
        approx_mb: 21,
        cc: &[],
    },
    SfzPack {
        id: "big-rusty-drums",
        name: "Big Rusty Drums",
        kind: "ドラム",
        author: "Karoryfer Samples",
        license: "CC0-1.0",
        repo: "sfzinstruments/karoryfer.big-rusty-drums",
        commit: "f07ce00df34a46b6b08375be56fe116cf15782bc",
        programs: &["Programs/02-basic.sfz"],
        approx_mb: 153,
        // スネアを 3 半音上げ(カーブ 9 で 80 ≒ +3 半音)、トップ・天井のマイクを上げて snap を足す。
        // 既定のままだと生録りの緩いスネアに聞こえる(2026-09-29 に聞き比べて決めた)
        cc: &[(81, 100), (82, 100), (83, 70), (89, 80)],
    },
    SfzPack {
        id: "swagbass",
        name: "Swagbass",
        kind: "エレキベース",
        author: "Karoryfer Samples",
        license: "CC0-1.0",
        repo: "sfzinstruments/karoryfer.swagbass",
        commit: "9d10fcae71af1975988ddecd5af1c95d372c7355",
        programs: &["swagbass_clean.sfz"],
        approx_mb: 146,
        cc: &[],
    },
    SfzPack {
        id: "emilyguitar",
        name: "Emilyguitar",
        kind: "エレキギター",
        author: "Karoryfer Samples",
        license: "CC0-1.0",
        repo: "sfzinstruments/karoryfer.emilyguitar",
        commit: "b4920dc662fd9cad6dcaccdeecffdd91c8725d8c",
        programs: &["emily_clean.sfz"],
        approx_mb: 124,
        cc: &[],
    },
    SfzPack {
        id: "cello",
        name: "Karoryfer x Bigcat Cello",
        kind: "チェロ",
        author: "Karoryfer Samples・Bigcat Instruments(演奏 Kamila Borowiak)",
        license: "CC0-1.0",
        repo: "sfzinstruments/karoryfer-bigcat.cello",
        commit: "6fd75fbfc1dbb3109bf26220ba1adea46188a18b",
        programs: &["Programs/01- Bowed (velocity layer).sfz"],
        approx_mb: 95,
        cc: &[],
    },
    SfzPack {
        id: "solo-sax",
        name: "MTG Solo Saxophones(アルト・テナー)",
        kind: "サックス",
        author: "MTG(Music Technology Group, UPF)、SFZ 化 kinwie",
        license: "CC-BY-4.0",
        repo: "sfzinstruments/MTG.SoloSax",
        commit: "b494d256549b3d088fdec176ce82867f8a1f58b2",
        programs: &[
            "MTG Solo Saxophones/MTG Alto Sax.sfz",
            "MTG Solo Saxophones/MTG Tenor Sax.sfz",
        ],
        approx_mb: 58,
        cc: &[],
    },
    SfzPack {
        id: "ixox-flute",
        name: "Ixox Flute",
        kind: "フルート",
        author: "Xavier Hosxe、SFZ 化 kinwie",
        license: "CC-BY-4.0",
        repo: "sfzinstruments/Ixox.Flute",
        commit: "0cc54468bb0d2d9b32921958585caad65ba8df21",
        programs: &["Ixox Flute.sfz"],
        approx_mb: 9,
        cc: &[],
    },
];

/// 音源の状態(一覧の 1 行)
#[derive(Clone, Debug, serde::Serialize)]
pub struct PackStatus {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub author: String,
    pub license: String,
    pub approx_mb: u32,
    /// 入っているか(すべての .sfz がある)
    pub installed: bool,
    /// 入れたときの楽器の名前(set_soundfont_instrument の sfz に渡すもの)
    pub instruments: Vec<String>,
    /// 選んだときに付ける調整つまみ(CC 番号 → 値)
    pub cc: std::collections::BTreeMap<u8, u8>,
}

/// 楽器(SFZ ライブラリからの名前)を選んだときに付ける調整つまみ。カタログの音源でなければ空
pub fn default_cc(instrument: &str) -> std::collections::BTreeMap<u8, u8> {
    PACKS
        .iter()
        .find(|p| {
            instrument
                .strip_prefix(p.id)
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .map(|p| p.cc.iter().copied().collect())
        .unwrap_or_default()
}

pub fn status(lib: &Path) -> Vec<PackStatus> {
    PACKS
        .iter()
        .map(|p| {
            let instruments: Vec<String> =
                p.programs.iter().map(|s| format!("{}/{s}", p.id)).collect();
            PackStatus {
                id: p.id.to_owned(),
                name: p.name.to_owned(),
                kind: p.kind.to_owned(),
                author: p.author.to_owned(),
                license: p.license.to_owned(),
                approx_mb: p.approx_mb,
                installed: instruments.iter().all(|i| lib.join(i).is_file()),
                instruments,
                cc: p.cc.iter().copied().collect(),
            }
        })
        .collect()
}

/// `a/b/../c` を `a/c` にする(区切りは `/`)。リポジトリの外へ出るなら None
pub fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for c in path.split(['/', '\\']) {
        if c.contains(':') {
            return None; // Windows のドライブ(C:)でライブラリの外へ出ない
        }
        match c {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            c => out.push(c),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(dir: &str, rel: &str) -> Option<String> {
    if dir.is_empty() {
        normalize(rel)
    } else {
        normalize(&format!("{dir}/{rel}"))
    }
}

/// URL のパスの 1 区切りを符号化する(空白・# などを %xx に)
fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn raw_url(pack: &SfzPack, path: &str) -> String {
    let enc: Vec<String> = path.split('/').map(encode_segment).collect();
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}",
        pack.repo,
        pack.commit,
        enc.join("/")
    )
}

/// 取得の計画: 書く文字のファイル(.sfz と include)と、取る波形(保存するパス → リポジトリのパス, 大きさ)
pub struct Plan {
    pub texts: Vec<(String, String)>,
    pub samples: Vec<(String, String, u64)>,
}

/// .sfz を読んで、取る波形を決める。`fetch_text` はリポジトリの中のパスの文字を返し、
/// `sizes` はリポジトリの全ファイルの大きさ(大文字小文字の違いの吸収にも使う)
pub fn plan(
    pack: &SfzPack,
    fetch_text: &mut dyn FnMut(&str) -> Result<String, String>,
    sizes: &HashMap<String, u64>,
) -> Result<Plan, String> {
    let lower: HashMap<String, &String> = sizes.keys().map(|k| (k.to_lowercase(), k)).collect();
    let mut texts: Vec<(String, String)> = Vec::new();
    let mut samples: Vec<(String, String, u64)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for prog in pack.programs {
        let dir = parent(prog).to_owned();
        let body = fetch_text(prog)?;
        let mut included: Vec<(String, String)> = Vec::new();
        let regions = glaux_engine::sfz::parse(&body, &mut |inc| {
            let path = join(&dir, inc).ok_or_else(|| format!("include の場所が不正です: {inc}"))?;
            let t = fetch_text(&path)?;
            included.push((path, t.clone()));
            Ok(t)
        })?;
        texts.push(((*prog).to_owned(), body));
        texts.extend(included);
        for rel in glaux_engine::sfz::used_samples(&regions) {
            let Some(save) = join(&dir, &rel) else {
                continue;
            };
            if !seen.insert(save.clone()) {
                continue;
            }
            // リポジトリのパス(大文字小文字が違っても拾う)
            let (repo_path, size) = match sizes.get(&save) {
                Some(s) => (save.clone(), *s),
                None => match lower.get(&save.to_lowercase()) {
                    Some(k) => ((*k).clone(), sizes[*k]),
                    None => continue, // 無い波形は鳴らない region になるだけ
                },
            };
            samples.push((save, repo_path, size));
        }
    }
    texts.sort();
    texts.dedup_by(|a, b| a.0 == b.0);
    if samples.is_empty() {
        return Err("鳴らせる波形が見つかりません".to_owned());
    }
    Ok(Plan { texts, samples })
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .build()
        .into()
}

fn fetch_string(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    let mut res = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let bytes = res
        .body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

/// リポジトリの全ファイルの大きさ(GitHub の tree API。1 回だけ呼ぶ)
fn repo_sizes(agent: &ureq::Agent, pack: &SfzPack) -> Result<HashMap<String, u64>, String> {
    let url = format!(
        "https://api.github.com/repos/{}/git/trees/{}?recursive=1",
        pack.repo, pack.commit
    );
    let body = agent
        .get(&url)
        .header("User-Agent", "Glaux")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("音源の一覧を取れません: {e}"))?
        .body_mut()
        .with_config()
        .limit(64 << 20)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let tree = v["tree"]
        .as_array()
        .ok_or("音源の一覧の形が想定と違います")?;
    Ok(tree
        .iter()
        .filter(|t| t["type"] == "blob")
        .filter_map(|t| Some((t["path"].as_str()?.to_owned(), t["size"].as_u64()?)))
        .collect())
}

/// GitHub から .sfz を読んで計画を立てる(波形はまだ取らない)
pub fn plan_online(agent: &ureq::Agent, pack: &SfzPack) -> Result<Plan, String> {
    let sizes = repo_sizes(agent, pack)?;
    plan(
        pack,
        &mut |path| fetch_string(agent, &raw_url(pack, path)),
        &sizes,
    )
}

/// 音源を SFZ ライブラリへ取得する。すでにある波形(大きさが同じもの)は取り直さない(途中からの再開)。
/// `progress(受信済み, 全体)` はバイト。戻り値は入れた楽器の名前
pub fn download(
    id: &str,
    lib: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Vec<String>, String> {
    let pack = PACKS
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("そのような音源はありません: {id}"))?;
    download_with(&agent(), pack, lib, progress)
}

/// [`download`] の、通信の設定と音源を渡す版
pub fn download_with(
    agent: &ureq::Agent,
    pack: &SfzPack,
    lib: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Vec<String>, String> {
    let plan = plan_online(agent, pack)?;
    let root = lib.join(pack.id);
    let total: u64 = plan.samples.iter().map(|s| s.2).sum();
    let mut got = 0u64;
    progress(0, total);
    for (save, repo_path, size) in &plan.samples {
        let dest = root.join(save);
        if std::fs::metadata(&dest).is_ok_and(|m| m.len() == *size) {
            got += size;
            progress(got, total);
            continue;
        }
        fetch_file(agent, &raw_url(pack, repo_path), &dest, *size, &mut |n| {
            progress(got + n, total)
        })?;
        got += size;
    }
    // 作者とライセンス、そして .sfz を最後に書く(途中で止まった音源が一覧に出ないように)
    let credits = format!(
        "{}\n作者: {}\nライセンス: {}\n取得元: https://github.com/{}/tree/{}\n",
        pack.name, pack.author, pack.license, pack.repo, pack.commit
    );
    write_file(&root.join("CREDITS.txt"), credits.as_bytes())?;
    for (path, text) in &plan.texts {
        write_file(&root.join(path), text.as_bytes())?;
    }
    progress(total, total);
    Ok(pack
        .programs
        .iter()
        .map(|p| format!("{}/{p}", pack.id))
        .collect())
}

fn write_file(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)
            .map_err(|e| format!("フォルダを作れません({}): {e}", d.display()))?;
    }
    std::fs::write(dest, bytes).map_err(|e| format!("書き込めません({}): {e}", dest.display()))
}

fn fetch_file(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    size: u64,
    progress: &mut dyn FnMut(u64),
) -> Result<(), String> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)
            .map_err(|e| format!("フォルダを作れません({}): {e}", d.display()))?;
    }
    let part = PathBuf::from(format!("{}.part", dest.display()));
    let mut res = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut reader = res
        .body_mut()
        .with_config()
        .limit(size.saturating_mul(2).max(1 << 20))
        .reader();
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("書き込めません({}): {e}", part.display()))?;
    let mut buf = vec![0u8; 1 << 16];
    let (mut got, mut last) = (0u64, 0u64);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("取得が途中で止まりました: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("書き込めません: {e}"))?;
        got += n as u64;
        if got - last >= 1 << 20 {
            last = got;
            progress(got);
        }
    }
    drop(file);
    if got != size {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "取得したファイルの大きさが違います({url}: {got} / {size} バイト)"
        ));
    }
    std::fs::rename(&part, dest).map_err(|e| format!("置き換えられません: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_normalize_and_stay_inside() {
        assert_eq!(normalize("a/b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize("a\\b/./c.wav").as_deref(), Some("a/b/c.wav"));
        assert_eq!(normalize("../x"), None);
        assert_eq!(normalize("C:/x"), None);
        assert_eq!(
            join("Programs", "../Samples/k.flac").as_deref(),
            Some("Samples/k.flac")
        );
        assert_eq!(join("", "k.flac").as_deref(), Some("k.flac"));
        assert_eq!(encode_segment("Pianet T#1.wav"), "Pianet%20T%231.wav");
    }

    #[test]
    fn plan_reads_includes_and_takes_only_played_samples() {
        let pack = SfzPack {
            id: "t",
            name: "t",
            kind: "t",
            author: "a",
            license: "CC0-1.0",
            repo: "o/r",
            commit: "c",
            programs: &["Programs/kit.sfz"],
            approx_mb: 1,
            cc: &[],
        };
        let files: HashMap<&str, &str> = [
            (
                "Programs/kit.sfz",
                "<control> default_path=../Samples/\n#include \"map.txt\"\n\
                 <region> sample=rel.wav key=36 trigger=release",
            ),
            (
                "Programs/map.txt",
                "<region> sample=Kick.wav key=36\n<region> sample=snare.wav key=38\n\
                 <region> sample=missing.wav key=40",
            ),
        ]
        .into_iter()
        .collect();
        let sizes: HashMap<String, u64> = [
            ("Samples/kick.wav".to_owned(), 100),
            ("Samples/snare.wav".to_owned(), 200),
            ("Samples/rel.wav".to_owned(), 300),
        ]
        .into_iter()
        .collect();
        let p = plan(
            &pack,
            &mut |path| {
                files
                    .get(path)
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("無い: {path}"))
            },
            &sizes,
        )
        .unwrap();
        assert_eq!(
            p.texts.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(),
            ["Programs/kit.sfz", "Programs/map.txt"]
        );
        // 大文字小文字の違いを吸収し、保存は .sfz に書かれた名前で。リリース音と無い波形は取らない
        assert_eq!(
            p.samples,
            [
                (
                    "Samples/Kick.wav".to_owned(),
                    "Samples/kick.wav".to_owned(),
                    100
                ),
                (
                    "Samples/snare.wav".to_owned(),
                    "Samples/snare.wav".to_owned(),
                    200
                ),
            ]
        );
    }

    #[test]
    fn catalog_is_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for p in PACKS {
            assert!(ids.insert(p.id), "id が重複: {}", p.id);
            assert_eq!(
                p.commit.len(),
                40,
                "コミットは完全な SHA で固定する: {}",
                p.id
            );
            assert!(
                p.license == "CC0-1.0" || p.license.starts_with("CC-BY-"),
                "{}",
                p.license
            );
            for prog in p.programs {
                assert!(glaux_engine::sfz::valid_name(&format!("{}/{prog}", p.id)));
            }
        }
        assert_eq!(
            default_cc("big-rusty-drums/Programs/02-basic.sfz").get(&89),
            Some(&80)
        );
        assert!(default_cc("big-rusty-drums-x/a.sfz").is_empty());
        assert!(default_cc("Mine/kit.sfz").is_empty());
        let st = status(Path::new("/nonexistent"));
        assert_eq!(st.len(), PACKS.len());
        assert!(st.iter().all(|s| !s.installed));
    }
}
