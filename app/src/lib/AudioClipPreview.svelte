<script lang="ts">
  // 音声クリップの波形プレビュー(Canvas)。ピークはバックエンドで計算し、
  // クリップ ID + 範囲 + 幅ごとにキャッシュする(スクロールや再描画で再取得しない)。
  import * as api from "./api";

  let {
    clipId,
    start,
    length,
    widthPx,
  }: {
    clipId: string;
    start: number;
    length: number;
    widthPx: number;
  } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  const H = 44;

  const cache = new Map<string, Promise<[number, number][]>>();

  function load(key: string, id: string, buckets: number): Promise<[number, number][]> {
    let p = cache.get(key);
    if (!p) {
      p = api.clipPeaks(id, buckets).then((r) => r.peaks);
      cache.set(key, p);
    }
    return p;
  }

  $effect(() => {
    const c = canvas;
    if (!c) return;
    const w = Math.max(1, Math.min(Math.round(widthPx), 4096));
    const buckets = Math.max(8, Math.min(w, 1024));
    const key = `${clipId}:${start}:${length}:${buckets}`;
    let cancelled = false;
    load(key, clipId, buckets)
      .then((peaks) => {
        if (cancelled) return;
        if (c.width !== w || c.height !== H) {
          c.width = w;
          c.height = H;
        }
        const g = c.getContext("2d")!;
        g.clearRect(0, 0, w, H);
        g.fillStyle = "rgba(255,255,255,0.8)";
        const mid = H / 2;
        const sx = w / peaks.length;
        for (let i = 0; i < peaks.length; i++) {
          const [lo, hi] = peaks[i];
          const y0 = mid - hi * mid;
          const y1 = mid - lo * mid;
          g.fillRect(i * sx, Math.min(y0, y1), Math.max(sx, 1), Math.max(1, Math.abs(y1 - y0)));
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
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
