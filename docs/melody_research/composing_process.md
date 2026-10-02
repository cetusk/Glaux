# 人は旋律・曲をどう作り、どう直しているか ― 作曲の過程・改稿の研究と実践

調査日: 2026-09-30。対象: 作曲過程の実証研究、スケッチ研究、ソングライティング・制作の現場、教材、作曲支援の道具。
目的: Glaux の AI の旋律生成(動機を作って並べ、局所的に変形する 1 回きりの生成)を、人の「粒度を上下しながら読み返し・聴き返す改稿のループ」に近づけるための材料。

**出典の区別**
- [一次・本文] 論文・書籍・記事の本文を自分で読んで確認した
- [一次・要旨] 一次文献の要旨(Crossref・出版社・ERIC の要旨)だけを確認した
- [二次] 解説記事・学位論文・百科事典・他者の要約・検索結果の抜粋を経由した
- [推測] 私の解釈・Glaux への当てはめ

補足: 今回はウェブ検索の枠を使い切った状態から始めたため、DuckDuckGo の HTML 版・Crossref の API・PDF の直接取得で調べた。有料の論文(Collins 2005 の本文、Cooper 1990 の本文など)は要旨や他者の要約までしか確かめられていない。その箇所はそう書いてある。

---

## 0. 要点(先に結論)

1. **作曲は「計画 → 実行」の一方通行ではない。問題が次々に生まれ、解いた結果が次の問題を作る**。Reitman(1965)はこれを「制約の増殖」、Collins(2005)は「問題の増殖と解の逐次実装」と呼び、**直線的にも再帰的にも**進むと報告した。巨視(macro)と微視(micro)の両方の粒度で「まとまり(chunk)」が見られる。
2. **うまくいく人は「全体 → 部分 → 全体」を往復する**。子どもの共同作曲(Wiggins 1994)でも、成功した組は最初に全体を思い描き、動機を作り、また全体に組み直して通して演奏していた。ランダムな探索はほとんど無かった。
3. **初心者は局所に閉じる**。和声課題で、初心者は和音を 1 つずつ解き、「生まれつつある譜面の形と釣り合い」をほとんど見なかった。熟達者は技術的な細部と、声部の動き・全体の釣り合い・様式を**同時に**見ていた(Colley ほか 1992)。文章の改稿でも同じで、学生は語・文の単位の「チェックリスト」で直し、「全体が何を必要としているか」を問わない。熟練者は**改稿の周回ごとに見る水準と判断基準を変える**(Sommers 1978/1980)。
4. **歴史的な大作曲家も「骨格を通しで書く → 直す」**。モーツァルトは楽章全体の旋律と低音を先に書き(草稿総譜)、内声は後で埋めた(Konrad)。ベートーヴェンは単旋律の「通しの下書き(continuity draft)」を何度も書き直し、「歓喜の歌」の冒頭主題だけで 19 通りの異稿がある(Winter 1977)。**冒頭 4 小節は早く決まり、苦労したのはその後の続き・6 小節目・14 小節目以降・終止・リズムの単調さ**だった。
5. **聴き返しの頻度は熟達の指標**。トラッカー(打ち込みソフト)の熟達者は、編集と試聴を細かく交互に行い、試聴の間の編集時間の中央値は **13.2 秒**。初心者は **67.2 秒**で、試聴までに多くを変える(Nash & Blackwell 2012)。1 回の試聴は中央値 1.84 秒(1 拍〜1 小節)と短い。一方、シーケンサー(DAW)では 1・2・4 小節や、区間・曲全体の長い通しの試聴が多い。
6. **現場の実践書は「短く聴く → 聴く範囲を広げる」「時間を置いて聴く」「足すより削る」を勧める**。DeSantis(Ableton『Making Music』2015)は、1〜2 小節のループで聴いた後に**ループを段階的に広げる**こと、曲の階層を「クリップ → 句 → 区間 → 曲全体」と捉えること、区間の境界の不自然さを粗い操作と細かい操作で消すことを勧める。理由の一つは「**良いものを想像するより、悪いものを聴き分けるほうが易しい**」。
7. **計画は「決めるもの」ではなく「状況の準備」で、書いた結果で書き換わる**。Leroux の作曲過程を調べた Donin & Theureau(2007)では、作曲家は既に書いたものを読み返しながら先の計画を直し、途中で 3 つの楽章を 1 つに統合した。**まとまりの問題は、楽章の水準と作品全体の水準の 2 つで同時に**扱われていた。
8. **AI と人の共同作曲の研究でも同じ結論**。AI が一度に全部を生成すると初心者は圧倒され、直すのは「部分の再生成で彫る」だけになる。少しずつ作り、途中で止めて評価する「チェックポイント」、意味のある単位(声部・時間)での分割、複数案(既定 3 案 + 原案)を聴き比べる機能が使われた(Cococo、Louie ほか CHI 2020)。AI Song Contest(Huang ほか 2020)では、**モデルが曲の構造を知らないため、人が全体の骨格を先に作り、区間の対比を手で作り、同じ区間の 2 回目を変えた**。

---

## 1. 作曲過程の実証研究

### 1.1 Reitman(1965)― 思考発話による最初期の作曲研究 [二次]
- 出典: W. R. Reitman, *Cognition and Thought: An Information-Processing Approach*, Wiley, 1965。内容は Eamonn Bell「On fugues and functionalism」(2020)の解説で確認。https://www.eamonnbell.com/blog/2020/11/20/on-fugues-and-functionalism/
- 知見: ピアニストが複数回の記録セッションで思考発話しながら無調のフーガを作曲(書き起こしは約 150 ページ)。課題の制約は「最終的にフーガであること」だけ(p.169)で、**制約は最初から在るのではなく作曲の途中で次々に現れた**(Reitman の用語で「制約の増殖(constraint proliferation)」)。例: 対旋律の案を「主題の繰り返しになってしまうから駄目」と退ける。作曲者は問題そのものを作り変え続けた。
- Reitman はこの研究から「定義のあいまいな問題(ill-defined problem)」を体系的に定義した。

### 1.2 S. Bennett(1976)― 職業作曲家 8 人への面接 [一次・要旨]
- 出典: Stephen Bennett, "The Process of Musical Creation: Interviews with Eight Composers", *Journal of Research in Music Education* 24(1): 3–13, 1976. doi:10.2307/3345061
- 知見: 過程はしばしば「**萌芽の着想(germinal idea)**」の発見から始まり、萌芽の短いスケッチ → **第 1 稿** → **第 1 稿の精緻化と洗練** → 最終稿の完成と浄書、と進む。初めての作品は平均 12.1 歳で、多くは歌か旋律。
- 注意: 面接による自己報告で、各段階の中での往復は要旨からは分からない。

### 1.3 Kratus(1989・1994・2001)― 子どもの作曲の時間の使い方 [一次・要旨]
- 出典: J. Kratus, "A Time Analysis of the Compositional Processes Used by Children Ages 7 to 11", *JRME* 37(1): 5–20, 1989. doi:10.2307/3344949 / "Relationships Among Children's Music Audiation and Their Compositional Processes and Products", *JRME* 42(2): 115–130, 1994. doi:10.2307/3345496 / "Effect of Available Tonality and Pitch Options...", *JRME* 49(4): 294–306, 2001. doi:10.2307/3345613
- 知見:
  - 60 人(7・9・11 歳)に 10 分で歌を作らせ、時間を「**探索(exploration)・展開(development)・反復(repetition)・沈黙(silence)**」に分けて測った。自分の歌を再現できた子とできなかった子では反復と探索の使い方が有意に違い、「**再現できる歌を作るには反復が必要**」。9・11 歳は大人の作曲家の報告に近い形で探索・展開・反復を使う。
  - 1994(9 歳 40 人): 音を頭の中で聴く力(audiation)は、**展開・沈黙の時間と正の相関、探索と負の相関**。その力は曲の調・拍の一貫性、リズム型の展開と正の相関。過程(どう時間を使ったか)は作品のまとまり・型の使い方・長さと関係した。
  - 2001(4 年生 48 人): 使える音板を 5 本から 10 本に増やすと、**探索が増え、曲は長くなり、再現できなくなった**。
- [推測] 選択肢を増やすと探索が増えてまとまりが落ちる。「頭の中で聴いて考える(沈黙)」時間が展開と一緒に増える。AI の生成でも、候補の空間を広げるほど「評価して戻る」工程が要る。

### 1.4 Colley ほか(1992)― 熟達者と初心者の比較(思考発話) [一次・要旨]
- 出典: A. Colley, L. Banton, J. Down, A. Pither, "An Expert-Novice Comparison in Musical Composition", *Psychology of Music* 20(2): 124–137, 1992. doi:10.1177/0305735692202003
- 知見: バッハのコラールを 1 時間で完成させる課題(ソプラノと最初の句の半分を提示)。熟達者 1 人・初心者 3 人。**完成できたのは熟達者だけ**。熟達者は基本の技術に注意しながら、**同時に**声部の動き・釣り合い・ジャンルの典型を考えていた。初心者は**和音ごとの解**に集中し、和声の規則は当てはめたが、**生まれつつある譜面の形と釣り合いをほとんど無視**した。
- [推測] Glaux の「局所の微調整は全体から浮く」の構図と同じ。

### 1.5 Younker & Smith(1996)― 熟達者と初心者の思考過程 [二次]
- 出典: B. A. Younker & W. H. Smith, "Comparing and Modeling Musical Thought Processes of Expert and Novice Composers", *Bulletin of the Council for Research in Music Education* 128: 25–36, 1996(JSTOR 40318786)。
- 確認できたのは被験者の構成だけ(大人の熟達者=作曲専攻の大学院生、大人の初心者=作曲経験の無い音楽教師、17 歳の熟達者 など)。このデータから、Smith & Smith が熟達者と初心者の「上位の行動」を生成規則として書いたモデルを作った(Springer 刊『Music Education: An Artificial Intelligence Approach』所収)。**結論の詳細は本文未確認**。

### 1.6 Wiggins(1994)― 「全体 → 部分 → 全体」 [一次・要旨]
- 出典: J. Wiggins, "Children's Strategies for Solving Compositional Problems with Peers", *JRME* 42(3): 232–252, 1994. doi:10.2307/3345702
- 知見: 小学 5 年の音楽の授業を 5 か月、映像と録音で追った。課題を完成できた組の方略は「**全体(最初の計画)→ 部分(動機の展開)→ 全体(組み直して練習)**」という型。決定は**最初から完成品を思い描いた全体的な視点**から出ていた。**ランダムな探索はごく少なかった**。

### 1.7 Folkestad, Hargreaves & Lindström(1998)― コンピュータでの作曲の「横」と「縦」 [一次・要旨]
- 出典: G. Folkestad, D. J. Hargreaves, B. Lindström, "Compositional Strategies in Computer-Based Music-Making", *British Journal of Music Education* 15(1): 83–97, 1998. doi:10.1017/S0265051700003788
- 知見: 15〜16 歳による 129 曲を 3 年間調べ、MIDI ファイルを作曲の途中の段階ごとに体系的に集めた。6 つの作り方が見つかり、大きく**横(HORIZONTAL)**と**縦(VERTICAL)**に分かれる。横は「作曲」と「編曲」が別の工程、縦は作曲と編曲が一体の工程。
- [推測] 横=旋律を先に時間順に書き、後で編曲。縦=ループの上に層を重ねていく DAW 的な作り方。Glaux の AI は「横」の書き方を一度だけしている。

### 1.8 Collins(2005・2007)、Collins & Dunn(2011)― 保存ファイルで追った作曲の実時間 [一次・要旨 + 二次]
- 出典:
  - D. Collins, "A synthesis process model of creative thinking in music composition", *Psychology of Music* 33(2): 193–216, 2005. doi:10.1177/0305735605050651 [一次・要旨]
  - D. Collins, "Real-time tracking of the creative music composition process", *Digital Creativity* 18(4): 239–256, 2007. doi:10.1080/14626260701743234 [一次・要旨(抜粋)]
  - D. Collins & M. Dunn, "Problem-solving strategies and processes in musical composition: Observations in real time", *Journal of Music, Technology & Education* 4(1): 47–76, 2011. doi:10.1386/jmte.4.1.47_1 [一次・要旨]
  - Scribd に掲載された要約(著者不明) https://www.scribd.com/document/646823424/Collins [二次]
- 知見:
  - 1 人の作曲家(委嘱作品)を **3 年間**追った。「名前を付けて保存」した MIDI ファイル、録音、半構造化面接、直後の回想発話、作曲家と研究者の照合で過程を再構成。
  - **巨視・微視の両方の水準で、過程と方略がまとまり(chunk)になっていた**。
  - 仮説モデル: 「**問題の増殖と解の逐次実装**」という生成的な過程で、**直線的にだけでなく再帰的にも**起きる。
  - **洞察(ひらめき)の瞬間**は、ゲシュタルト心理学の「問題の再構造化」に当たり、いくつかは実時間で重なって起きた(並列性)。二次の要約では、洞察は**主題を並べ替えた後や小さな編集の後**に起き、作曲家は**解を先送りし、再構造化を入れ子にしながら**複数の操作を同時に扱っていた。明確な最終形を持たず、下位目標を作っては解く過程だった。
  - 2011 年(大学生 3 人、思考発話・コンピュータのデータ・映像): **問題解決の方略は直線的な周期**、**音楽の構造化は非直線的・再帰的**で、巨視・微視の両水準で観察された。
- 注意: 本文は有料で未読。「巨視・微視」の具体的な粒度(何小節か)や、保存ファイルの数は確認できていない。

### 1.9 Burnard & Younker(2002・2004)― 作曲の道筋 [一次・要旨]
- 出典: P. Burnard & B. A. Younker, "Problem-Solving and Creativity: Insights from Students' Individual Composing Pathways", *International Journal of Music Education* 22(1): 59–76, 2004. doi:10.1177/0255761404042375 /「Mapping Pathways」*Music Education Research* 4(2), 2002.
- 知見: オーストラリア・カナダ・英国の生徒の作曲中の思考を比べた。共通の主題の一つは「**問題を見つける(知覚・枠付けする)ことと、解を探すことは、別種の思考を使う**」。
- [推測] AI でも「問題を見つける役(批評)」と「解を出す役(生成)」を分けるのは筋が通る。

### 1.10 Kennedy(2002)― 高校生の作曲 [一次・要旨]
- 出典: M. A. Kennedy, "Listening to the Music: Compositional Processes of High School Composers", *JRME* 50(2): 94–110, 2002. doi:10.2307/3345815
- 知見: 4 人の高校生を 2 つの課題で追い、過程のモデルを作った。モデルの重要な特徴は「**聴くことの役割**」「**一人で考える時間の必要**」「完成品の即興的な性格」。過程は個人ごとに違う。

### 1.11 Fautley(2005)― 集団作曲の過程のモデル [二次]
- 出典: M. Fautley, "A new model of the group composing process of lower secondary school students", *Music Education Research* 7(1): 39–57, 2005. doi:10.1080/14613800500042109。段階の名前は ResearchGate の引用抜粋で確認。
- 知見: 最初の確認 → 着想の**生成**と**探索** → **組織化** → **途中経過の演奏(work-in-progress performances)** → **改訂** → **変形と修正** → 最終演奏。
- [推測] 「途中経過の演奏」が改稿の引き金になっている。AI では「途中で通して聴く(評価する)」工程がこれに当たる。

### 1.12 McAdams(2004)― Roger Reynolds の作曲を着想から初演まで追う [一次・要旨]
- 出典: S. McAdams, "Problem-Solving Strategies in Music Composition: A Case Study", *Music Perception* 21(3): 391–429, 2004. doi:10.1525/mp.2004.21.3.391
- 知見: ノート・スケッチ・図・面接・最終譜から、3 つの作曲上の問題の解き方を追った。(1) 5 つの主題(**23〜100 秒**)をピアノ独奏と室内オーケストラの両方のために書く。(2) 2 つの大きな部分を、**似た時間構造で、同じ主題素材を全く違う仕方でたどる**ように作る。ここでは**空間的・図的な表現**と、譜面の図的な自己組織化が決め手になった。(3) 電子音の部分は、連続的な質感のまとまりに分け、**器楽の最終作曲の前に確定**させた。
- [推測] 時間構造(枠)を図で先に作り、素材を後からその枠に流し込む。上位の骨格を先に固定する例。

### 1.13 Donin & Theureau(2007)― Leroux《Voi(rex)》の作曲の活動分析 [一次・本文]
- 出典: N. Donin & J. Theureau, "Theoretical and methodological issues related to long term creative cognition: the case of musical composition", *Cognition, Technology & Work* 9: 233–251, 2007. doi:10.1007/s10111-007-0082-z(著者公開 PDF https://yannickprie.net/archives/ENACTION-SCHOOLS/docs/documents2009/donin-theureau.pdf)
- 方法: スケッチ・計画・ソフトの作業ファイルなどの痕跡をすべて集め、作曲家に作業を再現・説明してもらう「状況の再現面接」。
- 知見:
  - **計画ではなく「状況の準備」**: 作品全体を書く準備で、作曲家は計画するというより、書く瞬間に初めて定まる状況を準備している。その際「**読み返し**」によって、それまでに書いたものすべてを考慮する。譜を書くことは「その箇所を書くこと」と「この先の書く状況を準備すること」を同時に行う。書くことは**過去の絶えざる再定義**を伴う。
  - **知覚と行為のループが「発見と創造のループ」に発展する**。書いている最中の内的な聴取と実際の音の両方で「驚き」が起き、それが作曲を動かす。
  - 例 1: 演奏家に 26 の主要和音をそれぞれ十数通りで演奏させた録音(1 時間以上)を ProTools に並べて聴き直したところ、いくつかの長く伸ばした音が、以前に紙切れにメモしたまま使っていなかった「和音の平塗り(aplats d'accords)」という着想に合い、**約 15 秒の音を基に第 3 楽章全体を作る**と決めた。
  - 例 2: 第 3 楽章を終えた時点で**作品全体の計画を変え**、第 4 楽章を捨て、最初の計画の第 6・7・8 楽章を連結して 1 つの第 5 楽章にした。作曲家自身「楽章としてはあまり一貫していないが、作品全体の一部としては完全に一貫している」と述べる。**まとまりの問題は、楽章の水準と作品全体の水準で同時に**問われていた。
  - 前の段階との関係の種類: 予期の明確化と実行、素材の使い切り・再利用、構造の転用、着想から具体化への移行。先行楽章への参照(ドローン、旋律の輪郭)は「使う瞬間に」機会を捉えて入れる。

### 1.14 Pohjannoro(2016)― 直観と省察 [一次・要旨(検索結果の抜粋)]
- 出典: U. Pohjannoro, "Capitalising on intuition and reflection: Making sense of a composer's creative process", *Musicae Scientiae*, 2016. doi:10.1177/1029864915625727
- 知見: 作曲家のスタジオで作曲中に「刺激回想面接」を行い、手稿の全体と照合。ある危機的な局面の後、作曲家は「**合理的直観**」という状況限定の認知の道具を採った。流れるように書きながらも合理性を保ち、**美的な一貫性**を作る方法だった。

### 1.15 Webster のモデル(1990・2002)[二次]
- 出典: P. R. Webster, "Creative Thinking in Music: Advancing a Model", in T. Sullivan & L. Willingham (eds.), *Creativity and Music Education*, Canadian Music Educators' Association, 2002。段階の名前は ResearchGate の図の説明で確認。1990 年版は *Music Educators Journal* 76(9) の特集。
- 知見: 中心は 4 段階の創造的思考の過程: **準備**(素材・着想を集める)→ **離れる時間**(time away)→ **やり通す**(working through)→ **検証**。その周りに「可能にする技能」(音を頭の中で聴く力など)と「可能にする条件」(動機・環境など)。拡散的思考と収束的思考を行き来する。
- 注意: Wallas(1926)の準備・孵化・啓示・検証を音楽に当てはめたもの。実証の細部は本文未確認。

### 1.16 Barrett(2006)― 著名な作曲家教師と学生の 1 学期 [一次・要旨]
- 出典: M. S. Barrett, "'Creative collaboration': an 'eminence' study of teaching and learning in music composition", *Psychology of Music* 34(2): 195–218, 2006. doi:10.1177/0305735606061852
- 知見: 作曲の教えと学びは、共通の目標に向かう 2 人の**協働・共同の努力・社会的な支え**として描かれた。具体的な指導方略は要旨からは確認できない。

---

## 2. スケッチ研究(歴史的な作曲家の下書きと改稿)

### 2.1 ベートーヴェン

**(a) スケッチの性格 ― Cooper『Beethoven and the Creative Process』(1990) [一次・要旨]**
- 出典: Barry Cooper, *Beethoven and the Creative Process*, Oxford: Clarendon Press, 1990。章「The Sketching of Melody」の要旨 https://academic.oup.com/book/49144/chapter/422053324
- 知見: スケッチの大半は**単旋律の下書き**で、そのため一見旋律の形に関心が向いているように見える。この印象は、**ある作品の連続するスケッチがほぼ必ず目立った旋律の変更を示す**ことで強まる(要旨の記述)。ベートーヴェンが大量の予備スケッチを書く習慣は生前から珍しがられていた。
- 本文(何をどう直したかの分類)は貸出制限で未確認。

**(b) 「歓喜の歌」の主題 ― Winter(1977)の研究とその紹介 [二次]**
- 出典: Robert Winter, "The Sketches for the 'Ode to Joy'", in *Beethoven, Performers, and Critics*(1977 年の国際ベートーヴェン会議の報告集、Wayne State University Press 1980)。書評(JSTOR 843524)の抜粋: Winter は冒頭主題に**約 19 の異稿**があることを示した。紹介記事: Jeffrey Arlo Brown(VAN Magazine)「The Best and Worst of Beethoven's "Ode to Joy" Sketches, Ranked」(Google Arts & Culture、ベルリン国立図書館の資料)https://artsandculture.google.com/story/PQVxEVX-TfU7Pg
- 知見(記事の要約):
  - シラーの詩による最初の旋律のスケッチは 1798〜99 年で、今の旋律とは無関係。1823 年に本格的に作曲し、終楽章のスケッチは**約 50 ページ**。**1823 年 6 月までに冒頭の句は決まっていた**が、全体の完成には多くの作業が要った。
  - 退けられた案の問題点(記事が挙げたもの): 同じ 3 音の単調な**反復**で終わりが尻すぼみ / 西洋音楽で最も単純な**和声進行**に頼る / **3/8 拍子**にした案(拍子の選択の誤り)/ **2 つずつの組で動く**単調さ / 同じ音(A)の保続が貧弱 / 各句の**最後の小節が穏やかな順次進行**で弱い、短調への転換が唐突 / **4 分音符が続いてリズムも旋律も「のろのろ」** / ほぼ完成形だが **6 小節目と 14 小節目以降**が悪い。
- [推測] 直された箇所は、**句の終わり(終止)、句の後半の続き、リズムの単調さ(同じ音価の連続・同じ組の反復)、拍子**。冒頭の動機(4 小節)は早く決まり、その**続きと全体の起伏**に時間がかかった。Glaux の状況(動機は作れるが続き・全体が弱い)と同じ構図。

**(c) 《英雄》第 1 楽章の「通しの下書き」 ― Lockwood & Gosman の校訂版と Posen の学位論文 [二次]**
- 出典: Lewis Lockwood & Alan Gosman (eds.), *Beethoven's "Eroica" Sketchbook: A Critical Edition*, University of Illinois Press, 2013。Thomas Posen の学位論文の紹介ページ https://www.thomasposen.com/scholarship/dissertation/form-function-theory-beethoven-sketches/
- 知見: 提示部の**最初の通しの下書き(continuity sketch)**では、冒頭主題が **6 回**現れ、ソナタの慣習を「大きく乱して」いた。属調(変ロ長調)で主要主題が入る箇所は、意外に多くの後続の下書きで残された後に削られた。次の下書きでは主要主題と推移を保ったまま**副次主題をいろいろに変え**、後の稿では**副次主題の 2 つの部分を 1 つに統合**した。
- [推測] ベートーヴェンは**楽章全体を 1 本の線で通して書き、その通しの版を何度も書き直す**。部分だけを磨くのではなく、毎回「全体の通し」を作り直している。

### 2.2 モーツァルト ― 旋律と低音を楽章の最後まで先に書く [二次]
- 出典: Ulrich Konrad, *Mozart-Werkverzeichnis / Mozarts Schaffensweise*(1992)。英語版 Wikipedia「Mozart's compositional method」(Konrad 2006, p.103 を引用)https://en.wikipedia.org/wiki/Mozart%27s_compositional_method と、Konrad の論考の英訳の要約(Scribd)。
- 知見:
  - スケッチと草稿が**約 320 点**残り、作品の**約 1 割**をカバーする。独奏鍵盤曲のスケッチは無い。
  - 最も原始的なスケッチは走り書きで断片だけ。進んだスケッチは**最も目立つ線(旋律と、しばしば低音)**を書き、他の線は後で埋める。
  - 「**草稿総譜**」の段階でモーツァルトは作品を完成とみなし(1784 年以降は自作目録に記入)、その後に**内声と和声を肉付け**して清書した。
  - Konrad は、スケッチ・草稿(Entwurf)・パルティチェルのほか、流れを追う「**経過スケッチ(Verlaufsskizze)**」を区別する。
- [推測] 「**旋律と低音(骨格)を楽章の最後まで通す → 細部を埋める**」という上位の粒度を先に固める書き方。

### 2.3 チャイコフスキー ― 萌芽と「冷静な頭の仕事」、そして「縫い目」 [一次(書簡の英訳)・出典に不確かさあり]
- 出典: フォン・メック夫人宛書簡(1878 年 2 月 17 日/3 月 1 日、フィレンツェ)。Rosa Newmarch 編 *The Life and Letters of Peter Ilich Tchaikovsky*(1905)pp.274–275 の英訳を、Tchaikovsky Research のフォーラムで確認。http://www.tchaikovsky-research.net/en/forum/forum0223.html
- 知見: 「作品の萌芽は突然、思いがけなく来る」。土壌が整っていれば急速に根を張り、枝葉を伸ばす。**スケッチを始めると考えが次々に続く**。中断されると糸が切れ、そのときは「**冷静な頭の仕事と技術の知識**」が助けに来なければならない。1892 年の談話では「私の仕事の仕方はまったく職人的で、毎日同じ時間に規則正しく」働くと述べた。
- 形式についての嘆き(出典の特定なし、*Tchaikovsky: A Symposium*(1945)p.26 の英訳): 「生涯、音楽の形式を掴み操れないことに悩んできた」「経験のある目には**私の縫い目の糸が見える**」。https://www.tchaikovsky-research.net/en/forum/forum0058.html
- [推測] 部分と部分の**つなぎ目**が目立つことは、大作曲家にも自覚される典型的な弱点だった。

### 2.4 ヒンデミット ― 「稲妻のような全体の見通し」と「細部の寄せ集め」 [一次・本文]
- 出典: Paul Hindemith, *A Composer's World: Horizons and Limitations*, Harvard University Press, 1952, pp.60–62(Internet Archive の全文 https://archive.org/details/composersworldha002179mbp)
- 知見(本文): 夜の稲妻の一瞬に風景の全体と細部が見えるように、「**一瞬のうちに作品をその全体として、関連する細部がすべて正しい位置にある形で見られないなら、真の創造者ではない**」。ただしそれは第 612 小節の嬰ヘ音まで最初に決まるという意味ではない。最初の一瞬に細部に注意を向ければ全体は見えない。全体が見えていれば、細部は「全体の要求を満たす」ように決まる。
- ベートーヴェンの素材との格闘は、着想(Einfall)を良くするためではなく「**思い描いた全体の変えられない必然に合わせるため**」で、そのために**5 つ以上の版**を経て原形が分からないほど変えることもあった。
- 平凡な才能は「多くの美しい細部を継ぎ合わせれば全体も美しくなる」という公式で**細部を寄せ集め**るが、それは「収集」であって「有機体」を作らない。長い記譜の間に最初の見通しが薄れる危険があり、それを保つのが才能の特徴。
- 注意: これは規範的な主張で、実証ではない。Collins(1.8)や Donin(1.13)の観察では、全体の見通しそのものも途中で書き換わる。[推測] 両方をまとめると「**全体の見通しを常に参照して細部を決める。ただし全体の見通しも細部の発見で更新する**」。

---

## 3. 大衆音楽のソングライティング・制作の現場

### 3.1 Joe Bennett(2011)― 共作の過程 [一次・本文]
- 出典: Joe Bennett, "Collaborative songwriting – the ontology of negotiated creativity in popular music studio practice", *Journal on the Art of Record Production* 5, 2011. https://www.arpjournal.com/asarpwp/collaborative-songwriting-–-the-ontology-of-negotiated-creativity-in-popular-music-studio-practice/
- 知見:
  - 共作では 6 つの非直線的な過程が相互に働く: **刺激(stimulus)**(誰かが出す最初の素材)→ **承認 / 改変 / 交渉 / 拒否(veto)/ 合意**。拒否された案は受け入れるか、交渉して擁護・改変する。
  - 曲はほとんど無からは始まらない。ナッシュビルの作家はタイトルを持って来る、バンドはリフから、現代のポップはドラムマシンや伴奏トラックから。**トップライン(伴奏トラックの上に旋律と歌詞を書く)**という型がある。
  - 共作者は「**すぐそこにいる聴衆、もう一組の耳**」として働き、案が評価を通る確率を上げる。
  - 作業中はしばしば「**そこは後で直そう(fix that bit later)**」と合意し、仮の歌詞や仮の音で勢いを保ち、完璧主義を後回しにする。
  - **作り方が作品を形作る**: ドラムループは速いテンポ(120 BPM 以上)を促し、シーケンサーのループ機能はループ型の和声構造を普通にする。
- [推測] 「刺激 → 評価(拒否・改変)」の短い周期と、「仮置きして先に進む」ことが並存する。

### 3.2 ポップのトップライナーの実例 ― Ester Dean と Stargate [二次]
- 出典: John Seabrook, "The Song Machine", *The New Yorker*, 2012-03-26(本体は取得できず、Longreads の抜粋で確認)https://longreads.com/2012/03/22/the-song-machine/
- 知見: トップライナーの Ester Dean は、プロデューサーのトラックに合わせて、最初は「ナナナ」「バババ」といった**言葉以前の音**で旋律を歌い、次に携帯のメモからばらばらの言葉を拾って歌う。歌の中には**ヴァースもサビも無く、ただ違う旋律とリズムの部分**があった。整った歌詞はかえって旋律の才能を縛る、と Seabrook は書く。
- [推測・未確認] 記事の文脈からは、構成(どの断片をどの区間に置くか)は後で決める作り方と読めるが、選び方・組み立て方の詳細は本文で確認できていない。

### 3.3 Nash & Blackwell(2012)― 編集と試聴の周期を実測 [一次・本文]
- 出典: Chris Nash & Alan Blackwell, "Liveness and Flow in Notation Use", *Proceedings of NIME 2012*. https://www.nime.org/proceedings/2012/nime2012_217.pdf
- 方法: 2 年間の現地調査。**1,000 人以上**のシーケンサーとトラッカーの利用者の操作の記録・質問紙・映像。
- 知見:
  - トラッカーの熟達者(映像研究): 右手でカーソルを編集箇所の直前に戻し、左手で「カーソル位置から再生」を押し、再生中も左手は停止キーの上。何か聞こえたらすぐ編集に戻る。本人はこれを「**その場のデバッグ**」と呼んだ。計画を意識せず、直観的に書く。
  - 175 人・1,195 セッションの最初の 30 分: トラッカーの試聴は非常に短い(**中央値 1.84 秒**、1 拍〜1 小節)。シーケンサーは **1・2・4 小節(2・4・8 秒)**や、10〜90 秒の長い試聴が多く、**一度決めた試聴の長さを長く使い続ける**。**曲全体の長い試聴**はシーケンサーのほうが多い(編集画面の範囲が広いため)。
  - 試聴の間の編集時間: 熟達者 **中央値 13.2 秒**(最頻値 17.1 秒、n=574)、初心者 **中央値 67.2 秒**(最頻値 155.8 秒、n=548)。試聴の間の編集量: 熟達者 中央値 **2.36 回の編集**、初心者 **5.44 回**。熟達者は見た目だけで長く作業できるはずなのに**そうしない**。細かい編集と短い試聴を**織り交ぜる**。
- [推測] AI に当てはめると「**1 回の変更は小さく、変更したら必ずすぐ評価する**」「局所の試聴(1 拍〜4 小節)と全体の通し(区間〜曲)の 2 種類を持つ」。

### 3.4 Duignan, Noble & Biddle(2010)― 職業プロデューサー 17 人の観察 [一次・要旨]
- 出典: M. Duignan, J. Noble, R. Biddle, "Abstraction and Activity in Computer-Mediated Music Production", *Computer Music Journal* 34(4): 22–33, 2010. doi:10.1162/comj_a_00023(学位論文の要旨で確認)
- 知見: 音楽制作は、建築・アニメ・3D と同じく「**継続的な洗練**」で複雑なデジタルの作品を作り直していく活動。17 人の職業プロデューサーの詳細な観察と面接から、マルチトラックの比喩が提供する抽象化と作業の関係を調べた。

### 3.5 DeSantis『Making Music: 74 Creative Strategies for Electronic Music Producers』(Ableton, 2015) [一次・本文]
- 出典: Dennis DeSantis, *Making Music*, Ableton, 2015(公式に無料公開の PDF)https://cdn-resources.ableton.com/resources/uploads/makingmusic/MakingMusic_DennisDeSantis.pdf
- 知見:
  - **少しずつ聴いて、範囲を広げる**(「Listen in Chunks」): ループを 1〜2 小節、長くても 1 句に設定し、必要なだけ繰り返し聴く。次の塊に進み、最後まで行ったら**冒頭に戻ってループの長さを徐々に広げ**、1 回の通しで聴く範囲を大きくする。
  - **距離を置く**(「Avoidance List」): 仕上げた曲を**数日後**に聴き返すと、以前の曲と似すぎていることに気づく。作業中や直後には気づかない。音色だけでなく**形式(同じ種類の移行・構成)**の似通いもある。
  - **階層**(「Fuzzy Boundaries」): 多くの曲は時間を区間に分け、区間の中にさらに対比する小区間がある。DAW はこれを「**クリップ → 句 → 曲の区間 → 曲全体**」の箱で表す。箱の縁は時間の境界を示すが、音楽として望む結果をいつも表すわけではない。境界をぼかすには、**粗い操作**(一部のトラックだけ前の区間の素材を延ばす・早めに終える・境界の両側を消す)と、**細かい操作**(クリップの中の素材を直す、例えばフィルの頂点を次の区間の頭の前に置く「先取り」)がある。
  - **引き算の編曲**(「Arranging as a Subtractive Process」): 編曲で詰まったら、まず**タイムライン全体を素材で埋める**(20 秒以内、判断しない)。その後は足すのではなく削る。絵を描くのではなく彫刻。理由は「**良いものを想像するより、悪いものを聴き分けるほうが易しい**」から。実際に時間の流れを聴きながら直せる。
- [推測] 「全体を粗く埋めた下書き → 通しで聴いて悪い所を削る・直す」は、ベートーヴェンの通しの下書き・モーツァルトの草稿総譜と同じ形。

---

## 4. 作曲・ソングライティングの教材が教える改稿の手順

### 4.1 Kachulis『The Songwriter's Workshop: Melody』(Berklee Press) [一次(目次、検索結果の抜粋経由)]
- 出典: Jimmy Kachulis, *The Songwriter's Workshop: Melody*, Berklee Press, 2004。目次は検索結果の抜粋で確認。
- 構成: 第 1 部「リズム」A. 着想: 第 1 課 **音の長さ** / 第 2 課 **句の長さと間(space)** / 第 3 課 **句の始まりと強勢のある語** / 第 4 課 **句の終わり** / 第 5 課 **タイトルを強拍で終える**。B. 曲の区間: 第 6 課 …(以降は未確認)。別の抜粋では「**リズムの道具でヴァース・プリコーラス・サビを対比させ、タイトルを強調する**」ヒット曲の例が挙げられている。
- [推測] 教材は旋律を**音高より先にリズム(音価・句の長さ・句の始まり/終わり)**で設計させ、**区間の対比をリズムで作る**。Glaux の「全体のリズムの輪郭が変わらない」への直接の処方。

### 4.2 Andrea Stolpe(Berklee Online の講師)[一次・本文(本人サイト)]
- 出典: "Melody Makes All the Difference" https://www.andreastolpe.com/articles/melody-makes-all-the-difference / "A Process for Your Songwriting" https://www.andreastolpe.com/articles/a-process-for-your-songwriting / 本人の SNS 投稿(検索結果の抜粋)
- 知見:
  - 区間ごとに旋律が特に重要な瞬間は 2 つ: **区間の最初の行**(聴き手が覚えて期待する旋律の主題を確立する)と、**その主題から離れる行**(緊張を生み、感情的に大事な歌詞を目立たせる)。
  - 改稿の手順: (1) 各区間の最初の行が、繰り返せる明確な旋律の主題になっているか確かめる → (2) 最後の行がそのパターンから離れているか確かめる → (3) 音高・リズム・音の長さを少しずつ変えて試す → (4) その変化が歌詞の感情にどう効くかを評価する。
  - 「リズムが区間を通してパターンを敷くので、旋律も区間を通して繰り返せる」。
  - 区間の対比の作り方: 「**和音が同じでも、次の区間で旋律を持ち上げる**。ヴァースは低く、次で上げる」。
- [推測] 区間単位の検査項目: 「最初の行=提示」「最後の行=逸脱」「次の区間で音域を上げる」。

### 4.3 Pat Pattison・Berklee の講義 [二次]
- 出典: Berklee Online の講義概要(「Lyric Writing: Tools and Strategies」: 押韻・リズム・行の長さのパターンで区間の中に前進する動きを作り、**ヴァース・サビ・ブリッジの間に対比を作る**)https://online.berklee.edu/courses/lyric-writing-tools-and-strategies
- Pattison の考え方の詳細(安定・不安定な区間の構造、句の長さの対比)は、Glaux の別の調査(practice.md の 3.1)に既にある。

### 4.4 文章の改稿の研究からの類推 ― Sommers(1978/1980)、Flower & Hayes(1981)[一次・本文(ERIC の学位論文)/ 一次・書誌]
- 出典: Nancy Sommers, *Revision Strategies of Student Writers and Experienced Adult Writers*(学位論文、ERIC ED220839)https://files.eric.ed.gov/fulltext/ED220839.pdf 。同名の論文が *College Composition and Communication* 31(4): 378–388, 1980。Flower & Hayes, "A Cognitive Process Theory of Writing", *CCC* 32(4): 365–387, 1981(計画・翻訳・見直しが再帰的に働くモデル。書誌のみ確認)。
- 知見(学位論文の本文と要旨):
  - 大学 1 年生 8 人と経験のある大人の書き手 7 人に 3 つの作文を書かせ、それぞれ 2 回書き直させた。
  - 学生は文章を「**非常に分子的な水準**」で見て、**語から文の水準**で評価する。文章を部分の連なりとして見て、「**全体として何が必要か**」を問わず、個々の語・句・文をどう変えるかを問う。**内面化した定型のチェックリスト**で直す。「統一」「形式」のような全体的な概念でさえ、「序論・本論・結論がある」という部品の総和の意味にしか取らない。ある段落が良くないと思っても、削除や追加ではなく**機械的に段落を並べ替える**だけ。
  - 熟練者にとって改稿は「段階」ではなく、**書く過程の全体で起きる**。第 1 稿そのものが、改稿の理論で案を捨て選んだ結果。改稿の**水準と課題の違いを見て、周回ごとに関心を層に分ける**。**第 1 稿と第 2 稿で違う判断基準**を使い、ある関心を保留して別の関心を扱う。学生は第 1 稿と第 2 稿を**同じ基準**で評価し、早く終わらせようとする。
  - 改稿のモデルは「**不協和を感じ、耐え、解消する**」再帰的な過程。
- [推測] 音楽とは別分野だが、Colley ほか(1.4)と同じ構図で、AI の旋律生成にそのまま当てはまる。特に「**毎回同じチェックリストの点数で評価する**」ことは学生型の改稿で、熟練者型にするには「**周回ごとに見る水準を変える**」必要がある。

### 4.5 トップダウンとボトムアップ ― まとめ [推測(上の出典の整理)]
| 型 | 例 | 何を先に決めるか |
|---|---|---|
| トップダウン(骨格を先に通す) | モーツァルトの草稿総譜(旋律+低音を楽章の最後まで)、ベートーヴェンの通しの下書き、Reynolds の時間構造の図(McAdams 2004)、DeSantis の引き算の編曲、ナッシュビルの「タイトルから」 | 形式・時間の枠・主要な線 |
| ボトムアップ(素材から) | 萌芽の着想(S. Bennett 1976)、トップライナーの断片(Ester Dean)、Kratus の探索 → 展開、Stolpe の「歌詞の行から」 | 動機・フック・リズムの型 |
| 往復 | Wiggins の全体 → 部分 → 全体、Collins の直線+再帰、Donin の「計画=状況の準備、読み返しで書き換える」、ヒンデミット(全体の見通しで細部を決める) | 全体の見通しを持ちつつ、部分の発見で全体を更新 |

実証研究で**うまくいっている例はどれも「往復」**で、純粋なボトムアップ(動機を作って並べるだけ)は初心者の特徴として報告されている(Colley ほか、Sommers)。

---

## 5. 改稿で典型的に直すもの(実例から)

| 直す対象 | 実例と出典 | 確度 |
|---|---|---|
| **句の続きと終わり(終止)** | 「歓喜の歌」: 冒頭 4 小節は早く決まったが、句の最後の小節の穏やかな順次進行、6 小節目、14 小節目以降の終わり方を何度も直した(Winter 1977 → VAN) | 二次 |
| **リズムの単調さ** | 「歓喜の歌」: 4 分音符が続く「のろのろした」リズム、2 つずつの組で動く単調さを退けた / Kachulis の教材はリズム(音価・句の長さ・句の始まりと終わり)から設計させる | 二次 / 目次 |
| **拍子** | 「歓喜の歌」: 3/8 拍子の案を退けた | 二次 |
| **反復の量と場所** | 《英雄》: 最初の通しの下書きで主題が 6 回現れ、後で削った(Lockwood & Gosman → Posen) / 「歓喜の歌」: 同じ 3 音の反復 / AI Song Contest: 同じ区間を 2 回繰り返したら退屈だったので、2 回目の終わりを和声付けし直して変えた(Huang ほか 2020) | 二次 / 一次・本文 |
| **区間の構成(統合・削除・並べ替え)** | 《英雄》: 副次主題の 2 つの部分を 1 つに統合 / Leroux: 3 つの楽章を 1 つに統合、1 つを削除(Donin & Theureau 2007) / Collins: 主題を並べ替えた後に洞察が起きた | 二次 / 一次・本文 / 二次 |
| **区間の対比(音域・リズム)** | Stolpe: 次の区間で旋律を持ち上げる / Kachulis: リズムでヴァース・プリコーラス・サビを対比 / AI Song Contest: モデルでは対比を指定できず、ヴァースから続きを複数生成してサビを選んだ | 一次・本文 / 目次 / 一次・本文 |
| **区間の最初と最後の行** | Stolpe: 最初の行で主題を確立し、最後の行で離れる | 一次・本文 |
| **つなぎ目** | チャイコフスキー: 「縫い目の糸が見える」/ DeSantis: 区間の境界の不自然さを、一部のトラックだけ延ばす・早めに終える・フィルの頂点を前に置くなどでぼかす | 一次(英訳、出典不確か)/ 一次・本文 |
| **全体の見通しに合わせた細部** | ヒンデミット: ベートーヴェンは着想を「良くする」ためではなく全体の必然に合わせるために 5 つ以上の版を経た | 一次・本文(規範的主張) |
| **山の位置・音域の配分** | 直接の実証は見つからなかった。Cooper(1990)の本文にある可能性が高いが未確認。Glaux の別調査(cognition.md)の旋律の弧・山の研究を参照 | 未確認 |

---

## 6. 人の改稿のループを道具で支えた作曲支援の研究

### 6.1 Cococo(Louie ほか、CHI 2020)[一次・本文]
- 出典: Ryan Louie, Andy Coenen, Cheng-Zhi Anna Huang, Michael Terry, Carrie J. Cai, "Novice-AI Music Co-Creation via AI-Steering Tools for Deep Generative Models", CHI 2020. doi:10.1145/3313831.3376739(著者公開 PDF https://youralien.github.io/files/cococo_chi2020_copy.pdf)
- 道具: Coconet(4 声の補完モデル)の上に、(1) **声部レーン**(どの声部・どの小節を AI に生成させるかを先に指定)、(2) 例に**似せる/変える**スライダー、(3) 意味のスライダー(明るい/暗い、ありきたり/意外)、(4) **複数の案**(既定 **3 案**。サムネイルで聴き比べ、生成の前の状態も「原案」として常に比べられる。どれも採らない選択もできる)、(5) 矩形で選んだ範囲の**再補完(infill)**。
- 知見(初心者 21 人、従来の画面と比較):
  - 従来の画面では AI が**一度に全部**を自動補完し、利用者は再補完を繰り返して「**彫る(sculpting)**」しかなく、生成量に**圧倒された**。
  - Cococo では**少しずつ**作った。多いのは**声部ごと**、次に**時間の塊ごと**、その組み合わせ。ある参加者はこれを「**チェックポイント**」と呼び、先に進む前に止めて評価した(「AI が作った後に割り込み、途中で止め、先に進む前に感じを変えられる」)。
  - 生成 → **試聴** → 編集・誘導の周期。「複数の案」は「生成して聴き比べる」作曲法に自然に合った。21 人中 12 人は最初の生成の前にスライダーの値を変え、方向を先に与えた。
  - **少しずつ組み立てると、後で問題の箇所の「原因」を見つけやすい**。複数の声部を同時に生成すると「どの変更が何を起こしたか解きほぐせない」。
  - 一方で、AI の「全体最適」を信じて**局所の手直しをためらう**人もいた(「局所を変えると全体が台無しになるのでは」)。
  - 質問紙: 音楽の学び(平均 4.9 対 3.8、p=0.0003)、没入(6.0 対 4.4、p=0.0001)で Cococo が有意に高い。労力は差なし。
- [推測] AI 自身が作曲するときにも、「一度に全部 → 部分の再生成で彫る」より「**少しずつ作り、チェックポイントで止めて評価、原案を残して 3 案程度を比べる**」ほうが、原因の追跡と全体との整合に有利。

### 6.2 AI Song Contest(Huang ほか、ISMIR 2020)[一次・本文]
- 出典: Cheng-Zhi Anna Huang, Hendrik Vincent Koops, Ed Newton-Rex, Monica Dinculescu, Carrie J. Cai, "AI Song Contest: Human-AI Co-Creation in Songwriting", ISMIR 2020. https://arxiv.org/abs/2010.05388
- 知見(13 チーム・61 人):
  - 大きな一括のモデルは意味のある音楽の構造に分解できないので、多くのチームは**曲の構成要素(歌詞・旋律・和声・ドラム)ごとの小さなモデル**を使い、後で組み合わせた。しかし小さなモデルは曲全体の文脈を考慮できない。
  - **生成して選ぶ**: 旋律とベースを 450 本以上生成したチーム、デスメタルの歌詞を 1 万行生成したチーム。選別は手作業、または自動で絞ってから手作業。「難しいパズルのようで骨が折れる」。
  - **全体の骨格を先に作る**: 区間(ヴァース・サビ)ごとに合う和声進行を先に選び、和声を条件に旋律とベースを生成した。
  - **区間の対比**: 対比を直接指定する手段が無いため、ヴァースを起点に続きを複数生成してサビを選んだ、または区間ごとに温度を変えた。
  - **書き直しで変化を作る**: 旋律を伴奏を条件に再生成、伴奏を旋律を条件に再生成、を交互に繰り返してサビの「暗い版」を作った。最初は「**同じ区間を 2 回繰り返した**」が「**退屈だった**」ので、2 回目の終わりを和声付けし直して変えた。
  - 区間ごとの小さな VAE の潜在空間を「最短経路」でつなぎ、区間どうしに共通要素を持たせつつ違いも出した。
  - 設計への示唆: 旋律のモデルは「**ヴァースかサビか**」「**次の区間と対比させたいか**」を利用者が指定できるべき。モデルの中間表現は音楽家が扱う単位(動機・ヴァース・サビ・コーダ、リズム・音高)に合わせるべき。

### 6.3 その他の道具 [一次・要旨/本人記事]
- **Hyperscore**(Farbood, Pasztor & Jennings, *IEEE Computer Graphics and Applications* 2004, doi:10.1109/MCG.2004.1255809): 音楽の訓練の無い人(特に子ども)が**手描きの線で曲の全体をスケッチ**し、動機をその線に沿って配置する。全体の形 → 素材の割り当て、というトップダウンの入力。
- **JamSketch**(北原ほか、NIME 2017 / ISMIR 2018 LBD): 利用者が**旋律の概形(輪郭の曲線)**を描くと、遺伝的アルゴリズムがそれに沿う旋律を実時間で生成する。https://www.nime.org/proceedings/2017/nime2017_paper0101.pdf
- **Impromptu / Tuneblocks**(Bamberger、MIT): 旋律の動機を再生できる「曲のブロック」にして、学習者が並べ替え・聴き比べながら旋律を組み立てる。ブロックは「**知覚の単位であり作業の単位**」。組み立てることが「構成的な分析」になる。https://web.mit.edu/fnl/vol/171/bamberger.htm
- 部分の再生成(infilling)系の生成モデル(Anticipatory Music Transformer、MMM、Music SketchNet など)は、別の調査で扱われているので本稿では省く。

---

## 7. AI の作曲の手順に移せる、粒度を上下する改稿のループの形 [推測(上の知見の当てはめ)]

### 7.1 粒度(水準)の定義と、各水準で見るもの・直すもの

| 水準 | 単位 | 何を見るか(評価) | 何を直すか(操作) | 根拠 |
|---|---|---|---|---|
| **L0 曲全体** | 全区間 | 区間の並び、**起伏の曲線(密度・音域・強さの時間変化)**、山(最高音・最大密度)の位置と回数、**区間ごとの音域帯の配分**、同じ区間の 2 回目の変化 | 区間の追加・削除・統合・並べ替え、区間ごとの音域帯・密度・リズムの型の割り当て直し | ヒンデミット、Donin、《英雄》、AI Song Contest |
| **L1 区間** | 8〜16 小節 | 前後の区間との**対比の軸**(音域・音価/密度・句の長さ・句の始まりの位置)、区間の最初の行(提示)と最後の行(逸脱)、区間の頭と終わりのつなぎ | 区間全体のリズムの型・音域帯の変更、最後の句の書き直し、境界をまたぐ音(先取り・引き延ばし) | Stolpe、Kachulis、DeSantis、AI Song Contest |
| **L2 句** | 2〜4 小節 | 句の長さと息継ぎ、句の中の山、終止の強さ、**続きの自然さ**、同じ音価・同じ組の連続 | 句の後半の再生成(前後の句を文脈にして)、終止音の変更、句の長さの伸縮 | 「歓喜の歌」の改稿、Kachulis |
| **L3 動機・小節** | 1〜2 小節 | リズムの細胞、音程、和音との関係 | 音価・音高の小さな変更 | Nash(熟達者は小さな編集+短い試聴) |

### 7.2 ループの手順

1. **全体の見通しを先に仮で書く(L0)**。動機より先に、曲全体の設計図を 1 枚にする: 区間の並び、各区間の役割、**音域帯**(例: ヴァースは低め、サビで上げる)、**密度とリズムの型**、山を置く区間と小節、同じ区間の 2 回目をどう変えるか。これは「決定」ではなく「状況の準備」で、後で書き換えてよい(Donin & Theureau、Wiggins の最初の全体)。
2. **通しの下書きを一気に作る(L0→L1)**。モーツァルトの草稿総譜・ベートーヴェンの通しの下書き・DeSantis の「全体を埋める」のように、**全区間を粗く最後まで埋める**。この段階では細部を磨かない。区間ごとに生成する場合も、**前の区間の終わりと次の区間の役割を文脈として渡す**(AI Song Contest の「小さなモデルは文脈を知らない」への対策)。
3. **通しで評価して問題を列挙する(L0)**。「悪い所を聴き分けるほうが易しい」(DeSantis)ので、通しの下書きに対して問題のリストを作る。Collins の「問題の増殖」のとおり、問題は一度に全部見えなくてよい。見えた分だけ書き出す。
4. **問題を水準に振り分け、上の水準から直す**。1 周では 1 つの水準だけを扱い、その周の判断基準で評価する(Sommers の熟練者:「周回ごとに関心を層に分け、別の基準を使う」)。例: 1 周目は L0(区間の対比と山の位置)だけ、2 周目は L1(区間の最初と最後の行、つなぎ)、3 周目は L2(句の続きと終止)。**点検の点数を毎周同じチェックリストで出すだけの改稿は、Sommers の学生型**になる。
5. **局所を直すときは、必ず前後の文脈ごと直す**。直す範囲だけを孤立して再生成せず、前後 1 句(または前後の区間の端)を条件として渡す。Cococo の「原案」と同様、**直す前の版を残し、2〜3 案を作って比べる**。
6. **直したら、すぐ狭く評価し、その後に広げて評価する**。Nash の熟達者のように「小さく変えて短く聴く」を繰り返し(局所の評価)、区切りごとに DeSantis の「ループを広げる」ように区間 → 曲全体の通しで評価し直す。
7. **上の水準に戻る条件**(ここが「局所の微調整が全体から浮く」への対策の要):
   - 局所の変更が**区間の境界**(区間の最初・最後の句)に触れた → L1 で前後の区間とのつながりを評価し直す。
   - 局所の変更で**全体の指標**(起伏の曲線、区間ごとの音域帯、山の回数と位置、区間の対比)が設計図から外れた → L0 に戻る。
   - **同じ箇所の修正が 2〜3 回続いても良くならない** → 問題は上の水準にある(句の長さ・区間の役割・音域帯の割り当て)と見て 1 つ上に戻る。Collins の「問題の再構造化」・Reitman の「問題を作り変える」。
   - 局所の作業中に**良い着想が出た**(例: 予定外の良いリズムの型) → 全体の設計図のほうを書き換え、その着想を他の区間に波及させるか判断する(Donin の Leroux、Collins の洞察)。
8. **保留を許す**。すぐ解けない問題は「後で直す」印を付けてリストに残し、先に進む(Bennett の「fix that bit later」、Collins の「解の先送り」)。
9. **距離を置いた評価を最後に入れる**。人は数日後に聴き返して似通いや形式の癖に気づく(DeSantis)。AI では、作業中の文脈を持たない**別の評価役**(新しい文脈の批評)に曲全体を通しで評価させるのが相当する。問題を見つける思考と解を探す思考は別(Burnard & Younker)。
10. **終了条件は「欠点が無い」ではなく、全体の設計図が満たされ、直近の周で上の水準の問題が出なくなったこと**。

### 7.3 現状の Glaux の手順との差(作曲者の指摘との対応)

| 作曲者の指摘 | 人の過程で対応するもの | ループのどこで扱うか |
|---|---|---|
| 全体のリズムの輪郭が変わらない | 教材はリズム(音価・句の長さ・句の始まりと終わり)で区間を対比させる。「歓喜の歌」は単調なリズムを退けた | 手順 1 の設計図に「区間ごとのリズムの型・密度」を入れ、手順 4 の L0/L1 の周で区間の対比を評価 |
| 局所の微調整が全体から浮く | 初心者は和音ごと・語ごとに直し全体の形を見ない(Colley、Sommers)。熟達者は全体の見通しで細部を決める(ヒンデミット) | 手順 5(文脈ごと直す)と手順 7(上に戻る条件) |
| 前後の文脈を流れるように波に乗らせる | 通しの下書きを何度も書き直す(ベートーヴェン)。区間の境界をぼかす(DeSantis)。縫い目が見えるのは大作曲家にも弱点(チャイコフスキー) | 手順 2(全区間を通して先に作る)と L1 のつなぎの評価 |
| 音域が一定で特徴が無い | 「次の区間で旋律を持ち上げる」(Stolpe)。区間ごとの音域帯の割り当て | 手順 1 の設計図の音域帯、L0 の評価項目 |
| 1 回きりの生成 | 人は「萌芽 → スケッチ → 第 1 稿 → 精緻化」(S. Bennett)を、直線と再帰の両方で進む(Collins) | ループ全体 |

---

## 付録: 出典一覧

### 一次(本文を確認)
- Nash, C. & Blackwell, A. (2012). Liveness and Flow in Notation Use. NIME 2012. https://www.nime.org/proceedings/2012/nime2012_217.pdf
- Donin, N. & Theureau, J. (2007). Theoretical and methodological issues related to long term creative cognition: the case of musical composition. *Cognition, Technology & Work* 9: 233–251. https://yannickprie.net/archives/ENACTION-SCHOOLS/docs/documents2009/donin-theureau.pdf
- Bennett, J. (2011). Collaborative songwriting – the ontology of negotiated creativity in popular music studio practice. *Journal on the Art of Record Production* 5. https://www.arpjournal.com/asarpwp/collaborative-songwriting-–-the-ontology-of-negotiated-creativity-in-popular-music-studio-practice/
- Hindemith, P. (1952). *A Composer's World*. Harvard UP, pp.60–62. https://archive.org/details/composersworldha002179mbp
- DeSantis, D. (2015). *Making Music: 74 Creative Strategies for Electronic Music Producers*. Ableton. https://cdn-resources.ableton.com/resources/uploads/makingmusic/MakingMusic_DennisDeSantis.pdf
- Louie, R., Coenen, A., Huang, C.-Z. A., Terry, M., Cai, C. J. (2020). Novice-AI Music Co-Creation via AI-Steering Tools for Deep Generative Models. CHI 2020. https://youralien.github.io/files/cococo_chi2020_copy.pdf
- Huang, C.-Z. A., Koops, H. V., Newton-Rex, E., Dinculescu, M., Cai, C. J. (2020). AI Song Contest: Human-AI Co-Creation in Songwriting. ISMIR 2020. https://arxiv.org/abs/2010.05388
- Sommers, N. (1978/1980). Revision Strategies of Student Writers and Experienced Adult Writers(ERIC ED220839). https://files.eric.ed.gov/fulltext/ED220839.pdf
- Stolpe, A. Melody Makes All the Difference / A Process for Your Songwriting. https://www.andreastolpe.com/articles/
- Bamberger, J. MIT Faculty Newsletter(Impromptu / Tuneblocks). https://web.mit.edu/fnl/vol/171/bamberger.htm
- Tchaikovsky, P. I. フォン・メック夫人宛書簡 1878-02-17/03-01(Newmarch 訳)。http://www.tchaikovsky-research.net/en/forum/forum0223.html

### 一次(要旨を確認)
- Collins, D. (2005). *Psychology of Music* 33(2): 193–216. doi:10.1177/0305735605050651
- Collins, D. (2007). *Digital Creativity* 18(4): 239–256. doi:10.1080/14626260701743234
- Collins, D. & Dunn, M. (2011). *Journal of Music, Technology & Education* 4(1): 47–76. doi:10.1386/jmte.4.1.47_1
- Bennett, S. (1976). *JRME* 24(1): 3–13. doi:10.2307/3345061
- Kratus, J. (1989). *JRME* 37(1): 5–20. doi:10.2307/3344949 / (1994) *JRME* 42(2): 115–130. doi:10.2307/3345496 / (2001) *JRME* 49(4): 294–306. doi:10.2307/3345613
- Colley, A., Banton, L., Down, J., Pither, A. (1992). *Psychology of Music* 20(2): 124–137. doi:10.1177/0305735692202003
- Wiggins, J. (1994). *JRME* 42(3): 232–252. doi:10.2307/3345702
- Folkestad, G., Hargreaves, D. J., Lindström, B. (1998). *BJME* 15(1): 83–97. doi:10.1017/S0265051700003788
- Burnard, P. & Younker, B. A. (2004). *IJME* 22(1): 59–76. doi:10.1177/0255761404042375
- Kennedy, M. A. (2002). *JRME* 50(2): 94–110. doi:10.2307/3345815
- McAdams, S. (2004). *Music Perception* 21(3): 391–429. doi:10.1525/mp.2004.21.3.391
- Barrett, M. S. (2006). *Psychology of Music* 34(2): 195–218. doi:10.1177/0305735606061852
- Pohjannoro, U. (2016). *Musicae Scientiae*. doi:10.1177/1029864915625727
- Duignan, M., Noble, J., Biddle, R. (2010). *Computer Music Journal* 34(4): 22–33. doi:10.1162/comj_a_00023
- Cooper, B. (1990). *Beethoven and the Creative Process*. Oxford. 章「The Sketching of Melody」の要旨 https://academic.oup.com/book/49144/chapter/422053324
- Farbood, M., Pasztor, E., Jennings, K. (2004). Hyperscore. *IEEE CG&A*. doi:10.1109/MCG.2004.1255809
- Kitahara, T. ほか (2017). JamSketch. NIME 2017. https://www.nime.org/proceedings/2017/nime2017_paper0101.pdf

### 二次
- Reitman (1965) の解説: Eamonn Bell, "On fugues and functionalism" (2020). https://www.eamonnbell.com/blog/2020/11/20/on-fugues-and-functionalism/
- 「歓喜の歌」のスケッチ: Jeffrey Arlo Brown (VAN Magazine), Google Arts & Culture(Robert Winter 1977 に基づく)。https://artsandculture.google.com/story/PQVxEVX-TfU7Pg / 書評 JSTOR 843524(19 の異稿)
- 《英雄》のスケッチ: Thomas Posen の学位論文紹介(Lockwood & Gosman 2013 に基づく)。https://www.thomasposen.com/scholarship/dissertation/form-function-theory-beethoven-sketches/
- モーツァルト: Wikipedia「Mozart's compositional method」(Konrad に基づく)。https://en.wikipedia.org/wiki/Mozart%27s_compositional_method
- チャイコフスキーの「縫い目」: Tchaikovsky Research フォーラム(*Tchaikovsky: A Symposium* 1945 の英訳、原書簡は未特定)。https://www.tchaikovsky-research.net/en/forum/forum0058.html
- Collins (2005) の要約(Scribd)。https://www.scribd.com/document/646823424/Collins
- Webster (2002) のモデルの段階名(ResearchGate の図の説明)。
- Fautley (2005) の段階名(ResearchGate の引用抜粋)。
- Younker & Smith (1996)(JSTOR の抜粋のみ)。
- Seabrook, "The Song Machine" (2012) の抜粋(Longreads)。https://longreads.com/2012/03/22/the-song-machine/
- Kachulis『The Songwriter's Workshop: Melody』の目次(検索結果の抜粋)。

### 確認できなかったもの(今後の課題)
- Sloboda『The Musical Mind』(1985)の作曲の章、Collins (2005) 本文の巨視・微視の具体的な粒度、Cooper (1990) 本文の旋律の改稿の分類(山の位置・音域の変更の実例があるはず)、Younker & Smith (1996) の結論、McIntyre (2008) の本文。
