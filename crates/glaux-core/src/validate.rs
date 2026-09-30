//! プロジェクト全体の整合性チェック。読み込み直後や保存前に呼ぶ。
//! コマンド適用時の検証は `apply` 側で行うので、ここは「外から来たファイル」向け。

use crate::model::{ClipContent, Project, TrackKind, FORMAT_NAME};
use crate::time::{Tick, PPQ};
use std::collections::HashSet;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
}

impl Issue {
    fn error(msg: impl Into<String>) -> Self {
        Issue {
            severity: Severity::Error,
            message: msg.into(),
        }
    }
    fn warn(msg: impl Into<String>) -> Self {
        Issue {
            severity: Severity::Warning,
            message: msg.into(),
        }
    }
}

impl Project {
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();

        if self.format != FORMAT_NAME {
            issues.push(Issue::error(format!("unknown format `{}`", self.format)));
        }
        if self.version > crate::model::FORMAT_VERSION {
            issues.push(Issue::warn(format!(
                "format version {} is newer than this Glaux ({}); unknown fields may be lost on save",
                self.version,
                crate::model::FORMAT_VERSION
            )));
        }
        if self.ppq != PPQ {
            issues.push(Issue::error(format!(
                "unsupported ppq {} (expected {PPQ})",
                self.ppq
            )));
        }
        if self
            .time_sig_map
            .first()
            .map_or(true, |e| e.tick != Tick::ZERO)
        {
            issues.push(Issue::error("time_sig_map must start at tick 0"));
        }
        for e in &self.time_sig_map {
            if let Some(g) = &e.grouping {
                if let Err(msg) = crate::meter::check_grouping(e.num, g) {
                    issues.push(Issue::error(format!("time_sig_map @{}: {msg}", e.tick.0)));
                }
            }
        }

        let mut track_ids = HashSet::new();
        let mut clip_ids = HashSet::new();
        let mut fx_ids = HashSet::new();

        for lane in &self.master.automation {
            if !lane.points.windows(2).all(|w| w[0].tick <= w[1].tick) {
                issues.push(Issue::error(format!(
                    "master: automation {} not sorted",
                    lane.target
                )));
            }
        }
        for e in &self.master.effects {
            if !fx_ids.insert(&e.id) {
                issues.push(Issue::error(format!("duplicate effect id {}", e.id)));
            }
        }
        // つながりの表の誤りは警告(再生側は直列とみなして鳴らす)
        if let Some(l) = &self.master.fx_links {
            if let Err(e) = crate::model::routing::validate_links(&self.master.effects, l) {
                issues.push(Issue::warn(format!("master: fx_links: {e}")));
            }
        }
        for t in &self.tracks {
            if !track_ids.insert(&t.id) {
                issues.push(Issue::error(format!("duplicate track id {}", t.id)));
            }
            if !(-1.0..=1.0).contains(&t.pan) {
                issues.push(Issue::error(format!(
                    "track {}: pan {} out of range",
                    t.id, t.pan
                )));
            }
            for e in &t.effects {
                if !fx_ids.insert(&e.id) {
                    issues.push(Issue::error(format!("duplicate effect id {}", e.id)));
                }
            }
            if let Some(l) = &t.fx_links {
                if let Err(e) = crate::model::routing::validate_links(&t.effects, l) {
                    issues.push(Issue::warn(format!("track {}: fx_links: {e}", t.id)));
                }
            }
            for lane in &t.automation {
                if !lane.points.windows(2).all(|w| w[0].tick <= w[1].tick) {
                    issues.push(Issue::error(format!(
                        "track {}: automation {} not sorted",
                        t.id, lane.target
                    )));
                }
            }
            for c in &t.clips {
                if !clip_ids.insert(&c.id) {
                    issues.push(Issue::error(format!("duplicate clip id {}", c.id)));
                }
                if c.length.0 == 0 {
                    issues.push(Issue::error(format!("clip {}: length is 0", c.id)));
                }
                match (&t.kind, &c.content) {
                    (TrackKind::Midi, ClipContent::Midi { notes, .. }) => {
                        let mut note_ids = HashSet::new();
                        for n in notes {
                            if !note_ids.insert(&n.id) {
                                issues.push(Issue::error(format!(
                                    "clip {}: duplicate note id {}",
                                    c.id, n.id
                                )));
                            }
                            if n.pitch > 127 || n.vel > 127 {
                                issues.push(Issue::error(format!(
                                    "note {}: pitch/vel out of range",
                                    n.id
                                )));
                            }
                            if n.pos >= c.length {
                                issues.push(Issue::warn(format!(
                                    "note {} starts beyond clip {} end",
                                    n.id, c.id
                                )));
                            }
                        }
                    }
                    (TrackKind::Audio, ClipContent::Audio { asset, .. }) => {
                        if !self.assets.contains_key(asset) {
                            issues.push(Issue::error(format!(
                                "clip {}: missing asset {}",
                                c.id, asset
                            )));
                        }
                    }
                    _ => issues.push(Issue::error(format!(
                        "clip {} kind does not match track {}",
                        c.id, t.id
                    ))),
                }
            }
        }

        for (id, a) in &self.assets {
            if a.sample_rate == 0 || a.channels == 0 {
                issues.push(Issue::error(format!(
                    "asset {id}: invalid sample_rate/channels"
                )));
            }
        }

        issues
    }

    /// 検査の結果を、同じ種類ごとに 1 行にまとめる(ログ向け)。ノートの ID だけが違う指摘は 1 行にし、
    /// 該当するノートの ID を並べる。行の順は最初に出た順
    pub fn summarize_issues(issues: &[Issue]) -> Vec<String> {
        let mut groups: Vec<(String, &Severity, Vec<String>)> = Vec::new();
        for i in issues {
            let mut ids = Vec::new();
            let template: Vec<String> = i
                .message
                .split(' ')
                .map(|w| {
                    let core = w.trim_end_matches([':', ',', ')']);
                    if core.starts_with(crate::id::NoteId::PREFIX)
                        && core.as_bytes().get(crate::id::NoteId::PREFIX.len()) == Some(&b'_')
                    {
                        ids.push(core.to_owned());
                        w.replacen(core, "<note>", 1)
                    } else {
                        w.to_owned()
                    }
                })
                .collect();
            let template = template.join(" ");
            match groups
                .iter_mut()
                .find(|g| g.0 == template && g.1 == &i.severity)
            {
                Some(g) => g.2.extend(ids),
                None => groups.push((template, &i.severity, ids)),
            }
        }
        groups
            .into_iter()
            .map(|(template, sev, ids)| {
                let kind = match sev {
                    Severity::Error => "エラー",
                    Severity::Warning => "警告",
                };
                if ids.len() <= 1 {
                    // まとめるものが無ければ元の文のまま
                    let msg = match ids.first() {
                        Some(id) => template.replacen("<note>", id, 1),
                        None => template,
                    };
                    format!("{kind}: {msg}")
                } else {
                    format!(
                        "{kind} {} 件: {template}(該当: {})",
                        ids.len(),
                        ids.join(", ")
                    )
                }
            })
            .collect()
    }

    pub fn is_valid(&self) -> bool {
        self.validate()
            .iter()
            .all(|i| i.severity != Severity::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_of_the_same_kind_become_one_line_with_the_places() {
        let issues = vec![
            Issue::warn("note nt_aaaaaa starts beyond clip clp_prc001 end"),
            Issue::error("clip clp_x: length is 0"),
            Issue::warn("note nt_bbbbbb starts beyond clip clp_prc001 end"),
            Issue::warn("note nt_cccccc starts beyond clip clp_other end"),
            Issue::warn("note nt_dddddd starts beyond clip clp_prc001 end"),
        ];
        let lines = Project::summarize_issues(&issues);
        assert_eq!(
            lines,
            vec![
                "警告 3 件: note <note> starts beyond clip clp_prc001 end(該当: nt_aaaaaa, nt_bbbbbb, nt_dddddd)"
                    .to_owned(),
                "エラー: clip clp_x: length is 0".to_owned(),
                "警告: note nt_cccccc starts beyond clip clp_other end".to_owned(),
            ]
        );
    }
}
