# 改善項目の一覧(2026-09-25 の全体調査)

UI・機能・性能について全体を調査し、改善項目を洗い出した結果です。番号は調査時の報告と同じです。
状態が「済」になったら、対応したコミットを書き足します。

- 工数: S = 半日以内 / M = 1〜2 日 / L = それ以上
- 根拠のファイル:行は調査時点のもの
- 計測はサンドボックス(Linux VM)。画面は GPU なしの headless Chrome + Tauri のモックで測ったので実機より重めに出る。
  大規模の条件は 25 トラック・200 小節・約 4 万ノート・履歴 3,000 件

## 進め方

1. 安定化(1〜8)
2. 体感性能(9〜15)
3. 音質(16〜19)
4. UI の小粒な改善(S のもの)
5. AI 連携(30〜33)
6. 機能・Godot・配布

## 全体の所見

- エンジンの CPU 負荷には余裕がある(実プロジェクトで平均 3〜7%、p99 12% 以下。64 トラックでも 12% 前後)。
  性能より、音の抜け・切れ・余韻の長さといった品質の問題が目立つ
- 体感の重さの原因は、ピアノロールの巨大な canvas、編集ごとの全体の取り直し、範囲を無視した analyze_audio の 3 つ
- 問題がなかった点: レンダラの経路は固定容量の Vec・アトミック・ArcSwap だけで動く / 重い解析はスナップショットを
  取ってから spawn_blocking / ML の計画は一度だけ作って使い回し / HTTP MCP は loopback の Host 検証あり /
  リスナー・タイマーの解除漏れなし / 変更イベントの 80ms の合流は機能している

---

## 1. 最優先: 壊れる・失われる

| # | 内容 | 根拠 | 改善案 | 工数 | 状態 |
|---|---|---|---|---|---|
| 1 | 不正な値のコマンド 1 件でセッションのアクターが止まり、以後は編集も保存もできない(再現済み) | `Tick` の加算(glaux-core `time.rs:42`)が debug で桁あふれ panic。`add_clip {start: 18446744073709550000}` → `split_clip` で再現。`actor.rs:327` のループに panic の受け止めがない。release では黙って一周し、曲の終わりが遠くなって `export.rs` のレンダがメモリを使い果たしうる | `apply` で tick の上限(24 時間分)を検証し `checked_add` を使う。`actor_loop` で `catch_unwind` してエラーを返し、動き続ける | S | 済(1: 安定化) |
| 2 | 保存の途中で落ちると undo 履歴が丸ごと退避される | `store.rs:169-181` が history に追記してから project.json を書く(間で落ちると不一致 → `store.rs:121-126` で `.orphan` へ)。末尾の行が途中で切れていても全体を失敗扱い。compaction(`store.rs:222-241`)は base を先に書くので、そこで落ちると重複 ID で退避。fsync なし。`assets.rs:164-168` の `import_wav` は最終名へ直に書く。`from_json` 後の `validate()` なし | history を先行ログとして扱い、再現できれば project.json を直す。末尾の壊れた行は捨てる。base に「最初の残すエントリ ID」を記録。`sync_all`。WAV は一時ファイル + rename | M | 済(1: 安定化) |
| 3 | 同時発音が全トラック合計 64 声で頭打ち。超えたノートは無音(ボイススティールなし) | `render.rs:23`(`MAX_VOICES=64`)、`render.rs:958`(満杯なら発音しない)。20 トラック × 4 声で後ろの 4 トラックが無音。ClassicTest で推定 135 / 2456 ノート(5.5%)が鳴っていない。ボイス 1 個 8.3KB のうち 8KB は pluck の弦バッファ | 上限を 256 程度に。pluck のバッファを別プールに。満杯なら「リリース中で最も小さい → 最も古い」を 2〜5ms のフェードで奪う | M | 済(1: 安定化) |
| 4 | 音量スライダーを触った直後は Ctrl+Z / Space / Delete が効かない(実ブラウザで確認) | `App.svelte:254-258`、`Timeline.svelte:720`、`PianoRoll.svelte:1263` がフォーカスが INPUT なら全部無視(type=range も INPUT) | 無視は文字入力の欄(text / number / textarea / contenteditable / select)だけに | S | 済(1: 安定化) |
| 5 | 保存の失敗が画面に出ない。同じ曲を 2 つのプロセスで開くのを防げない | `Mutated.save_error` を Tauri の `apply_edit` / `undo` / `redo` が捨てている(`main.rs:1517-1545` ほか)。Windows では OneDrive やウイルス対策で rename が失敗しがち。二重に開くのは運用の注意だけ(一時ファイル名 `project.tmp` も共有) | rename を再試行。`ProjectChanged` に save_error を載せて UI で警告。プロジェクトフォルダに OS の排他ロック | S | 済(1: 安定化) |
| 6 | `project_version` が undo で戻るので、AI が古い状態を前提に編集しうる | `actor.rs:333` の `version = history.len()`。undo → 別の編集で同じ番号に戻る | 変更のたびに単調に増える `revision` を全応答に付ける(解析キャッシュのキーにも使う) | S | 済(1: 安定化) |
| 7 | `transport_state` を 2 か所から読み、入力メーターと DSP 負荷の値を奪い合う | `main.rs:1613-1628` の `take_input_peak_db()` / `take_stats()` は読むとリセット。App(`App.svelte:242-250`)と設定パネル(`SettingsPanel.svelte:85-93`、`113-124`)が別々にポーリング | ポーリングは App の 1 か所にし、共有ストアで配る | S | 済(1: 安定化) |
| 8 | Windows でチャットを停止しても AI の子プロセスが残り、編集を続けうる(推定。実機で要確認) | `chat.rs` の cancel は `cmd.exe` にだけ `start_kill`。node.exe が残ると stdout が閉じない | プロセスツリーごと止める(`taskkill /T /F` か Job Object) | S〜M | 済(Windows 実機で要確認) |

## 2. 体感性能

| # | 内容 | 計測値 | 改善案 | 工数 | 状態 |
|---|---|---|---|---|---|
| 9 | ピアノロールの canvas がクリップ全体の大きさで、停止中も描き直し続ける(`PianoRoll.svelte:287-301`、`526-538`) | 64 小節で 15000×1750 が 2 枚(1 枚約 105MB)。開くまで 1.1 秒、停止中 CPU 39〜50%(8 小節なら 0.3%) | canvas を見えている大きさにして、見えている範囲だけ描く。動的層は再生ヘッド・カーソル・ドラッグが変わったときだけ描く | 当面 S / 本命 M | 済(2: 体感性能) |
| 10 | 編集 1 回ごとにプロジェクトと履歴を全部取り直し、全クリップのプレビューを描き直す(`App.svelte:114-126`、`ClipPreview.svelte:13`) | 大規模で 1 回 96〜131ms(最大 907ms)、canvas 481 枚。受け渡し 2.3MB + 0.5MB | `get_history` に limit。`ProjectChanged.changes` で変わった部分だけ取り直す。`ClipPreview` は内容が同じなら描かない | M(limit は S) | 一部済(履歴の件数を絞り、プレビューは中身が同じなら描かない。変わった部分だけの取得は未) |
| 11 | `analyze_audio` が範囲を指定しても曲全体をレンダ。per_track は直列、結果のキャッシュなし(`analyze.rs:128-146`、`329-345`) | ClassicTest の 1 小節で 20 秒、per_track 31.5 秒 | 範囲の少し前から終わり + 余韻だけレンダ。per_track を並列(コア数 − 2)。revision をキーにキャッシュ | M | 済(結果のキャッシュは不要と判断) |
| 12 | 開発版(dev プロファイル)で外部クレートが最適化されていない(`Cargo.toml`) | ebur128 が debug 5,138ms / release 77ms。basic-pitch 60 秒で 6.8 秒 | `[profile.dev.package."*"] opt-level = 2` と glaux-core の指定 | S | 済 |
| 13 | SoundFont のゾーンが同じ波形を複製(`sf2.rs:119`)。書き出し・解析・render_note が毎回 WAV と SF2 を読み直す(`main.rs:1582`、`server.rs:1271`、`sound.rs:118`) | ClassicTest で波形 683MB(共有すれば 74MB)、RSS +797MB。読み直し 1 回 222ms、書き出し中の RSS 963MB | (フォント, sample_id, 範囲)→ `Arc` のキャッシュでプリセット間で共有。エンジンの bank を clone して渡す | S〜M | 済 |
| 14 | `get_project` / `get_history` の応答が巨大で、同じ内容が text と structuredContent に 2 重に入る(`server.rs:884`、rmcp `CallToolResult::structured`) | ノート 1 万で 621K 文字・通信 1.36MB。get_history は limit なしで 3,000 件 884K 文字 | `clip_ids` と tick 範囲の指定、ノートの列形式。大きい応答は text のみ。get_history は既定の limit と `target_count` だけの要約 | M | 済 |
| 15 | ノート編集の計算量がノート数の 2 乗(`apply.rs:441/463/472/475/493/515` の入れ子の線形探索) | 1 万ノートの update_notes 222ms(release)/ 621ms(debug)、remove_notes 270 / 932ms。その間アクターが止まる | `HashMap<NoteId, usize>` / `HashSet`。1 回の add_notes 内の重複 ID も検出 | S | 済 |

2 の効果(同じ条件で再計測): ピアノロールの canvas 26.3MPx×2 → 1.8MPx、CPU 停止中 39〜50% → 0.1%・再生中 36〜44% → 13.5%、
ズーム 278 → 95ms / 編集 1 回の再取得と再描画 96〜131 → 67ms(canvas の再描画 481 → 0 枚)/ ClassicTest の 1 小節の analyze_audio
20.1 → 0.33 秒、per_track 31.5 → 0.48 秒 / SoundFont の曲の解析のピークメモリ約 290MB(以前は bank だけで 797MB)/
get_project の応答 約 24.5 万字(2 重)→ 12.3 万字、compact で 7.1 万字 / 2 万ノートの一括編集 0.02 秒

## 3. 音の品質

| # | 内容 | 根拠 | 改善案 | 工数 | 状態 |
|---|---|---|---|---|---|
| 16 | SoundFont の decay・release が仕様の 9〜11 倍長い。subtractive だけ release の意味が違う | `multi.rs:423,432` が `t` を時定数として扱う(仕様は 100dB 変化する時間)。strings は離鍵後 -60dB まで 11.2 秒、organ は 8 秒で強制打ち切り。`subtractive.rs:202,211` も時定数(既定 0.2 秒で解放まで 1.79 秒)、fm / wavetable は「その時間で -60dB」 | SF2 は `exp(-ln(1e5)/(t*sr))`。subtractive の定義を統一(ParamSpec の説明も)。`finished` はピーク比 -80dB | S〜M | 済(subtractive は release だけ揃え、decay は説明を実際の長さに。上限を 8 秒に) |
| 17 | 再生中の編集・つまみ・ミュートで全ボイスが切れ、途中の音も戻らない(既知) | `render.rs:616,620` が差し替え・シークで `voices.clear()`、`start < pos` のノートは鳴らし直さない | トラックごとのイベント列 + 楽器種のハッシュが同じならボイスを維持。変わったトラックだけフェード。またいでいるノートを途中から鳴らし直す | M | 済 |
| 18 | オートメーションが階段状。書き出しでは 85ms 刻み | `render.rs:787-845` が処理単位の頭で 1 回だけ評価。書き出しの `BLOCK=4096`(`export.rs:57`) | 書き出しのブロックを 1024 に。本命は 64〜128 サンプルのサブブロックで評価 | S / M | 済(128 フレームごとに評価) |
| 19 | マスターの tanh が常にかかる / リバーブの長さが 48kHz 固定 / 補間が線形 / 書き出しにディザなし | `render.rs:1555`(-6dBFS で -0.7dB)、`effects.rs:166,184`、`multi.rs:398`・`sampler.rs:139`・`render.rs:1066`、`export.rs:161` | 一定値以上だけに効くソフトクリップ / Freeverb 標準 + サンプルレート換算 / 3 次 Hermite / TPDF ディザ + 24bit・32f | 各 S | 済(リバーブは長さの換算のみ。Freeverb 標準への拡張は未) |

## 4. 操作性(UI)

| # | 内容 | 根拠 | 改善案 | 工数 | 状態 |
|---|---|---|---|---|---|
| 20 | タイムラインを縦スクロールするとルーラーとセクション行が消える | `Timeline.svelte:1647-1651`、`1720-1734` に sticky がない | `sticky; top` と z-index | S | 済 |
| 21 | タイムラインに横ズームがない | `Timeline.svelte:32` の `PX_PER_WHOLE = 96` 固定(200 小節で 19,200px) | Ctrl+ホイールでカーソル位置を保って拡大縮小、「全体表示」 | M | 未 |
| 22 | 編集の失敗が黙って捨てられ、別の場所では `alert()` | `.catch(() => {})` が Timeline 22 か所・App 4 か所、`PianoRoll.svelte:854-860` は console.error だけ。`alert()` が Timeline に 8 か所 | 共通のトーストを作り、`api.applyEdit` を包む 1 か所で表示 | S〜M | 済 |
| 23 | 音作りのスライダーはドラッグ中に音も数値も変わらない。周波数が線形目盛り | `SoundDesignPanel.svelte:709-716` が onchange だけ。`sliderStep`(:166-170)が `range.skew` を無視 | skew に対応し数値を即時表示。ドラッグ中は履歴に載せないプレビュー値を送り、離したら `set_param` 1 回 | S + M | 一部済(対数目盛りと即時表示。ドラッグ中に音へ反映する処理は未) |
| 24 | 何が取り消されるか・やり直せるかが見えない | `App.svelte:861-862` のボタンは常に押せる。`actor.rs:514` は適用済みしか返さない | `get_history` に redoable、ボタンにラベルのツールチップ、無いときは無効 | S〜M | 済 |
| 25 | メニューが画面端ではみ出し Esc で閉じない。狭い画面でヘッダーの ⚙ が隠れる、下部パネルでピアノロールが狭い | `Timeline.svelte:2100-2112`。ヘッダーの必要幅 1306px | 共通の ContextMenu(画面内に収める・max-height・Esc)。ヘッダーの一部を「…」へ、下部パネルの折りたたみ | S〜M | 済(下部パネルの折りたたみと高さの上限も) |
| 26 | ツール名の日本語表示が 36 個中 9 個で、2 か所に別々の定義 | `App.svelte:104-112`、`ChatPanel.svelte:66-76` | `lib/toolLabels.ts` に全ツールをまとめる | S | 済 |
| 27 | ピアノロールに Ctrl+A・↑↓(移調)がない。トラック名・色の変更と複製、クオンタイズのボタンがない | `PianoRoll.svelte:1258-1330`、`Timeline.svelte:1578-1595` | Ctrl+A / ↑↓ / Shift+↑↓ / Alt+←→ / Ctrl+D / Q。トラック名のダブルクリック編集、色、複製 | S | 一部済(キー操作・名前・色・複製。クオンタイズのボタンは未) |
| 28 | チャット: 実行中に次の指示を書けない、再起動で表示が消える、最下部へ強制スクロール、素のテキスト表示 | `ChatPanel.svelte:307`、`:103-106` | 実行中も入力可、ログを `cache/` に保存して復元、下端付近だけ自動スクロール、Markdown | M | 未 |
| 29 | ピアノロールの色がテーマに追従しない / 固定の色が散在 / 内部 ID を常に表示 / アクセシビリティ | `PianoRoll.svelte:326,361,365,446-482,508` の琥珀色固定。`Timeline.svelte:1249`、`HistoryPanel.svelte:82` の ID。絵文字ボタンに aria-label なし、フォーカス表示なし | CSS 変数を読んで描く、色トークン、ID はツールチップへ、aria-label と :focus-visible | 各 S | 一部済(色のトークン・ピアノロールの色・ID・aria-label・フォーカス表示。SVG アイコン化は未) |

## 5. AI 連携

| # | 内容 | 根拠 | 改善案 | 工数 | 状態 |
|---|---|---|---|---|---|
| 30 | 指示文が 3 か所に重複し、毎ターン約 2 万トークン。内容の食い違いあり。Codex の引数の上限まで残り約 1,000 字 | `chat.rs` の SYSTEM_PROMPT(約 6,100 字)、`server.rs:2438` の instructions、`server.rs:948-1004` の apply_commands の説明。amp の gain の目安が `server.rs:2447` と `chat.rs:321` で違う、メタルの歪みの記述、import_sample の「WAV の絶対パス」 | 定石は `get_guide {topic}` か MCP の resource に移し、システムプロンプトは 1,500 字以内に。文字数を見張るテスト | M | 済(定石は get_guide へ。CORE 875 字・チャット約 1,400 字、長さをテストで見張る。apply_commands の説明の短縮は未) |
| 31 | まとめて編集できるツールがない(クリップの複製・小節の挿入と削除)。ノート ID を AI が 1 個ずつ作る | `model/clip.rs:58` の `Note.id` が必須 | `duplicate_clips`、`insert_bars` / `delete_bars`(Batch 1 件)、add_notes の id を省略可能に(MCP 層で採番)、簡潔なノート記法 | M | 済(簡潔なノート記法は未) |
| 32 | AI の 1 ターン分をまとめて取り消せない。AI が変えた箇所を画面で強調しない | `HistoryPanel.svelte:27-38` は 1 件ずつ。Timeline / PianoRoll に author の参照なし。作者名は接続元の名前(`server.rs:785-791`) | ターン ID を履歴に付け「このターンを取り消す」。直近ターンの変更を縁取り。作者名にモデル名 | M | 一部済(ターン単位の取り消しと縁取り。作者名にモデル名は未) |
| 33 | 差分の取得と、クリップ単位・範囲単位の取得がない(既知: HANDOFF §6) | `server.rs:30-42`。チャットの文脈はラベルの羅列で最大 10 件(`main.rs:1735-1765`) | `get_project` に clip_ids と tick 範囲、`get_changes {since_revision}` | M | 済(get_changes。get_project の絞り込みは 14 で済) |
| 34 | AI の変更をワンクリックで聴く手段がない | MCP に再生のツールなし | チャットの返答に「▶ 変更した範囲を聴く」。MCP に `preview`。画像を返す `render_view` | M | 未 |
| 35 | Claude と GPT の品質を比べる評価シナリオがない | Codex は模擬サーバーとの結合テストだけ | 10 題ほどのシナリオをヘッドレスで両方に回し、呼び出し回数・失敗・時間・調性を記録 | L | 未 |

## 6. 機能・Godot・導入

| # | 内容 | 根拠 | 改善案 | 工数 | 状態 |
|---|---|---|---|---|---|
| 36 | MIDI ファイル(.mid)の読み込みと書き出し | .mid を扱うコードがない | `midly`。テンポ・拍子・トラック・GM プログラムを移す。MCP にも | M | 済(トラック追加欄の「MIDI ファイル」・書き出し画面の MIDI・MCP の import_midi / export_midi) |
| 37 | トラックを音声にする(フリーズ)・トラックごとの書き出し(既知: HANDOFF §5) | Godot で CLAP のトラックが鳴らない件の本命の対策。今の回避手順(ソロで WAV)ではマスターの処理が 2 回かかる | マスター前で焼き込み、音声トラックに置いて元をミュート(1 件の履歴)。MCP の `bounce_track` と共通 | M | 済(トラックのメニューと MCP の bounce_track。トラックごとの書き出しは 38 で済) |
| 38 | 書き出しの選択肢・リミッター・MCP からの書き出し | `export_project_wav`(`main.rs:1567-1590`)は 48kHz・16bit 固定、範囲は曲全体、保存先固定 | 44.1k / 48k、16 / 24 / 32f、範囲、ラウドネス目標、FLAC。`limiter` エフェクト。MCP の `export_audio` | M | 一部済(形式・範囲・ラウドネス目標とピークのリミッタ・ステム・MCP の export_audio。FLAC とリアルタイムの limiter エフェクトは未) |
| 39 | セクションマーカーを人間が追加・名前変更・移動できない | `Timeline.svelte:1106-1124` は表示だけ | ルーラーの右クリックで追加、ダブルクリックで名前、ドラッグで移動(`set_sections`)。帯のクリックで範囲選択 | S | 済(ルーラーの右クリックで追加、帯のクリックで範囲選択・ダブルクリックで名前・ドラッグで移動・右クリックで削除) |
| 40 | ミキサー画面・レベルメーター・エフェクトの並べ替え(`move_effect` は既知の未実装) | パンはオートメーションのレーンからだけ。`transport_state` にトラックのメーターなし | 下部パネルに「ミキサー」タブ。トラックごとのピークをアトミックに。`move_effect` コマンド | L | 未 |
| 41 | キーとコードの表示 / 音声クリップのフェードと音量 / 曲の途中のテンポ変更 | harmony はあるが UI に表示なし。`gain_db` / `fade_*_ms` を操作する UI なし。BPM は先頭だけ(`App.svelte:613`) | コードの行とキーの表示、スケール外を暗く / フェードのハンドル / ルーラーの右クリックでテンポ変更 | S〜M | 一部済(キーをヘッダー、コードをルーラーに表示、ピアノロールでスケール外の行を暗く。フェード・途中のテンポ変更は未) |
| 42 | Godot: 書き出したゲームで曲を読めない可能性が高い(既知)/ ループ・展開の切り替え・トラック音量がない(既知)/ Linux・macOS 版がない | `song.rs` が `project.json` と WAV を直接読む。`player.rs` に loop・トラック音量なし。ビルドは `build_windows.bat` だけ | `EditorExportPlugin` で生のまま同梱 / `set_loop`・`queue_section`・`set_track_volume_db` / `build_linux.sh` と CI | M〜L | 一部済(書き出しプラグインで .pck に曲を同梱。ループ・展開の切り替え・Linux/macOS は未) |
| 43 | 配布: Releases と CI / 初回起動の体験 / 英語の README | `.github/` なし。最初は空の曲が開くだけで、CLI が無いことは送信して初めて分かる | タグで Windows 版と Godot の zip を Releases へ、PR ごとにテスト。初回のチェック画面・SoundFont の取得・デモ曲。README.en・GETTING_STARTED・TROUBLESHOOTING | M | 未 |

## 7. 優先度が中〜低のもの

エンジン
- 楽器が毎サンプル powf・tan・exp を計算(`subtractive.rs:233,274,276`、`wavetable.rs:358,370,381`、`drum.rs:120-177`)。
  制御レート(32 サンプル)にすれば 2〜2.5 倍速い。subtractive 22.6ns / unison7 53ns / wavetable unison7 69ns(1 ボイス・1 サンプル)
- CLAP を停止中・無音でも毎ブロック処理(`render.rs:745`、`plugin.rs:775,787` の is_constant 固定・Sleep を見ない)
- 出力バッファ 1024 固定(21ms)で、ライブ MIDI はブロック頭にまとめて発音(`output.rs:712`、`render.rs:727`)。256〜512 を既定に
- 書き出しを全曲メモリに溜める(`export.rs:68`、437 秒で 168MB)。書き出しが 1 スレッド(ClassicTest 437 秒で 19.5 秒)
- 試聴・ライブのボイスが楽器データの最後の参照になり、オーディオスレッドで巨大な解放が起きうる(`render.rs:258-283`、`output.rs:128`)
- ARM でデノーマル対策が効かない(`render.rs:133-145`)、コールバックの panic でプロセスが止まる
- テンポ変化が多いと再生データの構築が遅い(`time.rs:137` の線形走査 → 二分探索)

バックエンド
- CLAP の状態が 1 件約 99KB × 2(forward と inverse)で履歴に積まれる。TestSong では履歴の 70%。compaction は件数でしか働かない
- undo / redo / revert_to のたびに history.jsonl を全部書き直す(`store.rs:197`。3,000 件で MCP の undo 55ms)
- 非同期ハンドラの中で重い処理(`server.rs:1933-2090` の import / transcribe、Tauri の import・record_stop)→ `spawn_blocking`
- 起動時にエンジンと再構築を同期で行う(`main.rs:1885-1905`)、CLAP プラグインを毎回プロセス内で走査(`plugins.rs:231-235`)
- basic-pitch の窓が逐次(`glaux-ml/src/lib.rs:140-167`)、Beat This! が 30 秒未満で毎回計画を作り直す(`beats.rs:151-161`)
- project.json を整形出力(`model/mod.rs:162-164`。ノート 1 万で 1.69MB、整形なしなら 0.62MB)

UI
- 小節線が「トラック × 小節」の DOM(`Timeline.svelte:1274-1276`、24 トラック・200 小節で 5,050 個)
- 再生状態のオブジェクトを毎回丸ごと差し替え、setInterval の中の async が重なりうる(`App.svelte:242-250`)
- 音作りビューとオートメーションレーンが編集のたびにパラメータを取り直し、古い応答で上書きされうる
- 巨大なコンポーネント(Timeline 2,153 行・PianoRoll 1,820 行・App 1,401 行)と、3 か所に分かれたキー操作

## 計測値(抜粋)

| 項目 | 値 |
|---|---|
| エンジン負荷(実プロジェクト、平均 / p99) | CyberNeon 6.0% / 11%、CyberTechno 4.6% / 8.7%、ClassicTest 3.3% / 7.1% |
| ボイス 1 個(ns/サンプル) | subtractive 22.6 / fm 12.9 / pluck 6.6 / sampler 4.6 / sf2 piano 12.7 |
| エフェクト 1 個(ns/サンプル) | eq 9.8 / comp 11.1 / reverb 14.6 / amp 35.5 / tape 31.4 |
| 書き出し | 実時間の 16〜32 倍 |
| 再生データの構築 | 65k ノートで 5.4ms(差分構築は不要) |
| MCP(debug、ノート 1 万) | get_project 167ms・621K 文字、apply 1 音 31〜35ms、undo 55ms、起動 216ms |
| 画面(大規模) | 初回表示 0.6 秒、DOM 8,895 個、編集 1 回の再描画 96〜131ms |
