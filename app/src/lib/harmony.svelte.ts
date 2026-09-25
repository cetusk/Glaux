// 曲のキーと小節ごとのコード(ノートからの推定)。ヘッダーのキー、ルーラーのコード、ピアノロールのスケール外の行に使う。
import * as api from "./api";

export interface HarmonyView {
  key: { name: string; tonic: number; mode: string; confidence: number } | null;
  chords: { bar: number; tick: number; chord: string; confidence: number }[];
  /** キーのスケールの音(ピッチクラス 0..11。C = 0)。キーが無ければ空 */
  scale: number[];
}

export const harmonyStore = $state<{ view: HarmonyView | null }>({ view: null });

let timer: ReturnType<typeof setTimeout> | undefined;

/** 取り直す(編集が続くときは 400ms まとめる) */
export function refreshHarmony() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(async () => {
    timer = undefined;
    try {
      harmonyStore.view = await api.harmony();
    } catch {
      // 表示だけなので、取れなくても何もしない
    }
  }, 400);
}
