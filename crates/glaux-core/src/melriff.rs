//! 電子メロディー(リフ)を計画から作る(計画の style が riff のとき)。
//!
//! ハウス・EDM のリードは歌ではない。短いリフ(1〜2 小節)を和音に合わせて繰り返し、句の終わり(4・8 回目)だけ変える。
//! 旋律が変わらない分、リズム(休み・食い・アクセント)がフックになる(docs の調査: Butler 2006、Burns 1987)。
//! Testv2_rev3 で「歌の旋律として作っている」「似たテンションで延々とあっちこっちへ動く」「時折入る不自然な拍からの音」
//! と言われたことから、歌の作り方(骨格を経過音でつなぐ)とは別に置く。
//!
//! - リズム: リフの 16 分の格子をそのまま繰り返す(毎回同じ位置。拍の外れた音を散らさない)
//! - 高さ: キーのペンタトニックとその時の和音の音の梯子の上で、基準の音(和音の音)から形(段)をなぞる。
//!   頭・拍の頭・アクセント・伸ばす音・最後の音は和音の音にそろえる。和音が変われば同じ形で移る
//!   (follow: chord)。fixed なら最初の和音の高さのまま
//! - 句の変え方: tail(最後の 1 回の終わり)/ fill(最後の 1 拍を 16 分で埋める)/ rise(最後の 1 回を 1 段上から)/
//!   octave / sparse(強い拍だけ残して伸ばす。予告)/ shift(全部を 1 段上)

use crate::chord::parse_note;
use crate::melody::MelNote;
use crate::melplan::{PhraseSkeleton, RealizeInput, Realized};
use crate::plan::RiffPlan;
use crate::time::PPQ;

const STEP: u64 = PPQ / 4;

/// リフの 1 音(リフの頭からの位置・長さ・アクセント・何番目の音の頭か)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RiffHit {
    pub pos: u64,
    pub dur: u64,
    pub accent: bool,
    pub index: usize,
}

/// リズムの文字列を読む。(音の列, リフの長さ tick)
pub fn parse_rhythm(s: &str) -> Result<(Vec<RiffHit>, u64), String> {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .collect();
    if chars.is_empty() || chars.len() > 64 {
        return Err("rhythm は x X - . の 1〜64 文字(16 分 1 つが 1 文字)".to_owned());
    }
    let mut out: Vec<RiffHit> = Vec::new();
    let mut sounding = false;
    for (i, ch) in chars.iter().enumerate() {
        let t = i as u64 * STEP;
        match ch {
            'x' | 'X' => {
                out.push(RiffHit {
                    pos: t,
                    dur: STEP,
                    accent: *ch == 'X',
                    index: out.len(),
                });
                sounding = true;
            }
            '-' => {
                if sounding {
                    if let Some(l) = out.last_mut() {
                        l.dur += STEP;
                    }
                }
            }
            '.' => sounding = false,
            _ => return Err(format!("rhythm に使えない文字「{ch}」(x X - . だけ)")),
        }
    }
    if out.is_empty() {
        return Err("rhythm に音の頭(x)がありません".to_owned());
    }
    Ok((out, chars.len() as u64 * STEP))
}

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 和音の音の梯子(`low`〜`high` の中で、ピッチクラスが `pcs` の音)
fn ladder(pcs: &[u8], low: i32, high: i32) -> Vec<i32> {
    (low..=high)
        .filter(|p| pcs.contains(&(p.rem_euclid(12) as u8)))
        .collect()
}

fn nearest_index(l: &[i32], p: f64) -> usize {
    l.iter()
        .enumerate()
        .min_by(|a, b| (*a.1 as f64 - p).abs().total_cmp(&(*b.1 as f64 - p).abs()))
        .map_or(0, |x| x.0)
}

/// 計画の riff から音を作る。`Realized.accents` にアクセントの音の位置、`statements` にリフの 1 回ごとの頭を入れる
pub fn realize_riff(inp: &RealizeInput) -> Result<Realized, String> {
    let plan = inp.plan;
    if plan.riffs.is_empty() {
        return Err("計画に riffs がありません(style: riff には riffs が要ります)".to_owned());
    }
    let tonic_triad: Vec<u8> = {
        let s = crate::harmony::scale_pitch_classes(
            inp.key.tonic,
            if inp.key.minor { "minor" } else { "major" },
        );
        vec![s[0], s[2], s[4]]
    };
    let bar_tick = |bar: f64| -> u64 {
        let b = bar.floor().max(0.0) as usize;
        match inp.grid.get(b) {
            Some(&(s, l)) => s + ((bar - b as f64) * l as f64).round() as u64,
            None => inp.grid.last().map_or(0, |&(s, l)| {
                s + l + ((bar - inp.grid.len() as f64) * l as f64) as u64
            }),
        }
    };
    // 音の集合: キーのペンタトニック + その時の和音の音(調査: ペンタトニックと共通音で組み、ダイアトニック 100%)
    let penta: Vec<u8> = {
        let t = inp.key.tonic % 12;
        let steps: &[u8] = if inp.key.minor {
            &[0, 3, 5, 7, 10]
        } else {
            &[0, 2, 4, 7, 9]
        };
        steps.iter().map(|x| (t + x) % 12).collect()
    };
    let mut out = Realized::default();
    // 曲の山の区間(盛り上がりが最大、同じなら後ろ)。その最後の句の変える小節で、それまでの最高音より上の山を出す
    let peak_sec = plan
        .sections
        .iter()
        .enumerate()
        .max_by(|a, b| {
            a.1.energy
                .unwrap_or(5.0)
                .total_cmp(&b.1.energy.unwrap_or(5.0))
                .then(a.0.cmp(&b.0))
        })
        .map(|x| x.0);
    let mut max_before: i32 = 0;
    for (si, sec) in plan.sections.iter().enumerate() {
        if inp.only.is_some_and(|o| !o.iter().any(|n| n == &sec.name)) {
            continue;
        }
        let s0 = bar_tick((sec.start_bar - 1) as f64);
        let s1 = bar_tick((sec.start_bar - 1 + sec.bars) as f64);
        out.ranges.push((s0, s1));
        let reg: Vec<(f64, f64)> = sec
            .register
            .iter()
            .filter_map(|r| parse_note(&r.center).map(|c| (r.at, c as f64)))
            .collect();
        let center_at = |bar_pos: f64| -> f64 {
            if reg.is_empty() {
                return (inp.low as f64 + inp.high as f64) / 2.0;
            }
            let k = reg.partition_point(|r| r.0 <= bar_pos);
            match (k.checked_sub(1).map(|i| reg[i]), reg.get(k)) {
                (Some(a), Some(b)) if b.0 > a.0 => {
                    a.1 + (b.1 - a.1) * (bar_pos - a.0) / (b.0 - a.0)
                }
                (Some(a), _) => a.1,
                (None, Some(b)) => b.1,
                (None, None) => (inp.low as f64 + inp.high as f64) / 2.0,
            }
        };
        let mut at = 0.0;
        for (k, ph) in sec.phrases.iter().enumerate() {
            let bar_at = at;
            at += ph.bars;
            let nominal = bar_tick((sec.start_bar - 1) as f64 + bar_at);
            let p_start =
                (nominal as i64 + (ph.offset_beats * PPQ as f64).round() as i64).max(0) as u64;
            let p_end = bar_tick((sec.start_bar - 1) as f64 + at).min(s1);
            if p_start >= s1 || p_end <= p_start {
                continue;
            }
            let base = ph.label.trim_end_matches(['′', '″', '‴']);
            // 休みの句(リードが抜ける所。メリハリ)
            if base == "-" || base == "rest" {
                continue;
            }
            let riff: &RiffPlan = plan
                .riffs
                .iter()
                .find(|r| r.name == base)
                .or_else(|| {
                    ph.like
                        .as_ref()
                        .and_then(|l| plan.riffs.iter().find(|r| &r.name == l))
                })
                .unwrap_or(&plan.riffs[0]);
            let (hits, rlen) = parse_rhythm(&riff.rhythm)?;
            if riff.shape.len() != hits.len() {
                return Err(format!(
                    "リフ {} の shape の数が rhythm の音の頭の数と違います",
                    riff.name
                ));
            }
            let has = |t: &str| ph.transform.iter().any(|x| x == t);
            // オクターブ上は音域の上限を超えるなら使わない(代わりに 1 段上げる)
            let octave_wanted = has("octave");
            let reps = ((p_end - p_start) as f64 / rlen as f64).round().max(1.0) as u64;
            // 句ごとの変え方の選び方(2 つ前の句と同じ変え方にしない)
            let rnd = mix(inp.seed ^ ((si as u64) << 32)).wrapping_add(k as u64);
            let fixed = riff.follow.as_deref() == Some("fixed");
            let mut fixed_pitches: Option<Vec<i32>> = None;
            let mut phrase_first = None;
            for r in 0..reps {
                let st = p_start + r * rlen;
                if st >= p_end {
                    break;
                }
                let last = r + 1 == reps;
                out.statements.push(st);
                let anchor = riff
                    .anchor
                    .as_deref()
                    .and_then(parse_note)
                    .map(|a| a as f64)
                    .unwrap_or_else(|| center_at(bar_at + (st - p_start) as f64 / 3840.0));
                let mut shift = if has("shift") { 1 } else { 0 };
                // オクターブ上は音域の上限か E6 を超えるなら使わない(甲高くなる)
                let octave = if octave_wanted && anchor + 12.0 <= (inp.high as f64 + 2.0).min(88.0)
                {
                    12
                } else {
                    if octave_wanted {
                        shift += 1;
                    }
                    0
                };
                if last && has("rise") {
                    shift += 1;
                }
                let mut notes: Vec<(u64, u64, i32, bool)> = Vec::new();
                // 経過音(和音以外の音)は 1 小節に 1 つまで(和音の音を 7〜9 割に)
                let mut passing_left = riff.bars as usize;
                for h in &hits {
                    let t = st + h.pos;
                    if t >= p_end {
                        break;
                    }
                    let pcs = (inp.chord_at)(t).unwrap_or_else(|| tonic_triad.clone());
                    let mut set = penta.clone();
                    set.extend(pcs.iter().copied());
                    let l = ladder(&set, inp.low as i32 - 7, inp.high as i32 + 7);
                    let chord_l = ladder(&pcs, inp.low as i32 - 7, inp.high as i32 + 7);
                    if l.is_empty() || chord_l.is_empty() {
                        continue;
                    }
                    // 基準は和音の音(梯子の上で、基準の音に最も近い和音の音)
                    let a0 = chord_l[nearest_index(&chord_l, anchor)];
                    let i0 = l.iter().position(|&x| x == a0).unwrap_or(0) as i32;
                    let idx =
                        (i0 + riff.shape[h.index] + shift).clamp(0, l.len() as i32 - 1) as usize;
                    let mut p = l[idx];
                    // 頭の音・伸ばす音・最後の音は和音の音にそろえる(調査: 各小節の最初の打点と伸ばす音は構成音)
                    // 拍の頭・アクセントの音も(和音の音の割合を 7 割以上に。経過のペンタトニックは裏の 16 分だけ)
                    let strong = h.index == 0
                        || h.dur > STEP
                        || h.index + 1 == hits.len()
                        || h.pos % PPQ == 0
                        || h.accent;
                    if !pcs.contains(&(p.rem_euclid(12) as u8)) {
                        if strong || passing_left == 0 {
                            p = chord_l[nearest_index(&chord_l, p as f64)];
                        } else {
                            passing_left -= 1;
                        }
                    }
                    if fixed {
                        let fp = fixed_pitches.get_or_insert_with(Vec::new);
                        if fp.len() == hits.len() {
                            p = fp[h.index];
                        } else {
                            fp.push(p);
                        }
                    }
                    notes.push((t, h.dur, p + octave, h.accent));
                }
                // 最後の 1 回の変え方
                if last && has("tail") && notes.len() >= 2 {
                    // 4 回目だけ変える(調査: 最後の 1〜2 打点・音を 2 つに割る・新しい最高音はこの小節で 1 回)
                    let n = notes.len();
                    let climax = Some(si) == peak_sec && k + 1 == sec.phrases.len();
                    let top = notes.iter().map(|x| x.2).max().unwrap_or(0).max(if climax {
                        max_before
                    } else {
                        0
                    });
                    // 新しい山は、その時の音の集合(ペンタトニック + 和音の音)で top より上の次の音
                    let above = |t: u64| -> i32 {
                        let mut set = penta.clone();
                        set.extend((inp.chord_at)(t).unwrap_or_else(|| tonic_triad.clone()));
                        ladder(&set, top + 1, top + 12)
                            .first()
                            .copied()
                            .unwrap_or(top)
                    };
                    match if climax { 0 } else { rnd % 3 } {
                        0 => {
                            // 終わりから 2 番目で新しい山へ、最後で戻る
                            notes[n - 2].2 = above(notes[n - 2].0);
                            notes[n - 2].3 = true;
                        }
                        1 => {
                            // スタッター: 最後の音を 16 分 2 つに割り、2 つ目を上へ
                            let (t, d, p, a) = notes[n - 1];
                            if d >= 2 * STEP || (st + rlen).saturating_sub(t) >= 2 * STEP {
                                notes[n - 1] = (t, STEP, p, a);
                                notes.push((t + STEP, STEP, above(t + STEP), true));
                            } else {
                                notes[n - 1].2 = above(t);
                            }
                        }
                        _ => {
                            // 最後の 2 音を 1 つの長い音に(着地)
                            let (t, _, p, a) = notes[n - 2];
                            let end = (notes[n - 1].0 + notes[n - 1].1).max(t + 2 * STEP);
                            notes.truncate(n - 2);
                            notes.push((t, end - t, p, a));
                        }
                    }
                }
                if last && has("fill") {
                    // 最後の 1 拍を 16 分で埋め、次のリフの頭へ向かう
                    let beat = (st + rlen).min(p_end).saturating_sub(PPQ);
                    notes.retain(|n| n.0 < beat);
                    let from = notes.last().map_or(anchor.round() as i32, |n| n.2);
                    let target = phrase_first.unwrap_or(from);
                    // ペンタトニック + その拍の和音の音(F・B などは和音の音のときだけ)
                    let mut set = penta.clone();
                    set.extend((inp.chord_at)(beat).unwrap_or_else(|| tonic_triad.clone()));
                    let sl = ladder(&set, inp.low as i32 - 7, inp.high as i32 + 12);
                    let ti = nearest_index(&sl, target as f64) as i32;
                    for j in 0..4 {
                        let dir = if target >= from { -1 } else { 1 };
                        let idx = (ti + dir * (4 - j)).clamp(0, sl.len() as i32 - 1) as usize;
                        notes.push((beat + j as u64 * STEP, STEP, sl[idx], j == 0));
                    }
                }
                if has("sparse") {
                    // 予告(調査: ブレイクではプラックの線の 1・3 拍目にかかる打点だけ残す)
                    let mut kept: Vec<(u64, u64, i32, bool)> = Vec::new();
                    let mut beat = 0;
                    while beat < rlen {
                        if let Some(n) = notes
                            .iter()
                            .filter(|n| (n.0 - st).abs_diff(beat) <= 2 * STEP)
                            .min_by_key(|n| (n.0 - st).abs_diff(beat))
                        {
                            if !kept.contains(n) {
                                kept.push(*n);
                            }
                        }
                        beat += 2 * PPQ;
                    }
                    notes = kept;
                }
                if last && has("stop") {
                    // 次の区間(ドロップ)の直前の 1 拍はリードを止める
                    let cut = (st + rlen).min(p_end).saturating_sub(PPQ);
                    notes.retain(|n| n.0 < cut);
                }
                if phrase_first.is_none() {
                    phrase_first = notes.first().map(|n| n.2);
                }
                // 句の最後の音は 2〜4 マス伸ばす(調査: 句末の 1 音だけ伸ばす。fill・stop・予告では伸ばさない)
                if last && !has("fill") && !has("stop") && !has("sparse") {
                    if let Some(l) = notes.last_mut() {
                        let room = (st + rlen).min(p_end).saturating_sub(l.0);
                        l.1 = l.1.max((3 * STEP).min(room));
                    }
                }
                for (t, d, p, a) in notes {
                    // 16 分 1 つの音は短く切る(撥ねる)、伸ばす音はほぼいっぱい
                    // 16 分 1 つの音はゲート 55%(プラック)、伸ばす音はほぼいっぱい(調査: プラックは 45〜60%)
                    let gate = if d <= STEP {
                        d * 55 / 100
                    } else {
                        d - STEP / 4
                    };
                    out.notes.push(MelNote {
                        pos: t,
                        dur: gate.max(STEP / 2).min(p_end - t),
                        pitch: (p.clamp(inp.low as i32 - 5, inp.high as i32 + 3)) as u8,
                    });
                    if a {
                        out.accents.push(t);
                    }
                    if Some(si) != peak_sec {
                        max_before = max_before.max(p);
                    }
                }
            }
            out.skeletons.push(PhraseSkeleton {
                section: si,
                phrase: k,
                first: p_start,
                len: p_end - p_start,
                skeleton: vec![],
            });
        }
    }
    out.notes.sort_by_key(|n| (n.pos, n.pitch));
    out.notes.dedup_by_key(|n| n.pos);
    Ok(out)
}

// ---------------------------------------------------------------- 提案

/// リズムの型(1 小節 = 16 分 16 個)。X は裏のアクセント。調査の規則: 16 分の格子だけ、1 小節に 4〜7 音、
/// キック(0・4・8・12 マス)と重なる音は 2 つまで、1 拍目の頭は空けるか食う、伸ばす音は 1 つまで
const RIFF_RHYTHMS_1: &[&str] = &[
    "..X..x..x..X-.x.", // E(5,16) を 2 マス回した型(1 音を伸ばす)
    "x..X..x.x..X--x.", // 3-3-2 を 2 回(トレシーロ。1 音を伸ばす)
    "..X...x.x..X--..", // 裏から入り、伸ばす音を 1 つ
    ".x..X-.x..X..x..", // 16 分の裏から 3 つおき
    "...X..x...X..x-.", // キックを全部避ける
    "x..X..X..x..X---", // 3 つおきで終わりを伸ばす
    "..x.X-.x..x.X..x", // 次の小節の頭へ食う
    ".X.x..x-..X.x...", // 16 分の裏から
];

const RIFF_RHYTHMS_2: &[&str] = &[
    "..X..x..x..X..x.|..X..x..X-------",
    "x..X..x.x..X..x.|x..X..x.X-------",
    ".x..X..x..X..x..|.x..X..x..X-----",
];

fn onsets(r: &str) -> usize {
    r.chars().filter(|c| *c == 'x' || *c == 'X').count()
}

/// リフを 1 つ作る。`density` は 1 拍あたりの音の数の目安、`bars` は 1 か 2
pub fn make_riff(name: &str, bars: u32, density: f64, seed: u64) -> RiffPlan {
    let lib: &[&str] = if bars >= 2 {
        RIFF_RHYTHMS_2
    } else {
        RIFF_RHYTHMS_1
    };
    let want = density * 4.0 * bars as f64;
    let mut cands: Vec<(f64, &str)> = lib
        .iter()
        .map(|r| {
            let d = (onsets(r) as f64 - want).abs();
            (
                d + (mix(seed ^ r.len() as u64 ^ onsets(r) as u64) % 1000) as f64 / 700.0,
                *r,
            )
        })
        .collect();
    cands.sort_by(|a, b| a.0.total_cmp(&b.0));
    let rhythm = cands[0].1.to_owned();
    let n = onsets(&rhythm);
    // 形(ペンタトニック + 和音の音の梯子の段): 調査の音程の配分(同音 15〜35%・順次 30〜50%・3〜7 半音 20〜35%・
    // オクターブ 0〜10%)に寄せる。跳躍はリフに入れない(毎小節繰り返すと多すぎる。新しい山は変える小節で 1 回)。
    // 動機の中の幅は 5 段まで(おおむね 5〜9 半音)
    let mut shape: Vec<i32> = Vec::with_capacity(n);
    let mut cur = 0i32;
    for j in 0..n {
        if j == 0 {
            shape.push(0);
            continue;
        }
        let r = mix(seed.wrapping_add(j as u64 * 0x9E37)) % 100;
        let step: i32 = match r {
            0..=29 => 0,
            30..=52 => 1,
            53..=73 => -1,
            74..=83 => 2,
            _ => -2,
        };
        let mut next = cur + step;
        if !(-2..=3).contains(&next) {
            next = cur - step.signum();
        }
        // 行って戻るだけ(A–B–A)が続かないよう、2 つ前と同じ段に戻るなら同じ音の連打にする
        if j >= 2 && next == shape[j - 2] && next != cur && r % 2 == 0 {
            next = cur;
        }
        cur = next;
        shape.push(cur);
    }
    // 形を基準の音の周りにそろえる(リフどうしで高さがずれないように)
    let mean = (shape.iter().sum::<i32>() as f64 / n.max(1) as f64).round() as i32;
    for x in &mut shape {
        *x -= mean;
    }
    RiffPlan {
        name: name.to_owned(),
        bars,
        rhythm,
        shape,
        anchor: None,
        follow: None,
    }
}

/// 区間の並び(riff): 盛り上がりに応じて句(4 小節のまとまり)の名前と変え方を決める。
/// 盛り上がる区間: A → A′(tail)→ B → A″(rise・fill)。静かな区間: 前半は休み、後半に A を間引いて予告(sparse)。
/// 同じ系統の名前の後の区間(ドロップ2)は前の区間の並びを引き継ぎ、後半をオクターブ上に
pub fn riff_phrases(sections: &mut [crate::plan::SectionPlan], key: Option<crate::chord::Key>) {
    use crate::plan::PhrasePlan;
    let stem = |n: &str| -> String {
        n.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace())
            .to_owned()
    };
    for si in 0..sections.len() {
        let e = sections[si].energy.unwrap_or(5.0);
        let bars = sections[si].bars as f64;
        let groups = (bars / 4.0).floor().max(1.0) as usize;
        let glen = bars / groups as f64;
        let mk = |label: &str, t: &[&str]| PhrasePlan {
            label: label.to_owned(),
            bars: glen,
            transform: t.iter().map(|x| x.to_string()).collect(),
            ..Default::default()
        };
        let prev_same = (0..si).rev().find(|&j| {
            stem(&sections[j].name) == stem(&sections[si].name)
                && sections[j].bars == sections[si].bars
        });
        let phrases: Vec<PhrasePlan> = if let Some(j) = prev_same {
            // ドロップ2: 前半は同じリフ(オクターブ上の層が入れば上へ)、後半は新しいリフ B(調査: 移調だけにしない)
            let mut v = sections[j].phrases.clone();
            let n = v.len();
            for (k, p) in v.iter_mut().enumerate() {
                if k >= n / 2 {
                    p.label = if k + 1 == n {
                        "B′".to_owned()
                    } else {
                        "B".to_owned()
                    };
                    if k + 1 == n && !p.transform.iter().any(|x| x == "rise") {
                        p.transform.push("rise".to_owned());
                    }
                } else if !p.transform.iter().any(|x| x == "octave") {
                    p.transform.push("octave".to_owned());
                }
            }
            sections[si].like = Some(sections[j].name.clone());
            v
        } else if e >= 7.0 {
            // ドロップ: 同じリフ。4 小節目を変え(tail)、8 小節目はさらに大きく(fill)
            // 後半の 8 小節はリフを 1 段上げる(持ち上げ)
            (0..groups)
                .map(|k| match (k % 2 == 0, k >= groups / 2 && groups >= 4) {
                    (true, false) => mk("A", &["tail"]),
                    (false, false) => mk("A′", &["fill"]),
                    (true, true) => mk("A″", &["shift", "tail"]),
                    (false, true) => mk("A‴", &["shift", "fill"]),
                })
                .collect()
        } else if e >= 4.0 {
            // ブレイク: 前半は休み、後半で 1・3 拍目だけの予告。最後の 1 拍は止める
            (0..groups)
                .map(|k| {
                    if k < groups / 2 {
                        mk("-", &[])
                    } else if k + 1 == groups {
                        mk("A′", &["sparse", "stop"])
                    } else {
                        mk("A", &["sparse"])
                    }
                })
                .collect()
        } else {
            (0..groups).map(|_| mk("-", &[])).collect()
        };
        sections[si].phrases = phrases;
        sections[si].density.clear();
        sections[si].rhythm_family = None;
        // リフは同じ高さで繰り返す(盛り上がりはアレンジ・リフの差し替え・オクターブで出す)。音域の軌跡は平らにする
        let cs: Vec<f64> = sections[si]
            .register
            .iter()
            .filter_map(|r| parse_note(&r.center).map(|c| c as f64))
            .collect();
        if !cs.is_empty() {
            let mean = cs.iter().sum::<f64>() / cs.len() as f64;
            let first = sections[si].register[0].clone();
            let c = parse_note(&first.center)
                .map_or(mean, |c| (c as f64 + mean) / 2.0)
                .round() as i32;
            let c = match key {
                Some(k) => crate::harmony::snap_to_pitch_classes(
                    c,
                    &crate::harmony::scale_pitch_classes(
                        k.tonic,
                        if k.minor { "minor" } else { "major" },
                    ),
                ),
                None => c,
            };
            let center = crate::chord::note_name(c.clamp(0, 127) as u8);
            sections[si].register = vec![crate::plan::RegisterPoint {
                at: 0.0,
                center,
                span: first.span,
            }];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::Key;
    use crate::plan::{MelodyPlan, PhrasePlan, RegisterPoint, SectionPlan};

    fn plan() -> MelodyPlan {
        MelodyPlan {
            style: Some("riff".into()),
            riffs: vec![RiffPlan {
                name: "A".into(),
                bars: 1,
                rhythm: "X..x..x...x.x...".into(),
                shape: vec![0, 0, 1, -1, 0],
                anchor: None,
                follow: None,
            }],
            sections: vec![SectionPlan {
                name: "Drop".into(),
                start_bar: 1,
                bars: 8,
                register: vec![RegisterPoint {
                    at: 0.0,
                    center: "C6".into(),
                    span: Some(10),
                }],
                phrases: vec![
                    PhrasePlan {
                        label: "A".into(),
                        bars: 4.0,
                        ..Default::default()
                    },
                    PhrasePlan {
                        label: "A′".into(),
                        bars: 4.0,
                        transform: vec!["tail".into(), "fill".into()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_riff_repeats_its_rhythm_follows_the_chords_and_changes_at_the_end() {
        let p = plan();
        p.validate().unwrap();
        let grid: Vec<(u64, u64)> = (0..8).map(|b| (b * 3840, 3840)).collect();
        let names = ["Am", "F", "C", "G"];
        let f = |t: u64| {
            crate::chord::parse(names[((t / 3840) % 4) as usize])
                .ok()
                .flatten()
                .map(|c| c.pitch_classes())
        };
        let r = realize_riff(&RealizeInput {
            plan: &p,
            only: None,
            grid: &grid,
            chord_at: &f,
            key: Key::parse("A minor").unwrap(),
            low: 77,
            high: 91,
            max_width: 14,
            seed: 1,
            breath: 480,
            step: 240,
            regenerate_skeleton: false,
        })
        .unwrap();
        // 最初の 3 小節は同じリズム(小節の頭からの位置が同じ)
        let pos_in_bar = |b: u64| -> Vec<u64> {
            r.notes
                .iter()
                .filter(|n| n.pos / 3840 == b)
                .map(|n| n.pos % 3840)
                .collect()
        };
        assert_eq!(pos_in_bar(0), vec![0, 720, 1440, 2400, 2880]);
        assert_eq!(pos_in_bar(0), pos_in_bar(1));
        assert_eq!(pos_in_bar(0), pos_in_bar(2));
        // 小節の頭の音は和音の音、ほかもペンタトニック(A C D E G)か和音の音。和音の音は 7 割以上
        let penta = [9u8, 0, 2, 4, 7];
        let mut chord_tones = 0;
        for n in &r.notes {
            let pcs = f(n.pos).unwrap();
            let pc = n.pitch % 12;
            assert!(pcs.contains(&pc) || penta.contains(&pc), "{n:?}");
            if pcs.contains(&pc) {
                chord_tones += 1;
            }
            if n.pos % 3840 == 0 {
                assert!(pcs.contains(&pc), "{n:?}");
            }
        }
        assert!(
            chord_tones * 10 >= r.notes.len() * 7,
            "{chord_tones}/{}",
            r.notes.len()
        );
        // 8 小節目の最後の拍は 16 分で埋まる(fill)
        let last_beat: Vec<&MelNote> = r
            .notes
            .iter()
            .filter(|n| n.pos >= 7 * 3840 + 2880)
            .collect();
        assert_eq!(last_beat.len(), 4, "{:?}", last_beat);
        // アクセントの音(X)はリフの頭ごと
        assert!(r.accents.contains(&0) && r.accents.contains(&3840));
        assert_eq!(r.statements.len(), 8);
    }
}
