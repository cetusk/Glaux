<script lang="ts">
  // トラック下に開くオートメーションレーン(音量 / パン)。
  // ダブルクリックで点追加、ドラッグで移動、右クリックで削除。
  // すべて set_automation_points(レーン全置換)として Command API に流す。
  import * as api from "./api";
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
    target: "volume_db" | "pan";
    pxPerTick: number;
    totalPx: number;
    onTarget: (t: "volume_db" | "pan") => void;
    onClose: () => void;
  } = $props();

  const LANE_H = 64;
  const PAD = 5;
  const SNAP = 240; // 1/16 音符

  const range = $derived(
    target === "volume_db" ? { min: -60, max: 6 } : { min: -1, max: 1 },
  );

  const lane = $derived(
    track.automation.find((l) => l.target === `track/${target}`),
  );
  const points = $derived(lane?.points ?? []);

  /// レーンが無いときに表示する基準値(フェーダーの現在値)
  const faderValue = $derived(target === "volume_db" ? track.volume_db : track.pan);

  function yOf(v: number): number {
    const t = (range.max - v) / (range.max - range.min);
    return PAD + t * (LANE_H - PAD * 2);
  }

  function valueOf(y: number): number {
    const t = (y - PAD) / (LANE_H - PAD * 2);
    const v = range.max - t * (range.max - range.min);
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

    // 基準線(音量 0dB / パン中央)
    const zeroY = yOf(0);
    g.fillStyle = "rgba(255,255,255,0.12)";
    g.fillRect(0, zeroY, totalPx, 1);

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
          {
            op: "set_automation_points",
            track: track.id,
            target: `track/${target}`,
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
    return target === "volume_db" ? Math.round(v * 10) / 10 : Math.round(v * 100) / 100;
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
    commit(next, `${track.name} の${label(target)}オートメーションを編集`);
  }

  function onDblClick(e: MouseEvent) {
    if (hitPoint(e.offsetX, e.offsetY) >= 0) return;
    const tick = Math.max(0, Math.round(e.offsetX / pxPerTick / SNAP) * SNAP);
    const value = roundValue(valueOf(e.offsetY));
    commit([...points, { tick, value }], `${track.name} の${label(target)}に点を追加`);
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    const i = hitPoint(e.offsetX, e.offsetY);
    if (i < 0) return;
    const next = points.filter((_, k) => k !== i);
    commit(
      next,
      next.length === 0
        ? `${track.name} の${label(target)}オートメーションを削除`
        : `${track.name} の${label(target)}の点を削除`,
    );
  }

  function label(t: "volume_db" | "pan"): string {
    return t === "volume_db" ? "音量" : "パン";
  }
</script>

<div class="auto-row">
  <div class="auto-head">
    <div class="tabs">
      <button class:active={target === "volume_db"} onclick={() => onTarget("volume_db")}>
        音量
      </button>
      <button class:active={target === "pan"} onclick={() => onTarget("pan")}>パン</button>
      <button class="close" onclick={onClose} title="レーンを閉じる">✕</button>
    </div>
    <div class="hint">
      {#if points.length === 0}
        ダブルクリックで点を追加(破線 = フェーダー値)
      {:else}
        ドラッグ: 移動 / 右クリック: 削除 / レーンがフェーダーより優先
      {/if}
    </div>
    <div class="range-label">
      <span>{range.max}{target === "volume_db" ? " dB" : ""}</span>
      <span>{range.min}{target === "volume_db" ? " dB" : ""}</span>
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
  .auto-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .auto-head {
    width: 200px;
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
    border: none;
    background: none;
    color: var(--text-dim);
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
