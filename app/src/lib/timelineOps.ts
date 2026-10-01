// タイムラインの純粋な計算(クリップの複製・ループの展開・拍子とテンポの列・マーカー)。
// 画面の状態に触れないので、Timeline と子のメニューから共通に使う
import { newClipId, newNoteId } from "./ids";
import type { Clip, Project } from "./types";

/// `tick` の位置のテンポ(BPM)
export function bpmAt(project: Project, tick: number): number {
  let bpm = project.tempo_map[0]?.bpm ?? 120;
  for (const e of project.tempo_map) {
    if (e.tick <= tick) bpm = e.bpm;
  }
  return bpm;
}

/// ループクリップの繰り返しを実際のノートに展開した replace_clip(ループは解除)。
export function expandLoopCommand(clip: Clip): Record<string, unknown> | null {
  if (clip.kind !== "midi" || !clip.loop || !clip.loop_len) return null;
  const L = clip.loop_len;
  const notes: Record<string, unknown>[] = [];
  for (let offset = 0; offset < clip.length; offset += L) {
    for (const n of clip.notes) {
      if (n.pos >= L) continue;
      const pos = n.pos + offset;
      if (pos >= clip.length) continue;
      const end = Math.min(n.pos + n.dur, L) + offset;
      notes.push({ ...n, id: newNoteId(), pos, dur: Math.min(end, clip.length) - pos });
    }
  }
  const c = JSON.parse(JSON.stringify(clip)) as Record<string, unknown>;
  c.notes = notes;
  c.loop = false;
  delete c.loop_len;
  return { op: "replace_clip", id: clip.id, clip: c };
}

/// クリップを新しい ID(ノートも新 ID)で複製した add_clip 用の JSON を作る
export function cloneClip(clip: Clip, start: number): Record<string, unknown> {
  const c = JSON.parse(JSON.stringify(clip)) as Record<string, unknown>;
  c.id = newClipId();
  c.start = Math.max(0, Math.round(start));
  if (clip.kind === "midi") {
    c.notes = clip.notes.map((n) => ({ ...n, id: newNoteId() }));
  }
  return c;
}

/// ルーラーの右クリックメニュー(拍子・テンポ・マーカー)の状態。入力欄の値は文字列のまま持つ
export interface RulerMenuState {
  x: number;
  y: number;
  barIndex: number;
  num: string;
  den: string;
  grouping: string;
  bpm: string;
}

export type SigEvent = { tick: number; num: number; den: number; grouping?: number[] };

/// 拍子イベント列を整える: tick 順に並べ、直前と同じ拍子の変更は取り除く
export function normalizeSigs(events: SigEvent[]): SigEvent[] {
  const sorted = [...events].sort((a, b) => a.tick - b.tick);
  const out: typeof sorted = [];
  const same = (a?: number[], b?: number[]) => (a ?? []).join("+") === (b ?? []).join("+");
  for (const ev of sorted) {
    const prev = out[out.length - 1];
    if (prev && prev.num === ev.num && prev.den === ev.den && same(prev.grouping, ev.grouping)) continue;
    out.push(ev);
  }
  if (out.length === 0 || out[0].tick !== 0) out.unshift({ tick: 0, num: 4, den: 4 });
  return out;
}

/// セクションマーカー(曲の構成)
export interface Marker {
  tick: number;
  name: string;
  /// 曲の計画書(set_song_plan)の中身。マーカーを動かしても名前を変えても残す
  energy?: number;
  tracks?: string[];
  note?: string;
}

/// マーカーに乗せたときに出す、計画書の中身(無ければ空)
export function planText(m: Marker): string {
  const lines: string[] = [];
  if (m.energy !== undefined && m.energy !== null) lines.push(`計画の盛り上がり: ${m.energy} / 10`);
  if (m.tracks && m.tracks.length > 0) lines.push(`鳴らすトラック: ${m.tracks.join("・")}`);
  if (m.note) lines.push(`役割: ${m.note}`);
  return lines.length > 0 ? `\n${lines.join("\n")}` : "";
}

/** 入力欄を開いたら全選択してフォーカス */
export function focusSelect(el: HTMLInputElement) {
  el.focus();
  el.select();
}

/// 音声クリップがテンポに追従しているなら、元の素材の BPM
export function followBpm(clip: Clip): number | null {
  return clip.kind === "audio" && clip.stretch?.mode === "follow" ? clip.stretch.original_bpm : null;
}

/// クリップの右クリックメニューの項目
export type ClipMenuAction =
  | "split"
  | "split-head"
  | "dup"
  | "copy"
  | "cut"
  | "delete"
  | "loop-on"
  | "loop-off"
  | "expand"
  | "follow-on"
  | "follow-detect"
  | "follow-off"
  | "sep-builtin"
  | "sep-demucs"
  | "match"
  | "similar";
