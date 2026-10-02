# 旋律・記号音楽の自動評価指標と、その限界

調査日: 2026-09-30 / 対象: Glaux の `critique_melody`(`crates/glaux-core/src/melody.rs`)と `write_melody` の案選び

## 0. 凡例と結論の先取り

**確かめた度合いの印**

- **[一次]** 論文の本文(PDF を文字にして該当箇所を読んだ)や公式ドキュメントで確かめた
- **[一次・抄録]** 抄録(Europe PMC・arXiv など)だけで確かめた。本文の数値は未確認
- **[二次]** 総説や検索結果の要約で確かめた。一次の本文は読めていない
- **[推測]** 筆者(この調査)の推論・設計案。文献の裏付けは無い

**結論の先取り(5 行)**

1. 生成音楽の客観指標(MGEval・MusPy・PCE/GS・FMD など)は **「参照の曲集にどれだけ近いか」** を測る道具として作られている。「高いほど良い」「欠点が無ければ満点」という使い方を想定していない。作った側がはっきりそう書いている(Yang & Lerch)。
2. 人の評価とよく合った例は、**特徴量を「参照の平均からのずれ(両側)」で測った** 場合に多い。Pearce & Wiggins 2007 では音域を「データの平均からの距離」にして R=0.92。Collins ら 2016 ではシンコペーションが「多すぎても少なすぎても」減点で、R²=0.83。
3. 快さは予測しやすさに対して **逆 U 字** になる(Gold ら 2019、Cheung ら 2019)。驚きは「低いほど安全」でも「高いほど面白い」でもなく、**帯の中に入っているか** で測るべき。
4. 規則や代理の点数を最大化すると、無難・反復・退化へ落ちる例が繰り返し報告されている。規則だけで RL すると元のモデルからほぼランダムまで離れる(Jaques ら 2017)。尤度を最大化すると文章が平板・反復になる(Holtzman ら 2020)。代理の報酬を最適化しすぎると本当の品質が下がる(Gao ら 2022)。**点数は「選別のゲート」に使い、最大化の対象にしない** のが安全。
5. 人の好みはばらつきが大きい。30 万件の対比較で学んだ報酬モデルでも一致率は 60%、研究者どうしの一致率も 60% だった(MusicRL)。少ない人手で好みを学ぶなら、**指標を特徴量にした Bradley–Terry(対比較のロジスティック回帰)** が現実的。

---

## 1. 実際に起きた事例(ハウスのリードが 100 点)を文献の言葉で言い直す

`melody.rs` を読んで確かめた、事例の原因と実装の対応:

| 事例の原因 | 実装(melody.rs)での実態 | 文献から見た問題の型 |
|---|---|---|
| EDM では息継ぎの点検を切っていた | `Genre.breath=false`(edm/trap)。休みの割合そのものを測る指標が無い | MusPy の **empty-beat rate** に当たるものが無い。リードに「休みの割合の下限」が要る |
| 句が 1 つだと驚き・句の終わりの点検が働かない | `real.len() >= 2` の中でだけ驚き・句末を見る | 休みで句を切る方法は、休みが無い旋律で**点検そのものが消える**。自己類似(反復構造)で句を切る方法(Dai ら 2020)を代わりに使える |
| 平板の判定が全体の音域だけ | `step_ratio > g.step.1 && range < 5` のときだけ info | 局所の変化(小節あたりの異なる音高の数、輪郭の往復の率)を見ていない。Jazz Transformer は 1 小節・4 小節の窓で PCE を測っている |
| 良さへの加点が無い | `score = 100 - 12*warn - 4*info` | 「欠点が無い = 良い」という片側の設計。Yang & Lerch はこれらの指標が**美しさを測らない**と明言している |
| スタブが全コードに 7 度を積むので強拍の和音音が 100% | `pcs.contains(pitch%12)`。7 度・9 度まで含む和音だと音階の半分以上が「和音の音」になる | **偶然でも当たる割合(ベースライン)を引いていない**。4 音和音なら音階 7 音中 4 音が当たる |
| 各小節が隣の音の往復だけ、4 小節の型の繰り返し | `motif_coverage` の上限は edm で 0.95。`rhythm_reuse` は breath=false だと 0.95 超で info だけ | 反復は EDM では正常(Margulis・Butler)。問題は反復そのものではなく、**反復の中の変化(AA′)と対比が無い**こと |
| 跳躍 0 | 跳躍は「多すぎる」側しか warn しない | 跳躍・驚き・音域はどれも**両側の帯**で見るべき(Pearce & Wiggins、Collins、Gold) |

要するに `critique_melody` は「規則違反の検出器」としては働いているが、**規則をすべて守る退化した旋律**(ほぼ最も確からしい旋律)を見分ける仕組みを持っていない。これは Theis らの言う「尤度が非常に高いのに悪い標本」や、Holtzman らの「最大化すると平板で反復的になる」と同じ構造である。

---

## 2. 客観指標のカタログ

### 2.1 MGEval(Yang & Lerch 2018/2020)[一次]

- **出典**: Li-Chia Yang, Alexander Lerch, "On the evaluation of generative models in music", *Neural Computing and Applications* 32:4773–4784(2020、オンライン 2018)。postprint: https://musicinformatics.gatech.edu/wp-content_nondefault/uploads/2018/11/postprint.pdf / コード: https://github.com/RichardYang40148/mgeval
- **定義**: 9 つの特徴量。音高系は pitch count・pitch class histogram(12 次元)・pitch class transition matrix(12×12)・pitch range・average pitch interval。リズム系は note count・average inter-onset interval・note length histogram・note length transition matrix。
  - **絶対の測定**: 曲集ごとの特徴量の平均・分散。
  - **相対の測定**: 曲集の中の標本どうしの距離(intra-set)と、生成集と訓練集の間の距離(inter-set)を作る。それぞれの分布をカーネル密度推定で滑らかにし、**両者の重なり面積(OA)と KL ダイバージェンス** で「生成集が訓練集と同じ性質を持つか」を測る。
- **分かること**: 生成の集まりが、参照の集まりの低水準の統計(音域・音程・音価の分布)を再現しているか。モデルやデータの癖を見つける「形成的評価」(formative evaluation)。
- **分からないこと**: 作者が明言している。「提案手法は人間水準の創造性の文脈で曲を評価しようとはしておらず、音楽の美的知覚をモデル化しようともしていない」(本文 §1)。1 曲だけの良し悪しも測れない(集まりどうしの比較)。
- **人の評価との関係**: 論文の中では検証していない。尤度系の指標について、Theis らの「ある基準で良いことは別の基準で良いことを意味しない」と「尤度が非常に高い悪い標本」の例を引いている。
- **Glaux への含意**: 片側のしきい値ではなく、**参照の分布との重なり** で見る考え方をそのまま使える。1 曲の評価なら「参照の分布の中でその曲の値が何パーセンタイルか」になる。

### 2.2 MusPy の指標(Dong ら 2020)[一次]

- **出典**: Hao-Wen Dong ほか, "MusPy: A Toolkit for Symbolic Music Generation", ISMIR 2020。https://arxiv.org/abs/2008.01951 / 定義: https://muspy.readthedocs.io/en/latest/metrics.html
- **定義**(ドキュメントどおり):
  - pitch_range、n_pitches_used、n_pitch_classes_used
  - polyphony(鳴っている時刻の平均同時発音数)、polyphony_rate
  - pitch_in_scale_rate(ある音階に入る音の割合)、**scale_consistency**(すべての長調・短調の中で最大の pitch_in_scale_rate)
  - pitch_entropy / **pitch_class_entropy**(音高・音高クラスのヒストグラムのシャノンエントロピー、log₂)
  - **empty_beat_rate**(音の無い拍 ÷ 全拍)、empty_measure_rate
  - drum_in_pattern_rate、drum_pattern_consistency
  - **groove_consistency** = 1 − (1/(T−1)) Σ d(Gᵢ, Gᵢ₊₁)。Gᵢ は小節 i の打点の位置のベクトル、d はハミング距離
- **分かること**: 調性がはっきりしているか、休みがあるか、リズムの一貫性。
- **分からないこと**: どれも 1 曲の中の平均的な性質で、**変化や対比、山、問いと答え** は見えない。groove_consistency は高いほど「一貫」だが、ループの EDM は 1.0 に近づくので **高い = 良い とは言えない**。
- **人の評価との関係**: MusPy 自体は検証していない。
- **Glaux への含意**: `empty_beat_rate` は、事例の「休み 0.4%」をそのまま捕まえる指標。リードでも下限(例: 5〜10%)を置く価値がある [推測]。

### 2.3 MuseGAN の指標(Dong ら 2018)[二次]

- EB(empty bars)、UPC(小節あたりの使われた音高クラスの数)、QN(整った音価の割合)、DP(ドラムの型)、TD(トラック間の調性距離)。FMD の論文が "notes in scale (Dong et al. 2018)" として引いている。
- **Glaux への含意**: **UPC(小節あたりの音高クラスの数)** は「各小節が隣の音の往復だけ」(UPC=2)を直接捕まえる。

### 2.4 PCE・GS・CPI・SI(Wu & Yang 2020, Jazz Transformer)[一次]

- **出典**: Shih-Lun Wu, Yi-Hsuan Yang, "The Jazz Transformer on the Front Line: Exploring the Shortcomings of AI-composed Music through Quantitative Measures", ISMIR 2020。https://arxiv.org/abs/2008.01307 / コード MusDr: https://github.com/slSeanWU/MusDr。Pop Music Transformer(Huang & Yang 2020)系の後続研究も同じ指標を使う。
- **定義**:
  - **H1・H4(pitch class histogram entropy)**: 1 小節・4 小節の窓での音高クラスのエントロピー。低いと調がはっきり、高いと不安定。
  - **GS(grooving pattern similarity)**: 小節の打点の二値ベクトルどうしの一致の度合い。全小節の組で平均する。
  - **CPI(chord progression irregularity)**: 和音の三つ組(連続する 3 和音)のうち、固有なものの割合(%)。
  - **SI(structureness indicator)**: 自己類似行列から作る fitness scape plot で、ある長さの範囲(3〜8 秒、8〜15 秒、15 秒以上)の中で最も目立つ反復の強さ。
- **人の評価と指標の結果**:
  - 聴取実験: 59 人(うち 27 人は音楽経験 4〜5/5)。5 段階で Overall・Impression・Structureness・Richness を評価。生成は実曲より全項目で低い(p<0.05)。生成の Structureness の最高点の曲は 3.14、比べた実曲は 3.54。
  - 客観指標(表 2、pdftotext の列の順が崩れていたので、値の対応は本文の記述と照らして推定): 実曲は H1≈1.94、GS≈0.86、CPI≈40%、SI(3 つとも)≈0.35。生成は H1≈2.2〜2.5、GS≈0.69〜0.76、CPI≈73〜81%、長い SI≈0.10〜0.14。本文は「生成は局所の音高が不安定(H1・H4 が高い)、リズムと和声の一貫性が無い(GS が低く CPI が高い)、中長期の反復構造が無い(SI が低い)」とまとめている。
  - 評価の仕方: **「実データに最も近いチェックポイントを最良とする」**(太字の規則)。高いほど良いのではない。
- **分かること**: このケース(ランダムにさまよう生成)では、指標の方向と人の評価が一致した。
- **分からないこと・限界**: 逆向きの失敗(反復しすぎ・一貫しすぎ)では、GS→1、CPI→0、SI→高、H1→低 と「実データより整って」見える。**片側で使うと、今回のハウスのリードは「実曲より良い」判定になる**。
- **Glaux への含意**: SI と GS は「目標の帯」で使う。窓付きの PCE(H1)は「小節ごとに使う音が少なすぎる」も捕まえられる(下限側)。

### 2.5 FMD(Fréchet Music Distance, Retkowski ら 2024)[一次]

- **出典**: Jan Retkowski, Jakub Stępniak, Mateusz Modrzejewski, "Frechet Music Distance: A Metric For Generative Symbolic Music Evaluation", arXiv:2412.07948(2024-12、v2 2025-01)。
- **定義**: 記号音楽の埋め込み(CLaMP・CLaMP 2、ABC か MIDI)の分布をガウスで近似し、参照集と生成集の間の Fréchet 距離を取る(FID・FAD と同じ式)。
- **検証の結果**:
  - 同じ集からの無作為な部分集合どうしでは小さい(MAESTRO の 1000 曲で 0.28)。ジャンルが違うと大きい(MidiCaps と MAESTRO で 378)。**標本数に強く依存する**(MAESTRO の 100 曲だと 11.14)。
  - 条件付けを強めると値が下がる(MMT: 無条件 363.6 → 16 拍の続き 328.7)。
  - **速度(velocity)にノイズを入れてもほぼ 0 で、感度が無い**。音高のノイズには反応する。
  - 聴取での確かめは、音楽家 5 人の小さな試験だけ(「クラシックのピアノ曲か」の判定)。1 曲ごとの FMD で外れ値を見つけられた。
  - 著者の限界の記述: 埋め込みの学習データに偏る。旋法の違いなど細かな変化への感度はよく分かっていない。**音楽家による大規模な聴取での検証が今後必要**。
- **Glaux への含意**: 集まりの比較には強いが、1 旋律の良し悪しの点検には重い・不安定。「参照の分布からの距離」という考え方は、手作りの特徴量で小さく実装できる [推測]。

### 2.6 FAD 系と人の好みの大規模比較(Grötschla ら 2025、音声)[一次]

- **出典**: Florian Grötschla, Ahmet Solak, Luca A. Lanzendörfer, Roger Wattenhofer, "Benchmarking Music Generation Models and Metrics via Human Preference Studies", ICASSP 2025。https://arxiv.org/abs/2506.19085
- **方法**: 12 のモデルで 6,000 曲。Prolific で 2,500 人から 15,000 件の対比較(「どちらの曲が好きか」の二択、10 秒)。注意の確かめ(白色雑音の曲)付き。Elo(K=8、1 万回のブートストラップ)と **Bradley–Terry の強さ** に変換し、指標との Pearson・Spearman 相関を取った。
- **結果**: FAD の埋め込みによって相関が大きく違う。**音楽で学習した CLAP(LAION-MA)の FAD が最も人と合った**。どの指標も Riffusion を人の評価より悪く見る。
- **Glaux への含意**: 記号ではなく音声の研究だが、**「対比較 → Bradley–Terry → 指標との順位相関」** という指標の検証手順は、Glaux の点数の検証にもそのまま使える。

### 2.7 jSymbolic(McKay ら 2018)[二次]

- **出典**: Cory McKay, Julie Cumming, Ichiro Fujinaga, "jSymbolic 2.2: Extracting Features from Symbolic Music for use in Musicological and MIR Research", ISMIR 2018。https://jmir.sourceforge.net/publications/mckay18jsymbolic.pdf
- **定義**: 246 の固有の特徴(多次元を展開すると 1,497 の値)。音高の統計、旋律、和音と縦の音程、リズム、楽器、テクスチャ、強弱。
- **分かること / 分からないこと**: 様式の分類・コーパス研究のための特徴で、良さの評価のためのものではない。

### 2.8 FANTASTIC(Müllensiefen 2009)と、その後継の統合版 [二次+一次]

- **出典**: Daniel Müllensiefen, FANTASTIC: Feature ANalysis Technology Accessing STatistics (In a Corpus), 2009(R)。統合版: David M. Whyatt, Peter M. C. Harrison, "Computational Features for Symbolic Melody Analysis", arXiv:2608.19061(2026-08)。https://arxiv.org/abs/2608.19061 / https://github.com/dmwhyatt/melody-features
- **定義**: m-type(音程と音価比の n-gram、Glaux の「4 音の形」とほぼ同じ考え方)、輪郭(step contour・interpolation contour)、エントロピーなど。特徴的なのは **2 次の特徴(corpus-relative)** で、「その曲の特徴がコーパスの中でどれだけ典型的か」(文書頻度・TF-IDF・密度)を測る。
- **統合版**: FANTASTIC・SIMILE・IDyOM・jSymbolic・MIDI Toolbox・MUST・Partitura の 7 つを整理して 282 の特徴、Python 実装は 235。検証は民謡の地域分類(欧州か中国か、99.2%)で、**好み・覚えやすさとの関係は検証していない**。
- **人の評価との関係**: 下の Jakubowski ら(耳に残る曲)を参照。「2 次の特徴」が効いている。

### 2.9 IDyOM と Temperley のモデル(驚き・予測)[一次・抄録]

- **IDyOM**: Marcus T. Pearce, "Statistical learning and probabilistic prediction in music cognition: mechanisms of stylistic enculturation", *Ann. N.Y. Acad. Sci.* 1423:378–395(2018)。可変次数のマルコフ(PPM)で、**長期モデル(コーパスで学ぶ)と短期モデル(その曲の中で学ぶ)** を組み合わせ、音ごとの情報量(IC = −log₂ p)とエントロピーを出す。IC は聴き手の「予想外さ」の評定とよく合うと多くの研究で確かめられている(Agres ら 2016 の総説も同じ)。
- **Temperley**: David Temperley, "A Probabilistic Model of Melody Perception", *Cognitive Science* 32(2):418–444(2008)。生成モデルの 3 原則は、(a) 旋律は狭い音域にとどまる、(b) 音程は小さい、(c) 音はキーのプロファイルに従う。Glaux の `surprise()` はこれ。
- **重要な含意** [推測だが、定義から直接言える]: Temperley のモデルは **定常**(曲の中の反復を学ばない)。「狭い音域で、隣の音へ動き、音階の中の音」は、このモデルでは **最も確からしい旋律** になる。つまり事例のハウスのリードは驚きがほぼ最小で、「驚きが高すぎる」側の点検には決して掛からない。IDyOM の短期モデルのように **曲の中の反復を学ぶ** なら、同じ型の 4 回目以降は IC がほぼ 0 に落ちるので、「予測できすぎ」を検出できる。

### 2.10 驚きと快さの逆 U 字(Gold ら 2019、Cheung ら 2019)[一次・抄録/一次]

- **Gold ら 2019**: Benjamin P. Gold, Marcus T. Pearce, Ernest Mas-Herrero, Alain Dagher, Robert J. Zatorre, "Predictability and Uncertainty in the Pleasure of Music: A Reward for Learning?", *J. Neurosci.* 39(47):9397–9409。IDyOM で IC(予測しにくさ)とエントロピー(不確かさ)を測った。研究 1 は 43 人、研究 2 は 27 人(同じ刺激を 7 回)。**IC とエントロピーは好みに対して 2 次(逆 U 字)で効き、1 次より良く当てはまった**。**中くらいの複雑さが好まれる**。不確かな文脈では予測どおりの方が好まれる(相互作用)。繰り返し聴くと好みは下がるが、中くらいを好む傾向は崩れない。(抄録で確認。検索の要約では「低エントロピーの刺激でだけ逆 U 字、高エントロピーでは直線的に下がる」ともあるが、本文は未確認。)
- **Cheung ら 2019**: Vincent K. M. Cheung ほか, "Uncertainty and Surprise Jointly Predict Musical Pleasure and Amygdala, Hippocampus, and Auditory Cortex Activity", *Current Biology* 29(23):4084–4092。Billboard の 745 曲・約 8 万の和音で IDyOM を学習。39 人が 1,039 和音の快さを連続で評定。**快さが高いのは「不確かさが低い文脈で驚きが高い」と「不確かさが高い文脈で驚きが低い」の 2 つ**(回帰面は鞍形)。fMRI で扁桃体・海馬・聴覚野がこの相互作用を反映した。
- **Glaux への含意**: 驚きには **上限と下限の帯** を置く。さらに「どこで驚かせるか」が効く。反復で予測が固まった所(不確かさが低い所)で一度だけ外す、が理論に合う。これは EDM のノートにある「1〜2 小節の動機を繰り返し、最後だけ変える」と同じ形。

### 2.11 構造(自己類似・反復)の指標 [一次/二次]

- **fitness scape plot と SI**(2.4 を参照)。
- **階層的な反復構造(Dai, Zhang, Dannenberg 2020)[一次]**: "Automatic Analysis and Influence of Hierarchical Structure on Melody, Rhythm and Harmony in Popular Music", CSMC-MuMe 2020。https://arxiv.org/abs/2010.07518。POP909 で反復に基づいて句と区間を自動で切り出し、人のラベルと 92〜93% 一致。事実として:
  - **ほとんどの曲で、反復される旋律の句が曲の 50〜90% を覆う**
  - 旋律の句は 4 小節か 8 小節が大半。90% 以上の曲で、旋律を持つ異なる句は 2〜3 種類
  - 区間の終わりで V→I が 58%。句の終わりの V→I の遷移確率は 0.84〜0.94、それ以外の位置では 0.47
  - 全音符以上の長い音のうち、区間の途中の句の終わりにあるのは 6.4%、区間の終わりの句にあるのは 72%。**句の終わりは長い音**(Glaux の ending_ratio の裏付け)
  - 年代が新しいほど、句どうしの類似が下がり(区間の対比が増え)、句の複雑さが上がる
- **Information Rate(Dubnov)[二次]**: 現在と過去の相互情報量。**ランダムな列でも、反復しすぎる列でも最小になり、変化と反復の釣り合いで最大になる**。Variable Markov Oracle で計算できる。「反復は良いが、反復しかないのは悪い」を 1 つの数で表せる候補。

### 2.12 尤度・パープレキシティ [二次]

- Yang & Lerch が引く Theis らの指摘: 高い尤度でも悪い標本がある。Kader & Karmaker 2025 の総説(https://arxiv.org/abs/2509.00051)は、**客観指標と人の知覚の相関の弱さ、文化の偏り、標準化の欠如** を主な限界に挙げている(抄録で確認)。検索の要約では「PPL はモデルの大きさとともに下がり続けるが、知覚される品質を反映しないことがある」という報告も引かれていた(本文未確認)。

---

## 3. 指標と人の評価の関係:実証の結果

### 3.1 合った例(と、その共通点)

| 研究 | 設定 | 結果 | 共通点 |
|---|---|---|---|
| Pearce & Wiggins 2007 [一次] | コラールの旋律。3 つの Markov 系と原曲。音楽の専門家 16 人が Consensual Assessment Technique の変形で評定 | 原曲 > 生成。生成どうしの差は小さく、元にしたコラールの影響の方が大きい。**特徴量で回帰すると R=0.92(調整済み R²=0.81, 28 刺激)**。効いた特徴: 音域(**データの平均からの絶対距離**)、半音階的な音の数、不協和な跳躍の数、句の長さ | 特徴量を**参照からのずれ**で定義 |
| Collins ら 2016 [一次] | ショパンのマズルカ様式。専門家と愛好家の 2 群(各 16 人)、様式の成功を 7 段階 | **段階的回帰で R²=0.83**。相対拍重みエントロピーの係数は負で、**シンコペーションが刺激の平均より多すぎても少なすぎても減点**。キー感のエントロピー(調が定まるか)も効く。専門家どうしの方が評定の一致が高い | **両側のずれ**・調の確かさ |
| Wu & Yang 2020 [一次] | 上の 2.4 | 人の評価の差と、指標の「実曲からの距離」の向きが一致 | **実曲に最も近い = 最良** |
| Grötschla ら 2025 [一次] | 音声。1.5 万件の対比較 | 音楽用 CLAP の FAD が最も相関 | 参照の分布との距離 |

**注意**: 回帰は 28〜32 刺激で 5〜7 変数なので、過学習の危険がある。Collins らも「このモデルは新しい制約の候補を示すためのもの」と書いている。

### 3.2 合わなかった・弱かった例

- **MusicRL(Cideron ら 2024)[一次]**: https://arxiv.org/abs/2402.04229。MusicLM の利用者から 30 万件の対比較を集め、Bradley–Terry で報酬モデルを学習。
  - 報酬モデルの評価集での正解率は **60%**。ゼロから学習すると偶然の水準から上がらなかった
  - 研究チーム自身の好みとデータの一致も **60%**(156 対)。要約タスク(73〜77%)より低く、「音楽の好みの主観性」を示すと著者は書いている
  - テキストとの一致(MuLan スコア)が高い方を人が選んだのは **51.6%で、ほぼ偶然**
  - 音声を 3〜5 秒に切ると正解率は 60→56% に下がった。**長い文脈(= 構造・音楽性)が好みに効いている**
  - それでも RLHF で微調整したモデルは元のモデルに対して 74〜87% の勝率。**弱い信号でも、たくさん集めれば最適化には使える**
- **ヒット予測**: Pachet & Roy 2008 "Hit Song Science Is Not Yet a Science"(ISMIR)[二次]: 3.2 万曲・632 ラベルで、音響特徴からも人手のラベルからも人気は学習できなかった。Frieler, Jakubowski, Müllensiefen 2015 [二次, Jakubowski ら 2017 内の引用]: 266 曲の旋律特徴でヒットと非ヒットを分けた正解率は **52.6%**。Kopiez & Müllensiefen 2011: 14 曲で 100% だが、標本が小さく特殊。
- **覚えやすさ(耳に残る曲)**: Jakubowski, Finkel, Stewart, Müllensiefen 2017, "Dissecting an Earworm", *Psychol. Aesthet. Creat. Arts* [一次]: 3,000 人の調査で耳に残ると名指しされた 100 曲と、人気・様式を揃えた 100 曲を FANTASTIC で比較。交差検証の正解率は **62.5%**。
  - 効いた特徴 1: **大域の旋律の輪郭がポップスのコーパスで典型的**(アーチ形など)。この条件を満たす曲の約 80% が耳に残る曲だった
  - 効いた特徴 2: 輪郭が典型的でない曲では、**転回点の間の平均の傾き(音程パターン)が典型的でない** ほど耳に残る
  - 効いた特徴 3: テンポが速い
  - 要約すると **「典型的な枠 + 1 つの非典型」**
- **認識しやすさ(Hooked)**: Burgoyne ら 2013(ISMIR, "Hooked: A Game for Discovering What Makes Music Catchy")[一次] は、曲の途中から再生して認識までの時間を測るゲーム(のちに約 20 万人)。Van Balen ら 2015(ISMIR, "Corpus Analysis Tools for Computational Hook Discovery")[一次] では、認識しやすい区間は **特徴がコーパスの中で典型的(conventionality)**。さらに **曲の中で代表的な区間(recurrence = 反復される)** ほど認識しやすい。最も強い効果は歌の目立ち。
- **総説の結論**: Ji, Luo, Yang 2020 "A Comprehensive Survey on Deep Music Generation"(arXiv:2011.06801)[一次] の 6.2 節は「多くの客観指標が提案されたが、音楽の品質の量的評価と人の判断の相関はまだ無く、主観評価が欠かせない」。Lerch ら 2025 "Survey on the Evaluation of Generative Models in Music"(arXiv:2506.05104, ACM Computing Surveys)[二次: 要約で確認] も同じ趣旨で、形式や反復のような高水準の特徴を測る指標は「まだ提案されていない」とし、音高の分布を MIDI 番号で取ることも批判している(スケール度で取るべき)。

### 3.3 反復についての実証

- **Margulis 2013**(*Empirical Studies of the Arts* 31(1):45–57)[二次]: 馴染みの無い現代音楽(ベリオ・カーター)に **人工的に反復を差し込むと、楽しさ・興味・芸術性の評定が上がった**。さらに、反復を入れた版の方が「人間の作曲家の作品らしい」と判断された。
- **Dai ら 2020** [一次]: ポップスは反復される句が 50〜90%。
- **Van Balen ら 2015** [一次]: 曲の中で繰り返される区間ほど認識しやすい。
- **Gold ら 2019** [一次・抄録]: 同じ刺激を繰り返し聴くと好みは下がる(曲をまたいだ繰り返しの効果)。
- **含意**: 反復は良さの必要条件に近い。ただし **「反復の割合が高い = 良い」ではない**。Dai らの上限 90% や Information Rate の形(反復しすぎで下がる)が示すとおり、反復と対比の釣り合いが要る。

---

## 4. 点数で選ぶ・最適化すると無難になる(Goodhart の法則)

| 事例 | 何が起きたか | 出典 |
|---|---|---|
| RL Tuner(音楽理論の規則を報酬にした RL)[一次] | 規則: キーに留まる、動機を繰り返す、同じ音を 4 回より多く続けない、跳躍を解決する、など。元の Note RNN は「過度に同じ音を繰り返す音」が 63.3% あり、RL で約 0% になった。ただし **規則の報酬だけで学習すると、元モデルでの log p(a\|s) が平均 −3.65(p≈0.026、38 の選択肢ではほぼ一様ランダム)まで落ちた**。著者は「報酬を簡単に稼ぐ単純な作曲を学ばないよう」元モデルの対数尤度を報酬に足し、KL 正則化した。報酬の重みは指標が最も良くなる値ではなく「最も心地よく聞こえた値」を選んだ。聴取は MTurk で 192 件の対比較 | Jaques, Gu, Turner, Eck, "Tuning Recurrent Neural Networks with Reinforcement Learning"(ICLR 2017 WS)https://arxiv.org/abs/1611.02796 |
| 尤度の最大化で文章が退化 | ビーム探索などの最大化は「平板で、奇妙に反復的な」文章を生む。人間の文章は確率が最大の領域に無い。(検索の要約では、ビーム幅 16 の反復率 28.94% に対し人間 0.28%。本文の表は未確認) | Holtzman ら, "The Curious Case of Neural Text Degeneration", ICLR 2020, https://arxiv.org/abs/1904.09751 [一次・抄録] |
| 代理の報酬の過最適化 | 代理の報酬モデルで RL や best-of-n を強めるほど、本当の(金の)報酬は最初は上がり、その後下がる。関数形は最適化の方法で違う | Gao, Schulman, Hilton, "Scaling Laws for Reward Model Overoptimization", ICML 2023, https://arxiv.org/abs/2210.10760 [一次・抄録] |
| 分布の一致の指標は複製を褒める | 「訓練集を複製すれば完璧な点が取れ、新しさより同調を好む」。条件の性質を学習目標にも使うと Goodhart の法則に当たる、とも | Lerch ら 2025 総説 [二次] |
| 深層学習は複製しがち | 深層学習の系は訓練集の大きな塊を写すことがある。構造を明示的に与える非深層の MAIA Markov と、再実装した Music Transformer が同等 | Yin, Reuben, Stepney, Collins, "Deep learning's shallow gains", *Machine Learning* 2023 [二次: 抄録・報道] |
| Turing テストの偏り | 「区別できない」ことを美的・創造的な成功と取り違え、模倣を過大に褒める | Lerch ら 2025 [二次]、Ariza 2009(Yang & Lerch 内の引用)、Agres ら 2016 [一次] |

**Glaux の事例との対応** [推測]: `write_melody` は複数の案から最高点を選ぶので、best-of-n の最適化そのもの。点数が「欠点の不在」だけだと、**最も規則的で最も確からしい案(= 退化した案)** が勝ちやすい。Jaques らの「規則だけ RL」と同じ構造。

### 設計上の工夫(文献に出てくるもの)

1. **参照の分布との距離で測る**(MGEval の OA/KLD、FMD、Pearce & Wiggins の「平均からの距離」、Collins の両側の係数)。
2. **データの事前分布に錨を下ろす**(Jaques の log p + KL)。規則を満たすだけの退化を防ぐ。
3. **逆 U 字の目標**(Gold、Cheung、Information Rate)。
4. **多様性・独自性の罰則**(Yin らの originality report、nucleus sampling の発想)。
5. **最適化を弱める**(best-of-n の n を小さく、上位から確率的に選ぶ)(Gao ら)。
6. **人の対比較で重みを学ぶ**(MusicRL、Grötschla の Bradley–Terry)。

---

## 5. 人の評価の集め方と、少ない人手での好みの学習

### 5.1 方法の種類 [一次: Ji ら 2020 §6.2.1 / Agres ら 2016 / Lerch ら 2025 は二次]

- **Ji ら 2020 の整理**: (1) Turing テスト、(2) 2 つ以上から好きな方を選ぶ、(3) 並べて −1〜+1 で評価、(4) 複数案の順位付け、(5) 観点ごとの 5 段階 MOS など。条件は、十分で多様な人数、素人と専門家の混在、同じ環境、同じ指示。聴き疲れで判断の信頼性が落ちる。
- **Agres, Forth, Wiggins 2016**("Evaluation of Musical Creativity and Musical Metacreation Systems", *ACM Computers in Entertainment* 14(3))[一次]: 外部評価として、行動テスト、**Consensual Assessment Technique(専門家の独立の評定)**、質問紙と相関研究、生理計測、脳波。内部評価(システムの自己省察)として、**予測と期待のモデル(IDyOM の IC とエントロピー)** を挙げている。IC は予想外さの評定とよく合い、系列全体の情報量は記憶にも影響する(複雑な刺激は再認が悪い)。
- **Lerch ら 2025** [二次]: MUSHRA(ITU-R BS.1534)は MOS より少ない人数で有意差が出やすい。評価の観点・尺度・設計が研究ごとにばらばらで標準が無い。Chu ら 2022 は観点を 8 つにまとめた(Overall・Melodiousness・Naturalness・Correctness・Structureness・Rhythmicity・Richness・Creativity)。
- **Yang & Lerch** [一次]: 2 つのモデルの好みの比較は **相対の差しか分からず、絶対の品質は測れない**。統計的な有意性の報告が少ないこと自体が厳密さの欠如だと指摘している。
- **大規模の対比較**(Grötschla、MusicRL)は二択、10〜20 秒の断片、注意の確かめ付き、Elo / Bradley–Terry への変換。

### 5.2 少ない人手での好みの学習 [推測を含む]

- **Bradley–Terry** は「案 A が B より好まれる確率 = σ(s_A − s_B)」。s を指標の線形和 wᵀx にすれば、**特徴量の差 x_A − x_B に対するロジスティック回帰** になる。変数が 10〜20 なら、数十〜数百の対比較で重みの向きは見えてくる [推測。音楽での具体的な必要数を示した一次資料は見つけていない]。
- MusicRL の 60% という一致率が示すとおり、**個々の判断はノイズが大きい**。同じ人の好み(Glaux の利用者本人)に絞れば一致は上がる見込み [推測]。
- 専門家の評定は一致が高い(Collins ら)。CAT では少人数(10〜16 人)で使われている(Pearce & Wiggins、Collins)。
- **LLM に判定させる** ことについては、記号音楽での人との相関を示した一次資料は見つからなかった。ABC-Eval(arXiv:2509.23350)などの理解ベンチマークはあるが、判定者としての妥当性は未検証 [二次]。**生成した LLM 自身が評価すると同じ盲点を共有する** 危険がある [推測]。

---

## 6. 指標の早見表(何が分かり、何が分からないか)

| 指標 | 分かること | 分からないこと / 罠 | 人の評価との関係 |
|---|---|---|---|
| 音域(pitch range) | 声域・平板さの粗い目安 | 局所の平板さ(全体が広くても小節ごとは往復だけ) | 平均からの距離で強い予測子(Pearce & Wiggins) |
| 順次・跳躍の割合 | 歌いやすさ | 跳躍 0 は「安全」に見える | 単独の相関は未確認 |
| PCE(1・4 小節の窓) | 調の明確さ・局所の多様さ | 低すぎると単調 | 実曲との距離で人の評価と同じ向き(Wu & Yang) |
| scale consistency | 音階外れ | 調性音楽ならほぼ 1 | — |
| 強拍の和音音の割合 | 和声との整合 | **和音の構成音が多いと偶然でも高い** | Pearce らの「半音階的な音」は強い負の予測子 |
| empty beat rate | 休み・呼吸 | ジャンルで大きく違う | — |
| groove consistency / GS | リズムの一貫性 | 高すぎるとループ | 実曲との距離で使う |
| 動機の反復率 / SI | 構造・覚えやすさ | 高すぎると退化 | 反復は認識のしやすさ・楽しさを上げる(Van Balen、Margulis) |
| CPI | 和声進行の一貫性 | — | 実曲との距離で使う |
| 驚き(IC、Temperley / IDyOM) | 予想外さ | 定常モデルは反復を学ばない | **逆 U 字**(Gold、Cheung) |
| 典型性(2 次の特徴) | コーパスの中の普通さ | 典型的すぎると無個性 | 耳に残る曲 = 典型の枠 + 1 つの非典型(Jakubowski) |
| FMD / FAD | 集まりの分布の近さ | 1 曲には不向き・標本数に依存 | 埋め込み次第。音楽 CLAP の FAD は良い(Grötschla) |
| 尤度・PPL | データへの適合 | 最大化すると退化 | 品質を反映しないことがある |
| Information Rate | 反復と変化の釣り合い | 実装がやや重い | 直接の聴取検証は未確認 [二次] |

---

## 7. Glaux の critique_melody の設計の見直し案

`melody.rs` の実装(2026-09-30 時点)を前提にした提案。**すべて [推測](文献に基づく設計案)**。

### 7.1 点数の形を変える:「ゲート × 良さ」、満点は欠点の不在では取れない

- `score = 100 − 12·warn − 4·info` をやめ、3 段にする:
  1. **退化のゲート(hard gate)**: 下の 7.2 のどれかに当たったら、案選びでは最下位にする(点は例えば 40 を上限)。欠点の減点とは別に扱う。
  2. **帯からのずれ(欠点)**: 各指標を、ジャンルの目標の帯(下限・上限)からの距離で連続的に減点する。片側のしきい値はやめ、**すべて両側** にする(Pearce & Wiggins・Collins・Wu & Yang の「実曲に近いほど良い」)。距離は帯の幅で正規化する(z 値のような形)。
  3. **良さ(加点)**: 7.4 の項目を 0〜1 で測って足す。**欠点 0 でも良さが 0 なら 60〜70 点止まり** にし、100 点は良さがそろったときだけにする。
- 結果の JSON には、合計点とは別に `defects` / `qualities` / `gates` を分けて返す。AI が「何を足せば良くなるか」を読めるようにする。

### 7.2 退化の検出(今回の事例を確実に落とす)

| 検出 | 定義の案 | 事例での値の見込み |
|---|---|---|
| 小節あたりの音高クラスの数(UPC、MuseGAN) | 音のある小節の中央値が 2 以下なら退化 | 2(隣の音の往復) |
| 輪郭の往復の率 | 連続する 3 音で向きが反転し、かつ最初と 3 番目の音が同じ(a-b-a)割合。0.5 を超えたら退化 | ほぼ 1 |
| 局所の音域 | 1〜2 小節の窓の音域の中央値が 2 半音以下 | 1〜2 |
| 休みの割合(empty beat rate / 休符の時間の割合) | リード・歌では下限(例 5%)。EDM でも「1 句 8 小節で休み 0」は info 以上 | 0.4% |
| 圧縮率・異なる n-gram の割合 | (音程, 音価) の 4-gram の異なる種類 ÷ 全数が極端に低い。または LZ 圧縮率 | 極端に低い |
| 完全反復の連続 | 同じ 1〜4 小節が変化無しに 4 回以上続き、その後にも変化が無い(**AA′ の ′ が無い**) | 当たる |
| 自己予測の IC(短期モデル) | その旋律だけで学ぶ n-gram / PPM の IC の後半の平均がほぼ 0 | ほぼ 0 |

### 7.3 既存の指標の直し

1. **句の切り方の代わりを用意する**: 休みで切った句が 1 つ(または 8 小節を超える句がある)なら、**反復構造で 2・4・8 小節に切る**(Dai ら 2020 の考え方。最小限なら 4 小節ごとに切るだけでもよい)。こうすれば驚き・句の終わりの点検が常に働く。`real.len() >= 2` の条件で点検が消える作りをやめる。
2. **息継ぎを EDM でも「休みの量」として見る**: `breath=false` は「4 小節を超える句」の warn を切るだけにし、休みの割合の下限は全ジャンルに置く(EDM・トラップは下限を低めに)。トラップのノートの「休符多め」とも合う。
3. **平板の判定を局所化する**: 全体の音域ではなく、小節の窓の UPC・局所の音域・窓付きの PCE(Wu & Yang の H1)で見る。`range < 5` の条件を外す。
4. **跳躍は両側に**: `leap_ratio` の下限を置く(pop なら 5 半音以上が 0 なら info、句の頭か山の前に 1 つを提案)。
5. **強拍の和音音はベースラインを引く**:
   - 和音の構成音を **三和音(1・3・5)と、テンション(7・9・…)に分けて** 数える。テンションは 0.5 として数える
   - さらに「偶然で当たる割合」= |和音の音 ∩ 音階| ÷ |音階| を引いた **上乗せ(lift)** で判定する。7 度・9 度の和音だと偶然でも 4/7≈57% 当たるので、100% でも上乗せは +43%。三和音なら偶然 3/7≈43%
   - 強拍の音が **いつも同じ音高**(和音の中の 1 音に張り付く)なら info
6. **驚きを両側・位置付きに**:
   - 句ごとの最大 IC の下限・平均の上限は今もあるが、**下限を平均にも置く**(逆 U 字)
   - Temperley(定常)に加えて、**曲の中の反復で学ぶ短期モデル** の IC を出す。IDyOM の LTM+STM の簡易版で、長期は Temperley、短期は曲の中の n-gram
   - 「予測が固まった後(同じ型が 2〜3 回続いた後)に 1 回の驚き」を良さとして数える(Cheung ら の「不確かさが低い文脈での驚き」)
7. **反復は帯で**: `motif_coverage` は下限 30% と上限をすでに持っている。加えて「反復の中の変化」(下の AA′)を測る。上限 0.95(edm)は、AA′ の変化があれば許し、無ければ 0.8 程度で warn にする、という条件付きにする。
8. **リズムの再利用も帯で**: `rhythm_reuse` の 0.95 超を、breath=false でも「最後の小節が変わっていなければ」warn にする(EDM のノートの「最後だけ変える」を点検に落とす)。

### 7.4 良さの加点(良さを測る)

| 良さ | 測り方の案 | 根拠 |
|---|---|---|
| 山(輪郭のアーチ) | 句・区間ごとの最高音の列が、山の区間に向けて上がり、その後に下がる。区間の中央値の音高の差が 2〜5 半音 | Jakubowski(アーチ形の典型的な輪郭)、既存の最高音の点検 |
| 問いと答え | 2 つの句が **同じ頭(最初の 2〜4 音の形が一致)で、違う終わり**。前の句の終わりは V・2 度・5 度、後の句は I・1 度 | Dai ら(句末の V と区間末の I の使い分け) |
| AA′(反復 + 最後だけ変える) | 反復される単位(1・2・4 小節)の最後の出現で、終わりの 1〜2 拍が変わる | EDM のノート、Information Rate、Margulis |
| 1 つの非典型(フック) | 典型的な枠(音域・輪郭・音程の分布が帯の中)の中に、帯の外の要素(跳躍・食い・和音の外の音・高い連打)が **1〜2 か所だけ**、しかも反復される動機の中にある | Jakubowski(典型の輪郭 + 非典型の音程パターン)、Cheung |
| 句の終わりの伸びと終止 | 既存の ending_ratio に加え、句末の音が和音の音(区間末は主音)か | Dai ら(区間末の長い音 72%) |
| 区間の対比 | ヴァースとサビで中心の音高・音価の分布・リズム密度が違う | Dai ら(対比の増加の傾向) |

各項目は 0〜1 で、加点は合計 30〜40 点程度に抑える(良さの測り方は減点より不確かなので)。

### 7.5 目標の帯をどう決めるか

- 今の `Genre` の固定値を「帯」として続けて使い、**参照の旋律集で校正** する。ライセンス上問題の無い旋律、例えば自作の良い例・悪い例や、Essen 民謡集・POP909 の統計を使い、特徴量の分布の 10〜90 パーセンタイルを帯にする。
- 1 旋律の判定は「その値が参照の分布の何パーセンタイルか」で出す(MGEval の相対の測定を 1 曲に縮めた形)。

### 7.6 write_melody の案選び(過最適化を避ける)

- **最大の点を取る** のではなく、(1) ゲートを通った案だけ残し、(2) 点の上位 k 案から、**前に選んだ案・他の案と最も違うもの**、または確率的に選ぶ。best-of-n の過最適化(Gao ら)と、「最も確からしい = 退化」(Holtzman ら)を避けるため。
- 規則で直す道具(develop_motif など)の結果にも、**元の案からの距離の罰則**(Jaques の KL の考え方)を入れると、規則を満たすためだけの単純化を防げる。
- 複数案を返すときは、点数が近ければ **多様な 2〜3 案** をユーザーに聴かせる。

### 7.7 点検器そのものの検証(Goodhart に対する保険)

1. **意地悪な例の回帰テスト**: 今回のハウスのリード(往復・休み無し・4 小節ループ・跳躍 0・7 度のスタブ)を `melody.rs` のテストに入れ、**70 点未満** などを保証する。同様に「全部 8 分の音階」「1 音の連打のみ」「ランダムな跳躍」も入れる。FMD 論文の感度テストのように、良い例に摂動(休みを消す、音高を往復にする、変化を消す)を加えて **点が下がる向き** を確かめる。
2. **人の対比較で検証**: アプリ内で 2 案を聴いて選ぶ UI(※ 新しい画面の追加は保留の方針があるので、まず既存のチャット・MCP の流れで「A と B のどちらが良いか」を記録するだけでよい)。集めた対比較で、点数の差と選択の一致率(= Bradley–Terry の正解率)を測る。**60% を大きく超えなければ、点数は選別に使わない方がよい**(MusicRL の水準)。
3. 十分に集まったら、指標を特徴量にした **Bradley–Terry(ロジスティック回帰)で重みを学ぶ**。利用者本人の好みに合わせられる。
4. LLM(Claude)が自分の書いた旋律を言葉で評価するのは補助に留める。数値の点検と対比較が主。

### 7.8 優先順位(小さく始める順)

1. 7.2 の退化のゲート(UPC・往復の率・休みの割合・AA′ の欠如)と、事例の回帰テスト ― 実装が小さく、今回の事例を直接直す
2. 7.3-1 句の代わりの切り方(4 小節ごと)― 驚き・句末の点検を常に働かせる
3. 7.3-5 和音音のベースライン(三和音とテンションの分離・上乗せ)
4. 7.1 の点数の形(ゲート × 帯のずれ × 良さ)と 7.4 の加点(AA′・問いと答え・山から)
5. 7.6 の案選び(上位 k から多様なもの)
6. 7.3-6 の短期モデルの IC、7.5 の帯の校正、7.7 の対比較による検証と重みの学習

---

## 8. 出典一覧

一次(本文・公式を読んだ):

- Yang, L.-C., Lerch, A. (2020/2018). On the evaluation of generative models in music. *Neural Comput. Appl.* 32:4773–4784. https://musicinformatics.gatech.edu/wp-content_nondefault/uploads/2018/11/postprint.pdf ・ https://github.com/RichardYang40148/mgeval
- Dong, H.-W. ほか (2020). MusPy. ISMIR. https://arxiv.org/abs/2008.01951 ・ https://muspy.readthedocs.io/en/latest/metrics.html
- Wu, S.-L., Yang, Y.-H. (2020). The Jazz Transformer on the Front Line. ISMIR. https://arxiv.org/abs/2008.01307
- Retkowski, J., Stępniak, J., Modrzejewski, M. (2024). Frechet Music Distance. https://arxiv.org/abs/2412.07948
- Grötschla, F. ほか (2025). Benchmarking Music Generation Models and Metrics via Human Preference Studies. ICASSP. https://arxiv.org/abs/2506.19085
- Cideron, G. ほか (2024). MusicRL: Aligning Music Generation to Human Preferences. https://arxiv.org/abs/2402.04229
- Jaques, N., Gu, S., Turner, R. E., Eck, D. (2017). Tuning Recurrent Neural Networks with Reinforcement Learning. https://arxiv.org/abs/1611.02796
- Pearce, M. T., Wiggins, G. A. (2007). Evaluating Cognitive Models of Musical Composition. IJWCC. https://computationalcreativity.net/ijwcc07/papers/pearce-wiggins.pdf
- Collins, T., Laney, R., Willis, A., Garthwaite, P. H. (2016). Developing and evaluating computational models of musical style. *AI EDAM* 30(1):16–43. https://www.cambridge.org/core/services/aop-cambridge-core/content/view/2D13038AEC3BB894F1345C63F74F6CF4/S0890060414000687a.pdf/developing_and_evaluating_computational_models_of_musical_style.pdf
- Jakubowski, K., Finkel, S., Stewart, L., Müllensiefen, D. (2017). Dissecting an Earworm. *Psychol. Aesthet. Creat. Arts*. https://www.apa.org/pubs/journals/releases/aca-aca0000090.pdf
- Van Balen, J., Burgoyne, J. A., Bountouridis, D., Müllensiefen, D., Veltkamp, R. (2015). Corpus Analysis Tools for Computational Hook Discovery. ISMIR. https://archives.ismir.net/ismir2015/paper/000148.pdf
- Burgoyne, J. A., Bountouridis, D., Van Balen, J., Honing, H. (2013). Hooked. ISMIR. https://archives.ismir.net/ismir2013/paper/000101.pdf
- Dai, S., Zhang, H., Dannenberg, R. B. (2020). Automatic Analysis and Influence of Hierarchical Structure on Melody, Rhythm and Harmony in Popular Music. https://arxiv.org/abs/2010.07518
- Cheung, V. K. M. ほか (2019). Uncertainty and Surprise Jointly Predict Musical Pleasure…. *Curr. Biol.* 29:4084–4092. https://www.stefan-koelsch.de/papers/cheung_harrison_meyer_pearce_haynes_koelsch_2019_uncertainty_and_surprise_jointly_predict_musical_pleasure_and_amygdala_hippocampus_and_auditory_cortex_activity_current_biology_29.pdf
- Agres, K., Forth, J., Wiggins, G. A. (2016). Evaluation of Musical Creativity and Musical Metacreation Systems. *ACM CIE* 14(3). https://research.gold.ac.uk/id/eprint/21158/1/agress-forth-wiggins-2017-preprint.pdf
- Ji, S., Luo, J., Yang, X. (2020). A Comprehensive Survey on Deep Music Generation. https://arxiv.org/abs/2011.06801

一次・抄録:

- Gold, B. P. ほか (2019). Predictability and Uncertainty in the Pleasure of Music. *J. Neurosci.* 39(47):9397. https://www.jneurosci.org/content/39/47/9397
- Holtzman, A. ほか (2020). The Curious Case of Neural Text Degeneration. ICLR. https://arxiv.org/abs/1904.09751
- Gao, L., Schulman, J., Hilton, J. (2023). Scaling Laws for Reward Model Overoptimization. ICML. https://arxiv.org/abs/2210.10760
- Whyatt, D. M., Harrison, P. M. C. (2026). Computational Features for Symbolic Melody Analysis. https://arxiv.org/abs/2608.19061
- Kader, F. B., Karmaker, S. (2025). A Survey on Evaluation Metrics for Music Generation. https://arxiv.org/abs/2509.00051
- Temperley, D. (2008). A Probabilistic Model of Melody Perception. *Cognitive Science* 32. https://onlinelibrary.wiley.com/doi/10.1080/03640210701864089
- Pearce, M. T. (2018). Statistical learning and probabilistic prediction in music cognition. *Ann. N.Y. Acad. Sci.* 1423. https://nyaspubs.onlinelibrary.wiley.com/doi/10.1111/nyas.13654

二次(総説・検索の要約・報道):

- Lerch, A. ほか (2025). Survey on the Evaluation of Generative Models in Music. *ACM Comput. Surv.* https://arxiv.org/abs/2506.05104
- Yin, Z., Reuben, F., Stepney, S., Collins, T. (2023). Deep learning's shallow gains. *Machine Learning*. https://link.springer.com/article/10.1007/s10994-023-06309-w ・ https://www.york.ac.uk/news-and-events/news/2023/research/ai-generated-music-inferior-to-human-composed/
- Pachet, F., Roy, P. (2008). Hit Song Science Is Not Yet a Science. ISMIR. https://zenodo.org/records/1417723
- Margulis, E. H. (2013). Aesthetic Responses to Repetition in Unfamiliar Music. *Empirical Studies of the Arts* 31(1):45–57. https://journals.sagepub.com/doi/10.2190/EM.31.1.c
- McKay, C., Cumming, J., Fujinaga, I. (2018). jSymbolic 2.2. ISMIR. https://jmir.sourceforge.net/publications/mckay18jsymbolic.pdf
- Dubnov, S. ほか(Information Rate / Variable Markov Oracle)。https://escholarship.org/uc/item/7tg5c8rb
- Chu, H. ほか (2022). An Empirical Study on How People Perceive AI-generated Music. CIKM. https://dl.acm.org/doi/10.1145/3511808.3557235
- Sturm, B. L., Ben-Tal, O. (2017). Taking the Models back to Music Practice. *J. Creative Music Systems* 2(1).

未確認(記憶に基づく。今回は一次資料に当たれなかった): Biles の GenJam(1994)の「fitness bottleneck」(人が評価する対話型 GA では、人の評価が律速になる)。MuseGAN の指標の正確な定義(Dong ら 2018, AAAI)。
