//! アプリ内チャット: UI からの指示をヘッドレス `claude` に渡す。
//!
//! 仕組み:
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

/// チャットの実行状態。`AppState` が保持する。
pub struct ChatManager {
    mcp_url: String,
    /// 現在のプロジェクトフォルダ(プロジェクト切り替えで変わる)
    project_dir: Mutex<String>,
    session_id: Mutex<Option<String>>,
    /// チャットの AI に最後に見せた履歴エントリ ID。
    /// これ以降の人間の編集を次のターンでコンテキストとして注入する
    last_seen_entry: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
    running: AtomicBool,
    /// 使うモデル(`claude --model` に渡す)。None なら Claude Code の既定
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
    pluck=撥弦の物理モデル)を設定でき、\
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
    太いベースは sub、EDM のポンピングは sidechain エフェクト(source にキックのトラック ID、\
    release_ms を 8 分音符の長さ = 60000/BPM/2 に合わせると気持ちよく揺れる)、\
    ギターの歪みやメタルは distortion(square 波 + 高 drive)、\
    メタルのブリッジミュートの刻み(ズクズク)はノートに articulation: \"palm_mute\" を付けます\
    (add_notes / update_notes。ほかに staccato / accent、ロングトーンの表情付けに vibrato、\
    ギターソロの決め音に bend = チョーキング(全音下から滑り上がる)。低めの音 + 歪みと組み合わせると効果的)。\
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
    音色はプラグイン自身の画面で人間が作ります(AI からプラグインのつまみは動かせません)。\
    取り込んだ曲はパートに分けられます(separate_audio。builtin = 打楽器 / 音程楽器、demucs = ボーカル / ドラム / ベース / その他)。\
    ベースだけ譜起こししたいときは、分離 → transcribe_audio の順に。\
    テンポを変えるときは、音声クリップに set_clip_stretch(follow、original_bpm = 録音時のテンポ)を\
    付けておくと拍がずれずに伸縮します(音程は変わりません)。音声ファイルの配置を頼まれたら import_audio_clip を使います。\
    鼻歌や歌の録音(単旋律)は transcribe_audio で MIDI クリップにできます(ピアノ・ギターの和音や伴奏入りの素材は mode: \"poly\")。MIDI 化したら \
    analyze_harmony でキーを確認し、前後と 12 半音ずれた短い音(オクターブ誤検出)や外れた音を整えてから報告してください。\
    ミックスバランス: analyze_audio の per_track: true で各トラックのラウドネスと帯域の一覧が取れます。\
    定石: 主役(リード等)は伴奏より 2〜4dB 上に置く / 帯域の重心が被るトラックは EQ で住み分ける /\
    それでも埋もれるなら伴奏側に sidechain。音量を上げる前に、まず被りを削ることを検討してください。\
    セルフレビューの習慣: まとまった編集を終えたら、完了報告の前に必ず自己確認してください。\
    (1) analyze_harmony で調性が意図どおりか、(2) analyze_audio でクリップやバランスの破綻がないか。\
    問題があればその場で修正してから報告し、報告には確認結果(キー・LUFS など)を一言添えます。\
    返答は簡潔な日本語で、行った編集の要点だけ述べてください。";

impl ChatManager {
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
        let session_id = read_cache(&session_file(&project_dir));
        if session_id.is_some() {
            tracing::info!("前回のチャットセッションを再開します");
        }
        let last_seen_entry = read_cache(&last_seen_file(&project_dir));
        ChatManager {
            mcp_url,
            project_dir: Mutex::new(project_dir),
            session_id: Mutex::new(session_id),
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
        let session_id = read_cache(&session_file(&dir));
        let last_seen = read_cache(&last_seen_file(&dir));
        *self.project_dir.lock().expect("project_dir lock") = dir;
        *self.session_id.lock().expect("session_id lock") = session_id;
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
        self.session_id.lock().expect("session_id lock").is_some()
    }

    /// セッション ID を更新し、`cache/chat-session.txt` に永続化する。
    fn set_session_id(&self, sid: String) {
        let mut guard = self.session_id.lock().expect("session_id lock");
        if guard.as_deref() == Some(sid.as_str()) {
            return;
        }
        *guard = Some(sid.clone());
        drop(guard);
        write_cache(&session_file(&self.project_dir()), Some(&sid));
    }

    /// 会話をリセットする(次の送信が新しいセッションになる)。
    /// `last_seen_entry` は会話ではなく「AI に何を見せたか」の記録なので保持する。
    pub fn reset(&self) {
        *self.session_id.lock().expect("session_id lock") = None;
        let _ = std::fs::remove_file(session_file(&self.project_dir()));
    }

    /// 実行中の claude プロセスを止める。
    pub fn cancel(&self) {
        let child = self.child.lock().expect("child lock").take();
        if let Some(mut child) = child {
            // kill は非同期だが、start_kill で即シグナルだけ送れば十分
            let _ = child.start_kill();
        }
    }

    fn build_command(&self) -> Command {
        // Windows: ネイティブ版は claude.exe、npm 版は claude.cmd。
        // Rust の Command は .cmd を安全に(引数をエスケープして)起動できる。
        #[cfg(windows)]
        let program = "claude.cmd";
        #[cfg(not(windows))]
        let program = "claude";
        let mut cmd = Command::new(program);
        self.configure(&mut cmd);
        cmd
    }

    #[cfg(windows)]
    fn build_command_fallback(&self) -> Command {
        let mut cmd = Command::new("claude");
        self.configure(&mut cmd);
        cmd
    }

    fn configure(&self, cmd: &mut Command) {
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
            .arg(SYSTEM_PROMPT)
            .current_dir(self.project_dir())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(sid) = self.session_id.lock().expect("session_id lock").as_ref() {
            cmd.args(["--resume", sid]);
        }
        if let Some(model) = self.model.lock().expect("model lock").as_ref() {
            cmd.args(["--model", model]);
        }
        #[cfg(windows)]
        {
            // コンソールウィンドウを出さない(CREATE_NO_WINDOW)
            cmd.creation_flags(0x0800_0000);
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
    let mut child = mgr.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "claude コマンドが見つかりません。Claude Code をインストールして PATH を通してください"
                .to_owned()
        } else {
            format!("claude を起動できません: {e}")
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
        let (events, session_id) = parse_line(&line);
        if let Some(sid) = session_id {
            mgr.set_session_id(sid);
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
            Err(format!("claude が異常終了しました({s})\n{tail}"))
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

    #[test]
    fn ignores_garbage_and_unknown_types() {
        assert_eq!(parse_line("not json").0, vec![]);
        assert_eq!(parse_line(r#"{"type":"user","message":{}}"#).0, vec![]);
        // 空テキストブロックは出さない
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"  "}]}}"#;
        assert_eq!(parse_line(line).0, vec![]);
    }
}
