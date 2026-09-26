<script lang="ts">
  // エフェクトのプリセットの棚(ミキサーの下の段の右端。たためる)。エフェクト 1 つ分を名前・メモ付きで取っておき、
  // どのトラック・どの曲でも使う。
  // カードはノード表示へドラッグして置ける(線の上なら間に入り、それ以外は、つながずに置く)。「+」は出口の前へ。
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { focusNow } from "./menu";
  import { FX_COLORS, FX_KIND_JA, fxName } from "./fx";
  import { fxDrag, fxDropTargets } from "./fxDrag.svelte";
  import { showError, showToast } from "./toast.svelte";
  import type { EffectView, Project } from "./types";

  let { project, targetId }: { project: Project; targetId: string } = $props();

  function loadOpen(): boolean {
    try {
      return localStorage.getItem("glaux.fxShelf") !== "0";
    } catch {
      return true;
    }
  }
  let open = $state(loadOpen());
  function toggle() {
    open = !open;
    try {
      localStorage.setItem("glaux.fxShelf", open ? "1" : "0");
    } catch {
      // 覚えられなくても動作には関係しない
    }
  }

  // 一覧(AI が保存した分も拾えるよう、プロジェクトが変わるたびに読み直す。ファイルを並べるだけなので軽い)
  let presets = $state<api.FxPresetInfo[]>([]);
  function reload() {
    api
      .listFxPresets()
      .then((r) => (presets = r.presets))
      .catch(() => {});
  }
  $effect(() => {
    void project;
    reload();
  });

  let query = $state("");
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return presets;
    return presets.filter((p) =>
      [p.name, p.note ?? "", p.origin ?? "", p.kind, FX_KIND_JA[p.kind] ?? ""].some((s) => s.toLowerCase().includes(q)),
    );
  });

  const kindLabel = (p: api.FxPresetInfo) => (p.kind === "clap" ? (p.plugin_id?.split(".").pop() ?? "CLAP") : (FX_KIND_JA[p.kind] ?? p.kind));
  const colorOf = (p: api.FxPresetInfo) => FX_COLORS[p.kind] ?? "#777";

  async function add(p: api.FxPresetInfo, parked: boolean) {
    try {
      await api.applyFxPreset(targetId, p.name, { parked });
    } catch (e) {
      showError("エフェクトを足せませんでした", e);
    }
  }

  let confirmDelete = $state<string | null>(null);
  async function remove(p: api.FxPresetInfo) {
    if (confirmDelete !== p.name) {
      confirmDelete = p.name;
      return;
    }
    confirmDelete = null;
    try {
      await api.deleteFxPreset(p.name);
      reload();
    } catch (e) {
      showError("削除できませんでした", e);
    }
  }

  // ---- ノード表示へ運ぶ(pointer で自前に。HTML5 の DnD は Tauri の Windows 版では動かない) ----
  function onCardDown(ev: PointerEvent, p: api.FxPresetInfo) {
    if (ev.button !== 0 || (ev.target as HTMLElement).closest("button")) return;
    ev.preventDefault();
    const x0 = ev.clientX;
    const y0 = ev.clientY;
    let started = false;
    const move = (m: PointerEvent) => {
      if (!started && Math.abs(m.clientX - x0) + Math.abs(m.clientY - y0) < 5) return;
      started = true;
      fxDrag.preset = { name: p.name, kind: p.kind };
      fxDrag.x = m.clientX;
      fxDrag.y = m.clientY;
    };
    const up = (u: PointerEvent) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      if (!started) return;
      fxDrag.preset = null;
      fxDropTargets.canvas?.(p.name, u.clientX, u.clientY);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // 棚の要素を受け口として知らせる(ノード表示のカードを落とすと保存)
  let rootEl = $state<HTMLDivElement | undefined>(undefined);
  $effect(() => {
    const el = rootEl ?? null;
    fxDropTargets.shelf = el;
    return () => {
      if (fxDropTargets.shelf === el) fxDropTargets.shelf = null;
    };
  });

  // ---- 保存(ノード表示のカードの「エフェクトのプリセットに保存」から) ----
  let saving = $state<{ target: string; fx: EffectView; name: string; note: string; exists: boolean } | null>(null);

  /** 保存の入力欄を開く(名前はカードの表示名、メモはカードのメモを下書きに) */
  export function saveFrom(target: string, fx: EffectView) {
    if (!open) toggle();
    saving = { target, fx, name: fxName(fx), note: fx.note ?? "", exists: false };
  }

  async function commitSave(overwrite = false) {
    const s = saving;
    if (!s) return;
    const name = s.name.trim();
    if (!name) return;
    if (!overwrite && presets.some((p) => p.name === name)) {
      saving = { ...s, exists: true };
      return;
    }
    try {
      await api.saveFxPreset(s.target, s.fx.id, name, s.note.trim() || null, overwrite);
      saving = null;
      showToast("ok", `エフェクトのプリセット「${name}」を保存しました`);
      reload();
    } catch (e) {
      showError("保存できませんでした", e);
    }
  }
</script>

<div class="shelf" class:open class:drop={fxDrag.overShelf} bind:this={rootEl}>
  {#if fxDrag.overShelf}
    <div class="drop-msg"><Icon name="archive" />離すとエフェクトのプリセットに保存</div>
  {/if}
  {#if !open}
    <button class="rail" onclick={toggle} title="エフェクトのプリセットを開く" aria-expanded="false">
      <Icon name="archive" /><span>エフェクトのプリセット</span><span class="count">{presets.length}</span>
    </button>
  {:else}
    <div class="head">
      <Icon name="archive" /><b>エフェクトのプリセット</b><span class="count">{presets.length}</span>
      <span class="sp"></span>
      <button class="btn sm icon ghost" onclick={toggle} aria-expanded="true" title="たたむ" aria-label="たたむ"><Icon name="chevron-right" /></button>
    </div>
    <input class="search" type="search" placeholder="名前・メモ・種類で探す" bind:value={query} aria-label="エフェクトのプリセットを探す" />

    {#if saving}
      <div class="save">
        <span class="dim">{fxName(saving.fx)} を保存</span>
        <input
          use:focusNow
          placeholder="名前"
          bind:value={saving.name}
          oninput={() => saving && (saving.exists = false)}
          onkeydown={(e) => {
            if (e.isComposing) return;
            if (e.key === "Enter") commitSave();
            else if (e.key === "Escape") saving = null;
          }}
        />
        <textarea
          rows="2"
          placeholder="メモ(どんな音か・何に使うか)"
          bind:value={saving.note}
          onkeydown={(e) => {
            if (e.isComposing) return;
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) commitSave();
            else if (e.key === "Escape") saving = null;
          }}
        ></textarea>
        <div class="save-row">
          {#if saving.exists}
            <span class="warn">同じ名前があります</span>
            <button class="btn sm danger" onclick={() => commitSave(true)}>上書き</button>
          {:else}
            <button class="btn sm primary" onclick={() => commitSave()} disabled={!saving.name.trim()}>保存</button>
          {/if}
          <button class="btn sm ghost" onclick={() => (saving = null)}>やめる</button>
        </div>
      </div>
    {/if}

    <div class="list" role="list">
      {#each shown as p (p.name)}
        <div class="card" style="--nc:{colorOf(p)}" onpointerdown={(e) => onCardDown(e, p)} role="listitem" title={p.note ?? p.name}>
          <div class="top">
            <span class="kind">{kindLabel(p)}</span>
            <span class="sp"></span>
            <button class="btn sm icon ghost" onclick={() => add(p, false)} title="線の最後に足す" aria-label="線の最後に足す"><Icon name="plus" size={13} /></button>
            <button class="btn sm icon ghost" onclick={() => add(p, true)} title="つながずに置く(線を引くまで鳴らない)" aria-label="つながずに置く"><Icon name="unplug" size={13} /></button>
            <button
              class="btn sm icon ghost"
              class:armed={confirmDelete === p.name}
              onclick={() => remove(p)}
              onblur={() => confirmDelete === p.name && (confirmDelete = null)}
              title={confirmDelete === p.name ? "もう一度押すと削除(元に戻せません)" : "削除"}
              aria-label="削除"><Icon name="trash-2" size={13} /></button
            >
          </div>
          <b>{p.name}</b>
          {#if p.note}<span class="pnote">{p.note}</span>{/if}
          {#if p.origin}<span class="origin">{p.origin} から</span>{/if}
        </div>
      {:else}
        <div class="empty">
          {#if presets.length === 0}
            まだありません。左のカードをここへドラッグするか、カードの「…」→「エフェクトのプリセットに保存」で入れられます。どのトラック・曲でも使えます
          {:else}
            見つかりません
          {/if}
        </div>
      {/each}
      {#if shown.length > 0}
        <div class="hint">左へドラッグして置く(線の上なら間に入る)。左のカードをここへ落とすと保存。どのトラック・曲でも使える</div>
      {/if}
    </div>
  {/if}
</div>

{#if fxDrag.preset}
  <div class="float" style="left:{fxDrag.x + 10}px;top:{fxDrag.y + 10}px;--nc:{FX_COLORS[fxDrag.preset.kind] ?? '#777'}">
    <Icon name="archive" size={13} />{fxDrag.preset.name}
  </div>
{/if}

<style>
  .shelf {
    flex-shrink: 0;
    width: 32px;
    border-left: 1px solid var(--border);
    background: var(--bg-panel);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .shelf.open {
    width: 224px;
  }

  .shelf {
    position: relative;
  }

  .shelf.drop {
    outline: 2px dashed var(--accent);
    outline-offset: -3px;
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-panel));
  }

  .drop-msg {
    position: absolute;
    left: 8px;
    right: 8px;
    top: 40%;
    z-index: 3;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 12px 6px;
    border-radius: var(--r-md);
    background: rgba(10, 30, 28, 0.92);
    color: var(--accent);
    font-size: var(--fs-sm);
    text-align: center;
    pointer-events: none;
  }

  .float {
    position: fixed;
    z-index: 50;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    border-radius: 8px;
    border: 1px solid var(--nc);
    background: var(--bg-raised);
    color: var(--text);
    font-size: var(--fs-sm);
    box-shadow: var(--shadow-pop);
    pointer-events: none;
    white-space: nowrap;
  }

  .rail {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 10px 0;
    border: 0;
    border-radius: 0;
    background: none;
    color: var(--text-dim);
    writing-mode: vertical-rl;
    font-size: var(--fs-sm);
    --icon-size: 14px;
  }

  .rail:hover {
    color: var(--text);
    background: var(--bg-raised);
  }

  .head {
    height: 34px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 4px 0 10px;
    font-size: var(--fs-sm);
    --icon-size: 14px;
  }

  .count {
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }

  .sp {
    flex: 1;
  }

  .search {
    margin: 0 8px 6px;
    height: 24px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    padding: 0 6px;
  }

  .save {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 0 8px 8px;
    padding: 8px;
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-md);
    font-size: var(--fs-xs);
  }

  .save .dim {
    color: var(--accent);
  }

  .save input,
  .save textarea {
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    padding: 3px 6px;
    resize: vertical;
  }

  .save-row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .warn {
    color: var(--warn);
  }

  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0 8px 10px;
  }

  .card {
    user-select: none;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 5px 6px 7px 8px;
    border-radius: 8px;
    border: 1px solid var(--border-strong);
    border-left: 3px solid var(--nc);
    background: var(--bg-raised);
    cursor: grab;
    min-width: 0;
  }

  .card:active {
    cursor: grabbing;
  }

  .card .top {
    display: flex;
    align-items: center;
    gap: 1px;
  }

  .card .btn {
    width: 20px;
    height: 20px;
  }

  .card .kind {
    font-size: 10px;
    color: var(--nc);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .card b {
    font-size: var(--fs-sm);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .pnote {
    font-size: 10px;
    color: var(--text-dim);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .origin {
    font-size: 10px;
    color: var(--text-faint);
  }

  .armed {
    color: var(--danger-text);
  }

  .empty,
  .hint {
    font-size: var(--fs-xs);
    color: var(--text-faint);
    line-height: 1.5;
    padding: 4px 2px;
  }
</style>
