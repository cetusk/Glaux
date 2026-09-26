<script lang="ts">
  // ミキサーの右端の「モニター」列: ゴニオメーター(左右の広がりの形)・相関メーター・聴き方の切り替え。
  // 聴き方(モノ・サイド・入替・クロスフィード)は出力デバイスへの音だけに掛かり、書き出しには入らない。
  import { untrack } from "svelte";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { pollTransport, transportStore } from "./transport.svelte";
  import type { MonitorMode } from "./types";

  const MODES: { id: MonitorMode; label: string; title: string }[] = [
    { id: "stereo", label: "ステレオ", title: "そのまま聴く" },
    { id: "mono", label: "モノ", title: "左右を足して聴く(スマホのスピーカーなど、モノラルで鳴らしたときに音が消えないかの確認)" },
    { id: "side", label: "サイド", title: "左右の差だけを聴く(広がり・リバーブ・位相のずれの確認)" },
    { id: "swap", label: "入替", title: "左右を入れ替える(耳や部屋の癖を打ち消して、左右の偏りを確かめる)" },
  ];

  const mode = $derived(transportStore.state.monitor?.mode ?? "stereo");
  const crossfeed = $derived(transportStore.state.monitor?.crossfeed ?? false);
  const corr = $derived(transportStore.state.levels?.correlation ?? null);

  // 相関は表示だけなめらかに(問い合わせの間隔ごとに跳ねないように)
  let shownCorr = $state<number | null>(null);
  $effect(() => {
    void transportStore.seq;
    const c = corr;
    // 前の値は読むだけ(依存にすると自分の書き込みで回り続ける)
    const prev = untrack(() => shownCorr);
    shownCorr = c === null ? null : prev === null ? c : prev + (c - prev) * 0.35;
  });

  function setMode(m: MonitorMode, xf = crossfeed) {
    api
      .transportSetMonitor(m, xf)
      .then(() => pollTransport())
      .catch(() => {});
  }

  // ---- ゴニオメーター ----
  let canvas = $state<HTMLCanvasElement | undefined>(undefined);
  const SIZE = 116;
  $effect(() => {
    const c = canvas;
    if (!c) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let scale = 1;
    const draw = (pts: [number, number][]) => {
      const g = c.getContext("2d");
      if (!g) return;
      const dpr = window.devicePixelRatio || 1;
      if (c.width !== SIZE * dpr) {
        c.width = SIZE * dpr;
        c.height = SIZE * dpr;
      }
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      g.clearRect(0, 0, SIZE, SIZE);
      const h = SIZE / 2;
      // 目盛り: 縦 = 中央(モノ)、斜め = 左だけ・右だけ、横 = 逆相
      g.strokeStyle = "rgba(255,255,255,0.08)";
      g.lineWidth = 1;
      g.beginPath();
      g.moveTo(h, 4);
      g.lineTo(h, SIZE - 4);
      g.moveTo(4, h);
      g.lineTo(SIZE - 4, h);
      g.moveTo(h - (h - 8) * 0.7, h - (h - 8) * 0.7);
      g.lineTo(h + (h - 8) * 0.7, h + (h - 8) * 0.7);
      g.moveTo(h + (h - 8) * 0.7, h - (h - 8) * 0.7);
      g.lineTo(h - (h - 8) * 0.7, h + (h - 8) * 0.7);
      g.stroke();
      g.fillStyle = "rgba(255,255,255,0.25)";
      g.font = "9px sans-serif";
      g.fillText("L", 6, 12);
      g.fillText("R", SIZE - 12, 12);
      // 小さな音でも形が見えるよう、大きさは直近の最大に合わせる(ゆっくり戻す)
      let peak = 0;
      for (const [l, r] of pts) peak = Math.max(peak, Math.abs(l), Math.abs(r));
      const want = 1 / Math.max(peak, 0.02);
      scale = want < scale ? want : scale + (want - scale) * 0.1;
      const k = (h - 6) * scale * Math.SQRT1_2;
      g.fillStyle = "rgba(37,189,177,0.55)";
      for (const [l, r] of pts) {
        const x = h + (r - l) * k;
        const y = h - (l + r) * k;
        g.fillRect(x, y, 1.2, 1.2);
      }
    };
    const loop = async () => {
      try {
        if (transportStore.state.available) draw(await api.transportScope());
      } catch {
        // 起動直後など
      }
      if (stopped) return;
      timer = setTimeout(loop, transportStore.state.playing ? 50 : 400);
    };
    loop();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  });

  const corrPct = $derived(shownCorr === null ? 50 : ((shownCorr + 1) / 2) * 100);
  const corrTone = $derived(shownCorr === null ? "" : shownCorr < 0 ? "bad" : shownCorr < 0.3 ? "warn" : "good");
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<div class="strip mon" role="group" aria-label="モニター" onclick={(e) => e.stopPropagation()}>
  <div class="s-top"></div>
  <div class="s-name"><Icon name="headphones" size={13} /><span>モニター</span></div>
  <canvas
    bind:this={canvas}
    style="width:{SIZE}px;height:{SIZE}px"
    title="ゴニオメーター: 縦に伸びるほど中央(モノラル寄り)、横に広がるほど左右の差が大きい。横に寝るのは逆相(モノで消える)の印"
  ></canvas>
  <div
    class="corr"
    title="相関: +1 = モノラル、0 = 左右が無関係(広い)、マイナス = 逆相(モノラルで再生すると音が消える)。ふつうのミックスは 0〜+1 の間"
  >
    <div class="bar">
      <span class="mid"></span>
      {#if shownCorr !== null}<span class="mark {corrTone}" style="left:{corrPct}%"></span>{/if}
    </div>
    <div class="scale"><span>-1</span><b class={corrTone}>{shownCorr === null ? "—" : shownCorr.toFixed(2)}</b><span>+1</span></div>
  </div>
  <div class="modes" role="radiogroup" aria-label="聴き方">
    {#each MODES as m (m.id)}
      <button
        class="btn sm"
        class:on={mode === m.id}
        class:alt={mode === m.id && m.id !== "stereo"}
        role="radio"
        aria-checked={mode === m.id}
        title={m.title}
        onclick={() => setMode(m.id)}>{m.label}</button
      >
    {/each}
  </div>
  <button
    class="btn sm xf"
    class:on={crossfeed}
    aria-pressed={crossfeed}
    title="クロスフィード: ヘッドホンで聴くときに、左右の極端な分離をやわらげる(スピーカーで聴いたときの広がりに近づける)"
    onclick={() => setMode(mode, !crossfeed)}>クロスフィード</button
  >
  <div class="note">聴き方は書き出しに入りません</div>
</div>

<style>
  .mon {
    width: 132px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden auto;
    scrollbar-width: thin;
    min-height: 0;
  }

  .s-top {
    height: 4px;
    flex-shrink: 0;
    background: var(--text-faint);
  }

  .s-name {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 5px 7px 2px 8px;
    font-weight: 700;
    font-size: var(--fs-md);
    color: var(--text);
  }

  .s-name :global(.icon) {
    color: var(--text-dim);
  }

  canvas {
    display: block;
    margin: 4px auto 2px;
    background: #0e1a24;
    border-radius: 6px;
    flex-shrink: 0;
  }

  .corr {
    padding: 2px 8px 4px;
  }

  .bar {
    position: relative;
    height: 6px;
    border-radius: 3px;
    background: linear-gradient(90deg, #6b2b2b 0%, #4a3a22 50%, #1f4a46 100%);
  }

  .mid {
    position: absolute;
    left: 50%;
    top: -1px;
    bottom: -1px;
    width: 1px;
    background: rgba(255, 255, 255, 0.3);
  }

  .mark {
    position: absolute;
    top: -2px;
    width: 3px;
    height: 10px;
    margin-left: -1.5px;
    border-radius: 1px;
    background: var(--text);
  }

  .mark.good {
    background: var(--accent);
  }

  .mark.warn {
    background: #e0b050;
  }

  .mark.bad {
    background: #e06060;
  }

  .scale {
    display: flex;
    justify-content: space-between;
    font-size: 10px;
    color: var(--text-faint);
    margin-top: 2px;
  }

  .scale b {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }

  .scale b.warn {
    color: #e0b050;
  }

  .scale b.bad {
    color: #e06060;
  }

  .modes {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
    padding: 2px 6px;
  }

  .modes .btn,
  .xf {
    justify-content: center;
    padding-left: 2px;
    padding-right: 2px;
  }

  .modes .btn.alt {
    background: #5a4520;
    border-color: #e0b050;
    color: #ffe2a8;
  }

  .xf {
    margin: 3px 6px 0;
  }

  .note {
    font-size: 10px;
    color: var(--text-faint);
    padding: 4px 8px 6px;
    line-height: 1.3;
  }
</style>
