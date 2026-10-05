// 設計画面(曲の設計データ)の中身と、タイムラインのクリップの印。
// 中身はバックエンドの get_design(glaux_core::designcheck::design_view + 計画の一覧・履歴)。
// 画面で直す操作(計画の保存・区間・メモ・取り消し)もここ。人の操作として履歴に残る。
import { invoke } from "@tauri-apps/api/core";
import { abClear, abSetSide, applyEdit, getHistory, redo as songRedo, undo as songUndo } from "./api";
import { claimAb, endAbLoop, ensureAbPlaying, releaseAb, restartAb, startAbLoop } from "./abLoop";
import { addBars, barHead, defaultAbRange } from "./abRange";
import { showToast } from "./toast.svelte";
import type { Project } from "./types";

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
  /** 省略 = 採用済み / estimated = 推定(未確認)/ proposal = AI の案(枝) */
  state?: string | null;
  /** 派生元(案の元の計画) */
  derived_from?: { id: string; rev: number } | null;
  /** 案の音の編集の数 */
  edits?: number;
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
  /** 一組の曲の履歴の項目(案の採用など。計画の側だけでは取り消せない) */
  song_entry?: string;
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
  /** 案ごとの「変わる所」(案の ID → 区間の番号・範囲〈tick〉・曲全体に効くか・今の曲に当てられないか) */
  proposal_changes?: Record<string, ProposalChanges>;
}

export interface ProposalChanges {
  sections?: number[];
  ranges?: [number, number][];
  whole?: boolean;
  stale?: boolean;
  /** 当てられない理由(案を出した後に直された所の名前など) */
  why?: string;
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

// ---------------------------------------------------------------- 画面で直す(段階 2)


export interface Memo {
  target: string;
  text: string;
  when?: string;
}

/** 計画の中身(画面が直すときに丸ごと書き戻す。旋律の計画には無い) */
export function planBody(d: DesignData | null, planId: string | undefined): Record<string, unknown> | null {
  if (!d || !planId) return null;
  const p = d.plans.find((x) => x.plan_id === planId) as (PlanInfo & { body?: Record<string, unknown> }) | undefined;
  return p?.body ? structuredClone($state.snapshot(p.body) as Record<string, unknown>) : null;
}

// 設計画面の取り消し: 1 回の操作で直した先(曲〈区間・テンポ・案の採用〉と計画)の履歴の項目を覚えておき、まとめて戻す。
// 覚えが無ければ計画を 1 件戻す。案の採用は曲の編集 1 件として覚える(計画の変更は曲の編集と一組で記録してあり、
// 曲の側で戻すと、どの画面からでも計画も一緒に戻る)。
// 戻す・やり直す前に、覚えた項目がまだいちばん新しい(次にやり直す)かを確かめる(別の画面や AI が後から直していたら、それを戻してしまうので)。
// ほかの画面ですでに戻された(やり直された)操作は飛ばす
interface UndoUnit {
  song: number;
  plan: number;
  /** その操作が書いた曲の履歴の項目 */
  songEntry?: string;
  /** その操作が書いた計画の履歴の項目 */
  planEntries?: string[];
  label?: string;
}
const undoStack: UndoUnit[] = [];
const redoStack: UndoUnit[] = [];
/** 1 件ずつの操作を覚える(`ids` は書いた履歴の項目。古い順) */
function noteEdit(kind: "song" | "plan", ids: (string | null | undefined)[] = [undefined]) {
  for (const id of ids)
    undoStack.push(kind === "song" ? { song: 1, plan: 0, songEntry: id ?? undefined } : { song: 0, plan: 1, planEntries: id ? [id] : undefined });
  redoStack.length = 0;
}
function noteUnit(u: UndoUnit) {
  undoStack.push(u);
  redoStack.length = 0;
}
const sameSet = (a: string[], b: string[]) => a.length === b.length && b.every((x) => a.includes(x));

/** 覚えた項目が、今も取り消せる位置(redo なら次にやり直す位置)にあるか。違えば、変わっている方の名前 */
async function moved(u: UndoUnit, redo: boolean): Promise<string | null> {
  if (u.song && u.songEntry) {
    const h = await getHistory(1);
    const at = redo ? h.redoable[0]?.id : h.entries[h.entries.length - 1]?.id;
    if (at !== u.songEntry) return "曲";
  }
  if (u.plan && u.planEntries?.length) {
    const h = await invoke<{ applied: string[]; redoable: string[] }>("plan_head", { n: u.planEntries.length });
    if (!sameSet(redo ? h.redoable : h.applied, u.planEntries)) return "計画";
  }
  return null;
}

/** 覚えた操作が、今効いているか(曲の編集が曲の履歴に効いている・計画の項目が残っている) */
async function applied(u: UndoUnit): Promise<boolean> {
  if (u.song && u.songEntry) {
    const h = await getHistory();
    if (!h.entries.some((e) => e.id === u.songEntry)) return false;
  }
  if (u.plan && u.planEntries?.length) {
    const h = await invoke<{ present: string[] }>("plan_head", { n: 1, ids: u.planEntries });
    if (h.present.length < u.planEntries.length) return false;
  }
  return true;
}

/** ほかの画面(タイムラインの Ctrl+Z・履歴パネル・AI)で戻した・やり直した操作に合わせて、覚えた操作を並べ直す:
 *  戻された操作はやり直せる側へ、やり直された操作は取り消せる側へ */
async function reconcile(): Promise<void> {
  while (undoStack.length && !(await applied(undoStack[undoStack.length - 1]))) redoStack.push(undoStack.pop()!);
  while (redoStack.length && (await applied(redoStack[redoStack.length - 1]))) undoStack.push(redoStack.pop()!);
}

export async function designUndo(): Promise<void> {
  const had = undoStack.length > 0;
  await reconcile();
  let u = undoStack.pop();
  if (!u && had) {
    // 覚えていた操作はどれもほかの画面で戻されていた(ここで計画を 1 件戻すと、曲と食い違うことがある)
    showToast("warn", "設計画面で直した所は、ほかの画面ですでに戻されています");
    return;
  }
  u ??= { song: 0, plan: 1 };
  try {
    const m = await moved(u, false);
    if (m) {
      undoStack.push(u);
      showToast("warn", `${u.label ?? "この操作"}の後に${m}が直されているので、ここでは取り消せません(履歴パネル・計画の履歴で戻してください)`);
      return;
    }
    // 後に書いた方(計画)から戻す
    if (u.plan) {
      const r = await invoke<{ done: number }>("plan_step", { n: u.plan, redo: false });
      if (!r.done && !u.song) {
        showToast("warn", "取り消せる計画の変更はありません");
        return;
      }
    }
    for (let k = 0; k < u.song; k++) await songUndo();
    redoStack.push(u);
    if (u.label) showToast("ok", `${u.label}を取り消しました`);
  } catch (e) {
    showToast("error", `取り消せませんでした: ${e}`);
  }
}

export async function designRedo(): Promise<void> {
  const had = redoStack.length > 0;
  await reconcile();
  let u = redoStack.pop();
  if (!u && had) {
    showToast("warn", "設計画面で取り消した所は、ほかの画面ですでにやり直されています");
    return;
  }
  u ??= { song: 0, plan: 1 };
  try {
    const m = await moved(u, true);
    if (m) {
      redoStack.length = 0;
      showToast("warn", `取り消した後に${m}が直されているので、やり直せません`);
      return;
    }
    for (let k = 0; k < u.song; k++) await songRedo();
    if (u.plan) {
      const r = await invoke<{ done: number }>("plan_step", { n: u.plan, redo: true });
      if (!r.done && !u.song) {
        showToast("warn", "やり直せる計画の変更はありません");
        return;
      }
    }
    undoStack.push(u);
    if (u.label) showToast("ok", `${u.label}をやり直しました`);
  } catch (e) {
    showToast("error", `やり直せませんでした: ${e}`);
  }
}

/** 計画を保存する(作る・置き換える)。人の操作として計画の履歴に残る */
export async function savePlan(
  planId: string | null,
  kind: string,
  name: string,
  body: unknown,
  label: string,
  planState?: "estimated" | "adopted",
): Promise<string | null> {
  try {
    const r = await invoke<{ plan_id: string; entry_id?: string }>("plan_save", { planId, name, kind, body, planState, label });
    noteEdit("plan", [r.entry_id]);
    refreshDesign();
    return r.plan_id;
  } catch (e) {
    showToast("error", `「${label}」を保存できませんでした: ${e}`);
    return null;
  }
}

/** 区間(曲のデータ)を直す。区間の並びを丸ごと置き換える 1 件の編集(曲の履歴に残る) */
export async function editSections(
  project: Project,
  mutate: (secs: NonNullable<Project["sections"]>) => void,
  label: string,
): Promise<boolean> {
  const secs = structuredClone($state.snapshot(project.sections ?? []) as NonNullable<Project["sections"]>);
  secs.sort((a, b) => a.tick - b.tick);
  mutate(secs);
  try {
    const r = await applyEdit([{ op: "set_sections", sections: secs }], label);
    noteEdit("song", [r?.entry_id]);
    return true;
  } catch {
    return false;
  }
}

/** テンポ(曲のデータ。途中で変わらない曲だけ) */
export async function setTempo(bpm: number): Promise<void> {
  try {
    const r = await applyEdit([{ op: "set_tempo", events: [{ tick: 0, bpm }] }], `テンポを ${bpm} BPM に`);
    noteEdit("song", [r?.entry_id]);
  } catch {
    // applyEdit がエラーを出す
  }
}

/** 区間に ID が無ければ付ける(パートの計画は区間を ID で指すため)。付けたら読み直して true */
export async function ensureSectionIds(project: Project): Promise<boolean> {
  if ((project.sections ?? []).every((s) => s.id)) return false;
  await editSections(project, () => {}, "区間に ID を付ける");
  await refreshDesign();
  return true;
}

/** 計画の無い所(パート・曲全体)を、今の音から推定する。推定は未確認の扱いで、採用するまで AI は参考としてだけ使う。
 *  区間に ID が無ければ先に付ける。`auto` は設計画面を開いたときの自動の推定(推定できる所が無くても黙る) */
export async function estimatePlans(project: Project, auto = false): Promise<void> {
  try {
    await ensureSectionIds(project);
    const r = await invoke<{ created: string[]; song: boolean; entries: string[] }>("plan_estimate");
    if (r.entries.length) noteUnit({ song: 0, plan: r.entries.length, planEntries: r.entries, label: "計画の推定" });
    const what = [r.song ? "曲全体" : "", r.created.length ? `${r.created.length} パート` : ""].filter(Boolean).join("と ");
    if (what) showToast("ok", `今の音から${what}の計画を推定しました。上の帯で確かめて、採用してください`);
    else if (!auto) showToast("warn", "推定できる所がありませんでした(計画の無い、音のあるパートがありません)");
    refreshDesign();
  } catch (e) {
    if (!auto) showToast("error", `推定できませんでした: ${e}`);
  }
}

/** 一度も計画を作っていない曲で設計画面を開いたら、1 回だけ今の音から推定する(区間が無い曲ではしない) */
const autoEstimated = new Set<string>();
export function maybeAutoEstimate(project: Project, d: DesignData | null): void {
  if (!d || d.plans.length > 0 || d.history_total > 0 || !(project.sections ?? []).length) return;
  if (!d.parts.some((p) => p.cells.some((c) => c.notes > 0))) return;
  const key = `${project.meta?.title ?? ""}|${project.meta?.created ?? ""}`;
  if (autoEstimated.has(key)) return;
  autoEstimated.add(key);
  estimatePlans(project, true);
}

/** 曲全体の計画を直す(無ければ作る) */
export async function editSongPlan(mutate: (body: Record<string, unknown>) => void, label: string): Promise<void> {
  const d = designStore.data;
  const body = planBody(d, d?.song_plan_id) ?? {};
  mutate(body);
  await savePlan(d?.song_plan_id ?? null, "song", "曲全体", body, label);
}

/** パートの計画を直す(無ければ作る)。区間の ID が無ければ先に付ける */
export async function editPartPlan(
  project: Project,
  partIndex: number,
  mutate: (body: { track: string; function?: string; sections: Record<string, unknown>[] }, d: DesignData) => void,
  label: string,
): Promise<void> {
  await ensureSectionIds(project);
  const d = designStore.data;
  const part = d?.parts[partIndex];
  if (!d || !part) return;
  if (d.sections.some((s) => !s.id)) {
    showToast("error", "区間が無いので、パートの計画を作れません(set_song_plan か、タイムラインでマーカーを置いてください)");
    return;
  }
  const body = (planBody(d, part.plan_id) ?? { track: part.track_id, sections: [] }) as {
    track: string;
    function?: string;
    sections: Record<string, unknown>[];
  };
  body.track = part.track_id;
  body.sections = body.sections ?? [];
  mutate(body, d);
  await savePlan(part.plan_id ?? null, "part", part.name, body, label);
}

/** パートの計画の、区間 `i` の項目(無ければ測った段階で作る) */
export function partCell(body: { sections: Record<string, unknown>[] }, d: DesignData, partIndex: number, i: number) {
  const sid = d.sections[i]?.id ?? "";
  let e = body.sections.find((s) => s.section === sid);
  if (!e) {
    e = { section: sid, presence: d.parts[partIndex]?.cells[i]?.measured ?? 3 };
    body.sections.push(e);
  }
  return e as {
    section: string;
    presence: number;
    function?: string;
    register?: [number, number];
    locked?: boolean;
    density?: number;
    rhythm?: string;
    note?: string;
  };
}

/** 所のメモ(曲全体の計画の memos) */
export function memosFor(d: DesignData | null, target: string): Memo[] {
  const m = (d?.song as (SongPlan & { memos?: Memo[] }) | undefined)?.memos ?? [];
  return m.filter((x) => x.target === target);
}

export async function addMemo(target: string, text: string): Promise<void> {
  await editSongPlan((b) => {
    const list = (b.memos as Memo[] | undefined) ?? [];
    list.push({ target, text, when: new Date().toISOString() });
    b.memos = list;
  }, "メモを残す");
}

/** 推定した計画をまとめて採用する・捨てる */
export async function settleEstimated(adopt: boolean): Promise<void> {
  try {
    const r = await invoke<{ entries: string[] }>("plan_settle_estimated", { adopt });
    noteEdit("plan", r.entries);
    showToast("ok", adopt ? "推定した計画を採用しました" : "推定した計画を捨てました");
    refreshDesign();
  } catch (e) {
    showToast("error", `${adopt ? "採用" : "捨てる"}できませんでした: ${e}`);
  }
}

/** 計画を前の版の中身に戻す(戻したことも新しい版として残る) */
export async function restorePlan(planId: string, rev: number): Promise<void> {
  try {
    const r = await invoke<{ entry_id?: string }>("plan_restore", { planId, rev });
    noteEdit("plan", [r?.entry_id]);
    showToast("ok", `版 ${rev} に戻しました(戻したことも新しい版として残ります)`);
    refreshDesign();
  } catch (e) {
    showToast("error", `戻せませんでした: ${e}`);
  }
}

/** 計画の途中の変更だけを取り消す(後の変更は残す) */
export async function revertPlanEntry(entryId: string): Promise<void> {
  try {
    const r = await invoke<{ conflicts: string[]; entry_id?: string }>("plan_revert", { entryId });
    noteEdit("plan", [r.entry_id]);
    showToast(
      "ok",
      r.conflicts.length
        ? `取り消しました(後で同じ計画を ${r.conflicts.length} 回直しています。結果を確かめてください)`
        : "この変更だけ取り消しました",
    );
    refreshDesign();
  } catch (e) {
    showToast("error", `取り消せませんでした: ${e}`);
  }
}

/** チャットで AI に頼む(設計画面で選んでいる所を対象に添えて送る) */
export function askChat(text: string): void {
  window.dispatchEvent(new CustomEvent("glaux:chat-send", { detail: text }));
}

// ---------------------------------------------------------------- 案(枝。段階 3)

/** 案の一覧(AI が propose_design で出した、今の計画・曲には効かない派生の計画) */
export function proposals(d: DesignData | null): PlanInfo[] {
  return (d?.plans ?? []).filter((p) => p.state === "proposal");
}

/** 案の聴き比べで用意した音の情報(0 番 = 今、1 番〜 = 案。バックエンドの ab::AbManyInfo) */
export interface ProposalAbInfo {
  /** それぞれの統合ラウドネス(LUFS。短すぎ・無音なら null) */
  lufs: (number | null)[];
  /** そろえるために掛けた量(dB。0 か負) */
  gains_db: number[];
  from_secs: number;
  to_secs: number;
  /** それぞれが今と違い始める位置(範囲の頭からの秒。今自身と、今と同じ音は null) */
  first_diff_secs: (number | null)[];
  end_tick: number;
}

/** いっしょに聴き比べられる案の数(今 + 4 つ。バックエンドの ab::MAX_TAKES - 1) */
export const MAX_PROPOSALS_AB = 4;

/** 鳴らす音の字(0 = A = 今、1 = B …) */
export const abLetter = (i: number): string => "ABCDE"[i] ?? "?";

/** 案の聴き比べの状態(A = 今、B・C … = 案。案 1 つなら行ごと、2 つ以上なら「まとめて」) */
export const proposalAb = $state<{
  /** 聴き比べている案(空 = 聴き比べていない) */
  planIds: string[];
  /** 鳴らしている音(0 = 今、1〜 = planIds の順の案) */
  side: number;
  info: ProposalAbInfo | null;
  /** 用意している最中(1 つの案ならその ID、まとめてなら "all") */
  busy: string | null;
  /** 聴いている範囲(tick)と区間(区間で選んだとき) */
  start: number;
  end: number;
  section: number | null;
}>({
  planIds: [],
  side: 1,
  info: null,
  busy: null,
  start: 0,
  end: 0,
  section: null,
});

/** 案を 1 つ聴き比べている(行の中に切り替えを出す) */
export const abSingle = (planId: string): boolean => proposalAb.planIds.length === 1 && proposalAb.planIds[0] === planId;

/** 案(1 つ以上)を今と聴き比べる(範囲の頭から、最初の案を鳴らす) */
export async function abProposals(planIds: string[], start: number, end: number, section: number | null = null): Promise<void> {
  if (!planIds.length) return;
  claimAb("proposal", () => void endProposalAb());
  proposalAb.busy = planIds.length === 1 ? planIds[0] : "all";
  try {
    const info = await invoke<ProposalAbInfo>("ab_prepare_proposals", {
      planIds,
      startTick: Math.max(0, Math.round(start)),
      endTick: Math.max(0, Math.round(end)),
    });
    proposalAb.planIds = [...planIds];
    proposalAb.side = 1;
    proposalAb.info = info;
    // 範囲は長すぎると切り詰められる(end_tick)。その範囲をループにして頭から鳴らす
    const e = info.end_tick && info.end_tick > start ? info.end_tick : end;
    proposalAb.start = start;
    proposalAb.end = e;
    proposalAb.section = section;
    await startAbLoop(start, e);
  } catch (e) {
    showToast("error", `聴き比べを用意できませんでした: ${e}`);
  } finally {
    proposalAb.busy = null;
  }
}

/** 案(1 つ以上)を聴く範囲: いちばん多くの案が変わる区間(同じなら前の方)。区間に無ければ最初に変わる所から、
 *  何も分からなければ今の位置から(チャットから聴き比べるとき。設計画面は選んでいる区間も見る) */
export function proposalsRange(
  project: Project,
  d: DesignData | null,
  planIds: string[],
): { start: number; end: number; section: number | null } {
  const secs = [...(project.sections ?? [])].sort((a, b) => a.tick - b.tick);
  const count = new Map<number, number>();
  for (const id of planIds) for (const si of d?.proposal_changes?.[id]?.sections ?? []) count.set(si, (count.get(si) ?? 0) + 1);
  const best = [...count.entries()].sort((a, b) => b[1] - a[1] || a[0] - b[0])[0]?.[0];
  if (best != null && secs[best]) {
    const start = secs[best].tick;
    const end = secs[best + 1]?.tick ?? addBars(project, start, d?.sections[best]?.bars ?? 8);
    return { start, end, section: best };
  }
  const first = planIds.map((id) => d?.proposal_changes?.[id]?.ranges?.[0]).find((r) => r);
  if (first) {
    const start = barHead(project, first[0]);
    return { start, end: Math.max(first[1], addBars(project, start, 4)), section: null };
  }
  return { ...defaultAbRange(project), section: null };
}

/** 鳴らす音を切り替える(0 = 今、1〜 = 案)。同じ位置から続けて鳴る */
export async function setProposalSide(side: number): Promise<void> {
  if (side < 0 || side > proposalAb.planIds.length) return;
  proposalAb.side = side;
  abSetSide(abLetter(side).toLowerCase() as "a" | "b" | "c" | "d" | "e").catch(() => {});
  // 止まっている・範囲の外にいるなら、範囲の頭から鳴らす(押せば聞こえるように)
  if (proposalAb.planIds.length) await ensureAbPlaying(proposalAb.start, proposalAb.end).catch(() => {});
}

/** 聴いている範囲の頭から聴き直す */
export async function restartProposalAb(): Promise<void> {
  if (proposalAb.planIds.length) await restartAb(proposalAb.start).catch(() => {});
}

export async function endProposalAb(): Promise<void> {
  releaseAb("proposal");
  if (!proposalAb.planIds.length) return;
  proposalAb.planIds = [];
  proposalAb.info = null;
  abClear().catch(() => {});
  await endAbLoop();
}

/** 案を採用する(案の音を曲に当て、案の計画を今の計画にする)。同じ元の計画・同じ頼みから出たほかの案と、
 *  いっしょに聴き比べていた案は捨てる(計画の履歴に残るので取り消せる) */
export async function adoptProposal(planId: string, name: string): Promise<void> {
  const alsoDiscard = proposalAb.planIds.filter((id) => id !== planId);
  await endProposalAb();
  try {
    const r = await invoke<{
      discarded?: string[];
      entry_id?: string | null;
      plan_entries?: number;
      plan_entry_ids?: string[];
      kept_locked?: { what: string }[];
    }>("plan_adopt_proposal", {
      planId,
      alsoDiscard,
    });
    const gone = r?.discarded ?? [];
    // 1 回の Ctrl+Z で、曲に当てた音と計画の変更(ほかの案を捨てたことも)をまとめて戻せるように。
    // 計画の変更は曲の採用の編集と一組で記録してあるので、曲の編集を戻せば(どの画面からでも)計画も一緒に戻る
    noteUnit(
      r?.entry_id
        ? { song: 1, plan: 0, songEntry: r.entry_id, label: `案「${name}」の採用` }
        : { song: 0, plan: r?.plan_entries ?? 1, planEntries: r?.plan_entry_ids, label: `案「${name}」の採用` },
    );
    showToast(
      "ok",
      `案「${name}」を採用しました${gone.length ? `。ほかの案(${gone.map((n) => `「${n}」`).join("")})は捨てました` : ""}(曲と計画の履歴に残り、取り消せます)`,
    );
    // 固定の音を守るために外した編集(採用はしたが、聴き比べた音とは一部違う)
    if (r?.kept_locked?.length)
      showToast("warn", `固定の音を守るため、案の編集の一部は当てませんでした(${r.kept_locked.map((k) => k.what).join(" / ")})`, {
        ms: 9000,
      });
    refreshDesign();
  } catch (e) {
    showToast("error", `採用できませんでした: ${e}`);
  }
}

/** 案を捨てる */
export async function discardProposal(planId: string, name: string): Promise<void> {
  if (proposalAb.planIds.includes(planId)) await endProposalAb();
  try {
    const r = await invoke<{ entry_id?: string }>("plan_delete", { planId, label: `案「${name}」を捨てる` });
    noteUnit({ song: 0, plan: 1, planEntries: r?.entry_id ? [r.entry_id] : undefined, label: `案「${name}」を捨てたこと` });
    showToast("ok", `案「${name}」を捨てました`);
    refreshDesign();
  } catch (e) {
    showToast("error", `捨てられませんでした: ${e}`);
  }
}
