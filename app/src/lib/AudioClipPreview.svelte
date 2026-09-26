<script lang="ts">
  // 音声クリップの波形プレビュー(Canvas)。ピークはバックエンドで計算し、
  // クリップ ID + 範囲 + 幅ごとにキャッシュする(スクロールや再描画で再取得しない)。
  import * as api from "./api";

  let {
    clipId,
    start,
    length,
    widthPx,
    variant = "",
  }: {
    clipId: string;
    start: number;
    length: number;
    widthPx: number;
    /** 波形の見え方を変える設定(音量・テンポ追従)。変わったら取り直す */
    variant?: string;
  } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  /// 表示されている大きさ(CSS px)。画面の拡大率を掛けた解像度で描く(引き伸ばしてぼやけないように)
  let cssW = $state(0);
  let cssH = $state(0);

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
    const w = Math.max(1, cssW || widthPx);
    const H = Math.max(1, cssH || 44);
    const dpr = window.devicePixelRatio || 1;
    const pw = Math.max(1, Math.min(Math.round(w * dpr), 8192));
    const ph = Math.max(1, Math.round(H * dpr));
    const buckets = Math.max(8, Math.min(Math.round(w), 1024));
    const key = `${clipId}:${start}:${length}:${buckets}:${variant}`;
    let cancelled = false;
    load(key, clipId, buckets)
      .then((peaks) => {
        if (cancelled) return;
        if (c.width !== pw || c.height !== ph) {
          c.width = pw;
          c.height = ph;
        }
        const g = c.getContext("2d")!;
        g.setTransform(1, 0, 0, 1, 0, 0);
        g.clearRect(0, 0, pw, ph);
        // ここから下は CSS px で描く
        g.setTransform(pw / w, 0, 0, ph / H, 0, 0);
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

<canvas bind:this={canvas} bind:clientWidth={cssW} bind:clientHeight={cssH} class="preview"></canvas>

<style>
  .preview {
    position: absolute;
    inset: 18px 2px 2px 2px;
    width: calc(100% - 4px);
    height: calc(100% - 20px);
    pointer-events: none;
  }
</style>
