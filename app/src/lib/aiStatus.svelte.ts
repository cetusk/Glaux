// アプリ内チャット(UI → AI)の実行状態。
// ChatPanel が書き、App のヘッダーインジケータが読む(モジュールレベルの rune)。
// チャット実行中はターンの正確な境界が分かるので、MCP 呼び出しベースの
// 近似(タイムアウト)より優先してインジケータを点灯し続ける。
// epoch はプロジェクト切り替えのたびに増える。ChatPanel が表示クリアの合図に使う
export const chatStatus = $state({ running: false, epoch: 0 });
