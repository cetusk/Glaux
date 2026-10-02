# メロディーセンス・32 分音符レベルの表現・変拍子の調査(2026-09-28)

段階 2 で伴奏(和音の積み方・ベース・ドラム・つなぎ)を道具で組めるようになった。問題意識は次のとおり。

- 伴奏は道具で洗練されるが、**メロディーの「センス」が上がらないと、伴奏を改善する効果は小さい**。
- 普段は使わないかもしれないが、**奏法・表現の能力としては備えておいた方がよいもの**がある。

そこで、次の 3 つを網羅的に調べた。

1. **メロディーセンスの強化**(3 章)
2. **32 分音符レベルの繊細な表現**(4 章)
3. **変拍子の技法**(5 章)

1 章に要旨、2 章に 3 つを横断した進め方の案を置く。3〜5 章は各調査の本文。数値には出典を付け、出典の無い値は「経験則」と書いた。

---

## 1. 要旨

**メロディー**
- 「センスのある旋律」の中身は、かなりの部分が**数えられる性質**に分けられる。
  - 短い動機が繰り返され、少しずつ変わる。順次進行が多い。
  - 大きい跳躍の約 72% は向きを変える(von Hippel & Huron 2000)。
  - 最高音は狙った場所で 1 回。句の終わりは長く伸び、下がる。
  - 強拍は和声音。ロックの歌では強勢の 22.8% が 8 分の裏へ食い、その 77% は 1・3 拍目の直前(Tan ら 2019)。
- 覚えやすさを予測するのは「**ありふれた輪郭 + どこかに珍しい点**」「**反復の多さ**」「**予測しやすさ**」。快さは「不確かさ」と「驚き」の組み合わせで決まる(Jakubowski 2017、Van Balen 2015、Cheung 2019)。
- LLM の失敗の型は、Glaux で見えた弱点と同じだった。動機を写すだけで展開しない。リズムが単調。形式を保てない。理論の知識は答えられるが(68.4%)、音符には適用できない(36.7%)(Zhou ら 2024)。
- うまくいった方法は、どれも「**下書き → 客観的な点検 → 修正 → 複数案から選ぶ**」の流れ(ByteComposer、ComposerX、Libretto)。伴奏と同じく、**LLM は意図(動機・形式・山の位置)だけを決め、展開と点検は道具が決定的に行う**のが効く。
- Temperley 2008 の確率モデルは、音域の分散 29.0・近さの分散 7.2・キーの分布だけで音ごとの「驚き」を計算できる。**学習データ無しで点検に使える**。

**32 分音符レベルの表現**
- 装飾・奏法の多くは、「本音の前後 30〜130ms に、決まった形で数個の音やピッチの動きを足す」ことで書ける。
- 研究の数値で既定値を決められる。
  - 前打音は 8 分の 17〜35%(60〜125ms)で、前の音から時間を取る。
  - トリルは 1 秒 8〜12 音(ピアノの平均 10.1)。
  - メロディーは伴奏より 20〜30ms 早い。
  - ばらした和音は 50〜250ms に広がる。
  - ビブラートは 5〜7Hz(J-POP の歌は平均 6.53Hz・幅 181 セント)。
  - J-POP のしゃくり・こぶし・フォールは平均約 0.22 秒。
- **今のスキーマで足りないもの**:
  - ビブラートを pitch_curve(8 点・線形)では約 0.36 秒までしか描けない。
  - フォール・ドイト・スウェルに要る、ノートごとの音量の曲線が無い。
  - 7 連・9 連は 960 PPQ で割り切れない(丸めの誤差を次の音へ回す必要がある)。
- 管のジャズ奏法(フォール・ドイト・スクープ)を**音に展開する DAW は少ない**(Dorico も再生に反映しない)ので、差別化になる。

**変拍子**
- 変拍子の大半は **2 と 3 のまとまりの足し算**で説明できる(7/8 = 2+2+3、9/8 のアクサク = 2+2+2+3、11/8 = 2+2+3+2+2)。
- 分子と分母だけでは、このまとまりは決まらない。**まとまりはユークリッドリズムでほぼ作れる**(E(3,7) = 223)。
- 「長い拍」はちょうど 1.5 倍ではない(実測の S:L の平均は 0.656、周期ごとに 0.54〜0.76)。
- 今の Glaux は、グルーブ・ドラム・伴奏の道具が **4/4 の 16 分を前提**にしている。7/8 で「1 小節を 16 等分」すると 1 ステップが 210 tick になり、16 分の格子(240 tick)から外れる。
- 変拍子のデータは少なく、ライセンスも同梱に向かない。型は**規則(まとまり + ユークリッド + 型のセル)で作る**のが Glaux に合う。

---

## 2. 進め方(案)

### 2.1 考え方

- **メロディーを最優先にする。** 伴奏をこれ以上磨くより、旋律の点検と展開の道具の方が曲の印象を大きく変える。
- 32 分音符レベルの表現のうち、**旋律に直接効くもの**(しゃくり・フォール・ビブラート・前打音・メロディーのリード)は、メロディーの道具と一緒に進める。
- 変拍子は「普段は使わないが備える」側。ただし拍子の**まとまりをスキーマに持たせる**のは、後から入れるほど直す箇所が増えるので、早めに入れた方がよい。
- これまでの段階 3(音作り: シンセの変調・レイヤー・エフェクト)と段階 4(SFZ の音源など)は、この後に回す。**ノートごとの音量・明るさの曲線**だけは表現に要るので、段階 3 から前倒しする。

### 2.2 段階の案

| 段階 | 中身 | ねらい |
|---|---|---|
| **M1. 旋律の点検と展開** | `critique_melody`(順次進行の割合・跳躍の後の戻り・頂点の配置・音域・強拍の和声音・リズムの単調さと使い回し・動機の反復率・句と息継ぎ・句の終わり・シンコペーション・Temperley の驚き)、`develop_motif`(動機 + 形式の型 sentence / period / AABA / loop + 和音への合わせ込み + 頂点 1 か所 + 先取りのシンコペーション)、ジャンルの旋律の型の表、手引きの旋律の工程(フックを先に → 展開 → 点検 → 2 案を聴き比べ) | 旋律の弱点を数値で知り、動機を展開する作業を道具に任せる |
| **M2. 複数案と旋律の表情** | `write_melody`(4 案を作り、点検の点数で上位 2 案を置く)、`pitch_gesture`(しゃくり・こぶし・スクープ・フォール・ドイト・ベンド)と pitch_curve の拡張(16 点・区間ごとの曲がり方)、ビブラートの引数(速さ・深さ・開始・フェード)、`add_ornament`(前打音・モルデント・ターン・トリル)、`melody_lead`、ノートごとの音量・明るさの曲線(段階 3 から前倒し) | 旋律を選べるようにし、歌・管・リードらしい表情を付ける |
| **M3. 伴奏とリズムの細部** | `strum_chord`(ストローク・ばらし)、`drum_rudiment`(フラム・ドラッグ・バズロール・ラチェット・ハットロール)、`articulate_notes`、`tremolo`、`glissando`、連符の引数(誤差を次の音へ回す)、`shape_phrase`(句の弧と終止のリタルダンド) | 32 分音符レベルの奏法を一通り備える |
| **M4. 変拍子** | 拍子にまとまりを持たせる(`TimeSigEvent.grouping`、既定値はユークリッド、FORMAT_VERSION 2)、16 分の格子 × まとまりの共通の関数、既存の道具(write_drums・write_chords・write_bassline・apply_groove・swing_notes・critique)をまとまりで動かす、`extend_bar`(1 拍足す・抜く)、`write_polyrhythm` / `write_polymeter`(euclid 付き)、`set_meter_feel`(アクサクの長短の比) | 変拍子とポリリズムを道具で扱えるようにする |
| (その後) | 旧段階 3(シンセの変調・レイヤー・エフェクト)、旧段階 4(SFZ・無料音源・参考曲の構成)、変拍子の低優先(メトリック・モジュレーション・ヘミオラ・ティハイ・拍の置き換え・MusicXML の書き出し)、歌詞の音節に合わせる | |

進め方は表の順(M1 → M2 → M3 → M4)とし、旧段階 3・4 は後ろへ回した。スキーマの変更(M2 のビブラート・pitch_curve・音量の曲線、
M4 の拍子のまとまりと FORMAT_VERSION 2)は進める。歌詞に合わせる機能と学習済みモデルは後回しにする。

### 2.3 効果の測り方

- 旋律用の依頼を足す(例:「J-POP のサビのメロディーを 8 小節、王道進行の上に」「ハウスのリードのフックを 2 小節で」)。
- `critique_melody` の数値(順次進行・戻り・反復率・驚き)を、段階の前後で比べる。
- 聴いた評価は、旋律だけを取り出して(伴奏は同じ)ブラインドで対比較する。観点は「覚えやすさ・歌いたくなるか・ジャンルらしさ」。

---

## 3. メロディーセンスの強化


調査日: 2026-09-28。対象: LLM が MCP の道具で旋律を書く Glaux。時間の単位は 4 分 = 960 tick(8 分 = 480、16 分 = 240、4/4 の 1 小節 = 3840)。

### 3.0 結論(先に)

- 「センスのある旋律」の中身は、かなりの部分が**数えられる性質**に分けられる。短い動機が繰り返され、少しずつ変わる。順次進行が多い。跳躍の後は戻る。最高音は狙った場所で 1 回だけ出す。句の終わりは長く伸ばし、下がる。強拍は和声音に置き、リズムは同じ型を使い回しつつ拍の手前に食う(シンコペーション)。予測しやすさと驚きの釣り合いも大事。
- 研究では、覚えやすさを予測するのは「**ありふれた輪郭 + どこかに珍しい点**」「**反復の多さ**」「**予測しやすさ**」だった(Jakubowski 2017、Van Balen 2015、Janssen 2017)。
- 今の LLM の失敗の型は、Glaux で試した人の感想と同じだった。動機をコピーするだけで展開しない。リズムが単調。形式(AB など)を保てない(Zhou ら 2024)。
- 対策は、伴奏の道具と同じ形がよい。**LLM は意図(動機・形式・山の位置)だけを決め、展開と点検は道具が決定的に行う。** 最初に作るべきは点検の道具 `critique_melody` と、動機を展開する道具 `develop_motif`。その次に `write_melody`(複数の案を作り、点検の点数で選ぶ)。

---

### 3.1 作曲理論・実践の知見

#### 3.1.1 動機(モチーフ)とその展開

| 手法 | 中身 | 出典 |
|---|---|---|
| 提示 → 反復 → 断片化 → 終止(sentence) | 基本の考え(2 小節)を示し、それを繰り返す(移調・変形してよい)。次に 1 小節ずつに細かく刻んで、終止へ加速する | [Open Music Theory: Phrase archetypes](https://viva.pressbooks.pub/openmusictheory/chapter/phrase-archetypes-unique-forms/)、[Geneseo: Sentences and Periods](https://milnepublishing.geneseo.edu/fundamentals-function-form/chapter/35-sentences-and-periods/) |
| 反復進行(sequence) | 同じ形を音階の度数でずらす。Glaux では `transform_notes` にすでにある | 同上 |
| リズムの変形 | 拡大・縮小、拍をずらす(displacement)、裏拍に食う | [Tan, Lustig & Temperley 2019](https://davidtemperley.com/wp-content/uploads/2019/04/tan-lustig-temperley.pdf) |
| 和声への合わせ込み | 同じリズムと輪郭のまま、次の和音の構成音へ音を寄せる。ポップスで最も多い「変奏」 | 経験則(Hooktheory の分析譜で頻出) |

**失敗の型**(LLM): 動機をそのまま写すだけで、新しい要素を足さない。AB 形式や小節の長さを保てない([Zhou ら 2024「Can LLMs Reason in Music?」](https://arxiv.org/html/2407.21531v1))。

#### 3.1.2 フレーズの構造

| 型 | 構造(8 小節の例) | 使いどころ |
|---|---|---|
| sentence | a(2) a'(2) 断片(1+1) 終止(2) | サビ・フックの推進力 |
| period(問いと答え) | 前楽節(4、半終止 = 5 度などで開いて終わる)+ 後楽節(4、同じ頭で始めて主音で閉じる) | A メロ、ヴァース |
| AABA / AAB | 同じ句 2 回 → 対比 → 戻る | ポップス・ジャズのスタンダード |
| call & response | 2 小節ごとに「問い」と「応え」(楽器の合いの手でもよい) | ファンク・ブルース・ゴスペル |

- 句の長さは 2 小節か 4 小節が基本。変えるときは意図して変える([Open Music Theory](https://viva.pressbooks.pub/openmusictheory/chapter/phrase-archetypes-unique-forms/))。
- 歌詞の行の音節数は、前の行とそろえる(Max Martin の「melodic math」。[American Songwriter](https://americansongwriter.com/all-those-pop-songs-you-love-thats-not-magic-thats-melodic-math-and-this-is-how-it-works/)、[Music Business Worldwide](https://www.musicbusinessworldwide.com/how-max-martins-songwriting-techniques-are-used-to-write-hit-after-hit-after-hit/))。同じ本の中では「サビは開始 50 秒以内」「部分は 3〜4 種まで」とも書かれている(業界の経験則)。

#### 3.1.3 輪郭

| 性質 | 数値・内容 | 出典 |
|---|---|---|
| 音が近い(pitch proximity) | 小さい音程ほど多い。音程の出現頻度は 2 半音で最大になり、大きくなるほど指数的に減る | Huron 2006 の 5 つの規則性、[Vos & Troost 1989(PMC の引用)](https://pmc.ncbi.nlm.nih.gov/articles/PMC5844974/) |
| 下がるときは順次(step declination) | 下行の順次進行は上行より多い | [Huron 2006(Pearce の書評)](https://www.marcus-pearce.com/assets/papers/huron06-review.pdf) |
| 同じ向きに続く(step inertia) | 順次進行は、向きを変えるより同じ向きに続くことが多い | 同上。[Chiu & Temperley 2024](https://journals.sagepub.com/doi/full/10.1177/20592043231225731) |
| 跳躍の後は戻る(gap-fill・post-skip reversal) | 大きい跳躍の約 **72%** は向きを変える。音域の端から中央へ戻る「回帰」でほぼ説明できる | [von Hippel & Huron 2000(要旨)](https://www.researchgate.net/publication/224982434_Why_Do_Skips_Precede_Reversals_The_Effect_of_Tessitura_on_Melodic_Structure) |
| 弧の形 | 約 1 万の句(5〜11 音)の **40%** が上がって下がる弧。句を合わせた曲全体も凸になる | [Huron 1996「The Melodic Arch」](https://www.researchgate.net/publication/239063783_The_Melodic_Arch_in_Western_Folksongs) |
| 句の終わり | 句の終わりの音は長く、下がって終わる。音楽・言語・鳥の歌に共通する | [Tierney, Russo & Patel 2011(PNAS)](https://www.pnas.org/doi/full/10.1073/pnas.1103882108) |
| 最高音は 1 回 | 定旋律(cantus firmus)の規則: 頂点は 1 回、順次進行が主体、跳躍の後は反対向きの順次進行 | [Open Music Theory: cantus firmus](https://elliotthauser.com/openmusictheory/cantusFirmus.html) |

- ポップスの補足: サビで**同じ高い音をフックとして何度も叩く**のはよくある(J-POP の定石「同じ音の連続」)。そのため「最高音は 1 回」は**区間をまたいだ頂点の配置**として見る。規則は「曲の最高音はサビ(山)で初めて出す」「ヴァースで最高音を使わない」とするのが実用的([うちやま作曲教室](https://sakkyoku.info/tips/how-to-make-sabi-melody/)、[96bit-music](https://96bit-music.com/composer-chorus/))。

#### 3.1.4 音域とテッシトゥーラ

- 旋律は狭い音域にとどまる。Temperley はエッセン民謡集から、曲ごとの中心音の平均を Ab4(MIDI 68)、中心の周りの分散を 29.0(標準偏差 約 5.4 半音)と推定した([Temperley 2008](https://davidtemperley.com/wp-content/uploads/2015/11/temperley-cs08.pdf))。
- 歌の旋律の音域は 1 オクターブ半(19 半音)以内に収める。サビの中心は、ヴァースより 2〜5 半音高くする(経験則。J-POP の「サビで最高音」と合わせて使う)。

#### 3.1.5 和声との関係

| 項目 | 内容 | 出典 |
|---|---|---|
| 強拍は和声音 | ビバップのスケールは、強拍に和声音が来るように経過音を 1 つ足したもの | [Learn Jazz Standards](https://www.learnjazzstandards.com/blog/learning-jazz/jazz-theory/use-bebop-scales-like-pro/)、[Wikipedia: Bebop scale](https://en.wikipedia.org/wiki/Bebop_scale) |
| 非和声音の置き方 | 経過音・刺繍音は弱拍に、倚音・掛留は強拍に置き、順次進行で解決する | [Chord recognition 論文の整理](https://arxiv.org/pdf/1810.10002) |
| ロックの「旋律と和声の離婚」 | ポップ・ロックでは、旋律が和音に従わないことが古典より多い(CoCoPops の 414 旋律で検証) | [Arthur & Condit-Schultz, ISMIR 2023](https://archives.ismir.net/ismir2023/paper/000027.pdf) |
| エンクロージャ・半音の接近 | 目標の和声音を上下から挟む・半音で近づく | [Wikipedia: Bebop scale](https://en.wikipedia.org/wiki/Bebop_scale) |
| ガイドトーン | 3 度と 7 度をつなぐ線を骨格にする | 同上。ジャズの教本の定番(経験則) |
| 製品の指標 | Hooktheory は「旋律が和音の外にある頻度」を緊張の指標にしている | [Hooktheory SongMetrics](https://www.hooktheory.com/song-metrics/about) |

- 強拍の和声音の割合は、ジャンルの目安を**経験則**として置く(ポップのサビで 60〜80%、ロック・R&B は 50% 程度でもよい、ジャズは 3 度・7 度を多く)。コーパスの数値は見つからなかった。

#### 3.1.6 リズム

- ロックの歌の旋律では、強勢のある音節の **22.8%** が 8 分の裏へずれている(シンコペーション率 SQ = 0.228)。19 世紀の歌曲では 0 だった。ずれのうち **77.2%** は、1 拍目・3 拍目の直前(8 分の 4・8 番目)に食う「先取り型」だった(ASQ = 0.772。76 曲中 66 曲で 0.5 超え)([Tan, Lustig & Temperley 2019](https://davidtemperley.com/wp-content/uploads/2019/04/tan-lustig-temperley.pdf))。
  → 旋律を「強拍にそろえて書いてから、一部を 8 分前へ食わせる」のは、研究と合った操作。
- 速い曲は 8 分の、遅い曲は 16 分のシンコペーションが多い(同上。16 分の SQ は 120 BPM 超で急に減る)。
- 休符・アウフタクト・同じリズム型の繰り返しは、コーパスの数値が無いので経験則として扱う。Savage ら 2015 は、繰り返しの形式・規則的なリズム・1 音節 1 音の歌い方を、世界の音楽に共通する傾向として挙げている([PNAS](https://www.pnas.org/doi/10.1073/pnas.1414495112))。

#### 3.1.7 フックとサビ

| 知見 | 出典 |
|---|---|
| サビの 3 要素は「高い音・伸ばす音・リフレイン」 | 亀田誠治([Wikipedia「サビ」](https://ja.wikipedia.org/wiki/%E3%82%B5%E3%83%93)。既存調査の再掲) |
| 最高音をサビに置く。同じ音の連続で覚えやすく。似た短い句を重ねる。跳躍で印象を作る。サブドミナントで始める | [うちやま作曲教室](https://sakkyoku.info/tips/how-to-make-sabi-melody/) |
| 句の頭に上行の 5 音(ラシドレミ など)を弱拍から入れる | [soublog](https://soublog-goodluck.com/teach-how-to-make-music-climax/) |
| 歌詞の繰り返しが多い曲ほど 1 位になりやすい(Billboard 1958〜2012) | [Nunes, Ordanini & Valsesia 2015](https://psycnet.apa.org/record/2015-00801-001) |
| トランスのフックは、ブレイクの前に単純化した形で予告する | 既存調査(MUSIC_EXPRESSION_RESEARCH)の再掲 |

#### 3.1.8 歌いやすさ(プロソディ)

- 強勢のある音節を強拍に、弱い音節を弱拍に置く。Pattison は、作詞の「規則」はこれ 1 つだけだと言う([Berklee Online](https://online.berklee.edu/takenote/prosody-in-music-and-songwriting/)、[Sound On Sound](https://www.soundonsound.com/techniques/pat-pattison-writing-better-lyrics))。
- 日本語は 1 モーラ 1 音が基本。長音・促音・撥音の扱いを選べるようにする(経験則)。
- 息継ぎのため、2〜4 小節ごとに 8 分以上の休符を置く(経験則。句の終わりを長くする傾向 = Tierney 2011 と合う)。

#### 3.1.9 ジャンルごとの旋律の語法

| ジャンル | 語法 | 出典 |
|---|---|---|
| EDM(ハウス・トランス・フューチャーベース) | 1〜2 小節の短い動機を 4〜8 回繰り返し、最後の 1 回だけ変える。16 分の裏に食う。音域は狭く、6〜9 半音 | 経験則 |
| トラップ | 自然的短音階・和声的短音階・フリギア。1〜4 小節の短いループ。ベル・フルート。808 は滑らせて第 2 の旋律にする | [Production Music Live](https://www.productionmusiclive.com/blogs/news/trap-beat-guide-melodies-3-essential-scales-for-making-melodies)、[Songen](https://songen.app/blog/how-to-make-trap-melodies/) |
| ローファイ | ペンタトニック + 7th・9th の和声音。少ない音数、後ろにずらす、スウィング | 経験則(既存調査のスウィング 55〜62% と合わせる) |
| ジャズのアドリブ(ビバップ) | 強拍に和声音。ビバップのスケール、エンクロージャ、半音の接近、ガイドトーン | [Learn Jazz Standards](https://www.learnjazzstandards.com/blog/learning-jazz/jazz-theory/use-bebop-scales-like-pro/)、[Jens Larsen](https://jenslarsen.nl/why-barry-harris-approach-is-so-much-better-than-bebop-scales/) |
| ファンクのホーン | 16 分の短いキメ。休符が多い。ユニゾン・オクターブ。1 拍目の強調。コール & レスポンス | 経験則(既存調査のファンクのリズムの項と合わせる) |
| J-POP | A メロ(低く・語り)→ B メロ(上昇・溜め)→ サビ(最高音・伸ばし・リフレイン)。頭サビ | [DTM Labo](https://dtm-labo.com/production/song-structure-how-to/)、[うちやま作曲教室](https://sakkyoku.info/tips/how-to-make-sabi-melody/) |

---

### 3.2 研究: 良さ・覚えやすさ・期待

| 研究 | 分かったこと(数値) | Glaux での使い道 |
|---|---|---|
| [Jakubowski ら 2017「Dissecting an earworm」](https://www.apa.org/pubs/journals/releases/aca-aca0000090.pdf) | 耳に残る曲 100 と、人気・様式をそろえた対照 100 を、83 の特徴で比べた。耳に残る曲は、**全体の輪郭がありふれている**、**折り返し点の間の傾きが珍しい**、**テンポが速い** | 輪郭は定番(弧・下降終わり)に。驚きは局所に 1 つ |
| [Van Balen ら 2015(Hooked)](https://archives.ismir.net/ismir2015/paper/000148.pdf) | キャッチーさを予測するのは、旋律の**反復の多さ**、ポップのコーパスに対する**型どおりさ**、歌の目立ち | 反復率と「ありふれ度」を点検に入れる |
| [Janssen ら 2017(民謡の変化)](https://pmc.ncbi.nlm.nih.gov/articles/PMC5403935/) | 変わらずに伝わる句は、短い(β = −0.30)、予測しやすい(情報量が低い。β = −0.24)、音程が小さい(β = 0.10)。曲の前の方の句ほど安定(β = −0.10)。反復は β = 0.09。説明できたのは全体の 5% だけ | 句は短く。フックは予測しやすく |
| [Cheung ら 2019(Current Biology)](https://www.cell.com/current-biology/fulltext/S0960-9822(19)31258-8) | 快さは、不確かさと驚きの**組み合わせ**で決まる(鞍の形)。予測が難しい文脈では驚きの少ない音が、予測しやすい文脈では驚く音が快い | 「予測しやすい動機の中に驚きを 1 つ」の根拠 |
| Huron 2006(ITPRA)([MTO の書評](https://mtosmt.org/issues/mto.09.15.3/mto.09.15.3.aversa.html)) | 期待の反応は 5 つ: 想像・緊張(出来事の前)、予測・反応・評価(後) | 山の前に「溜め」(緊張)を置く設計の根拠 |
| Narmour の含意実現モデル → [Schellenberg 1997 の簡略化](https://en.wikipedia.org/wiki/Implication-Realization) | 2 要因に減らせる。**近さ**(5 半音以下の音程の後は 3 半音以下が来る)と、**反転**(7 半音以上の跳躍の後は、向きを変えて小さい音程が来る) | 点検の「跳躍の後の戻り」はこの定義を使う |
| [Pearce 2018(IDyOM)](https://nyaspubs.onlinelibrary.wiley.com/doi/full/10.1111/nyas.13654) | コーパスから学んだ確率で、音ごとの情報量(驚き)とエントロピー(不確かさ)を出す。曲の中の反復も学ぶ | 驚きの量の点検。本格版は学習データが要る |
| [Temperley 2008](https://davidtemperley.com/wp-content/uploads/2015/11/temperley-cs08.pdf) | 旋律の確率 = 音域の分布 × 前の音との近さの分布 × キーの分布(RPK)。エッセンからの推定値は、中心音の平均 68・分散 13.2、**音域の分散 29.0**、**近さの分散 7.2**、長調の確率 0.88。長調の旋律の 18.4% は主音 | **コーパス無しで情報量を計算できる**。Glaux の驚きの点検に直接使える |
| [Tierney ら 2011](https://www.pnas.org/doi/full/10.1073/pnas.1103882108) | 弧・下降の輪郭、句の終わりが長い、小さい音程。運動(発声)の制約で説明できる | 句の終わりの点検 |
| [Savage ら 2015](https://www.pnas.org/doi/10.1073/pnas.1414495112) | 304 録音。絶対的な普遍性は無いが、統計的な普遍性は数十ある(繰り返しの形式、規則的なリズム、1 音節 1 音の歌い方など) | 反復と規則的なリズムは「どのジャンルでも安全」 |

**要するに**: 骨格は予測しやすく(順次進行・弧・反復・強拍の和声音)、局所に驚き(跳躍・非和声音・シンコペーション・珍しい傾き)を 1〜2 か所。AI の旋律は、この「驚きの置き場所」と「反復の設計」が欠けている。

---

### 3.3 AI のメロディー生成

#### 3.3.1 モデル(重み・データのライセンスは一次情報で確認)

| モデル | 中身 | 重み | データ・注意 |
|---|---|---|---|
| [ChatMusician](https://huggingface.co/m-a-p/ChatMusician/blob/main/README.md)(2024) | LLaMA2-7B を ABC 記譜で継続学習。MusicPile(4B トークン) | MIT(モデルカード) | 自分で弱点を書いている: 厳密な書式の指示しか通らない、幻覚、**学習データのかなりの部分がアイルランド民謡**、文脈内学習・思考の連鎖が弱い。重みの元は LLaMA2 なので、LLaMA2 の利用規約も確認が要る |
| [MuPT](https://arxiv.org/abs/2404.06393)(2024) | ABC の多トラック版 SMT-ABC で事前学習。8192 トークン | 途中の重みまで公開 | 反復の構造で GPT-4 を 17% 上回り、聴き比べで 79% が選んだ |
| [NotaGen](https://github.com/ElectricAlexis/NotaGen)(2025) | ABC で 160 万曲を事前学習し、古典 9 千曲で微調整。CLaMP-DPO(人のラベル無しの強化学習) | MIT | 古典向け。最大のモデルは GPU 24GB。盗用の疑いがある生成(類似度 0.95 超)を除いている |
| [MelodyT5](https://github.com/sanderwood/melodyt5)(ISMIR 2024) | ABC の encoder-decoder。生成・和声付け・旋律付け・変奏など 7 つのタスク。MelodyHub(26 万旋律) | MIT | 個々のデータのライセンスは README に無い |
| [SongComposer](https://arxiv.org/abs/2402.17645)(ACL 2025) | (歌詞, 音高, 長さ, 休符)の組で、歌詞と旋律を 1 語ずつそろえる | 論文を確認 | 歌詞 → 旋律で、音高分布の類似度が 50.75%(GPT-4 より約 15 ポイント上) |
| [ComposerX](https://arxiv.org/abs/2404.18081)(2024) | GPT-4 の 6 役の分業(リーダー・旋律・和声・楽器・レビュアー・編曲)。レビューと修正を繰り返す | 無し(手順) | 多声を ABC の直線の文字列で書くのが難しい |
| [ByteComposer](https://arxiv.org/abs/2402.17785)(2024) | 構想 → 下書き → **自己評価と修正**(音域外など客観的な誤りを直す)→ **複数案から美的に選ぶ**(報酬モデル型の Voter) | 論文は CC BY-NC-SA | 「初心者の作曲家並み」 |
| [Libretto](https://arxiv.org/abs/2606.22708)(2026) | LLM エージェントに、拍の位置・声部・小節を明示した文法と、コーパスで較正した 6 観点(リズム・和声・旋律・織り・形式・変奏)の測定を渡し、点検と修正を繰り返させる | 論文 | **Glaux の方針(道具 + 点検)とほぼ同じ発想** |
| [Anticipatory Music Transformer](https://huggingface.co/stanford-crfm/music-medium-800k)(Stanford) | 途中を埋める生成(infilling)ができる | Apache-2.0 | Lakh MIDI で学習。Hookpad Aria の元 |
| [skytnt/midi-model](https://huggingface.co/skytnt/midi-model) | 0.2B の MIDI 生成 | Apache-2.0 | 学習データの Los Angeles MIDI Dataset は [CC BY-NC-SA 4.0](https://huggingface.co/datasets/projectlosangeles/Los-Angeles-MIDI-Dataset)(非営利) |
| Music Transformer / MusicVAE(Magenta) | 旋律の 2 小節・16 小節ループ、補間 | Apache-2.0([Magenta](https://github.com/magenta/magenta/tree/main/magenta/models/music_vae)) | 古い。旋律の骨格の補間には使える |

#### 3.3.2 LLM の失敗の型(Glaux の観察と同じ)

[Zhou ら 2024](https://arxiv.org/html/2407.21531v1)の報告:
- 動機を**写すだけ**で新しい要素を足さない。AB 形式と小節の長さを保てない。
- 人の作品と比べて、**単調で単純なリズム**で、展開や変化が足りない。
- 構造の無い和声の繰り返し、キーから外れた音。
- 理論の知識は答えられる(GPT-4 は知識 68.4%)が、推論では使えない(36.7%)。人の評価は 10 点満点で 4〜6。

→ 「知識はあるが、実際の音符に適用できない」。そのため、**知識を道具の中の決まった手順にし、LLM には意図の選択だけさせる**のが効く。

#### 3.3.3 評価の指標

- [MusPy](https://muspy.readthedocs.io/en/latest/doc/metrics.html): 音高の範囲・使った音の数・音階内の割合・ピッチクラスのエントロピー・空の拍の割合・グルーヴの一貫性(1 − 隣り合う小節の打点のハミング距離の平均)。
- IDyOM・Temperley の情報量(驚き)。
- 反復の率(MuPT・ChatMusician の「反復の検出率」)。
- ByteComposer・ComposerX・Libretto に共通する手順: **下書き → 客観的な点検 → 修正 → 複数案から選ぶ**。

#### 3.3.4 LLM に旋律を良く書かせる手順(プロンプトだけでできる部分)

1. 先にフック(サビの 1〜2 小節)を書く。言葉で「リズム型・輪郭・頂点の位置」を宣言してから音符にする。
2. 形式(sentence / period / AABA)を選び、どの小節が動機のどの変形かを表にする。
3. ヴァースはフックから作る(低く、音を少なく、同じリズムの頭を使う)。
4. 点検にかけ、数値の指摘を直す。
5. 3〜4 案を作り、点数と聴いた印象で選ぶ。

---

### 3.4 製品: どう「センス」を担保しているか

| 製品 | 旋律の機能 | センスの担保のしかた | 出典 |
|---|---|---|---|
| Hookpad Aria(Hooktheory) | 続きを書く・途中を埋める・和音から旋律・旋律から和音 | Anticipatory MT(3.6 億パラメータ)を TheoryTab 5 万曲で微調整。**周りの文脈で条件づけ**。33k 人に 318k 案を出した。定量評価は無し | [論文](https://arxiv.org/html/2502.08122v1)、[Aria](https://www.hooktheory.com/hookpad/aria) |
| Captain Melody(Mixed In Key) | 「Idea」の箱。**順次進行と跳躍の割合**、向きと速さ、休符・3 連、**和声音(1・3・5)の割合**、6 度・7 度の不協和の量、箱をつないで対比 / 複製して最後だけ変える | 旋律を部品(リズム・動き・音の選び方・問いと答え)に分け、それぞれを引数にする。音を安定(青)・面白い(緑)・緊張(黄)・音階外(赤)で色分け | [Wiki](https://mixedinkey.com/captain-plugins/wiki/melody-generator/)、[製品](https://mixedinkey.com/captain-plugins/captain-melody/) |
| Scaler 3 | 「Motions」: プロの演奏の句(旋律・ベース・アルペジオ)を和音に合わせる。音域・密度・回転を変えられる | **人が弾いた句の素材集 + 和音への合わせ込み** | [Scaler 3](https://scalermusic.com/products/scaler-3/)、[Plugin Boutique](https://help.pluginboutique.com/hc/en-us/articles/35864679857684-What-s-new-in-Scaler-3) |
| Melody Sauce 2(EVAbeat) | 雰囲気・複雑さ・速さ・スウィング・シンコペーション。300 のスタイル設定 | 「句を組み立てるエンジン」+ ジャンルの設定 | [Plugin Boutique](https://www.pluginboutique.com/product/3-Studio-Tools/93-Music-Theory-Tools/8877-Melody-Sauce-2) |
| Orb Melody → LANDR Composer | 複雑さ・密度・同時発音・シンコペーション・人間味 | 抽象的なつまみ | [LANDR](https://www.landr.com/plugins/producer-suite-3) |
| Band-in-a-Box Soloist | 数百のスタイルでソロを作る | **プロの演奏家から集めた句のデータベース**を、和音・キー・テンポに合わせて選んでつなぐ | [PG Music マニュアル](https://www.pgmusic.com/manuals/bbw2024full/chapter9.htm)、[Wikipedia](https://en.wikipedia.org/wiki/Band-in-a-Box) |
| Ableton Live 12 MIDI Tools | Shape: 描いた輪郭 + 音域 + 細かさ・つなぎの確率・密度・揺らぎで旋律を作る。Seed: 範囲内のランダム。変形: Ornament・Recombine・Connect(隙間を埋める)・Span。**音階を有効にすると度数で動く** | 「輪郭を人が描き、細部を確率で」 | [マニュアル](https://www.ableton.com/en/live-manual/12/midi-tools/) |
| Logic Pro Session Players | ベース・鍵盤・ドラム。和音トラックに従う | 旋律の機能は無い(伴奏専用) | [Apple サポート](https://support.apple.com/guide/logicpro/session-players-overview-lgcpbf624405/mac) |
| Synthesizer V Studio 2 | 旋律は作らない。**歌い方**(ピッチの曲線・ビブラート・タイミング)を AI で作り直す | Glaux の `pitch_curve` の参考 | [Sound On Sound](https://www.soundonsound.com/reviews/dreamtonics-synthesizer-v-studio-2-pro)、[AI Retakes](https://manual.synthv.info/ai-functions/ai-retakes/) |
| Suno / Udio | 音声を直接作る(参考のみ。設計には使わない) | — | — |

**共通点**: センスは「(a) 人が作った句の素材」「(b) 旋律を部品に分けた引数」「(c) 和音への合わせ込み」「(d) 文脈で条件づけた学習モデル」のどれかで担保している。Glaux の方針(決定的・ライセンスが安全)に合うのは (b) + (c) で、Captain Melody の分け方が最も近い。(a) は素材の権利が要り、(d) は重みとデータのライセンスが要る。

---

### 3.5 Glaux への提案

方針: **LLM は意図(動機・形式・山の位置・ジャンル)を少数の引数で渡す → 道具が決まった手順で展開する → 点検が数値で返す → LLM が直すか、別の案を選ぶ。** 乱数を使う所は `seed` で決定的にする(原則 2)。ID はコマンドを作る側で作る。

#### 優先度 1: `critique_melody`(旋律の点検)

`critique_arrangement` の旋律版。`glaux-core` に `melody_critique.rs` を置く(UI にもオーディオにも依存しない)。

**引数**: `clip_id`(または `track_id` + `bars`)、`chords`(省略時は `analyze_harmony` の推定を使う)、`key`、`genre`(しきい値の組を切り替える。既定 `pop`)、`role`(`verse` / `chorus` / `hook` / `lead`。計画書から自動)。

**返す数値と、指摘を出すしきい値**(★ = 研究の数値、無印 = 経験則の初期値):

| 指標 | 計算 | 指摘(pop の既定) |
|---|---|---|
| `step_ratio` | 隣り合う音程が 2 半音以下の割合(同音は別に数える) | < 0.45 → 「跳びすぎ」、> 0.95 かつ音域 < 5 → 「平板」 |
| `leap_recovery` ★ | 7 半音以上の跳躍の後、向きを変えて、より小さい音程が来る割合(Schellenberg の定義) | < 0.6 → 警告(研究では約 72%) |
| `peak` | 最高音の位置・回数・区間。計画書の最も盛り上がる区間と照合する | 曲の最高音が山の区間より前に出ている / ヴァースの最高音 ≥ サビの最高音 → 警告 |
| `range` / `tessitura` | 音域(半音)、中央値の音 | 歌で > 19 → 警告、< 5 → 情報。サビの中央値 − ヴァースの中央値 < 2 → 「サビが上がっていない」 |
| `strong_chord_tone` | 1・3 拍目に始まる音のうち、和声音の割合。非和声音なら、次の音が順次で解決するかも見る | < 0.5 → 警告。解決しない強拍の非和声音 → 1 つずつ指摘 |
| `rhythm_variety` | 音価の種類の数、最も多い音価の割合 | 最も多い音価 > 80% → 「リズムが単調」 |
| `rhythm_reuse` | 小節の打点の型(16 分 × 16 のビット列)が他の小節と一致する割合 | < 0.3 → 「リズムの動機が無い」、> 0.9 → 「変化が無い」 |
| `motif_coverage` | (音程, 長さの比)の 3〜6 音の並びが 2 回以上出る部分に含まれる音の割合(移調しても同じと見なす) | < 0.3 → 「覚えにくい」、> 0.85 → 「繰り返しすぎ」(EDM・トラップは上限を 0.95 に) |
| `syncopation` ★ | 8 分の裏で始まり、次の拍まで次の音が無い音の割合(Tan らの SQ を歌詞なしで近似)と、そのうち 1・3 拍目の直前の割合(ASQ) | pop の目安は SQ 0.1〜0.35(ロックの平均 0.228)。0 → 「全部表拍」の情報 |
| `phrases` | 1 拍以上の休符(または長い音の後の 8 分以上の休符)で句に分ける。句の長さ(小節)と、頭の位置(アウフタクトか) | 4 小節を超えて休みが無い → 「息継ぎが無い」。全部の句が 1 拍目頭で同じ長さ → 情報 |
| `phrase_ending` ★ | 句の最後の音の長さ ÷ 句の音の長さの中央値、最後の 3 音の向き、最後の音の度数 | 長さの比 < 1.5 が多い → 「句の終わりが伸びない」。前楽節が主音で閉じている → 「問いが閉じている」 |
| `surprise` ★ | Temperley の RPK モデルで、音ごとの情報量 −log₂P。音域の分散 29.0・近さの分散 7.2・キーの分布を使う(**コーパス不要**)。句ごとの平均と最大 | 句の最大が低すぎる(驚きが無い)/ 平均が高すぎる(ばらばら)。4 小節に 1 つ以上「目立つ驚き」(情報量が上位 10% の音)を求める |
| `score` | 上の指標を 0〜100 にまとめる(重みは定数。ベンチマークで調整) | 複数の案から選ぶときに使う |

- 指摘には `critique_arrangement` と同じく**直し方の道具**を付ける。例: 「跳躍の後に戻っていない(小節 5)→ `develop_motif` の `fill` か、次の音を 2 度下へ」。
- `critique_arrangement` からも、旋律のトラック(役割が lead / vocal)で自動的に呼ぶ。
- 実装は数百行で済み、学習モデルも要らない。**最初にやる価値が最も高い**(LLM が自分の旋律の弱点を数値で知れる)。

#### 優先度 2: `develop_motif`(動機を展開する)

LLM が動機(1〜2 小節)を書き、道具が形式に沿って決定的に展開する。`transform_notes` の上に「和音への合わせ込み」と「形式の型」を足したもの。

**引数**:
- `motif`: 動機のクリップ ID。または文字列 `"E5:q D5:e C5:e D5:q G4:q"`(音名:長さ。度数表記 `"3:q 2:e 1:e"` も可)。
- `chords`: 進行(write_chords と同じ書き方)。`key`、`bar`、`track_id`。
- `plan`: 小節ごとの操作の列。例 `"a | a>adapt | frag(2) seq(-1) | cadence(1)"`。型の名前でもよい: `sentence`(既定)/ `period` / `aaba` / `aab` / `call_response` / `loop`(EDM・トラップ。最後だけ変える)。
- `peak`: 頂点の小節と音(例 `"7:A5"`)。省略時は区間の 60〜75% の位置に 1 回置く(Huron の弧。位置は経験則)。
- `ending`: `open`(2・5・7 度で終える)/ `closed`(1 度。既定は型に従う)。
- `anticipate`: 1・3 拍目の音を 8 分前へ食わせる割合 0〜1(既定 0.2。Tan らの 0.228 に合わせる)。
- `seed`。

**操作**(すべて音階の度数の空間で計算する):
`repeat` / `adapt`(リズムと輪郭を保ち、強拍の音を次の和音の最も近い和声音へ、弱拍の音は順次のつながりを保って寄せる)/ `seq(n)`(度数で n ずらす)/ `frag(k)`(最初か最後の k 拍だけ)/ `invert` / `retro` / `augment` / `diminish` / `displace(拍)` / `tail`(最後の音を変える。問い → 答え)/ `fill`(跳躍の後に逆向きの順次を足す)/ `cadence(度数)`(句の終わりを伸ばし、休符を置く)。

**内部の手順**: ① 動機を(度数・拍の位置・長さ)に直す → ② 計画の各小節に操作を当てる → ③ `adapt` で和音へ合わせる(強拍が非和声音なら、次の音で順次で解決できるときだけ残す)→ ④ 頂点の音を 1 か所に置き、他の小節がそれを超えないよう、オクターブや度数で下げる → ⑤ 句の終わりを伸ばし、8 分以上の休符を入れる → ⑥ `anticipate` の割合で食わせる → ⑦ `critique_melody` の結果を添えて返す。

#### 優先度 3: `write_melody`(複数の案を作り、点検で選ぶ)

動機も道具に作らせる入口。ByteComposer・Libretto の「複数案 → 点検 → 選ぶ」を 1 回の呼び出しにする。

**引数**: `track_id`、`chords`、`key`、`bar`、`role`(`verse` / `pre` / `chorus` / `hook` / `lead` / `solo`)、`genre`、`form`(既定は役割から: chorus → sentence、verse → period)、`range`(既定: 歌の verse `"C4-D5"`、chorus `"E4-G5"`。lead `"C5-C6"`)、`density`(1 小節の音の数の目安。既定は役割とジャンルから)、`rhythm`(動機のリズムを文字列で固定できる。write_bassline の pattern と同じ記法)、`contour`(`arch` / `rise` / `fall` / `valley` / `flat_hook`)、`skeleton`(任意。1 小節 1〜2 音の骨格 `"E5 D5 C5 B4"`。あれば装飾だけ道具がする)、`candidates`(既定 4)、`seed`。

**内部**: 動機を作る(リズムはジャンルのリズム語彙から選ぶ。強拍 = 和声音、弱拍 = 経過音・刺繍音、輪郭の型に沿わせる)→ `develop_motif` で展開 → `critique_melody` で点数をつける → 最も点数の高い案を置き、他の案は点数と要約を返す(LLM が `seed` を指定して選び直せる)。**「点検の点数が最良」≠「最良の曲」なので、既定は上位 2 案を別クリップにして聴き比べられるようにする**のがよい。

#### 優先度 4: ジャンルの旋律の型(`genre` の中身)

点検のしきい値と生成の既定値を、ジャンルごとの表で持つ(`glaux-core` の定数。AI 向けの説明は日本語)。

| genre | 音域 | step_ratio | リズムの語彙 | SQ | 反復の上限 | その他 |
|---|---|---|---|---|---|---|
| pop / jpop | 12〜17 | 0.55〜0.85 | 8 分主体 + 伸ばし | 0.1〜0.35 | 0.85 | サビで最高音・同音連打を許す |
| edm | 6〜10 | 0.4〜0.8 | 16 分の裏・シンコペーション | 0.2〜0.5 | 0.95 | 1〜2 小節の loop、最後だけ変える |
| trap | 5〜12 | 0.5〜0.9 | 8 分・3 連、休符多め | 0.1〜0.4 | 0.95 | 短音階・和声的短音階・フリギア |
| lofi | 7〜12 | 0.5〜0.85 | 少ない音、後ろにずらす | 0.1〜0.3 | 0.8 | ペンタ + 7 度・9 度 |
| jazz | 15〜24 | 0.6〜0.9 | 8 分の連続 | 0.1〜0.3 | 0.5 | 強拍の和声音 ≥ 0.7、エンクロージャ |
| funk | 5〜12 | 0.3〜0.7 | 16 分のキメ、休符 ≥ 40% | 0.3〜0.6 | 0.9 | 1 拍目の強調、コール & レスポンス |

(数値はすべて経験則の初期値。手元の評価で調整する。)

#### 優先度 5: 歌詞の音節に合わせる(`lyrics` 引数)

- `write_melody` / `develop_motif` に `lyrics` を足す。日本語はモーラ区切り(`"か/ぜ/の/な/か/で"`)、英語は強勢の印(`"WALK-ing in the RAIN"`)。
- 音の数を音節の数に合わせる(足りなければ音を分け、多ければ伸ばしでまとめる)。英語の強勢は強拍か、その 8 分前(先取り)に置く(Pattison、Tan ら)。行の音節数がそろわないときは指摘する(melodic math)。
- 歌詞を音に持たせる(`Note` に `lyric` を足す)と `Project` のスキーマが変わる。最初はマーカーか別のデータで持ち、スキーマの変更は後で判断する。

#### 優先度 6: ガイドトーン・骨格からの装飾(`skeleton` と `style: "bebop"`)

- ジャズ・ローファイ用: 和音ごとの 3 度・7 度を、動きが最小になるようにつなぐ線(write_chords の動的計画法を流用)を骨格にし、エンクロージャ・半音の接近・ビバップのスケールで埋める。
- ポップにも効く: LLM が「骨格の音だけ」を決め(得意)、装飾とリズムは道具が行う(苦手な部分)。

#### 優先度 7: 工程の案内(`guide.rs` に書く)

旋律の作業の順番を道具の説明に入れる: ① 計画書で山の区間を決める → ② サビのフック(1〜2 小節)を先に書く → ③ `develop_motif` でサビを展開 → ④ ヴァースはフックのリズムの頭から、低く・少なく作る(`seq(-2)`・`augment`)→ ⑤ `critique_melody` → ⑥ 2 案を聴き比べる。トランスなら、ブレイクの前に単純化したフックを予告する。

#### 後回し(今はやらない)

- 学習モデルの同梱(Anticipatory MT の途中を埋める生成など)。重みは Apache-2.0 だが、学習データ(Lakh MIDI)の元の曲の権利があいまいで、モデルも重い(3.6 億〜8 億パラメータ)。規則 + 点検で足りない点がはっきりしてから検討する。skytnt/midi-model は学習データが非営利限定なので避ける。
- IDyOM の本格版(コーパスで学ぶ)。まずは Temperley のパラメータ式で足りる。
- GPL のコードは写さない。

#### 上位 5 つのまとめ

1. **`critique_melody`**: 順次進行の割合・跳躍の後の戻り(しきい値 0.6、研究では約 72%)・頂点の配置・音域・強拍の和声音・リズムの単調さ・動機の反復率・句と息継ぎ・句の終わり・シンコペーション・驚き(Temperley の式)を数値で返し、直し方を添える。
2. **`develop_motif`**: 動機 + 形式の型(sentence / period / AABA / loop)+ 和音への合わせ込み(`adapt`)+ 頂点 1 か所 + 先取りのシンコペーション(既定 0.2)。
3. **`write_melody`**: 役割・ジャンル・輪郭・骨格から 4 案を作り、点検の点数で上位 2 案を置いて聴き比べさせる。
4. **ジャンルの旋律の型**: 点検のしきい値と生成の既定値を 1 つの表で持つ。
5. **歌詞の音節**: モーラ・強勢に合わせて音を分け、強勢を強拍か、その 8 分前に置く。

---

## 4. 32 分音符レベルの繊細な表現


調べた日: 2026-09-28。数値には出典を付けた。出典の無い値は「経験則」と書いた。

### 4.0 要旨

- 装飾・奏法の多くは「本音(もとの音)の前後 30〜130ms に、決まった形で数個の音やピッチの動きを足す」ことで書ける。どれも、少ない引数から決まった手順で展開できる。
- 研究で確かめられた数値のうち、使えるもの:
  - 前打音は 8 分の長さの 17〜35%(60〜125ms)。前の音から時間を取り、本音は拍の上に残る。
  - トリルは 1 秒に 8〜12 音(ピアノの平均は 10.1)。
  - メロディーは伴奏より 20〜30ms 早い。
  - ばらした和音は 50〜250ms に広がり、聞こえる拍の位置はその中にある。
  - ビブラートは 5〜7Hz。深さは歌で ±50〜90 セント、J-POP では平均の幅 181 セント。
  - 終止のリタルダンドは v(x) = [1 + (w^q − 1)x]^(1/q) の式で表せる。
- Glaux の今のスキーマで足りないもの:
  - ビブラートを pitch_curve(最大 8 点・線形)では描けない。1 秒 5.5Hz で約 22 点が要る。
  - フォールやスウェルに要る音量の曲線が無い。
  - 7 連・9 連は 960 PPQ で割り切れない。
  - 装飾音をまとまりとして持てない。

---

### 4.1 時間の物差し(120 BPM のとき)

| 音価 | tick | ms | 備考 |
|---|---|---|---|
| 16 分 | 240 | 125 | |
| 16 分 3 連(1 拍 6 つ) | 160 | 83.3 | |
| 32 分 | 120 | 62.5 | |
| 32 分 3 連(1 拍 12) | 80 | 41.7 | トラップのハット |
| 64 分 | 60 | 31.25 | |
| 5 連(1 拍 5 つ) | 192 | 100 | 割り切れる |
| 7 連(1 拍 7 つ) | **137.14** | 71.4 | **割り切れない** |
| 9 連(1 拍 9 つ) | **106.67** | 55.6 | **割り切れない** |
| Elektron の 1/80 | 48 | 25 | リトリガーの最速 |

- ms から tick への換算: tick = ms × BPM × 960 / 60000(120 BPM なら 1ms = 1.92 tick)。
- 960 = 2^6・3・5 なので、7 連と 9 連は割り切れない。四捨五入の誤差は最大 0.5 tick(0.26ms)で、耳には分からない。ただし、合計の長さがずれないよう、誤差を次の音へ回して配る必要がある。

**聞こえ方の境目**
- 100ms より短い間隔は、個々の音としてははっきり聞こえない。250ms 未満では、タイミングの弁別閾が Weber の法則に従わない(KTH の総説の脚注、[Friberg ら 2006](https://continuum-hypothesis.com/music/kth.pdf))。
- フラムの 2 打の間隔が 1ms 変わると、音色の違いとして聞こえ始める。打楽器奏者は 1ms 未満の精度でこれを操る([Wessel & Wright](https://arxiv.org/pdf/2010.01570))。

---

### 4.2 装飾・奏法の一覧と、MIDI での表し方

#### 4.2.1 クラシックの装飾

| 名前 | 楽譜上の定義 | 実際のタイミング・強さ | MIDI での表し方 |
|---|---|---|---|
| 短前打音(acciaccatura、斜線付き) | できるだけ速く、拍の前に弾く([Grace note](https://en.wikipedia.org/wiki/Grace_note)) | ピアノの実測では 8 分の長さの 17〜35%、60〜125ms。**前の区間から時間を取り、本音は動かない**。短い前打音はテンポに比例して縮む。長いものは、遅いテンポでは比例以上に長くなる([Windsor ら](https://www.mcg.uva.nl/mmm-2003/papers/wadht-1.pdf)) | 装飾音を本音の pos − g に置く(g = 8 分の 20%、45〜125ms に収める)。前の音が重なるなら短くする。強さは本音の 0.6〜0.8 倍(経験則) |
| 長前打音(appoggiatura) | 拍の上で弾き、本音の時間と強調を取る。古典派ではおおむね本音の半分([Appoggiatura](https://en.wikipedia.org/wiki/Appoggiatura)) | 前打音の方を強く、本音を弱く(倚音から解決へ) | 本音の pos に前打音を置き、長さは本音の 1/2(付点の音符なら 2/3)。本音はその後ろに縮める。強さは前打音 1.0、本音 0.7(経験則) |
| 複前打音(2〜3 音、Schleifer など) | 本音へ滑り込む短い音の列 | 前打音と同じ扱い(経験則) | 1 音あたり 40〜70ms で本音の前に並べる |
| トリル | 本音と上の隣の音を速く交互に。バロックと古典派では上の音から始める([Trill](https://en.wikipedia.org/wiki/Trill_(music))) | ピアノは平均 10.1 音/秒(左手 9.1、右手 11.1)。強さは感情でよく変わるが、速さはあまり変わらない([Han & Bresin 2019](https://zenodo.org/records/3743459))。Moore(1992)の実測は 12Hz。弦楽器は、遅い曲で 6.4、中くらいで 9.3、速い曲で 12.1Hz(同論文が引く Moelants 2004) | テンポの格子に揃えて、1 拍を n 分割する(n は 1 秒 8〜12 音になるよう選ぶ)。終わりに後打音(ターン型)を付けてもよい。強さは 1.0 / 0.85 を交互に、拍頭を少し強く(経験則) |
| モルデント / 逆モルデント | 本音 → 上(下)の音 → 本音の 3 音([Mordent](https://en.wikipedia.org/wiki/Mordent)) | トリルと同じ速さの 2 音と、残りを本音(経験則) | 最初の 2 音を各 50〜70ms にし、残りを本音にする |
| ターン(回音) | 上 → 本音 → 下 → 本音([Ornament](https://en.wikipedia.org/wiki/Ornament_(music))) | 記号が音の上にあれば音の頭で、音と音の間にあれば音の後半で弾く(経験則) | 4 音を 32 分か 6 連で並べる |
| トレモロ(単音・和音の交互) | 記号の斜線の数で細かさを指定する。「できるだけ速く」の指定もある | できるだけ速い場合、弦楽器で 1 秒 10〜12 音程度(上の Moelants を目安に) | 数えるトレモロは斜線の数で分割。数えないトレモロは 1 秒の音数で書く |
| グリッサンド(ピアノ・ハープ) | 音階を滑らせる | ピアノの白鍵なら、両端の間の白鍵を等間隔に並べる。弱く始めて少し強く(経験則) | 区間の中で等分に並べる。弦楽器やトロンボーンの連続的なものは pitch_curve で書く |
| アルペッジャンド(和音のばらし) | 低い音から順に弾く | 最初から最後の音まで 50〜250ms(図から読んだ値)。聞こえる拍の位置は、その中の 13〜78% の所か、最初の音から 1〜17ms 後([Fu ら 2015](https://www.cs.cmu.edu/~rbd/papers/Statistical-View-ISMIR-2015.pdf)) | 幅を 80〜200ms、拍の位置を幅の 0〜30% の所とする(拍より少し前から始める) |

#### 4.2.2 ピアノの声部のずれ

- **メロディーのリード**: 強調した声部は、他の声部より約 30ms 早く鳴る([Goebl 2001](https://iwk.mdw.ac.at/goebl/papers/Goebl_JASA2001_melodyLead.pdf))。
  - 鍵盤に指が触れる時点ではほぼ同時で、ハンマーの速さの違いから生じる(強い音ほどハンマーが早く弦に当たる)。
  - **Glaux への含意**: 音源にはハンマーの遅れが無いので、ピアノらしくするにはリードを書き込む必要がある。
  - KTH のモデルでは平均 20ms としている([Friberg ら 2006](https://continuum-hypothesis.com/music/kth.pdf))。20〜50ms とする報告もある(Palmer 1996。[Goebl ら OFAI 報告](https://ofai.at/papers/oefai-tr-2003-28.pdf)で引用)。
- **ベースの先行**: ベースが約 50ms 先に出ることもある。どちらのずれも強拍で大きい([TISMIR 2025](https://transactions.ismir.net/articles/10.5334/tismir.317))。
- **前打音のある旋律**: 本音は伴奏より 12ms 早かった([Windsor ら](https://www.mcg.uva.nl/mmm-2003/papers/wadht-1.pdf))。

#### 4.2.3 ギター

| 奏法 | 中身 | 数値 | MIDI での表し方 |
|---|---|---|---|
| ストローク | 弦を順に鳴らす | 1 回の振りの周期の約 11% で 6 弦を鳴らし終える(隣の弦との間は 2.2%)。周期 500ms なら 55ms・弦ごとに 11ms([US7420114](https://patents.google.com/patent/US7420114)。特許なので経験則扱い)。Ample Guitar の Strum Time は「最初の音から最後の音まで」の時間で指定する([Ample 取扱説明書](https://www.amplesound.net/en/Guitar_Strummer.pdf)) | 下げは低い弦から、上げは高い弦から。幅は 20〜60ms。上げは低い弦を省き、少し弱く |
| ハンマリング・プリング | 2 つ目の音を撥弦しないで鳴らす | Shreddage 3 は、ノートを重ねるとレガートになる(12 半音以内)([Shreddage 3 取扱説明書](https://impactsoundworks.com/docs/Shreddage%203%20Precision%20Free%20Manual.pdf)) | articulation = Legato、強さを 0.7 倍(経験則) |
| スライド | フレットを滑る | 50〜150ms で到達(経験則) | pitch_curve で、始まりの音から目標の音へ |
| チョーキング(ベンド)・戻し | 弦を押し上げる | 半音 6% 〜 4 半音 26% の周波数変化([String Theory, PLOS ONE](https://journals.plos.org/plosone/article?id=10.1371%2Fjournal.pone.0102088))。Glaux の Bend は全音を 0.22 秒で上げる | pitch_curve(0 → +200 セントを 150〜250ms)。先に上げておくプリベンドや、戻しも同じ曲線で書く |
| ビブラート | 弦を揺らす | 4〜7Hz・±20〜50 セント(経験則) | 4.2.6 のビブラートの引数で指定 |
| デッドノート・レイク | 音程の無いミュート音 | Shreddage は強さの範囲で切り替える(C-1 のとき 1〜59 がミュート、120〜126 がレイク)([同](https://impactsoundworks.com/docs/Shreddage%203%20Precision%20Free%20Manual.pdf)) | 新しい奏法の値が要る(pluck で減衰を極端に短くする) |
| トレモロピッキング | 同じ音を速く刻む | 16 分や 32 分、または 1 秒 12〜16 回(経験則) | 格子で並べ、強さは交互に 1.0 / 0.85 |

#### 4.2.4 ドラム

| 奏法 | 定義 | 数値 | MIDI での表し方 |
|---|---|---|---|
| フラム | 弱い装飾音 + 主音。装飾音は拍の格子に数えない([PAS 40 ルーディメント](https://pas.org/rudiments/)) | 間隔は 15〜35ms(経験則)。1ms の違いが音色として聞こえる([Wessel & Wright](https://arxiv.org/pdf/2010.01570)) | 装飾音を pos − 25ms に置き、強さは主音の 0.4〜0.55 倍 |
| ドラッグ・ラフ | 2 つ以上の装飾音(ダブルストローク)+ 主音([Nexus](https://www.nexuspercussion.com/2011/07/the-dragruff-dilemma/)) | 装飾音の間隔は 30〜50ms(経験則) | 装飾音 2 つを pos − 2d、pos − d に置く |
| ラタマキュー | ドラッグ + 16 分 3 連 + 8 分 | 格子どおり | 型として展開する |
| バズロール(プレスロール) | 1 打ごとに何度も跳ねさせる。回数は決まっていない([Drum roll](https://en.wikipedia.org/wiki/Drum_roll)) | 跳ねの間隔は 1 秒 20〜30 回(経験則) | 32 分か 64 分を並べ、強さを小さく揺らす(seed 付き) |
| シングルストロークロール | 左右交互に 1 打ずつ | 人の上限は約 22 打/秒(1 分 1,334 打、[Guinness](https://www.guinnessworldrecords.com/world-records/632413-most-single-stroke-roll-drumbeats-in-a-minute-using-drumsticks)) | 1 秒 22 打を超えたら警告する |
| ゴースト | 16 分の裏に置くごく弱い音 | 強さの中央値は 20〜36(Groove MIDI の集計。段階 1 のグルーブの調査) | 既存の add_ghost_notes |
| ラチェット・リトリガー | 1 ステップを細かく連打する | Elektron の RATE は 1/1〜1/80、LEN、VELOCITY CURVE は −128(消えていく)〜 127(大きくなる)([Digitakt 取扱説明書](https://www.manualslib.com/manual/1275776/Elektron-Digitakt.html?page=29)) | Elektron と同じ意味の引数で展開する |
| トラップのハットロール | 1/32、1/16T、1/32T を混ぜる | 強さは大きくしていくか小さくしていく([MusicRadar](https://www.musicradar.com/how-to/how-to-program-mixed-resolution-trap-style-hi-hat-patterns)) | 区間・分割・強さの傾き。音程の傾きを付けてもよい |

#### 4.2.5 管楽器のジャズ奏法([Dorico](https://archive.steinberg.help/dorico_pro/v2/en/dorico/topics/notation_reference/notation_reference_jazz_articulations_c.html)・[StaffPad](https://staffpad.zendesk.com/hc/en-us/articles/360002334437-Doits-Falls-Plops-and-Scoops))

| 奏法 | 位置 | MIDI での表し方(数値は経験則) |
|---|---|---|
| スクープ / リフト | 音の前に、下から入る | pitch_curve で −100〜−300 セントから 0 へ、60〜120ms |
| プロップ | 音の前に、上から落ちて入る | +300〜+700 セントから 0 へ、80〜150ms |
| ドイト | 音の後に、上へ抜ける | 音の終わり 150〜400ms で +300〜+1200 セント。音量も下げる(**音量の曲線が要る**) |
| フォール | 音の後に、下へ落ちる | ドイトの逆向き。短いものと長いものがある |
| シェイク | 唇で上の音と速く揺らす(3 度くらいまで) | トリルに似て、ピッチの幅が広い。pitch_curve で ±200〜400 セント、6〜8Hz |

- Dorico は「これらのジャズ奏法は再生には反映されない」と明記している。記号は描けても、音に展開する DAW は少ない。Glaux の差別化になる。

#### 4.2.6 歌(J-POP)とビブラート

- **歌唱テクニックの実測**: 産総研の AIST-SIDB(J-POP の歌手 24 人の歌い方を、14 人が真似て歌った 48 歌唱)の分析([山本ら 2023](https://staff.aist.go.jp/m.goto/PAPER/TIPSJ202310yamamoto.pdf))。
  - 1 回あたりの平均の長さ(総時間 ÷ 回数で計算):

    | テクニック | 平均の長さ |
    |---|---|
    | しゃくり | 0.22 秒 |
    | こぶし | 0.23 秒 |
    | フォール | 0.22 秒 |
    | ヒーカップ(しゃくり声) | 0.16 秒 |
    | ビブラート | 0.63 秒 |

  - こぶし・フォール・ヒーカップは短い音符に多く、しゃくりとビブラートは長い音符に多い。フォールは、次の低い音へ滑らかにつなぐのに使われることが多い。
  - ビブラートは、rate の平均 6.53Hz、extent の平均 181.3 セント(極大と極小の差)。歌手による差が大きく、7.2〜8.7Hz の速い人と、5.1〜5.7Hz の遅い人がいる。
- **定義**([同論文 表 2](https://staff.aist.go.jp/m.goto/PAPER/TIPSJ202310yamamoto.pdf)):
  - しゃくり: 音高を上へ連続して変える(スクープと同じ)。
  - こぶし: 音高を U 字 / 逆 U 字に変える。
  - フォール: 音高を下へ連続して変える。
- **ビブラートの数値**

| 対象 | 速さ | 深さ | 出典 |
|---|---|---|---|
| クラシックの歌 | 平均 6.0Hz(4.5〜6.5) | 平均 ±71 セント | Prame([Voice Science](https://www.voicescience.org/lexicon/vibrato/)) |
| J-POP の歌 | 平均 6.53Hz | 平均の幅 181 セント | [山本ら 2023](https://staff.aist.go.jp/m.goto/PAPER/TIPSJ202310yamamoto.pdf) |
| バイオリン | 5.7Hz(1 ポジション)〜 6.3Hz(5 ポジション) | 幅 40〜108 セント | [Allen, Geringer ら 2009](https://journals.sagepub.com/doi/abs/10.1177/1948499209OS-400103) |
| 歌の終わり | 音の終わりで速くなる | — | Prame 1994([Han & Bresin](https://zenodo.org/records/3743459) で引用) |
| Synthesizer V の既定 | 5.5Hz | 幅 1 半音 | 開始 0.25 秒、入りと抜けのフェード各 0.2 秒([SynthV 取扱説明書](https://sv1.docs.dreamtonics.com/en/synthv/advanced-usage/note-properties)) |
| Glaux の今の Vibrato | 5.5Hz | ±30 セント | 0.12 秒後から 0.25 秒で全深度(`crates/glaux-dsp/src/expr.rs`) |

#### 4.2.7 ジャズのスウィングとアンサンブルのずれ

- ドラマーのスウィング比は、テンポが遅いと 3.5:1、速いと 1:1。中〜速いテンポでは、短い方の音が約 100ms でほぼ一定になる([Friberg & Sundström 1999/2002](https://acoustics.org/pressroom/httpdocs/137th/friberg.html))。
- ソロ奏者は、表拍ではドラマーより遅れ、裏拍では揃う([KTH Ensemble swing](https://www.speech.kth.se/music/performance/Texts/ensemble_swing.htm))。
- **Glaux への含意**: スウィング比を固定の値にするより、「短い方の音を 100ms 以上」とテンポから決める方が自然。

---

### 4.3 演奏表現のモデル

| モデル | 中身・数値 |
|---|---|
| **KTH の規則(Director Musices)**([総説 2006](https://continuum-hypothesis.com/music/kth.pdf)) | 規則の例: 句の弧(Phrase arch)・終止のリタルダンド・高い音ほど強く(High loud)・長短の対比(Duration contrast: 短い音はより短く、長い音はより長く。100ms を下限とする)・上行で速く(Faster uphill)・2:1 の比を縮める(Double duration)・句読点(Punctuation: 小さな区切りの後に短い無音)・同じ音の連打の間に短い無音・全体のアーティキュレーション(k=1 で、100ms を超える音の長さを 0.75 倍)・和声と旋律の緊張・Ensemble swing・Melodic sync・ノイズ(白色 + 1/f)。規則ごとの量 k で組み合わせ、感情の表現も作れる |
| **終止のリタルダンド**([Friberg & Sundberg 1999](https://pubs.aip.org/asa/jasa/article-abstract/105/3/1469/558501/Does-music-performance-allude-to-locomotion-A?redirectedFrom=fulltext)) | v(x) = [1 + (w^q − 1)x]^(1/q)。x は区間の中の位置(0〜1)、w は最後のテンポの比、q は曲がり方(2 = 一定の制動力、3 = 一定の制動仕事率)。走っている人が止まる動きと同じ形([Honing 2004 による紹介](https://eprints.illc.uva.nl/id/eprint/119/1/PP-2004-08.text.pdf)) |
| **Todd のフレーズの弧**([Todd 1992](https://pubs.aip.org/asa/jasa/article/91/6/3540/968369/The-dynamics-of-dynamics-A-model-of-musical)) | 句の中でクレッシェンドとデクレッシェンドの山を作り、「速いほど強く、遅いほど弱く」テンポと強弱を連動させる |
| **Widmer の規則の発見**([Widmer 2002 JNMR ほか](https://doi.org/10.1177/102986490500900101)) | モーツァルト 13 曲の演奏から、音ごとの判断をよく当てる単純な規則を 17 個見つけた。その後の Basis Mixer は、楽譜の特徴の重み付き和で表情を予測する(コードは GPL-3.0 なので設計の参考だけにする) |
| **VirtuosoNet**([Jeong ら 2019](https://mac.kaist.ac.kr/pubs/JeongKwonNam-neurips2018.pdf)) | 音符・声部・拍・小節の階層の RNN と条件付き VAE。小節ごとにテンポと強弱を決めてから、音ごとに細かく直す。GitHub の jdasam/virtuosoNet にはライセンスが無く、使えない |
| **ScorePerformer**([Borovik & Viro 2023](https://archives.ismir.net/ismir2023/paper/000069.pdf)) | Transformer。表情のスタイルを細かく操作できる。リポジトリは CC BY-NC-SA 4.0 |
| **DExter / RenderBox**([DExter](https://arxiv.org/pdf/2406.14850)、[RenderBox](https://arxiv.org/abs/2502.07711)) | 拡散モデル。知覚の変数や文章で表情を条件付けする |

**データセット(ライセンスは一次情報で確認した)**

| データ | 中身 | ライセンス | Glaux での扱い |
|---|---|---|---|
| **Vienna 4x22**([mdw](https://datasets.mdw.ac.at/datasets/dataset/98ea25fa-2468-43ff-929b-3c926e163583)・[CPJKU/vienna4x22](https://github.com/CPJKU/vienna4x22)) | 22 人 × 4 曲。楽譜と音ごとに対応付けた演奏 | **CC BY 4.0** | **集計値の同梱に使える**(帰属表示)。メロディーのリード・和音のずれ・前打音を測れる |
| Groove MIDI | ドラム 13.6 時間 | CC BY 4.0 | 集計済み(段階 1 の grooves.json) |
| MAESTRO v3([Magenta](https://magenta.withgoogle.com/datasets/maestro)) | 198.7 時間。ピアノの MIDI と音声 | CC BY-NC-SA 4.0 | 手元での研究だけ。同梱しない |
| ASAP / (n)ASAP([GitHub](https://github.com/CPJKU/asap-dataset)) | 222 曲・1,068 演奏。音ごとの対応付け | CC BY-NC-SA 4.0 | 同上 |
| Batik-plays-Mozart([GitHub](https://github.com/huispaty/batik_plays_mozart)) | モーツァルトのソナタ 12 曲。音ごとの対応付け | CC BY-NC-SA 4.0 | 同上 |
| PianoCoRe(2026、[Zenodo](https://zenodo.org/records/19186016)) | 25 万演奏・2.2 万時間 | CC BY-NC-SA 4.0 | 同上 |
| ATEPP([GitHub](https://github.com/tangjjbetsy/ATEPP)) | 約 1,000 時間。市販の録音から自動で譜起こし | README は CC BY 4.0、LICENSE ファイルは CC0 | もとは市販の録音。同梱は避ける |
| partitura([GitHub](https://github.com/CPJKU/partitura)) | 演奏と楽譜の対応の読み込み(Python) | Apache-2.0 | 集計の道具に使える(製品には入れない) |

---

### 4.4 DAW・製品の実装例

| 製品 | 機能 | 引数 |
|---|---|---|
| Ableton Live 12 の MIDI Tools([取扱説明書](https://www.ableton.com/en/live-manual/12/midi-tools/)) | **Ornament**(Flam / Grace Notes) | Position(格子に対する %。正なら本音の頭を置き換え、負なら前に足す)・Velocity・Amount(数)・Pitch(High / Low / Same、半音か音階の 1 段)・Chance |
|  | **Strum** | Strum Low / High(±100% で最大 1 格子)・Tension(加速) |
|  | Span・Time Warp・Velocity Shaper | Span は Legato / Tenuto / Staccato + Offset + Variation。Time Warp は速さの曲線(最大 3 点)。Velocity Shaper は強さの曲線 |
| Ableton の MPE 編集([取扱説明書](https://www.ableton.com/en/live-manual/12/editing-mpe/)) | ノートごとの曲線 | Pitch・Slide・Pressure・Velocity・Release Velocity |
| FL Studio | **Flam**([取扱説明書](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll_flam.htm)) | Time(絶対時間かテンポ基準か)・Before・Velocity |
|  | **Strum**([取扱説明書](https://www.image-line.com/fl-studio-learning/fl-studio-online-manual/html/pianoroll_strum.htm)) | 開始時刻と強さのずらし量と加速(Tension)、終わりの揃え、Trigger ahead(拍をまたぐ)、向きの交互 |
|  | Articulate | 音の長さと隙間を少し揺らす |
| Cubase | Note Expression([Steinberg](https://archive.steinberg.help/cubase_pro_artist/v10/en/cubase_nuendo/topics/note_expression/note_expression_vst_3_controllers_c.html)) | ノートごとに Tuning・Volume・Pan などの曲線 |
|  | Expression Map | 奏法の切り替え |
| Logic・Studio One | Articulation Set([macProVideo](https://www.macprovideo.com/article/logic-pro/creating-articulation-sets-in-logic-pro-x))・Sound Variations(5.2 から、[VI-CONTROL](https://vi-control.net/community/threads/studio-one-5-2-is-here-with-articulation-management.106768/)) | 奏法の ID をノートに持たせる。途中から再生しても正しい奏法で鳴る |
| Elektron | Retrig | RATE・LEN・VELOCITY CURVE(上の表) |
| Synthesizer V | 音符ごとのビブラート・音程の移り | 上の表 |
|  | パラメータの曲線 | Loudness・Tension・Breathiness・Voicing・Gender([公式](https://sv2.docs.dreamtonics.com/en/parameters)) |
| VOCALOID | 曲線 | PIT・PBS・DYN・BRE・BRI・CLE・GEN・POR(ポルタメントの開始位置)・OPE([Vocaloid Wiki](https://vocaloid.fandom.com/wiki/Using_the_Parameters)) |
| Ample Guitar | Strummer | Strum Time(最初の音から最後の音まで)を、キースイッチの強さで操作。時間の揺れ・音の間隔の揺れ([取扱説明書](https://www.amplesound.net/en/Guitar_Strummer.pdf)) |
| Shreddage 3 | 強さの範囲とキースイッチで奏法を切り替え、ノートの重なりでレガート | 4.2.3 を参照 |
| MPE・CLAP | ノートごとの音程 | MPE の既定は ±48 半音([MPE 仕様](https://d30pueezughrda.cloudfront.net/campaigns/mpe/mpespec.pdf))。CLAP は TUNING・VOLUME・BRIGHTNESS など([clap/events.h](https://github.com/free-audio/clap/blob/main/include/clap/events.h)) |

- 製品に共通するのは、次の 3 つの形。
  - 装飾は「本音の頭を置き換えるか、前に足すか」と「格子に対する割合」で決める。
  - ばらしは「幅・向き・加速・強さの傾き」で決める。
  - 連打は「速さ・長さ・強さの曲線」で決める。
- Glaux の道具も、同じ意味の引数にすると AI にも人にも分かりやすい。

---

### 4.5 Glaux への提案

方針:
- 道具は、意図を表す少数の引数を受け取り、決まった手順で絶対値のノート・曲線に展開する。
- 揺れを入れるときは seed を必須にする。
- ms で指定した値は、そのときのテンポで tick に直す。
- 優先度 **A** = すぐ効く、**B** = 普段は使わないが備える。

#### 4.5.1 道具

| 優先 | 道具名案 | 引数と既定値 | 内部の計算 |
|---|---|---|---|
| **A** | `strum_chord` | notes、direction(down / up / alternate_by_beat)、span_ms(ギター 35、ピアノのばらし 120)、anchor(0〜1、既定 0.2)、tension(−1〜1)、vel_slope(既定 −0.08/音)、up_skip_low(上げで低い弦を省く、既定 true)、seed、jitter_ms(2) | 同時の音を音高順に並べる。i 番目の音の開始 = 拍 − anchor·span + span·f(i/(n−1), tension)。alternate は、表拍を下げ・裏拍を上げにする |
| **A** | `add_ornament` | notes、kind(acciaccatura / appoggiatura / double_grace / mordent / inverted_mordent / turn / trill / schleifer)、interval(scale 既定。または半音)、grace_ratio(8 分に対する割合、既定 0.2。45〜125ms に収める)、trill_rate(既定 10 音/秒。8〜12 に収める)、start(upper / main、既定は様式で決める)、ending(none / turn)、vel_ratio(0.7) | acciaccatura は前の区間に置き、前の音を短くする([Windsor ら](https://www.mcg.uva.nl/mmm-2003/papers/wadht-1.pdf))。appoggiatura は本音の 1/2 を取る。トリルの 1 拍の分割数 n = round(rate × 60 / BPM)。7 連・9 連のときは、丸めの誤差を次の音に回す |
| **A** | `drum_rudiment` | notes か区間、kind(flam / drag / ruff / ratamacue / buzz / roll / ratchet / hat_roll)、grace_ms(flam 25 / drag 40)、grace_vel(0.5)、rate(1/32・1/16T・1/32T・Hz)、length、vel_curve(−128〜127、Elektron と同じ意味)、pitch_ramp(セント)、seed | フラムは装飾音を pos − grace_ms に置く。ロールは区間を rate で埋め、vel_curve で強さを傾ける。1 秒 22 打を超えたら警告する |
| **A** | `set_vibrato`(スキーマ拡張 S1 とあわせて) | notes、rate_hz(5.5)、depth_cents(±30。歌は ±60、J-POP の強いものは ±90)、delay_ms(250)、fade_in_ms(200)、fade_out_ms(0)、rate_end_hz(省略可。終わりで速める) | 音源の側で正弦波として作る。CLAP には TUNING として 1 音ごとに送る。既定の数値は Synthesizer V と同じ形 |
| **A** | `pitch_gesture` | notes、kind(scoop / plop / doit / fall / shakuri / kobushi / bend / prebend_release / slide_in / shake)、amount_cents、time_ms、curve(ease_out など) | 種類ごとの既定値: しゃくりは −150 → 0 を 120ms、こぶしは 0 → +80 → 0 を 200ms、フォールは音の終わり 250ms で −700(以上は経験則、長さは[山本ら](https://staff.aist.go.jp/m.goto/PAPER/TIPSJ202310yamamoto.pdf)の約 0.22 秒に合わせる)、ベンドは +200 を 200ms。pitch_curve の点に展開する。doit と fall は、音量の曲線(S2)があれば音量も下げる |
| **A** | `melody_lead` | track か notes、lead_ms(既定 20、0〜40)、mode(fixed / by_velocity)、bass_anticipate_ms(0) | 各時点で最も高い音(か、指定した声部)を前へずらす。by_velocity では、強さの差に比例させる(Goebl のハンマーの遅れの再現) |
| B | `shape_phrase` | 区間、arc(0〜1)、final_ritard(w 既定 0.6、q 既定 2)、couple_dynamics(true) | テンポ = v(x) = [1 + (w^q − 1)x]^(1/q) を拍ごとのテンポ変化として書く。強さはテンポに連動させる(Todd)。w と arc の既定値は経験則 |
| B | `articulate_notes` | notes、style(legato / tenuto / staccato / portato)、ratio(スタッカートは 0.5)、repeat_gap_ms(30)、legato_overlap_ms(15) | KTH の Overall articulation(k=1 で 0.75)と、連打の間の短い無音に当たる |
| B | `tremolo` | notes、kind(single / alternating / chord)、division(1/32・1/64)か rate_hz(12)、vel_pattern | 格子で埋める。数えないトレモロは rate_hz から最も近い分割を選ぶ |
| B | `glissando` | from、to、区間、scale(white / black / chromatic / key)、curve、vel_ramp | 区間の中で等分に並べる。連続的なものは pitch_curve で書く |
| B | `tuplet`(ノートを書く道具の共通の引数) | span(拍)、count(5・6・7・9…) | 丸めの誤差を次の音へ回し、合計の長さを保つ |
| B | ジャズのスウィングの自動値 | swing_notes に mode = "jazz_tempo" | 短い方の音を max(100ms, 3 連) で決める([Friberg & Sundström](https://acoustics.org/pressroom/httpdocs/137th/friberg.html)) |

#### 4.5.2 モデル・スキーマの拡張

| 番号 | 拡張 | 理由・中身 |
|---|---|---|
| **S1(A)** | ノートに `vibrato: {rate_hz, depth_cents, delay_ms, fade_in_ms, fade_out_ms, rate_end_hz?}` | pitch_curve(8 点・線形)では、三角波で近似しても 2 周期(5.5Hz で約 0.36 秒)までしか描けない。パラメータで持てば軽く、AI も編集しやすい。Articulation::Vibrato は、既定値のこれと同じ意味にする |
| **S2(A)** | ノートごとの音量・明るさの曲線(段階 3 の予定を前倒し) | フォール・ドイト・スウェル・スフォルツァンドのほか、弓のトレモロの揺れに要る。CLAP の VOLUME と BRIGHTNESS へ送る |
| **S3(A)** | pitch_curve の点数を 8 → 16 にし、区間ごとの曲がり方(linear / ease_in / ease_out / hold)を足す | しゃくりやこぶしは 3〜4 点の曲線で書けるが、線形だと機械的に聞こえる。音源の側は固定長の配列のまま増やせる |
| **S4(B)** | ノートに `ornament_of: NoteId`(装飾音のまとまり) | 本音を動かしたとき、装飾も一緒に動かせる。消すのもまとめてできる。MusicXML と MIDI の書き出しで装飾音として出せる。決定的な ID はコマンドを作る側が作る |
| **S5(B)** | Articulation に DeadNote・Harmonic・Ghost を足す | 音源の側の対応が要る(pluck の減衰、倍音の強調、ドラムの弱打) |
| **S6(B)** | 連符の表記の情報(`tuplet: {n, in_beats}`) | 位置は tick のままでよい。楽譜の書き出しと、AI が読み返すときの手掛かりにする |
| S7(B) | ノートを離すときの強さ(release velocity) | CLAP と MPE にある。スタッカートの切れ方やピアノの離鍵に使う。後回しでよい |

#### 4.5.3 上位 5 つ(効果の大きい順)

1. **`strum_chord`**: ギター・ピアノのコードが一度に「ジャーン」と鳴る機械っぽさが、いちばん目立つ。すぐ直せる。
2. **`drum_rudiment`**(フラム・ドラッグ・ロール・ラチェット・ハットロール): write_drums のロールを一般化する。Elektron と Ableton と同じ意味の引数にする。
3. **ノートのビブラートの引数(S1)+ `set_vibrato`**: 今の固定の ±30 セントを、歌・弦・ギターに合わせて変えられるようにする。
4. **`pitch_gesture` + pitch_curve の曲がり方(S3)**(しゃくり・スクープ・フォール・ベンド): J-POP の歌メロとジャズの管らしさに効く。Dorico でも再生されない部分なので、差別化になる。
5. **`add_ornament`**(前打音・トリル・モルデント・ターン): 研究の数値(前打音は 8 分の 20%、トリルは 1 秒 10 音)で、既定値の根拠がはっきりしている。

### 4.6 未確認・経験則の点(採用の前に確かめる)

- 次の値は経験則。Vienna 4x22(CC BY)や自前の録音で測って直すとよい。
  - フラムの間隔 25ms、ドラッグの間隔
  - ストロークの幅(特許の値)
  - スクープ・フォール・ドイトの深さと長さ
  - しゃくりとこぶしの深さ
- KTH の規則の既定の量(k)は、総説に数値の一覧が無かった。規則ごとの論文を当たる必要がある。
- CC BY-NC-SA のデータから集計した「数値だけ」を同梱してよいかは、判断を保留した。同梱は CC BY のものに限るのが安全。

---

## 5. 変拍子と拍子まわりのリズムの技法


PPQ 960(4 分 = 960 tick、8 分 = 480、16 分 = 240)。「経験則」と書いたものは出典で確かめていない目安。

### 5.0 要点

- 変拍子の大半は **2 と 3 のまとまりの足し算**(加算リズム)で説明できる。7/8 = 2+2+3 / 3+2+2 / 2+3+2、9/8 のアクサク = 2+2+2+3、11/8 のコパニツァ = 2+2+3+2+2。拍子記号の分子と分母だけでは、このまとまりは決まらない。
- まとまりは **ユークリッドリズム(Toussaint)でほぼ作れる**。E(3,7) = 223(ルチェニツァ)、E(4,9) = 2223(トルコのアクサク)、E(5,11) = 22223(回すとコパニツァ)。まとまりが指定されていないときの既定値を決める規則として使える。
- 実際の演奏の「長い拍」は、ちょうど 1.5 倍とは限らない。測った研究では、短い拍と長い拍の比 S:L の平均は 0.656(2:3 = 0.667 に近い)で、周期ごとに 0.54〜0.76 まで揺れる(遅いトランシルヴァニアの曲)。ブルガリアの録音 11 本では比が決まった値にならない(Moelants 2006)。
- DAW では Logic と Dorico と MuseScore と Cubase がまとまりを持てる(Cubase は譜面の表示だけ)。Ableton と SMF(MIDI ファイル)は分子と分母しか持たない。SMF の分母は 2 の累乗しか表せず、まとまりも表せない。MusicXML は `<beats>3+2</beats>` で表せる。
- Glaux への一番の提案は、`TimeSigEvent` に `grouping`(例 [2,2,3])を足し、すべての道具が「16 分の格子 × まとまり」から拍を引くようにすること。今の「4/4 の 16 ステップを 1 小節に等分」は、7/8 では 1 ステップが 210 tick になり、16 分の格子から外れる。

### 5.1 小節の長さ(tick)

| 拍子 | 1 小節の tick | 16 分のステップ数 | よくあるまとまり(8 分単位) |
|---|---|---|---|
| 5/8 | 2400 | 10 | 2+3 / 3+2 |
| 6/8 | 2880 | 12 | 3+3 |
| 7/8 | 3360 | 14 | 2+2+3 / 3+2+2 / 2+3+2 |
| 9/8 | 4320 | 18 | 3+3+3(複合)/ 2+2+2+3(アクサク) |
| 10/8 | 4800 | 20 | 3+2+2+3(トルコのジュルジュナ) |
| 11/8 | 5280 | 22 | 2+2+3+2+2 ほか 9 通り |
| 12/8 | 5760 | 24 | 3+3+3+3 |
| 13/8 | 6240 | 26 | 2+2+2+2+2+3 など |
| 15/8 | 7200 | 30 | 2+2+2+2+2+2+3 など |
| 5/4 | 4800 | 20 | 3+2(4 分単位) |
| 7/4 | 6720 | 28 | 4+3 / 2+2+3(4 分単位) |

(tick とステップ数は PPQ 960 からの計算。まとまりの出典は 5.2)

### 5.2 技法の一覧

#### 5.2.1 奇数拍子と拍のまとまり(加算リズム・アクサク)

- **定義**: 等しくない長さの拍(2 と 3)が並ぶ拍子。アクサク(トルコ語で「足を引きずる」)と呼ばれる。London は、細かい単位は等間隔で、拍は 2 か 3 の単位からなる、と整理した。拍の間隔が 100ms より長く 1.5〜2 秒より短いことを拍の条件にしている([London, Hearing in Time](https://academic.oup.com/book/11161/chapter-abstract/159625714)、[MTO Clayton 2020](https://mtosmt.org/issues/mto.20.26.1/mto.20.26.1.clayton.html))。
- **数え方**: 2 は「1-と」、3 は「1-と-た」。7/8 の 2+2+3 は「クイック・クイック・スロー」。

| 拍子 | まとまり | 代表曲・舞曲 | 出典 |
|---|---|---|---|
| 5/4 | 3+2 | Take Five(Brubeck, 1959)、Mars(Holst)、Mission: Impossible、15 Step(Radiohead) | [Quintuple meter](https://en.wikipedia.org/wiki/Quintuple_meter) |
| 5/8 | 2+3 | パイドゥシュコ(ブルガリア)、Tchaikovsky 交響曲 6 番 2 楽章 | 同上、[Toussaint](https://cgm.cs.mcgill.ca/~godfried/publications/banff-extended.pdf) |
| 7/8 | 2+2+3 | ルチェニツァ(ブルガリア)、Unsquare Dance(Brubeck) | [Septuple meter](https://en.wikipedia.org/wiki/Septuple_meter) |
| 7/8 | 3+2+2 | ピリン地方のルチェニツァ。カラマティアノス(ギリシャの 7/8。まとまりは出典で未確認) | 同上 |
| 7/4 | — | Money(Pink Floyd)、Solsbury Hill(Peter Gabriel)、All You Need Is Love の A メロ | 同上、[名古屋作曲の会](https://nu-composers.hateblo.jp/entry/2021/11/28/190000) |
| 9/8 | 2+2+2+3 | トルコのアクサク、ブルガリアのダイチョヴォ、Rondo alla Turca(Brubeck) | [Toussaint](https://cgm.cs.mcgill.ca/~godfried/publications/banff-extended.pdf) |
| 10/8 | 3+2+2+3 | トルコのウスル「ジュルジュナ」 | [Holzapfel ほか 2014](https://archives.ismir.net/ismir2014/paper/000265.pdf) |
| 11/8 | 2+2+3+2+2 | コパニツァ(ブルガリア)。The Eleven(Grateful Dead) | [Undecuple meter](https://en.wikipedia.org/wiki/Undecuple_meter) |
| 11/4 | — | Eleven Four(Brubeck 四重奏団)、キッズ・ノーリターン(相対性理論) | 同上、[名古屋作曲の会](https://nu-composers.hateblo.jp/entry/2021/11/28/190000) |
| 13/8 | 2+2+2+2+2+3 | Krivo Plovdivsko Horo(ブルガリア)、Mama Cone pita(マケドニア) | [Toussaint](https://cgm.cs.mcgill.ca/~godfried/publications/banff-extended.pdf) |
| 15/8 | 2+2+2+2+2+2+3 | ブルガリアのリズム(回した形) | 同上 |

- **テンポ**: ブルガリアの速い舞曲は 1 周期がおよそ 1〜2 秒([Moelants 2006 の要約](https://www.researchgate.net/publication/258173053_Perception_and_performance_of_aksak_metres))。7/8 で 1 周 1.2 秒なら 8 分 ≈ 170ms。London の下限(100ms)から、8 分は最大で毎分 600 程度まで。

#### 5.2.2 複合拍子(6/8・12/8)と 12/8 のベル

- 6/8 は 3+3(大きな拍が 2 つ)、3/4 は 2+2+2(大きな拍が 3 つ)。どちらも 8 分 6 つだが、アクセントの位置が違う([Hemiola](https://en.wikipedia.org/wiki/Hemiola))。
- **12/8 のスタンダードベル**(ベンベ): `X.X.XX.X.X.X` の 7 打。ユークリッドの E(7,12) = 2122122 を回した形([Bell pattern](https://en.wikipedia.org/wiki/Bell_pattern)、[Toussaint](https://cgm.cs.mcgill.ca/~godfried/publications/banff-extended.pdf))。
- **ソン・クラーベ 3-2**: 16 分で `X..X..X...X.X...`。「3 対 2 のクロスリズムの 12/8 のセルに対応する形」とされる([Bell pattern](https://en.wikipedia.org/wiki/Bell_pattern)、[Puget Sound 教科書](https://musictheory.pugetsound.edu/mt21c/ThreeTwoClave.html))。
- マリのマンデの太鼓は、拍の中の 3 つの細かい単位が等しくない。L:S:S = 41:31:28、2 つに分ける曲では L:S = 59:41。テンポや奏者によらずほぼ一定だった([Polak & London 2014, MTO](https://mtosmt.org/issues/mto.14.20.1/mto.14.20.1.polak-london.html))。12/8 を「均等な 3 連」で打つと平らに聞こえる理由になる。

#### 5.2.3 拍子の切り替え(混合拍子)と「1 拍足す・抜く」

- 小節ごとに拍子を変える。The Ocean(Led Zeppelin)は 4/4 と 7/8 を 1 小節ずつ交互、Schism(Tool)のヴァースは 5/8 と 7/8 を交互([Septuple meter](https://en.wikipedia.org/wiki/Septuple_meter))。
- ポップスでは、既存の拍子に「拍を足したり引いたりする」のが基本([SoundQuest](https://soundquest.jp/quest/rhythm/rhythm-mv2/irregular-times/))。B メロの最後に 2/4 を 1 小節だけ挟んでサビを遅らせる、または 1 拍抜いてサビに前のめりで入る、は J-POP でよく使われる(経験則。個別の曲は出典で確かめていない)。
- Dorico は、小節ごとに交互になる拍子(6/8+3/4)と、1 小節の中に 2 つ以上の拍子を並べる拍子(2/4+3/8+5/4)を区別している([Dorico ブログ](https://blog.dorico.com/2020/09/tip-create-advanced-time-signatures-with-the-popover/))。

#### 5.2.4 ポリリズム・クロスリズム・ヘミオラ

- **ポリリズム**: 同じ長さの中に、違う数の等間隔の打点を重ねる(3 対 2、4 対 3、5 対 4)。小節の頭はそろう。
- **ヘミオラ**: 3 対 2。3 拍子の 2 小節を 2 拍子の 3 小節のように弾く。バロックでは終止の直前に使うのが定番([Hemiola](https://en.wikipedia.org/wiki/Hemiola))。
- **トラップの 3 連のフロウ**: 4/4 の上に 1 拍 3 音。Duinker は 50 曲を調べ、混合・フレーズ単位・全体の 3 種類に分けた([Duinker 2019, Popular Music](https://www.cambridge.org/core/journals/popular-music/article/abs/good-things-come-in-threes-triplet-flow-in-recent-hiphop-music/99BC46987A0BF369A0A3CCFC54F1CDBB))。

#### 5.2.5 ポリメーター(周期の違う型の重ね)

- 拍の長さは同じで、型の周期が違う。小節の頭がそろわず、最小公倍数のステップで元に戻る。Elektron は「トラックごとの長さ」と「全体の長さ(M.LEN)」で作る([Digitakt マニュアル](https://www.manualslib.com/manual/1275776/Elektron-Digitakt.html?page=32)、[Elektronauts](https://www.elektronauts.com/t/master-length-and-change-length/144132))。
- **Meshuggah「Bleed」**: 手は 4/4 を刻み、足は別の周期の型を踏む([Louder](https://www.loudersound.com/bands-artists/meshuggah-bleed-story-behind-the-song)、[Premier Guitar](https://www.premierguitar.com/obsessive-progressive-how-to-decode-advanced-polymeters))。Djent・マスロックの典型は「スネアは 4/4 の 3 拍目に固定して、ギターとキックは長い奇数周期」(経験則)。
- Ableton では、クリップのループ長を 5/16・7/16 などにずらして作る([MusicRadar](https://www.musicradar.com/how-to/how-to-perfect-your-polyrhythms-in-ableton-live))。

#### 5.2.6 拍の錯覚(4/4 の中の 3-3-2)と拍の置き換え

- **3+3+2(トレシージョ)**: 8 分 8 つ(または 16 分 8 つ)を 3+3+2 に分ける。E(3,8)。Eye of the Tiger のイントロ、Girls Just Want to Have Fun のギターリフ([Open Music Theory](https://viva.pressbooks.pub/openmusictheory/chapter/rhythm-and-meter-in-pop-music/)、[Tresillo](https://en.wikipedia.org/wiki/Tresillo_(rhythm)))。
- **ダブル・トレシージョ**: 16 分 16 個を 3-3-3-3-2-2([Tresillo](https://en.wikipedia.org/wiki/Tresillo_(rhythm)))。EDM でよく使う E(5,16) = 33334 もある([Toussaint](https://cgm.cs.mcgill.ca/~godfried/publications/banff-extended.pdf))。
- **拍の置き換え(displacement)**: 同じ型を 8 分や 16 分だけずらす。4/4 のまま拍の頭が動いたように聞こえる(定義は一般的な用語。数値の出典なし)。

#### 5.2.7 メトリック・モジュレーション

- 前の区間の音符の長さを、後の区間の別の音符に読み替えてテンポを変える。**新テンポ = 旧テンポ × (新しい小節の中の軸の音符の数 ÷ 古い小節の中の数)**。例: 4 分 = 84 で、2 分音符 2 つ分を新しい区間の 2 分音符 3 つ分とすると 126。Carter のチェロ・ソナタ(1948)が始まり。Björk「Desired Constellation」は「付点 4 分 = 2 分」([Metric modulation](https://en.wikipedia.org/wiki/Metric_modulation))。
- 「3 連の 8 分 → 普通の 8 分」は、4/4 から 12/8 風に移る定番の読み替え([Avner Dorman の講義資料](https://avnerdorman.github.io/Theory-IV-Dashboard/lessons/week-04/metric_modulation_final.html))。

#### 5.2.8 インドのターラとティハイ

| ターラ | 拍 | 分け方 | 出典 |
|---|---|---|---|
| アディ(カルナータカ) | 8 | 4+2+2(ラグ 1 + ドゥルタ 2) | [Adi tala](https://en.wikipedia.org/wiki/Adi_tala) |
| ルーパク(ヒンドゥスターニー) | 7 | 3+2+2。1 拍目(サム)が「空拍(カーリー)」 | [Rupak Tala](https://en.wikipedia.org/wiki/Rupak_Tala) |
| ミシュラ・チャープ | 7 | 3+2+2(3+4) | [Holzapfel ほか 2014](https://archives.ismir.net/ismir2014/paper/000265.pdf)、[Carnatic blog](https://ramyasspace.wordpress.com/2011/06/24/suladi-talas-chapu-talas/) |
| カンダ・チャープ | 5 | 2+3 系 | [Holzapfel ほか 2014](https://archives.ismir.net/ismir2014/paper/000265.pdf) |

- **ティハイ**: 同じ句を 3 回くり返し、間に(ふつう)同じ長さの休みを 2 つ入れ、最後の音をサム(周期の頭)に着地させる([Chromatone](https://chromatone.center/theory/rhythm/system/tala/)、[Online Bharatanatyam](https://onlinebharatanatyam.com/2009/08/13/tala-system/))。長さの式: 3 × 句 + 2 × 休み = 着地点までの長さ。

#### 5.2.9 ガムラン(周期を区切る楽器)

- ゴングの周期を、ケノンが 4 つに、クンプルがさらに 2 つに、クトゥがさらに 2 つに分ける入れ子の周期。ランチャランは 16 拍(`T W T N | T P T N | T P T N | T P T G`)([Colotomy](https://en.wikipedia.org/wiki/Colotomy))。変拍子ではないが「長い周期の中の区切り」を持つ点で、ターラやポリメーターと同じ仕組みで表せる。

#### 5.2.10 ジャズの 5/4・7/4

- **Take Five**: ピアノのヴァンプが「3+2」をはっきり刻む。Morello はライドを「1 - 2 - & - 3 - 4 - & - 5」、ハイハットのペダルを 2 と 4 に置き、1 拍分の型を足して 5 拍にする([Drumeo](https://www.drumeo.com/beat/how-to-play-take-five-beginner-drummers/)、[DRUM! Magazine](https://drummagazine.com/joe-morellos-take-five-drum-part/))。ソロの間も誰かがヴァンプを弾き続けないと崩れた、という話が残っている。変拍子ではヴァンプ(繰り返す型)が拍のまとまりを支える。

### 5.3 変拍子でグルーブさせる実践

#### 5.3.1 アクセントとドラムの型

- 原則: **まとまりの頭が強拍**。キックとスネアを置くのはまとまりの頭が基本。長い拍(3)の頭にスネアを置くとバックビートに聞こえやすい(経験則)。
- 7/8(2+2+3、8 分 7 ステップ)の例(経験則):
  - キック `X...x..` / スネア `..X....` / ハット `xxxxxxx`(3 の頭にキックを置く型)
  - 「4/4 から 8 分を 1 つ抜いた」型: キック `X...X..` / スネア `..X..X.`
- 5/4(3+2、8 分 10 ステップ): キック `X.....X...` / スネア `...X....X.`(経験則)。ジャズでは Take Five 型(上記)。
- 12/8: ハットやベルで 12 の刻み、キックは付点 4 分(0・3・6・9)、スネアは 6(経験則)。
- ドラムの教材も、キックとスネアを「332」「233」などのまとまりで考える([drumscore 332](https://www.drumscore.com/1201-332-kick-and-snare-placement-in-grooves)、[233](https://drumscore.com/30-lessons/level-1/grooves/groove-concepts/1889-233-kick-snare-placement))。

#### 5.3.2 ベースとコード

- ベースの根音はまとまりの頭。長い拍(3)の最後の 8 分に経過音を入れて、次の頭へつなぐ(経験則)。
- コードチェンジはまとまりの頭で行う。7/8 なら「2+2 | 3」の境目か小節の頭(経験則)。
- 変拍子では、繰り返すリフ(ヴァンプ)を 1 本決めておくと崩れにくい(Take Five の話)。

#### 5.3.3 「長い拍」は何倍か(実測)

| 研究 | 対象 | 結果 |
|---|---|---|
| [Bonini Baraldi ほか 2015, EMR](https://www.researchgate.net/publication/295403633_Measuring_Aksak_Rhythm_and_Synchronization_in_Transylvanian_Village_Music_by_Using_Motion_Capture)([PDF](https://pdfs.semanticscholar.org/b806/d88d63db80a669919d27bbf5de0be0c83836.pdf)) | トランシルヴァニアの遅い曲(約 55〜61bpm) | S:L の平均 0.656(SD 0.0435、44 周期)。周期ごとに 0.539〜0.759。8 曲すべてで 2:3 < S:L < 3:4 |
| [Moelants 2006, Musicae Scientiae](https://journals.sagepub.com/doi/abs/10.1177/102986490601000201) | ブルガリアの市販の録音 11 本 | 比は決まった値にならない。まとまりの並び・曲・奏者で違う |
| [Goldberg 2015, EMR](https://www.researchgate.net/publication/304005581_Timing_Variations_in_Two_Balkan_Percussion_Performances) | バルカンの打楽器(長・短・短) | 2 つの短い拍の比は 0.92。短い拍どうしも同じ長さではない。旋律のまとまりや客とのやりとりで変わる |
| Cler & Estival 1997(上の Bonini Baraldi が引用) | トルコの速い舞曲 | 1 曲の中では比が安定している |
| [Bernacki 2023, Konservatoryum](https://dergipark.org.tr/en/pub/konservatoryum/article/1245746) | ピリン地方の 7/8(3+2+2) | 2:3 の比に合わない |

- まとめ: L = 1.5 × S は出発点としては良い。遅い曲では L がやや短め(S:L ≈ 0.65〜0.7、つまり L ≈ 1.43〜1.54 × S)。速い舞曲では 1 曲の中で安定する。短い拍どうしにも差がある(0.92)。
- スウィング: Take Five は 8 分をハネる(3 連寄り)(経験則)。バルカンの速い 7/8 や 11/8 は、8 分をハネさせず、拍の長さの比で揺らす(上の研究から)。「8 分のスウィング」と「アクサクの比」は別のつまみとして持つほうがよい。

### 5.4 DAW・製品・記法での扱い

| 製品・形式 | まとまりを持てるか | メトロノーム・表示 | 出典 |
|---|---|---|---|
| Logic Pro | 持てる。Beat Grouping 欄に「223」と打つと 2+2+3 | 連桁がまとまりに沿う。メトロノームの Group を入れるとまとまりごとにクリック | [Apple サポート](https://support.apple.com/guide/logicpro/metronome-project-settings-lgcpe1d6118e/mac)、[Apple 拍子](https://support.apple.com/guide/logicpro/time-and-key-signature-overview-lgcp6409cfb7/mac) |
| Cubase | 譜面上の複合拍子(4+4+3/8)は連桁とタイの表示だけ | メトロノームは拍子イベントごとにクリックの型を作り、拍ごとに強さを変える | [Steinberg ヘルプ](https://archive.steinberg.help/cubase_pro_artist/v9/en/cubase_nuendo/topics/midi_editor_score_editor/score_basics/score_editor_composite_time_signatures_and_the_for_grouping_only_option_c.html) |
| Dorico | 持てる。2+2+3/8(加算の表示)と [2+2+3]/8(7/8 表示でまとまりだけ)。交互・並べる・入れ替えの拍子も | 連桁がまとまりに沿う | [Dorico ブログ](https://blog.dorico.com/2020/09/tip-create-advanced-time-signatures-with-the-popover/)、[Steinberg フォーラム](https://forums.steinberg.net/t/additive-time-signatures/947811) |
| MuseScore | 持てる。拍子の作成で 2+2+3/8、Note Groups で連桁 | — | [MuseScore](https://musescore.org/en/node/326284)、[Handbook](https://musescore.org/en/handbook/2/time-signatures) |
| Ableton Live | 持てない。分子 1〜99、分母 1/2/4/8/16。クリップの拍子は表示だけで再生に影響しない | ポリメーターはクリップのループ長で | [Live 12 マニュアル](https://www.ableton.com/en/manual/arrangement-view/)、[Clip View](https://www.ableton.com/en/manual/clip-view/) |
| REAPER | 拍子とテンポのマーカー。部分小節も作れる | 小節頭は強い音、ほかは弱い音 | [Music Tools Lab](https://musictoolslab.com/blog/reaper-metronome)、[Sound On Sound](https://www.soundonsound.com/techniques/tempo-mapping) |
| Studio One | 未確認 | 未確認 | — |
| Elektron | 拍子ではなくトラックごとの長さでポリメーター | 全体の長さで一斉に頭へ戻す | [Digitakt マニュアル](https://www.manualslib.com/manual/1275776/Elektron-Digitakt.html?page=32) |
| SMF(MIDI ファイル) | 持てない。`FF 58 04 nn dd cc bb`。dd は 2 の何乗か(分母は 2 の累乗だけ)。cc = クリック 1 回あたりの MIDI クロック | cc で「付点 4 分ごとにクリック」は表せるが、等しくない拍は表せない | [SMF 仕様](https://midimusic.github.io/tech/midispec.html)、[RecordingBlogs](https://www.recordingblogs.com/wiki/midi-time-signature-meta-message) |
| MusicXML | 持てる。3+2/8 は `<beats>3+2</beats>` と `<beat-type>8</beat-type>` の 1 組。2/4+3/8 は 2 組 | — | [MusicXML 4.0 beats](https://www.w3.org/2021/06/musicxml40/musicxml-reference/elements/beats/) |

- 結論: まとまりは Glaux のプロジェクトの中に持ち、SMF に書き出すときは分子と分母だけにする(まとまりは失われる)。MusicXML に書き出すときは `beats` に入れる。

### 5.5 AI・研究・データ

- **ユークリッドリズム**: k 個の打点を n ステップにできるだけ均等に置く。Toussaint(2005)は多くの伝統的なリズムがこれで作れると示した。アクサクの一族と近い関係にある([Toussaint 論文](https://cgm.cs.mcgill.ca/~godfried/publications/banff-extended.pdf)、[Euclidean rhythm](https://en.wikipedia.org/wiki/Euclidean_rhythm))。

| E(k,n) | まとまり | 例(同論文) |
|---|---|---|
| E(2,5) | 23 | 回すと 32 = Take Five・Mars |
| E(3,7) | 223 | ルチェニツァ |
| E(4,9) | 2223 | トルコのアクサク、ダイチョヴォ |
| E(5,11) | 22223 | コパニツァ(回した形)、サワーリー・ターラ |
| E(6,13) | 222223 | Mama Cone pita / 回すと Krivo Plovdivsko Horo |
| E(3,8) | 332 | トレシージョ |
| E(5,8) | 21212 | シンキージョ |
| E(7,12) | 2122122 | アシャンティのベル(12/8 のベル) |
| E(5,16) | 33334 | EDM の型 |

- **拍と小節頭の推定**: Holzapfel・Krebs・Srinivasamurthy(ISMIR 2014)は、様式ごとに学習したベイズモデルで、トルコ(9/8 アクサク・10/8 ジュルジュナ・8/8 デュイェク、82 曲)・クレタ(2/4、42 曲)・カルナータカ(アディ・ルーパカ・ミシュラ・チャープ・カンダ・チャープ、118 曲)の拍を追った。9/8 アクサクの拍の F 値は 85.7〜91.0。従来法(Klapuri)は 69.4([論文](https://archives.ismir.net/ismir2014/paper/000265.pdf)、[CompMusic](https://compmusic.upf.edu/ismir-2014-odd))。
- **4/4 に偏ったモデルの補正**: Morais・McFee・Fuentes(2025)は、4/4 の注釈付きデータから拍を抜いて 3/4・2/4 の学習例を作り、少ない拍子の小節頭の推定を改善した([arXiv 2502.12972](https://arxiv.org/abs/2502.12972))。Glaux の道具も同じで、4/4 前提の型を「抜いて・足して」ほかの拍子に使える。
- **データセット**:

| 名前 | 中身 | ライセンス | Glaux での使い方 |
|---|---|---|---|
| CompMusic Carnatic Rhythm | 176 曲、4 ターラ、約 997 分、サムの注釈 22,646 | 注釈は CC BY-NC-SA 4.0、音声は市販の録音で再配布不可 | 非商用なので同梱しない。手元での検証だけ([mirdata](https://mirdata.readthedocs.io/en/0.3.9/_modules/mirdata/datasets/compmusic_carnatic_rhythm.html)) |
| Groove MIDI Dataset | 拍子は 5 種類、ほとんど 4/4(6/8 など少し) | CC BY 4.0 | 6/8 などの型の統計に使える(クレジット明記)([TFDS](https://www.tensorflow.org/datasets/catalog/groove)、[Magenta](https://magenta.withgoogle.com/datasets/groove)) |
| Lakh MIDI / Lakh Pianoroll | Pianoroll 版は 4/4 以外を除いた | 権利が不明な曲を含む | 同梱しない([LPD](http://hermandong.com/lakh-pianoroll-dataset/dataset.html)) |

- 結論: 変拍子の型は、データから学ぶより、**規則(まとまり+ユークリッド+型のセル)で作る**ほうが Glaux に合う。変拍子のデータは少なく、ライセンスも同梱に向かない。

### 5.6 Glaux への提案

方針: 「意図の少数の引数 → 決まった手順で絶対値に展開」。乱数は `seed`、新しい ID は作る側が生成する。16 分のステップを最小単位(240 tick)とし、どの拍子でも 1 小節 = `num × 16 / den` ステップ(7/8 = 14、5/4 = 20)。

#### 優先度 高

**A. 拍子にまとまりを持たせる(スキーマ変更)**
- `TimeSigEvent { tick, num, den, grouping: Option<Vec<u8>> }`。単位は分母の音符。和は `num`。`#[serde(default, skip_serializing_if = "Option::is_none")]`。
- `None` の既定値は関数 `default_grouping(num, den)` で決める:
  - den ≤ 4: 全部 1(4 分ずつが拍)。5/4 は [3,2]、7/4 は [4,3] を「上の段」として持つ(Take Five・Money 型)。
  - den ≥ 8 で num が 3 の倍数(6・9・12): 3 ずつ(複合拍子)。
  - それ以外: E(⌊num/2⌋, num) の間隔(5 → 2+3、7 → 2+2+3、11 → 2+2+2+2+3、13 → 2+2+2+2+2+3)。
- 検証(validate.rs): 和 = num、要素は 1〜num。要素が 2・3 以外なら警告だけ(アディ・ターラの 4+2+2 は認める)。
- `FORMAT_VERSION`: 読み込みは今のファイルのまま通る。ただし古い Glaux で開いて保存すると grouping が黙って消えるので、**2 に上げることを勧める**。`tests/schema.rs` の FIXTURE と `random_command` に grouping の生成を足す。`SetTimeSig` の逆コマンドは今の仕組みのまま使える。

**B. 共通の「拍の格子」関数(glaux-core)**
- `meter::beats_in_bar(project, bar) -> Vec<Beat { tick, len, level }>`。level は 0 = 小節頭、1 = まとまりの頭、2 = まとまりの中の 8 分、3 = 16 分。
- すべての道具(apply_groove・write_drums・write_chords・write_bassline・swing_notes・analyze_rhythm・critique_arrangement)がここから位置を引く。これで「1 小節を 16 等分」の 210 tick の端数(7/8 の場合)がなくなる。
- 道具は `get_project` に各小節の grouping を返し、AI が拍を数え間違えないようにする。

**C. write_drums の変拍子対応**
- 引数: `odd_meter`: `"group"`(既定。まとまりから組み立てる)/ `"cut"`(4/4 の型を小節の長さで切る。7/8 = 4/4 から最後の 8 分を抜く)/ `"stretch"`(今の等分。後方互換)。
- `"group"` の中身: 様式ごとに 2 のセルと 3 のセルを持つ(例 rock: 2 = キック `X.`、3 = スネア `X..`。キックとスネアはまとまりの頭で交互。最後のまとまりが 3 ならスネアを置く)。ハットは 8 分か 16 分を小節の長さだけ刻み、まとまりの頭を強く。
- 4/4 の場合は今の型のまま(結果を変えない。sound_regression やテストを守る)。

**D. write_chords・write_bassline をまとまりで動かす**
- `changes: "per_group" | "per_bar" | "per_long"`(既定: 変拍子では per_bar、per_group なら頭ごと)。
- リズムの文字列は 16 分ステップの長さ(7/8 = 14 文字)で受け、`|` でまとまりの境目を書けるようにする(`X...|X...|X.....`)。長さが合わなければ今の等分で当てて、警告を返す。
- ベース: まとまりの頭に根音。長いまとまり(3)の最後の 8 分に、次の根音への経過音(`approach: true` のとき)。

#### 優先度 中

**E. set_meter_feel(アクサクの長短の比)** — 新しい道具
- 引数: `clip`、`long_ratio`(1.33〜1.6、既定 1.5 = 変えない)、`short_skew`(同じ小節の短い拍どうしの比、0.9〜1.0、既定 1.0)。
- 中身: 小節の長さは変えずに、2 のまとまりの長さ S と 3 のまとまりの長さ L を `n2·S + n3·L = 小節長`、`L = long_ratio·S` から解く。ノートの位置は、まとまりの中で線形に写す。出典: S:L 0.656(Bonini Baraldi 2015)、短い拍どうしの比 0.92(Goldberg 2015)。
- swing_notes は「8 分・16 分の裏」を「まとまりの中の偶数番目のステップ」に一般化する(3 のまとまりの中はハネさせない、を既定に)。

**F. change_meter / extend_bar(1 拍足す・抜く)** — 新しい道具
- 引数: `bar`、`beats: +1 | -1 | +2`(または `to: "2/4"`)、`shift_after: true`。
- 中身: time_sig_map にその小節だけの拍子を足し、次の小節で元の拍子に戻す。それより後の全ノート・マーカー・オートメーションを差の tick だけずらす(絶対値の `Command` 列にして返す)。J-POP の「サビ前の 2/4」をこれ 1 回で作れる。

**G. write_polyrhythm / write_polymeter** — 新しい道具
- `write_polyrhythm(track, ratio: "3:2", span_beats: 2, pitch, vel, accent_first)`: span を ratio の左の数で等分して置く。tick で割り切れないときは四捨五入して丸めの誤差を返す(3 連の 4 分 = 320 tick は割り切れる)。
- `write_polymeter(track, pattern: "X..X..X.", unit: "16th", bars, reset_every_bars: 0)`: 型を周期のまま並べる。返り値に「元に戻るまでのステップ数 = 最小公倍数」を入れる。Elektron の M.LEN と同じ意味で `reset_every_bars` を持つ。Bleed 型は「ハットとスネアは 4/4、キックだけ長い周期」。
- `euclid(k, n, rotation)` を型の文字列の作り方として両方で受け付ける(`pattern: "E(5,16)"`)。

**H. critique・analyze_rhythm の変拍子対応**
- 「16 分の格子どおりの割合」を拍子ごとの格子で数える。まとまりの頭にキックかベースがある割合(低いと拍子が伝わりにくい)。ポリメーターの周期(最小公倍数)の報告。

#### 優先度 低

**I. metric_modulation** — `at_bar`、`from: "triplet_8th"`、`to: "8th"`(または `ratio: "3:2"`)、`time_sig`(任意)。新しいテンポ = 旧テンポ × 比を計算して TempoEvent を置く。
**J. displace** — `clip`、`by_steps`(16 分の数、正負)、`wrap: true`(小節の中で回す)。相対の指定は MCP 層で絶対の位置にしてから `Command` にする(コマンドは絶対値という原則)。
**K. hemiola** — `bars: [n, n+1]`(3 拍子の 2 小節)、`what: "accents" | "chords"`。アクセントとコードの変わり目を 2 拍ごとに置き直す。終止の前に使う。
**L. write_tihai** — `phrase`(ノート列かリズム文字列)、`gap_steps`、`land_on`(小節番号の頭)。着地から逆算して開始位置 = 着地 − (3 × 句 + 2 × 休み)。
**M. 書き出し** — MusicXML に書き出すときは `<beats>2+2+3</beats>`。SMF では分子と分母だけ(警告を返す)。SMF を読み込むときは cc(クリックの間隔)から 6/8・12/8 の付点 4 分クリックを推測して grouping の候補にする。

#### 上位 5 つ

1. A: `TimeSigEvent.grouping` と既定値の規則(ユークリッド)。FORMAT_VERSION 2
2. B: 16 分の格子 × まとまりの共通関数。「1 小節を 16 等分」をやめる
3. C: write_drums の `odd_meter: group | cut | stretch`
4. D: write_chords・write_bassline・apply_groove をまとまりの頭で動かす
5. F と G: extend_bar(1 拍足す・抜く)と write_polyrhythm / write_polymeter(euclid 付き)

GPL のコードは参照していない。上の手順はすべて論文・マニュアル・一般的な音楽理論からの設計。
