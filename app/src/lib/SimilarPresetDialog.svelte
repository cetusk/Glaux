<script lang="ts">
  import Icon from "./Icon.svelte";
  // 音声クリップの音に近い CLAP 音源のプリセットを探し、読み込んで、つまみを自動で詰める。
  // (MCP の find_similar_presets → load_plugin_preset → refine_plugin_params を人間が UI から行う版)
  import { onDestroy } from "svelte";
  import * as api from "./api";
  import type { Clip, Project } from "./types";

  let {
    project,
    clip,
    onClose,
  }: {
    project: Project;
    clip: Clip;
    onClose: () => void;
  } = $props();

  const clapTracks = $derived(
    project.tracks.filter((t) => t.kind === "midi" && t.device?.type === "clap"),
  );
  let trackId = $state<string | null>(null);
  $effect(() => {
    if (trackId === null || !clapTracks.some((t) => t.id === trackId)) {
      trackId = clapTracks[0]?.id ?? null;
    }
  });

  // 選んだトラックのプリセットのカテゴリ(絞り込み用)
  let categories = $state<{ name: string; count: number }[]>([]);
  let presetTotal = $state(0);
  let category = $state("");
  $effect(() => {
    const id = trackId;
    categories = [];
    presetTotal = 0;
    category = "";
    if (!id) return;
    api
      .clapPresets(id)
      .then((r) => {
        if (trackId !== id) return;
        categories = r.categories;
        presetTotal = r.total;
      })
      .catch(() => {});
  });

  let searching = $state(false);
  let progress = $state<api.PresetIndexProgress | null>(null);
  let results = $state<api.SimilarPreset[] | null>(null);
  let note = $state<string | null>(null);
  let clapNote = $state<string | null>(null);
  let error = $state<string | null>(null);
  let loadedId = $state<string | null>(null);
  let loadingId = $state<string | null>(null);
  let refining = $state(false);
  let refined = $state<Awaited<ReturnType<typeof api.refineClapParams>> | null>(null);

  let unlisten: (() => void) | null = null;
  api
    .onPresetIndex((p) => {
      if (searching) progress = p;
    })
    .then((u) => (unlisten = u))
    .catch(() => {});
  onDestroy(() => unlisten?.());

  const busy = $derived(searching || refining || loadingId !== null);

  async function search() {
    if (!trackId || busy) return;
    searching = true;
    progress = null;
    error = null;
    refined = null;
    try {
      const r = await api.findSimilarClapPresets(clip.id, trackId, category || null);
      results = r.results;
      note = r.note ?? null;
      clapNote = r.clap_note ?? null;
      progress = r.index;
    } catch (e) {
      error = String(e);
    } finally {
      searching = false;
    }
  }

  async function load(p: api.SimilarPreset) {
    if (!trackId || busy) return;
    loadingId = p.id;
    error = null;
    refined = null;
    try {
      await api.clapLoadPreset(trackId, p.id);
      loadedId = p.id;
    } catch (e) {
      error = String(e);
    } finally {
      loadingId = null;
    }
  }

  async function refine() {
    if (!trackId || busy) return;
    refining = true;
    error = null;
    try {
      refined = await api.refineClapParams(clip.id, trackId);
    } catch (e) {
      error = String(e);
    } finally {
      refining = false;
    }
  }

  function verdictClass(d: number): string {
    return d < 0.35 ? "good" : d < 0.7 ? "fair" : "far";
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && !busy && onClose()} />
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="backdrop" role="presentation" onclick={() => !busy && onClose()}></div>
<div class="panel" role="dialog" aria-label="似た音のプリセットを探す">
  <div class="head">
    <h2><Icon name="search" />似た音のプリセットを探す</h2>
    <button class="btn sm icon ghost" onclick={onClose} disabled={busy} title="閉じる(Esc)" aria-label="閉じる"><Icon name="x" /></button>
  </div>
  <div class="hint">
    目標の音: <b>{clip.name}</b>。CLAP 音源(Surge XT など)のプリセットを 1 音ずつ鳴らして比べ、近い順に並べます。
    単音のサンプル(1 音だけ鳴っている音)ほどよく合います。
  </div>

  {#if clapTracks.length === 0}
    <div class="warn">
      CLAP 音源のトラックがありません。MIDI トラックの音源メニューから Surge XT などの CLAP プラグインを選んでから開いてください。
    </div>
  {:else}
    <div class="row">
      <label>
        <span>探す音源</span>
        <select bind:value={trackId} disabled={busy}>
          {#each clapTracks as t (t.id)}
            <option value={t.id}>{t.name}</option>
          {/each}
        </select>
      </label>
      <label>
        <span>カテゴリ</span>
        <select bind:value={category} disabled={busy}>
          <option value="">すべて{presetTotal ? `(${presetTotal})` : ""}</option>
          {#each categories as c (c.name)}
            <option value={c.name}>{c.name}({c.count})</option>
          {/each}
        </select>
      </label>
    </div>
    <div class="row">
      <button class="primary" onclick={search} disabled={busy || !trackId}>
        {searching ? "探しています…" : results ? "もう一度探す(索引の続きも作る)" : "探す"}
      </button>
    </div>
    <div class="hint">
      初回はプリセットを鳴らした索引を作るので時間がかかります(1 回 90 秒まで。残りは次に押したときに続きを作ります)。
      カテゴリで絞ると速くなります。
    </div>

    {#if searching && progress}
      <div class="progress">
        <div class="bar" style="width:{progress.total ? (progress.indexed / progress.total) * 100 : 0}%"></div>
        <span>索引 {progress.indexed} / {progress.total}</span>
      </div>
    {/if}

    {#if note}<div class="note">{note}</div>{/if}
    {#if clapNote}<div class="note">{clapNote}</div>{/if}

    {#if results}
      {#if results.length === 0}
        <div class="note">候補が見つかりませんでした(索引がまだ無いか、どのプリセットも鳴りませんでした)。</div>
      {:else}
        <ul class="results">
          {#each results as r (r.id)}
            <li class:loaded={loadedId === r.id}>
              <div class="name">
                <span>{r.name}</span>
                <span class="cat">{r.category}</span>
              </div>
              <span class="verdict {verdictClass(r.distance)}" title="距離 {r.distance.toFixed(2)}(0.15 未満 ほぼ同じ … 0.7 以上 かなり違う)">
                {r.verdict}
              </span>
              <button onclick={() => load(r)} disabled={busy}>
                {loadingId === r.id ? "読み込み中…" : loadedId === r.id ? "読み込み済み" : "読み込む"}
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    {/if}

    <div class="section">
      <div class="section-title">つまみを詰める</div>
      <div class="hint">
        今の音色(読み込んだプリセット)から出発して、フィルターやエンベロープなどの主要なつまみを目標の音に近づけます(約 20 秒・Ctrl+Z で戻せます)。
      </div>
      <button onclick={refine} disabled={busy || !trackId}>
        {#if !refining}<Icon name="wand-sparkles" />{/if}{refining ? "合わせています…" : "つまみを自動で合わせる"}
      </button>
      {#if refined}
        <div class="refined">
          {#if refined.changed.length === 0}
            今の値のままが最も近かったため、つまみは変えませんでした。
          {:else}
            距離 {refined.initial_distance.toFixed(2)} → {refined.distance.toFixed(2)}({refined.verdict})。
            変えたつまみ: {refined.changed.map((c) => c.name).join("、")}
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  {#if error}<div class="warn">{error}</div>{/if}
</div>

<style>
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
    left: 50%;
    transform: translateX(-50%);
    width: min(520px, calc(100vw - 32px));
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.55);
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: var(--fs-lg);
  }

  h2 :global(.icon) {
    color: var(--accent);
  }

  .row {
    display: flex;
    gap: 10px;
    align-items: flex-end;
  }

  label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex: 1;
    font-size: 12px;
    color: var(--text-dim);
  }

  select {
    width: 100%;
  }

  .primary {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .hint {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }

  .note {
    font-size: 12px;
    color: var(--text-dim);
    border-left: 2px solid var(--border);
    padding-left: 8px;
  }

  .warn {
    color: var(--warn);
    font-size: 12px;
  }

  .progress {
    position: relative;
    height: 18px;
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
    font-size: 11px;
  }

  .progress .bar {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--accent-dim);
    opacity: 0.5;
  }

  .progress span {
    position: relative;
    padding-left: 6px;
    line-height: 18px;
  }

  .results {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .results li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .results li.loaded {
    border-color: var(--accent-dim);
  }

  .name {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .name span:first-child {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .cat {
    font-size: 11px;
    color: var(--text-dim);
  }

  .verdict {
    font-size: 11px;
    white-space: nowrap;
  }

  .verdict.good {
    color: var(--accent);
  }

  .verdict.fair {
    color: var(--text);
  }

  .verdict.far {
    color: var(--text-dim);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border-top: 1px solid var(--border);
    padding-top: 10px;
  }

  .section-title {
    font-size: 12px;
    font-weight: 600;
  }

  .refined {
    font-size: 12px;
  }
</style>
