//! アプリ内チャット: UI からの指示をヘッドレスの AI(`claude` または `codex`)に渡す。
//!
//! 相手は [`Provider`] で切り替える。Claude(Claude Code)の場合の仕組み:
//! - 送信ごとに `claude -p <指示> --output-format stream-json` を起動する。
//!   認証はホストの Claude Code ログインをそのまま使う(API キー不要)。
//! - その Claude には `--mcp-config` でこのアプリ自身の HTTP MCP サーバーだけを渡し
//!   (`--strict-mcp-config`)、glaux ツールを `--allowedTools mcp__glaux` で自動許可する。
//!   編集は従来どおり MCP → Session アクター経由なので、タイムライン反映・
//!   AI インジケータ・履歴ハイライトはそのまま機能する。
//! - 会話の継続は `--resume <session_id>`。stream-json の init イベントから
//!   session_id を拾って保持する。
//! - stdout の NDJSON を [`parse_line`] で UI 向けイベントに変換し、Tauri イベント
//!   `chat-event` として送る。
//!
//! GPT(Codex CLI)の場合は `codex exec --json` を起動し、同じアプリ内 MCP サーバーを
//! `-c mcp_servers.glaux.url=...` で渡す([`codex_args`])。会話の継続は `codex exec resume <thread_id>`、
//! 出力の JSONL は [`parse_codex_line`] で同じ UI イベントに変換する。認証はホストの Codex のログインを使う。

use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

/// UI(チャットパネル)に流すイベント。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    /// セッション開始(claude プロセスの初期化完了)
    Started,
    /// アシスタントの発話テキスト(メッセージ単位で届く)
    AssistantText { text: String },
    /// ツール呼び出し(名前は `mcp__glaux__` プレフィックスを剥がしたもの)
    ToolUse { name: String },
    /// ターン完了。`text` は最終応答の全文
    Result { ok: bool, text: String },
    /// 補足情報(会話のフォールバックなど。エラーほど深刻ではない)
    Notice { text: String },
    /// 起動失敗・異常終了など
    Error { message: String },
}

/// チャットの相手(起動する CLI)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Provider {
    /// Claude Code(`claude`)
    #[default]
    Claude,
    /// OpenAI の Codex CLI(`codex`)。GPT のモデルで動く
    Codex,
}

impl Provider {
    /// UI から届く名前を解釈する(未指定は Claude)。
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        match s.unwrap_or("") {
            "" | "claude" => Ok(Provider::Claude),
            "codex" => Ok(Provider::Codex),
            other => Err(format!("チャットの相手が不正です: {other}")),
        }
    }

    /// 画面に出す名前
    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "GPT",
        }
    }

    fn command_name(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    /// 1 行を UI イベントに変換する。会話(セッション)の ID を拾ったら返す。
    pub fn parse_line(self, line: &str) -> (Vec<ChatEvent>, Option<String>) {
        match self {
            Provider::Claude => parse_line(line),
            Provider::Codex => parse_codex_line(line),
        }
    }
}

/// 保存する会話 ID の書式。Claude は ID のみ(以前からの形式)、Codex は `codex:<ID>`。
fn encode_session(provider: Provider, id: &str) -> String {
    match provider {
        Provider::Claude => id.to_owned(),
        Provider::Codex => format!("codex:{id}"),
    }
}

fn decode_session(raw: &str) -> (Provider, String) {
    match raw.strip_prefix("codex:") {
        Some(id) => (Provider::Codex, id.to_owned()),
        None => (Provider::Claude, raw.to_owned()),
    }
}

/// stream-json の 1 行を UI イベントに変換する。session_id を拾ったら返す。
pub fn parse_line(line: &str) -> (Vec<ChatEvent>, Option<String>) {
    let v: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(_) => return (vec![], None),
    };
    let session_id = v["session_id"].as_str().map(str::to_owned);

    let events = match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => vec![ChatEvent::Started],
        Some("assistant") => {
            let mut out = Vec::new();
            if let Some(blocks) = v["message"]["content"].as_array() {
                for block in blocks {
                    match block["type"].as_str() {
                        Some("text") => {
                            if let Some(t) = block["text"].as_str() {
                                if !t.trim().is_empty() {
                                    out.push(ChatEvent::AssistantText { text: t.to_owned() });
                                }
                            }
                        }
                        Some("tool_use") => {
                            if let Some(name) = block["name"].as_str() {
                                let name = name.strip_prefix("mcp__glaux__").unwrap_or(name);
                                out.push(ChatEvent::ToolUse {
                                    name: name.to_owned(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            out
        }
        Some("result") => {
            let ok = !v["is_error"].as_bool().unwrap_or(false)
                && v["subtype"].as_str() == Some("success");
            let text = v["result"].as_str().unwrap_or_default().to_owned();
            vec![ChatEvent::Result { ok, text }]
        }
        _ => vec![],
    };
    (events, session_id)
}

/// `codex exec --json` の 1 行(JSONL)を UI イベントに変換する。thread_id を拾ったら返す。
///
/// 主な行: `thread.started` / `turn.started` / `item.started`・`item.completed`(item.type が
/// `agent_message` / `mcp_tool_call` / `command_execution` / `error` など)/ `turn.completed` /
/// `turn.failed` / `error`(再接続中などの途中経過。致命的なら続けて turn.failed が来る)。
pub fn parse_codex_line(line: &str) -> (Vec<ChatEvent>, Option<String>) {
    let v: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(_) => return (vec![], None),
    };
    let item = &v["item"];
    match v["type"].as_str() {
        Some("thread.started") => (
            vec![ChatEvent::Started],
            v["thread_id"].as_str().map(str::to_owned),
        ),
        Some("item.started") => {
            let name = match item["type"].as_str() {
                Some("mcp_tool_call") => item["tool"].as_str(),
                Some("command_execution") => Some("command_execution"),
                Some("web_search") => Some("web_search"),
                _ => None,
            };
            let events = name
                .map(|n| ChatEvent::ToolUse { name: n.to_owned() })
                .into_iter()
                .collect();
            (events, None)
        }
        Some("item.completed") => {
            let events = match item["type"].as_str() {
                Some("agent_message") => item["text"]
                    .as_str()
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| ChatEvent::AssistantText { text: t.to_owned() }),
                // 設定の警告など。古い Codex が知らない設定を無視した旨は出さない
                Some("error") => item["message"]
                    .as_str()
                    .filter(|m| !m.contains("unrecognized configuration"))
                    .map(|m| ChatEvent::Notice { text: m.to_owned() }),
                _ => None,
            };
            (events.into_iter().collect(), None)
        }
        Some("turn.completed") => (
            vec![ChatEvent::Result {
                ok: true,
                text: String::new(),
            }],
            None,
        ),
        Some("turn.failed") => (
            vec![ChatEvent::Result {
                ok: false,
                text: v["error"]["message"]
                    .as_str()
                    .unwrap_or("GPT の実行に失敗しました")
                    .to_owned(),
            }],
            None,
        ),
        Some("error") => (
            v["message"]
                .as_str()
                .map(|m| ChatEvent::Notice { text: m.to_owned() })
                .into_iter()
                .collect(),
            None,
        ),
        _ => (vec![], None),
    }
}

/// TOML の基本文字列にする(`-c key=value` の値)。JSON の文字列のエスケープは TOML でもそのまま通る。
fn toml_string(s: &str) -> String {
    serde_json::Value::String(s.to_owned()).to_string()
}

/// `codex exec` に渡す引数(指示本文は stdin で `-` として渡す)。
///
/// - `--ignore-user-config`: ユーザーの config.toml(ほかの MCP サーバー・承認設定など)を読まない。
///   Claude の `--strict-mcp-config` に相当。認証(ログイン)は読む
/// - ファイルの読み書きやコマンド実行はさせない(読み取り専用のサンドボックス + シェル系の機能を切る)。
///   Glaux のツールだけを承認なしで呼べるようにする(`approval_policy = "never"` だと既定では MCP も拒否される)
/// - システムプロンプトは `developer_instructions`(開発者の指示)として渡す
pub fn codex_args(mcp_url: &str, resume: Option<&str>, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into()];
    if resume.is_some() {
        args.push("resume".into());
    }
    for a in ["--json", "--skip-git-repo-check", "--ignore-user-config"] {
        args.push(a.into());
    }
    let configs = [
        "sandbox_mode=\"read-only\"".to_owned(),
        "approval_policy=\"never\"".to_owned(),
        "web_search=\"disabled\"".to_owned(),
        "features.shell_tool=false".to_owned(),
        "features.unified_exec=false".to_owned(),
        format!("mcp_servers.glaux.url={}", toml_string(mcp_url)),
        "mcp_servers.glaux.default_tools_approval_mode=\"approve\"".to_owned(),
        format!("developer_instructions={}", toml_string(SYSTEM_PROMPT)),
    ];
    for c in configs {
        args.push("-c".into());
        args.push(c);
    }
    if let Some(m) = model {
        args.push("-m".into());
        args.push(m.to_owned());
    }
    if let Some(id) = resume {
        args.push(id.to_owned());
    }
    args.push("-".into());
    args
}

/// チャットの実行状態。`AppState` が保持する。
pub struct ChatManager {
    mcp_url: String,
    /// 現在のプロジェクトフォルダ(プロジェクト切り替えで変わる)
    project_dir: Mutex<String>,
    /// 続けている会話(相手と ID)。相手を切り替えたら新しい会話にする
    session: Mutex<Option<(Provider, String)>>,
    /// 次のターンの相手
    provider: Mutex<Provider>,
    /// チャットの AI に最後に見せた履歴エントリ ID。
    /// これ以降の人間の編集を次のターンでコンテキストとして注入する
    last_seen_entry: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
    running: AtomicBool,
    /// 使うモデル(`claude --model` / `codex -m` に渡す)。None なら各 CLI の既定
    model: Mutex<Option<String>>,
}

/// `--model` に渡してよい値か(英数字と . _ - [ ] のみ。別オプションの注入を防ぐ)。
pub fn valid_model_name(m: &str) -> bool {
    !m.is_empty()
        && m.len() <= 64
        && !m.starts_with('-')
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '[' | ']'))
}

const SYSTEM_PROMPT: &str = "あなたは DAW『Glaux』に組み込まれた作曲アシスタントです。\
    プロジェクトの閲覧・編集は必ず glaux MCP ツール(get_project / apply_commands / undo / \
    checkpoint / revert_to / get_history など)で行い、project.json などのファイルを直接読み書き\
    しないでください。\
    重要: このプロジェクトは人間(ユーザー)も UI から並行して編集します。あなたの会話の記憶は\
    古くなっている可能性があるため、各レスポンスの project_version と履歴(get_history)を信頼し、\
    編集の前には必要に応じて get_project で最新状態を確認してください。\
    ノートの移調・時間移動・クオンタイズ・ベロシティ一括調整には専用ツール\
    (transpose_notes / shift_notes / quantize_notes / scale_velocity)があります。\
    「1 オクターブ上げて」「1 拍後ろに」のような相対編集は、自分で update_notes を\
    組み立てるよりこれらを使う方が速くて確実です。\
    音源: トラックには set_device で内蔵楽器(subtractive=シンセ全般 / drum=ドラム / \
    pluck=撥弦の物理モデル / fm=FM シンセ: エレピ・ベル・マレット・金属的な音 / wavetable=ウェーブテーブル: position を LFO やオートメーションで動かすウォブルベース・\
    うねるパッド・母音のような音・sync のギラついたリード)を設定でき、\
    list_params でパラメータの意味・範囲・現在値を確認して set_param で音作りができます。\
    ドラムのトラックには必ず drum を設定してください。\
    本物っぽい楽器一式(ピアノ・ストリングス・ブラス・ギター等)は SoundFont が使えます: \
    list_soundfonts で置いてある .sf2 とプリセットを確認し、set_soundfont_instrument で設定します。\
    .sf2 が 1 つも無ければ「FluidR3_GM.sf2 などのフリー SoundFont を設定の SoundFont フォルダに\
    置いてください」とユーザーに案内してください。\
    実録の音(録音済みの WAV)を鳴らしたいときは import_sample ツールでトラックの音源を \
    sampler にできます(path は WAV の絶対パス、root にサンプルの実音を指定)。\
    ギター・ベース・ハープなど「弾く弦」の音は pluck が第一候補です。\
    エレキギターの音は pluck 単体では「アンプに繋いでいない生弦」なので、\
    必ず amp エフェクト(アンプシミュレータ)を後段に入れます: gain_db 〜12 でクリーン、\
    24 前後でクランチ、40 以上でメタル。刻みはさらにノートに palm_mute。\
    出荷時プリセット(クリーンエレキ / クランチギター / メタルギター)の load_preset が早道です。\
    ジャンル表現の道具: EDM の supersaw は subtractive の unison=5〜7 + detune、\
    ハネ・シャッフルは swing_notes(0.667 ≈ 3 連、0.58 で軽く。既存のノリに合わせるなら analyze_rhythm の swing_ratio / 2)。\
    複数のトラックに同じリバーブ・ディレイを掛けたいときは、バス(add_track kind: \"bus\" + リバーブ mix 1.0)を作り、\
    各トラックから set_send で送ります(トラックごとに reverb を挿すより空間がまとまり、軽い)。\
    太いベースは sub、EDM のポンピングは sidechain エフェクト(source にキックのトラック ID、\
    release_ms を 8 分音符の長さ = 60000/BPM/2 に合わせると気持ちよく揺れる)、\
    ギターの歪みやメタルは distortion(square 波 + 高 drive)、\
    やまびこは delay(time_ms = 60000/BPM で 4 分、45000/BPM で付点 8 分)、クリーンギターやエレピの厚みは chorus、\
    Lo-fi Hip Hop やヴィンテージ感は tape(wow・flutter の揺れ + hiss + crackle(レコードのパチパチ)+ bits。ドラムバスやマスターにも)、\
    メタルのブリッジミュートの刻み(ズクズク)はノートに articulation: \"palm_mute\" を付けます\
    (add_notes / update_notes。ほかに staccato / accent、ロングトーンの表情付けに vibrato、\
    ギターソロの決め音に bend = チョーキング(全音下から滑り上がる)。低めの音 + 歪みと組み合わせると効果的)。\
    ストリングス・管・歌・シンセリードのつながったフレーズは、2 音目以降に legato(弾き直さずにつなぐ)、\
    音程を滑らせたいところに portamento(直前の音から約 0.15 秒で滑る。フレーズの頭なら全音下から滑り込む)を付けると、打ち込みっぽさが減ります。\
    滑る時間はトラック全体なら set_param track/glide_ms(ゆったりした弦は 250〜400、速いリードは 50〜80)、\
    1 音だけならノートの glide_ms。つなぎ目の長さは track/legato_ms(既定 30、パッド的にふんわりなら 80〜150)。\
    ビルドアップにはドラムのノート 55(リバースクラッシュ)が使えます。\
    ドラムパターンやリフの繰り返しは、1〜2 小節ぶんを書いて set_clip_loop {id, loop_len} でループにし、\
    resize_clip で伸ばすと速く、後から 1 か所直すだけで全体に反映されます。\
    自由なピッチの動き(ゆっくりしたチョーキング、ダイブ、ポルタメント、うねり)はノートの \
    pitch_curve([{tick, cents}]、tick はノート先頭からの相対、100 cents = 半音、最大 8 点)で描けます。\
    音色プリセット: 良い音ができたら save_preset で保存できます(全プロジェクト共通のライブラリ)。\
    音作りの依頼では、まず list_presets に使える音がないか確認 → load_preset で適用 → 微調整、\
    の順が速くて確実です。ユーザーが「この音を保存して」と言ったら save_preset を使ってください。\
    リズム感: analyze_rhythm でスウィング・グリッド(ストレート/3 連)・シンコペーション・\
    ヒューマナイズ量が分かります。既存の曲にフレーズを足すときは先に呼んで、同じノリで書いてください。\
    曲の構成: sections(set_sections で編集)に intro / Aメロ / サビ等のマーカーを置けます。\
    「サビだけ盛り上げて」のような指示は、まず sections を見て tick 範囲に解決してください。\
    構成が決まってきたら自発的に set_sections で記録しておくと後の指示が正確になります。\
    音楽理論の目: analyze_harmony でキーと小節ごとのコード進行が分かります。\
    メロディやハモリ、ベースラインを足す前に呼んで、スケール音・コードトーンに合った音を選んでください\
    (推定値なので、意図的な転調・借用和音を「修正」しないこと)。\
    耳: analyze_audio で自分の編集結果を数値で聴けます(ラウドネス・帯域バランス・クリップ検出など)。\
    音作りやミックス調整では、編集 → analyze_audio で確認 → 微調整のループを回してください。\
    時間変化するミックス(フェードイン/アウト、ビルドアップの音量カーブ、パンの揺れ)は\
    set_automation_points(target: track/volume_db または track/pan)で描けます。\
    音色の時間変化(フィルタスイープ、EDM のビルドアップで cutoff を開いていく等)も\
    target: device/<パラメータ名>(例 device/cutoff)で同様に描けます。\
    エフェクトのつまみも target: fx/<エフェクト ID>/<パラメータ名> で時間変化させられます(リバーブの mix、EQ の high_gain_db 等)。\
    曲全体のフェードアウトやマスターのエフェクトの時間変化は set_master_automation_points\
    (target: track/volume_db または fx/<マスターのエフェクト ID>/<パラメータ名>)で描けます。\
    音声素材: 人間が ⏺ で録音した演奏や音声ファイル(WAV / MP3 / FLAC / OGG / M4A)は音声トラック(kind: audio)のクリップとして置かれます。\
    それらは get_project で見え、analyze_audio で聴けます。\
    外部の CLAP プラグイン(Surge XT などのシンセ)は list_plugins で一覧でき、set_device {type: \"clap\", plugin_id} で音源にできます。\
    音色の大枠はプラグイン自身の画面で人間が作ります。つまみは list_params(filter で絞り込み。例 \"cutoff\")で探し、set_param {path: \"device/clap:<id>\"} や set_automation_points で動かせます(値はプラグインの単位、current_text が画面の表示)。ピアノロールのピッチカーブもプラグインに届きます。音色の土台は list_plugin_presets(filter・category で絞り込み)で探して load_plugin_preset で読み込めます。プリセットにはつまみで触れない設定(LFO のテンポ同期・モジュレーションの割り当て)も入っているので、動きのある音はまずプリセットから探してください。\
    CLAP のエフェクト(list_plugins の effect: true。Surge XT Effects・Dragonfly Reverb など)も add_effect / add_master_effect に\
    {type: \"clap\", plugin_id} で挿せます。つまみは list_params の effects(path fx/<id>/clap:<番号>、current_text が画面の表示)で見て動かします。\
    取り込んだ曲はパートに分けられます(separate_audio。builtin = 打楽器 / 音程楽器、demucs = ボーカル / ドラム / ベース / その他)。\
    ベースだけ譜起こししたいときは、分離 → transcribe_audio の順に。\
    テンポを変えるときは、音声クリップに set_clip_stretch(follow、original_bpm = 録音時のテンポ)を\
    付けておくと拍がずれずに伸縮します(音程は変わりません)。取り込んだ曲・ループ素材の元のテンポが分からなければ \
    analyze_beats で測れます(bpm、拍子、最初の小節頭)。音声ファイルの配置を頼まれたら import_audio_clip を使います。\
    鼻歌や歌の録音(単旋律)は transcribe_audio で MIDI クリップにできます(ピアノ・ギターの和音や伴奏入りの素材は mode: \"poly\")。MIDI 化したら \
    analyze_harmony でキーを確認し、前後と 12 半音ずれた短い音(オクターブ誤検出)や外れた音を整えてから報告してください。\
    音色を確かめるときは analyze_sound(track_id + pitch でトラックの 1 音、clip_id / file でサンプル)。数値に加えて、\
    words(CLAP というモデルで「聴いた」印象: 楽器らしさ・明るさ・質感・雰囲気の言葉)が返ります。\
    words は目安なので、数値(明るさ・包絡・倍音)と食い違うときは数値を優先してください。\
    「このサンプルに似た音を作って」と頼まれたら: まず analyze_sound で目標を把握し、\
    (1) シンセらしい単音なら match_sound(内蔵 subtractive / fm / wavetable を自動で選んでつまみを合わせる。reverb: true でリバーブも。30 秒ほど)、\
    (2) 複雑な音・生楽器寄りなら、CLAP プラグインのトラックで find_similar_presets(近いプリセットを探す)→ load_plugin_preset\
    → refine_plugin_params(主要なつまみを自動で詰める)、\
    の順に試し、仕上げは compare_sounds(a = 目標、b = トラックの音)の differences を見てつまみ・エフェクトで詰めます。\
    distance が 0.35 未満なら「よく似ている」、0.7 以上は別物です。結果は数値で報告し、最後は人間の耳で確かめてもらってください。\
    人間が自分で試したいときは、音声クリップを右クリック →「この音に似せた内蔵シンセのトラックを作る」\
    「この音に近い CLAP 音源のプリセットを探す」(候補の読み込み → つまみの自動調整まで画面でできる)と案内できます。\
    ミックスバランス: analyze_audio の per_track: true で各トラックのラウドネスと帯域の一覧が取れます。\
    定石: 主役(リード等)は伴奏より 2〜4dB 上に置く / 帯域の重心が被るトラックは EQ で住み分ける /\
    それでも埋もれるなら伴奏側に sidechain。音量を上げる前に、まず被りを削ることを検討してください。\
    セルフレビューの習慣: まとまった編集を終えたら、完了報告の前に必ず自己確認してください。\
    (1) analyze_harmony で調性が意図どおりか、(2) analyze_audio でクリップやバランスの破綻がないか。\
    問題があればその場で修正してから報告し、報告には確認結果(キー・LUFS など)を一言添えます。\
    返答は簡潔な日本語で、行った編集の要点だけ述べてください。";

impl ChatManager {
    /// 次のターンの相手を設定する。
    pub fn set_provider(&self, provider: Provider) {
        *self.provider.lock().expect("provider lock") = provider;
    }

    fn provider(&self) -> Provider {
        *self.provider.lock().expect("provider lock")
    }

    /// 次のターンから使うモデルを設定する(None / 空文字で既定)。
    pub fn set_model(&self, model: Option<String>) -> Result<(), String> {
        let model = model
            .filter(|m| !m.trim().is_empty())
            .map(|m| m.trim().to_owned());
        if let Some(m) = &model {
            if !valid_model_name(m) {
                return Err(format!("モデル名が不正です: {m}"));
            }
        }
        *self.model.lock().expect("model lock") = model;
        Ok(())
    }

    pub fn new(mcp_url: String, project_dir: String) -> Self {
        // 前回のセッション ID があれば読み込み、アプリ再起動をまたいで会話を継続する
        let session = read_cache(&session_file(&project_dir)).map(|s| decode_session(&s));
        if session.is_some() {
            tracing::info!("前回のチャットセッションを再開します");
        }
        let last_seen_entry = read_cache(&last_seen_file(&project_dir));
        ChatManager {
            mcp_url,
            project_dir: Mutex::new(project_dir),
            session: Mutex::new(session),
            provider: Mutex::new(Provider::default()),
            last_seen_entry: Mutex::new(last_seen_entry),
            child: Mutex::new(None),
            running: AtomicBool::new(false),
            model: Mutex::new(None),
        }
    }

    fn project_dir(&self) -> String {
        self.project_dir.lock().expect("project_dir lock").clone()
    }

    /// プロジェクト切り替え。実行中の指示は中断し、新プロジェクトの
    /// 会話キャッシュ(セッション ID / last_seen)を読み込み直す。
    pub fn switch_project(&self, dir: String) {
        self.cancel();
        let session = read_cache(&session_file(&dir)).map(|s| decode_session(&s));
        let last_seen = read_cache(&last_seen_file(&dir));
        *self.project_dir.lock().expect("project_dir lock") = dir;
        *self.session.lock().expect("session lock") = session;
        *self.last_seen_entry.lock().expect("last_seen lock") = last_seen;
    }

    pub fn last_seen_entry(&self) -> Option<String> {
        self.last_seen_entry.lock().expect("last_seen lock").clone()
    }

    /// AI に見せた最新の履歴エントリ ID を記録・永続化する。
    pub fn set_last_seen_entry(&self, id: Option<String>) {
        *self.last_seen_entry.lock().expect("last_seen lock") = id.clone();
        write_cache(&last_seen_file(&self.project_dir()), id.as_deref());
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn has_session(&self) -> bool {
        self.session.lock().expect("session lock").is_some()
    }

    /// 続けている会話の相手(会話が無ければ None)
    fn session_provider(&self) -> Option<Provider> {
        self.session
            .lock()
            .expect("session lock")
            .as_ref()
            .map(|(p, _)| *p)
    }

    /// 今の相手との会話の ID(相手が違えば None = 新しい会話)
    fn resume_id(&self) -> Option<String> {
        let provider = self.provider();
        self.session
            .lock()
            .expect("session lock")
            .as_ref()
            .filter(|(p, _)| *p == provider)
            .map(|(_, id)| id.clone())
    }

    /// 会話の ID を更新し、`cache/chat-session.txt` に永続化する。
    fn set_session_id(&self, provider: Provider, sid: String) {
        let mut guard = self.session.lock().expect("session lock");
        if guard.as_ref() == Some(&(provider, sid.clone())) {
            return;
        }
        *guard = Some((provider, sid.clone()));
        drop(guard);
        write_cache(
            &session_file(&self.project_dir()),
            Some(&encode_session(provider, &sid)),
        );
    }

    /// 会話をリセットする(次の送信が新しいセッションになる)。
    /// `last_seen_entry` は会話ではなく「AI に何を見せたか」の記録なので保持する。
    pub fn reset(&self) {
        *self.session.lock().expect("session lock") = None;
        let _ = std::fs::remove_file(session_file(&self.project_dir()));
    }

    /// 実行中の AI のプロセスを止める。
    pub fn cancel(&self) {
        let child = self.child.lock().expect("child lock").take();
        if let Some(mut child) = child {
            // Windows の npm 版(claude.cmd / codex.cmd)は cmd.exe の子として node.exe が動くので、
            // cmd.exe だけを止めると AI が残って編集を続けてしまう。プロセスツリーごと止める
            #[cfg(windows)]
            if let Some(pid) = child.id() {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &pid.to_string()])
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .status();
            }
            // kill は非同期だが、start_kill で即シグナルだけ送れば十分
            let _ = child.start_kill();
        }
    }

    fn build_command(&self) -> Command {
        // Windows: npm 版は <名前>.cmd、ネイティブ版は <名前>.exe。
        // Rust の Command は .cmd を安全に(引数をエスケープして)起動できる。
        let name = self.provider().command_name();
        #[cfg(windows)]
        let program = format!("{name}.cmd");
        #[cfg(not(windows))]
        let program = name;
        let mut cmd = Command::new(program);
        self.configure(&mut cmd);
        cmd
    }

    #[cfg(windows)]
    fn build_command_fallback(&self) -> Command {
        let mut cmd = Command::new(self.provider().command_name());
        self.configure(&mut cmd);
        cmd
    }

    fn configure(&self, cmd: &mut Command) {
        match self.provider() {
            Provider::Claude => self.configure_claude(cmd),
            Provider::Codex => {
                let model = self.model.lock().expect("model lock").clone();
                cmd.args(codex_args(
                    &self.mcp_url,
                    self.resume_id().as_deref(),
                    model.as_deref(),
                ));
            }
        }
        cmd.current_dir(self.project_dir())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            // コンソールウィンドウを出さない(CREATE_NO_WINDOW)
            cmd.creation_flags(0x0800_0000);
        }
    }

    fn configure_claude(&self, cmd: &mut Command) {
        let mcp_config = json!({
            "mcpServers": { "glaux": { "type": "http", "url": self.mcp_url } }
        });
        // 指示本文は引数ではなく stdin で渡す。「-」で始まる指示がオプション扱い
        // されるのを防ぎ、Windows のコマンドライン長制限も回避できる
        cmd.arg("-p")
            .args(["--output-format", "stream-json", "--verbose"])
            .arg("--mcp-config")
            .arg(mcp_config.to_string())
            .arg("--strict-mcp-config")
            .args(["--allowedTools", "mcp__glaux"])
            .arg("--append-system-prompt")
            .arg(SYSTEM_PROMPT);
        if let Some(sid) = self.resume_id() {
            cmd.args(["--resume", &sid]);
        }
        if let Some(model) = self.model.lock().expect("model lock").as_ref() {
            cmd.args(["--model", model]);
        }
    }

    fn spawn(&self) -> std::io::Result<Child> {
        match self.build_command().spawn() {
            Ok(child) => Ok(child),
            #[cfg(windows)]
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.build_command_fallback().spawn()
            }
            Err(e) => Err(e),
        }
    }
}

/// セッション ID の保存先(`cache/` は再生成可能データ置き場。Git 管理外)。
fn session_file(project_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(project_dir)
        .join("cache")
        .join("chat-session.txt")
}

fn last_seen_file(project_dir: &str) -> std::path::PathBuf {
    std::path::Path::new(project_dir)
        .join("cache")
        .join("chat-last-seen.txt")
}

fn read_cache(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn write_cache(path: &std::path::Path, value: Option<&str>) {
    match value {
        Some(v) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(path, v) {
                tracing::warn!("キャッシュを保存できません({}): {e}", path.display());
            }
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn emit(app: &tauri::AppHandle, event: &ChatEvent) {
    let _ = app.emit("chat-event", event);
}

/// 1 回の指示を実行する。完了(または失敗)までブロックするので、
/// 呼び出し側は tauri::async_runtime::spawn で回すこと。
pub async fn run_turn(app: tauri::AppHandle, mgr: std::sync::Arc<ChatManager>, prompt: String) {
    if mgr.running.swap(true, Ordering::SeqCst) {
        emit(
            &app,
            &ChatEvent::Error {
                message: "前の指示がまだ実行中です".to_owned(),
            },
        );
        return;
    }

    // 相手を切り替えたら新しい会話にする(もう一方の会話の記録は引き継げない)
    let provider = mgr.provider();
    if mgr.session_provider().is_some_and(|p| p != provider) {
        mgr.reset();
        emit(
            &app,
            &ChatEvent::Notice {
                text: format!(
                    "相手を {} に切り替えたので、新しい会話を始めます",
                    provider.label()
                ),
            },
        );
    }
    let had_resume = mgr.has_session();
    let mut saw_init = false;
    let mut result = run_turn_inner(&app, &mgr, &prompt, &mut saw_init).await;

    // 保存していたセッションが claude 側に残っていない場合、init 前に失敗する。
    // その場合だけ新しい会話にフォールバックして 1 回やり直す。
    if result.is_err() && had_resume && !saw_init {
        mgr.reset();
        emit(
            &app,
            &ChatEvent::Notice {
                text: "前回の会話を再開できなかったため、新しい会話で続けます".to_owned(),
            },
        );
        result = run_turn_inner(&app, &mgr, &prompt, &mut saw_init).await;
    }

    mgr.child.lock().expect("child lock").take();
    mgr.running.store(false, Ordering::SeqCst);

    if let Err(message) = result {
        emit(&app, &ChatEvent::Error { message });
    }
}

async fn run_turn_inner(
    app: &tauri::AppHandle,
    mgr: &ChatManager,
    prompt: &str,
    saw_init: &mut bool,
) -> Result<(), String> {
    let provider = mgr.provider();
    let command = provider.command_name();
    let mut child = mgr.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            match provider {
                Provider::Claude => "claude コマンドが見つかりません。Claude Code をインストールして PATH を通してください".to_owned(),
                Provider::Codex => "codex コマンドが見つかりません。Codex CLI をインストールしてください(npm i -g @openai/codex の後、codex login でログイン)".to_owned(),
            }
        } else {
            format!("{command} を起動できません: {e}")
        }
    })?;

    // 指示本文を stdin で渡す(書き終えたら閉じて EOF を伝える)
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| format!("指示の送信に失敗: {e}"))?;
        stdin
            .shutdown()
            .await
            .map_err(|e| format!("指示の送信に失敗: {e}"))?;
    }

    let stdout = child.stdout.take().ok_or("stdout を取得できません")?;
    let mut stderr = child.stderr.take().ok_or("stderr を取得できません")?;
    *mgr.child.lock().expect("child lock") = Some(child);

    // stderr は別タスクで吸っておき、異常終了時のエラーメッセージに使う
    let stderr_task = tokio::spawn(async move {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut got_result = false;
    while let Ok(Some(line)) = lines.next_line().await {
        let (events, session_id) = provider.parse_line(&line);
        if let Some(sid) = session_id {
            mgr.set_session_id(provider, sid);
        }
        for ev in &events {
            match ev {
                ChatEvent::Started => *saw_init = true,
                ChatEvent::Result { .. } => got_result = true,
                _ => {}
            }
            emit(app, ev);
        }
    }

    // プロセス終了を待つ(child は cancel() に取られている可能性がある)
    let status = {
        let child = mgr.child.lock().expect("child lock").take();
        match child {
            Some(mut child) => Some(child.wait().await.map_err(|e| e.to_string())?),
            None => None, // cancel 済み
        }
    };

    match status {
        None => Err("キャンセルしました".to_owned()),
        Some(s) if s.success() || got_result => Ok(()),
        Some(s) => {
            let stderr_text = stderr_task.await.unwrap_or_default();
            let tail: String = stderr_text
                .lines()
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            let hint = match provider {
                Provider::Codex => {
                    "\n(codex login でログインしてあるか、モデル名が正しいかも確認してください)"
                }
                Provider::Claude => "",
            };
            Err(format!("{command} が異常終了しました({s})\n{tail}{hint}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_are_validated() {
        for ok in [
            "opus",
            "sonnet",
            "haiku",
            "claude-opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-3.5",
        ] {
            assert!(valid_model_name(ok), "{ok}");
        }
        for bad in [
            "",
            "--dangerously-skip-permissions",
            "opus sonnet",
            "a;b",
            "x/y",
        ] {
            assert!(!valid_model_name(bad), "{bad}");
        }
    }

    #[test]
    fn parses_init_and_captures_session_id() {
        let (events, sid) =
            parse_line(r#"{"type":"system","subtype":"init","session_id":"abc-123","tools":[]}"#);
        assert_eq!(events, vec![ChatEvent::Started]);
        assert_eq!(sid.as_deref(), Some("abc-123"));
    }

    #[test]
    fn parses_assistant_text_and_tool_use() {
        let line = r#"{"type":"assistant","session_id":"abc","message":{"content":[
            {"type":"text","text":"ベースを追加します"},
            {"type":"tool_use","name":"mcp__glaux__apply_commands","input":{}}
        ]}}"#;
        let (events, _) = parse_line(line);
        assert_eq!(
            events,
            vec![
                ChatEvent::AssistantText {
                    text: "ベースを追加します".to_owned()
                },
                ChatEvent::ToolUse {
                    name: "apply_commands".to_owned()
                },
            ]
        );
    }

    #[test]
    fn parses_result_success_and_error() {
        let (events, _) =
            parse_line(r#"{"type":"result","subtype":"success","is_error":false,"result":"完了"}"#);
        assert_eq!(
            events,
            vec![ChatEvent::Result {
                ok: true,
                text: "完了".to_owned()
            }]
        );

        let (events, _) = parse_line(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":""}"#,
        );
        assert_eq!(
            events,
            vec![ChatEvent::Result {
                ok: false,
                text: String::new()
            }]
        );
    }

    // 以下の Codex の行は codex-cli 0.156.1 の `codex exec --json` の実際の出力から取った
    #[test]
    fn parses_codex_thread_tools_and_messages() {
        let (events, sid) =
            parse_codex_line(r#"{"type":"thread.started","thread_id":"01a0d570-4eae-7923"}"#);
        assert_eq!(events, vec![ChatEvent::Started]);
        assert_eq!(sid.as_deref(), Some("01a0d570-4eae-7923"));

        assert_eq!(parse_codex_line(r#"{"type":"turn.started"}"#).0, vec![]);

        let started = r#"{"type":"item.started","item":{"id":"item_0","type":"mcp_tool_call","server":"glaux","tool":"get_project","arguments":{"include_notes":false},"result":null,"error":null,"status":"in_progress"}}"#;
        assert_eq!(
            parse_codex_line(started).0,
            vec![ChatEvent::ToolUse {
                name: "get_project".to_owned()
            }]
        );
        // 同じ呼び出しの完了は出さない(開始時に 1 回だけ表示する)
        let done = started
            .replace("item.started", "item.completed")
            .replace("in_progress", "completed");
        assert_eq!(parse_codex_line(&done).0, vec![]);

        let msg = r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"ベースを追加しました"}}"#;
        assert_eq!(
            parse_codex_line(msg).0,
            vec![ChatEvent::AssistantText {
                text: "ベースを追加しました".to_owned()
            }]
        );

        let completed = r#"{"type":"turn.completed","usage":{"input_tokens":20,"cached_input_tokens":0,"output_tokens":10}}"#;
        assert_eq!(
            parse_codex_line(completed).0,
            vec![ChatEvent::Result {
                ok: true,
                text: String::new()
            }]
        );
    }

    #[test]
    fn parses_codex_errors_and_warnings() {
        let failed =
            r#"{"type":"turn.failed","error":{"message":"unexpected status 401 Unauthorized"}}"#;
        assert_eq!(
            parse_codex_line(failed).0,
            vec![ChatEvent::Result {
                ok: false,
                text: "unexpected status 401 Unauthorized".to_owned()
            }]
        );
        // 再接続中などの途中経過は補足として出す(致命的なら続けて turn.failed が来る)
        let retry = r#"{"type":"error","message":"Reconnecting... waiting for network"}"#;
        assert_eq!(
            parse_codex_line(retry).0,
            vec![ChatEvent::Notice {
                text: "Reconnecting... waiting for network".to_owned()
            }]
        );
        // 知らない設定を無視した旨(古い Codex)は出さない
        let ignored = r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Codex is ignoring 1 unrecognized configuration setting."}}"#;
        assert_eq!(parse_codex_line(ignored).0, vec![]);
        assert_eq!(parse_codex_line("WARNING: not json").0, vec![]);
    }

    #[test]
    fn sessions_remember_their_provider() {
        // 以前の形式(ID だけ)は Claude の会話として読む
        assert_eq!(
            decode_session("abc-123"),
            (Provider::Claude, "abc-123".to_owned())
        );
        for p in [Provider::Claude, Provider::Codex] {
            assert_eq!(
                decode_session(&encode_session(p, "01a0-ff")),
                (p, "01a0-ff".to_owned())
            );
        }
        assert_eq!(Provider::parse(None), Ok(Provider::Claude));
        assert_eq!(Provider::parse(Some("codex")), Ok(Provider::Codex));
        assert!(Provider::parse(Some("--x")).is_err());
    }

    #[test]
    fn codex_args_are_built_in_order() {
        let url = "http://127.0.0.1:41920/mcp";
        let args = codex_args(url, None, Some("gpt-6-astra"));
        assert_eq!(
            &args[..4],
            [
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--ignore-user-config"
            ]
        );
        assert!(args.contains(&"mcp_servers.glaux.url=\"http://127.0.0.1:41920/mcp\"".to_owned()));
        assert_eq!(&args[args.len() - 3..], ["-m", "gpt-6-astra", "-"]);

        let args = codex_args(url, Some("01a0d570-4eae"), None);
        assert_eq!(&args[..2], ["exec", "resume"]);
        assert_eq!(&args[args.len() - 2..], ["01a0d570-4eae", "-"]);

        // システムプロンプトは TOML の文字列として元に戻せる
        let dev = args
            .iter()
            .find_map(|a| a.strip_prefix("developer_instructions="))
            .expect("developer_instructions");
        let back: String = serde_json::from_str(dev).expect("TOML/JSON 文字列");
        assert_eq!(back, SYSTEM_PROMPT);
    }

    /// Windows で npm 版(codex.cmd)を起動すると cmd.exe を通るので、コマンドライン全体が
    /// 8191 文字を超えると起動できない。システムプロンプトを伸ばしたときに気付けるようにする
    #[test]
    fn codex_args_fit_windows_cmd_line_limit() {
        let args = codex_args(
            "http://127.0.0.1:41920/mcp",
            Some("01a0d570-4eae-7923-ae85-8c56e471244d"),
            Some("gpt-6-astra-2026-09-01"),
        );
        // 引数ごとの引用符と、内側の " を 2 重にする分を見込む(文字数は UTF-16 単位)
        let len: usize = args
            .iter()
            .map(|a| a.encode_utf16().count() + 3 + a.matches('"').count())
            .sum::<usize>()
            + "cmd.exe /e:ON /v:OFF /d /c \"codex.cmd\"".len();
        assert!(len < 7800, "コマンドラインが長すぎます({len} 文字)");
    }

    /// 実際の Codex CLI を、OpenAI の API を真似たサーバーにつないで動かす(手動用)。
    /// GLAUX_CODEX_BIN(codex の実行ファイル)と GLAUX_CODEX_MOCK(模擬サーバーの base_url)、
    /// GLAUX_CODEX_MCP(MCP サーバーの URL)を設定して `cargo test -p glaux-app -- --ignored codex_end_to_end`
    #[test]
    #[ignore]
    fn codex_end_to_end() {
        let (Ok(bin), Ok(mock), Ok(mcp)) = (
            std::env::var("GLAUX_CODEX_BIN"),
            std::env::var("GLAUX_CODEX_MOCK"),
            std::env::var("GLAUX_CODEX_MCP"),
        ) else {
            panic!("GLAUX_CODEX_BIN / GLAUX_CODEX_MOCK / GLAUX_CODEX_MCP を設定してください");
        };
        let run = |resume: Option<&str>| {
            let mut args = codex_args(&mcp, resume, None);
            let at = if resume.is_some() { 2 } else { 1 };
            let provider = [
                "model_provider=\"mock\"".to_owned(),
                "model_providers.mock.name=\"mock\"".to_owned(),
                format!("model_providers.mock.base_url={}", toml_string(&mock)),
                "model_providers.mock.wire_api=\"responses\"".to_owned(),
            ];
            for (i, c) in provider.iter().enumerate() {
                args.insert(at + i * 2, "-c".into());
                args.insert(at + i * 2 + 1, c.clone());
            }
            let mut child = std::process::Command::new(&bin)
                .args(&args)
                .current_dir(std::env::temp_dir())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("codex を起動");
            {
                use std::io::Write;
                let mut stdin = child.stdin.take().expect("stdin");
                stdin.write_all("ベースを作って".as_bytes()).expect("stdin");
            }
            let out = child.wait_with_output().expect("codex の終了");
            let mut events = Vec::new();
            let mut thread = None;
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let (ev, sid) = parse_codex_line(line);
                events.extend(ev);
                thread = thread.or(sid);
            }
            (events, thread)
        };

        let (events, thread) = run(None);
        let thread = thread.expect("thread_id");
        assert_eq!(events.first(), Some(&ChatEvent::Started), "{events:?}");
        assert!(
            events.contains(&ChatEvent::ToolUse {
                name: "get_project".to_owned()
            }),
            "{events:?}"
        );
        assert!(
            matches!(events.last(), Some(ChatEvent::Result { ok: true, .. })),
            "{events:?}"
        );
        // 同じ会話を再開できる
        let (events, again) = run(Some(&thread));
        assert_eq!(again.as_deref(), Some(thread.as_str()));
        assert!(
            matches!(events.last(), Some(ChatEvent::Result { ok: true, .. })),
            "{events:?}"
        );
    }

    #[test]
    fn ignores_garbage_and_unknown_types() {
        assert_eq!(parse_line("not json").0, vec![]);
        assert_eq!(parse_line(r#"{"type":"user","message":{}}"#).0, vec![]);
        // 空テキストブロックは出さない
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"  "}]}}"#;
        assert_eq!(parse_line(line).0, vec![]);
    }
}
