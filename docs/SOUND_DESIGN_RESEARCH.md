# 音作りとエフェクト: 高機能な DAW との違い(2026-10-02)

Glaux の音源・エフェクト・ルーティングを、最新の主要 DAW と、プロが足しているプラグインや手法と比べた調査です。
目的は、Glaux の内蔵の音を「プロの音作り」に近づけるために、何が足りないのか、どこから手を付けると効くのかを整理することです。

- 比べた DAW(2026-10 時点の最新版): Ableton Live 12.4、Logic Pro 12.3、Bitwig Studio 6.1、FL Studio 2026、Cubase 15、
  Fender Studio Pro 8(Studio One の後継)、REAPER 7.81、Pro Tools 2026.4
- Glaux の現状は、コード(crates/glaux-dsp・glaux-engine・glaux-clap・glaux-mcp)で確かめた
- DAW や製品の情報は Web の一次情報・専門誌で確かめた。確かめられなかったものは「未確認」と書いた
- 信号処理の改善の経緯は [DSP_RESEARCH.md](DSP_RESEARCH.md)、AI の道具の全体は [CAPABILITIES.md](CAPABILITIES.md) を参照

---

## 1. 要旨

- **差の中心は「動き」と「広がり」**: 内蔵音源の声はモノラルです(ユニゾンも左右に広がらない)。音源の中の変調は
  ADSR 1 本と限られた LFO だけです。プロの音色は、声ごとのステレオ、ドリフト(揺らぎ)、MSEG やマクロによる止まらない変調で
  「生きた音」にしています。Glaux の音が平板に聞こえる一番の理由はここにあると考えられます。
- **合成方式の幅は、軽量な DAW としては十分**: 減算・ウェーブテーブル・FM・撥弦の物理モデル・ドラム合成・マルチサンプラー
  (SF2 / SFZ)の 7 種があります。Live・Logic・Bitwig に比べて足りないのは、グラニュラー・加算/再合成・
  モジュラー環境・自作のウェーブテーブルの取り込みです。
- **エフェクトの種類は多いが、1 つ 1 つの「深さ」が足りない**: 25 種あり、True Peak リミッター・ダイナミック EQ・
  マルチバンド・FDN リバーブ・畳み込み・共鳴抑制・LUFS メーターまで持っています。一方、次のものはありません。
  - EQ のバンド数は 5 に固定で、M/S やリニアフェーズはない
  - コンプのキャラクター(VCA / FET / Opto)と先読み
  - ディレイのテンポ同期とディレイの種類
  - 歪み系のオーバーサンプリング(一部)
  - ボーカル系(ピッチ補正・ディエッサー・ゲート)
- **ルーティングとホストの穴**:
  - バスからバスへ送れない(グループを組めない)
  - CLAP 音源やバスをサイドチェインのキーにできない
  - **CLAP プラグインにテンポなどの演奏状態(トランスポート)を渡していない**(テンポ同期するプラグインが合わない)
  - CLAP のパラメータ変調・ポリフォニック変調を使っていない
- **AI の音作りは、どの DAW よりも進んでいる面がある**: 目標の音につまみを合わせる match_sound、言葉で追い込む
  refine_by_words、master_mix などは主要 DAW にありません。文章から音色を作る機能を自前の音源に持つ DAW は無く、
  Synplant 2 の PhenoType(2026-06)のような単体製品が出始めた段階です。ParamSpec(つまみの聴感の説明)と
  MCP を持つ Glaux は、ここで独自の強みを作れます。

---

## 2. Glaux の現状(コードで確かめたもの)

### 2.1 音源(7 種)

| 音源 | 方式 | 主なつまみ・特徴 | 足りないもの |
|---|---|---|---|
| subtractive | 減算。saw / square(PolyBLEP)・triangle・sine | ユニゾン 1〜7 声、デチューン、サブ、ノイズ 3 色、TPT SVF ローパス、ADSR、filter_env | フィルタはローパス 1 種、エンベロープ 1 本(フィルタと共用)、LFO なし、ベロシティは音量だけ |
| wavetable | 2048 点 × 16 枚 × 5 種、11 段のミップマップ | position、pos_env、position 専用のサイン LFO、ユニゾン 7 声、SVF ローパス、ADSR | テーブルの取り込み、フィルタエンベロープ、LFO の行き先 |
| fm | 2 オペレーター + 自己フィードバック | ratio、index と専用の減衰、ベロシティで深さ | 3 オペレーター以上、フィルタ、折り返し対策 |
| pluck | Karplus-Strong の拡張 | decay、brightness、pick、palm_mute を物理で | 胴鳴り(ボディ)、弦の種類 |
| drum | GM 配置の合成(modern / 808 / 909) | キック・スネア・ハットの音程・減衰・パンチ・スナッピー | 部品ごとの細かい作り込み |
| sampler | 単一サンプル、Hermite 補間、帯域制限の縮小版 | root、release、gain | ループ、フィルタ、ADSR、ステレオ再生 |
| sf2 / sfz | マルチサンプル(1 音に最大 8 ゾーン) | 範囲・ループ・エンベロープ・LFO・ローパス、SFZ のラウンドロビンと乱数 | CC の変調、リリーストリガー、キースイッチ |

共通の仕組み:
- 音ごとにピッチカーブ・ビブラート・音量カーブ・明るさカーブを描ける。奏法は 8 種(palm_mute・staccato・accent・vibrato・bend・legato・portamento ほか)
- 層(set_layer、本体 + 3 層)、マクロ(set_macro、8 個 × 16 先。再生前に値やオートメーションへ焼き込む)
- 同時発音は全体で 256 声(奪った声のフェード用に 128 枠)
- **声はモノラル**(左右差は層のパンと、ステレオの音声クリップだけ)

### 2.2 エフェクト(25 種)

| 分類 | Glaux のエフェクト |
|---|---|
| EQ | eq(SVF の 5 バンド固定: HP・低域シェルフ・中域ベル・高域シェルフ・LP)、dynamic_eq(1 バンド、外部キー可) |
| ダイナミクス | compressor(ピーク / RMS、ソフトニー、検出側ハイパス、先読みなし)、sidechain(ダッカー)、multiband(LR4 の 3 帯域)、transient、limiter(True Peak、先読み約 1.1ms)、volume_shaper |
| 歪み | distortion(tanh を ADAA)、amp(2 段のプリアンプ + キャビネットの近似、2 倍オーバーサンプリング)、tape(2 倍オーバーサンプリング)、clipper(soft / hard / fold)、bitcrush、virtual_bass |
| 空間 | reverb(8 本の FDN、room / plate)、convolution(分割 FFT、IR の取り込み)、delay(ms 指定・ピンポン・ダッキング)、chorus、width(M/S、低域のモノ化、デコリレーション) |
| 変調系 | phaser、flanger、tremolo / オートパン、trance_gate、auto_filter(LFO か入力音量) |
| 補正 | resonance(STFT の共鳴抑制) |

- オーバーサンプリングは amp と tape だけ、ADAA は distortion だけ。clipper・bitcrush・fm には折り返し対策が無い
- 変調系はテンポ同期できる(4/1〜1/16、付点・3 連)が、delay の時間は ms だけ

### 2.3 ルーティング・変調・品質

- トラックは MIDI / 音声 / バス。センド(プリ / ポスト)はトラックからバスへだけ。全トラックがマスター直結で、出力先は選べない
- エフェクトのつながり(fx_links)は分岐・合流・線ごとの音量を持つ有向グラフ
- オートメーション
  - 音量とパン: サンプルごと
  - 内蔵の音源・エフェクトのつまみ: 128 フレームごと
  - CLAP のつまみ: 64 サンプルごと
- LFO の変調(modulate): トラックに 8 本まで。中身は折れ線のオートメーションへの焼き込みで、声ごとの変調やリトリガーはない
- 遅延補正(PDC): CLAP と内蔵エフェクトの遅れを揃える(分岐の枝ごと・マスターのエフェクトは対象外)
- 内部処理は f32。最後にマスターのソフトクリップ
- メーター: LUFS(M / S / I)・True Peak・相関・1/3 オクターブのスペクトル、聴く機器の切り替え(スマホ・ノート PC)

### 2.4 CLAP ホスト

- 音源とエフェクトの両方を使え、パラメータ・状態・プリセットの読み込み・遅延・ノート表現(音程・音量・明るさ)に対応
- GUI は Windows だけ
- **トランスポート(テンポ・拍子・再生位置)を渡していない**
- パラメータ変調イベント・ポリフォニック変調・MPE は使っていない

### 2.5 AI の音作りの道具(主要 DAW に無い部分)

- analyze_sound / compare_sounds: 音色の記述子(包絡・倍音・スペクトル・言葉のラベル)と 2 音の距離
- match_sound / fit_instrument: 内蔵音源のつまみを、目標の音に CMA-ES で合わせる
- find_similar_presets / refine_plugin_params: CLAP プリセットから近いものを探し、つまみを追い込む
- refine_by_words: 「暖かく」「刺さらない」などの音色語に近づくよう、エフェクトのつまみを動かす
- master_mix / compare_mix / analyze_audio: マスタリングの組み立て、編集の前後の比較、曲全体の測定
- ステム分離、音声から MIDI への変換(単音・和音)

---

## 3. 最新の DAW の音源

### 3.1 DAW ごとの目玉

- **Ableton Live 12.4**: Wavetable(音声ファイルを波形表として取り込める)、Operator(4 オペレーター FM、
  倍音を描く加算)、Drift、Meld(2 系統の MPE シンセ)、Granulator III(MPE で粒を操る)、Collision・Tension・Electric
  (物理モデリング)。Sampler / Simpler / Drum Sampler。ラックに 16 のマクロとバリエーション、全デバイスの A/B 比較(12.3)。
  似た音色の検索、サンプルの自動タグ、Splice 統合。ラウンドロビンは標準で持たない。付属は Suite で 71GB 以上。
- **Logic Pro 12.3**: Alchemy(加算・スペクトル・グラニュラー・サンプラー・VA・フォルマントを 4 系統でモーフィング、
  MSEG)、Sculpture(物理モデリング)、ES2 ほか 28 種の音源。Sampler・Quick Sampler・Drum Machine Designer。
  Session Players(ドラム・ベース・鍵盤に加え、12.0 で Synth Player)、Chord ID、Stem Splitter。付属は約 72GB。
- **Bitwig Studio 6.1**: The Grid(230 以上のモジュール、内部 4 倍オーバーサンプリング、全部ステレオ)、
  Polymer、Phase-4(位相変調)。6.1 で Sampler を刷新(Spectral の伸縮、1 声 256 粒のグラニュラー、スライス)。
  **モジュレーター 42 種**(MSEG 5 種を含む)を、どのつまみにも声ごとに付けられる。MPE とノートごとの表情。
- **FL Studio 2026**: Sytrus(6 オペレーター)、Harmor(加算 516 倍音、再合成、画像から音)、Kepler(低エイリアス)、
  FLEX の刷新、Patcher。AI のアシスタント Gopher(操作の代行)、Loop Starter。
- **Cubase 15**: Retrologue 2、Padshop 2(グラニュラー・スペクトル)、HALion Sonic、Drum Machine、Pattern Editor。
  **モジュレーター 12 種**(Pro、1 トラック 8 枠、どのつまみにも)。Omnivocal(歌声合成、ベータ)、AI ステム分離。
- **Fender Studio Pro 8**: Mai Tai・Presence(16 枠の変調行列)、Sample One、Impact。音声→ノート、和音の提案、Moises 統合。
- **Pro Tools 2026.4 / REAPER 7**: 音源は最小限(Pro Tools は Massive X Player を同梱、REAPER は ReaSynth と
  ラウンドロビン対応の ReaSamplOmatic)。どちらも音作りはプラグイン任せの設計。

### 3.2 音源の機能の比較(○ = 標準で持つ、△ = 限定的、× = なし、? = 未確認)

| 機能 | Live | Logic | Bitwig | FL | Cubase | Fender SP | **Glaux** |
|---|---|---|---|---|---|---|---|
| VA 減算(フィルタの種類が豊富) | ○ | ○ | ○ | ○ | ○ | ○ | △(LP 1 種) |
| ウェーブテーブル + 自作波形の取り込み | ○ | ○ | ○ | △ | △ | × | △(取り込みなし) |
| FM / 位相変調 | ○ | ○ | ○ | ○ | △ | ? | △(2 オペレーター) |
| 加算・スペクトル・再合成 | △ | ○ | △ | ○ | ○ | × | × |
| グラニュラー | ○ | ○ | ○ | △ | ○ | × | × |
| 物理モデリング | ○ | ○ | △ | △ | × | × | △(撥弦のみ) |
| モジュラー環境 | △ | × | ○ | △ | × | × | × |
| マルチサンプル + ベロシティ | ○ | ○ | ○ | ○ | △ | ○ | ○(SF2 / SFZ) |
| ラウンドロビン | △ | ? | ? | ? | ? | ? | ○(SFZ) |
| スライス・サンプラー内の伸縮 | ○ | ○ | ○ | ○ | ○ | ○ | × |
| どのつまみにも効く汎用モジュレーター | ○ | × | ○ | △ | ○ | × | △(焼き込みの LFO) |
| MSEG(描く変調) | ○ | ○ | ○ | ○ | ○ | ? | × |
| MPE / 音ごとの表情 | ○ | ○ | ○ | ? | ○ | ? | △(音ごとの曲線は描ける。MPE 入力なし) |
| マクロ・A/B 比較 | ○ | ○ | ○ | △ | ○ | ○ | △(マクロのみ) |
| 似た音色の検索 | ○ | △ | ? | △ | △ | △ | ○(CLAP プリセット・match_sound) |
| 文章から音色 | × | × | × | × | × | × | △(refine_by_words はエフェクトだけ) |

出典: ableton.com(Live 12.2〜12.4 の公開情報・エディション比較)、help.ableton.com(ラウンドロビン)、support.apple.com、
kvraudio.com・synthanatomy.com・soundonsound.com(Logic Pro 12)、bitwig.com・downloads.bitwig.com(6.0 リリースノート)、
image-line.com(FL Studio 2026)、steinberg.help・musicradar.com(Cubase 15)、soundonsound.com(Fender Studio Pro 8・Pro Tools 2026.4)、
reaper.fm。

---

## 4. 最新の DAW のエフェクト・ミックス

### 4.1 DAW ごとの目玉

- **Live 12.4**: EQ Eight(M/S、2 倍オーバーサンプリング)、Glue Compressor、Roar(3 段のサチュレーション、直列・並列・
  マルチバンド・M/S・フィードバックの経路)、Hybrid Reverb(畳み込み + アルゴリズム)、Limiter の作り直し(True Peak)、
  Auto Shift(ピッチ補正)。Audio Effect Rack で並列とマクロ。LUFS メーターは標準に無い。
- **Logic Pro**: Channel EQ・Linear Phase EQ・**Match EQ**、1 台で 7 回路(VCA・FET・Opto など)を切り替えるコンプ、
  ChromaGlow(AI で機材の質感)、Space Designer・ChromaVerb・Quantec Room Simulator、Delay Designer(26 タップ)、
  Loudness Meter・MultiMeter、**Mastering Assistant**、Stem Splitter、Atmos のレンダラー。
- **Bitwig 6**: Compressor+(6 キャラクター)、ハード機を手本にした EQ 3 種、マルチバンドのクリッパー Over、
  Spectral Suite、FX Grid。どのつまみにもモジュレーター、オートメーションのクリップ化。
- **Cubase 15**: Frequency 2(8 バンド、バンドごとにダイナミック・リニアフェーズ・外部サイドチェイン)、Squasher、
  Imager、Magneto、**SuperVision**(18 モジュールのメーター)、UltraShaper、PitchShifter、Atmos と Ambisonics。
- **Fender Studio Pro 8**: Pro EQ3(ダイナミック、バンドのソロ、自動ゲイン補正)、Mix Engine FX(コンソールの
  エミュレーション)、OpenAIR2、Voice FX、Atmos。
- **REAPER 7**: 64bit 処理、エフェクトごとのオーバーサンプリング(最大 768kHz)、エフェクトをまとめるコンテナ、
  True Peak の ReaLimit、トラックごとの LUFS、パラメータごとの LFO と音声での制御。
- **FL Studio 2026**: Maximus、Emphasis、Soft Clipper、Wave Candy(LUFS・True Peak)、Transmitter
  (トランジェントとサステインに分けて別に処理)、AI マスタリング。
- **Pro Tools 2026.4**: Channel Strip、Pro シリーズのダイナミクス、Atmos・360 Reality Audio、ARA(Melodyne など)。

### 4.2 エフェクトの機能の比較

| 機能 | Live | Logic | Bitwig | FL | Cubase | Fender SP | REAPER | **Glaux** |
|---|---|---|---|---|---|---|---|---|
| ダイナミック EQ | × | × | × | ? | ○ | ○ | ? | △(1 バンド) |
| リニアフェーズ EQ | × | ○ | × | ? | ○ | △ | △ | × |
| M/S 処理 | ○ | ○ | ○ | ? | ○ | ? | ○ | △(width だけ) |
| マッチ EQ | × | ○ | × | × | ? | × | ? | △(master_mix が参照に寄せる) |
| マルチバンドコンプ | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ○(3 帯域) |
| コンプのキャラクター(VCA / FET / Opto) | △ | ○ | △ | ? | ○ | ○ | × | × |
| True Peak リミッター | ○ | ? | ? | ? | ○ | ? | ○ | ○ |
| クリッパー | ○ | △ | ○ | ○ | △ | ? | ○ | △(折り返し対策なし) |
| テープ・真空管のサチュレーション | ○ | ○ | ○ | ○ | ○ | ○ | △ | ○(tape・amp) |
| オーバーサンプリング | △ | ? | ○ | ? | ? | ? | ○ | △(amp・tape だけ) |
| 畳み込みリバーブ | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ○ |
| スペクトル・グラニュラー | ○ | △ | ○ | △ | △ | ? | △ | △(共鳴抑制だけ) |
| リアルタイムのピッチ補正 | ○ | ○ | ? | ○ | ○ | ○ | ○ | × |
| 並列のラック・コンテナ | ○ | △ | ○ | ○ | △ | ○ | ○ | ○(fx_links) |
| モジュレーター → 任意のつまみ | △ | △ | ○ | ○ | ○ | ? | ○ | △(焼き込み) |
| VCA・グループ | × | ○ | ? | ? | ○ | ○ | ○ | × |
| AI ステム分離 | ○ | ○ | ? | ○ | ○ | ○ | ? | ○ |
| AI マスタリング | × | ○ | × | ○ | × | × | × | ○(master_mix) |
| LUFS メーター | × | ○ | × | ○ | ○ | ○ | ○ | ○ |
| 相関・位相 | × | ○ | ? | ○ | ○ | ? | ○ | ○ |
| Atmos レンダラー | × | ○ | × | × | ○ | ○ | ? | × |

出典: ableton.com(Live 12.3)、synthanatomy.com、recordingmag.com、soundonsound.com(Roar)、audeobox.com・
support.apple.com・production-expert.com・synthtopia.com(Logic Pro)、bitwig.com、image-line.com・audiotechnology.com
(FL Studio)、kvraudio.com・steinberg.help(Cubase 15・Frequency 2)、s1manual.presonus.com(Studio One / Fender Studio Pro)、
reaper.fm、production-expert.com・soundonsound.com(Pro Tools)。

---

## 5. プロの音作り: 道具と手法

### 5.1 プロが足しているプラグイン(2025〜2026)

- **シンセ**:
  - Serum 2(2025-03): オシレーター 3 基が wavetable・multisample・granular・spectral を選べる、フィルター 2 基を直列・並列、カオス LFO。CLAP 版は無い。
  - Pigments 6〜7: Modal(物理モデリング)、声ごとに違う値を出す変調、S 字のアンプエンベロープでクリックを減らす。
  - Phase Plant: モジュール式。
  - Diva: 回路モデリング。
  - Zebra 3(2026): スプラインのオシレーター、105 応答のフィルター。
  - Omnisphere 3(2025-10): 全パッチ共通の Tone・Ambience などのつまみ、Patch Mutations、ドリフト。
  - Massive X 1.6〜1.7: Morpher・Animator。
- **サンプラーとライブラリ**: Kontakt 8(MIDI の Tools)、Spitfire(2025-04 に Splice が買収)、Orchestral Tools、
  Superior Drummer 3(マイク 11 系統)。マイク位置のミックス・ベロシティレイヤー・ラウンドロビン・リリースサンプルで
  生楽器らしさを出す。
- **エフェクト**:
  - FabFilter: Pro-Q 4(ダイナミック・スペクトルダイナミクス)、Pro-C 3(14 方式)、Pro-L 2、Saturn 2。
  - iZotope: Ozone 12(Stem EQ・Unlimiter・Master Assistant)、Neutron 5(Mix Assistant・Unmask)。
  - 補正・自動の調整: soothe2(共鳴)、Gullfoss(自動の帯域バランス)。
  - 歪み・空間: Soundtoys(Decapitator・EchoBoy)、Valhalla のリバーブ。
  - 実機のモデリング: UAD・Waves(1176・LA-2A・Pultec)。
  - ボーカル: Melodyne。
  - 比較: Metric AB(参照曲との比較)。
- **ギター**: Neural DSP Archetype(プリ FX・アンプ・IR のキャビネット・ポスト FX を 1 本に)。

### 5.2 「プロっぽい音」の要素

- **レイヤリング**: 役割(アタック・胴・空気感)ごとに重ね、層ごとに帯域を分ける。ベースはサブ(サイン、モノ)と
  歪ませた中高域に分ける
- **止まらない音**: マクロ・MSEG・ランダム・声ごとのばらつき・ドリフトで、音色が常に少しずつ動く
- **ステレオ**: ユニゾンを左右に広げ、低域はモノにする。モノに畳んだときに抜けないか確かめる
- **トランジェントとエンベロープ**: クリックの無い立ち上がりと、芯のあるアタック
- **倍音**: サチュレーションで小さなスピーカーでも聞こえる倍音を足す
- **リサンプリング**: 一度音声にして、切る・逆再生・グラニュラーで再加工する(EDM のベース)
- **バス処理とゲインステージング**: ドラム・シンセのバスでまとめて圧縮し、各段の入力レベルを揃える

定番の手順の例:
- **スーパーソー**: ノコギリ波 7 声、デチューン 25〜40%、広がり 60〜80%。1 オクターブ上を -6dB で重ね、
  ホールリバーブと付点 4 分のピンポンディレイを足す
- **キックとベース**: サイドチェイン、またはダイナミック EQ でキックの帯域だけベースを数 dB 下げる。キックの音程をキーに合わせる
- **ボーカル**: 補正 → ハイパス → 減算 EQ → コンプ → ディエッサー → サチュレーション → 加算 EQ
- **空間**: リバーブとディレイはセンドで使い、返りの帯域を絞ってプリディレイを置く

### 5.3 AI を使う音作りの流れ

- **音から音色**: Synplant 2 の Genopatch(参照音に近いシンセの設定を推測し、編集できるパッチとして出す)
- **文章から音色**: Synplant 2 の PhenoType(2026-06)。LLM を使わず、タグと同義語で文を読み、否定も扱い、ローカルで動く
- **文章から音声**: Stable Audio Open Small(CPU でも動く短い音)
- **似た音の検索**: Splice の Search with Sound / Create a Stack、Live 12 の Similarity Search と似たサンプルへの差し替え
- **変種の自動生成**: Omnisphere 3 の Patch Mutations、Massive X の Morpher
- **AI のミックス・マスタリング**: Ozone 12、Neutron 5、RoEx Automix(32 ステムまで)、Logic の Mastering Assistant
- **演奏の生成**: Logic の Session Players、FL Studio の Gopher・Loop Starter

### 5.4 プラグインの規格

- **CLAP**: 15 のホスト、約 394 本のプラグイン、最新は 1.2.10(2026-07)。
  - 対応しているホスト: Bitwig・REAPER・FL Studio。Studio One は一部の機能だけ。
  - まだ対応していないホスト: Cubase(予定)、Live・Logic・Pro Tools。
  - 強み: 声ごとの非破壊のパラメータ変調とノート表現。
- **VST3**: SDK 3.8(2025-10)で MIT ライセンスになり、オープンソースの DAW でも組み込める。Serum 2 のように CLAP の無い
  定番は VST3 で使われる
- **MPE**: 主要なシンセの標準になりつつある

出典: xferrecords.com・synthanatomy.com・musictech.com(Serum 2)、synthanatomy.com・soundonsound.com(Pigments)、
gearnews.com・musicradar.com(Zebra 3)、kvraudio.com(Omnisphere 3・Cubase 15・VST3 の MIT 化・PhenoType)、
fabfilter.com、izotope.com、soundonsound.com(Synplant 2)、stability.ai、support.splice.com、help.ableton.com、
roexaudio.com、en.wikipedia.org(CLAP)、spectral-colors.com。

---

## 6. 差の整理

「音への効き」は、プロらしさにどれだけ効くかの見立てです(大・中・小)。

| 領域 | Glaux の今 | プロの DAW・道具 | 音への効き |
|---|---|---|---|
| 声のステレオ | 声はモノラル、ユニゾンも中央 | ユニゾンを左右に広げる、声ごとのパン | **大** |
| 揺らぎ・ばらつき | なし(同じ音は毎回同じ) | ドリフト、声ごとのランダム、アナログのばらつき | **大** |
| 音源の中の変調 | ADSR 1 本、wavetable の position LFO だけ | 複数のエンベロープ・LFO・MSEG・変調行列、ベロシティとキーで音色が変わる | **大** |
| フィルタ | ローパス 1 種(SVF) | LP / HP / BP / ノッチ、ラダー・回路モデル、フィルタの歪み、フィルタ 2 基 | **大** |
| トラック全体の変調 | LFO をオートメーションに焼き込む(8 本) | どのつまみにも声ごとのモジュレーター(Bitwig 42 種・Cubase 12 種) | 中 |
| 合成方式 | 減算・WT・FM(2op)・撥弦・ドラム・マルチサンプラー | + グラニュラー・加算/再合成・モジュラー・自作 WT | 中 |
| サンプラー | 単一サンプルはワンショットだけ。SFZ は充実 | ループ・フィルタ・エンベロープ・スライス・伸縮・リサンプリング | 中 |
| EQ | 5 バンド固定 + 1 バンドのダイナミック | 8 バンド以上・バンドごとのダイナミック・M/S・リニアフェーズ・マッチ | 中 |
| コンプ | 1 種類の特性、先読みなし | 回路のキャラクター、先読み、上向き圧縮 | 中 |
| 歪みの品質 | amp・tape は 2 倍 OS、distortion は ADAA、clipper・fm は対策なし | 4 倍以上のオーバーサンプリングが標準 | 中 |
| 空間 | FDN(room / plate)、畳み込み、ディレイ(ms 指定) | 多数のアルゴリズム、シマー、テンポ同期のディレイ、マルチタップ | 中 |
| ボーカル | なし | ピッチ補正、ディエッサー、ゲート、ハーモナイザー | 小〜中(歌を扱うなら大) |
| ルーティング | バスへのセンドだけ、出力先固定 | グループ・バスからバス・VCA・どこからでもサイドチェイン | 中 |
| CLAP ホスト | トランスポートなし、変調イベントなし、GUI は Windows | トランスポート・声ごとの変調・ノート表現・全 OS の GUI | **大**(プラグインで作る音が合わない) |
| 内部精度 | f32 | 64bit が多い(REAPER) | 小 |
| メーター | LUFS・TP・相関・スペクトル | 同等 + ゴニオメーター・参照曲の比較 | 小 |
| AI の音作り | match_sound・refine_by_words・master_mix | 演奏の生成・ステム分離・マスタリング支援。文章からの音色はまだ単体製品のみ | Glaux が先行 |

---

## 7. 改善の方向(案)

内蔵で持つべきものと、プラグインに任せてよいものを分けて考えます。
大容量のサンプルライブラリ(オーケストラ・生ドラム)、個性の強い定番シンセ、アンプシミュは、CLAP(と将来の VST3)の
ホストを良くして外の製品を使う方が、利用者にとっても良いはずです。
内蔵は「どの曲にも要る土台」と「AI が操作できること」に集中します。

### 7.1 音源の「生きた音」(効きが大きい)

1. **ステレオの声**:
   - ユニゾンの声を左右に振り分ける(spread のつまみ)
   - 層のパンを声ごとに
   - sampler をステレオで鳴らす
   - あわせて低域のモノ化(width の考え方を音源の中にも)
2. **揺らぎ**: 声ごとのデチューンのばらつき、ゆっくりした音程・カットオフのドリフト、位相の乱数。つまみ 1 つ(analog)で
3. **音源の中の変調を増やす**:
   - フィルタエンベロープを独立させる
   - LFO を 2 本にし、行き先を選べるようにする(音程・カットオフ・音量・パン・position)
   - ベロシティとキーで音色が変わるように(ベロシティ → カットオフ・アタック、キートラッキング)
4. **フィルタの種類**: HP・BP・ノッチ、ラダー(4 極)、フィルタ前の歪み(drive)
5. **小さな品質の穴**:
   - triangle の帯域制限
   - fm の折り返し対策(オーバーサンプリング)
   - clipper のオーバーサンプリングか ADAA

### 7.2 変調の仕組み

- 今の modulate(オートメーションへの焼き込み)に加えて、エンジンの中で動く**声ごとのモジュレーター**
  (LFO・エンベロープ・MSEG・ランダム)と、つまみへの割り当て(変調行列)を持つ
- マクロを CLAP のつまみにも効くように(CLAP のパラメータ変調イベントで)

### 7.3 CLAP ホスト(外の音を正しく鳴らす)

- **トランスポートを渡す**(テンポ・拍子・再生位置・再生中か)。テンポ同期する LFO・アルペジエーター・ディレイが合うように
- パラメータ変調イベントとポリフォニック変調、MPE の入力
- Linux / macOS の GUI
- VST3 のホスト(SDK が MIT になったので検討の余地がある)

### 7.4 エフェクトの深さ

- EQ: バンド数を可変に、バンドごとのダイナミック動作、M/S、スペクトル表示(リニアフェーズは後回しでよい)
- コンプ: キャラクター(VCA・FET・Opto の検出と曲線)、先読み
- ディレイ: テンポ同期の時間、テープ・BBD・マルチタップの種類
- サチュレーション: テープ・真空管・トランジスタを選べる 1 台(オーバーサンプリング付き)
- リバーブ: hall・chamber・shimmer のアルゴリズム、変調
- 補正: ディエッサー、ゲート / エキスパンダ。ピッチ補正とハーモナイザーは、歌を扱うときに

### 7.5 ルーティング

- トラックの出力先をバスに(グループ)、バスからバスへの送り
- サイドチェインのキーを CLAP 音源・バスからも
- マスターのエフェクトと、分岐の枝ごとの遅延補正

### 7.6 AI と組み合わせて差を付ける(Glaux 独自の方向)

- **文章から音色**: 内蔵音源の ParamSpec(つまみの聴感の説明)と音色語の対応を使い、「暗くて太いベース、少し揺れる」
  から内蔵音源のパッチを作る(PhenoType と同じく、結果は編集できるパッチ)。refine_by_words を音源のつまみにも広げる
- **共通の高い階層のつまみ**: Omnisphere 3 の Tone・Ambience のような、どの音色にも効くマクロを標準で持ち、AI が
  「もう少し明るく・遠く」を 1 つのつまみで指示できるようにする
- **変種の自動生成**: 今の音色から少しずつ違う変種を作り(Patch Mutations)、analyze_sound で違いを数値で示す
- **定番の手順を道具に**: スーパーソー・キックとベースのすみ分け・ボーカルのチェーン・センドのリバーブを、
  プリセットや道具として呼べるようにする(今の出荷時プリセット 19 個の延長)

### 7.7 プラグインに任せてよいもの

- 大容量のサンプルライブラリ、個性の強い定番シンセ(Serum 2・Diva・Zebra 3 など)、アンプシミュ、
  Atmos などの立体音響、高度な修復(RX のような)

---

## 8. 確かめきれなかったこと

- 各 DAW の表の「?」の項目(FL Studio のダイナミック EQ、Logic の Adaptive Limiter の True Peak、
  多くの DAW のラウンドロビンなど)
- Bitwig 6 の「AI がモジュレーションを提案する」という報道(リリースノートには記載が無い)
- Vital 2 の有無、Phase Plant 3 の時期、NI の文章から音色を作る製品化
- 付属ライブラリの容量(Bitwig・Fender Studio Pro)
