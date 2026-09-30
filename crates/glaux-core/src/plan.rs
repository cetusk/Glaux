//! 旋律の計画(設計データ)と、その編集コマンド。
//!
//! 計画は曲とは別の文書([`PlanSet`])として持ち、曲と同じ git ライクな履歴([`crate::Session`])で管理する。
//! git に例えると、`plans.json` が作業ツリー、`plans.history.jsonl` がコミットの列、`plans.base.json` が起点。
//! 曲の側はクリップごとに「どの計画の、どの版から作った音符か」([`PlanRef`])を `Project::plan_refs` に持つ
//! (サブモジュールの参照)。計画の版と参照の版が違えば「計画が先に進んでいる(作り直し待ち)」。
//!
//! - 計画の中身(`body`)は JSON。`kind` が `"melody"` なら [`MelodyPlan`] として読めることを適用のたびに確かめる。
//!   無い項目は省略でき、知らない項目は残す(古い計画も新しい計画も読める)
//! - 編集は [`PlanCommand`]: 作る・消す・丸ごと置き換える・JSON Pointer の道で一部を変える。すべて絶対値で、
//!   版(`rev`)もコマンドを作る側が決める(決定性)。逆コマンドを返す
//! - 履歴の 1 件は件名に加えて経緯([`crate::EntryNote`]: なぜ・きっかけ・前後の測定)を持つ

use crate::error::{CoreError, Result};
use crate::history::{Document, HistoryCommand};
use crate::id::PlanId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const PLANS_FORMAT: &str = "glaux-plans";
pub const PLANS_VERSION: u32 = 1;

/// 計画の版への参照(クリップがどの版から作られたか・派生元)
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct PlanRef {
    pub id: PlanId,
    pub rev: u64,
    /// 中身の指紋([`Plan::digest`])。undo の後に同じ版の番号で別の中身になっても見分ける
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub digest: String,
}

/// 計画 1 つ
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub id: PlanId,
    pub name: String,
    /// 計画の種類(今は "melody" だけ)
    pub kind: String,
    /// 版(変えるたびに 1 つ上がる。値はコマンドを作る側が決める)
    pub rev: u64,
    /// 別案として派生した元(git の branch)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<PlanRef>,
    pub body: Value,
}

impl Plan {
    /// 中身(名前・種類・本体)の指紋。16 桁の 16 進(FNV-1a 64)。版の番号と派生元は含めない
    pub fn digest(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            h ^= 0xff;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        };
        feed(self.name.as_bytes());
        feed(self.kind.as_bytes());
        // serde_json の Map は BTreeMap(preserve_order を使っていない)なので、同じ中身は同じ文字列になる
        feed(self.body.to_string().as_bytes());
        format!("{h:016x}")
    }

    pub fn reference(&self) -> PlanRef {
        PlanRef {
            id: self.id.clone(),
            rev: self.rev,
            digest: self.digest(),
        }
    }

    /// 中身が種類の形に合うか
    pub fn validate(&self) -> Result<()> {
        if !self.body.is_object() {
            return Err(CoreError::InvalidPlan(
                "body は JSON のオブジェクト".to_owned(),
            ));
        }
        match self.kind.as_str() {
            "melody" => {
                let m: MelodyPlan = serde_json::from_value(self.body.clone()).map_err(|e| {
                    CoreError::InvalidPlan(format!("melody の計画として読めない: {e}"))
                })?;
                m.validate()
            }
            k => Err(CoreError::InvalidPlan(format!(
                "計画の種類が不明: {k}(使えるもの: melody)"
            ))),
        }
    }
}

/// すべての計画(`plans.json` の中身)
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PlanSet {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub plans: BTreeMap<PlanId, Plan>,
}

impl Default for PlanSet {
    fn default() -> Self {
        PlanSet {
            format: PLANS_FORMAT.to_owned(),
            version: PLANS_VERSION,
            plans: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------- 旋律の計画の形

/// 旋律の計画。区間(L0)→ 句 → 骨格の順に上から決める(docs の層の設計)。どの項目も省略できる
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct MelodyPlan {
    /// 置くトラックの ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    /// 役割(lead / chorus / verse など、write_melody の role)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// キー("A minor" など)と和音の進行(write_chords の書き方)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chords: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// 言葉での目標(作曲者の言葉をそのまま)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// 表情(強さ・切り方・ビブラート・グライド・ノリ)。省略で既定(量 0.6・tight)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<crate::melexpr::Expression>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<SectionPlan>,
}

/// 区間の計画(L0)
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct SectionPlan {
    pub name: String,
    /// 始まりの小節(1 始まり)と長さ(小節)
    pub start_bar: u32,
    pub bars: u32,
    /// 盛り上がり 0〜10
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<f32>,
    /// 音域の軌跡(区間の中の小節の位置ごとの中心と幅)。区間で最高音は 1 回
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub register: Vec<RegisterPoint>,
    /// 密度の曲線(1 拍あたりの音の数。句ごと、または区間を等分した点)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub density: Vec<f64>,
    /// リズムの系統: sustain(伸ばす中心)/ pulse(刻む中心)/ syncopated / sparse(休み多め)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rhythm_family: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phrases: Vec<PhrasePlan>,
    /// 前後の区間との受け渡し(引き継ぐ・跳んで入る・弱起 など)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// 前の区間への参照と変え方(ドロップ 2 = ドロップ 1 を「音域を上げる」など)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 音域の軌跡の 1 点
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct RegisterPoint {
    /// 区間の中の小節の位置(0 始まり、小数可)
    pub at: f64,
    /// 中心の音("E5" など)と幅(半音)
    pub center: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<u8>,
}

/// 句の計画
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct PhrasePlan {
    /// 名前(A・A′・B …)
    pub label: String,
    /// 長さ(小節)と、小節の頭からのずれ(拍。負なら弱起)
    pub bars: f64,
    #[serde(default)]
    pub offset_beats: f64,
    /// 似せる句の名前と変え方(移高・末尾の差し替え・リズムの分割 など)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub like: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transform: Vec<String>,
    /// 終止: open(開く)/ closed(閉じる)と、終わりの音度(1〜7)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cadence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending_degree: Option<u8>,
    /// 輪郭: arch / rise / fall / valley / flat
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contour: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<f64>,
    /// 骨格の音("E5:h" のように音と長さ。write_melody の motif と同じ書き方)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skeleton: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

const RHYTHM_FAMILIES: &[&str] = &["sustain", "pulse", "syncopated", "sparse"];
const CADENCES: &[&str] = &["open", "closed"];
const CONTOURS: &[&str] = &["arch", "rise", "fall", "valley", "flat"];

impl MelodyPlan {
    pub fn validate(&self) -> Result<()> {
        let bad = |s: String| Err(CoreError::InvalidPlan(s));
        if let Some(e) = &self.expression {
            if !(0.0..=1.0).contains(&e.amount) {
                return bad("expression/amount は 0〜1".to_owned());
            }
            if !crate::melexpr::FEELS.contains(&e.feel.as_str()) {
                return bad(format!(
                    "expression/feel は {} のどれか",
                    crate::melexpr::FEELS.join(" / ")
                ));
            }
            if !(1..=127).contains(&e.velocity) {
                return bad("expression/velocity は 1〜127".to_owned());
            }
        }
        for (i, s) in self.sections.iter().enumerate() {
            let at = format!("sections/{i}");
            if s.start_bar == 0 || s.bars == 0 {
                return bad(format!("{at}: start_bar と bars は 1 以上"));
            }
            if s.energy.is_some_and(|e| !(0.0..=10.0).contains(&e)) {
                return bad(format!("{at}: energy は 0〜10"));
            }
            if let Some(f) = &s.rhythm_family {
                if !RHYTHM_FAMILIES.contains(&f.as_str()) {
                    return bad(format!(
                        "{at}: rhythm_family は {} のどれか",
                        RHYTHM_FAMILIES.join(" / ")
                    ));
                }
            }
            if s.density.iter().any(|d| !(0.0..=16.0).contains(d)) {
                return bad(format!("{at}: density は 1 拍あたり 0〜16"));
            }
            for (k, r) in s.register.iter().enumerate() {
                if crate::chord::parse_note(&r.center).is_none() {
                    return bad(format!(
                        "{at}/register/{k}: center は \"E5\" のような音名(got: {})",
                        r.center
                    ));
                }
                if r.at < 0.0 || r.at > s.bars as f64 {
                    return bad(format!("{at}/register/{k}: at は 0〜{}", s.bars));
                }
            }
            for (k, p) in s.phrases.iter().enumerate() {
                let at = format!("{at}/phrases/{k}");
                if !(p.bars > 0.0 && p.bars <= 64.0) {
                    return bad(format!("{at}: bars は 0 より大きく 64 以下"));
                }
                if p.offset_beats.abs() > 8.0 {
                    return bad(format!("{at}: offset_beats は ±8 拍まで"));
                }
                if let Some(c) = &p.cadence {
                    if !CADENCES.contains(&c.as_str()) {
                        return bad(format!("{at}: cadence は open / closed"));
                    }
                }
                if let Some(c) = &p.contour {
                    if !CONTOURS.contains(&c.as_str()) {
                        return bad(format!(
                            "{at}: contour は {} のどれか",
                            CONTOURS.join(" / ")
                        ));
                    }
                }
                if p.ending_degree.is_some_and(|d| !(1..=7).contains(&d)) {
                    return bad(format!("{at}: ending_degree は 1〜7"));
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- コマンド

/// 計画の一部の編集(JSON Pointer の道。`body` の中を指す。"" は body 全体)
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanOp {
    /// オブジェクトの項目を置く(無ければ足す)・配列の要素を置き換える(道の最後が "-" なら末尾に足す)
    Set { path: String, value: Value },
    /// 配列の位置に差し込む(後ろはずれる)
    Insert { path: String, value: Value },
    /// 項目・要素を消す(配列の後ろは詰める)
    Remove { path: String },
}

impl PlanOp {
    pub fn path(&self) -> &str {
        match self {
            PlanOp::Set { path, .. } | PlanOp::Insert { path, .. } | PlanOp::Remove { path } => {
                path
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanCommand {
    Create {
        plan: Plan,
    },
    Delete {
        id: PlanId,
    },
    /// 丸ごと置き換える(名前・版・派生元も)
    Replace {
        plan: Plan,
    },
    /// 一部を変えて版を `rev` にする
    Edit {
        id: PlanId,
        rev: u64,
        ops: Vec<PlanOp>,
    },
}

impl PlanCommand {
    pub fn plan_id(&self) -> &PlanId {
        match self {
            PlanCommand::Create { plan } | PlanCommand::Replace { plan } => &plan.id,
            PlanCommand::Delete { id } | PlanCommand::Edit { id, .. } => id,
        }
    }

    /// この編集が計画 `id` の `path`(JSON Pointer。"" は全体)に触るか。作る・消す・置き換えるは全体に触る
    pub fn touches(&self, id: &PlanId, path: &str) -> bool {
        if self.plan_id() != id {
            return false;
        }
        match self {
            PlanCommand::Edit { ops, .. } => ops.iter().any(|o| {
                let p = o.path();
                // どちらかがもう一方の先頭(親か子)なら触っている
                is_prefix(p, path) || is_prefix(path, p)
            }),
            _ => true,
        }
    }
}

fn is_prefix(parent: &str, child: &str) -> bool {
    parent.is_empty()
        || child == parent
        || (child.starts_with(parent) && child.as_bytes().get(parent.len()) == Some(&b'/'))
}

impl HistoryCommand for PlanCommand {
    type Target = PlanId;
    fn targets(&self) -> BTreeSet<PlanId> {
        BTreeSet::from([self.plan_id().clone()])
    }
}

/// 計画の変更の通知
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "plan", rename_all = "snake_case")]
pub enum PlanChange {
    Created(PlanId),
    Updated(PlanId),
    Deleted(PlanId),
}

impl Document for PlanSet {
    type Command = PlanCommand;
    type Change = PlanChange;

    fn apply_command(&mut self, cmd: &PlanCommand) -> Result<(PlanCommand, Vec<PlanChange>)> {
        match cmd {
            PlanCommand::Create { plan } => {
                if self.plans.contains_key(&plan.id) {
                    return Err(CoreError::DuplicateId(plan.id.to_string()));
                }
                plan.validate()?;
                self.plans.insert(plan.id.clone(), plan.clone());
                Ok((
                    PlanCommand::Delete {
                        id: plan.id.clone(),
                    },
                    vec![PlanChange::Created(plan.id.clone())],
                ))
            }
            PlanCommand::Delete { id } => {
                let old = self
                    .plans
                    .remove(id)
                    .ok_or_else(|| CoreError::PlanNotFound(id.clone()))?;
                Ok((
                    PlanCommand::Create { plan: old },
                    vec![PlanChange::Deleted(id.clone())],
                ))
            }
            PlanCommand::Replace { plan } => {
                plan.validate()?;
                let slot = self
                    .plans
                    .get_mut(&plan.id)
                    .ok_or_else(|| CoreError::PlanNotFound(plan.id.clone()))?;
                let old = std::mem::replace(slot, plan.clone());
                Ok((
                    PlanCommand::Replace { plan: old },
                    vec![PlanChange::Updated(plan.id.clone())],
                ))
            }
            PlanCommand::Edit { id, rev, ops } => {
                let cur = self
                    .plans
                    .get(id)
                    .ok_or_else(|| CoreError::PlanNotFound(id.clone()))?;
                let mut next = cur.clone();
                let mut inverse = Vec::with_capacity(ops.len());
                for (i, op) in ops.iter().enumerate() {
                    let inv = apply_op(&mut next.body, op).map_err(|e| {
                        CoreError::InvalidPlan(format!("ops/{i}({}): {e}", op.path()))
                    })?;
                    inverse.push(inv);
                }
                inverse.reverse();
                next.rev = *rev;
                next.validate()?;
                let old_rev = cur.rev;
                self.plans.insert(id.clone(), next);
                Ok((
                    PlanCommand::Edit {
                        id: id.clone(),
                        rev: old_rev,
                        ops: inverse,
                    },
                    vec![PlanChange::Updated(id.clone())],
                ))
            }
        }
    }
}

// ---------------------------------------------------------------- JSON Pointer

fn tokens(path: &str) -> std::result::Result<Vec<String>, String> {
    if path.is_empty() {
        return Ok(vec![]);
    }
    let rest = path
        .strip_prefix('/')
        .ok_or_else(|| "道は \"/\" で始める(JSON Pointer)".to_owned())?;
    Ok(rest
        .split('/')
        .map(|t| t.replace("~1", "/").replace("~0", "~"))
        .collect())
}

fn escape(t: &str) -> String {
    t.replace('~', "~0").replace('/', "~1")
}

fn join(parent: &[String], last: &str) -> String {
    let mut s = String::new();
    for t in parent {
        s.push('/');
        s.push_str(&escape(t));
    }
    s.push('/');
    s.push_str(&escape(last));
    s
}

fn walk<'a>(root: &'a mut Value, toks: &[String]) -> std::result::Result<&'a mut Value, String> {
    let mut cur = root;
    for t in toks {
        cur = match cur {
            Value::Object(m) => m.get_mut(t).ok_or_else(|| format!("項目 \"{t}\" が無い"))?,
            Value::Array(a) => {
                let i: usize = t
                    .parse()
                    .map_err(|_| format!("配列の位置が数でない: {t}"))?;
                let len = a.len();
                a.get_mut(i)
                    .ok_or_else(|| format!("配列の位置 {i} が範囲外(長さ {len})"))?
            }
            _ => return Err(format!("\"{t}\" の手前が値(オブジェクトでも配列でもない)")),
        };
    }
    Ok(cur)
}

/// 1 つの編集を適用して、逆の編集を返す。失敗したときは値を変えない
fn apply_op(root: &mut Value, op: &PlanOp) -> std::result::Result<PlanOp, String> {
    let toks = tokens(op.path())?;
    let Some((last, parent_toks)) = toks.split_last() else {
        // body 全体
        return match op {
            PlanOp::Set { value, .. } => {
                let old = std::mem::replace(root, value.clone());
                Ok(PlanOp::Set {
                    path: String::new(),
                    value: old,
                })
            }
            _ => Err("body 全体には set だけ使える".to_owned()),
        };
    };
    let parent = walk(root, parent_toks)?;
    match (op, parent) {
        (PlanOp::Set { path, value }, Value::Object(m)) => {
            Ok(match m.insert(last.clone(), value.clone()) {
                Some(old) => PlanOp::Set {
                    path: path.clone(),
                    value: old,
                },
                None => PlanOp::Remove { path: path.clone() },
            })
        }
        (PlanOp::Set { path, value }, Value::Array(a)) => {
            if last == "-" {
                a.push(value.clone());
                return Ok(PlanOp::Remove {
                    path: join(parent_toks, &(a.len() - 1).to_string()),
                });
            }
            let i = index(last, a.len(), false)?;
            let old = std::mem::replace(&mut a[i], value.clone());
            Ok(PlanOp::Set {
                path: path.clone(),
                value: old,
            })
        }
        (PlanOp::Insert { path, value }, Value::Array(a)) => {
            let i = if last == "-" {
                a.len()
            } else {
                index(last, a.len(), true)?
            };
            a.insert(i, value.clone());
            Ok(PlanOp::Remove {
                path: if last == "-" {
                    join(parent_toks, &i.to_string())
                } else {
                    path.clone()
                },
            })
        }
        (PlanOp::Remove { path }, Value::Object(m)) => {
            let old = m
                .remove(last)
                .ok_or_else(|| format!("項目 \"{last}\" が無い"))?;
            Ok(PlanOp::Set {
                path: path.clone(),
                value: old,
            })
        }
        (PlanOp::Remove { path }, Value::Array(a)) => {
            let i = index(last, a.len(), false)?;
            let old = a.remove(i);
            Ok(PlanOp::Insert {
                path: path.clone(),
                value: old,
            })
        }
        (PlanOp::Insert { .. }, _) => Err("insert は配列の中だけ".to_owned()),
        _ => Err("手前が値(オブジェクトでも配列でもない)".to_owned()),
    }
}

fn index(t: &str, len: usize, allow_end: bool) -> std::result::Result<usize, String> {
    let i: usize = t
        .parse()
        .map_err(|_| format!("配列の位置が数でない: {t}"))?;
    if i < len || (allow_end && i == len) {
        Ok(i)
    } else {
        Err(format!("配列の位置 {i} が範囲外(長さ {len})"))
    }
}

// ---------------------------------------------------------------- 差

/// 2 つの値の差の 1 件(道と前後。無ければ None)
#[derive(Clone, PartialEq, Debug, Serialize)]
pub struct DiffItem {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

/// 2 つの JSON の差(オブジェクトは項目ごと、配列は位置ごと)
pub fn diff(a: &Value, b: &Value) -> Vec<DiffItem> {
    let mut out = Vec::new();
    diff_into(a, b, &mut Vec::new(), &mut out);
    out
}

fn diff_into(a: &Value, b: &Value, at: &mut Vec<String>, out: &mut Vec<DiffItem>) {
    let here = |at: &[String]| -> String { at.iter().map(|t| format!("/{}", escape(t))).collect() };
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for k in keys {
                at.push(k.clone());
                match (x.get(k), y.get(k)) {
                    (Some(p), Some(q)) => diff_into(p, q, at, out),
                    (p, q) => out.push(DiffItem {
                        path: here(at),
                        before: p.cloned(),
                        after: q.cloned(),
                    }),
                }
                at.pop();
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                at.push(i.to_string());
                match (x.get(i), y.get(i)) {
                    (Some(p), Some(q)) => diff_into(p, q, at, out),
                    (p, q) => out.push(DiffItem {
                        path: here(at),
                        before: p.cloned(),
                        after: q.cloned(),
                    }),
                }
                at.pop();
            }
        }
        _ if a != b => out.push(DiffItem {
            path: here(at),
            before: Some(a.clone()),
            after: Some(b.clone()),
        }),
        _ => {}
    }
}

/// 履歴をさかのぼって、計画 `id` の版 `rev` を探す(今の版なら今の計画)。無ければ None
pub fn plan_at_rev(session: &crate::Session<PlanSet>, id: &PlanId, rev: u64) -> Option<Plan> {
    let mut doc = session.doc().clone();
    if let Some(p) = doc.plans.get(id).filter(|p| p.rev == rev) {
        return Some(p.clone());
    }
    for e in session.history().applied().iter().rev() {
        if doc.apply_command(&e.inverse).is_err() {
            return None;
        }
        if let Some(p) = doc.plans.get(id).filter(|p| p.rev == rev) {
            return Some(p.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, EntryNote, Session, Trigger};
    use serde_json::json;

    fn plan(id: &PlanId) -> Plan {
        Plan {
            id: id.clone(),
            name: "ドロップ 1 のリード".into(),
            kind: "melody".into(),
            rev: 1,
            derived_from: None,
            body: json!({
                "genre": "house",
                "sections": [{
                    "name": "Drop 1", "start_bar": 25, "bars": 16,
                    "register": [{ "at": 0, "center": "A4", "span": 8 }],
                    "phrases": [{ "label": "A", "bars": 4 }, { "label": "A′", "bars": 4 }]
                }]
            }),
        }
    }

    fn edit(id: &PlanId, rev: u64, ops: Value) -> PlanCommand {
        PlanCommand::Edit {
            id: id.clone(),
            rev,
            ops: serde_json::from_value(ops).unwrap(),
        }
    }

    #[test]
    fn edits_are_reversible_and_keep_the_revision() {
        let id = PlanId::new();
        let mut s: Session<PlanSet> = Session::new(PlanSet::default());
        s.apply(
            PlanCommand::Create { plan: plan(&id) },
            Author::Human,
            "作る",
        )
        .unwrap();
        let before = s.doc().clone();
        let note = EntryNote {
            why: "音域が動かないと言われた".into(),
            trigger: Some(Trigger {
                kind: "user".into(),
                text: "一定の範囲を出ない".into(),
            }),
            measures: vec![],
        };
        s.apply_with_note(
            edit(
                &id,
                2,
                json!([
                    { "op": "set", "path": "/sections/0/register/0/span", "value": 13 },
                    { "op": "set", "path": "/sections/0/register/-", "value": { "at": 12, "center": "E5" } },
                    { "op": "insert", "path": "/sections/0/phrases/1", "value": { "label": "B", "bars": 2 } },
                    { "op": "remove", "path": "/genre" },
                    { "op": "set", "path": "/intent", "value": "波に乗る" }
                ]),
            ),
            Author::Ai { model: "t".into() },
            "音域の軌跡を上げる",
            note.clone(),
        )
        .unwrap();
        let p = &s.doc().plans[&id];
        assert_eq!(p.rev, 2);
        assert_eq!(p.body["sections"][0]["register"][0]["span"], 13);
        assert_eq!(p.body["sections"][0]["register"][1]["center"], "E5");
        assert_eq!(p.body["sections"][0]["phrases"][1]["label"], "B");
        assert_eq!(p.body["sections"][0]["phrases"][2]["label"], "A′");
        assert!(p.body.get("genre").is_none());
        assert_eq!(s.history().applied()[1].note.as_ref(), Some(&note));
        s.undo().unwrap();
        assert_eq!(s.doc(), &before);
        s.redo().unwrap();
        assert_eq!(s.doc().plans[&id].rev, 2);
        // 前の版は履歴から取り出せる
        let old = plan_at_rev(&s, &id, 1).unwrap();
        assert_eq!(old, before.plans[&id]);
        let d = diff(&old.body, &s.doc().plans[&id].body);
        assert!(
            d.iter().any(|x| x.path == "/sections/0/register/0/span"),
            "{d:?}"
        );
        assert!(
            d.iter().any(|x| x.path == "/genre" && x.after.is_none()),
            "{d:?}"
        );
        // 履歴の JSONL を読み戻して再生すると同じ
        let text = s.history().to_jsonl().unwrap();
        let entries = crate::History::<PlanCommand>::entries_from_jsonl(&text).unwrap();
        let again = Session::replay(PlanSet::default(), entries).unwrap();
        assert_eq!(again.doc(), s.doc());
    }

    #[test]
    fn a_broken_edit_changes_nothing() {
        let id = PlanId::new();
        let mut s: Session<PlanSet> = Session::new(PlanSet::default());
        s.apply(
            PlanCommand::Create { plan: plan(&id) },
            Author::Human,
            "作る",
        )
        .unwrap();
        let before = s.doc().clone();
        for ops in [
            // 2 つ目が範囲外
            json!([
                { "op": "set", "path": "/seed", "value": 3 },
                { "op": "set", "path": "/sections/5/bars", "value": 3 }
            ]),
            // 形に合わない(rhythm_family の値)
            json!([{ "op": "set", "path": "/sections/0/rhythm_family", "value": "wobbly" }]),
            // 音名でない
            json!([{ "op": "set", "path": "/sections/0/register/0/center", "value": "high" }]),
            json!([{ "op": "remove", "path": "" }]),
        ] {
            assert!(s.apply(edit(&id, 2, ops), Author::Human, "x").is_err());
            assert_eq!(s.doc(), &before);
        }
        assert!(s
            .apply(PlanCommand::Create { plan: plan(&id) }, Author::Human, "x")
            .is_err());
    }

    #[test]
    fn touches_matches_parents_and_children() {
        let id = PlanId::new();
        let c = edit(
            &id,
            2,
            json!([{ "op": "set", "path": "/sections/0/register", "value": [] }]),
        );
        assert!(c.touches(&id, "/sections/0"));
        assert!(c.touches(&id, "/sections/0/register/1/span"));
        assert!(!c.touches(&id, "/sections/0/regist"));
        assert!(!c.touches(&id, "/sections/1"));
        assert!(c.touches(&id, ""));
        assert!(!c.touches(&PlanId::new(), ""));
    }

    #[test]
    fn digest_follows_the_content_not_the_revision() {
        let id = PlanId::new();
        let a = plan(&id);
        let mut b = a.clone();
        b.rev = 9;
        assert_eq!(a.digest(), b.digest());
        b.body["seed"] = json!(1);
        assert_ne!(a.digest(), b.digest());
    }
}
