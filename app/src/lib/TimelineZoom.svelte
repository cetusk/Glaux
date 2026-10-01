<script lang="ts">
  // ルーラーの左(見出しの列)の表示の操作: 横の拡大・縮小・全体表示と、トラックの高さ・見出しの幅のポップアップ
  import Icon from "./Icon.svelte";
  import {
    layoutMax,
    saveTimelineLayout,
    TIMELINE_HEAD_W,
    TIMELINE_TRACK_H,
    timelineLayout,
    timelineZoom,
    TIMELINE_ZOOM_MAX,
    TIMELINE_ZOOM_MIN,
  } from "./selection.svelte";

  let {
    headW,
    trackH,
    onZoomBy,
    onFit,
    onSetZoom,
  }: {
    /// 今の見出しの幅・既定のトラックの高さ(ウィンドウに合わせて抑えた値)
    headW: number;
    trackH: number;
    onZoomBy: (factor: number) => void;
    onFit: () => void;
    onSetZoom: (zoom: number) => void;
  } = $props();

  let layoutMenu = $state(false);
</script>

<div class="zoom" title="横の拡大・縮小(タイムラインの上で Ctrl+ホイールでも)">
  <button class="btn sm icon-only" onclick={() => onZoomBy(1 / 1.5)} disabled={timelineZoom.value <= TIMELINE_ZOOM_MIN} aria-label="縮小" title="縮小"
    ><Icon name="zoom-out" size={13} /></button
  >
  <button class="btn sm icon-only" onclick={() => onZoomBy(1.5)} disabled={timelineZoom.value >= TIMELINE_ZOOM_MAX} aria-label="拡大" title="拡大"
    ><Icon name="zoom-in" size={13} /></button
  >
  <button class="btn sm" onclick={onFit} title="曲全体が横に収まるようにする">全体</button>
  <button
    class="btn sm icon-only"
    class:on={layoutMenu}
    onclick={() => (layoutMenu = !layoutMenu)}
    aria-label="トラックの表示の大きさ"
    title="トラックの高さと見出しの幅"><Icon name="rows-2" size={13} /></button
  >
  {#if Math.abs(timelineZoom.value - 1) > 0.01}
    <button class="btn sm" onclick={() => onSetZoom(1)} title="元の拡大率(100%)に戻す">{Math.round(timelineZoom.value * 100)}%</button>
  {/if}
</div>
{#if layoutMenu}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="menu-backdrop" role="presentation" onclick={() => (layoutMenu = false)}></div>
  <div class="layout-pop">
    <label>
      <span>トラックの高さ(全部) <b>{trackH}px</b></span>
      <input
        type="range"
        min={TIMELINE_TRACK_H.min}
        max={layoutMax("h")}
        step="2"
        value={trackH}
        oninput={(e) => {
          // 全部そろえる(見出しの下の端で個別に変えた高さも、この高さにする)
          timelineLayout.trackH = Number(e.currentTarget.value);
          timelineLayout.trackHeights = {};
        }}
        onchange={saveTimelineLayout}
        aria-label="トラックの高さ"
      />
    </label>
    <label>
      <span>見出しの幅 <b>{headW}px</b></span>
      <input
        type="range"
        min={TIMELINE_HEAD_W.min}
        max={layoutMax("w")}
        step="4"
        value={headW}
        oninput={(e) => (timelineLayout.headW = Number(e.currentTarget.value))}
        onchange={saveTimelineLayout}
        aria-label="見出しの幅"
      />
    </label>
    <button
      class="btn sm"
      onclick={() => {
        timelineLayout.trackH = TIMELINE_TRACK_H.def;
        timelineLayout.trackHeights = {};
        timelineLayout.headW = TIMELINE_HEAD_W.def;
        saveTimelineLayout();
      }}>元に戻す</button
    >
  </div>
{/if}

<style>
  .zoom {
    display: flex;
    align-items: center;
    gap: 3px;
    height: 100%;
    padding: 0 6px;
  }

  .zoom .btn {
    height: 18px;
    padding: 0 5px;
    font-size: var(--fs-xs);
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .layout-pop {
    position: absolute;
    top: 26px;
    left: 6px;
    z-index: 30;
    width: 220px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 12px;
    background: var(--bg-raised);
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    font-size: var(--fs-sm);
  }

  .layout-pop label {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .layout-pop input {
    width: 100%;
    accent-color: var(--accent);
  }

  .layout-pop .btn {
    align-self: flex-end;
  }
</style>
