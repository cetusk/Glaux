//! `GlauxPlayer`: Glaux の曲を鳴らし、「いま聞こえている位置」で拍・マーカー・ノートを知らせる Node。
//!
//! ```gdscript
//! @onready var music: GlauxPlayer = $GlauxPlayer
//!
//! func _ready():
//!     music.load_song("res://songs/stage1.glaux")
//!     music.watch_track("Kick")
//!     music.beat.connect(func(bar, beat): print(bar, ":", beat))
//!     music.note.connect(func(track, pitch, velocity): enemy.attack())
//!     music.play()
//!
//! func _process(_delta):
//!     enemy.phase = fmod(music.get_beat_position(), 1.0)
//! ```
//!
//! 「聞こえている位置」は、直前にミックスした位置 + そこからの経過時間 − 出力の遅れ(Godot の
//! `AudioServer` が報告する値)で求める。シグナルは `_process` で、前回から今回までに通り過ぎた
//! 出来事をまとめて出す(フレームの間隔ぶん遅れうるので、正確な位置が要るときは各シグナルに
//! 付いている時刻か `get_song_time()` を使う)。

use crate::song;
use crate::stream::{GlauxStream, MixClock};
use glaux_engine::render::{JumpRecord, Shared, NO_SEEK};
use glaux_engine::timeline::Timeline;
use godot::classes::{AudioServer, AudioStreamPlayer, INode, Node};
use godot::prelude::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

struct Loaded {
    timeline: Timeline,
    shared: Arc<Shared>,
    clock: Arc<MixClock>,
    sample_rate: f64,
    warnings: Vec<String>,
    /// 推定したキー(ノートが無い曲は None)
    key: Option<glaux_core::harmony::KeyEstimate>,
    /// キーのスケールのピッチクラス(キーが無ければ空)
    scale: Vec<u8>,
    /// 小節ごとのコード(時刻順)
    chords: Vec<ChordAt>,
    /// ここまでの「位置の飛び」(ループの折り返し・展開の切り替え)は処理済み(シークでも進める)
    jump_seen: u64,
    /// トラックごとの追加の音量(dB)と、フェードの途中なら(目標 dB, 1 秒あたりの変化 dB)
    track_db: Vec<f64>,
    track_fade: Vec<Option<(f64, f64)>>,
    /// 予約している展開の切り替え先(マーカーの名前)
    queued: Option<String>,
}

/// 小節ごとのコード。
struct ChordAt {
    sec: f64,
    bar: i64,
    name: String,
    /// 構成音のピッチクラス(ルートが先頭。"N.C." は空)
    pcs: Vec<u8>,
}

#[derive(GodotClass)]
#[class(init, base=Node)]
pub struct GlauxPlayer {
    base: Base<Node>,
    /// 出力するオーディオバス
    #[export]
    #[init(val = StringName::from("Master"))]
    bus: StringName,
    /// 音量(dB)
    #[export]
    volume_db: f32,
    /// 手動の遅れ補正(ms)。正の値でシグナル・位置を遅らせる
    /// (音より画面が早いと感じたら増やす。Bluetooth のヘッドホンなど)
    #[export]
    latency_offset_ms: f64,
    /// `play_note_at` の時刻の基準にする曲(BGM)の GlauxPlayer。効果音用の GlauxPlayer に BGM の
    /// GlauxPlayer を設定すると、BGM の時刻(`get_next_beat_time()` など)をそのまま渡せる。未設定なら自分の曲の時刻
    #[var]
    sync_to: Option<Gd<GlauxPlayer>>,
    song: Option<Loaded>,
    audio: Option<Gd<AudioStreamPlayer>>,
    watched: Vec<String>,
    /// シグナルを出し終えた位置(秒、聞こえている位置)
    emitted_to: f64,
    /// 返した位置の最大(推定のぶれで逆戻りしないように)
    last_time: f64,
    /// 止まっているときに返す位置(秒)
    held_time: f64,
    /// 再生中(一時停止・停止・曲の終わりで false)
    playing: bool,
    finished_sent: bool,
    last_error: GString,
}

#[godot_api]
impl INode for GlauxPlayer {
    fn ready(&mut self) {
        self.ensure_audio();
    }

    /// ツリーから外れるとき(シーン切り替え・終了)に音声スレッドの再生を止め、再生オブジェクトを手放す
    fn exit_tree(&mut self) {
        if let Some(a) = self.audio.as_mut() {
            a.stop();
        }
    }

    fn process(&mut self, delta: f64) {
        if self.song.is_none() {
            return;
        }
        if let Some(a) = self.audio.as_mut() {
            a.set_volume_db(self.volume_db);
        }
        self.step_fades(delta);
        if !self.playing {
            return;
        }
        // 位置の飛び(ループの折り返し・展開の切り替え)が聞こえたら、飛ぶ前の所までの出来事を出してから
        // 飛んだ先へ移る
        if let Some(rec) = self.heard_jump() {
            let sr = self.song.as_ref().map_or(48_000.0, |s| s.sample_rate);
            let (from, to) = (rec.from as f64 / sr, rec.to as f64 / sr);
            // 飛ぶ位置ちょうどの音は鳴らない(そのサンプルから飛んだ先を鳴らす)ので、その手前まで
            self.emit_until(from - 1e-6);
            if let Some(s) = self.song.as_mut() {
                s.jump_seen = rec.seq;
                if s.queued.is_some() && s.shared.jump_at.load(Ordering::Acquire) == NO_SEEK {
                    s.queued = None;
                }
            }
            self.reset_position(to);
            self.signals().jumped().emit(from, to);
        }
        let now = self.song_time();
        self.emit_until(now);
        // 曲の終わり(レンダラが余韻まで鳴らし切って止まった)
        let stopped = self
            .song
            .as_ref()
            .is_some_and(|s| !s.shared.playing.load(Ordering::Acquire));
        if stopped && !self.finished_sent {
            self.finished_sent = true;
            self.playing = false;
            self.signals().song_finished().emit();
        }
    }
}

#[godot_api]
impl GlauxPlayer {
    /// 拍の頭を通り過ぎた(`bar` は 1 始まりの小節、`beat` は 1 始まりの小節内の拍、`time` はその拍の秒)
    #[signal]
    fn beat(bar: i64, beat: i64, time: f64);

    /// マーカー(Glaux で打った「サビ」などの曲の構成)を通り過ぎた
    #[signal]
    fn section(name: GString, time: f64);

    /// 監視しているトラック(`watch_track`)のノートが鳴った
    #[signal]
    fn note(track: GString, pitch: i64, velocity: i64, time: f64, duration: f64);

    /// 曲が最後まで鳴り終わった
    #[signal]
    fn song_finished();

    /// 再生位置が飛んだのが聞こえた(ループの折り返し・`queue_section` の切り替え)。`from` から `to` 秒へ
    #[signal]
    fn jumped(from: f64, to: f64);

    /// 曲(`.glaux` フォルダ。例 `res://songs/stage1.glaux`)を読み込む。失敗したら false
    /// (理由は `get_last_error()`)。読み込むと停止した状態になる
    #[func]
    fn load_song(&mut self, path: GString) -> bool {
        self.ensure_audio();
        let sample_rate = AudioServer::singleton().get_mix_rate() as f64;
        match song::load(&path.to_string(), sample_rate) {
            Ok(s) => {
                for w in &s.warnings {
                    godot_warn!("Glaux: {w}");
                }
                let shared = Arc::new(Shared::new(s.data));
                let clock = Arc::new(MixClock::default());
                let mut stream = GlauxStream::new_gd();
                {
                    let mut st = stream.bind_mut();
                    st.shared = Some(shared.clone());
                    st.clock = clock.clone();
                }
                if let Some(a) = self.audio.as_mut() {
                    a.stop();
                    a.set_stream(&stream);
                    a.set_bus(&self.bus);
                    a.set_volume_db(self.volume_db);
                    a.play();
                }
                let scale = s
                    .harmony
                    .key
                    .as_ref()
                    .map(|k| glaux_core::harmony::scale_pitch_classes(k.tonic, k.mode))
                    .unwrap_or_default();
                let chords = s
                    .harmony
                    .chords
                    .iter()
                    .map(|c| ChordAt {
                        sec: s.timeline.tick_to_sec(c.tick as f64),
                        bar: c.bar as i64,
                        name: c.chord.clone(),
                        pcs: glaux_core::harmony::chord_pitch_classes(&c.chord).unwrap_or_default(),
                    })
                    .collect();
                self.song = Some(Loaded {
                    timeline: s.timeline,
                    shared,
                    clock,
                    sample_rate,
                    warnings: s.warnings,
                    key: s.harmony.key,
                    scale,
                    chords,
                    jump_seen: 0,
                    track_db: Vec::new(),
                    track_fade: Vec::new(),
                    queued: None,
                });
                if let Some(l) = self.song.as_mut() {
                    let n = l.timeline.tracks().len();
                    l.track_db = vec![0.0; n];
                    l.track_fade = vec![None; n];
                }
                self.playing = false;
                self.finished_sent = false;
                self.reset_position(0.0);
                self.last_error = GString::new();
                true
            }
            Err(e) => {
                godot_error!("Glaux: {e}");
                self.last_error = GString::from(e.as_str());
                false
            }
        }
    }

    /// 直前の失敗の理由
    #[func]
    fn get_last_error(&self) -> GString {
        self.last_error.clone()
    }

    /// 読み込みで気づいた注意(CLAP の音源はゲームでは鳴らない、など)
    #[func]
    fn get_warnings(&self) -> PackedStringArray {
        self.song
            .as_ref()
            .map(|s| s.warnings.iter().map(GString::from).collect())
            .unwrap_or_default()
    }

    /// 曲頭から再生する
    #[func]
    fn play(&mut self) {
        self.play_from(0.0);
    }

    /// `sec` 秒の位置から再生する
    #[func]
    fn play_from(&mut self, sec: f64) {
        let Some(s) = self.song.as_ref() else {
            return;
        };
        let sample = (sec.max(0.0) * s.sample_rate) as u64;
        s.shared.jump_at.store(NO_SEEK, Ordering::Release);
        s.shared.seek.store(sample, Ordering::Release);
        s.shared.pos.store(sample, Ordering::Release);
        s.clock.mix_start.store(sample, Ordering::Release);
        s.shared.playing.store(true, Ordering::Release);
        self.forget_jumps();
        self.reset_position(sec.max(0.0));
        self.playing = true;
        self.finished_sent = false;
    }

    /// 止める(位置は曲頭に戻る)
    #[func]
    fn stop(&mut self) {
        if let Some(s) = self.song.as_ref() {
            s.shared.playing.store(false, Ordering::Release);
            s.shared.jump_at.store(NO_SEEK, Ordering::Release);
            s.shared.seek.store(0, Ordering::Release);
        }
        self.forget_jumps();
        self.playing = false;
        self.reset_position(0.0);
    }

    /// 一時停止(`resume` で続きから)
    #[func]
    fn pause(&mut self) {
        self.held_time = self.song_time().max(0.0);
        if let Some(s) = self.song.as_ref() {
            s.shared.playing.store(false, Ordering::Release);
        }
        self.playing = false;
    }

    /// 一時停止した位置から再開する
    #[func]
    fn resume(&mut self) {
        let Some(s) = self.song.as_ref() else {
            return;
        };
        let pos = s.shared.pos.load(Ordering::Acquire);
        s.clock.mix_start.store(pos, Ordering::Release);
        s.shared.playing.store(true, Ordering::Release);
        self.reset_position(pos as f64 / s.sample_rate);
        self.playing = true;
        self.finished_sent = false;
    }

    /// 再生位置を `sec` 秒へ移す(再生中ならそのまま続く。飛ばした区間のシグナルは出さない)
    #[func]
    fn seek(&mut self, sec: f64) {
        let Some(s) = self.song.as_ref() else {
            return;
        };
        let sample = (sec.max(0.0) * s.sample_rate) as u64;
        s.shared.jump_at.store(NO_SEEK, Ordering::Release);
        s.shared.seek.store(sample, Ordering::Release);
        s.shared.pos.store(sample, Ordering::Release);
        s.clock.mix_start.store(sample, Ordering::Release);
        self.forget_jumps();
        self.reset_position(sec.max(0.0));
    }

    #[func]
    fn is_playing(&self) -> bool {
        self.playing
    }

    /// いま聞こえている位置(秒)。出力の遅れを補正済み。再生中は単調に増える
    #[func]
    fn get_song_time(&mut self) -> f64 {
        self.song_time()
    }

    /// いま聞こえている位置が曲頭から何拍目か(小数。例 12.37 = 13 拍目の 37% の所)。
    /// `fmod(x, 1.0)` で拍の中の位置(0〜1)になる
    #[func]
    fn get_beat_position(&mut self) -> f64 {
        let t = self.song_time();
        self.song
            .as_ref()
            .map_or(0.0, |s| s.timeline.beat_position(t))
    }

    /// いま聞こえている位置の小節(1 始まり)
    #[func]
    fn get_bar(&mut self) -> i64 {
        let t = self.song_time();
        self.song
            .as_ref()
            .map_or(0, |s| s.timeline.beat_at(t).bar as i64)
    }

    /// いま聞こえている位置の小節内の拍(1 始まり)
    #[func]
    fn get_beat(&mut self) -> i64 {
        let t = self.song_time();
        self.song
            .as_ref()
            .map_or(0, |s| s.timeline.beat_at(t).beat as i64)
    }

    /// いま聞こえている位置のテンポ(BPM)
    #[func]
    fn get_bpm(&mut self) -> f64 {
        let t = self.song_time();
        self.song.as_ref().map_or(0.0, |s| s.timeline.bpm_at(t))
    }

    /// 次の拍の頭の時刻(秒)。無ければ -1
    #[func]
    fn get_next_beat_time(&mut self) -> f64 {
        let t = self.song_time();
        self.song
            .as_ref()
            .and_then(|s| s.timeline.next_beat(t))
            .map_or(-1.0, |b| b.sec)
    }

    /// いま聞こえている位置のマーカー名(無ければ空)
    #[func]
    fn get_section(&mut self) -> GString {
        let t = self.song_time();
        self.song
            .as_ref()
            .and_then(|s| s.timeline.section_at(t))
            .map_or_else(GString::new, |m| m.name.as_str().into())
    }

    /// 曲の長さ(秒。最後のクリップ・マーカーまで。余韻は含まない)
    #[func]
    fn get_length(&self) -> f64 {
        self.song.as_ref().map_or(0.0, |s| s.timeline.end_sec())
    }

    /// トラック名の一覧
    #[func]
    fn get_track_names(&self) -> PackedStringArray {
        self.song
            .as_ref()
            .map(|s| {
                s.timeline
                    .tracks()
                    .iter()
                    .map(|t| GString::from(t.name.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// このトラック(名前か ID)のノートが鳴るたびに `note` シグナルを出す
    #[func]
    fn watch_track(&mut self, track: GString) {
        let name = track.to_string();
        if let Some(s) = self.song.as_ref() {
            if s.timeline.track(&name).is_none() {
                godot_warn!("Glaux: トラック「{name}」が見つかりません");
            }
        }
        if !self.watched.contains(&name) {
            self.watched.push(name);
        }
    }

    #[func]
    fn unwatch_track(&mut self, track: GString) {
        let name = track.to_string();
        self.watched.retain(|w| *w != name);
    }

    /// トラックの `[from, to)` 秒に始まるノート(先読み用)。
    /// 各要素は {time, duration, pitch, velocity}
    #[func]
    fn get_notes(&self, track: GString, from: f64, to: f64) -> VarArray {
        let mut out = VarArray::new();
        let Some(s) = self.song.as_ref() else {
            return out;
        };
        let Some(t) = s.timeline.track(&track.to_string()) else {
            return out;
        };
        for n in t.notes.iter().filter(|n| n.sec >= from && n.sec < to) {
            out.push(
                &dict_of(&[
                    ("time", (n.sec).to_variant()),
                    ("duration", (n.dur_sec).to_variant()),
                    ("pitch", (n.pitch as i64).to_variant()),
                    ("velocity", (n.vel as i64).to_variant()),
                ])
                .to_variant(),
            );
        }
        out
    }

    /// `[from, to)` 秒に頭がある拍(先読み用)。各要素は {time, bar, beat, beats_in_bar}
    #[func]
    fn get_beats(&self, from: f64, to: f64) -> VarArray {
        let mut out = VarArray::new();
        let Some(s) = self.song.as_ref() else {
            return out;
        };
        for b in s
            .timeline
            .beats()
            .iter()
            .filter(|b| b.sec >= from && b.sec < to)
        {
            out.push(
                &dict_of(&[
                    ("time", (b.sec).to_variant()),
                    ("bar", (b.bar as i64).to_variant()),
                    ("beat", (b.beat as i64).to_variant()),
                    ("beats_in_bar", (b.beats_in_bar as i64).to_variant()),
                ])
                .to_variant(),
            );
        }
        out
    }

    /// マーカーの一覧。各要素は {time, name}
    #[func]
    fn get_sections(&self) -> VarArray {
        let mut out = VarArray::new();
        let Some(s) = self.song.as_ref() else {
            return out;
        };
        for m in s.timeline.sections() {
            out.push(
                &dict_of(&[
                    ("time", (m.sec).to_variant()),
                    ("name", (m.name.as_str()).to_variant()),
                ])
                .to_variant(),
            );
        }
        out
    }

    /// 小節・拍(1 始まり)の頭の時刻(秒)。拍が曲の範囲外なら -1
    #[func]
    fn get_time_of(&self, bar: i64, beat: i64) -> f64 {
        self.song
            .as_ref()
            .and_then(|s| {
                s.timeline
                    .beats()
                    .iter()
                    .find(|b| b.bar as i64 == bar && b.beat as i64 == beat)
            })
            .map_or(-1.0, |b| b.sec)
    }

    // ---- 和声(効果音の音程を曲に合わせる) ----

    /// 曲のキー(ノートからの推定)。{name: "A minor", tonic: 9(C=0 のピッチクラス), mode: "minor", confidence}。
    /// ノートが無い曲は空の辞書
    #[func]
    fn get_key(&self) -> VarDictionary {
        match self.song.as_ref().and_then(|s| s.key.as_ref()) {
            Some(k) => dict_of(&[
                ("name", k.name.as_str().to_variant()),
                ("tonic", (k.tonic as i64).to_variant()),
                ("mode", k.mode.to_variant()),
                ("confidence", k.confidence.to_variant()),
            ]),
            None => VarDictionary::new(),
        }
    }

    /// 小節ごとのコードの一覧。各要素は {time, bar, chord}(chord は "Am" / "G7" / "N.C." など)
    #[func]
    fn get_chords(&self) -> VarArray {
        let mut out = VarArray::new();
        if let Some(s) = self.song.as_ref() {
            for c in &s.chords {
                out.push(
                    &dict_of(&[
                        ("time", c.sec.to_variant()),
                        ("bar", c.bar.to_variant()),
                        ("chord", c.name.as_str().to_variant()),
                    ])
                    .to_variant(),
                );
            }
        }
        out
    }

    /// `time` 秒(曲の時刻。ふつうは `get_song_time()`)に鳴っているコードの名前。無ければ空
    #[func]
    fn get_chord_at(&self, time: f64) -> GString {
        self.chord_at(time)
            .map_or_else(GString::new, |c| c.name.as_str().into())
    }

    /// `time` 秒のコードの構成音のピッチクラス(C=0..B=11。ルートが先頭)。"N.C." なら空
    #[func]
    fn get_chord_tones(&self, time: f64) -> PackedInt32Array {
        self.chord_at(time)
            .map(|c| c.pcs.iter().map(|&p| p as i32).collect())
            .unwrap_or_default()
    }

    /// キーのスケールのピッチクラス(トニックから 7 音。短調はナチュラルマイナー)
    #[func]
    fn get_scale_pitch_classes(&self) -> PackedInt32Array {
        self.song
            .as_ref()
            .map(|s| s.scale.iter().map(|&p| p as i32).collect())
            .unwrap_or_default()
    }

    /// `pitch`(MIDI 番号)を、キーのスケールでいちばん近い音に寄せる(同じ近さなら上)
    #[func]
    fn snap_to_scale(&self, pitch: i64) -> i64 {
        let scale = self
            .song
            .as_ref()
            .map(|s| s.scale.as_slice())
            .unwrap_or(&[]);
        glaux_core::harmony::snap_to_pitch_classes(pitch as i32, scale) as i64
    }

    /// `pitch` を、`time` 秒のコードの構成音でいちばん近い音に寄せる(同じ近さなら上)。
    /// コードが無い(N.C.)ときはキーのスケールに寄せる
    #[func]
    fn snap_to_chord(&self, pitch: i64, time: f64) -> i64 {
        match self.chord_at(time).filter(|c| !c.pcs.is_empty()) {
            Some(c) => glaux_core::harmony::snap_to_pitch_classes(pitch as i32, &c.pcs) as i64,
            None => self.snap_to_scale(pitch),
        }
    }

    /// `time` 秒のコードの構成音を、`base`(MIDI 番号。既定 60 = C4)以上で下から数えた `index` 番目の音
    /// (使い切ったら 1 オクターブ上へ。負なら下へ)。コンボの段階で構成音を上っていく、などに使う。
    /// コードが無いときはキーのスケールで数える
    #[func]
    fn get_chord_note(&self, time: f64, index: i64, #[opt(default = 60)] base: i64) -> i64 {
        match self.chord_at(time).filter(|c| !c.pcs.is_empty()) {
            Some(c) => {
                glaux_core::harmony::nth_pitch_from(&c.pcs, base as i32, index as i32) as i64
            }
            None => self.get_scale_note(index, base),
        }
    }

    /// キーのスケールの音を、`base`(既定 60)以上で下から数えた `index` 番目の音(負なら下へ)
    #[func]
    fn get_scale_note(&self, index: i64, #[opt(default = 60)] base: i64) -> i64 {
        let scale = self
            .song
            .as_ref()
            .map(|s| s.scale.as_slice())
            .unwrap_or(&[]);
        glaux_core::harmony::nth_pitch_from(scale, base as i32, index as i32) as i64
    }

    // ---- 1 音ずつ鳴らす(効果音) ----

    /// トラック(名前か ID)の音源とエフェクトで 1 音をすぐ鳴らす。曲を再生していなくても鳴る。
    /// `velocity` は 1〜127、`duration` は鍵盤を押している秒数(その後はリリースで消える)。
    /// 鳴らせないとき(トラックが無い、CLAP の音源)は false
    #[func]
    fn play_note(
        &mut self,
        track: GString,
        pitch: i64,
        #[opt(default = 100)] velocity: i64,
        #[opt(default = 0.2)] duration: f64,
    ) -> bool {
        self.schedule_note(&track.to_string(), pitch, velocity, duration, None)
    }

    /// `play_note` を時刻指定で鳴らす。`time` は `sync_to` の曲(未設定なら自分の曲)の時刻(秒)で、
    /// その時刻に**聞こえる**ように出力の遅れを見込んで鳴らす(例: `bgm.get_next_beat_time()`)。
    /// 過ぎた時刻ならすぐ鳴らす
    #[func]
    fn play_note_at(
        &mut self,
        track: GString,
        pitch: i64,
        time: f64,
        #[opt(default = 100)] velocity: i64,
        #[opt(default = 0.2)] duration: f64,
    ) -> bool {
        // 基準の曲の「いま聞こえている位置」(手動の遅れ補正を除いた本当の位置)
        let me = self.base().instance_id();
        let now = match self.sync_to.clone() {
            Some(mut other) if other.instance_id() != me => other.bind_mut().audible_time(),
            _ => self.audible_time(),
        };
        self.schedule_note(
            &track.to_string(),
            pitch,
            velocity,
            duration,
            Some(time - now),
        )
    }

    /// `play_note` / `play_note_at` で鳴らした音(と MIDI キーボードの音)をすべて離す
    #[func]
    fn release_notes(&mut self) {
        if let Some(s) = self.song.as_ref() {
            s.shared.live.push(glaux_engine::midi::LiveEvent::AllOff);
        }
    }

    // ---- ループと展開の切り替え(ゲームの場面に合わせて曲を組み替える) ----

    /// `from`〜`to` 秒を繰り返す(終わりに来たら頭へ。ループ中は曲の終わりでも止まらない)
    #[func]
    fn set_loop(&mut self, from: f64, to: f64) {
        let Some(s) = self.song.as_ref() else {
            return;
        };
        let (a, b) = (
            (from.max(0.0) * s.sample_rate) as u64,
            (to.max(0.0) * s.sample_rate) as u64,
        );
        if b <= a {
            godot_warn!("Glaux: ループの終わりは始まりより後にしてください");
            return;
        }
        // 前の区間と混ざって終わり <= 始まりにならないよう、終わりを後から書く
        s.shared.loop_end.store(0, Ordering::Release);
        s.shared.loop_start.store(a, Ordering::Release);
        s.shared.loop_end.store(b, Ordering::Release);
    }

    /// マーカー `name` の区間(次のマーカーか曲の終わりまで)を繰り返す。見つからなければ false
    #[func]
    fn set_loop_section(&mut self, name: GString) -> bool {
        let Some(s) = self.song.as_ref() else {
            return false;
        };
        let Some((a, b)) = Self::section_range(s, &name.to_string()) else {
            godot_warn!("Glaux: マーカー「{name}」が見つかりません");
            return false;
        };
        s.shared.loop_end.store(0, Ordering::Release);
        s.shared.loop_start.store(a, Ordering::Release);
        s.shared.loop_end.store(b, Ordering::Release);
        true
    }

    /// ループをやめる(最後まで鳴らして止まる)
    #[func]
    fn clear_loop(&mut self) {
        if let Some(s) = self.song.as_ref() {
            s.shared.loop_end.store(0, Ordering::Release);
            s.shared.loop_start.store(0, Ordering::Release);
        }
    }

    #[func]
    fn is_looping(&self) -> bool {
        self.song.as_ref().is_some_and(|s| {
            s.shared.loop_end.load(Ordering::Acquire) > s.shared.loop_start.load(Ordering::Acquire)
        })
    }

    /// マーカー `name` の区間へ、拍子に合わせて切り替える予約をする(ゲームの場面の変化に合わせて曲の展開を変える)。
    /// `at` は切り替える所: "beat"(次の拍)/ "bar"(次の小節の頭、既定)/ "section"(今の区間の終わり。ループ中は
    /// ループの終わり)。`loop_it` が true なら、切り替えた先の区間を繰り返す。切り替わったのが聞こえると
    /// `jumped` と `section` のシグナルが出る。予約は 1 つだけ(新しい予約で置き換わる)。失敗したら false
    #[func]
    fn queue_section(
        &mut self,
        name: GString,
        #[opt(default = "bar")] at: GString,
        #[opt(default = false)] loop_it: bool,
    ) -> bool {
        let Some(s) = self.song.as_ref() else {
            return false;
        };
        let name = name.to_string();
        let Some((to, to_end)) = Self::section_range(s, &name) else {
            godot_warn!("Glaux: マーカー「{name}」が見つかりません");
            return false;
        };
        let sr = s.sample_rate;
        // 描き出しはもう少し先まで進んでいるので、その先の区切りを選ぶ
        let pos = s.shared.pos.load(Ordering::Acquire);
        let ls = s.shared.loop_start.load(Ordering::Acquire);
        let le = s.shared.loop_end.load(Ordering::Acquire);
        let looping = le > ls;
        let beats = s.timeline.beats();
        let next_beat = |bar_only: bool| {
            beats
                .iter()
                .map(|b| ((b.sec * sr) as u64, b.beat))
                .find(|&(x, beat)| x > pos && (!bar_only || beat == 1))
                .map(|(x, _)| x)
        };
        let boundary = match at.to_string().as_str() {
            "beat" => next_beat(false),
            "bar" => next_beat(true),
            "section" => {
                let next_mark = s
                    .timeline
                    .sections()
                    .iter()
                    .map(|m| (m.sec * sr) as u64)
                    .find(|&x| x > pos);
                Some(next_mark.unwrap_or_else(|| Self::song_end_sample(s)))
            }
            other => {
                godot_warn!("Glaux: at は \"beat\" / \"bar\" / \"section\"(got: {other})");
                return false;
            }
        };
        // ループ中は、ループの終わりより先の区切りには届かないので、ループの終わりで切り替える
        let mut jump_at = boundary.unwrap_or_else(|| Self::song_end_sample(s).max(pos + 1));
        if looping && pos < le && jump_at > le {
            jump_at = le;
        }
        // 切り替えた先のループ: 指定があればその区間、今のループの外へ出るならループを外す、中ならそのまま
        let (jls, jle) = if loop_it {
            (to, to_end)
        } else if looping && (to < ls || to >= le) {
            (0, 0)
        } else {
            (0, NO_SEEK)
        };
        s.shared.jump_at.store(NO_SEEK, Ordering::Release);
        s.shared.jump_to.store(to, Ordering::Release);
        s.shared.jump_loop_start.store(jls, Ordering::Release);
        s.shared.jump_loop_end.store(jle, Ordering::Release);
        s.shared.jump_at.store(jump_at, Ordering::Release);
        if let Some(s) = self.song.as_mut() {
            s.queued = Some(name);
        }
        true
    }

    /// 切り替えの予約を取り消す
    #[func]
    fn cancel_queued_section(&mut self) {
        if let Some(s) = self.song.as_mut() {
            s.shared.jump_at.store(NO_SEEK, Ordering::Release);
            s.queued = None;
        }
    }

    /// 予約している切り替え先のマーカー(無ければ空)
    #[func]
    fn get_queued_section(&self) -> GString {
        self.song
            .as_ref()
            .and_then(|s| s.queued.as_deref())
            .map(GString::from)
            .unwrap_or_default()
    }

    // ---- トラックの音量(場面に合わせて楽器を足し引きする。曲のファイルは変えない) ----

    /// トラック(名前か ID)の音量を `db` にする(曲の中の音量に足す。0 で元のまま、-80 以下で無音)。
    /// `fade` 秒をかけて変える(0 ならすぐ)。見つからなければ false
    #[func]
    fn set_track_volume_db(
        &mut self,
        track: GString,
        db: f64,
        #[opt(default = 0.0)] fade: f64,
    ) -> bool {
        let Some(i) = self.track_index(&track.to_string()) else {
            godot_warn!("Glaux: トラック「{track}」が見つかりません");
            return false;
        };
        let Some(s) = self.song.as_mut() else {
            return false;
        };
        let db = db.clamp(-80.0, 24.0);
        if fade > 0.0 {
            let rate = ((db - s.track_db[i]).abs() / fade).max(1e-6);
            s.track_fade[i] = Some((db, rate));
        } else {
            s.track_fade[i] = None;
            s.track_db[i] = db;
            Self::apply_track_db(s, i);
        }
        true
    }

    /// トラックの今の追加の音量(dB。フェード中は途中の値)
    #[func]
    fn get_track_volume_db(&self, track: GString) -> f64 {
        self.track_index(&track.to_string())
            .and_then(|i| self.song.as_ref()?.track_db.get(i).copied())
            .unwrap_or(0.0)
    }

    /// 音声スレッドが動いているか(デバッグ用。ミックスした回数)
    #[func]
    fn get_mix_count(&self) -> i64 {
        self.song
            .as_ref()
            .map_or(0, |s| s.clock.mixes.load(Ordering::Relaxed) as i64)
    }
}

/// (キー, 値)の並びから辞書を作る
fn dict_of(items: &[(&str, Variant)]) -> VarDictionary {
    let mut d = VarDictionary::new();
    for (k, v) in items {
        d.set(&k.to_variant(), v);
    }
    d
}

impl GlauxPlayer {
    /// 自分の子に AudioStreamPlayer を 1 つ持つ(Glaux のストリームを流す)
    fn ensure_audio(&mut self) {
        if self.audio.is_some() {
            return;
        }
        let mut a = AudioStreamPlayer::new_alloc();
        a.set_name("GlauxAudio");
        a.set_bus(&self.bus);
        self.base_mut().add_child(&a);
        self.audio = Some(a);
    }

    /// 再生開始・シークの位置へ戻す。その位置ちょうどの出来事(曲頭の拍・マーカー等)も知らせるよう、
    /// 知らせ終えた位置はわずかに手前にする。再生直後は出力の遅れぶん「まだ聞こえていない」
    /// (位置が開始点より手前)になり、音が届いてから最初の出来事を知らせる
    fn reset_position(&mut self, sec: f64) {
        self.emitted_to = sec - 1e-6;
        self.last_time = f64::NEG_INFINITY;
        self.held_time = sec;
    }

    /// `time` 秒に鳴っているコード
    fn chord_at(&self, time: f64) -> Option<&ChordAt> {
        let s = self.song.as_ref()?;
        let i = s.chords.partition_point(|c| c.sec <= time);
        i.checked_sub(1).map(|i| &s.chords[i])
    }

    /// 手動の遅れ補正を除いた「いま聞こえている位置」(秒)。時刻指定の音の位置合わせに使う
    fn audible_time(&mut self) -> f64 {
        self.song_time() + self.latency_offset_ms / 1000.0
    }

    /// 1 音を鳴らす予約を積む。`delay` は「いまから何秒後に聞こえるか」(None ならすぐ)
    fn schedule_note(
        &mut self,
        track: &str,
        pitch: i64,
        velocity: i64,
        duration: f64,
        delay: Option<f64>,
    ) -> bool {
        let Some(s) = self.song.as_ref() else {
            godot_warn!("Glaux: 曲を読み込んでから鳴らしてください");
            return false;
        };
        let tracks = s.timeline.tracks();
        let Some(index) = tracks
            .iter()
            .position(|t| t.name == track)
            .or_else(|| tracks.iter().position(|t| t.id == track))
        else {
            godot_warn!("Glaux: トラック「{track}」が見つかりません");
            return false;
        };
        let at = match delay {
            None => 0,
            Some(d) => {
                // 時計 s のサンプルは、直前のミックスの頭(mix_clock)から (s - mix_clock)/sr 後に作られ、
                // 出力の遅れの後に聞こえる。いまから d 秒後に聞こえるサンプルを求める
                let server = AudioServer::singleton();
                let mix_clock = s.clock.mix_clock.load(Ordering::Acquire) as f64;
                let x = mix_clock
                    + (server.get_time_since_last_mix() + d - server.get_output_latency())
                        * s.sample_rate;
                x.max(0.0) as u64
            }
        };
        let note = glaux_engine::midi::TimedNote {
            at,
            track: index as u16,
            pitch: pitch.clamp(0, 127) as u8,
            vel: velocity.clamp(1, 127) as u8,
            dur: (duration.max(0.001) * s.sample_rate).min(u32::MAX as f64) as u32,
        };
        if !s.shared.notes.push(note) {
            godot_warn!("Glaux: 鳴らす予約が多すぎます(一度に 512 まで)");
            return false;
        }
        true
    }

    /// いま聞こえている位置(秒)
    fn song_time(&mut self) -> f64 {
        if self.song.is_none() {
            return 0.0;
        }
        if !self.playing {
            return self.held_time;
        }
        let (t, jumped) = self.raw_time();
        if jumped {
            // 飛んだのが聞こえた直後(`process` が知らせるまで)は、戻る向きでもそのまま返す
            self.held_time = t;
            return t;
        }
        // 推定のぶれで逆戻りしない(シーク・再生開始・位置の飛びでだけ戻る)
        self.last_time = self.last_time.max(t);
        self.held_time = self.last_time;
        self.last_time
    }

    /// 出力の遅れを引いた「いま聞こえている位置」(秒)と、まだ処理していない位置の飛びがもう聞こえているか。
    /// レンダラの時計で「いま聞こえているサンプル」を求め、その間に位置が飛んでいれば飛ぶ前の位置から数える
    fn raw_time(&self) -> (f64, bool) {
        let Some(s) = self.song.as_ref() else {
            return (0.0, false);
        };
        let server = AudioServer::singleton();
        let sr = s.sample_rate;
        let mix_start = s.clock.mix_start.load(Ordering::Acquire) as f64;
        let mix_clock = s.clock.mix_clock.load(Ordering::Acquire) as f64;
        let heard = mix_clock
            + (server.get_time_since_last_mix()
                - server.get_output_latency()
                - self.latency_offset_ms / 1000.0)
                * sr;
        let rec = s.shared.last_jump.read();
        let unseen = rec.seq > s.jump_seen;
        let pos = if unseen && heard < rec.clock as f64 {
            // 飛ぶ前の音がまだ聞こえている
            rec.from as f64 - (rec.clock as f64 - heard)
        } else {
            // 出力の遅れが小さい環境では、推定がまだ描き出していない所まで伸びることがある。
            // この先で位置が飛ぶ(ループの終わり・予約した切り替え)なら、その手前で止める
            // (飛ぶ位置の先の拍・ノートは鳴らないので知らせない)
            let p = mix_start + (heard - mix_clock);
            let ls = s.shared.loop_start.load(Ordering::Acquire);
            let le = s.shared.loop_end.load(Ordering::Acquire);
            let ja = s.shared.jump_at.load(Ordering::Acquire);
            let mut limit = f64::INFINITY;
            if le > ls && le as f64 > mix_start {
                limit = limit.min(le as f64);
            }
            if ja != NO_SEEK && ja as f64 > mix_start {
                limit = limit.min(ja as f64);
            }
            p.min(limit - 1.0)
        };
        (pos / sr, unseen && heard >= rec.clock as f64)
    }

    /// まだ処理していない位置の飛びが、もう聞こえていれば返す
    fn heard_jump(&self) -> Option<JumpRecord> {
        let s = self.song.as_ref()?;
        let (_, heard) = self.raw_time();
        heard.then(|| s.shared.last_jump.read())
    }

    /// これまでの位置の飛びは処理済みにする(シーク・再生開始の後は古い飛びで位置を求めない)
    fn forget_jumps(&mut self) {
        if let Some(s) = self.song.as_mut() {
            s.jump_seen = s.shared.last_jump.read().seq;
            s.queued = None;
        }
    }

    /// トラックの追加の音量をレンダラへ渡す
    fn apply_track_db(s: &Loaded, index: usize) {
        if let (Some(db), Some(g)) = (s.track_db.get(index), s.shared.live_gain.get(index)) {
            let amp = if *db <= -80.0 {
                0.0
            } else {
                10f64.powf(*db / 20.0) as f32
            };
            g.store(amp.to_bits(), Ordering::Relaxed);
        }
    }

    /// フェード中のトラックの音量を 1 フレーム分進める
    fn step_fades(&mut self, delta: f64) {
        let Some(s) = self.song.as_mut() else {
            return;
        };
        for i in 0..s.track_fade.len() {
            let Some((target, rate)) = s.track_fade[i] else {
                continue;
            };
            let cur = s.track_db[i];
            let step = rate * delta;
            let next = if (target - cur).abs() <= step {
                s.track_fade[i] = None;
                target
            } else {
                cur + step * (target - cur).signum()
            };
            s.track_db[i] = next;
            Self::apply_track_db(s, i);
        }
    }

    /// 名前か ID のトラックの添字(レンダラのトラックと同じ並び)
    fn track_index(&self, track: &str) -> Option<usize> {
        let tracks = self.song.as_ref()?.timeline.tracks();
        tracks
            .iter()
            .position(|t| t.name == track)
            .or_else(|| tracks.iter().position(|t| t.id == track))
    }

    /// 曲の終わり(最後の出来事の後の最初の小節の頭。無ければ終わりの秒)のサンプル
    fn song_end_sample(s: &Loaded) -> u64 {
        let end = s.timeline.end_sec();
        let sec = s
            .timeline
            .beats()
            .iter()
            .find(|b| b.beat == 1 && b.sec >= end - 1e-9)
            .map_or(end, |b| b.sec);
        (sec * s.sample_rate) as u64
    }

    /// マーカーの区間(始まりと、次のマーカーか曲の終わり)のサンプル
    fn section_range(s: &Loaded, name: &str) -> Option<(u64, u64)> {
        let secs = s.timeline.sections();
        let i = secs.iter().position(|m| m.name == name)?;
        let start = (secs[i].sec * s.sample_rate) as u64;
        let end = secs.get(i + 1).map_or_else(
            || Self::song_end_sample(s),
            |m| (m.sec * s.sample_rate) as u64,
        );
        (end > start).then_some((start, end))
    }

    /// `(emitted_to, now]` に通り過ぎた出来事をシグナルで出す
    fn emit_until(&mut self, now: f64) {
        let from = self.emitted_to;
        if now <= from {
            return;
        }
        self.emitted_to = now;
        let Some(s) = self.song.as_ref() else {
            return;
        };
        // 出来事を時刻順にまとめてから出す(借用を切るため先に集める)
        enum Ev {
            Beat(i64, i64, f64),
            Section(String, f64),
            Note(String, i64, i64, f64, f64),
        }
        let mut evs: Vec<(f64, u8, Ev)> = Vec::new();
        for m in s.timeline.sections_between(from, now) {
            evs.push((m.sec, 0, Ev::Section(m.name.clone(), m.sec)));
        }
        for b in s.timeline.beats_between(from, now) {
            evs.push((b.sec, 1, Ev::Beat(b.bar as i64, b.beat as i64, b.sec)));
        }
        for w in &self.watched {
            if let Some(t) = s.timeline.track(w) {
                for n in s.timeline.notes_between(t, from, now) {
                    evs.push((
                        n.sec,
                        2,
                        Ev::Note(w.clone(), n.pitch as i64, n.vel as i64, n.sec, n.dur_sec),
                    ));
                }
            }
        }
        evs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (_, _, e) in evs {
            match e {
                Ev::Beat(bar, beat, t) => self.signals().beat().emit(bar, beat, t),
                Ev::Section(name, t) => self
                    .signals()
                    .section()
                    .emit(&GString::from(name.as_str()), t),
                Ev::Note(track, pitch, vel, t, d) => {
                    self.signals()
                        .note()
                        .emit(&GString::from(track.as_str()), pitch, vel, t, d)
                }
            }
        }
    }
}
