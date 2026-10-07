//! 曲全体の書き出しを、いくつかのスレッドに分けて描く(CLAP プラグインの無い曲だけ)。
//!
//! - 1 段目: 音源のトラックを、サイドチェインでつながるもの同士の組に分け、組をいくつかの係(スレッド)に
//!   振り分けて描く。係の曲にはバスも置くが鳴らさず、入口に届いた音(出力先・センド)を取っておく。
//!   マスターの入口(エフェクトの前)の音も取っておく(係の曲のマスターは空にしてある)
//! - 2 段目: バスとマスターだけを残した曲を描き、バスとマスターの入口に、係が取っておいた音の合計を足して流す
//!
//! 1 回の処理(4096 フレーム)ごとに係から受け取って 2 段目に流すので、使うメモリは曲の長さによらない。
//! 違いはトラックを足し合わせる順(浮動小数の丸め)だけで、1 つで描いたときと同じ音になる。
//!
//! 並べられない曲(ソロ・トラックとバスの遅れのあるエフェクト・バスやマスターの音を検出信号に使うエフェクト・同時発音の多い曲・
//! 組が 1 つ・係が 1 人)は `None` を返し、呼び出し側が 1 つで描く。

use crate::data::{build_playback_data, BakedEffect, PlaybackData, SampleBank, MAX_TRACKS};
use crate::render::{block_step, OfflineRoute, Renderer, Shared};
use glaux_core::{MasterBus, Project, TrackKind};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;

/// 1 回の処理の長さ(フレーム)。1 つで描くときと同じ
pub(crate) const BLOCK: usize = 4096;

/// 係が 1 回の処理ごとに送る音: マスターの入口(左右)と、バスの入口(バスの並び順・左右)
struct Block {
    master: [Vec<f32>; 2],
    buses: Vec<[Vec<f32>; 2]>,
}

/// 同時発音の見積もりがこれを超える曲は並べない(発音数の上限で古い音を奪う所が、1 つで描いたときと変わるため。
/// 上限は 256。音を離した後の余韻の分を見込んで低めにする)
const MAX_PEAK_VOICES: usize = 160;

/// 曲全体を並べて描く。並べられない曲なら `None`。戻り値はステレオ・インターリーブで、
/// 末尾の無音は切り詰めていない(1 つで描いたときの `out` と同じ長さ)
pub(crate) fn render_whole(
    project: &Project,
    sample_rate: f64,
    bank: &SampleBank,
    master_clip: bool,
) -> Option<Vec<f32>> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    render_whole_with(project, sample_rate, bank, master_clip, threads)
}

pub(crate) fn render_whole_with(
    project: &Project,
    sample_rate: f64,
    bank: &SampleBank,
    master_clip: bool,
    threads: usize,
) -> Option<Vec<f32>> {
    if threads < 2
        || project.tracks.len() > MAX_TRACKS
        || project.tracks.iter().any(|t| t.solo)
        || !crate::plugins::project_plugins(project).is_empty()
    {
        return None;
    }
    let full = build_playback_data(project, sample_rate, bank);
    let buckets = plan(&full, threads)?;
    let end = full.end_sample;
    let step = block_step(&full);
    let cap = (end + (10.0 * sample_rate) as u64) as usize;
    let bus_ids: Vec<_> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Bus)
        .map(|t| t.id.clone())
        .collect();
    let nb = bus_ids.len();

    std::thread::scope(|scope| {
        // 1 段目: 係ごとに、受け持つトラックとすべてのバス(鳴らさない)だけの曲を描く
        let mut rxs: Vec<Receiver<Block>> = Vec::new();
        for bucket in &buckets {
            let keep: HashSet<_> = bucket
                .iter()
                .map(|&i| project.tracks[i].id.clone())
                .chain(bus_ids.iter().cloned())
                .collect();
            let mut part = project.clone();
            part.tracks.retain(|t| keep.contains(&t.id));
            part.master = MasterBus::default();
            let (tx, rx) = sync_channel::<Block>(8);
            rxs.push(rx);
            scope.spawn(move || {
                let mut data = build_playback_data(&part, sample_rate, bank);
                data.end_sample = end;
                let buses: Vec<usize> = part
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.kind == TrackKind::Bus)
                    .map(|(i, _)| i)
                    .collect();
                let route = OfflineRoute {
                    captured: vec![[Vec::new(), Vec::new()]; buses.len()],
                    capture: buses,
                    capture_master: true,
                    step: Some(step),
                    ..Default::default()
                };
                let (shared, mut r) = start(data, route, false);
                let mut buf = vec![0.0f32; BLOCK * 2];
                let mut made = 0usize;
                loop {
                    let base = r.clock();
                    r.offline_route().expect("設定済み").base = base;
                    r.process(&mut buf, 2);
                    let off = r.offline_route().expect("設定済み");
                    let take = |v: &mut [Vec<f32>; 2]| {
                        let mut out = [std::mem::take(&mut v[0]), std::mem::take(&mut v[1])];
                        out[0].resize(BLOCK, 0.0);
                        out[1].resize(BLOCK, 0.0);
                        out
                    };
                    let blk = Block {
                        master: take(&mut off.master_out),
                        buses: off.captured.iter_mut().map(take).collect(),
                    };
                    if tx.send(blk).is_err() {
                        break;
                    }
                    made += BLOCK;
                    if !shared.playing.load(Ordering::Acquire) || made >= cap {
                        break;
                    }
                }
            });
        }

        // 2 段目: バスとマスターだけの曲に、係の音の合計を流す
        let mut stage = project.clone();
        stage.tracks.retain(|t| t.kind == TrackKind::Bus);
        let mut data = build_playback_data(&stage, sample_rate, bank);
        data.end_sample = end;
        let route = OfflineRoute {
            inject: (0..nb).map(|j| (j, [Vec::new(), Vec::new()])).collect(),
            master_in: Some([Vec::new(), Vec::new()]),
            step: Some(step),
            ..Default::default()
        };
        let (shared, mut r) = start(data, route, master_clip);
        let mut out: Vec<f32> = Vec::with_capacity(((end as usize).min(cap) + BLOCK) * 2);
        let mut buf = vec![0.0f32; BLOCK * 2];
        let zero = || [vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]];
        while shared.playing.load(Ordering::Acquire) && out.len() / 2 < cap {
            let mut master = zero();
            let mut buses: Vec<[Vec<f32>; 2]> = (0..nb).map(|_| zero()).collect();
            for rx in &rxs {
                // 係が先に終わっていたら(同じ所で止まるので通常は無い)無音
                let Ok(blk) = rx.recv() else {
                    continue;
                };
                add(&mut master, &blk.master);
                for (acc, b) in buses.iter_mut().zip(&blk.buses) {
                    add(acc, b);
                }
            }
            let base = r.clock();
            let off = r.offline_route().expect("設定済み");
            off.base = base;
            off.master_in = Some(master);
            for ((_, dst), src) in off.inject.iter_mut().zip(buses) {
                *dst = src;
            }
            r.process(&mut buf, 2);
            out.extend_from_slice(&buf);
        }
        // 係に止まってもらう(受け手が無くなると送るのをやめる)
        drop(rxs);
        Some(out)
    })
}

/// 描き出し用のレンダラを用意する(再生中の状態から始める)
fn start(data: PlaybackData, route: OfflineRoute, master_clip: bool) -> (Arc<Shared>, Renderer) {
    let shared = Arc::new(Shared::new(data));
    shared.playing.store(true, Ordering::Release);
    shared.no_master_clip.store(!master_clip, Ordering::Release);
    let r = Renderer::new(shared.clone()).with_offline_route(route);
    (shared, r)
}

fn add(acc: &mut [Vec<f32>; 2], src: &[Vec<f32>; 2]) {
    for (a, s) in acc.iter_mut().zip(src) {
        for (x, y) in a.iter_mut().zip(s) {
            *x += *y;
        }
    }
}

/// 音源のトラックを組に分け、係(最大 `threads` 人)に振り分ける。並べられなければ `None`
fn plan(full: &PlaybackData, threads: usize) -> Option<Vec<Vec<usize>>> {
    if full.events.is_empty() && full.audio_events.is_empty() {
        return None;
    }
    let n = full.tracks.len();
    // トラック・バスの遅れのあるエフェクト(遅延補正が組をまたぐ)・プラグイン。
    // マスターのエフェクトは 2 段目でそのまま通すので、遅れがあってもよい
    let delayed = |fx: &[BakedEffect]| {
        fx.iter()
            .any(|e| e.plugin.is_some() || e.params.latency() > 0)
    };
    if full
        .tracks
        .iter()
        .any(|t| t.plugin.is_some() || delayed(&t.effects))
        || full.master_effects.iter().any(|e| e.plugin.is_some())
        || full
            .master_effects
            .iter()
            .any(|e| e.params.key_source().is_some())
    {
        return None;
    }
    // サイドチェインでつながる音源のトラックを同じ組に(バスの音を検出に使う・バスが検出に使うなら並べない)
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for (i, t) in full.tracks.iter().enumerate() {
        for e in &t.effects {
            let Some(k) = e.params.key_source().map(|k| k as usize) else {
                continue;
            };
            if k >= n {
                continue;
            }
            if t.is_bus || full.tracks[k].is_bus {
                return None;
            }
            let (a, b) = (root(&mut parent, i), root(&mut parent, k));
            parent[a] = b;
        }
    }
    // 組ごとの重さ(ノートと音声クリップの数。層の分も)
    let mut weight = vec![0usize; n];
    for e in full.events.iter() {
        if let Some(t) = full.tracks.get(e.track as usize) {
            weight[e.track as usize] += 1 + t.layers.len();
        }
    }
    for a in &full.audio_events {
        if let Some(w) = weight.get_mut(a.track as usize) {
            *w += 4;
        }
    }
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut slot_of: Vec<Option<usize>> = vec![None; n];
    for i in (0..n).filter(|&i| !full.tracks[i].is_bus) {
        let r = root(&mut parent, i);
        let g = *slot_of[r].get_or_insert_with(|| {
            groups.push((0, Vec::new()));
            groups.len() - 1
        });
        groups[g].0 += weight[i] + 1;
        groups[g].1.push(i);
    }
    if groups.len() < 2 || peak_voices(full) > MAX_PEAK_VOICES {
        return None;
    }
    // 重い組から順に、いちばん軽い係へ
    groups.sort_by_key(|g| std::cmp::Reverse(g.0));
    let k = threads.min(groups.len());
    let mut buckets: Vec<(usize, Vec<usize>)> = vec![(0, Vec::new()); k];
    for (w, tracks) in groups {
        let b = buckets
            .iter_mut()
            .min_by_key(|(load, _)| *load)
            .expect("係は 1 人以上");
        b.0 += w;
        b.1.extend(tracks);
    }
    let mut out: Vec<Vec<usize>> = buckets.into_iter().map(|(_, t)| t).collect();
    for b in &mut out {
        b.sort_unstable();
    }
    Some(out)
}

/// 同時に鳴る音の数の見積もり(離した後の余韻 1 秒を含める。層の分も)
fn peak_voices(full: &PlaybackData) -> usize {
    let tail = full.sample_rate as u64;
    let mut edges: Vec<(u64, i64)> = Vec::with_capacity(full.events.len() * 2);
    for e in full.events.iter() {
        let w = 1 + full
            .tracks
            .get(e.track as usize)
            .map_or(0, |t| t.layers.len()) as i64;
        edges.push((e.start, w));
        edges.push((e.end + tail, -w));
    }
    edges.sort_unstable();
    let (mut cur, mut peak) = (0i64, 0i64);
    for (_, d) in edges {
        cur += d;
        peak = peak.max(cur);
    }
    peak.max(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{
        AutomationLane, AutomationPoint, ClipContent, Effect, FxId, ParamPath, ParamValue, Send,
        Tick, Track, TrackId,
    };

    /// 同梱のデモ曲(サイドチェインでつながる組が 3 つ)の頭 4 小節に、バス(センド・出力先・バスからバスへ)・
    /// マスターのエフェクト(遅れのあるリミッタ)・音量のオートメーション(処理単位が細かくなる)を足した曲
    fn song() -> Project {
        let mut p = Project::from_json(include_str!("../../../app/src-tauri/demo/CyberNeon.json"))
            .expect("デモ曲");
        let bars = 4 * 4 * 960;
        for t in &mut p.tracks {
            for c in &mut t.clips {
                c.length = Tick(c.length.0.min(bars));
                if let ClipContent::Midi { notes, .. } = &mut c.content {
                    notes.retain(|n| n.pos.0 < bars);
                }
            }
        }
        let fx = |name: &str| Effect::builtin(FxId::new(), name);
        let mut verb = Track::new(TrackId::new(), "Verb", TrackKind::Bus);
        verb.effects.push(fx("reverb"));
        let mut echo = Track::new(TrackId::new(), "Echo", TrackKind::Bus);
        echo.effects.push(fx("delay"));
        echo.sends.push(Send {
            target: verb.id.clone(),
            level_db: -6.0,
            pre_fader: false,
        });
        let send = |t: &TrackId, db: f32, pre: bool| Send {
            target: t.clone(),
            level_db: db,
            pre_fader: pre,
        };
        p.tracks[1].sends.push(send(&verb.id, -3.0, false));
        p.tracks[5].sends.push(send(&echo.id, -8.0, true));
        p.tracks[4].output = Some(echo.id.clone());
        p.tracks[2].automation.push(AutomationLane {
            target: ParamPath::parse("track/volume_db").expect("パス"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -12.0,
                    curve: Default::default(),
                },
                AutomationPoint {
                    tick: Tick(bars),
                    value: 0.0,
                    curve: Default::default(),
                },
            ],
        });
        p.tracks.push(verb);
        p.tracks.push(echo);
        let mut eq = fx("eq");
        eq.params
            .insert("low_gain_db".into(), ParamValue::Float(2.0));
        p.master.effects.push(eq);
        p.master.effects.push(fx("limiter"));
        p.master.volume_db = -2.0;
        p
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len(), "長さが違う");
        a.iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
    }

    #[test]
    fn parallel_export_matches_the_single_renderer() {
        let p = song();
        let bank = SampleBank::default();
        let full = build_playback_data(&p, 48_000.0, &bank);
        let plan = plan(&full, 3).expect("並べられる曲");
        assert_eq!(plan.len(), 3, "組 3 つを係 3 人に: {plan:?}");
        let serial =
            crate::export::render_serial_for_test(&p, 48_000.0, &bank, true).expect("1 つで描く");
        let par = crate::export::trim_tail(
            render_whole_with(&p, 48_000.0, &bank, true, 3).expect("並べて描く"),
            48_000.0,
        );
        let d = max_diff(&serial, &par);
        assert!(d < 1e-5, "差 {d}");
        assert!(serial.iter().any(|v| v.abs() > 0.05), "音が出ている");
        assert!(
            serial.len() > 48_000 * 2 * 7,
            "4 小節ぶん: {}",
            serial.len()
        );
    }

    #[test]
    fn songs_that_cannot_be_split_fall_back() {
        let bank = SampleBank::default();
        // ソロ
        let mut p = song();
        p.tracks[0].solo = true;
        assert!(render_whole_with(&p, 48_000.0, &bank, true, 4).is_none());
        // トラックに遅れのあるエフェクト(先読みのリミッタ)
        let mut p = song();
        p.tracks[1]
            .effects
            .push(Effect::builtin(FxId::new(), "limiter"));
        let full = build_playback_data(&p, 48_000.0, &bank);
        assert!(plan(&full, 4).is_none());
        // 組が 1 つ(トラック 1 本)
        let mut p = song();
        p.tracks.truncate(1);
        let full = build_playback_data(&p, 48_000.0, &bank);
        assert!(plan(&full, 4).is_none());
        // 係が 1 人
        assert!(render_whole_with(&song(), 48_000.0, &bank, true, 1).is_none());
    }
}
