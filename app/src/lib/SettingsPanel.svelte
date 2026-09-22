<script lang="ts">
  import {
    ACCENT_PRESETS,
    applyTheme,
    playDoneChime,
    saveSettings,
    settings,
  } from "./settings.svelte";

  let { onClose }: { onClose: () => void } = $props();

  function pickAccent(name: string) {
    settings.accent = name;
    applyTheme();
    saveSettings();
  }

  function toggleNotify(e: Event) {
    settings.notifyOnAiDone = (e.currentTarget as HTMLInputElement).checked;
    saveSettings();
    if (settings.notifyOnAiDone) playDoneChime(); // 試聴を兼ねる
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onClose}></div>
<div class="panel">
  <div class="head">
    <h2>⚙ 設定</h2>
    <button onclick={onClose} title="閉じる">✕</button>
  </div>

  <div class="section">
    <div class="section-title">テーマカラー</div>
    <div class="swatches">
      {#each ACCENT_PRESETS as p (p.name)}
        <button
          class="swatch"
          class:active={settings.accent === p.name}
          style="--sw:{p.accent}"
          onclick={() => pickAccent(p.name)}
          title={p.label}
        >
          <span class="dot"></span>
          <span class="sw-label">{p.label}</span>
        </button>
      {/each}
    </div>
  </div>

  <div class="section">
    <div class="section-title">通知</div>
    <label class="row">
      <input
        type="checkbox"
        checked={settings.notifyOnAiDone}
        onchange={toggleNotify}
      />
      AI の作業完了時にチャイムを鳴らす
    </label>
  </div>

  <div class="section">
    <div class="section-title">録音</div>
    <label class="row">
      カウントイン
      <select
        value={String(settings.countInBars)}
        onchange={(e) => {
          settings.countInBars = Number((e.currentTarget as HTMLSelectElement).value);
          saveSettings();
        }}
      >
        <option value="0">なし</option>
        <option value="1">1 小節</option>
        <option value="2">2 小節</option>
      </select>
    </label>
    <label class="row">
      <input
        type="checkbox"
        checked={settings.metronomeOnRecord}
        onchange={(e) => {
          settings.metronomeOnRecord = (e.currentTarget as HTMLInputElement).checked;
          saveSettings();
        }}
      />
      録音中はメトロノームを鳴らす
    </label>
    <label class="row">
      レイテンシ補正
      <input
        class="num"
        type="number"
        min="0"
        max="500"
        step="5"
        value={settings.recordLatencyMs}
        onchange={(e) => {
          settings.recordLatencyMs = Math.max(0, Number((e.currentTarget as HTMLInputElement).value) || 0);
          saveSettings();
        }}
      />
      ms
    </label>
    <div class="hint">
      録音が拍より遅れて置かれるなら値を増やし、早すぎるなら減らします(目安: 出力デバイスの遅延 20〜100ms)。
    </div>
  </div>

  <div class="note">設定はこの PC に保存されます(プロジェクトには含まれません)。</div>
</div>

<style>
  .num {
    width: 64px;
    margin: 0 4px 0 8px;
  }

  .hint {
    font-size: 11px;
    color: var(--text-dim);
    margin-top: 4px;
    line-height: 1.5;
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 29;
    background: rgba(0, 0, 0, 0.45);
  }

  .panel {
    position: fixed;
    z-index: 30;
    top: 60px;
    right: 16px;
    width: 340px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.55);
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  h2 {
    margin: 0;
    font-size: 15px;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .section-title {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .swatches {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }

  .swatch {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 6px 8px;
  }

  .swatch.active {
    border-color: var(--sw);
    color: var(--text);
  }

  .dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--sw);
    flex-shrink: 0;
  }

  .sw-label {
    font-size: 11px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    cursor: pointer;
  }

  .note {
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
