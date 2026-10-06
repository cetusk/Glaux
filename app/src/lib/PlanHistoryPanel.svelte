<script lang="ts">
  // 計画の履歴(曲の履歴とは別。誰が・なぜ・きっかけ)。版ごとに「この版に戻す」(戻したことも新しい版として残る)と
  // 「この変更だけ取り消す」(後の変更は残す)。設計画面の Ctrl+Z も計画に効く
  import Icon from "./Icon.svelte";
  import { designStore, restorePlan, revertPlanEntry, type PlanHistoryEntry } from "./design.svelte";
  import { isEn, tr } from "./i18n.svelte";

  const d = $derived(designStore.data);
  const nameOf = (id: string) => d?.plans.find((p) => p.plan_id === id)?.name ?? id;
  const isProposal = (id: string) => d?.plans.find((p) => p.plan_id === id)?.state === "proposal";
  const kindOf = (id: string) => {
    if (isProposal(id)) return tr("案", "Proposal");
    const k = d?.plans.find((p) => p.plan_id === id)?.kind;
    return k === "song" ? tr("曲全体", "Whole song") : k === "part" ? tr("パート", "Part") : k === "melody" ? tr("旋律", "Melody") : "";
  };
  function who(e: PlanHistoryEntry): string {
    switch (e.author.kind) {
      case "ai":
        return `AI (${e.author.model})`;
      case "human":
        return tr("人間", "Human");
      default:
        return tr("システム", "System");
    }
  }
  // 表示名は画面の言語で(表示のたびに作る。言語を切り替えたら描き直される)
  const ops = (): Record<string, string> => ({
    create: tr("作る", "Create"),
    replace: tr("置き換え", "Replace"),
    edit: tr("直す", "Edit"),
    delete: tr("消す", "Delete"),
  });
  const triggers = (): Record<string, string> => ({
    user: tr("人の言葉", "User request"),
    finding: tr("点検の指摘", "Check finding"),
    listening: tr("聴き比べ", "Listening"),
  });
  function timeText(ts: string): string {
    const t = new Date(ts);
    return t.toLocaleString(isEn() ? "en-US" : "ja-JP", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
  }
</script>

<div class="panel">
  <h2>
    <Icon name="history" size={15} />{tr("計画の履歴", "Plan history")} <span class="count">{d?.history_total ?? 0}</span>
    {#if d?.redoable}<span class="redo" title={tr("取り消した計画の変更(AI に「やり直して」と頼めます)", "Undone plan changes (you can ask the AI to redo them)")}
        >{tr("取り消し済み", "Undone")} {d.redoable}</span
      >{/if}
  </h2>
  {#if !d}
    <div class="empty">{tr("読み込み中…", "Loading…")}</div>
  {:else if d.history.length === 0}
    <div class="empty">
      {tr(
        "まだ計画の変更はありません。計画は AI が set_song_plan・save_plan などで書くと、ここに理由と一緒に残ります。",
        "No plan changes yet. When the AI writes a plan with set_song_plan, save_plan, etc., it appears here with the reason.",
      )}
    </div>
  {:else}
    <ul>
      {#each d.history as e, k (e.entry_id)}
        <li class="entry {e.author.kind}" title={e.entry_id}>
          <div class="head">
            <span class="badge {e.author.kind}">{who(e)}</span>
            <span class="head-right">
              {#if k === 0}<span class="now">{tr("今", "Now")}</span>{/if}
              <span class="time">{timeText(e.time)}</span>
            </span>
          </div>
          <div class="label">
            <span class="plan">{kindOf(e.plan_id)}{tr(`「${nameOf(e.plan_id)}」`, ` "${nameOf(e.plan_id)}" `)}</span>
            {ops()[e.op] ?? e.op}{e.rev != null ? tr(`(版 ${e.rev})`, ` (rev ${e.rev})`) : ""}{e.reverts ? tr("・取り消し", " · revert") : ""}
          </div>
          {#if e.why}<div class="why">{e.why}</div>{/if}
          {#if e.trigger}
            <div class="trigger">{triggers()[e.trigger.kind] ?? e.trigger.kind}: {e.trigger.text}</div>
          {/if}
          {#if e.song_entry}
            <div class="linked" title={tr(`曲の履歴の ${e.song_entry} と一組`, `Paired with ${e.song_entry} in the song history`)}>
              {tr("曲の編集と一組(曲の側で取り消す・やり直すと、一緒に戻ります)", "Paired with a song edit (undo / redo it on the song side to revert both)")}
            </div>
          {:else if k > 0 && !e.reverts}
            <div class="ops">
              {#if e.rev != null && e.op !== "delete" && d.plans.some((p) => p.plan_id === e.plan_id) && !isProposal(e.plan_id)}
                <button
                  type="button"
                  title={tr("この計画をこの版の中身に戻す(戻したことも新しい版として残る)", "Restore this plan to this revision (the restore is kept as a new revision)")}
                  onclick={() => restorePlan(e.plan_id, e.rev!)}>{tr("この版に戻す", "Restore this rev")}</button
                >
              {/if}
              <button type="button" title={tr("この変更だけを取り消す(後の変更は残す)", "Revert only this change (later changes are kept)")} onclick={() => revertPlanEntry(e.entry_id)}
                >{tr("この変更だけ取り消す", "Revert this only")}</button
              >
            </div>
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
  .ops {
    display: flex;
    gap: 10px;
    margin-top: 4px;
  }
  .ops button {
    background: transparent;
    border: 0;
    padding: 0;
    color: var(--accent);
    font-size: var(--fs-xs);
    cursor: pointer;
  }
  .ops button:hover {
    text-decoration: underline;
  }
  .trigger {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    margin-top: 2px;
  }
  .linked {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    margin-top: 4px;
  }
</style>
