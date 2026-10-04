<script lang="ts">
  // 計画の履歴(曲の履歴とは別。誰が・なぜ・きっかけ)。段階 1 は見るだけ
  // (この版に戻す・この変更だけ取り消す・案の採用は段階 2。今はチャットで頼めるし、AI は undo_plan を使える)
  import Icon from "./Icon.svelte";
  import { designStore, type PlanHistoryEntry } from "./design.svelte";

  const d = $derived(designStore.data);
  const nameOf = (id: string) => d?.plans.find((p) => p.plan_id === id)?.name ?? id;
  const kindOf = (id: string) => {
    const k = d?.plans.find((p) => p.plan_id === id)?.kind;
    return k === "song" ? "曲全体" : k === "part" ? "パート" : k === "melody" ? "旋律" : "";
  };
  function who(e: PlanHistoryEntry): string {
    switch (e.author.kind) {
      case "ai":
        return `AI (${e.author.model})`;
      case "human":
        return "人間";
      default:
        return "システム";
    }
  }
  const OPS: Record<string, string> = { create: "作る", replace: "置き換え", edit: "直す", delete: "消す" };
  const TRIGGERS: Record<string, string> = { user: "人の言葉", finding: "点検の指摘", listening: "聴き比べ" };
  function timeText(ts: string): string {
    const t = new Date(ts);
    return t.toLocaleString("ja-JP", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }
</script>

<div class="panel">
  <h2>
    <Icon name="history" size={15} />計画の履歴 <span class="count">{d?.history_total ?? 0}</span>
    {#if d?.redoable}<span class="redo" title="取り消した計画の変更(AI に「やり直して」と頼めます)">取り消し済み {d.redoable}</span>{/if}
  </h2>
  {#if !d}
    <div class="empty">読み込み中…</div>
  {:else if d.history.length === 0}
    <div class="empty">まだ計画の変更はありません。計画は AI が set_song_plan・save_plan などで書くと、ここに理由と一緒に残ります。</div>
  {:else}
    <ul>
      {#each d.history as e, k (e.entry_id)}
        <li class="entry {e.author.kind}" title={e.entry_id}>
          <div class="head">
            <span class="badge {e.author.kind}">{who(e)}</span>
            <span class="head-right">
              {#if k === 0}<span class="now">今</span>{/if}
              <span class="time">{timeText(e.time)}</span>
            </span>
          </div>
          <div class="label">
            <span class="plan">{kindOf(e.plan_id)}「{nameOf(e.plan_id)}」</span>
            {OPS[e.op] ?? e.op}{e.rev != null ? `(版 ${e.rev})` : ""}{e.reverts ? "・取り消し" : ""}
          </div>
          {#if e.why}<div class="why">{e.why}</div>{/if}
          {#if e.trigger}
            <div class="trigger">{TRIGGERS[e.trigger.kind] ?? e.trigger.kind}: {e.trigger.text}</div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .panel {
    padding: 10px;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-md);
    margin: 0 0 8px;
    color: var(--text-dim);
  }
  .count {
    color: var(--accent);
  }
  .redo {
    margin-left: auto;
    font-size: var(--fs-xs);
    font-weight: 400;
    color: var(--text-faint);
  }
  .empty {
    color: var(--text-dim);
    font-size: var(--fs-sm);
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
    font-size: var(--fs-sm);
  }
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
  .head-right {
    display: flex;
    gap: 6px;
    align-items: center;
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
    color: var(--text-dim);
  }
  .now {
    font-size: 10px;
    color: var(--accent);
    border: 1px solid var(--accent-dim);
    border-radius: 3px;
    padding: 0 4px;
  }
  .time {
    font-size: 10px;
    color: var(--text-faint);
  }
  .plan {
    color: var(--text-dim);
  }
  .why {
    color: var(--text);
    margin-top: 2px;
  }
  .trigger {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    margin-top: 2px;
  }
</style>
