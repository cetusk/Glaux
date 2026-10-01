//! 簡潔なノートの書き方(apply_commands の add_notes / add_clip / add_track の notes)。
//!
//! AI がノートを `{"pos":15360,"dur":480,"pitch":64,"vel":96}` で 1 個ずつ書くと、送る量の多くがここに使われ、
//! tick の計算の間違いも起きやすい。ここでは次のような文字列も受け付け、MCP の層で今の形(クリップの頭からの
//! tick の絶対値)に直してからコマンドにする(コマンドの仕組みは変えない)。
//!
//! ```text
//! "5:1 1/8 E4 v96"          5 小節目の 1 拍目、8 分音符、E4、強さ 96
//! "5:2.5 1/16 C4+E4+G4"     5 小節目の 2 拍目の裏、16 分で和音
//! "@480 1/4. 60 staccato"   クリップの頭から 480 tick、付点 4 分、MIDI 番号 60、スタッカート
//! ```
//!
//! - 位置: `小節:拍`(どちらも 1 始まり、曲の小節。拍は拍子の分母の音符 = 6/8 なら 8 分で、小数可)か `@tick`(クリップの頭から)
//! - 長さ: `1/8`・`3/16`・付点 `1/4.`・3 連 `1/8t`、または tick の整数
//! - 音: 音名(`C4`・`F#3`・`Bb2`)か MIDI 番号。`+` でつなぐと同じ位置の和音
//! - 続けて(順不同): `v96`(強さ、省略で 100)、奏法の名前(`staccato` など、Articulation の名前)
//!
//! notes は文字列の配列のほか、`;` か改行で区切った 1 本の文字列でもよい。オブジェクトの書き方と混ぜてよい。

use glaux_core::Project;
use serde_json::{json, Value};
use std::collections::HashMap;

/// 省略時の強さ
const DEFAULT_VEL: u8 = 100;

/// 長さ: `1/8`・`3/16`・`1/4.`(付点)・`1/8t`(3 連)・tick の整数
fn parse_len(s: &str) -> Option<u64> {
    if let Ok(t) = s.parse::<u64>() {
        return (t > 0).then_some(t);
    }
    let (body, triplet) = match s.strip_suffix('t') {
        Some(b) => (b, true),
        None => (s, false),
    };
    let dots = body.chars().rev().take_while(|&c| c == '.').count();
    let body = &body[..body.len() - dots];
    let (n, d) = body.split_once('/')?;
    let (n, d): (u64, u64) = (n.parse().ok()?, d.parse().ok()?);
    if n == 0 || d == 0 {
        return None;
    }
    let whole = glaux_core::time::PPQ as f64 * 4.0;
    let mut len = whole * n as f64 / d as f64;
    let mut add = len / 2.0;
    for _ in 0..dots {
        len += add;
        add /= 2.0;
    }
    if triplet {
        len = len * 2.0 / 3.0;
    }
    let len = len.round() as u64;
    (len > 0).then_some(len)
}

/// 音: 音名か MIDI 番号
fn parse_pitch(s: &str) -> Option<u8> {
    s.parse::<u8>()
        .ok()
        .filter(|n| *n <= 127)
        .or_else(|| glaux_core::chord::parse_note(s))
}

/// 小節:拍 → 曲の頭からの tick(拍は拍子の分母の音符)
fn bar_beat_tick(project: &Project, s: &str) -> Option<u64> {
    let (bar, beat) = s.split_once(':')?;
    let bar: u32 = bar.trim().parse().ok()?;
    let beat: f64 = beat.trim().parse().ok()?;
    if beat < 1.0 || !beat.is_finite() {
        return None;
    }
    let (start, len) = glaux_core::arrange::bar_range(project, bar, 1)?;
    let den = project
        .time_sig_map
        .iter()
        .filter(|s| s.tick.0 <= start)
        .max_by_key(|s| s.tick)
        .map_or(4, |s| s.den.max(1)) as f64;
    let beat_len = glaux_core::time::PPQ as f64 * 4.0 / den;
    let off = ((beat - 1.0) * beat_len).round() as u64;
    // 小節を越える拍は書き間違いとみなす(次の小節は 小節:拍 で書く)
    (off < len).then_some(start + off)
}

/// 1 行を、クリップの頭(`clip_start`)からのノート(id なし。和音なら複数)に直す
pub fn parse_line(project: &Project, clip_start: u64, line: &str) -> Result<Vec<Value>, String> {
    let bad = |what: &str| format!("ノートの書き方が読めません({what}): \"{line}\"");
    let mut it = line.split_whitespace();
    let pos_s = it.next().ok_or_else(|| bad("空"))?;
    let pos = if let Some(t) = pos_s.strip_prefix('@') {
        t.parse::<u64>().map_err(|_| bad("位置 @tick"))?
    } else {
        let abs = bar_beat_tick(project, pos_s).ok_or_else(|| bad("位置 小節:拍"))?;
        abs.checked_sub(clip_start)
            .ok_or_else(|| bad("位置がクリップより前"))?
    };
    let dur = parse_len(it.next().ok_or_else(|| bad("長さが無い"))?).ok_or_else(|| bad("長さ"))?;
    let pitches: Vec<u8> = it
        .next()
        .ok_or_else(|| bad("音が無い"))?
        .split('+')
        .map(|p| parse_pitch(p).ok_or_else(|| bad("音")))
        .collect::<Result<_, _>>()?;
    let mut vel = DEFAULT_VEL;
    let mut articulation: Option<Value> = None;
    for tok in it {
        if let Some(v) = tok.strip_prefix('v').and_then(|v| v.parse::<u8>().ok()) {
            vel = v.clamp(1, 127);
        } else {
            // 奏法の名前は Articulation の serde の名前(snake_case)で確かめる
            serde_json::from_value::<glaux_core::Articulation>(json!(tok))
                .map_err(|_| bad(&format!("知らない指定 {tok}")))?;
            articulation = Some(json!(tok));
        }
    }
    Ok(pitches
        .into_iter()
        .map(|pitch| {
            let mut n = json!({ "pos": pos, "dur": dur, "pitch": pitch, "vel": vel });
            if let Some(a) = &articulation {
                n["articulation"] = a.clone();
            }
            n
        })
        .collect())
}

/// notes の中の文字列(簡潔な書き方)をオブジェクトに直す。無ければ何もしない
fn expand_list(notes: &mut Value, project: &Project, clip_start: u64) -> Result<(), String> {
    // 1 本の文字列なら ; か改行で区切った列
    if let Some(s) = notes.as_str() {
        *notes = Value::Array(
            s.split([';', '\n'])
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|l| json!(l))
                .collect(),
        );
    }
    let Some(arr) = notes.as_array_mut() else {
        return Ok(());
    };
    if !arr.iter().any(Value::is_string) {
        return Ok(());
    }
    let mut out = Vec::with_capacity(arr.len());
    for v in arr.drain(..) {
        match v.as_str() {
            Some(line) => out.extend(parse_line(project, clip_start, line)?),
            None => out.push(v),
        }
    }
    *arr = out;
    Ok(())
}

/// クリップのオブジェクト(add_clip の clip、add_track の clips の要素)の notes を直し、開始位置を覚える
fn expand_clip(
    clip: &mut Value,
    project: &Project,
    starts: &mut HashMap<String, u64>,
) -> Result<(), String> {
    let start = clip["start"].as_u64().unwrap_or(0);
    if let Some(id) = clip["id"].as_str() {
        starts.insert(id.to_owned(), start);
    }
    if let Some(notes) = clip.get_mut("notes") {
        expand_list(notes, project, start)?;
    }
    Ok(())
}

/// 1 つのコマンド(JSON)の中の簡潔な書き方を直す。`starts` は同じ呼び出しで先に作ったクリップの開始位置
pub fn expand_command(
    cmd: &mut Value,
    project: &Project,
    starts: &mut HashMap<String, u64>,
) -> Result<(), String> {
    match cmd.get("op").and_then(Value::as_str) {
        Some("add_notes") => {
            if cmd.get("notes").is_some_and(|n| {
                n.is_string() || n.as_array().is_some_and(|a| a.iter().any(Value::is_string))
            }) {
                let id = cmd["clip"].as_str().unwrap_or_default().to_owned();
                let start = match starts.get(&id) {
                    Some(s) => *s,
                    None => {
                        let cid = glaux_core::ClipId::parse(&id).map_err(|e| e.to_string())?;
                        project
                            .clip(&cid)
                            .map(|(_, c)| c.start.0)
                            .ok_or_else(|| format!("クリップが見つかりません: {id}"))?
                    }
                };
                if let Some(notes) = cmd.get_mut("notes") {
                    expand_list(notes, project, start)?;
                }
            }
        }
        Some("add_clip") => {
            if let Some(c) = cmd.get_mut("clip") {
                expand_clip(c, project, starts)?;
            }
        }
        Some("add_track") => {
            if let Some(clips) = cmd
                .get_mut("track")
                .and_then(|t| t.get_mut("clips"))
                .and_then(Value::as_array_mut)
            {
                for c in clips {
                    expand_clip(c, project, starts)?;
                }
            }
        }
        Some("batch") => {
            if let Some(cmds) = cmd.get_mut("commands").and_then(Value::as_array_mut) {
                for c in cmds {
                    expand_command(c, project, starts)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// 同じ呼び出しの中で作るクリップの開始位置を覚える(後の add_notes が簡潔な書き方を使えるように)
pub fn remember_starts(cmd: &Value, starts: &mut HashMap<String, u64>) {
    let mut clip = |c: &Value| {
        if let Some(id) = c["id"].as_str() {
            starts.insert(id.to_owned(), c["start"].as_u64().unwrap_or(0));
        }
    };
    match cmd.get("op").and_then(Value::as_str) {
        Some("add_clip") => clip(&cmd["clip"]),
        Some("add_track") => {
            for c in cmd["track"]["clips"].as_array().into_iter().flatten() {
                clip(c);
            }
        }
        Some("batch") => {
            for c in cmd["commands"].as_array().into_iter().flatten() {
                remember_starts(c, starts);
            }
        }
        _ => {}
    }
}

/// コマンドの中に簡潔な書き方(notes の文字列)があるか(無ければプロジェクトを読まずに済ませる)
pub fn has_compact(cmd: &Value) -> bool {
    fn notes_have(v: &Value) -> bool {
        v.get("notes").is_some_and(|n| {
            n.is_string() || n.as_array().is_some_and(|a| a.iter().any(Value::is_string))
        })
    }
    match cmd.get("op").and_then(Value::as_str) {
        Some("add_notes") => notes_have(cmd),
        Some("add_clip") => cmd.get("clip").is_some_and(notes_have),
        Some("add_track") => cmd
            .get("track")
            .and_then(|t| t.get("clips"))
            .and_then(Value::as_array)
            .is_some_and(|cs| cs.iter().any(notes_have)),
        Some("batch") => cmd
            .get("commands")
            .and_then(Value::as_array)
            .is_some_and(|cs| cs.iter().any(has_compact)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Tick, TimeSigEvent};

    #[test]
    fn lengths() {
        assert_eq!(parse_len("1/8"), Some(480));
        assert_eq!(parse_len("3/16"), Some(720));
        assert_eq!(parse_len("1/4."), Some(1440));
        assert_eq!(parse_len("1/4.."), Some(1680));
        assert_eq!(parse_len("1/8t"), Some(320));
        assert_eq!(parse_len("1/1"), Some(3840));
        assert_eq!(parse_len("250"), Some(250));
        assert_eq!(parse_len("0"), None);
        assert_eq!(parse_len("1/0"), None);
        assert_eq!(parse_len("x"), None);
    }

    #[test]
    fn lines_become_clip_relative_notes() {
        let mut p = Project::new("t");
        // 3 小節目から 6/8
        p.time_sig_map = vec![
            TimeSigEvent::new(Tick(0), 4, 4),
            TimeSigEvent::new(Tick(7680), 6, 8),
        ];
        // クリップは 2 小節目(3840)から
        let n = parse_line(&p, 3840, "2:3 1/8 E4 v96").unwrap();
        assert_eq!(
            n,
            vec![json!({"pos": 1920, "dur": 480, "pitch": 64, "vel": 96})]
        );
        // 和音と小数の拍と奏法
        let n = parse_line(&p, 3840, "2:1.5 1/16 C4+E4 staccato").unwrap();
        assert_eq!(n.len(), 2);
        assert_eq!(n[0]["pos"], 480);
        assert_eq!(n[1]["pitch"], 64);
        assert_eq!(n[0]["articulation"], "staccato");
        assert_eq!(n[0]["vel"], 100);
        // 6/8 の拍は 8 分(3 小節目の 4 拍目 = 7680 + 3 × 480)
        let n = parse_line(&p, 3840, "3:4 1/8 60").unwrap();
        assert_eq!(n[0]["pos"], 7680 + 1440 - 3840);
        // @tick はクリップの頭から
        let n = parse_line(&p, 3840, "@10 120 61").unwrap();
        assert_eq!(
            (n[0]["pos"].as_u64(), n[0]["dur"].as_u64()),
            (Some(10), Some(120))
        );
        // 書き間違い
        for bad in [
            "1:1 1/8 C4",   // クリップより前
            "2:5 1/8 C4",   // 4/4 に 5 拍目は無い
            "2:1 1/8 H4",   // 音名
            "2:1 1/8 C4 x", // 知らない指定
            "2:1 C4",       // 長さが無い
        ] {
            assert!(parse_line(&p, 3840, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn commands_mix_strings_and_objects() {
        let p = Project::new("t");
        let mut starts = HashMap::new();
        let mut cmd = json!({
            "op": "batch",
            "commands": [
                {"op": "add_clip", "track": "trk_x", "clip": {"id": "clp_a", "start": 3840, "notes": "2:1 1/4 C4; 2:2 1/4 D4"}},
                {"op": "add_notes", "clip": "clp_a", "notes": ["2:3 1/4 E4", {"pos": 2880, "dur": 960, "pitch": 65, "vel": 90}]}
            ]
        });
        assert!(has_compact(&cmd));
        expand_command(&mut cmd, &p, &mut starts).unwrap();
        let first = &cmd["commands"][0]["clip"]["notes"];
        assert_eq!(first[1]["pos"], 960);
        let second = &cmd["commands"][1]["notes"];
        assert_eq!(second[0]["pos"], 1920);
        assert_eq!(second[1]["pitch"], 65);
        assert!(!has_compact(&cmd));
    }
}
