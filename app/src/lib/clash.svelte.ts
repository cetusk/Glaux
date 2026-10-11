// パートどうしの音のぶつかり(glaux-core の clash。AI の check_clashes と同じ判定)。
// ピアノロールの印・タイムラインのルーラーの印に使う。編集のたびに取り直す(harmony と同じく 400ms まとめる)
import * as api from "./api";

export interface ClashNoteView {
  track_id: string;
  track: string;
  clip_id: string;
  note_id: string;
  pitch: number;
  name: string;
  start: number;
  end: number;
}

export interface ClashSpanView {
  tick: number;
  len: number;
  a_clip: string;
  b_clip: string;
}

export interface ClashItem {
  severity: "warn" | "info";
  kind: "semitone" | "minor_ninth" | "low_mud";
  bar: number;
  beat: number;
  tick: number;
  len: number;
  a: ClashNoteView;
  b: ClashNoteView;
  chord?: string;
  off_chord?: "a" | "b" | "both";
  count: number;
  bars: number[];
  spans?: ClashSpanView[];
  message: string;
  hint: string;
}

export interface ClashView {
  tracks: string[];
  track_ids: string[];
  skipped: string[];
  clashes: ClashItem[];
  harsh: { bar: number; beat: number; tick: number; len: number; relative: number; chord?: string; message: string }[];
  summary: string;
}

export const clashStore = $state<{ view: ClashView | null }>({ view: null });

let timer: ReturnType<typeof setTimeout> | undefined;
let seq = 0;

/** 取り直す(編集が続くときは 400ms まとめる。後から届いた古い応答では上書きしない) */
export function refreshClashes() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(async () => {
    timer = undefined;
    const my = ++seq;
    try {
      const view = await api.checkClashes();
      if (my === seq) clashStore.view = view;
    } catch {
      // 表示だけなので、取れなくても何もしない
    }
  }, 400);
}

/** 出てくる所すべて(spans が無い古い応答では最初の所だけ) */
export function spansOf(c: ClashItem): ClashSpanView[] {
  return c.spans && c.spans.length > 0 ? c.spans : [{ tick: c.tick, len: c.len, a_clip: c.a.clip_id, b_clip: c.b.clip_id }];
}

/** 吹き出しの文(何と何が・どう直すか) */
export function clashTip(c: ClashItem): string {
  return `${c.message}\n${c.hint}`;
}
