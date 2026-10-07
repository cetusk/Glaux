<script lang="ts">
  // 設計画面の上の「AI の案」の帯: 案ごとに今と聴き比べる・採用する・捨てる。案が 2 つ以上なら、まとめて切り替えて聴き比べる
  import Icon from "./Icon.svelte";
  import { addBars as addBarsIn, rangeFromChanges } from "./abRange";
  import type { Project } from "./types";
  import {
    MAX_PROPOSALS_AB,
    abLetter,
    abProposals,
    abSingle,
    adoptProposal,
    designSel,
    designStore,
    discardProposal,
    endProposalAb,
    proposalAb,
    proposals,
    restartProposalAb,
    setProposalSide,
    type DesignSel,
  } from "./design.svelte";
  import { selectionStore } from "./selection.svelte";
  import { transportStore } from "./transport.svelte";
  import { plural, tr } from "./i18n.svelte";

  let { project, pick }: { project: Project; pick: (s: DesignSel) => void } = $props();

  const d = $derived(designStore.data);
  const sel = $derived(designSel.sel);

  // ---- 案(枝)----
  const propList = $derived(proposals(d));
  const planName = (id: string | undefined) => d?.plans.find((p) => p.plan_id === id)?.name;
  const proposalWhy = (id: string) => d?.history.find((h) => h.plan_id === id && h.op === "create")?.why;
  const sortedSecs = $derived([...(project.sections ?? [])].sort((a, b) => a.tick - b.tick));
  /** `tick` から `n` 小節進んだ所(拍子どおり) */
  const addBars = (tick: number, n: number) => addBarsIn(project, tick, n);
  const sectionRange = (i: number) => ({
    start: sortedSecs[i].tick,
    end: sortedSecs[i + 1]?.tick ?? addBars(sortedSecs[i].tick, d?.sections[i]?.bars ?? 8),
  });
  const changesOf = (planId: string) => d?.proposal_changes?.[planId];
  /** 案を聴き比べる範囲: 区間を指定したらその区間。無ければ、選んでいる区間が変わる所ならそこ、
   *  そうでなければ変わる所の最初の区間 → 選んでいる区間 → 選んだ小節 → 変わる範囲の頭から 4 小節 → 再生位置から 8 小節 */
  function proposalRange(planId: string, prefer: number | null = null): { start: number; end: number; section: number | null } {
    const ch = changesOf(planId);
    const selI = sel.kind === "section" || sel.kind === "cell" ? sel.i : -1;
    if (prefer != null && sortedSecs[prefer]) return { ...sectionRange(prefer), section: prefer };
    const changed = ch?.sections ?? [];
    if (changed.length && sortedSecs[changed[0]]) {
      const i = changed.includes(selI) ? selI : changed[0];
      return { ...sectionRange(i), section: i };
    }
    if (selI >= 0 && sortedSecs[selI]) return { ...sectionRange(selI), section: selI };
    const r = selectionStore.range;
    if (r && r.endTick > r.startTick) return { start: r.startTick, end: r.endTick, section: null };
    const fromChanges = ch?.ranges?.length ? rangeFromChanges(project, ch.ranges) : null;
    if (fromChanges) return { ...fromChanges, section: null };
    const start = Math.max(0, transportStore.state.tick ?? 0);
    return { start, end: addBars(start, 8), section: null };
  }
  function listen(planId: string, prefer: number | null = null) {
    const r = proposalRange(planId, prefer);
    if (r.section != null) pick({ kind: "section", i: r.section });
    abProposals([planId], r.start, r.end, r.section);
  }
  // まとめて聴き比べる: 音の変わる・今の曲に当てられる案(最初の 4 つ)を、今と同じ範囲で切り替えて聴く
  const listenable = $derived(propList.filter((p) => p.edits && !changesOf(p.plan_id)?.stale));
  const allIds = $derived(listenable.slice(0, MAX_PROPOSALS_AB).map((p) => p.plan_id));
  /** どれかの案で変わる区間(区間ごとに、変わる案の数) */
  const changedBy = $derived.by(() => {
    const m = new Map<number, number>();
    for (const id of allIds) for (const si of changesOf(id)?.sections ?? []) m.set(si, (m.get(si) ?? 0) + 1);
    return m;
  });
  const allChanged = $derived([...changedBy.keys()].sort((a, b) => a - b));
  const multi = $derived(proposalAb.planIds.length >= 2);
  /** まとめて聴く範囲: 区間を指定したらそこ。無ければ、選んでいる区間が変わる所ならそこ、
   *  そうでなければいちばん多くの案が変わる区間(同じなら前の方)。どれも無ければ最初の案の範囲 */
  function listenAll(prefer: number | null = null) {
    if (allIds.length < 2) return;
    const selI = sel.kind === "section" || sel.kind === "cell" ? sel.i : -1;
    let i = prefer;
    if (i == null && changedBy.has(selI)) i = selI;
    if (i == null && allChanged.length) i = allChanged.reduce((b, x) => ((changedBy.get(x) ?? 0) > (changedBy.get(b) ?? 0) ? x : b));
    const r = i != null && sortedSecs[i] ? { ...sectionRange(i), section: i } : proposalRange(allIds[0]);
    if (r.section != null) pick({ kind: "section", i: r.section });
    abProposals(allIds, r.start, r.end, r.section);
  }
  /** まとめて聴き比べているときの、案の字(B・C …。入っていなければ null) */
  const letterOf = (planId: string) => {
    const k = proposalAb.planIds.indexOf(planId);
    return multi && k >= 0 ? abLetter(k + 1) : null;
  };
  // 案が無くなったら(採用・捨てる・AI が消した)聴き比べも終える
  $effect(() => {
    if (proposalAb.planIds.some((id) => !propList.some((p) => p.plan_id === id))) endProposalAb();
  });
</script>

{#if d && propList.length}
  <div class="proposals" role="region" aria-label={tr("AI の案", "AI proposals")}>
    <div class="phead">
      <b>{tr(`AI の案(${propList.length})`, `AI proposals (${propList.length})`)}</b>
      <span class="hint"
        >{tr(
          "今の曲と計画は変わっていません。聴き比べて、よければ採用してください。聴き比べ中は範囲を繰り返し鳴らし、A / B でいつでも切り替えられます",
          "The current song and plan are unchanged. Compare, and adopt if you like it. While comparing, the range loops and you can switch A / B anytime",
        )}</span
      >
    </div>
    {#if allIds.length >= 2}
      <div class="pall" class:on={multi}>
        {#if multi}
          {@const info = proposalAb.info}
          <div class="prow">
            <span class="abswitch" role="group" aria-label={tr("今と案を切り替える", "Switch between now and proposal")}>
              <button class="btn sm" class:on={proposalAb.side === 0} type="button" onclick={() => setProposalSide(0)}>A {tr("今", "Now")}</button>
              {#each proposalAb.planIds as id, k (id)}
                <button class="btn sm" class:on={proposalAb.side === k + 1} type="button" title={planName(id)} onclick={() => setProposalSide(k + 1)}
                  >{abLetter(k + 1)} {planName(id) ?? tr("案", "Proposal")}</button
                >
              {/each}
            </span>
            <button class="btn sm" type="button" title={tr("聴いている範囲の頭から聴き直す", "Replay from the start of the range")} onclick={restartProposalAb}
              >⏮ {tr("頭から", "From start")}</button
            >
            <span class="spacer"></span>
            <button class="btn sm" type="button" onclick={endProposalAb}>{tr("聴き比べを終える", "End compare")}</button>
          </div>
          <div class="prow">
            <span class="plabel">{tr("範囲", "Range")}</span>
            {#each allChanged as si (si)}
              <button
                class="btn sm secchip"
                class:on={proposalAb.section === si}
                type="button"
                title={tr(`この区間でまとめて聴き比べる(変わる案 ${changedBy.get(si)} つ)`, `Compare all in this section (${plural(changedBy.get(si) ?? 0, "proposal")} change)`)}
                disabled={proposalAb.busy != null}
                onclick={() => listenAll(si)}>{d.sections[si]?.name ?? tr(`区間 ${si + 1}`, `Section ${si + 1}`)}</button
              >
            {/each}
            <span class="abnote"
              >{proposalAb.section != null
                ? tr(`「${d.sections[proposalAb.section]?.name}」を聴いています`, `Playing "${d.sections[proposalAb.section]?.name}"`)
                : tr("選んだ範囲を聴いています", "Playing the selected range")}{#if info}{" "}· {info.lufs
                  .map((l, k) => `${k === 0 ? tr("今", "Now") : abLetter(k)} ${l?.toFixed(1) ?? "—"}`)
                  .join(" / ")} LUFS{tr("(音量はいちばん小さいものにそろえてあります)", " (levels matched to the quietest)")}{/if}</span
            >
          </div>
          <span class="pnote">{tr("切り替えても、同じ位置から続けて鳴ります(頭から聴くなら「頭から」)", "Switching keeps playing from the same position (use \"From start\" to replay)")}</span>
          {#if info}
            {#each proposalAb.planIds as id, k (id)}
              {#if info.first_diff_secs[k + 1] === null}
                <span class="pwarn"
                  >{abLetter(k + 1)}{tr(
                    `「${planName(id)}」はこの範囲では今と同じ音です(範囲の区間を変えてみてください)`,
                    ` "${planName(id)}" sounds the same as now in this range (try another section)`,
                  )}</span
                >
              {/if}
            {/each}
          {/if}
        {:else}
          <div class="prow">
            <button class="btn sm" type="button" disabled={proposalAb.busy != null} onclick={() => listenAll()}
              >{proposalAb.busy === "all"
                ? tr("用意しています…", "Preparing…")
                : tr(`まとめて聴き比べる(今 + 案 ${allIds.length} つ)`, `Compare all (now + ${plural(allIds.length, "proposal")})`)}</button
            >
            <span class="pnote"
              >{tr(
                `A = 今、${allIds.map((id, k) => `${abLetter(k + 1)} = ${planName(id)}`).join("、")} を同じ範囲で切り替えて聴けます`,
                `Switch between A = now, ${allIds.map((id, k) => `${abLetter(k + 1)} = ${planName(id)}`).join(", ")} over the same range`,
              )}{listenable.length > allIds.length
                ? tr(`(いっしょに聴けるのは ${MAX_PROPOSALS_AB} つまで)`, ` (up to ${MAX_PROPOSALS_AB} at once)`)
                : ""}</span
            >
          </div>
        {/if}
      </div>
    {/if}
    {#each propList as pr (pr.plan_id)}
      {@const on = abSingle(pr.plan_id)}
      {@const ch = changesOf(pr.plan_id)}
      {@const letter = letterOf(pr.plan_id)}
      <div class="prop" class:on={on || letter != null}>
        <div class="prow">
          {#if letter}<span class="pletter" class:cur={proposalAb.side === proposalAb.planIds.indexOf(pr.plan_id) + 1}>{letter}</span>{/if}
          <span class="pname">{pr.name}</span>
          <span class="pbase"
            >{pr.derived_from
              ? tr(`「${planName(pr.derived_from.id) ?? "元の計画"}」の案`, `Proposal for "${planName(pr.derived_from.id) ?? "the original plan"}"`)
              : tr("新しい計画の案", "Proposal for a new plan")}{pr.edits
              ? tr(` · 編集 ${pr.edits} 件`, ` · ${plural(pr.edits, "edit")}`)
              : tr(" · 計画だけ", " · plan only")}</span
          >
        </div>
        {#if proposalWhy(pr.plan_id)}<div class="pwhy">{proposalWhy(pr.plan_id)}</div>{/if}
        <div class="prow">
          <span class="plabel">{tr("変わる所", "Changes")}</span>
          {#if ch?.stale}
            <span class="pwarn"
              >{ch.why ??
                tr(
                  "案を出した後で曲が変わったので、今の曲に当てられません(AI に作り直してもらってください)",
                  "The song changed after this proposal was made, so it can't be applied (ask the AI to redo it)",
                )}</span
            >
          {:else if !pr.edits}
            <span class="pnote">{tr("音は変わりません(計画だけの案)", "No change to the sound (plan-only proposal)")}</span>
          {:else}
            {#each ch?.sections ?? [] as si (si)}
              <button
                class="btn sm secchip"
                class:on={on && proposalAb.section === si}
                type="button"
                title={tr("この区間で聴き比べる", "Compare in this section")}
                disabled={proposalAb.busy != null}
                onclick={() => listen(pr.plan_id, si)}>{d.sections[si]?.name ?? tr(`区間 ${si + 1}`, `Section ${si + 1}`)}</button
              >
            {:else}
              {#if ch?.ranges?.length}<span class="pnote">{tr(`区間の外(${ch.ranges.length} か所)`, `Outside sections (${plural(ch.ranges.length, "place")})`)}</span>{/if}
            {/each}
            {#if ch?.whole}<span class="pnote">{tr("+ 音色・ミックスなど曲全体", "+ whole song (sounds, mix, etc.)")}</span>{/if}
            {#if !ch?.sections?.length && !ch?.ranges?.length && !ch?.whole}<span class="pnote"
                >{ch?.silent_only
                  ? tr("違いは今鳴っていないトラック(ミュート中など)だけです", "Differences are only in tracks not currently heard (muted, etc.)")
                  : tr("音の違いは見つかりませんでした", "No audible difference found")}</span
              >{/if}
          {/if}
        </div>
        <div class="prow">
          {#if on}
            <span class="abswitch" role="group" aria-label={tr("今と案を切り替える", "Switch between now and proposal")}>
              <button class="btn sm" class:on={proposalAb.side === 0} type="button" onclick={() => setProposalSide(0)}>A {tr("今", "Now")}</button>
              <button class="btn sm" class:on={proposalAb.side === 1} type="button" onclick={() => setProposalSide(1)}>B {tr("案", "Proposal")}</button>
            </span>
            <button class="btn sm" type="button" title={tr("聴いている範囲の頭から聴き直す", "Replay from the start of the range")} onclick={restartProposalAb}
              >⏮ {tr("頭から", "From start")}</button
            >
            <span class="pnote">{tr("A / B を切り替えても、同じ位置から続けて鳴ります(頭から聴くなら「頭から」)", "Switching A / B keeps playing from the same position (use \"From start\" to replay)")}</span>
            <span class="abnote"
              >{proposalAb.section != null
                ? tr(`「${d.sections[proposalAb.section]?.name}」を聴いています`, `Playing "${d.sections[proposalAb.section]?.name}"`)
                : tr("選んだ範囲を聴いています", "Playing the selected range")}{#if proposalAb.info}{" "}· {tr("今", "Now")}
                {proposalAb.info.lufs[0]?.toFixed(1) ?? "—"} / {tr("案", "Proposal")}
                {proposalAb.info.lufs[1]?.toFixed(1) ?? "—"} LUFS{tr("(音量はそろえてあります)", " (levels matched)")}{/if}</span
            >
            {@const fd = proposalAb.info?.first_diff_secs[1]}
            {#if proposalAb.info && fd === null}
              <span class="pwarn"
                >{tr("この範囲では今と案の音が同じです(変わる所の区間を押して聴いてください)", "Now and the proposal sound the same in this range (press a section under Changes)")}</span
              >
            {:else if fd != null && fd > 1}
              <span class="pnote">{tr(`違いは範囲の頭から ${fd.toFixed(1)} 秒あたりから`, `Differences start about ${fd.toFixed(1)} s into the range`)}</span>
            {/if}
            <span class="spacer"></span>
            <button class="btn sm" type="button" onclick={endProposalAb}>{tr("聴き比べを終える", "End compare")}</button>
          {:else}
            <button
              class="btn sm"
              type="button"
              disabled={proposalAb.busy != null || !pr.edits || !!ch?.stale}
              title={pr.edits
                ? tr("今と案を、音量をそろえて切り替えて聴く(範囲は変わる所の区間)", "Switch between now and the proposal at matched levels (range: a changed section)")
                : tr("音の変わらない案です(計画だけ)", "This proposal doesn't change the sound (plan only)")}
              onclick={() => listen(pr.plan_id)}>{proposalAb.busy === pr.plan_id ? tr("用意しています…", "Preparing…") : tr("聴き比べる", "Compare")}</button
            >
            <span class="spacer"></span>
          {/if}
          <button
            class="btn sm primary"
            type="button"
            disabled={!!ch?.stale}
            title={tr(
              "案の音を曲に当て、案の計画を今の計画にする。同じ元の計画・同じ頼みから出たほかの案と、いっしょに聴き比べていた案は捨てる",
              "Apply the proposal's sound to the song and make its plan current. Other proposals from the same plan or request, and those being compared, are discarded",
            )}
            onclick={() => adoptProposal(pr.plan_id, pr.name)}>{tr("採用", "Adopt")}</button
          >
          <button class="btn sm" type="button" onclick={() => discardProposal(pr.plan_id, pr.name)}>{tr("捨てる", "Discard")}</button>
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  /* ---- 案(段階 3) ---- */
  .proposals {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-md);
    padding: 6px 10px;
    background: var(--bg-panel);
    font-size: var(--fs-sm);
    max-height: 40vh;
    overflow-y: auto;
  }

  .phead {
    display: flex;
    gap: 10px;
    align-items: baseline;
    flex-wrap: wrap;
  }

  .prop {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    border-radius: var(--r-sm);
    border: 1px dashed var(--border-strong);
  }

  .prop.on {
    border-style: solid;
    border-color: var(--accent);
  }

  .pall {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 8px;
    border-radius: var(--r-sm);
  }

  .pall.on {
    border: 1px solid var(--accent);
    /* 案の一覧を送っても、切り替えは見えたままにする */
    position: sticky;
    top: -6px;
    z-index: 1;
    background: var(--bg-panel);
  }

  .pletter {
    min-width: 18px;
    text-align: center;
    border-radius: 3px;
    border: 1px solid var(--accent-dim);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 600;
  }

  .pletter.cur {
    background: var(--accent);
    color: var(--bg);
  }

  .prow {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }

  .pname {
    font-weight: 600;
  }

  .plabel {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }

  .pnote {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .pwarn {
    color: var(--warn);
    font-size: var(--fs-xs);
  }

  .secchip {
    border-radius: 10px;
    padding: 0 8px;
    font-size: var(--fs-xs);
  }

  .secchip.on {
    border-color: var(--accent);
    color: var(--accent);
  }

  .pbase,
  .abnote {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }

  .pwhy {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }

  .abswitch {
    display: inline-flex;
    gap: 2px;
  }

  .abswitch .btn.on {
    background: var(--accent);
    color: var(--bg);
    border-color: var(--accent);
  }

  .spacer {
    flex: 1;
  }

  .hint {
    color: var(--text-dim);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
</style>
