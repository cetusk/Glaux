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
use glaux_engine::render::Shared;
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

    fn process(&mut self, _delta: f64) {
        if self.song.is_none() {
            return;
        }
        if let Some(a) = self.audio.as_mut() {
            a.set_volume_db(self.volume_db);
        }
        if !self.playing {
            return;
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
                self.song = Some(Loaded {
                    timeline: s.timeline,
                    shared,
                    clock,
                    sample_rate,
                    warnings: s.warnings,
                });
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
        s.shared.seek.store(sample, Ordering::Release);
        s.shared.pos.store(sample, Ordering::Release);
        s.clock.mix_start.store(sample, Ordering::Release);
        s.shared.playing.store(true, Ordering::Release);
        self.reset_position(sec.max(0.0));
        self.playing = true;
        self.finished_sent = false;
    }

    /// 止める(位置は曲頭に戻る)
    #[func]
    fn stop(&mut self) {
        if let Some(s) = self.song.as_ref() {
            s.shared.playing.store(false, Ordering::Release);
            s.shared.seek.store(0, Ordering::Release);
        }
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
        s.shared.seek.store(sample, Ordering::Release);
        s.shared.pos.store(sample, Ordering::Release);
        s.clock.mix_start.store(sample, Ordering::Release);
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

    /// いま聞こえている位置(秒)
    fn song_time(&mut self) -> f64 {
        let Some(s) = self.song.as_ref() else {
            return 0.0;
        };
        if !self.playing {
            return self.held_time;
        }
        let server = AudioServer::singleton();
        let mix_start = s.clock.mix_start.load(Ordering::Acquire) as f64 / s.sample_rate;
        let t = mix_start + server.get_time_since_last_mix()
            - server.get_output_latency()
            - self.latency_offset_ms / 1000.0;
        // 推定のぶれで逆戻りしない(シーク・再生開始でだけ戻る)
        self.last_time = self.last_time.max(t);
        self.held_time = self.last_time;
        self.last_time
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
