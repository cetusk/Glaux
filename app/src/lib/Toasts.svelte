<script lang="ts">
  import Icon from "./Icon.svelte";
  import { dismissToast, toasts } from "./toast.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div class="toast {t.kind}" role={t.kind === "error" ? "alert" : "status"}>
      <span class="text">{t.text}</span>
      {#if t.action}
        <button
          class="btn sm"
          onclick={() => {
            t.action?.run();
            dismissToast(t.id);
          }}>{t.action.label}</button
        >
      {/if}
      <button class="btn sm icon ghost" onclick={() => dismissToast(t.id)} aria-label="閉じる" title="閉じる"
        ><Icon name="x" /></button
      >
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    /* ステータスバー(高さ 26px)に重ならないように */
    bottom: 36px;
    z-index: 100;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: min(420px, calc(100vw - 32px));
    pointer-events: none;
  }

  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px 8px 12px;
    border-radius: var(--r-md);
    font-size: var(--fs-sm);
    line-height: 1.5;
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    border-left-width: 4px;
    box-shadow: var(--shadow-pop);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .toast.error {
    border-left-color: var(--danger);
  }

  .toast.warn {
    border-left-color: var(--warn);
  }

  .toast.ok {
    border-left-color: var(--ok);
  }

  .text {
    flex: 1;
  }
</style>
