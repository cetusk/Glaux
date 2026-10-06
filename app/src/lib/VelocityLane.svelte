<script lang="ts">
  import { plural, tr } from "./i18n.svelte";
  // ピアノロールの下端のベロシティの帯(縦スクロールしても下端に残り、横スクロールはノートと連動)。
  // 縦棒を上下にドラッグで強さを変え、選択中のノートを掴むと選択中すべてを同じ量だけ増減する
  import * as api from "./api";
  import type { Bar } from "./barMap";
  import { accentRgb, MAX_CANVAS_PX } from "./pianoRollOps";
  import { settings } from "./settings.svelte";
  import type { MidiClip, Note } from "./types";

  let {
    clip,
    songBars,
    pxPerTick,
    contentW,
    win,
    keyW,
    selected = $bindable(),
  }: {
    clip: MidiClip;
    /// 曲の小節のうちクリップに重なるもの
    songBars: Bar[];
    pxPerTick: number;
    contentW: number;
    /// ノートの canvas と同じ描画の窓(横の位置と幅だけ使う)
    win: { x: number; y: number; w: number; h: number };
    keyW: number;
    selected: Set<string>;
  } = $props();

  const VEL_H = 64;
  const VEL_PAD = 4;
  let velEl: HTMLCanvasElement | undefined = $state();
  /// ドラッグ中の変更(ノート ID → 元の強さ)と、掴んだノートの増減量
  let velDrag = $state<{ base: Map<string, number>; anchor: string; delta: number } | null>(null);

  function velFromY(y: number): number {
    const t = 1 - (y - VEL_PAD) / (VEL_H - VEL_PAD * 2);
    return Math.max(1, Math.min(127, Math.round(t * 127)));
  }

  function shownVel(n: Note): number {
    const d = velDrag;
    const base = d?.base.get(n.id);
    if (d && base !== undefined) return Math.max(1, Math.min(127, base + d.delta));
    return n.vel;
  }

  function velBarRect(n: Note): { x: number; w: number } {
    return { x: n.pos * pxPerTick, w: Math.max(3, Math.min(8, n.dur * pxPerTick - 1)) };
  }

  function drawVel() {
    const c = velEl;
    const currentClip = clip;
    if (!c || !currentClip) return;
    const dpr = window.devicePixelRatio || 1;
    const scale = Math.min(dpr, MAX_CANVAS_PX / win.w);
    const w = Math.max(1, Math.round(win.w * scale));
    const h = Math.round(VEL_H * scale);
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const g = c.getContext("2d")!;
    g.setTransform(scale, 0, 0, scale, -win.x * scale, 0);
    g.clearRect(0, 0, contentW, VEL_H);
    g.fillStyle = "#1b1b1b";
    g.fillRect(0, 0, contentW, VEL_H);
    // 目安線(25 / 50 / 75 / 100%)
    g.fillStyle = "#2a2a2a";
    for (const f of [0.25, 0.5, 0.75, 1]) {
      g.fillRect(0, Math.round(VEL_PAD + (1 - f) * (VEL_H - VEL_PAD * 2)), contentW, 1);
    }
    // 小節線
    g.fillStyle = "#333333";
    for (const b of songBars) {
      g.fillRect((b.tick - currentClip.start) * pxPerTick, 0, 1, VEL_H);
    }
    for (const n of currentClip.notes) {
      const v = shownVel(n);
      const { x, w } = velBarRect(n);
      const bh = (v / 127) * (VEL_H - VEL_PAD * 2);
      const isSel = selected.has(n.id);
      g.fillStyle = isSel ? `rgba(${accentRgb()}, 0.95)` : "rgba(94, 156, 224, 0.9)";
      g.fillRect(x, VEL_H - VEL_PAD - bh, w, bh);
      // 頭に丸(掴む場所の目印)
      g.beginPath();
      g.arc(x + w / 2, VEL_H - VEL_PAD - bh, 2.5, 0, Math.PI * 2);
      g.fill();
    }
  }

  $effect(() => {
    void clip;
    void selected;
    void songBars;
    void pxPerTick;
    void velDrag;
    void win;
    void settings.accent;
    drawVel();
  });

  /// x 位置の縦棒のノート(複数重なるときは開始位置が近い方)
  function velNoteAt(x: number): Note | null {
    const currentClip = clip;
    if (!currentClip) return null;
    let best: Note | null = null;
    let bestDist = Infinity;
    for (const n of currentClip.notes) {
      const { x: bx, w } = velBarRect(n);
      if (x < bx - 3 || x > bx + w + 3) continue;
      const d = Math.abs(x - (bx + w / 2));
      if (d < bestDist) {
        bestDist = d;
        best = n;
      }
    }
    return best;
  }

  function onVelDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const n = velNoteAt(e.clientX - rect.left + win.x);
    if (!n) return;
    const currentClip = clip;
    if (!currentClip) return;
    // 選択中のノートを掴んだら選択中すべてを同じ量だけ動かす
    let ids: string[];
    if (selected.has(n.id) && selected.size > 1) {
      ids = [...selected];
    } else {
      ids = [n.id];
      selected = new Set([n.id]);
    }
    const base = new Map<string, number>();
    for (const m of currentClip.notes) if (ids.includes(m.id)) base.set(m.id, m.vel);
    velDrag = { base, anchor: n.id, delta: velFromY(e.clientY - rect.top) - n.vel };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onVelMove(e: PointerEvent) {
    const d = velDrag;
    if (!d) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const anchorBase = d.base.get(d.anchor) ?? 64;
    velDrag = { ...d, delta: velFromY(e.clientY - rect.top) - anchorBase };
  }

  function onVelUp() {
    const d = velDrag;
    const currentClip = clip;
    if (!d || !currentClip) {
      velDrag = null;
      return;
    }
    const changes = [...d.base.entries()]
      .map(([id, v]) => ({ id, vel: Math.max(1, Math.min(127, v + d.delta)) }))
      .filter((c) => c.vel !== d.base.get(c.id));
    if (changes.length > 0) {
      const one = changes.length === 1 ? `(${changes[0].vel})` : tr(`(${changes.length} 個)`, ` (${plural(changes.length, "note")})`);
      // 反映されるまでのちらつきを避けるため、ドラッグ表示は編集の完了後に消す
      // (失敗は api.applyEdit がトーストで知らせる)
      api
        .applyEdit([{ op: "update_notes", clip: currentClip.id, changes }], tr(`ベロシティを変更${one}`, `Change velocity${one}`))
        .catch(() => {})
        .finally(() => (velDrag = null));
    } else {
      velDrag = null;
    }
  }
</script>

<div class="vel-row" style="height:{VEL_H}px">
  <div class="vel-corner" style="width:{keyW}px" title={tr("ベロシティ(音の強さ 1〜127)", "Velocity (note strength, 1–127)")}>Vel</div>
  <div class="vel-track" style="width:{contentW}px;height:{VEL_H}px">
  <canvas
    class="vel-layer"
    bind:this={velEl}
    style="left:{win.x}px;width:{win.w}px;height:{VEL_H}px"
    title={velDrag
      ? `${tr("ベロシティ", "Velocity")} ${Math.max(1, Math.min(127, (velDrag.base.get(velDrag.anchor) ?? 0) + velDrag.delta))}`
      : tr("縦棒を上下にドラッグで音の強さを変更(選択中のノートはまとめて変わる)", "Drag the bars up or down to change note strength (selected notes change together)")}
    onpointerdown={onVelDown}
    onpointermove={onVelMove}
    onpointerup={onVelUp}
    onpointercancel={() => (velDrag = null)}
  ></canvas>
  </div>
</div>

<style>
  /* ベロシティの帯: 縦スクロールしても下端に残る */
  .vel-row {
    display: flex;
    position: sticky;
    bottom: 0;
    z-index: 3;
    border-top: 1px solid var(--border);
  }

  .vel-corner {
    position: sticky;
    left: 0;
    z-index: 4;
    flex-shrink: 0;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    font-size: 10px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .vel-layer {
    cursor: ns-resize;
  }

  canvas {
    display: block;
    touch-action: none;
  }

  /* canvas は窓(見えている範囲 + 余白)の大きさで、その位置に置く */
  canvas.vel-layer {
    position: absolute;
  }

  .vel-track {
    position: relative;
    flex-shrink: 0;
  }
</style>
