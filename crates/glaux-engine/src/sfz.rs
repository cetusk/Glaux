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
use std::collections::HashMap;
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
    !name.is_empty()
        && (p.is_absolute() || !name.split(['/', '\\']).any(|c| c == ".."))
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
    let mut lines = Vec::new();
    let mut defines: Vec<(String, String)> = Vec::new();
    preprocess(text, include, &mut defines, 0, &mut lines)?;

    #[derive(PartialEq)]
    enum Level {
        Control,
        Global,
        Master,
        Group,
        Region,
        Ignore,
    }
    let mut level = Level::Ignore;
    let mut control = Region::new();
    let mut global = Region::new();
    let mut master = Region::new();
    let mut group = Region::new();
    let mut region: Option<Region> = None;
    let mut out = Vec::new();

    let finish = |region: &mut Option<Region>,
                  out: &mut Vec<Region>,
                  control: &Region,
                  layers: [&Region; 3]| {
        if let Some(r) = region.take() {
            let mut merged = Region::new();
            if let Some(dp) = control.get("default_path") {
                merged.insert("default_path".to_owned(), dp.clone());
            }
            for l in layers {
                merged.extend(l.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
            merged.extend(r);
            out.push(merged);
        }
    };

    for line in &lines {
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
                        Level::Ignore => continue,
                    };
                    map.insert(k, v);
                }
            }
        }
    }
    finish(&mut region, &mut out, &control, [&global, &master, &group]);
    Ok(out)
}

/// コメントを除き、`#define` を置き換え、`#include` を展開して行の列にする
fn preprocess(
    text: &str,
    include: &mut IncludeFn,
    defines: &mut Vec<(String, String)>,
    depth: u32,
    out: &mut Vec<String>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("#include が深すぎます(循環していませんか)".to_owned());
    }
    // ブロックコメント /* ... */
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("/*") {
        plain.push_str(&rest[..i]);
        rest = match rest[i + 2..].find("*/") {
            Some(j) => &rest[i + 2 + j + 2..],
            None => "",
        };
    }
    plain.push_str(rest);

    for raw in plain.lines() {
        let line = match raw.find("//") {
            Some(i) => &raw[..i],
            None => raw,
        };
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(d) = t.strip_prefix("#define") {
            let mut it = d.split_whitespace();
            if let (Some(name), Some(_)) = (it.next(), d.split_whitespace().nth(1)) {
                let value = d.trim_start()[name.len()..].trim().to_owned();
                defines.retain(|(n, _)| n != name);
                defines.push((name.to_owned(), value));
                // 長い名前から置き換える($A と $AB の取り違えを防ぐ)
                defines.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
            }
            continue;
        }
        let t = substitute(t, defines);
        if let Some(inc) = t.strip_prefix("#include") {
            let name = inc.trim().trim_matches('"');
            let body = include(&name.replace('\\', "/"))?;
            preprocess(&body, include, defines, depth + 1, out)?;
            continue;
        }
        out.push(t);
    }
    Ok(())
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
    let oct: i32 = oct.parse().ok()?;
    Some((oct + 1) * 12 + pc + acc)
}

fn num(r: &Region, k: &str) -> Option<f64> {
    r.get(k).and_then(|v| v.trim().parse::<f64>().ok())
}

fn note(r: &Region, k: &str) -> Option<i32> {
    r.get(k).and_then(|v| parse_note(v))
}

/// 波形の読み込みの使い回し(パス・切り出し → 波形)
pub type WaveCache = HashMap<(PathBuf, usize, usize), Arc<SampleData>>;

/// 読んだ波形と、そのループ点(ファイルに書かれていれば)
pub struct Loaded {
    pub data: Arc<SampleData>,
    pub loop_points: Option<(usize, usize)>,
}

/// region の列をゾーンにする。`dir` は .sfz のあるフォルダ、`load` は波形の読み込み。
/// 波形を 1 つも読めなければ Err。
pub fn build_zones(
    regions: &[Region],
    dir: &Path,
    cache: &mut WaveCache,
    load: &mut dyn FnMut(&Path) -> Result<Loaded, String>,
) -> Result<Vec<Zone>, String> {
    // キースイッチ: 既定(sw_default、無ければ最も低い sw_last)の region だけを使う
    let sw_default = regions
        .iter()
        .find_map(|r| note(r, "sw_default"))
        .or_else(|| regions.iter().filter_map(|r| note(r, "sw_last")).min());
    let mut files: HashMap<PathBuf, Option<Loaded>> = HashMap::new();
    let mut zones = Vec::new();
    let mut errors = Vec::new();

    for r in regions {
        if matches!(
            r.get("trigger").map(|s| s.as_str()),
            Some("release" | "release_key" | "legato")
        ) {
            continue;
        }
        if r.iter().any(|(k, v)| {
            (k.starts_with("locc") && v.trim().parse::<f64>().is_ok_and(|x| x > 0.0))
                || k.starts_with("on_locc")
                || k.starts_with("on_hicc")
        }) {
            continue;
        }
        if let (Some(sw), Some(def)) = (note(r, "sw_last"), sw_default) {
            if sw != def {
                continue;
            }
        }
        let Some(sample) = r.get("sample") else {
            continue;
        };
        if sample.starts_with('*') {
            continue; // *sine などの内蔵の発音器は扱わない
        }
        let mut path = dir.to_path_buf();
        if let Some(dp) = r.get("default_path") {
            path.push(dp.replace('\\', "/"));
        }
        path.push(sample.trim().replace('\\', "/"));

        if !files.contains_key(&path) {
            let loaded = match load(&path) {
                Ok(l) => Some(l),
                Err(e) => {
                    errors.push(e);
                    None
                }
            };
            files.insert(path.clone(), loaded);
        }
        let Some(Some(file)) = files.get(&path) else {
            continue;
        };
        let len = file.data.frames.len();
        let off = num(r, "offset").unwrap_or(0.0).max(0.0) as usize;
        let end = num(r, "end").map_or(len, |e| (e.max(0.0) as usize + 1).min(len));
        if end <= off + 1 {
            continue;
        }
        let data = if off == 0 && end == len {
            file.data.clone()
        } else {
            cache
                .entry((path.clone(), off, end))
                .or_insert_with(|| {
                    let d = &file.data;
                    Arc::new(SampleData {
                        frames: d.frames[off..end].to_vec(),
                        sample_rate: d.sample_rate,
                        side: d.side.as_ref().map(|s| s[off..end].to_vec()),
                        mips: Default::default(),
                    })
                })
                .clone()
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
            _ => file.loop_points,
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
        let loop_range = match (mode.as_str(), points) {
            ("loop_continuous" | "loop_sustain", Some((s, e))) if s >= off && e < end && e > s => {
                Some(((s - off) as f64, (e + 1 - off) as f64))
            }
            _ => None,
        };

        let volume = num(r, "volume").unwrap_or(0.0).clamp(-144.0, 24.0) as f32;
        let amplitude = num(r, "amplitude").unwrap_or(100.0).clamp(0.0, 100.0) as f32 / 100.0;
        let env = ZoneEnv {
            attack: (num(r, "ampeg_attack").unwrap_or(0.0) as f32).clamp(0.001, 10.0),
            hold: (num(r, "ampeg_hold").unwrap_or(0.0) as f32).clamp(0.0, 10.0),
            decay: (num(r, "ampeg_decay").unwrap_or(0.0) as f32).clamp(0.005, 30.0),
            sustain: (num(r, "ampeg_sustain").unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0),
            release: (num(r, "ampeg_release").unwrap_or(0.001) as f32).clamp(0.01, 10.0),
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
        let play = ZonePlay {
            seq_length: int("seq_length").unwrap_or(1).clamp(1, 255) as u8,
            seq_position: int("seq_position").unwrap_or(1).clamp(1, 255) as u8,
            rand_lo: (num(r, "lorand").unwrap_or(0.0) as f32).clamp(0.0, 1.0),
            rand_hi: (num(r, "hirand").unwrap_or(1.0) as f32).clamp(0.0, 1.0),
            group: int("group").unwrap_or(0) as u32,
            off_by: int("off_by").unwrap_or(0) as u32,
            one_shot: mode == "one_shot",
            keytrack: (num(r, "pitch_keytrack").unwrap_or(100.0) / 100.0) as f32,
            tune_semis: (num(r, "transpose").unwrap_or(0.0) + num(r, "tune").unwrap_or(0.0) / 100.0)
                as f32,
        };
        zones.push(Zone {
            key_lo: lo as u8,
            key_hi: hi as u8,
            vel_lo: vlo as u8,
            vel_hi: vhi as u8,
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
pub fn load_instrument(
    library: &Path,
    name: &str,
    cache: &mut WaveCache,
) -> Result<Arc<Vec<Zone>>, String> {
    if !valid_name(name) {
        return Err(format!("SFZ の名前が正しくありません: {name}"));
    }
    let path = library.join(name);
    let dir = path.parent().unwrap_or(library).to_path_buf();
    let text = read_text(&path)?;
    let regions = parse(&text, &mut |inc| read_text(&dir.join(inc)))?;
    let zones = build_zones(&regions, &dir, cache, &mut |p| {
        let data = crate::data::load_audio_file(p)?;
        Ok(Loaded {
            data: Arc::new(data),
            loop_points: wav_loop_points(p),
        })
    })?;
    Ok(Arc::new(zones))
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
        ";
        let rs = parse(text, &mut no_include).unwrap();
        let mut cache = WaveCache::new();
        let zs = build_zones(&rs, Path::new("lib"), &mut cache, &mut fake).unwrap();
        assert_eq!(
            zs.len(),
            4,
            "release・CC 条件・既定以外のキースイッチは除く"
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
        assert_eq!(e.data.frames.len(), 400);
        assert_eq!(e.play.rand_lo, 0.5);
        assert_eq!(zs[3].key_lo, 50);
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
        let zones = load_instrument(&dir, "inst/snare.sfz", &mut cache).unwrap();
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].loop_range, Some((1000.0, 21_000.0)));
        assert!(zones[1].loop_range.is_none());

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
