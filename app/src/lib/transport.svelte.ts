// 再生状態(transport_state)の共有ストア。
//
// transport_state の入力ピーク・DSP 負荷は「読むとリセット」なので、問い合わせは App の
// 1 か所だけで行い、ほかの画面(設定パネルのメーター等)はここを読む。以前は App と設定パネルが
// 別々に問い合わせて値を奪い合っていた。
//
// 値は項目ごとに比べて変わったものだけ書き換える(毎回オブジェクトを差し替えると、値が同じでも
// それに依存する描画がすべて走る)。
import * as api from "./api";
import type { TransportState } from "./types";

export const transportStore = $state<{
  state: TransportState;
  /** 問い合わせのたびに増える(値が同じでも「新しい読み取りが来た」ことを知りたい画面用) */
  seq: number;
  /** 速い更新を依頼している画面の数(入力メーター・MIDI の受信表示) */
  fast: number;
}>({
  state: { available: false, playing: false, tick: 0 },
  seq: 0,
  fast: 0,
});

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  return JSON.stringify(a) === JSON.stringify(b);
}

function apply(next: TransportState) {
  const cur = transportStore.state as unknown as Record<string, unknown>;
  const src = next as unknown as Record<string, unknown>;
  for (const k of Object.keys(src)) {
    if (!same(cur[k], src[k])) cur[k] = src[k];
  }
  for (const k of Object.keys(cur)) {
    if (!(k in src) && cur[k] !== undefined) cur[k] = undefined;
  }
  transportStore.seq += 1;
}

/** 今すぐ問い合わせて反映する(再生・停止などの操作の直後に使う) */
export async function pollTransport(): Promise<TransportState> {
  const t = await api.transportState();
  apply(t);
  return transportStore.state;
}

/** 速い更新を依頼する。返り値を呼ぶと取り消す */
export function requestFastPolling(): () => void {
  transportStore.fast += 1;
  let done = false;
  return () => {
    if (done) return;
    done = true;
    transportStore.fast -= 1;
  };
}

/**
 * 定期的な問い合わせを始める(App が 1 回だけ呼ぶ)。再生中・速い更新の依頼中は 80〜100ms、
 * 止まっているときは 400ms。前の応答を待ってから次を送るので、応答が遅くても重ならない。
 * 返り値を呼ぶと止まる
 */
export function startTransportPolling(): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const loop = async () => {
    try {
      await pollTransport();
    } catch {
      // 起動直後など。次の問い合わせで回復する
    }
    if (stopped) return;
    const ms = transportStore.fast > 0 ? 80 : transportStore.state.playing ? 100 : 400;
    timer = setTimeout(loop, ms);
  };
  loop();
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
  };
}
