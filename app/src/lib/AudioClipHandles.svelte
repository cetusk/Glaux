<script lang="ts">
  // 音声クリップの上のつまみ: フェード(下の角。暗い三角で長さを見せる)・クリップの音量(下の中央)と、
  // 譜起こしのボタン。ドラッグ中は試聴だけ(previewEdit)して、離したときに 1 回の編集にする
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { bpmAt } from "./timelineOps";
  import type { Clip, Project } from "./types";

  let {
    project,
    clip,
    pxPerTick,
    transcribing,
    onTranscribe,
  }: {
    project: Project;
    clip: Clip;
    pxPerTick: number;
    /// 譜起こし中のクリップ(どれかが譜起こし中ならボタンを押せなくする)
    transcribing: string | null;
    onTranscribe: (mode: "melody" | "poly") => void;
  } = $props();

  /// ドラッグ中のフェード・音量(離すまで表示と試聴だけ)
  let clipHandle = $state<{
    clip: Clip;
    which: "in" | "out" | "gain";
    startX: number;
    startY: number;
    orig: number;
    value: number;
  } | null>(null);

  /// `tick` の位置の 1 tick のミリ秒
  function msPerTick(tick: number): number {
    return 60000 / (bpmAt(project, tick) * project.ppq);
  }

  function clipMs(c: Clip): number {
    return c.length * msPerTick(c.start);
  }

  function fadeMs(c: Clip, which: "in" | "out"): number {
    if (c.kind !== "audio") return 0;
    if (clipHandle?.clip.id === c.id && clipHandle.which === which) return clipHandle.value;
    return (which === "in" ? c.fade_in_ms : c.fade_out_ms) ?? 0;
  }

  function clipGain(c: Clip): number {
    if (c.kind !== "audio") return 0;
    if (clipHandle?.clip.id === c.id && clipHandle.which === "gain") return clipHandle.value;
    return c.gain_db ?? 0;
  }

  function fadePx(c: Clip, which: "in" | "out"): number {
    return (fadeMs(c, which) / msPerTick(which === "in" ? c.start : c.start + c.length)) * pxPerTick;
  }

  function handleCommand(h: NonNullable<typeof clipHandle>): unknown {
    const key = h.which === "in" ? "fade_in_ms" : h.which === "out" ? "fade_out_ms" : "gain_db";
    return { op: "replace_clip", id: h.clip.id, clip: { ...h.clip, [key]: h.value } };
  }

  function onHandleDown(e: PointerEvent, which: "in" | "out" | "gain") {
    if (e.button !== 0 || clip.kind !== "audio") return;
    e.stopPropagation();
    const orig = which === "gain" ? (clip.gain_db ?? 0) : ((which === "in" ? clip.fade_in_ms : clip.fade_out_ms) ?? 0);
    clipHandle = { clip, which, startX: e.clientX, startY: e.clientY, orig, value: orig };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onHandleMove(e: PointerEvent) {
    const h = clipHandle;
    if (!h) return;
    let v: number;
    if (h.which === "gain") {
      // 上へ 1px = +0.1dB(Shift で細かく)。-36〜+12 dB
      const per = e.shiftKey ? 0.02 : 0.1;
      v = Math.round((h.orig - (e.clientY - h.startY) * per) * 10) / 10;
      v = Math.min(12, Math.max(-36, v));
    } else {
      const other = fadeMs(h.clip, h.which === "in" ? "out" : "in");
      const dx = (e.clientX - h.startX) * (h.which === "in" ? 1 : -1);
      const tickAt = h.which === "in" ? h.clip.start : h.clip.start + h.clip.length;
      v = h.orig + (dx / pxPerTick) * msPerTick(tickAt);
      // フェードイン + アウトはクリップの長さまで
      v = Math.round(Math.min(Math.max(0, clipMs(h.clip) - other), Math.max(0, v)));
    }
    if (v === h.value) return;
    h.value = v;
    api.previewEdit([handleCommand(h)]);
  }

  function onHandleUp() {
    const h = clipHandle;
    if (!h) return;
    clipHandle = null;
    if (h.value === h.orig) return;
    const label =
      h.which === "gain"
        ? `${h.clip.name} の音量を ${h.value > 0 ? "+" : ""}${h.value.toFixed(1)} dB に`
        : `${h.clip.name} のフェード${h.which === "in" ? "イン" : "アウト"}を ${Math.round(h.value)}ms に`;
    api.applyEdit([handleCommand(h)], label).catch(() => {});
  }

  /** つまみのダブルクリックで元に戻す(フェードなし / 0 dB) */
  function resetHandle(which: "in" | "out" | "gain") {
    if (clip.kind !== "audio") return;
    const key = which === "in" ? "fade_in_ms" : which === "out" ? "fade_out_ms" : "gain_db";
    if ((clip[key] ?? 0) === 0) return;
    const what = which === "gain" ? "音量を 0 dB に戻す" : `フェード${which === "in" ? "イン" : "アウト"}をなくす`;
    api.applyEdit([{ op: "replace_clip", id: clip.id, clip: { ...clip, [key]: 0 } }], `${clip.name} の${what}`).catch(() => {});
  }

  const fiPx = $derived(Math.min(fadePx(clip, "in"), clip.length * pxPerTick));
  const foPx = $derived(Math.min(fadePx(clip, "out"), clip.length * pxPerTick));
  const w = $derived(Math.max(clip.length * pxPerTick, 8));
  const gain = $derived(clipGain(clip));
</script>

{#if fiPx > 0 || foPx > 0}
  <svg class="fade-shade" width={w} height="100%" viewBox="0 0 {w} 100" preserveAspectRatio="none" aria-hidden="true">
    {#if fiPx > 0}<polygon points="0,0 {fiPx},0 0,100" />{/if}
    {#if foPx > 0}<polygon points="{w - foPx},0 {w},0 {w},100" />{/if}
  </svg>
{/if}
<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  class="fade-h in"
  class:active={clipHandle?.clip.id === clip.id && clipHandle.which === "in"}
  style="left:{fiPx}px"
  title="フェードイン {Math.round(fadeMs(clip, 'in'))}ms(横にドラッグ。ダブルクリックでなくす)"
  onpointerdown={(e) => onHandleDown(e, "in")}
  onpointermove={onHandleMove}
  onpointerup={onHandleUp}
  onpointercancel={() => (clipHandle = null)}
  ondblclick={(e) => {
    e.stopPropagation();
    resetHandle("in");
  }}
></span>
<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  class="fade-h out"
  class:active={clipHandle?.clip.id === clip.id && clipHandle.which === "out"}
  style="right:{foPx}px"
  title="フェードアウト {Math.round(fadeMs(clip, 'out'))}ms(横にドラッグ。ダブルクリックでなくす)"
  onpointerdown={(e) => onHandleDown(e, "out")}
  onpointermove={onHandleMove}
  onpointerup={onHandleUp}
  onpointercancel={() => (clipHandle = null)}
  ondblclick={(e) => {
    e.stopPropagation();
    resetHandle("out");
  }}
></span>
<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  class="gain-h"
  class:active={clipHandle?.clip.id === clip.id && clipHandle.which === "gain"}
  class:set={gain !== 0}
  title="クリップの音量(上下にドラッグ、Shift で細かく。ダブルクリックで 0 dB)"
  onpointerdown={(e) => onHandleDown(e, "gain")}
  onpointermove={onHandleMove}
  onpointerup={onHandleUp}
  onpointercancel={() => (clipHandle = null)}
  ondblclick={(e) => {
    e.stopPropagation();
    resetHandle("gain");
  }}>{gain > 0 ? "+" : ""}{gain.toFixed(1)} dB</span
>
<button
  class="transcribe"
  disabled={transcribing !== null}
  title="譜起こし(単旋律): 鼻歌・歌・単音の音声を MIDI クリップにする"
  onpointerdown={(e) => e.stopPropagation()}
  ondblclick={(e) => e.stopPropagation()}
  onclick={(e) => {
    e.stopPropagation();
    onTranscribe("melody");
  }}
>
  {#if transcribing === clip.id}<Icon name="loader-circle" size={12} />{:else}<Icon name="music" size={12} />{/if}
</button>
<button
  class="transcribe poly"
  disabled={transcribing !== null}
  title="譜起こし(和音): ピアノ・ギターのコードや伴奏入りの音声を MIDI クリップにする(学習済みモデル basic-pitch)"
  onpointerdown={(e) => e.stopPropagation()}
  ondblclick={(e) => e.stopPropagation()}
  onclick={(e) => {
    e.stopPropagation();
    onTranscribe("poly");
  }}
>
  <Icon name="list-music" size={12} />
</button>

<style>
  /* 音声クリップのフェード(暗い三角)と、そのつまみ・音量のつまみ */
  .fade-shade {
    position: absolute;
    left: 0;
    top: 0;
    height: 100%;
    pointer-events: none;
    z-index: 1;
  }

  .fade-shade polygon {
    fill: rgba(0, 0, 0, 0.4);
  }

  .fade-h {
    position: absolute;
    bottom: 1px;
    width: 9px;
    height: 9px;
    margin-left: -1px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.85);
    border: 1px solid rgba(0, 0, 0, 0.45);
    cursor: ew-resize;
    z-index: 3;
    opacity: 0;
    transition: opacity 0.1s;
  }

  .fade-h.out {
    margin-right: -1px;
  }

  .gain-h {
    position: absolute;
    bottom: 1px;
    left: 50%;
    transform: translateX(-50%);
    padding: 0 4px;
    border-radius: 3px;
    font-size: 10px;
    line-height: 13px;
    white-space: nowrap;
    color: #fff;
    background: rgba(0, 0, 0, 0.5);
    cursor: ns-resize;
    z-index: 3;
    opacity: 0;
    transition: opacity 0.1s;
  }

  /* つまみはクリップにマウスを乗せたときだけ出す(フェード・音量を変えてあれば薄く見せたまま)。
     クリップの枠はタイムラインのものなので :global */
  :global(.clip:hover) .fade-h,
  :global(.clip:hover) .gain-h,
  .fade-h.active,
  .gain-h.active {
    opacity: 1;
  }

  .gain-h.set {
    opacity: 0.7;
  }

  .transcribe.poly {
    right: 32px;
  }

  .transcribe {
    position: absolute;
    top: 2px;
    right: 10px;
    z-index: 3;
    display: inline-flex;
    font-size: 11px;
    line-height: 1;
    padding: 2px 3px;
    border-radius: 4px;
    border: 1px solid rgba(255, 255, 255, 0.35);
    background: rgba(0, 0, 0, 0.35);
    color: #fff;
    cursor: pointer;
  }
</style>
