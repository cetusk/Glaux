<script lang="ts">
  // 設計画面の右の「詳しい情報」: 選んだ所(曲全体・区間・パート・マス)の計画と実際・ずれ・作った元の計画との関係、
  // 選んだ所へのメモ、作り直し・AI に頼む
  import {
    addMemo,
    askChat,
    arcLabels,
    designStore,
    designTargetLabel,
    editPartPlan,
    functionLabels,
    memosFor,
    noteName,
    presenceLabels,
    type DesignCell,
    type DesignData,
    type DesignDeviation,
    type DesignSel,
  } from "./design.svelte";
  import { isEn, plural, tr } from "./i18n.svelte";
  import type { Project } from "./types";

  let {
    project,
    d,
    sel,
    estimated,
    totalBars,
    tempo,
    cellOff,
    offSection,
    onApplyArc,
  }: {
    project: Project;
    d: DesignData;
    sel: DesignSel;
    /** 推定(未確認)の計画がある */
    estimated: boolean;
    totalBars: number;
    /** テンポの表示(曲の途中で変わるなら「途中で変わる」) */
    tempo: string;
    /** 鳴らさない計画のマス・区間か */
    cellOff: (c: DesignCell) => boolean;
    offSection: (i: number) => boolean;
    /** 盛り上がりの型を当てる(曲線の書き換えは設計画面が持つ) */
    onApplyArc: () => void;
  } = $props();

  const functionNames = $derived(functionLabels());
  const presenceNames = $derived(presenceLabels());
  const arcNames = $derived(arcLabels());
  const applyArc = () => onApplyArc();

  const devOf = (pred: (x: DesignDeviation) => boolean) => (d?.deviations ?? []).filter(pred);
  const sectionKey = (i: number) => d?.sections[i]?.id ?? d?.sections[i]?.name ?? "";
  const partClips = (name: string) => Object.values(designStore.clips).filter((c) => c.track === name);

  function askAi() {
    window.dispatchEvent(new CustomEvent("glaux:focus-chat"));
  }

  async function setPartFunction(pi: number, f: string) {
    const p = d?.parts[pi];
    if (!p) return;
    await editPartPlan(
      project,
      pi,
      (b) => {
        if (f) b.function = f;
        else delete b.function;
      },
      tr(`「${p.name}」の働きを「${f ? functionNames[f] : "未設定"}」に`, `Set function of "${p.name}" to "${f ? functionNames[f] : "unset"}"`),
    );
  }

  // ---- メモ・頼む ----
  const memoKey = $derived.by(() => {
    if (!d) return "";
    switch (sel.kind) {
      case "song":
        return "song";
      case "lane":
        return sel.lane;
      case "section":
        return `section:${d.sections[sel.i]?.id ?? d.sections[sel.i]?.name}`;
      case "part":
        return `part:${d.parts[sel.p]?.track_id}`;
      case "cell":
        return `cell:${d.parts[sel.p]?.track_id}:${d.sections[sel.i]?.id ?? d.sections[sel.i]?.name}`;
      default:
        return "";
    }
  });
  const memos = $derived(memosFor(d, memoKey));
  let memoText = $state("");
  async function saveMemo() {
    const t = memoText.trim();
    if (!t || !memoKey) return;
    await addMemo(memoKey, t);
    memoText = "";
  }
  let overwriteArmed = $state(false);
  let overwriteTimer: ReturnType<typeof setTimeout> | undefined;
  function remake(overwrite: boolean) {
    if (overwrite && !overwriteArmed) {
      overwriteArmed = true;
      clearTimeout(overwriteTimer);
      overwriteTimer = setTimeout(() => (overwriteArmed = false), 4000);
      return;
    }
    overwriteArmed = false;
    askChat(
      overwrite
        ? "選んだ所を、計画の今の版から作り直してください。人が手で直した所も含めて上書きしてかまいません(作り直しの道具は overwrite_edits: true。固定の音は残ります)。"
        : "選んだ所を、計画の今の版から作り直してください。人が手で直した小節と固定の音は残し、残した所を報告してください。",
    );
  }
</script>

<aside class="panel" aria-live="polite">
  {#if sel.kind === "none"}
    <h3>{tr("選んでいません", "Nothing selected")}</h3>
    <p class="empty">
      {tr(
        "区間・パート・マスをクリックするか、段の見出しの「全体を選ぶ」で選ぶと、ここに詳しく出ます。何も選んでいないときの AI への指示は、曲全体が対象になります。",
        "Click a section, part or cell, or use \"Select all\" in a lane header, to see details here. With nothing selected, instructions to the AI apply to the whole song.",
      )}
    </p>
    {#if d.deviations.length}
      <h4>{tr(`計画と実際のずれ(${d.deviations.length})`, `Plan vs. actual (${d.deviations.length})`)}</h4>
      {#each d.deviations.slice(0, 8) as x, k (k)}
        <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
      {/each}
    {/if}
  {:else}
    <h3>{designTargetLabel(d, sel)}</h3>
    {#if sel.kind === "song"}
      <dl class="kv">
        <dt>{tr("ジャンル", "Genre")}</dt><dd>{d.song?.genre ?? "―"}</dd>
        <dt>{tr("盛り上がりの型", "Energy arc")}</dt><dd>{d.song?.arc ? arcNames[d.song.arc] : "―"}</dd>
        <dt>{tr("テンポ", "Tempo")}</dt><dd>{tempo}</dd>
        <dt>{tr("守ること", "Must-haves")}</dt><dd>{(d.song?.musts ?? []).join(tr("、", ", ")) || "―"}</dd>
        <dt>{tr("状態", "Status")}</dt>
        <dd>{d.song ? (d.song_estimated ? tr("推定(未確認)", "Estimated (unconfirmed)") : tr("採用済み", "Adopted")) : tr("計画なし", "No plan")}</dd>
      </dl>
      <div class="actions">
        <button
          class="btn sm"
          type="button"
          disabled={!d.song?.arc}
          title={tr(
            "盛り上がりの型に合わせて、区間ごとの盛り上がりの高さを動かす(区間の中の形は残す。取り消せる)",
            "Shift each section's energy level to fit the arc (shapes within sections are kept; can be undone)",
          )}
          onclick={applyArc}>{tr("型を盛り上がりに当てる", "Apply arc to energy")}</button
        >
      </div>
    {:else if sel.kind === "lane"}
      {#if sel.lane === "curve"}
        {@const hi = d.sections.reduce((a, s, i) => (s.measured > (d.sections[a]?.measured ?? -1) ? i : a), 0)}
        <dl class="kv">
          <dt>{tr("区間", "Sections")}</dt><dd>{d.sections.length}</dd>
          <dt>{tr("いちばん高い(実測)", "Highest (measured)")}</dt><dd>{d.sections[hi]?.name}({d.sections[hi]?.measured.toFixed(1)})</dd>
          <dt>{tr("段差", "Steps")}</dt>
          <dd>
            {d.sections
              .filter((s, i) => s.join === "step" && i < d.sections.length - 1)
              .map((s) => tr(`${s.name} → 次`, `${s.name} → next`))
              .join(tr("、", ", ")) || tr("なし", "None")}
          </dd>
          <dt>{tr("盛り上がりの型", "Energy arc")}</dt><dd>{d.song?.arc ? arcNames[d.song.arc] : "―"}</dd>
        </dl>
        {#each d.sections.filter((_, i) => offSection(i)) as s (s.name)}
          <div class="finding warn">
            <span class="dot"></span><span
              >{tr(
                `「${s.name}」が計画とずれている(計画 ${s.planned?.toFixed(1)} / 実測 ${s.measured.toFixed(1)})`,
                `"${s.name}" is off from the plan (plan ${s.planned?.toFixed(1)} / measured ${s.measured.toFixed(1)})`,
              )}</span
            >
          </div>
        {/each}
      {:else if sel.lane === "band"}
        <dl class="kv">
          <dt>{tr("パート", "Parts")}</dt><dd>{d.parts.length}</dd>
          <dt>{tr("計画のあるパート", "Parts with a plan")}</dt><dd>{d.parts.filter((p) => p.plan_id).length}</dd>
        </dl>
        {#each devOf((x) => !!x.track && (x.what.includes("音域") || x.what.includes("register"))) as x, k (k)}
          <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
        {/each}
      {:else if sel.lane === "table"}
        <dl class="kv">
          <dt>{tr("大きさ", "Size")}</dt><dd>{tr(`${d.parts.length} パート × ${d.sections.length} 区間`, `${plural(d.parts.length, "part")} × ${plural(d.sections.length, "section")}`)}</dd>
          <dt>{tr("固定", "Locked")}</dt><dd>{d.parts.reduce((a, p) => a + p.cells.filter((c) => c.locked).length, 0)} {tr("マス", "cells")}</dd>
          <dt>{tr("ずれ", "Off")}</dt><dd>{d.parts.reduce((a, p) => a + p.cells.filter(cellOff).length, 0)} {tr("マス", "cells")}</dd>
        </dl>
      {:else}
        <dl class="kv">
          <dt>{tr("並び", "Order")}</dt><dd>{d.sections.map((s) => `${s.name}(${s.bars})`).join(" → ")}</dd>
          <dt>{tr("長さ", "Length")}</dt><dd>{tr(`${totalBars} 小節`, `${plural(totalBars, "bar")}`)}</dd>
        </dl>
      {/if}
    {:else if sel.kind === "section"}
      {@const s = d.sections[sel.i]}
      {#if s}
        <dl class="kv">
          <dt>{tr("位置", "Position")}</dt><dd>{tr(`${s.start_bar} 小節目から ${s.bars} 小節`, `${plural(s.bars, "bar")} from bar ${s.start_bar}`)}</dd>
          <dt>{tr("盛り上がり", "Energy")}</dt>
          <dd>{tr("計画", "Plan")} {s.planned != null ? s.planned.toFixed(1) : "―"} / {tr("実測", "Measured")} {s.measured.toFixed(1)}</dd>
          <dt>{tr("次との境目", "Next boundary")}</dt><dd>{sel.i < d.sections.length - 1 ? (s.join === "step" ? tr("段差", "Step") : tr("つなぐ", "Smooth")) : "―"}</dd>
          <dt>{tr("意図", "Intent")}</dt><dd>{project.sections?.find((m) => m.name === s.name)?.note ?? "―"}</dd>
        </dl>
        {#each devOf((x) => x.section === sectionKey(sel.i)) as x, k (k)}
          <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
        {/each}
      {/if}
    {:else}
      {@const p = d.parts[sel.p]}
      {#if p}
        {#if sel.kind === "cell"}
          {@const c = p.cells[sel.i]}
          <dl class="kv">
            <dt>{tr("働き", "Function")}</dt><dd>{c.function ? functionNames[c.function] : p.function ? functionNames[p.function] : "―"}</dd>
            <dt>{tr("存在(計画)", "Presence (plan)")}</dt><dd>{c.planned != null ? `${c.planned} ${presenceNames[c.planned]}` : tr("計画なし", "No plan")}</dd>
            <dt>{tr("存在(実測)", "Presence (measured)")}</dt><dd>{c.measured} {presenceNames[c.measured]}{tr(`(${c.notes} 音)`, ` (${plural(c.notes, "note")})`)}</dd>
            <dt>{tr("音域(計画)", "Register (plan)")}</dt><dd>{c.planned_register ? `${noteName(c.planned_register[0])}〜${noteName(c.planned_register[1])}` : "―"}</dd>
            <dt>{tr("音域(実際)", "Register (actual)")}</dt><dd>{c.register ? `${noteName(c.register[0])}〜${noteName(c.register[1])}` : "―"}</dd>
            <dt>{tr("密度", "Density")}</dt><dd>{c.density == null ? "" : Math.round(c.density * 100) / 100}</dd>
            <dt>{tr("固定", "Locked")}</dt><dd>{c.locked ? tr("🔒 この区間は固定", "🔒 This section is locked") : tr("なし", "No")}</dd>
          </dl>
          {#each devOf((x) => x.track === p.name && x.section === c.section) as x, k (k)}
            <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
          {/each}
        {:else}
          <dl class="kv">
            <dt>{tr("働き", "Function")}</dt>
            <dd>
              <select
                value={p.function ?? ""}
                aria-label={tr("このパートの既定の働き", "Default function of this part")}
                onchange={(e) => setPartFunction(sel.kind === "part" ? sel.p : 0, (e.currentTarget as HTMLSelectElement).value)}
                ><option value="">―</option>{#each Object.entries(functionNames) as [k, n] (k)}<option value={k}>{n}</option>{/each}</select
              >
            </dd>
            <dt>{tr("計画", "Plan")}</dt>
            <dd>
              {p.plan_id
                ? p.estimated
                  ? tr("推定(未確認)", "Estimated (unconfirmed)")
                  : tr("採用済み", "Adopted")
                : tr("なし(直すと作られる)", "None (created when you edit)")}
            </dd>
            <dt>{tr("鳴っている区間", "Sections playing")}</dt><dd>{p.cells.filter((c) => c.notes > 0).length} / {p.cells.length}</dd>
          </dl>
          {#each devOf((x) => x.track === p.name) as x, k (k)}
            <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
          {/each}
        {/if}
        {#if partClips(p.name).length}
          <h4>{tr("クリップの状態", "Clip status")}</h4>
          {#each partClips(p.name) as c (c.clip_id)}
            <div class="clipstate">
              {#if c.plan === "ahead"}<span class="badge ahead">{tr("計画が先に進んだ", "Plan is ahead")}</span>{:else if c.plan === "in_sync"}<span class="badge"
                  >{tr("計画どおり", "Matches plan")}</span
                >{/if}
              {#if c.edited_bars?.length}<span class="badge edited"
                  >{tr(
                    `手で直した: ${c.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}〜${b}`)).join("・")} 小節`,
                    `Hand-edited bars: ${c.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}–${b}`)).join(", ")}`,
                  )}</span
                >{/if}
              {#if c.locked_notes}<span class="badge">🔒 {tr(`${c.locked_notes} 音`, `${plural(c.locked_notes, "note")}`)}</span>{/if}
            </div>
          {/each}
        {/if}
      {/if}
    {/if}
    <!-- メモ: 所に付く人の言葉(AI はその所を作る・直すときに読む)。AI とのやりとりは下のチャット -->
    <div class="memo-box">
      <h4>{tr(`メモ(${memos.length})`, `Notes (${memos.length})`)}</h4>
      {#if memos.length}
        <div class="thread">
          {#each memos as m, k (k)}
            <div class="memo">
              <div class="who">
                {tr("あなた", "You")}{m.when
                  ? ` · ${new Date(m.when).toLocaleString(isEn() ? "en-US" : "ja-JP", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })}`
                  : ""}
              </div>
              {m.text}
            </div>
          {/each}
        </div>
      {:else}
        <p class="empty">{tr("この所のメモはまだありません。AI は作る・直すときにここを読みます", "No notes here yet. The AI reads these when creating or editing")}</p>
      {/if}
      <textarea bind:value={memoText} rows="2" placeholder={tr("例: ここはもっと沈んだ感じに / ハットは抜きたい", "e.g. Make this feel darker / drop the hi-hat")}></textarea>
      <div class="actions">
        <button class="btn sm" type="button" disabled={!memoText.trim()} onclick={saveMemo}>{tr("メモを残す", "Add note")}</button>
        <button class="btn sm" type="button" onclick={askAi} title={tr("下のチャットの入力欄へ(選んだ所が対象として添わる)", "Go to the chat input below (the selection is attached as the target)")}
          >{tr("この所について AI に頼む ↓", "Ask AI about this ↓")}</button
        >
      </div>
    </div>
    <div class="actions two">
      <button
        class="btn sm"
        type="button"
        title={tr("計画の今の版から作り直すよう AI に頼む(手で直した小節と固定の音は残す)", "Ask the AI to regenerate from the current plan (keeps hand-edited bars and locked notes)")}
        onclick={() => remake(false)}>{tr("この版から作り直す", "Regenerate from plan")}</button
      >
      <button
        class="btn sm"
        class:danger={overwriteArmed}
        type="button"
        title={tr(
          "手で直した所も含めて作り直すよう AI に頼む(固定の音は残る)。押すと確かめの表示になり、もう一度で送る",
          "Ask the AI to regenerate including hand edits (locked notes are kept). Click once to arm, again to send",
        )}
        onclick={() => remake(true)}>{overwriteArmed ? tr("もう一度押すと送ります", "Click again to send") : tr("手直しも含めて上書き", "Overwrite hand edits")}</button
      >
    </div>
  {/if}
</aside>

<style>
  /* 詳しい情報 */
  .panel {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    min-height: 0;
    overflow-y: auto;
  }

  .panel h3 {
    margin: 0;
    font-size: var(--fs-md);
  }

  .panel h4 {
    margin: 4px 0 0;
    font-size: var(--fs-sm);
    color: var(--text-dim);
    font-weight: 600;
  }

  .empty {
    color: var(--text-faint);
    font-size: var(--fs-sm);
    margin: 0;
  }

  .kv {
    display: grid;
    grid-template-columns: 96px 1fr;
    gap: 4px 8px;
    font-size: var(--fs-sm);
    margin: 0;
  }

  .kv dt {
    color: var(--text-dim);
  }

  .kv dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .finding {
    display: flex;
    gap: 6px;
    font-size: var(--fs-sm);
    color: var(--text-dim);
  }

  .finding.info .dot {
    background: var(--text-faint);
  }
  .finding .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn);
    margin-top: 6px;
    flex: none;
  }

  .clipstate {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .badge {
    font-size: var(--fs-xs);
    border: 1px solid var(--border-strong);
    border-radius: 9px;
    padding: 0 6px;
    color: var(--text-dim);
  }

  .badge.ahead {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .badge.edited {
    border-color: var(--human);
    color: var(--human);
  }

  .actions {
    display: flex;
    gap: 6px;
  }

  .panel select {
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    padding: 3px 6px;
    min-width: 0;
  }

  .memo-box {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .thread {
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-height: 160px;
    overflow-y: auto;
    font-size: var(--fs-sm);
  }

  .memo {
    border-left: 2px solid var(--border-strong);
    padding: 2px 0 2px 8px;
  }

  .memo .who {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  .memo-box textarea {
    width: 100%;
    resize: vertical;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    padding: 6px 8px;
    font: inherit;
    font-size: var(--fs-sm);
  }

  .actions {
    flex-wrap: wrap;
  }

  .actions.two .btn {
    flex: 1;
  }
</style>
