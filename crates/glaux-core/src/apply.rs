//! コマンドの適用。
//!
//! [`Project::apply`] は成功時に **逆コマンド** と **変更通知** を返す。
//! 逆コマンドを Undo スタックに積むのが履歴の基本方式(Git の revert と同じ発想)。
//! 変更通知はロックフリーキュー経由でオーディオエンジンや UI に流す想定。

use crate::command::{Command, NoteChange, TrackProp};
use crate::error::{CoreError, Result};
use crate::id::{ClipId, TrackId};
use crate::model::{
    sort_notes, AutomationLane, ClipContent, ParamMap, ParamPath, ParamValue, Project, TrackKind,
};
use crate::time::{TempoMap, Tick};
use serde::{Deserialize, Serialize};

/// 適用結果。
#[derive(Clone, PartialEq, Debug)]
pub struct Applied {
    /// これを適用すると元に戻る
    pub inverse: Command,
    /// エンジン/UI への通知
    pub changes: Vec<Change>,
}

/// 「何が変わったか」の粗い通知。エンジンはこれを見て再生データを部分更新するか、
/// 構造変更なら再生データ全体を組み直す。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Change {
    TrackAdded {
        id: TrackId,
    },
    TrackRemoved {
        id: TrackId,
    },
    TrackPropChanged {
        id: TrackId,
    },
    TrackOrderChanged,
    ClipsChanged {
        track: TrackId,
    },
    NotesChanged {
        clip: ClipId,
    },
    /// 軽量更新: エンジンはグラフを組み直さずに値だけ差し替えられる
    ParamChanged {
        track: TrackId,
        path: ParamPath,
        value: Option<ParamValue>,
    },
    DeviceChanged {
        track: TrackId,
    },
    EffectsChanged {
        track: TrackId,
    },
    AutomationChanged {
        track: TrackId,
    },
    TempoChanged,
    TimeSigChanged,
    MasterChanged,
    AssetsChanged,
}

impl Project {
    /// コマンドを適用し、逆コマンドと変更通知を返す。
    /// 失敗した場合、プロジェクトは変更されない(`Batch` は途中まで適用した分を巻き戻す)。
    pub fn apply(&mut self, cmd: &Command) -> Result<Applied> {
        use Command::*;
        match cmd {
            // ---------------------------------------------------------- track
            AddTrack { track, index } => {
                if self.track_index(&track.id).is_some() {
                    return Err(CoreError::DuplicateId(track.id.to_string()));
                }
                for c in &track.clips {
                    if self.clip_location(&c.id).is_some() {
                        return Err(CoreError::DuplicateId(c.id.to_string()));
                    }
                }
                let idx = index.unwrap_or(self.tracks.len());
                check_index(idx, self.tracks.len() + 1)?;
                let mut t = track.clone();
                t.sort_clips();
                self.tracks.insert(idx, t);
                Ok(Applied {
                    inverse: RemoveTrack {
                        id: track.id.clone(),
                    },
                    changes: vec![Change::TrackAdded {
                        id: track.id.clone(),
                    }],
                })
            }
            RemoveTrack { id } => {
                let idx = self
                    .track_index(id)
                    .ok_or_else(|| CoreError::TrackNotFound(id.clone()))?;
                let track = self.tracks.remove(idx);
                Ok(Applied {
                    inverse: AddTrack {
                        track,
                        index: Some(idx),
                    },
                    changes: vec![Change::TrackRemoved { id: id.clone() }],
                })
            }
            SetTrackProp { id, prop } => {
                let t = self
                    .track_mut(id)
                    .ok_or_else(|| CoreError::TrackNotFound(id.clone()))?;
                let old = match prop {
                    TrackProp::Name(v) => {
                        TrackProp::Name(std::mem::replace(&mut t.name, v.clone()))
                    }
                    TrackProp::Color(v) => {
                        TrackProp::Color(std::mem::replace(&mut t.color, v.clone()))
                    }
                    TrackProp::Mute(v) => TrackProp::Mute(std::mem::replace(&mut t.mute, *v)),
                    TrackProp::Solo(v) => TrackProp::Solo(std::mem::replace(&mut t.solo, *v)),
                    TrackProp::VolumeDb(v) => {
                        TrackProp::VolumeDb(std::mem::replace(&mut t.volume_db, *v))
                    }
                    TrackProp::Pan(v) => {
                        if !(-1.0..=1.0).contains(v) {
                            return Err(CoreError::OutOfRange(format!("pan {v}")));
                        }
                        TrackProp::Pan(std::mem::replace(&mut t.pan, *v))
                    }
                };
                Ok(Applied {
                    inverse: SetTrackProp {
                        id: id.clone(),
                        prop: old,
                    },
                    changes: vec![Change::TrackPropChanged { id: id.clone() }],
                })
            }
            MoveTrack { id, to_index } => {
                let from = self
                    .track_index(id)
                    .ok_or_else(|| CoreError::TrackNotFound(id.clone()))?;
                check_index(*to_index, self.tracks.len())?;
                let t = self.tracks.remove(from);
                self.tracks.insert(*to_index, t);
                Ok(Applied {
                    inverse: MoveTrack {
                        id: id.clone(),
                        to_index: from,
                    },
                    changes: vec![Change::TrackOrderChanged],
                })
            }

            // ----------------------------------------------------------- clip
            AddClip { track, clip } => {
                if self.clip_location(&clip.id).is_some() {
                    return Err(CoreError::DuplicateId(clip.id.to_string()));
                }
                let t = self
                    .track_mut(track)
                    .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
                match (t.kind, &clip.content) {
                    (TrackKind::Midi, ClipContent::Midi { .. })
                    | (TrackKind::Audio, ClipContent::Audio { .. }) => {}
                    _ => return Err(CoreError::ClipKindMismatch),
                }
                let mut c = clip.clone();
                if let Some(notes) = c.notes_mut() {
                    sort_notes(notes);
                }
                t.clips.push(c);
                t.sort_clips();
                Ok(Applied {
                    inverse: RemoveClip {
                        id: clip.id.clone(),
                    },
                    changes: vec![Change::ClipsChanged {
                        track: track.clone(),
                    }],
                })
            }
            RemoveClip { id } => {
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let clip = self.tracks[ti].clips.remove(ci);
                let track = self.tracks[ti].id.clone();
                Ok(Applied {
                    inverse: AddClip {
                        track: track.clone(),
                        clip,
                    },
                    changes: vec![Change::ClipsChanged { track }],
                })
            }
            ReplaceClip { id, clip } => {
                if &clip.id != id {
                    return Err(CoreError::IdMismatch {
                        expected: id.to_string(),
                        got: clip.id.to_string(),
                    });
                }
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let mut c = clip.clone();
                if let Some(notes) = c.notes_mut() {
                    sort_notes(notes);
                }
                let old = std::mem::replace(&mut self.tracks[ti].clips[ci], c);
                self.tracks[ti].sort_clips();
                let track = self.tracks[ti].id.clone();
                Ok(Applied {
                    inverse: ReplaceClip {
                        id: id.clone(),
                        clip: old,
                    },
                    changes: vec![Change::ClipsChanged { track }],
                })
            }
            MoveClip { id, start, track } => {
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let old_track = self.tracks[ti].id.clone();
                let dest_ti = match track {
                    Some(t) => self
                        .track_index(t)
                        .ok_or_else(|| CoreError::TrackNotFound(t.clone()))?,
                    None => ti,
                };
                if self.tracks[dest_ti].kind != self.tracks[ti].kind {
                    return Err(CoreError::ClipKindMismatch);
                }
                let mut clip = self.tracks[ti].clips.remove(ci);
                let old_start = clip.start;
                clip.start = *start;
                self.tracks[dest_ti].clips.push(clip);
                self.tracks[dest_ti].sort_clips();
                let dest_track = self.tracks[dest_ti].id.clone();
                let mut changes = vec![Change::ClipsChanged {
                    track: old_track.clone(),
                }];
                if dest_ti != ti {
                    changes.push(Change::ClipsChanged { track: dest_track });
                }
                Ok(Applied {
                    inverse: MoveClip {
                        id: id.clone(),
                        start: old_start,
                        track: if dest_ti != ti { Some(old_track) } else { None },
                    },
                    changes,
                })
            }
            ResizeClip { id, length } => {
                if length.0 == 0 {
                    return Err(CoreError::OutOfRange("clip length must be > 0".into()));
                }
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let old = std::mem::replace(&mut self.tracks[ti].clips[ci].length, *length);
                let track = self.tracks[ti].id.clone();
                Ok(Applied {
                    inverse: ResizeClip {
                        id: id.clone(),
                        length: old,
                    },
                    changes: vec![Change::ClipsChanged { track }],
                })
            }
            SplitClip { id, at, new_id } => {
                if self.clip_location(new_id).is_some() {
                    return Err(CoreError::DuplicateId(new_id.to_string()));
                }
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let original = self.tracks[ti].clips[ci].clone();
                if !(*at > original.start && *at < original.end()) {
                    return Err(CoreError::InvalidSplit {
                        clip: id.clone(),
                        at: *at,
                    });
                }
                let left_len = *at - original.start;
                let right_len = original.end() - *at;

                let mut left = original.clone();
                left.length = left_len;
                let mut right = original.clone();
                right.id = new_id.clone();
                right.start = *at;
                right.length = right_len;

                match (&mut left.content, &mut right.content) {
                    (ClipContent::Midi { notes: ln, .. }, ClipContent::Midi { notes: rn, .. }) => {
                        rn.clear();
                        let mut keep = Vec::with_capacity(ln.len());
                        for n in ln.drain(..) {
                            if n.pos >= left_len {
                                let mut m = n;
                                m.pos = m.pos - left_len;
                                rn.push(m);
                            } else {
                                let mut m = n;
                                if m.end() > left_len {
                                    m.dur = left_len - m.pos;
                                }
                                keep.push(m);
                            }
                        }
                        *ln = keep;
                    }
                    (
                        ClipContent::Audio {
                            fade_out_ms: lfo, ..
                        },
                        ClipContent::Audio {
                            asset,
                            offset_samples,
                            fade_in_ms: rfi,
                            ..
                        },
                    ) => {
                        let sr = self
                            .assets
                            .get(asset)
                            .ok_or_else(|| CoreError::AssetNotFound(asset.clone()))?
                            .sample_rate;
                        let secs = self.tempo_map.tick_to_seconds(*at)
                            - self.tempo_map.tick_to_seconds(original.start);
                        *offset_samples += (secs * sr as f64).round() as u64;
                        *lfo = 0.0;
                        *rfi = 0.0;
                    }
                    _ => unreachable!("clone keeps content kind"),
                }

                self.tracks[ti].clips[ci] = left;
                self.tracks[ti].clips.push(right);
                self.tracks[ti].sort_clips();
                let track = self.tracks[ti].id.clone();
                Ok(Applied {
                    inverse: Command::batch(
                        "join",
                        vec![
                            RemoveClip { id: new_id.clone() },
                            ReplaceClip {
                                id: id.clone(),
                                clip: original,
                            },
                        ],
                    ),
                    changes: vec![Change::ClipsChanged { track }],
                })
            }

            // ----------------------------------------------------------- note
            AddNotes { clip, notes } => {
                let (ti, ci) = self
                    .clip_location(clip)
                    .ok_or_else(|| CoreError::ClipNotFound(clip.clone()))?;
                let existing = self.tracks[ti].clips[ci]
                    .notes_mut()
                    .ok_or_else(|| CoreError::NotMidiClip(clip.clone()))?;
                for n in notes {
                    if n.pitch > 127 || n.vel > 127 {
                        return Err(CoreError::OutOfRange(format!(
                            "note {}: pitch/vel must be <= 127",
                            n.id
                        )));
                    }
                    if existing.iter().any(|e| e.id == n.id) {
                        return Err(CoreError::DuplicateId(n.id.to_string()));
                    }
                }
                existing.extend(notes.iter().cloned());
                sort_notes(existing);
                Ok(Applied {
                    inverse: RemoveNotes {
                        clip: clip.clone(),
                        ids: notes.iter().map(|n| n.id.clone()).collect(),
                    },
                    changes: vec![Change::NotesChanged { clip: clip.clone() }],
                })
            }
            RemoveNotes { clip, ids } => {
                let (ti, ci) = self
                    .clip_location(clip)
                    .ok_or_else(|| CoreError::ClipNotFound(clip.clone()))?;
                let existing = self.tracks[ti].clips[ci]
                    .notes_mut()
                    .ok_or_else(|| CoreError::NotMidiClip(clip.clone()))?;
                for id in ids {
                    if !existing.iter().any(|e| &e.id == id) {
                        return Err(CoreError::NoteNotFound {
                            clip: clip.clone(),
                            note: id.clone(),
                        });
                    }
                }
                let removed: Vec<_> = existing
                    .iter()
                    .filter(|n| ids.contains(&n.id))
                    .cloned()
                    .collect();
                existing.retain(|n| !ids.contains(&n.id));
                Ok(Applied {
                    inverse: AddNotes {
                        clip: clip.clone(),
                        notes: removed,
                    },
                    changes: vec![Change::NotesChanged { clip: clip.clone() }],
                })
            }
            UpdateNotes { clip, changes } => {
                let (ti, ci) = self
                    .clip_location(clip)
                    .ok_or_else(|| CoreError::ClipNotFound(clip.clone()))?;
                let existing = self.tracks[ti].clips[ci]
                    .notes_mut()
                    .ok_or_else(|| CoreError::NotMidiClip(clip.clone()))?;
                // 事前検証(途中失敗で半端に変わらないように)
                for ch in changes {
                    if !existing.iter().any(|n| n.id == ch.id) {
                        return Err(CoreError::NoteNotFound {
                            clip: clip.clone(),
                            note: ch.id.clone(),
                        });
                    }
                    if ch.pitch.is_some_and(|p| p > 127) || ch.vel.is_some_and(|v| v > 127) {
                        return Err(CoreError::OutOfRange(format!(
                            "note {}: pitch/vel must be <= 127",
                            ch.id
                        )));
                    }
                }
                let mut inverse_changes = Vec::with_capacity(changes.len());
                for ch in changes {
                    let n = existing
                        .iter_mut()
                        .find(|n| n.id == ch.id)
                        .expect("validated");
                    let mut inv = NoteChange::new(ch.id.clone());
                    if let Some(v) = ch.pos {
                        inv.pos = Some(std::mem::replace(&mut n.pos, v));
                    }
                    if let Some(v) = ch.dur {
                        inv.dur = Some(std::mem::replace(&mut n.dur, v));
                    }
                    if let Some(v) = ch.pitch {
                        inv.pitch = Some(std::mem::replace(&mut n.pitch, v));
                    }
                    if let Some(v) = ch.vel {
                        inv.vel = Some(std::mem::replace(&mut n.vel, v));
                    }
                    if let Some(v) = ch.articulation {
                        inv.articulation = Some(std::mem::replace(&mut n.articulation, v));
                    }
                    inverse_changes.push(inv);
                }
                inverse_changes.reverse();
                sort_notes(existing);
                Ok(Applied {
                    inverse: UpdateNotes {
                        clip: clip.clone(),
                        changes: inverse_changes,
                    },
                    changes: vec![Change::NotesChanged { clip: clip.clone() }],
                })
            }

            // -------------------------------------------------------- params
            SetParam { track, path, value } => {
                let old = self.set_param(track, path, Some(value))?;
                Ok(Applied {
                    inverse: match old {
                        Some(v) => SetParam {
                            track: track.clone(),
                            path: path.clone(),
                            value: v,
                        },
                        None => UnsetParam {
                            track: track.clone(),
                            path: path.clone(),
                        },
                    },
                    changes: vec![Change::ParamChanged {
                        track: track.clone(),
                        path: path.clone(),
                        value: Some(value.clone()),
                    }],
                })
            }
            UnsetParam { track, path } => {
                let old = self.set_param(track, path, None)?;
                let v = old.ok_or_else(|| CoreError::ParamNotSet(path.clone()))?;
                Ok(Applied {
                    inverse: SetParam {
                        track: track.clone(),
                        path: path.clone(),
                        value: v,
                    },
                    changes: vec![Change::ParamChanged {
                        track: track.clone(),
                        path: path.clone(),
                        value: None,
                    }],
                })
            }
            SetDevice { track, device } => {
                let t = self
                    .track_mut(track)
                    .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
                let old = std::mem::replace(&mut t.device, device.clone());
                Ok(Applied {
                    inverse: SetDevice {
                        track: track.clone(),
                        device: old,
                    },
                    changes: vec![Change::DeviceChanged {
                        track: track.clone(),
                    }],
                })
            }
            AddEffect {
                track,
                effect,
                index,
            } => {
                if self.effect_location(&effect.id).is_some() {
                    return Err(CoreError::DuplicateId(effect.id.to_string()));
                }
                let t = self
                    .track_mut(track)
                    .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
                let idx = index.unwrap_or(t.effects.len());
                check_index(idx, t.effects.len() + 1)?;
                t.effects.insert(idx, effect.clone());
                Ok(Applied {
                    inverse: RemoveEffect {
                        id: effect.id.clone(),
                    },
                    changes: vec![Change::EffectsChanged {
                        track: track.clone(),
                    }],
                })
            }
            RemoveEffect { id } => {
                let (ti, ei) = self
                    .effect_location(id)
                    .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                let effect = self.tracks[ti].effects.remove(ei);
                let track = self.tracks[ti].id.clone();
                Ok(Applied {
                    inverse: AddEffect {
                        track: track.clone(),
                        effect,
                        index: Some(ei),
                    },
                    changes: vec![Change::EffectsChanged { track }],
                })
            }
            SetEffectBypass { id, bypass } => {
                let (ti, ei) = self
                    .effect_location(id)
                    .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                let old = std::mem::replace(&mut self.tracks[ti].effects[ei].bypass, *bypass);
                let track = self.tracks[ti].id.clone();
                Ok(Applied {
                    inverse: SetEffectBypass {
                        id: id.clone(),
                        bypass: old,
                    },
                    changes: vec![Change::EffectsChanged { track }],
                })
            }

            // ---------------------------------------------------- automation
            SetAutomationPoints {
                track,
                target,
                points,
            } => {
                let t = self
                    .track_mut(track)
                    .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
                let mut pts = points.clone();
                pts.sort_by_key(|p| p.tick);
                let pos = t.automation.iter().position(|l| &l.target == target);
                let old_points = match (pos, pts.is_empty()) {
                    (Some(i), true) => t.automation.remove(i).points,
                    (Some(i), false) => std::mem::replace(&mut t.automation[i].points, pts),
                    (None, true) => vec![],
                    (None, false) => {
                        t.automation.push(AutomationLane {
                            target: target.clone(),
                            points: pts,
                        });
                        vec![]
                    }
                };
                Ok(Applied {
                    inverse: SetAutomationPoints {
                        track: track.clone(),
                        target: target.clone(),
                        points: old_points,
                    },
                    changes: vec![Change::AutomationChanged {
                        track: track.clone(),
                    }],
                })
            }

            // -------------------------------------------------------- global
            SetTempo { events } => {
                let new_map = TempoMap::new(events.clone())?;
                let old = std::mem::replace(&mut self.tempo_map, new_map);
                Ok(Applied {
                    inverse: SetTempo { events: old.into() },
                    changes: vec![Change::TempoChanged],
                })
            }
            SetTimeSig { events } => {
                let mut ev = events.clone();
                ev.sort_by_key(|e| e.tick);
                if ev.first().map_or(true, |e| e.tick != Tick::ZERO) {
                    return Err(CoreError::OutOfRange(
                        "time signature map must start at tick 0".into(),
                    ));
                }
                for e in &ev {
                    if e.num == 0 || e.den == 0 || !e.den.is_power_of_two() {
                        return Err(crate::time::TimeError::InvalidTimeSig(e.den).into());
                    }
                }
                let old = std::mem::replace(&mut self.time_sig_map, ev);
                Ok(Applied {
                    inverse: SetTimeSig { events: old },
                    changes: vec![Change::TimeSigChanged],
                })
            }
            SetMasterVolume { volume_db } => {
                let old = std::mem::replace(&mut self.master.volume_db, *volume_db);
                Ok(Applied {
                    inverse: SetMasterVolume { volume_db: old },
                    changes: vec![Change::MasterChanged],
                })
            }
            AddAsset { id, asset } => {
                if self.assets.contains_key(id) {
                    return Err(CoreError::DuplicateId(id.to_string()));
                }
                self.assets.insert(id.clone(), asset.clone());
                Ok(Applied {
                    inverse: RemoveAsset { id: id.clone() },
                    changes: vec![Change::AssetsChanged],
                })
            }
            RemoveAsset { id } => {
                let asset = self
                    .assets
                    .remove(id)
                    .ok_or_else(|| CoreError::AssetNotFound(id.clone()))?;
                Ok(Applied {
                    inverse: AddAsset {
                        id: id.clone(),
                        asset,
                    },
                    changes: vec![Change::AssetsChanged],
                })
            }

            // --------------------------------------------------------- batch
            Batch { commands, label } => {
                let mut inverses = Vec::with_capacity(commands.len());
                let mut changes = Vec::new();
                for (i, c) in commands.iter().enumerate() {
                    match self.apply(c) {
                        Ok(a) => {
                            inverses.push(a.inverse);
                            changes.extend(a.changes);
                        }
                        Err(e) => {
                            // 巻き戻し。逆コマンドの適用は失敗しない前提。
                            for inv in inverses.iter().rev() {
                                self.apply(inv)
                                    .expect("rollback of a just-applied command must succeed");
                            }
                            return Err(CoreError::Batch {
                                index: i,
                                source: Box::new(e),
                            });
                        }
                    }
                }
                inverses.reverse();
                Ok(Applied {
                    inverse: Command::batch(format!("undo {label}"), inverses),
                    changes,
                })
            }
        }
    }

    /// パラメータを設定(`None` で削除)し、旧値を返す。
    fn set_param(
        &mut self,
        track: &TrackId,
        path: &ParamPath,
        value: Option<&ParamValue>,
    ) -> Result<Option<ParamValue>> {
        let t = self
            .track_mut(track)
            .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
        match path {
            ParamPath::Track { name } => {
                let v = value.ok_or_else(|| {
                    CoreError::OutOfRange(format!("track/{name} cannot be unset"))
                })?;
                let f = v
                    .as_f64()
                    .ok_or_else(|| CoreError::OutOfRange(format!("track/{name} must be numeric")))?
                    as f32;
                match name.as_str() {
                    "volume_db" => Ok(Some(ParamValue::Float(
                        std::mem::replace(&mut t.volume_db, f) as f64,
                    ))),
                    "pan" => {
                        if !(-1.0..=1.0).contains(&f) {
                            return Err(CoreError::OutOfRange(format!("pan {f}")));
                        }
                        Ok(Some(ParamValue::Float(
                            std::mem::replace(&mut t.pan, f) as f64
                        )))
                    }
                    _ => Err(CoreError::UnknownParam(path.clone())),
                }
            }
            ParamPath::Device { name } => {
                let dev = t
                    .device
                    .as_mut()
                    .ok_or_else(|| CoreError::NoDevice(track.clone()))?;
                Ok(set_in_map(&mut dev.params, name, value))
            }
            ParamPath::Effect { id, name } => {
                let ei = t
                    .effect_index(id)
                    .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                Ok(set_in_map(&mut t.effects[ei].params, name, value))
            }
        }
    }
}

fn set_in_map(map: &mut ParamMap, name: &str, value: Option<&ParamValue>) -> Option<ParamValue> {
    match value {
        Some(v) => map.insert(name.to_owned(), v.clone()),
        None => map.remove(name),
    }
}

fn check_index(index: usize, len: usize) -> Result<()> {
    if index < len {
        Ok(())
    } else {
        Err(CoreError::IndexOutOfRange { index, len })
    }
}
