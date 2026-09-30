//! 旋律の計画の提案と、計画から音符を作ること(上から下へ)。
//!
//! 今までの write_melody は 1〜2 小節の動機を作って展開する(下から上へ)ので、細部を変えても
//! 全体のリズムの輪郭と音域が変わらなかった。ここでは層の順に決める(docs の層の設計):
//!
//! 1. 区間(L0): 音域の軌跡(区間の中で動き、区間どうしで対比)・密度の起伏・句の並び・リズムの系統 → [`propose`]
//! 2. 骨格(L1): 句ごとにおおむね 2 拍に 1 音。音域の軌跡と句の輪郭からの目標の高さ、和音、音程の費用
//!    (旋律の還元の辺の費用を生成向けにしたもの)で動的計画法。終わりの音は終止の音度。A′ は A の骨格を写し、
//!    音域の軌跡に合わせてずらし、終わりを作り直す
//! 3. リズム(L2): 骨格の音の間に、句の密度とリズムの系統で打点を入れる(小節の型の繰り返しではなく句の単位)。
//!    句の終わりの音は伸ばし、句と句の間は息継ぎ
//! 4. 表面(L3): 骨格の 2 音の間を経過音・刺繍音で埋める
//!
//! どれも seed で決まる(同じ計画・同じ seed なら同じ音)。

use crate::chord::{note_name, parse_note, Key};
use crate::melody::MelNote;
use crate::plan::{MelodyPlan, PhrasePlan, RegisterPoint, SectionPlan};
use crate::time::PPQ;

// ---------------------------------------------------------------- 乱数(決定的)

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len())]
    }
}

// ---------------------------------------------------------------- 提案(L0)

/// 区間(曲のマーカーか、指定の範囲)
#[derive(Clone, Debug)]
pub struct SectionSpec {
    pub name: String,
    pub start_bar: u32,
    pub bars: u32,
    /// 盛り上がり 0〜10(無ければ 5)
    pub energy: Option<f32>,
}

/// 区間の並びから、区間ごとの計画を提案する。`low`〜`high` は旋律の音域、`breath` はジャンルが息継ぎを
/// 大きく取るか(歌・管)
pub fn propose(
    sections: &[SectionSpec],
    low: u8,
    high: u8,
    breath: bool,
    key: Option<Key>,
    seed: u64,
) -> Vec<SectionPlan> {
    // key があれば音域の点を音階の音にそろえる(読みやすさのため)
    let pcs_owned: Vec<u8> = key.map_or_else(Vec::new, |k| {
        crate::harmony::scale_pitch_classes(k.tonic, if k.minor { "minor" } else { "major" })
    });
    let scale_pcs: &[u8] = &pcs_owned;
    let mut rng = Rng::new(seed);
    let range = high.saturating_sub(low).max(7) as f64;
    let mut out: Vec<SectionPlan> = Vec::new();
    let mut prev_base: Option<f64> = None;
    let mut prev_family: Option<String> = None;
    for (si, s) in sections.iter().enumerate() {
        let e = s.energy.unwrap_or(5.0).clamp(0.0, 10.0) as f64;
        let span = (6.0 + 0.8 * e).round().clamp(6.0, 16.0);
        let mut base = low as f64 + range * (0.25 + 0.05 * e);
        // 前の区間と音域の中心が近すぎれば、盛り上がりの向きへずらす(区間どうしの対比)
        if let (Some(pb), Some(pe)) = (
            prev_base,
            si.checked_sub(1)
                .map(|i| sections[i].energy.unwrap_or(5.0) as f64),
        ) {
            if (base - pb).abs() < 3.0 {
                base = if e >= pe { pb + 3.0 } else { pb - 3.0 };
            }
        }
        let lo_c = low as f64 + span / 2.0 - 2.0;
        let hi_c = high as f64 - span / 2.0 + 2.0;
        base = base.clamp(lo_c.min(hi_c), hi_c.max(lo_c));
        prev_base = Some(base);
        let bars = s.bars.max(1) as f64;
        // 音域の軌跡: 盛り上がる区間は後半に山、静かな区間は谷から次へ上がる
        let shape: &[(f64, f64)] = if e >= 7.0 {
            &[(0.0, -3.0), (0.5, 0.0), (0.75, 3.5), (1.0, 0.5)]
        } else if e >= 4.0 {
            &[(0.0, -3.0), (0.6, 3.0), (1.0, 0.0)]
        } else {
            &[(0.0, 2.0), (0.5, -4.0), (1.0, 6.0)]
        };
        let register: Vec<RegisterPoint> = shape
            .iter()
            .map(|&(f, d)| RegisterPoint {
                at: (f * bars * 2.0).round() / 2.0,
                center: note_name(snap_scale(
                    (base + d).round().clamp(low as f64, high as f64) as i32,
                    scale_pcs,
                ) as u8),
                span: Some(span as u8),
            })
            .collect();
        // 句の長さ(揃えすぎない)と名前
        let lengths = phrase_lengths(s.bars, &mut rng);
        let n = lengths.len();
        let labels = labels_for(n, &mut rng);
        // 句の始まりのずれ(弱起・裏から)。同じ文字の句は元の句のずれを引き継ぎやすい
        let mut offsets: Vec<f64> = Vec::with_capacity(n);
        for (k, l) in labels.iter().enumerate() {
            let base_label = l.trim_end_matches(['′', '″', '‴']);
            let first = labels
                .iter()
                .position(|x| x.trim_end_matches(['′', '″', '‴']) == base_label);
            let off = match first {
                Some(j) if j < k && rng.unit() < 0.6 => offsets[j],
                _ => *rng.pick(&[0.0, 0.0, -0.5, 0.5, -1.0, 1.0]),
            };
            // 曲の頭より前には入れない
            let off = if s.start_bar == 1 && k == 0 {
                off.max(0.0)
            } else {
                off
            };
            offsets.push(off);
        }
        if n >= 3 && offsets.windows(2).all(|w| w[0] == w[1]) {
            offsets[1] = if offsets[1] == 0.5 { -0.5 } else { 0.5 };
        }
        let d0 = 0.9 + 0.13 * e + if breath { 0.0 } else { 0.25 };
        let peak_frac = if e >= 7.0 {
            0.75
        } else if e >= 4.0 {
            0.6
        } else {
            1.0
        };
        let mut phrases = Vec::with_capacity(n);
        let mut at = 0.0;
        for k in 0..n {
            let len = lengths[k];
            let (x0, x1) = (at / bars, (at + len) / bars);
            at += len;
            let last = k + 1 == n;
            let contour = if x0 <= peak_frac && peak_frac <= x1 && peak_frac < 1.0 {
                "arch"
            } else if x1 <= peak_frac {
                if e < 4.0 && k == 0 {
                    "valley"
                } else {
                    "rise"
                }
            } else {
                "fall"
            };
            let mult = if last {
                0.75
            } else {
                [0.85, 1.0, 1.15, 0.9][k % 4]
            };
            let label = labels[k].clone();
            let base_label = label.trim_end_matches(['′', '″', '‴']).to_owned();
            let primes = label.chars().count() - base_label.chars().count();
            phrases.push(PhrasePlan {
                label: label.clone(),
                bars: len,
                offset_beats: offsets[k],
                like: (primes > 0).then(|| base_label.clone()),
                transform: match primes {
                    0 => vec![],
                    1 => vec!["tail".to_owned()],
                    _ => vec!["shift".to_owned(), "tail".to_owned()],
                },
                cadence: Some(if last { "closed" } else { "open" }.to_owned()),
                ending_degree: Some(if last { 1 } else { *rng.pick(&[2u8, 5, 5, 3]) }),
                contour: Some(contour.to_owned()),
                density: Some(((d0 * mult) * 100.0).round() / 100.0),
                skeleton: vec![],
                note: None,
            });
        }
        let families: &[&str] = if e >= 7.0 {
            if breath {
                &["sustain", "syncopated"]
            } else {
                &["syncopated", "pulse"]
            }
        } else if e >= 4.0 {
            &["pulse", "syncopated"]
        } else {
            &["sparse", "sustain"]
        };
        let mut family = rng.pick(families).to_string();
        if prev_family.as_deref() == Some(family.as_str()) {
            family = families
                .iter()
                .find(|f| **f != family)
                .unwrap_or(&families[0])
                .to_string();
        }
        prev_family = Some(family.clone());
        out.push(SectionPlan {
            name: s.name.clone(),
            start_bar: s.start_bar.max(1),
            bars: s.bars.max(1),
            energy: s.energy,
            register,
            density: phrases.iter().filter_map(|p| p.density).collect(),
            rhythm_family: Some(family),
            phrases,
            handoff: None,
            like: None,
            note: None,
        });
    }
    // 山の区間(盛り上がりが最大)の音域の中心の最高より、ほかの区間は 2 半音以上低く
    if let Some(pi) = (0..out.len()).max_by(|&a, &b| {
        let e = |k: usize| sections[k].energy.unwrap_or(5.0);
        e(a).total_cmp(&e(b)).then(a.cmp(&b))
    }) {
        let top = out[pi]
            .register
            .iter()
            .filter_map(|r| parse_note(&r.center))
            .max()
            .unwrap_or(high);
        for (k, sec) in out.iter_mut().enumerate() {
            if k == pi {
                continue;
            }
            for r in &mut sec.register {
                if parse_note(&r.center).is_some_and(|c| c + 2 > top) {
                    r.center =
                        note_name(snap_scale(top as i32 - 2, scale_pcs).min(top as i32 - 1) as u8);
                }
            }
        }
    }
    out
}

fn snap_scale(p: i32, pcs: &[u8]) -> i32 {
    if pcs.is_empty() {
        p
    } else {
        crate::harmony::snap_to_pitch_classes(p, pcs)
    }
}

fn phrase_lengths(bars: u32, rng: &mut Rng) -> Vec<f64> {
    let v: Vec<u32> = match bars {
        0..=3 => vec![bars.max(1)],
        4 => rng.pick(&[vec![4], vec![2, 2], vec![4]]).clone(),
        8 => rng
            .pick(&[vec![4, 4], vec![2, 2, 4], vec![4, 2, 2]])
            .clone(),
        16 => rng
            .pick(&[
                vec![4, 4, 4, 4],
                vec![2, 2, 4, 4, 4],
                vec![4, 4, 2, 2, 4],
                vec![4, 4, 4, 2, 2],
            ])
            .clone(),
        n => {
            let mut v = vec![4; (n / 4) as usize];
            match n % 4 {
                0 => {}
                1 => {
                    if let Some(l) = v.last_mut() {
                        *l += 1;
                    }
                }
                r => v.push(r),
            }
            v
        }
    };
    v.into_iter().map(|b| b as f64).collect()
}

fn labels_for(n: usize, rng: &mut Rng) -> Vec<String> {
    let pick: Vec<&str> = match n {
        1 => vec!["A"],
        2 => rng.pick(&[vec!["A", "A′"], vec!["A", "B"]]).clone(),
        3 => rng
            .pick(&[vec!["A", "A′", "B"], vec!["A", "B", "A′"]])
            .clone(),
        4 => rng
            .pick(&[
                vec!["A", "A′", "B", "A″"],
                vec!["A", "A′", "B", "C"],
                vec!["A", "B", "A′", "C"],
                vec!["A", "A′", "A″", "B"],
            ])
            .clone(),
        5 => rng
            .pick(&[
                vec!["A", "A′", "B", "B′", "A″"],
                vec!["A", "A′", "B", "A″", "C"],
            ])
            .clone(),
        _ => {
            let mut v = vec!["A", "A′", "B", "A″"];
            let more = ["C", "C′", "D", "A‴", "E", "E′"];
            for k in 0..n.saturating_sub(4) {
                v.push(more[k % more.len()]);
            }
            v
        }
    };
    pick.into_iter().map(str::to_owned).collect()
}

// ---------------------------------------------------------------- 骨格の書き方

const LENS: [(u64, &str); 10] = [
    (3840, "w"),
    (2880, "h."),
    (1920, "h"),
    (1440, "q."),
    (960, "q"),
    (720, "e."),
    (480, "e"),
    (360, "s."),
    (240, "s"),
    (120, "t"),
];

fn fmt_len(d: u64) -> String {
    LENS.iter()
        .find(|(t, _)| *t == d)
        .map_or_else(|| d.to_string(), |x| x.1.to_owned())
}

fn parse_len(s: &str) -> Option<u64> {
    s.parse::<u64>()
        .ok()
        .or_else(|| LENS.iter().find(|(_, n)| *n == s).map(|x| x.0))
}

/// 骨格("E5:h D5:q" の列。"r:q" は休み)を (句の最初の音からの tick, 音) に
pub fn parse_skeleton(items: &[String]) -> Result<Vec<(u64, u8)>, String> {
    let mut t = 0;
    let mut out = Vec::new();
    for it in items.iter().flat_map(|s| s.split_whitespace()) {
        let (n, l) = it
            .split_once(':')
            .ok_or_else(|| format!("骨格は 音名:長さ(例 E5:h)。読めない: {it}"))?;
        let d = parse_len(l).ok_or_else(|| format!("骨格の長さが読めない: {it}"))?;
        if n != "r" {
            out.push((
                t,
                parse_note(n).ok_or_else(|| format!("骨格の音名が読めない: {it}"))?,
            ));
        }
        t += d;
    }
    Ok(out)
}

fn format_skeleton(anchors: &[(u64, u8)], end: u64) -> Vec<String> {
    anchors
        .iter()
        .enumerate()
        .map(|(i, &(t, p))| {
            let next = anchors.get(i + 1).map_or(end, |x| x.0);
            format!(
                "{}:{}",
                note_name(p),
                fmt_len(next.saturating_sub(t).max(1))
            )
        })
        .collect()
}

// ---------------------------------------------------------------- 作る(L1〜L3)

pub struct RealizeInput<'a> {
    pub plan: &'a MelodyPlan,
    /// 作る区間の名前(None なら全部)
    pub only: Option<&'a [String]>,
    /// (小節の頭, 長さ) の列(`arrange::bar_grid`)。計画の最後の区間の終わりまで
    pub grid: &'a [(u64, u64)],
    /// その時の和音の構成音(ピッチクラス)
    pub chord_at: &'a dyn Fn(u64) -> Option<Vec<u8>>,
    pub key: Key,
    pub low: u8,
    pub high: u8,
    /// 区間の中の音域の幅(半音)の上限(ジャンルの目安)
    pub max_width: u8,
    pub seed: u64,
    /// 句と句の間の息継ぎ(tick)
    pub breath: u64,
    /// リズムの細かさ(tick。16 分 = 240、8 分 = 480)
    pub step: u64,
    /// 計画に骨格があっても作り直す
    pub regenerate_skeleton: bool,
}

/// 作った結果
#[derive(Clone, Debug, Default)]
pub struct Realized {
    pub notes: Vec<MelNote>,
    /// 句ごとの骨格。計画に書き戻すと、次に作るとき同じ骨格から始まる
    pub skeletons: Vec<PhraseSkeleton>,
    /// 作った区間の範囲(tick)
    pub ranges: Vec<(u64, u64)>,
}

#[derive(Clone, Debug)]
pub struct PhraseSkeleton {
    pub section: usize,
    pub phrase: usize,
    /// 句の最初の音(tick)
    pub first: u64,
    pub skeleton: Vec<String>,
}

/// 句の骨格とリズム(A′ が写す元)
#[derive(Clone)]
struct Made {
    /// 最初の音からの (tick, 音)
    anchors: Vec<(u64, u8)>,
    /// 骨格の音の間ごとの、間の頭からの打点
    fills: Vec<Vec<u64>>,
    /// 骨格の音の間ごとの、間の音の高さ(骨格の音からの音階の段の差)
    inner: Vec<Vec<i32>>,
    len: u64,
}

struct Scale {
    pcs: Vec<u8>,
}

impl Scale {
    fn new(key: Key) -> Scale {
        let mut pcs = crate::harmony::scale_pitch_classes(
            key.tonic,
            if key.minor { "minor" } else { "major" },
        );
        // 段の計算のため昇順に(主音からの並びではない)
        pcs.sort_unstable();
        Scale { pcs }
    }
    fn contains(&self, p: i32) -> bool {
        self.pcs.contains(&(p.rem_euclid(12) as u8))
    }
    /// 音階の段(0 = C-1 の上の最初の音階の音から数える)
    fn degree(&self, p: i32) -> i32 {
        let oct = p.div_euclid(12);
        let pc = p.rem_euclid(12) as u8;
        let i = self
            .pcs
            .iter()
            .position(|&x| x >= pc)
            .unwrap_or(self.pcs.len());
        oct * self.pcs.len() as i32 + i as i32
    }
    fn pitch(&self, d: i32) -> i32 {
        let n = self.pcs.len() as i32;
        d.div_euclid(n) * 12 + self.pcs[d.rem_euclid(n) as usize] as i32
    }
}

/// 区間の音域の軌跡: (小節位置, 中心, 幅)
fn register_points(sec: &SectionPlan) -> Vec<(f64, f64, f64)> {
    sec.register
        .iter()
        .filter_map(|r| {
            parse_note(&r.center).map(|c| (r.at, c as f64, r.span.unwrap_or(10) as f64))
        })
        .collect()
}

/// 区間の音域: 軌跡の中心 ± 幅の半分を、ジャンルの幅の上限に収める
fn section_range(reg: &[(f64, f64, f64)], inp: &RealizeInput) -> (u8, u8) {
    if reg.is_empty() {
        return (inp.low, inp.high);
    }
    let half = reg.iter().map(|r| r.2).fold(0.0, f64::max) / 2.0;
    let lo = reg.iter().map(|r| r.1).fold(f64::MAX, f64::min) - half;
    let hi = reg.iter().map(|r| r.1).fold(f64::MIN, f64::max) + half;
    let over = (hi - lo - inp.max_width as f64).max(0.0) / 2.0;
    let l = (lo + over).round().max(inp.low as f64);
    let h = (hi - over).round().min(inp.high as f64).max(l + 5.0);
    (l as u8, h as u8)
}

pub fn realize(inp: &RealizeInput) -> Result<Realized, String> {
    let mut rng = Rng::new(inp.seed);
    // 音度(1〜7)のピッチクラスは主音から数える
    let degrees = crate::harmony::scale_pitch_classes(
        inp.key.tonic,
        if inp.key.minor { "minor" } else { "major" },
    );
    let sorted = Scale::new(inp.key);
    let bar_tick = |bar: f64| -> u64 {
        let b = bar.floor().max(0.0) as usize;
        match inp.grid.get(b) {
            Some(&(s, l)) => s + ((bar - b as f64) * l as f64).round() as u64,
            None => inp.grid.last().map_or(0, |&(s, l)| {
                s + l + ((bar - inp.grid.len() as f64) * l as f64) as u64
            }),
        }
    };
    let bar_of = |t: u64| -> (u64, u64) {
        let i = inp.grid.partition_point(|g| g.0 <= t).saturating_sub(1);
        inp.grid.get(i).copied().unwrap_or((0, 4 * PPQ))
    };
    let mut out = Realized::default();
    let mut prev_pitch: Option<u8> = None;
    // 山の区間(盛り上がりが最大。同じなら後ろ)の最高音より、ほかの区間は低く(山で新しい高さを出す)
    let peak = inp
        .plan
        .sections
        .iter()
        .enumerate()
        .filter(|(_, s)| s.energy.is_some())
        .max_by(|a, b| {
            a.1.energy
                .unwrap_or(0.0)
                .total_cmp(&b.1.energy.unwrap_or(0.0))
                .then(a.0.cmp(&b.0))
        })
        .map(|(i, s)| (i, section_range(&register_points(s), inp).1));
    // 区間ごとの最初の音(次の区間が弱起で入るとき、前の句の終わりを早める)
    let section_first: Vec<u64> = inp
        .plan
        .sections
        .iter()
        .map(|sec| {
            let nominal = bar_tick((sec.start_bar - 1) as f64);
            let off = sec.phrases.first().map_or(0.0, |p| p.offset_beats);
            (nominal as i64 + (off * PPQ as f64).round() as i64).max(0) as u64
        })
        .collect();
    for (si, sec) in inp.plan.sections.iter().enumerate() {
        if inp.only.is_some_and(|o| !o.iter().any(|n| n == &sec.name)) {
            continue;
        }
        let s0 = bar_tick((sec.start_bar - 1) as f64);
        let s1 = bar_tick((sec.start_bar - 1 + sec.bars) as f64);
        out.ranges.push((s0, s1));
        let family = sec.rhythm_family.as_deref().unwrap_or("pulse");
        // 音域の軌跡(区間の中の小節位置 → 中心・幅)
        let reg = register_points(sec);
        let next_section_first = section_first.get(si + 1).copied().unwrap_or(u64::MAX);
        let mid = (inp.low as f64 + inp.high as f64) / 2.0;
        let (sec_low, mut sec_high) = section_range(&reg, inp);
        if let Some((pi, top)) = peak {
            if pi != si {
                sec_high = sec_high.min(top.saturating_sub(2)).max(sec_low + 5);
            }
        }
        let sec_inp = RealizeInput {
            low: sec_low,
            high: sec_high,
            ..*inp
        };
        let inp = &sec_inp;
        let center_at = |bar_pos: f64| -> (f64, f64) {
            if reg.is_empty() {
                return (mid, (inp.high - inp.low) as f64);
            }
            let k = reg.partition_point(|r| r.0 <= bar_pos);
            match (k.checked_sub(1).map(|i| reg[i]), reg.get(k)) {
                (Some(a), Some(b)) if b.0 > a.0 => {
                    let x = (bar_pos - a.0) / (b.0 - a.0);
                    (a.1 + (b.1 - a.1) * x, a.2 + (b.2 - a.2) * x)
                }
                (Some(a), _) => (a.1, a.2),
                (None, Some(b)) => (b.1, b.2),
                (None, None) => (mid, 10.0),
            }
        };
        // 句の時間の割り当て
        let n = sec.phrases.len();
        let mut starts = Vec::with_capacity(n);
        let mut at = 0.0;
        for p in &sec.phrases {
            let nominal = bar_tick((sec.start_bar - 1) as f64 + at);
            let first =
                (nominal as i64 + (p.offset_beats * PPQ as f64).round() as i64).max(0) as u64;
            starts.push((at, nominal, first));
            at += p.bars;
        }
        let mut made: Vec<(String, Made)> = Vec::new();
        for (k, p) in sec.phrases.iter().enumerate() {
            let (bar_at, _, first) = starts[k];
            let next_first = starts
                .get(k + 1)
                .map_or(s1.min(next_section_first), |x| x.2);
            let end = next_first.saturating_sub(inp.breath).max(first + PPQ / 2);
            if first >= s1 {
                break;
            }
            let len = end - first;
            let density = p
                .density
                .unwrap_or_else(|| sec.density.get(k).copied().unwrap_or(1.5));
            let (_, span) = center_at(bar_at);
            let reference = p
                .like
                .as_ref()
                .and_then(|l| {
                    made.iter().rev().find(|(lab, _)| {
                        lab.trim_end_matches(['′', '″', '‴']) == l.trim_end_matches(['′', '″', '‴'])
                    })
                })
                .map(|x| x.1.clone());
            let target = |x: f64| -> f64 {
                // 句の中でも音域の軌跡の点を通る(句の途中の山を落とさない)
                let (c, _) = center_at(bar_at + x * p.bars);
                let s = span.max(4.0);
                let shape = match p.contour.as_deref().unwrap_or("arch") {
                    "arch" => 0.35 * s * (std::f64::consts::PI * x).sin() - 0.15 * s,
                    "valley" => -(0.35 * s * (std::f64::consts::PI * x).sin() - 0.15 * s),
                    "rise" => 0.3 * s * (x - 0.5),
                    "fall" => -0.3 * s * (x - 0.5),
                    _ => 0.0,
                };
                c + shape
            };
            // ---- 骨格(L1)
            let given = if inp.regenerate_skeleton || p.skeleton.is_empty() {
                None
            } else {
                Some(parse_skeleton(&p.skeleton)?)
            };
            let ending_pc = p
                .ending_degree
                .and_then(|d| degrees.get(d.saturating_sub(1) as usize).copied())
                .unwrap_or(inp.key.tonic % 12);
            // 写した骨格の音の数(A′ で元の句と同じ所。表面の音も写す)
            let mut copied_upto = 0usize;
            let anchors: Vec<(u64, u8)> = match (given, &reference) {
                (Some(g), _) => g.into_iter().filter(|a| a.0 < len).collect(),
                (None, Some(r)) => {
                    let same = r.len == len;
                    let scale_t = |t: u64| -> u64 {
                        if same {
                            t
                        } else {
                            let x = (t as f64 * len as f64 / r.len.max(1) as f64 / (PPQ / 2) as f64)
                                .round() as u64;
                            x * (PPQ / 2)
                        }
                    };
                    // 元の骨格を音域の軌跡へずらす(音階の段で)
                    let rmean = r.anchors.iter().map(|a| a.1 as f64).sum::<f64>()
                        / r.anchors.len().max(1) as f64;
                    let want = (0..r.anchors.len())
                        .map(|i| target(i as f64 / r.anchors.len().max(1) as f64))
                        .sum::<f64>()
                        / r.anchors.len().max(1) as f64;
                    let shift_steps = if p.transform.iter().any(|t| t == "shift")
                        || (want - rmean).abs() >= 2.0
                    {
                        ((want - rmean) / 1.7).round() as i32
                    } else {
                        0
                    };
                    let mut v: Vec<(u64, u8)> = r
                        .anchors
                        .iter()
                        .map(|&(t, pch)| {
                            let d = sorted.degree(pch as i32) + shift_steps;
                            (
                                scale_t(t),
                                fit_range(sorted.pitch(d), inp.low, inp.high, &sorted),
                            )
                        })
                        .filter(|a| a.0 < len)
                        .collect();
                    v.dedup_by_key(|a| a.0);
                    // 終わりを作り直す(tail)か、終止の音に合わせる
                    let tail = if p.transform.iter().any(|t| t == "tail") {
                        (v.len() / 3).max(1)
                    } else {
                        1
                    };
                    let keep = v.len().saturating_sub(tail);
                    copied_upto = keep;
                    let fixed: Vec<(u64, u8)> = v[..keep].to_vec();
                    let times: Vec<u64> = v[keep..].iter().map(|a| a.0).collect();
                    let tail_p = choose_pitches(
                        &times,
                        first,
                        len,
                        fixed.last().map(|a| a.1).or(prev_pitch),
                        &target,
                        span,
                        ending_pc,
                        inp,
                        &sorted,
                        &bar_of,
                    );
                    fixed
                        .into_iter()
                        .chain(times.into_iter().zip(tail_p))
                        .collect()
                }
                (None, None) => {
                    let times = anchor_times(first, len, family, &bar_of, &mut rng);
                    let ps = choose_pitches(
                        &times, first, len, prev_pitch, &target, span, ending_pc, inp, &sorted,
                        &bar_of,
                    );
                    times.into_iter().zip(ps).collect()
                }
            };
            if anchors.is_empty() {
                continue;
            }
            // ---- リズム(L2)と表面(L3)
            // 句のリズムの動機: 2 拍の型を主と副の 2 つ選び、骨格の音の間ごとに主・主・副・主 … と使う
            let (main_cell, alt_cell) = pick_cells(family, density, inp.step, &mut rng);
            let mut fills: Vec<Vec<u64>> = Vec::new();
            let mut inners: Vec<Vec<i32>> = Vec::new();
            let mut phrase_notes: Vec<(u64, u8)> = Vec::new();
            for (i, &(t, pch)) in anchors.iter().enumerate() {
                phrase_notes.push((t, pch));
                let Some(&(t2, p2)) = anchors.get(i + 1) else {
                    fills.push(vec![]);
                    inners.push(vec![]);
                    break;
                };
                let gap = t2 - t;
                // 写す元の句で同じ長さの間なら、同じリズムを使う(A′ のリズムは A と同じ)
                let copied = reference.as_ref().and_then(|r| {
                    let (a, b) = (r.anchors.get(i)?, r.anchors.get(i + 1)?);
                    (b.0 - a.0 == gap)
                        .then(|| r.fills.get(i).cloned())
                        .flatten()
                });
                let offs = match copied {
                    Some(f) => f,
                    None => {
                        let cell = if i % 4 == 2 || (i % 4 == 3 && rng.unit() < 0.3) {
                            &alt_cell
                        } else {
                            &main_cell
                        };
                        fill_cells(gap, cell)
                    }
                };
                let came = phrase_notes
                    .iter()
                    .rev()
                    .nth(1)
                    .map(|x| pch as i32 - x.1 as i32)
                    .or_else(|| prev_pitch.filter(|_| i == 0).map(|q| pch as i32 - q as i32))
                    .unwrap_or(0);
                let reuse = reference
                    .as_ref()
                    .filter(|_| i + 1 < copied_upto)
                    .and_then(|r| r.inner.get(i))
                    .filter(|x| x.len() == offs.len())
                    .cloned();
                let inner = match reuse {
                    Some(steps) => {
                        let d0 = sorted.degree(pch as i32);
                        steps.iter().map(|&x| sorted.pitch(d0 + x)).collect()
                    }
                    None => surface(pch, p2, offs.len(), came, &sorted, &mut rng),
                };
                let d0 = sorted.degree(pch as i32);
                inners.push(inner.iter().map(|&q| sorted.degree(q) - d0).collect());
                for (o, q) in offs.iter().zip(inner) {
                    // 強拍の経過音は和音の音に寄せる
                    let abs = first + t + o;
                    let (bs, bl) = bar_of(abs);
                    let strong = (abs - bs) % (bl / 2).max(1) == 0;
                    let q = match (strong, (inp.chord_at)(abs)) {
                        (true, Some(pcs)) if !pcs.contains(&((q.rem_euclid(12)) as u8)) => {
                            let s = crate::harmony::snap_to_pitch_classes(q, &pcs);
                            if (s - q).abs() <= 2 {
                                s
                            } else {
                                q
                            }
                        }
                        _ => q,
                    };
                    phrase_notes.push((t + o, fit_range(q, inp.low, inp.high, &sorted)));
                }
                fills.push(offs);
            }
            phrase_notes.sort_by_key(|x| x.0);
            for (i, &(t, pch)) in phrase_notes.iter().enumerate() {
                let next = phrase_notes.get(i + 1).map_or(len, |x| x.0);
                let gap = next - t;
                let inner = i + 1 < phrase_notes.len();
                // 句の中の音の切り方: 伸ばす系統はつなげ、刻む系統は短く切って間を見せる
                let dur = match family {
                    "sparse" if inner => gap.min(PPQ / 2 + PPQ / 4).max(inp.step),
                    "sustain" => gap,
                    _ if inner => (gap - gap / 5).max(inp.step / 2),
                    _ => gap,
                };
                out.notes.push(MelNote {
                    pos: first + t,
                    dur: dur.max(inp.step / 2).min(gap.max(1)),
                    pitch: pch,
                });
            }
            prev_pitch = anchors.last().map(|a| a.1);
            out.skeletons.push(PhraseSkeleton {
                section: si,
                phrase: k,
                first,
                skeleton: format_skeleton(&anchors, len),
            });
            made.push((
                p.label.clone(),
                Made {
                    anchors,
                    fills,
                    inner: inners,
                    len,
                },
            ));
        }
    }
    out.notes.sort_by_key(|n| (n.pos, n.pitch));
    out.notes.dedup_by_key(|n| n.pos);
    Ok(out)
}

/// 骨格の音の位置: おおむね 2 拍ごと。強拍(小節の頭・半ば)に寄せ、シンコペーションの系統では 8 分前へ食う。
/// 最後の音(句の終わり)は長く伸ばせる位置
fn anchor_times(
    first: u64,
    len: u64,
    family: &str,
    bar_of: &dyn Fn(u64) -> (u64, u64),
    rng: &mut Rng,
) -> Vec<u64> {
    let n = ((len as f64 / (2 * PPQ) as f64).round() as usize).max(2);
    let ending = (len / 4).clamp(PPQ, 2 * PPQ).min(len / 2);
    let mut v = vec![0u64];
    for j in 1..n - 1 {
        let raw = first + (len - ending) * j as u64 / (n - 1) as u64;
        let (bs, bl) = bar_of(raw);
        let half = (bl / 2).max(1);
        let rel = raw - bs;
        // 近い強拍へ(1 拍以内)。それ以外は拍へ
        let snapped = {
            let h = (rel + half / 2) / half * half;
            if h.abs_diff(rel) <= PPQ {
                bs + h
            } else {
                bs + (rel + PPQ / 2) / PPQ * PPQ
            }
        };
        let mut t = snapped.saturating_sub(first);
        if family == "syncopated" && rng.unit() < 0.45 {
            t = t.saturating_sub(PPQ / 2);
        }
        v.push(t);
    }
    let last = {
        let raw = first + len - ending;
        let (bs, _) = bar_of(raw);
        (bs + (raw - bs + PPQ / 2) / PPQ * PPQ).saturating_sub(first)
    };
    v.push(last.min(len.saturating_sub(PPQ / 2)));
    v.sort_unstable();
    v.dedup();
    // 間が狭すぎる骨格の音は落とす(8 分未満)
    let mut w: Vec<u64> = Vec::with_capacity(v.len());
    for t in v {
        let room = match w.last() {
            None => true,
            Some(&l) => t >= l + PPQ / 2,
        };
        if room {
            w.push(t);
        }
    }
    w
}

/// 骨格の音の高さ: 目標の高さ(音域の軌跡 + 句の輪郭)・和音・音程の費用の動的計画法。最後の音は終止の音
#[allow(clippy::too_many_arguments)]
fn choose_pitches(
    times: &[u64],
    first: u64,
    len: u64,
    prev: Option<u8>,
    target: &dyn Fn(f64) -> f64,
    span: f64,
    ending_pc: u8,
    inp: &RealizeInput,
    scale: &Scale,
    bar_of: &dyn Fn(u64) -> (u64, u64),
) -> Vec<u8> {
    if times.is_empty() {
        return vec![];
    }
    let cands: Vec<i32> = (inp.low as i32..=inp.high as i32)
        .filter(|&p| scale.contains(p))
        .collect();
    if cands.is_empty() {
        return vec![inp.low; times.len()];
    }
    let half = (span / 2.5).max(2.0);
    let node = |j: usize, p: i32| -> f64 {
        let t = times[j];
        let abs = first + t;
        let x = t as f64 / len.max(1) as f64;
        let d = (p as f64 - target(x)) / half;
        let mut c = 3.5 * d * d;
        let (bs, bl) = bar_of(abs);
        let strong = (abs - bs) % (bl / 2).max(1) == 0;
        match (inp.chord_at)(abs) {
            Some(pcs) if pcs.contains(&(p.rem_euclid(12) as u8)) => {}
            Some(_) => c += if strong { 1.4 } else { 0.4 },
            None => {}
        }
        if j + 1 == times.len() && p.rem_euclid(12) as u8 != ending_pc {
            c += 1e6;
        }
        c
    };
    let edge = |a: i32, b: i32| -> f64 {
        match (a - b).abs() {
            0 => 0.9,
            1..=2 => 0.15,
            3..=4 => 0.3,
            5..=7 => 1.1,
            8..=12 => 2.6,
            _ => 1e5,
        }
    };
    let m = cands.len();
    let mut cost = vec![vec![f64::INFINITY; m]; times.len()];
    let mut back = vec![vec![0usize; m]; times.len()];
    for (i, &p) in cands.iter().enumerate() {
        cost[0][i] = node(0, p) + prev.map_or(0.0, |q| 0.6 * edge(q as i32, p));
    }
    for j in 1..times.len() {
        for (i, &p) in cands.iter().enumerate() {
            let nc = node(j, p);
            for (h, &q) in cands.iter().enumerate() {
                let mut second = 0.0;
                // 2 つ前の音(句の最初の骨格の音なら、前の句の最後の音)
                let before = if j >= 2 {
                    Some(cands[back[j - 1][h]])
                } else {
                    prev.map(|x| x as i32)
                };
                if let Some(pp) = before {
                    // 同じ音への往復(A–B–A)を嫌う
                    if pp == p && p != q {
                        second += 0.5;
                    }
                    // 跳躍(5 半音以上)の後は逆向きへ戻る(研究では約 7 割)
                    let (d1, d2) = (q - pp, p - q);
                    if d1.abs() >= 5 {
                        if d1.signum() == d2.signum() && d2 != 0 {
                            second += 0.9;
                        } else if d2 != 0 {
                            second -= 0.15;
                        }
                    }
                }
                let c = cost[j - 1][h] + edge(q, p) + nc + second;
                if c < cost[j][i] {
                    cost[j][i] = c;
                    back[j][i] = h;
                }
            }
        }
    }
    let last = times.len() - 1;
    let mut i = (0..m)
        .min_by(|&a, &b| cost[last][a].total_cmp(&cost[last][b]))
        .unwrap_or(0);
    let mut out = vec![0u8; times.len()];
    for j in (0..times.len()).rev() {
        out[j] = cands[i] as u8;
        i = back[j][i];
    }
    out
}

/// リズムの系統ごとの 2 拍の型(2 拍の頭からの 16 分の位置。頭は骨格の音)
fn cells(family: &str) -> &'static [&'static [u64]] {
    match family {
        "sustain" => &[&[], &[6], &[4], &[4, 6], &[5]],
        "sparse" => &[&[], &[2], &[4], &[6]],
        "syncopated" => &[
            &[2, 6],
            &[6],
            &[3, 6],
            &[2, 5],
            &[3, 5, 6],
            &[1, 3, 6],
            &[3, 6, 7],
            &[2, 3, 6],
        ],
        _ => &[
            &[2, 4, 6],
            &[4, 6],
            &[2, 4],
            &[2, 6],
            &[3, 4, 6],
            &[2, 4, 5, 6],
        ],
    }
}

/// 句の主と副の型: 密度(1 拍あたりの音の数)に近い音の数の型から選ぶ。8 分の細かさなら 16 分の位置は使わない
fn pick_cells(family: &str, density: f64, step: u64, rng: &mut Rng) -> (Vec<u64>, Vec<u64>) {
    let want = (density * 2.0 - 1.0).max(0.0);
    let all: Vec<&[u64]> = cells(family)
        .iter()
        .copied()
        .filter(|c| step <= 240 || c.iter().all(|x| x % 2 == 0))
        .collect();
    let mut scored: Vec<(f64, &[u64])> = all
        .iter()
        .map(|c| ((c.len() as f64 - want).abs() + 0.6 * rng.unit(), *c))
        .collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    let to = |c: &[u64]| c.iter().map(|x| x * 240).collect::<Vec<u64>>();
    let main = scored.first().map_or_else(Vec::new, |x| to(x.1));
    let alt = scored.get(1).map_or_else(|| main.clone(), |x| to(x.1));
    (main, alt)
}

/// 骨格の 2 音の間(`gap` tick)に型を敷く(2 拍ごとに繰り返し、次の骨格の音の 16 分手前まで)
fn fill_cells(gap: u64, cell: &[u64]) -> Vec<u64> {
    let mut v = Vec::new();
    let mut w = 0;
    while w < gap {
        if w > 0 && w + 240 <= gap {
            v.push(w);
        }
        for &o in cell {
            if w + o + 120 < gap {
                v.push(w + o);
            }
        }
        w += 2 * PPQ;
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// 骨格の 2 音の間の音の高さ(k 個): 経過音・刺繍音
/// 音域に収め(外ならオクターブで折り返さずに端の音階の音へ)、音階の音にする
fn fit_range(p: i32, low: u8, high: u8, scale: &Scale) -> u8 {
    let mut q = p.clamp(low as i32, high as i32);
    while !scale.contains(q) && q > low as i32 {
        q -= 1;
    }
    while !scale.contains(q) && q < high as i32 {
        q += 1;
    }
    q.clamp(0, 127) as u8
}

/// `came` は骨格の音 `a` へ来た音程(前の音から。跳躍なら最初の音は逆向きへ戻す)
fn surface(a: u8, b: u8, k: usize, came: i32, scale: &Scale, rng: &mut Rng) -> Vec<i32> {
    if k == 0 {
        return vec![];
    }
    let da = scale.degree(a as i32);
    let db = scale.degree(b as i32);
    let d = db - da;
    let mut out = Vec::with_capacity(k);
    let mut prev = da;
    let dir0 = if rng.unit() < 0.5 { 1 } else { -1 };
    for i in 1..=k {
        let mut x = da + ((d as f64) * i as f64 / (k + 1) as f64).round() as i32;
        if i == 1 && came.abs() >= 5 {
            // 跳躍の後は逆向きへ 1 段
            x = da - came.signum();
        } else if k == 1 && x == db && d.abs() == 1 && rng.unit() < 0.3 {
            // 次の骨格の音の先取りばかりにしない: 逸音(離れる向き)か、越えてから戻る
            x = if rng.unit() < 0.5 { da - d } else { db + d };
        } else if x == prev && rng.unit() < 0.7 {
            // 同じ音の連打(刻み)
        } else if x == prev {
            // 目的の音から離れる向きの刺繍音
            let away = if d > 0 {
                -1
            } else if d < 0 {
                1
            } else if i % 2 == 1 {
                dir0
            } else {
                -dir0
            };
            x = prev + away;
        }
        out.push(scale.pitch(x));
        prev = x;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grid(bars: usize) -> Vec<(u64, u64)> {
        (0..bars).map(|b| (b as u64 * 3840, 3840)).collect()
    }

    fn plan_for(seed: u64) -> MelodyPlan {
        let sections = propose(
            &[
                SectionSpec {
                    name: "Break".into(),
                    start_bar: 1,
                    bars: 8,
                    energy: Some(3.0),
                },
                SectionSpec {
                    name: "Drop".into(),
                    start_bar: 9,
                    bars: 16,
                    energy: Some(9.0),
                },
            ],
            69,
            86,
            false,
            Key::parse("A minor"),
            seed,
        );
        MelodyPlan {
            sections,
            ..Default::default()
        }
    }

    fn realize_plan(plan: &MelodyPlan, seed: u64) -> Realized {
        let g = grid(24);
        let names = ["Am", "F", "C", "G"];
        let f = |t: u64| {
            crate::chord::parse(names[((t / 3840) % 4) as usize])
                .ok()
                .flatten()
                .map(|c| c.pitch_classes())
        };
        realize(&RealizeInput {
            plan,
            only: None,
            grid: &g,
            chord_at: &f,
            key: Key::parse("A minor").unwrap(),
            low: 69,
            high: 86,
            max_width: 14,
            seed,
            breath: PPQ / 2,
            step: 240,
            regenerate_skeleton: false,
        })
        .unwrap()
    }

    #[test]
    fn proposals_are_valid_and_vary_the_shape() {
        for seed in 0..20 {
            let plan = plan_for(seed);
            plan.validate().unwrap();
            let drop = &plan.sections[1];
            let brk = &plan.sections[0];
            // 句の長さか入りが揃いすぎない(4 句以上なら)
            if drop.phrases.len() >= 3 {
                let same = drop
                    .phrases
                    .windows(2)
                    .all(|w| w[0].bars == w[1].bars && w[0].offset_beats == w[1].offset_beats);
                assert!(!same, "seed {seed}: {:?}", drop.phrases);
            }
            // 区間どうしで音域の中心とリズムの系統が違う
            let c = |s: &SectionPlan| parse_note(&s.register[0].center).unwrap() as i32;
            assert!(c(drop) != c(brk) || drop.register.len() != brk.register.len());
            assert_ne!(drop.rhythm_family, brk.rhythm_family, "seed {seed}");
            assert_eq!(drop.phrases.last().unwrap().ending_degree, Some(1));
        }
    }

    #[test]
    fn a_realized_plan_moves_in_register_breathes_and_repeats_a_prime() {
        let mut warned = 0;
        for seed in 0..12 {
            let plan = plan_for(seed);
            let r = realize_plan(&plan, seed);
            assert!(!r.notes.is_empty());
            // 音域の外へ大きく出ない
            assert!(
                r.notes.iter().all(|n| (66..=89).contains(&n.pitch)),
                "seed {seed}"
            );
            // 句の終わり(区間の最後)は主音
            let drop_end = r
                .notes
                .iter()
                .filter(|n| n.pos < 24 * 3840)
                .max_by_key(|n| n.pos)
                .unwrap();
            assert_eq!(drop_end.pitch % 12, 9, "seed {seed}");
            // 分析にかけて、区間の粒度の warn(同じリズムの輪郭・音域が動かない)が出ないことが多い
            let p = crate::model::Project::new("t");
            let f = |_t: u64| None;
            let ctx = crate::melody::Context {
                project: &p,
                chord_at: &f,
                key: Key::parse("A minor"),
                genre: crate::melody::genre("edm").unwrap(),
                sections: plan
                    .sections
                    .iter()
                    .map(|s| crate::melody::SectionSpan {
                        name: s.name.clone(),
                        start: (s.start_bar as u64 - 1) * 3840,
                        end: (s.start_bar as u64 - 1 + s.bars as u64) * 3840,
                        energy: s.energy,
                    })
                    .collect(),
                target: "Lead".into(),
            };
            let st = crate::melstruct::analyze(&r.notes, &ctx);
            if st
                .findings
                .iter()
                .any(|f| f.severity == "warn" && f.what.starts_with("[区間]"))
            {
                warned += 1;
            }
            // 骨格は計画に書き戻せる形
            for sk in &r.skeletons {
                parse_skeleton(&sk.skeleton).unwrap();
            }
        }
        assert!(warned <= 3, "区間の warn が {warned}/12 案");
    }

    #[test]
    fn the_same_plan_and_seed_give_the_same_notes_and_a_written_back_skeleton_is_kept() {
        let mut plan = plan_for(4);
        let a = realize_plan(&plan, 4);
        let b = realize_plan(&plan, 4);
        assert_eq!(a.notes, b.notes);
        // 骨格を書き戻して別の seed で作ると、骨格の音は同じ位置に残る(リズムと表面だけ変わる)
        for sk in &a.skeletons {
            plan.sections[sk.section].phrases[sk.phrase].skeleton = sk.skeleton.clone();
        }
        let c = realize_plan(&plan, 99);
        assert_ne!(a.notes, c.notes);
        for sk in &a.skeletons {
            for (t, p) in parse_skeleton(&sk.skeleton).unwrap() {
                assert!(
                    c.notes
                        .iter()
                        .any(|n| n.pos == sk.first + t && n.pitch == p),
                    "{} {:?}",
                    sk.first + t,
                    p
                );
            }
        }
    }
}
