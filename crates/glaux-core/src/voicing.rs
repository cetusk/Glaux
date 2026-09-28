//! 和音の積み方(ボイシング)と声部のつながり(MCP の write_chords の中身)。
//!
//! コードごとに、形(密集・開離・ドロップ 2 など)に沿った候補を並べ、進行全体で
//! 「各声部の動きが小さい・一番上の声部が滑らか・平行 5 度 / 8 度が無い・低音域が濁らない」並びを
//! 動的計画法(Viterbi)で選ぶ(Tymoczko 2006 の考え方。決まった手順なので同じ入力なら同じ結果)。

use crate::chord::{Chord, Role};

/// 積み方の形
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// 密集(1 オクターブの中に積む)
    Close,
    /// 開離(3 声は真ん中を 1 オクターブ下げる。4 声以上はドロップ 2 と同じ)
    Open,
    /// 上から 2 番目の音を 1 オクターブ下げる(ピアノ・ギターの定番)
    Drop2,
    /// 上から 3 番目の音を 1 オクターブ下げる
    Drop3,
    /// 上から 2 番目と 4 番目を下げる(広く明るい。パッド・ストリングス向き)
    Spread,
    /// 3 度と 7 度だけ(ジャズの左手。低音は根音)
    Shell,
    /// 根音を省く(3・7 度とテンション。低音はベースに任せる)
    Rootless,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "close" => Style::Close,
            "open" => Style::Open,
            "drop2" | "drop_2" => Style::Drop2,
            "drop3" | "drop_3" => Style::Drop3,
            "spread" | "wide" => Style::Spread,
            "shell" => Style::Shell,
            "rootless" => Style::Rootless,
            _ => return None,
        })
    }
}

/// 積み方の条件
#[derive(Clone, Debug)]
pub struct Options {
    pub style: Style,
    /// 上の声部の数(低音は別)
    pub voices: usize,
    /// 上の声部の音域(MIDI 番号、両端を含む)
    pub low: u8,
    pub high: u8,
    /// 一番上の声部を寄せたい高さ(旋律の線を決めたいとき)
    pub top: Option<u8>,
    /// 低音(根音か分数コードの最低音)を別に置く音域。None なら低音を置かない
    pub bass: Option<(u8, u8)>,
}

/// 積んだ和音 1 つ
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voiced {
    pub bass: Option<u8>,
    /// 上の声部(低い順)
    pub upper: Vec<u8>,
}

/// 進行全体を積む(`chords` の順に 1 つずつ)
pub fn voice_progression(chords: &[Chord], opts: &Options) -> Result<Vec<Voiced>, String> {
    if chords.is_empty() {
        return Ok(vec![]);
    }
    if opts.low >= opts.high || opts.high - opts.low < 10 {
        return Err("音域が狭すぎます(10 半音以上)".to_owned());
    }
    let voices = match opts.style {
        Style::Shell => 2,
        _ => opts.voices.clamp(2, 6),
    };
    // コードごとの候補
    let cands: Vec<Vec<Vec<u8>>> = chords
        .iter()
        .map(|c| {
            let v = candidates(c, opts, voices);
            if v.is_empty() {
                Err(format!(
                    "{} を音域 {}〜{} に {voices} 声で積めません(音域を広げるか声を減らす)",
                    c.name,
                    crate::chord::note_name(opts.low),
                    crate::chord::note_name(opts.high)
                ))
            } else {
                Ok(v)
            }
        })
        .collect::<Result<_, _>>()?;
    // Viterbi
    let center = (opts.low as f64 + opts.high as f64) / 2.0;
    let unary = |v: &[u8]| -> f64 {
        let mean = v.iter().map(|&p| p as f64).sum::<f64>() / v.len() as f64;
        let top = *v.last().expect("空ではない") as f64;
        let mut c = (mean - center).abs() * 0.15;
        if let Some(t) = opts.top {
            c += (top - t as f64).abs() * 0.8;
        }
        // 隣どうしの半音(短 2 度)はぶつかって濁る。一番下の 2 声なら特に
        for (i, w) in v.windows(2).enumerate() {
            if w[1] - w[0] == 1 {
                c += if i == 0 { 5.0 } else { 3.0 };
            }
        }
        c
    };
    let mut cost: Vec<f64> = cands[0].iter().map(|v| unary(v)).collect();
    let mut back: Vec<Vec<usize>> = vec![vec![0; cands[0].len()]];
    for i in 1..cands.len() {
        let mut next = Vec::with_capacity(cands[i].len());
        let mut from = Vec::with_capacity(cands[i].len());
        for b in &cands[i] {
            let u = unary(b);
            let (best_j, best) = cands[i - 1]
                .iter()
                .enumerate()
                .map(|(j, a)| (j, cost[j] + motion(a, b)))
                .min_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)))
                .expect("候補がある");
            next.push(best + u);
            from.push(best_j);
        }
        cost = next;
        back.push(from);
    }
    // 最後から戻る(同じ値なら低い並びを選ぶ = 決定的)
    let mut k = cost
        .iter()
        .enumerate()
        .min_by(|x, y| x.1.total_cmp(y.1).then(x.0.cmp(&y.0)))
        .map(|(k, _)| k)
        .expect("候補がある");
    let mut picked = vec![0usize; cands.len()];
    for i in (0..cands.len()).rev() {
        picked[i] = k;
        k = back[i][k];
    }
    // 低音: 進行全体で動きが小さく、音域の真ん中から離れすぎない並びを選ぶ
    let bass = opts.bass.map(|(lo, hi)| bass_line(chords, lo, hi));
    let out = chords
        .iter()
        .enumerate()
        .map(|(i, _)| Voiced {
            bass: bass.as_ref().map(|b| b[i]),
            upper: cands[i][picked[i]].clone(),
        })
        .collect();
    Ok(out)
}

/// 低音の並び(各コードの根音か分数コードの最低音を、音域 lo〜hi のどのオクターブに置くか)
pub fn bass_line(chords: &[Chord], lo: u8, hi: u8) -> Vec<u8> {
    let center = (lo as f64 + hi as f64) / 2.0;
    let opts: Vec<Vec<u8>> = chords
        .iter()
        .map(|c| {
            let v: Vec<u8> = (lo..=hi).filter(|p| p % 12 == c.bass_pc()).collect();
            if v.is_empty() {
                vec![lo]
            } else {
                v
            }
        })
        .collect();
    let away = |p: u8| (p as f64 - center).abs() * 0.2;
    let mut cost: Vec<f64> = opts[0].iter().map(|&p| away(p)).collect();
    let mut back: Vec<Vec<usize>> = vec![vec![0; opts[0].len()]];
    for i in 1..opts.len() {
        let mut next = Vec::new();
        let mut from = Vec::new();
        for &b in &opts[i] {
            let (j, c) = opts[i - 1]
                .iter()
                .enumerate()
                .map(|(j, &a)| (j, cost[j] + (a as f64 - b as f64).abs()))
                .min_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)))
                .expect("候補がある");
            next.push(c + away(b));
            from.push(j);
        }
        cost = next;
        back.push(from);
    }
    let mut k = cost
        .iter()
        .enumerate()
        .min_by(|x, y| x.1.total_cmp(y.1).then(x.0.cmp(&y.0)))
        .map(|(k, _)| k)
        .expect("候補がある");
    let mut out = vec![0u8; opts.len()];
    for i in (0..opts.len()).rev() {
        out[i] = opts[i][k];
        k = back[i][k];
    }
    out
}

/// 2 つの積み方の間の動きの重さ
fn motion(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len());
    let mut c = 0.0;
    for i in 0..n {
        c += (a[i] as f64 - b[i] as f64).abs();
    }
    // 声の数が違えば、余った声の分だけ重くする
    c += (a.len() as f64 - b.len() as f64).abs() * 3.0;
    // 一番上の声部(旋律に聞こえる)は特に滑らかに
    c +=
        (*a.last().expect("空ではない") as f64 - *b.last().expect("空ではない") as f64).abs() * 0.5;
    // 平行 5 度・8 度(同じ向きに動いて、同じ完全音程のまま)
    for i in 0..n {
        for j in i + 1..n {
            let ia = (a[j] as i32 - a[i] as i32).rem_euclid(12);
            let ib = (b[j] as i32 - b[i] as i32).rem_euclid(12);
            let di = b[i] as i32 - a[i] as i32;
            let dj = b[j] as i32 - a[j] as i32;
            if ia == ib && (ia == 0 || ia == 7) && di != 0 && di.signum() == dj.signum() {
                c += 4.0;
            }
        }
    }
    c
}

/// 上の声部に使うピッチクラスの組(`voices` 個。重ねる音の選び方ごとに 1 組)
fn pick_tones(chord: &Chord, style: Style, voices: usize) -> Vec<Vec<u8>> {
    let pc = |iv: u8| (chord.root + iv) % 12;
    let by = |r: Role| chord.tones.iter().filter(move |t| t.role == r);
    let mut order: Vec<u8> = Vec::new();
    let push = |p: u8, order: &mut Vec<u8>| {
        if !order.contains(&p) {
            order.push(p);
        }
    };
    if style == Style::Shell {
        // 3 度(か sus)と 7 度(無ければ 6 度、それも無ければ 5 度)
        for t in by(Role::Third).chain(by(Role::Sus)) {
            push(pc(t.interval), &mut order);
        }
        let seventh = by(Role::Seventh)
            .chain(by(Role::Sixth))
            .chain(by(Role::Fifth))
            .next();
        if let Some(t) = seventh {
            push(pc(t.interval), &mut order);
        }
        order.truncate(2);
        while order.len() < 2 {
            order.push(chord.root);
        }
        return vec![order];
    }
    // 大事な順: 3 度(sus)→ 7 度・6 度 → 省けないテンション・変化した 5 度 → 根音 → 5 度 → 省けるテンション
    for t in by(Role::Third).chain(by(Role::Sus)) {
        push(pc(t.interval), &mut order);
    }
    for t in by(Role::Seventh).chain(by(Role::Sixth)) {
        push(pc(t.interval), &mut order);
    }
    for t in chord
        .tones
        .iter()
        .filter(|t| t.role == Role::Tension && !t.optional)
    {
        push(pc(t.interval), &mut order);
    }
    for t in by(Role::Fifth).filter(|t| !t.optional) {
        push(pc(t.interval), &mut order);
    }
    // テンションのある和音は、根音より 5 度を先に(Am9 の 4 声 = C E G B。根音はベースに任せる定番の形)。
    // テンションが無ければ根音が先(三和音・7th)
    let has_tension = chord
        .tones
        .iter()
        .any(|t| t.role == Role::Tension && !t.optional);
    if has_tension {
        for t in by(Role::Fifth).filter(|t| t.optional) {
            push(pc(t.interval), &mut order);
        }
    }
    if style != Style::Rootless {
        push(chord.root, &mut order);
    }
    for t in by(Role::Fifth).filter(|t| t.optional) {
        push(pc(t.interval), &mut order);
    }
    for t in chord
        .tones
        .iter()
        .filter(|t| t.role == Role::Tension && t.optional)
    {
        push(pc(t.interval), &mut order);
    }
    // パワーコードなど 3 度が無い和音
    if order.is_empty() {
        order.push(chord.root);
    }
    let base: Vec<u8> = order.iter().copied().take(voices).collect();
    if base.len() >= voices {
        return vec![base];
    }
    // 足りなければ重ねる。重ねてよいのは根音と 5 度(3 度・7 度・テンションを重ねると響きが偏る)。
    // どれを重ねるかは積み方の候補ごとに変える(根音を重ねた形・5 度を重ねた形)
    let mut doublable: Vec<u8> = vec![chord.root];
    for t in by(Role::Fifth) {
        if !doublable.contains(&pc(t.interval)) {
            doublable.push(pc(t.interval));
        }
    }
    doublable.retain(|p| base.contains(p) || style == Style::Rootless);
    if doublable.is_empty() {
        doublable = base.clone();
    }
    (0..doublable.len())
        .map(|start| {
            let mut v = base.clone();
            let mut k = start;
            while v.len() < voices {
                v.push(doublable[k % doublable.len()]);
                k += 1;
            }
            v
        })
        .collect()
}

/// コード 1 つの候補の積み方(低い順の音の並び)
fn candidates(chord: &Chord, opts: &Options, voices: usize) -> Vec<Vec<u8>> {
    // 分数コードで低音を置かないときは、その音を一番下にする
    let slash_low = if opts.bass.is_none() {
        chord.bass
    } else {
        None
    };
    let has_b9 = chord.tones.iter().any(|t| t.interval == 1);
    let mut out: Vec<Vec<u8>> = Vec::new();
    for mut tones in pick_tones(chord, opts.style, voices) {
        if let Some(b) = slash_low {
            if !tones.contains(&b) {
                let last = tones.len() - 1;
                tones[last] = b;
            }
        }
        add_candidates(&tones, opts, slash_low, has_b9, &mut out);
    }
    out.sort();
    out
}

fn add_candidates(
    tones: &[u8],
    opts: &Options,
    slash_low: Option<u8>,
    has_b9: bool,
    out: &mut Vec<Vec<u8>>,
) {
    for bottom in 0..tones.len() {
        // 密集の形(bottom の音から上へ、重なる音は 1 オクターブ上)
        let b_pc = tones[bottom];
        let mut ds: Vec<i32> = Vec::with_capacity(tones.len());
        for (i, &t) in tones.iter().enumerate() {
            let mut d = (t as i32 - b_pc as i32).rem_euclid(12);
            if i != bottom {
                while ds.contains(&d) || (d == 0) {
                    d += 12;
                }
            }
            ds.push(d);
        }
        ds.sort_unstable();
        // 密集形は、隣どうしの間が 5 度以内(重ねた音だけ 1 オクターブ上に離れた形は密集ではない)
        if ds.windows(2).any(|w| w[1] - w[0] > 7) {
            continue;
        }
        let shapes = shapes_for(&ds, opts.style);
        for shape in shapes {
            let lowest = *shape.iter().min().expect("空ではない");
            let highest = *shape.iter().max().expect("空ではない");
            for base in (opts.low as i32 - lowest)..=(opts.high as i32 - highest) {
                if base.rem_euclid(12) != b_pc as i32 {
                    continue;
                }
                let mut v: Vec<u8> = shape.iter().map(|d| (base + d) as u8).collect();
                v.sort_unstable();
                if !clean(&v, has_b9) {
                    continue;
                }
                if let Some(b) = slash_low {
                    if v[0] % 12 != b {
                        continue;
                    }
                }
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
    }
}

/// 密集の形(根からの半音の昇順)から、形の種類に合う並びを作る
fn shapes_for(close: &[i32], style: Style) -> Vec<Vec<i32>> {
    let n = close.len();
    let drop = |idxs: &[usize]| -> Vec<i32> {
        let mut v = close.to_vec();
        for &k in idxs {
            // 上から k 番目(1 始まり)を 1 オクターブ下げる
            if k <= n {
                v[n - k] -= 12;
            }
        }
        v.sort_unstable();
        v
    };
    match style {
        Style::Close | Style::Shell | Style::Rootless => vec![close.to_vec()],
        Style::Open => {
            if n >= 3 {
                vec![drop(&[2])]
            } else {
                vec![close.to_vec()]
            }
        }
        Style::Drop2 => vec![drop(&[2])],
        Style::Drop3 => {
            if n >= 4 {
                vec![drop(&[3])]
            } else {
                vec![drop(&[2])]
            }
        }
        Style::Spread => {
            if n >= 4 {
                vec![drop(&[2, 4])]
            } else {
                vec![drop(&[2])]
            }
        }
    }
}

/// 低音域で詰まらない・短 9 度(b9 の和音以外)が無い
fn clean(v: &[u8], has_b9: bool) -> bool {
    for w in v.windows(2) {
        let (a, b) = (w[0], w[1]);
        let gap = b - a;
        if gap == 0 {
            return false;
        }
        if a < 45 && gap < 7 {
            return false;
        }
        if a < 52 && gap < 3 {
            return false;
        }
    }
    if !has_b9 {
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                if v[j] - v[i] == 13 {
                    return false;
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::{parse, parse_in_key, Key};

    fn chords(s: &[&str]) -> Vec<Chord> {
        s.iter().map(|c| parse(c).unwrap().unwrap()).collect()
    }

    fn opts(style: Style, voices: usize) -> Options {
        Options {
            style,
            voices,
            low: 52,
            high: 76,
            top: None,
            bass: None,
        }
    }

    fn total_motion(v: &[Voiced]) -> i32 {
        v.windows(2)
            .map(|w| {
                w[0].upper
                    .iter()
                    .zip(&w[1].upper)
                    .map(|(a, b)| (*a as i32 - *b as i32).abs())
                    .sum::<i32>()
            })
            .sum()
    }

    #[test]
    fn close_triads_move_smoothly() {
        // I–IV–V–I の 3 声: 各声部はほとんど動かない(共通音は残る)
        let v = voice_progression(&chords(&["C", "F", "G", "C"]), &opts(Style::Close, 3)).unwrap();
        for x in &v {
            assert_eq!(x.upper.len(), 3);
            assert!(x.upper.iter().all(|p| (52..=76).contains(p)));
        }
        assert!(total_motion(&v) <= 12, "{v:?}");
        // 構成音だけ
        assert!(v[1].upper.iter().all(|p| [5, 9, 0].contains(&(p % 12))));
        // 同じ入力なら同じ結果
        assert_eq!(
            v,
            voice_progression(&chords(&["C", "F", "G", "C"]), &opts(Style::Close, 3)).unwrap()
        );
    }

    #[test]
    fn ii_v_i_keeps_thirds_and_sevenths_and_avoids_parallels() {
        let cs = chords(&["Dm7", "G7", "Cmaj7"]);
        for style in [Style::Close, Style::Drop2, Style::Drop3, Style::Spread] {
            let v = voice_progression(&cs, &opts(style, 4)).unwrap();
            for (c, x) in cs.iter().zip(&v) {
                let pcs: Vec<u8> = x.upper.iter().map(|p| p % 12).collect();
                let third = c.tones.iter().find(|t| t.role == Role::Third).unwrap();
                let seventh = c.tones.iter().find(|t| t.role == Role::Seventh).unwrap();
                assert!(
                    pcs.contains(&((c.root + third.interval) % 12)),
                    "{style:?} {v:?}"
                );
                assert!(
                    pcs.contains(&((c.root + seventh.interval) % 12)),
                    "{style:?} {v:?}"
                );
            }
            assert!(total_motion(&v) <= 14, "{style:?} {v:?}");
        }
        // ドロップ 2 は上 3 声の中に 1 オクターブを越える幅がある(密集より広い)
        let d2 = voice_progression(&cs, &opts(Style::Drop2, 4)).unwrap();
        assert!(d2.iter().all(|x| x.upper.last().unwrap() - x.upper[0] > 12));
    }

    #[test]
    fn shell_and_rootless() {
        let cs = chords(&["Dm7", "G7", "Cmaj7"]);
        let mut o = opts(Style::Shell, 4);
        o.bass = Some((36, 52));
        let v = voice_progression(&cs, &o).unwrap();
        for (c, x) in cs.iter().zip(&v) {
            assert_eq!(x.upper.len(), 2);
            assert_eq!(x.bass.unwrap() % 12, c.root);
        }
        // G7 の上は B と F
        let mut g: Vec<u8> = v[1].upper.iter().map(|p| p % 12).collect();
        g.sort_unstable();
        assert_eq!(g, vec![5, 11]);
        // ルートレス: 根音が上に無い
        let v = voice_progression(&chords(&["Dm9", "G13", "Cmaj9"]), &opts(Style::Rootless, 4))
            .unwrap();
        for (c, x) in chords(&["Dm9", "G13", "Cmaj9"]).iter().zip(&v) {
            assert!(!x.upper.iter().any(|p| p % 12 == c.root), "{v:?}");
        }
    }

    #[test]
    fn top_target_bass_and_slash_chords() {
        // 一番上の声部を E5 付近に
        let mut o = opts(Style::Close, 4);
        o.top = Some(76);
        let v = voice_progression(&chords(&["C", "Am", "F", "G"]), &o).unwrap();
        assert!(v.iter().all(|x| *x.upper.last().unwrap() >= 71), "{v:?}");
        // 低音: 分数コードは指定の音。前の低音の近くに置く
        let mut o = opts(Style::Close, 3);
        o.bass = Some((36, 52));
        let v = voice_progression(&chords(&["C", "G/B", "Am", "C/G"]), &o).unwrap();
        let bass: Vec<u8> = v.iter().map(|x| x.bass.unwrap()).collect();
        assert_eq!(
            bass.iter().map(|b| b % 12).collect::<Vec<_>>(),
            vec![0, 11, 9, 7]
        );
        assert!(
            bass.windows(2)
                .all(|w| (w[0] as i32 - w[1] as i32).abs() <= 5),
            "{bass:?}"
        );
        // 低音を置かないときは、分数コードの音を一番下に
        let v = voice_progression(&chords(&["C/E"]), &opts(Style::Close, 3)).unwrap();
        assert_eq!(v[0].upper[0] % 12, 4);
    }

    #[test]
    fn ninth_chords_drop_the_fifth_before_the_root_and_avoid_clusters() {
        // 4 声の close: 根音を省いて 3・5・7・9(Am9 = C E G B)。半音のぶつかりが無い
        let v = voice_progression(
            &chords(&["Am9", "Fmaj9", "Cmaj9", "G6"]),
            &opts(Style::Close, 4),
        )
        .unwrap();
        let mut am9: Vec<u8> = v[0].upper.iter().map(|p| p % 12).collect();
        am9.sort_unstable();
        assert_eq!(am9, vec![0, 4, 7, 11]);
        for x in &v[..3] {
            assert!(x.upper.windows(2).all(|w| w[1] - w[0] > 1), "{v:?}");
        }
        // 5 声なら根音も入る
        let v = voice_progression(&chords(&["Am9"]), &opts(Style::Close, 5)).unwrap();
        assert!(v[0].upper.iter().any(|p| p % 12 == 9));
    }

    #[test]
    fn low_register_stays_clear_and_errors_are_explained() {
        // 低い音域でも 3 度で詰まった和音は作らない
        let mut o = opts(Style::Close, 3);
        o.low = 36;
        o.high = 60;
        let v = voice_progression(&chords(&["C", "F"]), &o).unwrap();
        for x in &v {
            for w in x.upper.windows(2) {
                if w[0] < 45 {
                    assert!(w[1] - w[0] >= 7, "{v:?}");
                }
            }
        }
        // 狭すぎる音域はエラー
        let mut o = opts(Style::Close, 4);
        o.high = 58;
        assert!(voice_progression(&chords(&["C"]), &o).is_err());
        // ローマ数字から
        let k = Key::parse("C major").unwrap();
        let cs: Vec<Chord> = ["IVmaj7", "V7", "iii7", "vi"]
            .iter()
            .map(|s| parse_in_key(s, Some(k)).unwrap().unwrap())
            .collect();
        assert_eq!(
            voice_progression(&cs, &opts(Style::Drop2, 4))
                .unwrap()
                .len(),
            4
        );
    }
}
