<script lang="ts">
  import { dismissToast, toasts } from "./toast.svelte";
</script>

<div class="toasts" aria-live="polite">
  {#each toasts.list as t (t.id)}
    <div class="toast {t.kind}" role={t.kind === "error" ? "alert" : "status"}>
      <span class="text">{t.text}</span>
      <button class="close" onclick={() => dismissToast(t.id)} aria-label="閉じる" title="閉じる">✕</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
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
    align-items: flex-start;
    gap: 8px;
    padding: 8px 10px 8px 12px;
    border-radius: 6px;
    font-size: 12px;
    line-height: 1.5;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-left-width: 4px;
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.4);
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

  .close {
    border: none;
    background: none;
    padding: 0 2px;
    font-size: 11px;
    color: var(--text-dim);
  }
</style>
