//! SoundFont(.sf2)の読み込みとゾーン構築。
//!
//! パースは rustysynth(MIT、依存なし)に任せ、プリセット層とインストゥルメント層の
//! パラメータ合成(SF2 仕様: プリセット側は加算オフセット)をここで行って
//! `glaux_dsp::Zone` の列に落とす。再生は glaux-dsp のマルチサンプラー。
//!
//! .sf2 は曲プロジェクトにコピーせず、**全プロジェクト共通のライブラリフォルダ**
//! (`<設定ディレクトリ>/glaux/soundfonts/`)から名前で参照する
//! (FluidR3 などは 100MB 級で、プロジェクトごとの複製は現実的でないため)。

use glaux_dsp::{SampleData, Zone, ZoneEnv};
use rustysynth::SoundFont;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 既定の SoundFont ライブラリフォルダ。
pub fn default_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .or_else(|_| std::env::var("XDG_CONFIG_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".config")
        });
    base.join("glaux").join("soundfonts")
}

/// ライブラリフォルダ内の .sf2 ファイル名一覧。
pub fn list_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("sf2"))
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PresetMeta {
    pub bank: u16,
    pub preset: u16,
    pub name: String,
}

pub fn load_font(path: &Path) -> Result<Arc<SoundFont>, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("SoundFont を開けません({}): {e}", path.display()))?;
    SoundFont::new(&mut file)
        .map(Arc::new)
        .map_err(|e| format!("SoundFont として読めません({}): {e}", path.display()))
}

/// バイト列から SoundFont を読む(ゲームエンジンのパック内のファイルなど、パスで開けないとき)。
pub fn load_font_bytes(bytes: &[u8]) -> Result<Arc<SoundFont>, String> {
    let mut cur = std::io::Cursor::new(bytes);
    SoundFont::new(&mut cur)
        .map(Arc::new)
        .map_err(|e| format!("SoundFont として読めません: {e}"))
}

/// フォント内のプリセット一覧(バンク・番号順)。
pub fn list_presets(font: &SoundFont) -> Vec<PresetMeta> {
    let mut out: Vec<PresetMeta> = font
        .get_presets()
        .iter()
        .map(|p| PresetMeta {
            bank: p.get_bank_number() as u16,
            preset: p.get_patch_number() as u16,
            name: p.get_name().to_owned(),
        })
        .collect();
    out.sort_by_key(|m| (m.bank, m.preset));
    out
}

/// 指定プリセットのゾーン列を構築する。見つからなければ None。
pub fn build_zones(font: &SoundFont, bank: u16, preset: u16) -> Option<Arc<Vec<Zone>>> {
    let target = font
        .get_presets()
        .iter()
        .find(|p| p.get_bank_number() == bank as i32 && p.get_patch_number() == preset as i32)?;

    let wave = font.get_wave_data();
    let headers = font.get_sample_headers();
    let instruments = font.get_instruments();
    let mut zones = Vec::new();

    for pr in target.get_regions() {
        let Some(inst) = instruments.get(pr.get_instrument_id()) else {
            continue;
        };
        for ir in inst.get_regions() {
            // 音域・ベロシティはプリセット側とインストゥルメント側の積(交差)
            let key_lo = pr.get_key_range_start().max(ir.get_key_range_start());
            let key_hi = pr.get_key_range_end().min(ir.get_key_range_end());
            let vel_lo = pr
                .get_velocity_range_start()
                .max(ir.get_velocity_range_start());
            let vel_hi = pr.get_velocity_range_end().min(ir.get_velocity_range_end());
            if key_lo > key_hi || vel_lo > vel_hi {
                continue;
            }

            let start = ir.get_sample_start().max(0) as usize;
            let end = ir.get_sample_end().max(0) as usize;
            if end <= start || end > wave.len() {
                continue;
            }
            let frames: Vec<f32> = wave[start..end]
                .iter()
                .map(|&s| s as f32 / 32768.0)
                .collect();

            let sample_rate = headers
                .get(ir.get_sample_id())
                .map(|h| h.get_sample_rate() as f32)
                .unwrap_or(44_100.0);

            // ループ点(スライス内インデックスへ変換)
            let loop_range = match ir.get_sample_modes() {
                rustysynth::LoopMode::NoLoop => None,
                _ => {
                    let ls = ir.get_sample_start_loop().max(0) as usize;
                    let le = ir.get_sample_end_loop().max(0) as usize;
                    if le > ls && ls >= start && le <= end {
                        Some(((ls - start) as f64, (le - start) as f64))
                    } else {
                        None
                    }
                }
            };
            let loop_until_release = matches!(
                ir.get_sample_modes(),
                rustysynth::LoopMode::LoopUntilNoteOff
            );

            // チューニング: プリセット側は加算オフセット
            let coarse = (ir.get_coarse_tune() + pr.get_coarse_tune()) as f32;
            let fine = (ir.get_fine_tune() + pr.get_fine_tune()) as f32 / 100.0;
            let root = ir.get_root_key() as f32 - coarse - fine;

            // 減衰(dB)。Polyphone / rustysynth に倣い 0.4 倍で適用
            let att_db = 0.4 * (ir.get_initial_attenuation() + pr.get_initial_attenuation());
            let gain = 10.0_f32.powf(-att_db.max(0.0) / 20.0);

            // 音量エンベロープ: 時間(timecents 由来の秒)は乗算、サスティン(dB)は加算
            let sustain_db = ir.get_sustain_volume_envelope() + pr.get_sustain_volume_envelope();
            let env = ZoneEnv {
                attack: (ir.get_attack_volume_envelope() * pr.get_attack_volume_envelope())
                    .clamp(0.001, 10.0),
                hold: (ir.get_hold_volume_envelope() * pr.get_hold_volume_envelope())
                    .clamp(0.0, 10.0),
                decay: (ir.get_decay_volume_envelope() * pr.get_decay_volume_envelope())
                    .clamp(0.005, 30.0),
                sustain: 10.0_f32.powf(-sustain_db.clamp(0.0, 144.0) / 20.0),
                release: (ir.get_release_volume_envelope() * pr.get_release_volume_envelope())
                    .clamp(0.01, 10.0),
            };

            // フィルタ・LFO・モジュレーションエンベロープ
            // (Hz / 秒 はプリセット側が倍率、セント / dB は加算)
            let modu = glaux_dsp::ZoneMod {
                cutoff_hz: (ir.get_initial_filter_cutoff_frequency()
                    * pr.get_initial_filter_cutoff_frequency())
                .clamp(20.0, 20_000.0),
                q_db: (ir.get_initial_filter_q() + pr.get_initial_filter_q()).clamp(0.0, 96.0),
                vib_to_pitch: (ir.get_vibrato_lfo_to_pitch() + pr.get_vibrato_lfo_to_pitch())
                    as f32,
                vib_freq: (ir.get_frequency_vibrato_lfo() * pr.get_frequency_vibrato_lfo())
                    .clamp(0.1, 100.0),
                vib_delay: (ir.get_delay_vibrato_lfo() * pr.get_delay_vibrato_lfo())
                    .clamp(0.0, 20.0),
                mod_to_pitch: (ir.get_modulation_lfo_to_pitch() + pr.get_modulation_lfo_to_pitch())
                    as f32,
                mod_to_filter: (ir.get_modulation_lfo_to_filter_cutoff_frequency()
                    + pr.get_modulation_lfo_to_filter_cutoff_frequency())
                    as f32,
                mod_freq: (ir.get_frequency_modulation_lfo() * pr.get_frequency_modulation_lfo())
                    .clamp(0.1, 100.0),
                mod_delay: (ir.get_delay_modulation_lfo() * pr.get_delay_modulation_lfo())
                    .clamp(0.0, 20.0),
                env_to_pitch: (ir.get_modulation_envelope_to_pitch()
                    + pr.get_modulation_envelope_to_pitch()) as f32,
                env_to_filter: (ir.get_modulation_envelope_to_filter_cutoff_frequency()
                    + pr.get_modulation_envelope_to_filter_cutoff_frequency())
                    as f32,
                env_delay: (ir.get_delay_modulation_envelope()
                    * pr.get_delay_modulation_envelope())
                .clamp(0.0, 20.0),
                env_attack: (ir.get_attack_modulation_envelope()
                    * pr.get_attack_modulation_envelope())
                .clamp(0.001, 20.0),
                env_hold: (ir.get_hold_modulation_envelope() * pr.get_hold_modulation_envelope())
                    .clamp(0.0, 20.0),
                env_decay: (ir.get_decay_modulation_envelope()
                    * pr.get_decay_modulation_envelope())
                .clamp(0.001, 30.0),
                env_sustain: (1.0
                    - (ir.get_sustain_modulation_envelope()
                        + pr.get_sustain_modulation_envelope())
                        / 100.0)
                    .clamp(0.0, 1.0),
                env_release: (ir.get_release_modulation_envelope()
                    * pr.get_release_modulation_envelope())
                .clamp(0.001, 20.0),
            };

            zones.push(Zone {
                key_lo: key_lo.clamp(0, 127) as u8,
                key_hi: key_hi.clamp(0, 127) as u8,
                vel_lo: vel_lo.clamp(0, 127) as u8,
                vel_hi: vel_hi.clamp(0, 127) as u8,
                data: Arc::new(SampleData {
                    frames,
                    sample_rate,
                    side: None,
                }),
                loop_range,
                loop_until_release,
                root,
                gain,
                env,
                modu,
            });
        }
    }

    if zones.is_empty() {
        None
    } else {
        Some(Arc::new(zones))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ---- テスト用の最小 SF2 バイナリを組み立てる ----

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(id);
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        if body.len() % 2 == 1 {
            v.push(0); // RIFF はワード境界
        }
        v
    }

    fn list(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut inner = Vec::new();
        inner.extend_from_slice(kind);
        inner.extend_from_slice(body);
        chunk(b"LIST", &inner)
    }

    fn name20(s: &str) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[..s.len()].copy_from_slice(s.as_bytes());
        out
    }

    fn gen(oper: u16, amount: u16) -> [u8; 4] {
        let mut v = [0u8; 4];
        v[..2].copy_from_slice(&oper.to_le_bytes());
        v[2..].copy_from_slice(&amount.to_le_bytes());
        v
    }

    fn minimal_sf2() -> Vec<u8> {
        // INFO
        let mut info = Vec::new();
        info.extend(chunk(b"ifil", &[2, 0, 1, 0]));
        info.extend(chunk(b"isng", b"EMU8000\0"));
        info.extend(chunk(b"INAM", b"GlauxTest\0"));

        // sdta: 200 サンプルのサイン波(16bit)
        let mut smpl = Vec::new();
        for i in 0..200i32 {
            let s = ((i as f32 * 0.3).sin() * 20_000.0) as i16;
            smpl.extend_from_slice(&s.to_le_bytes());
        }
        let sdta = chunk(b"smpl", &smpl);

        // pdta
        let mut phdr = Vec::new();
        {
            // プリセット "TestPreset" bank0 preset5 → bag 0
            phdr.extend_from_slice(&name20("TestPreset"));
            phdr.extend_from_slice(&5u16.to_le_bytes()); // preset
            phdr.extend_from_slice(&0u16.to_le_bytes()); // bank
            phdr.extend_from_slice(&0u16.to_le_bytes()); // bag index
            phdr.extend_from_slice(&[0; 12]); // library/genre/morph
                                              // 終端 EOP → bag 1
            phdr.extend_from_slice(&name20("EOP"));
            phdr.extend_from_slice(&0u16.to_le_bytes());
            phdr.extend_from_slice(&0u16.to_le_bytes());
            phdr.extend_from_slice(&1u16.to_le_bytes());
            phdr.extend_from_slice(&[0; 12]);
        }
        let mut pbag = Vec::new();
        pbag.extend_from_slice(&gen(0, 0).map(|_| 0)); // zone0: gen 0, mod 0
        pbag[0..2].copy_from_slice(&0u16.to_le_bytes());
        pbag[2..4].copy_from_slice(&0u16.to_le_bytes());
        // 終端: gen 1, mod 0
        pbag.extend_from_slice(&1u16.to_le_bytes());
        pbag.extend_from_slice(&0u16.to_le_bytes());
        let pmod = vec![0u8; 10]; // 終端のみ
        let mut pgen = Vec::new();
        pgen.extend_from_slice(&gen(41, 0)); // instrument = 0
        pgen.extend_from_slice(&gen(0, 0)); // 終端

        let mut inst = Vec::new();
        {
            inst.extend_from_slice(&name20("TestInst"));
            inst.extend_from_slice(&0u16.to_le_bytes()); // bag 0
            inst.extend_from_slice(&name20("EOI"));
            inst.extend_from_slice(&1u16.to_le_bytes()); // bag 1
        }
        let mut ibag = Vec::new();
        ibag.extend_from_slice(&0u16.to_le_bytes()); // zone0: gen 0
        ibag.extend_from_slice(&0u16.to_le_bytes());
        ibag.extend_from_slice(&3u16.to_le_bytes()); // 終端: gen 3
        ibag.extend_from_slice(&0u16.to_le_bytes());
        let imod = vec![0u8; 10];
        let mut igen = Vec::new();
        igen.extend_from_slice(&gen(43, 0x7f00)); // keyRange 0..127(lo | hi<<8)
        igen.extend_from_slice(&gen(54, 1)); // sampleModes = continuous loop
        igen.extend_from_slice(&gen(53, 0)); // sampleID = 0(ゾーン末尾)
        igen.extend_from_slice(&gen(0, 0)); // 終端

        let mut shdr = Vec::new();
        {
            shdr.extend_from_slice(&name20("TestSample"));
            shdr.extend_from_slice(&0u32.to_le_bytes()); // start
            shdr.extend_from_slice(&100u32.to_le_bytes()); // end
            shdr.extend_from_slice(&10u32.to_le_bytes()); // startLoop
            shdr.extend_from_slice(&90u32.to_le_bytes()); // endLoop
            shdr.extend_from_slice(&48_000u32.to_le_bytes()); // sampleRate
            shdr.push(60); // originalPitch
            shdr.push(0); // pitchCorrection(i8)
            shdr.extend_from_slice(&0u16.to_le_bytes()); // link
            shdr.extend_from_slice(&1u16.to_le_bytes()); // type = mono
                                                         // 終端 EOS
            shdr.extend_from_slice(&name20("EOS"));
            shdr.extend_from_slice(&[0; 26]);
        }

        let mut pdta = Vec::new();
        pdta.extend(chunk(b"phdr", &phdr));
        pdta.extend(chunk(b"pbag", &pbag));
        pdta.extend(chunk(b"pmod", &pmod));
        pdta.extend(chunk(b"pgen", &pgen));
        pdta.extend(chunk(b"inst", &inst));
        pdta.extend(chunk(b"ibag", &ibag));
        pdta.extend(chunk(b"imod", &imod));
        pdta.extend(chunk(b"igen", &igen));
        pdta.extend(chunk(b"shdr", &shdr));

        let mut body = Vec::new();
        body.extend_from_slice(b"sfbk");
        body.extend(list(b"INFO", &info));
        body.extend(list(b"sdta", &sdta));
        body.extend(list(b"pdta", &pdta));
        chunk(b"RIFF", &body)
    }

    #[test]
    fn parses_minimal_sf2_and_builds_zones() {
        let bytes = minimal_sf2();
        let font = SoundFont::new(&mut Cursor::new(&bytes)).expect("最小 SF2 が読めるはず");

        let presets = list_presets(&font);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].bank, 0);
        assert_eq!(presets[0].preset, 5);
        assert_eq!(presets[0].name, "TestPreset");

        let zones = build_zones(&font, 0, 5).expect("ゾーンが構築できるはず");
        assert_eq!(zones.len(), 1);
        let z = &zones[0];
        assert_eq!((z.key_lo, z.key_hi), (0, 127));
        assert_eq!(z.data.frames.len(), 100);
        assert!((z.data.sample_rate - 48_000.0).abs() < 1.0);
        assert!((z.root - 60.0).abs() < 0.01);
        assert_eq!(z.loop_range, Some((10.0, 90.0)));
        assert!(!z.loop_until_release);

        // 存在しないプリセットは None
        assert!(build_zones(&font, 0, 99).is_none());
    }
}
