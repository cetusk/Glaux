<script lang="ts">
  import Icon from "./Icon.svelte";
  import * as api from "./api";
  import { selectionStore } from "./selection.svelte";
  import { transportStore } from "./transport.svelte";
  import { claimAb, endAbLoop, ensureAbPlaying, releaseAb, restartAb, startAbLoop } from "./abLoop";
  import type { Author, EntrySummary } from "./types";
  import { isEn, plural, tr } from "./i18n.svelte";

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
            tr(
              `取り消しました。ただし後続の ${r.conflicts.length} 件の編集が同じ対象を触っているため、結果を確認してください。`,
              `Reverted. ${r.conflicts.length} later edit(s) touch the same targets, so please check the result.`,
            ),
          );
        }
      })
      .catch((err) => showNotice(tr(`取り消せませんでした: ${err}`, `Couldn't revert: ${err}`)));
  }

  // ---- 音量をそろえた A/B の聴き比べ(A = その編集の前、B = 今) ----
  /** 範囲を決めないときに聴く長さ(tick。4/4 の 8 小節) */
  const AB_DEFAULT_TICKS = 960 * 4 * 8;
  let ab = $state<{ entry: EntrySummary; info: api.AbInfo; side: "a" | "b"; start: number; end: number } | null>(null);
  let abBusy = $state<string | null>(null);

  /** 聴き比べる範囲: ループ中ならその区間、範囲を選んでいればそこ、無ければ今の位置から 8 小節 */
  function abRange(): { start: number; end: number } {
    const loop = transportStore.state.loop;
    if (loop && loop[1] > loop[0]) return { start: loop[0], end: loop[1] };
    const r = selectionStore.range;
    if (r && r.endTick > r.startTick) return { start: r.startTick, end: r.endTick };
    const start = Math.max(0, transportStore.state.tick ?? 0);
    return { start, end: start + AB_DEFAULT_TICKS };
  }

  async function startAb(e: EntrySummary) {
    if (abBusy) return;
    abBusy = e.id;
    claimAb("history", endAb);
    try {
      const { start, end } = abRange();
      const info = await api.abPrepare(e.id, start, end);
      // 範囲は長すぎると切り詰められる(end_tick)。その範囲をループにして頭から鳴らす(聴き終えて黙らないように)
      const stop = info.end_tick && info.end_tick > start ? info.end_tick : end;
      ab = { entry: e, info, side: "b", start, end: stop };
      await startAbLoop(start, stop);
    } catch (err) {
      showNotice(tr(`聴き比べを用意できませんでした: ${err}`, `Couldn't prepare the comparison: ${err}`));
    } finally {
      abBusy = null;
    }
  }

  function setAbSide(side: "a" | "b") {
    if (!ab) return;
    ab.side = side;
    api.abSetSide(side).catch(() => {});
    // 止まっている・範囲の外にいるなら、範囲の頭から鳴らす
    ensureAbPlaying(ab.start, ab.end).catch(() => {});
  }

  function endAb() {
    releaseAb("history");
    ab = null;
    api.abClear().catch(() => {});
    endAbLoop();
  }

  // 聴き比べの途中で編集したら、「今」が古くなるので終える(古い音を聴き続けないように)
  let abTotal = 0;
  $effect(() => {
    const t = total;
    if (!ab) {
      abTotal = t;
      return;
    }
    if (t !== abTotal) {
      endAb();
      showNotice(tr("編集したので聴き比べを終えました(もう一度ヘッドホンのボタンで聴き比べられます)", "Comparison ended because of an edit (press the headphones button to compare again)"));
    }
  });

  function fmtLufs(v: number | null): string {
    return v == null ? "—" : `${v.toFixed(1)} LUFS`;
  }

  /** そろえるために下げた量の説明 */
  function matchText(info: api.AbInfo): string {
    if (info.lufs_a == null || info.lufs_b == null) return tr("片方がほぼ無音のため、音量はそろえていません", "One side is nearly silent, so levels aren't matched");
    const d = info.lufs_b - info.lufs_a;
    if (Math.abs(d) < 0.1) return tr("もともと同じ大きさです", "Already at the same level");
    return tr(
      `${d > 0 ? "今" : "前"}の方が ${Math.abs(d).toFixed(1)} dB 大きいので下げてそろえています`,
      `${d > 0 ? "Now" : "Before"} is ${Math.abs(d).toFixed(1)} dB louder, so it was lowered to match`,
    );
  }

  function authorLabel(a: Author): string {
    switch (a.kind) {
      case "ai":
        return `AI (${a.model})`;
      case "human":
        return tr("人間", "Human");
      case "system":
        return tr("システム", "System");
    }
  }

  function timeText(ts: string): string {
    return new Date(ts).toLocaleTimeString(isEn() ? "en-US" : "ja-JP");
  }
</script>

<div class="panel">
  <h2><Icon name="history" size={15} />{tr("履歴", "History")} <span class="count">{total}</span></h2>
  {#if ab}
    <div class="ab" role="group" aria-label={tr("音量をそろえた聴き比べ", "Level-matched comparison")}>
      <div class="ab-head">
        <Icon name="headphones" size={13} /><span class="ab-title" title={ab.entry.label}>{tr(`「${ab.entry.label}」の前と今`, `"${ab.entry.label}": before vs. now`)}</span>
        <button class="btn sm icon ghost" title={tr("聴き比べを終える", "End compare")} aria-label={tr("聴き比べを終える", "End compare")} onclick={endAb}
          ><Icon name="x" /></button
        >
      </div>
      <div class="ab-switch">
        <button class="btn sm" class:on={ab.side === "a"} onclick={() => setAbSide("a")} title={tr(`前(${fmtLufs(ab.info.lufs_a)})`, `Before (${fmtLufs(ab.info.lufs_a)})`)}
          >A {tr("前", "Before")}</button
        >
        <button class="btn sm" class:on={ab.side === "b"} onclick={() => setAbSide("b")} title={tr(`今(${fmtLufs(ab.info.lufs_b)})`, `Now (${fmtLufs(ab.info.lufs_b)})`)}
          >B {tr("今", "Now")}</button
        >
        <button class="btn sm" onclick={() => ab && restartAb(ab.start).catch(() => {})} title={tr("聴いている範囲の頭から聴き直す", "Replay from the start of the range")}
          >⏮ {tr("頭から", "From start")}</button
        >
      </div>
      <div class="ab-note">
        {tr("A / B を切り替えても、同じ位置から続けて鳴ります(頭から聴くなら「頭から」)", "Switching A / B keeps playing from the same position (use \"From start\" to replay)")}
      </div>
      <div class="ab-note">
        {matchText(ab.info)}{tr(
          `(前 ${fmtLufs(ab.info.lufs_a)} / 今 ${fmtLufs(ab.info.lufs_b)})`,
          ` (before ${fmtLufs(ab.info.lufs_a)} / now ${fmtLufs(ab.info.lufs_b)})`,
        )}
      </div>
    </div>
  {/if}
  {#if entries.length === 0}
    <div class="empty">{tr("まだ編集はありません", "No edits yet")}</div>
  {/if}
  {#if notice}
    <div class="notice">{notice}</div>
  {/if}
  <ul>
    {#each redoShown as e (e.id)}
      <li class="entry undone" title={tr(`取り消し済み(やり直しで戻せます)\n${e.id}`, `Undone (can be redone)\n${e.id}`)}>
        <div class="head">
          <span class="badge {e.author.kind}">{authorLabel(e.author)}</span>
          <span class="head-right"><span class="time">{tr("取り消し済み", "Undone")}</span></span>
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
            <button
              class="btn sm icon ghost"
              title={tr(
                "この編集の前と今を、音量をそろえて聴き比べる(ループ・選んだ範囲、無ければ今の位置から 8 小節)",
                "Compare before this edit and now at matched levels (loop or selected range, else 8 bars from the current position)",
              )}
              aria-label={tr("この編集の前と今を聴き比べる", "Compare before this edit and now")}
              disabled={abBusy != null}
              onclick={() => startAb(e)}
              ><Icon name={abBusy === e.id ? "loader-circle" : "headphones"} /></button
            >
            {#if !revertedIds.has(e.id)}
              <button
                class="btn sm icon ghost"
                title={tr(
                  "この編集だけ打ち消す(後の編集は残す。打ち消し自体も履歴に載り、Ctrl+Z で戻せます)",
                  "Revert only this edit (later edits are kept; the revert is recorded in history and can be undone with Ctrl+Z)",
                )}
                aria-label={tr("この編集だけ打ち消す", "Revert only this edit")}
                onclick={() => revert(e)}><Icon name="eraser" /></button
              >
            {/if}
          </span>
        </div>
        <div class="label">{e.label}</div>
        <div class="meta">
          <span>{tr(`${e.targets.length} 対象`, `${plural(e.targets.length, "target")}`)}</span>
          {#if e.reverts}
            {@const target = entries.find((x) => x.id === e.reverts)}
            <span class="revert"
              ><Icon name="eraser" size={11} />{target
                ? tr(`「${target.label}」の打ち消し`, `Revert of "${target.label}"`)
                : tr("以前の編集の打ち消し", "Revert of an earlier edit")}</span
            >
          {/if}
        </div>
      </li>
    {/each}
    {#if hidden > 0}
      <li class="more">{tr(`…以前の ${hidden} 件は表示を省略(履歴自体は保持されています)`, `…${hidden} older entries hidden (the history itself is kept)`)}</li>
    {/if}
  </ul>
</div>

<style>
  .panel {
    padding: 10px;
  }

  .ab {
    background: var(--bg);
    border: 1px solid var(--accent);
    border-radius: 6px;
    padding: 7px 9px;
    margin-bottom: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .ab-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
  }

  .ab-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ab-switch {
    display: flex;
    gap: 6px;
  }

  .ab-switch .btn {
    flex: 1;
  }

  .ab-switch .btn.on {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }

  .ab-note {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.4;
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
