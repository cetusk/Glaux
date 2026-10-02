# 良いリード・フック・トップラインの作り方とよくある失敗 ― ジャンルと実践の現場から

調査日: 2026-09-30 / 対象: Glaux の AI 作曲(MCP の道具)向け
表記: **[一次]** = 作曲家・プロデューサー本人の発言、または原著の研究論文を読んだもの。**[二次]** = 教材・ブログ・解説記事(書き手の意見やまとめ)。**[推測]** = 調査者(私)の推論・提案。
グリッド表記: 1 小節 = 16 分音符 16 マス。`x` = 音の頭、`-` = 前の音を伸ばす、`.` = 休み。小節の区切りは `|`。

---

## 0. 要点(先に結論)

1. **休みは「音の数」ではなく「音の長さ(ゲート)」の問題だった。** 今回のリードは 1 小節 6 音 ≒ 0.67 拍/音で、Hooktheory の集計で Avicii「Levels」0.63、Alan Walker「Faded」0.70、SHM「Don't You Worry Child」0.84 と同程度。音の数はヒット曲並みで、違いは「全部の音を次の音までつないだ(休み 0.4%)」ことと「音域・跳躍・リズムの型が無い」こと。
2. **ヒット曲のフック・サビは 1 セクションで 1 オクターブ前後以上動く。** Hooktheory の数値: Animals のサビ G#4–C#6(17 半音)、DYWC のサビ F#4–B5(17 半音)、Faded のサビ・Strobe 12 半音(半音数は音名から計算)。ただし Levels の有名なリード(Instrumental)は 8 半音で、狭くても「ペンタトニックの跳躍+決まったリズム」で成り立っている。狭さそのものより「跳躍ゼロ・山ゼロ」が問題。
3. **「短い動機(1〜2 小節)を作り、繰り返し、最後だけ変える」がジャンルを問わず最も多い処方。** Avicii の AAAA / AAAB(MusicRadar)、Hyperbits の「1 小節を複製して 2 小節目で 1 つだけ変える」、メロディックテクノの「2 小節を繰り返して終わりだけ変える」、ポップの SRDC(提示・反復・逸脱・結論)。今回は 4 小節を丸ごと繰り返しただけで「最後の変化」が無い。
4. **休符は教材がそろって挙げる第一の直し方。** 日本の初心者向け記事の「失敗あるある」1 番目が「休符がない」(SRM、2022)。トランス教材は「沈黙は音と同じくらい大事、息をするフレーズ」(Steve Allen、2026)。近藤浩治(任天堂)の作曲はマリオの走り・跳びのリズムに合わせる作業だった(LOC/GDC 2007)。
5. **フレーズを小節の頭から始めない。** Pat Pattison の「前が重い/後ろが重い」フレーズ、Hal Galper の「フレーズは裏拍から始めて 1 拍目を目的地にする」、日本の初心者記事の「いつもドアタマから始める」失敗。今回の型は毎小節の頭から始まっていると推測される。
6. **山(最高音)は 8 小節に 1〜2 回。** トランス教材の明示的な規則。最高音を決め、そこへ向かって上がり、戻る(Huron の「跳躍の後は逆向き」)。
7. **動機の変え方には名前の付いた道具がある。** 反復/似た反復(和音に合わせて音だけ変える)/ゼクエンツ(形を保って上げ下げ)/引く/音価を変える/ずらす/音を変える/足す(Shed the Music)。今回のドロップは「3 度上げただけ」=和音に合わせないゼクエンツで、道具の 1 つしか使っていない。
8. **リズムの定番は 3-3-2(トレシーロ)。** ビルボード上位 20 曲の調査で 2010 年以降に増え 2018 年頃に最多(SMC 2024)。ダンス・ポップのリードとドロップの基本型。
9. **「ドロップ=ブレイクダウンの旋律の続き」ではなくてよい。** Martin Garrix「Animals」はドロップで全く新しいリフに切り替える(Computer Music、2016)。ブレイクダウンでは旋律を不完全に(強拍だけ・1 オクターブ上の山だけ)見せて、ドロップで全体を出す手もある。
10. **研究でも「AI(LLM)の旋律は音域が狭く音程が小さい」。** ChatGPT 3.5 が作った MIDI は人の曲(Lakh)より音域が狭く、音程が小さく、リズムの一貫性(同じリズムの繰り返し)が弱い。音楽経験者ほど満足度が低い(KTH 学位論文、2023)。今回の症状(隣の音の往復・跳躍ゼロ)はこの傾向と一致する。
11. **表情はすべての音に同じにしない。** ビブラートは伸ばす音だけ、しかも遅らせて掛ける。ピッチベンドは短い音には 1〜2 半音の短いもの、長い音には大きく(SoundBridge、2023)。
12. **ジャンルの語法は「リズムの型」でかなり決まる。** 日本の初心者記事は「R&B なら 16 分、ポップ・ロックは 8 分、童謡は 4 分」。J-POP は 70 年代に基本が 4 分から 8 分へ移り、90 年代に言葉を詰め込む新しいリズム型が 6 割になった(畦地、2007)。ハウスは裏拍のスタブ、テックハウスは乾いた短いスタブと声のチョップ、メロディックテクノは 4 小節に 4〜8 音。

---

## 1. 今回の事例の診断(数値で比べる)

| 指標 | 今回のリード | 参考値(出典) | 評価 |
|---|---|---|---|
| 拍/音(音の密度) | 約 0.67(1 小節 6 音) | Levels 0.63、Faded 0.70、DYWC 0.84、Animals 0.41、Strobe 1.45(Hooktheory TheoryTab) | **問題なし**。音の数は普通 |
| 休みの割合 | 0.4% | コーパスの数値は見つからず。トランスのプラックはゲート約 45%、ローリングアルペジオは 85% 以上(myloops の教材) | **問題**。ゲート約 100% のレガートは「プラック」「スタブ」系の語法から外れる |
| 1 小節内の音域 | 平均 3.3 半音 | 2 小節単位の音域では 2010 年代のポップは 1960 年代より**広い**(Clark & Arthur 2023)。セクション全体では 12〜17 半音が多い(Hooktheory) | **問題** |
| 跳躍 | 0 回 | EDMProd は「6〜8 割が順次進行」、トランス教材は「順次中心で時々 4 度・5 度・オクターブ」 | **問題**。跳躍が 2〜4 割あってよい |
| 形 | 4 小節をほぼそのまま反復 | AAAB(Avicii)、繰り返して最後だけ変える(Hyperbits・メロディックテクノ)、ABCB(Shed)、SRDC | **問題**。「最後の変化」が無い |
| ドロップ | 同じ形を 3 度上 | Animals は新しいリフ。トランスは「ブレイクダウンで不完全に見せ、後で完全に」 | **問題** |
| 表情 | 全音に同じビブラート | 伸ばす音だけ・遅らせて(SoundBridge ほか) | **問題** |
| 和音との関係 | C–C–D over Am–F–C–G | C は Am・F・C の構成音、D は G の構成音 | おおむね問題なし |

[推測] 今回の症状は「和音の構成音に近い音を選び、隣の音へ動き、次の音まで伸ばす」という、ハーモニー的には安全だがリズムと形の無い書き方。KTH の研究(§5)が示す LLM の傾向(狭い音域・小さい音程)と一致する。

---

## 2. 研究・統計(一次研究を読んだもの)

### 2.1 ビルボード上位曲の旋律は単純になった ― ただし「1 秒あたりの音数」は増えた
- **Hamilton & Pearce(Queen Mary 大学)「Trajectories and revolutions in popular melody based on U.S. charts」Scientific Reports、2024** [一次の報道・大学発表で確認]
  - 1950〜2022 年の年間トップ 5 の主旋律。リズムと音の並びの複雑さは約 30% 下がり、**1 秒あたりの音数は増えた**。大きな転換は 1975、1996、2000 年(ヒップホップの主流化・ループ制作)。
  - 著者は「旋律の複雑さが下がった分、他の要素(音色など)に複雑さが移った可能性」を挙げる。
  - URL: https://www.nature.com/articles/s41598-024-64571-x / https://phys.org/news/2024-07-song-melodies-simpler.html / https://www.c4dm.eecs.qmul.ac.uk/news/2024-07-05.C4DM-study_pop_song_melodies/
  - [推測] 今のポップの旋律は「単純でも細かく動く」。今回のリードは単純さは合っているが、動き(休み・リズムの型)が足りない。

### 2.2 「メロディーは死んだ」か? ― Clark & Arthur(ジョージア工科大学)Empirical Musicology Review 17(2)、2023 年 11 月公開 [一次]
- 1960 年代と 2010 年代のヒット曲 1500 曲以上の旋律を自動採譜。指標は **2 小節ごとの音域(rolling range)**、繰り返しの量(gzip 圧縮率)、3 半音以下の音程の割合、リズム値の種類の数、半小節以上の音の割合。
- 結果: 2010 年代は繰り返しが**わずかに**多い。一方 **2 小節ごとの音域は 2010 年代の方が広い**(仮説と逆)。小さい音程の割合に差は無い。長い音の割合は年を追って減った。
- 背景: YouTube の Inside the Score「The Death of Melody」が「1 音の旋律・狭い音域」の広がりを批判していたことへの検証。
- URL: https://pdfs.semanticscholar.org/2ee5/5c2355a529368fea2998c6adecf2801dbaab.pdf (doi:10.18061/emr.v17i2.8746)
- [推測] Glaux の検査項目として「2 小節ごとの音域」はそのまま使える。今回の 3.3 半音/小節は、現代ポップが狭いどころか広い方向にある以上、明確に狭い。

### 2.3 ロックの旋律の音度 ― Temperley & de Clercq「Statistical Analysis of Harmony and Melody in Rock Music」Journal of New Music Research、2013 [一次]
- Rolling Stone 誌の 200 曲の旋律を採譜。旋律では **♭7 が 7 より多い**(和音の分布とは逆)。旋律で使う音の組み合わせは長音階が最多(24 曲)、次が長・短ペンタトニックの和集合(1-2-♭3-3-4-5-6-♭7、18 曲)。純粋なミクソリディア・ドリア・エオリアは各 2〜3 曲と少ない。
- URL: https://www.midside.com/publications/temperley_declercq_2013.pdf
- 注: **Hooktheory 自体の「旋律の音度の分布」集計は公開記事として見つけられなかった**。Hooktheory が公開しているのは曲ごとの指標(構成音の割合、拍/音、複雑さなど。https://www.hooktheory.com/song-metrics/about)と和音の統計(1300 曲の分析記事)。

### 2.4 サビの特徴 ― Van Balen ほか「An Analysis of Chorus Features in Popular Song」ISMIR 2013 [一次]
- サビ(とサビ的な部分)は、他の部分より大きく・鋭く・粗く、**音高が高く、音高がはっきりし**、音量の幅が小さく、音色の種類が多い。
- URL: https://webspace.science.uu.nl/~veltk101/publications/art/ismir2013-chorus.pdf

### 2.5 トレシーロ(3-3-2)の広がり ― Jajoria ほか SMC 2024 [一次]
- 1999〜2019 年のビルボード上位 20 曲。2000 年代初めはラテン系で多く、2010 年に底、その後ダンス系で増え、**2018 年頃に最多**(例: Rihanna「Where Have You Been」、Sia「Cheap Thrills」、Ed Sheeran「Shape of You」)。
- 定義: 付点 8 分+16 分+8 分休符+8 分を 2 回(= 3+3+2 の間隔)。
- URL: https://www.lsv.uni-saarland.de/wp-content/uploads/2025/07/SMC2024_Tresillo.pdf
- 補足 [二次]: Open Music Theory(Gotham ほか)は 3+3+2 のほか 3+2+3、2+3+3、**二重トレシーロ 3+3+3+3+2+2** を挙げる(U2「Electric Co.」のギター)。 https://human.libretexts.org/Bookshelves/Music/Music_Theory/Open_Music_Theory_2e_(Gotham_et_al.)/07:_Popular_Music/7.01:_Rhythm_and_Meter_in_Pop_Music

### 2.6 J-POP のリズムの変遷 ― 畦地希美「J-pop: リズムと歌詞の入れ込みルールの変遷」音楽教育実践ジャーナル 5(1)、2007 [一次]
- 60〜90 年代のオリコン上位各 50 曲(計 200 曲)。3 文字で始まるフレーズでは、わらべうたの型(パターン A)が 60 年代に 61% だったのが、90 年代には**わらべうたに無い新しい型(詰め込み型、パターン C)が 60%**に。
- 基本の音価は 60 年代の 4 分音符から 70 年代に 8 分音符へ。16 分音符は 60 年代ゼロ → 70 年代 2 割弱 → 以後 1 割程度。
- URL: https://www.jstage.jst.go.jp/article/jjomep/5/1/5_25/_pdf/-char/ja
- [推測] 日本語の歌の旋律を作るときは「1 音 1 モーラ」を基本に、8 分〜16 分の密度になる。インストのリードをそのまま日本語の歌に使うとリズムが合わない。

### 2.7 聴き手の予測 ― David Huron『Sweet Anticipation』MIT Press、2006 [二次(書評経由)]
- 旋律の規則性: 音は近くへ動く(近接)、下降の順次が多い、弧を描く輪郭、極端な音の後は中央へ戻る。聴き手が実際に期待するのは「大きな跳躍の後は向きが変わる」(post-skip reversal)。
- URL: https://www.doc.gold.ac.uk/~mas03dm/papers/huron06-review.pdf

### 2.8 フックの類型 ― Gary Burns「A typology of 'hooks' in popular records」Popular Music、1987 [一次]
- 反復・変奏・「転調」(ここでは要素の大きな変化の意味)の連続体としてフックを捉える。「完全な反復」は発振器の持続音で、変化の無い音楽は単調に、変化し続ける音楽も単調に聞こえる。
- リズムのフックは反復からではなく**変化から**生まれる: リズムが始まる瞬間、リフの無い区間の後で**リフが戻る瞬間**(Stones「Jumpin' Jack Flash」)。
- URL: https://www.tagg.org/xpdfs/burns87.pdf
- [推測] ドロップ前に一度リードを止めて(休みを作って)戻すこと自体がフックになる。今回のように 16 小節切れ目なく鳴らすと、この効果が使えない。

---

## 3. ソングライティングの教材・作家の発言

### 3.1 Pat Pattison(バークリー音楽大学) [一次(本人サイト)]
- **前が重い/後ろが重いフレーズ**: 小節の頭(か直前の弱起)から始まるフレーズは安定し「旗竿についた旗」のよう。頭の後から始まるフレーズは不安定で「旗竿から外れてはためく」。遅く始まるほど不安定。小節にも強弱(1 小節目が強、2 小節目が弱)があり、弱い小節で始めるには前の強い小節を空けておく必要がある。「Motion creates e-motion」。
  - https://www.patpattison.com/art-of-phrasing
- 型を作って繰り返し、意味のある所で型から外れる(自然な強弱に沿ったシンコペーション)。
  - https://www.patpattison.com/rhythms-and-variations
- 歌詞のフレーズと旋律のフレーズは一緒に終わる。 https://www.patpattison.com/lyric-and-melodic-phrases
- [推測] インストのリードでも「毎小節同じ位置から始まる」と安定しすぎて前へ進まない。フレーズの始まりを 1 拍目・2 拍目裏・前の小節の 4 拍目裏などに散らすのが直し方。

### 3.2 Max Martin の「メロディック・マス」 [二次]
- 旋律が先、歌詞は旋律に従う。音節の数を厳密に合わせ、ある行の次の行は「鏡像」になる(John Seabrook『The Song Machine』などで紹介)。ヴァースにもサビの要素を出しておく。
- 反復はほぼ正確だが、Martin が「許す」変化は 2 種類: 歌詞に合わせるための小さな変化、構成上の役割(A・B・A・C など)を果たすための変化(Top40Theory の「Shake It Off」分析)。
- https://www.top40theory.com/blog/max-martins-melodic-math-repetition-tweaks-in-taylor-swifts-shake-it-off / https://bobbyowsinskiblog.com/max-martin-formula/

### 3.3 Ryan Tedder(OneRepublic、プロデューサー)NPR「The Record」2010 年 10 月 [一次(インタビューの引用)]
- 「メロディーはどんな曲でも唯一最も大事なもの。何よりも勝る」
- 「リフは添え物でもあるが、添え物から主役になったとき、それがフックになる」
- 「(メインストリームで成功しない人は)意識的にか無意識にか、大きなサビを出すことを拒んでいる」
- https://mysongcoach.com/interview-with-ryan-tedder/

### 3.4 Gary Ewer(The Essential Secrets of Songwriting)2016 [二次]
- トップラインに当てはまる原則: 旋律は**低く始めて高くなる**(Beyoncé「Halo」はトップラインとして書かれ、低く始まりサビで頂点)。ヴァースは複雑なリズムでよく、**サビは単純で覚えやすいリズム**。題名の部分は音を長く伸ばす。繰り返さない音の並びは「覚えにくく歌いにくい」。
- https://www.secretsofsongwriting.com/2016/08/26/toplining-which-principles-of-songwriting-apply/

### 3.5 SEIDS(プロのトップライナー)2024 [一次(インタビュー記事)]
- 反復、コールアンドレスポンス、問いと答え、**間(休み)を入れて詰め込みすぎない**。サビは「でも」の転換点に。
- https://boombox.io/blog/how-to-write-a-topline-from-a-pro-topliner-seids/

### 3.6 トップライン一般 [二次]
- まず歌詞なしでハミング(モゴモゴ歌い)し、ビートの「ポケット」を探す(Kits.ai、Audient ほか)。ヴァースからサビへは音域・エネルギーの上昇で対比を作る。
- https://www.kits.ai/blog/toplining-creating-hit-melodies / https://audient.com/tutorial/7-tips-for-writing-toplines/

### 3.7 EDMProd「Advanced Melody Guide」(Connor O'Brien) [二次]
- 強拍に和音の構成音。動機の反復は 3 種類: **リズムだけ同じ**、**輪郭だけ同じ**、完全な反復(控えめに)。
- 4 要素: 動き(ポップは**順次 6〜8 割**)、**間**(休みが緊張と覚えやすさを作る)、リズム(覚えやすさのため**型は 2〜3 種類**に)、反復。
- 診断: 動きすぎ/止まりすぎ、詰めすぎ/空きすぎ、リズムが複雑すぎ/単調、反復が覚えやすさに役立っているか。
- https://www.edmprod.com/advanced-melodies-chord-tones-motifs/

### 3.8 Hyperbits(Serik) [二次]
- **1 小節の案を作り、2 小節目に複製して「1 つだけ変える」**。これを 4〜8 小節まで。
- 安定音(1・3・5 度)と不安定音。**音階を順に進む「ラン」と、順でない「ジャンプ」を交互に**。ランばかりの所にジャンプを足す。
- よくある失敗: 「しっくりくるまで弾き散らかして、そこで終わりにする」。
- https://hyperbits.com/blog/write-better-melodies/

### 3.9 Shed the Music(Bob Habersat)「Melodic Development + EDM」 [二次]
- 動機(普通 1 小節)→ 完全な反復 / 似た反復(和音に合わせて音を変える)/ ゼクエンツ(形とリズムを保ったまま上下) / 変化: **引く・音価を変える・ずらす・音を変える・足す**。予測できて単調でない **ABCB**。例: Marshmello「Alone」。
- https://www.shedthemusic.net/free-resources/melodic-development

### 3.10 SRDC(提示・反復・逸脱・結論) [二次(理論家の用語)]
- Walter Everett が名付けたポップ・ロックの 4 句の型。1 句目に基本の案、2 句目で反復(か応答)、3 句目で対比(断片化・和声リズムの加速・主和音から離れる)、4 句目で結論(基本の案に戻るか、強く終わる新しい素材)。
- https://viva.pressbooks.pub/openmusictheory/chapter/aaba-and-strophic-form/ / https://mtosmt.org/issues/mto.13.19.3/mto.13.19.3.callahan.php

### 3.11 「順次進行は 3 音まで」など初心者向けの規則 [二次]
- 耳で作る人は安全策で順次ばかりになる。「順次は 3 音まで」(Hack Music Theory の目安)。順次は半分〜3 分の 2、跳躍は 3 分の 1〜半分(emastered)。
- https://hackmusictheory.com/blogs/theory/posts/6616269/stepwise-melody-rule / https://emastered.com/blog/melodic-motion
- [推測] 「順次 3 音まで」は強すぎる規則だが、今回の「跳躍ゼロ」を検出する閾値としては使える。

### 3.12 日本の初心者向け記事 [二次]
- **SRM「作曲初心者のメロディ失敗あるある 7 つ」(2022)**: ①**休符がない**(詰め込みすぎて歌いにくく覚えにくい → 後半に休符を入れて聴き手に理解する時間を)②種類が少ない(リズム型・順次型・跳躍型の 3 種を場面ごとに使い分ける。例: A メロはリズム型、B メロは跳躍型、サビは順次型)③リズムがジャンルに合わない(R&B は 16 分、ポップ・ロックは 8 分、童謡は 4 分)④モチーフがない(とりとめがない)⑤**いつもドアタマから始める**(2 拍目裏や前の小節の 4 拍目裏から)⑥驚きがない(ブルーノート、ポリリズム、6 度以上の跳躍)⑦テーマを表現していない。
  - https://srm-music.com/melody/
- **SoundQuest「モチーフの提示・展開・解消」**: 提示 → 展開(反復・変形)→ 解消。例: ビゼー「カルメン」前奏曲は語頭(タンタカ)を固定し語尾の音を毎回変える。嵐「Monster」はモチーフの 1 回目は語尾だけ、2 回目は語頭と語尾、3 回目はリズム、4 回目は歌詞を詰め込み、と「崩し方」を段階的に強めて飽きを防ぐ。RADWIMPS「おしゃかしゃま」サビはほぼ完全な反復を入れ子にして計 12 回。
  - https://soundquest.jp/quest/melody/melody-mv1/motif-and-variations/

---

## 4. ジャンルごとの語法と典型的な失敗

### 4.1 ハウス(クラシック/ディープ/ピアノハウス)
- **リフは 2 音でもよい。** Mr Fingers「Can You Feel It」の riff は A と E の 2 音だけで、下の和音が Am7・Fmaj7・Em7 と変わっても繰り返す。3 度を抜くと長調・短調どちらの和音にも乗る(Attack Magazine、Oliver Curry、2012)[二次]。 https://www.attackmagazine.com/technique/passing-notes/ostinatos-and-acid-house-riffs/
- **ピアノハウスのリフは「2〜4 和音を中音域で打楽器的に刻む」**。起源は Harold Melvin & The Blue Notes「Bad Luck」(1973)の刻み(Beatportal)[二次]。和音を 16 分ずつ前後にずらしてシンコペーションを作る。 https://www.beatportal.com/articles/366921-the-eternal-appeal-of-piano-house
- Robin S「Show Me Love」(StoneBridge リミックス)は Korg M1 のオルガン音色の鋭いスタブ、Crystal Waters「Gypsy Woman」は「la da dee」の声のフックとゴスペル風オルガン(Wikipedia)[二次]。
- ディープハウス: 「ノリの良い反復と、機械的でノリの無い状態の境目は紙一重。タイミングや強弱を少し変えるだけでも変化を入れる」(MusicRadar「22 deep house production tips」2008)[二次]。 https://www.musicradar.com/tuition/tech/22-deep-house-production-tips-153784
- 間の使い方: 「動機を毎小節繰り返す代わりに、2 小節や 4 小節に 1 回にする。間が動機を目立たせる」(EDMProd の House ガイド)[二次]。 https://www.edmprod.com/how-to-make-house-music/
- 典型的な失敗 [推測]: 今回のように**4 つ打ちの上でレガートの線を切れ目なく**鳴らすこと。ハウスのリードは「スタブ(短い和音・短い音)+間」または「声のフック+応答」が基本で、長い線は 1 小節に 1 つの伸ばし程度。

### 4.2 テックハウス
- リードは「単純でリズミカルなスタブか、繰り返す動機」。波形は単純、乾いてパンチがあり、空間系は控えめ。声のサンプルは「単純に繰り返し、リズムに合うように」切って並べる(Beatportal、2024)[二次]。 https://www.beatportal.com/articles/692607-step-by-step-guide-to-producing-tech-house-like-fisher-chris-lake-cloonee-and-michael-bibi
- [推測] テックハウスでは「旋律」よりベースと声のチョップがフック。ピッチの動きは狭くてよいが、**休みとアクセントの位置**が命。

### 4.3 プログレッシブハウス / ビッグルーム / ポップ EDM
- **Avicii**(MusicRadar、Sara Simms、2024)[二次]: 旋律を先に作り、それに合わせて曲を作った。**AAAA**(1 つの旋律を 4 回)と **AAAB**(3 回繰り返して 4 回目を変える)。長い音を 2 つの短い音に割る「スタッター」でリズムに変化をつける(例: 1 小節目 3 拍目の音を 2 つに)。 https://www.musicradar.com/how-to/avicii-melodies
- **Avicii「Levels」**(Hooktheory)[二次(データ)]: C#m、127〜128BPM、**100% ダイアトニック、構成音 61%、0.63 拍/音**、主に 8 分音符。リード(Instrumental)は D#4–B4(8 半音)、複雑さ 16。E を中心にして C# へ跳び上がり、ペンタトニックで E へ下る。 https://www.hooktheory.com/theorytab/view/avicii/levels
- **Swedish House Mafia「Don't You Worry Child」**: サビは F#4–B5(17 半音)、0.84 拍/音。サビは 1〜3 小節目と 5〜7 小節目が同じで、跳躍と順次を組み合わせる(Hooktheory、EDM Tips の Will Darling)[二次]。 https://www.hooktheory.com/theorytab/view/swedish-house-mafia/dont-you-worry-child / https://edmtips.com/melody-patterns-every-producer-should-know/
- **Martin Garrix「Animals」**(Computer Music 2016、Hooktheory)[二次]: Fm、128BPM。ブレイクダウンの旋律を続けると思わせて、**ドロップでは全く新しいリフ**に切り替える。リフはプラック的でスタッカート、**構成音 88%、0.41 拍/音(密)**、サビ G#4–C#6(17 半音)。 https://www.musicradar.com/tuition/tech/anatomy-of-a-hit-martin-garrix-animals-636619 / https://www.hooktheory.com/theorytab/view/martin-garrix/animals
- **deadmau5「Strobe」**: 1.45 拍/音(長い音)、F#4–F#5、繰り返しが多い(Hooktheory)[二次]。長い音でも成立するのはテンポ感の違う曲だから。
- **Eric Prydz「Opus」**: 和音をアルペジオにした旋律を、テンポと層を少しずつ増やして 9 分かけて育てる(Wikipedia、EDM Tips)[二次]。
- **ベースが旋律**: Benny Benassi「Satisfaction」、Oliver Heldens「Gecko」、SHM「One」(EDM Tips)[二次]。
- ドロップ一般: 「少ない音の方がクラブでは強い」「リードは 1 つにして、聴き手が追えるようにする」「ドロップ直前の一瞬の沈黙が効く」(Point Blank、iZotope ほか)[二次]。 https://www.pointblankmusicschool.com/blog/how-to-make-your-drops-hit-harder-tips-for-edm-producers/
- 典型的な失敗 [推測]: ドロップ=ブレイクダウンの旋律を移調しただけ(今回)、リードを何枚も重ねて焦点がぼける、全部の拍を埋める。

### 4.4 トランス
- **Steve Allen(Trance Producer、2026)**[二次(制作者の教材)]:
  - 「偉大なトランスの旋律は驚くほど少ない音、**多くは 4〜6 音**でできている」
  - 8 小節の動機から始める。主に順次、**時々 4 度・5 度・オクターブの跳躍**。
  - 「**隙間を空ける。トランスの旋律で沈黙は音と同じくらい大事。息をするフレーズは詰め込んだフレーズより感情的に響く**」
  - 「最高音が感情の頂点。**8 小節に 1〜2 回**使い、ずっと使わない」
  - 付点・シンコペーション・裏拍のフレージングでグルーヴに合わせる。
  - https://tranceproducer.co.uk/blogs/news/how-to-write-trance-melodies-and-arps-from-scratch
- **myloops(2026)**[二次]: 138BPM で 16 分のアルペジオ(根音・3 度・5 度・オクターブ・5 度・3 度の 6 音型)。**ゲート長でサブジャンルが決まる: 約 45% はクラシックなプラック、85% 以上はローリングするアップリフティング**。「**均一な 16 分の列を、1 小節に 1 つの伸ばす音で崩す。それだけで『作曲された』と『計算された』が分かれる**」。ブレイクダウンでは旋律を早めに**不完全に**見せ、最後の 3 分の 1 で完全に。プラックの線なら強拍(1 と 3)だけ残す。レガートの線ならオクターブ上げて山の音だけをオルゴール風に。
  - https://www.myloops.net/programming-trance-arpeggios-and-rhythmic-sequences / https://www.myloops.net/building-a-powerful-trance-breakdown
- 典型的な失敗: 均一な 16 分の垂れ流し、旋律と和音を同時に作ろうとする(先に和声の土台を)。

### 4.5 メロディックテクノ / メロディックハウス(Afterlife 系)
- **「リードは忙しくない。4 小節で 4〜8 音、長く伸ばす音と聞こえる隙間」**。2 小節を作って**そのまま繰り返し、終わりだけ変える**。フレーズの最初と最後の音は和音の構成音。リードは G4–C6、和音は C2–G3 で住み分け。グライド 60〜100ms(150ms 以上はベースっぽくなる)。タイミング ±5〜10ms、ベロシティ ±10〜15 で人間らしく(myloops、2026)[二次]。 https://www.myloops.net/how-to-create-melodic-techno-chords-and-melodies
- 16〜32 小節かけてフィルターを開けるのが展開そのもの(同)。
- [推測] 旋律は少なく、変化は音色(フィルター・ディレイ)で出すジャンル。音を増やして変化を出すと逆に語法から外れる。

### 4.6 フューチャーベース / トロピカルハウス
- 声のチョップを楽器のように使う、**ピッチベンドとオクターブの跳躍**が特徴(RouteNote ほか)[二次]。ドロップのリードは「単純で繰り返す」。3 つ目のリード層を途中から**1 オクターブ上か下**で足して盛り上げる(EDMProd、Simon Haven、2025)[二次]。 https://www.edmprod.com/how-to-make-future-bass/
- Kygo「It Ain't Me」のドロップは Selena Gomez の声の音節をチョップしてサイドチェインした旋律(Wikipedia)[二次]。
- [推測] 音節を切った声のチョップは本質的に「短い音+休み」。これを合成リードで真似るなら、ゲートを短くし、音と音の間に 16 分の隙間を入れる。

### 4.7 ポップ / トップライン
- 低く始めてサビで頂点、ヴァースは複雑でもサビは単純なリズム、題名を伸ばす(Gary Ewer)。サビは音高が高い(Van Balen 2013)。
- ヴァースで話すように、サビで歌う(Chilly Gonzales が Drake・Nicki Minaj について)[一次(作曲家の分析)]。
- 1 音のフック: Lady Gaga「Poker Face」のような 1 音の繰り返しと、「Bad Romance」の単調なヴァースと旋律的なサビの対比(Inside the Score の批判に対する論考)[二次]。Rick Beato にも「How this 1-note melody took over pop music」という動画がある(**内容は未確認**)。 https://www.youtube.com/watch?v=I4kouRO_TfE
- [推測] 「同じ音の連打」は悪ではない。**リズムがはっきりしていて、その後に動く部分と対比されていれば**フックになる。今回の C–C–D–C–C–D は連打と隣の音の往復だけで、対比の相手が無い。

### 4.8 J-POP / アニソン / ボカロ
- **Sleepfreaks「最近の人気楽曲のメロディ解析」(宮川智希、2022)**[二次]: 対象 6 曲(マーシャル・マキシマイザー、フォニイ、グッバイ宣言、怪物、KING など)。A メロはシンコペーションが少ない。B メロは A メロに無いリズムを入れる。**サビは 6 曲すべて固有のリズム**を持つ(KING は 4 分音符 4 連がサビだけ、グッバイ宣言は 16 分を多用)。A メロで 1 オクターブ以上の広い音域、長 3 度以上の跳躍が多い。 https://sleepfreaks-dtm.com/chord-analize/hit-j-pop-2021-2022/
- 歌ものは一般にサビの音域を最も高くする(4th-signal ほか)[二次]。
- ヨナ抜き(ファとシを抜いた長音階)の使い分け: A メロだけヨナ抜きにしてサビで「シ」を多用し場面転換を感じさせる、「打上花火」ではサビで 4 度・7 度を抑え最後のフレーズで拍の頭に置く(記事の解説)[二次]。 https://note.com/masatsumu/n/n9884be97d9b9 / https://www.nikkei.com/article/DGXZQOUD295DC0Z20C23A6000000/
- ボカロ: 言葉の量と速さ、サビでの転調の多さ(末次智の指摘として)。反復回数が型にはまらない(イントロ 6 回、最後のサビ 5 回など、川本聡胤の著作より)[二次]。 https://musicmusicologic.com/common-character-of-vocalo-music/
- 典型的な失敗: A・B・サビで同じリズム型を使う(=今回の「全小節同じ型」)。

### 4.9 ゲーム音楽
- **近藤浩治(任天堂)**[一次(講演・インタビューの報道)]:
  - GDC 2007 の講演で「リズム」「バランス」「インタラクティブ」を挙げた。生演奏のリズムは演奏者のリズム感になってゲームに合わないことが多い、とも(GAME Watch)。 https://game.watch.impress.co.jp/docs/20070308/kondo.htm
  - 『スーパーマリオブラザーズ』の最初の案はゆったりした曲で没になり、マリオの**走りと跳びのリズム**に合わせて作り直した。アクションを引き立てない・走り跳びにタイミングが合わない・効果音と合わない曲は捨てた(Wikipedia「Super Mario Bros. theme」、LOC の資料)。 https://en.wikipedia.org/wiki/Super_Mario_Bros._theme
  - ドラムは 8 分をスイングさせ、旋律はスイングさせない。3 連と 2 分割が同時に鳴る(20K など)[二次]。
  - 「次の音との休符をどれくらい空けたら楽しくなるのか、それを考えるのは完全にパズル」という趣旨の発言が検索の要約に出たが、**原典は確認できなかった**。
- **植松伸夫**[二次(インタビューの報道)]: 今のゲーム音楽は「心地よいデジタルシンセとシーケンサーで型を破らない。変なものが減った」(Real Sound、NewsPicks のインタビュー報道)。 https://www.gamesradar.com/games/final-fantasy/final-fantasy-legend-nobuo-uematsu-says-video-game-music-is-getting-more-boring-and-baldurs-gate-3-publishing-lead-agrees-nothing-is-made-for-anyone-in-particular-anymore-except-shareholders/
- [推測] ゲーム音楽のループは何百回も聴かれるので、「休みの位置」と「最後の変化」が飽きにくさを決める。Glaux の Godot 連携(ループ再生)でも同じ。

### 4.10 ジャズ
- **Sonny Rollins「Blue 7」**: 3 音の動機だけでソロ全体を組み立てる(Gunther Schuller の 1958 年の論考で有名)[二次]。動機は 2〜4 音、移高・反転・拡大・縮小で変える。
- **Hal Galper『Forward Motion』**[二次(要約)]: フレーズは**裏拍から始め、小節線をまたいで 1 拍目(か 3 拍目)を目的地**にする。「1 拍目はフレーズの始まりではなく行き先」。強拍に構成音(1・3・5・7 度)。Dizzy Gillespie「裏拍が多いほどスイングする」。 https://halgalper.com/articles/understandingforwardmotion/
- 初心者の失敗: 音が多すぎる、音階を並べるだけ(「音階は旋律の作り方を教えてくれない」Jazzadvice)。 https://www.jazzadvice.com/lessons/15-mistakes-beginner-jazz-improvisers-make/

### 4.11 ヒップホップ(トップライン・サンプルのチョップ)
- **Chilly Gonzales「Rappers and Melody」(Drowned in Sound、2014)**[一次(作曲家の分析)]: ラッパーは**半音以内(E と F)**で音高を選ぶ(Jadakiss、Nas、Lloyd Banks)。Scarface・DMX は説教師の抑揚。Clipse はわらべうたのような歌い回し。Eminem は数小節でオクターブを滑る。Future・Rich Homie Quan は**同じ数音を臆面もなく繰り返して歌う**が、リズムの密度で MC の信用を保つ。 https://drownedinsound.com/in_depth/4148030-rappers-and-melody--an-analysis-by-chilly-gonzales
- J Dilla は素材をループせず、MPC で切って並べ直した(クオンタイズを切る、伸縮・移調)[二次]。 https://scholarworks.sfasu.edu/cgi/viewcontent.cgi?article=1211&context=etds
- トラップのビート: 2〜4 小節の短いループ、短調、**声のための空間(1〜5kHz)を空ける**、追いにくい旋律は選ばない(Waves、Soundfly ほか)[二次]。
- [推測] ヒップホップでは「狭い音域+同じ音の繰り返し」は正当な語法。ただし成立条件は**リズムの密度と間の変化**。今回のリードは音域の狭さだけ真似てリズムの変化が無い。

### 4.12 ローファイ
- 4〜8 音の短い動機を繰り返し、1〜2 音かリズムを少し変える。大きな跳躍や本題から離れる動きは避ける。スイング 50〜60%。90BPM を超えると忙しく感じる(Soundtrap、Melodics ほか)[二次]。 https://blog.soundtrap.com/create-lo-fi-beats/

---

## 5. 「AI の作ったメロディーはここが変」

- **KTH 学位論文(Marcus Warnerfjord、2023)「Evaluating ChatGPT's Ability to Compose Music Using the MIDI File Format」**[一次]
  - ChatGPT 3.5-turbo の MIDI を Lakh MIDI(人の曲)と比較。**使う音の種類が少ない、音域が狭い、音程が小さい**。音の長さはばらばらで、人の曲の方が**同じ音価を一貫して使う**(=リズムの型がある)。
  - 利用者の満足度は 5 段階で平均 1.77、**音楽経験が多い人ほど不満**。
  - https://kth.diva-portal.org/smash/get/diva2:1779208/FULLTEXT01.pdf
- **AudioCipher(Ezra Sandzer-Bell、2025)**[二次(実験記事)]: ChatGPT の作る MIDI は「**ほとんど 4 分と 2 分音符で、耐えがたく遅い**」(構造を聴くのにテンポを 200BPM に倍にした)。和音に合わない音を「経過音」と誤って呼ぶ。指示と違う大きな跳躍。MIDI の数値を直接書かせるコード生成の方が良かった。 https://www.audiocipher.com/post/chatgpt-music
- **旋律生成の研究(MELONS、MeloForm など、ISMIR 2022 ほか)**[一次]: 長さが伸びると長期の構造(主題のまとまり)が失われる。反復は「同じ和声とリズム」で暗に表すだけで、音楽的な形式には程遠い。 https://arxiv.org/pdf/2110.05020 / https://archives.ismir.net/ismir2022/paper/000068.pdf
- 1 次のマルコフ連鎖は「当てもなくさまよう」旋律になり、高次にするとフレーズらしさが出る(解説)[二次]。
- **Adam Neely「Suno, AI Music, and the Bad Future」**(2026 年初め)、Rick Beato「I'm Sick of This AI Crap」[一次(動画)]: 批判の中心は独自性の欠如と技能の喪失で、旋律の具体的な欠点の分析ではない。 https://musictech.com/news/music/adam-neely-generative-ai-replacing-music-craft/
- Suno の曲は「数回聴くと単調、構成は堅実だが予測どおり」、Udio の方が変化があって自然(比較記事)[二次]。 https://blog.samplefocus.com/blog/suno-ai-vs-udio/
- [推測] 記号(MIDI)で旋律を書く LLM の典型的な癖は次の 5 つ:
  1. 和音に安全な音を選び、隣の音へ動く(狭い音域・跳躍ゼロ)
  2. 休符を書かない(音を次の音の頭までつなぐ)、または全部 4 分・2 分
  3. リズムの型を決めずに音価を選ぶ(型が無いか、1 つの型を全小節で使う)
  4. 形が「4 小節を丸ごと反復」か「毎回違う」の両極端で、「繰り返して最後だけ変える」が出ない
  5. 表情(ビブラート・ベンド・ベロシティ)を一律に掛ける
  今回の事例はほぼ全部に当てはまる。

---

## 6. 道具に入れられそうな型

### 6.1 リズムの型(16 分の格子、1 小節 = 16 マス)

出典のある型:

| 名前 | 格子 | 由来・使い道 |
|---|---|---|
| トレシーロ 3-3-2(短い音) | `x..x..x.x..x..x.` | SMC 2024、Open Music Theory。ダンス・ポップのリード、スタブ、ドロップ |
| トレシーロ(伸ばし付き) | `x-.x-.x.x-.x-.x.` | 同上。プラックより少し長いゲート |
| 二重トレシーロ 3+3+3+3+2+2 | `x..x..x..x..x.x.` | Open Music Theory(U2「Electric Co.」)。フューチャーベースの和音スタブ、トロピカルのチョップ |
| 3+3+3+3+4 | `x..x..x..x..x...` | 二重トレシーロの変形。最後に 1 拍の間 |
| 裏拍スタブ(ハウスのオルガン・ピアノ) | `..x...x...x...x.` | 裏拍にだけ短い音。キックと交互(ハードハウスの「ドンク」ベースも同じ位置) |
| 8 分の刻み+スタッター(Avicii 型) | `x-x-x-x-xxx-x-x-` | MusicRadar: 8 分中心で、3 拍目の音を 2 つに割る |
| トランスのゲート付き 16 分+伸ばし 1 つ | `xxxxxxxxxxxx----` | myloops: 均一な 16 分を 1 小節に 1 つの伸ばしで崩す(ゲート約 45%) |
| マリオ地上の冒頭(8 分単位、2 小節) | `x.x...x...x.x...|x.......x.......` | 一般に知られる譜: E E 休 E 休 C E 休 G 休 休 休 G(低) 休 休 休(**要照合**) |

[推測] ジャンル別に提案する型(教材の記述から組み立てたもの。実曲の採譜ではない):

| ジャンル・用途 | 格子 | 意図 |
|---|---|---|
| ハウスのリード(問い、2 拍+休み) | `x..x..x.x-......` | トレシーロの頭 3 つ+伸ばしで止め、後半 1.5 拍を空ける |
| ハウスのリード(答え、後ろ寄り) | `....x.x.x.x.x---` | 2 拍目から始めて、最後の音を伸ばす(ビブラートを掛ける場所) |
| テックハウスの 1 音スタブ | `..x..x....x..x..` | 狭い音域でもよいが、アクセントの位置で聴かせる |
| メロディックテクノ(4 小節で 5 音) | `x-------x-------|x-----------....|x-------x---....|x-----------....` | 長い音と隙間。2 小節ごとに終わりだけ変える |
| トランスのアンセム(付点・シンコペーション) | `x--x--x-x--x-.x-` | 付点 8 分の連なり+最後に短い休み |
| フューチャーベースの和音スタブ | `x..x..x...x.x...` | 3-3-4-2-4 のずらし。ドロップで声のチョップと交互 |
| ポップのトップライン(弱起で始まる句) | `....x.x.x-x-x---` | 小節の頭を空けて後ろ寄りに始め、最後を伸ばす(Pattison の「後ろが重い」) |
| J-POP のサビ(16 分の詰め込み+句末の伸ばし) | `x.xxx.x.x-xxx---` | 16 分の密度と、句の終わりの伸ばし |
| トラップの上物ループ(2 小節) | `x.....x...x.....|x.x.......x.....` | 声のための空間を大きく空ける |
| ローファイの動機(スイング前提) | `x-.x..x-x-......` | 4〜5 音で後半を空ける |

### 6.2 形の型(小節単位)

| 名前 | 並び | 出典 |
|---|---|---|
| AAAA | 1 つの案を 4 回 | Avicii(MusicRadar) |
| **AAAB** | 3 回繰り返して 4 回目を変える | Avicii(MusicRadar) |
| 複製して 1 つ変える | A A' A A''(各回で 1 要素だけ変更) | Hyperbits |
| 2 小節繰り返し+終わりだけ変える | [ab][ab'] | メロディックテクノ(myloops) |
| ABCB | 予測できて単調でない | Shed the Music |
| **SRDC** | 提示・反復・逸脱(断片化、和声リズムの加速)・結論 | Everett / Open Music Theory |
| 問いと答え | 1〜2 小節の問い(開いて終わる)+答え(構成音で閉じる) | SEIDS、SoundQuest |
| 段階的に崩す | 1 回目は語尾だけ、2 回目は頭と尾、3 回目はリズム… | SoundQuest(嵐「Monster」) |
| ドロップで新しいリフ | ブレイクダウン = 旋律 A、ドロップ = リフ B | Garrix「Animals」 |
| 不完全 → 完全 | ブレイクダウンで強拍だけ/山の音だけ → ドロップで全体 | myloops(トランス) |
| リフの休止と復帰 | 数小節リフを止めて戻す | Burns 1987(「Jumpin' Jack Flash」) |

### 6.3 動機の変え方(道具の操作として)
反復 / 似た反復(和音に合わせて音だけ変える)/ ゼクエンツ(形を保って上下)/ 引く / 音価を変える / ずらす(拍位置)/ 音を変える / 足す(スタッター)/ 断片化 / 反転 / オクターブ重ね ― Shed the Music、SoundQuest、ジャズの動機展開。

### 6.4 検査項目の目安 [推測(出典の数値を元にした目安)]
- 休みの割合(鳴っていない時間): リードで 20〜50%。0% に近ければ警告(トランスのプラックはゲート 45%)
- 2 小節ごとの音域: 5 半音以上。セクション全体で 8〜17 半音(Hooktheory の数値)
- 跳躍(4 半音以上)の割合: 2〜4 割(順次 6〜8 割、EDMProd)。「順次 4 音以上連続」が続けば警告
- 最高音: 8 小節に 1〜2 回(トランス教材)
- リズムの型: 2〜3 種類(EDMProd)。全小節同じなら警告、全小節違っても警告
- フレーズの始まり: 全部が 1 拍目なら警告(SRM、Pattison、Galper)
- 強拍とフレーズの最初・最後の音: 和音の構成音(EDMProd、myloops)
- 4 小節(か 8 小節)の最後の小節が前と同じなら「最後の変化なし」の警告(AAAB)
- ビブラート: 1 拍以上の音だけ、1/8 程度遅らせて掛ける

---

## 7. 今回のリード(ハウス 124BPM、Am–F–C–G)の直し方

[推測] 以下はすべて調査結果からの提案。音名は C5 = 中央ド の 1 オクターブ上(MIDI 72)。

1. **ゲートを切る。** 同じ音の数のままでも、各音の長さを 16 分〜8 分にし、句の終わりにだけ伸ばす音を置く。これだけで休みの割合は 0.4% から 30〜50% になる。
2. **1〜2 小節の動機を作る。** リズムは 3-3-2 の頭を使い、2 拍+休みの「問い」にする。
3. **答えは後ろ寄りに始めて、跳躍を 1 つ入れ、構成音で伸ばして閉じる。**
4. **4 小節目は変える(AAAB または SRDC)。** 丸ごとの反復をやめる。
5. **音域を 1 オクターブ近くに広げ、最高音は 8 小節に 1〜2 回。**
6. **ドロップは「3 度上」ではなく**、(a) 同じ動機を密にする(スタッター・オクターブ重ね)、(b) 新しいリフに切り替える(Animals 型)、(c) ブレイクダウンで山の音だけ見せ、ドロップで全体を出す、のどれか。
7. **ドロップの直前にリードを止める**(1 拍〜1 小節の休み)。
8. **表情を分ける。** ビブラートは 1 拍以上の音だけに遅らせて掛ける。短い音はベロシティの強弱だけ。グライドは山への跳躍と句の終わりだけ。

### 書き直しの例 A(ブレイクダウン・最初の提示。休み多め)

```
小節1 Am (問い)   x..x..x.x-......   E5  E5  D5  C5(8分)          → 後半1.5拍休み
小節2 F  (答え)   ....x.x.x.x.x---   A4  C5  F5  E5  C5(伸ばし)   → C5 にビブラート
小節3 C  (問い')  x..x..x.x-......   G5  E5  D5  C5               → 頭を山の G5 に(似た反復)
小節4 G  (結論)   ..x.x.x.x-----..   G4  B4  D5  B4(伸ばし)       → 上行アルペジオで止める
```
- 鳴っている割合は約 42%、音域 G4–G5(12 半音)、跳躍: C5→F5(4 度)、E5 ← G5 の下降 3 度、G4→B4→D5 のアルペジオ。
- フレーズの始まり: 1 拍目・2 拍目・1 拍目・1 拍目の裏(2 マス目)と散らしている。

### 書き直しの例 B(ドロップ。密度を上げて AAAB)

```
小節1 Am  x-.x-.x-.x-.x-x-   E5 E5 A5 G5 E5 D5   (二重トレシーロ、A5 へ 4 度の跳躍)
小節2 F   x-----..x.x.x-..   C5(伸ばし) A4 C5 D5
小節3 C   x-.x-.x-.x-.x-x-   E5 E5 G5 G5 E5 D5   (小節1の似た反復)
小節4 G   x-----..x.x.x-..   B4(伸ばし) G4 B4 D5
小節5〜7  小節1〜3 と同じ(オクターブ下を重ねる)
小節8 G   x.x.xxx-x-------   D5 E5 G5 A5 B5 A5(伸ばし)  → 山の B5 は 8 小節で 1 回だけ、最後は伸ばして次へ
```
- 小節 1〜7 は「1 小節 + 1 小節」の 2 小節の型の繰り返し、小節 8 だけ違う(AAAB)。
- 2 周目の 8 小節ではスタッター(3 拍目の音を 16 分 2 つに)を足す、フィルターを開けるなど「1 つだけ変える」。

### 生成の道具(Glaux の EDM リズム型)への反映
- 今の「音を伸ばしてつなぐ型ばかり」に、§6.1 の休みを含む型を加える。少なくとも トレシーロ(短)、裏拍スタブ、問い(2 拍+休み)、答え(後ろ寄り)、Avicii 型スタッター、メロディックテクノの長音+隙間。
- 型に「ゲート長(音の長さの割合)」を持たせ、同じ格子でもプラック(45%)とレガート(100%)を選べるようにする。
- 形(AAAB、SRDC、2 小節反復+終わり変更)を別の引数にして、「最後の小節を必ず変える」を既定にする。
- 生成後に §6.4 の検査を返し、AI(LLM)が自分で直せるようにする(休み 0.4%、1 小節 3.3 半音、跳躍 0 のような数値を出す)。

---

## 8. 出典一覧(種類別)

**研究(一次)**
- Hamilton & Pearce 2024, Scientific Reports: https://www.nature.com/articles/s41598-024-64571-x(報道 https://phys.org/news/2024-07-song-melodies-simpler.html)
- Clark & Arthur 2023, Empirical Musicology Review 17(2): https://pdfs.semanticscholar.org/2ee5/5c2355a529368fea2998c6adecf2801dbaab.pdf
- Temperley & de Clercq 2013, JNMR: https://www.midside.com/publications/temperley_declercq_2013.pdf
- Van Balen et al. 2013, ISMIR: https://webspace.science.uu.nl/~veltk101/publications/art/ismir2013-chorus.pdf
- Jajoria et al. 2024, SMC: https://www.lsv.uni-saarland.de/wp-content/uploads/2025/07/SMC2024_Tresillo.pdf
- 畦地希美 2007, 音楽教育実践ジャーナル 5(1): https://www.jstage.jst.go.jp/article/jjomep/5/1/5_25/_pdf/-char/ja
- Burns 1987, Popular Music: https://www.tagg.org/xpdfs/burns87.pdf
- Warnerfjord 2023, KTH: https://kth.diva-portal.org/smash/get/diva2:1779208/FULLTEXT01.pdf
- MELONS(arXiv 2021): https://arxiv.org/pdf/2110.05020 / MeloForm(ISMIR 2022): https://archives.ismir.net/ismir2022/paper/000068.pdf
- Huron 2006 書評: https://www.doc.gold.ac.uk/~mas03dm/papers/huron06-review.pdf

**作家・演奏家の発言(一次)**
- Pat Pattison: https://www.patpattison.com/art-of-phrasing / https://www.patpattison.com/rhythms-and-variations
- Ryan Tedder(NPR 2010 の引用): https://mysongcoach.com/interview-with-ryan-tedder/
- SEIDS(2024): https://boombox.io/blog/how-to-write-a-topline-from-a-pro-topliner-seids/
- Chilly Gonzales(2014): https://drownedinsound.com/in_depth/4148030-rappers-and-melody--an-analysis-by-chilly-gonzales
- 近藤浩治(GDC 2007 報道): https://game.watch.impress.co.jp/docs/20070308/kondo.htm / https://en.wikipedia.org/wiki/Super_Mario_Bros._theme
- 植松伸夫(報道): https://www.gamesradar.com/games/final-fantasy/final-fantasy-legend-nobuo-uematsu-says-video-game-music-is-getting-more-boring-and-baldurs-gate-3-publishing-lead-agrees-nothing-is-made-for-anyone-in-particular-anymore-except-shareholders/
- Hal Galper: https://halgalper.com/articles/understandingforwardmotion/
- Adam Neely(報道): https://musictech.com/news/music/adam-neely-generative-ai-replacing-music-craft/

**教材・解説(二次)**
- MusicRadar(Avicii、2024): https://www.musicradar.com/how-to/avicii-melodies
- Computer Music(Animals、2016): https://www.musicradar.com/tuition/tech/anatomy-of-a-hit-martin-garrix-animals-636619
- Hooktheory TheoryTab: Levels / Animals / Strobe / Don't You Worry Child / Faded(https://www.hooktheory.com/theorytab/view/...)、SongMetrics: https://www.hooktheory.com/song-metrics/about
- Attack Magazine(2012): https://www.attackmagazine.com/technique/passing-notes/ostinatos-and-acid-house-riffs/
- Trance Producer(Steve Allen、2026): https://tranceproducer.co.uk/blogs/news/how-to-write-trance-melodies-and-arps-from-scratch
- myloops: https://www.myloops.net/how-to-create-melodic-techno-chords-and-melodies / https://www.myloops.net/programming-trance-arpeggios-and-rhythmic-sequences / https://www.myloops.net/building-a-powerful-trance-breakdown
- EDMProd: https://www.edmprod.com/advanced-melodies-chord-tones-motifs/ / https://www.edmprod.com/how-to-make-future-bass/ / https://www.edmprod.com/how-to-make-house-music/
- Hyperbits: https://hyperbits.com/blog/write-better-melodies/
- Shed the Music: https://www.shedthemusic.net/free-resources/melodic-development
- EDM Tips(Will Darling): https://edmtips.com/melody-patterns-every-producer-should-know/
- Gary Ewer(2016): https://www.secretsofsongwriting.com/2016/08/26/toplining-which-principles-of-songwriting-apply/
- Top40Theory: https://www.top40theory.com/blog/max-martins-melodic-math-repetition-tweaks-in-taylor-swifts-shake-it-off
- Beatportal(テックハウス 2024 / ピアノハウス): https://www.beatportal.com/articles/692607-step-by-step-guide-to-producing-tech-house-like-fisher-chris-lake-cloonee-and-michael-bibi / https://www.beatportal.com/articles/366921-the-eternal-appeal-of-piano-house
- MusicRadar(ディープハウス 2008): https://www.musicradar.com/tuition/tech/22-deep-house-production-tips-153784
- Open Music Theory: https://human.libretexts.org/Bookshelves/Music/Music_Theory/Open_Music_Theory_2e_(Gotham_et_al.)/07:_Popular_Music/7.01:_Rhythm_and_Meter_in_Pop_Music
- SRM(2022): https://srm-music.com/melody/
- SoundQuest: https://soundquest.jp/quest/melody/melody-mv1/motif-and-variations/
- Sleepfreaks(2022): https://sleepfreaks-dtm.com/chord-analize/hit-j-pop-2021-2022/
- AudioCipher(2025): https://www.audiocipher.com/post/chatgpt-music
- SoundBridge(2023): https://www.soundbridge.io/add-pitch-bend-automation-to-your-melodies
- Jazzadvice: https://www.jazzadvice.com/lessons/15-mistakes-beginner-jazz-improvisers-make/

**確認できなかったもの**
- Hooktheory による旋律の音度分布の全体集計(公開記事を見つけられず)
- 近藤浩治の「休符の空け方はパズル」という発言の原典
- Rick Beato「How this 1-note melody took over pop music」の中身
- Jack Perricone『Melody in Songwriting』(Berklee Press)の本文(書誌のみ確認)
- Andrew Huang の旋律についての特定の動画
