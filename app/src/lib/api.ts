import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AiActivity,
  AppInfo,
  ChatEvent,
  HistorySnapshot,
  PresetInfo,
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

/** 既定の作業フォルダ(新規プロジェクトの作成先)を変更・永続化する。 */
export function setProjectsDir(path: string): Promise<{ default_dir: string }> {
  return invoke("set_projects_dir", { path });
}

/** 現在のプロジェクトを移動 / 名前変更する(省略した方は現状維持)。 */
export function moveProject(
  destParent: string | null,
  newName: string | null,
): Promise<{ path: string; title?: string; project_version?: number; moved: boolean }> {
  return invoke("move_project", { destParent, newName });
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

// ---- 音作りビュー ----

export function getTrackParams(trackId: string): Promise<import("./types").TrackParams> {
  return invoke("get_track_params", { trackId });
}

/** WAV を取り込んでトラックの音源を sampler にする(1 undo)。 */
export function importSample(
  trackId: string,
  path: string,
): Promise<{ asset_id: string; project_version: number }> {
  return invoke("import_sample", { trackId, path });
}

// ---- 音色プリセット ----

export function listPresets(): Promise<{ presets: PresetInfo[] }> {
  return invoke("list_presets");
}

/** トラックの現在の音(音源 + エフェクト)をプリセット保存する。 */
export function savePreset(
  trackId: string,
  name: string,
  overwrite = false,
): Promise<{ saved: string }> {
  return invoke("save_preset", { trackId, name, overwrite });
}

/** プリセットをトラックに適用する(音源差し替え + エフェクト置換。1 undo)。 */
export function loadPreset(trackId: string, name: string): Promise<{ applied: string }> {
  return invoke("load_preset", { trackId, name });
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

/** ループ区間を設定する(tick)。 */
export function transportSetLoop(startTick: number, endTick: number): Promise<void> {
  return invoke("transport_set_loop", {
    startTick: Math.max(0, Math.round(startTick)),
    endTick: Math.max(0, Math.round(endTick)),
  });
}

export function transportClearLoop(): Promise<void> {
  return invoke("transport_clear_loop");
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
