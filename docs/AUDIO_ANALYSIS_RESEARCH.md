# 音を分析する能力の強化: 研究・ツールの調査(2026-09-23)

目的: AI(Claude)は音声を直接聴けず、今は曲全体の数値の要約(LUFS・帯域エネルギー・スペクトル重心・
オンセット数)でしか音を知覚できない。「サンプル音源に似た音を作る」などの前段として、
音を分析する能力を強くするために、取り入れられる研究・ツールを広く調べた。

調べ方: 3 つのテーマ(従来型の音響分析 / 音を理解する学習モデル / 音色のマッチング)を並行して
Web 調査した。**ライセンス・性能の記述は各出典に基づくが、採用前に各リポジトリの LICENSE と
重みの配布条件を必ず最終確認すること。** 「推測」と書いたものは調査者の見立て。

採用の条件: GPL / AGPL を避ける(学習済みモデルの重みの非商用制限も不可)、一般的な CPU で動く、
Rust から使える(純 Rust クレート、または ONNX を tract で推論)、Python 依存は任意の外部連携のみ。

---

## 1. すぐ使える・取り入れやすいもの

### 1.1 従来型の音響分析(純 Rust で実装できる)

| 候補 | ライセンス | できること | 取り入れ方 |
|---|---|---|---|
| **Timbre Toolbox の記述子**(Peeters ら 2011) | 論文を仕様に自前実装 | 単音の音色を数値化: 立ち上がり時間・減衰・スペクトル重心とその推移・広がり・平坦さ・倍音構造(奇数/偶数倍音比・tristimulus・非調和性)・倍音とノイズの比(HNR)・ビブラート | 既存の FFT と YIN の上に実装し、「数値 + 言葉のラベル」で返す MCP ツールにする。知覚研究では **立ち上がり時間・スペクトル重心・スペクトルの細かな凹凸**が音色の主な次元とされる(Caclin ら 2005、McAdams 2019) |
| **`ebur128` クレート** | MIT | BS.1770 準拠の瞬時・短期・統合ラウドネス、ラウドネスレンジ(LRA)、True Peak。EBU のテストに合格 | 依存を足すだけ。ラウドネスの時間変化とダイナミクスを AI に渡す |
| ステレオ幅・位相 | 自前(数十行) | 左右の相関、M/S のエネルギー比、帯域ごとの相関 | CLAP プラグインの出力がステレオなので、広がりの診断に使える |
| トラック間のマスキング(簡易版) | 自前 | 聴覚の帯域(バーク帯域)ごとに「あるトラックが別のトラックを何 dB 上回っている時間の割合」 | 「どの帯域で何が何を埋もれさせているか」を返す。厳密な心理音響モデル(ISO 532-1)は後回しでよい(推測) |
| `spectrum-analyzer`、`pitch-detection`、`resonant-analysis`(2026-07、MFCC・chroma・キー推定・LUFS) | MIT / Apache-2.0 | スペクトル・音程・各種特徴量 | resonant-analysis は新しく実績が少ないので、依存より参考実装として読む(推測) |

### 1.2 軽い学習モデル(ONNX を tract で推論)

| 候補 | ライセンス | できること | 取り入れ方 |
|---|---|---|---|
| **SwiftF0**(2025) | MIT、ONNX 公開 | 音程推定。約 9.6 万パラメータと非常に軽く、CPU で CREPE の約 42 倍速く、雑音下でも高精度 | YIN の上位版として、単音の音程・ビブラートの測定と、鼻歌の譜起こしの改善に使う |
| **Beat This!**(ISMIR 2024) | MIT(コード・重み) | 音声からビート・小節頭(ダウンビート)・テンポを推定 | 純 Rust の移植 beat-this-rs(MIT、推論エンジン rten)がある。**取り込んだ音声の元テンポの自動検出**(テンポ追従の original_bpm)や、録音の小節合わせに使える。rten と tract の 2 つの推論エンジンを並べるかは要検討 |

### 1.3 音を「言葉で」理解する

| 候補 | ライセンス | できること | 取り入れ方 |
|---|---|---|---|
| **LAION-CLAP 音楽版**(`larger_clap_music`) | Apache-2.0(重み) | 音と言葉を同じ空間に写すモデル。「明るい」「歪んだ」「金属的な」などの言葉と音の近さが測れる。音声側の ONNX 版の公開例あり(約 276MB) | 音声側だけを ONNX で動かし、**音色語の辞書は言葉側の埋め込みを事前計算して同梱**する。`describe_timbre`(近い言葉の上位を返す)、`similar_sounds`(音色の類似検索)を作る。人の音色評価との一致は CLAP 系で最も安定だが完全ではなく、EQ による音色の変化は捉えにくい → 1.1 の数値と併用が前提。サイズが大きいので初回に取得する方式が現実的(推測)。tract が演算を全部扱えるかは要確認(無理なら `ort` の tract バックエンド) |
| CLAP 埋め込み + 自前の小さな分類器 | 自前の重み | NSynth(CC BY 4.0、約 30 万音)の質感ラベル(bright / dark / distortion / percussive / reverb / fast_decay 等)で学習すると、ゼロショットより安定する見込み(推測) | 重みを Glaux の自前資産にできる |
| PANNs CNN14 | CC-BY-4.0 | 527 種類の音のタグ(「歪んだギター」「シンセ」など粗い分類) | 軽い補助として |

---

## 2. 音色のマッチング(似た音を作る)

事前学習済みで「すぐ使える」モデルは見つからなかった。代わりに、論文と市販品の両方で筋が良いと確認できた
**「多面的な音色の距離 + CMA-ES(微分を使わない進化的な最適化)+ プリセット検索」** の組み合わせが、Rust だけで作れる。

| 候補 | 状況 | 取り入れ方 |
|---|---|---|
| **Instrumental**(2026-03 の論文) | Glaux に最も近い構成。28 パラメータの減算式シンセを CMA-ES で探索し、距離は mel スペクトログラム + スペクトル重心 + MFCC。CPU で約 18 秒で改善の 9 割に到達 | この設計をそのまま借りる。コツは「スペクトルから初期値を決める」「3 つの音高で同時に合わせる(過学習を防ぐ)」 |
| `cmaes` クレート | MIT / Apache | CMA-ES の Rust 実装。内蔵シンセのつまみ探索にそのまま使える |
| 音色の距離(auraloss の multi-resolution STFT 距離など) | Apache-2.0(式を移植) | 減算式シンセにはスペクトログラム系の距離が良く、音量の揺れ(AM)には包絡を時間伸縮して比べる方法が良い(Salimi ら 2025)→ 観点ごとの差を AI に返す |
| プリセット検索 | 学習不要 | Surge XT の 2,944 プリセットを数音高で鳴らし、特徴(と CLAP の埋め込み)を保存しておき、サンプルに近い順に提示する。そこから主要なつまみ数十個を CMA-ES で詰める。Synplant 2 の Genopatch も「ニューラルネットの初期推定 + 反復改良」の混合方式とされる |
| Surge XT 向けの flow matching(Hayes ら、ISMIR 2025) | コード MIT、重みの公開は未確認 | Surge XT で既存手法を上回った最も近い研究。重みが公開されたら ONNX 化を検討。派生の synth-setter は GPL で不可 |
| 初期値を出す小さなモデルを自前で学習 | 自前の重み | 内蔵シンセで合成データを作り、オフラインで学習した小さな CNN を ONNX で動かし、CMA-ES の初期値に使う |
| DDSP・torchsynth・DiffMoog 等(微分可能なシンセ) | Apache / MIT | 実行時に使うのは不向き(Surge XT は微分できず、この問題では CMA-ES が勾配法より良いとの報告もある)。学習データ作りの道具としては有用 |

---

## 3. 任意の外部連携(重い・ネットワークを使う)

| 候補 | ライセンス / 条件 | 用途 |
|---|---|---|
| Gemini API の音声入力 | 利用者の API キー。音声を外部に送る | 「このミックスの問題点」のような自由記述の聞き取り。音楽の音色の説明はいちばん現実的な候補(推測) |
| Qwen3-Omni Captioner(30B)、MOSS-Music(8B、2026-05)、Qwen2-Audio(7B) | Apache-2.0 | ローカルの音声理解 LLM。GPU 前提で CPU での常用は非現実的 |
| All-In-One(ビート・小節頭・曲の構成ラベル) | MIT | 前段に音源分離が必要で重い。Demucs と同じく外部ツールとして連携 |

---

## 4. 使えないもの(ライセンス)

| 名前 | 理由 |
|---|---|
| aubio、qm-vamp-plugins | GPL |
| Essentia(本体) | AGPL。学習済みモデル群も非商用(CC BY-NC-SA) |
| PESTO(音程推定) | LGPL-3.0(静的リンク前提の Rust では扱いにくい) |
| madmom の学習済みモデル | 非商用(CC BY-NC-SA)。コードは BSD |
| MERT、MuQ / MuQ-MuLan、AudioCraft 版 EnCodec の重み、Audio Flamingo 系、Qwen2.5-Omni-3B | 非商用・研究用途のみ |
| M2D / M2D-CLAP | 評価用ライセンス |
| synth-setter | GPL |

性能の高い音楽向けモデルほど重みが非商用であることが多い点に注意。

---

## 5. 取り入れる順番の提案

| 段階 | 内容 | 使うもの | AI に増える感覚 |
|---|---|---|---|
| **A. 音を細かく数値化する**(純 Rust) | 単音の音色記述子、ラウドネスの時間変化・LRA・True Peak、ステレオ幅、トラック間のマスキング | Timbre Toolbox の仕様、`ebur128`、自前実装 | 「立ち上がりが遅い」「奇数倍音が多い(矩形波っぽい)」「2〜4 kHz で Lead が Pad に埋もれている」 |
| **B. 軽いモデルで精度を上げる** | 音程・ビブラートの高精度化、音声のビート・テンポ・小節頭 | SwiftF0、Beat This! | 取り込んだ音声のテンポが分かる、録音の音程が正確になる |
| **C. 音を言葉で捉える** | 音色語との近さ、音色の類似検索 | LAION-CLAP 音楽版(+ 自前の分類器) | 「明るく金属的なプラック」のような言葉で音を把握できる |
| **D. 似た音を作る** | 観点ごとの差の比較、内蔵シンセの自動フィット、Surge のプリセット検索 | A・C の特徴、`cmaes` | サンプルを渡すと近い音色を作れる |

進み具合: A 済(analyze_sound、analyze_audio の LRA・True Peak・ステレオ・マスキング)。
B1 済(SwiftF0 を同梱し analyze_sound の音程に使う。モデルは 1.1MB)。
B2 済(Beat This! small を同梱。rten ではなく tract で推論。analyze_beats と、テンポ追従の元テンポの自動検出)。
C 済(LAION-CLAP。**音楽版 `larger_clap_music` の Hugging Face 版は言葉側が壊れていた**ため `larger_clap_music_and_speech` に変更。
音声側 280MB は初回に取得、音色語 108 語は事前計算して同梱。analyze_sound の words)。
D 済(compare_sounds / match_sound / find_similar_presets と、音声クリップのメニュー「この音に似せた内蔵シンセのトラックを作る」)。

### 実際に試した結果(2026-09-23)

- CLAP の印象語: FluidR3 のピアノ・ベース・フルート・シンセリード・シンセパッドの 1 音で、楽器の 1 位がすべて正解
  (ピアノ / ベースギター / フルート / シンセリード / シンセパッド)。合成音のキックも「キック」
- subtractive の自動合わせ: 自分のパッチを目標にするとほぼ復元(距離 0.05)。FluidR3 の実楽器では 20 秒で
  1.17 → 0.48(ピアノ)、2.32 → 0.56(ベース)、0.97 → 0.59(フルート)、1.38 → 0.75(リード)、0.78 → 0.63(パッド)。
  8 倍の時間をかけても少ししか縮まらず、subtractive で表せる範囲の限界と見られる(推測)。次の一手は
  pluck / FM 的な音源の追加、エフェクト(reverb・chorus)込みの探索、Surge のつまみの探索
- (2026-09-24 追記)内蔵 FM シンセと、自動合わせの音源自動選択・リバーブ込みを追加。FM ベルは周波数比 3.5 を
  ほぼ完全に復元(距離 0.001)、リバーブ量・広さも復元。実楽器では fm 向きの音(ピアノ・フルート・パッド)が改善、
  subtractive 向きの音(ベース・リード)は探索時間が半分になった分やや悪化(0.56 → 0.63、0.75 → 0.89)
- (2026-09-24 追記)Surge XT のつまみの自動合わせ(refine_plugin_params)。プラックにした目標に対し距離 0.88 → 0.10。
  Surge は同じつまみでも毎回少し音が違う(距離 0.008〜0.04)ため、それより小さい差は詰められない
- プリセット検索: あるプリセットを別の高さ・サンプルレートで鳴らした音を目標にすると、同じプリセットが 1 位
  (距離 0.33、2 位は 1.4 以上。CLAP の近さも 0.85 対 0.55)

A は依存がほぼ増えず、既存の analyze_audio を大きく強化できるので最初の候補。
B・C はモデルの取得・同梱の方式(アプリのサイズ)を決めてから進める。

---

## 主な出典

- Timbre Toolbox: https://www.mcgill.ca/mpcl/files/mpcl/peeters_2011_jasa.pdf / 音色の知覚次元: https://www.mcgill.ca/mpcl/files/mpcl/caclin_2005_jasa_0.pdf
- ebur128: https://github.com/sdroege/ebur128
- SwiftF0: https://github.com/lars76/swift-f0 / https://arxiv.org/abs/2508.18440
- Beat This!: https://github.com/CPJKU/beat_this / beat-this-rs: https://github.com/danigb/beat-this-rs
- LAION-CLAP 音楽版: https://huggingface.co/laion/larger_clap_music / CLAP 系と音色語の一致の比較: https://arxiv.org/abs/2510.14249
- NSynth: https://magenta.tensorflow.org/datasets/nsynth / PANNs: https://zenodo.org/records/3576403
- Instrumental(CMA-ES による音色マッチング): https://arxiv.org/html/2603.15905 / cmaes: https://github.com/pengowen123/cmaes
- 距離尺度の比較(Salimi ら 2025): https://arxiv.org/abs/2506.22628 / auraloss: https://github.com/csteinmetz1/auraloss
- Surge XT 向け flow matching(Hayes ら 2025): https://arxiv.org/abs/2506.07199
- Synplant 2 の Genopatch: https://soniccharge.com/forum/topic/2589-how-was-synplant-2s-ml-trained
- 音声理解 LLM: https://huggingface.co/Qwen/Qwen3-Omni-30B-A3B-Captioner / https://github.com/OpenMOSS/MOSS-Music / Gemini の音声入力: https://ai.google.dev/gemini-api/docs/audio
- All-In-One: https://github.com/mir-aidj/all-in-one
