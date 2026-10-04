//! MusicXML(partwise)の読み込み(MCP の import_musicxml とアプリの読み込み)。
//!
//! MIDI(SMF)より情報が多い: パート名・強弱記号・スタッカートやアクセント・スラー・移調楽器の指定・
//! リハーサルマーク・拍子のまとまり。読んだものは MIDI の読み込みと同じ形([`MidiSong`])に直し、
//! トラックの作り方・音源の選び方・テンポと拍子の扱いを共通にする。
//!
//! - score-partwise と score-timewise(小節の中にパートが並ぶ形。読み込みで組み替える)
//! - 1 パート = 1 トラック。段が複数あるパート(ピアノの右手・左手など)は段ごとに分ける(2 段なら上段・下段)。
//!   声部は既定では分けない(`split_voices` で分ける)。パート名が無ければ略称 → 楽器名 → 音色(GM)の名前。
//!   `<backup>` / `<forward>` で声部の位置を戻す・進める
//! - 高さ: step + alter + octave に、移調楽器の `<transpose>`(chromatic・octave-change)を足して実音にする。
//!   打楽器(unpitched)は、その音の楽器の `<midi-unpitched>`(GM のドラムの鍵盤)、無ければ表示の高さ
//! - タイはつないで 1 音に。和音(`<chord/>`)は前の音と同じ位置
//! - 強さ: 強弱記号(pp〜ff)と `<sound dynamics>`(100 = MIDI の 90)。sf・sfz・fz は次の音だけ強く。
//!   松葉(クレッシェンド・ディミヌエンド)はその間の音を、終わりの強弱記号(無ければ 1 段 = 16)へ少しずつ動かす
//! - 奏法: スタッカート系 → staccato、アクセント系 → accent、スラーの中の 2 音目から → legato
//! - テンポ: `<sound tempo>`、無ければメトロノーム記号(拍の単位を 4 分に直す)。拍子は最初のパートから
//! - 目印: リハーサルマーク → 区間の目印
//! - 繰り返し: 反復記号(times)・1 番 2 番括弧・`<sound>` の D.C. / D.S. / Fine / To Coda を演奏の順に展開する
//!   (`expand_repeats: false` で書かれた順)。戻った小節では拍子・テンポ・強さを書かれた順のものに戻す
//! - 装飾音符: 主音符の拍の頭から短く(32 分か、主音符の長さの一部)並べ、主音符はその後ろから。斜線の無い
//!   前打音 1 つは主音符の半分。`steal-time-previous` のものは拍の前に置く。キュー音符は飛ばす。.mxl(zip)も読む

use crate::midi::{ImportMidiRequest, Imported, MidiSong, Part, RawNote};
use glaux_core::{Articulation, SectionMarker, TempoEvent, Tick, TimeSigEvent, PPQ};
use roxmltree::{Document, Node};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;

/// 読み込んだ結果と、移せなかったことの報告
pub struct ScoreRead {
    pub song: MidiSong,
    pub notes: Vec<String>,
    pub title: Option<String>,
}

/// .mxl(zip)なら中の楽譜を取り出す。そうでなければそのまま文字列に
pub fn read_text(bytes: &[u8]) -> Result<String, String> {
    if bytes.starts_with(b"PK") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!(".mxl を開けません: {e}"))?;
        // META-INF/container.xml の rootfile、無ければ最初の .xml / .musicxml
        let mut root: Option<String> = None;
        if let Ok(mut f) = z.by_name("META-INF/container.xml") {
            let mut s = String::new();
            let _ = f.read_to_string(&mut s);
            let opt = roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            };
            if let Ok(doc) = Document::parse_with_options(&s, opt) {
                root = doc
                    .descendants()
                    .find(|n| n.has_tag_name("rootfile"))
                    .and_then(|n| n.attribute("full-path"))
                    .map(str::to_owned);
            }
        }
        let name = match root {
            Some(r) => r,
            None => (0..z.len())
                .filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_owned()))
                .find(|n| {
                    !n.starts_with("META-INF") && (n.ends_with(".xml") || n.ends_with(".musicxml"))
                })
                .ok_or(".mxl の中に楽譜がありません")?,
        };
        let mut f = z
            .by_name(&name)
            .map_err(|e| format!(".mxl の中の {name} を読めません: {e}"))?;
        let mut inner = Vec::new();
        f.read_to_end(&mut inner)
            .map_err(|e| format!(".mxl の中の {name} を読めません: {e}"))?;
        return Ok(decode(&inner));
    }
    Ok(decode(bytes))
}

/// 文字列に直す。UTF-16(Finale などが書き出す。BOM で見分ける)・UTF-8 以外の 1 バイトの文字コード
/// (Latin-1 とみなす)も読む
fn decode(bytes: &[u8]) -> String {
    // UTF-16(Finale などが書き出す。BOM で見分ける)は UTF-8 に直す
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let s = if bytes.starts_with(&[0xff, 0xfe]) {
        utf16(false)
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        utf16(true)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => s.to_owned(),
            // UTF-8 でなければ Latin-1(ISO-8859-1。古い楽譜ソフトの書き出し)とみなす
            Err(_) => bytes.iter().map(|&b| b as char).collect(),
        }
    };
    let s = s.trim_start_matches('\u{feff}');
    // 宣言の encoding(UTF-16・ISO-8859-1 など)が残っていると roxmltree が読めないので、宣言を置き換える
    match s.find("?>") {
        Some(end)
            if s.starts_with("<?xml") && s[..end].to_ascii_lowercase().contains("encoding") =>
        {
            format!("<?xml version=\"1.0\"?>{}", &s[end + 2..])
        }
        _ => s.to_owned(),
    }
}

fn child<'a>(n: Node<'a, 'a>, name: &str) -> Option<Node<'a, 'a>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn text_of(n: Node, name: &str) -> Option<String> {
    child(n, name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_owned())
}

fn num<T: std::str::FromStr>(n: Node, name: &str) -> Option<T> {
    text_of(n, name).and_then(|t| t.parse().ok())
}

/// 強弱記号 → 強さ
fn dynamic_vel(name: &str) -> Option<u8> {
    Some(match name {
        "pppp" => 22,
        "ppp" => 30,
        "pp" => 42,
        "p" => 55,
        "mp" => 68,
        "mf" => 80,
        "f" => 96,
        "ff" => 110,
        "fff" => 120,
        "ffff" => 126,
        _ => return None,
    })
}

/// 拍の単位 → 4 分に対する倍率
fn beat_unit(unit: &str, dots: usize) -> f64 {
    let base = match unit {
        "whole" => 4.0,
        "half" => 2.0,
        "quarter" => 1.0,
        "eighth" => 0.5,
        "16th" => 0.25,
        "32nd" => 0.125,
        _ => 1.0,
    };
    let mut m = base;
    let mut add = base / 2.0;
    for _ in 0..dots {
        m += add;
        add /= 2.0;
    }
    m
}

/// パートの情報(part-list から)
#[derive(Default, Clone)]
struct PartInfo {
    name: String,
    channel: Option<u8>,
    program: Option<u8>,
    volume: Option<u8>,
    pan: Option<u8>,
    /// 楽器 ID → GM の鍵盤(打楽器)
    unpitched: HashMap<String, u8>,
}

/// 読み方の指定
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    /// 繰り返し記号・1 番 2 番括弧・D.C. / D.S. / Fine / Coda を演奏の順に展開する(既定 true)
    pub expand_repeats: bool,
    /// 1 つの段の中の声部(ソプラノとアルトなど)もトラックに分ける(既定 false。ピアノの和音は分けない方が扱いやすい)
    pub split_voices: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            expand_repeats: true,
            split_voices: false,
        }
    }
}

/// 音名 → ド からの半音
fn step_semis(step: Option<&str>) -> i32 {
    match step {
        Some("D") => 2,
        Some("E") => 4,
        Some("F") => 5,
        Some("G") => 7,
        Some("A") => 9,
        Some("B") => 11,
        _ => 0,
    }
}

/// 方向(`<direction>` / `<sound>`)が決めるテンポ(sound tempo、無ければメトロノーム記号を 4 分に直したもの)
fn direction_tempo(e: Node) -> Option<f64> {
    let sound = if e.has_tag_name("sound") {
        Some(e)
    } else {
        child(e, "sound")
    };
    let tempo = sound
        .and_then(|s| s.attribute("tempo"))
        .and_then(|t| t.parse::<f64>().ok())
        .or_else(|| {
            let m = e.descendants().find(|c| c.has_tag_name("metronome"))?;
            let unit = text_of(m, "beat-unit").unwrap_or_default();
            let dots = m
                .children()
                .filter(|c| c.has_tag_name("beat-unit-dot"))
                .count();
            num::<f64>(m, "per-minute").map(|pm| pm * beat_unit(&unit, dots))
        });
    tempo.filter(|b| *b > 0.0 && *b < 1000.0)
}

/// 方向が決める強さ(sound dynamics か強弱記号。sf などの一時的な強さは除く)
fn direction_dynamic(e: Node) -> Option<u8> {
    let mut vel = None;
    let sound = if e.has_tag_name("sound") {
        Some(e)
    } else {
        child(e, "sound")
    };
    if let Some(d) = sound
        .and_then(|s| s.attribute("dynamics"))
        .and_then(|t| t.parse::<f64>().ok())
    {
        vel = Some((d * 0.9).round().clamp(1.0, 127.0) as u8);
    }
    for d in e
        .children()
        .filter(|c| c.has_tag_name("direction-type"))
        .flat_map(|dt| dt.children().filter(|c| c.has_tag_name("dynamics")))
        .flat_map(|x| x.children().filter(|c| c.is_element()))
    {
        if let Some(v) = dynamic_vel(d.tag_name().name()) {
            vel = Some(v);
        }
    }
    vel
}

/// 拍子(`<time>`)→(拍の数, 拍の単位, まとまり)
fn time_sig(t: Node) -> Option<(u8, u8, Option<Vec<u8>>)> {
    let beats = text_of(t, "beats").unwrap_or_default();
    let den: u8 = num(t, "beat-type").unwrap_or(4);
    let parts: Vec<u8> = beats
        .split('+')
        .filter_map(|x| x.trim().parse().ok())
        .collect();
    let n: u32 = parts.iter().map(|&x| x as u32).sum();
    (n > 0 && n <= 64).then(|| (n as u8, den, (parts.len() > 1).then_some(parts)))
}

/// 小節の頭の時点で効いている状態(繰り返しで戻ったときに、書かれた順の状態に戻すため)
#[derive(Clone, Debug, PartialEq)]
struct MeasureState {
    divisions: f64,
    transpose: i32,
    vel: u8,
    tempo: Option<f64>,
    sig: Option<(u8, u8, Option<Vec<u8>>)>,
}

/// 書かれた順に小節をなめて、各小節の頭の状態を求める
fn measure_states(measures: &[Option<Node>]) -> Vec<MeasureState> {
    let mut st = MeasureState {
        divisions: 1.0,
        transpose: 0,
        vel: 80,
        tempo: None,
        sig: None,
    };
    let mut out = Vec::with_capacity(measures.len());
    for m in measures {
        out.push(st.clone());
        let Some(m) = m else { continue };
        for e in m.children().filter(|c| c.is_element()) {
            match e.tag_name().name() {
                "attributes" => {
                    if let Some(d) = num::<f64>(e, "divisions").filter(|d| *d > 0.0) {
                        st.divisions = d;
                    }
                    if let Some(t) = child(e, "transpose") {
                        st.transpose = num::<i32>(t, "chromatic").unwrap_or(0)
                            + 12 * num::<i32>(t, "octave-change").unwrap_or(0);
                    }
                    if let Some(t) = child(e, "time").and_then(time_sig) {
                        st.sig = Some(t);
                    }
                }
                "direction" | "sound" => {
                    if let Some(v) = direction_dynamic(e) {
                        st.vel = v;
                    }
                    if let Some(t) = direction_tempo(e) {
                        st.tempo = Some(t);
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// 小節の `<barline>` で、指定の位置(left / right)のもの
fn barlines<'a>(m: Node<'a, 'a>, location: &'a str) -> impl Iterator<Item = Node<'a, 'a>> + 'a {
    m.children().filter(move |c| {
        c.has_tag_name("barline") && c.attribute("location").unwrap_or("right") == location
    })
}

fn repeat_dir(m: Node, location: &str, dir: &str) -> Option<u32> {
    barlines(m, location)
        .filter_map(|b| child(b, "repeat"))
        .find(|r| r.attribute("direction") == Some(dir))
        .map(|r| {
            r.attribute("times")
                .and_then(|t| t.parse().ok())
                .unwrap_or(2)
        })
}

/// 複縦線(区切りの線)か。始まりの反復記号の無い反復は、直前の複縦線まで戻る
fn double_bar(m: Node, location: &str) -> bool {
    barlines(m, location).any(|b| {
        matches!(
            text_of(b, "bar-style").as_deref(),
            Some("light-light" | "light-heavy" | "heavy-light" | "heavy-heavy")
        )
    })
}

/// 括弧(1 番・2 番)の (番号の一覧, 種類)。番号は "1, 2" や "1-3"
fn ending(m: Node, location: &str) -> Option<(Vec<u32>, String)> {
    barlines(m, location)
        .filter_map(|b| child(b, "ending"))
        .next()
        .map(|e| {
            let mut nums = Vec::new();
            for part in e.attribute("number").unwrap_or("1").split([',', ' ']) {
                let part = part.trim();
                if let Some((a, b)) = part.split_once('-') {
                    if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                        nums.extend(a..=b.min(a + 16));
                    }
                } else if let Ok(n) = part.parse() {
                    nums.push(n);
                }
            }
            (nums, e.attribute("type").unwrap_or("start").to_owned())
        })
}

/// 小節の中の `<sound>` の属性(direction の中も含む)
fn sound_attr<'a>(m: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    m.descendants()
        .filter(|c| c.has_tag_name("sound"))
        .find_map(|s| s.attribute(name))
}

/// 演奏の順(書かれた小節の番号の並び)。繰り返し・括弧・D.C. / D.S. / Fine / To Coda を展開する。
/// 2 つ目は展開する記号があったか
fn play_order(measures: &[Option<Node>]) -> (Vec<usize>, bool) {
    let n = measures.len();
    let mut segnos: HashMap<String, usize> = HashMap::new();
    let mut codas: HashMap<String, usize> = HashMap::new();
    let mut any = false;
    for (i, m) in measures.iter().enumerate() {
        let Some(m) = m else { continue };
        if let Some(s) = sound_attr(*m, "segno") {
            segnos.entry(s.to_owned()).or_insert(i);
        }
        if let Some(c) = sound_attr(*m, "coda") {
            codas.entry(c.to_owned()).or_insert(i);
        }
        any |= m
            .children()
            .filter(|c| c.has_tag_name("barline"))
            .any(|b| child(b, "repeat").is_some() || child(b, "ending").is_some())
            || ["dacapo", "dalsegno", "tocoda", "fine"]
                .iter()
                .any(|a| sound_attr(*m, a).is_some());
    }
    if !any {
        return ((0..n).collect(), false);
    }
    // 括弧の終わり(start の小節から、その括弧が閉じる小節)
    let ending_end = |from: usize| -> usize {
        for (j, m) in measures.iter().enumerate().skip(from) {
            if let Some(m) = m {
                if let Some((_, ty)) = ending(*m, "right") {
                    if ty == "stop" || ty == "discontinue" {
                        return j;
                    }
                }
                // 次の括弧が始まったら、その手前まで
                if j > from && ending(*m, "left").is_some_and(|(_, ty)| ty == "start") {
                    return j - 1;
                }
            }
        }
        from
    };
    let mut order = Vec::new();
    let mut i = 0usize;
    let mut repeat_start = 0usize;
    let mut pass = 1u32;
    let mut taken: HashMap<usize, u32> = HashMap::new();
    let mut back_jump = false;
    // D.C. / D.S. の後(繰り返しは 1 回だけ、Fine で終わり、To Coda で Coda へ)
    let mut after_jump = false;
    let mut jumped = false;
    let limit = 16 * n + 64;
    while i < n && order.len() < limit {
        let Some(m) = measures[i] else {
            order.push(i);
            i += 1;
            continue;
        };
        if repeat_dir(m, "left", "forward").is_some() {
            if !back_jump {
                pass = 1;
            }
            repeat_start = i;
        } else if !back_jump && double_bar(m, "left") {
            repeat_start = i;
        }
        back_jump = false;
        if let Some((nums, ty)) = ending(m, "left") {
            if ty == "start" {
                let end = ending_end(i);
                // D.C. の後は最後の括弧を通る。それ以外は今の回の番号の括弧
                let skip = if after_jump {
                    measures
                        .get(end + 1)
                        .copied()
                        .flatten()
                        .is_some_and(|nx| ending(nx, "left").is_some_and(|(_, t)| t == "start"))
                } else {
                    !nums.contains(&pass)
                };
                if skip {
                    i = end + 1;
                    continue;
                }
            }
        }
        order.push(i);
        if after_jump && sound_attr(m, "fine").is_some() {
            break;
        }
        if after_jump {
            if let Some(c) = sound_attr(m, "tocoda").and_then(|c| codas.get(c)) {
                if *c > i {
                    i = *c;
                    continue;
                }
            }
        }
        if let Some(times) = repeat_dir(m, "right", "backward") {
            if !after_jump {
                let t = taken.entry(i).or_insert(1);
                if *t < times {
                    *t += 1;
                    pass += 1;
                    i = repeat_start;
                    back_jump = true;
                    continue;
                }
                taken.remove(&i);
                pass = 1;
            }
            repeat_start = i + 1;
        } else if double_bar(m, "right") && ending(m, "right").is_none() {
            repeat_start = i + 1;
        }
        if !jumped {
            if sound_attr(m, "dacapo") == Some("yes") {
                jumped = true;
                after_jump = true;
                i = 0;
                continue;
            }
            if let Some(s) = sound_attr(m, "dalsegno").and_then(|s| segnos.get(s)) {
                jumped = true;
                after_jump = true;
                i = *s;
                continue;
            }
        }
        i += 1;
    }
    (order, true)
}

/// トラックの分け方の鍵(段, 声部)。声部を分けないときは声部を 0 にする
type VoiceKey = (u32, u32);

/// 主音符を待っている装飾音符
struct Grace {
    pitch: u8,
    slash: bool,
    chord: bool,
    before: bool,
}

/// 松葉(クレッシェンド / ディミヌエンド)
struct Wedge {
    start: f64,
    stop: f64,
    cresc: bool,
}

/// 松葉の分だけ強さを動かす。目標は松葉の終わりから 1 小節ほどの強弱記号(向きが合うもの)、
/// 無ければ 1 段(16)。目標の記号が無いときは、次の強弱記号まで上げた(下げた)ままにする
fn apply_wedges(notes: &mut [RawNote], wedges: &[Wedge], dyns: &[(f64, u8)]) {
    let level_at = |t: f64| {
        dyns.iter()
            .rev()
            .find(|d| d.0 <= t + 0.5)
            .map_or(80, |d| d.1)
    };
    for w in wedges {
        if w.stop <= w.start {
            continue;
        }
        let v0 = level_at(w.start) as f64;
        let target = dyns
            .iter()
            .find(|d| d.0 > w.start + 0.5 && d.0 <= w.stop + 4.0 * PPQ as f64)
            .map(|d| d.1 as f64)
            .filter(|&v| if w.cresc { v > v0 } else { v < v0 });
        let v1 = target.unwrap_or(if w.cresc {
            (v0 + 16.0).min(127.0)
        } else {
            (v0 - 16.0).max(20.0)
        });
        let next_dyn = dyns
            .iter()
            .find(|d| d.0 > w.stop + 0.5)
            .map_or(f64::MAX, |d| d.0);
        for n in notes.iter_mut() {
            let t = n.0 as f64;
            let want = if t >= w.start && t <= w.stop {
                v0 + (v1 - v0) * (t - w.start) / (w.stop - w.start)
            } else if target.is_none() && t > w.stop && t < next_dyn {
                v1
            } else {
                continue;
            };
            // その音の強さは、その時点の強弱記号の強さ(+ sf などの上乗せ)。記号の強さとの差だけ動かす
            let add = want - level_at(t) as f64;
            n.3 = (n.3 as f64 + add).round().clamp(1.0, 127.0) as u8;
        }
    }
}

pub fn parse(xml: &str) -> Result<ScoreRead, String> {
    parse_with(xml, ParseOptions::default())
}

pub fn parse_with(xml: &str, opts: ParseOptions) -> Result<ScoreRead, String> {
    // MusicXML はほぼ必ず DOCTYPE(外部の DTD の参照)を持つ。DTD は許すが外へは取りに行かない
    let opt = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = Document::parse_with_options(xml, opt)
        .map_err(|e| format!("MusicXML を読めません: {e}"))?;
    let root = doc.root_element();
    let timewise = root.has_tag_name("score-timewise");
    if !timewise && !root.has_tag_name("score-partwise") {
        return Err(format!(
            "MusicXML ではありません(ルートの要素が {})",
            root.tag_name().name()
        ));
    }
    let title = child(root, "work")
        .and_then(|w| text_of(w, "work-title"))
        .or_else(|| text_of(root, "movement-title"));
    // part-list
    let mut infos: HashMap<String, PartInfo> = HashMap::new();
    let mut list_order: Vec<String> = Vec::new();
    if let Some(pl) = child(root, "part-list") {
        for sp in pl.children().filter(|c| c.has_tag_name("score-part")) {
            let id = sp.attribute("id").unwrap_or_default().to_owned();
            // 名前: パート名 → 略称 → 楽器名(空の名前は飛ばす)。どれも無ければ後で音色(GM)の名前
            let named = |tag: &str| text_of(sp, tag).filter(|t| !t.is_empty());
            let name = named("part-name")
                .or_else(|| named("part-abbreviation"))
                .or_else(|| {
                    sp.descendants()
                        .find(|c| c.has_tag_name("instrument-name"))
                        .and_then(|c| c.text())
                        .map(|t| t.trim().to_owned())
                        .filter(|t| !t.is_empty())
                })
                .unwrap_or_default();
            let mut info = PartInfo {
                name,
                ..Default::default()
            };
            for mi in sp.children().filter(|c| c.has_tag_name("midi-instrument")) {
                let inst = mi.attribute("id").unwrap_or_default().to_owned();
                if let Some(ch) = num::<u8>(mi, "midi-channel") {
                    info.channel.get_or_insert(ch.saturating_sub(1).min(15));
                }
                if let Some(pg) = num::<u8>(mi, "midi-program") {
                    info.program.get_or_insert(pg.saturating_sub(1).min(127));
                }
                if let Some(v) = num::<f64>(mi, "volume") {
                    info.volume
                        .get_or_insert((v.clamp(0.0, 100.0) * 1.27).round() as u8);
                }
                if let Some(p) = num::<f64>(mi, "pan") {
                    info.pan
                        .get_or_insert(((p.clamp(-90.0, 90.0) / 90.0) * 63.0 + 64.0).round() as u8);
                }
                if let Some(u) = num::<u8>(mi, "midi-unpitched") {
                    info.unpitched.insert(inst, u.saturating_sub(1).min(127));
                }
            }
            list_order.push(id.clone());
            infos.insert(id, info);
        }
    }
    // パートごとの小節の中身。partwise は <part><measure>…、timewise は <measure><part>… を組み替える
    let parts: Vec<(String, Vec<Option<Node>>)> = if timewise {
        let measures: Vec<Node> = root
            .children()
            .filter(|c| c.has_tag_name("measure"))
            .collect();
        let mut ids = list_order.clone();
        for m in &measures {
            for p in m.children().filter(|c| c.has_tag_name("part")) {
                let id = p.attribute("id").unwrap_or_default();
                if !ids.iter().any(|x| x == id) {
                    ids.push(id.to_owned());
                }
            }
        }
        ids.into_iter()
            .map(|id| {
                let ms = measures
                    .iter()
                    .map(|m| {
                        m.children()
                            .find(|p| p.has_tag_name("part") && p.attribute("id") == Some(&id))
                    })
                    .collect();
                (id, ms)
            })
            .collect()
    } else {
        root.children()
            .filter(|c| c.has_tag_name("part"))
            .map(|p| {
                (
                    p.attribute("id").unwrap_or_default().to_owned(),
                    p.children()
                        .filter(|c| c.has_tag_name("measure"))
                        .map(Some)
                        .collect(),
                )
            })
            .collect()
    };
    let mut song = MidiSong::default();
    let mut report: Vec<String> = Vec::new();
    // 演奏の順は最初のパートの繰り返し記号から(どのパートも同じ小節割りのはず)
    let written = parts.first().map_or(0, |p| p.1.len());
    let (order, has_repeats) = match parts.first() {
        Some((_, ms)) if opts.expand_repeats => play_order(ms),
        _ => ((0..written).collect(), false),
    };
    if has_repeats {
        report.push(format!(
            "繰り返し記号を演奏の順に展開しました(書かれた {written} 小節 → {} 小節)",
            order.len()
        ));
    } else if !opts.expand_repeats {
        let n = parts.first().map_or(0, |(_, ms)| {
            ms.iter()
                .flatten()
                .filter(|m| {
                    m.children()
                        .filter(|c| c.has_tag_name("barline"))
                        .any(|b| child(b, "repeat").is_some())
                })
                .count()
        });
        if n > 0 {
            report.push(format!(
                "繰り返し記号({n} か所)は展開していません(書かれた順に 1 回ずつ並べています)"
            ));
        }
    }
    let mut graces_placed = 0usize;
    let mut graces_dropped = 0usize;
    let mut cues = 0usize;
    let mut names_seen: HashMap<String, usize> = HashMap::new();
    for (pi, (id, measures)) in parts.iter().enumerate() {
        let id = id.as_str();
        let info = infos.get(id).cloned().unwrap_or_else(|| PartInfo {
            name: id.to_owned(),
            ..Default::default()
        });
        let first = pi == 0;
        let states = measure_states(measures);
        let mut divisions = 1.0f64;
        let mut transpose = 0i32;
        let mut vel: u8 = 80;
        let mut sforzando = false;
        let mut pos: f64 = 0.0; // tick(小数のまま進め、音の頭で丸める)
        let mut measure_start: f64 = 0.0;
        let mut emitted_sig: Option<(u8, u8, Option<Vec<u8>>)> = None;
        let mut emitted_tempo: Option<f64> = None;
        // (段, 声部)ごとの音(段が 1 つで声部を分けなければ 1 つだけ)
        let mut tracks: BTreeMap<VoiceKey, (Vec<RawNote>, BTreeMap<usize, Articulation>)> =
            BTreeMap::new();
        // タイの途中の音((段, 声部, 音高) → その鍵の notes の番号)
        let mut open_ties: HashMap<(VoiceKey, u8), usize> = HashMap::new();
        // スラーの中か(番号ごと)
        let mut slurs: HashMap<String, bool> = HashMap::new();
        // 主音符を待っている装飾音符
        let mut graces: HashMap<VoiceKey, Vec<Grace>> = HashMap::new();
        // 強さの変わり目と松葉(強さを後から動かす)
        let mut dyns: Vec<(f64, u8)> = Vec::new();
        let mut wedges: Vec<Wedge> = Vec::new();
        let mut open_wedges: HashMap<String, (f64, bool)> = HashMap::new();
        let mut last_onset: f64 = 0.0;
        // 直前の主音符の頭を装飾音符の分だけずらした量(和音の残りの音も同じだけずらす)
        let mut last_shift: f64 = 0.0;
        let mut any_unpitched = false;
        for &mi in &order {
            // 小節の頭の状態を書かれた順のものに(繰り返しで戻ったときの拍子・テンポ・強さ)
            if let Some(st) = states.get(mi) {
                divisions = st.divisions;
                transpose = st.transpose;
                if st.vel != vel {
                    vel = st.vel;
                    dyns.push((pos, vel));
                }
                if first {
                    if let Some(t) = st.tempo.filter(|t| Some(*t) != emitted_tempo) {
                        if emitted_tempo.is_some() {
                            let tick = Tick(pos.round() as u64);
                            song.tempos.retain(|x| x.tick != tick);
                            song.tempos.push(TempoEvent { tick, bpm: t });
                        }
                        emitted_tempo = Some(t);
                    }
                    if st.sig.is_some() && st.sig != emitted_sig && emitted_sig.is_some() {
                        let (num, den, grouping) = st.sig.clone().unwrap_or((4, 4, None));
                        song.sigs.push(TimeSigEvent {
                            tick: Tick(pos.round() as u64),
                            num,
                            den,
                            grouping,
                        });
                        emitted_sig = st.sig.clone();
                    }
                }
            }
            let Some(m) = measures.get(mi).copied().flatten() else {
                continue;
            };
            let mut furthest = pos;
            for e in m.children().filter(|c| c.is_element()) {
                match e.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = num::<f64>(e, "divisions") {
                            if d > 0.0 {
                                divisions = d;
                            }
                        }
                        if let Some(t) = child(e, "transpose") {
                            transpose = num::<i32>(t, "chromatic").unwrap_or(0)
                                + 12 * num::<i32>(t, "octave-change").unwrap_or(0);
                        }
                        if first {
                            if let Some((num, den, grouping)) = child(e, "time").and_then(time_sig)
                            {
                                emitted_sig = Some((num, den, grouping.clone()));
                                song.sigs.push(TimeSigEvent {
                                    tick: Tick(pos.round() as u64),
                                    num,
                                    den,
                                    grouping,
                                });
                            }
                        }
                    }
                    "direction" | "sound" => {
                        let at = pos
                            + child(e, "offset")
                                .and_then(|o| o.text())
                                .and_then(|t| t.trim().parse::<f64>().ok())
                                .map_or(0.0, |o| o * PPQ as f64 / divisions);
                        if let Some(v) = direction_dynamic(e) {
                            vel = v;
                            dyns.push((at, v));
                        }
                        for dt in e.children().filter(|c| c.has_tag_name("direction-type")) {
                            for x in dt.children().filter(|c| c.is_element()) {
                                match x.tag_name().name() {
                                    "dynamics" => {
                                        if x.children().filter(|c| c.is_element()).any(|d| {
                                            matches!(
                                                d.tag_name().name(),
                                                "sf" | "sfz" | "fz" | "sfp" | "rfz" | "sffz"
                                            )
                                        }) {
                                            sforzando = true;
                                        }
                                    }
                                    "wedge" => {
                                        let n = x.attribute("number").unwrap_or("1").to_owned();
                                        match x.attribute("type") {
                                            Some("crescendo") => {
                                                open_wedges.insert(n, (at, true));
                                            }
                                            Some("diminuendo") => {
                                                open_wedges.insert(n, (at, false));
                                            }
                                            Some("stop") => {
                                                if let Some((s, cresc)) = open_wedges.remove(&n) {
                                                    wedges.push(Wedge {
                                                        start: s,
                                                        stop: at,
                                                        cresc,
                                                    });
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    "rehearsal" if first => {
                                        if let Some(t) = x.text() {
                                            song.markers.push(SectionMarker {
                                                tick: Tick(at.round() as u64),
                                                name: t.trim().to_owned(),
                                                ..Default::default()
                                            });
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if first {
                            if let Some(bpm) = direction_tempo(e) {
                                let tick = Tick(at.round() as u64);
                                song.tempos.retain(|t| t.tick != tick);
                                song.tempos.push(TempoEvent { tick, bpm });
                                emitted_tempo = Some(bpm);
                            }
                        }
                    }
                    "backup" => {
                        let d = num::<f64>(e, "duration").unwrap_or(0.0) * PPQ as f64 / divisions;
                        pos = (pos - d).max(measure_start);
                    }
                    "forward" => {
                        let d = num::<f64>(e, "duration").unwrap_or(0.0) * PPQ as f64 / divisions;
                        pos += d;
                        furthest = furthest.max(pos);
                    }
                    "note" => {
                        if child(e, "cue").is_some() {
                            cues += 1;
                            continue;
                        }
                        let staff: u32 = num(e, "staff").unwrap_or(1);
                        let voice: u32 = if opts.split_voices {
                            num(e, "voice").unwrap_or(1)
                        } else {
                            0
                        };
                        let key = (staff, voice);
                        let pitch: Option<i32> = if let Some(p) = child(e, "pitch") {
                            let step = step_semis(text_of(p, "step").as_deref());
                            let alter = num::<f64>(p, "alter").unwrap_or(0.0).round() as i32;
                            let oct = num::<i32>(p, "octave").unwrap_or(4);
                            Some((oct + 1) * 12 + step + alter + transpose)
                        } else if let Some(u) = child(e, "unpitched") {
                            any_unpitched = true;
                            let inst = child(e, "instrument").and_then(|i| i.attribute("id"));
                            inst.and_then(|i| info.unpitched.get(i).map(|&k| k as i32))
                                .or_else(|| {
                                    let step = step_semis(text_of(u, "display-step").as_deref());
                                    num::<i32>(u, "display-octave").map(|o| (o + 1) * 12 + step)
                                })
                        } else {
                            None // 休み
                        };
                        let pitch = pitch.filter(|p| (0..=127).contains(p)).map(|p| p as u8);
                        // 装飾音符: 長さが無いので、次の主音符が来るまで待たせる
                        if let Some(g) = child(e, "grace") {
                            if let Some(p) = pitch {
                                graces.entry(key).or_default().push(Grace {
                                    pitch: p,
                                    slash: g.attribute("slash") == Some("yes"),
                                    chord: child(e, "chord").is_some(),
                                    before: g.attribute("steal-time-previous").is_some(),
                                });
                            }
                            continue;
                        }
                        let dur = num::<f64>(e, "duration").unwrap_or(0.0) * PPQ as f64 / divisions;
                        let chord = child(e, "chord").is_some();
                        let onset = if chord { last_onset } else { pos };
                        if !chord {
                            last_onset = pos;
                            last_shift = 0.0;
                        }
                        let pending = if chord {
                            Vec::new()
                        } else {
                            graces.remove(&key).unwrap_or_default()
                        };
                        if let Some(p) = pitch {
                            let (notes, arts) = tracks.entry(key).or_default();
                            // 装飾音符を置く: 拍の頭から短く(前打音 1 つは主音符の半分)、主音符はその後ろから。
                            // steal-time-previous のものは拍の前に置き、主音符は動かさない
                            if !pending.is_empty() {
                                let slots = pending.iter().filter(|g| !g.chord).count().max(1);
                                let long = slots == 1 && !pending[0].slash && !pending[0].before;
                                let unit = if long {
                                    (dur / 2.0).round()
                                } else {
                                    (PPQ as f64 / 8.0).min(dur / (2.0 * slots as f64)).round()
                                };
                                let before = pending[0].before;
                                let base = if before {
                                    onset - unit * slots as f64
                                } else {
                                    onset
                                };
                                let mut slot = 0usize;
                                for (k, g) in pending.iter().enumerate() {
                                    if k > 0 && !g.chord {
                                        slot += 1;
                                    }
                                    let at = (base + unit * slot as f64).max(0.0);
                                    notes.push((
                                        at.round() as u64,
                                        unit.max(1.0) as u64,
                                        g.pitch,
                                        vel.saturating_sub(8).max(1),
                                    ));
                                    graces_placed += 1;
                                }
                                if !before {
                                    last_shift = unit * slots as f64;
                                }
                            }
                            let shift = last_shift.min(dur - 1.0).max(0.0);
                            let ties: Vec<&str> = e
                                .children()
                                .filter(|c| c.has_tag_name("tie"))
                                .filter_map(|c| c.attribute("type"))
                                .collect();
                            let start = (onset + shift).round() as u64;
                            let len = (dur - shift).round().max(1.0) as u64;
                            let continued =
                                ties.contains(&"stop") && open_ties.contains_key(&(key, p));
                            if continued {
                                // タイでつなぐ: 前の音を伸ばす
                                let i = open_ties[&(key, p)];
                                let n = notes[i];
                                notes[i] = (n.0, (start + len).saturating_sub(n.0), n.2, n.3);
                                if !ties.contains(&"start") {
                                    open_ties.remove(&(key, p));
                                }
                            } else {
                                let v = if sforzando {
                                    sforzando = false;
                                    vel.saturating_add(15).min(127)
                                } else {
                                    vel
                                };
                                notes.push((start, len, p, v.max(1)));
                                let i = notes.len() - 1;
                                if ties.contains(&"start") {
                                    open_ties.insert((key, p), i);
                                }
                                // 奏法とスラー
                                let mut art = None;
                                let in_slur = slurs.values().any(|&x| x);
                                if in_slur {
                                    art = Some(Articulation::Legato);
                                }
                                if let Some(nt) = child(e, "notations") {
                                    if let Some(a) = child(nt, "articulations") {
                                        for x in a.children().filter(|c| c.is_element()) {
                                            match x.tag_name().name() {
                                                "staccato" | "staccatissimo" | "spiccato" => {
                                                    art = Some(Articulation::Staccato)
                                                }
                                                "accent" | "strong-accent" => {
                                                    art = Some(Articulation::Accent)
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    for s in nt.children().filter(|c| c.has_tag_name("slur")) {
                                        let n = s.attribute("number").unwrap_or("1").to_owned();
                                        match s.attribute("type") {
                                            Some("start") => {
                                                slurs.insert(n, true);
                                            }
                                            Some("stop") => {
                                                slurs.insert(n, false);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                if let Some(a) = art {
                                    arts.insert(i, a);
                                }
                            }
                        } else {
                            // 休みの前の装飾音符は置けない
                            graces_dropped += pending.len();
                        }
                        if !chord {
                            pos += dur;
                            furthest = furthest.max(pos);
                        }
                    }
                    _ => {}
                }
            }
            // 次の小節の頭: この小節で最も進んだ所(弱起の小節は短い)
            pos = furthest.max(measure_start);
            measure_start = pos;
        }
        graces_dropped += graces.values().map(Vec::len).sum::<usize>();
        if tracks.values().all(|(n, _)| n.is_empty()) {
            report.push(format!(
                "パート「{}」には音がありません(読み込まない)",
                if info.name.is_empty() { id } else { &info.name }
            ));
            continue;
        }
        let base_name = if info.name.is_empty() {
            // パート名が無ければ音色(GM)の名前、それも無ければ番号
            match info.program {
                Some(pg) => crate::midi::GM_NAMES[pg as usize & 127].to_owned(),
                None => format!("パート {}", pi + 1),
            }
        } else {
            info.name.clone()
        };
        let drum = info.channel == Some(9) || (any_unpitched && info.channel.is_none());
        let used: Vec<VoiceKey> = tracks
            .iter()
            .filter(|(_, (n, _))| !n.is_empty())
            .map(|(k, _)| *k)
            .collect();
        let staves: Vec<u32> = {
            let mut v: Vec<u32> = used.iter().map(|k| k.0).collect();
            v.dedup();
            v
        };
        for (key, (mut notes, arts)) in tracks {
            if notes.is_empty() {
                continue;
            }
            let (staff, voice) = key;
            // 段が複数あれば段ごとにトラック(2 段なら上段・下段)。声部も分けるなら「声部 n」を足す
            let k_staff = staves.iter().position(|&s| s == staff).unwrap_or(0) + 1;
            let mut label = match (staves.len(), k_staff) {
                (1, _) => base_name.clone(),
                (2, 1) => format!("{base_name}(上段)"),
                (2, _) => format!("{base_name}(下段)"),
                _ => format!("{base_name}({staff} 段目)"),
            };
            if used.iter().filter(|k| k.0 == staff).count() > 1 {
                label = format!("{label} 声部{voice}");
            }
            // 同じ名前のトラックは番号を付ける
            let count = names_seen.entry(label.clone()).or_insert(0);
            *count += 1;
            let name = if *count > 1 {
                format!("{label} {count}")
            } else {
                label
            };
            apply_wedges(&mut notes, &wedges, &dyns);
            // 位置でそろえる(奏法の番号も並べ替えに合わせる)
            let mut order: Vec<usize> = (0..notes.len()).collect();
            order.sort_by_key(|&i| (notes[i].0, notes[i].2));
            let sorted: Vec<RawNote> = order.iter().map(|&i| notes[i]).collect();
            let arts_sorted: BTreeMap<usize, Articulation> = order
                .iter()
                .enumerate()
                .filter_map(|(new, &old)| arts.get(&old).map(|a| (new, *a)))
                .collect();
            song.parts.push(Part {
                name,
                channel: if drum {
                    9
                } else {
                    info.channel.filter(|&c| c != 9).unwrap_or(pi as u8 % 16)
                },
                program: info.program.unwrap_or(0),
                notes: sorted,
                volume: info.volume,
                pan: info.pan,
                articulations: arts_sorted,
            });
        }
    }
    if graces_placed > 0 {
        report.push(format!(
            "装飾音符 {graces_placed} 個は主音符の頭に短く置きました(前打音 1 つは主音符の半分)"
        ));
    }
    if graces_dropped > 0 {
        report.push(format!(
            "装飾音符 {graces_dropped} 個は続く音符が無いので飛ばしました"
        ));
    }
    if cues > 0 {
        report.push(format!("キュー音符 {cues} 個は飛ばしました"));
    }
    // テンポ・拍子は頭に無ければ既定、同じ位置は後のもの
    song.tempos.sort_by_key(|t| t.tick);
    if song.tempos.first().is_none_or(|t| t.tick.0 > 0) {
        song.tempos.insert(
            0,
            TempoEvent {
                tick: Tick(0),
                bpm: 120.0,
            },
        );
    }
    // 同じテンポが続くものは 1 つに(繰り返しで同じ値を置き直したもの)
    song.tempos.dedup_by(|b, a| a.bpm == b.bpm);
    song.sigs.sort_by_key(|s| s.tick);
    song.sigs.dedup_by(|b, a| {
        if a.tick == b.tick {
            *a = b.clone();
            true
        } else {
            false
        }
    });
    // 同じ拍子が続くものは 1 つに
    song.sigs
        .dedup_by(|b, a| a.num == b.num && a.den == b.den && a.grouping == b.grouping);
    if song.sigs.first().is_none_or(|s| s.tick.0 > 0) {
        song.sigs.insert(0, TimeSigEvent::new(Tick(0), 4, 4));
    }
    song.markers.sort_by_key(|m| m.tick);
    Ok(ScoreRead {
        song,
        notes: report,
        title,
    })
}

/// 楽譜のファイル(.musicxml / .xml / .mxl)かどうか(拡張子で見る)
pub fn is_score_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".musicxml") || p.ends_with(".mxl") || p.ends_with(".xml")
}

/// ファイルを読んで、読み込みのコマンドを作る(MIDI の読み込みと同じ置き方。ブロッキング)。
/// 返り値の 2 つ目は移せなかったことの報告
pub fn import_file(
    project: &glaux_core::Project,
    req: &ImportMidiRequest,
) -> Result<(Imported, Vec<String>), String> {
    let path = std::path::Path::new(&req.path);
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", req.path))?;
    let text = read_text(&bytes)?;
    let r = parse_with(
        &text,
        ParseOptions {
            expand_repeats: req.expand_repeats.unwrap_or(true),
            split_voices: req.split_voices.unwrap_or(false),
        },
    )?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let presets: Vec<(u16, u16)>;
    let inst = match &req.soundfont {
        Some(file) => {
            let font = glaux_engine::sf2::load_font(&glaux_engine::sf2::default_dir().join(file))?;
            presets = glaux_engine::sf2::list_presets(&font)
                .into_iter()
                .map(|m| (m.bank, m.preset))
                .collect();
            crate::midi::Instruments::Soundfont {
                file,
                presets: &presets,
            }
        }
        None => crate::midi::Instruments::Builtin,
    };
    let mut imp = crate::midi::import_commands(
        project,
        &r.song,
        &file_name,
        req.start_tick,
        req.set_tempo,
        &inst,
    )?;
    imp.label = format!(
        "MusicXML「{}」を読み込み(トラック {} 本)",
        r.title.as_deref().unwrap_or(&file_name),
        imp.tracks.len()
    );
    Ok((imp, r.notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="4.0">
  <work><work-title>Test</work-title></work>
  <part-list>
    <score-part id="P1"><part-name>Flute</part-name>
      <midi-instrument id="P1-I1"><midi-channel>1</midi-channel><midi-program>74</midi-program><volume>80</volume><pan>-45</pan></midi-instrument>
    </score-part>
    <score-part id="P2"><part-name>Clarinet in Bb</part-name></score-part>
    <score-part id="P3"><part-name>Drums</part-name>
      <score-instrument id="P3-I36"><instrument-name>Kick</instrument-name></score-instrument>
      <midi-instrument id="P3-I36"><midi-channel>10</midi-channel><midi-unpitched>37</midi-unpitched></midi-instrument>
    </score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>2</divisions><time><beats>3+2</beats><beat-type>8</beat-type></time></attributes>
      <direction><direction-type><rehearsal>A</rehearsal></direction-type></direction>
      <direction><direction-type><metronome><beat-unit>quarter</beat-unit><per-minute>100</per-minute></metronome></direction-type></direction>
      <direction><direction-type><dynamics><p/></dynamics></direction-type></direction>
      <note><pitch><step>C</step><octave>5</octave></pitch><duration>1</duration><notations><slur type="start" number="1"/></notations></note>
      <note><pitch><step>D</step><octave>5</octave></pitch><duration>1</duration><notations><articulations><staccato/></articulations></notations></note>
      <note><pitch><step>E</step><alter>-1</alter><octave>5</octave></pitch><duration>1</duration><notations><slur type="stop" number="1"/></notations></note>
      <note><grace/><pitch><step>F</step><octave>5</octave></pitch></note>
      <note><pitch><step>G</step><octave>5</octave></pitch><duration>2</duration><tie type="start"/></note>
    </measure>
    <measure number="2">
      <direction><direction-type><dynamics><f/></dynamics></direction-type><sound tempo="120"/></direction>
      <note><pitch><step>G</step><octave>5</octave></pitch><duration>2</duration><tie type="stop"/></note>
      <note><pitch><step>C</step><octave>6</octave></pitch><duration>2</duration><notations><articulations><accent/></articulations></notations></note>
      <note><chord/><pitch><step>E</step><octave>6</octave></pitch><duration>2</duration></note>
      <note><rest/><duration>1</duration></note>
    </measure>
  </part>
  <part id="P2">
    <measure number="1">
      <attributes><divisions>1</divisions><transpose><diatonic>-1</diatonic><chromatic>-2</chromatic></transpose></attributes>
      <note><pitch><step>D</step><octave>4</octave></pitch><duration>1</duration><voice>1</voice></note>
      <backup><duration>1</duration></backup>
      <note><pitch><step>B</step><octave>3</octave></pitch><duration>1</duration><voice>2</voice></note>
    </measure>
  </part>
  <part id="P3">
    <measure number="1">
      <attributes><divisions>1</divisions></attributes>
      <note><unpitched><display-step>F</display-step><display-octave>4</display-octave></unpitched><duration>1</duration><instrument id="P3-I36"/></note>
    </measure>
  </part>
</score-partwise>"#;

    #[test]
    fn a_score_becomes_parts_with_dynamics_articulations_and_tempo() {
        let r = parse(XML).unwrap();
        assert_eq!(r.title.as_deref(), Some("Test"));
        let s = &r.song;
        // パート名・音色(1 始まり → 0 始まり)・音量・パン
        let fl = &s.parts[0];
        assert_eq!(fl.name, "Flute");
        assert_eq!(fl.program, 73);
        assert_eq!(fl.volume, Some(102));
        assert!(fl.pan.unwrap() < 64);
        // 8 分(divisions 2 → 480 tick)。C5 D5 Eb5、G5 はタイで 4 分 + 4 分 = 1920、和音 C6・E6
        let pitches: Vec<(u64, u64, u8, u8)> = fl.notes.clone();
        assert_eq!(pitches[0], (0, 480, 72, 55));
        assert_eq!(pitches[1].2, 74);
        assert_eq!(pitches[2].2, 75);
        // G5 の前の装飾音符 F5(斜線なしの前打音)は G5 の頭の半分を使い、G5 はその後ろから(タイで次の小節まで)
        let f = pitches.iter().find(|n| n.2 == 77).unwrap();
        assert_eq!((f.0, f.1), (1440, 480));
        let g = pitches.iter().find(|n| n.2 == 79).unwrap();
        assert_eq!((g.0, g.1), (1920, 1440));
        // 2 小節目の頭は 5/8 の後(1440 + 960 = 2400 tick。3+2 の 8 分 5 つ)
        let c6 = pitches.iter().find(|n| n.2 == 84).unwrap();
        let e6 = pitches.iter().find(|n| n.2 == 88).unwrap();
        assert_eq!(c6.0, 2400 + 960);
        assert_eq!(e6.0, c6.0);
        assert_eq!(c6.3, 96, "f の強さ");
        // 奏法: スラーの 2 音目は legato のはずがスタッカートが優先、3 音目は legato、アクセント
        assert_eq!(fl.articulations.get(&1), Some(&Articulation::Staccato));
        assert_eq!(fl.articulations.get(&2), Some(&Articulation::Legato));
        let c6_i = fl.notes.iter().position(|n| n.2 == 84).unwrap();
        assert_eq!(fl.articulations.get(&c6_i), Some(&Articulation::Accent));
        // テンポ(メトロノーム 100 → 2 小節目の sound 120)・拍子のまとまり・リハーサルマーク
        assert_eq!(s.tempos[0].bpm, 100.0);
        assert_eq!(s.tempos[1].bpm, 120.0);
        assert_eq!(s.tempos[1].tick.0, 2400);
        assert_eq!((s.sigs[0].num, s.sigs[0].den), (5, 8));
        assert_eq!(s.sigs[0].grouping, Some(vec![3, 2]));
        assert_eq!(s.markers[0].name, "A");
        // 移調楽器(B♭ クラリネット: 書かれた D4 → 実音 C4)と 2 声部
        let cl = &s.parts[1];
        assert_eq!(
            cl.notes.iter().map(|n| n.2).collect::<Vec<_>>(),
            vec![57, 60]
        );
        assert_eq!(cl.notes[0].0, cl.notes[1].0);
        // 打楽器は GM の鍵盤(midi-unpitched 37 → 36)、10ch
        let dr = &s.parts[2];
        assert!(dr.is_drum());
        assert_eq!(dr.notes[0].2, 36);
        assert!(r.notes.iter().any(|n| n.contains("装飾音符")));
    }

    #[test]
    fn non_musicxml_is_rejected() {
        assert!(parse("<html/>").is_err());
        assert!(parse("not xml").is_err());
    }

    #[test]
    fn a_doctype_and_an_mxl_zip_are_read() {
        let with_dtd = XML.replacen(
            "<score-partwise",
            "<!DOCTYPE score-partwise PUBLIC \"-//Recordare//DTD MusicXML 4.0 Partwise//EN\" \"http://www.musicxml.org/dtds/partwise.dtd\">\n<score-partwise",
            1,
        );
        assert_eq!(parse(&with_dtd).unwrap().song.parts.len(), 3);
        // .mxl: META-INF/container.xml が中の楽譜を指す
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("META-INF/container.xml", o).unwrap();
            std::io::Write::write_all(
                &mut w,
                br#"<container><rootfiles><rootfile full-path="score/song.musicxml"/></rootfiles></container>"#,
            )
            .unwrap();
            w.start_file("score/song.musicxml", o).unwrap();
            std::io::Write::write_all(&mut w, with_dtd.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        let text = read_text(buf.get_ref()).unwrap();
        assert_eq!(parse(&text).unwrap().song.parts[0].name, "Flute");
    }

    #[test]
    fn what_glaux_writes_comes_back() {
        use glaux_core::{Clip, ClipId, Note, NoteId, Project, Track, TrackId, TrackKind};
        let mut p = Project::new("往復");
        let mut t = Track::new(TrackId::new(), "Lead", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 2));
        let seq = [
            (0u64, 960u64, 72u8),
            (960, 480, 74),
            (1440, 480, 76),
            (1920, 1920, 79),
            (3840, 960, 77),
            (4800, 2880, 76),
        ];
        if let Some(ns) = c.notes_mut() {
            for (pos, dur, pitch) in seq {
                ns.push(Note {
                    locked: false,
                    id: NoteId::new(),
                    pos: Tick(pos),
                    dur: Tick(dur),
                    pitch,
                    vel: 90,
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                    condition: None,
                });
            }
        }
        t.clips.push(c);
        p.tracks.push(t);
        let (xml, _) = crate::musicxml::to_musicxml(&p).unwrap();
        let r = parse(&xml).unwrap();
        let back: Vec<(u64, u64, u8)> = r.song.parts[0]
            .notes
            .iter()
            .map(|n| (n.0, n.1, n.2))
            .collect();
        assert_eq!(back, seq.to_vec());
    }

    #[test]
    fn a_two_staff_part_without_a_name_becomes_two_tracks() {
        let xml = r#"<score-partwise version="4.0">
  <part-list><score-part id="P1"><part-name print-object="no"></part-name>
    <midi-instrument id="P1-I1"><midi-program>1</midi-program></midi-instrument></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>1</divisions><staves>2</staves></attributes>
      <note><pitch><step>E</step><octave>5</octave></pitch><duration>2</duration><tie type="start"/><staff>1</staff></note>
      <note><pitch><step>E</step><octave>5</octave></pitch><duration>2</duration><tie type="stop"/><staff>1</staff></note>
      <backup><duration>4</duration></backup>
      <note><pitch><step>C</step><octave>3</octave></pitch><duration>4</duration><staff>2</staff></note>
      <note><chord/><pitch><step>G</step><octave>3</octave></pitch><duration>4</duration><staff>2</staff></note>
    </measure>
  </part>
</score-partwise>"#;
        let r = parse(xml).unwrap();
        let names: Vec<&str> = r.song.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Acoustic Grand Piano(上段)", "Acoustic Grand Piano(下段)"]
        );
        // 上段はタイで 1 音、下段は和音
        assert_eq!(r.song.parts[0].notes, vec![(0, 3840, 76, 80)]);
        assert_eq!(r.song.parts[1].notes.len(), 2);
    }

    /// 1 声の楽譜を partwise で組む(小節の中身の並び)
    fn one_part(measures: &[&str]) -> String {
        let body: String = measures
            .iter()
            .enumerate()
            .map(|(i, m)| format!("<measure number=\"{}\">{m}</measure>", i + 1))
            .collect();
        format!(
            r#"<score-partwise><part-list><score-part id="P1"><part-name>A</part-name></score-part></part-list>
            <part id="P1">{body}</part></score-partwise>"#
        )
    }

    fn whole(step: &str) -> String {
        format!("<note><pitch><step>{step}</step><octave>4</octave></pitch><duration>4</duration></note>")
    }

    fn starts_and_pitches(r: &ScoreRead) -> Vec<(u64, u8)> {
        r.song.parts[0].notes.iter().map(|n| (n.0, n.2)).collect()
    }

    #[test]
    fn repeats_and_endings_are_played_in_order() {
        // |: C | D [1. E :| [2. F | G
        let attrs = "<attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>";
        let xml = one_part(&[
            &format!("{attrs}<barline location=\"left\"><repeat direction=\"forward\"/></barline>{}", whole("C")),
            &whole("D"),
            &format!("<barline location=\"left\"><ending number=\"1\" type=\"start\"/></barline>{}<barline location=\"right\"><ending number=\"1\" type=\"stop\"/><repeat direction=\"backward\"/></barline>", whole("E")),
            &format!("<barline location=\"left\"><ending number=\"2\" type=\"start\"/></barline>{}<barline location=\"right\"><ending number=\"2\" type=\"discontinue\"/></barline>", whole("F")),
            &whole("G"),
        ]);
        let r = parse(&xml).unwrap();
        let pitches: Vec<u8> = starts_and_pitches(&r).iter().map(|x| x.1).collect();
        // C D E C D F G
        assert_eq!(pitches, vec![60, 62, 64, 60, 62, 65, 67]);
        assert_eq!(r.song.parts[0].notes[6].0, 6 * 3840);
        assert!(
            r.notes.iter().any(|n| n.contains("5 小節 → 7 小節")),
            "{:?}",
            r.notes
        );
        // 展開しない指定なら書かれた順
        let r = parse_with(
            &xml,
            ParseOptions {
                expand_repeats: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(r.song.parts[0].notes.len(), 5);
        assert!(r.notes.iter().any(|n| n.contains("展開していません")));
    }

    #[test]
    fn times_da_capo_al_fine_and_restored_meter() {
        // |: C :|x3  D(3/4) Fine の印は C、D の後で D.C.
        let xml = one_part(&[
            "<attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes><direction><sound tempo=\"90\"/></direction><note><pitch><step>C</step><octave>4</octave></pitch><duration>4</duration></note><barline location=\"right\"><repeat direction=\"backward\" times=\"3\"/></barline>",
            "<attributes><time><beats>3</beats><beat-type>4</beat-type></time></attributes><direction><sound tempo=\"140\" fine=\"yes\"/></direction><note><pitch><step>D</step><octave>4</octave></pitch><duration>3</duration></note>",
            "<note><pitch><step>E</step><octave>4</octave></pitch><duration>3</duration></note><direction><sound dacapo=\"yes\"/></direction>",
        ]);
        let r = parse(&xml).unwrap();
        // C×3、D、E、(D.C.)C(繰り返しは 1 回だけ)、D で Fine
        let got = starts_and_pitches(&r);
        let pitches: Vec<u8> = got.iter().map(|x| x.1).collect();
        assert_eq!(pitches, vec![60, 60, 60, 62, 64, 60, 62]);
        // D.C. で戻った C は 4/4・テンポ 90 に戻り、次の D で 3/4・140
        let back = got[5].0;
        assert_eq!(back, 3 * 3840 + 2 * 2880);
        let sig_at = |t: u64| {
            r.song
                .sigs
                .iter()
                .rev()
                .find(|s| s.tick.0 <= t)
                .unwrap()
                .num
        };
        let bpm_at = |t: u64| {
            r.song
                .tempos
                .iter()
                .rev()
                .find(|x| x.tick.0 <= t)
                .unwrap()
                .bpm
        };
        assert_eq!((sig_at(back), bpm_at(back)), (4, 90.0));
        assert_eq!((sig_at(got[6].0), bpm_at(got[6].0)), (3, 140.0));
    }

    #[test]
    fn timewise_is_regrouped_into_parts() {
        let xml = r#"<score-timewise>
  <part-list><score-part id="P1"><part-name>Up</part-name></score-part><score-part id="P2"><part-name>Down</part-name></score-part></part-list>
  <measure number="1">
    <part id="P1"><attributes><divisions>1</divisions></attributes><note><pitch><step>E</step><octave>5</octave></pitch><duration>4</duration></note></part>
    <part id="P2"><attributes><divisions>1</divisions></attributes><note><pitch><step>C</step><octave>3</octave></pitch><duration>4</duration></note></part>
  </measure>
  <measure number="2">
    <part id="P1"><note><pitch><step>F</step><octave>5</octave></pitch><duration>4</duration></note></part>
    <part id="P2"><note><pitch><step>D</step><octave>3</octave></pitch><duration>4</duration></note></part>
  </measure>
</score-timewise>"#;
        let r = parse(xml).unwrap();
        let names: Vec<&str> = r.song.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Up", "Down"]);
        assert_eq!(starts_and_pitches(&r), vec![(0, 76), (3840, 77)]);
        assert_eq!(
            r.song.parts[1]
                .notes
                .iter()
                .map(|n| (n.0, n.2))
                .collect::<Vec<_>>(),
            vec![(0, 48), (3840, 50)]
        );
    }

    #[test]
    fn hairpins_move_the_velocity_toward_the_next_dynamic() {
        let q = |s: &str| {
            format!("<note><pitch><step>{s}</step><octave>4</octave></pitch><duration>1</duration></note>")
        };
        let xml = one_part(&[
            &format!(
                "<attributes><divisions>1</divisions></attributes><direction><direction-type><dynamics><p/></dynamics></direction-type></direction>\
                 <direction><direction-type><wedge type=\"crescendo\"/></direction-type></direction>{}{}{}{}",
                q("C"), q("D"), q("E"), q("F")
            ),
            &format!(
                "<direction><direction-type><wedge type=\"stop\"/></direction-type></direction><direction><direction-type><dynamics><f/></dynamics></direction-type></direction>{}\
                 <direction><direction-type><wedge type=\"diminuendo\"/></direction-type></direction>{}{}<direction><direction-type><wedge type=\"stop\"/></direction-type></direction>{}",
                q("G"), q("A"), q("B"), q("C")
            ),
        ]);
        let r = parse(&xml).unwrap();
        let vels: Vec<u8> = r.song.parts[0].notes.iter().map(|n| n.3).collect();
        // p(55)から f(96)へ上がる。f の後のディミヌエンドは目標の記号が無いので 1 段(16)下げ、その後も下げたまま
        assert_eq!(vels[0], 55);
        assert!(
            vels[0] < vels[1] && vels[1] < vels[2] && vels[2] < vels[3] && vels[3] < 96,
            "{vels:?}"
        );
        assert_eq!(vels[4], 96);
        assert!(vels[5] == 96 && vels[6] < vels[5], "{vels:?}");
        assert_eq!(vels[7], 80);
    }

    #[test]
    fn voices_can_be_split_and_acciaccaturas_are_short() {
        let xml = r#"<score-partwise><part-list><score-part id="P1"><part-name>Choir</part-name></score-part></part-list>
  <part id="P1"><measure number="1">
    <attributes><divisions>2</divisions></attributes>
    <note><grace slash="yes"/><pitch><step>B</step><octave>4</octave></pitch><voice>1</voice></note>
    <note><pitch><step>C</step><octave>5</octave></pitch><duration>8</duration><voice>1</voice></note>
    <backup><duration>8</duration></backup>
    <note><pitch><step>E</step><octave>4</octave></pitch><duration>8</duration><voice>2</voice></note>
  </measure></part></score-partwise>"#;
        // 既定は 1 トラック。斜線付きの装飾音符は 32 分(120 tick)で、主音符はその後ろから
        let r = parse(xml).unwrap();
        assert_eq!(r.song.parts.len(), 1);
        let n = &r.song.parts[0].notes;
        assert!(n.contains(&(0, 120, 71, 72)), "{n:?}");
        assert!(n.contains(&(120, 3720, 72, 80)), "{n:?}");
        assert!(n.contains(&(0, 3840, 64, 80)), "{n:?}");
        // 声部を分ける
        let r = parse_with(
            xml,
            ParseOptions {
                split_voices: true,
                ..Default::default()
            },
        )
        .unwrap();
        let names: Vec<&str> = r.song.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Choir 声部1", "Choir 声部2"]);
        assert_eq!(r.song.parts[1].notes, vec![(0, 3840, 64, 80)]);
    }

    #[test]
    fn a_repeat_without_a_start_goes_back_to_the_double_bar() {
        // A | B ‖ C :| D  → A B C B C D(複縦線の後ろから繰り返す)
        let xml = one_part(&[
            &format!("<attributes><divisions>1</divisions></attributes>{}", whole("A")),
            &format!("{}<barline location=\"right\"><bar-style>light-light</bar-style></barline>", whole("B")),
            &whole("C"),
            &format!("{}<barline location=\"right\"><bar-style>light-heavy</bar-style><repeat direction=\"backward\"/></barline>", whole("D")),
            &whole("E"),
        ]);
        let r = parse(&xml).unwrap();
        let pitches: Vec<u8> = starts_and_pitches(&r).iter().map(|x| x.1).collect();
        assert_eq!(pitches, vec![69, 71, 60, 62, 60, 62, 64]);
    }

    #[test]
    fn utf16_and_latin1_files_are_read() {
        let xml = one_part(&["<attributes><divisions>1</divisions></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>4</duration></note>"])
            .replace("<part-name>A</part-name>", "<part-name>Flûte</part-name>");
        let decl = "<?xml version=\"1.0\" encoding=\"UTF-16\"?>";
        let mut le = vec![0xff, 0xfe];
        for u in format!("{decl}{xml}").encode_utf16() {
            le.extend(u.to_le_bytes());
        }
        let r = parse(&read_text(&le).unwrap()).unwrap();
        assert_eq!(r.song.parts[0].name, "Flûte");
        // Latin-1(û = 0xFB)
        let latin: Vec<u8> = format!("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>{xml}")
            .chars()
            .map(|c| c as u32 as u8)
            .collect();
        let r = parse(&read_text(&latin).unwrap()).unwrap();
        assert_eq!(r.song.parts[0].name, "Flûte");
    }
}
