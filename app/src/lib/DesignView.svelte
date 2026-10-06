<script lang="ts">
  // 設計画面(曲の設計データ)。曲全体の計画、区間ごとの計画(盛り上がり・パートごとの音域・パートの役割)と
  // 測った値・ずれ、選んだ所の詳しい情報。画面で直す(段階 2): 盛り上がりの点・区間の平行移動・境目、音域の帯、
  // 役割のマス(選択肢・数字キー・塗り)、曲全体の項目、メモ、推定した計画の採用。区間とテンポは曲の履歴、ほかは計画の履歴に残る。
  // 見た目と操作は docs/SONG_DESIGN_DATA.md §5(触れる試作で決めたもの)に沿う。
  import { tick } from "svelte";
  import Icon from "./Icon.svelte";
  import { addBars as addBarsIn, rangeFromChanges } from "./abRange";
  import { abLooping } from "./abLoop";
  import { shouldYieldKey } from "./keys";
  import { transportSeek } from "./api";
  import type { Project } from "./types";
  import {
    arcLabels,
    functionLabels,
    presenceLabels,
    MAX_PROPOSALS_AB,
    abLetter,
    abProposals,
    abSingle,
    addMemo,
    adoptProposal,
    askChat,
    discardProposal,
    endProposalAb,
    proposalAb,
    proposals,
    restartProposalAb,
    setProposalSide,
    designSel,
    designStore,
    designTargetLabel,
    editPartPlan,
    editSections,
    editSongPlan,
    estimatePlans,
    maybeAutoEstimate,
    memosFor,
    noteName,
    partCell,
    sectionCurve,
    setTempo,
    settleEstimated,
    type DesignCell,
    type DesignData,
    type DesignDeviation,
    type DesignSel,
  } from "./design.svelte";
  import { saveSettings, settings } from "./settings.svelte";
  import { selectionStore } from "./selection.svelte";
  import { transportStore } from "./transport.svelte";
  import { showToast } from "./toast.svelte";
  import { isEn, plural, tr } from "./i18n.svelte";

  let { project }: { project: Project } = $props();

  const d = $derived(designStore.data);
  // 働き・存在の段階・盛り上がりの型の表示名(画面の言語で。言語を切り替えると作り直される)
  const functionNames = $derived(functionLabels());
  const presenceNames = $derived(presenceLabels());
  const arcNames = $derived(arcLabels());
  const sel = $derived(designSel.sel);

  // ---- 開け閉め・大きさ(覚えておく) ----
  function load(key: string, fallback: string): string {
    try {
      return localStorage.getItem(key) ?? fallback;
    } catch {
      return fallback;
    }
  }
  function store(key: string, v: string) {
    try {
      localStorage.setItem(key, v);
    } catch {
      // 保存できなくても動作には関係しない
    }
  }
  type Lane = "song" | "curve" | "band" | "table";
  let folded = $state<Record<Lane, boolean>>({
    song: load("glaux.design.fold.song", "1") === "1",
    curve: load("glaux.design.fold.curve", "0") === "1",
    band: load("glaux.design.fold.band", "0") === "1",
    table: load("glaux.design.fold.table", "0") === "1",
  });
  function toggleFold(l: Lane, e: MouseEvent) {
    if ((e.target as HTMLElement).closest(".tools, .legend, .qmark, .pickall, input, select, label")) return;
    folded[l] = !folded[l];
    store(`glaux.design.fold.${l}`, folded[l] ? "1" : "0");
  }

  const CT = 14;
  const CB = 22;
  let CH = $state(Number(load("glaux.design.curveH", "130")) || 130);
  const CP = $derived(CH - CT - CB);
  const cy = (v: number) => CT + (10 - v) * (CP / 10);
  const PMIN = 12;
  const PMAX = 108;
  let BH = $state(Number(load("glaux.design.bandH", "300")) || 300);
  const BY = (p: number) => BH - ((p - PMIN) / (PMAX - PMIN)) * BH;
  const ROW_MIN = 24;
  const ROW_MAX = 64;
  let tableH = $state(Number(load("glaux.design.tableBodyH", "0")) || 0);
  const nParts = $derived(Math.max(1, d?.parts.length ?? 1));
  const tableBody = $derived(tableH > 0 ? tableH : 38 * nParts);
  const rowH = $derived(Math.max(ROW_MIN, Math.min(ROW_MAX, Math.floor(tableBody / nParts))));
  let showMeasured = $state(load("glaux.design.measured", "1") === "1");
  let showOverlap = $state(false);

  // ---- 横の位置。既定は曲全体が見える幅(全体)。拡大・縮小はタイムラインと別に持つ(設計は曲全体を見渡すことが多いため)
  const totalBars = $derived(Math.max(1, ...(d?.sections ?? []).map((s) => s.start_bar - 1 + s.bars)));
  let visW = $state(600);
  let zoom = $state<number | null>(Number(load("glaux.design.pxPerBar", "0")) || null);
  const fitPx = $derived(Math.max(4, visW / totalBars));
  const pxb = $derived(zoom ?? fitPx);
  function setZoom(z: number | null) {
    zoom = z == null ? null : Math.max(4, Math.min(160, z));
    store("glaux.design.pxPerBar", String(zoom ?? 0));
  }
  const W = $derived(Math.max(visW, Math.round(totalBars * pxb)));
  const x0 = (i: number) => ((d?.sections[i]?.start_bar ?? 1) - 1) * pxb;
  const xw = (i: number) => (d?.sections[i]?.bars ?? 1) * pxb;
  let sx = $state(0);
  let hbar = $state<HTMLDivElement>();
  function onHbar() {
    sx = hbar?.scrollLeft ?? 0;
  }
  function onWheel(e: WheelEvent) {
    const dx = e.shiftKey ? e.deltaY : e.deltaX;
    if (!dx || !hbar) return;
    e.preventDefault();
    hbar.scrollLeft += dx;
  }

  // ---- 選ぶ ----
  function pick(s: DesignSel) {
    const prev = designSel.sel;
    designSel.sel = s;
    if (s.kind === "part" || s.kind === "cell") designSel.part = s.p;
    // 別の区間(またはその区間のマス)を選んだら、再生位置をその区間の頭へ(再生中ならそこから続けて鳴る)。
    // 同じ区間の中で選び直したとき(曲線を直すなど)と、聴き比べの間(範囲の外は鳴らない)は動かさない
    const secOf = (x: DesignSel) => (x.kind === "section" || x.kind === "cell" ? x.i : -1);
    const i = secOf(s);
    if (i >= 0 && i !== secOf(prev) && sortedSecs[i] && !abLooping()) transportSeek(sortedSecs[i].tick).catch(() => {});
  }
  function pickWhole(k: "song" | "sections" | "curve" | "band" | "table", e: MouseEvent) {
    e.stopPropagation();
    const on = k === "song" ? sel.kind === "song" : sel.kind === "lane" && sel.lane === k;
    // 「全体を選択中」をもう一度押すと選択を外す
    pick(on ? { kind: "none" } : k === "song" ? { kind: "song" } : { kind: "lane", lane: k });
  }
  const isWhole = (k: string) => (k === "song" ? sel.kind === "song" : sel.kind === "lane" && sel.lane === k);
  const selPart = $derived(Math.min(designSel.part, Math.max(0, (d?.parts.length ?? 1) - 1)));

  // ---- ドラッグ(大きさの変更)。外で離して終わりの知らせが届かなくても、戻ってきたときボタンが離れていれば終える ----
  let dragging = $state(false);
  function track(move: (e: PointerEvent) => void, done: () => void) {
    dragging = true;
    let ended = false;
    const mv = (e: PointerEvent) => {
      if (e.buttons === 0) return end();
      move(e);
    };
    const end = () => {
      if (ended) return;
      ended = true;
      dragging = false;
      window.removeEventListener("pointermove", mv);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
      done();
    };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
  }
  // 縮めたときに枠のスクロールが詰まってつまみがカーソルから離れないよう、下に一時的な余白を足す(上へスクロールすると消える)
  let group = $state<HTMLDivElement>();
  let slack = $state(0);
  function fitSlack(grow: boolean, top?: number) {
    const g = group;
    if (!g) return;
    const t = top ?? g.scrollTop;
    const content = g.scrollHeight - slack;
    let need = Math.max(0, t + g.clientHeight - content);
    if (!grow) need = Math.min(need, slack);
    slack = need;
    if (top != null) queueMicrotask(() => g && (g.scrollTop = top));
  }
  function grip(kind: "curve" | "band" | "table", e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const y0 = e.clientY;
    const v0 = kind === "curve" ? CH : kind === "band" ? BH : tableBody;
    track(
      async (m) => {
        const t = group?.scrollTop ?? 0;
        const v = v0 + m.clientY - y0;
        if (kind === "curve") CH = Math.round(Math.max(130, Math.min(480, v)));
        else if (kind === "band") BH = Math.round(Math.max(160, Math.min(900, v)));
        else tableH = Math.round(Math.max(80, Math.min(ROW_MAX * nParts, v)));
        await tick();
        fitSlack(true, t);
      },
      () => {
        store("glaux.design.curveH", String(CH));
        store("glaux.design.bandH", String(BH));
        store("glaux.design.tableBodyH", String(tableH));
      },
    );
  }
  function resetGrip(kind: "curve" | "band" | "table") {
    if (kind === "curve") CH = 130;
    else if (kind === "band") BH = 300;
    else tableH = 0;
    store("glaux.design.curveH", String(CH));
    store("glaux.design.bandH", String(BH));
    store("glaux.design.tableBodyH", String(tableH));
    tick().then(() => fitSlack(false));
  }

  // ---- 計画と実際 ----
  const avgCurve = (c: [number, number][]) => {
    const N = 32;
    let sum = 0;
    for (let k = 0; k <= N; k++) sum += valueAt(c, k / N);
    return sum / (N + 1);
  };
  function valueAt(c: [number, number][], t: number): number {
    if (t <= c[0][0]) return c[0][1];
    for (let k = 1; k < c.length; k++) {
      const [a, va] = c[k - 1];
      const [b, vb] = c[k];
      if (t <= b) return b > a ? va + ((vb - va) * (t - a)) / (b - a) : vb;
    }
    return c[c.length - 1][1];
  }
  const offSection = (i: number) => {
    const s = d?.sections[i];
    return !!s && s.planned != null && Math.abs(s.measured - s.planned) >= 1.5;
  };
  function cellOff(c: DesignCell): boolean {
    if (c.planned == null) return false;
    if (c.planned === 0) return c.notes > 0;
    if (c.notes === 0) return true;
    if (Math.abs(c.planned - c.measured) >= 2) return true;
    if (c.planned_register && c.register) {
      const [lo, hi] = c.planned_register;
      return c.register[0] < lo - 2 || c.register[1] > hi + 2;
    }
    return false;
  }
  const estimated = $derived(!!d && (d.song_estimated || d.parts.some((p) => p.estimated)));
  const tempo = $derived.by(() => {
    const t = project.tempo_map ?? [];
    if (t.length > 1) return tr("途中で変わる", "Varies");
    return `${t[0]?.bpm ?? 120} BPM`;
  });

  // パートの色(タイムラインで付けた色。無ければ既定の灰色)
  const colorOf = (i: number) => d?.parts[i]?.color ?? "#9a9a9a";
  // 強いパートどうしの重なり(参考)
  const overlaps = $derived.by(() => {
    if (!d || !showOverlap) return [];
    const out: { i: number; lo: number; hi: number }[] = [];
    d.sections.forEach((_, i) => {
      const strong = d.parts.map((p) => p.cells[i]).filter((c) => c && (c.planned ?? 0) >= 3 && c.planned_register);
      for (let a = 0; a < strong.length; a++)
        for (let b = a + 1; b < strong.length; b++) {
          const [l1, h1] = strong[a].planned_register!;
          const [l2, h2] = strong[b].planned_register!;
          const lo = Math.max(l1, l2);
          const hi = Math.min(h1, h2);
          if (hi > lo) out.push({ i, lo, hi });
        }
    });
    return out;
  });

  // ---- 詳しい情報(右のパネル) ----
  const devOf = (pred: (x: DesignDeviation) => boolean) => (d?.deviations ?? []).filter(pred);
  const sectionKey = (i: number) => d?.sections[i]?.id ?? d?.sections[i]?.name ?? "";
  const partClips = (name: string) => Object.values(designStore.clips).filter((c) => c.track === name);

  function askAi() {
    window.dispatchEvent(new CustomEvent("glaux:focus-chat"));
  }

  // ================================================================ 直す(段階 2)
  const clamp10 = (v: number) => Math.max(0, Math.min(10, Math.round(v * 10) / 10));
  const hasSections = $derived((project.sections ?? []).length > 0);
  // 読み直すまでの下書き(ドラッグ中・保存の返事を待つ間に見せる)。読み直したら捨てる(ドラッグ中は捨てない)
  let curveDraft = $state<Record<number, [number, number][]>>({});
  let bandDraft = $state<Record<number, [number, number]>>({});
  let cellDraft = $state<Record<string, number>>({});
  let seenData: DesignData | null = null;
  $effect(() => {
    const cur = designStore.data;
    if (cur !== seenData) {
      seenData = cur;
      if (!dragging) {
        curveDraft = {};
        bandDraft = {};
        cellDraft = {};
      }
    }
  });
  /** 盛り上がりの形(下書きがあれば下書き)。計画が無い区間は null */
  const curveOf = (i: number): [number, number][] | null => {
    if (curveDraft[i]) return curveDraft[i];
    const s = d?.sections[i];
    return s ? sectionCurve(s) : null;
  };
  /** 直すときの元の形(計画が無ければ測った値の平ら) */
  const editBase = (i: number): [number, number][] => {
    const c = curveOf(i);
    if (c) return c.map(([t, v]) => [t, v] as [number, number]);
    const m = clamp10(d?.sections[i]?.measured ?? 5);
    return [
      [0, m],
      [1, m],
    ];
  };
  const joinOf = (i: number) => d?.sections[i]?.join ?? "smooth";
  function needSections(): boolean {
    if (hasSections) return true;
    showToast(
      "warn",
      tr(
        "区間がありません。タイムラインでマーカーを置くか、AI に set_song_plan で計画書を書いてもらうと直せます",
        "No sections. Place markers on the timeline, or have the AI write a plan with set_song_plan, to edit this",
      ),
    );
    return false;
  }
  /** 下書きの形を区間に書き込む(1 件の編集。曲の履歴に残る) */
  async function commitCurves(idx: number[], label: string) {
    const c = { ...curveDraft };
    await editSections(
      project,
      (secs) => {
        for (const i of idx) {
          if (!secs[i] || !c[i]) continue;
          secs[i].curve = c[i];
          secs[i].energy = Math.round(avgCurve(c[i]) * 10) / 10;
        }
      },
      label,
    );
  }
  /** つないだ境目は、隣の端も同じ値にそろえる(下書きの上で) */
  function linkEnds(i: number, k: number, v: number, draft: Record<number, [number, number][]>, changed: Set<number>) {
    const c = draft[i];
    if (k === c.length - 1 && joinOf(i) !== "step" && d?.sections[i + 1]) {
      draft[i + 1] = draft[i + 1] ?? editBase(i + 1);
      draft[i + 1][0][1] = v;
      changed.add(i + 1);
    }
    if (k === 0 && i > 0 && joinOf(i - 1) !== "step") {
      draft[i - 1] = draft[i - 1] ?? editBase(i - 1);
      const p = draft[i - 1];
      p[p.length - 1][1] = v;
      changed.add(i - 1);
    }
  }
  function svgPoint(e: PointerEvent | MouseEvent, svg: SVGSVGElement, i: number) {
    const r = svg.getBoundingClientRect();
    return { t: (e.clientX - r.left - x0(i)) / xw(i), v: 10 - (e.clientY - r.top - CT) / (CP / 10) };
  }
  // 点をドラッグ(上下で値、間の点は左右にも)
  function dragPoint(e: PointerEvent, i: number, k: number) {
    if (e.button !== 0 || !needSections()) return;
    e.preventDefault();
    e.stopPropagation();
    pick({ kind: "section", i });
    const svg = (e.currentTarget as SVGElement).ownerSVGElement!;
    const changed = new Set([i]);
    const draft: Record<number, [number, number][]> = { [i]: editBase(i) };
    const n = draft[i].length;
    let moved = false;
    track(
      (m) => {
        moved = true;
        const p = svgPoint(m, svg, i);
        const v = clamp10(p.v);
        draft[i][k][1] = v;
        if (k > 0 && k < n - 1) draft[i][k][0] = Math.max(draft[i][k - 1][0] + 0.02, Math.min(draft[i][k + 1][0] - 0.02, p.t));
        linkEnds(i, k, v, draft, changed);
        curveDraft = { ...curveDraft, ...structuredClone(draft) };
      },
      () => {
        if (moved) commitCurves([...changed], tr(`「${d?.sections[i]?.name}」の盛り上がりの形を変える`, `Change energy shape of "${d?.sections[i]?.name}"`));
      },
    );
  }
  // 区間の面: 上下にドラッグでその区間ごと平行に(Shift で全区間)。ダブルクリックで点を足す
  let lastDown = { t: 0, x: 0, y: 0 };
  function dragArea(e: PointerEvent, i: number) {
    if (e.button !== 0) return;
    const now = performance.now();
    const svg = (e.currentTarget as SVGElement).ownerSVGElement!;
    if (now - lastDown.t < 350 && Math.hypot(e.clientX - lastDown.x, e.clientY - lastDown.y) < 6) {
      lastDown.t = 0;
      if (!needSections()) return;
      const p = svgPoint(e, svg, i);
      if (p.t <= 0.02 || p.t >= 0.98) return;
      const c = editBase(i);
      c.push([Math.round(p.t * 1000) / 1000, clamp10(p.v)]);
      c.sort((a, b) => a[0] - b[0]);
      curveDraft = { ...curveDraft, [i]: c };
      commitCurves([i], tr(`「${d?.sections[i]?.name}」の盛り上がりに点を足す`, `Add energy point to "${d?.sections[i]?.name}"`));
      return;
    }
    lastDown = { t: now, x: e.clientX, y: e.clientY };
    e.preventDefault();
    pick({ kind: "section", i });
    const all = e.shiftKey;
    const y0 = e.clientY;
    const idx = all ? (d?.sections.map((_, k) => k) ?? []) : [i];
    const start: Record<number, [number, number][]> = Object.fromEntries(idx.map((k) => [k, editBase(k)]));
    let moved = false;
    const changed = new Set(idx);
    track(
      (m) => {
        if (!moved && Math.abs(m.clientY - y0) <= 3) return;
        if (!moved && !needSections()) return;
        moved = true;
        const dv = -(m.clientY - y0) / (CP / 10);
        const draft: Record<number, [number, number][]> = {};
        for (const k of idx) draft[k] = start[k].map(([t, v]) => [t, clamp10(v + dv)] as [number, number]);
        if (!all) {
          const c = draft[i];
          linkEnds(i, c.length - 1, c[c.length - 1][1], draft, changed);
          linkEnds(i, 0, c[0][1], draft, changed);
        }
        curveDraft = { ...curveDraft, ...draft };
      },
      () => {
        if (moved)
          commitCurves(
            [...changed],
            all
              ? tr("盛り上がり全体を平行に動かす", "Shift whole energy curve")
              : tr(`「${d?.sections[i]?.name}」の盛り上がりを平行に動かす`, `Shift energy of "${d?.sections[i]?.name}"`),
          );
      },
    );
  }
  function removePoint(e: MouseEvent, i: number, k: number) {
    e.preventDefault();
    const c = editBase(i);
    if (k === 0 || k === c.length - 1) {
      showToast("warn", tr("区間の両端の点は消せません", "Can't remove a section's end points"));
      return;
    }
    if (!needSections()) return;
    c.splice(k, 1);
    curveDraft = { ...curveDraft, [i]: c };
    commitCurves([i], tr(`「${d?.sections[i]?.name}」の盛り上がりの点を消す`, `Remove energy point from "${d?.sections[i]?.name}"`));
  }
  async function toggleJoin(i: number) {
    if (!needSections()) return;
    const toStep = joinOf(i) !== "step";
    const a = editBase(i);
    const b = editBase(i + 1);
    if (!toStep) b[0][1] = a[a.length - 1][1];
    await editSections(
      project,
      (secs) => {
        secs[i].join = toStep ? "step" : undefined;
        if (!toStep && secs[i + 1]) {
          secs[i + 1].curve = b;
          secs[i + 1].energy = Math.round(avgCurve(b) * 10) / 10;
        }
      },
      toStep
        ? tr(`「${d?.sections[i]?.name}」の次との境目を段差に`, `Make boundary after "${d?.sections[i]?.name}" a step`)
        : tr(`「${d?.sections[i]?.name}」の次との境目をつなぐ`, `Smooth boundary after "${d?.sections[i]?.name}"`),
    );
  }
  /** 盛り上がり全体: 上げ下げ(dv)か、平均を中心に起伏を大きく・小さく(scale) */
  async function curveAll(op: { dv?: number; scale?: number }, label: string) {
    if (!needSections() || !d) return;
    const all = d.sections.map((_, i) => editBase(i));
    const mean = all.flat().reduce((a, p) => a + p[1], 0) / Math.max(1, all.flat().length);
    const draft: Record<number, [number, number][]> = {};
    all.forEach((c, i) => {
      draft[i] = c.map(([t, v]) => [t, clamp10(op.scale ? mean + (v - mean) * op.scale : v + (op.dv ?? 0))] as [number, number]);
    });
    curveDraft = draft;
    await commitCurves(Object.keys(draft).map(Number), label);
  }
  /** 盛り上がりの型を当てる(区間の形は残し、平均を型の高さに合わせる) */
  async function applyArc() {
    const arc = d?.song?.arc;
    if (!d || !arc || !needSections()) return;
    const n = d.sections.length;
    const target = (t: number) =>
      arc === "rise" ? 2 + 7 * t : arc === "waves" ? 5 + 3.5 * Math.sin(Math.PI * (4 * t - 0.5)) : arc === "peak" ? 2 + 7 * Math.pow(Math.sin(Math.PI * Math.min(1, t * 1.15)), 1.5) : arc === "sink" ? 8 - 6 * t : 5;
    const draft: Record<number, [number, number][]> = {};
    d.sections.forEach((_, i) => {
      const c = editBase(i);
      const dv = target(n > 1 ? i / (n - 1) : 0.5) - avgCurve(c);
      draft[i] = c.map(([t, v]) => [t, clamp10(v + dv)] as [number, number]);
    });
    curveDraft = draft;
    await commitCurves(Object.keys(draft).map(Number), tr(`盛り上がりの型(${arcNames[arc]})を当てる`, `Apply energy arc (${arcNames[arc]})`));
  }

  // ---- 音域の帯(選んだパート) ----
  const regOf = (i: number): [number, number] | null => bandDraft[i] ?? d?.parts[selPart]?.cells[i]?.planned_register ?? null;
  function dragBand(e: PointerEvent, i: number, edge: "move" | "top" | "bottom") {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    pick({ kind: "cell", p: selPart, i });
    const part = d?.parts[selPart];
    if (!part) return;
    const all = e.shiftKey;
    const idx = all ? part.cells.map((_, k) => k).filter((k) => regOf(k)) : [i];
    const start: Record<number, [number, number]> = Object.fromEntries(idx.map((k) => [k, [...regOf(k)!] as [number, number]]));
    const y0 = e.clientY;
    let moved = false;
    track(
      (m) => {
        const dp = -Math.round(((m.clientY - y0) / BH) * (PMAX - PMIN));
        if (dp === 0 && !moved) return;
        moved = true;
        const draft: Record<number, [number, number]> = {};
        for (const k of idx) {
          const [lo, hi] = start[k];
          draft[k] =
            edge === "top"
              ? [lo, Math.min(127, Math.max(lo + 1, hi + dp))]
              : edge === "bottom"
                ? [Math.max(0, Math.min(hi - 1, lo + dp)), hi]
                : [Math.max(0, lo + dp), Math.min(127, hi + dp)];
        }
        bandDraft = { ...bandDraft, ...draft };
      },
      () => {
        if (!moved) return;
        const draft = { ...bandDraft };
        editPartPlan(
          project,
          selPart,
          (b, dd) => {
            for (const k of idx) if (draft[k]) partCell(b, dd, selPart, k).register = draft[k];
          },
          tr(
            `「${part.name}」の${all ? "全区間" : `「${d?.sections[i]?.name}」`}の音域の帯を変える`,
            `Change register band of "${part.name}" (${all ? "all sections" : `"${d?.sections[i]?.name}"`})`,
          ),
        );
      },
    );
  }
  let bandTarget = $state<"one" | "all">("one");
  /** 音域の帯をまとめて: 上げ下げ(dp 半音)か、幅を広げる・狭める(dw 半音ずつ上下に) */
  async function bandBulk(dp: number, dw: number) {
    if (!d) return;
    const parts = bandTarget === "all" ? d.parts.map((_, k) => k) : [selPart];
    let n = 0;
    for (const pi of parts) {
      const p = d.parts[pi];
      if (!p.cells.some((c) => c.planned_register)) continue;
      n++;
      await editPartPlan(
        project,
        pi,
        (b, dd) => {
          p.cells.forEach((c, i) => {
            if (!c.planned_register) return;
            let [lo, hi] = c.planned_register;
            lo = lo + dp - dw;
            hi = hi + dp + dw;
            if (hi - lo < 1) return;
            partCell(b, dd, pi, i).register = [Math.max(0, lo), Math.min(127, hi)];
          });
        },
        dw
          ? tr(`「${p.name}」の音域の幅を${dw > 0 ? "広げる" : "狭める"}`, `${dw > 0 ? "Widen" : "Narrow"} register of "${p.name}"`)
          : tr(
              `「${p.name}」の音域を${dp > 0 ? "上げる" : "下げる"}(${Math.abs(dp) === 12 ? "オクターブ" : "半音"})`,
              `${dp > 0 ? "Raise" : "Lower"} register of "${p.name}" (${Math.abs(dp) === 12 ? "octave" : "semitone"})`,
            ),
      );
    }
    if (!n) showToast("warn", tr("計画の音域の帯がまだありません(「実際の音域を計画に」で作れます)", "No planned register bands yet (create them with \"Match plan to actual\")"));
  }
  /** 選んだパートの実際の音域を、計画の帯にする(鳴っている区間だけ) */
  async function bandFromReal() {
    const p = d?.parts[selPart];
    if (!p) return;
    await editPartPlan(
      project,
      selPart,
      (b, dd) => {
        p.cells.forEach((c, i) => {
          if (c.register) partCell(b, dd, selPart, i).register = c.register;
        });
      },
      tr(`「${p.name}」の実際の音域を計画の帯にする`, `Set planned register of "${p.name}" from actual`),
    );
  }

  // ---- 役割のマス ----
  const lvOf = (pi: number, i: number) => {
    const k = `${pi}:${i}`;
    if (k in cellDraft) return cellDraft[k];
    const c = d?.parts[pi]?.cells[i];
    return c?.planned ?? null;
  };
  async function setPresence(pi: number, idx: number[], lv: number) {
    const p = d?.parts[pi];
    if (!p) return;
    const keep = idx.filter((i) => !p.cells[i]?.locked);
    if (keep.length < idx.length) showToast("warn", tr("固定のマスは変えませんでした(固定を外すと変えられます)", "Locked cells were left unchanged (unlock them to change)"));
    if (!keep.length) return;
    cellDraft = { ...cellDraft, ...Object.fromEntries(keep.map((i) => [`${pi}:${i}`, lv])) };
    await editPartPlan(
      project,
      pi,
      (b, dd) => {
        for (const i of keep) partCell(b, dd, pi, i).presence = lv;
      },
      tr(
        `「${p.name}」の${keep.length > 1 ? `${keep.length} 区間` : `「${d?.sections[keep[0]]?.name}」`}を「${presenceNames[lv]}」に`,
        `Set "${p.name}" ${keep.length > 1 ? `(${keep.length} sections)` : `"${d?.sections[keep[0]]?.name}"`} to "${presenceNames[lv]}"`,
      ),
    );
  }
  async function setCellProp(pi: number, i: number, prop: "function" | "locked", v: string | boolean | undefined) {
    const p = d?.parts[pi];
    if (!p) return;
    await editPartPlan(
      project,
      pi,
      (b, dd) => {
        const c = partCell(b, dd, pi, i);
        if (prop === "function") {
          if (!v || v === b.function) delete c.function;
          else c.function = String(v);
        } else c.locked = v ? true : undefined;
      },
      prop === "locked"
        ? tr(`「${p.name}」の「${d?.sections[i]?.name}」を${v ? "固定する" : "固定を外す"}`, `${v ? "Lock" : "Unlock"} "${p.name}" / "${d?.sections[i]?.name}"`)
        : tr(`「${p.name}」の「${d?.sections[i]?.name}」の働きを変える`, `Change function of "${p.name}" / "${d?.sections[i]?.name}"`),
    );
  }
  async function setPartFunction(pi: number, f: string) {
    const p = d?.parts[pi];
    if (!p) return;
    await editPartPlan(
      project,
      pi,
      (b) => {
        if (f) b.function = f;
        else delete b.function;
      },
      tr(`「${p.name}」の働きを「${f ? functionNames[f] : "未設定"}」に`, `Set function of "${p.name}" to "${f ? functionNames[f] : "unset"}"`),
    );
  }
  // 選択肢(マスの ▾ で開き、∧ で閉じる)
  let pop = $state<{ p: number; i: number; x: number; y: number; up: boolean } | null>(null);
  function togglePop(e: MouseEvent, pi: number, i: number) {
    e.stopPropagation();
    if (pop && pop.p === pi && pop.i === i) {
      pop = null;
      return;
    }
    pick({ kind: "cell", p: pi, i });
    const cell = (e.currentTarget as HTMLElement).closest(".cell") as HTMLElement;
    const r = cell.getBoundingClientRect();
    const up = r.bottom + 330 > window.innerHeight;
    pop = { p: pi, i, x: Math.max(8, Math.min(window.innerWidth - 270, r.right - 260)), y: up ? r.top - 4 : r.bottom + 4, up };
  }
  // 押したまま横へドラッグで、同じ段階を塗る
  let paint: { p: number; lv: number; from: number; cells: Set<number> } | null = null;
  function paintStart(e: PointerEvent, pi: number, i: number) {
    if (e.button !== 0 || (e.target as HTMLElement).closest(".cellbtn")) return;
    e.preventDefault();
    paint = { p: pi, lv: lvOf(pi, i) ?? d?.parts[pi]?.cells[i]?.measured ?? 3, from: i, cells: new Set() };
    track(
      () => {},
      () => {
        const pt = paint;
        paint = null;
        // 塗り始めのマスも同じ段階にする(計画の無いマスは、ここで計画に入る)
        if (pt && pt.cells.size) setPresence(pt.p, [pt.from, ...pt.cells], pt.lv);
      },
    );
  }
  function paintEnter(e: PointerEvent, pi: number, i: number) {
    if (!paint || e.buttons === 0 || paint.p !== pi || i === paint.from) return;
    if (d?.parts[pi]?.cells[i]?.locked) return;
    paint.cells.add(i);
    cellDraft = { ...cellDraft, [`${pi}:${i}`]: paint.lv };
  }
  function onKey(e: KeyboardEvent) {
    // 文字を打つ欄などには譲る(以前はスライダー・選択欄に入力先があるだけで、数字キーと Esc が効かなかった)
    if (shouldYieldKey(e)) return;
    if (e.key === "Escape" && pop) {
      pop = null;
      return;
    }
    if (sel.kind === "cell" && /^[0-5]$/.test(e.key) && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      pop = null;
      setPresence(sel.p, [sel.i], Number(e.key));
    }
  }

  // ---- 曲全体 ----
  async function songField(key: string, value: unknown, label: string) {
    await editSongPlan((b) => {
      if (value === "" || value == null || (Array.isArray(value) && value.length === 0)) delete b[key];
      else b[key] = value;
    }, label);
  }
  let moodInput = $state("");
  let mustInput = $state("");
  const tempoSingle = $derived((project.tempo_map ?? []).length <= 1);

  // ---- メモ・頼む ----
  const memoKey = $derived.by(() => {
    if (!d) return "";
    switch (sel.kind) {
      case "song":
        return "song";
      case "lane":
        return sel.lane;
      case "section":
        return `section:${d.sections[sel.i]?.id ?? d.sections[sel.i]?.name}`;
      case "part":
        return `part:${d.parts[sel.p]?.track_id}`;
      case "cell":
        return `cell:${d.parts[sel.p]?.track_id}:${d.sections[sel.i]?.id ?? d.sections[sel.i]?.name}`;
      default:
        return "";
    }
  });
  const memos = $derived(memosFor(d, memoKey));
  const hasMemo = (key: string) => memosFor(d, key).length > 0;
  let memoText = $state("");
  async function saveMemo() {
    const t = memoText.trim();
    if (!t || !memoKey) return;
    await addMemo(memoKey, t);
    memoText = "";
  }
  let overwriteArmed = $state(false);
  let overwriteTimer: ReturnType<typeof setTimeout> | undefined;
  function remake(overwrite: boolean) {
    if (overwrite && !overwriteArmed) {
      overwriteArmed = true;
      clearTimeout(overwriteTimer);
      overwriteTimer = setTimeout(() => (overwriteArmed = false), 4000);
      return;
    }
    overwriteArmed = false;
    askChat(
      overwrite
        ? "選んだ所を、計画の今の版から作り直してください。人が手で直した所も含めて上書きしてかまいません(作り直しの道具は overwrite_edits: true。固定の音は残ります)。"
        : "選んだ所を、計画の今の版から作り直してください。人が手で直した小節と固定の音は残し、残した所を報告してください。",
    );
  }
  // 計画の無い所(音のあるパート・曲全体)の数。計画を一度も作っていない曲は、開いたときに 1 回だけ推定する
  const missingPlans = $derived(
    d ? d.parts.filter((p) => !p.plan_id && p.cells.some((c) => c.notes > 0)).length + (d.song ? 0 : 1) : 0,
  );
  $effect(() => {
    maybeAutoEstimate(project, d);
  });

  // 推定した計画: 「次から確認せずに採用する」なら、出てきたらすぐ採用する
  $effect(() => {
    if (settings.autoAdoptEstimated && estimated) settleEstimated(true);
  });

  // ---- 案(枝)----
  const propList = $derived(proposals(d));
  const planName = (id: string | undefined) => d?.plans.find((p) => p.plan_id === id)?.name;
  const proposalWhy = (id: string) => d?.history.find((h) => h.plan_id === id && h.op === "create")?.why;
  const sortedSecs = $derived([...(project.sections ?? [])].sort((a, b) => a.tick - b.tick));
  /** `tick` から `n` 小節進んだ所(拍子どおり) */
  const addBars = (tick: number, n: number) => addBarsIn(project, tick, n);
  const sectionRange = (i: number) => ({
    start: sortedSecs[i].tick,
    end: sortedSecs[i + 1]?.tick ?? addBars(sortedSecs[i].tick, d?.sections[i]?.bars ?? 8),
  });
  const changesOf = (planId: string) => d?.proposal_changes?.[planId];
  /** 案を聴き比べる範囲: 区間を指定したらその区間。無ければ、選んでいる区間が変わる所ならそこ、
   *  そうでなければ変わる所の最初の区間 → 選んでいる区間 → 選んだ小節 → 変わる範囲の頭から 4 小節 → 再生位置から 8 小節 */
  function proposalRange(planId: string, prefer: number | null = null): { start: number; end: number; section: number | null } {
    const ch = changesOf(planId);
    const selI = sel.kind === "section" || sel.kind === "cell" ? sel.i : -1;
    if (prefer != null && sortedSecs[prefer]) return { ...sectionRange(prefer), section: prefer };
    const changed = ch?.sections ?? [];
    if (changed.length && sortedSecs[changed[0]]) {
      const i = changed.includes(selI) ? selI : changed[0];
      return { ...sectionRange(i), section: i };
    }
    if (selI >= 0 && sortedSecs[selI]) return { ...sectionRange(selI), section: selI };
    const r = selectionStore.range;
    if (r && r.endTick > r.startTick) return { start: r.startTick, end: r.endTick, section: null };
    const fromChanges = ch?.ranges?.length ? rangeFromChanges(project, ch.ranges) : null;
    if (fromChanges) return { ...fromChanges, section: null };
    const start = Math.max(0, transportStore.state.tick ?? 0);
    return { start, end: addBars(start, 8), section: null };
  }
  function listen(planId: string, prefer: number | null = null) {
    const r = proposalRange(planId, prefer);
    if (r.section != null) pick({ kind: "section", i: r.section });
    abProposals([planId], r.start, r.end, r.section);
  }
  // まとめて聴き比べる: 音の変わる・今の曲に当てられる案(最初の 4 つ)を、今と同じ範囲で切り替えて聴く
  const listenable = $derived(propList.filter((p) => p.edits && !changesOf(p.plan_id)?.stale));
  const allIds = $derived(listenable.slice(0, MAX_PROPOSALS_AB).map((p) => p.plan_id));
  /** どれかの案で変わる区間(区間ごとに、変わる案の数) */
  const changedBy = $derived.by(() => {
    const m = new Map<number, number>();
    for (const id of allIds) for (const si of changesOf(id)?.sections ?? []) m.set(si, (m.get(si) ?? 0) + 1);
    return m;
  });
  const allChanged = $derived([...changedBy.keys()].sort((a, b) => a - b));
  const multi = $derived(proposalAb.planIds.length >= 2);
  /** まとめて聴く範囲: 区間を指定したらそこ。無ければ、選んでいる区間が変わる所ならそこ、
   *  そうでなければいちばん多くの案が変わる区間(同じなら前の方)。どれも無ければ最初の案の範囲 */
  function listenAll(prefer: number | null = null) {
    if (allIds.length < 2) return;
    const selI = sel.kind === "section" || sel.kind === "cell" ? sel.i : -1;
    let i = prefer;
    if (i == null && changedBy.has(selI)) i = selI;
    if (i == null && allChanged.length) i = allChanged.reduce((b, x) => ((changedBy.get(x) ?? 0) > (changedBy.get(b) ?? 0) ? x : b));
    const r = i != null && sortedSecs[i] ? { ...sectionRange(i), section: i } : proposalRange(allIds[0]);
    if (r.section != null) pick({ kind: "section", i: r.section });
    abProposals(allIds, r.start, r.end, r.section);
  }
  /** まとめて聴き比べているときの、案の字(B・C …。入っていなければ null) */
  const letterOf = (planId: string) => {
    const k = proposalAb.planIds.indexOf(planId);
    return multi && k >= 0 ? abLetter(k + 1) : null;
  };
  // 案が無くなったら(採用・捨てる・AI が消した)聴き比べも終える
  $effect(() => {
    if (proposalAb.planIds.some((id) => !propList.some((p) => p.plan_id === id))) endProposalAb();
  });

  let lanesEl = $state<HTMLDivElement>();
  // 見える幅(曲全体が見える縮尺の計算に使う)。中身を読み込んでから段ができるので、段ができたら測る
  $effect(() => {
    const el = lanesEl;
    if (!el) return;
    const measure = () => {
      const w = el.querySelector(".hwin")?.clientWidth;
      if (w) visW = w;
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  });
</script>

<svelte:window onkeydown={onKey} onpointerdown={(e) => pop && !(e.target as HTMLElement).closest(".pop, .cellbtn") && (pop = null)} />

<div class="design" class:dragging>
  {#if designStore.error && !d}
    <div class="empty-all">{tr("設計データを読めませんでした", "Couldn't load design data")}: {designStore.error}</div>
  {:else if !d}
    <div class="empty-all">{tr("読み込み中…", "Loading…")}</div>
  {:else}
    {#if estimated}
      <div class="estimate" role="status">
        <b>{tr("推定した計画(未確認)があります", "There are estimated (unconfirmed) plans")}</b>
        <span
          >{tr(
            "今の音から推定した計画です。採用するまで、AI は参考としてだけ使います。",
            "Plans estimated from the current audio. Until adopted, the AI uses them only as a reference.",
          )}</span
        >
        <span class="spacer"></span>
        <button class="btn sm primary" type="button" onclick={() => settleEstimated(true)}>{tr("この計画を採用", "Adopt plans")}</button>
        <button
          class="btn sm"
          type="button"
          title={tr("画面で直してから「この計画を採用」を押すと、直した計画が今の計画になります", "Edit here, then press \"Adopt plans\" to make the edited plans current")}
          onclick={() => showToast("warn", tr("直してから「この計画を採用」を押すと、直した計画が今の計画になります", "Edit first, then press \"Adopt plans\" to make the edited plans current"))}
          >{tr("直してから採用", "Edit, then adopt")}</button
        >
        <button class="btn sm" type="button" onclick={() => settleEstimated(false)}>{tr("捨てる", "Discard")}</button>
        <label
          ><input
            type="checkbox"
            checked={settings.autoAdoptEstimated}
            onchange={(e) => {
              settings.autoAdoptEstimated = (e.currentTarget as HTMLInputElement).checked;
              saveSettings();
            }}
          /> {tr("次から確認せずに採用する", "Adopt without asking next time")}</label
        >
      </div>
    {/if}
    {#if propList.length}
      <div class="proposals" role="region" aria-label={tr("AI の案", "AI proposals")}>
        <div class="phead">
          <b>{tr(`AI の案(${propList.length})`, `AI proposals (${propList.length})`)}</b>
          <span class="hint"
            >{tr(
              "今の曲と計画は変わっていません。聴き比べて、よければ採用してください。聴き比べ中は範囲を繰り返し鳴らし、A / B でいつでも切り替えられます",
              "The current song and plan are unchanged. Compare, and adopt if you like it. While comparing, the range loops and you can switch A / B anytime",
            )}</span
          >
        </div>
        {#if allIds.length >= 2}
          <div class="pall" class:on={multi}>
            {#if multi}
              {@const info = proposalAb.info}
              <div class="prow">
                <span class="abswitch" role="group" aria-label={tr("今と案を切り替える", "Switch between now and proposal")}>
                  <button class="btn sm" class:on={proposalAb.side === 0} type="button" onclick={() => setProposalSide(0)}>A {tr("今", "Now")}</button>
                  {#each proposalAb.planIds as id, k (id)}
                    <button class="btn sm" class:on={proposalAb.side === k + 1} type="button" title={planName(id)} onclick={() => setProposalSide(k + 1)}
                      >{abLetter(k + 1)} {planName(id) ?? tr("案", "Proposal")}</button
                    >
                  {/each}
                </span>
                <button class="btn sm" type="button" title={tr("聴いている範囲の頭から聴き直す", "Replay from the start of the range")} onclick={restartProposalAb}
                  >⏮ {tr("頭から", "From start")}</button
                >
                <span class="spacer"></span>
                <button class="btn sm" type="button" onclick={endProposalAb}>{tr("聴き比べを終える", "End compare")}</button>
              </div>
              <div class="prow">
                <span class="plabel">{tr("範囲", "Range")}</span>
                {#each allChanged as si (si)}
                  <button
                    class="btn sm secchip"
                    class:on={proposalAb.section === si}
                    type="button"
                    title={tr(`この区間でまとめて聴き比べる(変わる案 ${changedBy.get(si)} つ)`, `Compare all in this section (${plural(changedBy.get(si) ?? 0, "proposal")} change)`)}
                    disabled={proposalAb.busy != null}
                    onclick={() => listenAll(si)}>{d.sections[si]?.name ?? tr(`区間 ${si + 1}`, `Section ${si + 1}`)}</button
                  >
                {/each}
                <span class="abnote"
                  >{proposalAb.section != null
                    ? tr(`「${d.sections[proposalAb.section]?.name}」を聴いています`, `Playing "${d.sections[proposalAb.section]?.name}"`)
                    : tr("選んだ範囲を聴いています", "Playing the selected range")}{#if info}{" "}· {info.lufs
                      .map((l, k) => `${k === 0 ? tr("今", "Now") : abLetter(k)} ${l?.toFixed(1) ?? "—"}`)
                      .join(" / ")} LUFS{tr("(音量はいちばん小さいものにそろえてあります)", " (levels matched to the quietest)")}{/if}</span
                >
              </div>
              <span class="pnote">{tr("切り替えても、同じ位置から続けて鳴ります(頭から聴くなら「頭から」)", "Switching keeps playing from the same position (use \"From start\" to replay)")}</span>
              {#if info}
                {#each proposalAb.planIds as id, k (id)}
                  {#if info.first_diff_secs[k + 1] === null}
                    <span class="pwarn"
                      >{abLetter(k + 1)}{tr(
                        `「${planName(id)}」はこの範囲では今と同じ音です(範囲の区間を変えてみてください)`,
                        ` "${planName(id)}" sounds the same as now in this range (try another section)`,
                      )}</span
                    >
                  {/if}
                {/each}
              {/if}
            {:else}
              <div class="prow">
                <button class="btn sm" type="button" disabled={proposalAb.busy != null} onclick={() => listenAll()}
                  >{proposalAb.busy === "all"
                    ? tr("用意しています…", "Preparing…")
                    : tr(`まとめて聴き比べる(今 + 案 ${allIds.length} つ)`, `Compare all (now + ${plural(allIds.length, "proposal")})`)}</button
                >
                <span class="pnote"
                  >{tr(
                    `A = 今、${allIds.map((id, k) => `${abLetter(k + 1)} = ${planName(id)}`).join("、")} を同じ範囲で切り替えて聴けます`,
                    `Switch between A = now, ${allIds.map((id, k) => `${abLetter(k + 1)} = ${planName(id)}`).join(", ")} over the same range`,
                  )}{listenable.length > allIds.length
                    ? tr(`(いっしょに聴けるのは ${MAX_PROPOSALS_AB} つまで)`, ` (up to ${MAX_PROPOSALS_AB} at once)`)
                    : ""}</span
                >
              </div>
            {/if}
          </div>
        {/if}
        {#each propList as pr (pr.plan_id)}
          {@const on = abSingle(pr.plan_id)}
          {@const ch = changesOf(pr.plan_id)}
          {@const letter = letterOf(pr.plan_id)}
          <div class="prop" class:on={on || letter != null}>
            <div class="prow">
              {#if letter}<span class="pletter" class:cur={proposalAb.side === proposalAb.planIds.indexOf(pr.plan_id) + 1}>{letter}</span>{/if}
              <span class="pname">{pr.name}</span>
              <span class="pbase"
                >{pr.derived_from
                  ? tr(`「${planName(pr.derived_from.id) ?? "元の計画"}」の案`, `Proposal for "${planName(pr.derived_from.id) ?? "the original plan"}"`)
                  : tr("新しい計画の案", "Proposal for a new plan")}{pr.edits
                  ? tr(` · 編集 ${pr.edits} 件`, ` · ${plural(pr.edits, "edit")}`)
                  : tr(" · 計画だけ", " · plan only")}</span
              >
            </div>
            {#if proposalWhy(pr.plan_id)}<div class="pwhy">{proposalWhy(pr.plan_id)}</div>{/if}
            <div class="prow">
              <span class="plabel">{tr("変わる所", "Changes")}</span>
              {#if ch?.stale}
                <span class="pwarn"
                  >{ch.why ??
                    tr(
                      "案を出した後で曲が変わったので、今の曲に当てられません(AI に作り直してもらってください)",
                      "The song changed after this proposal was made, so it can't be applied (ask the AI to redo it)",
                    )}</span
                >
              {:else if !pr.edits}
                <span class="pnote">{tr("音は変わりません(計画だけの案)", "No change to the sound (plan-only proposal)")}</span>
              {:else}
                {#each ch?.sections ?? [] as si (si)}
                  <button
                    class="btn sm secchip"
                    class:on={on && proposalAb.section === si}
                    type="button"
                    title={tr("この区間で聴き比べる", "Compare in this section")}
                    disabled={proposalAb.busy != null}
                    onclick={() => listen(pr.plan_id, si)}>{d.sections[si]?.name ?? tr(`区間 ${si + 1}`, `Section ${si + 1}`)}</button
                  >
                {:else}
                  {#if ch?.ranges?.length}<span class="pnote">{tr(`区間の外(${ch.ranges.length} か所)`, `Outside sections (${plural(ch.ranges.length, "place")})`)}</span>{/if}
                {/each}
                {#if ch?.whole}<span class="pnote">{tr("+ 音色・ミックスなど曲全体", "+ whole song (sounds, mix, etc.)")}</span>{/if}
                {#if !ch?.sections?.length && !ch?.ranges?.length && !ch?.whole}<span class="pnote"
                    >{ch?.silent_only
                      ? tr("違いは今鳴っていないトラック(ミュート中など)だけです", "Differences are only in tracks not currently heard (muted, etc.)")
                      : tr("音の違いは見つかりませんでした", "No audible difference found")}</span
                  >{/if}
              {/if}
            </div>
            <div class="prow">
              {#if on}
                <span class="abswitch" role="group" aria-label={tr("今と案を切り替える", "Switch between now and proposal")}>
                  <button class="btn sm" class:on={proposalAb.side === 0} type="button" onclick={() => setProposalSide(0)}>A {tr("今", "Now")}</button>
                  <button class="btn sm" class:on={proposalAb.side === 1} type="button" onclick={() => setProposalSide(1)}>B {tr("案", "Proposal")}</button>
                </span>
                <button class="btn sm" type="button" title={tr("聴いている範囲の頭から聴き直す", "Replay from the start of the range")} onclick={restartProposalAb}
                  >⏮ {tr("頭から", "From start")}</button
                >
                <span class="pnote">{tr("A / B を切り替えても、同じ位置から続けて鳴ります(頭から聴くなら「頭から」)", "Switching A / B keeps playing from the same position (use \"From start\" to replay)")}</span>
                <span class="abnote"
                  >{proposalAb.section != null
                    ? tr(`「${d.sections[proposalAb.section]?.name}」を聴いています`, `Playing "${d.sections[proposalAb.section]?.name}"`)
                    : tr("選んだ範囲を聴いています", "Playing the selected range")}{#if proposalAb.info}{" "}· {tr("今", "Now")}
                    {proposalAb.info.lufs[0]?.toFixed(1) ?? "—"} / {tr("案", "Proposal")}
                    {proposalAb.info.lufs[1]?.toFixed(1) ?? "—"} LUFS{tr("(音量はそろえてあります)", " (levels matched)")}{/if}</span
                >
                {@const fd = proposalAb.info?.first_diff_secs[1]}
                {#if proposalAb.info && fd === null}
                  <span class="pwarn"
                    >{tr("この範囲では今と案の音が同じです(変わる所の区間を押して聴いてください)", "Now and the proposal sound the same in this range (press a section under Changes)")}</span
                  >
                {:else if fd != null && fd > 1}
                  <span class="pnote">{tr(`違いは範囲の頭から ${fd.toFixed(1)} 秒あたりから`, `Differences start about ${fd.toFixed(1)} s into the range`)}</span>
                {/if}
                <span class="spacer"></span>
                <button class="btn sm" type="button" onclick={endProposalAb}>{tr("聴き比べを終える", "End compare")}</button>
              {:else}
                <button
                  class="btn sm"
                  type="button"
                  disabled={proposalAb.busy != null || !pr.edits || !!ch?.stale}
                  title={pr.edits
                    ? tr("今と案を、音量をそろえて切り替えて聴く(範囲は変わる所の区間)", "Switch between now and the proposal at matched levels (range: a changed section)")
                    : tr("音の変わらない案です(計画だけ)", "This proposal doesn't change the sound (plan only)")}
                  onclick={() => listen(pr.plan_id)}>{proposalAb.busy === pr.plan_id ? tr("用意しています…", "Preparing…") : tr("聴き比べる", "Compare")}</button
                >
                <span class="spacer"></span>
              {/if}
              <button
                class="btn sm primary"
                type="button"
                disabled={!!ch?.stale}
                title={tr(
                  "案の音を曲に当て、案の計画を今の計画にする。同じ元の計画・同じ頼みから出たほかの案と、いっしょに聴き比べていた案は捨てる",
                  "Apply the proposal's sound to the song and make its plan current. Other proposals from the same plan or request, and those being compared, are discarded",
                )}
                onclick={() => adoptProposal(pr.plan_id, pr.name)}>{tr("採用", "Adopt")}</button
              >
              <button class="btn sm" type="button" onclick={() => discardProposal(pr.plan_id, pr.name)}>{tr("捨てる", "Discard")}</button>
            </div>
          </div>
        {/each}
      </div>
    {/if}
    <div class="main">
      <div class="lanes" bind:this={lanesEl}>
        <!-- 曲全体 -->
        <section class="lane song" class:collapsed={folded.song} class:on={sel.kind === "song"}>
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="lanehead main-head" onclick={(e) => toggleFold("song", e)}>
            <button class="fold" type="button" aria-label={tr("折りたたむ・開く", "Collapse / expand")} aria-expanded={!folded.song}
              ><Icon name="chevron-down" size={14} /></button
            >
            <h2>{tr("曲全体", "Whole song")}</h2>
            <button class="btn sm pickall" class:on={isWhole("song")} type="button" onclick={(e) => pickWhole("song", e)}
              >{isWhole("song") ? tr("全体を選択中", "All selected") : tr("全体を選ぶ", "Select all")}</button
            >
            {#if folded.song}
              <span class="summary"
                >{[d.song?.genre, (d.song?.mood ?? []).join("・"), d.song?.key, tempo, tr(`${totalBars} 小節`, `${plural(totalBars, "bar")}`), d.song?.arc ? arcNames[d.song.arc] : null]
                  .filter(Boolean)
                  .join(" · ")}</span
              >
            {/if}
            {#if d.song_estimated}<span class="tag">{tr("推定", "Estimated")}</span>{/if}
          </div>
          {#if !folded.song}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="songgrid" onclick={() => pick({ kind: "song" })}>
              <label class="field"
                ><span>{tr("ジャンル", "Genre")}</span><input
                  type="text"
                  value={d.song?.genre ?? ""}
                  onchange={(e) => songField("genre", (e.currentTarget as HTMLInputElement).value.trim(), tr("ジャンルを変える", "Change genre"))}
                /></label
              >
              <div class="field">
                <span>{tr("雰囲気の言葉", "Mood words")}</span>
                <div class="tags">
                  {#each d.song?.mood ?? [] as m (m)}<span class="word"
                      >{m}<button
                        class="wx"
                        type="button"
                        aria-label={tr(`${m} を外す`, `Remove ${m}`)}
                        onclick={() => songField("mood", (d?.song?.mood ?? []).filter((x) => x !== m), tr(`雰囲気の言葉「${m}」を外す`, `Remove mood word "${m}"`))}
                        >×</button
                      ></span
                    >{/each}
                  <input
                    class="wordin"
                    type="text"
                    placeholder={tr("+ 足す", "+ Add")}
                    bind:value={moodInput}
                    onkeydown={(e) => {
                      if (e.key === "Enter" && !e.isComposing && moodInput.trim()) {
                        songField("mood", [...(d?.song?.mood ?? []), moodInput.trim()], tr(`雰囲気の言葉「${moodInput.trim()}」を足す`, `Add mood word "${moodInput.trim()}"`));
                        moodInput = "";
                      }
                    }}
                  />
                </div>
              </div>
              <label class="field"
                ><span>{tr("キー・旋法", "Key / mode")}</span><input
                  type="text"
                  value={d.song?.key ?? ""}
                  onchange={(e) => songField("key", (e.currentTarget as HTMLInputElement).value.trim(), tr("キーの狙いを変える", "Change target key"))}
                /></label
              >
              <label class="field"
                ><span>{tr("テンポ(タイムラインと同じ値)", "Tempo (same as timeline)")}</span>
                {#if tempoSingle}
                  <span class="row"
                    ><input
                      type="number"
                      min="20"
                      max="400"
                      step="0.5"
                      value={project.tempo_map?.[0]?.bpm ?? 120}
                      onchange={(e) => {
                        const v = Number((e.currentTarget as HTMLInputElement).value);
                        if (v >= 20 && v <= 400) setTempo(v);
                      }}
                    /> BPM</span
                  >
                {:else}<div>{tr("途中で変わる", "Varies")} <small>{tr("(タイムラインで直す)", "(edit on the timeline)")}</small></div>{/if}
              </label>
              <div class="field">
                <span>{tr("長さ", "Length")}</span>
                <div>{tr(`${totalBars} 小節`, `${plural(totalBars, "bar")}`)} <small>{tr("(タイムラインから)", "(from the timeline)")}</small></div>
              </div>
              <label class="field"
                ><span>{tr("盛り上がりの型", "Energy arc")}</span><select
                  value={d.song?.arc ?? ""}
                  onchange={(e) => songField("arc", (e.currentTarget as HTMLSelectElement).value, tr("盛り上がりの型を変える", "Change energy arc"))}
                  ><option value="">―</option>{#each Object.entries(arcNames) as [k, n] (k)}<option value={k}>{n}</option>{/each}</select
                ></label
              >
              {#each [["brightness", tr("明るさ", "Brightness"), d.song?.brightness, tr("暗い", "Dark"), tr("明るい", "Bright")], ["density", tr("音の密度", "Density"), d.song?.density, tr("まばら", "Sparse"), tr("ぎっしり", "Dense")], ["organic", tr("質感", "Texture"), d.song?.organic, tr("無機質", "Synthetic"), tr("有機的", "Organic")]] as [key, name, v, lo, hi] (key)}
                <label class="field">
                  <span>{name}{v == null ? tr("(未設定)", " (unset)") : `  ${v}`}</span>
                  <input
                    type="range"
                    min="0"
                    max="10"
                    step="1"
                    value={v ?? 5}
                    class:unset={v == null}
                    onchange={(e) => songField(String(key), Number((e.currentTarget as HTMLInputElement).value), tr(`${name}を変える`, `Change ${String(name).toLowerCase()}`))}
                  />
                  <div class="ends"><span>{lo}</span><span>{hi}</span></div>
                </label>
              {/each}
              <label class="field"
                ><span>{tr("音量の目標", "Target loudness")}</span><select
                  value={d.song?.loudness != null ? String(d.song.loudness) : ""}
                  onchange={(e) => {
                    const v = (e.currentTarget as HTMLSelectElement).value;
                    songField("loudness", v === "" ? null : Number(v), tr("音量の目標を変える", "Change target loudness"));
                  }}
                  ><option value="">―</option>{#each [-14, -11, -10, -9, -8, -7, -6] as l (l)}<option value={String(l)}
                      >{l} LUFS{l === -14 ? tr("(配信)", " (streaming)") : l <= -9 ? tr("(クラブ)", " (club)") : ""}</option
                    >{/each}</select
                ></label
              >
              <div class="field">
                <span>{tr("守ること", "Must-haves")}</span>
                <div class="tags">
                  {#each d.song?.musts ?? [] as m (m)}<span class="word must"
                      >{m}<button
                        class="wx"
                        type="button"
                        aria-label={tr(`${m} を外す`, `Remove ${m}`)}
                        onclick={() => songField("musts", (d?.song?.musts ?? []).filter((x) => x !== m), tr(`守ることを外す`, `Remove must-have`))}>×</button
                      ></span
                    >{/each}
                  <input
                    class="wordin"
                    type="text"
                    placeholder={tr("+ 足す", "+ Add")}
                    bind:value={mustInput}
                    onkeydown={(e) => {
                      if (e.key === "Enter" && !e.isComposing && mustInput.trim()) {
                        songField("musts", [...(d?.song?.musts ?? []), mustInput.trim()], tr("守ることを足す", "Add must-have"));
                        mustInput = "";
                      }
                    }}
                  />
                </div>
              </div>
              <label class="field"
                ><span>{tr("参考曲(、で区切る)", "References (comma-separated)")}</span><input
                  type="text"
                  value={(d.song?.refs ?? []).join("、")}
                  onchange={(e) =>
                    songField(
                      "refs",
                      (e.currentTarget as HTMLInputElement).value
                        .split(/[、,]/)
                        .map((x) => x.trim())
                        .filter(Boolean),
                      tr("参考曲を変える", "Change references"),
                    )}
                /></label
              >
              <label class="field wide"
                ><span>{tr("全体のメモ", "Overall note")}</span><input
                  type="text"
                  value={d.song?.note ?? ""}
                  onchange={(e) => songField("note", (e.currentTarget as HTMLInputElement).value.trim(), tr("全体のメモを変える", "Change overall note"))}
                /></label
              >
            </div>
          {/if}
        </section>

        <!-- 区間ごとの計画(時間の流れ)。縦のスクロールはこの枠の 1 つだけ -->
        <div class="timegroup" bind:this={group} onscroll={() => fitSlack(false)}>
          <div class="gtop">
            <div class="grouphead">
              {tr("区間ごとの計画", "Plan by section")}<span>{tr("左から右へ曲の時間の流れ", "Song time flows left to right")}</span>
              {#if missingPlans > 0 && (project.sections ?? []).length}
                <button
                  class="btn sm"
                  type="button"
                  title={tr(
                    "計画の無いパート(と曲全体)を、今の音から推定する。推定は未確認の扱いで、確かめて採用するまで AI は参考としてだけ使う",
                    "Estimate plans for parts without one (and the whole song) from the current audio. Estimates are unconfirmed; the AI uses them only as a reference until adopted",
                  )}
                  onclick={() => estimatePlans(project)}>{tr(`計画の無い所を推定(${missingPlans})`, `Estimate missing plans (${missingPlans})`)}</button
                >
              {/if}
              <div class="zoom" role="group" aria-label={tr("横の縮尺", "Horizontal zoom")}>
                <button class="btn sm" class:on={zoom == null} type="button" title={tr("曲全体が見える幅にする", "Fit the whole song")} onclick={() => setZoom(null)}
                  >{tr("全体", "Fit")}</button
                >
                <button class="btn sm icon" type="button" title={tr("縮小", "Zoom out")} aria-label={tr("縮小", "Zoom out")} onclick={() => setZoom(pxb / 1.5)}><Icon name="zoom-out" size={13} /></button>
                <button class="btn sm icon" type="button" title={tr("拡大", "Zoom in")} aria-label={tr("拡大", "Zoom in")} onclick={() => setZoom(pxb * 1.5)}><Icon name="zoom-in" size={13} /></button>
              </div>
            </div>
            <section class="lane">
              <div class="lbody">
                <div class="labels">
                  <div class="lab1 sechead" class:on={isWhole("sections")}>
                    {tr("区間", "Sections")}<button class="btn sm pickall mini" class:on={isWhole("sections")} type="button" onclick={(e) => pickWhole("sections", e)}
                      >{isWhole("sections") ? tr("全体を選択中", "All selected") : tr("全体を選ぶ", "Select all")}</button
                    >
                  </div>
                </div>
                <div class="hwin" onwheel={onWheel}>
                  <div class="hcontent ruler" style="width:{W}px;transform:translateX({-sx}px)">
                    {#each d.sections as s, i (i)}
                      <button
                        class="sec"
                        class:sel={sel.kind === "section" && sel.i === i}
                        style="left:{x0(i)}px;width:{xw(i)}px"
                        type="button"
                        title={s.name}
                        onclick={() => pick({ kind: "section", i })}
                        ><b>{s.name}</b> {s.bars}{#if hasMemo(`section:${s.id ?? s.name}`)}<span class="cm" title={tr("メモがあります", "Has notes")}>💬</span>{/if}</button
                      >
                    {/each}
                  </div>
                </div>
              </div>
            </section>
          </div>

          <!-- 盛り上がり -->
          <section class="lane" class:collapsed={folded.curve} class:on={isWhole("curve")}>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="lanehead main-head" onclick={(e) => toggleFold("curve", e)}>
              <button class="fold" type="button" aria-label={tr("折りたたむ・開く", "Collapse / expand")} aria-expanded={!folded.curve}
                ><Icon name="chevron-down" size={14} /></button
              >
              <h2>{tr("盛り上がり", "Energy")}</h2>
              <button class="btn sm pickall" class:on={isWhole("curve")} type="button" onclick={(e) => pickWhole("curve", e)}
                >{isWhole("curve") ? tr("全体を選択中", "All selected") : tr("全体を選ぶ", "Select all")}</button
              >
              {#if folded.curve}
                <span class="summary">{tr("区間ごとの盛り上がりの計画と実測(開くと見えます)", "Planned and measured energy per section (expand to view)")}</span>
              {:else}
                <button type="button" class="qmark" aria-label={tr("説明", "Help")} title={tr(
                    "区間ごとの盛り上がりの計画(実線)と実測(点線)。● は実測とずれた区間。\n点をドラッグで動かす。ダブルクリックで点を足し、右クリックで消す。\n区間の面を上下にドラッグでその区間ごと、Shift + ドラッグで全体を平行に動かす。境目の ⌇ を押すと「つなぐ / 段差」",
                    "Planned (solid) and measured (dotted) energy per section. ● marks sections off from the measurement.\nDrag a point to move it. Double-click to add a point, right-click to remove it.\nDrag a section's area up/down to shift that section; Shift + drag shifts all. Click ⌇ at a boundary to toggle Smooth / Step",
                  )}>?</button
                >
                <div class="tools">
                  <button class="btn sm" type="button" title={tr("盛り上がり全体を 0.5 上げる", "Raise the whole curve by 0.5")}
                    onclick={() => curveAll({ dv: 0.5 }, tr("盛り上がり全体を上げる", "Raise whole energy"))}>▲ {tr("全体", "All")}</button
                  >
                  <button class="btn sm" type="button" title={tr("盛り上がり全体を 0.5 下げる", "Lower the whole curve by 0.5")}
                    onclick={() => curveAll({ dv: -0.5 }, tr("盛り上がり全体を下げる", "Lower whole energy"))}>▼ {tr("全体", "All")}</button
                  >
                  <span class="sep"></span>
                  <button class="btn sm" type="button" title={tr("平均を中心に起伏を大きく", "Increase contrast around the average")}
                    onclick={() => curveAll({ scale: 1.2 }, tr("盛り上がりの起伏を大きく", "Increase energy contrast"))}>{tr("起伏 ＋", "Contrast ＋")}</button
                  >
                  <button class="btn sm" type="button" title={tr("平均を中心に起伏を小さく", "Decrease contrast around the average")}
                    onclick={() => curveAll({ scale: 1 / 1.2 }, tr("盛り上がりの起伏を小さく", "Decrease energy contrast"))}>{tr("起伏 −", "Contrast −")}</button
                  >
                </div>
                <div class="legend">
                  <label
                    ><input
                      type="checkbox"
                      bind:checked={showMeasured}
                      onchange={() => store("glaux.design.measured", showMeasured ? "1" : "0")}
                    /> {tr("実測を表示", "Show measured")}</label
                  >
                </div>
              {/if}
            </div>
            {#if !folded.curve}
              <div class="lbody">
                <div class="labels" style="height:{CH}px">
                  <div class="cvlegend" class:nomeasure={!showMeasured} style="top:{CT}px;height:{cy(0) - CT}px">
                    <div title={tr("計画した盛り上がり", "Planned energy")}><i class="ln"></i>{tr("計画", "Plan")}</div>
                    <div class="m" title={tr("今の音から測った盛り上がり", "Energy measured from the current audio")}><i class="ln dash"></i>{tr("実測", "Measured")}</div>
                    <hr />
                    <div title={tr("次の区間となめらかにつなぐ境目", "Boundary that blends smoothly into the next section")}><span class="mk">⌇</span>{tr("つなぐ", "Smooth")}</div>
                    <div title={tr("次の区間で急に変わる境目", "Boundary that changes abruptly into the next section")}><span class="mk warn">↕</span>{tr("段差", "Step")}</div>
                    <div class="m" title={tr("計画と実測が 1.5 以上ずれた区間(上の点)", "Sections where plan and measurement differ by 1.5 or more (dot at top)")}>
                      <span class="dot"></span>{tr("ずれ", "Off")}
                    </div>
                  </div>
                  <div class="axisline" style="top:{CT}px;height:{cy(0) - CT}px"></div>
                  {#each [0, 2, 4, 6, 8, 10] as v (v)}
                    <span class="axis" class:edge={v === 0 || v === 10} style="top:{cy(v)}px">{v}</span>
                  {/each}
                </div>
                <div class="hwin" onwheel={onWheel}>
                  <div class="hcontent" style="width:{W}px;transform:translateX({-sx}px)">
                    <svg width={W} height={CH} viewBox="0 0 {W} {CH}" role="img" aria-label={tr("盛り上がりの計画と実測", "Planned and measured energy")}>
                      <rect x="0" y="0" width={W} height={CT} class="outside" />
                      <rect x="0" y={cy(0)} width={W} height={CH - cy(0)} class="outside" />
                      {#each [0, 2, 4, 6, 8, 10] as v (v)}
                        <line x1="0" x2={W} y1={cy(v)} y2={cy(v)} class={v === 0 || v === 10 ? "edge" : "grid"} />
                      {/each}
                      <!-- 重ね順: 区間の面(当たり) → 線と点 → 境目の印。境目の点が次の区間の面に隠れないように -->
                      {#each d.sections as s, i (i)}
                        <line x1={x0(i)} x2={x0(i)} y1="0" y2={CH} class="secline" />
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <rect x={x0(i)} y="0" width={xw(i)} height={CH} fill="transparent" class="hit" onpointerdown={(e) => dragArea(e, i)} />
                      {/each}
                      {#each d.sections as s, i (i)}
                        {@const c = curveOf(i)}
                        {@const X = (t: number) => x0(i) + t * xw(i)}
                        {@const on = sel.kind === "section" && sel.i === i}
                        {#if showMeasured}
                          <line x1={x0(i) + 2} x2={x0(i) + xw(i) - 2} y1={cy(s.measured)} y2={cy(s.measured)} class="measured" />
                          {#if offSection(i)}
                            <circle cx={X(0.5)} cy="7" r="3.5" class="offdot"><title
                              >{tr(
                                `実測とずれている(計画 ${s.planned?.toFixed(1)} / 実測 ${s.measured.toFixed(1)})`,
                                `Off from measured (plan ${s.planned?.toFixed(1)} / measured ${s.measured.toFixed(1)})`,
                              )}</title
                            ></circle>
                          {/if}
                        {/if}
                        {#if c}
                          {@const path = c.map(([t, v], k) => `${k ? "L" : "M"}${X(t)},${cy(v)}`).join(" ")}
                          <path d="{path} L{X(1)},{cy(0)} L{X(0)},{cy(0)} Z" class="area" class:on pointer-events="none" />
                          <path d={path} class="plan" pointer-events="none" />
                        {/if}
                      {/each}
                      {#each d.sections as s, i (i)}
                        {@const c = curveOf(i)}
                        {@const X = (t: number) => x0(i) + t * xw(i)}
                        {@const on = sel.kind === "section" && sel.i === i}
                        {#if c}
                          {#each c as [t, v], k (k)}
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <circle
                              cx={X(t)}
                              cy={cy(v)}
                              r={on ? 5 : 3.5}
                              class="pt"
                              class:on
                              onpointerdown={(e) => dragPoint(e, i, k)}
                              oncontextmenu={(e) => removePoint(e, i, k)}
                            />
                          {/each}
                        {/if}
                      {/each}
                      {#each d.sections as s, i (i)}
                        {#if i < d.sections.length - 1}
                          {@const bx = x0(i + 1)}
                          <!-- svelte-ignore a11y_click_events_have_key_events -->
                          <!-- svelte-ignore a11y_no_static_element_interactions -->
                          <g class="join" class:step={s.join === "step"} onclick={() => toggleJoin(i)}>
                            <rect x={bx - 8} y={CH - 15} width="16" height="12" rx="3" />
                            <text x={bx} y={CH - 6}>{s.join === "step" ? "↕" : "⌇"}</text>
                            <title
                              >{s.join === "step"
                                ? tr("段差(急に変わる)。押すとつなぐ", "Step (abrupt). Click to smooth")
                                : tr("つなぐ(なめらか)。押すと段差に", "Smooth. Click to make a step")}</title
                            >
                          </g>
                        {/if}
                      {/each}
                    </svg>
                  </div>
                </div>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="vgrip"
                role="separator"
                aria-orientation="horizontal"
                title={tr("ドラッグで盛り上がりの段の高さを変える(ダブルクリックで元に戻す)", "Drag to resize the energy lane (double-click to reset)")}
                onpointerdown={(e) => grip("curve", e)}
                ondblclick={() => resetGrip("curve")}
              ></div>
            {/if}
          </section>

          <!-- パートごとの音域 -->
          <section class="lane" class:collapsed={folded.band} class:on={isWhole("band")}>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="lanehead main-head" onclick={(e) => toggleFold("band", e)}>
              <button class="fold" type="button" aria-label={tr("折りたたむ・開く", "Collapse / expand")} aria-expanded={!folded.band}
                ><Icon name="chevron-down" size={14} /></button
              >
              <h2>{tr("パートごとの音域", "Register by part")}</h2>
              <button class="btn sm pickall" class:on={isWhole("band")} type="button" onclick={(e) => pickWhole("band", e)}
                >{isWhole("band") ? tr("全体を選択中", "All selected") : tr("全体を選ぶ", "Select all")}</button
              >
              {#if folded.band}
                <span class="summary">{tr("選んでいるパート", "Selected part")}: {d.parts[selPart]?.name ?? "―"}</span>
              {:else}
                <button type="button" class="qmark" aria-label={tr("説明", "Help")} title={tr(
                    "左の一覧(か役割の段のパート名)でパートを選ぶと、そのパートの音域の帯(計画)が色付きで前に出て、動かせる。点線の枠は実際の音域(下 10%〜上 90%)。\n帯を上下にドラッグで移す、上下の端で幅(Shift で全区間まとめて)。下の端のつまみで段の高さ",
                    "Pick a part in the list on the left (or a part name in the role lane) to bring its planned register band forward in color and edit it. The dotted box is the actual register (10th to 90th percentile).\nDrag a band up/down to move it, drag its top/bottom edge to change its width (Shift for all sections). Use the grip at the bottom to resize the lane",
                  )}>?</button
                >
                <div class="tools">
                  <select bind:value={bandTarget} aria-label={tr("まとめて動かす対象", "Apply to")}
                    ><option value="one">{tr("選んだパート", "Selected part")}</option><option value="all">{tr("全パート", "All parts")}</option></select
                  >
                  <button class="btn sm" type="button" onclick={() => bandBulk(1, 0)}>▲ {tr("半音", "Semi")}</button>
                  <button class="btn sm" type="button" onclick={() => bandBulk(-1, 0)}>▼ {tr("半音", "Semi")}</button>
                  <button class="btn sm" type="button" onclick={() => bandBulk(12, 0)}>▲ 8va</button>
                  <button class="btn sm" type="button" onclick={() => bandBulk(-12, 0)}>▼ 8va</button>
                  <span class="sep"></span>
                  <button class="btn sm" type="button" onclick={() => bandBulk(0, 2)}>{tr("幅 ＋", "Width ＋")}</button>
                  <button class="btn sm" type="button" onclick={() => bandBulk(0, -2)}>{tr("幅 −", "Width −")}</button>
                  <span class="sep"></span>
                  <button class="btn sm" type="button" title={tr("選んだパートの実際の音域を、計画の帯にする", "Set the selected part's planned band from its actual register")}
                    onclick={bandFromReal}>{tr("実際を計画に", "Match plan to actual")}</button
                  >
                </div>
                <div class="legend">
                  <label title={tr("強いパートどうしの計画の音域の重なりを斜線で(参考。警告ではない)", "Hatch where strong parts' planned registers overlap (for reference, not a warning)")}
                    ><input type="checkbox" bind:checked={showOverlap} /> {tr("重なり", "Overlap")}</label
                  >
                </div>
              {/if}
            </div>
            {#if !folded.band}
              <div class="bandlane">
                <div class="labels partlist" style="max-height:{BH}px">
                  {#each d.parts as p, pi (p.track_id)}
                    <button
                      class="partbtn"
                      class:on={pi === selPart}
                      type="button"
                      onclick={() => pick({ kind: "part", p: pi })}
                      ><span class="chip" style="background:{colorOf(pi)}"></span>{p.name}</button
                    >
                  {:else}
                    <div class="emptyrow">{tr("MIDI のトラックがありません", "No MIDI tracks")}</div>
                  {/each}
                </div>
                <div class="hwin" onwheel={onWheel}>
                  <div class="octaves" style="height:{BH}px">
                    {#each [24, 36, 48, 60, 72, 84, 96] as p (p)}
                      <span class="octave" style="top:{BY(p)}px">C{p / 12 - 1}</span>
                    {/each}
                  </div>
                  <div class="hcontent" style="width:{W}px;transform:translateX({-sx}px)">
                    <svg width={W} height={BH} viewBox="0 0 {W} {BH}" role="img" aria-label={tr("パートごとの音域", "Register by part")}>
                      <defs>
                        <pattern id="glaux-hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                          <line x1="0" y1="0" x2="0" y2="6" class="hatch" />
                        </pattern>
                      </defs>
                      {#each [24, 36, 48, 60, 72, 84, 96] as p (p)}
                        <line x1="0" x2={W} y1={BY(p)} y2={BY(p)} class="grid" />
                      {/each}
                      {#each d.sections as _, i (i)}
                        <line x1={x0(i)} x2={x0(i)} y1="0" y2={BH} class="secline" />
                      {/each}
                      <!-- 選んでいないパート: 計画の帯の薄い輪郭だけ -->
                      {#each d.parts as p, pi (p.track_id)}
                        {#if pi !== selPart}
                          {#each p.cells as c, i (i)}
                            {#if c.planned_register && (c.planned ?? 0) > 0}
                              <rect
                                x={x0(i) + 2}
                                width={Math.max(0, xw(i) - 4)}
                                y={BY(c.planned_register[1])}
                                height={BY(c.planned_register[0]) - BY(c.planned_register[1])}
                                rx="2"
                                class="other"
                              />
                            {/if}
                          {/each}
                        {/if}
                      {/each}
                      {#each overlaps as o, k (k)}
                        <rect x={x0(o.i) + 3} width={Math.max(0, xw(o.i) - 6)} y={BY(o.hi)} height={BY(o.lo) - BY(o.hi)} fill="url(#glaux-hatch)" />
                      {/each}
                      <!-- 選んだパート: 計画の帯を色付きで、実際の音域を点線の枠で -->
                      {#if d.parts[selPart]}
                        {@const p = d.parts[selPart]}
                        {#each p.cells as c, i (i)}
                          {@const on = sel.kind === "cell" && sel.p === selPart && sel.i === i}
                          {@const r = regOf(i)}
                          {#if r && (lvOf(selPart, i) ?? c.planned ?? 0) > 0}
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <rect
                              x={x0(i) + 2}
                              width={Math.max(0, xw(i) - 4)}
                              y={BY(r[1])}
                              height={BY(r[0]) - BY(r[1])}
                              rx="2"
                              class="mine"
                              class:on
                              style="--c:{colorOf(selPart)}"
                              onpointerdown={(e) => dragBand(e, i, "move")}
                            />
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <rect x={x0(i) + 2} width={Math.max(0, xw(i) - 4)} y={BY(r[1]) - 3} height="6" class="edge-hit" onpointerdown={(e) => dragBand(e, i, "top")} />
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <rect x={x0(i) + 2} width={Math.max(0, xw(i) - 4)} y={BY(r[0]) - 3} height="6" class="edge-hit" onpointerdown={(e) => dragBand(e, i, "bottom")} />
                            <text x={x0(i) + 6} y={BY(r[1]) + 13} class="bandlabel" style="fill:{colorOf(selPart)}"
                              >{noteName(r[0])}〜{noteName(r[1])}</text
                            >
                          {/if}
                          {#if c.register}
                            <rect
                              x={x0(i) + xw(i) * 0.3}
                              width={Math.max(2, xw(i) * 0.4)}
                              y={BY(c.register[1]) - 1}
                              height={Math.max(2, BY(c.register[0]) - BY(c.register[1]) + 2)}
                              class="real"
                              pointer-events="none"
                            ><title>{tr("実際の音域", "Actual register")} {noteName(c.register[0])}〜{noteName(c.register[1])}</title></rect>
                          {/if}
                        {/each}
                      {/if}
                    </svg>
                  </div>
                </div>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="vgrip"
                role="separator"
                aria-orientation="horizontal"
                title={tr("ドラッグで音域の段の高さ(音高の縦の大きさ)を変える(ダブルクリックで元に戻す)", "Drag to resize the register lane (pitch scale; double-click to reset)")}
                onpointerdown={(e) => grip("band", e)}
                ondblclick={() => resetGrip("band")}
              ></div>
            {/if}
          </section>

          <!-- パートの役割 -->
          <section class="lane" class:collapsed={folded.table} class:on={isWhole("table")} class:compact={rowH < 34}>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="lanehead main-head" onclick={(e) => toggleFold("table", e)}>
              <button class="fold" type="button" aria-label={tr("折りたたむ・開く", "Collapse / expand")} aria-expanded={!folded.table}
                ><Icon name="chevron-down" size={14} /></button
              >
              <h2>{tr("パートの役割", "Part roles")}</h2>
              <button class="btn sm pickall" class:on={isWhole("table")} type="button" onclick={(e) => pickWhole("table", e)}
                >{isWhole("table") ? tr("全体を選択中", "All selected") : tr("全体を選ぶ", "Select all")}</button
              >
              {#if folded.table}
                <span class="summary">{tr(`${d.parts.length} パート × ${d.sections.length} 区間`, `${plural(d.parts.length, "part")} × ${plural(d.sections.length, "section")}`)}</span>
              {:else}
                <button type="button" class="qmark" aria-label={tr("説明", "Help")} title={tr(
                    "マスは区間ごとの存在の段階(計画)。計画の無いマスは、測った段階を薄く出す。\nマスを押すと選ぶ。役割を変えるのはマスの ▾(選ぶと出る)か数字キー 0〜5。押したまま横へドラッグで同じ段階を塗る。\n● は計画と実際がずれたマス。🔒 は固定(AI が作り直さない。固定のマスは塗らない)",
                    "Each cell is the planned presence per section. Cells without a plan show the measured level faintly.\nClick a cell to select it. Change the role with the cell's ▾ (shown when selected) or number keys 0–5. Hold and drag sideways to paint the same level.\n● marks cells where plan and actual differ. 🔒 means locked (the AI won't redo it; locked cells aren't painted)",
                  )}>?</button
                >
                <span class="hint">{tr("🔒 固定 · 💬 メモ · ● 実際とずれ · 薄い字 = 計画なし(実測)", "🔒 Locked · 💬 Note · ● Off from actual · Faint = no plan (measured)")}</span>
              {/if}
            </div>
            {#if !folded.table}
              <div class="vscroll" style="height:{tableBody}px">
                <div class="lbody">
                  <div class="labels">
                    {#each d.parts as p, pi (p.track_id)}
                      <button
                        class="mlabel"
                        class:on={pi === selPart}
                        style="height:{rowH}px"
                        type="button"
                        onclick={() => pick({ kind: "part", p: pi })}
                      >
                        <span class="trackname"><span class="chip" style="background:{colorOf(pi)}"></span>{p.name}</span>
                        <small
                          >{p.function ? (functionNames[p.function] ?? p.function) : p.plan_id ? "" : tr("計画なし", "No plan")}{p.estimated
                            ? tr("(推定)", " (estimated)")
                            : ""}</small
                        >
                      </button>
                    {:else}
                      <div class="emptyrow">{tr("MIDI のトラックがありません", "No MIDI tracks")}</div>
                    {/each}
                  </div>
                  <div class="hwin" onwheel={onWheel}>
                    <div class="hcontent" style="width:{W}px;transform:translateX({-sx}px)">
                      {#each d.parts as p, pi (p.track_id)}
                        <div class="mrow" style="height:{rowH}px">
                          {#each p.cells as c, i (i)}
                            {@const planned = lvOf(pi, i)}
                            {@const lv = planned ?? c.measured}
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <div
                              class="cell"
                              class:sel={sel.kind === "cell" && sel.p === pi && sel.i === i}
                              class:diff={cellOff(c)}
                              class:unplanned={planned == null}
                              style="left:{x0(i)}px;width:{xw(i)}px;height:{rowH - 1}px"
                              role="button"
                              tabindex="0"
                              aria-label="{p.name} / {d.sections[i]?.name}: {planned != null
                                ? presenceNames[planned]
                                : tr(`計画なし(実測 ${presenceNames[c.measured]})`, `No plan (measured: ${presenceNames[c.measured]})`)}"
                              onclick={() => pick({ kind: "cell", p: pi, i })}
                              onkeydown={(e) => {
                                if (e.key === "Enter" || e.key === " ") {
                                  e.preventDefault();
                                  togglePop(e as unknown as MouseEvent, pi, i);
                                }
                              }}
                              onpointerdown={(e) => paintStart(e, pi, i)}
                              onpointerenter={(e) => paintEnter(e, pi, i)}
                            >
                              {#if c.locked}<span class="lock" title={tr("固定(AI が作り直さない)", "Locked (the AI won't redo it)")}>🔒</span>{/if}
                              {#if hasMemo(`cell:${p.track_id}:${d.sections[i]?.id ?? d.sections[i]?.name}`)}<span class="cm" title={tr("メモがあります", "Has notes")}>💬</span>{/if}
                              <span class="steps">
                                {#each [1, 2, 3, 4, 5] as n (n)}<i class:on={n <= lv}></i>{/each}
                              </span>
                              <span class="lab"
                                >{planned != null
                                  ? presenceNames[planned]
                                  : c.notes > 0
                                    ? tr(`実測 ${presenceNames[c.measured]}`, `Measured: ${presenceNames[c.measured]}`)
                                    : "―"}</span
                              >
                              <button
                                class="cellbtn"
                                class:open={pop?.p === pi && pop?.i === i}
                                type="button"
                                title={pop?.p === pi && pop?.i === i
                                  ? tr("選択肢を閉じる", "Close options")
                                  : tr("役割を変える(数字キー 0〜5 でも)", "Change role (or number keys 0–5)")}
                                aria-label={tr(`${p.name} / ${d.sections[i]?.name} の役割を変える`, `Change role of ${p.name} / ${d.sections[i]?.name}`)}
                                onclick={(e) => togglePop(e, pi, i)}><Icon name="chevron-down" size={12} /></button
                              >
                            </div>
                          {/each}
                        </div>
                      {/each}
                    </div>
                  </div>
                </div>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="vgrip"
                role="separator"
                aria-orientation="horizontal"
                title={tr("ドラッグで役割の段の高さ(行の高さ)を変える(ダブルクリックで元に戻す)", "Drag to resize the role lane (row height; double-click to reset)")}
                onpointerdown={(e) => grip("table", e)}
                ondblclick={() => resetGrip("table")}
              ></div>
            {/if}
          </section>
          <div style="height:{slack}px;flex:none" aria-hidden="true"></div>
          <div class="hbar" bind:this={hbar} onscroll={onHbar} aria-label={tr("横スクロール", "Horizontal scroll")}>
            <div style="width:{W}px;height:1px"></div>
          </div>
        </div>
      </div>

      <!-- 詳しい情報 -->
      <aside class="panel" aria-live="polite">
        {#if sel.kind === "none"}
          <h3>{tr("選んでいません", "Nothing selected")}</h3>
          <p class="empty">
            {tr(
              "区間・パート・マスをクリックするか、段の見出しの「全体を選ぶ」で選ぶと、ここに詳しく出ます。何も選んでいないときの AI への指示は、曲全体が対象になります。",
              "Click a section, part or cell, or use \"Select all\" in a lane header, to see details here. With nothing selected, instructions to the AI apply to the whole song.",
            )}
          </p>
          {#if d.deviations.length}
            <h4>{tr(`計画と実際のずれ(${d.deviations.length})`, `Plan vs. actual (${d.deviations.length})`)}</h4>
            {#each d.deviations.slice(0, 8) as x, k (k)}
              <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
            {/each}
          {/if}
        {:else}
          <h3>{designTargetLabel(d, sel)}</h3>
          {#if sel.kind === "song"}
            <dl class="kv">
              <dt>{tr("ジャンル", "Genre")}</dt><dd>{d.song?.genre ?? "―"}</dd>
              <dt>{tr("盛り上がりの型", "Energy arc")}</dt><dd>{d.song?.arc ? arcNames[d.song.arc] : "―"}</dd>
              <dt>{tr("テンポ", "Tempo")}</dt><dd>{tempo}</dd>
              <dt>{tr("守ること", "Must-haves")}</dt><dd>{(d.song?.musts ?? []).join(tr("、", ", ")) || "―"}</dd>
              <dt>{tr("状態", "Status")}</dt>
              <dd>{d.song ? (d.song_estimated ? tr("推定(未確認)", "Estimated (unconfirmed)") : tr("採用済み", "Adopted")) : tr("計画なし", "No plan")}</dd>
            </dl>
            <div class="actions">
              <button
                class="btn sm"
                type="button"
                disabled={!d.song?.arc}
                title={tr(
                  "盛り上がりの型に合わせて、区間ごとの盛り上がりの高さを動かす(区間の中の形は残す。取り消せる)",
                  "Shift each section's energy level to fit the arc (shapes within sections are kept; can be undone)",
                )}
                onclick={applyArc}>{tr("型を盛り上がりに当てる", "Apply arc to energy")}</button
              >
            </div>
          {:else if sel.kind === "lane"}
            {#if sel.lane === "curve"}
              {@const hi = d.sections.reduce((a, s, i) => (s.measured > (d.sections[a]?.measured ?? -1) ? i : a), 0)}
              <dl class="kv">
                <dt>{tr("区間", "Sections")}</dt><dd>{d.sections.length}</dd>
                <dt>{tr("いちばん高い(実測)", "Highest (measured)")}</dt><dd>{d.sections[hi]?.name}({d.sections[hi]?.measured.toFixed(1)})</dd>
                <dt>{tr("段差", "Steps")}</dt>
                <dd>
                  {d.sections
                    .filter((s, i) => s.join === "step" && i < d.sections.length - 1)
                    .map((s) => tr(`${s.name} → 次`, `${s.name} → next`))
                    .join(tr("、", ", ")) || tr("なし", "None")}
                </dd>
                <dt>{tr("盛り上がりの型", "Energy arc")}</dt><dd>{d.song?.arc ? arcNames[d.song.arc] : "―"}</dd>
              </dl>
              {#each d.sections.filter((_, i) => offSection(i)) as s (s.name)}
                <div class="finding warn">
                  <span class="dot"></span><span
                    >{tr(
                      `「${s.name}」が計画とずれている(計画 ${s.planned?.toFixed(1)} / 実測 ${s.measured.toFixed(1)})`,
                      `"${s.name}" is off from the plan (plan ${s.planned?.toFixed(1)} / measured ${s.measured.toFixed(1)})`,
                    )}</span
                  >
                </div>
              {/each}
            {:else if sel.lane === "band"}
              <dl class="kv">
                <dt>{tr("パート", "Parts")}</dt><dd>{d.parts.length}</dd>
                <dt>{tr("計画のあるパート", "Parts with a plan")}</dt><dd>{d.parts.filter((p) => p.plan_id).length}</dd>
              </dl>
              {#each devOf((x) => !!x.track && x.what.includes("音域")) as x, k (k)}
                <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
              {/each}
            {:else if sel.lane === "table"}
              <dl class="kv">
                <dt>{tr("大きさ", "Size")}</dt><dd>{tr(`${d.parts.length} パート × ${d.sections.length} 区間`, `${plural(d.parts.length, "part")} × ${plural(d.sections.length, "section")}`)}</dd>
                <dt>{tr("固定", "Locked")}</dt><dd>{d.parts.reduce((a, p) => a + p.cells.filter((c) => c.locked).length, 0)} {tr("マス", "cells")}</dd>
                <dt>{tr("ずれ", "Off")}</dt><dd>{d.parts.reduce((a, p) => a + p.cells.filter(cellOff).length, 0)} {tr("マス", "cells")}</dd>
              </dl>
            {:else}
              <dl class="kv">
                <dt>{tr("並び", "Order")}</dt><dd>{d.sections.map((s) => `${s.name}(${s.bars})`).join(" → ")}</dd>
                <dt>{tr("長さ", "Length")}</dt><dd>{tr(`${totalBars} 小節`, `${plural(totalBars, "bar")}`)}</dd>
              </dl>
            {/if}
          {:else if sel.kind === "section"}
            {@const s = d.sections[sel.i]}
            {#if s}
              <dl class="kv">
                <dt>{tr("位置", "Position")}</dt><dd>{tr(`${s.start_bar} 小節目から ${s.bars} 小節`, `${plural(s.bars, "bar")} from bar ${s.start_bar}`)}</dd>
                <dt>{tr("盛り上がり", "Energy")}</dt>
                <dd>{tr("計画", "Plan")} {s.planned != null ? s.planned.toFixed(1) : "―"} / {tr("実測", "Measured")} {s.measured.toFixed(1)}</dd>
                <dt>{tr("次との境目", "Next boundary")}</dt><dd>{sel.i < d.sections.length - 1 ? (s.join === "step" ? tr("段差", "Step") : tr("つなぐ", "Smooth")) : "―"}</dd>
                <dt>{tr("意図", "Intent")}</dt><dd>{project.sections?.find((m) => m.name === s.name)?.note ?? "―"}</dd>
              </dl>
              {#each devOf((x) => x.section === sectionKey(sel.i)) as x, k (k)}
                <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
              {/each}
            {/if}
          {:else}
            {@const p = d.parts[sel.p]}
            {#if p}
              {#if sel.kind === "cell"}
                {@const c = p.cells[sel.i]}
                <dl class="kv">
                  <dt>{tr("働き", "Function")}</dt><dd>{c.function ? functionNames[c.function] : p.function ? functionNames[p.function] : "―"}</dd>
                  <dt>{tr("存在(計画)", "Presence (plan)")}</dt><dd>{c.planned != null ? `${c.planned} ${presenceNames[c.planned]}` : tr("計画なし", "No plan")}</dd>
                  <dt>{tr("存在(実測)", "Presence (measured)")}</dt><dd>{c.measured} {presenceNames[c.measured]}{tr(`(${c.notes} 音)`, ` (${plural(c.notes, "note")})`)}</dd>
                  <dt>{tr("音域(計画)", "Register (plan)")}</dt><dd>{c.planned_register ? `${noteName(c.planned_register[0])}〜${noteName(c.planned_register[1])}` : "―"}</dd>
                  <dt>{tr("音域(実際)", "Register (actual)")}</dt><dd>{c.register ? `${noteName(c.register[0])}〜${noteName(c.register[1])}` : "―"}</dd>
                  <dt>{tr("密度", "Density")}</dt><dd>{c.density == null ? "" : Math.round(c.density * 100) / 100}</dd>
                  <dt>{tr("固定", "Locked")}</dt><dd>{c.locked ? tr("🔒 この区間は固定", "🔒 This section is locked") : tr("なし", "No")}</dd>
                </dl>
                {#each devOf((x) => x.track === p.name && x.section === c.section) as x, k (k)}
                  <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
                {/each}
              {:else}
                <dl class="kv">
                  <dt>{tr("働き", "Function")}</dt>
                  <dd>
                    <select
                      value={p.function ?? ""}
                      aria-label={tr("このパートの既定の働き", "Default function of this part")}
                      onchange={(e) => setPartFunction(sel.kind === "part" ? sel.p : 0, (e.currentTarget as HTMLSelectElement).value)}
                      ><option value="">―</option>{#each Object.entries(functionNames) as [k, n] (k)}<option value={k}>{n}</option>{/each}</select
                    >
                  </dd>
                  <dt>{tr("計画", "Plan")}</dt>
                  <dd>
                    {p.plan_id
                      ? p.estimated
                        ? tr("推定(未確認)", "Estimated (unconfirmed)")
                        : tr("採用済み", "Adopted")
                      : tr("なし(直すと作られる)", "None (created when you edit)")}
                  </dd>
                  <dt>{tr("鳴っている区間", "Sections playing")}</dt><dd>{p.cells.filter((c) => c.notes > 0).length} / {p.cells.length}</dd>
                </dl>
                {#each devOf((x) => x.track === p.name) as x, k (k)}
                  <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
                {/each}
              {/if}
              {#if partClips(p.name).length}
                <h4>{tr("クリップの状態", "Clip status")}</h4>
                {#each partClips(p.name) as c (c.clip_id)}
                  <div class="clipstate">
                    {#if c.plan === "ahead"}<span class="badge ahead">{tr("計画が先に進んだ", "Plan is ahead")}</span>{:else if c.plan === "in_sync"}<span class="badge"
                        >{tr("計画どおり", "Matches plan")}</span
                      >{/if}
                    {#if c.edited_bars?.length}<span class="badge edited"
                        >{tr(
                          `手で直した: ${c.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}〜${b}`)).join("・")} 小節`,
                          `Hand-edited bars: ${c.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}–${b}`)).join(", ")}`,
                        )}</span
                      >{/if}
                    {#if c.locked_notes}<span class="badge">🔒 {tr(`${c.locked_notes} 音`, `${plural(c.locked_notes, "note")}`)}</span>{/if}
                  </div>
                {/each}
              {/if}
            {/if}
          {/if}
          <!-- メモ: 所に付く人の言葉(AI はその所を作る・直すときに読む)。AI とのやりとりは下のチャット -->
          <div class="memo-box">
            <h4>{tr(`メモ(${memos.length})`, `Notes (${memos.length})`)}</h4>
            {#if memos.length}
              <div class="thread">
                {#each memos as m, k (k)}
                  <div class="memo">
                    <div class="who">
                      {tr("あなた", "You")}{m.when
                        ? ` · ${new Date(m.when).toLocaleString(isEn() ? "en-US" : "ja-JP", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })}`
                        : ""}
                    </div>
                    {m.text}
                  </div>
                {/each}
              </div>
            {:else}
              <p class="empty">{tr("この所のメモはまだありません。AI は作る・直すときにここを読みます", "No notes here yet. The AI reads these when creating or editing")}</p>
            {/if}
            <textarea bind:value={memoText} rows="2" placeholder={tr("例: ここはもっと沈んだ感じに / ハットは抜きたい", "e.g. Make this feel darker / drop the hi-hat")}></textarea>
            <div class="actions">
              <button class="btn sm" type="button" disabled={!memoText.trim()} onclick={saveMemo}>{tr("メモを残す", "Add note")}</button>
              <button class="btn sm" type="button" onclick={askAi} title={tr("下のチャットの入力欄へ(選んだ所が対象として添わる)", "Go to the chat input below (the selection is attached as the target)")}
                >{tr("この所について AI に頼む ↓", "Ask AI about this ↓")}</button
              >
            </div>
          </div>
          <div class="actions two">
            <button
              class="btn sm"
              type="button"
              title={tr("計画の今の版から作り直すよう AI に頼む(手で直した小節と固定の音は残す)", "Ask the AI to regenerate from the current plan (keeps hand-edited bars and locked notes)")}
              onclick={() => remake(false)}>{tr("この版から作り直す", "Regenerate from plan")}</button
            >
            <button
              class="btn sm"
              class:danger={overwriteArmed}
              type="button"
              title={tr(
                "手で直した所も含めて作り直すよう AI に頼む(固定の音は残る)。押すと確かめの表示になり、もう一度で送る",
                "Ask the AI to regenerate including hand edits (locked notes are kept). Click once to arm, again to send",
              )}
              onclick={() => remake(true)}>{overwriteArmed ? tr("もう一度押すと送ります", "Click again to send") : tr("手直しも含めて上書き", "Overwrite hand edits")}</button
            >
          </div>
        {/if}
      </aside>
    </div>
    {#if pop && d.parts[pop.p]}
      {@const p = d.parts[pop.p]}
      {@const c = p.cells[pop.i]}
      {@const cur = lvOf(pop.p, pop.i)}
      <div class="pop" style="left:{pop.x}px;{pop.up ? `bottom:${window.innerHeight - pop.y}px` : `top:${pop.y}px`}" role="dialog" aria-label={tr("役割を変える", "Change role")}>
        <h4>{p.name} / {d.sections[pop.i]?.name}</h4>
        {#each presenceNames as name, n (n)}
          <button
            class="opt"
            class:on={cur === n}
            type="button"
            onclick={() => {
              const t = pop;
              pop = null;
              if (t) setPresence(t.p, [t.i], n);
            }}
          >
            <span class="steps">{#each [1, 2, 3, 4, 5] as k (k)}<i class:on={k <= n}></i>{/each}</span>
            <span>{n} {name}</span>
          </button>
        {/each}
        <label class="row"
          >{tr("働き", "Function")} <select
            value={c.function ?? p.function ?? ""}
            onchange={(e) => {
              const t = pop;
              pop = null;
              if (t) setCellProp(t.p, t.i, "function", (e.currentTarget as HTMLSelectElement).value);
            }}
            ><option value="">―</option>{#each Object.entries(functionNames) as [k, n] (k)}<option value={k}>{n}</option>{/each}</select
          ></label
        >
        <label class="row"
          ><input
            type="checkbox"
            checked={!!c.locked}
            onchange={(e) => {
              const t = pop;
              pop = null;
              if (t) setCellProp(t.p, t.i, "locked", (e.currentTarget as HTMLInputElement).checked);
            }}
          /> 🔒 {tr("固定する", "Lock")}</label
        >
        <p class="help">
          {tr(
            "固定した所は、AI が作り直すとき(「手直しも含めて上書き」でも)変えません。あなたはいつでも変えられます。",
            "Locked parts are not changed when the AI regenerates (even with \"Overwrite hand edits\"). You can still change them anytime.",
          )}
        </p>
      </div>
    {/if}
  {/if}
</div>

<style>
  .design {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-2);
    background: var(--bg);
    font-size: var(--fs-md);
  }
  .design.dragging,
  .design.dragging :global(*) {
    user-select: none !important;
  }
  .empty-all {
    margin: auto;
    color: var(--text-dim);
  }
  .estimate {
    flex: none;
    display: flex;
    gap: var(--sp-3);
    align-items: center;
    flex-wrap: wrap;
    border: 1px dashed var(--border-strong);
    border-radius: var(--r-md);
    padding: 6px 12px;
    background: var(--bg-panel);
    font-size: var(--fs-sm);
  }
  .estimate span {
    color: var(--text-dim);
  }
  .main {
    flex: 1 1 0;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 300px;
    gap: var(--sp-2);
  }
  .lanes {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    user-select: none;
  }
  .lane {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden;
    min-width: 0;
    flex: none;
    display: flex;
    flex-direction: column;
  }
  .lane.on {
    border-color: var(--accent-dim);
  }
  .lane.on > .main-head h2 {
    color: var(--accent);
  }
  .lanehead {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 10px;
    background: var(--bg-lane);
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .main-head {
    cursor: pointer;
  }
  .main-head:hover {
    background: var(--bg-lane-alt);
  }
  .lane.collapsed > .main-head {
    border-bottom: 0;
  }
  .song > .main-head {
    background: var(--bg-raised);
  }
  .lanehead h2 {
    margin: 0;
    font-size: var(--fs-md);
    font-weight: 600;
    white-space: nowrap;
  }
  .fold {
    background: transparent;
    border: 0;
    color: var(--text);
    width: 24px;
    height: 24px;
    margin-left: -4px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--r-sm);
    flex: none;
  }
  .fold :global(svg) {
    transition: transform 0.12s;
  }
  .lane.collapsed .fold :global(svg) {
    transform: rotate(-90deg);
  }
  .pickall {
    border-radius: 10px;
    color: var(--text-dim);
    background: transparent;
    padding: 0 8px;
    font-size: var(--fs-xs);
    flex: none;
  }
  .pickall.on {
    color: var(--accent);
    border-color: var(--accent-dim);
  }
  .pickall.mini {
    padding: 0 6px;
  }
  .summary,
  .hint {
    color: var(--text-dim);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .summary {
    flex: 1;
  }
  .tag {
    font-size: var(--fs-xs);
    border: 1px dashed var(--border-strong);
    border-radius: 9px;
    padding: 0 6px;
    color: var(--text-dim);
  }
  .qmark {
    background: transparent;
    padding: 0;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1px solid var(--border-strong);
    color: var(--text-dim);
    font-size: var(--fs-xs);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: help;
    flex: none;
  }
  .legend {
    margin-left: auto;
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .legend label {
    display: flex;
    gap: 4px;
    align-items: center;
    cursor: pointer;
  }
  /* 曲全体 */
  .songgrid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    gap: 8px 16px;
    padding: 10px 12px 12px;
    cursor: pointer;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .field > span {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .field small {
    color: var(--text-faint);
  }
  .field.wide {
    grid-column: 1 / -1;
  }
  .hint-row {
    color: var(--text-dim);
    font-size: var(--fs-sm);
  }
  .tags {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .word {
    font-size: var(--fs-xs);
    padding: 1px 8px;
    border-radius: 10px;
    border: 1px solid var(--border-strong);
    background: var(--bg-raised);
  }
  .word.must {
    border-color: var(--accent-dim);
  }
  .ends {
    display: flex;
    justify-content: space-between;
    color: var(--text-faint);
    font-size: 10px;
  }
  /* 区間ごとの計画の枠 */
  .timegroup {
    flex: 1 1 0;
    min-height: 200px;
    overflow-y: auto;
    overscroll-behavior: contain;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--bg-inset);
    min-width: 0;
  }
  .gtop {
    position: sticky;
    top: 0;
    z-index: 3;
    background: var(--bg-inset);
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 4px;
    flex: none;
  }
  .grouphead {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 0 4px;
    font-size: var(--fs-xs);
    font-weight: 600;
    color: var(--text-dim);
  }
  .grouphead span {
    font-weight: 400;
    color: var(--text-faint);
  }
  .zoom {
    margin-left: auto;
    display: flex;
    gap: 2px;
  }
  .zoom .btn {
    font-weight: 400;
  }
  .lbody {
    display: grid;
    grid-template-columns: 140px minmax(0, 1fr);
  }
  .labels {
    border-right: 1px solid var(--border);
    background: var(--bg-panel);
    position: relative;
  }
  .hwin {
    overflow: hidden;
    position: relative;
  }
  .hcontent {
    position: relative;
    will-change: transform;
  }
  svg {
    display: block;
  }
  .lab1 {
    padding: 4px 6px 4px 10px;
    color: var(--text-dim);
    font-size: var(--fs-sm);
    height: 30px;
  }
  .sechead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 4px;
  }
  .sechead.on {
    background: var(--bg-raised);
  }
  .ruler {
    height: 30px;
  }
  .sec {
    position: absolute;
    top: 0;
    height: 30px;
    border: 0;
    border-left: 1px solid var(--border-strong);
    border-radius: 0;
    background: transparent;
    padding: 5px 6px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }
  .sec b {
    color: var(--text);
    font-weight: 600;
  }
  .sec.sel {
    background: var(--bg-raised);
    color: var(--accent);
  }
  .sec.sel b {
    color: var(--accent);
  }
  /* 盛り上がり */
  .cvlegend {
    position: absolute;
    left: 12px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }
  .cvlegend > div {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 15px;
    cursor: help;
  }
  .cvlegend hr {
    border: 0;
    border-top: 1px solid var(--border);
    width: 100%;
    margin: 2px 0;
  }
  .cvlegend .ln {
    width: 14px;
    border-top: 2px solid var(--accent);
  }
  .cvlegend .ln.dash {
    border-top: 2px dashed var(--text-dim);
  }
  .cvlegend .mk {
    width: 14px;
    height: 12px;
    border: 1px solid var(--border-strong);
    border-radius: 3px;
    background: var(--bg-raised);
    font-size: 9px;
    line-height: 10px;
    text-align: center;
  }
  .cvlegend .mk.warn {
    border-color: var(--warn);
    color: var(--warn);
  }
  .cvlegend .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--warn);
    margin: 0 4px 0 3px;
  }
  .cvlegend.nomeasure .m {
    opacity: 0.35;
  }
  .axisline {
    position: absolute;
    right: 30px;
    border-left: 1px solid var(--border);
  }
  .axis {
    position: absolute;
    right: 8px;
    font-size: 10px;
    color: var(--text-faint);
    transform: translateY(-50%);
    font-variant-numeric: tabular-nums;
  }
  .axis.edge {
    color: var(--text-dim);
  }
  .outside {
    fill: var(--bg-inset);
  }
  line.grid {
    stroke: var(--bg-lane-alt);
    stroke-dasharray: 2 4;
  }
  line.edge {
    stroke: var(--border-strong);
  }
  line.secline {
    stroke: var(--border);
  }
  svg line,
  svg text {
    pointer-events: none;
  }
  path.area {
    fill: var(--accent);
    fill-opacity: 0.07;
  }
  path.area.on {
    fill-opacity: 0.18;
  }
  path.plan {
    stroke: var(--accent);
    stroke-width: 2;
    fill: none;
  }
  circle.pt {
    fill: var(--bg-panel);
    stroke: var(--accent);
    stroke-width: 1.5;
  }
  circle.pt.on {
    fill: var(--accent);
  }
  line.measured {
    stroke: var(--text-dim);
    stroke-width: 1.5;
    stroke-dasharray: 5 4;
    pointer-events: none;
  }
  circle.offdot {
    fill: var(--warn);
  }
  g.join rect {
    fill: var(--bg-raised);
    stroke: var(--border-strong);
  }
  g.join text {
    font-size: 10px;
    text-anchor: middle;
    fill: var(--text-dim);
  }
  g.join.step rect {
    stroke: var(--warn);
  }
  g.join.step text {
    fill: var(--warn);
  }
  /* 音域 */
  .bandlane {
    display: grid;
    grid-template-columns: 140px minmax(0, 1fr);
  }
  .partlist {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px;
    overflow-y: auto;
  }
  .partbtn {
    display: flex;
    gap: 6px;
    align-items: center;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    color: var(--text-dim);
    font-size: var(--fs-sm);
    padding: 3px 6px;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .partbtn:hover {
    background: var(--bg-raised);
    color: var(--text);
  }
  .partbtn.on {
    border-color: var(--accent-dim);
    background: var(--bg-raised);
    color: var(--text);
  }
  .chip {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex: none;
  }
  .emptyrow {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    padding: 6px 10px;
  }
  .octaves {
    position: absolute;
    left: 0;
    top: 0;
    width: 30px;
    pointer-events: none;
    background: linear-gradient(90deg, var(--bg-panel) 60%, transparent);
    z-index: 1;
  }
  .octave {
    position: absolute;
    left: 4px;
    font-size: 10px;
    color: var(--text-faint);
    transform: translateY(-50%);
  }
  rect.other {
    fill: var(--text);
    fill-opacity: 0.04;
    stroke: var(--text);
    stroke-opacity: 0.14;
    pointer-events: none;
  }
  rect.mine {
    fill: var(--c);
    fill-opacity: 0.26;
    stroke: var(--c);
    cursor: pointer;
  }
  rect.mine.on {
    fill-opacity: 0.42;
    stroke-width: 2;
  }
  rect.real {
    fill: none;
    stroke: var(--text);
    stroke-opacity: 0.7;
    stroke-dasharray: 3 3;
  }
  text.bandlabel {
    font-size: 10.5px;
    pointer-events: none;
  }
  line.hatch {
    stroke: var(--text);
    stroke-opacity: 0.25;
  }
  /* 役割 */
  .vscroll {
    overflow-y: auto;
    overflow-x: hidden;
  }
  .mlabel {
    display: flex;
    flex-direction: column;
    justify-content: center;
    width: 100%;
    border: 0;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    background: transparent;
    padding: 2px 10px;
    text-align: left;
    overflow: hidden;
  }
  .mlabel > * {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mlabel.on {
    background: var(--bg-raised);
  }
  .trackname {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: var(--fs-sm);
  }
  .mlabel small {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    padding-left: 14px;
  }
  .lane.compact .mlabel small {
    display: none;
  }
  .mrow {
    position: relative;
    border-bottom: 1px solid var(--border);
  }
  .cell {
    position: absolute;
    top: 0;
    border: 0;
    border-left: 1px solid var(--border);
    border-radius: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: transparent;
    color: var(--text);
    padding: 0 4px;
    font-size: var(--fs-xs);
  }
  .cell:hover {
    background: var(--bg-raised);
  }
  .cell.sel {
    outline: 1px solid var(--accent);
    outline-offset: -1px;
    background: var(--bg-raised);
  }
  .cell.unplanned {
    color: var(--text-faint);
  }
  .steps {
    display: flex;
    gap: 2px;
  }
  .steps i {
    width: 4px;
    height: 10px;
    background: var(--bg-lane-alt);
    border-radius: 1px;
  }
  .steps i.on {
    background: var(--text-dim);
  }
  .cell:not(.unplanned) .steps i.on {
    background: var(--text);
  }
  .cell.sel .steps i.on {
    background: var(--accent);
  }
  .cell .lab {
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cell.diff::after {
    content: "";
    position: absolute;
    top: 5px;
    right: 5px;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--warn);
  }
  .cell .lock {
    position: absolute;
    left: 4px;
    top: 2px;
    font-size: 9px;
  }
  .vgrip {
    flex: none;
    height: 9px;
    cursor: row-resize;
    border-top: 1px solid var(--border);
    background: var(--bg-lane);
    position: relative;
    touch-action: none;
  }
  .vgrip::after {
    content: "";
    position: absolute;
    left: 50%;
    top: 3px;
    width: 36px;
    height: 2px;
    margin-left: -18px;
    border-radius: 1px;
    background: var(--border-strong);
  }
  .vgrip:hover::after {
    background: var(--accent);
  }
  .hbar {
    position: sticky;
    bottom: 0;
    z-index: 3;
    flex: none;
    overflow-x: auto;
    overflow-y: hidden;
    height: 20px;
    padding-bottom: 6px;
    margin-left: 141px;
    background: var(--bg-inset);
  }
  /* 詳しい情報 */
  .panel {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
  }
  .panel h3 {
    margin: 0;
    font-size: var(--fs-md);
  }
  .panel h4 {
    margin: 4px 0 0;
    font-size: var(--fs-sm);
    color: var(--text-dim);
    font-weight: 600;
  }
  .empty {
    color: var(--text-faint);
    font-size: var(--fs-sm);
    margin: 0;
  }
  .kv {
    display: grid;
    grid-template-columns: 96px 1fr;
    gap: 4px 8px;
    font-size: var(--fs-sm);
    margin: 0;
  }
  .kv dt {
    color: var(--text-dim);
  }
  .kv dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .finding {
    display: flex;
    gap: 6px;
    font-size: var(--fs-sm);
    color: var(--text-dim);
  }
  .finding .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn);
    margin-top: 6px;
    flex: none;
  }
  .finding.info .dot {
    background: var(--text-faint);
  }
  .clipstate {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .badge {
    font-size: var(--fs-xs);
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    padding: 0 6px;
    color: var(--text-dim);
  }
  .badge.ahead {
    border-color: var(--accent-dim);
    color: var(--accent);
  }
  .badge.edited {
    border-color: var(--human);
    color: var(--human);
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  /* ---- 案(段階 3) ---- */
  .proposals {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-md);
    padding: 6px 10px;
    background: var(--bg-panel);
    font-size: var(--fs-sm);
    max-height: 40vh;
    overflow-y: auto;
  }
  .phead {
    display: flex;
    gap: 10px;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .prop {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    border-radius: var(--r-sm);
    border: 1px dashed var(--border-strong);
  }
  .prop.on {
    border-style: solid;
    border-color: var(--accent);
  }
  .pall {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    border-radius: var(--r-sm);
  }
  .pall.on {
    border: 1px solid var(--accent);
    /* 案の一覧を送っても、切り替えは見えたままにする */
    position: sticky;
    top: -6px;
    z-index: 1;
    background: var(--bg-panel);
  }
  .pletter {
    min-width: 18px;
    text-align: center;
    border-radius: 3px;
    border: 1px solid var(--accent-dim);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .pletter.cur {
    background: var(--accent);
    color: var(--bg);
  }
  .prow {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .pname {
    font-weight: 600;
  }
  .plabel {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .pnote {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }
  .pwarn {
    color: var(--warn);
    font-size: var(--fs-xs);
  }
  .secchip {
    border-radius: 10px;
    padding: 0 8px;
    font-size: var(--fs-xs);
  }
  .secchip.on {
    border-color: var(--accent);
    color: var(--accent);
  }
  .pbase,
  .abnote {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .pwhy {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .abswitch {
    display: inline-flex;
    gap: 2px;
  }
  .abswitch .btn.on {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }

  /* ---- 直す(段階 2) ---- */
  .spacer {
    flex: 1;
  }
  .estimate label {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .songgrid input[type="text"],
  .songgrid input[type="number"],
  .songgrid select,
  .panel select {
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    padding: 3px 6px;
    min-width: 0;
  }
  .songgrid input[type="number"] {
    width: 90px;
  }
  .songgrid input[type="range"] {
    accent-color: var(--accent);
    width: 100%;
  }
  .songgrid input[type="range"].unset {
    opacity: 0.4;
  }
  .songgrid .row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .word {
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .wx {
    background: transparent;
    border: 0;
    padding: 0 2px;
    color: var(--text-dim);
    font-size: var(--fs-xs);
    line-height: 1;
  }
  .wx:hover {
    color: var(--text);
  }
  .wordin {
    width: 80px;
    background: transparent !important;
    border: 1px dashed var(--border-strong) !important;
    border-radius: 10px !important;
    padding: 0 8px !important;
    font-size: var(--fs-xs) !important;
  }
  .tools {
    display: flex;
    gap: 4px;
    align-items: center;
    flex-wrap: wrap;
  }
  .tools .btn {
    padding: 1px 8px;
    font-size: var(--fs-xs);
  }
  .tools select {
    background: var(--bg-raised);
    color: var(--text);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-xs);
    padding: 1px 4px;
  }
  .tools .sep {
    width: 1px;
    height: 14px;
    background: var(--border-strong);
    margin: 0 4px;
  }
  rect.hit {
    cursor: ns-resize;
  }
  circle.pt {
    cursor: grab;
  }
  g.join {
    cursor: pointer;
  }
  rect.edge-hit {
    fill: transparent;
    cursor: ns-resize;
  }
  rect.mine {
    cursor: grab;
  }
  .sec .cm,
  .cell .cm {
    font-size: 9px;
  }
  .cell .cm {
    position: absolute;
    left: 4px;
    bottom: 1px;
  }
  .cell {
    cursor: pointer;
  }
  .cellbtn {
    position: absolute;
    right: 3px;
    top: 50%;
    transform: translateY(-50%);
    width: 20px;
    height: 18px;
    padding: 0;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-sm);
    background: var(--bg-raised);
    color: var(--text);
    display: none;
    align-items: center;
    justify-content: center;
  }
  .cell:hover .cellbtn,
  .cell.sel .cellbtn,
  .cellbtn.open {
    display: inline-flex;
  }
  .cellbtn.open {
    border-color: var(--accent);
    color: var(--accent);
  }
  .cellbtn.open :global(svg) {
    transform: rotate(180deg);
  }
  .cell.sel .lab,
  .cell:hover .lab {
    max-width: calc(100% - 64px);
  }
  .pop {
    position: fixed;
    z-index: 20;
    width: 260px;
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-md);
    padding: 8px;
    box-shadow: var(--shadow-pop);
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: var(--fs-sm);
  }
  .pop h4 {
    margin: 0 0 4px;
    font-size: var(--fs-sm);
  }
  .pop .opt {
    display: grid;
    grid-template-columns: 34px 1fr;
    gap: 6px;
    align-items: center;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    padding: 3px 6px;
    text-align: left;
    font-size: var(--fs-sm);
  }
  .pop .opt:hover {
    background: var(--bg-panel);
  }
  .pop .opt.on {
    border-color: var(--accent-dim);
  }
  .pop .opt .steps i.on {
    background: var(--text);
  }
  .pop .row {
    display: flex;
    gap: 6px;
    align-items: center;
    color: var(--text-dim);
  }
  .pop select {
    flex: 1;
    background: var(--bg-panel);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .pop .help {
    margin: 0;
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }
  .memo-box {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .thread {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 160px;
    overflow-y: auto;
    font-size: var(--fs-sm);
  }
  .memo {
    border-left: 2px solid var(--border-strong);
    padding: 2px 0 2px 8px;
  }
  .memo .who {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }
  .memo-box textarea {
    width: 100%;
    resize: vertical;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    padding: 6px 8px;
    font: inherit;
    font-size: var(--fs-sm);
  }
  .actions {
    flex-wrap: wrap;
  }
  .actions.two .btn {
    flex: 1;
  }
  @media (max-width: 980px) {
    .main {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
