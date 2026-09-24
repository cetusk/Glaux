//! 曲の時間軸: 秒 ↔ tick ↔ 拍・小節と、再生中に起きる出来事(拍・マーカー・ノート)の一覧。
//!
//! ゲーム(Godot)側で「いま聞こえている位置」から敵の動きを決めたり、次の拍・次のキックを
//! 先読みしたりするのに使う。オーディオとは独立した純粋な計算で、再生と同じテンポマップ・
//! 拍子・ノート(ループの繰り返し展開込み)から作るので、鳴っている音とずれない。

use glaux_core::{ClipContent, Project, TempoMap, Tick};

/// 拍 1 つ。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeatMark {
    /// 曲頭からの秒
    pub sec: f64,
    pub tick: u64,
    /// 小節番号(1 始まり)
    pub bar: u32,
    /// 小節内の拍(1 始まり。1 = 小節の頭)
    pub beat: u32,
    /// その小節の拍数(拍子の分子)
    pub beats_in_bar: u32,
}

/// 曲構成のマーカー(「サビ」など)。
#[derive(Clone, Debug, PartialEq)]
pub struct SectionMark {
    pub sec: f64,
    pub tick: u64,
    pub name: String,
}

/// ノート 1 つ(ループの繰り返しは展開済み)。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteMark {
    pub sec: f64,
    pub dur_sec: f64,
    pub tick: u64,
    pub pitch: u8,
    pub vel: u8,
}

/// トラック 1 本分のノート(開始順)。
#[derive(Clone, Debug, PartialEq)]
pub struct TrackNotes {
    pub name: String,
    pub id: String,
    pub notes: Vec<NoteMark>,
}

/// 曲の時間軸。
#[derive(Clone, Debug)]
pub struct Timeline {
    tempo: TempoMap,
    beats: Vec<BeatMark>,
    sections: Vec<SectionMark>,
    tracks: Vec<TrackNotes>,
    /// 最後のノート・クリップ・マーカーの終わり(秒)
    end_sec: f64,
}

impl Timeline {
    pub fn from_project(project: &Project) -> Timeline {
        let tempo = project.tempo_map.clone();
        let ppq = project.ppq.max(1);
        let to_sec = |tick: u64| tempo.tick_to_seconds(Tick(tick));

        // 曲の終わり(tick): クリップ・マーカーの最後
        let mut end_tick = 0u64;
        for t in &project.tracks {
            for c in &t.clips {
                end_tick = end_tick.max((c.start + c.length).0);
            }
        }
        for s in &project.sections {
            end_tick = end_tick.max(s.tick.0);
        }

        // ノート(MIDI トラックのみ。ループは再生と同じく展開)
        let tracks = project
            .tracks
            .iter()
            .map(|t| {
                let mut notes: Vec<NoteMark> = t
                    .clips
                    .iter()
                    .filter(|c| matches!(c.content, ClipContent::Midi { .. }))
                    .flat_map(|c| {
                        c.playback_notes()
                            .into_iter()
                            .map(move |n| (c.start.0 + n.pos.0, n.dur.0, n.pitch, n.vel))
                    })
                    .map(|(start, dur, pitch, vel)| {
                        let sec = to_sec(start);
                        NoteMark {
                            sec,
                            dur_sec: to_sec(start + dur) - sec,
                            tick: start,
                            pitch,
                            vel,
                        }
                    })
                    .collect();
                notes.sort_by(|a, b| a.sec.total_cmp(&b.sec).then(a.pitch.cmp(&b.pitch)));
                TrackNotes {
                    name: t.name.clone(),
                    id: t.id.to_string(),
                    notes,
                }
            })
            .collect();

        // 拍: 拍子の区間ごとに、曲の終わりの次の小節の頭まで
        let mut sigs = project.time_sig_map.clone();
        sigs.sort_by_key(|s| s.tick);
        if sigs.first().is_none_or(|s| s.tick.0 > 0) {
            sigs.insert(
                0,
                glaux_core::TimeSigEvent {
                    tick: Tick::ZERO,
                    num: 4,
                    den: 4,
                },
            );
        }
        let mut beats = Vec::new();
        let mut bar = 0u32;
        for (i, sig) in sigs.iter().enumerate() {
            let num = sig.num.max(1) as u32;
            let beat_len = (ppq * 4 / sig.den.max(1) as u64).max(1);
            let seg_end = sigs.get(i + 1).map(|s| s.tick.0);
            let mut tick = sig.tick.0;
            let mut beat = 0u32;
            loop {
                if seg_end.is_some_and(|e| tick >= e) {
                    break;
                }
                // 最後の区間は、曲の終わりを過ぎた最初の小節の頭まで(その頭は含む)
                let last = seg_end.is_none() && tick > end_tick && beat == 0;
                if beat == 0 {
                    bar += 1;
                }
                beats.push(BeatMark {
                    sec: to_sec(tick),
                    tick,
                    bar,
                    beat: beat + 1,
                    beats_in_bar: num,
                });
                if last {
                    break;
                }
                beat = (beat + 1) % num;
                tick += beat_len;
            }
        }
        if beats.is_empty() {
            beats.push(BeatMark {
                sec: 0.0,
                tick: 0,
                bar: 1,
                beat: 1,
                beats_in_bar: 4,
            });
        }

        let mut sections: Vec<SectionMark> = project
            .sections
            .iter()
            .map(|s| SectionMark {
                sec: to_sec(s.tick.0),
                tick: s.tick.0,
                name: s.name.clone(),
            })
            .collect();
        sections.sort_by(|a, b| a.sec.total_cmp(&b.sec));

        Timeline {
            end_sec: to_sec(end_tick),
            tempo,
            beats,
            sections,
            tracks,
        }
    }

    /// 曲の終わり(最後のクリップ・マーカーの終わり。余韻は含まない)。
    pub fn end_sec(&self) -> f64 {
        self.end_sec
    }

    pub fn tick_to_sec(&self, tick: f64) -> f64 {
        let t0 = tick.max(0.0).floor();
        let a = self.tempo.tick_to_seconds(Tick(t0 as u64));
        let frac = tick.max(0.0) - t0;
        if frac <= 0.0 {
            return a;
        }
        a + (self.tempo.tick_to_seconds(Tick(t0 as u64 + 1)) - a) * frac
    }

    pub fn sec_to_tick(&self, sec: f64) -> f64 {
        self.tempo.seconds_to_tick_f64(sec.max(0.0))
    }

    pub fn bpm_at(&self, sec: f64) -> f64 {
        self.tempo.bpm_at(Tick(self.sec_to_tick(sec) as u64))
    }

    pub fn beats(&self) -> &[BeatMark] {
        &self.beats
    }

    pub fn sections(&self) -> &[SectionMark] {
        &self.sections
    }

    pub fn tracks(&self) -> &[TrackNotes] {
        &self.tracks
    }

    /// 名前(同名なら先頭)か ID でトラックを探す。
    pub fn track(&self, name_or_id: &str) -> Option<&TrackNotes> {
        self.tracks
            .iter()
            .find(|t| t.name == name_or_id)
            .or_else(|| self.tracks.iter().find(|t| t.id == name_or_id))
    }

    /// `sec` の位置が曲頭から何拍目か(小数。拍の途中は秒で比例配分)。
    /// 曲頭より前は負、最後の拍より後は最後の拍の長さで延ばす。
    pub fn beat_position(&self, sec: f64) -> f64 {
        let b = &self.beats;
        let len_at = |i: usize| -> f64 {
            if i + 1 < b.len() {
                b[i + 1].sec - b[i].sec
            } else if i > 0 {
                b[i].sec - b[i - 1].sec
            } else {
                60.0 / self.tempo.bpm_at(Tick::ZERO)
            }
        };
        if sec < b[0].sec {
            return (sec - b[0].sec) / len_at(0).max(1e-9);
        }
        let i = b.partition_point(|x| x.sec <= sec).saturating_sub(1);
        i as f64 + (sec - b[i].sec) / len_at(i).max(1e-9)
    }

    /// `sec` の位置の拍(その拍の頭の情報)。
    pub fn beat_at(&self, sec: f64) -> BeatMark {
        let i = self
            .beats
            .partition_point(|x| x.sec <= sec)
            .saturating_sub(1);
        self.beats[i]
    }

    /// `sec` より後(`sec` ちょうどは含まない)で最初の拍。最後の拍の後なら None。
    pub fn next_beat(&self, sec: f64) -> Option<BeatMark> {
        let i = self.beats.partition_point(|x| x.sec <= sec);
        self.beats.get(i).copied()
    }

    /// `(from, to]` に頭がある拍(再生中に「いま通り過ぎた拍」を知らせるのに使う)。
    pub fn beats_between(&self, from: f64, to: f64) -> &[BeatMark] {
        let a = self.beats.partition_point(|x| x.sec <= from);
        let b = self.beats.partition_point(|x| x.sec <= to);
        &self.beats[a..b.max(a)]
    }

    /// `(from, to]` に始まるマーカー。
    pub fn sections_between(&self, from: f64, to: f64) -> &[SectionMark] {
        let a = self.sections.partition_point(|x| x.sec <= from);
        let b = self.sections.partition_point(|x| x.sec <= to);
        &self.sections[a..b.max(a)]
    }

    /// `sec` の位置のマーカー(直前のもの)。
    pub fn section_at(&self, sec: f64) -> Option<&SectionMark> {
        let i = self.sections.partition_point(|x| x.sec <= sec);
        i.checked_sub(1).map(|i| &self.sections[i])
    }

    /// トラックの `(from, to]` に始まるノート。
    pub fn notes_between<'a>(
        &'a self,
        track: &'a TrackNotes,
        from: f64,
        to: f64,
    ) -> &'a [NoteMark] {
        let a = track.notes.partition_point(|x| x.sec <= from);
        let b = track.notes.partition_point(|x| x.sec <= to);
        &track.notes[a..b.max(a)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{
        Clip, ClipId, Note, NoteId, SectionMarker, TempoEvent, TimeSigEvent, Track, TrackId,
        TrackKind,
    };

    fn note(pos: u64, pitch: u8) -> Note {
        Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(240),
            pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
        }
    }

    /// 120BPM・4/4 で 2 小節、3 小節目から 3/4。キックはループ(1 小節 = 4 拍打ち)
    fn project() -> Project {
        let mut p = Project::new("t");
        p.time_sig_map = vec![
            TimeSigEvent {
                tick: Tick::ZERO,
                num: 4,
                den: 4,
            },
            TimeSigEvent {
                tick: Tick(7680),
                num: 3,
                den: 4,
            },
        ];
        let mut t = Track::new(TrackId::new(), "Kick", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "k", Tick(0), Tick(7680));
        if let ClipContent::Midi {
            notes,
            looped,
            loop_len,
        } = &mut c.content
        {
            *notes = (0..4).map(|i| note(i * 960, 36)).collect();
            *looped = true;
            *loop_len = Some(Tick(3840));
        }
        t.clips.push(c);
        p.tracks.push(t);
        p.sections = vec![
            SectionMarker {
                tick: Tick(0),
                name: "intro".into(),
            },
            SectionMarker {
                tick: Tick(7680),
                name: "サビ".into(),
            },
        ];
        p
    }

    #[test]
    fn beats_follow_time_signatures() {
        let tl = Timeline::from_project(&project());
        let b = tl.beats();
        // 4/4 × 2 小節 = 8 拍、3 小節目(3/4)は 3 拍、その次の小節頭で終わる
        assert_eq!((b[0].bar, b[0].beat, b[0].sec), (1, 1, 0.0));
        assert_eq!((b[4].bar, b[4].beat), (2, 1));
        assert_eq!((b[8].bar, b[8].beat, b[8].beats_in_bar), (3, 1, 3));
        assert_eq!((b[10].bar, b[10].beat), (3, 3));
        assert_eq!((b[11].bar, b[11].beat), (4, 1));
        assert_eq!(b.len(), 12);
        assert!((b[8].sec - 4.0).abs() < 1e-9, "120BPM で 8 拍 = 4 秒");
    }

    #[test]
    fn beat_position_and_lookups() {
        let tl = Timeline::from_project(&project());
        assert!((tl.beat_position(0.25) - 0.5).abs() < 1e-9);
        assert!((tl.beat_position(4.75) - 9.5).abs() < 1e-9);
        assert!(tl.beat_position(-0.5) < 0.0, "曲頭より前は負");
        assert_eq!(tl.beat_at(1.1).bar, 1);
        assert_eq!(tl.beat_at(1.1).beat, 3);
        let nb = tl.next_beat(1.0).unwrap();
        assert!((nb.sec - 1.5).abs() < 1e-9, "ちょうど拍の上なら次の拍");
        assert_eq!(tl.section_at(4.2).unwrap().name, "サビ");
        assert_eq!(tl.section_at(0.0).unwrap().name, "intro");
    }

    #[test]
    fn events_between_are_half_open() {
        let tl = Timeline::from_project(&project());
        // (0.4, 1.0] には 0.5 秒・1.0 秒の拍
        let b = tl.beats_between(0.4, 1.0);
        assert_eq!(b.len(), 2);
        // 続けて (1.0, 1.2] は空(同じ拍を 2 回知らせない)
        assert!(tl.beats_between(1.0, 1.2).is_empty());
        // ループが展開されてキックは 2 小節で 8 つ
        let kick = tl.track("Kick").unwrap();
        assert_eq!(kick.notes.len(), 8);
        assert_eq!(
            tl.notes_between(kick, 1.9, 2.1).len(),
            1,
            "2 小節目頭のキック"
        );
        assert_eq!(tl.sections_between(3.9, 4.0)[0].name, "サビ");
    }

    #[test]
    fn tempo_changes_move_beats() {
        let mut p = project();
        p.tempo_map = TempoMap::new(vec![
            TempoEvent {
                tick: Tick::ZERO,
                bpm: 120.0,
            },
            TempoEvent {
                tick: Tick(3840),
                bpm: 60.0,
            },
        ])
        .unwrap();
        let tl = Timeline::from_project(&p);
        let b = tl.beats();
        // 2 小節目からは 1 拍 1 秒
        assert!((b[4].sec - 2.0).abs() < 1e-9);
        assert!((b[5].sec - 3.0).abs() < 1e-9);
        assert!((tl.bpm_at(2.5) - 60.0).abs() < 1e-9);
        assert!((tl.tick_to_sec(tl.sec_to_tick(2.5)) - 2.5).abs() < 1e-6);
    }
}
