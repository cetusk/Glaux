// 設計画面(曲の設計データ)の中身と、タイムラインのクリップの印。
// 中身はバックエンドの get_design(glaux_core::designcheck::design_view + 計画の一覧・履歴)。
// 段階 1 は見るだけ(直すのはチャットで AI に頼む。画面で直すのは段階 2)。
import { invoke } from "@tauri-apps/api/core";

export interface DesignSection {
  id?: string;
  name: string;
  start_bar: number;
  bars: number;
  /** 計画の盛り上がり(形の平均) */
  planned?: number;
  /** 区間の中の形 [位置 0〜1, 値 0〜10] */
  curve?: [number, number][];
  join?: "smooth" | "step";
  /** 測った盛り上がり 0〜10 */
  measured: number;
}

export interface DesignCell {
  section: string;
  planned?: number;
  function?: string;
  planned_register?: [number, number];
  locked?: boolean;
  measured: number;
  notes: number;
  register?: [number, number];
  density: number;
}

export interface DesignPart {
  track_id: string;
  name: string;
  color?: string;
  plan_id?: string;
  function?: string;
  estimated?: boolean;
  cells: DesignCell[];
}

export interface DesignDeviation {
  severity: "warn" | "info";
  section?: string;
  track?: string;
  what: string;
  fix: string;
}

export interface ClipState {
  clip_id: string;
  track: string;
  plan?: "in_sync" | "ahead" | "missing";
  edited_bars?: [number, number][];
  locked_notes?: number;
}

export interface SongPlan {
  genre?: string;
  mood?: string[];
  key?: string;
  arc?: string;
  brightness?: number;
  density?: number;
  organic?: number;
  loudness?: number;
  musts?: string[];
  refs?: string[];
  note?: string;
}

export interface PlanInfo {
  plan_id: string;
  name: string;
  kind: string;
  rev: number;
  state?: string | null;
}

export interface PlanHistoryEntry {
  entry_id: string;
  time: string;
  author: { kind: "human" } | { kind: "ai"; model: string } | { kind: "system" };
  subject: string;
  plan_id: string;
  op: string;
  rev?: number;
  why?: string;
  trigger?: { kind: string; text: string };
  reverts?: string;
}

export interface DesignData {
  project_version: number;
  song?: SongPlan;
  song_plan_id?: string;
  song_estimated?: boolean;
  sections: DesignSection[];
  parts: DesignPart[];
  deviations: DesignDeviation[];
  clips: ClipState[];
  plans: PlanInfo[];
  history: PlanHistoryEntry[];
  history_total: number;
  redoable: number;
}

/** 働き・段階・盛り上がりの型の名前(バックエンドの glaux_core::plan と同じ) */
export const FUNCTIONS: Record<string, string> = {
  beat: "ビート(拍の土台)",
  bass: "低音の土台",
  sub: "サブ",
  harmony: "和声の支え",
  rhythm: "リズムの彩り",
  lead: "主役",
  hook: "フック",
  answer: "合いの手",
  texture: "質感・空気",
  ear_candy: "飾り(一度きりの小技)",
  transition: "つなぎ",
};
export const PRESENCE = ["鳴らさない", "気配", "背景", "支え", "前面", "主役"];
export const ARCS: Record<string, string> = {
  rise: "段々に上がる",
  waves: "波を 2 回",
  peak: "山を 1 つ",
  sink: "沈んでいく",
  flat: "平ら(ループ向け)",
};

export const designStore = $state<{
  data: DesignData | null;
  loading: boolean;
  error: string | null;
  /** タイムラインのクリップの印(クリップ ID → 状態) */
  clips: Record<string, ClipState>;
}>({ data: null, loading: false, error: null, clips: {} });

let seq = 0;

/** 設計画面の中身を読み直す(設計画面を開いている間、曲・計画が変わるたびに呼ぶ) */
export async function refreshDesign(): Promise<void> {
  const my = ++seq;
  designStore.loading = true;
  try {
    const d = await invoke<DesignData>("get_design", { limit: 100 });
    if (my !== seq) return;
    designStore.data = d;
    designStore.error = null;
    designStore.clips = Object.fromEntries(d.clips.map((c) => [c.clip_id, c]));
  } catch (e) {
    if (my === seq) designStore.error = String(e);
  } finally {
    if (my === seq) designStore.loading = false;
  }
}

let clipSeq = 0;

/** タイムラインのクリップの印だけを読み直す(軽い。盛り上がりなどは測らない) */
export async function refreshClipStates(): Promise<void> {
  const my = ++clipSeq;
  try {
    const r = await invoke<{ clips: ClipState[] }>("clip_states");
    if (my !== clipSeq) return;
    designStore.clips = Object.fromEntries(r.clips.map((c) => [c.clip_id, c]));
  } catch {
    // 印が出ないだけで、編集には関係しない
  }
}

/** 設計画面で選んでいる所(チャットの対象にもなる) */
export type DesignSel =
  | { kind: "none" }
  | { kind: "song" }
  | { kind: "lane"; lane: "sections" | "curve" | "band" | "table" }
  | { kind: "section"; i: number }
  | { kind: "part"; p: number }
  | { kind: "cell"; p: number; i: number };

export const designSel = $state<{ sel: DesignSel; part: number }>({ sel: { kind: "none" }, part: 0 });

/** 選んでいる所の名前(チャットの対象の表示)。何も選んでいなければ null */
export function designTargetLabel(d: DesignData | null, sel: DesignSel): string | null {
  switch (sel.kind) {
    case "none":
      return null;
    case "song":
      return "曲全体";
    case "lane":
      return { sections: "区間の構成全体", curve: "盛り上がり全体", band: "音域全体(全パート)", table: "役割全体" }[sel.lane];
    case "section":
      return d?.sections[sel.i] ? `区間「${d.sections[sel.i].name}」` : null;
    case "part":
      return d?.parts[sel.p] ? `パート「${d.parts[sel.p].name}」` : null;
    case "cell":
      return d?.parts[sel.p] && d.sections[sel.i] ? `${d.parts[sel.p].name} / ${d.sections[sel.i].name}` : null;
  }
}

/** チャットに添える対象の説明(AI が読む。ID も添える) */
export function designTargetPrompt(d: DesignData | null, sel: DesignSel): string | null {
  const label = designTargetLabel(d, sel);
  if (!label || !d) return null;
  let ids = "";
  if (sel.kind === "section") ids = d.sections[sel.i]?.id ? `(区間 ${d.sections[sel.i].id})` : "";
  if (sel.kind === "part") ids = `(トラック ${d.parts[sel.p]?.track_id})`;
  if (sel.kind === "cell") {
    const s = d.sections[sel.i];
    ids = `(トラック ${d.parts[sel.p]?.track_id}、区間 ${s?.id ?? s?.name}、${s?.start_bar}〜${(s?.start_bar ?? 0) + (s?.bars ?? 1) - 1} 小節)`;
  }
  return (
    `【設計画面で選んでいる所】${label}${ids}。` +
    "計画と実際は get_design で読める。計画を直すなら set_song_plan(区間)・save_plan / edit_plan(kind song・part)、" +
    "音を直すなら人の手直し(edited_bars)と固定の音を残す。\n"
  );
}

/** 盛り上がりの形(無ければ平ら)。値は 0〜10 */
export function sectionCurve(s: DesignSection): [number, number][] | null {
  if (s.curve && s.curve.length > 0) return s.curve;
  if (s.planned == null) return null;
  return [
    [0, s.planned],
    [1, s.planned],
  ];
}

/** MIDI のノート番号を音名に */
export function noteName(p: number): string {
  const N = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  return N[((p % 12) + 12) % 12] + (Math.floor(p / 12) - 1);
}
