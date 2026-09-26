<script lang="ts">
  // ミキサーの右端の「モニター」列: ゴニオメーター(左右の広がりの形)・相関メーター・ラウドネス(LUFS)と True Peak・
  // スペクトル(1/3 オクターブ、傾き付き)・聴き方の切り替え。
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

  // ---- ラウドネス ----
  const loud = $derived(transportStore.state.loudness);
  const fmt = (v: number | null | undefined) => (v === null || v === undefined ? "—" : v.toFixed(1));
  function resetLoudness() {
    api
      .transportResetLoudness()
      .then(() => pollTransport())
      .catch(() => {});
  }

  // ---- スペクトル ----
  let specCanvas = $state<HTMLCanvasElement | undefined>(undefined);
  const SPEC_H = 64;
  /** 傾き(dB/oct)。ミックスのふつうのスペクトルは高域ほど下がるので、持ち上げて平らに見えるように。
   *  帯域ごとのエネルギーに掛けるので 1.5(FFT の密度で表示する解析器の定番の 4.5dB/oct と同じ見え方。
   *  ピンクノイズは帯域ごとのエネルギーが平らで、密度では -3dB/oct になる分の差) */
  const TILT = 1.5;
  let hold: number[] = [];
  function drawSpectrum(bands: number[], db: number[], dt: number) {
    const c = specCanvas;
    const g = c?.getContext("2d");
    if (!c || !g) return;
    const dpr = window.devicePixelRatio || 1;
    if (c.width !== SIZE * dpr) {
      c.width = SIZE * dpr;
      c.height = SPEC_H * dpr;
    }
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, SIZE, SPEC_H);
    const n = db.length;
    const bw = SIZE / n;
    const y = (v: number) => SPEC_H * (1 - Math.max(0, Math.min(1, (v + 60) / 60)));
    // 目盛り(-12 / -24 / -36 / -48 dB)
    g.strokeStyle = "rgba(255,255,255,0.06)";
    for (const v of [-12, -24, -36, -48]) {
      g.beginPath();
      g.moveTo(0, y(v));
      g.lineTo(SIZE, y(v));
      g.stroke();
    }
    if (hold.length !== n) hold = db.map(() => -120);
    for (let i = 0; i < n; i++) {
      const v = db[i] + TILT * Math.log2(bands[i] / 1000);
      hold[i] = Math.max(v, hold[i] - 10 * dt);
      g.fillStyle = "rgba(37,189,177,0.6)";
      g.fillRect(i * bw + 0.5, y(v), bw - 1, SPEC_H - y(v));
      g.fillStyle = "rgba(255,255,255,0.5)";
      g.fillRect(i * bw + 0.5, y(hold[i]), bw - 1, 1);
    }
  }

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
    let last = performance.now();
    const loop = async () => {
      try {
        if (transportStore.state.available) {
          draw(await api.transportScope());
          if (transportStore.state.playing) {
            const sp = await api.transportSpectrum();
            const now = performance.now();
            drawSpectrum(sp.bands, sp.db, (now - last) / 1000);
            last = now;
          }
        }
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
  <div class="cols">
    <div class="col">
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
    </div>
    <div class="col">
      <button
        class="loud"
        title={`マスターのラウドネス(LUFS)と True Peak。M = 瞬時(0.4 秒)、S = 短期(3 秒)、I = 統合(再生を始めてから)、TP = True Peak の最大(サンプルの間の山も含む)。\n配信の目安: I が -14 前後、TP が -1 以下。押すと測り直す`}
        onclick={resetLoudness}
      >
        <span>M</span><b>{fmt(loud?.momentary)}</b>
        <span>S</span><b>{fmt(loud?.short_term)}</b>
        <span>I</span><b class="big">{fmt(loud?.integrated)}</b>
        <span>TP</span><b class:bad={(loud?.true_peak_max ?? -100) > -1}>{fmt(loud?.true_peak_max)}</b>
      </button>
      <canvas
        bind:this={specCanvas}
        class="spec"
        style="width:{SIZE}px;height:{SPEC_H}px"
        title="スペクトル(1/3 オクターブ、25Hz〜20kHz)。高域を持ち上げて表示している(FFT 表示の解析器で定番の 4.5dB/oct と同じ見え方)ので、バランスの良いミックスはおおむね平らに見える。白い線は直近の最大"
      ></canvas>
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
  </div>
</div>

<style>
  .mon {
    width: 262px;
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

  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0 2px;
    padding: 0 2px;
  }

  .col {
    min-width: 0;
    display: flex;
    flex-direction: column;
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

  .loud {
    display: grid;
    grid-template-columns: auto 1fr auto 1fr;
    gap: 1px 5px;
    align-items: baseline;
    margin: 2px 6px 4px;
    padding: 4px 6px;
    background: #0e1a24;
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 10px;
    color: var(--text-faint);
    cursor: pointer;
    text-align: left;
  }

  .loud b {
    color: var(--text);
    font-variant-numeric: tabular-nums;
    text-align: right;
    font-size: 11px;
  }

  .loud b.big {
    color: var(--accent);
  }

  .loud b.bad {
    color: #e06060;
  }

  canvas.spec {
    margin: 0 auto 4px;
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
