<script lang="ts">
  // タイムラインの右クリックメニューの枠(背景のクリックで閉じる幕と、画面に収める箱)。
  // 中身(項目)は各メニューのコンポーネントが children で渡す。項目の見た目はここで共通に決める
  import type { Snippet } from "svelte";
  import { keepInView } from "./menu";

  let {
    x,
    y,
    extraClass = "",
    onClose,
    children,
  }: {
    x: number;
    y: number;
    /// 箱に足すクラス(sig-menu / clip-menu)
    extraClass?: string;
    onClose: () => void;
    children: Snippet;
  } = $props();
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="menu-backdrop" role="presentation" onclick={onClose} oncontextmenu={(e) => { e.preventDefault(); onClose(); }}></div>
<div class={extraClass ? `track-menu ${extraClass}` : "track-menu"} use:keepInView style="left:{x}px;top:{y}px">
  {@render children()}
</div>

<style>
  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .track-menu {
    position: fixed;
    z-index: 20;
    /* 画面に収まらない分は中でスクロール(位置は keepInView が画面内へ寄せる) */
    max-height: calc(100vh - 16px);
    max-width: min(440px, calc(100vw - 16px));
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 200px;
    font-size: var(--fs-md);
  }

  /* 項目は各メニューのコンポーネントのものなので :global で(.track-menu を先祖に付けて、ほかの画面の同名のクラスには掛けない) */

  /* メニューの項目: アイコン + 文字 + 右端にキー */
  .track-menu :global(button) {
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    border: none;
    background: none;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
    --icon-size: 15px;
  }

  .track-menu :global(button > .icon) {
    color: var(--text-dim);
  }

  .track-menu :global(button small) {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .track-menu :global(.ic-space) {
    width: 15px;
    flex-shrink: 0;
  }

  .track-menu :global(.key) {
    margin-left: auto;
    padding-left: 24px;
    font-size: var(--fs-xs);
    font-family: var(--mono);
    color: var(--text-faint);
  }

  .track-menu :global(button:hover:not(:disabled)) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .track-menu :global(button.danger:hover) {
    background: var(--danger-bg);
    color: var(--danger-text);
  }

  .track-menu :global(.menu-sep) {
    height: 1px;
    background: var(--border);
    margin: 2px 4px;
  }

  .track-menu :global(.menu-note) {
    font-size: var(--fs-xs);
    color: var(--text-faint);
    padding: 2px 10px 4px 35px;
    max-width: 300px;
    line-height: 1.5;
  }

  .track-menu :global(.preset-title) {
    font-size: 10px;
    color: var(--text-dim);
    padding: 2px 10px;
    letter-spacing: 0.05em;
  }
</style>
