//! AI 向けの定石集(MCP の `get_guide`)と、共通の短い指示(サーバーの instructions)。
//!
//! 以前は同じ内容がチャットのシステムプロンプト(約 6,100 字)・サーバーの instructions(約 2,000 字)・
//! ツールの説明に重複して書かれ、毎ターン全部を送っていた(食い違いもあった)。
//! 常に要る「進め方」だけを [`CORE`] に置き、音作りやジャンルの定石は必要なときに
//! `get_guide {topic}` で読ませる。

/// 共通の指示(MCP サーバーの instructions。アプリ内チャットのシステムプロンプトにも同じ骨子を入れる)
pub const CORE: &str = "Glaux(AI と共同作業できる DAW)のプロジェクト編集サーバー。\
進め方: get_project(include_notes: false)で構造を把握 → 必要なクリップだけ clip_ids と note_format: \"compact\" で読む → \
apply_commands で編集(ノート・クリップの ID は省略可。サーバーが振る)。\
相対編集は専用ツール: 移調 transpose_notes / 時間移動 shift_notes / クオンタイズ quantize_notes / ハネ swing_notes / \
強さ scale_velocity。構成は duplicate_clips(「サビをもう 1 回」)/ insert_bars / delete_bars。\
感覚: analyze_harmony(キーとコード)・analyze_rhythm(ノリ)・analyze_audio(音量・帯域。per_track でトラック別)・\
analyze_sound(音色)。フレーズを足す前に harmony と rhythm を見て合わせる。\
人間も並行して編集する。project_version が最後に見た値より大きければ get_changes {since: 最後の entry_id} で確認してから作業する。\
音源の選び方・ジャンル・奏法・ミックス・音声素材・似た音作り・CLAP の定石は get_guide {topic}\
(instruments / genres / expression / mix / audio / sound_match / clap)で読める。\
セルフレビュー: まとまった編集の後、完了報告の前に analyze_harmony で調性、analyze_audio でクリップやバランスの破綻を\
確かめ、問題があれば直してから、確認結果(キー・LUFS など)を一言添えて報告する。\
ミックスを変えたら compare_mix で前後を比べる(音量の差ではなく、音量をそろえた違いで判断する)。\
大きな試行錯誤の前は checkpoint、戻すときは revert_to / revert。";

/// (トピック名, 見出し, 本文)
pub const TOPICS: &[(&str, &str, &str)] = &[
    (
        "instruments",
        "音源の選び方",
        "- トラックの音源は set_device {track, device: {type: \"builtin\", name}}。つまみは list_params で意味・範囲・現在値を見て set_param。\n\
- subtractive: シンセ全般(リード・ベース・パッド)。unison + detune で厚く(supersaw)。\n\
- fm: エレピ・ベル・マレット・FM ベースなど金属的・打鍵的な音。\n\
- wavetable: position を LFO やオートメーションで動かすウォブルベース・うねるパッド・母音のような音・sync のギラついたリード。\n\
- drum: ドラムキット(GM 配置)。**ドラムのトラックには必ず drum**。55 はリバースクラッシュ(ビルドアップ用)。\n\
- pluck: ギター・ベース・ハープなど弾く弦の物理モデル。\n\
- エレキギター: pluck だけでは「アンプに繋いでいない生弦」なので、必ず amp エフェクトを後ろに挿す。amp の gain_db は\n\
  〜10 でクリーン、15〜25 でクランチ、30 前後でオーバードライブ、40 以上でメタル。メタルの刻みはノートに articulation: \"palm_mute\"。\n\
  出荷時プリセット(クリーンエレキ / クランチギター / メタルギター)を load_preset するのが早い。\n\
- 本物っぽい楽器一式(ピアノ・ストリングス・ブラス等): SoundFont。list_soundfonts で .sf2 とプリセットを見て\n\
  set_soundfont_instrument。.sf2 が無ければ「FluidR3_GM.sf2 などのフリー SoundFont を設定の SoundFont フォルダに置いて」と案内する。\n\
- 実録の音を鳴らす: import_sample(WAV の絶対パス。root にサンプルの実音)でトラックの音源を sampler にする。\n\
- 音色プリセット: 音作りの依頼ではまず list_presets → load_preset → 微調整。良い音ができたら save_preset(全プロジェクト共通)。\n\
- エフェクトのプリセット: エフェクト 1 つ分(list_effect_presets → load_effect_preset)。エフェクトを足す前に使える設定がないか見る。\n\
  外してある(parked: true)エフェクトは鳴らないが、ユーザーが取っておいたもの。頼まれない限り消さない。",
    ),
    (
        "genres",
        "ジャンルの定石",
        "- EDM: supersaw は subtractive の unison 5〜7 + detune。ポンピングは sidechain エフェクト(source にキックのトラック ID、\n\
  release_ms = 60000/BPM/2 で 8 分に合わせる)。ビルドアップは cutoff を device/cutoff のオートメーションで開いていく + ドラムのノート 55。\n\
- ダブステップ・ベースミュージック: wavetable のウォブル(position を lfo_rate で揺らす。8 分 = BPM/30 Hz)。出荷時プリセット「ウォブルベース」。\n\
- メタル: pluck + amp(gain_db 40 以上)+ palm_mute の刻み。\n\
- Lo-fi Hip Hop・ヴィンテージ: tape エフェクト(wow / flutter の揺れ、hiss、crackle、bits)。ドラムバスやマスターにも。ハネは swing_notes。\n\
- ハネ・シャッフル: swing_notes(0.667 ≈ 3 連、0.58 で軽く)。既存のノリに合わせるなら analyze_rhythm の swing_ratio を参考に。\n\
- 繰り返し: ドラムパターンやリフは 1〜2 小節を書いて set_clip_loop {id, loop_len} → resize_clip で伸ばす(1 か所直せば全体に反映)。\n\
- 曲の構成: set_sections で intro / Aメロ / サビ などのマーカーを置く。「サビだけ盛り上げて」は sections を見て tick 範囲に解決する。\n\
  構成が決まってきたら自発的に set_sections で記録しておくと後の指示が正確になる。",
    ),
    (
        "expression",
        "奏法と表情",
        "- ノートの articulation: palm_mute(ブリッジミュート)/ staccato / accent / vibrato(ロングトーンの表情)/\n\
  bend(チョーキング。全音下から滑り上がる)/ legato(弾き直さずにつなぐ)/ portamento(直前の音から滑る。フレーズの頭なら全音下から)。\n\
  楽器ごとに効くものが違う(list_params の articulations)。\n\
- ストリングス・管・歌・シンセリードのつながったフレーズは、2 音目以降に legato、音程を滑らせたい所に portamento を付けると打ち込みっぽさが減る。\n\
- 滑る時間はトラック全体なら set_param track/glide_ms(ゆったりした弦 250〜400、速いリード 50〜80)、1 音だけならノートの glide_ms。\n\
  つなぎ目の長さは track/legato_ms(既定 30、パッド的にふんわりなら 80〜150)。\n\
- 自由なピッチの動き(ゆっくりしたチョーキング・ダイブ・うねり)はノートの pitch_curve([{tick, cents}]。tick はノート先頭からの相対、\n\
  100 cents = 半音、最大 8 点)。",
    ),
    (
        "mix",
        "ミックスとエフェクト",
        "- エフェクト: add_effect(eq / compressor / reverb / distortion / amp / sidechain / delay / chorus / tape)→\n\
  set_param(fx/<id>/<名前>)。マスターは add_master_effect / set_master_param。\n\
- distortion はシンセ・ドラム等の歪み。エレキギターの歪みは amp(instruments を参照)。\n\
- 空間: 複数のトラックに同じリバーブ・ディレイを掛けるなら、バス(add_track kind: \"bus\" + リバーブ mix 1.0)を作り、\n\
  各トラックから set_send で送る(トラックごとに挿すより空間がまとまり軽い)。\n\
- delay の time_ms: 4 分 = 60000/BPM、付点 8 分 = 45000/BPM。厚みと広がりは chorus。\n\
- エフェクトのつながり(ノード表示の線): 並列(原音 + リバーブ、パラレル・コンプ)にしたいときは set_fx_links で\n\
  [in→eq, eq→out, eq→rev(gain_db で混ぜる量), rev→out] のように分けて合流させる。鳴るのは入力から出口までたどれるものだけ\n\
  (list_params の sounding)。ユーザーがつないだ表は読んでから、必要な線だけを足し引きする。\n\
- バランス: analyze_audio {per_track: true} で各トラックのラウドネスと帯域。主役は伴奏より 2〜4dB 上 / 帯域の重心が被る\n\
  トラックは EQ で住み分け / それでも埋もれるなら伴奏側に sidechain。音量を上げる前に被りを削ることを検討する。\n\
- 時間変化: set_automation_points(target: track/volume_db・track/pan・device/<パラメータ>・fx/<id>/<パラメータ>)。\n\
  曲全体のフェードやマスターのエフェクトは set_master_automation_points。\n\
- ミックス調整は「checkpoint → 編集 → compare_mix {checkpoint} で前後を比べる → 微調整」のループで行う。\n\
  人の耳は 0.5〜1dB 大きいだけで良く聞こえるので、loudness_diff_db ではなく tonal_balance・stereo・plr/psr で良し悪しを決める。\n\
  音量まで変わったなら match_gain_db の分だけ戻してから比べ直す。\n\
- 自動ミキシングの研究の経験則: (1) まず各トラックのラウドネスをおおむね揃え、主役だけ 2〜4dB 上げる。\n\
  (2) 低い帯域ほど中央、高い帯域ほど左右へ(キック・ベース・低音のパッドはパン 0。stereo.low_correlation は 1 近く)。\n\
  (3) 被りはまず EQ で削る(持ち上げるより削る。狭く削って広く持ち上げる)。(4) コンプの量は役割とクレストファクターで決める。\n\
- EQ: ベース・キック以外は hp_freq で 80〜150Hz 以下を切ると低域がすっきりする。刺さる高域やノイズは lp_freq。\n\
- compressor: ratio 2〜4・knee_db 6〜12 で自然に揃える。ボーカルやバス・マスターは detector: \"rms\"、ドラムの山を抑えるなら \"peak\"。\n\
  アタックを 10〜30ms にすると打点の抜けが残る。バス・マスターは sc_hpf_hz 80〜150 で低音によるポンピングを防ぐ。\n\
- 音量の仕上げ: 配信は正規化される(Spotify・YouTube -14 LUFS、Apple Music -16)。-14 より大きいマスターは下げて再生されるだけで、\n\
  潰した分ダイナミクスを失う(analyze_audio の streaming で予測が見られる)。True Peak は -1 dBTP 以下\n\
  (export_audio のリミッタが保証する)。plr_db・psr_min_db はおおむね 8 以上を保つ(下回ると潰しすぎの目安。規格ではない)。",
    ),
    (
        "audio",
        "音声素材",
        "- 録音や音声ファイル(WAV / MP3 / FLAC / OGG / M4A)は音声トラック(kind: audio)のクリップ。置くのは import_audio_clip。\n\
  get_project で見え、analyze_audio で聴ける。\n\
- パート分離: separate_audio(builtin = 打楽器 / 音程楽器、demucs = ボーカル / ドラム / ベース / その他)。\n\
- 譜起こし: transcribe_audio(鼻歌・単旋律。ピアノ・ギターの和音や伴奏入りは mode: \"poly\")。MIDI 化したら analyze_harmony で\n\
  キーを確認し、前後と 12 半音ずれた短い音(オクターブ誤検出)や外れた音を整えてから報告する。ベースだけなら分離 → 譜起こし。\n\
- テンポを変えるときは、音声クリップに set_clip_stretch(follow、original_bpm = 録音時のテンポ)を付けると拍がずれない。\n\
  元のテンポが分からなければ analyze_beats(bpm・拍子・最初の小節頭)。",
    ),
    (
        "sound_match",
        "似た音を作る",
        "- 音色を確かめる: analyze_sound(track_id + pitch でトラックの 1 音、clip_id / file でサンプル)。数値に加えて words\n\
  (楽器らしさ・明るさ・質感の言葉)が返る。words は目安なので、数値(明るさ・包絡・倍音)と食い違えば数値を優先。\n\
- 「このサンプルに似た音を作って」: analyze_sound で目標を把握 →\n\
  (1) シンセらしい単音なら match_sound(内蔵 subtractive / fm / wavetable を自動で選んでつまみを合わせる。reverb: true でリバーブも。30 秒ほど)、\n\
  (2) 複雑な音・生楽器寄りなら CLAP のトラックで find_similar_presets → load_plugin_preset → refine_plugin_params、\n\
  仕上げは compare_sounds(a = 目標、b = トラックの音)の differences を見てつまみ・エフェクトで詰める。\n\
- distance が 0.35 未満なら「よく似ている」、0.7 以上は別物。結果は数値で報告し、最後は人間の耳で確かめてもらう。\n\
- 人間が自分で試すなら、音声クリップの右クリック →「この音に似せた内蔵シンセのトラックを作る」「この音に近い CLAP 音源のプリセットを探す」と案内できる。",
    ),
    (
        "clap",
        "CLAP プラグイン",
        "- list_plugins で一覧(Surge XT などの音源、effect: true はエフェクト)。音源は set_device {type: \"clap\", plugin_id}。\n\
- 音色の大枠はプラグイン自身の画面で人間が作る。つまみは list_params(filter で絞る。例 \"cutoff\")で探し、\n\
  set_param {path: \"device/clap:<id>\"} や set_automation_points で動かす(値はプラグインの単位、current_text が画面の表示)。\n\
- 音色の土台は list_plugin_presets(filter・category で絞る)→ load_plugin_preset。プリセットにはつまみで触れない設定\n\
  (LFO のテンポ同期・モジュレーション)も入っているので、動きのある音はまずプリセットから探す。\n\
- CLAP のエフェクトも add_effect / add_master_effect に {type: \"clap\", plugin_id} で挿せる。つまみは list_params の effects\n\
  (path fx/<id>/clap:<番号>)。\n\
- ピアノロールのピッチカーブもプラグインに届く。",
    ),
];

/// トピックの本文(見出し付き)。無ければ None
pub fn guide(topic: &str) -> Option<String> {
    TOPICS
        .iter()
        .find(|(name, _, _)| *name == topic)
        .map(|(_, title, body)| format!("# {title}\n{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_short_and_lists_every_topic() {
        // 毎ターン送るので短く保つ(以前は instructions 約 2,000 字 + チャット 6,100 字)
        let n = CORE.chars().count();
        assert!(n <= 1_000, "CORE が長すぎます({n} 字)");
        for (name, _, body) in TOPICS {
            assert!(CORE.contains(name), "CORE に {name} が無い");
            assert!(!body.is_empty());
            assert!(guide(name).is_some());
        }
        assert!(guide("no_such").is_none());
    }
}
