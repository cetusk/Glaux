<script lang="ts">
  // MIDI クリップのノート縮小プレビュー(Canvas 版)。
  // 以前は SVG rect をノート数ぶん DOM に並べていたが、ノートが数千になると
  // 変更のたびの DOM 再構築が最大のボトルネックになるため Canvas に置き換えた。
  import type { MidiClip } from "./types";

  let { clip, widthPx }: { clip: MidiClip; widthPx: number } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();

  const H = 44; // クリップ内側の描画高さ(CSS 側と合わせる)

  $effect(() => {
    const c = canvas;
    if (!c) return;
    const notes = clip.notes;
    const w = Math.max(1, Math.min(Math.round(widthPx), 4096));
    if (c.width !== w || c.height !== H) {
      c.width = w;
      c.height = H;
    }
    const g = c.getContext("2d")!;
    g.clearRect(0, 0, w, H);
    if (notes.length === 0) return;

    let lo = 127;
    let hi = 0;
    for (const n of notes) {
      if (n.pitch < lo) lo = n.pitch;
      if (n.pitch > hi) hi = n.pitch;
    }
    const span = Math.max(hi - lo + 1, 12);
    const rowH = Math.max(2, Math.min(6, H / span));
    const scaleX = w / clip.length;

    g.fillStyle = "rgba(255,255,255,0.85)";
    for (const n of notes) {
      const x = n.pos * scaleX;
      const nw = Math.max(n.dur * scaleX, 1.5);
      const y = ((hi - n.pitch) / span) * (H - rowH);
      g.fillRect(x, y, nw, rowH);
    }
  });
</script>

<canvas bind:this={canvas} class="preview"></canvas>

<style>
  .preview {
    position: absolute;
    inset: 18px 2px 2px 2px;
    width: calc(100% - 4px);
    height: calc(100% - 20px);
    pointer-events: none;
  }
</style>
