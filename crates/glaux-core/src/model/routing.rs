//! エフェクトのつながり(ノード表示の線)。
//!
//! トラック(とマスター)のエフェクトは、`fx_links` があれば「入力 → … → 出口」の**有向グラフ**として鳴る。
//! - 1 つの口から何本でも出せる(分岐 = 同じ音を配る)。1 つの口に何本でも入れられる(合流 = 足し合わせ)
//! - 入力から出口まで線でたどれるエフェクトだけが鳴る。たどれないものは設定を残したまま鳴らない
//! - 線ごとに音量(dB)を持てる。輪になるつなぎ方はできない
//!
//! `fx_links` が無い(`None`)トラックは、今までどおり「並び順の直列(外してある `parked` を除く)」。
//! [`effective_links`] はどちらの場合も同じ形(線の一覧)で返すので、エンジンと画面はこれだけを見ればよい。

use super::track::Effect;
use crate::id::FxId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{HashMap, HashSet};

/// 線の端: 入力(音源・受けた音)/ 出口(音量・パンへ)/ エフェクト。JSON では `"in"` / `"out"` / `"fx_xxxxxx"`
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum FxNode {
    Input,
    Output,
    Fx(FxId),
}

impl Serialize for FxNode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            FxNode::Input => s.serialize_str("in"),
            FxNode::Output => s.serialize_str("out"),
            FxNode::Fx(id) => s.serialize_str(id.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for FxNode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "in" => Ok(FxNode::Input),
            "out" => Ok(FxNode::Output),
            _ => FxId::parse(&s)
                .map(FxNode::Fx)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl std::fmt::Display for FxNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FxNode::Input => f.write_str("in"),
            FxNode::Output => f.write_str("out"),
            FxNode::Fx(id) => write!(f, "{id}"),
        }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// 線 1 本(from の出口 → to の入口)。`gain_db` はこの線を通る音の量
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FxLink {
    pub from: FxNode,
    pub to: FxNode,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain_db: f32,
}

impl FxLink {
    pub fn new(from: FxNode, to: FxNode) -> Self {
        FxLink {
            from,
            to,
            gain_db: 0.0,
        }
    }
}

/// ノード表示での入力と出口の置き場所 [x, y](画面の表示だけ。音には関係しない)
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct FxIoPos {
    pub input: [f32; 2],
    pub output: [f32; 2],
}

/// 実際に使う線の一覧。`links` が無ければ、並び順の直列(外してあるものを除く)を線にして返す。
pub fn effective_links(effects: &[Effect], links: Option<&[FxLink]>) -> Vec<FxLink> {
    if let Some(l) = links {
        return l.to_vec();
    }
    let mut out = Vec::new();
    let mut prev = FxNode::Input;
    for e in effects.iter().filter(|e| !e.ui.parked) {
        let n = FxNode::Fx(e.id.clone());
        out.push(FxLink::new(prev, n.clone()));
        prev = n;
    }
    out.push(FxLink::new(prev, FxNode::Output));
    out
}

fn reach(links: &[FxLink], start: &FxNode, forward: bool) -> HashSet<FxNode> {
    let mut seen = HashSet::from([start.clone()]);
    let mut stack = vec![start.clone()];
    while let Some(n) = stack.pop() {
        for l in links {
            let (a, b) = if forward {
                (&l.from, &l.to)
            } else {
                (&l.to, &l.from)
            };
            if a == &n && seen.insert(b.clone()) {
                stack.push(b.clone());
            }
        }
    }
    seen
}

/// 鳴るエフェクト(入力から出口まで線でたどれるもの)
pub fn sounding(links: &[FxLink]) -> HashSet<FxId> {
    let from_in = reach(links, &FxNode::Input, true);
    let to_out = reach(links, &FxNode::Output, false);
    from_in
        .intersection(&to_out)
        .filter_map(|n| match n {
            FxNode::Fx(id) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// 鳴るエフェクトを処理の順(つながりの順。同じ段では `effects` の並び順)に並べる
pub fn processing_order(effects: &[Effect], links: &[FxLink]) -> Vec<FxId> {
    let on = sounding(links);
    let ids: Vec<&FxId> = effects
        .iter()
        .map(|e| &e.id)
        .filter(|id| on.contains(*id))
        .collect();
    let mut indeg: HashMap<&FxId, usize> = ids.iter().map(|id| (*id, 0)).collect();
    for l in links {
        if let (FxNode::Fx(a), FxNode::Fx(b)) = (&l.from, &l.to) {
            if on.contains(a) {
                if let Some(d) = indeg.get_mut(b) {
                    *d += 1;
                }
            }
        }
    }
    let mut out: Vec<FxId> = Vec::with_capacity(ids.len());
    let mut done: HashSet<&FxId> = HashSet::new();
    // 入次数 0 のもののうち、並び順で先のものから(小さいので素直に探す)
    while out.len() < ids.len() {
        let Some(next) = ids
            .iter()
            .find(|id| !done.contains(**id) && indeg[**id] == 0)
            .copied()
        else {
            break; // 輪(検証で弾いているので来ない)
        };
        done.insert(next);
        out.push(next.clone());
        for l in links {
            if let (FxNode::Fx(a), FxNode::Fx(b)) = (&l.from, &l.to) {
                if a == next {
                    if let Some(d) = indeg.get_mut(b) {
                        *d = d.saturating_sub(1);
                    }
                }
            }
        }
    }
    out
}

/// 線の一覧が正しいか(端のエフェクトがある・向き・重複・輪・音量)
pub fn validate_links(effects: &[Effect], links: &[FxLink]) -> Result<(), String> {
    let ids: HashSet<&FxId> = effects.iter().map(|e| &e.id).collect();
    let mut seen = HashSet::new();
    for l in links {
        for n in [&l.from, &l.to] {
            if let FxNode::Fx(id) = n {
                if !ids.contains(id) {
                    return Err(format!("線の端のエフェクトがありません: {id}"));
                }
            }
        }
        if l.from == FxNode::Output || l.to == FxNode::Input {
            return Err(format!("線の向きが逆です: {} → {}", l.from, l.to));
        }
        if l.from == l.to {
            return Err(format!("自分自身にはつなげません: {}", l.from));
        }
        if !l.gain_db.is_finite() || !(-120.0..=24.0).contains(&l.gain_db) {
            return Err(format!("線の音量が範囲外です: {} dB", l.gain_db));
        }
        if !seen.insert((l.from.clone(), l.to.clone())) {
            return Err(format!("同じ線が 2 本あります: {} → {}", l.from, l.to));
        }
    }
    // 輪: どの線についても、to から from へ戻れないこと
    for l in links {
        if reach(links, &l.to, true).contains(&l.from) {
            return Err(format!("輪になるつなぎ方です: {} → {}", l.from, l.to));
        }
    }
    Ok(())
}

/// エフェクトを出口の直前に入れる(出口へ入っていた線をこのエフェクトへ付け替え、このエフェクト → 出口)。
/// 出口へ入る線が 1 本も無ければ、入力 → エフェクト → 出口 にする
pub fn insert_before_output(links: &[FxLink], id: &FxId) -> Vec<FxLink> {
    let node = FxNode::Fx(id.clone());
    let mut out: Vec<FxLink> = Vec::with_capacity(links.len() + 1);
    let mut redirected = false;
    for l in links {
        if l.to == FxNode::Output {
            redirected = true;
            if !out.iter().any(|x| x.from == l.from && x.to == node) {
                out.push(FxLink {
                    from: l.from.clone(),
                    to: node.clone(),
                    gain_db: l.gain_db,
                });
            }
        } else {
            out.push(l.clone());
        }
    }
    if !redirected {
        out.push(FxLink::new(FxNode::Input, node.clone()));
    }
    out.push(FxLink::new(node, FxNode::Output));
    out
}

/// エフェクトの線を全部外し、前後をつなぎ直す(a → X → b を a → b に。音量は足す。すでにある線は重ねない)
pub fn unlink_bridging(links: &[FxLink], id: &FxId) -> Vec<FxLink> {
    let node = FxNode::Fx(id.clone());
    let ins: Vec<&FxLink> = links.iter().filter(|l| l.to == node).collect();
    let outs: Vec<&FxLink> = links.iter().filter(|l| l.from == node).collect();
    let mut out: Vec<FxLink> = links
        .iter()
        .filter(|l| l.from != node && l.to != node)
        .cloned()
        .collect();
    for a in &ins {
        for b in &outs {
            if a.from == b.to || out.iter().any(|x| x.from == a.from && x.to == b.to) {
                continue;
            }
            out.push(FxLink {
                from: a.from.clone(),
                to: b.to.clone(),
                gain_db: (a.gain_db + b.gain_db).clamp(-120.0, 24.0),
            });
        }
    }
    out
}

/// 線 from → to の間にエフェクトを入れる(from → X は元の音量、X → to は 0 dB)。その線が無ければ None
pub fn split_link(links: &[FxLink], from: &FxNode, to: &FxNode, id: &FxId) -> Option<Vec<FxLink>> {
    let i = links.iter().position(|l| &l.from == from && &l.to == to)?;
    let node = FxNode::Fx(id.clone());
    let mut out = links.to_vec();
    let gain = out[i].gain_db;
    out.remove(i);
    out.push(FxLink {
        from: from.clone(),
        to: node.clone(),
        gain_db: gain,
    });
    out.push(FxLink::new(node, to.clone()));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(n: &str) -> Effect {
        Effect::builtin(FxId::new(), n)
    }
    fn id(e: &Effect) -> FxNode {
        FxNode::Fx(e.id.clone())
    }

    #[test]
    fn serial_links_skip_parked_and_keep_order() {
        let (a, mut b, c) = (fx("eq"), fx("delay"), fx("reverb"));
        b.ui.parked = true;
        let effects = vec![a.clone(), b.clone(), c.clone()];
        let l = effective_links(&effects, None);
        assert_eq!(
            l,
            vec![
                FxLink::new(FxNode::Input, id(&a)),
                FxLink::new(id(&a), id(&c)),
                FxLink::new(id(&c), FxNode::Output),
            ]
        );
        assert_eq!(
            processing_order(&effects, &l),
            vec![a.id.clone(), c.id.clone()]
        );
        assert!(!sounding(&l).contains(&b.id));
    }

    #[test]
    fn branches_merge_and_dead_ends() {
        let (eq, rev, dly, lone) = (fx("eq"), fx("reverb"), fx("delay"), fx("tape"));
        let effects = vec![rev.clone(), eq.clone(), dly.clone(), lone.clone()];
        let links = vec![
            FxLink::new(FxNode::Input, id(&eq)),
            FxLink::new(id(&eq), FxNode::Output),
            FxLink::new(id(&eq), id(&rev)),
            FxLink {
                from: id(&rev),
                to: FxNode::Output,
                gain_db: -8.0,
            },
            // 入力から来ているが出口に届かない
            FxLink::new(id(&eq), id(&dly)),
            // どこにもつながっていない
        ];
        validate_links(&effects, &links).unwrap();
        let on = sounding(&links);
        assert!(on.contains(&eq.id) && on.contains(&rev.id));
        assert!(!on.contains(&dly.id) && !on.contains(&lone.id));
        // 並び順では rev が先だが、つながりの順で eq が先
        assert_eq!(
            processing_order(&effects, &links),
            vec![eq.id.clone(), rev.id.clone()]
        );
        // JSON は "in" / "out" / fx の ID
        let j = serde_json::to_value(&links[3]).unwrap();
        assert_eq!(j["to"], "out");
        assert_eq!(j["gain_db"], -8.0);
        assert!(serde_json::to_value(&links[0])
            .unwrap()
            .get("gain_db")
            .is_none());
    }

    #[test]
    fn rejects_cycles_duplicates_and_dangling() {
        let (a, b) = (fx("eq"), fx("delay"));
        let effects = vec![a.clone(), b.clone()];
        let cyc = vec![FxLink::new(id(&a), id(&b)), FxLink::new(id(&b), id(&a))];
        assert!(validate_links(&effects, &cyc).is_err());
        let dup = vec![FxLink::new(id(&a), id(&b)), FxLink::new(id(&a), id(&b))];
        assert!(validate_links(&effects, &dup).is_err());
        let back = vec![FxLink::new(FxNode::Output, id(&a))];
        assert!(validate_links(&effects, &back).is_err());
        let gone = vec![FxLink::new(FxNode::Input, FxNode::Fx(FxId::new()))];
        assert!(validate_links(&effects, &gone).is_err());
    }

    #[test]
    fn insert_and_bridge() {
        let (a, b, c) = (fx("eq"), fx("reverb"), fx("delay"));
        let links = vec![
            FxLink::new(FxNode::Input, id(&a)),
            FxLink::new(id(&a), FxNode::Output),
            FxLink {
                from: FxNode::Input,
                to: FxNode::Output,
                gain_db: -6.0,
            },
        ];
        // 出口へ入っていた 2 本を b へ付け替え、b → 出口
        let l = insert_before_output(&links, &b.id);
        assert!(l.contains(&FxLink::new(id(&a), id(&b))));
        assert!(l.contains(&FxLink {
            from: FxNode::Input,
            to: id(&b),
            gain_db: -6.0
        }));
        assert!(l.contains(&FxLink::new(id(&b), FxNode::Output)));
        validate_links(&[a.clone(), b.clone()], &l).unwrap();
        // b を抜くと元の形に戻る(並びは変わってよい)
        let back = unlink_bridging(&l, &b.id);
        assert_eq!(back.len(), 3);
        assert!(back.contains(&FxLink::new(id(&a), FxNode::Output)));
        // 線の無いトラックへの追加
        let l2 = insert_before_output(&[], &c.id);
        assert_eq!(
            l2,
            vec![
                FxLink::new(FxNode::Input, id(&c)),
                FxLink::new(id(&c), FxNode::Output)
            ]
        );
    }
}
