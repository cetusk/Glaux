import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AiActivity,
  AppInfo,
  ChatEvent,
  HistorySnapshot,
  ProjectSnapshot,
  RecentProject,
  TransportState,
} from "./types";

export function getProject(): Promise<ProjectSnapshot> {
  return invoke("get_project");
}

export function getHistory(): Promise<HistorySnapshot> {
  return invoke("get_history");
}

export function undo(): Promise<{ undone: number; project_version: number }> {
  return invoke("undo");
}

export function redo(): Promise<{ redone: number; project_version: number }> {
  return invoke("redo");
}

export function appInfo(): Promise<AppInfo> {
  return invoke("app_info");
}

/** MCP / UI どちらの編集でも発火する。受けたら全体を取得し直す。 */
export function onProjectChanged(cb: () => void): Promise<UnlistenFn> {
  return listen("project-changed", cb);
}

/** AI(MCP クライアント)のツール呼び出しの開始/終了。 */
export function onAiActivity(cb: (a: AiActivity) => void): Promise<UnlistenFn> {
  return listen<AiActivity>("ai-activity", (e) => cb(e.payload));
}

// ---- UI からの編集(Command API 経由、author: human) ----

/** Command JSON を適用する。履歴に載り、AI からも get_history で見える。 */
export function applyEdit(
  commands: unknown[],
  label: string,
): Promise<{ entry_id: string; project_version: number }> {
  return invoke("apply_edit", { commands, label });
}

// ---- プロジェクト管理 ----

export function listRecentProjects(): Promise<{
  recent: RecentProject[];
  default_dir: string;
}> {
  return invoke("list_recent_projects");
}

/** プロジェクトを開く(インプロセス切り替え。MCP・チャット・エンジンは追従する)。 */
export function openProject(
  path: string,
  create = false,
): Promise<{ title: string; path: string; project_version: number }> {
  return invoke("open_project", { path, create });
}

/** `parentDir/name.glaux` に新規プロジェクトを作成して開く。 */
export function createProject(
  parentDir: string,
  name: string,
): Promise<{ title: string; path: string; project_version: number }> {
  return invoke("create_project", { parentDir, name });
}

/** プロジェクトを WAV に書き出す(<プロジェクト>/export/ 配下)。 */
export function exportWav(): Promise<{ path: string; seconds: number }> {
  return invoke("export_project_wav");
}

// ---- トランスポート(再生) ----

export function transportState(): Promise<TransportState> {
  return invoke("transport_state");
}

export function transportPlay(): Promise<void> {
  return invoke("transport_play");
}

export function transportPause(): Promise<void> {
  return invoke("transport_pause");
}

export function transportStop(): Promise<void> {
  return invoke("transport_stop");
}

export function transportSeek(tick: number): Promise<void> {
  return invoke("transport_seek", { tick: Math.max(0, Math.round(tick)) });
}

/** ノートを 1 音だけ試聴する(そのトラックの音源・音量・パンで鳴る)。 */
export function previewNote(trackId: string, pitch: number): Promise<void> {
  return invoke("preview_note", { trackId, pitch });
}

// ---- チャット(UI → AI 指示) ----

/** 指示を送る。進捗は onChatEvent で届く。 */
export function sendChat(prompt: string): Promise<void> {
  return invoke("send_chat", { prompt });
}

export function cancelChat(): Promise<void> {
  return invoke("cancel_chat");
}

/** 会話をリセットする(次の送信が新しいセッションになる)。 */
export function resetChat(): Promise<void> {
  return invoke("reset_chat");
}

export function onChatEvent(cb: (e: ChatEvent) => void): Promise<UnlistenFn> {
  return listen<ChatEvent>("chat-event", (e) => cb(e.payload));
}
