<script lang="ts">
  import Icon from "./Icon.svelte";
  // トラック下に開くオートメーションレーン。対象は音量・パン・音源のつまみ
  // (device/<名前>)・エフェクトのつまみ(fx/<id>/<名前>)。
  // ダブルクリックで点追加、ドラッグで移動、右クリックで削除。
  // すべて set_automation_points(レーン全置換)として Command API に流す。
  // マスター(track.id が MASTER_FOCUS_ID の擬似トラック)では音量とマスターのエフェクトが対象で、
  // set_master_automation_points を使う。
  import * as api from "./api";
  import { MASTER_FOCUS_ID } from "./selection.svelte";
  import type { AutomationPoint, Track } from "./types";

  let {
    track,
    target,
    pxPerTick,
    totalPx,
    onTarget,
    onClose,
  }: {
    track: Track;
    /** レーンのパラメータ(track/volume_db, track/pan, device/<名前>, fx/<id>/<名前>) */
    target: string;
    pxPerTick: number;
    totalPx: number;
    onTarget: (t: string) => void;
    onClose: () => void;
  } = $props();

  const LANE_H = 64;
  const PAD = 5;
  const SNAP = 240; // 1/16 音符

  /// 選べるパラメータ(範囲・単位・現在値つき)
  interface Target {
    path: string;
    label: string;
    min: number;
    max: number;
    unit: string;
    current: number;
    int: boolean;
    /** 周波数のように範囲が広い正の値は対数目盛り */
    log: boolean;
  }

  const isMaster = $derived(track.id === MASTER_FOCUS_ID);

  const BUILTIN: Target[] = $derived([
    { path: "track/volume_db", label: "音量", min: -60, max: 6, unit: " dB", current: track.volume_db, int: false, log: false },
    ...(isMaster
      ? []
      : [{ path: "track/pan", label: "パン", min: -1, max: 1, unit: "", current: track.pan, int: false, log: false }]),
  ]);

  let paramTargets = $state<Target[]>([]);
  $effect(() => {
    // 音源・エフェクトの構成が変わったら一覧を取り直す
    void track.device;
    void track.effects;
    (isMaster ? api.getMasterParams() : api.getTrackParams(track.id))
      .then((info) => {
        const out: Target[] = [];
        const add = (prefix: string, p: import("./types").ParamView) => {
          if (p.range.kind !== "float" && p.range.kind !== "int") return;
          const { min, max } = p.range;
          out.push({
            path: p.path,
            label: `${prefix}${p.display_name}`,
            min,
            max,
            unit: p.unit ? ` ${p.unit}` : "",
            current: Number(p.current),
            int: p.range.kind === "int",
            log: min > 0 && max / min >= 100,
          });
        };
        for (const p of info.params) add("音色: ", p);
        for (const fx of info.effects) for (const p of fx.params) add(`${fx.name}: `, p);
        paramTargets = out;
      })
      .catch(() => (paramTargets = []));
  });

  const allTargets = $derived([...BUILTIN, ...paramTargets]);
  /// つまみが多い(CLAP プラグインは数百)ときの絞り込み
  let paramFilter = $state("");
  const shownTargets = $derived(
    paramFilter.trim()
      ? paramTargets.filter((t) => t.label.toLowerCase().includes(paramFilter.trim().toLowerCase()))
      : paramTargets,
  );
  const cur = $derived(
    allTargets.find((t) => t.path === target) ?? {
      path: target,
      label: target,
      min: 0,
      max: 1,
      unit: "",
      current: 0,
      int: false,
      log: false,
    },
  );
  const range = $derived({ min: cur.min, max: cur.max });

  const lane = $derived(track.automation.find((l) => l.target === target));
  const points = $derived(lane?.points ?? []);
  /// レーンが描かれているパラメータ(一覧で目印を付ける)
  const lanePaths = $derived(
    new Set(track.automation.filter((l) => l.points.length > 0).map((l) => l.target)),
  );

  /// レーンが無いときに表示する基準値(フェーダー・つまみの現在値)
  const faderValue = $derived(cur.current);

  function norm(v: number): number {
    if (cur.log) {
      return (Math.log(v) - Math.log(range.min)) / (Math.log(range.max) - Math.log(range.min));
    }
    return (v - range.min) / (range.max - range.min);
  }

  function yOf(v: number): number {
    const t = 1 - Math.min(1, Math.max(0, norm(v)));
    return PAD + t * (LANE_H - PAD * 2);
  }

  function valueOf(y: number): number {
    const t = 1 - (y - PAD) / (LANE_H - PAD * 2);
    const u = Math.min(1, Math.max(0, t));
    const v = cur.log
      ? Math.exp(Math.log(range.min) + u * (Math.log(range.max) - Math.log(range.min)))
      : range.min + u * (range.max - range.min);
    return Math.min(range.max, Math.max(range.min, v));
  }

  let canvas: HTMLCanvasElement | undefined = $state();
  let drag = $state<{ index: number; tick: number; value: number } | null>(null);

  function draw() {
    const c = canvas;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.min(Math.round(totalPx * dpr), 16000);
    const h = Math.round(LANE_H * dpr);
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const g = c.getContext("2d")!;
    const scale = w / totalPx;
    g.setTransform(scale, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, totalPx, LANE_H);

    // 基準線(音量 0dB / パン中央。0 を含む範囲のときだけ)
    if (range.min < 0 && range.max > 0) {
      g.fillStyle = "rgba(255,255,255,0.12)";
      g.fillRect(0, yOf(0), totalPx, 1);
    }

    // ドラッグ中のプレビューを織り込んだ点列
    const pts: AutomationPoint[] = points.map((p, i) =>
      drag && drag.index === i ? { ...p, tick: drag.tick, value: drag.value } : p,
    );
    pts.sort((a, b) => a.tick - b.tick);

    if (pts.length === 0) {
      // レーン未使用: フェーダー値の位置に破線
      g.strokeStyle = "rgba(255,255,255,0.35)";
      g.setLineDash([5, 5]);
      g.beginPath();
      g.moveTo(0, yOf(faderValue));
      g.lineTo(totalPx, yOf(faderValue));
      g.stroke();
      g.setLineDash([]);
      return;
    }

    // カーブ(先頭より前・末尾より後は端の値を保持)
    const accent = getComputedStyle(document.documentElement)
      .getPropertyValue("--accent")
      .trim();
    g.strokeStyle = accent;
    g.lineWidth = 1.5;
    g.beginPath();
    g.moveTo(0, yOf(pts[0].value));
    g.lineTo(pts[0].tick * pxPerTick, yOf(pts[0].value));
    for (let i = 0; i < pts.length; i++) {
      const a = pts[i];
      const b = pts[i + 1];
      const ax = a.tick * pxPerTick;
      if (!b) {
        g.lineTo(totalPx, yOf(a.value));
        break;
      }
      const bx = b.tick * pxPerTick;
      if (a.curve === "hold") {
        g.lineTo(bx, yOf(a.value));
        g.lineTo(bx, yOf(b.value));
      } else if (a.curve === "exponential") {
        for (let k = 1; k <= 12; k++) {
          const t = k / 12;
          g.lineTo(ax + (bx - ax) * t, yOf(a.value + (b.value - a.value) * t * t));
        }
      } else {
        g.lineTo(bx, yOf(b.value));
      }
    }
    g.stroke();

    // 点
    for (const p of pts) {
      g.fillStyle = accent;
      g.beginPath();
      g.arc(p.tick * pxPerTick, yOf(p.value), 3.5, 0, Math.PI * 2);
      g.fill();
    }
  }

  $effect(() => {
    void points;
    void drag;
    void totalPx;
    void pxPerTick;
    void target;
    void faderValue;
    draw();
  });

  // ---- 編集 ----

  function commit(next: AutomationPoint[], label: string) {
    const sorted = [...next].sort((a, b) => a.tick - b.tick);
    api
      .applyEdit(
        [
          isMaster
            ? { op: "set_master_automation_points", target, points: sorted }
            : {
                op: "set_automation_points",
                track: track.id,
                target,
                points: sorted,
              },
        ],
        label,
      )
      .catch(() => {});
  }

  function hitPoint(x: number, y: number): number {
    for (let i = 0; i < points.length; i++) {
      const px = points[i].tick * pxPerTick;
      const py = yOf(points[i].value);
      if (Math.abs(px - x) < 7 && Math.abs(py - y) < 7) return i;
    }
    return -1;
  }

  function roundValue(v: number): number {
    if (cur.int) return Math.round(v);
    // 範囲に応じた有効桁(範囲の 1/1000 程度)に丸める
    const span = Math.abs(v) > 0 && cur.log ? Math.abs(v) : range.max - range.min;
    const step = Math.pow(10, Math.floor(Math.log10(Math.max(span, 1e-6))) - 2);
    return Math.round(v / step) * step;
  }

  function fmt(v: number): string {
    const digits = cur.int || Math.abs(v) >= 100 ? 0 : Math.abs(v) >= 10 ? 1 : 2;
    return `${v.toFixed(digits)}${cur.unit}`;
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button === 2) return;
    const i = hitPoint(e.offsetX, e.offsetY);
    if (i >= 0) {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
      drag = { index: i, tick: points[i].tick, value: points[i].value };
    }
  }

  function onPointerMove(e: PointerEvent) {
    const el = e.currentTarget as HTMLElement;
    if (!drag) {
      el.style.cursor = hitPoint(e.offsetX, e.offsetY) >= 0 ? "grab" : "crosshair";
      return;
    }
    const tick = Math.max(0, Math.round(e.offsetX / pxPerTick / SNAP) * SNAP);
    drag = { ...drag, tick, value: roundValue(valueOf(e.offsetY)) };
  }

  function onPointerUp() {
    if (!drag) return;
    const d = drag;
    drag = null;
    const next = points.map((p, i) =>
      i === d.index ? { ...p, tick: d.tick, value: d.value } : p,
    );
    commit(next, `${track.name} の「${label(target)}」オートメーションを編集`);
  }

  function onDblClick(e: MouseEvent) {
    if (hitPoint(e.offsetX, e.offsetY) >= 0) return;
    const tick = Math.max(0, Math.round(e.offsetX / pxPerTick / SNAP) * SNAP);
    const value = roundValue(valueOf(e.offsetY));
    commit([...points, { tick, value }], `${track.name} の「${label(target)}」に点を追加`);
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const i = hitPoint(e.offsetX, e.offsetY);
    if (i < 0) return;
    const next = points.filter((_, k) => k !== i);
    commit(
      next,
      next.length === 0
        ? `${track.name} の「${label(target)}」オートメーションを削除`
        : `${track.name} の「${label(target)}」の点を削除`,
    );
  }

  function label(_t: string): string {
    return cur.label;
  }
</script>

<div class="auto-row">
  <div class="auto-head">
    <div class="tabs">
      <button class:active={target === "track/volume_db"} onclick={() => onTarget("track/volume_db")}>
        音量{#if lanePaths.has("track/volume_db")}<span class="dot" title="描いてある"></span>{/if}
      </button>
      {#if !isMaster}
        <button class:active={target === "track/pan"} onclick={() => onTarget("track/pan")}>
          パン{#if lanePaths.has("track/pan")}<span class="dot" title="描いてある"></span>{/if}
        </button>
      {/if}
      <button class="btn sm icon ghost close" onclick={onClose} title="レーンを閉じる" aria-label="レーンを閉じる"><Icon name="x" /></button>
    </div>
    {#if paramTargets.length > 0}
      <select
        class="param-pick"
        value={target.startsWith("track/") ? "" : target}
        onchange={(e) => {
          const v = (e.currentTarget as HTMLSelectElement).value;
          if (v) onTarget(v);
        }}
        title="音色・エフェクトのつまみを時間で動かす(● はレーンが描かれているもの)"
      >
        <option value="">{isMaster ? "マスターのエフェクトのつまみ…" : "音色・エフェクトのつまみ…"}</option>
        {#each shownTargets.slice(0, 300) as t (t.path)}
          <option value={t.path}>{lanePaths.has(t.path) ? "● " : ""}{t.label}</option>
        {/each}
        {#if shownTargets.length > 300}
          <option value="" disabled>…ほか {shownTargets.length - 300} 個(下の欄で絞り込み)</option>
        {/if}
      </select>
      {#if paramTargets.length > 40}
        <input
          class="param-filter"
          type="search"
          placeholder={`つまみを絞り込み(${paramTargets.length} 個)`}
          bind:value={paramFilter}
        />
      {/if}
    {/if}
    <div class="hint">
      {#if points.length === 0}
        ダブルクリックで点を追加(破線 = 今の値)
      {:else}
        ドラッグ: 移動 / 右クリック: 削除 / レーンがつまみより優先
      {/if}
    </div>
    <div class="range-label">
      <span>{fmt(range.max)}</span>
      <span>{fmt(range.min)}</span>
    </div>
  </div>
  <div class="lane" style="width:{totalPx}px;height:{LANE_H}px">
    <canvas
      bind:this={canvas}
      style="width:{totalPx}px;height:{LANE_H}px"
      onpointerdown={onPointerDown}
      onpointermove={onPointerMove}
      onpointerup={onPointerUp}
      ondblclick={onDblClick}
      oncontextmenu={onContextMenu}
    ></canvas>
  </div>
</div>

<style>
  .param-filter {
    width: 100%;
    margin-top: 3px;
    font-size: 10px;
    padding: 1px 4px;
  }

  .auto-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .auto-head {
    width: var(--head-w, 200px);
    flex-shrink: 0;
    padding: 5px 10px;
    background: color-mix(in srgb, var(--bg-panel) 80%, var(--accent) 6%);
    border-right: 1px solid var(--border);
    position: sticky;
    left: 0;
    z-index: 2;
  }

  .tabs {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .tabs button {
    font-size: 10px;
    padding: 1px 8px;
  }

  .tabs button.active {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .tabs .close {
    margin-left: auto;
  }

  .dot {
    display: inline-block;
    width: 5px;
    height: 5px;
    margin-left: 3px;
    border-radius: 50%;
    background: currentColor;
    vertical-align: 2px;
  }

  .param-pick {
    width: 100%;
    margin-top: 4px;
    font-size: 10px;
  }

  .hint {
    font-size: 9px;
    color: var(--text-dim);
    margin-top: 4px;
    line-height: 1.4;
  }

  .range-label {
    position: absolute;
    right: 4px;
    top: 22px;
    bottom: 2px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    font-size: 8px;
    color: var(--text-dim);
    text-align: right;
    pointer-events: none;
  }

  .lane {
    position: relative;
    background: color-mix(in srgb, var(--bg-lane) 85%, var(--accent) 4%);
  }

  canvas {
    display: block;
    touch-action: none;
  }
</style>
