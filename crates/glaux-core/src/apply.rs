//! コマンドの適用。
//!
//! [`Project::apply`] は成功時に **逆コマンド** と **変更通知** を返す。
//! 逆コマンドを Undo スタックに積むのが履歴の基本方式(Git の revert と同じ発想)。
//! 変更通知はロックフリーキュー経由でオーディオエンジンや UI に流す想定。

use crate::command::{Command, EffectProp, NoteChange, TrackProp};
use crate::error::{CoreError, Result};
use crate::id::{ClipId, NoteId, TrackId};
use crate::model::routing::{insert_before_output, unlink_bridging, validate_links};
use crate::model::{
    sort_notes, AutomationLane, Clip, ClipContent, Note, ParamMap, ParamPath, ParamValue,
    PitchPoint, Project, Stretch, TrackKind,
};
use crate::time::{TempoMap, Tick, MAX_TICK};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
    MetaChanged,
    SectionsChanged,
}

impl Project {
    /// コマンドを適用し、逆コマンドと変更通知を返す。
    /// 失敗した場合、プロジェクトは変更されない(`Batch` は途中まで適用した分を巻き戻す)。
    pub fn apply(&mut self, cmd: &Command) -> Result<Applied> {
        check_ticks(cmd)?;
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
            SetClipLoop { id, loop_len } => {
                if loop_len.is_some_and(|l| l.0 == 0) {
                    return Err(CoreError::OutOfRange("loop_len must be > 0".into()));
                }
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let track = self.tracks[ti].id.clone();
                let ClipContent::Midi {
                    looped,
                    loop_len: cur,
                    ..
                } = &mut self.tracks[ti].clips[ci].content
                else {
                    return Err(CoreError::NotMidiClip(id.clone()));
                };
                let old = if *looped { *cur } else { None };
                *looped = loop_len.is_some();
                *cur = *loop_len;
                Ok(Applied {
                    inverse: SetClipLoop {
                        id: id.clone(),
                        loop_len: old,
                    },
                    changes: vec![Change::ClipsChanged { track }],
                })
            }
            SetClipStretch { id, stretch } => {
                if let Stretch::Follow { original_bpm } = stretch {
                    if !Stretch::BPM_RANGE.contains(original_bpm) {
                        return Err(CoreError::OutOfRange(format!(
                            "original_bpm must be within 20..=400: {original_bpm}"
                        )));
                    }
                }
                let (ti, ci) = self
                    .clip_location(id)
                    .ok_or_else(|| CoreError::ClipNotFound(id.clone()))?;
                let track = self.tracks[ti].id.clone();
                let ClipContent::Audio { stretch: cur, .. } =
                    &mut self.tracks[ti].clips[ci].content
                else {
                    return Err(CoreError::NotAudioClip(id.clone()));
                };
                let old = std::mem::replace(cur, stretch.clone());
                Ok(Applied {
                    inverse: SetClipStretch {
                        id: id.clone(),
                        stretch: old,
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
                            stretch,
                            ..
                        },
                    ) => {
                        let sr = self
                            .assets
                            .get(asset)
                            .ok_or_else(|| CoreError::AssetNotFound(asset.clone()))?
                            .sample_rate;
                        // テンポ追従中は素材の秒数が tick に比例する
                        let secs = stretch
                            .follow_seconds(left_len.0 as f64)
                            .unwrap_or_else(|| {
                                self.tempo_map.tick_to_seconds(*at)
                                    - self.tempo_map.tick_to_seconds(original.start)
                            });
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
                // ID の重複はハッシュで調べる(ノート数の 2 乗にしない。1 回の追加の中の重複も弾く)
                let mut ids: HashSet<&NoteId> = existing.iter().map(|e| &e.id).collect();
                for n in notes {
                    if n.pitch > 127 || n.vel > 127 {
                        return Err(CoreError::OutOfRange(format!(
                            "note {}: pitch/vel must be <= 127",
                            n.id
                        )));
                    }
                    check_note_extras(&n.id, &n.pitch_curve, n.glide_ms)?;
                    if !ids.insert(&n.id) {
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
                let present: HashSet<&NoteId> = existing.iter().map(|e| &e.id).collect();
                for id in ids {
                    if !present.contains(id) {
                        return Err(CoreError::NoteNotFound {
                            clip: clip.clone(),
                            note: id.clone(),
                        });
                    }
                }
                let targets: HashSet<&NoteId> = ids.iter().collect();
                let removed: Vec<_> = existing
                    .iter()
                    .filter(|n| targets.contains(&n.id))
                    .cloned()
                    .collect();
                existing.retain(|n| !targets.contains(&n.id));
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
                // ID → 位置(並べ替えは最後なので、更新の間は位置が変わらない)
                let index: HashMap<NoteId, usize> = existing
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (n.id.clone(), i))
                    .collect();
                // 事前検証(途中失敗で半端に変わらないように)
                for ch in changes {
                    if !index.contains_key(&ch.id) {
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
                    check_note_extras(
                        &ch.id,
                        ch.pitch_curve.as_deref().unwrap_or(&[]),
                        ch.glide_ms.filter(|g| *g > 0.0),
                    )?;
                }
                let mut inverse_changes = Vec::with_capacity(changes.len());
                for ch in changes {
                    let n = &mut existing[index[&ch.id]];
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
                    if let Some(v) = &ch.pitch_curve {
                        inv.pitch_curve = Some(std::mem::replace(&mut n.pitch_curve, v.clone()));
                    }
                    if let Some(v) = ch.glide_ms {
                        let new = (v > 0.0).then_some(v);
                        inv.glide_ms = Some(std::mem::replace(&mut n.glide_ms, new).unwrap_or(0.0));
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
                if self.effect_location(&effect.id).is_some()
                    || self.master_effect_index(&effect.id).is_some()
                {
                    return Err(CoreError::DuplicateId(effect.id.to_string()));
                }
                let t = self
                    .track_mut(track)
                    .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
                let idx = index.unwrap_or(t.effects.len());
                check_index(idx, t.effects.len() + 1)?;
                t.effects.insert(idx, effect.clone());
                // つながりの表があるトラックでは、出口の直前に入れて鳴らす(parked なら、つながずに置く)
                let old_links = t.fx_links.clone();
                if let Some(l) = &old_links {
                    if !effect.ui.parked {
                        t.fx_links = Some(insert_before_output(l, &effect.id));
                    }
                }
                Ok(Applied {
                    inverse: undo_add_effect(&effect.id, Some(track), old_links),
                    changes: vec![Change::EffectsChanged {
                        track: track.clone(),
                    }],
                })
            }
            RemoveEffect { id } => {
                // つながりの表があれば、線を外して前後をつなぎ直す(戻すときは表ごと戻す)
                if let Some(ei) = self.master_effect_index(id) {
                    let effect = self.master.effects.remove(ei);
                    let old_links = self.master.fx_links.clone();
                    if let Some(l) = &old_links {
                        self.master.fx_links = Some(unlink_bridging(l, id));
                    }
                    let add = AddMasterEffect {
                        effect,
                        index: Some(ei),
                    };
                    return Ok(Applied {
                        inverse: with_links(add, None, old_links),
                        changes: vec![Change::MasterChanged],
                    });
                }
                let (ti, ei) = self
                    .effect_location(id)
                    .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                let t = &mut self.tracks[ti];
                let effect = t.effects.remove(ei);
                let old_links = t.fx_links.clone();
                if let Some(l) = &old_links {
                    t.fx_links = Some(unlink_bridging(l, id));
                }
                let track = t.id.clone();
                let add = AddEffect {
                    track: track.clone(),
                    effect,
                    index: Some(ei),
                };
                Ok(Applied {
                    inverse: with_links(add, Some(&track), old_links),
                    changes: vec![Change::EffectsChanged { track }],
                })
            }
            SetFxLinks { track, links } => {
                let (effects, slot, change) = match track {
                    Some(tid) => {
                        let t = self
                            .track_mut(tid)
                            .ok_or_else(|| CoreError::TrackNotFound(tid.clone()))?;
                        (
                            &t.effects,
                            &mut t.fx_links,
                            Change::EffectsChanged { track: tid.clone() },
                        )
                    }
                    None => (
                        &self.master.effects,
                        &mut self.master.fx_links,
                        Change::MasterChanged,
                    ),
                };
                if let Some(l) = links {
                    validate_links(effects, l).map_err(CoreError::InvalidLinks)?;
                }
                let old = std::mem::replace(slot, links.clone());
                Ok(Applied {
                    inverse: SetFxLinks {
                        track: track.clone(),
                        links: old,
                    },
                    changes: vec![change],
                })
            }
            MoveEffect { id, to_index } => {
                if let Some(from) = self.master_effect_index(id) {
                    check_index(*to_index, self.master.effects.len())?;
                    let e = self.master.effects.remove(from);
                    self.master.effects.insert(*to_index, e);
                    return Ok(Applied {
                        inverse: MoveEffect {
                            id: id.clone(),
                            to_index: from,
                        },
                        changes: vec![Change::MasterChanged],
                    });
                }
                let (ti, from) = self
                    .effect_location(id)
                    .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                let effects = &mut self.tracks[ti].effects;
                check_index(*to_index, effects.len())?;
                let e = effects.remove(from);
                effects.insert(*to_index, e);
                Ok(Applied {
                    inverse: MoveEffect {
                        id: id.clone(),
                        to_index: from,
                    },
                    changes: vec![Change::EffectsChanged {
                        track: self.tracks[ti].id.clone(),
                    }],
                })
            }
            SetEffectProp { id, prop } => {
                let (effect, change, has_links) = if let Some(ei) = self.master_effect_index(id) {
                    let has = self.master.fx_links.is_some();
                    (&mut self.master.effects[ei], Change::MasterChanged, has)
                } else {
                    let (ti, ei) = self
                        .effect_location(id)
                        .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                    let t = &mut self.tracks[ti];
                    let track = t.id.clone();
                    let has = t.fx_links.is_some();
                    (&mut t.effects[ei], Change::EffectsChanged { track }, has)
                };
                if matches!(prop, EffectProp::Parked(_)) && has_links {
                    return Err(CoreError::InvalidLinks(
                        "つながりの表(fx_links)があるので parked は使えません。set_fx_links で線を外してください"
                            .to_owned(),
                    ));
                }
                let ui = &mut effect.ui;
                let old = match prop {
                    EffectProp::Label(v) => {
                        EffectProp::Label(std::mem::replace(&mut ui.label, v.clone()))
                    }
                    EffectProp::Parked(v) => {
                        EffectProp::Parked(std::mem::replace(&mut ui.parked, *v))
                    }
                    EffectProp::Note(v) => {
                        EffectProp::Note(std::mem::replace(&mut ui.note, v.clone()))
                    }
                    EffectProp::Pos(v) => {
                        if v.is_some_and(|p| !p[0].is_finite() || !p[1].is_finite()) {
                            return Err(CoreError::OutOfRange(format!("pos {v:?}")));
                        }
                        EffectProp::Pos(std::mem::replace(&mut ui.pos, *v))
                    }
                };
                Ok(Applied {
                    inverse: SetEffectProp {
                        id: id.clone(),
                        prop: old,
                    },
                    changes: vec![change],
                })
            }
            SetEffectBypass { id, bypass } => {
                if let Some(ei) = self.master_effect_index(id) {
                    let old = std::mem::replace(&mut self.master.effects[ei].bypass, *bypass);
                    return Ok(Applied {
                        inverse: SetEffectBypass {
                            id: id.clone(),
                            bypass: old,
                        },
                        changes: vec![Change::MasterChanged],
                    });
                }
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

            SetSend {
                track,
                target,
                level_db,
                pre_fader,
            } => {
                let target_kind = self
                    .track(target)
                    .ok_or_else(|| CoreError::TrackNotFound(target.clone()))?
                    .kind;
                let t = self
                    .track_mut(track)
                    .ok_or_else(|| CoreError::TrackNotFound(track.clone()))?;
                if let Some(db) = level_db {
                    if t.kind == TrackKind::Bus || target_kind != TrackKind::Bus || track == target
                    {
                        return Err(CoreError::OutOfRange(
                            "センドはバス以外のトラックからバスへだけ送れます".into(),
                        ));
                    }
                    if !(-60.0..=12.0).contains(db) {
                        return Err(CoreError::OutOfRange(format!(
                            "level_db must be within -60..=12: {db}"
                        )));
                    }
                }
                let old = t
                    .sends
                    .iter()
                    .position(|s| &s.target == target)
                    .map(|i| t.sends.remove(i));
                if let Some(db) = level_db {
                    let at = t
                        .sends
                        .partition_point(|s| s.target.as_str() < target.as_str());
                    t.sends.insert(
                        at,
                        crate::model::Send {
                            target: target.clone(),
                            level_db: *db,
                            pre_fader: *pre_fader,
                        },
                    );
                }
                Ok(Applied {
                    inverse: SetSend {
                        track: track.clone(),
                        target: target.clone(),
                        level_db: old.as_ref().map(|s| s.level_db),
                        pre_fader: old.is_some_and(|s| s.pre_fader),
                    },
                    changes: vec![Change::TrackPropChanged { id: track.clone() }],
                })
            }

            SetEffectState { id, state } => {
                let (effect, change) = if let Some(ei) = self.master_effect_index(id) {
                    (&mut self.master.effects[ei], Change::MasterChanged)
                } else {
                    let (ti, ei) = self
                        .effect_location(id)
                        .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
                    let track = self.tracks[ti].id.clone();
                    (
                        &mut self.tracks[ti].effects[ei],
                        Change::EffectsChanged { track },
                    )
                };
                let crate::PluginSource::Clap { state: slot, .. } = &mut effect.source else {
                    return Err(CoreError::OutOfRange(format!(
                        "{id} は CLAP プラグインのエフェクトではありません"
                    )));
                };
                let old = std::mem::replace(slot, state.clone());
                Ok(Applied {
                    inverse: SetEffectState {
                        id: id.clone(),
                        state: old,
                    },
                    changes: vec![change],
                })
            }

            AddMasterEffect { effect, index } => {
                if self.effect_location(&effect.id).is_some()
                    || self.master_effect_index(&effect.id).is_some()
                {
                    return Err(CoreError::DuplicateId(effect.id.to_string()));
                }
                let len = self.master.effects.len();
                let idx = index.unwrap_or(len);
                check_index(idx, len + 1)?;
                self.master.effects.insert(idx, effect.clone());
                let old_links = self.master.fx_links.clone();
                if let Some(l) = &old_links {
                    if !effect.ui.parked {
                        self.master.fx_links = Some(insert_before_output(l, &effect.id));
                    }
                }
                Ok(Applied {
                    inverse: undo_add_effect(&effect.id, None, old_links),
                    changes: vec![Change::MasterChanged],
                })
            }
            SetMasterParam { path, value } => {
                let old = self.set_master_param(path, Some(value))?;
                Ok(Applied {
                    inverse: match old {
                        Some(v) => SetMasterParam {
                            path: path.clone(),
                            value: v,
                        },
                        None => UnsetMasterParam { path: path.clone() },
                    },
                    changes: vec![Change::MasterChanged],
                })
            }
            UnsetMasterParam { path } => {
                let old = self.set_master_param(path, None)?;
                let v = old.ok_or_else(|| CoreError::ParamNotSet(path.clone()))?;
                Ok(Applied {
                    inverse: SetMasterParam {
                        path: path.clone(),
                        value: v,
                    },
                    changes: vec![Change::MasterChanged],
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
                        insert_lane(&mut t.automation, target, pts);
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

            SetMasterAutomationPoints { target, points } => {
                match target {
                    ParamPath::Track { name } if name == "volume_db" => {}
                    ParamPath::Effect { id, .. } if self.master_effect_index(id).is_some() => {}
                    ParamPath::Effect { id, .. } => {
                        return Err(CoreError::EffectNotFound(id.clone()));
                    }
                    _ => {
                        return Err(CoreError::OutOfRange(format!(
                            "master automation target must be track/volume_db or fx/<master fx>/<param>: {target}"
                        )));
                    }
                }
                let lanes = &mut self.master.automation;
                let mut pts = points.clone();
                pts.sort_by_key(|p| p.tick);
                let pos = lanes.iter().position(|l| &l.target == target);
                let old_points = match (pos, pts.is_empty()) {
                    (Some(i), true) => lanes.remove(i).points,
                    (Some(i), false) => std::mem::replace(&mut lanes[i].points, pts),
                    (None, true) => vec![],
                    (None, false) => {
                        insert_lane(lanes, target, pts);
                        vec![]
                    }
                };
                Ok(Applied {
                    inverse: SetMasterAutomationPoints {
                        target: target.clone(),
                        points: old_points,
                    },
                    changes: vec![Change::MasterChanged],
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
            SetTitle { title } => {
                let old = std::mem::replace(&mut self.meta.title, title.clone());
                Ok(Applied {
                    inverse: SetTitle { title: old },
                    changes: vec![Change::MetaChanged],
                })
            }
            SetSections { sections } => {
                let mut new = sections.clone();
                new.sort_by_key(|m| m.tick);
                let old = std::mem::replace(&mut self.sections, new);
                Ok(Applied {
                    inverse: SetSections { sections: old },
                    changes: vec![Change::SectionsChanged],
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
            ParamPath::Track { name } if name == "glide_ms" || name == "legato_ms" => {
                let (field, range) = if name == "glide_ms" {
                    (&mut t.glide_ms, crate::model::GLIDE_MS_RANGE)
                } else {
                    (&mut t.legato_ms, crate::model::LEGATO_MS_RANGE)
                };
                let new = match value {
                    None => None,
                    Some(v) => {
                        let f = v.as_f64().ok_or_else(|| {
                            CoreError::OutOfRange(format!("track/{name} must be numeric"))
                        })? as f32;
                        if !range.contains(&f) {
                            return Err(CoreError::OutOfRange(format!(
                                "track/{name} {f}({}〜{})",
                                range.start(),
                                range.end()
                            )));
                        }
                        Some(f)
                    }
                };
                Ok(std::mem::replace(field, new).map(|f| ParamValue::Float(f as f64)))
            }
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

impl Project {
    /// マスターバスのエフェクトのパラメータ(`fx/<id>/<name>` のみ)。
    fn set_master_param(
        &mut self,
        path: &ParamPath,
        value: Option<&ParamValue>,
    ) -> Result<Option<ParamValue>> {
        let ParamPath::Effect { id, name } = path else {
            return Err(CoreError::UnknownParam(path.clone()));
        };
        let ei = self
            .master_effect_index(id)
            .ok_or_else(|| CoreError::EffectNotFound(id.clone()))?;
        Ok(set_in_map(&mut self.master.effects[ei].params, name, value))
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

/// レーンを対象パスの文字列順の位置に挿入する(削除 → 逆コマンドで再追加したときに
/// 元の並びへ戻るよう、並びを正規形に保つ)。
fn insert_lane(
    lanes: &mut Vec<AutomationLane>,
    target: &ParamPath,
    points: Vec<crate::model::AutomationPoint>,
) {
    let key = target.to_string();
    let at = lanes.partition_point(|l| l.target.to_string() < key);
    lanes.insert(
        at,
        AutomationLane {
            target: target.clone(),
            points,
        },
    );
}

/// ノートのピッチカーブと滑る時間の検証。
fn check_note_extras(
    id: &crate::NoteId,
    curve: &[crate::model::PitchPoint],
    glide_ms: Option<f32>,
) -> Result<()> {
    crate::model::check_pitch_curve(curve)
        .map_err(|e| CoreError::OutOfRange(format!("note {id}: {e}")))?;
    if let Some(g) = glide_ms {
        if !crate::model::GLIDE_MS_RANGE.contains(&g) {
            return Err(CoreError::OutOfRange(format!(
                "note {id}: glide_ms {g}(10〜2000)"
            )));
        }
    }
    Ok(())
}

// ---- 位置・長さの上限 ---------------------------------------------------------

fn tick_ok(what: &str, t: Tick) -> Result<()> {
    if t > MAX_TICK {
        return Err(CoreError::OutOfRange(format!(
            "{what} {} は上限 {} を超えています(4/4 で 10 万小節まで)",
            t.0, MAX_TICK.0
        )));
    }
    Ok(())
}

fn notes_ok(notes: &[Note]) -> Result<()> {
    for n in notes {
        tick_ok("note pos", n.pos)?;
        tick_ok("note dur", n.dur)?;
        tick_ok("note end", n.pos + n.dur)?;
        pitch_curve_ok(&n.pitch_curve)?;
    }
    Ok(())
}

fn pitch_curve_ok(points: &[PitchPoint]) -> Result<()> {
    points
        .iter()
        .try_for_each(|p| tick_ok("pitch_curve tick", p.tick))
}

fn clip_ok(c: &Clip) -> Result<()> {
    tick_ok("clip start", c.start)?;
    tick_ok("clip length", c.length)?;
    tick_ok("clip end", c.start + c.length)?;
    if let ClipContent::Midi {
        notes, loop_len, ..
    } = &c.content
    {
        if let Some(l) = loop_len {
            tick_ok("loop_len", *l)?;
        }
        notes_ok(notes)?;
    }
    Ok(())
}

fn lanes_ok(lanes: &[AutomationLane]) -> Result<()> {
    lanes
        .iter()
        .flat_map(|l| &l.points)
        .try_for_each(|p| tick_ok("automation tick", p.tick))
}

/// エフェクトを足したときの逆コマンド。つながりの表があれば、消したあとに表を元に戻す
fn undo_add_effect(
    id: &crate::id::FxId,
    track: Option<&TrackId>,
    old_links: Option<Vec<crate::model::FxLink>>,
) -> Command {
    with_links(Command::RemoveEffect { id: id.clone() }, track, old_links)
}

/// `cmd` のあとにつながりの表を `old_links` に戻す(表が無かったなら `cmd` だけ)
fn with_links(
    cmd: Command,
    track: Option<&TrackId>,
    old_links: Option<Vec<crate::model::FxLink>>,
) -> Command {
    match old_links {
        None => cmd,
        Some(l) => Command::batch(
            "エフェクトとつながり",
            vec![
                cmd,
                Command::SetFxLinks {
                    track: track.cloned(),
                    links: Some(l),
                },
            ],
        ),
    }
}

/// コマンドに含まれる位置・長さがすべて [`MAX_TICK`] 以内か(適用の前に検査する)。
fn check_ticks(cmd: &Command) -> Result<()> {
    use Command::*;
    match cmd {
        AddTrack { track, .. } => {
            track.clips.iter().try_for_each(clip_ok)?;
            lanes_ok(&track.automation)
        }
        AddClip { clip, .. } | ReplaceClip { clip, .. } => clip_ok(clip),
        MoveClip { start, .. } => tick_ok("clip start", *start),
        ResizeClip { length, .. } => tick_ok("clip length", *length),
        SetClipLoop {
            loop_len: Some(l), ..
        } => tick_ok("loop_len", *l),
        SplitClip { at, .. } => tick_ok("split at", *at),
        AddNotes { notes, .. } => notes_ok(notes),
        UpdateNotes { changes, .. } => changes.iter().try_for_each(|c| {
            if let Some(p) = c.pos {
                tick_ok("note pos", p)?;
            }
            if let Some(d) = c.dur {
                tick_ok("note dur", d)?;
            }
            pitch_curve_ok(c.pitch_curve.as_deref().unwrap_or_default())
        }),
        SetAutomationPoints { points, .. } | SetMasterAutomationPoints { points, .. } => points
            .iter()
            .try_for_each(|p| tick_ok("automation tick", p.tick)),
        SetTempo { events } => events
            .iter()
            .try_for_each(|e| tick_ok("tempo tick", e.tick)),
        SetTimeSig { events } => events
            .iter()
            .try_for_each(|e| tick_ok("time_sig tick", e.tick)),
        SetSections { sections } => sections
            .iter()
            .try_for_each(|m| tick_ok("section tick", m.tick)),
        // Batch の中身は、それぞれを適用するときに検査される
        _ => Ok(()),
    }
}
