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

export const pianoRollStore = $state<{ focus: FocusClip | null }>({ focus: null });

/// 音作りビューで開いているトラック。ChatPanel が「音作り中のトラック」として指示に添える。
export interface SoundDesignFocus {
  trackId: string;
  trackName: string;
}

export const soundDesignStore = $state<{ focus: SoundDesignFocus | null }>({ focus: null });
