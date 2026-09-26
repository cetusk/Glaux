// ミキサーの中のドラッグ(エフェクトのプリセットの棚 ⇔ ノード表示)。
// HTML5 のドラッグ&ドロップは、Tauri(Windows)ではファイルのドロップの受け取りに取られて動かないので、
// pointer イベントで自前に運ぶ。棚とノード表示は別のコンポーネントなので、ここで状態と受け口を共有する。

export interface DraggedPreset {
  name: string;
  kind: string;
}

export const fxDrag = $state<{
  /** 棚から運んでいるプリセット */
  preset: DraggedPreset | null;
  /** 運んでいる位置(画面の座標) */
  x: number;
  y: number;
  /** ノード表示のカードを棚の上まで運んでいる(離すとプリセットに保存) */
  overShelf: boolean;
}>({ preset: null, x: 0, y: 0, overShelf: false });

/** 受け口(棚の要素と、ノード表示の「置く」処理) */
export const fxDropTargets: {
  shelf: HTMLElement | null;
  canvas: ((name: string, clientX: number, clientY: number) => void) | null;
} = { shelf: null, canvas: null };

/** 画面の座標が棚の上か */
export function isOverShelf(x: number, y: number): boolean {
  const r = fxDropTargets.shelf?.getBoundingClientRect();
  return !!r && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
}
