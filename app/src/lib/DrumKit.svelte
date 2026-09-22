<script lang="ts">
  import { DRUM_PIECES, type DrumPiece } from "./drumMap";

  let {
    highlight = null,
    onHit,
  }: {
    highlight?: number | null;
    onHit: (pitch: number) => void;
  } = $props();

  /** リムのラグ(留め具)の位置 */
  function lugs(p: DrumPiece, n = 8): { x: number; y: number }[] {
    const out = [];
    for (let i = 0; i < n; i++) {
      const a = (i / n) * Math.PI * 2 - Math.PI / 2;
      out.push({ x: p.cx + Math.cos(a) * (p.r - 1), y: p.cy + Math.sin(a) * (p.r - 1) });
    }
    return out;
  }
</script>

<div class="kit">
  <svg viewBox="0 0 400 175" preserveAspectRatio="xMidYMid meet">
    <defs>
      <radialGradient id="cymbal-g" cx="0.4" cy="0.4">
        <stop offset="0%" stop-color="#e5c46e" />
        <stop offset="55%" stop-color="#c9a44e" />
        <stop offset="100%" stop-color="#8a6d2f" />
      </radialGradient>
      <radialGradient id="head-g" cx="0.42" cy="0.4">
        <stop offset="0%" stop-color="#f2ede4" />
        <stop offset="70%" stop-color="#ddd6c9" />
        <stop offset="100%" stop-color="#bcb4a5" />
      </radialGradient>
      <radialGradient id="kick-g" cx="0.42" cy="0.4">
        <stop offset="0%" stop-color="#57506e" />
        <stop offset="100%" stop-color="#2c2840" />
      </radialGradient>
    </defs>

    {#each DRUM_PIECES as p (p.pitch)}
      <g
        class="piece"
        class:hl={highlight === p.pitch}
        onpointerdown={() => onHit(p.pitch)}
        role="button"
        tabindex="-1"
      >
        {#if p.kind === "cymbal"}
          <!-- シンバル: 金属光沢 + 溝 + ベル。ハイハットは二枚重ねに見せる -->
          {#if p.pitch === 42 || p.pitch === 46}
            <circle cx={p.cx + 2.5} cy={p.cy + 3} r={p.r} fill="#7a6128" opacity="0.9" />
          {/if}
          <circle class="sel" cx={p.cx} cy={p.cy} r={p.r} fill="url(#cymbal-g)" />
          <circle cx={p.cx} cy={p.cy} r={p.r * 0.72} fill="none" stroke="rgba(60,42,8,0.25)" />
          <circle cx={p.cx} cy={p.cy} r={p.r * 0.45} fill="none" stroke="rgba(60,42,8,0.2)" />
          <circle cx={p.cx} cy={p.cy} r={p.r * 0.16} fill="#e8cd85" stroke="rgba(60,42,8,0.35)" />
          {#if p.pitch === 46}
            <!-- オープンハットは隙間を示す破線 -->
            <circle cx={p.cx} cy={p.cy} r={p.r + 3} fill="none"
              stroke="var(--accent)" stroke-width="1" stroke-dasharray="3 4" opacity="0.7" />
          {/if}
          <text x={p.cx} y={p.cy + 4} class="t-dark">{p.short}</text>
        {:else if p.kind === "pad"}
          <!-- 電子パッド(クラップ) -->
          <rect class="sel" x={p.cx - p.r - 4} y={p.cy - p.r * 0.8} width={(p.r + 4) * 2}
            height={p.r * 1.6} rx="6" fill="#3a3450" stroke="#565073" stroke-width="1.5" />
          <rect x={p.cx - p.r + 1} y={p.cy - p.r * 0.8 + 5} width={p.r * 2 - 2}
            height={p.r * 1.6 - 10} rx="3" fill="#4c4566" />
          <text x={p.cx} y={p.cy + 4}>{p.short}</text>
        {:else}
          <!-- 太鼓: シェル(リム)+ ヘッド + ラグ -->
          <circle class="sel" cx={p.cx} cy={p.cy} r={p.r}
            fill={p.pitch === 36 ? "url(#kick-g)" : "#4a445f"} stroke="#1d1a2b" stroke-width="1.5" />
          <circle cx={p.cx} cy={p.cy} r={p.r - 3.5}
            fill={p.pitch === 36 ? "url(#kick-g)" : "url(#head-g)"} />
          {#if p.pitch === 38}
            <!-- スネアのスナッピー(響き線)の示唆 -->
            <g stroke="rgba(120,110,95,0.55)" stroke-width="1">
              {#each [-6, -2, 2, 6] as off}
                <line x1={p.cx - p.r + 7} y1={p.cy + off} x2={p.cx + p.r - 7} y2={p.cy + off} />
              {/each}
            </g>
          {/if}
          {#if p.pitch === 36}
            <circle cx={p.cx} cy={p.cy} r={p.r * 0.32} fill="#1d1a2b" opacity="0.5" />
          {/if}
          {#each lugs(p, p.r > 30 ? 10 : 8) as l}
            <circle cx={l.x} cy={l.y} r="1.6" fill="#8b849e" />
          {/each}
          <text class={p.pitch === 36 ? "" : "t-dark"} x={p.cx} y={p.cy + 4}>{p.short}</text>
        {/if}
        <title>{p.name}(ノート {p.pitch})— クリックで挿入カーソル位置に打ち込み</title>
      </g>
    {/each}
  </svg>
</div>

<style>
  .kit {
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    padding: 4px 8px;
    flex-shrink: 0;
  }

  svg {
    display: block;
    height: 150px;
    margin: 0 auto;
  }

  .piece {
    cursor: pointer;
  }

  .piece:hover {
    filter: brightness(1.18);
  }

  .piece:active {
    filter: brightness(1.45);
  }

  .piece.hl .sel {
    stroke: var(--accent);
    stroke-width: 2.5;
  }

  .piece text {
    fill: var(--text);
    font-size: 11px;
    font-weight: 700;
    text-anchor: middle;
    pointer-events: none;
    user-select: none;
  }

  .piece text.t-dark {
    fill: #2e2818;
  }
</style>
