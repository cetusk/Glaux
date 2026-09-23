//! CLAP プラグインのプリセットから、目標の音に近いものを探す。
//!
//! 1. 索引: プリセットを 1 つずつ C4 で 1 秒鳴らし(余韻込み 2 秒)、小さな要約
//!    (`sound_match::summary`)と CLAP の埋め込み(モデルがあれば)を設定フォルダのキャッシュに保存する。
//!    一度に作りきれない数でも、時間の上限までで区切って続きは次回に回す(何度でも続きから)。
//! 2. 絞り込み: 目標の要約・埋め込みとの近さの順位を足し合わせ(順位の融合)、上位の候補を選ぶ。
//! 3. 確かめ: 候補を目標と同じ高さ・長さで鳴らし直し、`sound_match::compare` の距離で並べ直す。

use glaux_engine::plugins::{render_presets, PresetRenderSpec};
use glaux_engine::sound_match;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

const INDEX_VERSION: u32 = 1;
const INDEX_PITCH: u8 = 60;
const RENDER_RATE: f64 = 48_000.0;

#[derive(Serialize, Deserialize, Default)]
struct IndexFile {
    version: u32,
    plugin_id: String,
    /// プリセット ID → 要約
    entries: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    name: String,
    category: String,
    summary: Vec<f32>,
    /// CLAP の埋め込み(int8 + base64。値 = q / 127 × 0.35)。モデルが無いときに作った分は None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clap: Option<String>,
    /// 無音だった(鳴らない・効果音用など)
    #[serde(default)]
    silent: bool,
}

/// 索引の置き場所(`%APPDATA%\glaux\cache\presets\<プラグイン>.json`)。
fn index_path(plugin_id: &str) -> PathBuf {
    let safe: String = plugin_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    crate::presets::default_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default()
        .join("cache")
        .join("presets")
        .join(format!("{safe}.json"))
}

fn load_index(plugin_id: &str) -> IndexFile {
    std::fs::read(index_path(plugin_id))
        .ok()
        .and_then(|b| serde_json::from_slice::<IndexFile>(&b).ok())
        .filter(|f| f.version == INDEX_VERSION && f.plugin_id == plugin_id)
        .unwrap_or_else(|| IndexFile {
            version: INDEX_VERSION,
            plugin_id: plugin_id.to_owned(),
            entries: BTreeMap::new(),
        })
}

fn save_index(f: &IndexFile) -> Result<(), String> {
    let path = index_path(&f.plugin_id);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(f).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn encode_embedding(e: &[f32]) -> String {
    use base64::Engine;
    let q: Vec<u8> = e
        .iter()
        .map(|v| ((v / 0.35 * 127.0).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect();
    base64::engine::general_purpose::STANDARD.encode(q)
}

fn decode_embedding(s: &str) -> Option<Vec<f32>> {
    use base64::Engine;
    let b = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
    Some(b.iter().map(|v| *v as i8 as f32 / 127.0 * 0.35).collect())
}

fn round3(v: &[f32]) -> Vec<f32> {
    v.iter().map(|x| (x * 1000.0).round() / 1000.0).collect()
}

/// 索引の進み具合。
#[derive(Clone, Copy, Debug, Serialize)]
pub struct IndexProgress {
    /// 対象(絞り込み後)のプリセット数と、そのうち索引済みの数
    pub total: usize,
    pub indexed: usize,
    /// 今回新しく鳴らした数
    pub added: usize,
}

/// 対象のプリセットの索引を、`budget` 秒以内で作り足す。
pub fn build_index(
    plugin_id: &str,
    category: Option<&str>,
    budget: std::time::Duration,
    progress: &mut dyn FnMut(IndexProgress),
) -> Result<IndexProgress, String> {
    let started = std::time::Instant::now();
    let list = glaux_engine::plugins::presets(plugin_id, false)?;
    let wanted: Vec<glaux_clap::PresetEntry> = list
        .iter()
        .filter(|p| category_matches(&p.category, category))
        .cloned()
        .collect();
    let mut index = load_index(plugin_id);
    let missing: Vec<glaux_clap::PresetEntry> = wanted
        .iter()
        .filter(|p| !index.entries.contains_key(&p.id()))
        .cloned()
        .collect();
    let use_clap = glaux_ml::clap::available();
    let spec = PresetRenderSpec {
        pitch: INDEX_PITCH,
        velocity: 0.8,
        hold: 1.0,
        total: 2.0,
        sample_rate: RENDER_RATE,
    };
    let total = wanted.len();
    let mut indexed = total - missing.len();
    let mut added = 0;
    // 鳴らした音は別スレッドで要約・埋め込みにする(CLAP の推論が鳴らすより遅いため)
    let (tx, rx) = std::sync::mpsc::channel::<(glaux_clap::PresetEntry, Vec<f32>)>();
    let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 8);
    let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Entry)>::new()));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let rx = rx.clone();
            let results = results.clone();
            std::thread::spawn(move || loop {
                let job = rx.lock().map(|r| r.recv());
                let Ok(Ok((preset, frames))) = job else { break };
                let peak = frames.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                let entry = if peak < 1e-4 {
                    Entry {
                        name: preset.name.clone(),
                        category: preset.category.clone(),
                        summary: Vec::new(),
                        clap: None,
                        silent: true,
                    }
                } else {
                    let clap = if use_clap {
                        glaux_ml::clap::embed(&frames, RENDER_RATE as f32)
                            .ok()
                            .map(|e| encode_embedding(&e))
                    } else {
                        None
                    };
                    Entry {
                        name: preset.name.clone(),
                        category: preset.category.clone(),
                        summary: round3(&sound_match::summary(&frames, RENDER_RATE as f32)),
                        clap,
                        silent: false,
                    }
                };
                if let Ok(mut r) = results.lock() {
                    r.push((preset.id(), entry));
                }
            })
        })
        .collect();
    if !missing.is_empty() {
        render_presets(plugin_id, &missing, spec, |i, r| {
            if let Ok(frames) = r {
                let _ = tx.send((missing[i].clone(), frames));
            }
            added += 1;
            indexed += 1;
            if added % 25 == 0 {
                progress(IndexProgress {
                    total,
                    indexed,
                    added,
                });
            }
            started.elapsed() < budget
        })?;
    }
    drop(tx);
    for h in handles {
        let _ = h.join();
    }
    let done = std::mem::take(&mut *results.lock().map_err(|e| e.to_string())?);
    for (id, e) in done {
        index.entries.insert(id, e);
    }
    save_index(&index)?;
    let p = IndexProgress {
        total,
        indexed: wanted
            .iter()
            .filter(|p| index.entries.contains_key(&p.id()))
            .count(),
        added,
    };
    progress(p);
    Ok(p)
}

fn category_matches(category: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => category.to_lowercase().starts_with(&f.to_lowercase()),
    }
}

/// 見つかった候補。
#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    pub category: String,
    /// 目標と同じ高さ・長さで鳴らし直して比べた距離(0.15 未満 ほぼ同じ … 0.7 以上 かなり違う)
    pub distance: f32,
    pub spectral: f32,
    pub envelope: f32,
    /// CLAP での印象の近さ(-1〜1。モデルがあるとき)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clap_similarity: Option<f32>,
}

/// 目標の音に近いプリセットを探す(索引済みのものの中から)。`shortlist` 個を鳴らし直して確かめ、`limit` 個返す。
pub fn find_similar(
    plugin_id: &str,
    target: &crate::sound::LoadedSound,
    category: Option<&str>,
    limit: usize,
    shortlist: usize,
) -> Result<Vec<Candidate>, String> {
    let index = load_index(plugin_id);
    let list = glaux_engine::plugins::presets(plugin_id, false)?;
    let d = crate::sound::describe(target);
    let pitch = d
        .pitch
        .as_ref()
        .map(|p| p.midi)
        .or_else(|| glaux_engine::timbre::dominant_pitch(&target.frames, target.sample_rate))
        .unwrap_or(INDEX_PITCH);
    let hold = crate::sound::estimate_hold(&d) as f64;
    let target_summary = sound_match::summary(&target.frames, target.sample_rate);
    let target_clap = if glaux_ml::clap::available() {
        Some(crate::sound::embedding(target)?)
    } else {
        None
    };
    // 索引の中で候補を順位づける
    let rows: Vec<(&glaux_clap::PresetEntry, &Entry)> = list
        .iter()
        .filter(|p| category_matches(&p.category, category))
        .filter_map(|p| index.entries.get(&p.id()).map(|e| (p, e)))
        .filter(|(_, e)| !e.silent)
        .collect();
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let by_summary: Vec<f32> = rows
        .iter()
        .map(|(_, e)| sound_match::summary_distance(&target_summary, &e.summary))
        .collect();
    let by_clap: Option<Vec<f32>> = target_clap.as_ref().map(|t| {
        rows.iter()
            .map(|(_, e)| {
                e.clap
                    .as_deref()
                    .and_then(decode_embedding)
                    .map(|v| 1.0 - glaux_ml::clap::similarity(t, &v))
                    .unwrap_or(2.0)
            })
            .collect()
    });
    let rank = |v: &[f32]| -> Vec<usize> {
        let mut idx: Vec<usize> = (0..v.len()).collect();
        idx.sort_by(|a, b| v[*a].total_cmp(&v[*b]));
        let mut r = vec![0; v.len()];
        for (pos, i) in idx.into_iter().enumerate() {
            r[i] = pos;
        }
        r
    };
    // 順位の融合(reciprocal rank fusion)
    let rs = rank(&by_summary);
    let rc = by_clap.as_ref().map(|c| rank(c));
    let mut score: Vec<(usize, f32)> = (0..rows.len())
        .map(|i| {
            let mut s = 1.0 / (60.0 + rs[i] as f32);
            if let Some(rc) = &rc {
                s += 1.0 / (60.0 + rc[i] as f32);
            }
            (i, s)
        })
        .collect();
    score.sort_by(|a, b| b.1.total_cmp(&a.1));
    let picked: Vec<usize> = score
        .iter()
        .take(shortlist.max(limit))
        .map(|(i, _)| *i)
        .collect();
    // 目標と同じ高さ・長さで鳴らし直して確かめる
    let presets: Vec<glaux_clap::PresetEntry> = picked.iter().map(|i| rows[*i].0.clone()).collect();
    let secs = (target.frames.len() as f64 / target.sample_rate as f64).clamp(0.3, 4.0);
    let spec = PresetRenderSpec {
        pitch,
        velocity: 0.8,
        hold,
        total: secs,
        sample_rate: RENDER_RATE,
    };
    let mut out = Vec::new();
    render_presets(plugin_id, &presets, spec, |i, r| {
        if let Ok(frames) = r {
            let dist = sound_match::compare(
                &target.frames,
                target.sample_rate,
                &frames,
                RENDER_RATE as f32,
            );
            let row = rows[picked[i]];
            let clap_similarity = match (&target_clap, &by_clap) {
                (Some(_), Some(c)) if c[picked[i]] < 2.0 => Some(1.0 - c[picked[i]]),
                _ => None,
            };
            out.push(Candidate {
                id: row.0.id(),
                name: row.1.name.clone(),
                category: row.1.category.clone(),
                distance: dist.total,
                spectral: dist.spectral,
                envelope: dist.envelope,
                clap_similarity,
            });
        }
        true
    })?;
    out.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    out.truncate(limit);
    Ok(out)
}
