// 音色エディタの状態・履歴・音のつなぎ。
//
// - 楽器の画面(editors/*.ts)は「音色の値 d」を持ち、つまみや絵を動かすと d を書き換えて描き直す。
//   離したときに pushHist(言葉) を呼ぶと、確定した値との差を Command(set_param など)にして 1 件の編集として当てる。
// - ドラッグ中は d と確定した値の差を preview_edit で音にだけ当てる(履歴には残さない)。
// - 取り消しはトラックごと: いちばん新しい編集がこのトラックのものなら普通の取り消し、そうでなければ
//   「この変更だけ取り消す」(revert)で戻す。AI がこのトラックの音色を変えたときも同じ並びに積む。
// - A(作り込む前)を聴いている間は、開いたときの音源を試聴に当て、画面もその値に固定して編集させない。
// - 画面だけの設定(UI_KEYS: どれを調整しているか・音域・道具など)は音色に入れず、取り消しでも戻さない。
import * as api from "../api";
import { tr } from "../i18n.svelte";
import { showToast } from "../toast.svelte";
import type { EntrySummary, Project, Track } from "../types";
import { clone } from "./dom";

export type Data = Record<string, any>;

/** つまみの今の値(get_track_params の current。書いていないつまみは既定値) */
export type Params = Record<string, any>;

export interface Loaded {
  params: Params;
  track: Track;
  project: Project;
}

/** 楽器ごとの画面 */
export interface Editor {
  /** 内蔵の音源の名前(fm4 など)か、device の種類(sampler / sfz) */
  kind: string;
  title: () => string;
  /** 音色の値を、つまみの今の値(と device)から作る。画面だけの設定は入れない */
  load(l: Loaded): Data | Promise<Data>;
  /** 画面だけの設定の初期値(初めて開いたとき) */
  uiInit?(): Data;
  /** 音色の値 → つまみの値(set_param する物。名前 → 値) */
  params(d: Data): Params;
  /** つまみ以外に当てる Command(device の差し替えなど)。prev → next */
  extraCommands?(trackId: string, prev: Data, next: Data): unknown[];
  /** つまみ以外の中身の印(ウェーブテーブルの作り方の手順など)。変わったら commit で当てる */
  signature?(d: Data): string;
  /** 印が変わったときの確定(つまみの差 cmds も一緒に 1 件の編集にする)。entry_id と、確定した値へ足す物を返す */
  commit?(cmds: unknown[], label: string, next: Data): Promise<{ entry_id: string; patch?: Data }>;
  /** 印が変わっている間の試聴に足す Command(仮のテーブルなど) */
  previewExtra?(next: Data): Promise<unknown[]>;
  /** 「LFO・エンベロープも動かす」を切ったときに試聴で 0 にするつまみ */
  staticParams?: Params;
  render(body: HTMLElement): void;
  draw(): void;
  refresh(): void;
  onKey?(m: number, on: boolean): void;
  onDelete?(): boolean;
  help: () => [string, string][];
  /** 開いたとき・読み直したときに、絵の材料を取り直す(素材の波形など) */
  reload?(): void;
  [k: string]: any;
}

export const EDITORS: Record<string, Editor> = {};
export function register(e: Editor) {
  EDITORS[e.kind] = e;
}

/** 画面だけの設定: 音色を変えず、見え方や「どれを調整するか」だけを変える。取り消しの対象にせず、A / B を切り替えても今のまま */
export const UI_KEYS = ["sel", "selSlice", "selCut", "playing", "hit", "specT", "previewNote", "previewHi", "onlyLo", "showShelf", "tool", "selOp", "selKey", "preset"];
export const soundOf = (d: Data): Data => {
  const o = { ...d };
  UI_KEYS.forEach((k) => delete o[k]);
  return o;
};
export const uiOf = (d: Data): Data => Object.fromEntries(UI_KEYS.filter((k) => k in d).map((k) => [k, d[k]]));

export const state = {
  /** 作り込んでいるトラック */
  track: "",
  trackName: "",
  /** 楽器(EDITORS の鍵) */
  inst: "",
  ab: "now" as "now" | "before",
  /** トラックごと: 開いたときの音色の値と device(A の音) */
  initial: {} as Record<string, Data>,
  initialDevice: {} as Record<string, unknown>,
  /** トラックごと: 今の値(画面だけの設定を含む) */
  data: {} as Record<string, Data>,
  /** トラックごと: 確定した音色の値(曲に当たっている値) */
  committed: {} as Record<string, Data>,
  abStash: null as Data | null,
  project: null as Project | null,
};

/** 見出しの帯(Svelte)に見せる状態 */
export const view = $state({
  open: false,
  sel: null as string | null,
  ab: "now" as "now" | "before",
  canUndo: false,
  canRedo: false,
  loading: false,
  error: null as string | null,
  title: "",
});

export const cur = (): Data => state.data[state.track];
/** 今見せている値(A を聴いている間は作り込む前の値) */
export const shown = cur;
export const ed = (): Editor => EDITORS[state.inst];

/** 画面だけの状態(履歴に入れない): 欄ごとの道具・目盛りの長さ・描き直す欄・ドラッグ */
export const ui: {
  tool: Record<string, string>;
  panes: { draw: () => void }[];
  envT: Record<string, number>;
  envPickNeed: Record<string, number>;
  drag: { snap: string; target: Element; id: number } | null;
  dragCancelled: boolean;
} = { tool: {}, panes: [], envT: {}, envPickNeed: {}, drag: null, dragCancelled: false };

export const toolOf = (id: string, def = "view") => ui.tool[`${state.track}:${id}`] ?? def;
export const setTool = (id: string, t: string) => {
  ui.tool[`${state.track}:${id}`] = t;
  rerender();
};
export const toast = (t: string) => showToast("ok", t);
export const canEdit = () => {
  if (state.ab === "before") {
    showToast("warn", tr("A(作り込む前)を聴いている間は調整できません", "You can't edit while listening to A (before)"));
    return false;
  }
  return true;
};
/** 欄の絵を描き直す(つまみを動かしたときなど) */
export const drawPanes = () => ui.panes.forEach((p) => p.draw());
export function setSel(label: string | null) {
  view.sel = label;
}

// ---- 画面の組み立て ----
let body: HTMLElement | null = null;
let rootEl: HTMLElement | null = null;
export function mountBody(b: HTMLElement | null, root: HTMLElement | null) {
  body = b;
  rootEl = root;
}
/** 楽器の画面を作り直す(押している間の音を止め、A のときは入力を押せなくし、絵の高さをそろえて描く) */
export function rerender() {
  if (!body || !state.inst || !cur()) return;
  stopHeld();
  ui.panes = [];
  ed().render(body);
  if (state.ab === "before")
    body.querySelectorAll<HTMLInputElement | HTMLSelectElement>(".col.center input, .col.right input, .col.right select:not(.trackpick), .col.left input").forEach((i) => {
      if ((i as HTMLInputElement).type === "text") return;
      i.disabled = true;
      i.title = tr("A(作り込む前)を聴いている間は調整できません", "You can't edit while listening to A (before)");
    });
  // 先に描いて絵の下の文(マウスの下の値・説明)を入れてから高さをそろえ、そろえた大きさでもう一度描く
  // (文が後から伸びると、下のつまみが欄からはみ出していた)
  requestAnimationFrame(() => {
    draw();
    alignPaneCanvases();
    draw();
  });
}
/** 絵を描き直す(値が変わったら試聴にも当てる) */
export function draw() {
  if (!state.inst || !EDITORS[state.inst] || !cur()) return;
  ed().draw();
  syncPreview();
}

/** 音域の選択欄の後ろの文: 見出しの行に入るか測って、入らなければ「を表示」→「で弾いたとき」の順に省く */
export function fitRangePick() {
  rootEl?.querySelectorAll<HTMLElement>(".edithead").forEach((h) => {
    const rp = h.querySelector(".rangepick");
    if (!rp) return;
    rp.classList.remove("no-tail", "no-say");
    const need = () => {
      h.style.width = "max-content";
      const w = h.scrollWidth;
      h.style.width = "";
      return w;
    };
    if (need() > h.clientWidth) rp.classList.add("no-tail");
    if (need() > h.clientWidth) rp.classList.add("no-say");
  });
}
/** 下の枠の中で隣り合う欄の見出しの高さと絵の上下をそろえる(道具の行の数が欄ごとに違っても、絵の上下がそろう) */
export function alignPaneCanvases() {
  fitRangePick();
  rootEl?.querySelectorAll(".editpanes").forEach((ep) => {
    const hs = [...ep.querySelectorAll<HTMLElement>(":scope > .pane > .panehead")];
    hs.forEach((h) => {
      h.style.minHeight = h.dataset.reserve ? `${h.dataset.reserve}px` : "";
    });
    const mx = Math.max(0, ...hs.map((h) => h.getBoundingClientRect().height));
    if (hs.length > 1)
      hs.forEach((h) => {
        h.style.minHeight = `${mx}px`;
      });
  });
  rootEl?.querySelectorAll(".editpanes").forEach((ep) => {
    const cs = [...ep.querySelectorAll<HTMLCanvasElement>(":scope > .pane > canvas.fill")];
    if (cs.length < 2) return;
    cs.forEach((c) => {
      c.style.flex = "";
      c.style.marginTop = "";
      c.style.height = "";
    });
    const rs = cs.map((c) => c.getBoundingClientRect());
    const top = Math.max(...rs.map((r) => r.top)),
      bot = Math.min(...rs.map((r) => r.bottom));
    if (bot - top < 48) return;
    cs.forEach((c, i) => {
      c.style.flex = "none";
      c.style.marginTop = `${top - rs[i].top}px`;
      c.style.height = `${bot - top}px`;
    });
  });
}

// ---- 音色の値 ↔ Command ----
const near = (a: unknown, b: unknown) =>
  typeof a === "number" && typeof b === "number" ? Math.abs(a - b) <= Math.max(1e-6, Math.abs(b) * 1e-6) : JSON.stringify(a) === JSON.stringify(b);

/** 確定した値 → 今の値 の差の Command */
export function toCommands(prev: Data, next: Data): unknown[] {
  const e = ed();
  const a = e.params(prev),
    b = e.params(next);
  const out: unknown[] = [];
  for (const [k, v] of Object.entries(b)) {
    if (!near(a[k], v)) out.push({ op: "set_param", track: state.track, path: `device/${k}`, value: v });
  }
  out.push(...(e.extraCommands?.(state.track, prev, next) ?? []));
  return out;
}

// ---- 取り消しの並び(トラックごと) ----
type Undone = { orig: string; mode: "undo" | "revert"; revert?: string; label: string };
const stacks: Record<string, { done: { id: string; label: string }[]; undone: Undone[] }> = {};
const stack = () => (stacks[state.track] ??= { done: [], undone: [] });
/** エディタが作った履歴の項目(取り消し・やり直しも)。AI の変更を拾うときに除く */
const ownIds = new Set<string>();
/** もう見た履歴の項目(開いたとき以前の物は積まない) */
const seenIds = new Set<string>();
let queue: Promise<unknown> = Promise.resolve();
function refreshUndo() {
  const s = stack();
  view.canUndo = s.done.length > 0;
  view.canRedo = s.undone.length > 0;
}

/** 離したとき: 確定した値との差を 1 件の編集として当てる(値が変わらない操作は積まない) */
export function pushHist(label: string) {
  if (state.ab === "before" || ui.dragCancelled) return;
  const e = ed();
  const next = soundOf(cur());
  const prev = state.committed[state.track];
  const cmds = toCommands(prev, next);
  const custom = !!e.commit && !!e.signature && e.signature(prev) !== e.signature(next);
  if (!cmds.length && !custom) return;
  state.committed[state.track] = clone(next);
  // 当てるとエンジンは曲の値に戻る(試聴の差は無くなる)
  lastPreview = "[]";
  const tid = state.track,
    name = state.trackName;
  queue = queue.then(async () => {
    try {
      const r = custom ? await e.commit!(cmds, `${name}: ${label}`, next) : await api.applyEdit(cmds, `${name}: ${label}`);
      const patch = (r as { patch?: Data }).patch;
      if (patch) {
        Object.assign(state.committed[tid] ?? {}, patch);
        if (state.data[tid]) Object.assign(state.data[tid], patch);
      }
      ownIds.add(r.entry_id);
      seenIds.add(r.entry_id);
      const s = (stacks[tid] ??= { done: [], undone: [] });
      // 履歴の知らせが先に届いて「ほかからの変更」として積まれていたら、言葉だけ差し替える(2 回積まない)
      const dup = s.done.find((x) => x.id === r.entry_id);
      if (dup) dup.label = label;
      else s.done.push({ id: r.entry_id, label });
      s.undone = [];
      refreshUndo();
    } catch {
      // 当てられなかった(applyEdit が知らせる)。曲の値で読み直す
      await reloadFromTrack(true);
    }
  });
  syncPreview();
}

export async function undo() {
  if (state.ab === "before") return showToast("warn", tr("A(作り込む前)を聴いている間は取り消せません", "You can't undo while listening to A (before)"));
  await queue;
  const s = stack();
  const h = s.done.pop();
  if (!h) return;
  stopAllSounds();
  try {
    const hist = await api.getHistory(1);
    if (hist.entries[hist.entries.length - 1]?.id === h.id) {
      await api.undo();
      s.undone.push({ orig: h.id, mode: "undo", label: h.label });
    } else {
      const r = await api.revertEntry(h.id);
      ownIds.add(r.entry_id);
      seenIds.add(r.entry_id);
      s.undone.push({ orig: h.id, mode: "revert", revert: r.entry_id, label: h.label });
    }
    toast(tr(`取り消し: ${h.label}`, `Undo: ${h.label}`));
  } catch (e) {
    s.done.push(h);
    showToast("error", tr(`取り消せませんでした: ${e}`, `Couldn't undo: ${e}`));
  }
  refreshUndo();
}

export async function redo() {
  if (state.ab === "before") return showToast("warn", tr("A(作り込む前)を聴いている間はやり直せません", "You can't redo while listening to A (before)"));
  await queue;
  const s = stack();
  const u = s.undone.pop();
  if (!u) return;
  stopAllSounds();
  try {
    if (u.mode === "undo") {
      const hist = await api.getHistory(1);
      if (hist.redoable[0]?.id !== u.orig) {
        s.undone = [];
        refreshUndo();
        return showToast("warn", tr("取り消した後にほかの編集が入ったので、やり直せません", "Another edit came after the undo, so it can't be redone"));
      }
      await api.redo();
      s.done.push({ id: u.orig, label: u.label });
    } else {
      const r = await api.revertEntry(u.revert!);
      ownIds.add(r.entry_id);
      seenIds.add(r.entry_id);
      s.done.push({ id: r.entry_id, label: u.label });
    }
    toast(tr(`やり直し: ${u.label}`, `Redo: ${u.label}`));
  } catch (e) {
    s.undone.push(u);
    showToast("error", tr(`やり直せませんでした: ${e}`, `Couldn't redo: ${e}`));
  }
  refreshUndo();
}

/** 履歴に入った、このトラックの音色の変更(AI・ほかの画面)を取り消しの並びに積む */
export function noteHistory(entries: EntrySummary[]) {
  for (const e of entries) {
    if (seenIds.has(e.id)) continue;
    seenIds.add(e.id);
    if (!view.open || ownIds.has(e.id) || e.reverts) continue;
    const mine = e.targets.some((t) => t.kind === "track" && t.id === state.track);
    const clipOrNote = e.targets.some((t) => t.kind === "clip" || t.kind === "note");
    if (!mine || clipOrNote) continue;
    const s = stack();
    if (s.done.some((x) => x.id === e.id)) continue;
    s.done.push({ id: e.id, label: e.label });
    s.undone = [];
  }
  refreshUndo();
}

// ---- 試聴(ドラッグ中の値・A の音・このトラックだけ) ----
let lastPreview = "[]";
let previewRaf = 0;
let previewSeq = 0;
/** 「このトラックだけ」(ほかのトラックを止めて聴く。曲には残さない) */
export const listen = $state({ solo: false, chord: false, moveMod: true });
export function syncPreview() {
  if (previewRaf) return;
  previewRaf = requestAnimationFrame(async () => {
    previewRaf = 0;
    // 閉じた・楽器を切り替えた後に届いた予約は捨てる
    if (!state.track || !cur() || !EDITORS[state.inst] || !view.open) return;
    const seq = ++previewSeq;
    const e = ed();
    const cmds: unknown[] = [];
    if (state.ab === "before") cmds.push({ op: "set_device", track: state.track, device: state.initialDevice[state.track] });
    else {
      cmds.push(...toCommands(state.committed[state.track], soundOf(cur())));
      // 作り方の手順が変わっている間は、仮のテーブルなどを当てる(作るのに時間がかかるので、後から来た物だけ使う)
      if (e.previewExtra && e.signature && e.signature(state.committed[state.track]) !== e.signature(cur())) {
        const sig0 = e.signature(cur());
        if (sig0 === lastExtraSig) cmds.push(...lastExtra);
        else {
          try {
            const extra = await e.previewExtra(soundOf(cur()));
            if (seq !== previewSeq) return;
            lastExtraSig = sig0;
            lastExtra = extra;
            cmds.push(...extra);
          } catch {
            // 作れない手順(試聴しない)
          }
        }
      }
    }
    if (listen.solo) cmds.push({ op: "set_track_prop", id: state.track, prop: "solo", value: true });
    // 「LFO・エンベロープも動かす」を切ったとき: 音色を動かすつまみを 0 にして鳴らす(曲には残さない)
    if (!listen.moveMod) for (const [k, v] of Object.entries(e.staticParams ?? {})) cmds.push({ op: "set_param", track: state.track, path: `device/${k}`, value: v });
    const sig = JSON.stringify(cmds);
    if (sig === lastPreview) return;
    lastPreview = sig;
    api.previewEdit(cmds);
  });
}
let lastExtraSig = "";
let lastExtra: unknown[] = [];
/** 曲が変わった後(エンジンが曲の値に戻った)に、試聴をもう一度当てる */
function resendPreview() {
  lastPreview = "";
  syncPreview();
}

// ---- 鳴らす(エンジンのライブの列。押している間だけ鳴る) ----
export interface Voice {
  stop(): void;
  update?(): void;
}
const keyVoices = new Map<number, Voice>();
/** 絵(鍵盤・帯など)を押している間鳴らす音。画面の作り直し・楽器の切り替え・取り消しで必ず止める */
const held = new Set<Voice>();
export function holdVoice(v: Voice) {
  held.add(v);
  return v;
}
export function releaseVoice(v: Voice | null | undefined) {
  if (v && held.delete(v)) v.stop();
}
export function stopHeld() {
  held.forEach((v) => v.stop());
  held.clear();
}
/** 音を鳴らす(離すまで)。和音のときは +4・+7 も */
export function playNotes(notes: number[], vel = 100): Voice {
  const tid = state.track;
  const ms = notes.map((m) => Math.max(0, Math.min(127, Math.round(m))));
  ms.forEach((m) => api.liveNoteOn(tid, m, vel).catch(() => undefined));
  let stopped = false;
  return {
    stop() {
      if (stopped) return;
      stopped = true;
      ms.forEach((m) => api.liveNoteOff(m).catch(() => undefined));
    },
  };
}
export const keyListeners = new Set<(m: number, on: boolean) => void>();
export function noteOn(m: number) {
  if (keyVoices.has(m) || !state.track) return;
  keyVoices.set(m, playNotes(listen.chord ? [m, m + 4, m + 7] : [m]));
  keyListeners.forEach((f) => f(m, true));
  ed()?.onKey?.(m, true);
}
export function noteOff(m: number) {
  const v = keyVoices.get(m);
  if (!v) return;
  v.stop();
  keyVoices.delete(m);
  keyListeners.forEach((f) => f(m, false));
  ed()?.onKey?.(m, false);
}
/** 鳴っている音をすべて止める(下の鍵盤・押している間の音) */
export function stopAllSounds() {
  [...keyVoices.keys()].forEach((m) => noteOff(m));
  stopHeld();
  api.liveAllOff().catch(() => undefined);
}
export const isSounding = () => keyVoices.size > 0 || held.size > 0;

// ---- 開く・閉じる・読み込み ----
async function loadTrack(): Promise<Data | null> {
  const p = state.project;
  const t = p?.tracks.find((x) => x.id === state.track);
  if (!p || !t) return null;
  const tp = await api.getTrackParams(state.track);
  const params: Params = Object.fromEntries(tp.params.map((x) => [x.name, x.current]));
  return await ed().load({ params, track: t, project: p });
}

/** 音源の種類から、エディタの鍵(無ければ null) */
export function editorKindOf(t: Track | undefined | null): string | null {
  const d = t?.device;
  if (!d) return null;
  const k = !d.type || d.type === "builtin" ? (d.name ?? "") : d.type;
  return EDITORS[k] ? k : null;
}

/** トラックを開く(初めてなら、今の音を「作り込む前」として覚える) */
export async function openTrack(project: Project, trackId: string) {
  if (state.ab === "before") setAB("now");
  stopAllSounds();
  const t = project.tracks.find((x) => x.id === trackId);
  const kind = editorKindOf(t);
  if (!t || !kind) return;
  state.project = project;
  state.track = trackId;
  state.trackName = t.name;
  state.inst = kind;
  view.open = true;
  view.title = ed().title();
  view.loading = true;
  view.error = null;
  setSel(null);
  try {
    const d = await loadTrack();
    if (!d) return;
    if (!state.initial[trackId]) {
      state.initial[trackId] = clone(d);
      state.initialDevice[trackId] = clone(t.device);
    }
    state.data[trackId] = { ...(ed().uiInit?.() ?? {}), ...(state.data[trackId] ? uiOf(state.data[trackId]) : {}), ...d };
    state.committed[trackId] = clone(soundOf(state.data[trackId]));
  } catch (e) {
    view.error = String(e);
  } finally {
    view.loading = false;
  }
  refreshUndo();
  ed().reload?.();
  lastPreview = "";
  rerender();
}

/** 曲が変わったら(AI・取り消し・ほかの画面)、音色の値が確定した値と違えば読み直す(画面だけの設定は今のまま) */
export async function onProjectChanged(project: Project) {
  state.project = project;
  if (!view.open || !state.track) return;
  const t = project.tracks.find((x) => x.id === state.track);
  if (!t) return close();
  if (editorKindOf(t) !== state.inst) {
    // 音源を差し替えた: 開き直す(作り込む前は今の音)
    delete state.initial[state.track];
    return openTrack(project, state.track);
  }
  state.trackName = t.name;
  await reloadFromTrack(false);
  // エンジンが曲の値に戻ったので、試聴(A・このトラックだけ)を当て直す
  resendPreview();
}

async function reloadFromTrack(force: boolean) {
  await queue;
  const d = await loadTrack().catch(() => null);
  if (!d) return;
  const committed = state.committed[state.track];
  const e = ed();
  const same =
    committed &&
    JSON.stringify(e.params(d)) === JSON.stringify(e.params(committed)) &&
    JSON.stringify(e.extraCommands?.(state.track, committed, d) ?? []) === "[]" &&
    (!e.signature || e.signature(d) === e.signature(committed));
  if (same && !force) return;
  state.committed[state.track] = clone(soundOf(d));
  if (state.ab === "before") {
    state.abStash = { ...d };
    return;
  }
  state.data[state.track] = { ...d, ...uiOf(cur() ?? {}) };
  ed().reload?.();
  ed().refresh();
}

export function close() {
  if (state.ab === "before") setAB("now");
  stopAllSounds();
  view.open = false;
  state.inst = "";
  if (listen.solo || lastPreview !== "[]") api.previewEdit([]);
  listen.solo = false;
  lastPreview = "[]";
}

// ---- A / B ----
/** A: 今の値を取っておき、作り込む前の値の写しを見せて鳴らす(画面だけの設定は持ち越す)。B: 取っておいた値に戻す */
export function setAB(ab: "now" | "before") {
  if (state.ab === ab) return;
  stopAllSounds();
  if (ab === "before") {
    state.abStash = cur();
    state.data[state.track] = { ...clone(state.initial[state.track]), ...uiOf(cur()) };
  } else if (state.abStash) {
    state.data[state.track] = { ...state.abStash, ...uiOf(cur()) };
    state.abStash = null;
  }
  state.ab = ab;
  view.ab = ab;
  ed().reload?.();
  rerender();
  syncPreview();
}

/** 作り込む前(開いたとき)の音に戻す(戻したことも履歴に残る) */
export function revertAll() {
  if (!canEdit()) return;
  const dev = state.initialDevice[state.track];
  const label = tr("音色を作り込む前に戻す", "Revert the sound to before editing");
  const tid = state.track,
    name = state.trackName;
  state.data[state.track] = { ...clone(state.initial[state.track]), ...uiOf(cur()) };
  state.committed[state.track] = clone(soundOf(cur()));
  lastPreview = "[]";
  queue = queue.then(async () => {
    try {
      const r = await api.applyEdit([{ op: "set_device", track: tid, device: dev }], `${name}: ${label}`);
      ownIds.add(r.entry_id);
      seenIds.add(r.entry_id);
      const s = (stacks[tid] ??= { done: [], undone: [] });
      if (!s.done.some((x) => x.id === r.entry_id)) s.done.push({ id: r.entry_id, label });
      s.undone = [];
      refreshUndo();
    } catch {
      await reloadFromTrack(true);
    }
  });
  ed().reload?.();
  ed().refresh();
}

// ---- ドラッグ: Ctrl+Z / Esc で「そのドラッグをやめて、つまむ前の値に戻す」。ウィンドウから外れたら「離した」として扱う ----
export function dragStart(ev: PointerEvent) {
  if (ev.button !== 0 || !(ev.target as Element).closest?.(".ed-body canvas, .ed-body .kn input, .ed-body .grip")) return;
  ui.drag = { snap: JSON.stringify(cur()), target: ev.target as Element, id: ev.pointerId };
  ui.dragCancelled = false;
}
export function dragEnd() {
  ui.drag = null;
}
/** 取り消したドラッグの「離した」が終わってから元に戻す */
export function afterPointerUp() {
  ui.dragCancelled = false;
}
export function cancelDrag(): boolean {
  if (!ui.drag) return false;
  const d0 = JSON.parse(ui.drag.snap);
  ui.drag = null;
  ui.dragCancelled = true;
  state.data[state.track] = { ...d0, ...uiOf(cur()) };
  stopHeld();
  ed().tbl = null;
  ed().refresh();
  syncPreview();
  toast(tr("ドラッグをやめて、つまむ前に戻しました", "Cancelled the drag and restored the value from before"));
  return true;
}
export function endDrag() {
  if (!ui.drag) return;
  const t = ui.drag.target;
  ui.drag = null;
  if (t && t.isConnected) t.dispatchEvent(new PointerEvent("pointerup", { bubbles: true, pointerId: 1 }));
}
