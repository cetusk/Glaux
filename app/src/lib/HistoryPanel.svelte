<script lang="ts">
  import Icon from "./Icon.svelte";
  import * as api from "./api";
  import type { Author, EntrySummary } from "./types";

  let {
    entries,
    total,
    redoable = [],
  }: { entries: EntrySummary[]; total: number; redoable?: EntrySummary[] } = $props();

  /// やり直せる(取り消した)編集。上から「一番先にやり直すもの」が最後になるように並べる
  const redoShown = $derived([...redoable].reverse());

  // 新しい順に表示。件数が増えると全件再描画が重くなり(つまみ操作で履歴は
  // どんどん増える)、再生中の音切れの一因になるため、取得も表示も直近だけに絞る
  // (api.getHistory の HISTORY_LIMIT 件)
  const reversed = $derived([...entries].reverse());
  const hidden = $derived(Math.max(0, total - entries.length));

  /// 既に取り消し済みのエントリ ID(↩ ボタンを出さない)
  const revertedIds = $derived(
    new Set(entries.map((e) => e.reverts).filter((x): x is string => !!x)),
  );

  let notice = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  function showNotice(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 6000);
  }

  function revert(e: EntrySummary) {
    api
      .revertEntry(e.id)
      .then((r) => {
        if (r.conflicts.length > 0) {
          showNotice(
            `取り消しました。ただし後続の ${r.conflicts.length} 件の編集が同じ対象を触っているため、結果を確認してください。`,
          );
        }
      })
      .catch((err) => showNotice(`取り消せませんでした: ${err}`));
  }

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
  <h2><Icon name="history" size={15} />履歴 <span class="count">{total}</span></h2>
  {#if entries.length === 0}
    <div class="empty">まだ編集はありません</div>
  {/if}
  {#if notice}
    <div class="notice">{notice}</div>
  {/if}
  <ul>
    {#each redoShown as e (e.id)}
      <li class="entry undone" title={`取り消し済み(やり直しで戻せます)\n${e.id}`}>
        <div class="head">
          <span class="badge {e.author.kind}">{authorLabel(e.author)}</span>
          <span class="head-right"><span class="time">取り消し済み</span></span>
        </div>
        <div class="label">{e.label}</div>
      </li>
    {/each}
    {#each reversed as e (e.id)}
      <li class="entry {e.author.kind}" title={e.id}>
        <div class="head">
          <span class="badge {e.author.kind}">{authorLabel(e.author)}</span>
          <span class="head-right">
            <span class="time">{timeText(e.timestamp)}</span>
            {#if !revertedIds.has(e.id)}
              <button
                class="btn sm icon ghost"
                title="この編集だけ打ち消す(後の編集は残す。打ち消し自体も履歴に載り、Ctrl+Z で戻せます)"
                aria-label="この編集だけ打ち消す"
                onclick={() => revert(e)}><Icon name="rotate-ccw" /></button
              >
            {/if}
          </span>
        </div>
        <div class="label">{e.label}</div>
        <div class="meta">
          <span>{e.targets.length} 対象</span>
          {#if e.reverts}
            {@const target = entries.find((x) => x.id === e.reverts)}
            <span class="revert"
              ><Icon name="rotate-ccw" size={11} />{target ? `「${target.label}」の打ち消し` : "以前の編集の打ち消し"}</span
            >
          {/if}
        </div>
      </li>
    {/each}
    {#if hidden > 0}
      <li class="more">…以前の {hidden} 件は表示を省略(履歴自体は保持されています)</li>
    {/if}
  </ul>
</div>

<style>
  .panel {
    padding: 10px;
  }

  .entry.undone {
    opacity: 0.45;
    border-style: dashed;
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
    display: inline-flex;
    align-items: center;
    gap: 3px;
    color: var(--warn);
  }

  .empty {
    color: var(--text-dim);
    font-size: 13px;
    padding: 12px 0;
  }

  .head-right {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .notice {
    background: color-mix(in srgb, var(--warn) 15%, transparent);
    border: 1px solid var(--warn);
    border-radius: 6px;
    color: var(--warn);
    font-size: 12px;
    padding: 6px 9px;
    margin-bottom: 8px;
  }
  .more {
    font-size: 10px;
    color: var(--text-dim);
    padding: 6px 4px;
    list-style: none;
  }
</style>
