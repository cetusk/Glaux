//! 旋律の構造の分析(MCP の analyze_melody の中身)。旋律を「区間 → 句 → 骨格」の層に読み返し、粒度ごとに測る。
//!
//! 生成の側(上から下へ作る)と同じ層を、どの旋律(人が書いた・読み込んだ・生成した)からも取り出せるようにする。
//! 測る側が正確でないと、直すループは回らない(docs の調査)。
//!
//! - 骨格: 最短経路による旋律の還元(Wang・Wu・Dannenberg・Xia, "Automatic Melody Reduction via Shortest Path Finding",
//!   ISMIR 2025)。句の音を頂点に、2 小節以内の前向きの辺を張り、始めの音から終わりの音への最短経路を骨格とする。
//!   辺の費用は動きの種類(同音 0.1・2 度 0.3・オクターブ違い 1.0・複合 2 度 1.3・同じ和音の分散 1.5・その他 3.0)と
//!   距離((j − i)^1.6)の和に、行き先の音の重要度(音域の端・拍・長さ・和音の音)を掛けたもの
//! - 句: 次の音までの間(長い音か休み)が曲の中央値の 2 倍以上で 1 拍以上ある所を、強い順に区切る(句は 3 拍以上)。
//!   6 小節を超えて区切れなければ 4 小節ごとに切る
//! - 繰り返しの地図: 句どうしのリズム(句の頭からの 16 分の位置)と輪郭(骨格の高さの動き)の似かたで A・A′・B … と名付ける
//! - 区間: 2 小節ごとの音域の中心・幅と密度の曲線、句をまたぐリズムの似かた、山の位置。区間どうしの対比
//! - 位置による違い: 人の旋律は句の終わりで長い音・主音が増える(Dai ら 2020。深層モデルはどこでも同じ分布で、
//!   AI らしさの兆候)

use crate::critique::Finding;
use crate::melody::{Context, MelNote};
use serde::Serialize;

const PPQ: u64 = crate::time::PPQ;

/// 句 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct PhraseInfo {
    /// 句の番号(0 始まり)と名前(A・A′・B …)
    pub index: usize,
    pub label: String,
    /// 似ている前の句(A′ なら A の番号)と、その似かた(0〜1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub like: Option<usize>,
    pub similarity: f64,
    /// 始まり(曲の小節番号、1 始まり)と、小節の頭からのずれ(拍。負なら前の小節の終わりから入る弱起)
    pub start_bar: usize,
    pub offset_beats: f64,
    /// 長さ(小節)と音の数、密度(1 拍あたりの音の数)
    pub bars: f64,
    pub notes: usize,
    pub density: f64,
    /// 音域(最低・最高・中央値の音高)
    pub low: u8,
    pub high: u8,
    pub center: u8,
    /// 骨格の輪郭: arch(弧)/ rise / fall / valley / flat
    pub contour: &'static str,
    /// 骨格の音の数
    pub skeleton: usize,
    /// 終わりの音の間(次の句の頭まで。最後の句は音の長さ)÷ 句の中の音の間の中央値。1.5 以上なら長い音か休みで
    /// 句の終わりが聞こえる。終わりの音の音度(1〜7。調の外なら 0)
    pub ending_ratio: f64,
    pub ending_degree: u8,
    /// 区切り方: "rest"(休み)/ "long"(長い音)/ "bars"(区切れず 4 小節ごと)/ "end"(最後)
    pub split: &'static str,
    /// 最初の音の頭と、最後の音の終わり(tick)
    #[serde(skip)]
    pub start: u64,
    #[serde(skip)]
    pub end: u64,
}

/// 区間 1 つ
#[derive(Clone, Debug, Serialize)]
pub struct SectionInfo {
    pub name: String,
    pub start_bar: usize,
    pub bars: usize,
    /// 句の番号の範囲と、名前の並び("A A′ B A″")
    pub phrases: Vec<usize>,
    pub form: String,
    /// 2 小節ごとの音域の中心(中央値の音高。音が無ければ 0)と幅(最高 − 最低)
    pub register_curve: Vec<u8>,
    pub span_curve: Vec<u8>,
    /// 2 小節ごとの密度(1 拍あたりの音の数)
    pub density_curve: Vec<f64>,
    /// 区間の音域(最低・最高)と、2 小節ごとの中心の動き(標準偏差、半音)
    pub low: u8,
    pub high: u8,
    pub register_motion: f64,
    /// 1 小節ごとの音域の中心の最高と最低の差(半音)。句の単位で一方向に動く旋律は 2 小節の中心では小さく見えるので併せて見る
    pub register_travel: u8,
    /// 密度の変化(変動係数)
    pub density_variation: f64,
    /// 小節どうしのリズム(16 分の打点の型)の似かたの平均(0〜1。1 なら全部の小節が同じリズム)
    pub rhythm_sameness: f64,
    /// 句どうしのリズムの似かたの平均(句が 2 つ以上のとき)
    pub phrase_rhythm_sameness: Option<f64>,
    /// 最高音が初めて出る位置(区間の中の割合 0〜1)と回数
    pub peak_at: f64,
    pub peak_count: usize,
    /// 新しい素材の割合(新しい名前の句 ÷ 句の数)
    pub novelty: f64,
    /// 音が鳴っている所で、2 拍ごとの頭(小節の頭と半ば)に音の頭がある割合(参考の数値。4 分で歩く人の旋律でも 1 に近い)
    pub half_bar_hits: f64,
}

/// 曲全体
#[derive(Clone, Debug, Serialize)]
pub struct SongInfo {
    /// 句の終わりで間を取る(終わりの音の間が句の中の中央値の 1.5 倍以上)割合
    pub long_endings: f64,
    /// 主音で終わる割合: 句の終わり / それ以外の音
    pub tonic_at_endings: f64,
    pub tonic_elsewhere: f64,
    /// 骨格の音の割合(骨格の音 ÷ 全部の音)
    pub skeleton_share: f64,
}

/// 分析の結果
#[derive(Clone, Debug, Serialize)]
pub struct Structure {
    pub key: String,
    pub phrases: Vec<PhraseInfo>,
    pub sections: Vec<SectionInfo>,
    pub song: SongInfo,
    /// 骨格の音(絶対 tick と音高)
    pub skeleton: Vec<(u64, u8)>,
    /// 粒度ごとの指摘("song" / "section" / "phrase" のどれかを what の頭に付ける)
    pub findings: Vec<Finding>,
    /// LLM が読む要約(1 行 1 つ)
    pub summary: Vec<String>,
}

/// 行き先の音の重要度(小さいほど骨格に残りやすい)
fn importance(
    n: &MelNote,
    bar_start: u64,
    bar_len: u64,
    lo: u8,
    hi: u8,
    chord: Option<&[u8]>,
) -> f64 {
    let rel = n.pos.saturating_sub(bar_start);
    let half = (bar_len / 2).max(1);
    let near = |a: u64, b: u64| a % b < 60 || b - a % b < 60;
    let beat = if near(rel, half) {
        0.85
    } else if near(rel, PPQ) {
        0.95
    } else if near(rel, PPQ / 2) {
        1.05
    } else {
        1.15
    };
    let dur = if n.dur + 60 >= 2 * PPQ {
        0.85
    } else if n.dur + 60 >= PPQ {
        0.95
    } else if n.dur + 30 >= PPQ / 2 {
        1.05
    } else {
        1.15
    };
    let ch = match chord {
        Some(pcs) if pcs.contains(&(n.pitch % 12)) => 0.85,
        Some(_) => 1.15,
        None => 1.0,
    };
    // 音域の端ほど小さい(山と谷は残りやすい)
    let pos = if hi > lo {
        (n.pitch - lo) as f64 / (hi - lo) as f64
    } else {
        0.5
    };
    let reg = 1.0 - 0.1 * (2.0 * pos - 1.0).abs();
    beat * dur * ch * reg
}

/// 辺の種類の費用
fn edge_kind(a: u8, b: u8, same_chord: Option<&[u8]>) -> f64 {
    let d = (a as i32 - b as i32).abs();
    if d == 0 {
        0.1
    } else if d <= 2 {
        0.3
    } else if d % 12 == 0 {
        1.0
    } else if d % 12 <= 2 || d % 12 >= 10 {
        1.3
    } else if same_chord.is_some_and(|pcs| pcs.contains(&(a % 12)) && pcs.contains(&(b % 12))) {
        1.5
    } else {
        3.0
    }
}

/// 句の音(`idx`)の骨格(最短経路で残る音の番号)
fn reduce(
    notes: &[MelNote],
    idx: &[usize],
    bar_of: &dyn Fn(u64) -> (u64, u64),
    chord_at: &dyn Fn(u64) -> Option<Vec<u8>>,
) -> Vec<usize> {
    if idx.len() <= 2 {
        return idx.to_vec();
    }
    let lo = idx.iter().map(|&i| notes[i].pitch).min().unwrap_or(0);
    let hi = idx.iter().map(|&i| notes[i].pitch).max().unwrap_or(127);
    let alpha: Vec<f64> = idx
        .iter()
        .map(|&i| {
            let (s, l) = bar_of(notes[i].pos);
            importance(&notes[i], s, l, lo, hi, chord_at(notes[i].pos).as_deref())
        })
        .collect();
    let n = idx.len();
    let mut best = vec![f64::INFINITY; n];
    let mut prev = vec![usize::MAX; n];
    best[0] = 0.0;
    for j in 1..n {
        let tj = notes[idx[j]].pos;
        let (_, bar_len) = bar_of(tj);
        for i in (0..j).rev() {
            // 2 小節より離れた辺は張らない(ただし隣の音へは必ず)
            if i + 1 < j && tj - notes[idx[i]].pos > 2 * bar_len {
                break;
            }
            if best[i].is_infinite() {
                continue;
            }
            let (a, b) = (notes[idx[i]], notes[idx[j]]);
            let ca = chord_at(a.pos);
            let same = match (&ca, chord_at(b.pos)) {
                (Some(x), Some(y)) if *x == y => Some(x.as_slice()),
                _ => None,
            };
            let cost = alpha[j] * (((j - i) as f64).powf(1.6) + edge_kind(a.pitch, b.pitch, same));
            if best[i] + cost < best[j] {
                best[j] = best[i] + cost;
                prev[j] = i;
            }
        }
    }
    let mut path = vec![n - 1];
    let mut k = n - 1;
    while prev[k] != usize::MAX {
        k = prev[k];
        path.push(k);
    }
    path.reverse();
    // 順次進行の多い旋律は最短経路でもあまり減らない(論文も後処理で和音ごとの箱に詰める)。
    // 経路の音を 2 拍ごとの箱に入れ、箱ごとに最も重要な音(α が最小、同じなら長い方)を 1 つ残す。始めと終わりは必ず残す
    let bin = |k: usize| notes[idx[k]].pos / (2 * PPQ);
    let (first, last) = (path[0], path[path.len() - 1]);
    let mut kept = vec![first];
    let inner = &path[1..path.len() - 1];
    let mut x = 0;
    while x < inner.len() {
        let b = bin(inner[x]);
        let mut y = x;
        while y < inner.len() && bin(inner[y]) == b {
            y += 1;
        }
        if b != bin(first) && b != bin(last) {
            let best = inner[x..y].iter().copied().min_by(|&p, &q| {
                alpha[p]
                    .total_cmp(&alpha[q])
                    .then(notes[idx[q]].dur.cmp(&notes[idx[p]].dur))
            });
            kept.extend(best);
        }
        x = y;
    }
    if last != first {
        kept.push(last);
    }
    kept.into_iter().map(|k| idx[k]).collect()
}

/// 骨格の高さの動きから輪郭の型
fn contour_of(pitches: &[u8]) -> &'static str {
    if pitches.len() < 2 {
        return "flat";
    }
    let first = pitches[0] as i32;
    let last = *pitches.last().unwrap_or(&0) as i32;
    let hi = *pitches.iter().max().unwrap_or(&0) as i32;
    let lo = *pitches.iter().min().unwrap_or(&0) as i32;
    let mid_hi = pitches[1..pitches.len() - 1]
        .iter()
        .any(|&p| p as i32 >= hi && hi - first.max(last) >= 2);
    let mid_lo = pitches[1..pitches.len() - 1]
        .iter()
        .any(|&p| p as i32 <= lo && first.min(last) - lo >= 2);
    if hi - lo <= 2 {
        "flat"
    } else if mid_hi {
        "arch"
    } else if mid_lo {
        "valley"
    } else if last > first {
        "rise"
    } else {
        "fall"
    }
}

/// 2 つの句の似かた: (リズム 0〜1, 輪郭 0〜1)。輪郭は表面の音の高さ(平均からのずれ)を 16 点に並べ直して比べる
/// (平均 4 半音の差で 0。移調した繰り返しは同じ輪郭)
fn phrase_similarity(a: &[(u64, u8)], b: &[(u64, u8)]) -> (f64, f64) {
    // リズム: 句の頭からの 16 分の位置の集合の一致
    let set = |v: &[(u64, u8)]| -> Vec<u64> {
        let s = v.first().map_or(0, |x| x.0);
        v.iter().map(|x| (x.0 - s + 120) / 240).collect()
    };
    let (ra, rb) = (set(a), set(b));
    let inter = ra.iter().filter(|x| rb.contains(x)).count();
    let union = ra.len() + rb.len() - inter;
    let rhythm = if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    };
    let resample = |v: &[(u64, u8)]| -> Vec<f64> {
        if v.is_empty() {
            return vec![0.0; 16];
        }
        let mean = v.iter().map(|x| x.1 as f64).sum::<f64>() / v.len() as f64;
        (0..16)
            .map(|k| v[(k * v.len() / 16).min(v.len() - 1)].1 as f64 - mean)
            .collect()
    };
    let (pa, pb) = (resample(a), resample(b));
    let mad = pa.iter().zip(&pb).map(|(x, y)| (x - y).abs()).sum::<f64>() / 16.0;
    (rhythm, (1.0 - mad / 4.0).max(0.0))
}

fn median_u8(v: &mut [u8]) -> u8 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

fn std_dev(v: &[f64]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = v.iter().sum::<f64>() / v.len() as f64;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

pub fn analyze(notes_in: &[MelNote], ctx: &Context) -> Structure {
    let notes = crate::melody::top_line(notes_in.to_vec());
    let key = ctx.key.unwrap_or_else(|| crate::melody::guess_key(&notes));
    let key_name = format!(
        "{} {}",
        ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"][key.tonic as usize],
        if key.minor { "minor" } else { "major" }
    );
    let mut findings: Vec<Finding> = Vec::new();
    let empty = |findings: Vec<Finding>| Structure {
        key: key_name.clone(),
        phrases: vec![],
        sections: vec![],
        song: SongInfo {
            long_endings: 0.0,
            tonic_at_endings: 0.0,
            tonic_elsewhere: 0.0,
            skeleton_share: 0.0,
        },
        skeleton: vec![],
        findings,
        summary: vec![],
    };
    if notes.len() < 4 {
        return empty(vec![Finding {
            severity: "info",
            target: ctx.target.clone(),
            what: "音が少なすぎて分析できない(4 音以上)".to_owned(),
            fix: String::new(),
        }]);
    }
    let end = notes.iter().map(|n| n.pos + n.dur).max().unwrap_or(0);
    let grid = crate::arrange::bar_grid(ctx.project, end.max(1) + 1);
    let bar_idx = |t: u64| grid.partition_point(|(s, _)| *s <= t).saturating_sub(1);
    let bar_of = |t: u64| {
        grid.get(bar_idx(t))
            .map_or((0, 4 * PPQ), |&(s, l)| (s, l.max(1)))
    };

    // ---- 句の区切り ----
    // 次の音までの間(IOI)が曲の中央値の 2 倍以上で 1 拍以上ある所を候補にし(長い音か休み)、強い候補から採る。
    // どの句も 3 拍以上にする(短く切った音の後の小さな休みでは区切らない)
    let ioi = |i: usize| {
        notes
            .get(i + 1)
            .map_or(notes[i].dur, |nx| nx.pos - notes[i].pos)
    };
    let mut iois: Vec<u64> = (0..notes.len() - 1).map(ioi).collect();
    iois.sort_unstable();
    let med_ioi = iois[iois.len() / 2].max(1);
    let mut cands: Vec<(f64, usize)> = (0..notes.len() - 1)
        .filter(|&i| {
            // 休みを伴うか、とても長い音(中央値の 3 倍以上)の後だけ。伸ばす音が並ぶ旋律を細かく切らない
            let rest = notes[i + 1].pos.saturating_sub(notes[i].pos + notes[i].dur);
            ioi(i) >= 2 * med_ioi && ioi(i) >= PPQ && (rest >= PPQ / 2 || ioi(i) >= 3 * med_ioi)
        })
        .map(|i| {
            let rest = notes[i + 1].pos.saturating_sub(notes[i].pos + notes[i].dur);
            (ioi(i) as f64 / med_ioi as f64 + rest as f64 / PPQ as f64, i)
        })
        .collect();
    cands.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
    let min_len = 3 * PPQ;
    // 句の頭の音の番号(0 と、採った区切りの次の音)
    let mut heads: Vec<usize> = vec![0];
    for &(_, i) in &cands {
        let h = i + 1;
        let at = heads.partition_point(|&x| x < h);
        let prev = notes[heads[at - 1]].pos;
        let next = heads.get(at).map_or(end, |&x| notes[x].pos);
        if notes[h].pos - prev >= min_len && next - notes[h].pos >= min_len {
            heads.insert(at, h);
        }
    }
    let bounds: Vec<(usize, usize, &'static str)> = heads
        .iter()
        .enumerate()
        .map(|(k, &h)| {
            let last = heads.get(k + 1).map_or(notes.len() - 1, |&x| x - 1);
            let why = if last + 1 == notes.len() {
                "end"
            } else if notes[last + 1]
                .pos
                .saturating_sub(notes[last].pos + notes[last].dur)
                >= PPQ / 2
            {
                "rest"
            } else {
                "long"
            };
            (h, last, why)
        })
        .collect();
    // 6 小節を超えて区切れない句は 4 小節ごとに切る
    let mut split_bounds: Vec<(usize, usize, &'static str)> = Vec::new();
    for (a, b, w) in bounds {
        let first_bar = bar_idx(notes[a].pos);
        let last_bar = bar_idx(notes[b].pos);
        if last_bar - first_bar < 6 {
            split_bounds.push((a, b, w));
            continue;
        }
        let mut s = a;
        for i in a..=b {
            let next_chunk = notes.get(i + 1).is_some_and(|nx| {
                (bar_idx(nx.pos) - first_bar) / 4 != (bar_idx(notes[i].pos) - first_bar) / 4
            });
            if i == b {
                split_bounds.push((s, i, w));
            } else if next_chunk {
                split_bounds.push((s, i, "bars"));
                s = i + 1;
            }
        }
    }

    // ---- 句ごとの骨格・特徴 ----
    let scale =
        crate::harmony::scale_pitch_classes(key.tonic, if key.minor { "minor" } else { "major" });
    let degree = |p: u8| -> u8 {
        let pc = (p as i32 - key.tonic as i32).rem_euclid(12) as u8;
        scale
            .iter()
            .position(|&s| (s as i32 - key.tonic as i32).rem_euclid(12) as u8 == pc)
            .map_or(0, |i| i as u8 + 1)
    };
    let mut skeleton_idx: Vec<usize> = Vec::new();
    let mut phrases: Vec<PhraseInfo> = Vec::new();
    let mut phrase_notes: Vec<Vec<(u64, u8)>> = Vec::new();
    for (k, &(a, b, why)) in split_bounds.iter().enumerate() {
        let idx: Vec<usize> = (a..=b).collect();
        let sk = reduce(&notes, &idx, &bar_of, ctx.chord_at);
        skeleton_idx.extend(&sk);
        let seg = &notes[a..=b];
        let (bs, bl) = bar_of(seg[0].pos);
        let mut off = (seg[0].pos - bs) as f64 / PPQ as f64;
        // 小節の後半で始まる句は、次の小節への弱起とみなす
        let mut start_bar = bar_idx(seg[0].pos) + 1;
        if off >= (bl as f64 / PPQ as f64) / 2.0
            && seg.len() > 1
            && bar_idx(seg[1].pos) > bar_idx(seg[0].pos)
        {
            off -= bl as f64 / PPQ as f64;
            start_bar += 1;
        }
        let beats =
            (seg[seg.len() - 1].pos + seg[seg.len() - 1].dur - seg[0].pos) as f64 / PPQ as f64;
        let mut ps: Vec<u8> = seg.iter().map(|n| n.pitch).collect();
        let low = *ps.iter().min().unwrap_or(&0);
        let high = *ps.iter().max().unwrap_or(&0);
        let center = median_u8(&mut ps);
        let mut gaps: Vec<u64> = (a..b).map(ioi).collect();
        gaps.sort_unstable();
        let med = gaps.get(gaps.len() / 2).copied().unwrap_or(med_ioi).max(1);
        let sk_p: Vec<u8> = sk.iter().map(|&i| notes[i].pitch).collect();
        phrases.push(PhraseInfo {
            index: k,
            label: String::new(),
            like: None,
            similarity: 0.0,
            start_bar,
            offset_beats: round2(off),
            bars: round2(beats / 4.0),
            notes: seg.len(),
            density: round2(seg.len() as f64 / beats.max(0.25)),
            low,
            high,
            center,
            contour: contour_of(&sk_p),
            skeleton: sk.len(),
            ending_ratio: round2(ioi(b) as f64 / med as f64),
            ending_degree: degree(seg[seg.len() - 1].pitch),
            split: why,
            start: seg[0].pos,
            end: seg[seg.len() - 1].pos + seg[seg.len() - 1].dur,
        });
        phrase_notes.push(seg.iter().map(|n| (n.pos, n.pitch)).collect());
    }
    // 繰り返しの地図: 前の句のどれかとの似かたで名前を付ける
    let name = |l: u8, primes: usize| -> String {
        let ch = (b'A' + l.min(25)) as char;
        let marks = ["", "′", "″", "‴"];
        format!("{ch}{}", marks[primes.min(3)])
    };
    let mut next_letter = 0u8;
    let mut letters: Vec<(u8, usize)> = Vec::new(); // (文字, ′ の数)
    for k in 0..phrases.len() {
        let mut best: Option<(usize, f64)> = None;
        for j in 0..k {
            let (r, c) = phrase_similarity(&phrase_notes[j], &phrase_notes[k]);
            // 名前は高さの形を主に見る(リズムだけ同じなら別の句)
            let s = 0.3 * r + 0.7 * c;
            if best.map_or(true, |(_, b)| s > b) {
                best = Some((j, s));
            }
        }
        let label = match best {
            // ほぼ同じなら同じ名前
            Some((j, s)) if s >= 0.95 => {
                phrases[k].like = Some(j);
                phrases[k].similarity = round2(s);
                letters.push(letters[j]);
                phrases[j].label.clone()
            }
            // 似ていれば同じ文字の変形(′ を 1 つ増やす)
            Some((j, s)) if s >= 0.6 => {
                let l = letters[j].0;
                let n = letters
                    .iter()
                    .filter(|x| x.0 == l)
                    .map(|x| x.1 + 1)
                    .max()
                    .unwrap_or(1);
                phrases[k].like = Some(j);
                phrases[k].similarity = round2(s);
                letters.push((l, n));
                name(l, n)
            }
            other => {
                if let Some((_, s)) = other {
                    phrases[k].similarity = round2(s);
                }
                let l = next_letter;
                next_letter = next_letter.saturating_add(1);
                letters.push((l, 0));
                name(l, 0)
            }
        };
        phrases[k].label = label;
    }

    // ---- 区間 ----
    let spans: Vec<(String, u64, u64)> = if ctx.sections.is_empty() {
        vec![("全体".to_owned(), notes[0].pos, end)]
    } else {
        ctx.sections
            .iter()
            .map(|s| (s.name.clone(), s.start, s.end))
            .collect()
    };
    let mut sections: Vec<SectionInfo> = Vec::new();
    for (name, s0, s1) in spans {
        let inside: Vec<&MelNote> = notes.iter().filter(|n| s0 <= n.pos && n.pos < s1).collect();
        if inside.len() < 4 {
            continue;
        }
        let b0 = bar_idx(s0.max(inside[0].pos));
        let b_first = bar_idx(s0);
        let b_last = bar_idx(inside[inside.len() - 1].pos);
        let nbars = (bar_idx(s1.saturating_sub(1)).max(b_last) - b_first + 1).max(1);
        let _ = b0;
        let mut reg = Vec::new();
        let mut spn = Vec::new();
        let mut dens = Vec::new();
        for w in (b_first..b_first + nbars).step_by(2) {
            let (ws, _) = grid.get(w).copied().unwrap_or((0, 0));
            let we = grid.get(w + 2).map_or(u64::MAX, |g| g.0);
            let mut ps: Vec<u8> = inside
                .iter()
                .filter(|n| ws <= n.pos && n.pos < we)
                .map(|n| n.pitch)
                .collect();
            let beats = (we.min(s1).saturating_sub(ws)) as f64 / PPQ as f64;
            dens.push(round2(ps.len() as f64 / beats.max(1.0)));
            if ps.is_empty() {
                reg.push(0);
                spn.push(0);
            } else {
                spn.push(ps.iter().max().unwrap_or(&0) - ps.iter().min().unwrap_or(&0));
                reg.push(median_u8(&mut ps));
            }
        }
        let centers: Vec<f64> = reg.iter().filter(|&&c| c > 0).map(|&c| c as f64).collect();
        let bar_centers: Vec<u8> = (b_first..b_first + nbars)
            .filter_map(|b| {
                let (ws, _) = grid.get(b).copied()?;
                let we = grid.get(b + 1).map_or(u64::MAX, |g| g.0);
                let mut ps: Vec<u8> = inside
                    .iter()
                    .filter(|n| ws <= n.pos && n.pos < we)
                    .map(|n| n.pitch)
                    .collect();
                (!ps.is_empty()).then(|| median_u8(&mut ps))
            })
            .collect();
        // 2 拍ごとの頭に音の頭がある割合(最初の音から最後の音までの間の頭だけ数える)
        let half_bar_hits = {
            let (lo, hi) = (inside[0].pos, inside[inside.len() - 1].pos);
            let mut heads = 0usize;
            let mut hits = 0usize;
            for b in b_first..b_first + nbars {
                let Some(&(bs, bl)) = grid.get(b) else {
                    continue;
                };
                for h in [bs, bs + bl / 2] {
                    if h < lo || h > hi {
                        continue;
                    }
                    heads += 1;
                    if inside.iter().any(|n| n.pos.abs_diff(h) <= 30) {
                        hits += 1;
                    }
                }
            }
            round2(hits as f64 / heads.max(1) as f64)
        };
        let register_travel =
            bar_centers.iter().max().unwrap_or(&0) - bar_centers.iter().min().unwrap_or(&0);
        let active: Vec<f64> = dens.iter().copied().filter(|&d| d > 0.0).collect();
        let dmean = active.iter().sum::<f64>() / active.len().max(1) as f64;
        // 小節のリズムの型(16 分の打点の集合)どうしの似かた
        let mut masks: std::collections::BTreeMap<usize, u64> = Default::default();
        for n in &inside {
            let b = bar_idx(n.pos);
            let bs = grid.get(b).map_or(0, |g| g.0);
            let step = ((n.pos - bs + 120) / 240).min(63);
            *masks.entry(b).or_default() |= 1u64 << step;
        }
        let m: Vec<u64> = masks.values().copied().collect();
        let mut sims = Vec::new();
        for i in 0..m.len() {
            for j in i + 1..m.len() {
                let inter = (m[i] & m[j]).count_ones() as f64;
                let union = (m[i] | m[j]).count_ones().max(1) as f64;
                sims.push(inter / union);
            }
        }
        let rhythm_sameness = round2(sims.iter().sum::<f64>() / sims.len().max(1) as f64);
        let pidx: Vec<usize> = phrases
            .iter()
            .filter(|p| {
                let t = phrase_notes[p.index][0].0;
                s0 <= t && t < s1
            })
            .map(|p| p.index)
            .collect();
        let mut psims = Vec::new();
        for (x, &i) in pidx.iter().enumerate() {
            for &j in &pidx[x + 1..] {
                psims.push(phrase_similarity(&phrase_notes[i], &phrase_notes[j]).0);
            }
        }
        let low = inside.iter().map(|n| n.pitch).min().unwrap_or(0);
        let high = inside.iter().map(|n| n.pitch).max().unwrap_or(0);
        let first_peak = inside
            .iter()
            .find(|n| n.pitch == high)
            .map_or(s0, |n| n.pos);
        let labels: Vec<&str> = pidx.iter().map(|&i| phrases[i].label.as_str()).collect();
        let new = pidx.iter().filter(|&&i| phrases[i].like.is_none()).count();
        sections.push(SectionInfo {
            name,
            start_bar: b_first + 1,
            bars: nbars,
            form: labels.join(" "),
            phrases: pidx.clone(),
            register_curve: reg,
            span_curve: spn,
            density_curve: dens,
            low,
            high,
            register_motion: round2(std_dev(&centers)),
            register_travel,
            density_variation: round2(if dmean > 0.0 {
                std_dev(&active) / dmean
            } else {
                0.0
            }),
            rhythm_sameness,
            phrase_rhythm_sameness: (!psims.is_empty())
                .then(|| round2(psims.iter().sum::<f64>() / psims.len() as f64)),
            peak_at: round2(
                (first_peak - s0) as f64 / (s1.min(end).saturating_sub(s0)).max(1) as f64,
            ),
            peak_count: inside.iter().filter(|n| n.pitch == high).count(),
            novelty: round2(new as f64 / pidx.len().max(1) as f64),
            half_bar_hits,
        });
    }

    // ---- 曲全体: 位置による違い ----
    let endings: Vec<usize> = split_bounds.iter().map(|&(_, b, _)| b).collect();
    let long_endings = phrases.iter().filter(|p| p.ending_ratio >= 1.5).count() as f64
        / phrases.len().max(1) as f64;
    let is_tonic = |p: u8| (p as i32 - key.tonic as i32).rem_euclid(12) == 0;
    let tonic_end = endings
        .iter()
        .filter(|&&i| is_tonic(notes[i].pitch))
        .count() as f64
        / endings.len().max(1) as f64;
    let others: Vec<usize> = (0..notes.len()).filter(|i| !endings.contains(i)).collect();
    let tonic_else = others.iter().filter(|&&i| is_tonic(notes[i].pitch)).count() as f64
        / others.len().max(1) as f64;
    skeleton_idx.sort_unstable();
    skeleton_idx.dedup();
    let song = SongInfo {
        long_endings: round2(long_endings),
        tonic_at_endings: round2(tonic_end),
        tonic_elsewhere: round2(tonic_else),
        skeleton_share: round2(skeleton_idx.len() as f64 / notes.len() as f64),
    };

    // ---- 指摘 ----
    let mut warn = |sev: &'static str, level: &str, what: String, fix: &str| {
        findings.push(Finding {
            severity: sev,
            target: ctx.target.clone(),
            what: format!("[{level}] {what}"),
            fix: fix.to_owned(),
        })
    };
    for s in &sections {
        let one_idea = s.phrases.len() >= 3 && s.form.split(' ').all(|l| l.starts_with('A'));
        let phrase_same = s.phrase_rhythm_sameness.unwrap_or(0.0);
        if s.bars >= 8
            && (s.rhythm_sameness >= 0.9
                || (one_idea && (s.rhythm_sameness >= 0.7 || phrase_same >= 0.7)))
        {
            warn(
                "warn",
                "区間",
                format!(
                    "「{}」: どこもほぼ同じリズムの輪郭(小節どうしの似かた {:.0}%、句どうし {:.0}%、形 {})。細部を変えても全体のリズムの輪郭が変わらない",
                    s.name,
                    s.rhythm_sameness * 100.0,
                    phrase_same * 100.0,
                    s.form
                ),
                "句ごとにリズムを作り直す: 対比する句(B)を入れ、句の長さ・始まりの位置(弱起・裏から)・終わりの長さを句ごとに変え、密度に起伏を付ける",
            );
        } else if s.bars >= 8
            && (s.rhythm_sameness >= 0.7 || (s.phrases.len() >= 3 && phrase_same >= 0.7))
        {
            warn(
                "info",
                "区間",
                format!(
                    "「{}」: リズムが一様(小節どうしの似かた {:.0}%、句どうし {:.0}%)。音の高さの形で句を描き分けている場合は問題ない",
                    s.name,
                    s.rhythm_sameness * 100.0,
                    phrase_same * 100.0
                ),
                "問いと答え・最後の句でリズムを変える(長い音・休み・裏からの入り)",
            );
        }
        let wide = s.high - s.low;
        let centers: Vec<u8> = s
            .register_curve
            .iter()
            .copied()
            .filter(|&c| c > 0)
            .collect();
        let drift = centers.iter().max().unwrap_or(&0) - centers.iter().min().unwrap_or(&0);
        if s.bars >= 8
            && drift <= 5
            && s.register_motion < 2.0
            && wide <= 12
            && s.register_travel < 7
        {
            warn(
                "warn",
                "区間",
                format!(
                    "「{}」: 音域が動かない(2 小節ごとの中心の幅 {} 半音・動き {:.1} 半音、音域 {} 半音)。起伏が無く特徴が出ない",
                    s.name, drift, s.register_motion, wide
                ),
                "区間の中で音域の軌跡を作る(低く始めて上がり、後半に山、戻る)。山の区間は音域を 12 半音以上に",
            );
        }
        if s.bars >= 8
            && s.density_variation < 0.12
            && s.density_curve.iter().filter(|&&d| d > 0.0).count() >= 4
        {
            warn(
                "info",
                "区間",
                format!(
                    "「{}」: 音の密度が変わらない(変動 {:.0}%)",
                    s.name,
                    s.density_variation * 100.0
                ),
                "疎 → 密 → 解放の起伏を付ける(句の終わり・区間の終わりで伸ばす)",
            );
        }
    }
    // 区間どうしの対比(旋律のある区間が 2 つ以上)
    for w in sections.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let ca = a
            .register_curve
            .iter()
            .filter(|&&c| c > 0)
            .map(|&c| c as f64)
            .sum::<f64>()
            / a.register_curve.iter().filter(|&&c| c > 0).count().max(1) as f64;
        let cb = b
            .register_curve
            .iter()
            .filter(|&&c| c > 0)
            .map(|&c| c as f64)
            .sum::<f64>()
            / b.register_curve.iter().filter(|&&c| c > 0).count().max(1) as f64;
        let da = a.density_curve.iter().sum::<f64>() / a.density_curve.len().max(1) as f64;
        let db = b.density_curve.iter().sum::<f64>() / b.density_curve.len().max(1) as f64;
        if (ca - cb).abs() < 2.0 && (da - db).abs() < 0.15 * da.max(db).max(0.1) {
            warn(
                "info",
                "曲",
                format!(
                    "「{}」と「{}」の旋律に対比が無い(音域の中心の差 {:.1} 半音、密度 {:.2} と {:.2})",
                    a.name, b.name, (ca - cb).abs(), da, db
                ),
                "区間ごとに音域帯・密度・リズムの系統を割り当てる(盛り上がる区間は中心を 3〜7 半音上げる)",
            );
        }
    }
    if phrases.len() >= 3 {
        if song.long_endings < 0.34 {
            warn(
                "warn",
                "句",
                format!(
                    "句の終わりで間(長い音か休み)を取る句が少ない({:.0}%)。区切りが聞こえず、句の形が見えない",
                    song.long_endings * 100.0
                ),
                "句の最後の音を句の中の音の 1.5 倍以上に伸ばす。区間の最後の句は特に長く",
            );
        }
        let lens: Vec<String> = phrases.iter().map(|p| format!("{:.1}", p.bars)).collect();
        let starts: Vec<String> = phrases
            .iter()
            .map(|p| format!("{:.1}", p.offset_beats))
            .collect();
        if lens.windows(2).all(|w| w[0] == w[1]) && starts.windows(2).all(|w| w[0] == w[1]) {
            warn(
                "info",
                "句",
                format!("すべての句が同じ長さ({} 小節)・同じ位置から始まる", lens[0]),
                "句の長さを揃えすぎない(2+2+4 など)。弱起や裏から入る句を混ぜる",
            );
        }
        if song.tonic_at_endings <= song.tonic_elsewhere && endings.len() >= 3 {
            warn(
                "info",
                "句",
                format!(
                    "句の終わりで主音に落ち着く割合が、ほかの位置と変わらない({:.0}% と {:.0}%)",
                    song.tonic_at_endings * 100.0,
                    song.tonic_elsewhere * 100.0
                ),
                "区間の最後の句は主音(か主和音の音)で閉じ、途中の句は 2 度・5 度で開く",
            );
        }
    }

    // ---- 要約 ----
    let note_name = crate::chord::note_name;
    let mut summary = Vec::new();
    summary.push(format!(
        "キー {}。{} 句、骨格の音 {:.0}%。句の終わりで間を取る句 {:.0}%、主音で終わる句 {:.0}%(ほかの音 {:.0}%)",
        key_name,
        phrases.len(),
        song.skeleton_share * 100.0,
        song.long_endings * 100.0,
        song.tonic_at_endings * 100.0,
        song.tonic_elsewhere * 100.0
    ));
    for s in &sections {
        let curve: Vec<String> = s
            .register_curve
            .iter()
            .map(|&c| {
                if c == 0 {
                    "・".to_owned()
                } else {
                    note_name(c)
                }
            })
            .collect();
        summary.push(format!(
            "区間「{}」({} 小節目から {} 小節): 形 {}。音域 {}〜{}、2 小節ごとの中心 {}(動き {:.1} 半音)、密度 {}、小節のリズムの似かた {:.0}%、山 {:.0}% の位置に {} 回",
            s.name,
            s.start_bar,
            s.bars,
            if s.form.is_empty() { "―" } else { &s.form },
            note_name(s.low),
            note_name(s.high),
            curve.join("→"),
            s.register_motion,
            s.density_curve
                .iter()
                .map(|d| format!("{d:.1}"))
                .collect::<Vec<_>>()
                .join("→"),
            s.rhythm_sameness * 100.0,
            s.peak_at * 100.0,
            s.peak_count
        ));
    }
    for p in &phrases {
        summary.push(format!(
            "句 {}「{}」{} 小節目{}・{:.1} 小節: {}〜{}、輪郭 {}、密度 {:.1}、終わり {}(間 {:.1} 倍){}",
            p.index + 1,
            p.label,
            p.start_bar,
            if p.offset_beats < 0.0 {
                format!("(弱起 {:.1} 拍)", -p.offset_beats)
            } else if p.offset_beats > 0.0 {
                format!("({:.1} 拍目から)", p.offset_beats + 1.0)
            } else {
                String::new()
            },
            p.bars,
            note_name(p.low),
            note_name(p.high),
            p.contour,
            p.density,
            if p.ending_degree == 0 {
                "調の外".to_owned()
            } else {
                format!("{} 度", p.ending_degree)
            },
            p.ending_ratio,
            p.like
                .map(|j| format!("、句 {} に似る({:.0}%)", j + 1, p.similarity * 100.0))
                .unwrap_or_default()
        ));
    }
    Structure {
        key: key_name,
        phrases,
        sections,
        song,
        skeleton: skeleton_idx
            .iter()
            .map(|&i| (notes[i].pos, notes[i].pitch))
            .collect(),
        findings,
        summary,
    }
}

impl Structure {
    /// 分析から旋律の計画(区間 → 句 → 骨格)を推定する。今の旋律を計画として保存し、計画を直して作り直す出発点
    pub fn to_plan(&self) -> crate::plan::MelodyPlan {
        use crate::plan::{MelodyPlan, PhrasePlan, RegisterPoint, SectionPlan};
        let name = crate::chord::note_name;
        let len = |d: u64| -> &'static str {
            const L: [(u64, &str); 10] = [
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
            L.iter()
                .min_by_key(|(t, _)| (*t as i64 - d as i64).abs())
                .map_or("q", |x| x.1)
        };
        let half = |x: f64| (x * 2.0).round() / 2.0;
        let sections = self
            .sections
            .iter()
            .map(|sec| {
                let register = sec
                    .register_curve
                    .iter()
                    .zip(&sec.span_curve)
                    .enumerate()
                    .filter(|(_, (&c, _))| c > 0)
                    .map(|(i, (&c, &w))| RegisterPoint {
                        at: (2 * i) as f64,
                        center: name(c),
                        span: Some(w),
                    })
                    .collect();
                let phrases: Vec<PhrasePlan> = sec
                    .phrases
                    .iter()
                    .map(|&k| {
                        let p = &self.phrases[k];
                        let sk: Vec<(u64, u8)> = self
                            .skeleton
                            .iter()
                            .copied()
                            .filter(|&(t, _)| p.start <= t && t < p.end)
                            .collect();
                        let skeleton = sk
                            .iter()
                            .enumerate()
                            .map(|(i, &(t, pitch))| {
                                let next = sk.get(i + 1).map_or(p.end, |x| x.0);
                                format!("{}:{}", name(pitch), len(next.saturating_sub(t)))
                            })
                            .collect();
                        PhrasePlan {
                            label: p.label.clone(),
                            bars: half(p.bars).max(0.5),
                            offset_beats: p.offset_beats,
                            like: p.like.map(|j| self.phrases[j].label.clone()),
                            transform: vec![],
                            cadence: Some(
                                if p.ending_degree == 1 {
                                    "closed"
                                } else {
                                    "open"
                                }
                                .to_owned(),
                            ),
                            ending_degree: (p.ending_degree > 0).then_some(p.ending_degree),
                            contour: Some(p.contour.to_owned()),
                            density: Some(p.density),
                            skeleton,
                            note: None,
                        }
                    })
                    .collect();
                SectionPlan {
                    name: sec.name.clone(),
                    start_bar: sec.start_bar as u32,
                    bars: sec.bars as u32,
                    energy: None,
                    register,
                    density: phrases.iter().filter_map(|p| p.density).collect(),
                    rhythm_family: None,
                    phrases,
                    handoff: None,
                    like: None,
                    note: None,
                }
            })
            .collect();
        MelodyPlan {
            key: Some(self.key.clone()),
            sections,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::{parse, Key};
    use crate::melody::genre;
    use crate::model::Project;

    const BAR: u64 = 3840;

    fn n(pos: u64, dur: u64, pitch: u8) -> MelNote {
        MelNote { pos, dur, pitch }
    }

    fn ctx<'a>(p: &'a Project, f: &'a dyn Fn(u64) -> Option<Vec<u8>>, key: &str) -> Context<'a> {
        Context {
            project: p,
            chord_at: f,
            key: Key::parse(key),
            genre: genre("pop").unwrap(),
            sections: vec![],
            target: "Lead".into(),
        }
    }

    /// 「歓喜の歌」(パブリックドメイン)の 16 小節。4 小節の句が A A′ B A′ に並ぶ
    fn ode() -> Vec<MelNote> {
        let (q, h, dq, e) = (960, 1920, 1440, 480);
        let (c, d, e4, f, g) = (60u8, 62u8, 64u8, 65u8, 67u8);
        let g3 = 55u8;
        let seq: Vec<(u64, u8)> = vec![
            // A
            (q, e4),
            (q, e4),
            (q, f),
            (q, g),
            (q, g),
            (q, f),
            (q, e4),
            (q, d),
            (q, c),
            (q, c),
            (q, d),
            (q, e4),
            (dq, e4),
            (e, d),
            (h, d),
            // A′
            (q, e4),
            (q, e4),
            (q, f),
            (q, g),
            (q, g),
            (q, f),
            (q, e4),
            (q, d),
            (q, c),
            (q, c),
            (q, d),
            (q, e4),
            (dq, d),
            (e, c),
            (h, c),
            // B
            (q, d),
            (q, d),
            (q, e4),
            (q, c),
            (q, d),
            (e, e4),
            (e, f),
            (q, e4),
            (q, c),
            (q, d),
            (e, e4),
            (e, f),
            (q, e4),
            (q, d),
            (q, c),
            (q, d),
            (h, g3),
            // A′
            (q, e4),
            (q, e4),
            (q, f),
            (q, g),
            (q, g),
            (q, f),
            (q, e4),
            (q, d),
            (q, c),
            (q, c),
            (q, d),
            (q, e4),
            (dq, d),
            (e, c),
            (h, c),
        ];
        let mut t = 0;
        let mut v = Vec::new();
        for (dur, p) in seq {
            // 句の終わり(2 分音符)は少し短く切って息継ぎ
            let sounding = if dur == h { dur - e } else { dur };
            v.push(n(t, sounding, p));
            t += dur;
        }
        v
    }

    #[test]
    fn ode_to_joy_has_four_phrases_in_aaba_form() {
        let p = Project::new("m");
        let f = |_t: u64| None;
        let s = analyze(&ode(), &ctx(&p, &f, "C major"));
        let labels: Vec<&str> = s.phrases.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(s.phrases.len(), 4, "{:#?}", s.summary);
        assert_eq!(labels[0], "A");
        assert!(labels[1].starts_with('A'), "{labels:?}");
        assert!(labels[2].starts_with('B'), "{labels:?}");
        assert!(labels[3].starts_with('A'), "{labels:?}");
        assert_eq!(s.phrases[3].like, Some(1), "{:#?}", s.phrases);
        // 句の終わりは伸び、最後は主音
        assert!(s.song.long_endings >= 0.99, "{:?}", s.song);
        assert_eq!(s.phrases[3].ending_degree, 1);
        // 骨格は音の一部だけ
        assert!(s.song.skeleton_share < 0.7, "{:?}", s.song);
        assert!(
            !s.findings.iter().any(|f| f.severity == "warn"),
            "{:#?}\n{:#?}",
            s.findings,
            s.summary
        );
    }

    #[test]
    fn the_analysis_becomes_a_valid_plan() {
        let p = Project::new("m");
        let f = |_t: u64| None;
        let s = analyze(&ode(), &ctx(&p, &f, "C major"));
        let plan = s.to_plan();
        plan.validate().unwrap();
        let sec = &plan.sections[0];
        let labels: Vec<&str> = sec.phrases.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["A", "A′", "B", "A′"]);
        assert_eq!(sec.phrases[3].like.as_deref(), Some("A′"));
        assert_eq!(sec.phrases[3].cadence.as_deref(), Some("closed"));
        assert_eq!(sec.phrases[0].bars, 4.0);
        // 骨格は句の頭の音から(歓喜の歌は E4 から)
        assert!(
            sec.phrases[0].skeleton[0].starts_with("E4:"),
            "{:?}",
            sec.phrases[0].skeleton
        );
        assert_eq!(sec.register.len(), 8);
        // JSON を経ても計画として読める
        let v = serde_json::to_value(&plan).unwrap();
        let back: crate::plan::MelodyPlan = serde_json::from_value(v).unwrap();
        assert_eq!(back, plan);
    }

    #[test]
    fn the_house_lead_is_flagged_at_the_section_level() {
        let p = Project::new("m");
        let f = |t: u64| {
            let names = ["Am9", "Fmaj7", "Cmaj7", "G6"];
            Some(
                parse(names[((t / BAR) % 4) as usize])
                    .unwrap()
                    .unwrap()
                    .pitch_classes(),
            )
        };
        let v: Vec<MelNote> = crate::melody::tests::HOUSE_LEAD
            .iter()
            .map(|&(a, b, c)| n(a, b, c))
            .collect();
        let s = analyze(&v, &ctx(&p, &f, "A minor"));
        let whats: Vec<&str> = s.findings.iter().map(|f| f.what.as_str()).collect();
        assert!(
            whats
                .iter()
                .any(|w| w.starts_with("[区間]") && w.contains("同じリズム")),
            "{whats:?}\n{:#?}",
            s.summary
        );
        assert!(
            whats
                .iter()
                .any(|w| w.contains("間(長い音か休み)を取る句が少ない")),
            "{whats:?}"
        );
        assert!(
            whats.iter().any(|w| w.contains("音域が動かない")),
            "{whats:?}"
        );
    }

    #[test]
    fn the_skeleton_keeps_the_first_and_last_note_and_drops_passing_tones() {
        let p = Project::new("m");
        let f = |_t: u64| Some(vec![0, 4, 7]);
        // C E(8 分) D(16 分・経過) E(付点) G(2 分)
        let v = vec![
            n(0, 960, 60),
            n(960, 480, 64),
            n(1440, 240, 62),
            n(1680, 240, 64),
            n(1920, 1920, 67),
        ];
        let s = analyze(&v, &ctx(&p, &f, "C major"));
        let sk: Vec<u8> = s.skeleton.iter().map(|x| x.1).collect();
        assert_eq!(sk.first(), Some(&60));
        assert_eq!(sk.last(), Some(&67));
        assert!(!sk.contains(&62), "{sk:?}");
    }
}
