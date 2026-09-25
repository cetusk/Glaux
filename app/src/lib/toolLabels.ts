// AI(MCP)のツール名の日本語表示。ヘッダーの作業中表示とチャットの ⚙ 表示で共通。
// 以前は 36 個のうち 7〜9 個にしか訳が無く(今は 41 個)、2 か所に別々に書かれていた。

interface ToolLabel {
  /** チャットの ⚙ 表示(名詞) */
  short: string;
  /** ヘッダーの作業中表示(「AI が…」に続く) */
  doing: string;
}

const LABELS: Record<string, ToolLabel> = {
  // 基本
  get_project: { short: "プロジェクトを確認", doing: "プロジェクトを読んでいます" },
  apply_commands: { short: "編集を適用", doing: "編集しています" },
  undo: { short: "取り消し", doing: "取り消しています" },
  redo: { short: "やり直し", doing: "やり直しています" },
  checkpoint: { short: "チェックポイント作成", doing: "チェックポイントを作成しています" },
  revert_to: { short: "チェックポイントへ巻き戻し", doing: "巻き戻しています" },
  revert: { short: "編集を取り消し", doing: "編集を取り消しています" },
  get_history: { short: "履歴を確認", doing: "履歴を確認しています" },
  list_params: { short: "パラメータを確認", doing: "パラメータを確認しています" },
  get_changes: { short: "変更を確認", doing: "変更を確認しています" },
  get_guide: { short: "定石を確認", doing: "定石を読んでいます" },
  // 構成
  duplicate_clips: { short: "クリップを複製", doing: "クリップを複製しています" },
  insert_bars: { short: "小節を挿入", doing: "小節を挿入しています" },
  delete_bars: { short: "小節を削除", doing: "小節を削除しています" },
  bounce_track: { short: "トラックを音声にする", doing: "トラックを音声に描き出しています" },
  export_audio: { short: "書き出し", doing: "WAV に書き出しています" },
  // 分析
  analyze_audio: { short: "音を聴いて確認", doing: "音を聴いています" },
  analyze_harmony: { short: "キーとコードを確認", doing: "キーとコードを調べています" },
  analyze_rhythm: { short: "リズムを確認", doing: "リズムを調べています" },
  analyze_beats: { short: "テンポを測定", doing: "テンポを測っています" },
  analyze_sound: { short: "音色を確認", doing: "音色を調べています" },
  compare_sounds: { short: "音を比較", doing: "音を比べています" },
  match_sound: { short: "音色を似せる", doing: "音色を似せています" },
  // ノート
  transpose_notes: { short: "移調", doing: "移調しています" },
  shift_notes: { short: "ノートを移動", doing: "ノートを動かしています" },
  swing_notes: { short: "スウィング", doing: "ハネを付けています" },
  quantize_notes: { short: "クオンタイズ", doing: "タイミングを揃えています" },
  scale_velocity: { short: "強さを調整", doing: "音の強さを調整しています" },
  // 音色
  list_presets: { short: "プリセット一覧", doing: "プリセットを探しています" },
  save_preset: { short: "プリセットを保存", doing: "プリセットを保存しています" },
  load_preset: { short: "プリセットを適用", doing: "プリセットを読み込んでいます" },
  delete_preset: { short: "プリセットを削除", doing: "プリセットを削除しています" },
  find_similar_presets: { short: "似たプリセットを探す", doing: "似たプリセットを探しています" },
  list_soundfonts: { short: "SoundFont 一覧", doing: "SoundFont を確認しています" },
  set_soundfont_instrument: { short: "SoundFont の楽器を設定", doing: "楽器を設定しています" },
  // CLAP
  list_plugins: { short: "プラグイン一覧", doing: "プラグインを確認しています" },
  list_plugin_presets: { short: "プラグインのプリセット一覧", doing: "プラグインのプリセットを探しています" },
  load_plugin_preset: { short: "プラグインのプリセットを適用", doing: "プラグインのプリセットを読み込んでいます" },
  refine_plugin_params: { short: "プラグインのつまみを調整", doing: "プラグインのつまみを詰めています" },
  // 素材
  import_sample: { short: "サンプルを取り込み", doing: "サンプルを取り込んでいます" },
  import_audio_clip: { short: "音声を配置", doing: "音声を取り込んでいます" },
  transcribe_audio: { short: "譜起こし", doing: "譜起こししています" },
  separate_audio: { short: "パート分離", doing: "パートに分けています" },
  // GPT(Codex)の組み込みの動作
  command_execution: { short: "コマンドを実行", doing: "コマンドを実行しています" },
  web_search: { short: "Web を検索", doing: "Web を検索しています" },
};

/** `mcp__glaux__` などの接頭辞を外したツール名 */
function bare(name: string): string {
  return name.replace(/^mcp__[^_]+__/, "");
}

/** チャットの ⚙ 表示用(訳が無ければツール名のまま) */
export function toolShort(name: string): string {
  return LABELS[bare(name)]?.short ?? bare(name);
}

/** ヘッダーの作業中表示用(訳が無ければ「<ツール名> を実行しています」) */
export function toolDoing(name: string): string {
  const n = bare(name);
  return LABELS[n]?.doing ?? `${n} を実行しています`;
}
