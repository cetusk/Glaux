// 音量をそろえた A/B の聴き比べの再生の補助(案の聴き比べと、履歴の「編集の前と今」の聴き比べで共通)。
//
// 聴き比べ中のレンダラは、用意した範囲の外では無音になる。範囲を 1 回聴き終えると黙ってしまい、
// A / B を押しても鳴らなかったので、聴き比べの間は範囲をループにし、終えたら元のループの設定に戻す。
import { transportClearLoop, transportPlay, transportSeek, transportSetLoop } from "./api";
import { transportStore } from "./transport.svelte";

/** 聴き比べを始める前のループの設定(聴き比べ中でなければ null) */
let saved: { loop: [number, number] | null } | null = null;

/** 範囲をループにして、頭から鳴らす */
export async function startAbLoop(start: number, end: number): Promise<void> {
  if (saved === null) {
    const l = transportStore.state.loop;
    saved = { loop: l && l[1] > l[0] ? [l[0], l[1]] : null };
  }
  await transportSetLoop(start, end);
  await transportSeek(start);
  if (!transportStore.state.playing) await transportPlay();
}

/** A / B を押したとき: 止まっている・範囲の外にいるなら、範囲の頭から鳴らす */
export async function ensureAbPlaying(start: number, end: number): Promise<void> {
  const t = transportStore.state.tick ?? 0;
  if (t < start || t >= end) await transportSeek(start);
  if (!transportStore.state.playing) await transportPlay();
}

/** 範囲の頭から聴き直す */
export async function restartAb(start: number): Promise<void> {
  await transportSeek(start);
  if (!transportStore.state.playing) await transportPlay();
}

/** 聴き比べを終えたら、ループを元の設定に戻す */
export async function endAbLoop(): Promise<void> {
  if (saved === null) return;
  const s = saved;
  saved = null;
  try {
    if (s.loop) await transportSetLoop(s.loop[0], s.loop[1]);
    else await transportClearLoop();
  } catch {
    // 戻せなくても聴き比べには関係しない
  }
}
