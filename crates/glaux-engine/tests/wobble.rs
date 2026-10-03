//! 声ごとのモジュレーター: 描き出した音の揺れが、テンポと速さのオートメーションに合う
use glaux_core::*;

/// 1 音(A5。1ms の窓で音量を測れる高さ)を 4 小節伸ばす subtractive。mod1 で音量を矩形に揺らす
fn project(rate: f64, lane: Option<Vec<(u64, f64)>>, bpm: f64) -> Project {
    let mut p = Project::new("w");
    p.tempo_map = TempoMap::new(vec![TempoEvent { tick: Tick(0), bpm }]).unwrap();
    let mut t = Track::new(TrackId::new(), "B", TrackKind::Midi);
    let mut d = Device::builtin("subtractive");
    for (k, v) in [
        ("sustain", 1.0),
        ("mod1_rate", rate),
        ("mod1_amp", 1.0),
        ("attack", 0.001),
        ("release", 0.01),
    ] {
        d.params.insert(k.into(), ParamValue::Float(v));
    }
    d.params
        .insert("mod1_shape".into(), ParamValue::Enum("square".into()));
    t.device = Some(d);
    let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 4));
    c.notes_mut().unwrap().push(Note {
        id: NoteId::new(),
        pos: Tick(0),
        dur: Tick(3840 * 4),
        pitch: 81,
        vel: 100,
        articulation: Default::default(),
        pitch_curve: vec![],
        glide_ms: None,
        vibrato: None,
        volume_curve: vec![],
        brightness_curve: vec![],
        condition: None,
    });
    t.clips.push(c);
    if let Some(points) = lane {
        t.automation.push(AutomationLane {
            target: ParamPath::device("mod1_rate"),
            points: points
                .into_iter()
                .map(|(tick, value)| AutomationPoint {
                    tick: Tick(tick),
                    value,
                    curve: Curve::Hold,
                })
                .collect(),
        });
    }
    p.tracks.push(t);
    p
}

/// 1ms ごとの音量(左)の、鳴っている所 / 消えている所の切り替わりの間隔(ms)
fn gate_intervals(out: &[f32], from_ms: usize, to_ms: usize) -> Vec<usize> {
    let ms: Vec<f32> = out
        .chunks(96)
        .map(|c| (c.iter().step_by(2).map(|v| v * v).sum::<f32>() / 48.0).sqrt())
        .collect();
    let peak = ms[from_ms..to_ms].iter().fold(0.0f32, |a, b| a.max(*b));
    let on: Vec<bool> = ms.iter().map(|v| *v > peak * 0.3).collect();
    let edges: Vec<usize> = (from_ms + 1..to_ms)
        .filter(|&i| on[i] && !on[i - 1])
        .collect();
    edges.windows(2).map(|w| w[1] - w[0]).collect()
}

#[test]
fn wobble_period_follows_tempo_and_rate_lane() {
    let bank = glaux_engine::SampleBank::default();
    // 1/8(拍あたり 2 回)・120 BPM = 250ms ごと
    let out =
        glaux_engine::export::render_project(&project(2.0, None, 120.0), 48_000.0, &bank).unwrap();
    let iv = gate_intervals(&out, 100, 1900);
    assert!(
        iv.len() >= 4 && iv.iter().all(|d| d.abs_diff(250) <= 3),
        "{iv:?}"
    );
    // 150 BPM なら 200ms
    let out =
        glaux_engine::export::render_project(&project(2.0, None, 150.0), 48_000.0, &bank).unwrap();
    let iv = gate_intervals(&out, 100, 1500);
    assert!(iv.iter().all(|d| d.abs_diff(200) <= 3), "{iv:?}");
    // 速さのオートメーション: 1 小節目は 1/4(500ms)、2 小節目から 1/16(125ms)
    let out = glaux_engine::export::render_project(
        &project(2.0, Some(vec![(0, 1.0), (3840, 4.0)]), 120.0),
        48_000.0,
        &bank,
    )
    .unwrap();
    let slow = gate_intervals(&out, 50, 1950);
    let fast = gate_intervals(&out, 2100, 3900);
    assert!(slow.iter().all(|d| d.abs_diff(500) <= 4), "{slow:?}");
    assert!(fast.iter().all(|d| d.abs_diff(125) <= 3), "{fast:?}");
}
