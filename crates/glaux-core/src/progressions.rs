//! 定番のコード進行の表(MCP の suggest_progression の中身)。ジャンルと雰囲気で候補を選び、キーに当てて返す。
//!
//! 進行はローマ数字で持つ(長調の進行は長調のキー、短調の進行は短調のキーに当てる。キーの長短が違えば平行調に読み替える)。
//! 並びは「よく使う順」。AI が暗算で進行を作るより、定番から選んで区間ごとに変える方が外さない。

use crate::chord::{self, Key};

/// 進行 1 つ
pub struct Progression {
    pub name: &'static str,
    /// `|` で小節、空白で小節内(write_chords と同じ書き方)
    pub roman: &'static str,
    /// 短調の進行か
    pub minor: bool,
    pub genres: &'static [&'static str],
    pub moods: &'static [&'static str],
    pub note: &'static str,
}

pub const PROGRESSIONS: &[Progression] = &[
    Progression {
        name: "王道進行",
        roman: "IVmaj7 | V7 | iii7 | vi",
        minor: false,
        genres: &["jpop", "pop", "anime", "edm"],
        moods: &["emotional", "bright", "sad"],
        note: "J-POP のサビの定番。切なさと高揚",
    },
    Progression {
        name: "丸サ進行",
        roman: "IVmaj7 | III7 | vi7 | v7 I7",
        minor: false,
        genres: &["citypop", "jpop", "neosoul", "lofi"],
        moods: &["chill", "urban", "emotional"],
        note: "Just the Two of Us 進行。都会的でおしゃれ",
    },
    Progression {
        name: "I–V–vi–IV",
        roman: "I | V | vi | IV",
        minor: false,
        genres: &["pop", "rock", "edm", "country"],
        moods: &["bright", "uplifting", "epic"],
        note: "洋楽ポップで最も多い 4 和音",
    },
    Progression {
        name: "vi–IV–I–V",
        roman: "vi | IV | I | V",
        minor: false,
        genres: &["pop", "edm", "rock", "trance"],
        moods: &["emotional", "epic", "sad"],
        note: "I–V–vi–IV を短調側から。感傷的に盛り上がる",
    },
    Progression {
        name: "カノン進行",
        roman: "I V | vi iii | IV I | IV V",
        minor: false,
        genres: &["pop", "jpop", "ballad", "classical"],
        moods: &["bright", "emotional", "nostalgic"],
        note: "パッヘルベルのカノン。ベースが順に下がる",
    },
    Progression {
        name: "小室進行",
        roman: "vi | IV | V | I",
        minor: false,
        genres: &["jpop", "edm", "anime", "trance"],
        moods: &["dramatic", "emotional", "epic"],
        note: "90 年代の J-POP・ダンス。短調の響きで駆け上がる",
    },
    Progression {
        name: "I–vi–IV–V",
        roman: "I | vi | IV | V",
        minor: false,
        genres: &["pop", "retro", "doowop", "ballad"],
        moods: &["nostalgic", "bright", "sweet"],
        note: "50 年代進行。懐かしく甘い",
    },
    Progression {
        name: "ii–V–I",
        roman: "ii7 | V7 | Imaj7 | %",
        minor: false,
        genres: &["jazz", "lofi", "bossa", "citypop"],
        moods: &["chill", "sophisticated", "resolved"],
        note: "ジャズの基本の終止",
    },
    Progression {
        name: "I–vi–ii–V(循環)",
        roman: "Imaj7 vi7 | ii7 V7",
        minor: false,
        genres: &["jazz", "lofi", "citypop", "funk"],
        moods: &["chill", "sophisticated", "groovy"],
        note: "繰り返しに向く循環コード",
    },
    Progression {
        name: "下降のメジャー 7th",
        roman: "IVmaj7 | iii7 | ii7 | Imaj7",
        minor: false,
        genres: &["lofi", "neosoul", "citypop", "chill"],
        moods: &["chill", "dreamy", "melancholic"],
        note: "ローファイの定番。ゆっくり下る 7th",
    },
    Progression {
        name: "ローファイの ii–V",
        roman: "ii9 | V13 | Imaj9 | vi9",
        minor: false,
        genres: &["lofi", "jazz", "neosoul"],
        moods: &["chill", "dreamy", "sophisticated"],
        note: "テンション入りの ii–V–I–vi",
    },
    Progression {
        name: "フューチャーベースの進行",
        roman: "IVmaj9 | V7sus4 V7 | vi9 | iii7",
        minor: false,
        genres: &["futurebass", "edm", "pop"],
        moods: &["emotional", "bright", "dreamy"],
        note: "広く積んだ maj9・sus のスタブ(spread で)",
    },
    Progression {
        name: "ミクソリディアンのロック",
        roman: "I | bVII | IV | I",
        minor: false,
        genres: &["rock", "pop", "funk"],
        moods: &["bright", "anthemic", "groovy"],
        note: "♭VII の明るい骨太さ",
    },
    Progression {
        name: "ファンクのヴァンプ",
        roman: "I9 | IV9",
        minor: false,
        genres: &["funk", "disco", "soul"],
        moods: &["groovy", "bright"],
        note: "属 9 の 2 和音を繰り返す(ギターのカッティング向き)",
    },
    Progression {
        name: "12 小節のブルース",
        roman: "I7 | IV7 | I7 | I7 | IV7 | IV7 | I7 | I7 | V7 | IV7 | I7 | V7",
        minor: false,
        genres: &["blues", "rock", "jazz", "funk"],
        moods: &["groovy", "gritty"],
        note: "ブルースの基本形",
    },
    Progression {
        name: "短調の EDM 進行",
        roman: "i | VI | III | VII",
        minor: true,
        genres: &["edm", "trance", "house", "pop", "futurebass"],
        moods: &["emotional", "epic", "dark"],
        note: "Am–F–C–G。トランス・プログレッシブハウスの王道",
    },
    Progression {
        name: "短調の下降",
        roman: "i | VII | VI | VII",
        minor: true,
        genres: &["edm", "rock", "cinematic", "trap"],
        moods: &["epic", "dark", "dramatic"],
        note: "壮大・叙事的",
    },
    Progression {
        name: "アンダルシア終止",
        roman: "i | VII | VI | V7",
        minor: true,
        genres: &["latin", "flamenco", "cinematic", "rock"],
        moods: &["dramatic", "tense", "dark"],
        note: "Am–G–F–E。フラメンコ・情熱",
    },
    Progression {
        name: "トラップの短調",
        roman: "i | VI | VII | i",
        minor: true,
        genres: &["trap", "hiphop", "drill"],
        moods: &["dark", "tense", "moody"],
        note: "暗く張り詰めた 4 和音",
    },
    Progression {
        name: "ディープハウスの短調",
        roman: "i9 | VImaj7 | IIImaj7 | VII",
        minor: true,
        genres: &["house", "deephouse", "lofi"],
        moods: &["chill", "moody", "dreamy"],
        note: "m9 と maj7 のスタブ(裏拍で短く)",
    },
    Progression {
        name: "ドリアンのヴァンプ",
        roman: "i7 | IV9",
        minor: true,
        genres: &["funk", "house", "soul", "jazz"],
        moods: &["groovy", "cool"],
        note: "短調で IV が長三和音(ドリアン)。ファンク・ハウスの 2 和音",
    },
    Progression {
        name: "フリジアン",
        roman: "i | bII | i | bII",
        minor: true,
        genres: &["metal", "cinematic", "trap"],
        moods: &["dark", "tense", "exotic"],
        note: "半音上の ♭II で緊張",
    },
    Progression {
        name: "短調のバラード",
        roman: "i | iv | VII | III",
        minor: true,
        genres: &["ballad", "pop", "jpop", "rnb"],
        moods: &["sad", "emotional", "melancholic"],
        note: "Am–Dm–G–C。哀愁から平行長調へ",
    },
];

/// 候補 1 つ(キーに当てたもの)
#[derive(Clone, Debug, serde::Serialize)]
pub struct Suggestion {
    pub name: &'static str,
    pub roman: &'static str,
    /// キーに当てたコード(write_chords にそのまま渡せる)
    pub chords: String,
    /// 当てたキー("C major" など。長短が違えば平行調)
    pub key: String,
    pub bars: usize,
    pub genres: &'static [&'static str],
    pub moods: &'static [&'static str],
    pub note: &'static str,
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// ジャンルと雰囲気で選んだ候補(よく合う順、最大 `count`)。`key` の既定は C major / A minor
pub fn suggest(
    genre: Option<&str>,
    mood: Option<&str>,
    key: Option<Key>,
    count: usize,
) -> Result<Vec<Suggestion>, String> {
    let g = genre.map(|s| s.trim().to_lowercase().replace(['-', ' ', '_'], ""));
    let m = mood.map(|s| s.trim().to_lowercase());
    let mut scored: Vec<(i32, usize)> = PROGRESSIONS
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut score = 0;
            if let Some(g) = &g {
                if p.genres.iter().any(|x| x == g) {
                    score += 3;
                }
            }
            if let Some(m) = &m {
                if p.moods.iter().any(|x| x == m) {
                    score += 2;
                }
            }
            (score, i)
        })
        .collect();
    if g.is_some() || m.is_some() {
        scored.retain(|(s, _)| *s > 0);
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out = Vec::new();
    for (_, i) in scored.into_iter().take(count.clamp(1, PROGRESSIONS.len())) {
        let p = &PROGRESSIONS[i];
        // 進行の長短に合わせたキー(違えば平行調)
        let k = match key {
            Some(k) if k.minor == p.minor => k,
            Some(k) if k.minor => Key {
                tonic: (k.tonic + 3) % 12,
                minor: false,
            },
            Some(k) => Key {
                tonic: (k.tonic + 9) % 12,
                minor: true,
            },
            None if p.minor => Key {
                tonic: 9,
                minor: true,
            },
            None => Key {
                tonic: 0,
                minor: false,
            },
        };
        let bars = chord::split_progression(p.roman)?;
        let chords = bars
            .iter()
            .map(|bar| {
                bar.iter()
                    .map(|sym| {
                        chord::parse_in_key(sym, Some(k))
                            .map(|c| c.map_or_else(|| "N.C.".to_owned(), |c| c.name))
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|v| v.join(" "))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" | ");
        out.push(Suggestion {
            name: p.name,
            roman: p.roman,
            chords,
            key: format!(
                "{} {}",
                NAMES[k.tonic as usize],
                if k.minor { "minor" } else { "major" }
            ),
            bars: bars.len(),
            genres: p.genres,
            moods: p.moods,
            note: p.note,
        });
    }
    Ok(out)
}

/// 表にあるジャンルと雰囲気の一覧(候補が無いときに知らせる)
pub fn vocabulary() -> (Vec<&'static str>, Vec<&'static str>) {
    let mut g: Vec<&str> = PROGRESSIONS
        .iter()
        .flat_map(|p| p.genres.iter().copied())
        .collect();
    let mut m: Vec<&str> = PROGRESSIONS
        .iter()
        .flat_map(|p| p.moods.iter().copied())
        .collect();
    g.sort_unstable();
    g.dedup();
    m.sort_unstable();
    m.dedup();
    (g, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_progression_reads_in_both_modes() {
        for p in PROGRESSIONS {
            for key in ["C major", "A minor", "F# major", "Eb minor"] {
                let s = suggest(None, None, Key::parse(key), 50).unwrap();
                assert!(s.iter().any(|x| x.name == p.name), "{} {key}", p.name);
            }
        }
    }

    #[test]
    fn genre_and_mood_pick_and_keys_apply() {
        let s = suggest(Some("jpop"), Some("emotional"), Key::parse("C major"), 3).unwrap();
        assert_eq!(s[0].name, "王道進行");
        assert_eq!(s[0].chords, "Fmaj7 | G7 | Em7 | Am");
        // 短調の進行を短調のキーで
        let s = suggest(Some("trance"), None, Key::parse("F minor"), 5).unwrap();
        let edm = s.iter().find(|x| x.name == "短調の EDM 進行").unwrap();
        assert_eq!(edm.chords, "Fm | Db | Ab | Eb");
        assert_eq!(edm.key, "F minor");
        // 長調の進行を短調のキーで頼むと平行調(A minor → C major)
        let s = suggest(Some("jpop"), None, Key::parse("A minor"), 10).unwrap();
        let ou = s.iter().find(|x| x.name == "王道進行").unwrap();
        assert_eq!(ou.key, "C major");
        // 丸サ進行(D major)
        let s = suggest(Some("citypop"), None, Key::parse("D major"), 5).unwrap();
        let m = s.iter().find(|x| x.name == "丸サ進行").unwrap();
        assert_eq!(m.chords, "Gmaj7 | F#7 | Bm7 | Am7 D7");
        // 当てはまらなければ空
        assert!(suggest(Some("polka"), None, None, 5).unwrap().is_empty());
        let (g, m) = vocabulary();
        assert!(g.contains(&"lofi") && m.contains(&"chill"));
    }
}
