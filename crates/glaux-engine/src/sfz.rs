//! SFZ(テキストで書くサンプル音源の定義)の読み込みとゾーン構築。
//!
//! パーサは自前(SFZ はテキストの「見出し + opcode=値」の並び)。組み立てたゾーンは SoundFont と
//! 同じ `glaux_dsp::Zone` に落とし、再生は glaux-dsp のマルチサンプラーが受け持つ。
//!
//! .sfz とサンプルは曲プロジェクトにコピーせず、**全プロジェクト共通のライブラリフォルダ**
//! (`<設定ディレクトリ>/glaux/sfz/`)からの相対パスで参照する(SoundFont と同じ考え方)。
//!
//! 対応している opcode(ほかは無視する):
//! - 見出し: `<control>` `<global>` `<master>` `<group>` `<region>`、`#define` `#include`、`default_path`
//! - 範囲: `key` `lokey` `hikey`(音名 c4 = 60 も可)`lovel` `hivel`
//! - 音程: `pitch_keycenter` `pitch_keytrack` `transpose` `tune`
//! - 音量: `volume` `amplitude`、`ampeg_attack` `ampeg_hold` `ampeg_decay` `ampeg_sustain` `ampeg_release`
//! - 波形: `sample` `offset` `end` `loop_mode` `loop_start` `loop_end`(WAV の smpl のループ点も読む)
//! - 選び方: `seq_length` `seq_position`(ラウンドロビン)`lorand` `hirand`
//! - 止め方: `group` `off_by`(チョーク)、`loop_mode=one_shot`
//! - フィルタ: `cutoff` `resonance`(ローパスのみ)
//! - 読み飛ばす region: `trigger=release` / `legato`、CC の条件(`locc` が 0 より上)、既定以外のキースイッチ

use glaux_dsp::{SampleData, Zone, ZoneEnv, ZoneMod, ZonePlay};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 既定の SFZ ライブラリフォルダ(SoundFont のフォルダの隣)。
pub fn default_dir() -> PathBuf {
    crate::sf2::default_dir().with_file_name("sfz")
}

/// 楽器の名前として使えるか。ライブラリフォルダからの相対パス(`..` で外へ出ない)か、
/// 別の場所に置いた大きな音源のための絶対パス。拡張子は .sfz
pub fn valid_name(name: &str) -> bool {
    let p = Path::new(name);
    // 相対の名前に ":" があると Windows ではドライブの指定(C:x.sfz)になりライブラリの外を指す
    !name.is_empty()
        && (p.is_absolute() || (!name.split(['/', '\\']).any(|c| c == "..") && !name.contains(':')))
        && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sfz"))
}

/// ライブラリフォルダ内の .sfz(サブフォルダも 4 段まで)。区切りは `/`、名前順。
pub fn list_files(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, depth: u32, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.filter_map(|e| e.ok()) {
            if out.len() >= 2_000 {
                return;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            if path.is_dir() {
                if depth < 4 {
                    walk(&path, &format!("{prefix}{name}/"), depth + 1, out);
                }
            } else if path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("sfz"))
            {
                out.push(format!("{prefix}{name}"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", 0, &mut out);
    out.sort();
    out
}

/// 1 つの region(上位の見出しの opcode を合わせたもの)
pub type Region = HashMap<String, String>;

/// `#include` で読む中身を返す(`path` は .sfz のあるフォルダからの相対)
pub type IncludeFn<'a> = dyn FnMut(&str) -> Result<String, String> + 'a;

/// SFZ の本文を region の列にする。
pub fn parse(text: &str, include: &mut IncludeFn) -> Result<Vec<Region>, String> {
    parse_with_control(text, include).map(|(r, _)| r)
}

/// 音源の調整つまみ(`<control>` の label_ccN と set_ccN)。CC 番号・名前・既定値
#[derive(Clone, Debug, serde::Serialize)]
pub struct Control {
    pub cc: u8,
    pub label: String,
    pub default: u8,
}

/// [`parse`] の、`<control>` の opcode も返す版
pub fn parse_with_control(
    text: &str,
    include: &mut IncludeFn,
) -> Result<(Vec<Region>, Region), String> {
    let mut text_out = String::new();
    let mut defines: Vec<(String, String)> = Vec::new();
    preprocess(text, include, &mut defines, 0, &mut text_out)?;

    #[derive(PartialEq)]
    enum Level {
        Control,
        Global,
        Master,
        Group,
        Region,
        Curve,
        Ignore,
    }
    let mut level = Level::Ignore;
    let mut control = Region::new();
    let mut global = Region::new();
    let mut master = Region::new();
    let mut group = Region::new();
    let mut region: Option<Region> = None;
    // <curve> の定義(curve_index と vNNN)。region の CC の評価に使う
    let mut curves: Vec<Region> = Vec::new();
    let mut out = Vec::new();

    let finish = |region: &mut Option<Region>,
                  out: &mut Vec<Region>,
                  control: &Region,
                  layers: [&Region; 3]| {
        if let Some(r) = region.take() {
            let mut merged = Region::new();
            // default_path と、CC の既定値(set_cc / set_hdcc。CC の条件を判定するのに使う)
            for (k, v) in control {
                if k == "default_path" || k.starts_with("set_cc") || k.starts_with("set_hdcc") {
                    merged.insert(k.clone(), v.clone());
                }
            }
            for l in layers {
                merged.extend(l.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            merged.extend(r);
            out.push(merged);
        }
    };

    for line in text_out.lines() {
        for tok in tokenize(line) {
            match tok {
                Tok::Header(h) => {
                    finish(&mut region, &mut out, &control, [&global, &master, &group]);
                    level = match h.as_str() {
                        "control" => Level::Control,
                        "global" => {
                            global.clear();
                            master.clear();
                            group.clear();
                            Level::Global
                        }
                        "master" => {
                            master.clear();
                            group.clear();
                            Level::Master
                        }
                        "group" => {
                            group.clear();
                            Level::Group
                        }
                        "region" => {
                            region = Some(Region::new());
                            Level::Region
                        }
                        "curve" => {
                            curves.push(Region::new());
                            Level::Curve
                        }
                        _ => Level::Ignore,
                    };
                }
                Tok::Op(k, v) => {
                    let map = match level {
                        Level::Control => &mut control,
                        Level::Global => &mut global,
                        Level::Master => &mut master,
                        Level::Group => &mut group,
                        Level::Region => match region.as_mut() {
                            Some(r) => r,
                            None => continue,
                        },
                        Level::Curve => match curves.last_mut() {
                            Some(c) => c,
                            None => continue,
                        },
                        Level::Ignore => continue,
                    };
                    map.insert(k, v);
                }
            }
        }
    }
    finish(&mut region, &mut out, &control, [&global, &master, &group]);
    // 使っているカーブの点を region に写す(カーブは region より後に書かれることが多い)
    let curves: HashMap<i64, String> = curves
        .iter()
        .filter_map(|c| {
            let idx = num(c, "curve_index")? as i64;
            let mut pts: Vec<(u32, f64)> = c
                .iter()
                .filter_map(|(k, v)| {
                    let i: u32 = k.strip_prefix('v')?.parse().ok()?;
                    Some((i.min(127), v.trim().parse().ok()?))
                })
                .collect();
            pts.sort_by_key(|p| p.0);
            let text = pts
                .iter()
                .map(|(i, v)| format!("{i}:{v}"))
                .collect::<Vec<_>>()
                .join(",");
            Some((idx, text))
        })
        .collect();
    if !curves.is_empty() {
        for r in out.iter_mut() {
            let used: Vec<i64> = r
                .iter()
                .filter(|(k, _)| k.contains("_curvecc"))
                .filter_map(|(_, v)| v.trim().parse::<f64>().ok().map(|x| x as i64))
                .collect();
            for i in used {
                if let Some(t) = curves.get(&i) {
                    r.insert(format!("#curve{i}"), t.clone());
                }
            }
        }
    }
    Ok((out, control))
}

/// コメントを除き、`#define` を置き換え、`#include` を展開する。どちらも行の途中に書いてよい
/// (`<region> #define $KEY 21 lokey=21 #include "Data/sample.txt"` のように region ごとに定義し直す音源がある)。
/// `#define` の値は次の空白まで。取り込んだ中身は前後で改行を入れてつなぐ(値が混ざらないように)
fn preprocess(
    text: &str,
    include: &mut IncludeFn,
    defines: &mut Vec<(String, String)>,
    depth: u32,
    out: &mut String,
) -> Result<(), String> {
    if depth > 8 {
        return Err("#include が深すぎます(循環していませんか)".to_owned());
    }
    let text = strip_comments(text);
    let mut rest = text.as_str();
    while let Some(i) = rest.find('#') {
        let after = &rest[i + 1..];
        let (is_define, is_include) = (after.starts_with("define"), after.starts_with("include"));
        if !is_define && !is_include {
            // 音名(c#4)やファイル名の # はそのまま
            out.push_str(&substitute(&rest[..i + 1], defines));
            rest = after;
            continue;
        }
        out.push_str(&substitute(&rest[..i], defines));
        if is_define {
            let (name, r) = next_word(&after["define".len()..]);
            let (value, r) = next_word(r);
            if !name.is_empty() {
                defines.retain(|(n, _)| n != name);
                defines.push((name.to_owned(), value.to_owned()));
                // 長い名前から置き換える($A と $AB の取り違えを防ぐ)
                defines.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
            }
            rest = r;
        } else {
            let r = after["include".len()..].trim_start_matches([' ', '\t']);
            let (name, r) = match r.strip_prefix('"') {
                Some(q) => match q.find('"') {
                    Some(j) => (&q[..j], &q[j + 1..]),
                    None => (q.lines().next().unwrap_or(""), ""),
                },
                None => next_word(r),
            };
            let name = substitute(name, defines).replace('\\', "/");
            let body = include(&name)?;
            out.push('\n');
            preprocess(&body, include, defines, depth + 1, out)?;
            out.push('\n');
            rest = r;
        }
    }
    out.push_str(&substitute(rest, defines));
    Ok(())
}

/// 空白を飛ばして次の語と、その後ろを返す
fn next_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// `// 行コメント` と `/* ブロックコメント */` を除く(`//****` のような行の中の `/*` は行コメントの一部)
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let line = rest.find("//");
        let block = rest.find("/*");
        match (line, block) {
            (Some(l), b) if b.is_none_or(|b| l <= b) => {
                out.push_str(&rest[..l]);
                rest = match rest[l..].find('\n') {
                    Some(n) => &rest[l + n..],
                    None => "",
                };
            }
            (_, Some(b)) => {
                out.push_str(&rest[..b]);
                rest = match rest[b + 2..].find("*/") {
                    Some(j) => &rest[b + 2 + j + 2..],
                    None => "",
                };
                // ブロックコメントは空白 1 つに(前後の opcode がつながらないように)
                out.push(' ');
            }
            _ => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

fn substitute(line: &str, defines: &[(String, String)]) -> String {
    if !line.contains('$') {
        return line.to_owned();
    }
    let mut s = line.to_owned();
    for (n, v) in defines {
        if s.contains(n.as_str()) {
            s = s.replace(n.as_str(), v);
        }
    }
    s
}

enum Tok {
    Header(String),
    Op(String, String),
}

/// 1 行を見出しと opcode に分ける。値は空白を含んでよい(次の `名前=` か見出しまで)
fn tokenize(line: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut rest = line;
    loop {
        let (seg, header) = match rest.find('<') {
            Some(i) => match rest[i..].find('>') {
                Some(j) => (&rest[..i], Some((&rest[i + 1..i + j], &rest[i + j + 1..]))),
                None => (rest, None),
            },
            None => (rest, None),
        };
        ops(seg, &mut toks);
        match header {
            Some((h, after)) => {
                toks.push(Tok::Header(h.trim().to_ascii_lowercase()));
                rest = after;
            }
            None => break,
        }
    }
    toks
}

fn ops(seg: &str, toks: &mut Vec<Tok>) {
    for word in seg.split_whitespace() {
        let op = word.split_once('=').filter(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        match op {
            Some((k, v)) => toks.push(Tok::Op(k.to_ascii_lowercase(), v.to_owned())),
            None => {
                // 値の続き(ファイル名の空白など)
                if let Some(Tok::Op(_, v)) = toks.last_mut() {
                    if !v.is_empty() {
                        v.push(' ');
                    }
                    v.push_str(word);
                }
            }
        }
    }
}

/// 音名(c4 = 60、c#4、db4、c-1 = 0)または数字を MIDI ノート番号にする。
pub fn parse_note(s: &str) -> Option<i32> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }
    let mut chars = s.chars();
    let pc = match chars.next()?.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (acc, oct) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b').filter(|r| !r.is_empty()) {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    // 壊れた値(c999999999)でも溢れないように、鍵盤のあるオクターブだけ
    let oct: i32 = oct.parse().ok().filter(|o| (-2..=10).contains(o))?;
    Some((oct + 1) * 12 + pc + acc)
}

fn num(r: &Region, k: &str) -> Option<f64> {
    r.get(k).and_then(|v| v.trim().parse::<f64>().ok())
}

fn note(r: &Region, k: &str) -> Option<i32> {
    r.get(k).and_then(|v| parse_note(v))
}

/// 読んだ音声ファイルの使い回し(パス → 波形とループ点)。ゾーンはファイル丸ごとを共有し、範囲で切り分ける。
/// 楽器が使っている間は残す([`retain_used`])ので、調整つまみを変えて組み直すときも読み直さない
pub type WaveCache = HashMap<PathBuf, (Arc<SampleData>, Option<(usize, usize)>)>;

/// どの楽器(ゾーン)からも使われなくなったファイルを捨てる
pub fn retain_used(cache: &mut WaveCache) {
    cache.retain(|_, (d, _)| Arc::strong_count(d) > 1);
}

/// 読んだ波形と、そのループ点(ファイルに書かれていれば)
pub struct Loaded {
    pub data: Arc<SampleData>,
    pub loop_points: Option<(usize, usize)>,
}

/// キースイッチの既定(sw_default、無ければ最も低い sw_last)
fn default_switch(regions: &[Region]) -> Option<i32> {
    regions
        .iter()
        .find_map(|r| note(r, "sw_default"))
        .or_else(|| regions.iter().filter_map(|r| note(r, "sw_last")).min())
}

/// 普通にノートを弾いたときに鳴る region か(release・CC の条件付き・既定以外のキースイッチは鳴らない)
fn region_plays(r: &Region, sw_default: Option<i32>) -> bool {
    if matches!(
        r.get("trigger").map(|s| s.as_str()),
        Some("release" | "release_key" | "legato")
    ) {
        return false;
    }
    // CC で鳴らす region は弾いても鳴らない。CC の範囲の条件は、CC の既定値で満たすものだけ
    for (k, v) in r {
        if k.starts_with("on_locc") || k.starts_with("on_hicc") {
            return false;
        }
        let (lo, n) = match (k.strip_prefix("locc"), k.strip_prefix("hicc")) {
            (Some(n), _) => (true, n),
            (_, Some(n)) => (false, n),
            _ => continue,
        };
        let (Ok(n), Ok(x)) = (n.parse::<u32>(), v.trim().parse::<f64>()) else {
            continue;
        };
        let cc = cc_default(r, n);
        if (lo && cc < x) || (!lo && cc > x) {
            return false;
        }
    }
    match (note(r, "sw_last"), sw_default) {
        (Some(sw), Some(def)) => sw == def,
        _ => true,
    }
}

/// CC の既定値(0〜127)。`<control>` の set_cc / set_hdcc、無ければ音量 100・パン 64・エクスプレッション 127・ほかは 0
fn cc_default(r: &Region, n: u32) -> f64 {
    if let Some(v) = num(r, &format!("set_cc{n}")) {
        return v;
    }
    if let Some(v) = num(r, &format!("set_hdcc{n}")) {
        return v * 127.0;
    }
    match n {
        7 => 100.0,
        10 => 64.0,
        11 => 127.0,
        _ => 0.0,
    }
}

/// opcode の値に、CC で足す分(`<name>_onccN` × カーブを通した CC の既定値)を足したもの
fn val(r: &Region, name: &str, default: f64) -> f64 {
    num(r, name).unwrap_or(default) + by_cc(r, name, |v, x| v * x)
}

/// カーブ `idx` に CC の値(0〜127)を通す。`<curve>` の定義(点の間は直線、v000 の既定 0・v127 の既定 1)か、
/// 決まったカーブ(0 = 0〜1、1 = −1〜1、2 = 1〜0、3 = 1〜−1、4 = 2 乗、5 = 平方根)。知らない番号は 0 番
fn curve_value(r: &Region, idx: i64, cc: f64) -> f64 {
    let x = (cc / 127.0).clamp(0.0, 1.0);
    if let Some(text) = r.get(&format!("#curve{idx}")) {
        let mut pts: Vec<(f64, f64)> = text
            .split(',')
            .filter_map(|p| {
                let (i, v) = p.split_once(':')?;
                Some((i.parse::<f64>().ok()?, v.parse::<f64>().ok()?))
            })
            .collect();
        if pts.first().is_none_or(|p| p.0 > 0.0) {
            pts.insert(0, (0.0, 0.0));
        }
        if pts.last().is_none_or(|p| p.0 < 127.0) {
            pts.push((127.0, 1.0));
        }
        let c = cc.clamp(0.0, 127.0);
        let i = pts
            .iter()
            .position(|p| p.0 >= c)
            .unwrap_or(pts.len() - 1)
            .max(1);
        let (a, b) = (pts[i - 1], pts[i]);
        return if b.0 > a.0 {
            a.1 + (b.1 - a.1) * (c - a.0) / (b.0 - a.0)
        } else {
            b.1
        };
    }
    match idx {
        1 => x * 2.0 - 1.0,
        2 => 1.0 - x,
        3 => 1.0 - x * 2.0,
        4 => x * x,
        5 => x.sqrt(),
        _ => x,
    }
}

/// `<name>_ccN` / `<name>_onccN` を CC の既定値で評価した和(`f(値, カーブを通した CC)`)
fn by_cc(r: &Region, name: &str, f: impl Fn(f64, f64) -> f64) -> f64 {
    cc_terms(r, name).map(|(v, cc)| f(v, cc)).sum()
}

/// `<name>_ccN` / `<name>_onccN` を CC の既定値で評価した積(無ければ 1)
fn by_cc_product(r: &Region, name: &str, f: impl Fn(f64, f64) -> f64) -> f64 {
    cc_terms(r, name).map(|(v, cc)| f(v, cc)).product()
}

fn cc_terms<'a>(r: &'a Region, name: &'a str) -> impl Iterator<Item = (f64, f64)> + 'a {
    r.iter().filter_map(move |(k, v)| {
        let rest = k.strip_prefix(name)?;
        // 書き方は _onccN / _ccN / ccN(SFZ 1 の ampeg_releasecc64 など)
        let n = rest
            .strip_prefix("_oncc")
            .or_else(|| rest.strip_prefix("_cc"))
            .or_else(|| rest.strip_prefix("cc"))?;
        let n: u32 = n.parse().ok()?;
        let curve = num(r, &format!("{name}_curvecc{n}")).map_or(0, |c| c as i64);
        Some((
            v.trim().parse::<f64>().ok()?,
            curve_value(r, curve, cc_default(r, n)),
        ))
    })
}

/// region の波形の、.sfz のあるフォルダからの相対パス(区切りは `/`。*sine などの内蔵の発音器は None)
fn sample_path(r: &Region) -> Option<String> {
    let sample = r.get("sample")?.trim();
    if sample.starts_with('*') {
        return None;
    }
    let dp = r.get("default_path").map(|d| d.trim()).unwrap_or("");
    Some(format!("{dp}{sample}").replace('\\', "/"))
}

/// 鳴る region が使う波形(.sfz のあるフォルダからの相対パス、重複なし、出てきた順)。音源の取得で使う
pub fn used_samples(regions: &[Region]) -> Vec<String> {
    let sw_default = default_switch(regions);
    let mut seen = std::collections::HashSet::new();
    regions
        .iter()
        .filter(|r| region_plays(r, sw_default))
        .filter_map(sample_path)
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// region の列をゾーンにする。`dir` は .sfz のあるフォルダ、`load` は波形の読み込み。
/// 波形を 1 つも読めなければ Err。
pub fn build_zones(
    regions: &[Region],
    dir: &Path,
    cache: &mut WaveCache,
    load: &mut dyn FnMut(&Path) -> Result<Loaded, String>,
) -> Result<Vec<Zone>, String> {
    let sw_default = default_switch(regions);
    // 読めなかったファイル(同じファイルを何度も試さない)
    let mut failed: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut zones = Vec::new();
    let mut errors = Vec::new();
    // *silence の波形(10ms の無音)
    let silent = Arc::new(SampleData::mono(vec![0.0; 480], 48_000.0));

    for r in regions {
        if !region_plays(r, sw_default) {
            continue;
        }
        // *silence は音の出ない短いゾーンにする(ハイハットのチョークを無音の region で組む音源がある)
        let silence = r
            .get("sample")
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("*silence"));
        let (data, file_loop, off, end) = if silence {
            let d = silent.clone();
            let n = d.frames.len();
            (d, None, 0usize, n)
        } else {
            let Some(rel) = sample_path(r) else {
                continue;
            };
            let path = dir.join(&rel);
            if !cache.contains_key(&path) && !failed.contains(&path) {
                match load(&path) {
                    Ok(l) => {
                        cache.insert(path.clone(), (l.data, l.loop_points));
                    }
                    Err(e) => {
                        errors.push(e);
                        failed.insert(path.clone());
                    }
                }
            }
            let Some((data, loop_points)) = cache.get(&path) else {
                continue;
            };
            let len = data.frames.len();
            // 壊れた値(1e30 など)でも溢れないように、波形の長さに収めてから整数にする
            let off = (num(r, "offset").unwrap_or(0.0) + by_cc(r, "offset", |v, x| v * x))
                .clamp(0.0, len as f64) as usize;
            let end = num(r, "end").map_or(len, |e| {
                (e.clamp(0.0, len as f64) as usize)
                    .saturating_add(1)
                    .min(len)
            });
            if end <= off.saturating_add(1) {
                continue;
            }
            (data.clone(), *loop_points, off, end)
        };

        let key = note(r, "key");
        let lo = note(r, "lokey").or(key).unwrap_or(0).clamp(0, 127);
        let hi = note(r, "hikey").or(key).unwrap_or(127).clamp(0, 127);
        let center = note(r, "pitch_keycenter").or(key).unwrap_or(60);
        let vlo = num(r, "lovel").unwrap_or(1.0).clamp(0.0, 127.0);
        let vhi = num(r, "hivel").unwrap_or(127.0).clamp(0.0, 127.0);
        if lo > hi || vlo > vhi {
            continue;
        }

        // ループ: 指定が無ければファイルのループ点があるときだけループする
        let points = match (num(r, "loop_start"), num(r, "loop_end")) {
            (Some(s), Some(e)) => Some((s.max(0.0) as usize, e.max(0.0) as usize)),
            _ => file_loop,
        };
        let mode = r
            .get("loop_mode")
            .or_else(|| r.get("loopmode"))
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| {
                if points.is_some() {
                    "loop_continuous".to_owned()
                } else {
                    "no_loop".to_owned()
                }
            });
        // ループ点は波形の頭からの位置のまま(ゾーンは波形を切り出さず、範囲 off..end で鳴らす)
        let loop_range = match (mode.as_str(), points) {
            ("loop_continuous" | "loop_sustain", Some((s, e))) if s >= off && e < end && e > s => {
                Some((s as f64, (e + 1) as f64))
            }
            _ => None,
        };

        let volume = (val(r, "volume", 0.0) + num(r, "group_volume").unwrap_or(0.0))
            .clamp(-144.0, 24.0) as f32;
        // amplitude_ccN は CC の値に比例して掛かる(既定値で評価。音量 cc7 の既定 100 なら約 -2dB)
        let amplitude = (num(r, "amplitude").unwrap_or(100.0).clamp(0.0, 100.0) / 100.0
            * by_cc_product(r, "amplitude", |v, x| v / 100.0 * x)) as f32;
        let env = ZoneEnv {
            attack: (val(r, "ampeg_attack", 0.0) as f32).clamp(0.001, 10.0),
            hold: (val(r, "ampeg_hold", 0.0) as f32).clamp(0.0, 10.0),
            decay: (val(r, "ampeg_decay", 0.0) as f32).clamp(0.005, 30.0),
            sustain: (val(r, "ampeg_sustain", 100.0) as f32 / 100.0).clamp(0.0, 1.0),
            release: (val(r, "ampeg_release", 0.001) as f32).clamp(0.01, 10.0),
        };
        let mut modu = ZoneMod::default();
        let lowpass = r
            .get("fil_type")
            .is_none_or(|t| t.trim().starts_with("lpf"));
        if let (Some(c), true) = (num(r, "cutoff"), lowpass) {
            modu.cutoff_hz = (c as f32).clamp(20.0, 20_000.0);
            modu.q_db = (num(r, "resonance").unwrap_or(0.0) as f32).clamp(0.0, 40.0);
        }
        let int = |k: &str| num(r, k).map(|v| v as i64);
        // ベロシティ曲線(amp_velcurve_N。最大 4 点、ベロシティ順)
        let mut points: Vec<(u8, f32)> = r
            .iter()
            .filter_map(|(k, v)| {
                let n: u8 = k.strip_prefix("amp_velcurve_")?.parse().ok()?;
                Some((n.min(127), v.trim().parse::<f32>().ok()?.clamp(0.0, 1.0)))
            })
            .collect();
        points.sort_by_key(|p| p.0);
        points.truncate(4);
        let mut velcurve = [(0u8, 0.0f32); 4];
        velcurve[..points.len()].copy_from_slice(&points);
        let velcurve_len = points.len() as u8;
        let play = ZonePlay {
            seq_length: int("seq_length").unwrap_or(1).clamp(1, 255) as u8,
            seq_position: int("seq_position").unwrap_or(1).clamp(1, 255) as u8,
            rand_lo: (num(r, "lorand").unwrap_or(0.0) as f32).clamp(0.0, 1.0),
            rand_hi: (num(r, "hirand").unwrap_or(1.0) as f32).clamp(0.0, 1.0),
            group: int("group").unwrap_or(0) as u32,
            off_by: int("off_by").unwrap_or(0) as u32,
            one_shot: mode == "one_shot",
            keytrack: (num(r, "pitch_keytrack").unwrap_or(100.0) / 100.0) as f32,
            tune_semis: (num(r, "transpose").unwrap_or(0.0)
                + (val(r, "tune", 0.0) + val(r, "pitch", 0.0)) / 100.0)
                as f32,
            veltrack: Some(
                (val(r, "amp_veltrack", 100.0) / 100.0)
                    .clamp(-1.0, 1.0)
                    .abs() as f32,
            ),
            velcurve,
            velcurve_len,
        };
        zones.push(Zone {
            key_lo: lo as u8,
            key_hi: hi as u8,
            vel_lo: vlo as u8,
            vel_hi: vhi as u8,
            start: off,
            end,
            data,
            loop_range,
            loop_until_release: mode == "loop_sustain",
            root: center as f32,
            gain: 10.0_f32.powf(volume / 20.0) * amplitude,
            env,
            modu,
            play,
        });
    }
    if zones.is_empty() {
        return Err(match errors.first() {
            Some(e) => format!("SFZ の波形を読めません: {e}"),
            None => "SFZ に鳴らせる region がありません".to_owned(),
        });
    }
    if !errors.is_empty() {
        tracing::warn!(
            "SFZ の波形のうち {} 個を読めませんでした(例: {})",
            errors.len(),
            errors[0]
        );
    }
    Ok(zones)
}

/// WAV の smpl チャンクから最初のループ点(開始, 終了。終了はそのサンプルを含む)を読む。
pub fn wav_loop_points(path: &Path) -> Option<(usize, usize)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return None;
    }
    let mut ch = [0u8; 8];
    for _ in 0..64 {
        f.read_exact(&mut ch).ok()?;
        let size = u32::from_le_bytes([ch[4], ch[5], ch[6], ch[7]]) as u64;
        if &ch[0..4] == b"smpl" {
            let mut body = vec![0u8; size.min(4096) as usize];
            f.read_exact(&mut body).ok()?;
            let u = |i: usize| {
                body.get(i..i + 4)
                    .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            };
            if u(28)? == 0 {
                return None;
            }
            return Some((u(44)?, u(48)?));
        }
        f.seek(SeekFrom::Current((size + (size & 1)) as i64)).ok()?;
    }
    None
}

/// ライブラリの .sfz を開いてゾーンを作る(`#include` と波形は .sfz のあるフォルダから)。
/// `cc` は調整つまみの上書き(CC 番号 → 0〜127。音源の set_cc の代わりに使う)
pub fn load_instrument(
    library: &Path,
    name: &str,
    cc: &BTreeMap<u8, u8>,
    cache: &mut WaveCache,
) -> Result<Arc<Vec<Zone>>, String> {
    let (dir, mut regions, _) = read_instrument(library, name)?;
    for r in regions.iter_mut() {
        for (n, v) in cc {
            r.remove(&format!("set_hdcc{n}"));
            r.insert(format!("set_cc{n}"), v.min(&127).to_string());
        }
    }
    // 使う波形のうち、まだ読んでいないものを先に並列で読む(大きな音源は数百ファイルある)
    let missing: Vec<PathBuf> = used_samples(&regions)
        .into_iter()
        .map(|rel| dir.join(rel))
        .filter(|p| !cache.contains_key(p))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    for (path, loaded) in load_parallel(&missing) {
        if let Ok(l) = loaded {
            cache.insert(path, (l.data, l.loop_points));
        }
    }
    let zones = build_zones(&regions, &dir, cache, &mut |p| {
        let mut data = crate::data::load_audio_file(p)?;
        // 左右差成分は鳴らさない(マルチサンプラーは中央成分だけを読む)ので持たない
        data.side = None;
        Ok(Loaded {
            data: Arc::new(data),
            loop_points: wav_loop_points(p),
        })
    })?;
    Ok(Arc::new(zones))
}

fn read_instrument(library: &Path, name: &str) -> Result<(PathBuf, Vec<Region>, Region), String> {
    if !valid_name(name) {
        return Err(format!("SFZ の名前が正しくありません: {name}"));
    }
    let path = library.join(name);
    let dir = path.parent().unwrap_or(library).to_path_buf();
    let text = read_text(&path)?;
    let (regions, control) = parse_with_control(&text, &mut |inc| read_text(&dir.join(inc)))?;
    Ok((dir, regions, control))
}

/// 音源の調整つまみの一覧(名前の付いた CC。CC 番号順)。波形は読まない
pub fn controls(library: &Path, name: &str) -> Result<Vec<Control>, String> {
    let (_, _, control) = read_instrument(library, name)?;
    let mut out: Vec<Control> = control
        .iter()
        .filter_map(|(k, v)| {
            let n: u8 = k.strip_prefix("label_cc")?.parse().ok()?;
            (n < 128).then(|| {
                let default = num(&control, &format!("set_cc{n}"))
                    .or_else(|| num(&control, &format!("set_hdcc{n}")).map(|x| x * 127.0))
                    .unwrap_or(match n {
                        7 => 100.0,
                        10 => 64.0,
                        11 => 127.0,
                        _ => 0.0,
                    });
                Control {
                    cc: n,
                    label: v.trim().to_owned(),
                    default: default.round().clamp(0.0, 127.0) as u8,
                }
            })
        })
        .collect();
    out.sort_by_key(|c| c.cc);
    Ok(out)
}

/// ファイルを並列に読む(左右差成分は持たない)。戻り値の順は `paths` と同じとは限らない
fn load_parallel(paths: &[PathBuf]) -> Vec<(PathBuf, Result<Loaded, String>)> {
    let load = |p: &PathBuf| {
        let r = crate::data::load_audio_file(p).map(|mut d| {
            d.side = None;
            Loaded {
                data: Arc::new(d),
                loop_points: wav_loop_points(p),
            }
        });
        (p.clone(), r)
    };
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    if paths.len() < 4 || threads == 1 {
        return paths.iter().map(load).collect();
    }
    let chunk = paths.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|part| s.spawn(move || part.iter().map(load).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{} を開けません: {e}", path.display()))?;
    // UTF-8 でなければ Latin-1 として読む(古い SFZ)
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_include(_: &str) -> Result<String, String> {
        Err("include なし".to_owned())
    }

    #[test]
    fn notes_parse_like_sfz() {
        assert_eq!(parse_note("c4"), Some(60));
        assert_eq!(parse_note("C#4"), Some(61));
        assert_eq!(parse_note("db4"), Some(61));
        assert_eq!(parse_note("c-1"), Some(0));
        assert_eq!(parse_note("b3"), Some(59));
        assert_eq!(parse_note("64"), Some(64));
        assert_eq!(parse_note("h4"), None);
    }

    #[test]
    fn headers_inherit_and_values_may_have_spaces() {
        let text = r#"
            /* ブロック
               コメント */
            //*********************************
            //--- /* 行コメントの中の開き */
            #define $VEL 100
            <control> default_path=Samples/
            <global> ampeg_release=0.5 // 行コメント
            <group> lovel=1 hivel=$VEL
            <region> sample=Piano C4 soft.wav key=c4
            <region> sample=d4.wav lokey=61 hikey=63 pitch_keycenter=62 ampeg_release=1
            <group> lovel=101
            <region>sample=loud.wav key=60
        "#;
        let rs = parse(text, &mut no_include).unwrap();
        assert_eq!(rs.len(), 3);
        assert_eq!(rs[0]["sample"], "Piano C4 soft.wav");
        assert_eq!(rs[0]["default_path"], "Samples/");
        assert_eq!(rs[0]["hivel"], "100");
        assert_eq!(rs[0]["ampeg_release"], "0.5");
        assert_eq!(rs[1]["ampeg_release"], "1");
        // 新しい <group> で前の group の opcode は消える(global は残る)
        assert!(!rs[2].contains_key("hivel"));
        assert_eq!(rs[2]["lovel"], "101");
        assert_eq!(rs[2]["ampeg_release"], "0.5");
    }

    #[test]
    fn inline_define_and_include_per_region() {
        let text = "#define $EXT flac\n<region> #define $KEY 21 lokey=21 hikey=22 #include \"Data/s.txt\"\n\
                    <region> #define $KEY 24 lokey=23 hikey=25 #include \"Data/s.txt\"";
        let rs = parse(text, &mut |name| {
            assert_eq!(name, "Data/s.txt");
            Ok("sample=PIANO $KEY.$EXT\npitch_keycenter=$KEY".to_owned())
        })
        .unwrap();
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0]["sample"], "PIANO 21.flac");
        assert_eq!(rs[1]["sample"], "PIANO 24.flac");
        assert_eq!(rs[1]["pitch_keycenter"], "24");
        assert_eq!(rs[1]["hikey"], "25");
    }

    #[test]
    fn include_expands_relative_files() {
        let text = "<group> key=36\n#include \"kick.sfz\"";
        let rs = parse(text, &mut |name| {
            assert_eq!(name, "kick.sfz");
            Ok("<region> sample=kick1.wav seq_length=2 seq_position=1\n<region> sample=kick2.wav seq_length=2 seq_position=2".to_owned())
        })
        .unwrap();
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[1]["key"], "36");
        assert_eq!(rs[1]["seq_position"], "2");
    }

    fn fake(_: &Path) -> Result<Loaded, String> {
        Ok(Loaded {
            data: Arc::new(SampleData::mono(vec![0.1; 1000], 48_000.0)),
            loop_points: None,
        })
    }

    #[test]
    fn zones_follow_the_opcodes() {
        let text = "
            <region> sample=a.wav key=c4 volume=-6 loop_mode=loop_continuous loop_start=100 loop_end=899
            <region> sample=b.wav lokey=36 hikey=36 pitch_keytrack=0 loop_mode=one_shot group=1 off_by=2 transpose=2 tune=50
            <region> sample=c.wav key=40 trigger=release
            <region> sample=d.wav key=41 locc64=64
            <region> sample=e.wav key=42 offset=200 end=599 lorand=0.5 hirand=1
            <region> sample=ks1.wav key=50 sw_last=24 sw_default=24
            <region> sample=ks2.wav key=50 sw_last=25
            <region> sample=cc_on.wav key=51 locc74=1 offset_cc74=127 amplitude_oncc7=100
            <region> sample=env.wav key=52 ampeg_sustain=0 ampeg_sustain_oncc103=100 ampeg_releasecc64=5 amp_veltrack=0
            <region> sample=tune.wav key=53 tune_cc89=1200 tune_curvecc89=9 amplitude_cc15=70 amplitude_curvecc15=33
            <curve>curve_index=9 v000=-1 v063=0 v127=1
            <curve>curve_index=33 v000=0.13 v037=0.8 v077=1 v127=1
        ";
        // CC の既定値は <control> の set_cc(先頭に足す)
        let text = format!("<control> set_cc74=100 set_cc103=127 set_cc89=63 set_cc15=100\n{text}");
        let rs = parse(&text, &mut no_include).unwrap();
        let mut cache = WaveCache::new();
        let zs = build_zones(&rs, Path::new("lib"), &mut cache, &mut fake).unwrap();
        assert_eq!(
            zs.len(),
            7,
            "release・既定値で外れる CC 条件・既定以外のキースイッチは除く"
        );
        assert_eq!(
            used_samples(&rs),
            [
                "a.wav",
                "b.wav",
                "e.wav",
                "ks1.wav",
                "cc_on.wav",
                "env.wav",
                "tune.wav"
            ]
        );
        let a = &zs[0];
        assert_eq!((a.key_lo, a.key_hi, a.root), (60, 60, 60.0));
        assert!((a.gain - 0.501).abs() < 0.01);
        assert_eq!(a.loop_range, Some((100.0, 900.0)));
        let b = &zs[1];
        assert!(b.play.one_shot && b.loop_range.is_none());
        assert_eq!((b.play.group, b.play.off_by), (1, 2));
        assert_eq!(b.play.keytrack, 0.0);
        assert!((b.play.tune_semis - 2.5).abs() < 1e-6);
        let e = &zs[2];
        assert_eq!(
            (e.start, e.end),
            (200, 600),
            "offset〜end の範囲(波形は切り出さず共有)"
        );
        assert_eq!(e.play.rand_lo, 0.5);
        assert_eq!(zs[3].key_lo, 50);
        // CC の既定値で評価: offset は 127 × 100/127、amplitude は cc7 の既定 100 で 100/127
        assert_eq!(zs[4].start, 100);
        assert!((zs[4].gain - 100.0 / 127.0).abs() < 1e-3);
        // エンベロープも CC の既定値で足す(ペダル cc64 の既定は 0)。amp_veltrack=0 はベロシティで音量が変わらない
        let env = &zs[5];
        assert_eq!(env.env.sustain, 1.0);
        assert_eq!(env.env.release, 0.01);
        assert!((env.play.vel_gain(0.5) * 0.5 - 1.0).abs() < 1e-6);
        // 既定(amp_veltrack=100)はベロシティの 2 乗
        assert!((zs[0].play.vel_gain(0.5) * 0.5 - 0.25).abs() < 1e-6);
        // カーブ: 真ん中(63)で音程が変わらない。音量は 100 の位置で 1(カーブ 33)× 70%
        let tune = &zs[6];
        assert!(
            tune.play.tune_semis.abs() < 1e-6,
            "{}",
            tune.play.tune_semis
        );
        assert!((tune.gain - 0.7).abs() < 1e-6, "{}", tune.gain);
    }

    #[test]
    fn silence_regions_carry_choke_groups() {
        // Big Rusty Drums の形: クローズの鍵盤に、音のあるゾーン(group 11)と無音のゾーン(group 15)。
        // オープン(46)は 15 で止まる
        let text = "
            <region> sample=closed.wav key=42 group=11
            <region> sample=*silence key=42 group=15
            <region> sample=open.wav key=46 group=16 off_by=15 loop_mode=one_shot
            <region> sample=*sine key=50
        ";
        let rs = parse(text, &mut no_include).unwrap();
        let zs = build_zones(&rs, Path::new("lib"), &mut WaveCache::new(), &mut fake).unwrap();
        assert_eq!(zs.len(), 3, "*silence は残し、*sine は除く");
        assert_eq!(used_samples(&rs), ["closed.wav", "open.wav"]);
        let p = glaux_dsp::MultiSamplerParams {
            zones: Arc::new(zs),
            gain: 1.0,
        };
        let open = glaux_dsp::MultiVoice::start(&p, 46, 1.0, Default::default(), 48_000.0);
        let closed = glaux_dsp::MultiVoice::start(&p, 42, 1.0, Default::default(), 48_000.0);
        let groups = closed.groups();
        assert!(groups.contains(&11) && groups.contains(&15), "{groups:?}");
        assert!(
            groups.iter().any(|g| open.stopped_by(*g)),
            "オープンが止まる"
        );
    }

    #[test]
    fn broken_numbers_do_not_overflow() {
        let text = "<region> sample=a.wav key=c999999999 end=1e30 offset=1e30
                    <region> sample=b.wav key=60 end=1e30";
        let rs = parse(text, &mut no_include).unwrap();
        let zs = build_zones(&rs, Path::new("lib"), &mut WaveCache::new(), &mut fake).unwrap();
        assert_eq!(zs.len(), 1, "壊れた region は捨て、残りは鳴る");
    }

    #[test]
    fn names_stay_inside_the_library() {
        assert!(valid_name("Piano/piano.sfz"));
        assert!(!valid_name("../x.sfz"));
        assert!(!valid_name("a/../../x.sfz"));
        assert!(
            valid_name("/samples/piano.sfz"),
            "別の場所の音源は絶対パスで"
        );
        assert!(!valid_name("x.wav"));
        assert!(!valid_name("C:x.sfz"), "Windows のドライブ指定");
        assert!(!valid_name(""));
    }

    #[test]
    fn loads_from_disk_with_smpl_loop_and_plays_round_robin() {
        let dir = std::env::temp_dir().join(format!("glaux_sfz_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inst/wav")).unwrap();
        // 2 つの波形(低い音と高い音)。1 つ目に smpl のループ点を付ける
        let write = |name: &str, hz: f32, smpl: bool| {
            let path = dir.join("inst/wav").join(name);
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..24_000 {
                let v = (i as f32 * hz * std::f32::consts::TAU / 48_000.0).sin();
                w.write_sample((v * 20_000.0) as i16).unwrap();
            }
            w.finalize().unwrap();
            if smpl {
                let mut bytes = std::fs::read(&path).unwrap();
                let mut body = vec![0u8; 36 + 24];
                body[28..32].copy_from_slice(&1u32.to_le_bytes());
                body[44..48].copy_from_slice(&1000u32.to_le_bytes());
                body[48..52].copy_from_slice(&20_999u32.to_le_bytes());
                bytes.extend_from_slice(b"smpl");
                bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&body);
                let riff = (bytes.len() - 8) as u32;
                bytes[4..8].copy_from_slice(&riff.to_le_bytes());
                std::fs::write(&path, bytes).unwrap();
            }
        };
        write("low.wav", 220.0, true);
        write("high.wav", 880.0, false);
        std::fs::write(
            dir.join("inst/snare.sfz"),
            "<group> key=38 pitch_keytrack=0\n\
             <region> sample=wav\\low.wav seq_length=2 seq_position=1\n\
             <region> sample=wav/high.wav seq_length=2 seq_position=2\n",
        )
        .unwrap();
        assert_eq!(list_files(&dir), vec!["inst/snare.sfz".to_owned()]);
        let mut cache = WaveCache::new();
        let zones =
            load_instrument(&dir, "inst/snare.sfz", &Default::default(), &mut cache).unwrap();
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].loop_range, Some((1000.0, 21_000.0)));
        assert!(zones[1].loop_range.is_none());

        // 調整つまみを変えて組み直しても、使っている間はファイルを読み直さない(キャッシュの波形を共有する)
        let again = load_instrument(
            &dir,
            "inst/snare.sfz",
            &[(7u8, 90u8)].into_iter().collect(),
            &mut cache,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&zones[0].data, &again[0].data));
        assert!(zones[0].data.side.is_none(), "左右差成分は持たない");
        drop(again);
        let p = glaux_dsp::MultiSamplerParams { zones, gain: 1.0 };
        let crossings = |variant: u32| {
            let mut v = glaux_dsp::MultiVoice::start_variant(
                &p,
                38,
                1.0,
                Default::default(),
                48_000.0,
                variant,
            );
            let x: Vec<f32> = (0..4_800).map(|_| v.next(&p)).collect();
            x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
        };
        assert!(crossings(0) < 30, "1 回目は低い音");
        assert!(crossings(1) > 60, "2 回目は高い音");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
