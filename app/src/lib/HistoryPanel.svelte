<script lang="ts">
  import type { Author, EntrySummary } from "./types";

  let { entries }: { entries: EntrySummary[] } = $props();

  // 新しい順に表示
  const reversed = $derived([...entries].reverse());

  function authorLabel(a: Author): string {
    switch (a.kind) {
      case "ai":
        return `AI (${a.model})`;
      case "human":
        return "人間";
      case "system":
        return "システム";
    }
  }

  function timeText(ts: string): string {
    return new Date(ts).toLocaleTimeString("ja-JP");
  }
</script>

<div class="panel">
  <h2>履歴 <span class="count">{entries.length}</span></h2>
  {#if entries.length === 0}
    <div class="empty">まだ編集はありません</div>
  {/if}
  <ul>
    {#each reversed as e (e.id)}
      <li class="entry {e.author.kind}">
        <div class="head">
          <span class="badge {e.author.kind}">{authorLabel(e.author)}</span>
          <span class="time">{timeText(e.timestamp)}</span>
        </div>
        <div class="label">{e.label}</div>
        <div class="meta">
          <code>{e.id}</code>
          <span>{e.targets.length} 対象</span>
          {#if e.reverts}
            <span class="revert">↩ {e.reverts} の取り消し</span>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
</div>

<style>
  .panel {
    padding: 10px;
  }

  h2 {
    font-size: 13px;
    margin: 0 0 8px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .count {
    color: var(--accent);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .entry {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 7px 9px;
  }

  /* AI の編集をハイライト(HANDOFF の UI 方針) */
  .entry.ai {
    border-left: 3px solid var(--ai);
  }

  .entry.human {
    border-left: 3px solid var(--human);
  }

  .entry.system {
    border-left: 3px solid var(--system);
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 3px;
  }

  .badge {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 3px;
    font-weight: 600;
  }

  .badge.ai {
    background: color-mix(in srgb, var(--ai) 25%, transparent);
    color: var(--ai);
  }

  .badge.human {
    background: color-mix(in srgb, var(--human) 25%, transparent);
    color: var(--human);
  }

  .badge.system {
    background: color-mix(in srgb, var(--system) 25%, transparent);
    color: var(--system);
  }

  .time {
    font-size: 11px;
    color: var(--text-dim);
  }

  .label {
    font-size: 13px;
    line-height: 1.4;
  }

  .meta {
    display: flex;
    gap: 8px;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-dim);
  }

  .revert {
    color: #e8a07c;
  }

  .empty {
    color: var(--text-dim);
    font-size: 13px;
    padding: 12px 0;
  }
</style>
