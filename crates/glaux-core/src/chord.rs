//! コード記号とローマ数字を読む(MCP の write_chords などの土台)。
//!
//! AI が和音を MIDI 番号の足し算で書くと、テンション・転回・分数コードで間違えやすい。ここで記号を読み、
//! 構成音を「役割」(根音・3 度・5 度・7 度・6 度・テンション)付きで返す。積み方(ボイシング)は `voicing` が決める。
//!
//! 読めるもの: 根音(C, F#, Bb, E♭)、m / min / -、dim / °、aug / +、sus2 / sus4 / sus、5(パワーコード)、
//! 6・m6・69、7・maj7(M7・Δ7・△7・ma7)・m7・mMaj7・m7b5(ø)・dim7・7sus4、9・11・13(maj9・m11 など)、
//! add9 / add2 / add11、オルタード(b5・#5・b9・#9・#11・b13・alt)、分数コード(C/E)、N.C.(休み)。
//! ローマ数字(キーを渡したとき): I〜VII(大文字 = 長三和音、小文字 = 短三和音)、前の b / #、後ろは上と同じ記号
//! (V7・ii7・viiø7・bVII・IVmaj7)、借用の V/V・V7/ii。

use serde::Serialize;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];
const SHARP_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

/// ピッチクラスの綴り(♭ 系か ♯ 系)
fn spell(pc: u8, flats: bool) -> &'static str {
    if flats {
        FLAT_NAMES[pc as usize % 12]
    } else {
        SHARP_NAMES[pc as usize % 12]
    }
}

/// 構成音の役割(積み方で、省いてよいか・重ねてよいかを決める)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Root,
    Third,
    /// sus の 2 度・4 度(3 度の代わり)
    Sus,
    Fifth,
    Sixth,
    Seventh,
    /// 9・11・13 とその変化
    Tension,
}

/// 構成音 1 つ
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Tone {
    /// 根音からの半音(0〜11)
    pub interval: u8,
    pub role: Role,
    /// 省いてよい(完全 5 度・11/13 の和音の 9 度など)
    pub optional: bool,
}

/// 読んだコード
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Chord {
    /// 表示用の名前(読んだ記号をそろえたもの。例 "Bbmaj7" / "G7/B")
    pub name: String,
    /// 根音のピッチクラス(C = 0)
    pub root: u8,
    /// 分数コードの最低音(無ければ根音)
    pub bass: Option<u8>,
    pub tones: Vec<Tone>,
}

impl Chord {
    /// 構成音のピッチクラス(根音から、役割の順)
    pub fn pitch_classes(&self) -> Vec<u8> {
        self.tones
            .iter()
            .map(|t| (self.root + t.interval) % 12)
            .collect()
    }

    /// 最低音に置く音(分数コードならその音、無ければ根音)
    pub fn bass_pc(&self) -> u8 {
        self.bass.unwrap_or(self.root)
    }
}

/// 音名の先頭(A〜G と # / b / ♯ / ♭)を読む。(ピッチクラス, 読んだバイト数)
pub fn parse_pitch_class(s: &str) -> Option<(u8, usize)> {
    let mut chars = s.char_indices();
    let (_, c) = chars.next()?;
    let base: i32 = match c.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let mut acc = 0i32;
    let mut used = c.len_utf8();
    for (i, ch) in chars {
        let d = match ch {
            '#' | '♯' => 1,
            'b' | '♭' => -1,
            _ => break,
        };
        acc += d;
        used = i + ch.len_utf8();
    }
    Some(((base + acc).rem_euclid(12) as u8, used))
}

/// 音名とオクターブ("C4" = 60、"Bb2" = 46、"F#-1" = 6)を MIDI 番号に
pub fn parse_note(s: &str) -> Option<u8> {
    let s = s.trim();
    let (pc, used) = parse_pitch_class(s)?;
    let oct: i32 = s[used..].trim().parse().ok()?;
    // 臨時記号で C の下へ出る Cb4 などは、ピッチクラスではなく音の高さで数える
    let letter = s.chars().next()?.to_ascii_uppercase();
    let natural = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        _ => 11,
    };
    let acc = {
        let d = pc as i32 - natural;
        if d > 6 {
            d - 12
        } else if d < -6 {
            d + 12
        } else {
            d
        }
    };
    let p = (oct + 1) * 12 + natural + acc;
    (0..=127).contains(&p).then_some(p as u8)
}

/// MIDI 番号の音名("C4" / "Bb2")
pub fn note_name(p: u8) -> String {
    format!("{}{}", NOTE_NAMES[(p % 12) as usize], p as i32 / 12 - 1)
}

/// キー(トニックと長調・短調)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub tonic: u8,
    pub minor: bool,
}

impl Key {
    /// "C major" / "A minor" / "Bb" / "F#m" / "Am"
    pub fn parse(s: &str) -> Option<Key> {
        let s = s.trim();
        let (tonic, used) = parse_pitch_class(s)?;
        let rest = s[used..].trim().to_ascii_lowercase();
        let minor = match rest.as_str() {
            "" | "major" | "maj" | "dur" => false,
            "m" | "minor" | "min" | "moll" => true,
            _ => return None,
        };
        Some(Key { tonic, minor })
    }

    /// ♭ 系のキーか(F・Bb・Eb・Ab・Db・Gb の長調、D・G・C・F・Bb・Eb の短調)
    pub fn uses_flats(&self) -> bool {
        if self.minor {
            matches!(self.tonic, 2 | 7 | 0 | 5 | 10 | 3)
        } else {
            matches!(self.tonic, 5 | 10 | 3 | 8 | 1 | 6)
        }
    }

    /// 音階の度数(1〜7)の音の、トニックからの半音(短調は自然短音階)
    fn degree(&self, d: u8) -> u8 {
        let steps: [u8; 7] = if self.minor {
            [0, 2, 3, 5, 7, 8, 10]
        } else {
            [0, 2, 4, 5, 7, 9, 11]
        };
        steps[(d as usize - 1) % 7]
    }
}

/// 3 和音の種類
#[derive(Clone, Copy, PartialEq)]
enum Triad {
    Major,
    Minor,
    Dim,
    Aug,
    Sus2,
    Sus4,
    Power,
}

/// コード記号("Am7" / "Bbmaj9" / "G7(b9)" / "C/E" / "F#m7b5")を読む。"N.C." は Ok(None)
pub fn parse(symbol: &str) -> Result<Option<Chord>, String> {
    let s = symbol.trim();
    if s.is_empty() {
        return Err("コードが空です".to_owned());
    }
    if matches!(
        s.to_ascii_uppercase().as_str(),
        "N.C." | "NC" | "N.C" | "-" | "REST"
    ) {
        return Ok(None);
    }
    let (root, used) = parse_pitch_class(s).ok_or_else(|| format!("根音が読めません: {symbol}"))?;
    let rest = &s[used..];
    let (quality, bass) = split_bass(rest).map_err(|e| format!("{e}: {symbol}"))?;
    let tones = parse_quality(quality).map_err(|e| format!("{e}: {symbol}"))?;
    // 書かれた綴りのまま(Db は Db、C# は C#)。音名の 2 文字目以降の b / ♭ を見る
    let has_flat = |t: &str| t.chars().skip(1).any(|c| c == 'b' || c == '♭');
    let bass_text = rest.rsplit_once('/').map_or("", |(_, b)| b);
    let flats = has_flat(&s[..used]) || has_flat(bass_text);
    Ok(Some(Chord {
        name: display_name(root, quality, bass, flats),
        root,
        bass,
        tones,
    }))
}

/// ローマ数字("V7" / "ii7" / "bVII" / "V/V" / "IVmaj7/V")をキーで読む。コード記号も読める(ローマ数字でなければ記号として)
pub fn parse_in_key(symbol: &str, key: Option<Key>) -> Result<Option<Chord>, String> {
    let s = symbol.trim();
    match (key, roman_head(s)) {
        (Some(k), Some(_)) => parse_roman(s, k),
        _ => parse(s),
    }
}

/// ローマ数字で始まるか(先頭の b / # のあとに I / V)。"Bb" のような音名とは区別する
fn roman_head(s: &str) -> Option<()> {
    let t = s.trim_start_matches(['b', '#', '♭', '♯']);
    let first = t.chars().next()?;
    matches!(first, 'I' | 'V' | 'i' | 'v').then_some(())
}

fn parse_roman(s: &str, key: Key) -> Result<Option<Chord>, String> {
    // 借用の V/V: 右側を先に読み、その根音を長調のトニックとして左側を読む
    if let Some((left, right)) = s.split_once('/') {
        if roman_head(right).is_some() {
            let target = parse_roman(right, key)?.ok_or("借用の先が休みです")?;
            let local = Key {
                tonic: target.root,
                minor: false,
            };
            return parse_roman(left, local);
        }
    }
    let mut acc: i32 = 0;
    let mut t = s;
    while let Some(c) = t.chars().next() {
        match c {
            'b' | '♭' => acc -= 1,
            '#' | '♯' => acc += 1,
            _ => break,
        }
        t = &t[c.len_utf8()..];
    }
    let numerals = [
        ("VII", 7),
        ("vii", 7),
        ("III", 3),
        ("iii", 3),
        ("IV", 4),
        ("iv", 4),
        ("VI", 6),
        ("vi", 6),
        ("II", 2),
        ("ii", 2),
        ("V", 5),
        ("v", 5),
        ("I", 1),
        ("i", 1),
    ];
    let (num, degree) = numerals
        .iter()
        .find(|(n, _)| t.starts_with(n))
        .ok_or_else(|| format!("ローマ数字が読めません: {s}"))?;
    let lower = num.chars().all(|c| c.is_ascii_lowercase());
    let rest = &t[num.len()..];
    let root = ((key.tonic as i32 + key.degree(*degree) as i32 + acc).rem_euclid(12)) as u8;
    let (quality, bass) = split_bass(rest).map_err(|e| format!("{e}: {s}"))?;
    // 小文字は短三和音。記号が dim / ø / ° / m7b5 のときはそれに従う
    let q = quality.trim();
    let explicit =
        q.starts_with("dim") || q.starts_with('°') || q.starts_with('ø') || q.starts_with("aug");
    let q = if lower && !explicit && !q.starts_with('m') {
        format!("m{q}")
    } else {
        q.to_owned()
    };
    let tones = parse_quality(&q).map_err(|e| format!("{e}: {s}"))?;
    Ok(Some(Chord {
        name: display_name(root, &q, bass, key.uses_flats() || acc < 0),
        root,
        bass,
        tones,
    }))
}

/// 分数コードの "/E" を切り出す("6/9" は分数ではない)
fn split_bass(rest: &str) -> Result<(&str, Option<u8>), String> {
    if let Some(i) = rest.rfind('/') {
        let after = &rest[i + 1..];
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            return Ok((rest, None));
        }
        let (pc, used) = parse_pitch_class(after).ok_or("分数コードの最低音が読めません")?;
        if used != after.len() {
            return Err("分数コードの最低音が読めません".to_owned());
        }
        return Ok((&rest[..i], Some(pc)));
    }
    Ok((rest, None))
}

fn display_name(root: u8, quality: &str, bass: Option<u8>, flats: bool) -> String {
    let q = normalize(quality);
    match bass {
        Some(b) => format!("{}{q}/{}", spell(root, flats), spell(b, flats)),
        None => format!("{}{q}", spell(root, flats)),
    }
}

/// 品質の記号をそろえる(Δ → maj、ø → m7b5、- → m など)
fn normalize(q: &str) -> String {
    let mut s: String = q
        .trim()
        .replace(['(', ')', ',', ' '], "")
        .replace("△", "maj")
        .replace("Δ", "maj")
        .replace("♭", "b")
        .replace("♯", "#")
        .replace("ø", "m7b5")
        .replace("°", "dim")
        .replace("Maj", "maj")
        .replace("MA", "maj")
        .replace("min", "m")
        .replace("mM", "mmaj")
        .replace('M', "maj");
    // ma7 / ma9 …(maj の略)。madd9(マイナーの add9)は残す
    for n in ["13", "11", "9", "7"] {
        s = s.replace(&format!("ma{n}"), &format!("maj{n}"));
    }
    if s.starts_with('-') {
        s.replace_range(0..1, "m");
    }
    if s.starts_with('+') {
        s.replace_range(0..1, "aug");
    }
    if s.starts_with('o') && !s.starts_with("om") {
        s.replace_range(0..1, "dim");
    }
    // ø7 は m7b5 と同じ
    s.replace("m7b57", "m7b5")
}

/// 根音の後ろ(品質・7th・テンション・変化)を構成音にする
fn parse_quality(q: &str) -> Result<Vec<Tone>, String> {
    let s = normalize(q);
    if s.contains("m7b5") && !s.starts_with("m7b5") {
        return Err("m7b5 の位置が読めません".to_owned());
    }
    let mut r = s.as_str();
    // 3 和音
    let mut triad = Triad::Major;
    let eat = |r: &mut &str, p: &str| -> bool {
        if r.starts_with(p) {
            *r = &r[p.len()..];
            true
        } else {
            false
        }
    };
    let mut half_dim = false;
    if eat(&mut r, "m7b5") {
        triad = Triad::Dim;
        half_dim = true;
    } else if eat(&mut r, "dim") {
        triad = Triad::Dim;
    } else if eat(&mut r, "aug") {
        triad = Triad::Aug;
    } else if r.starts_with('m') && !r.starts_with("maj") {
        r = &r[1..];
        triad = Triad::Minor;
    } else if r == "5" {
        return Ok(vec![
            tone(0, Role::Root, false),
            tone(7, Role::Fifth, false),
        ]);
    }
    // 7th・6th・拡張
    let mut seventh: Option<u8> = if half_dim { Some(10) } else { None };
    let mut sixth = false;
    let mut ext: u8 = 0; // 9 / 11 / 13
    let mut adds: Vec<u8> = Vec::new();
    let mut alts: Vec<(u8, Role)> = Vec::new();
    let mut no_fifth = false;
    let mut sharp5 = false;
    let mut flat5 = false;
    let major7 = eat(&mut r, "maj");
    if major7 && r.is_empty() {
        // "Cmaj" = 長三和音
    }
    for (p, n) in [("13", 13u8), ("11", 11), ("9", 9), ("7", 7)] {
        if eat(&mut r, p) {
            ext = n;
            break;
        }
    }
    if ext == 0 && !major7 {
        if eat(&mut r, "69") || eat(&mut r, "6/9") {
            sixth = true;
            adds.push(14);
        } else if eat(&mut r, "6") {
            sixth = true;
        }
    }
    if ext == 0 && major7 && !r.starts_with(|c: char| c.is_ascii_digit()) && !r.is_empty() {
        // "maj" の後に add などが続く形は下で読む
    }
    if ext > 0 {
        seventh = Some(if major7 {
            11
        } else if triad == Triad::Dim && !half_dim {
            9 // dim7(減 7 度)
        } else {
            10
        });
    } else if major7 {
        seventh = Some(11);
        if !r.is_empty() && !r.starts_with(['a', 's', 'b', '#']) {
            return Err("maj の後ろが読めません".to_owned());
        }
    }
    // 残り: sus・add・変化(順不同)
    while !r.is_empty() {
        if eat(&mut r, "sus2") {
            triad = Triad::Sus2;
        } else if eat(&mut r, "sus4") || eat(&mut r, "sus") {
            triad = Triad::Sus4;
        } else if eat(&mut r, "add9") || eat(&mut r, "add2") {
            adds.push(14);
        } else if eat(&mut r, "add11") || eat(&mut r, "add4") {
            adds.push(17);
        } else if eat(&mut r, "add13") || eat(&mut r, "add6") {
            adds.push(21);
        } else if eat(&mut r, "alt") {
            alts.extend([
                (13, Role::Tension),
                (15, Role::Tension),
                (18, Role::Tension),
                (20, Role::Tension),
            ]);
            no_fifth = true;
        } else if eat(&mut r, "b5") || eat(&mut r, "-5") {
            flat5 = true;
        } else if eat(&mut r, "#5") || eat(&mut r, "+5") {
            sharp5 = true;
        } else if eat(&mut r, "b9") {
            alts.push((13, Role::Tension));
        } else if eat(&mut r, "#9") {
            alts.push((15, Role::Tension));
        } else if eat(&mut r, "#11") {
            alts.push((18, Role::Tension));
        } else if eat(&mut r, "b13") {
            alts.push((20, Role::Tension));
        } else if eat(&mut r, "no3") {
            triad = Triad::Power;
        } else if eat(&mut r, "no5") {
            no_fifth = true;
        } else {
            return Err(format!("読めない記号「{r}」"));
        }
    }
    // 組み立て
    let mut out = vec![tone(0, Role::Root, false)];
    match triad {
        Triad::Major | Triad::Aug => out.push(tone(4, Role::Third, false)),
        Triad::Minor | Triad::Dim => out.push(tone(3, Role::Third, false)),
        Triad::Sus2 => out.push(tone(2, Role::Sus, false)),
        Triad::Sus4 => out.push(tone(5, Role::Sus, false)),
        Triad::Power => {}
    }
    let fifth = if triad == Triad::Dim || flat5 {
        6
    } else if triad == Triad::Aug || sharp5 {
        8
    } else {
        7
    };
    let altered_fifth = fifth != 7;
    if !no_fifth {
        // 完全 5 度は、ほかの音が多いときは省いてよい
        out.push(tone(fifth, Role::Fifth, !altered_fifth));
    }
    if sixth {
        out.push(tone(9, Role::Sixth, false));
    }
    if let Some(sv) = seventh {
        out.push(tone(sv, Role::Seventh, false));
    }
    // 拡張: 9 は 9、11 は 9 と 11、13 は 9 と 13(11 は長 3 度とぶつかるので省く)
    let minorish = matches!(triad, Triad::Minor | Triad::Dim);
    match ext {
        9 => out.push(tone(14 % 12, Role::Tension, false)),
        11 => {
            out.push(tone(14 % 12, Role::Tension, true));
            out.push(tone(17 % 12, Role::Tension, false));
            if !minorish {
                // 属 11 は 3 度を省く(sus4 に近い響き)
                out.retain(|t| t.role != Role::Third);
            }
        }
        13 => {
            out.push(tone(14 % 12, Role::Tension, true));
            if minorish {
                out.push(tone(17 % 12, Role::Tension, true));
            }
            out.push(tone(21 % 12, Role::Tension, false));
        }
        _ => {}
    }
    for a in adds {
        out.push(tone(a % 12, Role::Tension, false));
    }
    for (a, role) in alts {
        // 変化した音が、同じ度数の元の音を置き換える
        let iv = a % 12;
        out.retain(|t| {
            !(t.role == Role::Tension
                && ((iv == 1 || iv == 3) && t.interval == 2
                    || iv == 6 && t.interval == 5
                    || iv == 8 && t.interval == 9))
        });
        out.push(tone(iv, role, false));
    }
    // 同じ音を 2 回持たない(先の方を残す)
    let mut seen = [false; 12];
    out.retain(|t| !std::mem::replace(&mut seen[t.interval as usize], true));
    Ok(out)
}

fn tone(interval: u8, role: Role, optional: bool) -> Tone {
    Tone {
        interval,
        role,
        optional,
    }
}

/// コード進行の文字列を、小節ごとのコードの並びにする。`|` で小節を区切り、小節の中は空白で区切って均等に分ける。
/// `%` は前の小節をもう一度。"N.C." は休み。返り値は小節ごとの (記号) の並び
pub fn split_progression(s: &str) -> Result<Vec<Vec<String>>, String> {
    let mut bars: Vec<Vec<String>> = Vec::new();
    for raw in s.split('|') {
        let items: Vec<String> = raw.split_whitespace().map(str::to_owned).collect();
        if items.is_empty() {
            // 先頭・末尾の | は無視
            continue;
        }
        if items.len() == 1 && items[0] == "%" {
            let prev = bars.last().cloned().ok_or("最初の小節に % は使えません")?;
            bars.push(prev);
            continue;
        }
        if items.len() > 16 {
            return Err("1 小節のコードは 16 個まで".to_owned());
        }
        bars.push(items);
    }
    if bars.is_empty() {
        return Err("コード進行が空です".to_owned());
    }
    Ok(bars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcs(s: &str) -> Vec<u8> {
        let mut v = parse(s).unwrap().unwrap().pitch_classes();
        v.sort_unstable();
        v
    }

    fn sorted(mut v: Vec<u8>) -> Vec<u8> {
        v.sort_unstable();
        v
    }

    #[test]
    fn notes_and_names() {
        assert_eq!(parse_note("C4"), Some(60));
        assert_eq!(parse_note("A4"), Some(69));
        assert_eq!(parse_note("Bb2"), Some(46));
        assert_eq!(parse_note("F#3"), Some(54));
        assert_eq!(parse_note("Cb4"), Some(59));
        assert_eq!(parse_note("B#3"), Some(60));
        assert_eq!(parse_note("C-1"), Some(0));
        assert_eq!(parse_note("H4"), None);
        assert_eq!(note_name(61), "C#4");
        assert_eq!(note_name(46), "Bb2");
    }

    #[test]
    fn triads_sevenths_and_symbols() {
        assert_eq!(pcs("C"), vec![0, 4, 7]);
        assert_eq!(pcs("Am"), vec![0, 4, 9]);
        assert_eq!(pcs("A-"), vec![0, 4, 9]);
        assert_eq!(pcs("Bdim"), vec![2, 5, 11]);
        assert_eq!(pcs("B°"), vec![2, 5, 11]);
        assert_eq!(pcs("Caug"), vec![0, 4, 8]);
        assert_eq!(pcs("C+"), vec![0, 4, 8]);
        assert_eq!(pcs("Csus4"), vec![0, 5, 7]);
        assert_eq!(pcs("Dsus2"), vec![2, 4, 9]);
        assert_eq!(pcs("E5"), vec![4, 11]);
        assert_eq!(pcs("G7"), vec![2, 5, 7, 11]);
        assert_eq!(pcs("Cmaj7"), vec![0, 4, 7, 11]);
        assert_eq!(pcs("CM7"), vec![0, 4, 7, 11]);
        assert_eq!(pcs("CΔ7"), vec![0, 4, 7, 11]);
        assert_eq!(pcs("C△7"), vec![0, 4, 7, 11]);
        assert_eq!(pcs("Dm7"), vec![0, 2, 5, 9]);
        assert_eq!(pcs("Bm7b5"), vec![2, 5, 9, 11]);
        assert_eq!(pcs("Bø"), vec![2, 5, 9, 11]);
        assert_eq!(pcs("Bdim7"), vec![2, 5, 8, 11]);
        assert_eq!(pcs("CmMaj7"), vec![0, 3, 7, 11]);
        assert_eq!(pcs("Cm(maj7)"), vec![0, 3, 7, 11]);
        assert_eq!(pcs("G7sus4"), vec![0, 2, 5, 7]);
        assert_eq!(pcs("C6"), vec![0, 4, 7, 9]);
        assert_eq!(pcs("Am6"), vec![0, 4, 6, 9]);
        assert_eq!(pcs("C69"), vec![0, 2, 4, 7, 9]);
        assert_eq!(pcs("C6/9"), vec![0, 2, 4, 7, 9]);
        assert_eq!(pcs("Eb"), vec![3, 7, 10]);
        assert_eq!(pcs("F#m"), vec![1, 6, 9]);
    }

    #[test]
    fn extensions_adds_and_alterations() {
        assert_eq!(pcs("Cmaj9"), vec![0, 2, 4, 7, 11]);
        assert_eq!(pcs("Dm9"), vec![0, 2, 4, 5, 9]);
        assert_eq!(pcs("G9"), vec![2, 5, 7, 9, 11]);
        // 属 11 は 3 度を省く
        assert_eq!(pcs("G11"), vec![0, 2, 5, 7, 9]);
        assert_eq!(pcs("Dm11"), vec![0, 2, 4, 5, 7, 9]);
        // 13 は 11 を省く
        assert_eq!(pcs("G13"), vec![2, 4, 5, 7, 9, 11]);
        assert_eq!(pcs("Cadd9"), vec![0, 2, 4, 7]);
        assert_eq!(pcs("Cadd2"), vec![0, 2, 4, 7]);
        assert_eq!(pcs("Cmadd9"), vec![0, 2, 3, 7]);
        assert_eq!(pcs("Cma7"), vec![0, 4, 7, 11]);
        assert_eq!(pcs("Cmin7"), vec![0, 3, 7, 10]);
        assert_eq!(pcs("G7b9"), vec![2, 5, 7, 8, 11]);
        assert_eq!(pcs("G7(b9)"), vec![2, 5, 7, 8, 11]);
        assert_eq!(pcs("G7#9"), vec![2, 5, 7, 10, 11]);
        assert_eq!(pcs("C7#11"), vec![0, 4, 6, 7, 10]);
        assert_eq!(pcs("G7b13"), vec![2, 3, 5, 7, 11]);
        assert_eq!(pcs("G7alt"), vec![1, 3, 5, 7, 8, 10, 11]);
        assert_eq!(pcs("C7b5"), vec![0, 4, 6, 10]);
        assert_eq!(pcs("C7#5"), vec![0, 4, 8, 10]);
        // 13(b9): b9 が 9 を置き換える
        assert_eq!(pcs("G13b9"), vec![2, 4, 5, 7, 8, 11]);
        // 役割
        let g7 = parse("G7").unwrap().unwrap();
        assert!(g7.tones.iter().any(|t| t.role == Role::Fifth && t.optional));
        let b = parse("Bm7b5").unwrap().unwrap();
        assert!(b.tones.iter().any(|t| t.role == Role::Fifth && !t.optional));
    }

    #[test]
    fn slash_chords_rests_and_errors() {
        let c = parse("C/E").unwrap().unwrap();
        assert_eq!((c.root, c.bass, c.bass_pc()), (0, Some(4), 4));
        assert_eq!(c.name, "C/E");
        let g = parse("G7/B").unwrap().unwrap();
        assert_eq!(g.bass, Some(11));
        assert_eq!(parse("Bb/D").unwrap().unwrap().name, "Bb/D");
        // 書かれた綴りのまま
        assert_eq!(parse("Dbmaj7").unwrap().unwrap().name, "Dbmaj7");
        assert_eq!(parse("C#m7").unwrap().unwrap().name, "C#m7");
        assert_eq!(parse("Ab/Eb").unwrap().unwrap().name, "Ab/Eb");
        assert_eq!(parse("N.C.").unwrap(), None);
        assert!(parse("H7").is_err());
        assert!(parse("Cxyz").is_err());
        assert!(parse("C/Q").is_err());
    }

    #[test]
    fn roman_numerals_in_a_key() {
        let c = Key::parse("C major").unwrap();
        let am = Key::parse("A minor").unwrap();
        let get = |s: &str, k: Key| parse_in_key(s, Some(k)).unwrap().unwrap();
        assert_eq!(get("I", c).name, "C");
        assert_eq!(get("vi", c).name, "Am");
        assert_eq!(get("V7", c).name, "G7");
        assert_eq!(get("ii7", c).name, "Dm7");
        assert_eq!(get("IVmaj7", c).name, "Fmaj7");
        assert_eq!(get("bVII", c).name, "Bb");
        // ♭ 系のキーは ♭ で(F minor の VI = Db)、♯ 系は ♯ で(E major の iii = G#m)
        assert_eq!(get("VI", Key::parse("F minor").unwrap()).name, "Db");
        assert_eq!(get("iii", Key::parse("E major").unwrap()).name, "G#m");
        assert_eq!(get("viiø7", c).name, "Bm7b5");
        assert_eq!(get("iii7", c).name, "Em7");
        // 借用: V/V = D、V7/ii = A7
        assert_eq!(get("V/V", c).name, "D");
        assert_eq!(get("V7/ii", c).name, "A7");
        // 短調: i iv v VI VII と、和声的短音階の V
        assert_eq!(get("i", am).name, "Am");
        assert_eq!(get("VI", am).name, "F");
        assert_eq!(get("VII", am).name, "G");
        assert_eq!(get("V7", am).name, "E7");
        // 王道進行 IV△7–V7–iii7–vi(C)
        let names: Vec<String> = ["IVmaj7", "V7", "iii7", "vi"]
            .iter()
            .map(|s| get(s, c).name)
            .collect();
        assert_eq!(names, vec!["Fmaj7", "G7", "Em7", "Am"]);
        // 分数のローマ数字(最低音は音名)
        assert_eq!(get("I/E", c).name, "C/E");
        // キーが無ければ記号として読む
        assert_eq!(parse_in_key("Bb", None).unwrap().unwrap().name, "Bb");
        // 音名の Bb はローマ数字と間違えない
        assert_eq!(get("Bb", c).name, "Bb");
        assert_eq!(sorted(get("vi7", c).pitch_classes()), vec![0, 4, 7, 9]);
    }

    #[test]
    fn progressions_split_into_bars() {
        let b = split_progression("| Am7 | Fmaj7 | C G/B | % |").unwrap();
        assert_eq!(b.len(), 4);
        assert_eq!(b[2], vec!["C".to_owned(), "G/B".to_owned()]);
        assert_eq!(b[3], b[2]);
        assert!(split_progression("% | C").is_err());
        assert!(split_progression(" | ").is_err());
    }
}
