//! グルーブの型を当てる計算(MCP の `apply_groove` / `add_ghost_notes` の中身)。
//!
//! AI が作った曲はノートがほぼ 16 分の格子どおりで、強弱も平らだった。人間らしさの研究では、ランダムな揺れより
//! 「楽器ごと・拍の位置ごとの決まったずれと強弱の型」が効き、揺れは小さく相関のあるもの(1/f)にとどめるのがよい
//! (誇張すると評価が下がる)。型は Groove MIDI Dataset(Google Magenta、CC BY 4.0)の 4/4 のビートを集計して
//! 作った `data/grooves.json`(16 分の位置ごとの平均のずれ・揺れ・強さ・出現率)と、手作りの電子音楽の型。
//!
//! 位置は曲の小節線から数えた 16 分の番号(0〜15)で型を引く。ずれは 16 分に対する割合で持つので、テンポに比例する。
//! 乱数は呼び出し側の `seed` から作る(同じ入力なら同じ結果。コマンドは決定的)。

use crate::id::NoteId;
use crate::model::Note;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const SIXTEENTH: f64 = (crate::time::PPQ / 4) as f64;

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Slot {
    /// 16 分の位置からの平均のずれ(16 分に対する割合。正 = 後ろ)
    pub offset: f64,
    /// ずれの標準偏差(同じ単位)
    pub sd: f64,
    /// 平均の強さ(0 = データが無い)
    pub vel: u8,
    /// 1 小節あたりの出現率(ゴーストノートを足すときに使う)
    pub prob: f64,
}

#[derive(Debug, Deserialize)]
pub struct Style {
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub handmade: bool,
    pub bpm: u32,
    /// kick / snare / hat / ride / tom / crash / ghost(スネアのゴースト)ごとの 16 分の 16 か所
    pub parts: HashMap<String, Vec<Slot>>,
}

#[derive(Debug, Deserialize)]
struct Library {
    source: String,
    styles: HashMap<String, Style>,
}

fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(|| {
        serde_json::from_str(include_str!("../data/grooves.json"))
            .expect("data/grooves.json は同梱のデータ")
    })
}

/// 使える型の名前(データセット由来か、手作りか)と、元のテンポの目安
pub fn styles() -> Vec<(&'static str, bool, u32)> {
    let mut v: Vec<_> = library()
        .styles
        .iter()
        .map(|(k, s)| (k.as_str(), s.handmade, s.bpm))
        .collect();
    v.sort_by_key(|(k, h, _)| (*h, *k));
    v
}

/// 型の出典の表記(CC BY 4.0 の帰属表示)
pub fn source_note() -> &'static str {
    &library().source
}

pub fn style(name: &str) -> Option<&'static Style> {
    library().styles.get(name)
}

/// GM のドラムの音程 → 型の楽器
pub fn drum_part(pitch: u8) -> Option<&'static str> {
    Some(match pitch {
        35 | 36 => "kick",
        37..=40 => "snare",
        42 | 44 | 46 | 22 | 26 => "hat",
        51 | 53 | 59 => "ride",
        41 | 43 | 45 | 47 | 48 | 50 | 58 => "tom",
        49 | 52 | 55 | 57 => "crash",
        _ => return None,
    })
}

/// 同じ seed から同じ列を作る小さな乱数(xorshift64*)
struct Rng(u64);

impl Rng {
    fn new(seed: u64, salt: u64) -> Rng {
        Rng((seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// 0〜1
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// 1/f(ピンク)に近い揺れの列(Voss-McCartney、8 列)。分散がおよそ 1 になるように割る
fn pink(seed: u64, salt: u64, n: usize) -> Vec<f64> {
    const ROWS: usize = 8;
    let mut rng = Rng::new(seed, salt);
    let mut rows = [0.0f64; ROWS];
    for r in rows.iter_mut() {
        *r = rng.unit() * 2.0 - 1.0;
    }
    (0..n)
        .map(|i| {
            // i の下位ビットが変わった列だけ引き直す
            let k = ((i as u64).trailing_zeros() as usize).min(ROWS - 1);
            rows[k] = rng.unit() * 2.0 - 1.0;
            let s: f64 = rows.iter().sum();
            // 一様(-1〜1)の分散 1/3 が 8 列で 8/3
            s / (8.0f64 / 3.0).sqrt()
        })
        .collect()
}

/// 型の当て方
#[derive(Clone, Debug)]
pub struct GrooveOptions {
    /// 先に 16 分の格子へ寄せる量(0〜1)。0 なら今の位置(スウィング済みでも)にずれを足す
    pub quantize: f64,
    /// 型のずれの効き(0〜1.5。1 = 型どおり)
    pub timing: f64,
    /// 型の強弱の効き(0〜1。1 = 型の強弱の比のとおり)
    pub velocity: f64,
    /// 小さな揺れ(1/f)の標準偏差(tick)
    pub humanize_ticks: f64,
    /// 楽器ごとの前ノリ(負)・後ノリ(正)(tick)
    pub pocket_ticks: HashMap<String, f64>,
    /// ドラム以外のトラックで使う型の楽器(kick / snare / hat …)。None ならドラムとして音程で分ける
    pub as_part: Option<String>,
    pub seed: u64,
}

/// ノートの位置と強さの変更(変わるものだけ)
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEdit {
    pub id: NoteId,
    pub pos: u64,
    pub vel: u8,
}

/// `bar_start_of(絶対 tick)` はその位置を含む小節の頭(tick)を返す関数(拍子に沿う)
pub fn apply(
    notes: &[Note],
    clip_start: u64,
    clip_len: u64,
    style: &Style,
    opts: &GrooveOptions,
    bar_start_of: &dyn Fn(u64) -> u64,
) -> Vec<NoteEdit> {
    // 楽器ごとの最大の強さ(強弱の比の基準)
    let max_vel: HashMap<&str, f64> = style
        .parts
        .iter()
        .map(|(k, v)| {
            (
                k.as_str(),
                v.iter().map(|s| s.vel).max().unwrap_or(0) as f64,
            )
        })
        .collect();
    // 揺れは楽器ごとに 1 本の 1/f の列を、時間順に使う
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by_key(|&i| (notes[i].pos, notes[i].pitch));
    let parts: Vec<&str> = notes
        .iter()
        .map(|n| match &opts.as_part {
            Some(p) => p.as_str(),
            None => drum_part(n.pitch).unwrap_or("other"),
        })
        .collect();
    let mut streams: HashMap<&str, (Vec<f64>, usize)> = HashMap::new();
    let mut out = Vec::new();
    for &i in &order {
        let n = &notes[i];
        let part = parts[i];
        let abs = clip_start + n.pos.0;
        let bar = bar_start_of(abs);
        let x = (abs - bar) as f64 / SIXTEENTH;
        let idx = x.round();
        let grid_abs = bar as f64 + idx * SIXTEENTH;
        let slot = style
            .parts
            .get(part)
            .and_then(|v| v.get((idx as i64).rem_euclid(16) as usize))
            .copied();
        let mut pos = abs as f64 + (grid_abs - abs as f64) * opts.quantize.clamp(0.0, 1.0);
        if let Some(s) = slot {
            if s.vel > 0 {
                pos += s.offset * SIXTEENTH * opts.timing.clamp(0.0, 1.5);
            }
        }
        pos += opts.pocket_ticks.get(part).copied().unwrap_or(0.0);
        // 小節の頭の音は揺らさない(拍の土台)
        let on_downbeat = (idx as i64).rem_euclid(16) == 0 && (x - idx).abs() < 0.25;
        if opts.humanize_ticks > 0.0 && !on_downbeat {
            let salt = part
                .bytes()
                .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
            let (seq, k) = streams
                .entry(part)
                .or_insert_with(|| (pink(opts.seed, salt, notes.len().max(1)), 0));
            pos += seq[*k % seq.len()] * opts.humanize_ticks;
            *k += 1;
        }
        let new_pos = (pos.round().max(clip_start as f64) as u64 - clip_start)
            .min(clip_len.saturating_sub(1));
        let mut vel = n.vel;
        if let (Some(s), Some(&mx)) = (slot, max_vel.get(part)) {
            if s.vel > 0 && mx > 0.0 {
                let rel = s.vel as f64 / mx;
                let k = 1.0 + opts.velocity.clamp(0.0, 1.0) * (rel - 1.0);
                vel = (n.vel as f64 * k).round().clamp(1.0, 127.0) as u8;
            }
        }
        if new_pos != n.pos.0 || vel != n.vel {
            out.push(NoteEdit {
                id: n.id.clone(),
                pos: new_pos,
                vel,
            });
        }
    }
    out
}

/// ゴーストノートを置く位置と強さ(クリップ内の tick, 強さ)。型のスネアのゴーストの出現率 × `density` で、
/// 16 分の各位置に置くかを決める。同じ音程の音がある所・その前後 60 tick・バックビート(2・4 拍)は避ける
#[allow(clippy::too_many_arguments)]
pub fn ghost_positions(
    notes: &[Note],
    pitch: u8,
    clip_start: u64,
    clip_len: u64,
    style: &Style,
    density: f64,
    velocity: Option<u8>,
    seed: u64,
    bar_start_of: &dyn Fn(u64) -> u64,
) -> Vec<(u64, u8)> {
    let ghosts = style.parts.get("ghost").cloned().unwrap_or_else(|| {
        vec![
            Slot {
                offset: 0.0,
                sd: 0.0,
                vel: 0,
                prob: 0.0
            };
            16
        ]
    });
    let taken: Vec<u64> = notes
        .iter()
        .filter(|n| n.pitch == pitch)
        .map(|n| n.pos.0)
        .collect();
    let mut rng = Rng::new(seed, 0x6857);
    let mut out = Vec::new();
    let mut t = 0u64;
    while t < clip_len {
        let abs = clip_start + t;
        let bar = bar_start_of(abs);
        let idx = (((abs - bar) as f64 / SIXTEENTH).round() as i64).rem_euclid(16) as usize;
        let on_grid = (abs - bar) % SIXTEENTH as u64 == 0;
        if on_grid && idx != 4 && idx != 12 {
            let s = ghosts[idx];
            let p = (s.prob * density).clamp(0.0, 0.95);
            let free = taken.iter().all(|&x| x.abs_diff(t) > 60);
            if free && rng.unit() < p {
                let v = velocity.unwrap_or(if s.vel > 0 { s.vel } else { 32 });
                // 強さを少し揺らす(±4)
                let jitter = (rng.unit() * 8.0 - 4.0).round() as i32;
                out.push((t, (v as i32 + jitter).clamp(1, 60) as u8));
            }
        }
        t += SIXTEENTH as u64;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Articulation;
    use crate::time::Tick;

    fn note(i: usize, pos: u64, pitch: u8, vel: u8) -> Note {
        Note {
            id: NoteId::parse(&format!("nt_g{i:05}")).unwrap(),
            pos: Tick(pos),
            dur: Tick(120),
            pitch,
            vel,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
            glide_ms: None,
        }
    }

    /// 4/4 の 1 小節 = 3840
    fn bars(t: u64) -> u64 {
        t / 3840 * 3840
    }

    fn opts() -> GrooveOptions {
        GrooveOptions {
            quantize: 0.0,
            timing: 1.0,
            velocity: 1.0,
            humanize_ticks: 0.0,
            pocket_ticks: HashMap::new(),
            as_part: None,
            seed: 7,
        }
    }

    #[test]
    fn library_loads_with_datasets_and_handmade_styles() {
        let names: Vec<_> = styles().iter().map(|s| s.0).collect();
        for n in [
            "funk", "hiphop", "rock", "pop", "soul", "house", "techno", "trap",
        ] {
            assert!(names.contains(&n), "{names:?}");
        }
        assert!(source_note().contains("CC BY 4.0"));
        let funk = style("funk").unwrap();
        assert!(!funk.handmade && funk.files >= 40);
        assert_eq!(funk.parts["hat"].len(), 16);
    }

    #[test]
    fn funk_pushes_the_backbeat_late_and_shapes_hat_accents() {
        // 格子どおりの 1 小節: キック 1・3 拍、スネア 2・4 拍、ハット 8 分(全部同じ強さ)
        let mut ns = vec![
            note(0, 0, 36, 100),
            note(1, 1920, 36, 100),
            note(2, 960, 38, 100),
            note(3, 2880, 38, 100),
        ];
        for k in 0..16 {
            ns.push(note(10 + k, k as u64 * 240, 42, 100));
        }
        let edits = apply(&ns, 0, 3840, style("funk").unwrap(), &opts(), &bars);
        let get = |id: &str| edits.iter().find(|e| e.id.as_str() == id).cloned();
        // バックビートのスネアは後ろへ(数 tick)
        let s2 = get("nt_g00002").expect("スネアは動く");
        assert!(s2.pos > 960 && s2.pos < 960 + 30, "{s2:?}");
        // 1 拍目のキックは基準なのでほぼ動かない
        assert!(get("nt_g00000").map_or(true, |e| e.pos.abs_diff(0) <= 2));
        // ハットは表(4 分)が強く、16 分の裏が弱い
        let vel = |i: usize| get(&format!("nt_g{:05}", 10 + i)).map_or(100, |e| e.vel);
        assert!(vel(0) > vel(1) + 20, "{} {}", vel(0), vel(1));
        assert!(vel(4) > vel(3) + 20, "{} {}", vel(4), vel(3));
        // 同じ入力なら同じ結果
        assert_eq!(
            edits,
            apply(&ns, 0, 3840, style("funk").unwrap(), &opts(), &bars)
        );
    }

    #[test]
    fn humanize_is_small_correlated_and_keeps_downbeats() {
        let ns: Vec<Note> = (0..64).map(|k| note(k, k as u64 * 240, 42, 90)).collect();
        let mut o = opts();
        o.timing = 0.0;
        o.velocity = 0.0;
        o.humanize_ticks = 12.0;
        let edits = apply(&ns, 0, 3840 * 4, style("techno").unwrap(), &o, &bars);
        let moved: HashMap<String, i64> = edits
            .iter()
            .map(|e| (e.id.as_str().to_owned(), e.pos as i64))
            .collect();
        let dev: Vec<f64> = ns
            .iter()
            .map(|n| {
                (moved.get(n.id.as_str()).copied().unwrap_or(n.pos.0 as i64) - n.pos.0 as i64)
                    as f64
            })
            .collect();
        // 小節の頭は動かない
        for b in 0..4 {
            assert_eq!(dev[b * 16], 0.0);
        }
        let sd = (dev.iter().map(|d| d * d).sum::<f64>() / dev.len() as f64).sqrt();
        assert!(sd > 4.0 && sd < 30.0, "{sd}");
        // 隣どうしに相関がある(白色ノイズなら 0 付近)
        let xs: Vec<f64> = dev.iter().copied().filter(|d| *d != 0.0).collect();
        let m = xs.iter().sum::<f64>() / xs.len() as f64;
        let c1: f64 = xs.windows(2).map(|w| (w[0] - m) * (w[1] - m)).sum::<f64>();
        let c0: f64 = xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>();
        assert!(c1 / c0 > 0.2, "{}", c1 / c0);
        // seed が違えば違う揺れ
        o.seed = 8;
        assert_ne!(
            edits,
            apply(&ns, 0, 3840 * 4, style("techno").unwrap(), &o, &bars)
        );
    }

    #[test]
    fn ghosts_avoid_backbeats_and_existing_notes() {
        let ns = vec![
            note(0, 960, 38, 100),
            note(1, 2880, 38, 100),
            note(2, 1440, 38, 90),
        ];
        let g = ghost_positions(
            &ns,
            38,
            0,
            3840 * 8,
            style("funk").unwrap(),
            1.0,
            None,
            3,
            &bars,
        );
        assert!(!g.is_empty());
        for (t, v) in &g {
            let idx = (t % 3840) / 240;
            assert!(idx != 4 && idx != 12, "バックビートを避ける: {t}");
            assert!(t.abs_diff(1440) > 60, "既存の音の近くを避ける");
            assert!((1..=60).contains(v));
        }
        // 密度 0 なら置かない
        assert!(ghost_positions(
            &ns,
            38,
            0,
            3840 * 8,
            style("funk").unwrap(),
            0.0,
            None,
            3,
            &bars
        )
        .is_empty());
    }
}
