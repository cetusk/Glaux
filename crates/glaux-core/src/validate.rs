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

        let mut track_ids = HashSet::new();
        let mut clip_ids = HashSet::new();
        let mut fx_ids = HashSet::new();

        for e in &self.master.effects {
            if !fx_ids.insert(&e.id) {
                issues.push(Issue::error(format!("duplicate effect id {}", e.id)));
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

    pub fn is_valid(&self) -> bool {
        self.validate()
            .iter()
            .all(|i| i.severity != Severity::Error)
    }
}
