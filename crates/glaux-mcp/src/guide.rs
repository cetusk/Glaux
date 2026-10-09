//! AI 向けの定石集(MCP の `get_guide`)と、共通の短い指示(サーバーの instructions)。
//!
//! 以前は同じ内容がチャットのシステムプロンプト(約 6,100 字)・サーバーの instructions(約 2,000 字)・
//! ツールの説明に重複して書かれ、毎ターン全部を送っていた(食い違いもあった)。
//! 常に要る「進め方」だけを [`CORE`] に置き、音作りやジャンルの定石は必要なときに
//! `get_guide {topic}` で読ませる。

/// 共通の指示(MCP サーバーの instructions。アプリ内チャットのシステムプロンプトにも同じ骨子を入れる)
pub const CORE: &str = "Glaux(AI と共同作業できる DAW)の編集サーバー。\
進め方: get_project(include_notes: false)で構造 → 必要なクリップだけ clip_ids と note_format: \"compact\" で読む → \
apply_commands で編集。\
相対編集: transpose_notes・shift_notes・quantize_notes・swing_notes・scale_velocity・transform_notes。構成: duplicate_clips・insert_bars・delete_bars。\
曲作りは topic workflow の工程(set_song_plan で区間 → save_plan で曲全体とパートの計画 → 骨格 = suggest_progression・\
write_drums・write_chords・write_bassline・write_transition → 旋律(critique_melody で点検)→ 表情 → 点検)に沿う。\
感覚: analyze_harmony・analyze_rhythm・analyze_audio・analyze_sound。\
人も並行して編集する。project_version が進んだら get_changes {since: 最後の entry_id}。\
定石は get_guide {topic}(commands・workflow・revise・melody・groove・arrangement・instruments・sound_design・genres・expression・mix・mastering・\
audio・sound_match・clap。user: はユーザーの定石。あれば先に読む)。\
仕上げは master_mix。\
曲作りの報告前は critique_arrangement の warn を直し、analyze_harmony・analyze_audio の結果を添える。\
修正は topic revise: 応答の aftercare の warn を直し、報告前に review_edits。\
音・ミックスを変えたら critique_mix の warn を直し compare_mix。大きな試行の前は checkpoint。\
案・候補を頼まれたら(音色・型・ミックスも)曲は変えずに propose_design を案ごとに呼ぶ(ミュートのトラックや複製で代用しない)。";

/// (トピック名, 見出し, 本文)
pub const TOPICS: &[(&str, &str, &str)] = &[
    (
        "commands",
        "apply_commands の各 op の詳しい書き方",
        "- 奏法(ノートの articulation。省略で通常、update_notes でも変えられる):\n\
  palm_mute(ブリッジミュート。減衰が速いこもった刻み)/ staccato(音価の半分で切る)/ accent(強く明るく)/\n\
  vibrato(後半にかけて深くなるピッチの揺れ。ロングトーンの表情)/ bend(チョーキング: 全音下から書かれた音へ滑り上がる。ギターソロの決め音)/\n\
  legato(同じトラックの直前の音から弾き直さずにつなぐ。弦・管・歌・リードのフレーズ、ギターのハンマリング。前の音との隙間 0.3 秒まで。\n\
  つなげたい 2 音目以降に付ける)/ portamento(legato でつなぎ、直前の音の高さから約 0.15 秒で滑らせる。直前の音が無い\n\
  (フレーズの頭・0.3 秒より離れた)ときは全音下から滑り込む。ストリングスのポルタメント・シンセのグライド・ギターのスライド)。\n\
- 滑る時間: トラック全体は set_param {track, path: \"track/glide_ms\", value}(10〜2000ms、既定 150)、1 音だけならノートの glide_ms\n\
  (0 で解除してトラックの値へ)。レガートのつなぎ目は track/legato_ms(5〜200ms、既定 30。長いほどふんわり重なる)。どちらも unset_param で既定。\n\
- 連続ピッチカーブ: ノートの pitch_curve に [{tick, cents, shape?}](tick はノートの頭から、cents は書かれた音からのずれ。100 = 半音、\n\
  ±2400 まで、最大 16 点。shape は linear(既定)/ ease_in / ease_out / ease_in_out / hold、両端は保持)。自由なベンド・うねりに。\n\
  例: 1 拍かけてチョーキング = [{tick:0,cents:-200},{tick:960,cents:0}]、ダイブ = [{tick:0,cents:0},{tick:1920,cents:-1200}]。\n\
  update_notes の pitch_curve で差し替え、[] で削除。しゃくり・フォールなどの定番は pitch_gesture、ビブラートの細かい指定はノートの vibrato か set_vibrato。\n\
- メタルの「ズクズク」した刻み: pluck + amp(gain_db 40 以上)+ 低音 + palm_mute のノート。\n\
- バス(リターン): add_track の kind: \"bus\"(クリップは置けない。エフェクトを挿して共有のリバーブ・ディレイに。リバーブは mix: 1.0 = ウェットのみが基本)。\n\
  set_send {track, target, level_db, pre_fader?} でトラックからバスへ送る(level_db -60〜12。省略でセンドを外す。pre_fader: true でフェーダーの前 =\n\
  トラックの音量に追従しない)。送り先はバスだけで、バスからほかのバスへも送れる(輪になる送りは失敗)。バスはソロの影響を受けない。\n\
  歌・スネア・パッドを同じ空間に置くのに。\n\
- グループ(まとめ): set_track_prop {id, prop: \"output\", value: <バスの ID> | null} でトラックの出力先をバスにする(null でマスター)。\n\
  ドラムを 1 本のバスにまとめてコンプ・音量をまとめて動かすなど。バスの出力先を別のバスにして段にできる(輪は失敗)。\n\
  遅れのあるエフェクトがあっても、合流するところで自動で揃える。バスをソロにすると流れ込むトラックも鳴る。\n\
- マスターのエフェクト: add_master_effect {effect, index?} / set_master_param {path: \"fx/<id>/<名前>\", value} / unset_master_param {path}。\n\
  削除・並べ替え・バイパスはトラックと同じ remove_effect / move_effect {id, to_index} / set_effect_bypass。チェーンは get_project の master.effects。\n\
- set_effect_prop {id, prop: \"label\" | \"parked\" | \"note\" | \"pos\", value}: 表示名・線から外す・メモ・ノード表示の位置。\n\
  parked: true のエフェクトは鳴らないが設定は残る(ユーザーが取っておいたものなので、頼まれない限り消さない)。\n\
- set_fx_links {track?, links}: エフェクトのつながり(ノード表示の線)を丸ごと置き換える。track 省略でマスター。links は [{from, to, gain_db?}] で、\n\
  端は \"in\"(音源・受けた音)/ \"out\"(音量・パンへ)/ エフェクト ID。1 つの口から何本でも出せ(分岐)、何本でも入れられる(合流 = 足し合わせ)。\n\
  入力から出口まで線でたどれるエフェクトだけが鳴る(各エフェクトの sounding)。輪は不可。例: 原音とリバーブを並列に =\n\
  [in→eq, eq→out, eq→rev(gain_db -8), rev→out]。null で並び順の直列に戻す。表(tracks[].fx_links)があるトラックでは parked は使えず、\n\
  add_effect は出口の直前に入り、remove_effect は前後をつなぎ直す。書き換える前に今の表を読み、ユーザーのつなぎ方を崩さない。\n\
- set_clip_loop {id, loop_len}: MIDI クリップのループ(クリップの頭からの tick の長さを、クリップの長さまで繰り返す。null で解除)。\n\
  ドラムパターンやリフは 1〜2 小節を作ってループにし、resize_clip で伸ばすのが速い。ループの範囲より後ろのノートは鳴らない。\n\
- set_clip_stretch {id, stretch}: 音声クリップのテンポ追従。{mode: \"follow\", original_bpm} で素材を original_bpm の演奏として扱い、\n\
  曲のテンポを変えても拍がずれないよう音程を保ったまま伸縮する(録音・取り込みのときの曲のテンポを入れる)。{mode: \"none\"} で解除。\n\
- set_automation_points {track, target, points}: target は \"track/volume_db\" / \"track/pan\" / \"device/<パラメータ名>\"(list_params の連続値。\n\
  値はパラメータと同じ単位)/ \"fx/<エフェクト ID>/<パラメータ名>\"。points は [{tick, value, curve?}](curve は linear / hold / exponential)。\n\
  フェードイン・ビルドアップの音量・左右の揺れ・フィルタのスイープに。レーンがあるとフェーダー・つまみの値より優先。空の配列でレーンを消す。\n\
  マスターは set_master_automation_points {target, points}(\"track/volume_db\" で曲全体のフェードアウト、\"fx/<マスターのエフェクト ID>/<名前>\")。\n\
- set_clip_plan {clip, plan: {id, rev, digest} | null}: クリップがどの旋律の計画の版から作られたか(get_plan の plan_id・rev・digest)。\n\
  計画が先に進むと get_plan で plan_ahead と出る。",
    ),
    (
        "workflow",
        "曲を作る工程と点検表",
        "打ち込みの機械っぽさ・平板さは、AI が作った曲で実際に多かった弱点。次の工程で作る。\n\
1. 計画: set_song_plan で計画書を書く。区間の名前・小節数・盛り上がり(energy 0〜10)・鳴らすトラックの名前・役割\n\
   (動かすもの = ビルドのフィルタ等も note に)。マーカーが置かれ、曲の長さ(秒)が返るので依頼の長さに合わせる。\n\
   critique_arrangement が計画と実際(盛り上がりの上がり下がり・鳴らすトラック)を突き合わせる。フレーズは 4 / 8 / 16 小節単位。山(サビ・ドロップ)の前に静かな区間を置くと山が立つ。\n\
   参考曲の音声があれば analyze_reference で区間の並び・小節数・音量の差を読み、計画に写す(メロディは写さない)。\n\
   区間の中の盛り上がりの形は curve、ビルド → ドロップの落差のような急な変わり目は join: step。\n\
   続けて、音を書く前に必ず曲全体の狙い(save_plan kind song: 雰囲気・盛り上がりの型・明るさ・守ること)と、トラックごとの\n\
   パートの計画(kind part: 区間ごとの働き・存在の段階 0〜5・音域の帯)を書く(トラックを作ってから。人が「計画は作らない」と\n\
   言ったときだけ省く)。作る道具はパートの計画に沿い(鳴らさない区間に書かない・帯へ寄せる)、設計画面に計画と実際が並ぶ。\n\
   計画より先に音を書くと、設計画面に音域の帯が無く「実際を計画に」しか出ない。get_design が計画と実際のずれを返す。\n\
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
        "revise",
        "作った曲を直すとき(直す範囲は狭く、確かめる範囲は広く)",
        "- 直す範囲は頼まれた所だけ(頼まれていないパート・区間を勝手に変えない)。確かめる範囲は広く取る。\n\
- 直す前に読む: 対象のクリップに加え、前後 1〜2 小節、同じ小節のほかのパート(get_project の clip_ids で同じ区間のクリップ)、\n\
  その小節の和音(analyze_harmony の start_tick / end_tick)、区間の計画(sections の energy・tracks・note)。\n\
- ノートを変える道具の応答には aftercare が付く(変わった範囲 ranges と、その周りの点検 issues)。\n\
  warn(clash = ほかのパートと半音でぶつかる、stale_copy / stale_section = 直す前と同じ中身だった繰り返しが古いまま、\n\
  beyond_clip = クリップの外にはみ出す)は直すか、わざとなら報告で理由を言う。info(out_of_key・leap・velocity_step・\n\
  denser / thinner・overlap)は意図と照らして見直す。\n\
- 繰り返し: 「サビを直して」なら繰り返しの全部に当てる(duplicate_clips で置き直すか同じ編集)。「1 回目のサビを」ならそこだけ。\n\
  どちらか分からなければ 1 か所だけ直し、報告で「ほかのサビにも当てるか」を尋ねる。\n\
- 前後とのつなぎ: 直した範囲の頭と終わりで音程が跳ぶ・強さが段差になるなら、つなぎの音・強さを合わせる。\n\
- 厚さ: 音を足して厚くなったら critique_mix で住み分けと音量、薄くなったら critique_arrangement で区間の盛り上がりを見る。\n\
  ベース・キック・音色・エフェクトを変えたら compare_mix(render)で前後を比べる。\n\
- 報告の前に review_edits(before_entry に今回の最初の編集、または back に今回の編集の数。音が変わったなら render: true)。\n\
  報告には「直した所」「確かめた所(checked)」「残した知らせとその理由」を短く添える。\n\
- 大きく直す前は checkpoint。結果が悪ければ revert_to で戻して別の案を試す。\n\
- 人の手直しと固定: 直す前に get_design の clips を見る。edited_bars(人が手で直した小節)と固定の音(locked)は人の意図。\n\
  固定の音は AI の編集では変わらない(外した編集は応答の kept_locked。固定を外すのは人だけ)。\n\
  realize_melody・revise_melody の作り直しは、手で直した小節を既定で残す(応答の protected)。残したことを報告し、\n\
  人が「手直しも含めて」と言ったときだけ overwrite_edits: true。ほかの道具でも、手で直した小節は消さずに避けて書く。\n\
- 作る道具(write_bassline / write_chords / write_arpeggio / write_melody / write_drums)は、トラックに採用済みのパートの計画があれば\n\
  目安にする: 鳴らさない区間・固定の区間には書かず、区間ごとに音域の帯へ寄せる(range を省くと帯で作る)。合わせた所は応答の plan。\n\
  計画を見ずに作るときだけ follow_plan: false。\n\
- 案(枝): 好みが分かれる・大きく変える直しは、曲を直接変えずに propose_design で案として出す(元の計画と、案の音になる編集の列)。\n\
  人が設計画面で聴き比べて「採用」すると曲に当たる。出したら「案を出したので聴き比べて」と伝えて待つ。\n\
  別々の案を並べて比べてもらうときは、案ごとに propose_design を呼ぶ。案が 2 つ以上あると、人は「まとめて聴き比べる」で\n\
  今(A)と案(B・C … 4 つまで)を同じ範囲で切り替えて聴ける。別々の問いに案を並べるときは group に問いの名前を付ける\n\
  (採用すると同じ問いの案だけ捨て、別の問いの案は残る)。\n\
- 人のメモ: get_design の song.memos は、人が設計画面で所(区間・パート・マス・段の全体)に付けた言葉の意図。その所を作る・直すときは\n\
  必ず読んで従う(AI はメモを書かない。判断の理由は計画の why に残す)。\n\
- 計画とのずれ: get_design の deviations は計画と実際の食い違い。直すのは計画か音のどちらか(人の意図に近い方。迷えば尋ねる)。\n\
  推定した計画(state: estimated)は参考だけ。採用は人がする。",
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
全体の流れは analyze_melody で読み返す(句の地図 A・A′・B、区間ごとの音域と密度の曲線、骨格)。[区間] の warn\n\
(どこも同じリズムの輪郭・音域が動かない)は音を少し変えても直らない。区間の音域の軌跡と句の形から作り直す。\n\
ハウス・EDM のリードは歌ではない: plan_melody の style riff(edm の既定)で、短いリフを和音に合わせて繰り返し\n\
4 小節目だけ変える。歌のビブラートや句の弧は付けない。変化はリフの差し替え・オクターブ・アレンジで出す。\n\
上から下へ作る(推奨。区間をまたぐ旋律・リード): plan_melody で区間ごとの計画(音域の軌跡・密度・句の並び・終止・\n\
リズムの系統)を立て、get_plan で読んで作曲者の言葉に合わせ edit_plan で直し、realize_melody で音にする。\n\
直すときも上から: 区間の register・rhythm_family → 句の bars・offset_beats・cadence → 骨格(skeleton)の順に edit_plan して\n\
realize_melody(sections で区間だけ・seed でリズムと表面だけ変える)。write_melody は短い動機からの手早い案。\n\
直すのは revise_melody(手を何案か試し、測って良くなったときだけ採用。auto で指摘から安い手を順に)。聴いた感想は\n\
手に訳す: 機械的 → expression(amount・feel)/ 同じ輪郭 → reseed・rephrase / 音域が狭い → register spread・shift。\n\
計画: analyze_melody の plan を save_plan で保存し(why に作曲者の言葉)、edit_plan で上の粒度から直す\n\
(区間の register・density → 句の長さ・入り・終止 → 骨格)。why・trigger・measures(前後の測定)を必ず残し、\n\
効果の無かった変更は plan_log で探して undo_plan の revert で戻す。計画の履歴は曲の履歴と別。\n\
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
        "arrangement",
        "曲構成と展開(区間の役割・密度・緊張と解放・変わり目)",
        "- 区間には役割がある: イントロ(期待させる。要素は絞る)/ A メロ・ヴァース(語る。低め・薄め)/ B メロ・プレコーラス・ビルド(高まる。\n\
  密度・音域・リズムの細かさを少しずつ上げる)/ サビ・ドロップ(いちばん高い・広い・厚い)/ ブレイク(息をつく。抜いて対比を作る)/\n\
  ブリッジ(新しい和声・景色)/ アウトロ(閉じる)。set_song_plan の energy(0〜10)で山と谷を先に決め、critique_arrangement で\n\
  計画と実際(鳴らすトラック・盛り上がり)を突き合わせる。\n\
- 対比が展開を作る: 次の区間は前の区間と 2 つ以上を変える(鳴らすトラック・音域・リズムの細かさ・和音の積み方・ステレオの広さ・\n\
  リバーブの量・ベースの型)。同じ要素のまま音量だけ上げても山にならない。サビ・ドロップの前は 1 拍〜1 小節抜く(write_transition の gap_beats)。\n\
- 密度の階段: 8 小節(EDM は 16 小節)ごとに 1 つ足すか抜く。ヴァース 2 回目は 1 回目と同じにせず、カウンターメロディ・ハットの刻み・\n\
  パッドの動きなどを 1 つ足す。ラスサビは転調・ハーモニー・高いオクターブ・ドラムの手数のどれかで 1 段上げる。\n\
- 周波数の席: 区間ごとに、低域(キック・ベース)・中低域(コード・パッド)・中域(主旋律)・高域(ハット・きらめき)の席に誰が座るかを決める。\n\
  主旋律の音域にコード楽器を重ねない(write_chords の range を下げるか、主旋律の間はコードを短く刻む)。\n\
- 主旋律と応答: 主旋律が休む所(フレーズの終わりの 1〜2 拍)に別の楽器の短い応答(コール・アンド・レスポンス)を入れる。\n\
  主旋律が動いている間は伴奏を動かしすぎない。\n\
- 緊張と解放: ビルドは「上げる」要素を重ねる(ハイパスを開く・スネアの連打を細かく・ライザー・ピッチを上げる・リバーブを長く)。\n\
  shape_automation(exp でカットオフ、volume の swell)と write_transition(ロール・ライザー・リバースクラッシュ)。\n\
  解放(ドロップ・サビの頭)はクラッシュ + キック + ベースの頭を揃え、ビルドの要素を一斉に止める。\n\
- 変わり目のつなぎ: 区切りの前 1〜2 小節にフィル(write_drums の区切り・drum_rudiment)、区切りの頭にクラッシュ。\n\
  雰囲気を変えるときは、前の区間の終わりの和音を次の区間の調の V や借用和音にしてつなぐ(suggest_progression)。\n\
- 長さの目安: ポップ 3〜4 分(サビまで 1 分以内)/ EDM のクラブ用は 5〜7 分(DJ がつなげるよう頭と終わりの 16〜32 小節はドラム中心)/\n\
  配信用の EDM は 3 分前後 / ゲーム・映像は ループの継ぎ目(最後の小節から頭へ自然に戻る)を確かめる。\n\
- 確かめ方: critique_arrangement の warn を直す → analyze_audio の short_term_lufs で区間ごとの音量の推移が計画の energy と同じ形か\n\
  (山の区間が 2〜4 LU 大きいのがふつう)→ per_track の sections で、各トラックが区間ごとに出入りしているか。",
    ),
    (
        "instruments",
        "音源の選び方",
        "- トラックの音源は set_device {track, device: {type: \"builtin\", name}}。つまみは list_params で意味・範囲・現在値を見て set_param。\n\
- subtractive: シンセ全般(リード・ベース・パッド)。unison + detune で厚く(supersaw)。\n\
  osc_level 0 で雑音だけの音源(noise_color white / pink / brown、crackle でレコードのパチパチ)。プリセット「レコードノイズ」\n\
  (ローファイの地の音。長い音を 1 つ曲の長さぶん)・「ノイズのライザー」(ビルドのシューッ)・「風」。\n\
- **生きた音**(subtractive・wavetable 共通のつまみ。既定は全部 0 = 止まった音。ただし add_track で新しく作ると analog 0.2・spread 0.5 で始まる。\n\
  要らなければ 0 を指定): 止まった・平らな音はプロっぽく聞こえない。\n\
  spread(ユニゾンを左右に。0.5〜0.8。ベースは 0)、analog(揺らぎ。0.2〜0.4 で自然)、filter_type(lp24 で太く、hp で細く、bp で電話)、\n\
  drive(フィルタ前の歪みで厚み)、filter_decay + filter_env(音量と別に頭だけ開く = プラック・ベースのアタック)、\n\
  vel_cutoff(強く弾くと明るい)、key_track(高い音ほど明るい。0.5)、lfo1/lfo2(target: pitch でビブラート 0.05、cutoff でワウ、\n\
  amp でトレモロ、pan でオートパン、position は wavetable)。プリセット「ワイドなスーパーソー」「アナログベース」「プラック」\n\
  「ビンテージ・パッド」「ビブラートのリード」「動くウェーブテーブル・パッド」が手本。\n\
- **言葉から音色を作るなら design_sound**(「暗くて太いベース、少し揺れる」→ 内蔵音源のパッチ。新しいトラックか track_id で置き換え)。\n\
- **迷ったら mutate_sound**: 今の音色から互いに違う変種を言葉付きで出し、人に選んでもらう(apply で当てる。当ててから呼ぶと次の世代)。\n\
- **言葉の指示は set_character**: 「もう少し明るく・太く・遠く・柔らかく」は、どの音色でも共通の大きなつまみ\n\
  (brightness・body・motion・width・space・attack・grit、0〜100、50 = 作ったときの音)で 1〜2 個動かす。細かいつまみを探す前に使う。\n\
- fm: エレピ・ベル・マレット・FM ベースなど金属的・打鍵的な音。\n\
- 音を重ねるなら set_layer(本体 + 3 層。キックにサブ〈key_range 35-36〉、リード・コードに 1 オクターブ下のサイン、\n\
  強く弾いたときだけ鳴る層〈vel_range〉)。意味の取っ手は set_macro(「明るさ」= cutoff と reverb.mix など。値は macro/N)。\n\
- 動きのある音色は modulate(トラックの LFO。音源・エフェクトのつまみをテンポに合わせて揺らす): ワブルベース = wavetable の\n\
  position か subtractive の cutoff を 1/8〜1/16 の sine、うねるパッド = cutoff を 2/1 の triangle、ランダムに動く音色 = random。\n\
- wavetable: position を LFO やオートメーションで動かすウォブルベース・うねるパッド・母音のような音・sync のギラついたリード。\n\
- ワブル(subtractive・wavetable): 声ごとのモジュレーター mod1 / mod2 を使う。速さはテンポに合う拍あたりの回数(mod1_rate: 1/4 = 1、\n\
  1/8 = 2、1/8t = 3、1/16 = 4、1/16t = 6)、形 mod1_shape(wub・yoi・saw_down_curve・stairs・custom = mod1_points で描く)、\n\
  1 本から複数の行き先へ(mod1_cutoff・mod1_position・mod1_warp・mod1_pitch・mod1_res・mod1_drive・mod1_amp。−1〜1)。\n\
  ワブルの表情は「速さのリズム」: write_wobble で拍ごとに速さを切り替える(例 [1/8, 1/8, 1/16t, 1/4]、区間ごとに pattern を変える)。\n\
  さらに、ノートを刻む・音程を跳ばす(オクターブ・5 度)・2 本目の mod を別の速さで変形やピッチに送る・ドロップの後半で形を替える、で単調にしない。\n\
  retrig: note は音ごとに揺れ直す(刻むワブル)、song は曲の拍に固定(長い音で速さを替えるとき)。\n\
- fm4: 4 オペレーターの FM(8 アルゴリズム)。DX のエレピ(既定)・ベル・ブラス・オルガン・FM ベース。モジュレーターの level が明るさ、\n\
  その decay を短くすると頭だけ明るい打鍵の音。2 オペレーターの fm で足りなければこちら。\n\
- additive: 加算合成(部分音 最大 64 本)。tilt で明るさ、odd_even -1 でクラリネット風、formant_db・formant_hz で声のような母音、\n\
  damping で撥弦・打鍵の減衰、inharmonic でベル。澄んだパッドに向く(部分音が多いほど重い)。\n\
- granular: 取り込んだ音声から粒を切り出して重ねる。import_sample の instrument: \"granular\" で素材を入れ、position(どこを)・\n\
  grain_ms(粒の長さ)・density(1 秒の数)・spray_ms・pitch_rand・spread・scan(進める速さ)で、声を止めて伸ばす・きらめきの雲・ゆっくり移るパッド。\n\
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
  sampler のつまみ: loop(+ loop_start / loop_end / loop_xfade_ms)で押さえている間伸ばす、attack_ms / decay_ms / sustain で形、\n\
  filter_type・cutoff・vel_cutoff で明るさ、key_track: false でどの鍵盤も元の高さ、slices でチョップ(root から半音ずつ)、\n\
  orig_bpm にループ素材の元のテンポを入れると曲のテンポに合わせて伸縮。\n\
- 音声からウェーブテーブル: import_wavetable で声・楽器の 1 音・シンセの音を 2048 点 × N 枚のテーブルにして wavetable の table に\n\
  (時間変化を position で行き来する。pos_env・LFO で動かすとしゃべる・うねる音。配布形式のテーブル WAV もそのまま読める)。\n\
- ウェーブテーブルを作り込む: make_wavetable で 元(shape = 定番の変化 / harmonics = 倍音の設計図 / audio / library / current)→\n\
  edits(tilt・odd_even・band・saturate・fold・phase align・smooth・resize・mix など)。結果の summary(明るさの幅・急に変わる所・\n\
  直流)を見て、source: current でさらに直す。describe_wavetable で中身の要約、wavetable_library で棚(save / load / export)。\n\
  鳴らすときの変形は wavetable の warp(bend / squeeze / sync / mirror / quantize / fm)と warp_amount(modulate で動かすと\n\
  position とは別の軸でうねる)。グロウル = growl か fold を position と warp_amount の 2 本の LFO で、\n\
  リース = analog の saw 寄り + unison 2〜3・detune 10〜20 + 低めの cutoff。\n\
- リサンプリング: resample_to_sampler で、作ったトラック(の範囲)をエフェクト込みで描き出してサンプラーの音源にする\n\
  (フレーズを弾き直す・スライスして並べ替える・ループして伸ばす)。元と同じ大きさは強さ 127 で root を弾いたとき。\n\
- 音色プリセット: 音作りの依頼ではまず list_presets → load_preset → 微調整。良い音ができたら save_preset(全プロジェクト共通)。\n\
- エフェクトのプリセット: エフェクト 1 つ分(list_effect_presets → load_effect_preset)。エフェクトを足す前に使える設定がないか見る。\n\
  外してある(parked: true)エフェクトは鳴らないが、ユーザーが取っておいたもの。頼まれない限り消さない。",
    ),
    (
        "sound_design",
        "音作りの定石(役割ごとの作り方・重ね方・歪みの使い分けと確かめ方)",
        "- 役割から決める: ベース = 低域の芯(サイン・三角の基音)+ 中域の輪郭(倍音。小さいスピーカーで聞こえるのはこちら)/\n\
  リード = 中域 1〜4 kHz の存在感・動き(ビブラート・グライド)/ パッド = 広さと時間の変化(遅い立ち上がり・フィルタの揺れ・spread)/\n\
  プラック = 速い減衰(filter_decay + filter_env で頭だけ明るく)/ キー = 打鍵の頭と減衰(fm4 のエレピ・additive のベル)。\n\
- 低域はモノで 1 つだけ: 150 Hz 以下はベースかキックのどちらかが主役。重ねたベースの低域側の層は spread 0。サブベースはサイン 1 本。\n\
- 重ねる(set_layer): 頭の層(クリック・ノイズの短い音)+ 胴の層 + 尾の層(パッド・リバーブ)のように時間で分けるか、\n\
  低域・中域・高域で分ける。同じ帯域に同じ役割の層を重ねると濁るだけ。\n\
- 動き: 止まった音はプロっぽく聞こえない。analog 0.2〜0.4、ゆっくりした LFO(cutoff・position・pan)、modulate でテンポに合わせた揺れ、\n\
  ノートの強さで明るさ(vel_cutoff)。長い音はオートメーションで区間ごとに開閉する。\n\
- 歪みの使い分け: saturator(テープ・真空管の温かい偶数倍音。ベース・ボーカル・バスに薄く)/ distortion・clipper(硬い奇数倍音。\n\
  ロック・ダブステップのベース、ドラムバスの音圧)/ bitcrush(ローファイ・ゲーム機)/ amp(ギター)。歪ませる前に低域を\n\
  ハイパスで分け(低域は歪ませず中域だけ)、歪みの後に EQ で 3〜5 kHz の耳に痛い所と 8 kHz 以上のざらつきを整える。\n\
  和音・ベースと重なるパートを強く歪ませると相互変調で濁る(単音のリード・ベースの上の帯域に使う)。\n\
- 歪み・コンプ・リミッタの確かめ方: 掛けたら check_distortion。impact の crest_change_db・attack_change_db が −3 dB より下なら潰しすぎ\n\
  (打楽器は芯が無くなり、ベースは平たくなる)、high_change_db が +3 dB を超えたらざらつき、probe で入力の大きさごとの歪み率・\n\
  相互変調・押さえ込みを見て、トラックの実際のピーク(analyze_audio)の所で歪みすぎていないか。critique_mix の squashed_drums も見る。\n\
- 言葉の調整: 「明るく・太く・遠く・柔らかく」は set_character、細かい追い込みは refine_by_words(聴き比べながらつまみを決める)。\n\
  音色を数で確かめるなら analyze_sound、2 つの音を比べるなら compare_sounds、参考の音に寄せるなら match_sound。\n\
- 音色エディタ(人が開く画面)で作り込まれた音は、つまみ・手で描いた形(ウェーブテーブルの作り方・加算合成の partial_edits・\n\
  サンプラーの slice_points・SFZ の key_adjust)を壊さないよう、変えるつまみだけ set_param で動かす。",
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
- EDM の音作り: スーパーソウは subtractive の unison 5〜7 + detune。ポンピングは sidechain エフェクト(source にキックのトラック ID。\n\
  CLAP 音源のトラックや、ドラムをまとめたバスもキーにできる。release_ms = 60000/BPM/2)か shape_automation の pump。ダブステップのウォブルは write_wobble(声ごとのモジュレーターの速さを拍ごとに切り替える)。\n\
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
        "- 定番の手順は apply_recipe(send_reverb・send_delay・kick_bass・vocal_chain・supersaw・parallel_drums)で 1 回で組む。\n\
- 仕上げの前に critique_mix(低域の広がり・クリック・キックとベース・True Peak・モノ・刺さり・こもり・かぶり・止まった音)。\n\
  warn は fix の道具で直し、もう一度呼んで確かめる。\n\
- エフェクト: add_effect(eq / eq8 / dynamic_eq / resonance / deesser / compressor / multiband / transient / gate / limiter / width / virtual_bass / reverb / convolution(import_ir で) / distortion / saturation / amp / sidechain / delay / chorus / tape /\n\
  clipper / bitcrush / tremolo / phaser / flanger / trance_gate / auto_filter / volume_shaper / pitch_shift / harmonizer / pitch_correct)→\n\
  set_param(fx/<id>/<名前>)。マスターは add_master_effect / set_master_param。\n\
- 細かい帯域は eq8(8 バンド。b3_gain_db のように番号で選ぶ。range_db でその帯域が大きいときだけ動くダイナミック、stereo で mid / side だけ)。\n\
  歌のサ行の刺さりは deesser、かぶり・ノイズ・残響の尻尾は gate。倍音で太く温かくは saturation(tape / tube / transistor / soft_clip)。\n\
  コンプの癖は character(vca = バスのまとまり、fet = ドラム・歌を前へ、opto = なめらか)、頭の山を確実に取るなら lookahead_ms。\n\
  ディレイは sync(1/4・1/8d など)でテンポに合わせ、type で tape(揺れて丸く飽和)・bbd(暗く温かい)・multitap(刻み)。\n\
  リバーブの character は hall(長く広い)・chamber(密で明るい)・shimmer(オクターブ上へ昇る)も。\
  低域の濁りは low_mult 0.5〜0.8、長い残響の金属的な鳴きは modulation 0.2〜0.5、部屋の距離感は early 0.3〜0.6。\n\
  歌のハモりは harmonizer、音程補正は pitch_correct(key・scale を曲に合わせる。speed_ms 0〜5 でケロケロ)、音程の移調は pitch_shift。\n\
- distortion はシンセ・ドラム等の歪み。エレキギターの歪みは amp(instruments を参照)。\n\
- 空間: 複数のトラックに同じリバーブ・ディレイを掛けるなら、バス(add_track kind: \"bus\" + リバーブ mix 1.0)を作り、\n\
  各トラックから set_send で送る(トラックごとに挿すより空間がまとまり軽い)。\n\
- delay の time_ms: 4 分 = 60000/BPM、付点 8 分 = 45000/BPM。厚みと広がりは chorus。歌・リードには duck_db 3〜6(ダッキングディレイ)。\n\
- テンポに合わせて動かす(sync = 1/4・1/8d・1/8t など): ポンピングは volume_shaper(サイドチェイン無しで 4 分ごとに沈める。\n\
  ベース・パッド・コード)、パッドを刻むのは trance_gate、ビルドのハイパスや うねるベースは auto_filter(highpass で cutoff を\n\
  オートメーション、LFO は depth)、ファンクのオートワウは auto_filter bandpass + env_amount 2〜3、エレピの揺れは tremolo\n\
  (stereo 1 でオートパン)・phaser。質感: 音圧は clipper(ドラムバス・マスターの前に drive 2〜6)、ローファイ・ゲーム機は bitcrush、\n\
  80 年代のスネアは reverb の gate_ms 150〜300。\n\
- 歪み・コンプ・リミッタを掛けたら check_distortion で確かめる(エフェクトを外した音と比べたクレスト・音の頭・4 kHz 以上・\n\
  雑音っぽさの変化と、入力の大きさごとの歪み率・相互変調・押さえ込み)。crest_change_db が −3 dB より下・attack_change_db が\n\
  −3 dB より下なら潰しすぎ。和音のパート・ベースと重なるパートは imd_pct を低く(歪みは単音のリード・ベースの上の帯域に)。\n\
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
        "mastering",
        "マスタリング(音量・ピーク・釣り合いの仕上げと確かめ方)",
        "- 順番: ミックスを直してから(マスターで直さない)→ master_mix(EQ → コンプ → リミッタ。reference_file で参照曲に寄せる)→\n\
  compare_mix で前後 → critique_mix {genre} で目安と比べる → export_audio。\n\
- 音量: 配信は正規化される(Spotify・YouTube -14 LUFS、Apple Music -16)。-14 より大きいマスターは下げて再生されるだけで、\n\
  潰した分ダイナミクスを失う(analyze_audio の streaming)。クラブ用の EDM は -9〜-6 LUFS も使われるが、PLR が 7 を下回らない範囲で。\n\
  アコースティック・ジャズ・クラシック・映像は -18〜-14 LUFS で、ダイナミクスを残す。\n\
- ピーク: True Peak -1 dBTP 以下(export_audio のリミッタが保証)。analyze_audio の master_clip が出たら(マスターの最後のクリップ防止が\n\
  1 dB 以上押さえ込んでいる)、音量を下げるかリミッタで扱う。クリップ防止に頼ると歪む。\n\
- 潰し具合: plr_db・psr_min_db はおおむね 8 以上(ジャンルの目安は critique_mix の genre)。リミッタのゲインリダクションは 3〜4 dB まで、\n\
  それ以上要るならミックスの段でピークの元(キック・スネアの頭、ベースの低域)を整える。check_distortion をマスターに近いバスで使い、\n\
  クレストと音の頭の変化を見る。\n\
- 釣り合い: tonal_balance の slope_db_per_oct(ポップ・EDM はおおむね -4.5〜-3、アコースティックはもう少し暗い)と deviations\n\
  (±3 dB の出っ張り)。EQ は ±1〜2 dB の広い山で。低域は stereo.low_correlation が 1 近く(150 Hz 以下はモノ)。\n\
  mono_loudness_change_db が −3 より大きく下がるなら広げすぎ。\n\
- 聴き比べ: 音量をそろえて比べる(compare_mix の match_gain_db の分を戻す)。大きいほうが良く聞こえるのは錯覚。\n\
- 最後に: 曲の頭と終わりの無音・フェード、区間ごとの音量の推移(short_term_lufs)が計画どおりか、モノで聴いて主旋律が消えないか。",
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

/// ユーザーが書いた定石の置き場所(設定フォルダの guides。中の .md が `user:<ファイル名>` のトピックになる)
pub fn user_dir() -> std::path::PathBuf {
    crate::presets::default_dir()
        .parent()
        .map(|p| p.join("guides"))
        .unwrap_or_else(|| std::path::PathBuf::from("guides"))
}

/// ユーザーの定石の一覧(トピック名 `user:<ファイル名>`, 見出し = 最初の行の「# 」の後か、ファイル名)。名前順
pub fn user_topics(dir: &std::path::Path) -> Vec<(String, String)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<(String, String)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path
                .extension()
                .and_then(|x| x.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
                != Some("md")
            {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_owned();
            let text = std::fs::read_to_string(&path).ok()?;
            let title = text
                .lines()
                .find(|l| !l.trim().is_empty())
                .and_then(|l| l.trim().strip_prefix("# "))
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .unwrap_or(&stem)
                .to_owned();
            Some((format!("user:{stem}"), title))
        })
        .collect();
    out.sort();
    out
}

/// ユーザーの定石を読む(`user:<ファイル名>`。長すぎるものは 12,000 字で切る)
pub fn user_guide(dir: &std::path::Path, topic: &str) -> Option<String> {
    let stem = topic.strip_prefix("user:")?;
    // フォルダの外を読まない
    if stem.is_empty() || stem.contains(['/', '\\']) || stem.contains("..") {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(format!("{stem}.md"))).ok()?;
    Some(text.chars().take(12_000).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_short_and_lists_every_topic() {
        // 毎ターン送るので短く保つ(以前は instructions 約 2,000 字 + チャット 6,100 字)
        let n = CORE.chars().count();
        assert!(n <= 1_100, "CORE が長すぎます({n} 字)");
        for (name, _, body) in TOPICS {
            assert!(CORE.contains(name), "CORE に {name} が無い");
            assert!(!body.is_empty());
            assert!(guide(name).is_some());
        }
        assert!(guide("no_such").is_none());
    }

    #[test]
    fn user_guides_are_listed_and_read_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("glaux_guides_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("my_mix.md"),
            "# 自分のミックスの決まり\n- キックは -6 dB",
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), "見出しなし").unwrap();
        std::fs::write(dir.join("skip.txt"), "x").unwrap();
        let t = user_topics(&dir);
        assert_eq!(
            t,
            vec![
                (
                    "user:my_mix".to_owned(),
                    "自分のミックスの決まり".to_owned()
                ),
                ("user:notes".to_owned(), "notes".to_owned()),
            ]
        );
        assert!(user_guide(&dir, "user:my_mix").unwrap().contains("キック"));
        assert!(user_guide(&dir, "user:../x").is_none());
        assert!(user_guide(&dir, "my_mix").is_none());
        assert!(user_topics(&dir.join("none")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
