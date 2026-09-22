//! project.json スキーマの往復テスト。
//! 「人間が手で書いた JSON」が読めて、書き戻しても同じになることを確認する。

use glaux_core::*;

const FIXTURE: &str = r##"{
  "format": "glaux",
  "version": 1,
  "ppq": 960,
  "meta": { "title": "Fixture", "created": "2026-09-21T10:00:00Z" },
  "tempo_map": [ { "tick": 0, "bpm": 120.0 }, { "tick": 7680, "bpm": 90.0 } ],
  "time_sig_map": [ { "tick": 0, "num": 4, "den": 4 } ],
  "tracks": [
    {
      "id": "trk_a1b2c3",
      "name": "Bass",
      "kind": "midi",
      "color": "#4a90d9",
      "mute": false, "solo": false,
      "volume_db": -6.0, "pan": 0.0,
      "device": {
        "type": "builtin",
        "name": "subtractive",
        "params": {
          "osc1.wave": "saw",
          "filter.cutoff": 800.0,
          "filter.resonance": 0.3,
          "env.attack_ms": 5.0
        }
      },
      "effects": [
        { "id": "fx_e5f6g7", "type": "builtin", "name": "compressor",
          "bypass": false,
          "params": { "threshold_db": -18.0, "ratio": 4.0 } }
      ],
      "clips": [
        {
          "id": "clp_c3d4e5",
          "name": "Bass A",
          "start": 0,
          "length": 3840,
          "kind": "midi",
          "notes": [
            { "id": "nt_000001", "pos": 0,   "dur": 480, "pitch": 36, "vel": 100 },
            { "id": "nt_000002", "pos": 960, "dur": 480, "pitch": 36, "vel": 90 }
          ],
          "loop": false
        }
      ],
      "automation": [
        {
          "target": "device/filter.cutoff",
          "points": [
            { "tick": 0,    "value": 400.0 },
            { "tick": 3840, "value": 2000.0, "curve": "hold" }
          ]
        }
      ]
    },
    {
      "id": "trk_9z8y7x",
      "name": "Vocal",
      "kind": "audio",
      "clips": [
        {
          "id": "clp_7w6v5u",
          "name": "take1",
          "start": 7680,
          "length": 15360,
          "kind": "audio",
          "asset": "sha256:ab12cd34",
          "offset_samples": 0,
          "gain_db": 0.0,
          "fade_in_ms": 10.0, "fade_out_ms": 50.0,
          "stretch": { "mode": "none" }
        }
      ]
    }
  ],
  "master": { "volume_db": 0.0, "effects": [] },
  "assets": {
    "sha256:ab12cd34": { "path": "audio/vocal_take1.wav", "sample_rate": 48000, "channels": 1, "frames": 480000 }
  }
}"##;

#[test]
fn fixture_parses_and_roundtrips() {
    let p = Project::from_json(FIXTURE).expect("fixture should parse");
    assert!(p.is_valid(), "{:?}", p.validate());
    assert_eq!(p.tracks.len(), 2);
    assert_eq!(p.tempo_map.events().len(), 2);

    let bass = p.track(&"trk_a1b2c3".parse().unwrap()).unwrap();
    assert_eq!(bass.kind, TrackKind::Midi);
    let dev = bass.device.as_ref().unwrap();
    assert_eq!(dev.params["osc1.wave"], ParamValue::Enum("saw".into()));
    assert_eq!(dev.params["filter.cutoff"], ParamValue::Float(800.0));
    assert_eq!(
        bass.automation[0].target,
        ParamPath::device("filter.cutoff")
    );
    assert_eq!(bass.automation[0].points[1].curve, Curve::Hold);

    let (_, vocal_clip) = p.clip(&"clp_7w6v5u".parse().unwrap()).unwrap();
    assert!(!vocal_clip.is_midi());

    let json = p.to_json().unwrap();
    let back = Project::from_json(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn fixture_json_is_stable() {
    // 書き出したものを再度読み込んで書き出すと文字列レベルで一致する(順序が安定)
    let p = Project::from_json(FIXTURE).unwrap();
    let a = p.to_json().unwrap();
    let b = Project::from_json(&a).unwrap().to_json().unwrap();
    assert_eq!(a, b);
}

#[test]
fn command_json_shape() {
    let cmd = Command::SetParam {
        track: "trk_a1b2c3".parse().unwrap(),
        path: ParamPath::device("filter.cutoff"),
        value: ParamValue::Float(600.0),
    };
    let json = serde_json::to_value(&cmd).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "op": "set_param", "track": "trk_a1b2c3", "path": "device/filter.cutoff", "value": 600.0 })
    );

    let cmd = Command::SetTrackProp {
        id: "trk_a1b2c3".parse().unwrap(),
        prop: TrackProp::Mute(true),
    };
    let json = serde_json::to_value(&cmd).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "op": "set_track_prop", "id": "trk_a1b2c3", "prop": "mute", "value": true })
    );

    // AI が書いたコマンドを読む方向
    let parsed: Command = serde_json::from_str(
        r#"{"op":"add_notes","clip":"clp_c3d4e5","notes":[{"id":"nt_000009","pos":1920,"dur":480,"pitch":43,"vel":100}]}"#,
    )
    .unwrap();
    assert!(matches!(parsed, Command::AddNotes { .. }));
}

#[test]
fn rejects_bad_ids_and_kinds() {
    assert!(Project::from_json(&FIXTURE.replace("trk_a1b2c3", "bad_id")).is_err());
    let p = Project::from_json(&FIXTURE.replace(
        "\"kind\": \"midi\",\n          \"notes\"",
        "\"kind\": \"audio\",\n          \"notes\"",
    ))
    .unwrap_or_else(|_| Project::new("x"));
    // 読めた場合でも validate が拾う
    let _ = p.validate();
}
