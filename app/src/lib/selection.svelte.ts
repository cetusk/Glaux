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

/// MIDI キーボードの送り先として「アーム」したトラック(🎹)。1 つだけ。
/// アームしたトラックがあると ⏺ は MIDI 録音になる。
/// 未アームなら、ピアノロールで開いているトラック → 最初の MIDI トラックの音で鳴らす
export const midiArmStore = $state<{ trackId: string | null }>({ trackId: null });

/// 音源ピッカーを開いているトラックと、出す位置(画面の座標)。トラックの見出しと
/// インスペクターの「変更」から同じピッカーを開く(以前は選び方が 3 か所に分かれていた)
export const instrumentPickerStore = $state<{
  open: { trackId: string; x: number; y: number; tab?: "builtin" | "preset" | "sf2" | "clap" | "sample" } | null;
}>({ open: null });

/// インスペクター(音作りパネル)の幅。開いている間、タイムラインとピアノロールをこの幅だけ押し縮める
function loadInspectorWidth(): number {
  try {
    const v = Number(localStorage.getItem("glaux.inspectorWidth"));
    return Number.isFinite(v) && v >= 300 ? Math.min(v, 640) : 360;
  } catch {
    return 360;
  }
}
export const inspectorStore = $state<{ width: number }>({ width: loadInspectorWidth() });

export function saveInspectorWidth() {
  try {
    localStorage.setItem("glaux.inspectorWidth", String(inspectorStore.width));
  } catch {
    // 保存できなくても動作には関係しない
  }
}

/// 本体の画面(タイムライン / ミキサー)と、ミキサーで選んでいるトラック(下のノード表示の対象)。
/// `highlightFx` はミキサーの列で押したエフェクト(下のカードを光らせる)
export const viewStore = $state<{
  main: "timeline" | "mixer";
  mixerTrack: string | null;
  highlightFx: string | null;
}>({ main: "timeline", mixerTrack: null, highlightFx: null });
