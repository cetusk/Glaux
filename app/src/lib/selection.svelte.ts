// タイムライン上の小節範囲選択(マスク)。
// Timeline が書き、ChatPanel が「指示の対象範囲」として読む。
// bar は 0 始まり(表示は +1)、tick は [startTick, endTick) の半開区間。
export interface BarRange {
  startBar: number;
  endBar: number;
  startTick: number;
  endTick: number;
}

export const selectionStore = $state<{ range: BarRange | null }>({ range: null });

/// ピアノロールで開いているクリップ。ChatPanel が「対象クリップ」として指示に添える。
export interface FocusClip {
  clipId: string;
  clipName: string;
  trackId: string;
  trackName: string;
  /// 開いたときに中央に表示するクリップ内 tick(ダブルクリック位置)
  anchorTick?: number;
}

/// ピアノロールの状態。`focus` = 上(メイン)ペイン、`second` = 分割時の下ペイン。
/// `active` はキーボード操作(コピペ・削除・奏法)を受け付けるペイン。
export const pianoRollStore = $state<{
  focus: FocusClip | null;
  second: FocusClip | null;
  active: "main" | "second";
}>({ focus: null, second: null, active: "main" });

/// ノートのクリップボード(ピアノロールの 2 ペイン間・クリップ間で共有)。
export interface ClipboardNote {
  dpos: number;
  dur: number;
  pitch: number;
  vel: number;
  articulation?: string;
}
export const noteClipboard = $state<{ items: ClipboardNote[] }>({ items: [] });

/// 音作りビューで開いているトラック。ChatPanel が「音作り中のトラック」として指示に添える。
export interface SoundDesignFocus {
  trackId: string;
  trackName: string;
}

export const soundDesignStore = $state<{ focus: SoundDesignFocus | null }>({ focus: null });

/// 音作りビューをマスターバスで開くときの trackId
export const MASTER_FOCUS_ID = "__master__";
