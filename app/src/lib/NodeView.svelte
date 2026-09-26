<script lang="ts">
  // エフェクトのノード表示: カードを自由に置き、口から線を引いてつなぐ。1 つの口から何本でも出せ(分岐)、
  // 1 つの口に何本でも入れられる(合流 = 足し合わせ)。入力から出口まで線でたどれるカードだけが鳴り、
  // たどれないカードは点線で「鳴らない」(設定は残る)。線はクリックで音量と「切る」、ダブルクリックで切る、
  // Ctrl+ドラッグでなぞった線をまとめて切る。つながりは set_fx_links、位置は set_effect_prop(pos)で、
  // すべて Command を通るので Ctrl+Z で戻せる。つながりの表が無いトラックは並び順の直列として見せ、
  // 最初に線を変えたときに表になる。
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import {
    connectProblem,
    effectiveLinks,
    fromInput,
    fxColor,
    fxIcon,
    fxKind,
    fxName,
    FX_KIND_JA,
    IN,
    insertBeforeOutput,
    linkKey,
    OUT,
    processingOrder,
    soundingSet,
    splitLink,
    IR_FROM_FILE,
    irChoices,
    trackChoices,
    unlinkBridging,
  } from "./fx";
  import { loadIrFromFile } from "./ir";
  import { fxDrag, fxDropTargets, isOverShelf } from "./fxDrag.svelte";
  import { deviceName } from "./instruments";
  import { focusNow, keepInView } from "./menu";
  import { fmtValue, fromPos, SLIDER_MAX, toPos } from "./params";
  import { MASTER_FOCUS_ID, viewStore } from "./selection.svelte";
  import { showError } from "./toast.svelte";
  import { newFxId } from "./ids";
  import type { EffectView, FxLink, ParamView, Project, TrackParams } from "./types";

  let { project, targetId, onSavePreset }: {
    project: Project;
    targetId: string;
    /** カードをエフェクトのプリセットに保存する(棚を持つ親が受け持つ) */
    onSavePreset?: (fx: EffectView) => void;
  } = $props();

  const isMaster = $derived(targetId === MASTER_FOCUS_ID);
  const track = $derived(isMaster ? null : (project.tracks.find((t) => t.id === targetId) ?? null));
  const targetName = $derived(isMaster ? "マスター" : (track?.name ?? ""));

  // ---- つまみの一覧(インスペクターと同じ取り方。どの編集でも取り直す = AI の編集も反映) ----
  let info = $state<TrackParams | null>(null);
  $effect(() => {
    const id = targetId;
    void project;
    (id === MASTER_FOCUS_ID ? api.getMasterParams() : api.getTrackParams(id))
      .then((r) => {
        info = r;
        localPos = {};
      })
      .catch(() => (info = null));
  });

  let clapEffects = $state<api.ClapPluginInfo[]>([]);
  /** CLAP 音源の名前(入力の表示用) */
  let clapNames = $state(new Map<string, string>());
  let clapLoaded = false;
  $effect(() => {
    if (clapLoaded) return;
    clapLoaded = true;
    api
      .clapPlugins()
      .then((r) => {
        clapEffects = r.plugins.filter((p) => p.effect);
        clapNames = new Map(r.plugins.map((p) => [p.id, p.name]));
      })
      .catch(() => {});
  });

  const all = $derived(info?.effects ?? []);
  const ids = $derived(new Set(all.map((e) => e.id)));
  /** 保存してあるつながりの表(無ければ並び順の直列として見せる) */
  const rawLinks = $derived(isMaster ? project.master.fx_links : track?.fx_links);
  const links = $derived(
    effectiveLinks(all, rawLinks).filter((l) => (l.from === IN || ids.has(l.from)) && (l.to === OUT || ids.has(l.to))),
  );
  const on = $derived(soundingSet(links));
  const reached = $derived(fromInput(links));
  const order = $derived(processingOrder(all, links));

  // ---- 並べ方 ----
  const IO_W = 120;
  const CARD_W = 196;
  const GAP = 48;
  const TOP = 40;
  const ROW_H = 200;
  const PORT_Y = 22;
  /** 保存してある入力・出口の置き場所(無ければ自動) */
  const ioSaved = $derived(isMaster ? project.master.fx_io_pos : track?.fx_io_pos);
  /** ドラッグの直後は保存が戻るまで手元の位置 */
  let localIo = $state<{ input?: [number, number]; output?: [number, number] }>({});
  $effect(() => {
    void ioSaved;
    localIo = {};
  });
  const inPos = $derived.by((): [number, number] => {
    if (drag?.kind === "io" && drag.id === IN && drag.moved) return [drag.x, drag.y];
    return localIo.input ?? ioSaved?.input ?? [24, TOP];
  });

  /** 位置を保存していないカードの置き場所: 鳴るカードは入力からの段数で左から、鳴らないカードは下の段に */
  const auto = $derived.by(() => {
    const depth = new Map<string, number>([[IN, 0]]);
    for (const id of order) {
      const d = Math.max(0, ...links.filter((l) => l.to === id && depth.has(l.from)).map((l) => depth.get(l.from)!));
      depth.set(id, d + 1);
    }
    const cols = new Map<number, string[]>();
    for (const id of order) cols.set(depth.get(id)!, [...(cols.get(depth.get(id)!) ?? []), id]);
    const pos = new Map<string, [number, number]>();
    let rows = 1;
    let maxDepth = 0;
    for (const [d, list] of cols) {
      maxDepth = Math.max(maxDepth, d);
      rows = Math.max(rows, list.length);
      list.forEach((id, r) => pos.set(id, [24 + IO_W + GAP + (d - 1) * (CARD_W + GAP), TOP + r * ROW_H]));
    }
    all
      .filter((e) => !on.has(e.id))
      .forEach((e, i) => pos.set(e.id, [24 + IO_W + GAP + (i % 5) * (CARD_W + GAP), TOP + rows * ROW_H + 30 + Math.floor(i / 5) * ROW_H]));
    return { pos, maxDepth };
  });

  /** ドラッグの直後は保存が戻るまで手元の位置 */
  let localPos = $state<Record<string, [number, number]>>({});
  function posOf(id: string): [number, number] {
    if (drag?.kind === "card" && drag.id === id && drag.moved) return [drag.x, drag.y];
    const e = all.find((x) => x.id === id);
    return localPos[id] ?? e?.pos ?? auto.pos.get(id) ?? [0, 0];
  }

  /** 出口: 保存してあればそこ、無ければ鳴るカードの右か、いちばん右のカードの右 */
  const outPos = $derived.by((): [number, number] => {
    if (drag?.kind === "io" && drag.id === OUT && drag.moved) return [drag.x, drag.y];
    const saved = localIo.output ?? ioSaved?.output;
    if (saved) return saved;
    let x = 24 + IO_W + GAP + auto.maxDepth * (CARD_W + GAP);
    for (const e of all) {
      if (!on.has(e.id)) continue;
      x = Math.max(x, posOf(e.id)[0] + CARD_W + GAP);
    }
    return [x, TOP];
  });

  function nodePos(id: string): [number, number] {
    if (id === IN) return inPos;
    if (id === OUT) return outPos;
    return posOf(id);
  }
  const outPort = (id: string): [number, number] => {
    const [x, y] = nodePos(id);
    return id === IN ? [x + IO_W + 1, y + PORT_Y + 12] : [x + CARD_W + 1, y + PORT_Y];
  };
  const inPort = (id: string): [number, number] => {
    const [x, y] = nodePos(id);
    return id === OUT ? [x - 1, y + PORT_Y + 12] : [x - 1, y + PORT_Y];
  };
  type Bez = [[number, number], [number, number], [number, number], [number, number]];
  function bez(a: [number, number], b: [number, number]): Bez {
    const dx = Math.max(50, Math.abs(b[0] - a[0]) * 0.5);
    return [a, [a[0] + dx, a[1]], [b[0] - dx, b[1]], b];
  }
  const bezPath = (p: Bez) => `M${p[0][0]} ${p[0][1]} C${p[1][0]} ${p[1][1]}, ${p[2][0]} ${p[2][1]}, ${p[3][0]} ${p[3][1]}`;
  function bezPts(p: Bez, n = 40): [number, number][] {
    const out: [number, number][] = [];
    for (let i = 0; i <= n; i++) {
      const t = i / n;
      const u = 1 - t;
      const f = (k: 0 | 1) => u * u * u * p[0][k] + 3 * u * u * t * p[1][k] + 3 * u * t * t * p[2][k] + t * t * t * p[3][k];
      out.push([f(0), f(1)]);
    }
    return out;
  }
  const linkBez = (l: FxLink) => bez(outPort(l.from), inPort(l.to));
  const nameOf = (id: string) => (id === IN ? "入力" : id === OUT ? "出口" : fxName(all.find((e) => e.id === id) ?? { id, name: "?", type: "builtin" }));

  /** 点に近い線(24px 以内) */
  function linkNear(pt: [number, number]): FxLink | null {
    let best: FxLink | null = null;
    let bd = 24;
    for (const l of links)
      for (const q of bezPts(linkBez(l))) {
        const d = Math.hypot(q[0] - pt[0], q[1] - pt[1]);
        if (d < bd) {
          bd = d;
          best = l;
        }
      }
    return best;
  }
  /** 線分 a-b となぞって交わる線 */
  function crossed(a: [number, number], b: [number, number]): Set<string> {
    const side = (p: number[], q: number[], r: number[]) => (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    const out = new Set<string>();
    for (const l of links) {
      const pts = bezPts(linkBez(l), 30);
      for (let i = 0; i + 1 < pts.length; i++) {
        const [c, d] = [pts[i], pts[i + 1]];
        if (side(a, b, c) * side(a, b, d) < 0 && side(c, d, a) * side(c, d, b) < 0) {
          out.add(linkKey(l));
          break;
        }
      }
    }
    return out;
  }

  // ---- 編集 ----
  async function edit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (err) {
      showError("変更できませんでした", err);
    }
  }
  const linksCmd = (next: FxLink[]) =>
    isMaster ? { op: "set_fx_links", links: next } : { op: "set_fx_links", track: targetId, links: next };
  const setLinks = (next: FxLink[], label: string, extra: unknown[] = []) => edit([...extra, linksCmd(next)], label);
  const posCmd = (id: string, pos: [number, number] | null) => ({ op: "set_effect_prop", id, prop: "pos", value: pos });

  // ---- ドラッグ(カード・口・なぞって切る) ----
  type Drag =
    | { kind: "card"; id: string; x: number; y: number; ox: number; oy: number; sx: number; sy: number; moved: boolean; free: boolean; hot: FxLink | null }
    | { kind: "port"; id: string; side: "in" | "out"; x: number; y: number; target: string | null; problem: string | null }
    | { kind: "knife"; a: [number, number]; b: [number, number]; cut: Set<string> }
    | { kind: "io"; id: string; x: number; y: number; ox: number; oy: number; sx: number; sy: number; moved: boolean };
  let drag = $state<Drag | null>(null);
  let canvasEl = $state<HTMLDivElement | undefined>(undefined);

  function spacePt(ev: { clientX: number; clientY: number }): [number, number] {
    if (!canvasEl) return [0, 0];
    const r = canvasEl.getBoundingClientRect();
    return [ev.clientX - r.left + canvasEl.scrollLeft, ev.clientY - r.top + canvasEl.scrollTop];
  }
  function nodeAt(ev: PointerEvent): string | null {
    const el = document.elementFromPoint(ev.clientX, ev.clientY) as HTMLElement | null;
    return el?.closest<HTMLElement>("[data-node]")?.dataset.node ?? null;
  }

  function follow(ev: PointerEvent, move: (m: PointerEvent) => void, up: (u: PointerEvent) => void) {
    const mv = (m: PointerEvent) => move(m);
    const u = (e: PointerEvent) => {
      window.removeEventListener("pointermove", mv);
      window.removeEventListener("pointerup", u);
      up(e);
    };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", u);
    ev.preventDefault();
  }

  function onCardDown(ev: PointerEvent, e: EffectView) {
    if (ev.button !== 0) return;
    const t = ev.target as HTMLElement;
    if (t.closest("button, input, select, textarea, [data-port]")) return;
    const [x, y] = posOf(e.id);
    const [px, py] = spacePt(ev);
    const free = !links.some((l) => l.from === e.id || l.to === e.id);
    drag = { kind: "card", id: e.id, x, y, ox: px - x, oy: py - y, sx: x, sy: y, moved: false, free, hot: null };
    sel = null;
    follow(
      ev,
      (m) => {
        if (drag?.kind !== "card") return;
        const [mx, my] = spacePt(m);
        const nx = Math.max(0, mx - drag.ox);
        const ny = Math.max(0, my - drag.oy);
        const moved = drag.moved || Math.abs(nx - drag.sx) + Math.abs(ny - drag.sy) > 4;
        fxDrag.overShelf = moved && !!onSavePreset && isOverShelf(m.clientX, m.clientY);
        drag = { ...drag, x: nx, y: ny, moved, hot: moved && drag.free && !fxDrag.overShelf ? linkNear([mx, my]) : null };
      },
      () => {
        const d = drag;
        const toShelf = fxDrag.overShelf;
        fxDrag.overShelf = false;
        drag = null;
        if (d?.kind !== "card" || !d.moved) return;
        if (toShelf) {
          onSavePreset?.(e);
          return;
        }
        const pos: [number, number] = [Math.round(d.x), Math.round(d.y)];
        if (d.hot) {
          // 線に入れたら、置き場所はつながりの順の自動に任せる(隣のカードに重ならないように)
          const { [e.id]: _, ...rest } = localPos;
          localPos = rest;
          setLinks(splitLink(links, d.hot.from, d.hot.to, e.id), `${nameOf(e.id)} を「${nameOf(d.hot.from)}」と「${nameOf(d.hot.to)}」の間に入れる`, [posCmd(e.id, null)]);
        } else {
          localPos = { ...localPos, [e.id]: pos };
          edit([posCmd(e.id, pos)], `${targetName} の ${nameOf(e.id)} を動かす`);
        }
      },
    );
  }

  /** 入力・出口を動かす(位置は set_fx_io_pos で曲に保存。音には関係しない) */
  function onIoDown(ev: PointerEvent, id: string) {
    if (ev.button !== 0 || (ev.target as HTMLElement).closest("[data-port], button")) return;
    const [x, y] = nodePos(id);
    const [px, py] = spacePt(ev);
    drag = { kind: "io", id, x, y, ox: px - x, oy: py - y, sx: x, sy: y, moved: false };
    sel = null;
    follow(
      ev,
      (m) => {
        if (drag?.kind !== "io") return;
        const [mx, my] = spacePt(m);
        const nx = Math.max(0, mx - drag.ox);
        const ny = Math.max(0, my - drag.oy);
        drag = { ...drag, x: nx, y: ny, moved: drag.moved || Math.abs(nx - drag.sx) + Math.abs(ny - drag.sy) > 4 };
      },
      () => {
        const d = drag;
        if (d?.kind !== "io" || !d.moved) {
          drag = null;
          return;
        }
        const p: [number, number] = [Math.round(d.x), Math.round(d.y)];
        const next = { input: id === IN ? p : [...inPos] as [number, number], output: id === OUT ? p : [...outPos] as [number, number] };
        localIo = next;
        drag = null;
        edit(
          [isMaster ? { op: "set_fx_io_pos", pos: next } : { op: "set_fx_io_pos", track: targetId, pos: next }],
          `${targetName} の${id === IN ? "入力" : "出口"}を動かす`,
        );
      },
    );
  }

  function onPortDown(ev: PointerEvent, id: string, side: "in" | "out") {
    if (ev.button !== 0) return;
    ev.stopPropagation();
    const [x, y] = spacePt(ev);
    drag = { kind: "port", id, side, x, y, target: null, problem: null };
    sel = null;
    follow(
      ev,
      (m) => {
        if (drag?.kind !== "port") return;
        const [mx, my] = spacePt(m);
        const over = nodeAt(m);
        let target: string | null = null;
        let problem: string | null = null;
        if (over && over !== id) {
          const [a, b] = side === "out" ? [id, over] : [over, id];
          problem = connectProblem(links, a, b);
          if (!problem) target = over;
        }
        drag = { ...drag, x: mx, y: my, target, problem };
      },
      () => {
        const d = drag;
        drag = null;
        if (d?.kind !== "port" || !d.target) return;
        const [a, b] = side === "out" ? [id, d.target] : [d.target, id];
        setLinks([...links, { from: a, to: b }], `${targetName}: ${nameOf(a)} → ${nameOf(b)} をつなぐ`);
      },
    );
  }

  function onCanvasDown(ev: PointerEvent) {
    if (ev.button !== 0 || ev.target !== ev.currentTarget) return;
    sel = null;
    menu = null;
    if (!(ev.ctrlKey || ev.metaKey)) return;
    const p = spacePt(ev);
    drag = { kind: "knife", a: p, b: p, cut: new Set() };
    follow(
      ev,
      (m) => {
        if (drag?.kind !== "knife") return;
        const b = spacePt(m);
        drag = { ...drag, b, cut: crossed(drag.a, b) };
      },
      () => {
        const d = drag;
        drag = null;
        if (d?.kind !== "knife" || d.cut.size === 0) return;
        setLinks(links.filter((l) => !d.cut.has(linkKey(l))), `${targetName} の線を ${d.cut.size} 本切る`);
      },
    );
  }

  // ---- 線の選択(音量・切る) ----
  let sel = $state<string | null>(null);
  let selGain = $state<number | null>(null);
  const selLink = $derived(links.find((l) => linkKey(l) === sel) ?? null);
  function cut(l: FxLink) {
    sel = null;
    setLinks(
      links.filter((x) => linkKey(x) !== linkKey(l)),
      `${targetName}: ${nameOf(l.from)} → ${nameOf(l.to)} を切る`,
    );
  }
  function commitGain(l: FxLink, db: number) {
    selGain = null;
    setLinks(
      links.map((x) => (linkKey(x) === linkKey(l) ? { ...x, gain_db: db } : x)),
      `${targetName}: ${nameOf(l.from)} → ${nameOf(l.to)} の音量を ${db.toFixed(1)} dB に`,
    );
  }
  function onKey(ev: KeyboardEvent) {
    if ((ev.target as HTMLElement).closest("input, textarea, select")) return;
    if ((ev.key === "Delete" || ev.key === "Backspace") && selLink) {
      ev.preventDefault();
      cut(selLink);
    } else if (ev.key === "Escape") {
      sel = null;
      menu = null;
    }
  }
  const mid = (l: FxLink) => bezPts(linkBez(l), 2)[1];

  // ---- 棚(エフェクトのプリセット)から運ばれてくるもの ----
  let ghost = $state<{ x: number; y: number; hot: FxLink | null } | null>(null);
  function clientInside(cx: number, cy: number) {
    if (!canvasEl) return false;
    const r = canvasEl.getBoundingClientRect();
    return cx >= r.left && cx <= r.right && cy >= r.top && cy <= r.bottom;
  }
  $effect(() => {
    const p = fxDrag.preset;
    if (!p || !clientInside(fxDrag.x, fxDrag.y)) {
      ghost = null;
      return;
    }
    const [x, y] = spacePt({ clientX: fxDrag.x, clientY: fxDrag.y });
    ghost = { x: x - CARD_W / 2, y: y - 16, hot: linkNear([x, y]) };
  });
  async function dropPreset(name: string, cx: number, cy: number) {
    ghost = null;
    if (!clientInside(cx, cy)) return;
    const [x, y] = spacePt({ clientX: cx, clientY: cy });
    const pos: [number, number] = [Math.round(x - CARD_W / 2), Math.round(y - 16)];
    const hot = linkNear([x, y]);
    try {
      await api.applyFxPreset(targetId, name, hot ? { split: [hot.from, hot.to] } : { parked: true, pos });
    } catch (err) {
      showError("エフェクトを足せませんでした", err);
    }
  }
  $effect(() => {
    fxDropTargets.canvas = dropPreset;
    return () => {
      if (fxDropTargets.canvas === dropPreset) fxDropTargets.canvas = null;
    };
  });

  // ---- カードの操作 ----
  function toggleBypass(e: EffectView) {
    edit([{ op: "set_effect_bypass", id: e.id, bypass: !e.bypass }], `${targetName} の ${fxName(e)} を${e.bypass ? "有効に" : "バイパス"}`);
  }

  let renaming = $state<string | null>(null);
  function commitName(e: EffectView, v: string) {
    // Enter の後、入力欄が消えるときの blur でもう一度呼ばれるので 1 回だけ
    if (renaming !== e.id) return;
    renaming = null;
    const label = v.trim();
    if ((label || undefined) === e.label) return;
    edit([{ op: "set_effect_prop", id: e.id, prop: "label", value: label || null }], `${targetName} の ${fxName(e)} の名前を「${label || fxName({ ...e, label: undefined })}」に`);
  }

  let noting = $state<string | null>(null);
  function commitNote(e: EffectView, v: string) {
    if (noting !== e.id) return;
    noting = null;
    const note = v.trim();
    if ((note || undefined) === e.note) return;
    edit([{ op: "set_effect_prop", id: e.id, prop: "note", value: note || null }], `${targetName} の ${fxName(e)} のメモ`);
  }

  function cutAll(e: EffectView) {
    menu = null;
    setLinks(links.filter((l) => l.from !== e.id && l.to !== e.id), `${targetName} の ${fxName(e)} の線を全部切る`);
  }
  function bridge(e: EffectView) {
    menu = null;
    setLinks(unlinkBridging(links, e.id), `${targetName} の ${fxName(e)} を抜いて前後をつなぐ`);
  }
  function connectToOut(e: EffectView) {
    menu = null;
    setLinks(insertBeforeOutput(unlinkBridging(links, e.id), e.id), `${targetName} の ${fxName(e)} を出口の前につなぐ`);
  }
  function arrange() {
    const cmds: unknown[] = all.filter((e) => e.pos).map((e) => posCmd(e.id, null));
    if (ioSaved) cmds.push(isMaster ? { op: "set_fx_io_pos", pos: null } : { op: "set_fx_io_pos", track: targetId, pos: null });
    localPos = {};
    localIo = {};
    if (cmds.length > 0) edit(cmds, `${targetName} のエフェクトを並べ直す`);
  }

  function remove(e: EffectView) {
    menu = null;
    edit([{ op: "remove_effect", id: e.id }], `${targetName} の ${fxName(e)} を削除`);
  }

  function paramCommand(p: ParamView, raw: string | number | boolean): unknown {
    let value: unknown = raw;
    if (p.range.kind === "float") value = Number(raw);
    if (p.range.kind === "int") value = Math.round(Number(raw));
    if (p.range.kind === "bool") value = Boolean(raw);
    return isMaster ? { op: "set_master_param", path: p.path, value } : { op: "set_param", track: targetId, path: p.path, value };
  }
  function commitParam(p: ParamView, raw: string | number | boolean) {
    edit([paramCommand(p, raw)], `${targetName} の ${p.display_name} を変更`);
  }
  /** ドラッグ中: 表示と音だけ変える(離したときに 1 回だけ確定する) */
  function dragParam(p: ParamView, v: number) {
    dragValues[p.path] = v;
    api.previewEdit([paramCommand(p, v)]);
  }
  let dragValues = $state<Record<string, number>>({});

  // ---- メニュー ----
  let menu = $state<{ fx: EffectView; x: number; y: number } | null>(null);
  /** エフェクトを足すメニュー。`split` があれば、その線の間に入れる */
  let addMenu = $state<{ x: number; y: number; split?: FxLink } | null>(null);
  function at(ev: MouseEvent) {
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    return { x: r.left, y: r.bottom + 4 };
  }

  function addEffect(name: string) {
    const split = addMenu?.split;
    addMenu = null;
    const clapId = name.startsWith("clap:") ? name.slice(5) : null;
    const id = newFxId();
    const base = clapId ? { id, type: "clap", plugin_id: clapId } : { id, type: "builtin", name };
    const label = clapId ? (clapEffects.find((p) => p.id === clapId)?.name ?? clapId) : name;
    if (split) {
      // 線の途中に入れる: つながずに足してから、その線を「元 → 新しいエフェクト → 先」に付け替える(1 回の undo)
      const effect = { ...base, parked: true };
      edit(
        [
          isMaster ? { op: "add_master_effect", effect } : { op: "add_effect", track: targetId, effect },
          linksCmd(splitLink(links, split.from, split.to, id)),
        ],
        `${targetName} の ${nameOf(split.from)} と ${nameOf(split.to)} の間に ${label} を追加`,
      );
      return;
    }
    // つながりの表があれば出口の直前に、無ければ並びの最後に入る(どちらでも鳴る)
    edit(
      [isMaster ? { op: "add_master_effect", effect: base } : { op: "add_effect", track: targetId, effect: base }],
      `${targetName} に ${label} を追加`,
    );
  }

  // ---- 入力・出口 ----
  const inputLabel = $derived(
    isMaster
      ? "全トラック"
      : track?.kind === "bus"
        ? "送られてきた音"
        : track?.kind === "audio"
          ? "音声"
          : track?.device?.type === "clap"
            ? (clapNames.get(track.device.plugin_id ?? "") ?? deviceName(track.device))
            : deviceName(track?.device ?? null),
  );
  const outputLabel = $derived.by(() => {
    if (isMaster) return "出力";
    if (!track) return "";
    const sends = (track.sends ?? []).length;
    return `音量 ${track.volume_db.toFixed(1)} dB${sends ? `・送り ${sends}` : ""}`;
  });
  const inCount = (id: string) => links.filter((l) => l.to === id).length;
  const outCount = (id: string) => links.filter((l) => l.from === id).length;
  const outConnected = $derived(links.some((l) => l.to === OUT && (l.from === IN || on.has(l.from))));

  const size = $derived.by(() => {
    let w = Math.max(outPos[0], inPos[0]) + IO_W + 60;
    let h = Math.max(TOP + ROW_H + 80, inPos[1] + 160, outPos[1] + 160);
    for (const e of all) {
      const [x, y] = posOf(e.id);
      w = Math.max(w, x + CARD_W + 60);
      h = Math.max(h, y + 260);
    }
    if (ghost) {
      w = Math.max(w, ghost.x + CARD_W + 60);
      h = Math.max(h, ghost.y + 240);
    }
    return { w, h };
  });

  const shownParams = (e: EffectView) => e.params.slice(0, fxKind(e) === "clap" ? 2 : 3);
  const cableFrom = $derived.by((): [number, number] | null => {
    const d = drag;
    if (d?.kind !== "port") return null;
    return d.side === "out" ? outPort(d.id) : inPort(d.id);
  });
</script>

<div class="wrap">
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div class="nodes" bind:this={canvasEl} role="application" aria-label="エフェクトのノード表示" tabindex="0" onkeydown={onKey}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="space" class:knife={drag?.kind === "knife"} style="width:{size.w}px;height:{size.h}px" onpointerdown={onCanvasDown}>
    <svg class="wires" width={size.w} height={size.h}>
      {#each links as l (linkKey(l))}
        {@const d = bezPath(linkBez(l))}
        {@const live = (l.from === IN || on.has(l.from)) && (l.to === OUT || on.has(l.to))}
        {@const k = linkKey(l)}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <path
          class="hit"
          {d}
          onpointerdown={(ev) => {
            ev.stopPropagation();
            menu = null;
            sel = sel === k ? null : k;
            selGain = null;
            canvasEl?.focus();
          }}
          ondblclick={(ev) => {
            ev.stopPropagation();
            cut(l);
          }}
        />
        <path class="wire" class:dead={!live} class:sel={sel === k} class:hot={drag?.kind === "card" && drag.hot === l} class:cut={drag?.kind === "knife" && drag.cut.has(k)} {d} />
        {#if live}
          <path class="wire flow" class:sel={sel === k} {d} />
        {/if}
      {/each}
      {#if drag?.kind === "port" && cableFrom}
        {@const p = drag.side === "out" ? bez(cableFrom, [drag.x, drag.y]) : bez([drag.x, drag.y], cableFrom)}
        <path class="wire cable" d={bezPath(p)} />
      {/if}
      {#if drag?.kind === "knife"}
        <line class="knife-line" x1={drag.a[0]} y1={drag.a[1]} x2={drag.b[0]} y2={drag.b[1]} />
      {/if}
    </svg>

    {#each links.filter((l) => (l.gain_db ?? 0) !== 0) as l (linkKey(l))}
      {@const m = mid(l)}
      <span class="gain" style="left:{m[0]}px;top:{m[1]}px">{(l.gain_db ?? 0) > 0 ? "+" : ""}{(l.gain_db ?? 0).toFixed(1)} dB</span>
    {/each}

    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="io" class:lifted={drag?.kind === "io" && drag.id === IN && drag.moved} data-node={IN} style="left:{inPos[0]}px;top:{inPos[1]}px;width:{IO_W}px" onpointerdown={(ev) => onIoDown(ev, IN)}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <span class="port out" data-port onpointerdown={(ev) => onPortDown(ev, IN, "out")} title="ここから線を引いてつなぐ"></span>
      <Icon name={isMaster ? "merge" : "plug"} />
      <b>入力</b><small>{inputLabel}</small>
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="io"
      class:lifted={drag?.kind === "io" && drag.id === OUT && drag.moved}
      class:target={drag?.kind === "port" && drag.target === OUT}
      class:warn={!outConnected}
      data-node={OUT}
      style="left:{outPos[0]}px;top:{outPos[1]}px;width:{IO_W}px"
      onpointerdown={(ev) => onIoDown(ev, OUT)}
    >
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <span class="port in" data-port onpointerdown={(ev) => onPortDown(ev, OUT, "in")} title="ここへ線を引いてつなぐ"></span>
      {#if inCount(OUT) > 1}<span class="sum">{inCount(OUT)} 本</span>{/if}
      <Icon name="volume-2" />
      <b>出口</b><small>{outConnected ? outputLabel : "何もつながっていない(鳴らない)"}</small>
    </div>
    <button class="add" style="left:{outPos[0] - GAP / 2 - 12}px;top:{outPos[1] + PORT_Y + 24}px" onclick={(e) => (addMenu = at(e))} title="エフェクトを足す(出口の前に入る)" aria-label="エフェクトを足す"
      ><Icon name="plus" size={14} /></button
    >

    {#each all as e (e.id)}
      {@const [x, y] = posOf(e.id)}
      {@const sounding = on.has(e.id)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="card"
        class:mute={!sounding}
        class:bypass={e.bypass && sounding}
        class:lifted={drag?.kind === "card" && drag.id === e.id && drag.moved}
        class:target={drag?.kind === "port" && drag.target === e.id}
        class:hl={viewStore.highlightFx === e.id}
        data-node={e.id}
        style="left:{x}px;top:{y}px;width:{CARD_W}px;--nc:{fxColor(e)}"
        onpointerdown={(ev) => onCardDown(ev, e)}
      >
        {#if !sounding}
          <span class="badge"><Icon name="unplug" size={11} />鳴らない({reached.has(e.id) ? "出口まで届いていない" : "入力から来ていない"})</span>
        {:else if outCount(e.id) > 1}
          <span class="badge dim"><Icon name="split" size={11} />{outCount(e.id)} つに分かれる</span>
        {/if}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="port in" data-port onpointerdown={(ev) => onPortDown(ev, e.id, "in")} title="線を引いてつなぐ(ここへ入る音)"></span>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span class="port out" data-port onpointerdown={(ev) => onPortDown(ev, e.id, "out")} title="線を引いてつなぐ(ここから出る音)"></span>
        {#if inCount(e.id) > 1}<span class="sum">{inCount(e.id)} 本</span>{/if}
        <div class="bar">
          <span class="kind"><Icon name={fxIcon(e)} size={13} /></span>
          {#if renaming === e.id}
            <input
              class="name-input"
              value={e.label ?? ""}
              placeholder={fxName({ ...e, label: undefined })}
              use:focusNow
              onkeydown={(ev) => {
                if (ev.isComposing) return;
                if (ev.key === "Enter") commitName(e, ev.currentTarget.value);
                else if (ev.key === "Escape") renaming = null;
              }}
              onblur={(ev) => commitName(e, ev.currentTarget.value)}
            />
          {:else}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <b title={`${fxName(e)}(ダブルクリックで名前を変える)`} ondblclick={() => (renaming = e.id)}>{fxName(e)}</b>
          {/if}
          {#if sounding}
            <button class="btn sm icon" class:on={!e.bypass} onclick={() => toggleBypass(e)} title={e.bypass ? "バイパス中(押すと有効)" : "有効(押すとバイパス)"} aria-label="有効 / バイパス"
              ><Icon name="power" /></button
            >
          {/if}
          <button class="btn sm icon ghost" onclick={(ev) => (menu = { fx: e, ...at(ev) })} title="その他" aria-label="その他"><Icon name="ellipsis" /></button>
        </div>
        <div class="body">
          <span class="kind-label">{fxKind(e) === "clap" ? "CLAP" : (FX_KIND_JA[fxKind(e)] ?? fxKind(e))}{#if e.label}<span class="dim"> · {fxName({ ...e, label: undefined })}</span>{/if}</span>
          {#if noting === e.id}
            <textarea
              class="note-input"
              rows="2"
              use:focusNow
              placeholder="メモ(なぜ取っておいたかなど)"
              value={e.note ?? ""}
              onkeydown={(ev) => {
                if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) commitNote(e, ev.currentTarget.value);
                else if (ev.key === "Escape") noting = null;
              }}
              onblur={(ev) => commitNote(e, ev.currentTarget.value)}
            ></textarea>
          {:else if e.note}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="note" ondblclick={() => (noting = e.id)} title="ダブルクリックで書き直す"><Icon name="pin" size={11} />{e.note}</div>
          {/if}
          {#each shownParams(e) as p (p.path)}
            <div class="pm" title={p.description}>
              <div class="top"><span>{p.display_name}</span><span>{fmtValue(p, dragValues[p.path])}</span></div>
              {#if p.range.kind === "float" || p.range.kind === "int"}
                <input
                  type="range"
                  min="0"
                  max={SLIDER_MAX}
                  value={toPos(p, Number(p.current))}
                  oninput={(ev) => dragParam(p, fromPos(p, Number(ev.currentTarget.value)))}
                  onchange={(ev) => {
                    delete dragValues[p.path];
                    commitParam(p, fromPos(p, Number(ev.currentTarget.value)));
                  }}
                  aria-label={p.display_name}
                />
              {:else if p.range.kind === "enum"}
                {@const tc = trackChoices(p, project.tracks) ?? irChoices(p, project.assets)}
                <select
                  value={String(p.current)}
                  onchange={(ev) => {
                    const v = ev.currentTarget.value;
                    if (v === IR_FROM_FILE) {
                      ev.currentTarget.value = String(p.current);
                      loadIrFromFile(isMaster ? null : targetId, p.path);
                    } else commitParam(p, v);
                  }}
                  aria-label={p.display_name}
                >
                  {#if tc}
                    {#each tc as c (c.value)}<option value={c.value}>{c.label}</option>{/each}
                  {:else}
                    {#each p.range.choices as c (c)}<option value={c}>{c}</option>{/each}
                  {/if}
                </select>
              {/if}
            </div>
          {/each}
          {#if e.params.length > shownParams(e).length}
            <span class="more">ほか {(e.param_total ?? e.params.length) - shownParams(e).length} 個(インスペクターで)</span>
          {/if}
        </div>
      </div>
    {/each}

    {#if ghost}
      <div class="drop-ghost" style="left:{ghost.x}px;top:{ghost.y}px;width:{CARD_W}px">
        <Icon name="archive" size={13} />{ghost.hot ? `「${nameOf(ghost.hot.from)}」と「${nameOf(ghost.hot.to)}」の間に入れる` : "ここに置く(つながずに)"}
      </div>
    {/if}
    {#if drag?.kind === "card" && drag.moved && (drag.hot || fxDrag.overShelf)}
      <div class="tip" style="left:{drag.x + 20}px;top:{drag.y - 28}px">
        {fxDrag.overShelf ? "離すとエフェクトのプリセットに保存(ここには残る)" : `離すと「${nameOf(drag.hot!.from)}」と「${nameOf(drag.hot!.to)}」の間に入る`}
      </div>
    {/if}
    {#if drag?.kind === "port" && (drag.target || drag.problem)}
      <div class="tip" class:bad={!!drag.problem} style="left:{drag.x + 14}px;top:{drag.y - 30}px">
        {#if drag.problem}{drag.problem}{:else}
          {@const [a, b] = drag.side === "out" ? [drag.id, drag.target!] : [drag.target!, drag.id]}
          離すと「{nameOf(a)} → {nameOf(b)}」をつなぐ
        {/if}
      </div>
    {/if}
    {#if drag?.kind === "knife" && drag.cut.size > 0}
      <div class="tip bad" style="left:{drag.b[0] + 12}px;top:{drag.b[1] - 28}px">離すと {drag.cut.size} 本切る</div>
    {/if}

    {#if selLink}
      {@const m = mid(selLink)}
      {@const g = selGain ?? selLink.gain_db ?? 0}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="pop" style="left:{m[0]}px;top:{m[1]}px" onpointerdown={(ev) => ev.stopPropagation()}>
        <div class="row dim">{nameOf(selLink.from)} → {nameOf(selLink.to)}</div>
        <div class="row">
          <Icon name="volume-2" size={14} />
          <input
            type="range"
            min="-30"
            max="6"
            step="0.5"
            value={g}
            oninput={(ev) => (selGain = Number(ev.currentTarget.value))}
            onchange={(ev) => commitGain(selLink, Number(ev.currentTarget.value))}
            ondblclick={() => commitGain(selLink, 0)}
            aria-label="この線の音量"
          />
          <span class="v">{g > 0 ? "+" : ""}{g.toFixed(1)} dB</span>
        </div>
        <div class="row">
          <button
            class="btn sm"
            onclick={(ev) => {
              const l = selLink;
              sel = null;
              addMenu = { ...at(ev), split: l };
            }}
            title="この線の間にエフェクトを入れる"><Icon name="plus" />ここにエフェクトを足す</button
          >
          <button class="btn sm danger" onclick={() => cut(selLink)}><Icon name="scissors" />切る</button>
        </div>
        <div class="row">
          <span class="dim small">Delete でも切れる・ダブルクリックでもすぐ切れる</span>
        </div>
      </div>
    {/if}
  </div>
</div>
<button class="btn sm arrange" onclick={arrange} title="置き場所を自動に戻す(入力と出口も。鳴るカードはつながりの順、鳴らないカードは下に)"><Icon name="layout-grid" />整列</button>
</div>

{#if menu || addMenu}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="menu-backdrop" role="presentation" onclick={() => ((menu = null), (addMenu = null))}></div>
{/if}
{#if menu}
  {@const e = menu.fx}
  <div class="menu" use:keepInView style="left:{menu.x}px;top:{menu.y}px">
    <button
      onclick={() => {
        renaming = e.id;
        menu = null;
      }}><Icon name="pencil" />名前を変える</button
    >
    <button
      onclick={() => {
        noting = e.id;
        menu = null;
      }}><Icon name="pin" />メモを書く</button
    >
    <div class="menu-sep"></div>
    {#if !on.has(e.id)}
      <button onclick={() => connectToOut(e)}><Icon name="plug" />出口の前につなぐ</button>
    {/if}
    {#if links.some((l) => l.from === e.id || l.to === e.id)}
      <button onclick={() => bridge(e)}><Icon name="unplug" />抜いて前後をつなぐ</button>
      <button onclick={() => cutAll(e)}><Icon name="scissors" />線を全部切る</button>
    {/if}
    {#if onSavePreset}
      <div class="menu-sep"></div>
      <button
        onclick={() => {
          const fx = e;
          menu = null;
          onSavePreset(fx);
        }}><Icon name="archive" />エフェクトのプリセットに保存</button
      >
    {/if}
    {#if fxKind(e) === "clap" && !e.missing}
      <button
        onclick={() => {
          const id = e.id;
          menu = null;
          api.clapOpenGui(null, id).catch((err) => showError("プラグインの画面を開けませんでした", err));
        }}><Icon name="app-window" />プラグインの画面を開く</button
      >
    {/if}
    <div class="menu-sep"></div>
    <button class="danger" onclick={() => remove(e)}><Icon name="trash-2" />削除<span class="key">Ctrl+Z で戻せます</span></button>
  </div>
{/if}
{#if addMenu && info}
  <div class="menu" use:keepInView style="left:{addMenu.x}px;top:{addMenu.y}px">
    {#if addMenu.split}
      <div class="menu-h">{nameOf(addMenu.split.from)} と {nameOf(addMenu.split.to)} の間に入れる</div>
      <div class="menu-sep"></div>
    {/if}
    <div class="menu-h">内蔵</div>
    {#each info.available_effects as fx (fx.name)}
      <button class="rich" onclick={() => addEffect(fx.name)}><span>{FX_KIND_JA[fx.name] ?? fx.name}<small>{fx.name} · {fx.description}</small></span></button>
    {/each}
    {#if clapEffects.length > 0}
      <div class="menu-sep"></div>
      <div class="menu-h">CLAP プラグイン</div>
      {#each clapEffects as p (p.id)}
        <button class="rich" onclick={() => addEffect(`clap:${p.id}`)}><Icon name="plug" /><span>{p.name}<small>{p.vendor} {p.version}</small></span></button>
      {/each}
    {/if}
  </div>
{/if}

<style>
  .wrap {
    position: absolute;
    inset: 0;
  }

  .nodes {
    position: absolute;
    inset: 0;
    overflow: auto;
    user-select: none;
    outline: none;
    background:
      radial-gradient(circle at 1px 1px, #2b2b2b 1px, transparent 1.4px) 0 0 / 20px 20px,
      #121212;
  }

  .space {
    position: relative;
    min-width: 100%;
    min-height: 100%;
  }

  .space.knife {
    cursor: crosshair;
  }

  .arrange {
    position: absolute;
    top: 8px;
    right: 14px;
    z-index: 6;
    background: var(--bg-raised);
  }

  .wires {
    position: absolute;
    left: 0;
    top: 0;
    pointer-events: none;
  }

  .hit {
    fill: none;
    stroke: transparent;
    stroke-width: 14;
    pointer-events: stroke;
    cursor: pointer;
  }

  .hit:hover + .wire {
    stroke: #cfcfcf;
  }

  .wire {
    fill: none;
    stroke: #6b6b6b;
    stroke-width: 2.5;
    pointer-events: none;
  }

  .wire.flow {
    stroke: #a0a0a0;
    stroke-dasharray: 2 10;
    stroke-linecap: round;
    animation: flow 1.2s linear infinite;
  }

  .wire.dead {
    stroke: #4a4a4a;
    stroke-dasharray: 5 5;
  }

  .wire.sel,
  .wire.hot {
    stroke: var(--accent);
    stroke-width: 3.5;
  }

  .wire.cut {
    stroke: var(--danger);
    stroke-width: 3.5;
  }

  .wire.cable {
    stroke: var(--accent);
    stroke-dasharray: 6 5;
  }

  .knife-line {
    stroke: var(--danger);
    stroke-width: 2;
    stroke-dasharray: 6 4;
  }

  @keyframes flow {
    to {
      stroke-dashoffset: -24;
    }
  }

  .gain {
    position: absolute;
    transform: translate(-50%, -50%);
    font-size: 10px;
    font-family: var(--mono);
    color: var(--text-dim);
    background: #181818;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0 5px;
    pointer-events: none;
    z-index: 2;
  }

  .io {
    position: absolute;
    padding: 8px;
    border-radius: 10px;
    border: 1px solid var(--border-strong);
    background: #1a1a1a;
    text-align: center;
    color: var(--text-dim);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    z-index: 1;
  }

  .io {
    cursor: grab;
  }

  .io.lifted {
    cursor: grabbing;
    z-index: 4;
    border-color: var(--accent);
    box-shadow: 0 16px 30px rgba(0, 0, 0, 0.6);
  }

  .io.target {
    border-color: var(--accent);
  }

  .io.warn small {
    color: var(--warn);
  }

  .io b {
    color: var(--text);
    font-size: var(--fs-sm);
  }

  .io small {
    font-size: 10px;
    line-height: 1.3;
  }

  .port {
    position: absolute;
    top: 15px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: #2a2a2a;
    border: 2px solid #9a9a9a;
    cursor: crosshair;
    z-index: 3;
  }

  .port:hover,
  .card.target .port.in,
  .io.target .port.in {
    border-color: var(--accent);
    background: #173;
  }

  .io .port {
    top: 27px;
  }

  .port.in {
    left: -8px;
  }

  .port.out {
    right: -8px;
  }

  .sum {
    position: absolute;
    left: -34px;
    top: 13px;
    font-size: 9px;
    color: var(--text-dim);
    background: #181818;
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 0 3px;
    white-space: nowrap;
  }

  .io .sum {
    top: 25px;
  }

  .add {
    position: absolute;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--bg-raised);
    color: var(--text-dim);
    z-index: 2;
  }

  .card {
    position: absolute;
    border-radius: 10px;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.45);
    cursor: grab;
    z-index: 1;
  }

  .card.lifted {
    z-index: 4;
    cursor: grabbing;
    border-color: var(--accent);
    box-shadow: 0 20px 36px rgba(0, 0, 0, 0.7);
  }

  .card.target,
  .card.hl {
    border-color: var(--accent);
    box-shadow:
      0 0 0 1px var(--accent),
      0 6px 16px rgba(0, 0, 0, 0.45);
  }

  .card.mute {
    border-style: dashed;
    border-color: #5a5a5a;
    background: #181818;
    box-shadow: none;
  }

  .card.mute .bar {
    background: #1f1f1f;
  }

  .card.mute .port {
    border-color: #555;
    background: #151515;
  }

  .card.mute .body {
    opacity: 0.6;
  }

  .card.bypass .bar b {
    text-decoration: line-through;
    color: var(--text-dim);
  }

  .card.bypass .body {
    opacity: 0.45;
  }

  .badge {
    position: absolute;
    left: 10px;
    top: -19px;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 10px;
    color: var(--text-faint);
    white-space: nowrap;
    pointer-events: none;
  }

  .badge.dim {
    color: var(--text-dim);
  }

  .bar {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 5px 0 9px;
    border-radius: 10px 10px 0 0;
    background: color-mix(in srgb, var(--nc) 28%, var(--bg-panel));
    border-bottom: 1px solid color-mix(in srgb, var(--nc) 50%, transparent);
  }

  .bar .kind {
    display: inline-flex;
    color: var(--nc);
  }

  .bar b {
    flex: 1;
    min-width: 0;
    font-size: var(--fs-md);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .name-input {
    flex: 1;
    min-width: 0;
    height: 22px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    padding: 0 5px;
  }

  .dim {
    color: var(--text-dim);
  }

  .small {
    font-size: 10px;
  }

  .body {
    padding: 6px 10px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .kind-label {
    font-size: 10px;
    color: var(--nc);
  }

  .note {
    display: flex;
    gap: 4px;
    font-size: 10px;
    color: var(--text-dim);
    line-height: 1.4;
  }

  .note-input {
    width: 100%;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-xs);
    resize: vertical;
  }

  .pm {
    font-size: 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pm .top {
    display: flex;
    justify-content: space-between;
    color: var(--text-dim);
  }

  .pm .top span:last-child {
    font-family: var(--mono);
  }

  .pm input[type="range"] {
    width: 100%;
    height: 12px;
    margin: 0;
    accent-color: var(--nc);
  }

  .pm select {
    height: 20px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: 10px;
  }

  .more {
    font-size: 10px;
    color: var(--text-faint);
  }

  .drop-ghost {
    position: absolute;
    z-index: 5;
    height: 60px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 0 8px;
    border: 1px dashed var(--accent);
    border-radius: 10px;
    background: rgba(10, 30, 28, 0.6);
    color: var(--accent);
    font-size: var(--fs-xs);
    text-align: center;
    pointer-events: none;
  }

  .tip {
    position: absolute;
    z-index: 7;
    font-size: var(--fs-xs);
    color: var(--accent);
    background: rgba(10, 30, 28, 0.94);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-md);
    padding: 3px 8px;
    white-space: nowrap;
    pointer-events: none;
  }

  .tip.bad {
    color: var(--danger-text);
    border-color: var(--danger);
    background: rgba(40, 14, 18, 0.94);
  }

  .pop {
    position: absolute;
    z-index: 8;
    transform: translate(-50%, 14px);
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: var(--fs-sm);
  }

  .pop .row {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }

  .pop input[type="range"] {
    width: 130px;
    accent-color: var(--accent);
  }

  .pop .v {
    font-family: var(--mono);
    font-size: 11px;
    width: 58px;
    text-align: right;
    color: var(--text-dim);
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .menu {
    position: fixed;
    z-index: 20;
    max-height: calc(100vh - 16px);
    overflow-y: auto;
    min-width: 200px;
    max-width: 360px;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 0;
    background: none;
    text-align: left;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
    --icon-size: 15px;
  }

  .menu button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .menu button > :global(.icon) {
    color: var(--text-dim);
  }

  .menu button.rich span {
    display: flex;
    flex-direction: column;
  }

  .menu small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .menu .danger {
    color: var(--danger-text);
  }

  .menu .key {
    margin-left: auto;
    padding-left: 20px;
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }

  .menu-h {
    font-size: var(--fs-xs);
    color: var(--text-faint);
    padding: 4px 10px 2px;
  }

  .menu-sep {
    height: 1px;
    background: var(--border);
    margin: 3px 4px;
  }
</style>
