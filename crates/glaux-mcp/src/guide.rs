//! AI 向けの定石集(MCP の `get_guide`)と、共通の短い指示(サーバーの instructions)。
//!
//! 以前は同じ内容がチャットのシステムプロンプト(約 6,100 字)・サーバーの instructions(約 2,000 字)・
//! ツールの説明に重複して書かれ、毎ターン全部を送っていた(食い違いもあった)。
//! 常に要る「進め方」だけを [`CORE`] に置き、音作りやジャンルの定石は必要なときに
//! `get_guide {topic}` で読ませる。

/// 共通の指示(MCP サーバーの instructions。アプリ内チャットのシステムプロンプトにも同じ骨子を入れる)
pub const CORE: &str = "Glaux(AI と共同作業できる DAW)のプロジェクト編集サーバー。\
進め方: get_project(include_notes: false)で構造を把握 → 必要なクリップだけ clip_ids と note_format: \"compact\" で読む → \
apply_commands で編集。\
相対編集: transpose_notes / shift_notes / quantize_notes / swing_notes / scale_velocity / transform_notes。構成: duplicate_clips / insert_bars / delete_bars。\
曲作りは get_guide {topic: \"workflow\"} の工程(set_song_plan で計画 → 骨格 = suggest_progression・\
write_drums・write_chords・write_bassline・write_transition → 旋律 → 表情 → 点検)に沿う。\
旋律は write_melody か develop_motif(動機を展開)、critique_melody で点検。\
感覚: analyze_harmony・analyze_rhythm・analyze_audio(per_track)・analyze_sound。\
人も並行して編集する。project_version が進んでいたら get_changes {since: 最後の entry_id} で確認する。\
定石は get_guide {topic}(workflow / melody / groove / instruments / genres / expression / mix / audio / sound_match / clap)。\
ハネ(swing_notes)の後は apply_groove(quantize 0)を重ねる。仕上げは master_mix。\
完了の報告の前に critique_arrangement の warn を直し、analyze_harmony で調性、analyze_audio でバランスを確かめ、\
結果を一言添える。ミックスを変えたら compare_mix で前後を比べる。大きな試行の前は checkpoint。";

/// (トピック名, 見出し, 本文)
pub const TOPICS: &[(&str, &str, &str)] = &[
    (
        "workflow",
        "曲を作る工程と点検表",
        "打ち込みの機械っぽさ・平板さは、AI が作った曲で実際に多かった弱点。次の工程で作る。\n\
1. 計画: set_song_plan で計画書を書く。区間の名前・小節数・盛り上がり(energy 0〜10)・鳴らすトラックの名前・役割\n\
   (動かすもの = ビルドのフィルタ等も note に)。マーカーが置かれ、曲の長さ(秒)が返るので依頼の長さに合わせる。\n\
   critique_arrangement が計画と実際(盛り上がりの上がり下がり・鳴らすトラック)を突き合わせる。フレーズは 4 / 8 / 16 小節単位。山(サビ・ドロップ)の前に静かな区間を置くと山が立つ。\n\
   参考曲の音声があれば analyze_reference で区間の並び・小節数・音量の差を読み、計画に写す(メロディは写さない)。\n\
2. 骨格: テンポ・キー → コード進行 → ドラム → ベース → コード楽器 → 主旋律。フレーズを足す前に analyze_harmony / analyze_rhythm。\n\
   コード進行は suggest_progression で定番から選ぶ(区間ごとに変える)。\n\
   ドラムは write_drums(ジャンルの型・区間ごとに intensity を変える・区切りのフィル・ビルドのロールと gap_beats)。\n\
   キック・スネア・ハットを別トラックにするなら parts(kick / snare / hat / tom / cymbal / perc)で呼び分ける(同じ style・seed)。\n\
   コード楽器は write_chords で置く(和音の積み方と声部のつながりを計算する。暗算で MIDI 番号を書かない)。\n\
   シンセのアルペジオ・ハープ・ピアノの分散和音は write_arpeggio(同じ進行の文字列。型・刻み・オクターブ・強弱の列)。\n\
   進行は記号か、key を付けてローマ数字。積み方はジャンルで: ポップのピアノ = drop2 か close + eighth、\n\
   ハウスのスタブ = close + offbeat(gate 0.4)、パッド・ストリングス = spread + sustain、ジャズ = shell か rootless + charleston、\n\
   ギター = open + voices 6 + range E2-C5 の後に strum_chord(カッティングは rhythm 16 分 + articulation staccato、\n\
   メタルの刻みは articulation palm_mute)。区間ごとに rhythm・range・voices を変えると区間の差になる。\n\
   ベースは write_bassline(伴奏と同じ進行の文字列を渡す): ハウス・ポップ = root8 か offbeat、ディスコ = octave、\n\
   トラップ = 808(C1〜C3)、ファンク = funk か follow_kick(キックと同じ位置)、ジャズ・ローファイ = walking、バラード = root。\n\
   主旋律は write_melody(役割とジャンルから複数案を作り点検で選ぶ)か、短い動機を書いて develop_motif で展開し、\n\
   critique_melody で点検する(topic: melody)。\n\
   繰り返すドラム・リフは add_clip の clip に \"loop\": true, \"loop_len\": 3840(1 小節)を入れ、length を区間の長さにする\n\
   (1 回で済む。試しの編集は要らない)。区間ごとに別のクリップにしておくと、区間の差を付けやすい。\n\
3. 区間の差: 同じ繰り返しにしない。区間ごとにトラックを抜き差しし、区切りの前 1〜2 小節にフィル・ライザー、\n\
   ドロップ・サビの直前に 1 拍〜1 小節の無音(write_transition: gap_beats・リバースクラッシュ・ロール・クラッシュ)。イントロは絞る(全部鳴らさない)。\n\
   1 つのトラックを同じ型のまま 3 区間以上続けない(ブレイクでは抜くか変える。フィルタを動かすだけでは差にならない)。\n\
   抜き差しの例: ファンク・ポップ = イントロはドラムとギターだけ → A メロでベース → B メロでホーン・コード → サビで全部 →\n\
   ブレイクはドラムかベースだけ / EDM = イントロはキックとハット → ビルドでスネアとライザー → ドロップで全部 → ブレイクはパッドと旋律。\n\
4. 表情: ドラムに apply_groove(ジャンルの style。clip_ids で曲じゅうのクリップにまとめて)、必要ならファンク系に\n\
   (ハネる曲は swing_notes の後に必ず apply_groove を quantize 0 で重ねる。ハネだけでは全部の音が同じ位置にそろう)\n\
   add_ghost_notes。ベースは apply_groove as_part: kick、コードの刻みは hat。リード・弦・管のつながったフレーズに legato / portamento、伸ばしに vibrato(topic: expression)。\n\
5. 動き: shape_automation でビルドアップ(カットオフを exp で開く)、区間の頭の音量の出し入れ、パッドの swell、ポンピング(pump)。\n\
6. 音作り・ミックス(topic: instruments / mix)→ 仕上げに必ず master_mix でマスタリング(音量・ピーク・帯域の釣り合い)。\n\
7. 点検(完了の報告の前に必ず): critique_arrangement の warn を直す → analyze_harmony で調性 → analyze_audio でバランス。\n\
点検表: 格子どおりが 95% を超えるトラックが無い / 強弱に幅がある / 3 分の曲でオートメーションが数本以上ある / \n\
区間の energy に差がある(山と谷)/ 同じ型のまま 3 区間以上続くトラックが無い / 低い音域でトラックがぶつからない /\n\
主旋律の critique_melody に warn が無い / マスターに master_mix の処理がある。",
    ),
    (
        "melody",
        "旋律の作り方と点検",
        "旋律の「センス」の多くは数えられる性質。LLM は音符を全部書くと、動機を写すだけ・リズムが単調・形式が崩れる、に\n\
なりやすい。意図(動機・形式・山の位置)だけ決め、展開と点検は道具に任せる。\n\
手早く: write_melody に役割(verse / pre / chorus / hook / lead)・ジャンル・進行を渡すと、ジャンルのリズムの型と輪郭から\n\
動機を作って展開し、点検の点数で足切りした案を違う順に並べて先頭を置く(candidates 案、place: 2〜3 で 2 案目からも\n\
複製トラックに置いて聴き比べ)。\n\
返る motif を手で直して develop_motif に渡せば、動機だけ変えて展開し直せる。rhythm でリズムの型を固定できる。\n\
工程: 1. 計画書で山(サビ・ドロップ)の区間を決める → 2. 山のフック(1〜2 小節の動機)を先に書く。書く前に言葉で\n\
「リズムの型・輪郭(弧 / 上昇 / 下降)・一番高い音の位置」を決める → 3. develop_motif で展開(サビは sentence、\n\
A メロ・ヴァースは period、EDM・トラップは loop)→ 4. ヴァースはフックのリズムの頭から、低く・音を少なく\n\
(同じ動機を seq(-2) や別の形式で)→ 5. critique_melody の warn を直す → 6. seed や form・輪郭を変えた 2〜3 案を\n\
別のクリップに作り、score と聴いた印象で選ぶ(使わない案は消す)。\n\
点数の読み方: 欠点が無いだけなら 70 点で、strengths(山が 1 回・句の終わり・問いと答え・驚きの一瞬・跳躍と戻り)が\n\
付くほど上がる。「欠点が無い」と「良い」は別物(研究で繰り返し確かめられている)。最大の点の案を機械的に選ばず、\n\
点数は足切りにして、輪郭・密度の違う案を人に聴き比べてもらう。\n\
LLM が自分で音符を書くと陥りやすい型(研究でも Glaux でも確認): 格子を全部埋める(休み無し)・隣の音を行き来するだけ\n\
(C–D–C–D)・跳躍が無い・4 小節の型をそのまま繰り返す・全部の音に同じ表情。critique_melody はこれらを warn にする。\n\
良い旋律の性質(研究): 順次進行が多く、7 半音以上の跳躍の後は約 72% が逆向きに戻る。句は上がって下がる弧。\n\
最高音は山の区間で初めて出し、1 回か、フックとして同じ音を叩く。句の終わりは長く伸ばし下がる。強拍は和音の音\n\
(外すなら次の音で 2 度で解決)。ロックの歌では約 23% の音が 8 分前に食う(1・3 拍の直前が多い)。\n\
覚えやすいのは「ありふれた輪郭 + 局所に 1〜2 か所の驚き(跳躍・和音の外の音・食い)」と反復の多さ。\n\
リズム: 動機のリズムは繰り返しつつ、2 回目以降は少し変える(develop_motif の vary・diminish・augment・displace)。\n\
全部の小節が同じリズムだと単調(歌ものは critique_melody が warn)。長い音と短い音、休符、食いを混ぜる。\n\
2〜4 小節ごとに息継ぎ(8 分以上の休符)。シンセのリードも同じ: 音を次の音までつながず、プラック・スタブは音価の\n\
50〜70% で切る。句は小節の頭からばかり始めず、裏から入って 1 拍目を行き先にする。句ごとに行き先(山の音)を決めて\n\
同じ向きに 3〜4 音進み、4 度以上の跳躍を 1 つ入れて逆向きに戻す。繰り返しは 2 回目まで同じでよく、3 回目の後半か\n\
4 回目を変える(AAAB)。移調しただけの繰り返しは変化として弱い(和音に合わせて音を選び直す・リズムを変える)。\n\
ビブラートは伸ばす音だけに、遅らせて掛ける。音域は歌で 19 半音以内、サビの中心はヴァースより 2〜5 半音高く。\n\
ジャンル: ポップ・J-POP = A メロ(低く語る)→ B メロ(上昇・溜め)→ サビ(最高音・伸ばし・リフレイン)/\n\
EDM のリード = 1〜2 小節の動機を繰り返し最後だけ変える、16 分の裏に食う、休みを挟む(トレシーロ x..x..x. や裏拍の\n\
スタブ、問いと答え)、音域は狭くても跳躍で形を作る(Levels は 8 半音) / トラップ = 短音階・\n\
和声的短音階・フリギアの短いループ、休符多め / ローファイ = ペンタトニック + 7 度・9 度、少ない音 /\n\
ジャズ = 強拍に 3 度・7 度、半音で近づく・上下から挟む / ファンクのホーン = 16 分の短いキメ、休符が多い。\n\
トランスのフックはブレイクの前に単純化した形で予告する。",
    ),
    (
        "groove",
        "グルーブと動き",
        "- apply_groove は、人間のドラマーの演奏から集計した型(funk / hiphop / soul / rock / pop / jazz / latin / neworleans / afrobeat)と\n\
  電子音楽の手作りの型(house / techno / trap)で、楽器ごと・16 分の位置ごとのずれと強弱を付ける。既定値から始め、誇張しない\n\
  (ずれを 2 倍にすると評価が下がるという研究がある)。ジャンルの目安:\n\
  ハウス・テクノ・トランス・EDM → house / techno(タイミングはほぼ格子、強弱で揺らす)/ トラップ → trap /\n\
  ヒップホップ・ローファイ → hiphop(+ swing_notes 0.54〜0.62 を先に。ジャズは swing_notes の mode: jazz_tempo でテンポから)/ ファンク・ディスコ・R&B → funk か soul / ポップス・ロック → pop か rock。\n\
- 電子音楽の型(house / techno / trap)では 4 つ打ちのキックは動かさない(位置も強さも一定が土台。キックに手で強弱を付けない)。\n\
- ループのクリップを毎回同じにしないなら set_note_condition: ハットやゴーストに probability 0.6〜0.8、\n\
  4 小節目だけのフィル・スネアの足しに every \"4:4\"(再生と書き出しで同じ結果)。\n\
- ドラムの細部は drum_rudiment: スネアのキメに flam、フィルの頭に drag・ruff、ビルドに roll(vel_curve で強く)、\n\
  トラップのハットの連打は hat_roll(rate 1/32・1/16t)か ratchet(音を 2〜4 回に割る)、ジャズのブラシ風は buzz。\n\
- 変拍子: 拍子に拍のまとまりを持たせる(apply_commands の set_time_sig で {\"num\": 7, \"den\": 8, \"grouping\": [2, 2, 3]}。\n\
  省略すると 7/8 = 2+2+3、5/8 = 2+3、9/8 = 3+3+3、11/8 = 2+2+2+2+3、5/4 = 3+2)。get_project の time_sig_map の meter・bar_ticks・\n\
  steps_16th で 1 小節の長さと 16 分の数を確かめる(7/8 = 3360 tick = 14 ステップ)。write_drums・write_chords・write_bassline・\n\
  write_melody・apply_groove・swing_notes はまとまりに沿う(キックとスネアはまとまりの頭に交互、和音はまとまりの頭で変わる、\n\
  ハネは 3 のまとまりの最後の 8 分を動かさない)。リズムの文字列は 1 小節のステップ数(7/8 なら 14 文字)で書いてもよい。\n\
  1 小節だけ拍を足す・抜く(サビ前の 2/4)は change_meter。ポリリズム(3:2)は write_polyrhythm、周期の違う型の重ね\n\
  (ユークリッドリズム E(5,16) など)は write_polymeter。バルカン風の 7/8・9/8 は set_meter_feel で長い拍を 1.4〜1.45 倍に\n\
  (ちょうど 1.5 倍より少し詰めると前へ転がる。8 分のハネとは別)。4/4 の中の 3+3+2 は write_bassline の tresillo などで。\n\
  音価の読み替えでテンポを変えるのは metric_modulation(3 連の 8 分 = 8 分で 1.5 倍)、3 拍子の終止前は hemiola、\n\
  区間の終わりのキメは write_tihai(同じ句を 3 回で小節の頭に着地)、リフを裏から聞かせるのは shift_notes の wrap_in_bar。\n\
  譜面に書き出すなら export_musicxml(拍のまとまりも残る。SMF の export_midi では分子と分母だけ)。\n\
- まとめて当てる: clip_ids に曲じゅうのクリップ(ドラム・ベース・コード)を渡す。1 回の undo で戻り、クリップごとに揺れは変わる。\n\
- ループのクリップは中身に当たるので、揺れも毎回同じ(ドラムマシンらしさ。電子音楽ならそれで良い)。\n\
  生演奏らしさが要るジャンル(funk / soul / jazz / hiphop・ローファイ)で humanize_ms を使うなら unroll_loop: true。\n\
- 楽器ごとの前ノリ・後ノリ: pocket_ms。レイドバック(ヒップホップ・ネオソウル)は {snare: 6〜10, hat: -3}、前のめり(パンク)は {snare: -5}。\n\
- 小さな揺れ: humanize_ms 3〜8(1/f の相関がある揺れ。小節の頭は揺らさない)。電子音楽のドラムは 0〜3。\n\
- スウィング: swing_notes(0.54 = ストレートのまま硬さが取れる、0.58 = 軽く、0.62 = はっきり、0.667 = 3 連)を先に掛け、\n\
  apply_groove は quantize 0 のまま重ねる(ハネだけで終えない。ヒップホップ・ローファイは hiphop、ジャズは jazz の型)。\n\
- ゴーストノート: add_ghost_notes(ファンク・ソウル・R&B・ヒップホップ。density 0.3〜0.7)。足した後に apply_groove。\n\
- 動き(shape_automation): 位置は「小節:拍」、長さは bars。例:\n\
  ビルドアップ 8 小節 = {target: device/cutoff, shape: exp, from: 300, to: 12000} とスネアの連打、ドロップ直前に 1 拍〜1 小節の無音 /\n\
  ハイパスで抜く = eq の hp_freq を exp で 20 → 800 / ポンピング = {target: track/volume_db, shape: pump, from: 0, to: -6〜-10, period_beats: 1} /\n\
  パッドのスウェル = {shape: swell} / フェードアウト = {shape: log, from: 0, to: -60} / ウォブル = {shape: sine, period_beats: 0.5}。\n\
- critique_arrangement で「格子どおり」「強弱が平ら」「オートメーションが無い」「同じ型のまま」が消えたかを確かめる。",
    ),
    (
        "instruments",
        "音源の選び方",
        "- トラックの音源は set_device {track, device: {type: \"builtin\", name}}。つまみは list_params で意味・範囲・現在値を見て set_param。\n\
- subtractive: シンセ全般(リード・ベース・パッド)。unison + detune で厚く(supersaw)。\n\
  osc_level 0 で雑音だけの音源(noise_color white / pink / brown、crackle でレコードのパチパチ)。プリセット「レコードノイズ」\n\
  (ローファイの地の音。長い音を 1 つ曲の長さぶん)・「ノイズのライザー」(ビルドのシューッ)・「風」。\n\
- fm: エレピ・ベル・マレット・FM ベースなど金属的・打鍵的な音。\n\
- 音を重ねるなら set_layer(本体 + 3 層。キックにサブ〈key_range 35-36〉、リード・コードに 1 オクターブ下のサイン、\n\
  強く弾いたときだけ鳴る層〈vel_range〉)。意味の取っ手は set_macro(「明るさ」= cutoff と reverb.mix など。値は macro/N)。\n\
- 動きのある音色は modulate(トラックの LFO。音源・エフェクトのつまみをテンポに合わせて揺らす): ワブルベース = wavetable の\n\
  position か subtractive の cutoff を 1/8〜1/16 の sine、うねるパッド = cutoff を 2/1 の triangle、ランダムに動く音色 = random。\n\
- wavetable: position を LFO やオートメーションで動かすウォブルベース・うねるパッド・母音のような音・sync のギラついたリード。\n\
- drum: ドラムキット(GM 配置)。**ドラムのトラックには必ず drum**。55 はリバースクラッシュ(ビルドアップ用)。\n\
  kit: トラップ・ヒップホップは 808(kick_decay を伸ばしてベースの役も)、ハウス・テクノは 909、その他は modern。\n\
  キックの音程(kick_tune)は曲の主音に合わせる。スネアは snare_tune・snare_snappy、ハットの長さは hat_decay。\n\
- pluck: ギター・ベース・ハープなど弾く弦の物理モデル。\n\
- エレキギター: pluck だけでは「アンプに繋いでいない生弦」なので、必ず amp エフェクトを後ろに挿す。amp の gain_db は\n\
  〜10 でクリーン、15〜25 でクランチ、30 前後でオーバードライブ、40 以上でメタル。メタルの刻みはノートに articulation: \"palm_mute\"。\n\
  出荷時プリセット(クリーンエレキ / クランチギター / メタルギター)を load_preset するのが早い。\n\
- 本物っぽい楽器一式(ピアノ・ストリングス・ブラス等): SoundFont。list_soundfonts で .sf2 とプリセットを見て\n\
  set_soundfont_instrument。.sf2 が無ければ「アプリの設定 → 表示 →『はじめの確認』の『GM 音源を取得』で入れられる(手持ちの .sf2 の追加も可)」と案内する。\n\
  SFZ の楽器(list_soundfonts の sfz。ラウンドロビン・ハイハットのチョーク付きの実録音源)は set_soundfont_instrument の sfz で。\n\
  返り値の controls が音源の調整つまみ(マイクの混ぜ方・スネアの音程や snap)。sfz_cc で上書き。生の録音は EQ・コンプで仕上げる。\n\
  無料の SFZ 音源(packs)は installed=false ならユーザーに「音源を選ぶ →『SoundFont・SFZ』から取得」を案内する。\n\
- 実録の音を鳴らす: import_sample(WAV の絶対パス。root にサンプルの実音)でトラックの音源を sampler にする。\n\
- 音色プリセット: 音作りの依頼ではまず list_presets → load_preset → 微調整。良い音ができたら save_preset(全プロジェクト共通)。\n\
- エフェクトのプリセット: エフェクト 1 つ分(list_effect_presets → load_effect_preset)。エフェクトを足す前に使える設定がないか見る。\n\
  外してある(parked: true)エフェクトは鳴らないが、ユーザーが取っておいたもの。頼まれない限り消さない。",
    ),
    (
        "genres",
        "ジャンルの語法",
        "- テンポの目安: ハウス 118〜128 / テクノ 125〜135 / トランス 128〜140 / ダブステップ 140(ハーフタイム)/ ドラムンベース 170〜176 /\n\
  フューチャーベース 140〜160 / ヒップホップ 80〜95 / ローファイ 70〜90 / トラップ 130〜150(ハーフタイム)/ ファンク・ディスコ 100〜120 /\n\
  シンセポップ 110〜128 / J-POP 90〜180。\n\
- ドラムの型: ハウス・テクノ = 4 つ打ち + 2・4 拍にクラップ + 裏拍のオープンハット / 2-step = 4 つ打ちから 2・4 拍目のキックを抜く /\n\
  トラップ = キックとスネアはハーフタイム(スネアは 3 拍目)、ハットは 16 分に 2 分割・3 分割の連打を混ぜる、808 は長く伸ばしてグライド /\n\
  ドラムンベース = 2・4 拍のスネア + 細かいブレイク / ファンク = 1 拍目を強く、16 分のシンコペーションとゴースト。\n\
- 進行: 洋楽ポップ I–V–vi–IV・I–vi–IV–V / J-POP の王道進行 IV△7–V7–iii7–vi、丸サ進行 IV△7–III7–vi7–v7–I7 /\n\
  ハウス = m7・m9 の和音を裏拍で短く刻む / フューチャーベース = sus2・sus4・add9・maj9 を広く積んだスーパーソウのスタブ /\n\
  ローファイ = maj7・m9・13 の和音と ii–V / ファンク = 9th・11th 付きの属七の 1〜2 和音のヴァンプ / トランス = 短調・アルペジオ。\n\
- ベースの型: ルートの 8 分(ハウス・トランス)/ 裏拍(トランス)/ オクターブ跳躍(ディスコ・ファンク)/ 808 の伸ばし + グライド(トラップ)/\n\
  ウォーキング(ローファイ・ジャズ)/ 1 音のシンコペーション(ファンク)。\n\
- 構成: EDM = イントロ 16 → ビルド 8 → ドロップ 16 → ブレイク 16 → ビルド 8 → ドロップ 16 → アウトロ 16(小節)。スネアの連打で予告し、\n\
  ドロップの前に 1 拍〜1 小節の無音。J-POP = A メロ 8 → B メロ 8 → サビ 8〜16(頭サビ・落ちサビ・ラスサビ)。サビは高い音・伸ばす音・リフレイン。\n\
- EDM の音作り: スーパーソウは subtractive の unison 5〜7 + detune。ポンピングは sidechain エフェクト(source にキックのトラック ID、\n\
  release_ms = 60000/BPM/2)か shape_automation の pump。ダブステップのウォブルは wavetable の position を lfo_rate で揺らす(8 分 = BPM/30 Hz)。\n\
- メタル: pluck + amp(gain_db 40 以上)+ palm_mute の刻み。Lo-fi・ヴィンテージ: tape エフェクト(wow / flutter・hiss・crackle・bits)。\n\
- 繰り返し: ドラムやリフは 1〜2 小節を書き、add_clip の clip に \"loop\": true, \"loop_len\"(既存のクリップなら set_clip_loop)。\n\
  区間ごとの変化(フィル・抜き差し)は別のクリップで。\n\
- 数値の多くは経験則。critique_arrangement と analyze_audio で確かめる。",
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
- 自由なピッチの動き(ゆっくりしたチョーキング・ダイブ・うねり)はノートの pitch_curve([{tick, cents, shape?}]。tick はノート先頭からの相対、\n\
  100 cents = 半音、最大 16 点、shape は次の点までの曲がり方 linear / ease_in / ease_out / ease_in_out / hold)。\n\
- 定番の音程の表情は pitch_gesture: 歌メロ = 上への跳躍に shakuri(しゃくり)、伸ばしに kobushi を少し、句の終わりに fall を少し /\n\
  ジャズの管 = 句の頭に scoop・plop、句の終わりに fall・doit、伸ばしに shake / ギター = bend・prebend_release・slide_in。\n\
  全部の音に付けるとくどいので、規則(target)と probability(既定 0.7)で選ぶ。\n\
- ビブラートは set_vibrato(style: vocal / vocal_strong / strings / guitar / wind / synth)で伸ばしの音にだけ付ける。\n\
  歌は 250ms ほど後から揺らし始め、サビの最後の伸ばしは vocal_strong や rate_end_hz で終わりを速めると盛り上がる。\n\
- 装飾音は add_ornament(acciaccatura・appoggiatura・mordent・turn・trill・schleifer)。クラシック・バロックは trill と turn、\n\
  ケルト・和風の笛は acciaccatura と schleifer、ジャズは acciaccatura を半音で(interval 1)。付けすぎない(probability 0.3〜0.5)。\n\
- ピアノや伴奏と重なる旋律は melody_lead で 20〜30ms 先に鳴らすと浮き上がる(和音はいちばん上の音だけ動く)。\n\
- ピアノの分散和音・バラードは sustain_pedal(和音が変わる所で踏み替えて響きをつなぐ)。\n\
- 1 音の中の強弱・明るさは note_dynamics: 弦・管・パッドの伸ばしに swell、ブラスのキメに sfz・fp、長い音の終わりに fade、\n\
  シンセのリードの伸ばしに open(暗くから開く)。ノートの volume_curve(dB)・brightness_curve(−1〜1)に展開される。\n\
- 和音が一度に「ジャーン」と鳴るのは機械っぽさの筆頭。ギターのコードは strum_chord(style guitar: 拍の頭は下げ・裏は上げ。\n\
  上げは上の 4 本を弱く速く = up_strings・up_velocity・up_span。16 分のカッティングは 16 分の刻みの和音に掛ける)、\n\
  ピアノのアルペジオ風のばらしは style piano、ハープは harp。読み込んだ MIDI や打ち込みにも後から掛けられる。\n\
- 音の長さがそろいすぎているときは articulate_notes(弦・管のつながりは legato + slur、刻みは staccato、ほどよく切るなら portato)。\n\
  弦の刻み・マンドリンは tremolo single、ピアノ・弦の揺れは alternating、盛り上げの和音は chord。\n\
  区切りへの駆け上がりは glissando(ピアノ・ハープは steps、弦・トロンボーン・シンセは continuous)。\n\
- 句の呼吸は shape_phrase(句の半ばへ少し速く、終わりをリタルダンド。強さも連動)。曲の終わりは final_tempo 0.5〜0.6、\n\
  バラードの句の切れ目は 0.8〜0.9 と ritard_beats 2。テンポの変化として書くので、ノートの位置は変わらない。",
    ),
    (
        "mix",
        "ミックスとエフェクト",
        "- エフェクト: add_effect(eq / dynamic_eq / resonance / compressor / multiband / transient / limiter / width / virtual_bass / reverb / convolution(import_ir で) / distortion / amp / sidechain / delay / chorus / tape /\n\
  clipper / bitcrush / tremolo / phaser / flanger / trance_gate / auto_filter / volume_shaper)→\n\
  set_param(fx/<id>/<名前>)。マスターは add_master_effect / set_master_param。\n\
- distortion はシンセ・ドラム等の歪み。エレキギターの歪みは amp(instruments を参照)。\n\
- 空間: 複数のトラックに同じリバーブ・ディレイを掛けるなら、バス(add_track kind: \"bus\" + リバーブ mix 1.0)を作り、\n\
  各トラックから set_send で送る(トラックごとに挿すより空間がまとまり軽い)。\n\
- delay の time_ms: 4 分 = 60000/BPM、付点 8 分 = 45000/BPM。厚みと広がりは chorus。歌・リードには duck_db 3〜6(ダッキングディレイ)。\n\
- テンポに合わせて動かす(sync = 1/4・1/8d・1/8t など): ポンピングは volume_shaper(サイドチェイン無しで 4 分ごとに沈める。\n\
  ベース・パッド・コード)、パッドを刻むのは trance_gate、ビルドのハイパスや うねるベースは auto_filter(highpass で cutoff を\n\
  オートメーション、LFO は depth)、ファンクのオートワウは auto_filter bandpass + env_amount 2〜3、エレピの揺れは tremolo\n\
  (stereo 1 でオートパン)・phaser。質感: 音圧は clipper(ドラムバス・マスターの前に drive 2〜6)、ローファイ・ゲーム機は bitcrush、\n\
  80 年代のスネアは reverb の gate_ms 150〜300。\n\
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
- 言葉で追い込む: 「もっと暖かく・刺さらないように」のような注文は、必要なエフェクト(eq など)を足してから\n\
  refine_by_words {toward: [\"warm\"], away: [\"harsh\"], params: [動かすつまみ]} で、音を聴き比べながらつまみを決められる。\n\
- 仕上げ(マスタリング): master_mix で、マスターの最後に EQ → コンプ → リミッタを足して音量と釣り合いを整える\n\
  (reference_file に参照曲を渡すと、その音色の釣り合い・広がり・音量に寄せる)。足した後は compare_mix で前後を確かめる。\n\
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
