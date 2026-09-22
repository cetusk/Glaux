<script lang="ts">
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { selectionStore } from "./selection.svelte";
  import type { RecentProject } from "./types";

  let { title }: { title: string } = $props();

  let openMenu = $state(false);
  let recent = $state<RecentProject[]>([]);
  let defaultDir = $state("");
  let parentDir = $state("");
  let newName = $state("");
  let busy = $state(false);
  let menuError = $state<string | null>(null);

  async function toggle() {
    openMenu = !openMenu;
    menuError = null;
    if (openMenu) {
      try {
        const r = await api.listRecentProjects();
        recent = r.recent;
        defaultDir = r.default_dir;
        if (!parentDir) parentDir = r.default_dir;
      } catch (e) {
        menuError = String(e);
      }
    }
  }

  function afterSwitch() {
    // チャットパネルに会話表示のクリアを促す(プロジェクトごとに会話は別)
    chatStatus.epoch += 1;
    selectionStore.range = null;
    openMenu = false;
    busy = false;
    newName = "";
  }

  async function openPath(path: string) {
    if (busy) return;
    busy = true;
    menuError = null;
    try {
      await api.openProject(path);
      afterSwitch();
    } catch (e) {
      menuError = String(e);
      busy = false;
    }
  }

  async function browseAndOpen() {
    const dir = await pickFolder({ directory: true, title: "Glaux プロジェクトフォルダを開く" });
    if (typeof dir === "string") await openPath(dir);
  }

  async function browseParentDir() {
    const dir = await pickFolder({ directory: true, title: "新規プロジェクトの作成場所" });
    if (typeof dir === "string") parentDir = dir;
  }

  async function createNew() {
    if (busy || !newName.trim()) return;
    busy = true;
    menuError = null;
    try {
      await api.createProject(parentDir, newName);
      afterSwitch();
    } catch (e) {
      menuError = String(e);
      busy = false;
    }
  }

  function onNameKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      createNew();
    }
  }
</script>

<div class="menu-root">
  <button class="title-btn" onclick={toggle} title="プロジェクトを切り替える">
    <span class="title-text">{title}</span>
    <span class="chev">▾</span>
  </button>

  {#if openMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="backdrop" onclick={() => (openMenu = false)}></div>
    <div class="dropdown">
      {#if menuError}
        <div class="menu-error">{menuError}</div>
      {/if}

      <div class="section">
        <div class="section-title">新規プロジェクト</div>
        <div class="new-row">
          <input
            type="text"
            placeholder="曲名"
            bind:value={newName}
            onkeydown={onNameKeydown}
            disabled={busy}
          />
          <button onclick={createNew} disabled={busy || !newName.trim()}>作成</button>
        </div>
        <button class="loc" onclick={browseParentDir} title="クリックで作成場所を変更">
          場所: {parentDir || defaultDir}
        </button>
      </div>

      <div class="section">
        <button class="wide" onclick={browseAndOpen} disabled={busy}>
          フォルダを選択して開く…
        </button>
      </div>

      {#if recent.length > 0}
        <div class="section">
          <div class="section-title">最近使ったプロジェクト</div>
          {#each recent as r (r.path)}
            <button
              class="recent-item"
              class:current={r.current}
              disabled={busy || r.current || !r.exists}
              onclick={() => openPath(r.path)}
              title={r.exists ? r.path : `見つかりません: ${r.path}`}
            >
              <span class="recent-title">
                {r.title}{r.current ? "(開いています)" : ""}{r.exists ? "" : "(見つかりません)"}
              </span>
              <span class="recent-path">{r.path}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .menu-root {
    position: relative;
  }

  .title-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    font-size: 15px;
    border: none;
    background: none;
    padding: 2px 6px;
  }

  .title-btn:hover {
    color: var(--accent);
  }

  .chev {
    font-size: 10px;
    color: var(--text-dim);
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 9;
  }

  .dropdown {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 10;
    width: 380px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.5);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-height: 70vh;
    overflow-y: auto;
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .section-title {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .new-row {
    display: flex;
    gap: 6px;
  }

  .new-row input {
    flex: 1;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 13px;
  }

  .new-row input:focus {
    outline: none;
    border-color: var(--accent-dim);
  }

  .loc {
    text-align: left;
    font-size: 11px;
    color: var(--text-dim);
    border: none;
    background: none;
    padding: 0 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .loc:hover {
    color: var(--accent);
  }

  .wide {
    width: 100%;
    text-align: center;
  }

  .recent-item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    width: 100%;
    text-align: left;
    padding: 6px 8px;
  }

  .recent-item.current {
    border-color: var(--accent-dim);
  }

  .recent-item:disabled {
    opacity: 0.55;
  }

  .recent-title {
    font-size: 13px;
    font-weight: 600;
  }

  .recent-path {
    font-size: 10px;
    color: var(--text-dim);
    font-family: Consolas, monospace;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .menu-error {
    background: #46242c;
    color: #ffb4c0;
    font-size: 12px;
    padding: 6px 8px;
    border-radius: 6px;
  }
</style>
