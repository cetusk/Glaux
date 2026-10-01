// 拍子イベント(time_sig_map)を考慮した小節マップ。
// Timeline / PianoRoll / 小節ナビゲーションが共有する tick ↔ 小節の変換。
// 拍子変更イベントの tick は新しい小節の頭として扱う(直前の小節が中途半端な
// 長さでも、そこで区切る = 一般的な DAW と同じ挙動)。

import type { Project } from "./types";

export interface Bar {
  /** 0 始まりの小節番号 */
  index: number;
  /** 小節頭の絶対 tick */
  tick: number;
  /** この小節の長さ(tick) */
  len: number;
  /** この小節の拍子 */
  num: number;
  den: number;
  /** 拍のまとまり(分母の音符の数)。7/8 なら [2, 2, 3] */
  grouping: number[];
  /** この小節で拍子が変わった(ルーラーに表示する) */
  sigChange: boolean;
}

/** まとまりの既定値(glaux-core の meter::default_grouping と同じ規則) */
export function defaultGrouping(num: number, den: number): number[] {
  const n = Math.max(1, num);
  if (den <= 4) {
    if (n === 5) return [3, 2];
    if (n === 6) return [3, 3];
    if (n === 7) return [4, 3];
    return Array(n).fill(1);
  }
  if (n % 3 === 0) return Array(n / 3).fill(3);
  if (n === 1) return [1];
  if (n % 2 === 1) return [...Array((n - 3) / 2).fill(2), 3];
  return Array(n / 2).fill(2);
}

/** 拍子のまとまり(指定が無いか和が合わなければ既定値) */
export function groupingOf(sig: { num: number; den: number; grouping?: number[] }): number[] {
  const g = sig.grouping;
  if (g && g.length > 0 && g.every((x) => x >= 1) && g.reduce((a, b) => a + b, 0) === sig.num) return g;
  return defaultGrouping(sig.num, sig.den);
}

/** "7/8 2+2+3" の形(4 分ずつの拍子は "4/4" だけ) */
export function meterLabel(bar: { num: number; den: number; grouping: number[] }): string {
  if (bar.grouping.every((g) => g === 1)) return `${bar.num}/${bar.den}`;
  return `${bar.num}/${bar.den} ${bar.grouping.join("+")}`;
}

/** 小節の中のまとまりの頭(小節頭からの tick。先頭 0 は含めない) */
export function groupHeads(bar: Bar, ppq: number): number[] {
  const unit = (ppq * 4) / bar.den;
  const out: number[] = [];
  let t = 0;
  for (const g of bar.grouping) {
    t += g * unit;
    if (t < bar.len) out.push(t);
  }
  return out;
}

/** endTick を覆う小節列を作る(最低 minBars、末尾に extra 小節の余白)。 */
export function buildBars(
  project: Project,
  endTick: number,
  minBars = 16,
  extra = 2,
): Bar[] {
  const ppq = project.ppq;
  const sigs =
    project.time_sig_map.length > 0
      ? [...project.time_sig_map].sort((a, b) => a.tick - b.tick)
      : [{ tick: 0, num: 4, den: 4, grouping: undefined as number[] | undefined }];

  const bars: Bar[] = [];
  let index = 0;

  for (let si = 0; si < sigs.length; si++) {
    const sig = sigs[si];
    const segEnd = sigs[si + 1]?.tick ?? Infinity;
    const barLen = Math.max(1, (ppq * 4 * sig.num) / sig.den);
    let t = sig.tick;
    let first = true;

    while (t < segEnd) {
      const len = Math.min(barLen, segEnd - t);
      bars.push({
        index,
        tick: t,
        len,
        num: sig.num,
        den: sig.den,
        grouping: groupingOf(sig),
        sigChange: first && si > 0,
      });
      first = false;
      index += 1;
      t += len;

      if (segEnd === Infinity && t >= endTick && bars.length >= minBars) {
        // 余白ぶんを足して終了
        for (let k = 0; k < extra; k++) {
          bars.push({
            index,
            tick: t,
            len: barLen,
            num: sig.num,
            den: sig.den,
            grouping: groupingOf(sig),
            sigChange: false,
          });
          index += 1;
          t += barLen;
        }
        return bars;
      }
      // 安全弁(異常データでの無限ループ防止)
      if (bars.length > 4096) return bars;
    }
  }
  return bars;
}

/** tick を含む小節を返す(範囲外は端の小節)。 */
export function barAtTick(bars: Bar[], tick: number): Bar {
  let lo = 0;
  let hi = bars.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (bars[mid].tick <= tick) lo = mid;
    else hi = mid - 1;
  }
  return bars[lo];
}

/** 小節列の終端 tick。 */
export function barsEndTick(bars: Bar[]): number {
  const last = bars[bars.length - 1];
  return last ? last.tick + last.len : 0;
}

/** 「前の小節頭へ」。小節の途中なら現在の小節頭、頭にいるなら前の小節頭。 */
export function prevBarHead(bars: Bar[], tick: number): number {
  const bar = barAtTick(bars, tick);
  if (tick - bar.tick > bar.len * 0.1) return bar.tick;
  const prev = bars[Math.max(0, bar.index - 1)];
  return prev.tick;
}

/** 「次の小節頭へ」。 */
export function nextBarHead(bars: Bar[], tick: number): number {
  const bar = barAtTick(bars, tick);
  return bar.tick + bar.len;
}

/** 表示窓の位置「小節.拍.16 分」(どれも 1 始まり)。拍は拍子の分母の音符 */
export function formatPosition(bars: Bar[], tick: number, ppq: number): string {
  if (bars.length === 0) return "1.1.1";
  const bar = barAtTick(bars, tick);
  const beatLen = (ppq * 4) / bar.den;
  const inBar = Math.max(0, tick - bar.tick);
  const beat = Math.floor(inBar / beatLen);
  const sixteenth = Math.floor((inBar - beat * beatLen) / (ppq / 4));
  return `${bar.index + 1}.${beat + 1}.${sixteenth + 1}`;
}

/** tick を秒に(テンポの変化を考慮) */
export function tickToSeconds(tempoMap: { tick: number; bpm: number }[], tick: number, ppq: number): number {
  const events = tempoMap.length > 0 ? tempoMap : [{ tick: 0, bpm: 120 }];
  let secs = 0;
  for (let i = 0; i < events.length; i++) {
    const start = events[i].tick;
    if (tick <= start) break;
    const end = Math.min(tick, events[i + 1]?.tick ?? Infinity);
    secs += ((end - start) / ppq) * (60 / events[i].bpm);
  }
  return secs;
}

/** 秒を「分:秒.1/10」に */
export function formatSeconds(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs - m * 60;
  return `${m}:${s.toFixed(1).padStart(4, "0")}`;
}

/** テンポの表示(小数 2 桁まで、末尾の 0 は付けない)。読み込んだ楽譜の 132.35281141881174 のような値を短く見せる */
export function fmtBpm(bpm: number): string {
  return String(Math.round(bpm * 100) / 100);
}
