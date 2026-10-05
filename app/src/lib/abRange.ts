// 聴き比べる範囲の決め方(拍子どおりの小節で)。履歴・チャットのターン・案の聴き比べで共通
import type { Project } from "./types";
import { barAtTick, buildBars } from "./barMap";
import { selectionStore } from "./selection.svelte";
import { transportStore } from "./transport.svelte";

/** `tick` を含む小節の頭 */
export function barHead(project: Project, tick: number): number {
  const bars = buildBars(project, tick + 1, 1, 1);
  return bars.length ? barAtTick(bars, tick).tick : tick;
}

/** `tick` を含む小節の頭から `n` 小節進んだ所 */
export function addBars(project: Project, tick: number, n: number): number {
  const bars = buildBars(project, tick + 1, 1, n + 1);
  const at = bars.length ? barAtTick(bars, tick) : null;
  return at && bars[at.index + n] ? bars[at.index + n].tick : tick + project.ppq * 4 * n;
}

/** 範囲を決めないとき: ループ中ならその区間、範囲を選んでいればそこ、無ければ今の位置の小節から 8 小節 */
export function defaultAbRange(project: Project | null): { start: number; end: number } {
  const loop = transportStore.state.loop;
  if (loop && loop[1] > loop[0]) return { start: loop[0], end: loop[1] };
  const r = selectionStore.range;
  if (r && r.endTick > r.startTick) return { start: r.startTick, end: r.endTick };
  const t = Math.max(0, transportStore.state.tick ?? 0);
  if (!project) return { start: t, end: t + 960 * 4 * 8 };
  const start = barHead(project, t);
  return { start, end: addBars(project, start, 8) };
}

/** 音の違う範囲(曲の頭からの tick)から聴く範囲: 最初に違う所の小節の頭から、その範囲の終わりか 4 小節の長い方(8 小節まで)。
 *  違いが無い・曲全体に効く違いだけなら null */
export function rangeFromChanges(project: Project, ranges: [number, number][]): { start: number; end: number } | null {
  const first = ranges[0];
  if (!first) return null;
  const start = barHead(project, first[0]);
  const end = Math.min(Math.max(first[1], addBars(project, start, 4)), addBars(project, start, 8));
  return { start, end };
}
