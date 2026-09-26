# DAW の信号処理の調査(2026-09)

ミキシングに必要な「人間の耳への最適化」と「PC への最適化(処理負荷・遅延・安定性)」の 2 つの観点で、
DAW に関わる信号処理の定番と最新の動向(主に 2020〜2026 年)を広く調べ、Glaux の今の実装と突き合わせた。

- 調べ方: 6 つの分野に分けて文献・規格・論文・OSS を調査し(Web 検索を含む)、Glaux のコードを読んで今の実装を棚卸しした
- 数値や仕様は出典を付けた。**「未確認」「二次情報」と書いたものは一次資料で確かめていない**。採用する前に原典で確認すること
- ライセンスは 2026-09-26 時点の各リポジトリ・crates.io の表示による。使う前に LICENSE を必ず確認する
- Glaux のファイルの位置は `crates/` からの相対パス(行番号は 2026-09-26 時点)

---

## 1. 結論(先に要点)

1. **すぐ直すべき穴が 4 つある**(どれも作業は小さい)
   - Windows でオーディオスレッドの優先度が上がっていない可能性が高い(cpal の `realtime` 機能が無効。§5.1)
   - 非正規化数(denormal)の対策が x86_64 だけ。Apple Silicon などでは何もしていない(§5.2)
   - 書き出しのリミッタがサンプル値のピークしか見ておらず、True Peak を守れない(§3.1)
   - ステレオ素材・バスのパンで、端に振ると近い側が +3 dB になる(§3.6)
2. **音質で効き目が大きいのは「非線形のエイリアス対策」「EQ とコンプの作り直し」「補間の改善」**。
   distortion / amp / tape / soft_clip は `tanh` を等倍レートで掛けているだけ。EQ は RBJ の biquad で高域が縮み、
   オートメーションで係数が段差になる。コンプは線形領域のピーク検出・ハードニー(§4)
3. **耳への最適化の土台は「ラウドネスを合わせて比べる」と「心理音響に基づく数値」**。
   規格(BS.1770-5、EBU R128、AES77-2023)と配信の目標値は整理できた。Glaux の LUFS の実装は規格どおりで良い(§3.1)。
   次の一歩は、PLR/PSR、配信先ごとの正規化の予測、心理音響に基づくマスキング解析(§3.2)
4. **AI との共同作業では「LLM は音を読めるが聴けない」ことが研究で確かめられた**(§6.3)。
   Glaux の「数値の解析を AI に渡す」方針は正しい。処理の前後の差を数値で返す道具と、
   CMA-ES で参照曲・言葉に寄せる仕組み(ST-ITO / InstructFX2FX 型)が、既存の資産で作れていちばん効く(§6)
5. **研究の自動ミキシングは「つまみの値を推定する」方式に集まっている**(Diff-MST、MEGAMI、StemFX、LLM2Fx-Tools)。
   Glaux の Command と履歴(取り消せる・監査できる編集)は研究側が今求めている形をすでに持っている(§6.1, §6.4)
6. **PC への最適化は「処理しない」工夫と「優先度」から**。並列のグラフ処理は効果は大きいが作業も大きく、計測してからでよい(§5)
7. **ライセンスの落とし穴が多い**。使えないもの: Diff-MST、DeepAFx-ST、RAVE、MERT、Matchering、Rubber Band、
   chowdsp_utils の DSP 群、FFTW、KFR、IEM Plug-in Suite、SPARTA、CamillaDSP、GstPEAQ、SoX のコード(§7)
8. 取り入れの順番は §8 に 3 段階でまとめた

---

## 2. Glaux の今の実装(棚卸し)

### 良い所

- ラウドネス(統合 LUFS)は自前で規格どおり(K 特性、400ms ブロック、-70 / -10 のゲート。`glaux-engine/src/analyze.rs:494-554`)。
  LRA・True Peak・短期ラウドネスは ebur128 クレート(`analyze.rs:188-216`)
- シンセの帯域制限: subtractive の saw/square は PolyBLEP、フィルタは TPT 型 SVF、wavetable は 11 段のミップマップ、
  sampler と SoundFont は Hermite 補間
- 書き出しは 16/24bit で TPDF ディザ。ラウドネス目標と 5ms 先読みのリミッタ(`glaux-engine/src/export.rs`)
- ボイスは全体 256 + スティール用 32、リリース中で最も古いものを 3ms でフェードして奪う(`render.rs`)
- 解析が豊富: Bark 24 帯域のマスキング判定、ステレオ(相関・250Hz 以下の相関・S/M 比・左右バランス)、
  音色(YIN の音程、包絡、倍音 16 次、HNR、スペクトルの特徴)、ノートからのキー・コード
- 無音が 4 秒続いたトラックは処理を省く。プラグインの遅延補正(PDC)あり。RT スレッドでアロケーション・ロックをしない

### 弱い所(根拠のファイルと行)

| # | 弱点 | 場所 |
|---|---|---|
| 1 | 非線形(tanh)にオーバーサンプリング・ADAA がない。FM は帯域制限なし。三角波は素朴な計算 | `glaux-dsp/src/effects.rs:603, 619-621, 754`、`fm.rs:139-142`、`subtractive.rs:245` |
| 2 | パラメータの平滑化がない。オートメーションは 128 フレームごとに段差、EQ の係数もそのたびに計算し直し。フェーダー・パンは差し替えで即変化 | `render.rs:43-44, 995`、`effects.rs:1481`、`render.rs:1573-1574` |
| 3 | リバーブが Freeverb の縮小版(コム 4 + オールパス 2、モノラル入力、プリディレイなし)。バッファが 48kHz 分なので高いレートで部屋が小さくなる | `effects.rs:163-232, 593, 1636` |
| 4 | コンプが最小構成(瞬時ピーク・ハードニー・先読みなし・検出側のハイパスなし) | `effects.rs:574-591` |
| 5 | 再生時のマスターは soft_clip だけでリミッタがない。書き出しのリミッタはサンプル値のピークで判定 | `render.rs:26-35`、`export.rs:313-322` |
| 6 | EQ は RBJ の biquad(DF1、f32)で 3 バンド固定。HP/LP なし | `effects.rs:42-139` |
| 7 | ディレイ・コーラス・テープ・pluck は線形補間。ディレイはテンポ同期なし、最大遅延が約 1.36 秒(48kHz)で高いレートでは短くなる | `effects.rs:317-320, 388-395`、`pluck.rs:118-125` |
| 8 | SF2 はモノラル出力(パン・センド・CC 未対応)、ループ境界で補間が折り返さない。sampler はステレオ素材の side を捨てる | `multi.rs:12, 401-413`、`sampler.rs:138-145` |
| 9 | SIMD なし、1 サンプルずつ enum 分岐、オーディオスレッド 1 本、subtractive のフィルタで毎サンプル `tan` | `voice.rs:128-139`、`subtractive.rs:278` |
| 10 | denormal 対策(FTZ/DAZ)が x86_64 だけ | `render.rs:222-231` |
| 11 | PDC は起動時に 1 回だけ遅延を聞く(変更通知を無視)。分岐した枝の遅れを揃えない | `glaux-clap/src/plugin.rs:195-199`、`host.rs:223`、`render.rs:1751` |
| 12 | サイドチェインの検出に CLAP 音源の出力が入らない。バスをソースにできない | `render.rs:1711-1721, 1476-1481` |
| 13 | ステレオ素材のパンが端で +3 dB(√2 をパン位置に関係なく掛ける)。幅や M/S のエフェクトなし。内蔵楽器はモノラル出力 | `render.rs:1559-1562`、`voice.rs:128` |
| 14 | メーターはブロックごとのサンプルピークだけ(RMS / LUFS / True Peak のリアルタイム表示なし) | `render.rs:126-167` |
| 15 | バッファは cpal に 1024 固定を要求(48kHz で約 21ms)。利用者が選べない | `glaux-engine/src/output.rs:720-747` |
| 16 | タイムストレッチは WSOLA のみ(トランジェント保持・位相ボコーダなし)。書き出しは 44.1/48k のみ、ノイズシェーピングなし | `stretch.rs`、`export.rs:250, 444-469` |

---

## 3. 人間の耳への最適化

### 3.1 ラウドネスの規格と配信の目標値

**計算方法(BS.1770)**: K 特性(高域シェルフ + ハイパス)→ チャンネルごとの二乗平均 → 重み付き和 →
400ms ブロック(75% 重ね)を -70 LUFS の絶対ゲートと -10 LU の相対ゲートでふるう。
LRA(EBU Tech 3342)は 3 秒窓の短期値を -70 / -20 LU でふるい、10〜95 パーセンタイルの幅。
PLR = 最大 True Peak − 統合ラウドネス、PSR = True Peak − 短期ラウドネス。

**最新の動き**
- **ITU-R BS.1770-5(2023/11)**: 附属書 4(オブジェクト音声は BS.2051 の配置にレンダリングしてから測る。条件の報告が必要)が
  増えただけで、**ステレオの計算は -4 と同じ**。Glaux の実装はそのままで良い
- **EBU R128 s2 第 3 版(2023/11)**: 配信では放送の -23 LUFS をメタデータで正規化するのが基本。使えない端末では暫定で -20〜-16 LUFS、
  端末側に Tech 3344 準拠の True Peak リミッタ。アルバム単位の正規化を推奨
- **AES TD1008(2021)→ AES77-2023(正式な推奨)**

  | 対象 | 目標 |
  |---|---|
  | 音声 | -18 LUFS(ダイアログで測る) |
  | 曲単位で正規化する音楽 | -16 LUFS |
  | アルバム単位 | 最大の曲を -14 LUFS(アルバム全体で約 -16) |
  | True Peak の上限 | -1 dBTP(コーデックに入れる時点)。ビットレートが低いほど下げる |

  そのほか: 同じラウドネス値でも音声は大きく聞こえるので音楽を 2〜3 LU 高く置く。符号化直前のリミッタは 1 dB 程度までを出発点に。
  PLR の高い音源は明瞭で疲れにくい
- **配信サービス**

  | サービス | 目標 | 備考 | 確かさ |
  |---|---|---|---|
  | Spotify | -14 LUFS(Loud -11 / Quiet -19) | Loud では -1 dB のリミッタ(アタック 5ms・ディケイ 100ms)。推奨は -1 dBTP 以下、-14 より大きいマスターは -2 dBTP 以下。アルバム再生はアルバム単位 | 公式 |
  | Apple Music | -16 LUFS(Sound Check) | 持ち上げ方向の扱いは未確認 | 二次情報 |
  | YouTube | -14 LUFS | 下げるだけで持ち上げない | 二次情報(2019) |
  | Amazon / Tidal / Deezer | -14 / -14 / -15 | | 未確認 |
- **実態**: MixCheck の投稿データ(Mourgela ら、AES 157th、2024)ではマスターの約 79% が -14 より大きく、0 dBTP 付近が多い
  (アマチュア中心のデータ)。正規化があっても音圧を上げる習慣は残っている

**実装の要点**
- K 特性の係数は 48kHz 用(Glaux は解析を 48kHz でレンダーして使っている。他のレートでは係数を設計し直す)
- **True Peak は 4 倍以上のオーバーサンプリング**(48kHz で 4 倍、96kHz で 2 倍。48 タップ 4 位相の FIR 補間)。
  4 倍での見落としは最悪で約 -0.69 dB(規格の式からの計算)。リミッタの検出に使うなら 8 倍か、上限を少し下げて吸収する
- **リミッタはチェーンの最後**(サンプルレート変換や高域のフィルタの後で True Peak が上がりうるため)
- PSR の目安は実務家の経験則(Ian Shepherd: いちばん大きい所でも PSR 8 を下回らない)。規格ではない

### 3.2 等ラウドネス・臨界帯域・マスキングと「被り」の解消

- **ISO 226:2023**: 2003 年版との差は最大 0.6 dB で実用上同じ(20Hz の閾値を 0.4 dB 下げ、式の有効桁を見直し)
- 帯域の尺度: Bark(z = 13·atan(0.00076f) + 3.5·atan((f/7500)²))、ERB = 24.7·(4.37f/1000 + 1) Hz、ERB 数 = 21.4·log10(4.37f/1000 + 1)
- **同時マスキング**は高域側へ裾を引く。Schroeder の広がり関数(dB)= 15.81 + 7.5(Δz+0.474) − 17.5√(1+(Δz+0.474)²)。
  閾値の下げ幅はマスカーが純音寄りなら大きく雑音寄りなら小さい(Johnston 1988 の数値は未確認)
- **時間マスキング**: 前向き(後の音を隠す)最大 100〜200ms、後ろ向き約 5〜20ms(文献により幅。二次情報)
- **ラウドネスモデル**: ISO 532-1(Zwicker)、532-2(定常音)、**532-3:2023(時間変化と両耳の抑制を扱う。参考コード付き。利用条件は未確認)**。
  「埋もれ具合」は部分ラウドネスで数値にできる
- **被りの解消の原理**
  - マスキングを減らす自動 EQ(Hafezi & Reiss 2015、JAES): 知覚モデルでトラックごとのマスキング閾値を出し、主役の重要な帯域が
    他のトラックの閾値の下に沈む所を見つけて、隠している側を下げる。サブグループ単位の版は Ronan ら(2018)
  - スペクトル・ダッキング / ダイナミック EQ: サイドチェインの帯域ごとのエンベロープで、別のトラックの同じ帯域だけを下げる
    (FabFilter Pro-Q 4 の Spectral Dynamics は帯域内でしきい値を超えた周波数だけに効く)
  - 共鳴抑制(soothe 系、内部は非公開。公開情報からの一般原理): STFT の振幅を、なめらかにした包絡(1/3 オクターブや ERB の平滑化、
    中央値、ケプストラム包絡)と比べ、はみ出した分だけビンごとに下げ、周波数と時間の方向にならす。STFT 長ぶんの遅延が出る
  - 明瞭度の予測: 過渡・定常・残差に分けて各成分のマスキングを比べる方法で主観評価と ρ = 0.84(arXiv 2103.12152)
- Glaux は今、24 Bark 帯域 × 100ms のエネルギーでトラックを比べている(`analyze.rs:269-444`)。広がり関数・絶対閾・時間マスキング・
  トーン性を入れると、「どのトラックのどの帯域を何 dB 下げればよいか」を根拠付きで AI に渡せる

### 3.3 トーナルバランス(音色の全体の傾き)

- 市販曲の長時間平均スペクトルは **100Hz〜4kHz でおよそ -5 dB/オクターブ**で下がる(Pestana, Reiss ら 2013、1950〜2010 年の曲)。
  ジャンルの差は主に低域と明るさに出る
- iZotope Tonal Balance Control はジャンル別の目標範囲を 4 帯域で示す(曲線は独自データ)
- スペクトル重心は低域の量に大きく左右される。知覚上の明るさには Sharpness(DIN 45692)の方が近い
- 実装: 1/3 オクターブか ERB 帯域の長時間平均、「100Hz〜4kHz の傾き」と「帯域ごとの目標とのずれ」を出す。ジャンルの目標は自前で集める

### 3.4 ダイナミクスと知覚

- 音量を合わせて比べると、コンプを強く掛けた版とそうでない版の好みの差・奥行きの差は見つからなかった(Hjortkjær & Walther-Hansen、JAES 2014)
- 正規化される環境では「大きい方が有利」がなくなり、かけすぎたリミッタはピークの歪みとコーデックのはみ出しを増やすだけ(TD1008)
- **聴き比べはラウドネスを合わせて行う**(しないと「大きい方が良く聞こえる」偏りが入る)
- ラウドネスの時間的な積分は約 100〜200ms。Momentary 400ms / Short-term 3s はこれに合わせてある

### 3.5 低域

- 低い音は定位しにくい(サブウーファーのクロスオーバー 80Hz は THX 由来とされる)。帯域のある実際の音ではそれより下でも方向が分かる場合がある
- ミックスで低域をモノにする周波数 100〜150Hz は慣行で、規格はない(未確認)
- **仮想低音(ミッシングファンダメンタル)**: 倍音から基音を知覚させる。非線形素子 + 帯域通過(相互変調が出やすい)/
  位相ボコーダ(過渡がにじむ)/ 過渡と定常で使い分けるハイブリッド。音源分離と組み合わせる新しい例(Giampiccolo ら、IEEE SPL 2023)。特許は未調査

### 3.6 ステレオ・空間・モニタリング

**パンと M/S**
- パン則: -3 dB(等パワー。音場の中で音量一定)、-6 dB(直線。モノラルにしたとき一定)、-4.5 dB(折衷)
- ステレオ素材は「バランス(反対側を下げる)」か「左右を別々にパン」(Ableton Live 11 の Split Stereo Pan)。
  一般的なバランスの法則は「近い側 0 dB・遠い側を下げる」。**Glaux は端で近い側が +3 dB になっている**(`render.rs:1559-1562`)。
  直すと既存の曲の音が変わるので、プロジェクトの設定にするか判断が要る
- M = (L+R)/√2、S = (L−R)/√2。S を上げると広がるがモノラルで消える。Haas(1〜30ms の遅延)は広がるがモノラルでくし形になる
- **幅の拡張の最新**: Das(Sonos、DAFx24)は velvet noise の畳み込みかランダム位相のオールパスでデコリレーションし、
  完全再構成のクロスオーバーで低域と高域を別の幅で混ぜ、トランジェントは素通しにする(実装は CC0)。
  velvet noise の目安は長さ 15〜30ms・密度 50 パルス/ms(短いと色付き)。最適化版 OVN(Schlecht ら 2018)
- 幅を操作する前後で**相関とモノラルのラウドネス差**を必ず測る。低域のモノ化は LR4 のクロスオーバーで

**ヘッドホンでのミックス**(Glaux の利用者に多い)
- **クロスフィード(bs2b)**: 既定 700Hz / 4.5dB(方位 30°・約 3m のスピーカーに近い)。Chu Moy 700Hz / 6dB、Jan Meier 650Hz / 9.5dB。
  libbs2b は MIT(Rust 移植も MIT と表示。LICENSE 本文は未確認)
- **ヘッドホン補正**: AutoEq(コード MIT)。測定データの再配布条件は未確認 → **データは同梱せず、利用者が AutoEq 形式の PEQ を読み込む**
- ターゲット: Harman のオーバーイヤー 2018 / インイヤー 2019。2023〜2024 年に再検証(Senselab、Olive ら AES)
- **仮想スピーカー**: HRIR で ±30° のスピーカーと短い初期反射。SADIE II(Apache-2.0)を事前変換して埋め込むのが最小構成
- **落とし穴: クロスフィード・補正・仮想スピーカーは書き出しに混ぜない**(モニターの出口だけ)

**イマーシブ**(優先度は低い)
- Dolby Atmos(7.1.2 ベッド + 最大 118 オブジェクト、ADM BWF、音楽の目安 -18 LKFS / -1 dBTP は二次情報)。レンダラは非公開
- Apple: 個人化 HRTF(iOS 16、2022)、WWDC25(2025)で ASAF と配信コーデック APAC
- IAMF / Eclipsa Audio(Google・Samsung、2025、ロイヤリティフリー。libiamf は BSD-3-Clause-Clear、DAW プラグインは Apache-2.0)
- Ambisonics: AmbiX(ACN + SN3D)。IEM Plug-in Suite と SPARTA は GPL-3.0。SAF のコアは ISC(一部 GPLv2)。EBU の ADM レンダラ libear は Apache-2.0
- HRTF: SOFA 形式(AES69-2022、中身は HDF5)。SADIE II(Apache-2.0)、MIT KEMAR(引用で自由)、SONICOM(ライセンス未確認)、
  Mesh2HRTF(EUPL)。個人化 HRTF のアップサンプリングを競う LAP Challenge 2024(1 位は MERL の neural field)
- 部屋の補正: 測定は指数スイープ(Farina 2000)。低域の山だけを最小位相の IIR で削る(谷は持ち上げない)。CamillaDSP と DRC-FIR は GPL

### 3.7 メータ

- ラウドネス: モーメンタリ 400ms / ショートターム 3s / インテグレーテッド / LRA。True Peak は 4 倍オーバーサンプリング
  (ebur128 クレートは 96kHz 未満で 4 倍)。`ebur128-stream`(MIT/Apache、v0.1)は「ホットパスでアロケーションなし」をうたう
- PPM / VU(約 300ms の積分)/ K-System(K-20/14/12)は、LUFS の普及で役割が薄れた(Katz 本人も LUFS + PLR に移行)。表示モードの一つ程度で十分
- スペクトラムアナライザ: 48kHz で 4096〜16384 点、Hann か Blackman-Harris、重なり 50〜75%。1/3〜1/24 オクターブの平滑化、指数平均と
  ピークホールド、傾き補正(ピンクノイズが水平に見える 3〜4.5 dB/oct)。低域の分解能には帯域ごとの FFT 長か CQT / NSGT。
  再割り当て法は持続する倍音を鋭く表示(FFT 3 本)
- 相関 = ΣLR/√(ΣL²ΣR²)(表示は数百 ms の指数平均が慣習)。ゴニオメータは (S, M) を 45° 回して描く
- ダイナミクス: クレストファクター、PLR、PSR
- **A/B 比較は必ずラウドネスをそろえる**。確認用のモニター切り替え: モノラル、S だけ、小型スピーカー(150Hz〜8kHz 程度に絞ってモノラル。
  数値は設計値で、実測データは使わない)、低音量

### 3.8 ディザとノイズシェーピング

- TPDF(±1 LSB)で量子化歪みとノイズの揺れを消す(Glaux は実装済み)
- ノイズシェーピング: Wannamaker の F 特性(15 phon の等ラウドネス由来)の 9 次で、聴感補正した SN 比が +17 dB(約 3 bit)、補正なしの雑音は +18 dB
- 係数は 44.1kHz 用が多い(SoX の表で 48k 版があるのは Gesemann と Shibata だけ。**SoX は LGPL なので係数は論文から自分で設計**)
- ディザはリミッタの後・チェーンの最後。24bit と float ではシェーピング不要。意味があるのは 16bit のときだけ

### 3.9 知覚品質の客観評価

| 手法 | 概要 | ライセンス |
|---|---|---|
| PEAQ(ITU-R BS.1387) | 参照と劣化音の比較。コーデック向け | GstPEAQ は LGPL-2 |
| ViSQOL v3 | ガンマトーン + NSIM、48kHz の音楽モード | Apache-2.0(C++) |
| PEMO-Q | | 商用 |
| audiobox-aesthetics(Meta、2025) | 参照なしで制作品質(PQ)・複雑さ・楽しさ・有用性を予測 | CC-BY 4.0(一部 MIT)。重みの条件は要確認 |
| SongEval(2025) | 2,399 曲・専門家 16 人・5 観点 | CC BY 4.0 |

- ミックスには唯一の正解がないので、参照と比べる PEAQ / ViSQOL は **DSP の回帰テスト**向き。ミックスの良し悪しには参照なしの評価か埋め込みの距離
- 評価器がポップを過大評価するジャンルの偏りが指摘されている(ISMIR 2026)。**判断の根拠にはせず目安として**使う

### 3.10 聴覚疲労とモニター音量

- K-System: ピンクノイズ -20 dBFS RMS を 1 本あたり 83 dB SPL(C 特性)に校正
- ITU-T H.870 V2(2022、WHO と共同): 成人の週あたり 1.6 Pa²h(80 dBA を週 40 時間)。より慎重には 75 dBA 相当
- SPL の校正がないと被ばく量は推定できない。DAW ができるのは校正信号・休憩の促し・音量をそろえた比較まで

---

## 4. 要素技術(音質)

### 4.1 フィルタ・EQ

- **定番**: TPT/ZDF(Zavalishin "The Art of VA Filter Design" rev. 2.1.2、2020)。**Cytomic(Simper)の線形台形 SVF**は
  係数をオーディオレートで動かしても安定し、1 つの構造で LP / HP / BP / ノッチ / ピーク / シェルフを出せる
- **RBJ の双一次変換はナイキスト付近で縮む(cramping)**。補正は Orfanidis(JAES 1997、ナイキストでのゲインを指定)か
  **Vicanek "Matched Second Order Digital Filters"(2016)と 2 極シェルフ**(閉形式、負荷は RBJ と同程度)。Vicanek "Fast-Settling Filters"(2025)
- 微分可能な全極 IIR(Yu ら、DAFx24)で時変フィルタ・コンプを学習で当てはめる研究
- 線形位相 EQ は FFT 畳み込みで数十 ms の遅延 → PDC が前提。dynamic EQ は「SVF のピークのゲインをサイドチェインのエンベロープで動かす」が
  作りやすい(係数を毎サンプル変えるので SVF 必須)。チルトは同じ中心の低域シェルフと高域シェルフを逆ゲインで
- 数値: biquad は f32 だと低い周波数・高いレートで係数の精度が落ちる。SVF の方が有利。状態と累積は f64、バッファは f32 が妥当

### 4.2 非線形とエイリアス(折り返し)

- **オーバーサンプリング**: 2 倍ずつの多相ハーフバンドを段数分。IIR(HIIR 方式、2 経路のオールパス。WTFPL)は軽いが位相が回る。FIR は直線位相だが重い
- **ADAA(antiderivative anti-aliasing)**: 1 次(Parker ら 2016)、2 次・3 次(Bilbao・Esqueda・Parker、IEEE SPL 2017)。
  オーバーサンプリングより軽い。1 次の tanh は原始関数 log cosh を使い、x[n] ≈ x[n−1] のときだけ中点で評価(0 除算の回避)。半サンプルの遅延が入る
- 最新: 状態を持つ系への ADAA(Holters、DAFx19)、WDF の非線形素子への ADAA(DAFx20)、ADAA 用の補間フィルタ(Zheleznov & Bilbao、DAFx24)、
  ニューラル歪みの折り返し対策(DAFx25。活性化のなめらか化・教師生徒・LSTM 内のオーバーサンプリング)
- **BLEP 系**: minBLEP / polyBLEP(段差)、polyBLAMP(角。最大 50 dB 下げると報告、Esqueda ら DAFx16)
- 多段で段間にフィルタがある構成(Glaux の amp)は、段ごとの ADAA より全体のオーバーサンプリングが素直
- テープ・真空管は、飽和・ヒステリシス(Jiles-Atherton)・ワウフラッター・ヘッドの特性を分けて作るのが一般的(ヒステリシスの詳細は未確認)

### 4.3 仮想アナログとニューラルモデル

- WDF(Wave Digital Filter): chowdsp_wdf(**BSD-3**、arXiv 2210.12554)。状態空間モデル(DK 法)
- ニューラルのアンプ・ペダル: Wright ら(2020、WaveNet 型と RNN 型、学習データは 3 分で足りる)。**NAM**(WaveNet A1/A2・LSTM、
  Slimmable NAM 2025 は実行時に計算量を変えられる)。NAM Core は MIT
- Rust: NeuralAmpModeler-rs(Apache-2.0)は AVX2 必須(ARM の Mac で動かない懸念)、nam-rs(MIT)はアロケーションせずに動くとする。
  `.nam` は JSON で `sample_rate` を持つ(レートが違えば変換が要る)
- GuitarML Proteus は GPL(避ける)

### 4.4 ダイナミクス

- **Giannoulis・Massberg・Reiss(JAES 2012)**: フィードフォワード、ゲインの計算を log 領域で、なめらかなピーク検出。ソフトニーは二次式の補間
- プログラム依存リリース(2 段のリリース、検出量で時定数を変える。出典は未確認)。マルチバンドは LR4 のクロスオーバー(3 帯域以上はオールパス補償)
- **リミッタ**: ルックアヘッド + ピークホールド + FIR(ボックスフィルタの多段)でゲインをならす(Signalsmith 2022、MIT)。True Peak は BS.1770-5 のとおり 4 倍以上
- ゲート・エキスパンダはヒステリシスとホールド必須。トランジェントシェイパーは速い包絡と遅い包絡の差でゲインを作る(学術的な出典は未確認)
- Glaux のコンプは左右の最大値を線形領域で追っているので、アタック・リリースの効き方が音量で変わり、ニーもない

### 4.5 リバーブ

- **FDN**(Jot 1991): 直交行列(Householder / Hadamard)とディレイ線ごとの減衰フィルタで帯域ごとの T60 を制御。Dattorro のプレート(1997)。
  総説は Välimäki ら "Fifty Years of Artificial Reverberation"(IEEE TASLP 2012)
- 最新: 微分可能 FDN で色付きを抑える(Dal Santo ら DAFx23)、小規模 FDN の最適化(2024〜2025)、RIR2FDN(DAFx24)、減衰フィルタの微分可能設計(2025)、
  dark velvet noise で非指数減衰(Fagerström ら 2024)とそのバイノーラル版(DAFx24)
- 実装の手引き: Signalsmith "Let's Write A Reverb"(ADC21、コード MIT): 拡散段 → FDN → Householder。ディレイ長は互いに素に近く、ゆっくり変調すると金属的な響きが減る
- 畳み込み: 一様 / 非一様パーティション(Gardner 1995、Wefers 2015)。Rust は fft-convolver(MIT、2 段版あり、処理中アロケーションなし)
- 品質の指標: エコー密度、スペクトルの平坦さ、T60 の精度、色付き

### 4.6 ディレイ・モジュレーション・ピッチ / ストレッチ

- 分数遅延: 変調するディレイは 3〜4 点の Lagrange / Hermite。フィードバック内の固定遅延は Thiran のオールパス(変調時は過渡ノイズ)。
  高品質は windowed sinc。Signalsmith dsp(MIT)に Lagrange・多相・Kaiser-sinc の補間
- フェイザーは 1 次オールパスの多段を LFO で(TPT 形なら変調に強い)
- ピッチ・ストレッチ: PSOLA / WSOLA、位相ボコーダ(位相ロック)、混合。**"Phase Vocoder Done Right"(PGHI、Průša & Holighaus 2017)**は
  ピーク検出も過渡検出も要らない位相補正。Signalsmith Stretch(MIT)は大きなピッチ変更に強く、時間の伸縮は 0.75〜1.5 倍が最適。
  Bungee は MPL-2.0、**Rubber Band は GPL**(避ける)

### 4.7 サンプルレート変換

- windowed sinc の多相。r8brain(MIT)、libsamplerate(2016 年に BSD-2 へ)。**Rust は rubato(MIT/Apache、v5.0.0)**:
  非同期 sinc(比率を実行中に変えられる、SIMD 対応)・多項式・FFT 同期、処理中アロケーションなし

### 4.8 シンセ側

- 三角波とハードシンクに polyBLAMP、サイン波のハードシンクの折り返し対策(DAFx22)
- 高次の積分型ウェーブテーブル(Franck & Välimäki、JAES 2013)
- FM は変調指数と最高倍音から帯域がナイキストを超えないよう抑える(DAFx20 "Practical Linear and Exponential FM")。フィードバック FM はオーバーサンプリングが現実的

---

## 5. PC への最適化(負荷・遅延・安定性)

### 5.1 スレッドの優先度(すぐ効く)

- **cpal の `realtime` 機能が無効**で、cpal の WASAPI のソースには機能なしの場合 `TODO: Actually set the thread priority` とある。
  → Windows でオーディオスレッドが MMCSS の「Pro Audio」に上がっていない可能性が高い。中で使う `audio_thread_priority` は MPL-2.0(使うだけなら問題なし)
- Windows: MMCSS(`AvSetMmThreadCharacteristics("Pro Audio")`、1 プロセス 32 本まで)。ハイブリッド CPU(E コア・コアパーキング・EcoQoS)は
  ドロップアウトの原因になる → `SetProcessInformation(ProcessPowerThrottling)` で抑制から外す(Steinberg の案内)
- macOS: `os_workgroup`(WWDC20)。自前のリアルタイムスレッドは `os_workgroup_join` でデバイスのワークグループに参加(途中で変わりうる)

### 5.2 非正規化数(denormal)

- aarch64 は FPCR の FZ(ビット 24)がスレッドごとで、子スレッドに受け継がれない。Mixxx #16126 で実害(大音量のデジタルノイズ)の報告。
  `no_denormals` 0.3(MIT)は x86 と aarch64 の両方に対応。Rust で FP 環境を書き換えるのは言語上グレー(標準化の提案 libs-team ACP #877)

### 5.3 バッファと I/O

- Glaux は 1024 固定(48kHz で約 21ms)。**選べるようにし、実際の `buffer_size()` と xrun を表示**する
- cpal 0.18.0(2026-06): PipeWire 直結・`realtime` 機能・ハードウェア遅延込みのタイムスタンプ。0.18.2(2026-08): 各バックエンドで xrun を報告。
  WASAPI 排他は PR #1368(出力のみ・オプトイン)が進行中で v0.19 の目標
- WASAPI 共有でも Windows 10 以降は `IAudioClient3` で 10ms 未満の周期(ドライバ次第)
- **ASIO は 2025-10 に SDK が「GPLv3 または独自ライセンス」**。Glaux(MIT/Apache)は GPL 側を選べないので独自ライセンスで扱うことになる
- Linux: PipeWire の quantum、`pw-top` の ERR 列で xrun を監視

### 5.4 「処理しない」工夫

- **CLAP のプロセスステータス**: `SLEEP`(次のイベント・入力の変化まで不要)、`CONTINUE_IF_NOT_QUIET`、`TAIL`(tail 拡張のテール長で判断)。
  **`constant_mask`**(一定値・無音のチャンネルのビット)をホストが入力に付ければ、プラグインが処理を省ける。
  今の「無音が 4 秒続いたら止める」固定判定より正確で積極的に省ける(clack-extensions の `tail` 機能を有効にする)
- 無音フラグ(silence mask)で無音入力のノードを丸ごと省く(Rust の Firewheel、MIT/Apache)
- **パラメータの平滑化**: 係数はブロックの頭で計算し、ブロック内は線形補間が定番。一次ローパスで平滑化するなら係数をサンプルレートに合わせる
- オーバーサンプリングは歪み系だけに

### 5.5 SIMD とブロック処理

- まず**データの並べ方**(ブロック処理、チャンネル・ボイスをまとめる)を整える方が効く。Glaux の effects の `process` は 1 サンプルずつ `(l, r)` を返す形で、
  自動ベクトル化が効きにくい → ブロック単位の API に
- `std::simd` は 2026 年も nightly 限定。安定版では `wide`(Zlib/Apache/MIT)、`pulp`(MIT、実行時の切り替え内蔵)、`fearless_simd`(1.0 が 2026-09-21)、
  `multiversion`、`macerator`。Rust 1.86(2025-04)で安全な関数にも `#[target_feature]`
- FMA は結果の数値を変える(テストの比較値に影響)

### 5.6 並列のグラフ処理(効果は大きいが作業も大きい)

- 主要な DAW は「依存関係つきのグラフを、準備のできたノードからワーカーに配る」方式
  - Ardour(`libs/ardour/graph.cc`): 入力のないノードを最初のキューへ、終わったノードが下流の参照カウントを減らし 0 のものをキューへ。セマフォでワーカーを起こす
  - Tracktion Graph(ロックなし、v3 で macOS の Audio Workgroup に参加)、Ableton(最大 64 スレッド、**最長経路が限界**)、Bitwig(トラック単位)
  - REAPER の Anticipative FX: 空いた CPU で FX を少し先まで順不同に処理。入力モニタリングは同期処理の方が向く(Sound On Sound 2011)
- 実装: 実行順は UI スレッドで作って渡す。ワーカーはスリープ + スピン、全ワーカーをワークグループ / MMCSS に登録。**トラックが少ないと同期のコストの方が大きい**ので直列の経路も残す。
  CLAP の `thread-pool` 拡張(プラグインのボイス処理をホストのプールで。仕様書自身が RT の規則を破りうると注意)
- Glaux は分岐・合流のエフェクトグラフを持ったので、トラック内の枝も並列の単位になりうる。**まず計測してから**

### 5.7 ロックフリーとメモリ

- SPSC: `rtrb` 0.4(MIT/Apache)、`ringbuf` 0.5。最新値の受け渡し: `triple_buffer` 9.0(MPL-2.0)、SeqLock(ADC 2024)
- **解放の後回し**: `basedrop`(MIT/Apache)。Glaux は古い再生データを「墓場」に 8 件まで残す(`output.rs`)が、件数で判断しているだけなので、
  `arc_swap` の Guard を持っている間に差し替えが続くと最後の参照がオーディオスレッドに残りうる → `Arc::strong_count == 1` を確かめて UI 側で解放
- 事前確保と、「トラック × チャンネル × ブロック」の平らなバッファの使い回し

### 5.8 FFT とたたみ込み

- `rustfft` 6.4(AVX / SSE4.1 / NEON を実行時選択)、`realfft` 3.5(MIT)。計画とスクラッチは事前に作り、RT では `process_with_scratch` 系だけ
- FFTW(GPL)、KFR(GPL/商用)は避ける。PFFFT・pocketfft は BSD 系
- 畳み込みは `fft-convolver`(MIT)。長い IR の後ろ側を別スレッドで計算するのは並列化の後で

### 5.9 計測

- 負荷は「ブロックの処理時間 ÷ 締め切り」の平均と最大。Glaux は平均・最大・超過回数を取っている → **トラック別・プラグイン別**に広げる
- **RealtimeSanitizer(RTSan)**(LLVM 20〜): RT 関数の中の malloc・ロックを検出。Rust は `rtsan-standalone`(Apache-2.0、Linux/macOS)→ CI に
- Tracy(`tracy-client`、機能フラグで切り替え)、Superluminal、perf。ベンチは `criterion` か `divan`(条件を固定)

### 5.10 数値の精度

- 位相の累積は f32 だと長時間でずれる → f64 か [0,1) に折り返すか整数の位相アキュムレータ
- 状態と累積は f64、バッファは f32

### 5.11 GPU と推論

- GPU Audio(SDK 無償公開 2025-03、「96 サンプルで動く」と主張)。Rust から使う手段はない → 様子見
- 推論は**オーディオスレッドの外**で。ANIRA(2025、arXiv)は専用スレッドプール + 遅延の申告(PDC に乗せる)+ 最初の空打ち。
  Rust: `tract`(Glaux が使用中)、`ort`(まだ RC)、`candle`

---

## 6. AI・機械学習

### 6.1 自動ミキシング・スタイル転写

- 系譜: 微分可能なミキシングコンソール(Steinmetz ら、ICASSP 2021)→ FxNorm-Automix(Sony、ISMIR 2022、コード MIT)→
  **Diff-MST(ISMIR 2024、コードは CC-BY-NC-SA で使えない)**→ MEGAMI(ICASSP 2026、プロのミックスの「分布」を拡散モデルで)→
  StemFX(2026-07、ステムごとに可変長の FX チェーンを予測、反復最適化の 4000 倍以上速い)→ 逐次ステムブレンド(2026-08)→
  Diff2Mix(ISMIR 2026)。ライブ向けの AiLive Mixer(ICASSP 2026、遅延ゼロ、今はゲインだけ)
- **共通点は「解釈できて後から人が直せるつまみの値を出す」**。Glaux の Command と内蔵 eq / compressor / reverb にそのまま写せる
- 評価は SI-SDR のようなエネルギー系より、CLAP 埋め込みの分布距離(FAD / KAD)へ
- 公開されているマルチトラックのデータセットの多くは非商用(MUSDB18 は学術のみ、MedleyDB の一部は CC BY-NC-SA)
- **知識ベースの経験則**(De Man & Reiss 2013、Pestana & Reiss 2014、"Ten Years of Automatic Mixing" 2017): 各トラックのラウドネスを揃えてから主役を持ち上げる、
  低い帯域ほど中央・高い帯域ほど広げる、被りは EQ で、コンプの量はクレストファクターと役割から。主役ボーカルの好ましい音量は聴き手で 2 群に分かれる
- 2 段階(グループ内のバランス → グループ間)に分けると有効だが、グループ分けを誤ると悪くなる(Reiss ら 2026)

### 6.2 微分可能 DSP と効果の推定

- ライブラリ: dasp-pytorch(Apache-2.0)、GRAFX(DAFx24、Apache-2.0、大きな処理グラフを GPU で並列最適化)、NablAFx(2025)
- **ST-ITO(ISMIR 2024 最優秀論文、コード Apache-2.0)**: 自己教師ありの「制作スタイルの埋め込み」+ **微分を使わない最適化**で、
  微分できない任意の効果チェーンを制御。Glaux の CMA-ES による音色合わせ(match_sound)をミックス・マスターに広げたものにあたる
- DiffVox(DAFx 2025、コード MIT): ボーカルの効果のパラメータ分布(プリセット分布も公開)。DeepAFx-ST は Adobe の研究用ライセンス(非商用)
- 効果のブラインド推定(Peladeau & Peeters、ICASSP 2024)、WildFX(REAPER + 任意のプラグインで効果グラフ付きデータを合成)

### 6.3 「聴ける」LLM の限界

- "LLMs can read music, but struggle to hear it"(PMLR 2026): Gemini 2.5 Pro / Flash と Qwen2.5-Omni で、シンコペーション・移調・和音の種類の判定が
  **入力を MIDI から音声に替えると急落**。思考の連鎖でも補えない
- MRMAD(EMNLP 2026): 18 の音声 LLM が音の劣化の診断・比較を苦手とし、人との差が大きい
- MixAssist(2025): プロと初心者のミキシング対話 431 ターン。Qwen-Audio を微調整したものが最良
- → **音声 LLM に聴かせて判断させるより、処理の前後の数値の差を返す方が確実**。Glaux の方針(analyze_audio の数値を AI に渡す)は研究と一致

### 6.4 LLM エージェントが DAW を操作する研究

- WavCraft(ICLR 2024 WS)、Loop Copilot(2023)、**LLM2Fx(WASPAA 2025、言葉からつまみをゼロショット)**、**LLM2Fx-Tools(ICLR 2026、ツール呼び出しで FX チェーン。データセット LP-Fx)**、
  Text2FX(ICASSP 2025、CLAP + DDSP)、**InstructFX2FX(DAFx26、LLM が計画し CLAP 距離の最適化が詰める。LLM に聞き直す方式より 10 組中 9 組で良い)**、
  TimberAgent(2026、プリセット検索)、DAWZY(2025-12、REAPER + MCP、取り消せる原子的な操作、操作前の状態の再取得)、ableton-mcp、reaper-mcp
- 共通の設計: 取り消せる細かい操作、操作の前に状態を読み直す、LLM が計画し数値の最適化が詰める。**Glaux の Command と履歴はこの先を行っている**。
  差をつける余地は「測定 → 修正 → 再測定」を 1 つの道具で回せること

### 6.5 マスタリング AI(公開されている範囲)

- iZotope Ozone(Master Assistant): 大量の曲を 10 ジャンルに分けてジャンルごとの目標スペクトル。分類器で入力をジャンルの混合比に写し、固有の目標カーブへ EQ のノードを寄せる
- LANDR Synapse: スタイル判定、ダイナミクス・帯域・幅の解析、参照曲に寄せる(内部は非公開)
- Matchering: RMS・周波数特性・ピーク・ステレオ幅を参照曲に合わせる(**GPLv3**。コードは見ずに考え方だけ参考に)
- ITO-Master(Sony、ISMIR 2025): 推論時最適化 + CLAP の言葉の条件で、利用者が微調整できるスタイル転写
- → 中身は「目標トーンカーブ + EQ マッチ + ラウドネス目標のリミッタ」が中心で、pure Rust で作れる

### 6.6 音源分離

- HT Demucs(9.20 dB、コード MIT、**重みのライセンスは公式に未回答** #327。Meta のリポジトリは 2025-01 に読み取り専用)、BS-RoFormer(SDX23 1 位、9.80 dB)、
  Mel-RoFormer、SCNet(軽い)、MSST(コミュニティの重みはライセンスが全部「Not stated」)
- Demucs の ONNX 化(Mixxx GSoC 2025、CPU で約 18% 速く品質差 0.1 dB 未満)。軽量・リアルタイム: HS-TasNet(23ms、4.65 dB)、RT-STT(1M パラメータ未満)
- Music Source Restoration チャレンジ 2025(分離 + EQ・コンプ・リバーブ・マスタリングを元に戻す)
- → 外部ツールとして呼ぶ今の形が妥当。**重みのライセンスが明確になるまで同梱しない**

### 6.7 ニューラル音声モデル

- DAC(Descript、**コードも重みも MIT**)、EnCodec(24/48kHz 版のコード MIT。重みは要確認)、RAVE(**CC-BY-NC**)、Neutone SDK(PyTorch のモデルを DAW で)、
  Stable Audio Open / 3.0(Community License。3.0 は二次情報)
- ミキシングへの直接の効き目は小さい。コーデックの潜在空間を類似検索に、生成は外部連携に

### 6.8 推論の実装

- tract(pure Rust、ONNX のテストの約 85%)を主に、足りない演算が出たら ort。rten(pure Rust)、candle / burn
- RT で動かすなら RTNeural 型の小さな自前 RNN に限る。重みのライセンス(CC-BY-NC など)に注意

---

## 7. ライセンスの一覧

| 区分 | もの |
|---|---|
| **使える(MIT / BSD / Apache / ISC / Zlib / CC0 / CC-BY)** | chowdsp_wdf(BSD-3)、NAM Core(MIT)、NeuralAmpModeler-rs(Apache-2.0)、nam-rs(MIT)、Signalsmith の dsp・Stretch・reverb 例・limiter(MIT)、r8brain(MIT)、libsamplerate(BSD-2)、rubato・fundsp(MIT/Apache)、HIIR(WTFPL)、fft-convolver・realfft・rustfft(MIT 系)、wide・pulp・fearless_simd・multiversion(MIT 系)、rtrb・ringbuf・basedrop(MIT/Apache)、ebur128(MIT)、libbs2b(MIT)、StereoWidener(CC0)、SADIE II(Apache-2.0)、MIT KEMAR(引用条件)、libear(Apache-2.0)、SAF コア(ISC)、ViSQOL(Apache-2.0)、ST-ITO(Apache-2.0)、dasp-pytorch・GRAFX(Apache-2.0)、DiffVox・FxNorm-Automix(MIT)、DAC(MIT)、audiobox-aesthetics(CC-BY 4.0、重みは要確認)、RTNeural(BSD-3) |
| **条件付き** | audio_thread_priority・Bungee・triple_buffer(MPL-2.0、ファイル単位のコピーレフト)、libiamf(BSD-3-Clause-Clear、特許は別)、Mesh2HRTF(EUPL)、Stable Audio(Community License)、ASIO(独自ライセンス側) |
| **避ける(GPL / LGPL / 非商用 / 不明)** | Diff-MST・Diff2Mix(CC-BY-NC 系)、DeepAFx-ST(Adobe 研究用)、RAVE・MERT(CC-BY-NC)、Matchering(GPLv3)、Rubber Band(GPL)、chowdsp_utils の DSP 群(GPLv3)、GuitarML Proteus(GPL)、FFTW・KFR(GPL)、IEM Plug-in Suite・SPARTA(GPL-3.0)、CamillaDSP・DRC-FIR(GPL)、GstPEAQ(LGPL-2)、SoX(LGPL-2.1)、Demucs の重み・MSST の重み(ライセンス不明)、SONICOM HRTF・AutoEq の測定データ(再配布条件が未確認) |

---

## 8. Glaux への取り入れ計画

優先度と作業量(S/M/L)は各調査の評価をまとめたもの。★ は複数の調査で挙がったもの。

### 段階 1: 小さくてすぐ効く(S 中心)

| # | 内容 | 観点 | 根拠 |
|---|---|---|---|
| 1 | ★ cpal の `realtime` 機能を有効に(Windows の MMCSS)。Windows で電力の抑制から外す | PC | §5.1 |
| 2 | ★ aarch64 でも denormal を 0 に(FPCR の FZ) | PC | §5.2 |
| 3 | ★ 書き出しのリミッタを True Peak の検出に(4〜8 倍、上限 -1 dBTP、大きいマスターは -2)。かけた量を報告 | 耳 | §3.1 |
| 4 | PLR / PSR と、配信先ごとの正規化の予測(Spotify・Apple・YouTube・AES77)を解析と書き出しの報告に | 耳・AI | §3.1 |
| 5 | ★ コンプの作り直し(log 領域の検出、ソフトニー、ピーク / RMS、検出側のハイパス、ステレオリンク) | 音質 | §4.4 |
| 6 | ★ EQ を Cytomic SVF か Vicanek の matched 2 次に(シェルフは 2 極)。HP / LP を足す | 音質 | §4.1 |
| 7 | ディレイ・コーラス・テープ・pluck の補間を 4 点(Lagrange / Hermite。sampler の Hermite を再利用) | 音質 | §4.6 |
| 8 | パラメータの平滑化(係数はブロックの頭で計算してブロック内で補間。フェーダー・パンはランプ) | 音質 | §5.4 |
| 9 | ★ 相関メータ・ゴニオメータ、モニターの切り替え(モノラル・S だけ・左右入れ替え)、クロスフィード(bs2b 相当) | 耳 | §3.6, §3.7 |
| 10 | バッファサイズを選べるように、実際のサイズと xrun を表示 | PC | §5.3 |
| 11 | 墓場の解放を `strong_count == 1` の確認に | PC | §5.7 |
| 12 | ★ 処理の前後の比較ツール(MCP): Command の前後で帯域・ラウドネス・マスキング・ステレオの差を返す。**音量を揃えた A/B** | AI | §6.3, §3.4 |
| 13 | AI 向けの手引き(MCP の guide)に、自動ミキシングの経験則・配信の目標値・True Peak の注意を書き足す | AI | §6.1, §3.1 |

### 段階 2: 中くらい(M 中心)

| # | 内容 | 観点 | 根拠 |
|---|---|---|---|
| 14 | ★ distortion / amp / tape / soft_clip の折り返し対策(1 次 ADAA か 2〜4 倍の多相ハーフバンド。amp は全体をオーバーサンプリング) | 音質 | §4.2 |
| 15 | ★ マスターにリアルタイムの True Peak ルックアヘッドリミッタ(PDC と整合) | 耳 | §4.4 |
| 16 | ★ リアルタイムの LUFS(M/S/I)・True Peak のメータとスペクトラムアナライザ(1/n オクターブ・傾き・ピークホールド)。計算は RT の外 | 耳 | §3.7 |
| 17 | ★ 心理音響に基づくマスキング解析(広がり関数・絶対閾・時間マスキング・トーン性・ERB)→「どのトラックのどの帯域を何 dB」を返す | 耳・AI | §3.2 |
| 18 | ダイナミック EQ / スペクトル・ダッキング(IIR のフィルタバンクで低遅延) | 耳 | §3.2 |
| 19 | ★ 参照曲に寄せる(ミックス・マスター): 内蔵の eq / compressor / master を CMA-ES で。距離は帯域・LUFS・LRA・幅・CLAP | AI | §6.2, §6.5 |
| 20 | マスタリング助手: 目標トーンカーブ + EQ マッチ + 配信先ごとのラウドネス目標 + True Peak | AI・耳 | §6.5 |
| 21 | 言葉の指示で効果を追い込む(Claude が計画し、CLAP の印象語との距離で CMA-ES が詰める) | AI | §6.4 |
| 22 | リバーブを FDN(8〜16 ch)に、プレート(Dattorro)をプリセットに。プリディレイ、高いサンプルレートでも同じ部屋に | 音質 | §4.5 |
| 23 | 帯域別のステレオ指標とモノラルにしたときの LUFS の低下を解析に(パンニングインデックス、負の相関の時間の割合、低域の S/M 比) | 耳・AI | §3.6 |
| 24 | トーナルバランス解析(1/3 オクターブ / ERB の長時間平均、傾き dB/oct、目標とのずれ) | 耳 | §3.3 |
| 25 | 内蔵エフェクトをブロック処理 + SIMD(`wide` か `pulp`)に | PC | §5.5 |
| 26 | CLAP の `tail`・プロセスステータス・`constant_mask` を使う。PDC の遅延変更通知に対応 | PC | §5.4 |
| 27 | トラック別・プラグイン別の処理時間、Tracy、RTSan を Linux の CI に | PC | §5.9 |
| 28 | 幅のエフェクト(M/S のゲイン、低域のモノ化、velvet noise のデコリレーション、トランジェントは素通し) | 耳 | §3.6 |
| 29 | ステレオ素材のパンをバランスの法則に直し、パン則を選べるように(既存の曲の音が変わるので判断が要る) | 耳 | §3.6 |
| 30 | sampler のピッチを上げたときの帯域制限(多相 sinc)と、読み込み時の rubato による変換。sampler と SF2 のステレオ | 音質 | §4.7, §2 |
| 31 | マルチバンドコンプ(LR4)とトランジェントシェイパー | 音質 | §4.4 |

### 段階 3: 大きい(L)・様子見

| # | 内容 | 観点 |
|---|---|---|
| 32 | 依存グラフによる並列処理(Ardour 型、ワークグループ / MMCSS に参加)。**計測してから** | PC |
| 33 | 畳み込みリバーブ(fft-convolver + realfft) | 音質 |
| 34 | タイムストレッチを PGHI + WSOLA の混合に(Signalsmith Stretch は MIT の C++) | 音質 |
| 35 | 共鳴抑制エフェクト(STFT + 包絡比較。遅延と RT の制約が難所) | 耳 |
| 36 | 仮想スピーカー(SADIE II の HRIR を埋め込み、±30°、初期反射)、小型スピーカーのシミュレーション(自前設計の EQ) | 耳 |
| 37 | 16bit 書き出し用のノイズシェーピング(44.1k / 48k 別に論文から係数を設計) | 耳 |
| 38 | NAM の読み込み(アンプの置き換え。ARM で動くかと FFI が課題)、WDF による回路モデル | 音質 |
| 39 | 客観品質評価(ViSQOL の移植、または CLAP 埋め込みの距離)を DSP の回帰テストに。audiobox-aesthetics を目安として | AI |
| 40 | 分離の強化(RoFormer 系を外部オプションに。重みは同梱しない) | AI |
| 41 | 仮想低音、ISO 532 系の部分ラウドネス、イマーシブ(Atmos / IAMF / Ambisonics)、ルーム補正、GPU オーディオ、RT のニューラル効果器 | 各 |

---

## 9. 確かめていないことのまとめ

- 配信: Apple Music の持ち上げ方向の動作、Amazon / Tidal / Deezer の目標値、Atmos の音楽の目安値(二次情報)
- 心理音響: Johnston のマスキング閾値の下げ幅、時間マスキングの数値(二次情報)、低域のモノ化の周波数(慣行)、仮想低音の特許、ISO 532-3 付属コードの利用条件
- ライセンス: audiobox-aesthetics・EnCodec の重み、MEGAMI・StemFX・Fx-Encoder++・Text2FX・SCNet・MixAssist のコード・重み、SONICOM と AutoEq の測定データ、
  libbs2b の Rust 移植と libmysofa の LICENSE 本文
- 技術: テープのヒステリシスの詳細、プログラム依存リリースとトランジェントシェイパーの学術的な出典、tract で NAM を実時間処理できるか、tract の int8 対応、
  cpal の macOS ワークグループ対応、OS からのヘッドトラッキングの取得、REAPER の録音待機トラックの振り分け
- BS.1770-5 の -4 からの差分の細部、Stable Audio 3.0 の一次情報

---

## 10. 出典

### ラウドネス・配信・心理音響

- ITU-R BS.1770-5(2023/11): https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en / PDF: https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf
- EBU R128 s2(2023/11): https://tech.ebu.ch/docs/r/r128s2.pdf / Tech 3342: https://tech.ebu.ch/docs/tech/tech3342.pdf / Tech 3341: https://tech.ebu.ch/docs/tech/tech3341.pdf
- AES TD1008: https://aes.org/wp-content/uploads/2024/01/20210924_TD1008_v3.13.pdf / AES77-2023: https://www.aes.org/standards/blog/2023/7/aes77-2023
- Spotify のラウドネス正規化: https://support.spotify.com/us/artists/article/loudness-normalization/
- Apple の LUFS 移行(二次情報): https://www.meterplugs.com/blog/2022/03/23/apple-switch-to-lufs.html / YouTube(二次情報): https://www.meterplugs.com/blog/2019/09/18/youtube-changes-loudness-reference-to-14-lufs.html
- Mourgela ら(AES 157th, 2024): https://arxiv.org/pdf/2412.03373
- PLR / PSR: https://www.meterplugs.com/blog/2017/05/18/crest-factor-psr-and-plr.html / https://productionadvice.co.uk/plr/
- ISO 226 改訂の解説: https://www.jstage.jst.go.jp/article/ast/45/1/45_e23.66/_article / ISO 532-3:2023: https://www.iso.org/standard/69856.html
- Hafezi & Reiss(マスキングを減らす自動 EQ): https://qmro.qmul.ac.uk/xmlui/handle/123456789/7804 / Ronan ら 2018: https://arxiv.org/pdf/1803.09960 / 明瞭度の予測: https://arxiv.org/abs/2103.12152
- FabFilter Pro-Q 4: https://www.fabfilter.com/news/1733994000/fabfilter-releases-pro-q-4-equalizer-plug-in / soothe2: https://oeksound.com/manuals/soothe2/ / Gullfoss: https://www.soundtheory.com/gullfoss
- Pestana ら 2013(市販曲のスペクトル): https://www.researchgate.net/publication/274511175 / iZotope Tonal Balance: https://s3.amazonaws.com/izotopedownloads/docs/tonal-balance-control/meters-and-target-curves/index.html
- Hjortkjær & Walther-Hansen 2014: https://orbit.dtu.dk/en/publications/perceptual-effects-of-dynamic-range-compression-in-popular-music-/
- 仮想低音: https://ieeexplore.ieee.org/document/10187677/ / https://www.researchgate.net/publication/236843786
- ノイズシェーピング(Wannamaker): https://www.researchgate.net/publication/242019172 / SoX の係数表(LGPL、参照のみ): https://github.com/chirlu/sox/blob/master/src/dither.c
- ViSQOL: https://github.com/google/visqol / GstPEAQ: https://github.com/HSU-ANT/gstpeaq
- K-System: https://www.digido.com/ufaqs/k-system/ / ITU-T H.870 V2: https://www.itu.int/epublications/publication/itu-t-h-870-v2-2022-03-guidelines-for-safe-listening-devices-systems
- 時間マスキング(二次情報): https://pmc.ncbi.nlm.nih.gov/articles/PMC2676627/

### ステレオ・空間・メータ

- ebur128: https://github.com/sdroege/ebur128 / ebur128-stream: https://github.com/KitaitiMakoto/ebur128-stream
- Das, Stereo Widening(DAFx24): https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_92.pdf / 実装(CC0): https://github.com/orchidas/StereoWidener / OVN: https://www.audiolabs-erlangen.de/resources/2018-DAFx-VND
- bs2b: https://bs2b.sourceforge.net/ / https://bs2b.sourceforge.net/license.html / Rust 版: https://github.com/xikxp1/bs2b
- AutoEq: https://github.com/jaakkopasanen/AutoEq / Harman ターゲット: https://www.soundguys.com/harman-target-soundguys-preference-curve-validated-125420/ / https://www.headphonesty.com/2024/10/harman-target-curve-alive/
- Sonarworks Virtual Monitoring: https://www.sonarworks.com/soundid-reference/virtual-monitoring
- SADIE II: https://www.york.ac.uk/sadie-project/database.html / MIT KEMAR: https://sound.media.mit.edu/resources/KEMAR.html / SONICOM: https://www.sonicom.eu/the-new-and-extended-sonicom-hrtf-dataset/ / https://arxiv.org/abs/2507.05053
- LAP Challenge 2024: https://www.merl.com/news/award-20240829-1540 / https://www.imperial.ac.uk/sonicom/lap-challenge/ / Mesh2HRTF: https://github.com/Any2HRTF/Mesh2HRTF
- SOFA: https://www.sofaconventions.org/mediawiki/index.php/SOFA_(Spatially_Oriented_Format_for_Acoustics) / libmysofa: https://github.com/hoene/libmysofa / sofar: https://github.com/andreiltd/sofar
- IEM Plug-in Suite: https://github.com/tu-studio/IEMPluginSuite / SAF: https://github.com/leomccormack/Spatial_Audio_Framework / SPARTA: https://github.com/leomccormack/SPARTA
- EBU ADM Renderer: https://github.com/ebu/ebu_adm_renderer / libear: https://github.com/ebu/libear
- Eclipsa Audio: https://opensource.googleblog.com/2025/01/introducing-eclipsa-audio-immersive-audio-for-everyone.html / https://opensource.googleblog.com/2025/06/introducing-open-source-daw-plugin-for-eclipsa-audio.html / OBR: https://github.com/google/obr / https://audiolab.york.ac.uk/2025/11/12/the-open-binaural-renderer-for-eclipsa-audio/
- Apple ASAF / APAC: https://developer.apple.com/videos/play/wwdc2025/403/ / https://www.flatpanelshd.com/news.php?subaction=showfull&id=1750327296 / 空間オーディオ: https://support.apple.com/en-us/109354 / https://www.production-expert.com/production-expert-1/why-your-atmos-mix-will-sound-different-on-apple-music / 個人化: https://musictech.com/news/gear/ios-16-personalised-spatial-audio-ear-scans-iphone-camera/
- Atmos(二次情報): https://en.wikipedia.org/wiki/Dolby_Atmos / https://ralphsutton.com/dolby-atmos-standards-deliverables-2025/
- Farina の指数スイープ: https://www.angelofarina.it/Public/Presentations/AES122-Farina.pdf

### 要素技術

- Zavalishin: https://archive.org/details/the-art-of-va-filter-design-rev.-2.1.2 / Cytomic: https://cytomic.com/technical-papers/ / https://www.cytomic.com/files/dsp/SvfLinearTrapOptimised.pdf
- Vicanek: https://vicanek.de/articles/BiquadFits.pdf / https://vicanek.de/articles/2poleShelvingFits.pdf / https://vicanek.de/articles/FastSettlingFilters.pdf / Orfanidis 1997: https://aes.org/publications/elibrary-page/?id=7854
- 微分可能な全極フィルタ(DAFx24): https://arxiv.org/abs/2404.07970v2
- ADAA: https://www.research.ed.ac.uk/en/publications/antiderivative-antialiasing-for-memoryless-nonlinearities/ / Holters: https://www.hsu-hh.de/ant/wp-content/uploads/sites/699/2020/10/DAFx2019_paper_4.pdf / WDF + ADAA: https://dafx2020.mdw.ac.at/proceedings/papers/DAFx2020_paper_35.pdf / 補間フィルタ: https://dafx.de/paper-archive/2024/papers/DAFx24_paper_33.pdf / 実務の注意: https://jatinchowdhury18.medium.com/practical-considerations-for-antiderivative-anti-aliasing-d5847167f510
- BLAMP: http://research.spa.aalto.fi/publications/papers/dafx16-blamp/ / HIIR: https://github.com/unevens/hiir/blob/master/readme.txt
- ニューラル歪みの折り返し: https://arxiv.org/abs/2505.04082 / https://arxiv.org/pdf/2505.11375 / https://arxiv.org/pdf/2501.18470
- chowdsp_wdf: https://github.com/Chowdhury-DSP/chowdsp_wdf / chowdsp_utils: https://github.com/Chowdhury-DSP/chowdsp_utils / Wright ら 2020: https://doi.org/10.3390/app10030766
- NAM: https://github.com/sdatkinson/NeuralAmpModelerCore / https://neural-amp-modeler.readthedocs.io/en/latest/model-file.html / NeuralAudio: https://github.com/mikeoliphant/NeuralAudio / NeuralAmpModeler-rs: https://docs.rs/NeuralAmpModeler-rs/latest/neural_amp_modeler_rs/ / Slimmable NAM: https://arxiv.org/pdf/2511.07470 / nam-rs: https://docs.rs/nam-rs/latest/nam_rs/ / Proteus: https://github.com/GuitarML/Proteus
- 光学式コンプの状態空間モデル: https://arxiv.org/pdf/2408.12549 / Giannoulis ら 2012: https://secure.aes.org/forum/pubs/journal/?ID=174 / Signalsmith のリミッタ: https://signalsmith-audio.co.uk/writing/2022/limiter/
- リバーブ: https://signalsmith-audio.co.uk/writing/2021/lets-write-a-reverb/ / https://github.com/Signalsmith-Audio/reverb-example-code / Dattorro: https://ccrma.stanford.edu/~dattorro/EffectDesignPart1.pdf / Välimäki ら: https://dl.acm.org/doi/10.1109/TASL.2012.2189567 / 微分可能 FDN: https://www.dafx.de/paper-archive/2023/DAFx23_paper_32.pdf / https://arxiv.org/pdf/2402.11216 / https://arxiv.org/pdf/2511.20380 / 文献一覧: https://github.com/gdalsanto/delay-network-reverbs / FDN の散乱: https://arxiv.org/pdf/1912.08888 / dark velvet noise: https://arxiv.org/abs/2403.20090 / https://www.dafx.de/paper-archive/2024/papers/DAFx24_paper_63.pdf / Wefers 2015: https://publications.rwth-aachen.de/record/466561
- ストレッチ: https://arxiv.org/abs/2202.07382 / Signalsmith Stretch: https://github.com/Signalsmith-Audio/signalsmith-stretch / Bungee: https://github.com/bungee-audio-stretch/bungee / Rubber Band: https://breakfastquay.com/rubberband/license.html / Signalsmith dsp: https://github.com/Signalsmith-Audio/dsp
- SRC: https://github.com/HEnquist/rubato/ / https://github.com/avaneev/r8brain-free-src / https://libsndfile.github.io/libsamplerate/license.html / fundsp: https://github.com/SamiPerttu/fundsp
- シンセ: https://www.dafx.de/paper-archive/2012/papers/dafx12_submission_69.pdf / https://dafx2020.mdw.ac.at/proceedings/papers/DAFx2020_paper_61.pdf / https://dafx.de/paper-archive/2022/papers/DAFx20in22_paper_3.pdf

### PC への最適化

- SIMD: https://shnatsel.medium.com/the-state-of-simd-in-rust-in-2025-32c263e5f53d / https://github.com/rust-lang/portable-simd / https://blog.rust-lang.org/2025/04/03/Rust-1.86.0/ / https://github.com/linebender/fearless_simd / https://github.com/rust-lang/libs-team/issues/877
- DAW の方式: https://github.com/Ardour/ardour/blob/master/libs/ardour/graph.cc / https://forum.juce.com/t/tracktion-graph-with-audio-workgroups/62610 / https://help.ableton.com/hc/en-us/articles/209067649-Multi-core-performance-in-Ableton-Live-FAQ / https://www.soundonsound.com/techniques/running-multiple-plug-ins / https://www.kvraudio.com/forum/viewtopic.php?t=591669 / Firewheel: https://github.com/BillyDM/Firewheel/blob/main/DESIGN_DOC.md
- OS: https://developer.apple.com/videos/play/wwdc2020/10224/ / https://developer.apple.com/documentation/audiotoolbox/adding-parallel-real-time-threads-to-audio-workgroups / https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service / https://helpcenter.steinberg.de/hc/en-us/articles/8412891538066 / https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessinformation / https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/low-latency-audio / https://github.com/mozilla/audio_thread_priority
- CLAP: https://github.com/free-audio/clap/blob/main/include/clap/process.h / https://github.com/free-audio/clap/blob/main/include/clap/audio-buffer.h / https://github.com/free-audio/clap/blob/main/include/clap/ext/thread-pool.h
- cpal: https://github.com/RustAudio/cpal/blob/master/CHANGELOG.md / https://github.com/RustAudio/cpal/issues/1220 / https://github.com/RustAudio/cpal/pull/1368 / ASIO: https://www.kvraudio.com/news/steinberg-moves-vst-3-sdk-to-mit-open-source-license-asio-now-gplv3-65179 / PipeWire: https://www.linuxdj.com/notes/pipewire-quantum-in-2026-choosing-64-128-256-512-without-xruns/
- ロックフリー: https://micahrj.github.io/posts/basedrop/ / https://conference.audio.dev/session/2024/wait-free-thread-synchronisation-with-the-seqlock/ / https://conference.audio.dev/session/2025/demystifying-stdmemory_order/
- denormal: https://github.com/mixxxdj/mixxx/issues/16126 / https://github.com/Sin-tel/no_denormals
- FFT / 畳み込み: https://github.com/ejmahler/RustFFT / https://github.com/neodsp/fft-convolver / http://publications.rwth-aachen.de/record/466561/files/466561.pdf / https://github.com/project-gemmi/benchmarking-fft / https://kfrlib.com/
- GPU / 推論: https://synthanatomy.com/2025/03/gpu-audio-releases-sdk-bringing-gpu-processing-to-all-plugin-developers-musicians-and-more.html / https://www.soundonsound.com/news/gpu-audio-update-their-sdk / https://arxiv.org/pdf/2106.03037 / https://arxiv.org/abs/2506.12665
- 計測: https://releases.llvm.org/20.1.0/tools/clang/docs/RealtimeSanitizer.html / https://steck.tech/posts/rtsan-in-rust/ / https://github.com/rust-lang/rfcs/pull/3766 / https://github.com/wolfpld/tracy

### AI・機械学習

- 自動ミキシング: https://arxiv.org/pdf/2407.08889 / https://github.com/sai-soum/Diff-MST / https://arxiv.org/abs/2511.08040 / https://arxiv.org/abs/2607.15634 / https://arxiv.org/abs/2608.05506 / https://arxiv.org/abs/2608.05442 / https://arxiv.org/abs/2603.15995 / https://arxiv.org/abs/2507.02273 / https://arxiv.org/abs/2608.28127 / https://github.com/sony/FxNorm-automix / https://arxiv.org/abs/2208.11428 / https://arxiv.org/abs/2609.02835 / https://sigsep.github.io/datasets/musdb.html / 経験則: https://www.open-access.bcu.ac.uk/4968/1/WIMP2017_DeManEtAl.pdf / https://csteinmetz1.github.io/AutomaticMixingPapers/
- 微分可能 DSP・効果の推定: https://github.com/csteinmetz1/dasp-pytorch / https://arxiv.org/pdf/2408.03204 / https://github.com/sh-lee97/grafx / https://arxiv.org/pdf/2502.11668 / https://arxiv.org/abs/2410.21233 / https://github.com/csteinmetz1/st-ito / https://github.com/adobe-research/DeepAFx-ST/blob/main/LICENSE / https://github.com/SonyResearch/diffvox / https://arxiv.org/pdf/2505.11315 / https://arxiv.org/abs/2310.11781 / https://arxiv.org/abs/2507.10534
- 分離: https://arxiv.org/abs/2211.08553 / https://github.com/facebookresearch/demucs / https://github.com/facebookresearch/demucs/issues/327 / https://mixxx.org/news/2025-10-27-gsoc2025-demucs-to-onnx-dhunstack/ / https://arxiv.org/abs/2309.02612 / https://arxiv.org/abs/2310.01809 / https://arxiv.org/abs/2401.13276 / https://arxiv.org/abs/2607.23395 / https://github.com/ZFTurbo/Music-Source-Separation-Training/issues/245 / https://arxiv.org/abs/2511.13146 / https://arxiv.org/abs/2402.17701 / https://arxiv.org/pdf/2603.04032
- ニューラル音声: https://github.com/descriptinc/descript-audio-codec / https://github.com/facebookresearch/encodec / https://github.com/acids-ircam/RAVE / https://arxiv.org/abs/2508.09126 / https://huggingface.co/stabilityai/stable-audio-open-1.0 / https://www.therundown.ai/tools/stable-audio-3-0
- 評価・聴ける LLM: https://github.com/facebookresearch/audiobox-aesthetics / https://arxiv.org/pdf/2502.05139 / https://arxiv.org/abs/2505.10793 / https://arxiv.org/abs/2601.07237 / https://arxiv.org/abs/2608.11755 / https://arxiv.org/abs/2607.13903 / https://proceedings.mlr.press/v303/carone26a.html / https://arxiv.org/abs/2608.22236 / https://arxiv.org/abs/2507.06329 / https://huggingface.co/Qwen/Qwen3-Omni-30B-A3B-Captioner / https://github.com/shansongliu/MU-LLaMA / https://github.com/LAION-AI/CLAP
- LLM エージェント: https://github.com/SonyResearch/LLM2Fx / https://arxiv.org/abs/2505.20770 / https://arxiv.org/abs/2512.01559 / https://arxiv.org/abs/2409.18847 / https://arxiv.org/abs/2606.22005 / https://arxiv.org/abs/2603.09332 / https://arxiv.org/abs/2512.03289 / https://arxiv.org/pdf/2403.09527v2 / https://arxiv.org/pdf/2310.12404 / https://github.com/ahujasid/ableton-mcp / https://github.com/bonfire-systems/reaper-mcp
- マスタリング: https://s3.amazonaws.com/izotopedownloads/docs/ozone9/en/master-assistant/index.html / https://www.izotope.com/en/learn/how-to-use-master-assistant-in-ozone / https://www.landr.com/what-is-ai-mastering-and-how-does-it-work / https://blog.landr.com/synapse-engine-update/ / https://github.com/sergree/matchering / https://arxiv.org/abs/2506.16889
- 推論: https://github.com/pykeio/ort / https://arewelearningyet.com/inference/ / https://github.com/jatinchowdhury18/RTNeural
