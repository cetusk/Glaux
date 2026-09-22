<script lang="ts">
  // ギターの指板(pluck トラック用の入力盤)。ドラムキットと同じ発想で、
  // フレットをクリックすると挿入カーソル位置にそのピッチを打ち込む。
  let {
    highlight,
    onHit,
  }: {
    highlight: number | null;
    onHit: (pitch: number) => void;
  } = $props();

  // 標準チューニング。タブ譜と同じく上が 1 弦(高い E)
  const STRINGS = [
    { label: "e", base: 64 },
    { label: "B", base: 59 },
    { label: "G", base: 55 },
    { label: "D", base: 50 },
    { label: "A", base: 45 },
    { label: "E", base: 40 },
  ];
  const FRET_COUNT = 16; // 0(開放)〜15
  const MARKS = new Set([3, 5, 7, 9, 15]);

  const NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
  function noteName(pitch: number): string {
    return `${NOTE_NAMES[pitch % 12]}${Math.floor(pitch / 12) - 1}`;
  }
</script>

<div class="fretboard">
  <div class="row head">
    <div class="str-label"></div>
    {#each Array(FRET_COUNT) as _, f}
      <div class="fret-no" class:mark={MARKS.has(f)} class:oct={f === 12}>
        {f === 12 ? "12●" : MARKS.has(f) ? `${f}・` : f}
      </div>
    {/each}
  </div>
  {#each STRINGS as s (s.label)}
    <div class="row">
      <div class="str-label">{s.label}</div>
      {#each Array(FRET_COUNT) as _, f}
        {@const pitch = s.base + f}
        <button
          class="fret"
          class:nut={f === 0}
          class:hl={highlight === pitch}
          title={`${noteName(pitch)} — ${s.label} 弦 ${f === 0 ? "開放" : `${f} フレット`}(クリックで挿入カーソル位置に打ち込み)`}
          onclick={() => onHit(pitch)}
        >
          <span class="dot"></span>
        </button>
      {/each}
    </div>
  {/each}
</div>

<style>
  .fretboard {
    flex-shrink: 0;
    padding: 6px 12px 8px;
    background: color-mix(in srgb, var(--bg-panel) 70%, #3a2a1a 30%);
    border-bottom: 1px solid var(--border);
    user-select: none;
  }

  .row {
    display: grid;
    grid-template-columns: 30px repeat(16, minmax(28px, 1fr));
    align-items: stretch;
  }

  .row.head {
    margin-bottom: 2px;
  }

  .fret-no {
    text-align: center;
    font-size: 9px;
    color: var(--text-dim);
    white-space: nowrap;
  }

  .fret-no.mark,
  .fret-no.oct {
    color: var(--accent);
  }

  .str-label {
    font-size: 10px;
    font-weight: 700;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .fret {
    position: relative;
    height: 19px;
    padding: 0;
    margin: 0;
    background: linear-gradient(#5a4632, #4c3a28);
    border: none;
    border-radius: 0;
    border-right: 2px solid #8a8a92; /* フレットワイヤー */
    cursor: pointer;
  }

  /* 弦(行の中央を走る線。細い弦ほど細く見せたいが行単位では同一でよい) */
  .fret::before {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: 1.5px;
    background: linear-gradient(#d8d4c8, #9a968c);
    pointer-events: none;
  }

  .fret.nut {
    background: #2a2118;
    border-right: 4px solid #d8d4c8; /* ナット */
  }

  .fret:hover {
    filter: brightness(1.35);
  }

  .dot {
    position: absolute;
    inset: 0;
    margin: auto;
    width: 11px;
    height: 11px;
    border-radius: 50%;
    background: var(--accent);
    opacity: 0;
    pointer-events: none;
  }

  .fret:hover .dot {
    opacity: 0.45;
  }

  .fret.hl .dot {
    opacity: 1;
    box-shadow: 0 0 6px var(--accent);
  }
</style>
