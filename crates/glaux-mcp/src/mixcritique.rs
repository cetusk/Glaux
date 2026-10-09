//! 音と混ぜ具合の点検(MCP の critique_mix)。エンジンの測定([`glaux_engine::mixcheck`])と、
//! つまみの設定から分かること(止まった音・広げられるユニゾン・空間が無い)から、指摘と直し方を組み立てる。
//!
//! 指摘は `{severity: warn | info, kind, track?, message, fix?}`。fix は直し方の道具と引数の案
//! (`{tool, args}`)か、文章の案(`hint`)。warn は直した方がよいもの、info は好みで検討するもの。

use glaux_core::{ParamValue, PluginSource, Project, Track, TrackKind};
use glaux_engine::mixcheck::MixCheck;
use serde_json::{json, Value};

/// 曲の頭からの秒 → "小節:拍"(1 始まり)
pub fn bar_beat(project: &Project, secs: f64) -> String {
    let tick = project.tempo_map.seconds_to_tick_f64(secs).max(0.0) as u64;
    let grid = glaux_core::arrange::bar_grid(project, tick + 1);
    let k = grid.partition_point(|(s, _)| *s <= tick).saturating_sub(1);
    let (start, _) = grid.get(k).copied().unwrap_or((0, 3840));
    let den = project
        .time_sig_map
        .iter()
        .filter(|s| s.tick.0 <= start)
        .max_by_key(|s| s.tick)
        .map_or(4, |s| s.den.max(1)) as f64;
    let beat = (tick - start) as f64 / (3840.0 / den) + 1.0;
    format!("{}:{}", k + 1, (beat * 10.0).round() / 10.0)
}

fn num(t: &Track, name: &str) -> Option<f64> {
    t.device
        .as_ref()?
        .params
        .get(name)
        .and_then(ParamValue::as_f64)
}

fn builtin(t: &Track) -> Option<&str> {
    match &t.device.as_ref()?.source {
        PluginSource::Builtin { name } => Some(name.as_str()),
        _ => None,
    }
}

/// ベースらしいトラックか(名前・低い音)
fn is_bassy(t: &Track) -> bool {
    let n = t.name.to_lowercase();
    if n.contains("bass") || n.contains("ベース") || n.contains("sub") || n.contains("サブ") {
        return true;
    }
    let mut p: Vec<u8> = t
        .clips
        .iter()
        .filter_map(|c| c.notes())
        .flat_map(|ns| ns.iter().map(|n| n.pitch))
        .collect();
    p.sort_unstable();
    p.get(p.len() / 2).is_some_and(|m| *m < 48)
}

/// 平均の音の長さ(tick)
fn mean_note_len(t: &Track) -> f64 {
    let ls: Vec<u64> = t
        .clips
        .iter()
        .filter_map(|c| c.notes())
        .flat_map(|ns| ns.iter().map(|n| n.dur.0))
        .collect();
    if ls.is_empty() {
        0.0
    } else {
        ls.iter().sum::<u64>() as f64 / ls.len() as f64
    }
}

/// ジャンルごとの目安(経験則。規格ではない): (名前の並び, PLR の下限 dB, 250Hz 以下の割合の範囲)。
/// PLR は True Peak − 統合ラウドネス(小さいほど潰れている)。低域の割合は analyze_audio の band_energy.low
pub const GENRES: &[(&[&str], f64, (f64, f64))] = &[
    (
        &[
            "edm",
            "house",
            "techno",
            "trance",
            "dubstep",
            "dnb",
            "drum_and_bass",
            "future_bass",
            "electro",
        ],
        7.0,
        (0.35, 0.62),
    ),
    (
        &["hiphop", "hip_hop", "trap", "rnb", "r&b"],
        7.0,
        (0.4, 0.66),
    ),
    (
        &[
            "pop", "jpop", "j-pop", "synthpop", "kpop", "rock", "funk", "disco",
        ],
        8.0,
        (0.25, 0.52),
    ),
    (&["metal", "punk"], 7.0, (0.22, 0.5)),
    (&["lofi", "lo-fi", "chill"], 9.0, (0.3, 0.56)),
    (
        &[
            "jazz",
            "acoustic",
            "folk",
            "classical",
            "ambient",
            "cinematic",
            "orchestral",
            "game",
        ],
        11.0,
        (0.15, 0.46),
    ),
];

/// ジャンルの名前 → 目安(PLR の下限, 低域の割合の範囲)。知らない名前は None
pub fn genre_targets(genre: &str) -> Option<(f64, (f64, f64))> {
    let g = genre.trim().to_lowercase().replace([' ', '-'], "_");
    GENRES
        .iter()
        .find(|(names, _, _)| names.iter().any(|n| n.replace('-', "_") == g))
        .map(|(_, plr, low)| (*plr, *low))
}

/// 測定と設定から指摘を組み立てる。`kick`・`bass` は低域の重なりを測ったトラック。
/// `genre` を渡すと、そのジャンルの目安(潰し具合・低域の量)とも比べる
pub fn findings(
    project: &Project,
    check: &MixCheck,
    kick: Option<&Track>,
    bass: Option<&Track>,
    genre: Option<&str>,
) -> Vec<Value> {
    let mut out = Vec::new();
    let track_by_id = |id: &str| project.tracks.iter().find(|t| t.id.to_string() == id);

    // ---- トラックごと(描き出しの測定) ----
    for c in &check.tracks {
        let Some(t) = track_by_id(&c.track_id) else {
            continue;
        };
        if let Some(db) = c.low_side_to_mid_db {
            // −6dB を超えたら直した方がよい。−12〜−6dB は、生楽器のステレオ録音やホールの響きなら自然なので好みで
            if db > -12.0 {
                let severity = if db > -6.0 { "warn" } else { "info" };
                let msg = format!(
                    "「{}」の低域(150Hz 以下)が左右に広がっている(左右の差が中央の {db:.0}dB)。モノで聴くと減り、クラブでは低音が揺れる",
                    t.name
                );
                let synth_spread = num(t, "spread").unwrap_or(0.0) > 0.0;
                out.push(json!({
                    "severity": severity,
                    "kind": "wide_low_end",
                    "track": t.name,
                    "track_id": t.id.to_string(),
                    "message": msg,
                    "fix": if synth_spread && is_bassy(t) {
                        json!({ "tool": "apply_commands", "args": { "commands": [
                            { "op": "set_param", "track": t.id.to_string(), "path": "device/spread", "value": 0.0 }
                        ], "label": "ベースの広がりを止める" }, "hint": "ベースは spread 0(低域は中央に)" })
                    } else {
                        json!({ "tool": "apply_commands", "args": { "commands": [
                            { "op": "add_effect", "track": t.id.to_string(),
                              "effect": { "type": "builtin", "name": "width", "params": { "width": 1.0, "mono_below_hz": 150.0 } } }
                        ], "label": "低域をモノに" }, "hint": "width の mono_below_hz 120〜150 で低域だけ中央に" })
                    },
                }));
            }
        }
        if !c.abrupt_starts.is_empty() || !c.abrupt_ends.is_empty() {
            let at: Vec<String> = c
                .abrupt_starts
                .iter()
                .chain(&c.abrupt_ends)
                .take(4)
                .map(|s| bar_beat(project, *s))
                .collect();
            let (what, fix) = match (t.kind, builtin(t)) {
                (TrackKind::Audio, _) => (
                    "音声クリップの頭・終わりが切れている",
                    json!({ "hint": "クリップの頭と終わりに数 ms のフェード(fade_in_ms・fade_out_ms)" }),
                ),
                (_, Some("subtractive" | "wavetable" | "fm")) => (
                    "立ち上がり・離しが速すぎる",
                    json!({ "tool": "apply_commands", "args": { "commands": [
                        { "op": "set_param", "track": t.id.to_string(), "path": "device/attack", "value": 0.004 },
                        { "op": "set_param", "track": t.id.to_string(), "path": "device/release", "value": 0.03 }
                    ], "label": "クリックを消す" }, "hint": "attack 3〜5ms、release 20ms 以上" }),
                ),
                _ => (
                    "音の頭・終わりが段差になっている",
                    json!({ "hint": "サンプルの頭の無音を詰めすぎていないか、release を長く" }),
                ),
            };
            out.push(json!({
                "severity": "warn",
                "kind": "clicks",
                "track": t.name,
                "track_id": t.id.to_string(),
                "message": format!(
                    "「{}」で音の頭 {} か所・終わり {} か所にクリック(プツッという音)。{what}。例: {}",
                    t.name, c.abrupt_starts.len(), c.abrupt_ends.len(), at.join("、")
                ),
                "fix": fix,
            }));
        }
    }

    // ---- キックとベース ----
    if let (Some(k), Some(b), Some(ov)) = (kick, bass, check.kick_bass_overlap) {
        let separated = b.effects.iter().any(|e| {
            matches!(&e.source, PluginSource::Builtin { name } if name == "sidechain" || name == "dynamic_eq")
                && e.params.get("source") == Some(&ParamValue::Enum(k.id.to_string()))
        });
        if ov >= 0.5 && !separated {
            out.push(json!({
                "severity": "warn",
                "kind": "kick_bass",
                "track": b.name,
                "track_id": b.id.to_string(),
                "message": format!(
                    "キック「{}」が鳴る時間の {:.0}% で、ベース「{}」も 40〜120Hz で強く鳴っている(低音がぶつかって濁る・音圧が出ない)",
                    k.name, ov * 100.0, b.name
                ),
                "fix": { "tool": "apply_recipe", "args": { "recipe": "kick_bass", "kick": k.id.to_string(), "bass": b.id.to_string() },
                         "hint": "ベースをキックで沈める(サイドチェイン)か、キックの音程を主音に合わせる" },
            }));
        }
    }

    // ---- 潰れ(打楽器のクレスト) ----
    for c in &check.tracks {
        let Some(t) = track_by_id(&c.track_id) else {
            continue;
        };
        if crate::recipes::is_drum(t) && c.peak_db > -40.0 && c.crest_db > 0.0 && c.crest_db < 9.0 {
            out.push(json!({
                "severity": "info",
                "kind": "squashed_drums",
                "track": t.name,
                "track_id": t.id.to_string(),
                "message": format!("「{}」のクレスト(ピーク − RMS)が {:.1} dB(打楽器はふつう 12 以上。音の頭が潰れて前に出ない)", t.name, c.crest_db),
                "fix": { "tool": "check_distortion", "args": { "track_id": t.id.to_string() },
                         "hint": "歪み・コンプ・リミッタを弱める(コンプは attack を 10〜30ms に)。check_distortion で、エフェクトを外した音と比べる" },
            }));
        }
    }

    // ---- 曲全体 ----
    let mix = &check.mix;
    if let Some(mc) = &mix.master_clip {
        if mc.max_reduction_db >= 1.0 || mc.pre_clip_peak_db > 0.0 {
            out.push(json!({
                "severity": "warn",
                "kind": "master_clip",
                "message": format!(
                    "マスターの最後のクリップ防止で最大 {:.1} dB 押さえ込まれている(防止の前のピーク {:+.1} dBFS、時間の {:.1}%)。ピークが潰れて歪む",
                    mc.max_reduction_db, mc.pre_clip_peak_db, mc.engaged_ratio * 100.0
                ),
                "fix": { "tool": "master_mix", "args": {}, "hint": "マスターの音量か、大きいトラックの音量を下げる。音圧が要るなら master_mix の limiter で(クリップ防止に頼らない)" },
            }));
        }
    }
    if let Some((plr_min, (lo, hi))) = genre.and_then(genre_targets) {
        let g = genre.unwrap_or_default();
        if mix.plr_db.is_finite() && mix.plr_db < plr_min {
            out.push(json!({
                "severity": if mix.plr_db < plr_min - 2.0 { "warn" } else { "info" },
                "kind": "genre_plr",
                "message": format!("PLR が {:.1} dB({g} の目安は {plr_min:.0} 以上。下回ると潰しすぎで、正規化される配信では小さく聞こえる)", mix.plr_db),
                "fix": { "hint": "マスターのリミッタ・クリッパの量を減らす(input_db・drive を下げる)。check_distortion で潰れの元を探す" },
            }));
        }
        let low = mix.band_energy.low;
        if low < lo || low > hi {
            out.push(json!({
                "severity": "info",
                "kind": "genre_low",
                "message": format!(
                    "250Hz 以下の割合が {:.2}({g} の目安は {lo:.2}〜{hi:.2}。{})",
                    low,
                    if low < lo { "低音が足りず軽い" } else { "低音が多くこもる・ほかが埋もれる" }
                ),
                "fix": { "hint": if low < lo { "キック・ベースを上げるか、ベースに倍音(saturator)を足す" } else { "ベース以外にハイパス、ベースとキックの重なりを sidechain で" } },
            }));
        }
    }
    if mix.true_peak_dbtp > -1.0 {
        out.push(json!({
            "severity": "warn",
            "kind": "true_peak",
            "message": format!("True Peak が {:.1} dBTP(-1 を超えると配信の変換で歪む)", mix.true_peak_dbtp),
            "fix": { "tool": "master_mix", "args": {}, "hint": "マスターに limiter(ceiling -1)か master_mix" },
        }));
    }
    if mix.stereo.mono_loudness_change_db < -2.5 {
        out.push(json!({
            "severity": "warn",
            "kind": "mono_compat",
            "message": format!(
                "モノで聴くと {:.1}dB 小さくなる(左右で打ち消し合う成分が多い。スマホ・店内放送で痩せる)",
                mix.stereo.mono_loudness_change_db
            ),
            "fix": { "hint": "広げすぎの音(spread・width・chorus)を控えめに、逆相の重なりを確かめる" },
        }));
    }
    let dev = |lo: u32, hi: u32| {
        mix.tonal_balance
            .deviations
            .iter()
            .filter(|(hz, _)| (lo..=hi).contains(hz))
            .map(|(_, d)| *d)
            .fold(f64::MIN, f64::max)
    };
    let harsh = dev(2000, 5000);
    if harsh > 4.0 {
        // 高域の多いトラックを疑う
        let suspect = check
            .tracks
            .iter()
            .filter(|c| c.peak_db > -40.0)
            .filter_map(|c| track_by_id(&c.track_id))
            .max_by(|a, b| centroid(check, a).total_cmp(&centroid(check, b)));
        out.push(json!({
            "severity": "warn",
            "kind": "harsh",
            "track": suspect.map(|t| t.name.clone()),
            "message": format!("2〜5kHz が全体の傾きより {harsh:.1}dB 出ている(耳に痛い・疲れる)"),
            "fix": match suspect {
                Some(t) => json!({ "tool": "apply_commands", "args": { "commands": [
                    { "op": "add_effect", "track": t.id.to_string(), "effect": { "type": "builtin", "name": "resonance" } }
                ], "label": "刺さる共鳴を抑える" }, "hint": format!("「{}」に resonance(共鳴抑制)か、EQ で 3kHz 付近を -2〜3dB", t.name) }),
                None => json!({ "hint": "刺さるトラックに resonance か、EQ で 3kHz 付近を下げる" }),
            },
        }));
    }
    let mud = dev(200, 400);
    if mud > 4.0 {
        out.push(json!({
            "severity": "info",
            "kind": "muddy",
            "message": format!("200〜400Hz が全体の傾きより {mud:.1}dB 出ている(こもり・濁り)"),
            "fix": { "hint": "パッド・ギター・ピアノの 250〜400Hz を EQ で -2〜3dB、ベース以外にハイパス" },
        }));
    }
    for m in check.masking.iter().take(3) {
        out.push(json!({
            "severity": "info",
            "kind": "masking",
            "track": m.track,
            "message": format!(
                "「{}」の {}〜{}Hz が「{}」に隠れている(時間の {:.0}%)",
                m.track, m.band_hz.0, m.band_hz.1, m.masked_by, m.time_ratio * 100.0
            ),
            "fix": { "hint": format!("「{}」のその帯域を EQ で -{:.1}dB、または片方の音域・定位をずらす", m.masked_by, m.suggest_cut_db) },
        }));
    }

    // ---- 設定から分かること(止まった音・空間) ----
    for t in &project.tracks {
        let Some(name) = builtin(t) else { continue };
        if !matches!(name, "subtractive" | "wavetable") || t.kind == TrackKind::Bus {
            continue;
        }
        let unison = num(t, "unison").unwrap_or(1.0);
        let spread = num(t, "spread").unwrap_or(0.0);
        if unison >= 3.0 && spread == 0.0 && !is_bassy(t) {
            out.push(json!({
                "severity": "info",
                "kind": "narrow_unison",
                "track": t.name,
                "track_id": t.id.to_string(),
                "message": format!("「{}」はユニゾン {} 声だが中央に重なっている(広げると厚みが増す。モノでは変わらない)", t.name, unison),
                "fix": { "tool": "apply_commands", "args": { "commands": [
                    { "op": "set_param", "track": t.id.to_string(), "path": "device/spread", "value": 0.6 }
                ], "label": "ユニゾンを広げる" } },
            }));
        }
        let moving = num(t, "analog").unwrap_or(0.0) > 0.0
            || num(t, "lfo1_depth").unwrap_or(0.0) > 0.0
            || num(t, "lfo2_depth").unwrap_or(0.0) > 0.0
            || !t.modulators.is_empty()
            || t.automation.iter().any(|l| l.points.len() > 1);
        if !moving && mean_note_len(t) >= glaux_core::PPQ as f64 * 2.0 {
            out.push(json!({
                "severity": "info",
                "kind": "static_sound",
                "track": t.name,
                "track_id": t.id.to_string(),
                "message": format!("「{}」の長い音がまったく動かない(止まった音は打ち込みっぽく聞こえる)", t.name),
                "fix": { "tool": "apply_commands", "args": { "commands": [
                    { "op": "set_param", "track": t.id.to_string(), "path": "device/analog", "value": 0.3 }
                ], "label": "揺らぎを足す" }, "hint": "analog 0.2〜0.4、ゆっくりした LFO(cutoff 0.2Hz・深さ 0.15)、modulate" },
            }));
        }
    }
    let playing: Vec<&Track> = project
        .tracks
        .iter()
        .filter(|t| {
            t.kind != TrackKind::Bus
                && t.clips
                    .iter()
                    .any(|c| c.notes().is_none_or(|n| !n.is_empty()))
        })
        .collect();
    let has_space = project.tracks.iter().any(|t| {
        t.effects.iter().any(|e| {
            matches!(&e.source, PluginSource::Builtin { name } if name == "reverb" || name == "convolution" || name == "delay")
        })
    });
    if playing.len() >= 3 && !has_space {
        out.push(json!({
            "severity": "info",
            "kind": "no_space",
            "message": "どのトラックにもリバーブ・ディレイが無い(音が近く平面的に聞こえる)",
            "fix": { "tool": "apply_recipe", "args": { "recipe": "send_reverb", "tracks": playing.iter().filter(|t| !crate::recipes::is_drum(t)).map(|t| t.id.to_string()).collect::<Vec<_>>() } },
        }));
    }
    out.sort_by_key(|f| if f["severity"] == "warn" { 0 } else { 1 });
    out
}

/// トラックのスペクトルの重心(高いほど刺さりやすい)
fn centroid(check: &MixCheck, t: &Track) -> f64 {
    check
        .tracks
        .iter()
        .find(|c| c.track_id == t.id.to_string())
        .map_or(f64::MIN, |c| c.centroid_hz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_beat_counts_from_one() {
        let p = Project::new("t");
        // 120 BPM: 2 秒 = 4 拍 = 2 小節目の頭
        assert_eq!(bar_beat(&p, 0.0), "1:1");
        assert_eq!(bar_beat(&p, 2.0), "2:1");
        assert_eq!(bar_beat(&p, 2.75), "2:2.5");
    }

    #[test]
    fn genre_names_are_forgiving() {
        assert_eq!(genre_targets("House").map(|g| g.0), Some(7.0));
        assert_eq!(genre_targets("J-POP").map(|g| g.0), Some(8.0));
        assert_eq!(genre_targets(" drum and bass ").map(|g| g.0), Some(7.0));
        assert_eq!(genre_targets("lo-fi").map(|g| g.0), Some(9.0));
        assert!(genre_targets("polka").is_none());
    }
}
