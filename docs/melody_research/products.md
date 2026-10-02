# 作曲支援製品と人と AI の共同作曲の研究 ― 旋律の質(センス)をどう担保し、どんな失敗・不満が報告されているか

調査日: 2026-09-30 / 対象: 製品 15 件余り、研究 20 件余り

## 表記の約束

| 印 | 意味 |
|---|---|
| 【一次】 | 論文 PDF・公式マニュアル・開発者ブログの本文を自分で読んで確かめた |
| 【一次・要約】 | 一次情報のページだが、取得ツールの要約を経由して読んだ(数値や言い回しの細部はずれている恐れがある) |
| 【二次】 | レビュー記事・フォーラムの書き込み・まとめサイト |
| 【推測】 | 調査者の解釈・Glaux への当てはめ |

---

## 0. 要旨

1. **「欠点が無い」と「良い」は別物だ**という報告が、30 年前から繰り返し出ている。
   - GenJam(1999): 人の評価を使わずに育てたソロは「ひどくはないが、いい瞬間がめったに出ない」。
   - folk-rnn の編集者は、機械の曲が「平凡すぎる」ところの音を変えて「特別に」した。
   - 2026 年の Libretto(Claude を使うエージェント作曲)では、LLM の旋律が「音階的で順次進行ばかり」に寄る退化を、コーパスに対する百分位の**両端**として検出している。
   - Glaux の点検(`100 - 12×警告 - 4×情報`)は欠点を減点するだけなので、同じ落とし穴にはまっている。
2. 製品が旋律の質を支える方法は、主に 5 通りある。
   - (a) 学習データの分布に任せて、たくさん出した中から人に選ばせる
   - (b) プリセットやパターン語彙
   - (c) 「休み」「跳躍」「密度」などの明示のつまみ
   - (d) 自動の選別(棄却サンプリング、分類器)
   - (e) 分布の中の位置で診断する
   
   最終的な品質は、**ほぼすべての製品で人の選別と編集が担っている**。
3. 不満でいちばん多いのは次の 3 つ。
   - 操作できない(「サイコロを振っている感じ」)
   - 全部作り直すか全部受け入れるかしかない
   - ありふれていて平凡・文脈に合わない
   
   これへの答えとして定着してきたのが次の 4 つ:
   - 部分の再生成(infill)
   - パートと範囲の限定
   - 複数案の試聴
   - 意味のあるつまみ(「普通⇔意外」「明るい⇔暗い」)
4. 好みの取り込みは「採用・無視の暗黙の記録」(Hookpad Aria)と「2 案から任意でトロフィー」(MusicRL)が大規模に動いている。評価させすぎると疲れるので、評価の手間を小さく保つ設計が肝心(GenJam の「適応度のボトルネック」)。

---

## 1. 製品

### 1.1 Hookpad / Aria(Hooktheory)

- **できること**【一次】
  - 譜面(リードシート)を書くエディタ Hookpad に組み込まれた生成機能で、2024 年 3 月に公開。
  - 範囲(小節)を選ぶと、次の 4 通りを作る。
    - 左から右への続き
    - 途中の穴埋め(fill-in-the-middle)
    - 旋律から和音
    - 和音から旋律
  - 生成は拍子・キー・テンポを条件にする。
  - 1 つの区間について案を何度でも出し直して試聴し、気に入ったものを「accept」する。
  - モデルは Anticipatory Music Transformer(Thickstun・Donahue ら)を、Hooktheory の TheoryTab(ポップスの採譜 約 5 万曲)で微調整したもの。
- **質の担保**
  - 【一次】学習データを「ヒット曲の採譜」に絞っている。範囲とパートを限定する。短い案を多数出させ、選ぶのは人。
  - 【一次】採用・無視を暗黙のフィードバックとして記録している。論文は今後の方向として、ユーザーの反応への整合(RLHF)と、モデルの A/B テストを挙げている。
  - 【一次・要約】公式ブログ(2024-10)では、一度に 4 小節を作るとしている。開発者自身も「この変奏は完璧か? そうは思わない」「いい出発点」と書いている。
- **Hookpad 本体の「センスの可視化」**【一次・要約】
  - 音と和音を**音階度で色分け**する(1〜7 度を赤・橙・黄・緑・青・紫・桃)。
  - 「Magic Chord」は、次の和音の候補を「有名曲がどうしたか」の統計で示す。
  - SongMetrics はデータベースと比べた次の値を曲ごとに出す。
    - 和音の複雑さ
    - 旋律の複雑さ(非音階音+シンコペーション)
    - 和音と旋律の緊張(旋律が和音外の音になる頻度)
    - 進行の新しさ(データベースでの珍しさ)
- **報告された失敗・不満**
  - 【一次】論文の利用統計: 3k 人が 31.8 万案を生成し、7.4 万案を採用。
    - 【推測】単純計算で採用率は約 23%。約 4 案に 3 案は捨てられている。
  - 【一次】利用者 8 人への 1 時間のインタビューの結果:
    - 案を「そのまま使うことはまれ」だが、行き詰まりを抜けるのに役立つ。
    - 短い案は「creative sparks」と呼ばれ、文から音楽を作るツールより主導権が残る。
    - 一方で「**もっと操作したい**」という要望が出た。挙がったのはジャンル、感情(明るい・悲しい)、想定楽器(歌・ギター)、構造(A メロ・サビ)。
  - 【二次・公式フォーラム 2024-03〜04】
    - 「案の 3 分の 1 以上が、既存の旋律と同じか、オクターブ移しただけ」
    - 「半分は既にあるものを出してきた」
    - → **文脈の丸写し**。どの区間を参照させるかを選びたい、という要望も出ている。
  - 【一次】Amuse 論文の事前調査で、Aria の利用者は「曲のテーマ(歌詞の内容)に合わない」と不満を述べている。
- 出典
  - Donahue ら「Hookpad Aria: A Copilot for Songwriters」arXiv 2502.08122(2025): https://arxiv.org/abs/2502.08122
  - https://www.hooktheory.com/blog/generative-ai-songwriting/ (2024)
  - https://www.hooktheory.com/support/hookpad
  - https://www.hooktheory.com/song-metrics/about
  - https://forum.hooktheory.com/t/introducing-aria-your-new-generative-ai-assistant-in-hookpad/8082

### 1.2 Captain Plugins / Captain Melody(Mixed In Key)

- **できること**【一次・要約】
  - Captain Chords の進行に合わせて旋律を作る。
  - 「IDEA」タブのつまみ:
    - 形: Steps(順次進行への依存度)、Leaps(3 度以上の跳躍を含む音の数)、Octave Range
    - リズム: Density(音数)、Triplets、Length(音価)、**Rests(音の無い区間がいくつあるか)**
    - 和声: First Note(開始の音階度)、Lanes(各音階度の出現割合)
  - 音の色分け: 和音の音 = 青、2・7 度の非和音音 = 黄、4・6 度 = 緑。
  - つまみごとの「Update」で**その要素だけ**再生成でき、サイコロ(ダイス)で全部を乱数化することもできる。
- **質の担保**
  - 【推測】旋律の「センス」を、休み・跳躍・順次進行・密度という数えられる性質に分けて、人に直接つまませている。
  - 【二次・KVR 2020】評では「スタイルとプリセットが中心」とされている。
- **不満**【二次】
  - 「いちばん難しいのは、気に入る最初のリフを見つけること」(Magnetic Mag、2022-10)
  - 「満足する旋律を得るには Captain Melody の奥まで入り込む必要がある」(KVR ユーザー評、2020)
  - 旋律の質そのものへの強い批判は、今回見つけた範囲では少ない。
- 出典
  - https://mixedinkey.com/captain-plugins/how-to-guide/captain-melody/
  - https://magneticmag.com/2022/10/captain-plugins-5-review/
  - https://www.kvraudio.com/product/captain-plugins-by-mixed-in-key/reviews

### 1.3 Scaler 3(Scaler Music / Plugin Boutique)

- **できること**【二次】
  - 和音の検出と提案が中心。
  - 旋律は「Motions」(和音に追従する約 1,000 の演奏フレーズ。ムード・ジャンルのタグ付き)と「Phrases」で作る。
- **質の担保**【推測】人が作ったフレーズ語彙を和音に当てはめる方式なので、破綻は少ないが意外性も少ない。
- **不満**【二次・公式フォーラム 2025-11〜2026-07】
  - 「和音から気に入る動機や旋律を見つけるのに苦労する」
  - 「内蔵の旋律生成がほしい」
  - 「プリセット中心なので、作ったものが**唯一のものだと感じられない**」
  - Sound On Sound の評(2026-02)は、Motions を「演奏らしく試聴する道具」として評価しつつ、「深く、習得に時間がかかる」としている。
- 出典
  - https://forum.scalermusic.com/t/melody-generator/23899
  - https://www.soundonsound.com/reviews/scaler-music-scaler-3

### 1.4 Orb Producer Suite 3(Hexachords → LANDR)

- **できること**【二次】Orb Melody はつまみで作る方式。
  - Density(密度)
  - Complexity(複雑さ)
  - 音域(オクターブ)
  - シンコペーション
  - 「human touch」
- **不満**【二次・KVR 2020】
  - 「基本の説明が見つけにくい」
  - Density と Complexity の違いが分かりにくい(つまみの意味が人に伝わらない問題)
- 出典
  - https://www.kvraudio.com/forum/viewtopic.php?t=556373
  - https://www.landr.com/plugins/producer-suite-3

### 1.5 Melody Sauce 2(Evabeat)

- **できること**【二次】
  - Mood(Light / Dark)と Complexity(Simple / Complex、それぞれに「EXTRA」)を選び、9 つのパッドから旋律を出す。
  - Style モードもある。
  - Advanced Editor では、追従する和音の指定と、小節ごとの編集ができる。
- **質の担保**【推測】気分と複雑さの 2 軸に絞った、少ない語彙の操作。
- 出典
  - https://www.pluginboutique.com/product/3-Studio-Tools/93-Music-Theory-Tools/8877-Melody-Sauce-2
  - https://altwire.net/melody-sauce-2-review/

### 1.6 AIVA

- **できること**【二次】
  - スタイル(プリセットか、ユーザーが上げた MIDI・音声の「influence」)から曲全体を作る。
  - ピアノロールで編集して再生成する。
- **評判**【二次】
  - 作曲家 Kevin MacLeod(2019-05)は「粘りのある変わった旋律を作る。**跳躍が適切な場所にある**」と評価した。
  - 同時に、「出力のうち使えるのは 15% ほどで、それも手間がかかる」とも書いている。
  - 近年のまとめ記事では「パターンを繰り返す」「盛り上げ方がいつも同じ(金管+クレッシェンド+シンバル)」という批判が多い。
- 出典
  - https://incompetech.com/music/ai/AIVA/AIVA.html (2019)
  - https://aiva.crisp.help/en/article/upload-influence-guidelines-1485kpl/

### 1.7 Magenta Studio(Google Magenta、Ableton Live 用)

- **できること**【一次・要約】
  - Generate: MusicVAE で 4 小節の単旋律・ドラムを生成し、生成数を選べる。
  - Continue: RNN で最大 32 小節まで続きを作る。
  - Interpolate: 2 つのクリップの間を最大 16 段階で補間する。
  - Groove / Drumify: ドラム用。
  - どれにも**温度(temperature)**のスライダーがある。
- **質の担保**【一次】
  - 学習データの分布に任せる。
  - 操作は温度(保守的⇔ばらつき)だけ。
  - 変化量を選び、複数案を出して人が選ぶ。
- **不満・限界**
  - 【一次(孫引き)】Magenta Studio 以外の例: 操作が温度だけの MMM を DAW に組み込んだ 2023 年の調査でも、利用者は「もっと操作したい」と答えた(Composer's Assistant 2 論文の引用による)。
  - 【一次】folk-rnn の報告: 温度を 0.1 まで下げると**非常に反復的**になり、2 以上では「新音楽のパロディ」のようになる。温度だけでは細かく操れない。
- 出典
  - https://magenta.tensorflow.org/studio-announce (2019-02)
  - https://magenta.tensorflow.org/studio/

### 1.8 Bach Doodle(Google、Coconet)

- **できること**【一次】
  - 2019-03 の Google Doodle。
  - 利用者が 2 小節の旋律を簡易な五線で書くと、Coconet がバッハ風の 4 声に和声づけする。
  - ブラウザで動かした(実行時間を 40 秒から 2 秒へ、モデルは約 400KB)。
- **質の担保**【一次】
  - バッハのコラールで学習した分布。
  - Coconet は「一度に左から右へ」ではなく、ギブスサンプリングで書き直しを繰り返す。
- **報告された失敗・不満**【一次】
  - 3 日で 5,500 万件の問い合わせがあった。
  - 80% のセッションで、複数の和声づけを試していた(=出し直しが基本動作)。
  - 評価は「Good」が 53.8%、無評価が 35.2%、残り約 11% が Poor / Neutral。
  - 上級者からの不満: **連続 5 度・8 度**。2,180 万件の出力を分析すると、1 小節あたり連続 5 度が 0.365 回、連続 8 度が 0.391 回あった。
  - 利用者の入力が**学習データの分布の外**(音域が MIDI 60〜81 の外、隣の音の差が 1 オクターブ超)だと増え、少ないほど高評価と相関した。
  - 著者らは「連続 5 度・8 度は出力品質の**便利な代理指標**になる。推論時に検出したらギブスサンプリングを追加する使い方も考えられる」と書いている。
  - 設計面: 利用者テストで「和声の概念を知らない人が多い」と分かり、短いアニメーションで説明した。16 分音符や転調は「隠し機能」に回した。
- 出典
  - Huang ら「The Bach Doodle」ISMIR 2019: https://arxiv.org/abs/1907.06637 / https://archives.ismir.net/ismir2019/paper/000097.pdf

### 1.9 Band-in-a-Box(PG Music)の Melodist / Soloist

- **できること**【一次・要約】
  - 「Melodist」を選ぶと、その様式でイントロ・和音・旋律・編曲を一度に作る。既存の進行に旋律だけを付けることもできる。
  - Soloist には次のモードがある。
    - 通常のソロ
    - Fills(一定割合の時間だけ和音の上で「noodle」する)
    - Solo Around Melody
    - Trade 4s
- **質の担保**【推測】様式ごとの規則・データで作る「様式の模倣」。「Fills」の割合指定は、**鳴らす時間の割合=休みの量**を直接指定している点で興味深い。
- 出典
  - https://www.pgmusic.com/manuals/bbw2024full/chapter9.htm

### 1.10 BandLab SongStarter

- **できること**【二次】
  - ジャンルを選ぶ(任意で 50 文字までの歌詞・絵文字)と、ビート・旋律・和音の「アイデア」を **3 つ**出す。
  - 「day / dusk / night」ボタンで気分違いにできる。
  - Regenerate で出し直せる。
  - 2022-03 公開、TensorFlow を使い Google と協力。
- **質の担保**【推測】行き詰まりを抜けるきっかけ(完成品ではなく出発点)として位置づけ、3 案を並べて選ばせる。
- 出典
  - https://blog.bandlab.com/introducing-songstarter/
  - https://musically.com/2022/03/08/bandlab-gets-into-ai-music-with-new-songstarter-feature/

### 1.11 Logic Pro の Session Players(Apple、2024)

- **できること**【一次・要約】
  - Keyboard / Bass / Drummer が、Chord トラックと Arrangement(A メロ・サビ)に従って演奏を作る。
  - Complexity(忙しさ)と Intensity(強さ)の 2 軸で操作する。
  - Bass には「根音だけ⇔旋律的」の種類、フレージング(音の長さ)、オクターブの指定がある。
  - 画面の濃い点は主要なアクセント、薄い点は Complexity を上げると足される音を表す。
  - 最終的には MIDI リージョンに変換して手で直す。
- **不満**【一次・要約】Sound On Sound(2024-09): 最大の不満は、既存の MIDI 演奏から和音を自動で取り出せないこと。
- 【推測】「複雑さを上げるとどの音が足されるか」を**点の濃淡で予告する表示**は、操作の結果を事前に理解させる良い例。
- 出典
  - https://www.soundonsound.com/techniques/logic-pro-session-players
  - https://support.apple.com/guide/logicpro/session-players-overview-lgcpbf624405/mac

### 1.12 Ableton Live 12 の MIDI Generators / Transformations

- **できること**【一次・要約(公式マニュアル)】
  - Generators:
    - Seed: 音域・音価・ベロシティの範囲・声部数・密度の中で乱数で置く
    - Shape: 描いた輪郭に沿って置く。Tie と Jitter がある
    - Rhythm / Euclidean
    - Stacks: 和音
  - Transformations(既存の音の変形):
    - Ornament(装飾音)
    - Connect(音の間を埋める)
    - Recombine(位置・音高・音価を入れ替える、鏡像、回転)
    - Span(レガート・スタッカート)
    - その他 Arpeggiate・Strum・Time Warp など
  - クリップにスケールが設定されていると、音高のパラメータは**音階度**で扱われる(scale-aware)。
  - Auto Apply でつまみを動かすたびに作り直される。
- **質の担保**【一次・要約】「疑似乱数の道具」であり、センスは人の選別と変形に任されている。
  - Sound On Sound(Oli Freke、2024-08)の助言:
    - 「耳に引っかかったものに留まり、仕上げる規律が必要」
    - 「説得力のあるリフには音域を 4 度〜5 度に絞り、声部を 1 に」
- 【推測】「生成」と「変形」を分けて、変形(輪郭を保ったまま装飾・入れ替え)を用意している点は、Glaux の transform / ornament と同じ発想。
- 出典
  - https://www.ableton.com/en/live-manual/12/midi-tools/
  - https://www.soundonsound.com/techniques/ableton-live-12-midi-generators

### 1.13 Suno / Udio(音声で曲を丸ごと作る)

- **できること**【二次】
  - 文(と歌詞)から歌入りの曲全体を作る。
  - 部分の作り直し:
    - Udio の「Audio Inpainting」(2024-05。区間を選び前後の文脈で作り直す。のちに「Edit」へ発展)
    - Suno の「Replace Section」(2024-10、有料版)
  - Suno Studio(2025〜)ではステムと MIDI の書き出しができる。
- **旋律の評判**
  - 【二次】Production Expert(Russ Hughes、2025-09-24):
    - 「和音進行が予測可能すぎて、次の行が来る前に歌えた」
    - 「すべてが決まり文句に浸っている」
    - 「何にでも似ていて何にも似ていない」
  - 【一次】「The Algorithmic Flattening of Sound」(arXiv 2608.06106、2026-08): 同じ数の人の曲と比べると、Suno はジャンル間の音響の差が縮む。
  - 【一次】Amuse 論文(2025)の事前調査:
    - 利用者が Udio を和声の参考に使ったが、「使える要素を取り出すのが大仕事」だった。
    - 音声で出てくるので、編集の流れに組み込めない。
- **研究で見た「文から音楽」系への不満**【一次】Ronchini ら(CMMR 2025、MusicGen を使った制作者 17 人の調査):
  - 「思い描いたものと違う」(creative misalignment)
  - テンポ・キーが合わない
  - 「一部だけ変えられない。**結果に 2 回目の指示を足して一部だけ変えたい**」「再生成の繰り返しの過程がほしい」
- 出典
  - https://www.production-expert.com/production-expert-1/suno-is-fun-but-professional-musicians-shouldnt-lose-any-sleep
  - https://arxiv.org/html/2608.06106
  - https://musically.com/2024/05/13/ai-music-startup-udio-adds-subscriptions-and-inpainting/
  - https://arxiv.org/abs/2509.23364

### 1.14 Staccato

- **できること**【二次】
  - チャット型の画面で文から MIDI を作る。
  - Extend(続き)・Rewrite(書き換え)・Accompany(伴奏付け)がある。
  - MIDI エディタが中心にあり、生成→編集→再生成を往復できる。
  - DAW プラグインもある。
- 出典
  - https://staccato.ai/
  - https://www.audiocipher.com/post/staccato-ai-midi-extension

### 1.15 調べが薄いもの

- Instachord、Musicfy(声を楽器音に変換するのが中心で、旋律生成は主題ではない)は、一次情報で旋律の質の担保を論じた資料を見つけられなかった。
- 製品の不満の多くはフォーラムやまとめ記事に限られ、系統だった調査は少ない。

### 1.16 製品の比較表

| 製品 | 入力 | 旋律の作り方 | 人の操作 | 質の担保の主役 |
|---|---|---|---|---|
| Hookpad Aria | 既存の旋律・和音+範囲 | 学習モデル(ポップスの採譜で微調整) | 範囲選択・無限の出し直し・採用 | 分布+人の選別(採用率 約 23%) |
| Captain Melody | 進行 | 規則+乱数 | Steps / Leaps / Rests / Density など | 明示のつまみ |
| Scaler 3 | 進行 | フレーズ語彙(Motions) | ムード・ジャンルで選ぶ | 人が作った語彙 |
| Orb Melody | 進行 | 規則+乱数 | Density / Complexity など | つまみ |
| Magenta Studio | クリップ | VAE / RNN | 温度・生成数 | 分布+選別 |
| Bach Doodle | 2 小節の旋律 | Coconet(ギブス) | 出し直し・評価 | 分布(分布外で崩れる) |
| Logic Session Players | 和音・構成 | 様式の演奏モデル | Complexity / Intensity | 様式+MIDI にして手直し |
| Ableton Generators | 範囲・形 | 疑似乱数(スケール対応) | 多数のつまみ・変形 | 人の選別 |
| Suno / Udio | 文・歌詞 | 音声モデル | 区間の作り直し | 分布(「平均的」になりがち) |

---

## 2. 研究

### 2.1 Cococo(Louie・Coenen・Huang・Terry・Cai、CHI 2020)

- **課題**【一次】初心者 11 人の事前調査(Coconet で 4 声を埋める普通の画面)で、次の 2 つが分かった。
  - (1) **情報過多**: 一度に多く作られるので評価できない。どの音が濁りの原因か分からない。
  - (2) 非決定的な出力: 調和はしているが目的と合わない。目的を伝える手段が無い。
  - 参加者はこれを「**サイコロを振っている**」と表現した。
  - 参加者は本来「小節ごと・声部ごと」に作りたかった。
- **道具**【一次】
  - Voice Lanes: 生成する声部と時間を先に指定する。
  - Example-based slider: 選んだ例に「似せる⇔離す」。
  - Semantic sliders: 「普通⇔意外」(温度)と「明るい(長三和音)⇔暗い」。
  - Multiple Alternatives: 案の数を選び、文脈にはめて試聴する。
  - Infill mask: 矩形で消して埋め直す。
  - スライダーは「soft prior」で実現している。モデルの分布に、望む性質の事前分布を掛け合わせる。確率を 0 にしないので、文脈との整合も保たれる。
- **結果**【一次】初心者 21 人の被験者内比較で、Cococo のほうが有意に高かった項目:
  - 創作の目的を表現できたか、自己効力感、学び、没入、完成度
  - 操作できる感覚、理解しやすさ、信頼、協働感、**所有感**(自分の貢献だと感じる度合い)
  - 独自性は差が無かった。労力も差が無かった(「つまみを考える労力」と「思いを伝えられない労力」の種類が違う)。
- **定性の知見**【一次】
  - 「ベースを決めて、次にテノール…と少しずつ」作るようになった(意味のある単位への分解)。
  - 複数案で「この構成ではいい案が出ない」と推論できた。
  - 開発中に気づいた点: 非決定性のせいで案が「ばらばらで焦点が無い」か「互いに似すぎる」のどちらかになりがちだった → 例示スライダーを作った。
- 続報 Suh ら「AI as Social Glue」(CHI 2021、Cococo を 2 人組 15 組で使用)【二次・要約】: AI が 2 人の間の共通の土台・意見の対立の緩衝材になる役割を報告している。
- 出典
  - https://ceur-ws.org/Vol-2848/HAI-GEN-Paper-1.pdf(短縮版)
  - CHI 版 https://www.researchgate.net/publication/341687252
  - https://dl.acm.org/doi/10.1145/3411764.3445219

### 2.2 AI Song Contest(Huang・Koops・Newton-Rex・Dinculescu・Cai、ISMIR 2020)

- **対象**【一次】13 チーム・61 人が、ユーロビジョン風の 3 分以内の曲を AI で作った。
- **旋律の選び方**【一次】
  - **生成して選ぶ**: T8 は 450 以上の旋律とベースを生成した。T2 は MusicVAE の組み合わせから「最も魅力的なもの」を手で選んだ。
  - **2 段の選別**: T8 は「**キャッチーさ分類器**」(曲のランキングで学習)で絞ってからアーティストへ渡した。T5 は歌詞と旋律の強勢のパターンを機械的に合わせた。
  - プライミング: 「上がる旋律がほしいときは上がる入力を与えた」。
  - 温度の使い分け: A メロとサビで温度を変え、対比を作った。
- **報告された困難**【一次】
  - ML は**直接操れない**。
  - 小さなモデルは**曲全体の構造を知らない**。対比(A メロとサビ)を伝える手段が無い。
  - 選別が「骨の折れる」「難しいパズル」だった(一方で「機械の奔流が楽しい」という声もあった)。
  - 「同じレイブの区間を 2 回繰り返したが、**退屈だと気づいた**」→ 2 回目の終わりを Coconet で和声づけし直した(T13)。
  - 創作の「発散と収束」の循環に、「モデルを探し・選ぶ」循環が重なって、速い反復を邪魔した。
- **提言**【一次】
  - 分解可能・操作可能・解釈可能・適応的なインターフェース。
  - 旋律モデルが「A メロかサビか」「次の区間と対比させるか」を受け取れるようにする。
  - 動機・A メロ・サビ、リズムと音高を第一級の部品にする。
  - 有効だったやり方(例: 旋律と歌詞の強勢合わせ)を他の利用者へ見せる。
- 出典
  - https://arxiv.org/abs/2010.05388

### 2.3 Hookpad Aria 論文(Donahue ら、2025)→ 1.1 を参照

### 2.4 Amuse(Kim・Lee・Donahue、CHI 2025 最優秀論文)

- **できること**【一次】画像・文・音声の「着想」から和音進行を提案する(Hookpad と Aria と併用)。
- **LLM の旋律・和音の質の担保**【一次】
  - GPT-4o に和音進行を作らせると、次の 2 つの問題が出た。
    - **多様性が低い**(何度聞いても似た答え=均質化)
    - 実際の音楽の分布からずれる
  - 対策:
    - (1) 1 回の指示で「根音・和音の種類・テンション・進行の型・終止を変えた多数の案」を出させる
    - (2) Hooktheory のデータで学習した和音モデルを事前分布にして、LLM の案を**棄却サンプリング**で選ぶ
  - これで実データとの分布の差(JSD)が縮んだ。
- **事前調査の不満**【一次】
  - Aria の案は「着想(歌詞のテーマ)に合わない」。
  - 案は「好奇心をくすぐる火花」がよく、作りすぎると「創造性が冷める」。
- 出典
  - https://arxiv.org/abs/2412.18940

### 2.5 folk-rnn(Sturm・Ben-Tal ら、JCMS 2017 / Leonardo 2021)

- **利用の実態**【一次】folkrnn.org のログより:
  - パラメータを変えて生成し直した一連の操作は平均 6 回で、全生成の 57% を占めた。
  - 変えたパラメータで最も多いのは温度(40%)。
  - 冒頭の ABC を自分で与える使い方が 20%。
- **編集者が直したところ**【一次】アルバム「Let's Have Another Gan Ainm」では、10 万曲から選び、**すべての曲に手を入れた**。
  - 流れを良くするため、つなぎ目(1 番かっこ・2 番かっこ)と曲の終わりを直した。
  - 「ありふれた型への適合」と「目立つ特別な特徴」の**釣り合い**を取った。構造のために**反復を強めた**箇所もあれば、「**平凡すぎる**」ところの音を変えて特別にした箇所もある。
  - 例: 「3 つ目の応答が 2 小節目と似すぎていて、何も面白くない」→ 呼びかけの反行と組み合わせて書き直した。
- **作曲家の使い方**【一次】
  - 何百も生成して大半を捨てた。
  - 学習データで珍しい条件(珍しい拍子・旋法、長い音や**休符**)で始めると、「様式の約束が弱まるが消えない」領域が見つかった。
  - 生成物を 12 音列のように素材として扱った作曲家もいる。
- **提言**【一次】美を判定する評価器ではなく、次のような「人工の批評家」がほしい。
  - 類似度で創作空間の地図を作る
  - 新しさ・対比を探すときは非類似を使う
  - 受け入れられない出力をその場の入力に基づいて除く
- 出典
  - Ben-Tal・Harris・Sturm「How Music AI Is Useful」Leonardo 54(5), 2021: https://kth.diva-portal.org/smash/get/diva2:1464959/FULLTEXT01.pdf

### 2.6 Sony CSL Paris とプロのアーティスト(Deruty ら、TISMIR 2022)

- **対象**【一次】BassNet / LeadNet(既存の音声トラックに合うベース・リードを作り、潜在空間で変奏を探す)などの試作を、プロに依頼して作品を作ってもらった。
- **知見**【一次】
  - 「生成して選ぶ」と「高次の操作で探る」の 2 通りの使い方がある。
  - 潜在空間は、見える化が無いと「試行錯誤」しかできない。
  - 要望: **過去の操作の記録**、ループ再生しながらつまみを探ること、操作の効果を知覚的な特徴に対応づけて見せること。
  - 「LeadNet で、自分では**思いつかなかった旋律**に出会った」。
  - ベースの出力を 2 オクターブ上げて旋律に使うなど、**想定外の使い方**も起きた。
  - 機械学習の成功基準と、アーティストが感じる価値とが食い違うことがある。
- 出典
  - https://transactions.ismir.net/articles/10.5334/tismir.100

### 2.7 実践する音楽家との共同設計(Krol・Llano・Loor、CHI 2025)

- 【一次】
  - 音楽家は「AI は協働者でなく道具」と考え、「旋律や和音を書くのは我々の持ち物」と**所有感**を強く気にする。
  - 望まれたのは「**変奏**を作る道具」で、変化が「足りない」「多すぎる」両方の不満があった。
    - 「変化の量のスライダー」
    - 「AI が全部解釈する⇔少しだけ取る」を**滑らかに選べる主導権の度合い**
    - 「自分の曲で学習させたい」(個人のモデル)
  - 価値は「全体ではなく**瞬間**」にある。気に入った音だけを選んで切り貼りする。
- 出典
  - https://arxiv.org/abs/2502.09055

### 2.8 Composer's Assistant 2(Malandro、ISMIR 2024、REAPER 用)

- **細かい操作の定義**【一次】
  - 横の音の密度(拍あたりの発音数を 6 段階)
  - 縦の密度
  - **順次進行・跳躍の割合**(1〜2 半音を順次、3 半音以上を跳躍とし、7 段階)
  - 音域
  - **リズムの面白さ**(リズムのベクトルと、それをずらしたものとの自己相関の最大値を 1 から引く。低 / 中 / 高)
  - リズムの明示指定
- 経緯: 温度だけの MMM の利用者調査(2023)で「もっと操作を」と言われたことが出発点。
- 聴取実験では、実在曲との有意差は無かった。
- 【推測】「リズムが単調か」を**自己相関で数える**定義は、Glaux の「4 小節の型の繰り返し」の検出にもそのまま使える。
- 出典
  - https://arxiv.org/abs/2407.14700

### 2.9 Libretto(Xu、arXiv 2026-06)― LLM エージェント作曲の評価

- **構成**【一次】
  - Claude Code(Opus 4.8)をエージェントにして、明示の発音位置を持つ文法で曲を書かせる。
  - 314 曲(Lakh MIDI、8 ジャンル)に対して、リズム・和声・旋律・織り・形式・曲内の変化の **29 軸の百分位**を出す。
    - 軸の例: 順次進行の割合、上行の割合、音程のエントロピー、音域、シンコペーション率、発音位置のエントロピー、自己類似度、新しさ率、異なる小節の割合
  - 百分位の高い・低いは良し悪しではない。**5 以下か 95 以上を「退化した極端」**とし、少しの予算(例: 3 軸)まで許す。
  - 生成→測定→音楽家の言葉での助言(数値の目標を直接は見せない。例: 「織りを疎でなく」「極端な軸を真ん中寄りへ」)→書き直し、を最大 3 回まわす。最良の案を保持する。
  - 丸写しの危険度も測る。
- **結果**【一次】
  - 穴埋めの合格率: 12% → 39%(ループの効果)
  - 曲全体: 62% → 94%
  - 参考例の検索(retrieval)で、曲全体の合格率が 25% → 75%
  - 失敗例: 「和音が 1 小節に 2 つのベルトコンベア」「**音階的で、順次進行ばかりの旋律**」(順次進行が 95 百分位、音の長さの変化が 3 百分位)
  - 映画音楽では、検索なしだと「**上行の順次進行の割合 98 百分位**」に偏った。
- 【推測】Glaux の事例(隣の音の往復だけ、跳躍 0、休み 0.4%、4 小節の型の繰り返し)は、この論文の「退化」とほぼ同じ形。**欠点の数ではなく分布の両端を見る**と検出できる。
- 出典
  - https://arxiv.org/abs/2606.22708

### 2.10 規則を報酬にした RL(Jaques・Gu・Turner・Eck、RL Tuner、arXiv 2016 / ICML 2017 版は「Sequence Tutor」)

- **報酬**【一次】Gauldin の旋律作法の教科書から、次を報酬・罰にした。
  - 調内であること
  - 主音で始まり主音で終わること
  - 同じ音を 4 回を超えて続けない(Gauldin の第一原則: 「過度の同音反復と**静的な輪郭**を避ける」)
  - 1・2・3 拍ずれの**自己相関が 0.15 を超えたら罰**
  - 5 度以上の跳躍は反対方向で解決する。同じ向きに 2 回跳ばない
  - **最高音と最低音はそれぞれ 1 回だけ**
  - 動機(3 種以上の音を含む 1 小節)を奏で、それを繰り返す
- **結果**【一次】元の RNN は「濁ることがあり、たいてい**退屈**。休みを頻繁に置き、同じ音を繰り返し、変化が少ない」。RL 後のほうが聴取実験で有意に好まれた。
- **注意**【一次】著者ら自身が「これらの規則は網羅的でも必須でもなく、**特に面白いものでもない**。伝統的な構造へ導くだけ」と書いている。
  - 【推測】規則だけでは「センス」は出ない。Glaux で言えば「良さ」の加点軸の候補(最高音の唯一性、跳躍の解決、動機の反復、自己相関の上限)にはなる。
- 出典
  - https://arxiv.org/abs/1611.02796

### 2.11 MusicRL(Cideron ら、Google DeepMind、ICML 2024)

- 【一次】
  - MusicLM の公開画面(AI Test Kitchen)で、1 つの指示につき 20 秒の案を 2 つ出す。
  - 利用者は**任意で片方にトロフィー**を付ける。**あえて具体的な指示は出さず**、全体としての好みを集めた。
  - 30 万件の対比較を集めて報酬モデルを作り、RLHF した。
  - 利用者の好みで調整した版は、元のモデルに対して 74% の割合で好まれた。
  - 分析では、「文への忠実さと音質」は好みの**一部しか説明しない**。好みは「音楽性」と強く相関した。
- 出典
  - https://arxiv.org/abs/2402.04229

### 2.12 GenJam(Biles、1994〜。1999 年の報告)― 対話型進化と「適応度のボトルネック」

- 【一次】
  - 人の師匠がソロを聴きながら「g」(良い)/「b」(悪い)を打ち込み、それを適応度にする。
  - 音楽は実時間で 1 つずつ聴くしかないので、**評価が律速**になり、「たいてい単調な失敗作の流れ」を聴き続ける**疲労**が生じる。
  - 人の評価なしで育てる実験: 生成器を改良した後では「ひどくはない」ソロになったが、「**よく訓練されたソロに現れる本当にいい瞬間はめったに出ない**」。
  - わざと悪く育てることも簡単にできた。結論は「趣味は説明できない(no accounting for taste)」。
- 【推測】Glaux の点検が 100 点を付けた単調なリードは、まさに「ひどくはないが、いい瞬間が無い」状態。欠点の減点は「ひどくない」を保証するだけで、「いい瞬間」は人の評価(または好みのモデル)が要る。
- 出典
  - https://genjam.org/wp-content/uploads/2019/07/bilessmc99.pdf

### 2.13 属性で操る研究(言葉の好みを数えられる性質へ)

- 【一次・要約】MusicVAE(Roberts ら、ICML 2018)
  - 属性: 調内の割合、音の密度、**平均音程**、16 分・8 分のシンコペーション。
  - 各属性の上位 4 分の 1 と下位 4 分の 1 の潜在ベクトルの差を「属性ベクトル」とし、足し引きで狙った変化を起こせた。
  - ただし密度を上げると 8 分のシンコペーションが下がるなど、**属性同士が干渉**する。
- 【一次・要約】FIGARO(von Rütte ら、ICLR 2023)は、小節ごとの「専門家の記述」(音の密度・平均音高・平均音価・和音・楽器)を条件に生成する。
- 【一次・要約】MuseCoco(Microsoft、2023)は、文 → 属性(楽器・リズムの強さ・音域・感情・ジャンルなど 12 種)→ 音楽、の 2 段にしている。
- 【推測】「もっと休みを」「もっと跳ねて」のような言葉を**数えられる属性の相対変更**に訳す層は、研究でも標準の作り方。Glaux の原則(相対操作は UI / MCP 層で絶対値にしてから Command へ)と相性がよい。
- 出典
  - https://arxiv.org/abs/1803.05428
  - https://arxiv.org/abs/2201.10936
  - https://arxiv.org/abs/2306.00110

### 2.14 評価指標の研究(補足)

- 【一次・要約】Yang & Lerch(Neural Computing and Applications、2018)は、生成物の集合と学習データの集合で、音高の数・発音間隔などの特徴の**分布を比べて**評価する方法を提案した。
  - 【推測】Libretto の百分位はこの流れの上にある。
- 【一次・要約】Gold・Pearce ら(J. Neurosci. 2019)は、IDyOM で測った予測しにくさ(驚き)が**中程度**の旋律を人が最も好むことを示した(逆 U 字)。さらに不確かさ(エントロピー)との交互作用を報告している。
  - 【推測】「驚きの少なさ」は欠点として数えにくいが、好みの点では損になる。驚きは「少なすぎ」も「多すぎ」も点検すべき。
- 出典
  - https://link.springer.com/article/10.1007/s00521-018-3849-7
  - https://www.jneurosci.org/content/39/47/9397

---

## 3. 横断的な整理

### 3.1 旋律の質をどう担保しているか(5 つの型)

| 型 | 例 | 強み | 弱み |
|---|---|---|---|
| (a) 学習分布+人の選別 | Aria、Magenta、Coconet、Suno | 自然で様式に合う | 平均に寄る・文脈の丸写し・分布外で崩れる・選別が大変 |
| (b) 人が作った語彙・規則 | Scaler Motions、Band-in-a-Box、Captain | 破綻しない | 唯一のものに感じない・意外性が無い |
| (c) 明示のつまみ | Captain(Rests・Leaps)、CA2、Orb、Ableton、Logic | 人の意図を直接伝えられる | つまみの意味が伝わらないことがある(Orb)・干渉する(MusicVAE) |
| (d) 自動の選別 | AISC のキャッチーさ分類器、Amuse の棄却サンプリング、Bach Doodle の連続 5 度 | 人の手間を減らす | 分類器の偏り・「欠点が無い=良い」ではない |
| (e) 分布の中の位置で診断 | Libretto、Yang & Lerch、SongMetrics | 退化を両側で見つけ、直し方を言葉で返せる | 良し悪しではなく「典型からの距離」でしかない |

### 3.2 報告された失敗・不満の類型

1. **平凡・ありふれている**
   - Suno の「予測できる進行」
   - Scaler の「唯一のものに感じない」
   - folk-rnn の「平凡すぎる箇所」
   - LLM の均質化(Amuse)
   - LLM の「音階的な順次進行ばかり」(Libretto)
   - Note RNN の「退屈」(RL Tuner)
2. **反復しすぎ・変化が無い**
   - AIVA の「パターンを繰り返す」
   - 温度が低いと反復的になる(folk-rnn)
   - 「同じ区間を 2 回で退屈」(AISC)
   - Aria が既存の旋律を丸写しする
3. **操作できない・サイコロ**: Cococo、AISC、Aria の「もっと操作を」、Ronchini
4. **量が多すぎる・選別が重い**: Cococo の情報過多、AISC の「骨の折れるパズル」、GenJam の疲労
5. **全部か無しか**: Cococo、Ronchini の「一部だけ変えたい」
6. **文脈・構造を知らない**: Aria の「テーマに合わない」、AISC の「構造を知らない・対比を伝えられない」
7. **分布外の入力で崩れる**: Bach Doodle の連続 5 度、folk-rnn の見慣れない種
8. **使える割合が低い**: AIVA の 15%(MacLeod)、Aria の採用 約 23%(計算)、folk-rnn は全曲に手直し
9. **所有感の喪失の不安**: Krol ら、Cococo(操作の道具で所有感が上がった)

### 3.3 Glaux の事例との対応【推測】

| Glaux で起きたこと | 研究・製品での対応物 |
|---|---|
| 休み 0.4% | Captain の Rests つまみ、Band-in-a-Box の Fills の割合。Glaux の EDM 設定は `breath: false` で休みの点検自体が外れている(house → edm) |
| 隣の音の往復だけ・跳躍 0 | Libretto の「順次進行 95 百分位」の退化、Captain の Leaps、CA2 の跳躍の割合、MacLeod の「適切な場所の跳躍」 |
| 4 小節の型の繰り返し | RL Tuner の自己相関の罰、CA2 のリズムの面白さ、AISC の「2 回で退屈」、folk-rnn の編集者が平凡な応答を書き直した例 |
| 欠点の減点だけで 100 点 | GenJam の「ひどくないが、いい瞬間が無い」、RL Tuner 著者の「規則は面白さではない」 |
| 点数で案を選んだ | AISC の分類器で絞る→**人が選ぶ**。どの製品も最終選択は人 |

---

## 4. 人の好みの取り込み方(整理)

| 方式 | 例 | 知見 |
|---|---|---|
| 複数案から選ぶ | Cococo、Aria(無限)、SongStarter(3 案)、Magenta(生成数)、Melody Sauce(9 パッド) | 選ぶ行為自体が不確かさを減らす(Cococo)。ただし案が似すぎ・ばらばらすぎの両極がある |
| 採用・無視の暗黙の記録 | Aria(31.8 万案中 7.4 万採用)、folk-rnn のログ | 手間ゼロで大量に集まる。RLHF・A/B の土台 |
| 2 案の対比較 | MusicRL(任意のトロフィー、指示なし) | 30 万件。忠実さ・音質では説明しきれない「音楽性」が出た |
| 良い・悪いのキー入力 | GenJam | 評価の疲労が律速。案の変化を大きく・賢くして試行回数を減らすべき |
| 言葉・意味のつまみ | Cococo(普通⇔意外、明るい⇔暗い)、Aria の要望(ジャンル・感情・楽器・構造)、MuseCoco(文→属性) | 意味の付いた軸は初心者にも使いやすい。軸のラベルと実際の変化がずれることもある |
| 数えられる性質のつまみ | Captain(Rests・Leaps・Steps・Density)、CA2、Logic(Complexity) | 意図が直接伝わる。意味の説明と結果の予告が要る(Logic の点の濃淡) |
| 例に似せる・離す | Cococo の Example slider、Interpolate、AIVA の influence、Krol の「個人のモデル」 | 言葉にできない好みを例で伝える |
| 部分の再生成 | Cococo の Infill / Voice Lanes、Aria の穴埋め、Udio / Suno、CA2 | 「全部か無しか」への答え。小さく作って積み上げる流れを作る |
| 変化量・主導権の度合い | Krol(変化量のスライダー、「AI が全部⇔少しだけ」) | 所有感を保つ |

---

## 5. 「センスの良さ」の説明・可視化

- **音階度の色分け**: Hookpad(1〜7 度を虹色)、Captain Melody(和音の音 = 青、2・7 度 = 黄、4・6 度 = 緑)。旋律の各音の「役割」が一目で分かる。
- **曲ごとの指標**: Hooktheory の SongMetrics(旋律の複雑さ、和音と旋律の緊張、進行の新しさ)。**データベース内の相対値**で示す。
- **操作の結果の予告**: Logic の Session Player(Complexity を上げると足される音を薄い点で表示)。
- **軸ごとのヒートマップ**: Libretto(29 軸の百分位、端の軸に点)。「合格か不合格か」を「どの次元を直すべきかの地図」に変える。
- **理由の言語化**: Libretto(数値の目標を見せず、音楽家の言葉で直し方を返す)。Bach Doodle は代理指標(連続 5 度)を品質の目安にした。
- **説明の難しさ**:
  - Bach Doodle の事前テストでは「和声の概念を知らない人が多い」と分かった(短いアニメーションで補った)。
  - Deruty らは、潜在空間は見える化が無いと試行錯誤しかできない、と報告している。

---

## 6. Glaux の共同作業に取り入れられそうな仕組み【推測・提案】

**点検と点数**

- **欠点の減点と別に「典型からの位置」を返す。**
  - ジャンルごとの参照集合(内蔵の例や、ユーザーが選んだ参照曲)に対して、各指標(休みの割合、順次進行・跳躍の割合、音程のエントロピー、音域、シンコペーション、発音位置のエントロピー、自己類似度・異なる小節の割合、最高音の唯一性)の百分位を出す。
  - 5 以下と 95 以上を「退化」として、**両側で**指摘する(Libretto)。
  - 休みが「多すぎ」だけでなく「**少なすぎ**」、跳躍が「多すぎ」だけでなく「**無さすぎ**」も警告にする。
- **1 つの点数で案を自動選択しない。**
  - 点数は「足切り(ゲート)」にだけ使う。
  - 通った案の中から、互いに違う 2〜4 案を人に試聴させる(Cococo、Aria、SongStarter)。
  - 自動で選ぶなら、点数最大ではなく「ゲート通過+多様性」で選ぶ。
- **「良さ」の加点軸を持つ。** 欠点の減点は「ひどくない」の保証にとどまる(GenJam)。候補:
  - 驚きが中程度の音がある(Temperley / IDyOM の情報量、逆 U 字)
  - 最高音が 1 回だけ、山の区間にある
  - 跳躍とその解決がある
  - 動機が反復しつつ最後だけ変わる
  - 1〜3 拍ずれの自己相関が高すぎない(RL Tuner)
  
  点数は、欠点と良さを分けて返す。
- **ジャンル設定の穴をふさぐ。** EDM / ハウスのリードでも、休みの割合と跳躍の割合の**下限**を持つ。`breath: false` は「4 小節超の無休止」の点検だけを外し、休みの割合の下限は残す。
  - 【推測】ハウスのリードにもゲートや休符による「間」がある。具体的な下限値は参照曲で測って決める。
- **分布外・文脈のずれを代理指標にする。** Bach Doodle の連続 5 度のように、「和音と合わない強拍」「ジャンルの音域外」を品質の代理にする。検出したら自動で部分を作り直す。

**操作と好み**

- **言葉 → 属性の相対変更 → 絶対値の Command、の層を MCP に置く。**
  - 例: 「もっと休みを」→ 休みの割合 +15%、「もっと跳ねて」→ シンコペーション +、跳躍 +。
  - 結果は、変わった属性の値(前 → 後)で報告する。
  - Glaux の原則 3(相対操作は UI / MCP で絶対値に)にそのまま沿う。
- **生成道具の引数に、Captain Melody / CA2 相当のつまみを持たせる。**
  - Rests、Leaps / Steps、Density、Range、Syncopation、Rhythmic interest(自己相関で定義)、Contour(Ableton の Shape)
  - AI(LLM)もこのつまみを通して作るので、結果が点検と同じ言葉で説明できる。
- **部分の再生成(infill)を第一級の操作にする。**
  - 「トラック × 小節範囲」を指定して作り直す。前後の文脈を保つ。
  - 「全部か無しか」を避け、少しずつ組み立てる流れを支える(Cococo、Aria、Udio / Suno)。
  - Command と undo で試行の費用を小さく保つ。
- **暗黙の好みを履歴から学ぶ。**
  - Glaux の Session(git ライクな履歴)には「AI の案 → 人の修正」の差分が残る。次の 3 つを記録し、プロジェクトやユーザーの「好みの傾向」(例: 休みを増やして戻しがち、跳躍を足しがち)を要約する。
    - 採用
    - 却下(undo)
    - 採用後の編集量
  - 要約した傾向を、次の生成のつまみの初期値や、点検の百分位の目標帯の補正に使う(Aria のデータの循環、MusicRL)。
- **A/B は軽く、任意に。**
  - 2 案を短いループで聴き比べ、キー 1 つで選ぶ(MusicRL のトロフィー、GenJam の g / b)。
  - 評価は強制せず、回数も絞る(評価の疲労=適応度のボトルネック)。
  - 案同士の差は、聴いて分かるくらい大きくする。
- **例に似せる・離すのつまみ。** 選んだ区間(自分の過去のリードなど)を例にして、「もっと似せる / もっと離す」を指定する(Cococo の例示スライダー)。言葉にできない好みを例で伝える。
- **変化量と主導権の度合いを選べるようにする。** 「変奏の量」スライダーと、「AI が書く範囲」を決める設定を用意する。例: 動機だけ・4 小節・全体(Krol ら、Cococo の Voice Lanes)。案は短く「火花」として出す(Aria、Amuse)。

**可視化と説明**

- **旋律の見える化。**
  - ピアノロールの音を音階度・和音の音かどうかで色分けする(Hookpad、Captain)。
  - 点検結果を軸ごとの帯(百分位)で見せる(Libretto のヒートマップ、SongMetrics)。
  - 「どこが典型の端にいるか」を指して説明する。
- **直し方を言葉で返す。** 数値の目標ではなく、「3〜4 小節目に 8 分休符を入れて息をつかせる」「サビ前で 5 度上へ跳んで戻る」のような具体的な直し方にする(Libretto)。構造上の対比(A メロとサビで密度・音域を変える)も指摘する(AISC)。

**LLM の偏りへの対策**

- 【一次の知見に基づく提案】LLM は均質化しやすく(Amuse)、音階的な順次進行に寄りやすい(Libretto)。対策は 3 つ。
  - (1) 生成前に、ジャンルの実例(内蔵の短い参照フレーズ)を文脈に入れる(Libretto で合格率 25% → 75%)
  - (2) 一度に多様な案を出させる指示にする(Amuse)
  - (3) 生成後に分布の端をゲートで落とし、音楽家の言葉で書き直させるループを最大 2〜3 回にする(Libretto)

---

## 7. 主な出典(年)

- Louie ら「Novice-AI Music Co-Creation via AI-Steering Tools for Deep Generative Models」CHI 2020 / 短縮版 https://ceur-ws.org/Vol-2848/HAI-GEN-Paper-1.pdf
- Suh ら「AI as Social Glue」CHI 2021 https://dl.acm.org/doi/10.1145/3411764.3445219
- Huang ら「The Bach Doodle」ISMIR 2019 https://arxiv.org/abs/1907.06637
- Huang ら「AI Song Contest: Human-AI Co-Creation in Songwriting」ISMIR 2020 https://arxiv.org/abs/2010.05388
- Donahue ら「Hookpad Aria: A Copilot for Songwriters」2025 https://arxiv.org/abs/2502.08122
- Hooktheory ブログ(2024-10)https://www.hooktheory.com/blog/generative-ai-songwriting/ 、SongMetrics https://www.hooktheory.com/song-metrics/about 、フォーラム(2024)https://forum.hooktheory.com/t/introducing-aria-your-new-generative-ai-assistant-in-hookpad/8082
- Kim・Lee・Donahue「Amuse」CHI 2025 https://arxiv.org/abs/2412.18940
- Ben-Tal・Harris・Sturm「How Music AI Is Useful」Leonardo 2021 https://kth.diva-portal.org/smash/get/diva2:1464959/FULLTEXT01.pdf
- Deruty ら「On the Development and Practice of AI Technology for Contemporary Popular Music Production」TISMIR 2022 https://transactions.ismir.net/articles/10.5334/tismir.100
- Krol ら「Exploring the Needs of Practising Musicians in Co-Creative AI Through Co-Design」CHI 2025 https://arxiv.org/abs/2502.09055
- Ronchini ら「AI-Assisted Music Production: A User Study on Text-to-Music Models」CMMR 2025 https://arxiv.org/abs/2509.23364
- Malandro「Composer's Assistant 2」ISMIR 2024 https://arxiv.org/abs/2407.14700
- Xu「Libretto: Giving LLM Agents a Sense of Musical Structure」2026 https://arxiv.org/abs/2606.22708
- Jaques ら「Tuning Recurrent Neural Networks with Reinforcement Learning」2016 https://arxiv.org/abs/1611.02796
- Cideron ら「MusicRL」ICML 2024 https://arxiv.org/abs/2402.04229
- Biles「Life with GenJam」1999 https://genjam.org/wp-content/uploads/2019/07/bilessmc99.pdf
- Roberts ら「MusicVAE」ICML 2018 https://arxiv.org/abs/1803.05428 / von Rütte ら「FIGARO」ICLR 2023 https://arxiv.org/abs/2201.10936 / Lu ら「MuseCoco」2023 https://arxiv.org/abs/2306.00110
- Yang & Lerch 2018 https://link.springer.com/article/10.1007/s00521-018-3849-7 / Gold ら 2019 https://www.jneurosci.org/content/39/47/9397
- 製品:
  - Mixed In Key https://mixedinkey.com/captain-plugins/how-to-guide/captain-melody/
  - Magenta Studio https://magenta.tensorflow.org/studio/
  - Ableton マニュアル https://www.ableton.com/en/live-manual/12/midi-tools/
  - SOS(Ableton 2024-08、Logic 2024-09、Scaler 3 2026-02)
  - PG Music https://www.pgmusic.com/manuals/bbw2024full/chapter9.htm
  - BandLab https://blog.bandlab.com/introducing-songstarter/
  - AIVA 評 https://incompetech.com/music/ai/AIVA/AIVA.html (2019)
  - Production Expert の Suno 評(2025-09)
  - Scaler フォーラム https://forum.scalermusic.com/t/melody-generator/23899
  - Staccato https://staccato.ai/
