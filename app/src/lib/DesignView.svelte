<script lang="ts">
  // 設計画面(曲の設計データ)。段階 1 は見るだけ: 曲全体の計画、区間ごとの計画(盛り上がり・パートごとの音域・
  // パートの役割)と測った値・ずれ、選んだ所の詳しい情報。直すのはチャットで AI に頼む(画面で直すのは段階 2)。
  // 見た目と操作は docs/SONG_DESIGN_DATA.md §5(触れる試作で決めたもの)に沿う。
  import { tick } from "svelte";
  import Icon from "./Icon.svelte";
  import type { Project } from "./types";
  import {
    ARCS,
    FUNCTIONS,
    PRESENCE,
    designSel,
    designStore,
    designTargetLabel,
    noteName,
    sectionCurve,
    type DesignCell,
    type DesignDeviation,
    type DesignSel,
  } from "./design.svelte";

  let { project }: { project: Project } = $props();

  const d = $derived(designStore.data);
  const sel = $derived(designSel.sel);

  // ---- 開け閉め・大きさ(覚えておく) ----
  function load(key: string, fallback: string): string {
    try {
      return localStorage.getItem(key) ?? fallback;
    } catch {
      return fallback;
    }
  }
  function store(key: string, v: string) {
    try {
      localStorage.setItem(key, v);
    } catch {
      // 保存できなくても動作には関係しない
    }
  }
  type Lane = "song" | "curve" | "band" | "table";
  let folded = $state<Record<Lane, boolean>>({
    song: load("glaux.design.fold.song", "1") === "1",
    curve: load("glaux.design.fold.curve", "0") === "1",
    band: load("glaux.design.fold.band", "0") === "1",
    table: load("glaux.design.fold.table", "0") === "1",
  });
  function toggleFold(l: Lane, e: MouseEvent) {
    if ((e.target as HTMLElement).closest(".tools, .legend, .qmark, .pickall, input, select, label")) return;
    folded[l] = !folded[l];
    store(`glaux.design.fold.${l}`, folded[l] ? "1" : "0");
  }

  const CT = 14;
  const CB = 22;
  let CH = $state(Number(load("glaux.design.curveH", "130")) || 130);
  const CP = $derived(CH - CT - CB);
  const cy = (v: number) => CT + (10 - v) * (CP / 10);
  const PMIN = 12;
  const PMAX = 108;
  let BH = $state(Number(load("glaux.design.bandH", "300")) || 300);
  const BY = (p: number) => BH - ((p - PMIN) / (PMAX - PMIN)) * BH;
  const ROW_MIN = 24;
  const ROW_MAX = 64;
  let tableH = $state(Number(load("glaux.design.tableBodyH", "0")) || 0);
  const nParts = $derived(Math.max(1, d?.parts.length ?? 1));
  const tableBody = $derived(tableH > 0 ? tableH : 38 * nParts);
  const rowH = $derived(Math.max(ROW_MIN, Math.min(ROW_MAX, Math.floor(tableBody / nParts))));
  let showMeasured = $state(load("glaux.design.measured", "1") === "1");
  let showOverlap = $state(false);

  // ---- 横の位置。既定は曲全体が見える幅(全体)。拡大・縮小はタイムラインと別に持つ(設計は曲全体を見渡すことが多いため)
  const totalBars = $derived(Math.max(1, ...(d?.sections ?? []).map((s) => s.start_bar - 1 + s.bars)));
  let visW = $state(600);
  let zoom = $state<number | null>(Number(load("glaux.design.pxPerBar", "0")) || null);
  const fitPx = $derived(Math.max(4, visW / totalBars));
  const pxb = $derived(zoom ?? fitPx);
  function setZoom(z: number | null) {
    zoom = z == null ? null : Math.max(4, Math.min(160, z));
    store("glaux.design.pxPerBar", String(zoom ?? 0));
  }
  const W = $derived(Math.max(visW, Math.round(totalBars * pxb)));
  const x0 = (i: number) => ((d?.sections[i]?.start_bar ?? 1) - 1) * pxb;
  const xw = (i: number) => (d?.sections[i]?.bars ?? 1) * pxb;
  let sx = $state(0);
  let hbar = $state<HTMLDivElement>();
  function onHbar() {
    sx = hbar?.scrollLeft ?? 0;
  }
  function onWheel(e: WheelEvent) {
    const dx = e.shiftKey ? e.deltaY : e.deltaX;
    if (!dx || !hbar) return;
    e.preventDefault();
    hbar.scrollLeft += dx;
  }

  // ---- 選ぶ ----
  function pick(s: DesignSel) {
    designSel.sel = s;
    if (s.kind === "part" || s.kind === "cell") designSel.part = s.p;
  }
  function pickWhole(k: "song" | "sections" | "curve" | "band" | "table", e: MouseEvent) {
    e.stopPropagation();
    const on = k === "song" ? sel.kind === "song" : sel.kind === "lane" && sel.lane === k;
    // 「全体を選択中」をもう一度押すと選択を外す
    pick(on ? { kind: "none" } : k === "song" ? { kind: "song" } : { kind: "lane", lane: k });
  }
  const isWhole = (k: string) => (k === "song" ? sel.kind === "song" : sel.kind === "lane" && sel.lane === k);
  const selPart = $derived(Math.min(designSel.part, Math.max(0, (d?.parts.length ?? 1) - 1)));

  // ---- ドラッグ(大きさの変更)。外で離して終わりの知らせが届かなくても、戻ってきたときボタンが離れていれば終える ----
  let dragging = $state(false);
  function track(move: (e: PointerEvent) => void, done: () => void) {
    dragging = true;
    let ended = false;
    const mv = (e: PointerEvent) => {
      if (e.buttons === 0) return end();
      move(e);
    };
    const end = () => {
      if (ended) return;
      ended = true;
      dragging = false;
      window.removeEventListener("pointermove", mv);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
      done();
    };
    window.addEventListener("pointermove", mv);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
  }
  // 縮めたときに枠のスクロールが詰まってつまみがカーソルから離れないよう、下に一時的な余白を足す(上へスクロールすると消える)
  let group = $state<HTMLDivElement>();
  let slack = $state(0);
  function fitSlack(grow: boolean, top?: number) {
    const g = group;
    if (!g) return;
    const t = top ?? g.scrollTop;
    const content = g.scrollHeight - slack;
    let need = Math.max(0, t + g.clientHeight - content);
    if (!grow) need = Math.min(need, slack);
    slack = need;
    if (top != null) queueMicrotask(() => g && (g.scrollTop = top));
  }
  function grip(kind: "curve" | "band" | "table", e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const y0 = e.clientY;
    const v0 = kind === "curve" ? CH : kind === "band" ? BH : tableBody;
    track(
      async (m) => {
        const t = group?.scrollTop ?? 0;
        const v = v0 + m.clientY - y0;
        if (kind === "curve") CH = Math.round(Math.max(130, Math.min(480, v)));
        else if (kind === "band") BH = Math.round(Math.max(160, Math.min(900, v)));
        else tableH = Math.round(Math.max(80, Math.min(ROW_MAX * nParts, v)));
        await tick();
        fitSlack(true, t);
      },
      () => {
        store("glaux.design.curveH", String(CH));
        store("glaux.design.bandH", String(BH));
        store("glaux.design.tableBodyH", String(tableH));
      },
    );
  }
  function resetGrip(kind: "curve" | "band" | "table") {
    if (kind === "curve") CH = 130;
    else if (kind === "band") BH = 300;
    else tableH = 0;
    store("glaux.design.curveH", String(CH));
    store("glaux.design.bandH", String(BH));
    store("glaux.design.tableBodyH", String(tableH));
    tick().then(() => fitSlack(false));
  }

  // ---- 計画と実際 ----
  const avgCurve = (c: [number, number][]) => {
    const N = 32;
    let sum = 0;
    for (let k = 0; k <= N; k++) sum += valueAt(c, k / N);
    return sum / (N + 1);
  };
  function valueAt(c: [number, number][], t: number): number {
    if (t <= c[0][0]) return c[0][1];
    for (let k = 1; k < c.length; k++) {
      const [a, va] = c[k - 1];
      const [b, vb] = c[k];
      if (t <= b) return b > a ? va + ((vb - va) * (t - a)) / (b - a) : vb;
    }
    return c[c.length - 1][1];
  }
  const offSection = (i: number) => {
    const s = d?.sections[i];
    return !!s && s.planned != null && Math.abs(s.measured - s.planned) >= 1.5;
  };
  function cellOff(c: DesignCell): boolean {
    if (c.planned == null) return false;
    if (c.planned === 0) return c.notes > 0;
    if (c.notes === 0) return true;
    if (Math.abs(c.planned - c.measured) >= 2) return true;
    if (c.planned_register && c.register) {
      const [lo, hi] = c.planned_register;
      return c.register[0] < lo - 2 || c.register[1] > hi + 2;
    }
    return false;
  }
  const estimated = $derived(!!d && (d.song_estimated || d.parts.some((p) => p.estimated)));
  const tempo = $derived.by(() => {
    const t = project.tempo_map ?? [];
    if (t.length > 1) return "途中で変わる";
    return `${t[0]?.bpm ?? 120} BPM`;
  });

  // パートの色(タイムラインで付けた色。無ければ既定の灰色)
  const colorOf = (i: number) => d?.parts[i]?.color ?? "#9a9a9a";
  // 強いパートどうしの重なり(参考)
  const overlaps = $derived.by(() => {
    if (!d || !showOverlap) return [];
    const out: { i: number; lo: number; hi: number }[] = [];
    d.sections.forEach((_, i) => {
      const strong = d.parts.map((p) => p.cells[i]).filter((c) => c && (c.planned ?? 0) >= 3 && c.planned_register);
      for (let a = 0; a < strong.length; a++)
        for (let b = a + 1; b < strong.length; b++) {
          const [l1, h1] = strong[a].planned_register!;
          const [l2, h2] = strong[b].planned_register!;
          const lo = Math.max(l1, l2);
          const hi = Math.min(h1, h2);
          if (hi > lo) out.push({ i, lo, hi });
        }
    });
    return out;
  });

  // ---- 詳しい情報(右のパネル) ----
  const devOf = (pred: (x: DesignDeviation) => boolean) => (d?.deviations ?? []).filter(pred);
  const sectionKey = (i: number) => d?.sections[i]?.id ?? d?.sections[i]?.name ?? "";
  const partClips = (name: string) => Object.values(designStore.clips).filter((c) => c.track === name);

  function askAi() {
    window.dispatchEvent(new CustomEvent("glaux:focus-chat"));
  }

  let lanesEl = $state<HTMLDivElement>();
  // 見える幅(曲全体が見える縮尺の計算に使う)。中身を読み込んでから段ができるので、段ができたら測る
  $effect(() => {
    const el = lanesEl;
    if (!el) return;
    const measure = () => {
      const w = el.querySelector(".hwin")?.clientWidth;
      if (w) visW = w;
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  });
</script>

<div class="design" class:dragging>
  {#if designStore.error && !d}
    <div class="empty-all">設計データを読めませんでした: {designStore.error}</div>
  {:else if !d}
    <div class="empty-all">読み込み中…</div>
  {:else}
    {#if estimated}
      <div class="estimate" role="status">
        <b>推定した計画(未確認)があります</b>
        <span
          >今の音から推定した計画です。採用するまで、AI は参考としてだけ使います。採用・直してから採用・捨てるは、次の段階で画面からできるようになります(今はチャットで頼めます)。</span
        >
      </div>
    {/if}
    <div class="main">
      <div class="lanes" bind:this={lanesEl}>
        <!-- 曲全体 -->
        <section class="lane song" class:collapsed={folded.song} class:on={sel.kind === "song"}>
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="lanehead main-head" onclick={(e) => toggleFold("song", e)}>
            <button class="fold" type="button" aria-label="折りたたむ・開く" aria-expanded={!folded.song}
              ><Icon name="chevron-down" size={14} /></button
            >
            <h2>曲全体</h2>
            <button class="btn sm pickall" class:on={isWhole("song")} type="button" onclick={(e) => pickWhole("song", e)}
              >{isWhole("song") ? "全体を選択中" : "全体を選ぶ"}</button
            >
            {#if folded.song}
              <span class="summary"
                >{[d.song?.genre, (d.song?.mood ?? []).join("・"), d.song?.key, tempo, `${totalBars} 小節`, d.song?.arc ? ARCS[d.song.arc] : null]
                  .filter(Boolean)
                  .join(" · ")}</span
              >
            {/if}
            {#if d.song_estimated}<span class="tag">推定</span>{/if}
          </div>
          {#if !folded.song}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="songgrid" onclick={() => pick({ kind: "song" })}>
              <div class="field"><span>ジャンル</span><div>{d.song?.genre ?? "―"}</div></div>
              <div class="field">
                <span>雰囲気の言葉</span>
                <div class="tags">
                  {#each d.song?.mood ?? [] as m (m)}<span class="word">{m}</span>{:else}―{/each}
                </div>
              </div>
              <div class="field"><span>キー・旋法</span><div>{d.song?.key ?? "―"}</div></div>
              <div class="field"><span>テンポ</span><div>{tempo} <small>(タイムラインと同じ値)</small></div></div>
              <div class="field"><span>長さ</span><div>{totalBars} 小節 <small>(タイムラインから)</small></div></div>
              <div class="field"><span>盛り上がりの型</span><div>{d.song?.arc ? ARCS[d.song.arc] : "―"}</div></div>
              {#each [["明るさ", d.song?.brightness, "暗い", "明るい"], ["音の密度", d.song?.density, "まばら", "ぎっしり"], ["質感", d.song?.organic, "無機質", "有機的"]] as [name, v, lo, hi] (name)}
                <div class="field">
                  <span>{name}</span>
                  {#if v != null}
                    <div class="meter" title="{v} / 10"><i style="width:{(Number(v) / 10) * 100}%"></i></div>
                    <div class="ends"><span>{lo}</span><span>{hi}</span></div>
                  {:else}<div>―</div>{/if}
                </div>
              {/each}
              <div class="field"><span>音量の目標</span><div>{d.song?.loudness != null ? `${d.song.loudness} LUFS` : "―"}</div></div>
              <div class="field">
                <span>守ること</span>
                <div class="tags">
                  {#each d.song?.musts ?? [] as m (m)}<span class="word must">{m}</span>{:else}―{/each}
                </div>
              </div>
              <div class="field"><span>参考曲</span><div>{(d.song?.refs ?? []).join("、") || "―"}</div></div>
              <div class="field wide"><span>全体のメモ</span><div>{d.song?.note ?? "―"}</div></div>
              {#if !d.song}
                <div class="field wide hint-row">
                  曲全体の計画はまだありません。チャットで「曲全体の狙いを決めよう」のように頼むと、AI が書きます(画面で書くのは次の段階)。
                </div>
              {/if}
            </div>
          {/if}
        </section>

        <!-- 区間ごとの計画(時間の流れ)。縦のスクロールはこの枠の 1 つだけ -->
        <div class="timegroup" bind:this={group} onscroll={() => fitSlack(false)}>
          <div class="gtop">
            <div class="grouphead">
              区間ごとの計画<span>左から右へ曲の時間の流れ</span>
              <div class="zoom" role="group" aria-label="横の縮尺">
                <button class="btn sm" class:on={zoom == null} type="button" title="曲全体が見える幅にする" onclick={() => setZoom(null)}>全体</button>
                <button class="btn sm icon" type="button" title="縮小" aria-label="縮小" onclick={() => setZoom(pxb / 1.5)}><Icon name="zoom-out" size={13} /></button>
                <button class="btn sm icon" type="button" title="拡大" aria-label="拡大" onclick={() => setZoom(pxb * 1.5)}><Icon name="zoom-in" size={13} /></button>
              </div>
            </div>
            <section class="lane">
              <div class="lbody">
                <div class="labels">
                  <div class="lab1 sechead" class:on={isWhole("sections")}>
                    区間<button class="btn sm pickall mini" class:on={isWhole("sections")} type="button" onclick={(e) => pickWhole("sections", e)}
                      >{isWhole("sections") ? "全体を選択中" : "全体を選ぶ"}</button
                    >
                  </div>
                </div>
                <div class="hwin" onwheel={onWheel}>
                  <div class="hcontent ruler" style="width:{W}px;transform:translateX({-sx}px)">
                    {#each d.sections as s, i (i)}
                      <button
                        class="sec"
                        class:sel={sel.kind === "section" && sel.i === i}
                        style="left:{x0(i)}px;width:{xw(i)}px"
                        type="button"
                        title={s.name}
                        onclick={() => pick({ kind: "section", i })}><b>{s.name}</b> {s.bars}</button
                      >
                    {/each}
                  </div>
                </div>
              </div>
            </section>
          </div>

          <!-- 盛り上がり -->
          <section class="lane" class:collapsed={folded.curve} class:on={isWhole("curve")}>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="lanehead main-head" onclick={(e) => toggleFold("curve", e)}>
              <button class="fold" type="button" aria-label="折りたたむ・開く" aria-expanded={!folded.curve}
                ><Icon name="chevron-down" size={14} /></button
              >
              <h2>盛り上がり</h2>
              <button class="btn sm pickall" class:on={isWhole("curve")} type="button" onclick={(e) => pickWhole("curve", e)}
                >{isWhole("curve") ? "全体を選択中" : "全体を選ぶ"}</button
              >
              {#if folded.curve}
                <span class="summary">区間ごとの盛り上がりの計画と実測(開くと見えます)</span>
              {:else}
                <button type="button" class="qmark" aria-label="説明" title="区間ごとの盛り上がりの計画(実線)と実測(点線)。境目の印はつなぐ / 段差。● は実測とずれた区間。&#10;直すのは次の段階で(今はチャットで頼めます)">?</button>
                <div class="legend">
                  <label
                    ><input
                      type="checkbox"
                      bind:checked={showMeasured}
                      onchange={() => store("glaux.design.measured", showMeasured ? "1" : "0")}
                    /> 実測を表示</label
                  >
                </div>
              {/if}
            </div>
            {#if !folded.curve}
              <div class="lbody">
                <div class="labels" style="height:{CH}px">
                  <div class="cvlegend" class:nomeasure={!showMeasured} style="top:{CT}px;height:{cy(0) - CT}px">
                    <div title="計画した盛り上がり"><i class="ln"></i>計画</div>
                    <div class="m" title="今の音から測った盛り上がり"><i class="ln dash"></i>実測</div>
                    <hr />
                    <div title="次の区間となめらかにつなぐ境目"><span class="mk">⌇</span>つなぐ</div>
                    <div title="次の区間で急に変わる境目"><span class="mk warn">↕</span>段差</div>
                    <div class="m" title="計画と実測が 1.5 以上ずれた区間(上の点)"><span class="dot"></span>ずれ</div>
                  </div>
                  <div class="axisline" style="top:{CT}px;height:{cy(0) - CT}px"></div>
                  {#each [0, 2, 4, 6, 8, 10] as v (v)}
                    <span class="axis" class:edge={v === 0 || v === 10} style="top:{cy(v)}px">{v}</span>
                  {/each}
                </div>
                <div class="hwin" onwheel={onWheel}>
                  <div class="hcontent" style="width:{W}px;transform:translateX({-sx}px)">
                    <svg width={W} height={CH} viewBox="0 0 {W} {CH}" role="img" aria-label="盛り上がりの計画と実測">
                      <rect x="0" y="0" width={W} height={CT} class="outside" />
                      <rect x="0" y={cy(0)} width={W} height={CH - cy(0)} class="outside" />
                      {#each [0, 2, 4, 6, 8, 10] as v (v)}
                        <line x1="0" x2={W} y1={cy(v)} y2={cy(v)} class={v === 0 || v === 10 ? "edge" : "grid"} />
                      {/each}
                      {#each d.sections as s, i (i)}
                        {@const c = sectionCurve(s)}
                        {@const X = (t: number) => x0(i) + t * xw(i)}
                        {@const on = sel.kind === "section" && sel.i === i}
                        <line x1={x0(i)} x2={x0(i)} y1="0" y2={CH} class="secline" />
                        <!-- svelte-ignore a11y_click_events_have_key_events -->
                        <!-- svelte-ignore a11y_no_static_element_interactions -->
                        <rect x={x0(i)} y="0" width={xw(i)} height={CH} fill="transparent" onclick={() => pick({ kind: "section", i })} />
                        {#if c}
                          {@const path = c.map(([t, v], k) => `${k ? "L" : "M"}${X(t)},${cy(v)}`).join(" ")}
                          <path d="{path} L{X(1)},{cy(0)} L{X(0)},{cy(0)} Z" class="area" class:on pointer-events="none" />
                          <path d={path} class="plan" pointer-events="none" />
                          {#each c as [t, v], k (k)}
                            <circle cx={X(t)} cy={cy(v)} r={on ? 4.5 : 3} class="pt" class:on pointer-events="none" />
                          {/each}
                        {/if}
                        {#if showMeasured}
                          <line x1={x0(i) + 2} x2={x0(i) + xw(i) - 2} y1={cy(s.measured)} y2={cy(s.measured)} class="measured" />
                          {#if offSection(i)}
                            <circle cx={X(0.5)} cy="7" r="3.5" class="offdot"><title>実測とずれている(計画 {s.planned?.toFixed(1)} / 実測 {s.measured.toFixed(1)})</title></circle>
                          {/if}
                        {/if}
                        {#if i < d.sections.length - 1}
                          {@const bx = x0(i + 1)}
                          <g class="join" class:step={s.join === "step"}>
                            <rect x={bx - 8} y={CH - 15} width="16" height="12" rx="3" />
                            <text x={bx} y={CH - 6}>{s.join === "step" ? "↕" : "⌇"}</text>
                            <title>{s.join === "step" ? "段差(急に変わる)" : "つなぐ(なめらか)"}</title>
                          </g>
                        {/if}
                      {/each}
                    </svg>
                  </div>
                </div>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="vgrip"
                role="separator"
                aria-orientation="horizontal"
                title="ドラッグで盛り上がりの段の高さを変える(ダブルクリックで元に戻す)"
                onpointerdown={(e) => grip("curve", e)}
                ondblclick={() => resetGrip("curve")}
              ></div>
            {/if}
          </section>

          <!-- パートごとの音域 -->
          <section class="lane" class:collapsed={folded.band} class:on={isWhole("band")}>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="lanehead main-head" onclick={(e) => toggleFold("band", e)}>
              <button class="fold" type="button" aria-label="折りたたむ・開く" aria-expanded={!folded.band}
                ><Icon name="chevron-down" size={14} /></button
              >
              <h2>パートごとの音域</h2>
              <button class="btn sm pickall" class:on={isWhole("band")} type="button" onclick={(e) => pickWhole("band", e)}
                >{isWhole("band") ? "全体を選択中" : "全体を選ぶ"}</button
              >
              {#if folded.band}
                <span class="summary">選んでいるパート: {d.parts[selPart]?.name ?? "―"}</span>
              {:else}
                <button type="button" class="qmark" aria-label="説明" title="左の一覧(か役割の段のパート名)でパートを選ぶと、そのパートの音域の帯(計画)が色付きで前に出る。点線の枠は実際の音域(下 10%〜上 90%)。&#10;下の端のつまみで段の高さ(音高の縦の大きさ)を変える">?</button>
                <div class="legend">
                  <label title="強いパートどうしの計画の音域の重なりを斜線で(参考。警告ではない)"
                    ><input type="checkbox" bind:checked={showOverlap} /> 重なり</label
                  >
                </div>
              {/if}
            </div>
            {#if !folded.band}
              <div class="bandlane">
                <div class="labels partlist" style="max-height:{BH}px">
                  {#each d.parts as p, pi (p.track_id)}
                    <button
                      class="partbtn"
                      class:on={pi === selPart}
                      type="button"
                      onclick={() => pick({ kind: "part", p: pi })}
                      ><span class="chip" style="background:{colorOf(pi)}"></span>{p.name}</button
                    >
                  {:else}
                    <div class="emptyrow">MIDI のトラックがありません</div>
                  {/each}
                </div>
                <div class="hwin" onwheel={onWheel}>
                  <div class="octaves" style="height:{BH}px">
                    {#each [24, 36, 48, 60, 72, 84, 96] as p (p)}
                      <span class="octave" style="top:{BY(p)}px">C{p / 12 - 1}</span>
                    {/each}
                  </div>
                  <div class="hcontent" style="width:{W}px;transform:translateX({-sx}px)">
                    <svg width={W} height={BH} viewBox="0 0 {W} {BH}" role="img" aria-label="パートごとの音域">
                      <defs>
                        <pattern id="glaux-hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
                          <line x1="0" y1="0" x2="0" y2="6" class="hatch" />
                        </pattern>
                      </defs>
                      {#each [24, 36, 48, 60, 72, 84, 96] as p (p)}
                        <line x1="0" x2={W} y1={BY(p)} y2={BY(p)} class="grid" />
                      {/each}
                      {#each d.sections as _, i (i)}
                        <line x1={x0(i)} x2={x0(i)} y1="0" y2={BH} class="secline" />
                      {/each}
                      <!-- 選んでいないパート: 計画の帯の薄い輪郭だけ -->
                      {#each d.parts as p, pi (p.track_id)}
                        {#if pi !== selPart}
                          {#each p.cells as c, i (i)}
                            {#if c.planned_register && (c.planned ?? 0) > 0}
                              <rect
                                x={x0(i) + 2}
                                width={Math.max(0, xw(i) - 4)}
                                y={BY(c.planned_register[1])}
                                height={BY(c.planned_register[0]) - BY(c.planned_register[1])}
                                rx="2"
                                class="other"
                              />
                            {/if}
                          {/each}
                        {/if}
                      {/each}
                      {#each overlaps as o, k (k)}
                        <rect x={x0(o.i) + 3} width={Math.max(0, xw(o.i) - 6)} y={BY(o.hi)} height={BY(o.lo) - BY(o.hi)} fill="url(#glaux-hatch)" />
                      {/each}
                      <!-- 選んだパート: 計画の帯を色付きで、実際の音域を点線の枠で -->
                      {#if d.parts[selPart]}
                        {@const p = d.parts[selPart]}
                        {#each p.cells as c, i (i)}
                          {@const on = sel.kind === "cell" && sel.p === selPart && sel.i === i}
                          {#if c.planned_register && (c.planned ?? 0) > 0}
                            <!-- svelte-ignore a11y_click_events_have_key_events -->
                            <!-- svelte-ignore a11y_no_static_element_interactions -->
                            <rect
                              x={x0(i) + 2}
                              width={Math.max(0, xw(i) - 4)}
                              y={BY(c.planned_register[1])}
                              height={BY(c.planned_register[0]) - BY(c.planned_register[1])}
                              rx="2"
                              class="mine"
                              class:on
                              style="--c:{colorOf(selPart)}"
                              onclick={() => pick({ kind: "cell", p: selPart, i })}
                            />
                            <text x={x0(i) + 6} y={BY(c.planned_register[1]) + 13} class="bandlabel" style="fill:{colorOf(selPart)}"
                              >{noteName(c.planned_register[0])}〜{noteName(c.planned_register[1])}</text
                            >
                          {/if}
                          {#if c.register}
                            <rect
                              x={x0(i) + xw(i) * 0.3}
                              width={Math.max(2, xw(i) * 0.4)}
                              y={BY(c.register[1]) - 1}
                              height={Math.max(2, BY(c.register[0]) - BY(c.register[1]) + 2)}
                              class="real"
                              pointer-events="none"
                            ><title>実際の音域 {noteName(c.register[0])}〜{noteName(c.register[1])}</title></rect>
                          {/if}
                        {/each}
                      {/if}
                    </svg>
                  </div>
                </div>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="vgrip"
                role="separator"
                aria-orientation="horizontal"
                title="ドラッグで音域の段の高さ(音高の縦の大きさ)を変える(ダブルクリックで元に戻す)"
                onpointerdown={(e) => grip("band", e)}
                ondblclick={() => resetGrip("band")}
              ></div>
            {/if}
          </section>

          <!-- パートの役割 -->
          <section class="lane" class:collapsed={folded.table} class:on={isWhole("table")} class:compact={rowH < 34}>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="lanehead main-head" onclick={(e) => toggleFold("table", e)}>
              <button class="fold" type="button" aria-label="折りたたむ・開く" aria-expanded={!folded.table}
                ><Icon name="chevron-down" size={14} /></button
              >
              <h2>パートの役割</h2>
              <button class="btn sm pickall" class:on={isWhole("table")} type="button" onclick={(e) => pickWhole("table", e)}
                >{isWhole("table") ? "全体を選択中" : "全体を選ぶ"}</button
              >
              {#if folded.table}
                <span class="summary">{d.parts.length} パート × {d.sections.length} 区間</span>
              {:else}
                <button type="button" class="qmark" aria-label="説明" title="マスは区間ごとの存在の段階(計画)。計画の無いマスは、測った段階を薄く出す。&#10;● は計画と実際がずれたマス。🔒 は固定(AI が作り直さない)。マスを押すと右に詳しく">?</button>
                <span class="hint">🔒 固定 · ● 実際とずれ · 薄い字 = 計画なし(実測)</span>
              {/if}
            </div>
            {#if !folded.table}
              <div class="vscroll" style="height:{tableBody}px">
                <div class="lbody">
                  <div class="labels">
                    {#each d.parts as p, pi (p.track_id)}
                      <button
                        class="mlabel"
                        class:on={pi === selPart}
                        style="height:{rowH}px"
                        type="button"
                        onclick={() => pick({ kind: "part", p: pi })}
                      >
                        <span class="trackname"><span class="chip" style="background:{colorOf(pi)}"></span>{p.name}</span>
                        <small>{p.function ? FUNCTIONS[p.function] ?? p.function : p.plan_id ? "" : "計画なし"}{p.estimated ? "(推定)" : ""}</small>
                      </button>
                    {:else}
                      <div class="emptyrow">MIDI のトラックがありません</div>
                    {/each}
                  </div>
                  <div class="hwin" onwheel={onWheel}>
                    <div class="hcontent" style="width:{W}px;transform:translateX({-sx}px)">
                      {#each d.parts as p, pi (p.track_id)}
                        <div class="mrow" style="height:{rowH}px">
                          {#each p.cells as c, i (i)}
                            {@const lv = c.planned ?? c.measured}
                            <button
                              class="cell"
                              class:sel={sel.kind === "cell" && sel.p === pi && sel.i === i}
                              class:diff={cellOff(c)}
                              class:unplanned={c.planned == null}
                              style="left:{x0(i)}px;width:{xw(i)}px;height:{rowH - 1}px"
                              type="button"
                              aria-label="{p.name} / {d.sections[i]?.name}: {c.planned != null ? PRESENCE[c.planned] : `計画なし(実測 ${PRESENCE[c.measured]})`}"
                              onclick={() => pick({ kind: "cell", p: pi, i })}
                            >
                              {#if c.locked}<span class="lock" title="固定(AI が作り直さない)">🔒</span>{/if}
                              <span class="steps">
                                {#each [1, 2, 3, 4, 5] as n (n)}<i class:on={n <= lv}></i>{/each}
                              </span>
                              <span class="lab">{c.planned != null ? PRESENCE[c.planned] : c.notes > 0 ? `実測 ${PRESENCE[c.measured]}` : "―"}</span>
                            </button>
                          {/each}
                        </div>
                      {/each}
                    </div>
                  </div>
                </div>
              </div>
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="vgrip"
                role="separator"
                aria-orientation="horizontal"
                title="ドラッグで役割の段の高さ(行の高さ)を変える(ダブルクリックで元に戻す)"
                onpointerdown={(e) => grip("table", e)}
                ondblclick={() => resetGrip("table")}
              ></div>
            {/if}
          </section>
          <div style="height:{slack}px;flex:none" aria-hidden="true"></div>
          <div class="hbar" bind:this={hbar} onscroll={onHbar} aria-label="横スクロール">
            <div style="width:{W}px;height:1px"></div>
          </div>
        </div>
      </div>

      <!-- 詳しい情報 -->
      <aside class="panel" aria-live="polite">
        {#if sel.kind === "none"}
          <h3>選んでいません</h3>
          <p class="empty">
            区間・パート・マスをクリックするか、段の見出しの「全体を選ぶ」で選ぶと、ここに詳しく出ます。何も選んでいないときの AI
            への指示は、曲全体が対象になります。
          </p>
          {#if d.deviations.length}
            <h4>計画と実際のずれ({d.deviations.length})</h4>
            {#each d.deviations.slice(0, 8) as x, k (k)}
              <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
            {/each}
          {/if}
        {:else}
          <h3>{designTargetLabel(d, sel)}</h3>
          {#if sel.kind === "song"}
            <dl class="kv">
              <dt>ジャンル</dt><dd>{d.song?.genre ?? "―"}</dd>
              <dt>盛り上がりの型</dt><dd>{d.song?.arc ? ARCS[d.song.arc] : "―"}</dd>
              <dt>テンポ</dt><dd>{tempo}</dd>
              <dt>守ること</dt><dd>{(d.song?.musts ?? []).join("、") || "―"}</dd>
              <dt>状態</dt><dd>{d.song ? (d.song_estimated ? "推定(未確認)" : "採用済み") : "計画なし"}</dd>
            </dl>
          {:else if sel.kind === "lane"}
            {#if sel.lane === "curve"}
              {@const hi = d.sections.reduce((a, s, i) => (s.measured > (d.sections[a]?.measured ?? -1) ? i : a), 0)}
              <dl class="kv">
                <dt>区間</dt><dd>{d.sections.length}</dd>
                <dt>いちばん高い(実測)</dt><dd>{d.sections[hi]?.name}({d.sections[hi]?.measured.toFixed(1)})</dd>
                <dt>段差</dt><dd>{d.sections.filter((s, i) => s.join === "step" && i < d.sections.length - 1).map((s) => `${s.name} → 次`).join("、") || "なし"}</dd>
                <dt>盛り上がりの型</dt><dd>{d.song?.arc ? ARCS[d.song.arc] : "―"}</dd>
              </dl>
              {#each d.sections.filter((_, i) => offSection(i)) as s (s.name)}
                <div class="finding warn"><span class="dot"></span><span>「{s.name}」が計画とずれている(計画 {s.planned?.toFixed(1)} / 実測 {s.measured.toFixed(1)})</span></div>
              {/each}
            {:else if sel.lane === "band"}
              <dl class="kv">
                <dt>パート</dt><dd>{d.parts.length}</dd>
                <dt>計画のあるパート</dt><dd>{d.parts.filter((p) => p.plan_id).length}</dd>
              </dl>
              {#each devOf((x) => !!x.track && x.what.includes("音域")) as x, k (k)}
                <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
              {/each}
            {:else if sel.lane === "table"}
              <dl class="kv">
                <dt>大きさ</dt><dd>{d.parts.length} パート × {d.sections.length} 区間</dd>
                <dt>固定</dt><dd>{d.parts.reduce((a, p) => a + p.cells.filter((c) => c.locked).length, 0)} マス</dd>
                <dt>ずれ</dt><dd>{d.parts.reduce((a, p) => a + p.cells.filter(cellOff).length, 0)} マス</dd>
              </dl>
            {:else}
              <dl class="kv">
                <dt>並び</dt><dd>{d.sections.map((s) => `${s.name}(${s.bars})`).join(" → ")}</dd>
                <dt>長さ</dt><dd>{totalBars} 小節</dd>
              </dl>
            {/if}
          {:else if sel.kind === "section"}
            {@const s = d.sections[sel.i]}
            {#if s}
              <dl class="kv">
                <dt>位置</dt><dd>{s.start_bar} 小節目から {s.bars} 小節</dd>
                <dt>盛り上がり</dt><dd>計画 {s.planned != null ? s.planned.toFixed(1) : "―"} / 実測 {s.measured.toFixed(1)}</dd>
                <dt>次との境目</dt><dd>{sel.i < d.sections.length - 1 ? (s.join === "step" ? "段差" : "つなぐ") : "―"}</dd>
                <dt>意図</dt><dd>{project.sections?.find((m) => m.name === s.name)?.note ?? "―"}</dd>
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
                  <dt>働き</dt><dd>{c.function ? FUNCTIONS[c.function] : p.function ? FUNCTIONS[p.function] : "―"}</dd>
                  <dt>存在(計画)</dt><dd>{c.planned != null ? `${c.planned} ${PRESENCE[c.planned]}` : "計画なし"}</dd>
                  <dt>存在(実測)</dt><dd>{c.measured} {PRESENCE[c.measured]}({c.notes} 音)</dd>
                  <dt>音域(計画)</dt><dd>{c.planned_register ? `${noteName(c.planned_register[0])}〜${noteName(c.planned_register[1])}` : "―"}</dd>
                  <dt>音域(実際)</dt><dd>{c.register ? `${noteName(c.register[0])}〜${noteName(c.register[1])}` : "―"}</dd>
                  <dt>密度</dt><dd>{c.density}</dd>
                  <dt>固定</dt><dd>{c.locked ? "🔒 この区間は固定" : "なし"}</dd>
                </dl>
                {#each devOf((x) => x.track === p.name && x.section === c.section) as x, k (k)}
                  <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
                {/each}
              {:else}
                <dl class="kv">
                  <dt>働き</dt><dd>{p.function ? FUNCTIONS[p.function] ?? p.function : "―"}</dd>
                  <dt>計画</dt><dd>{p.plan_id ? (p.estimated ? "推定(未確認)" : "採用済み") : "なし"}</dd>
                  <dt>鳴っている区間</dt><dd>{p.cells.filter((c) => c.notes > 0).length} / {p.cells.length}</dd>
                </dl>
                {#each devOf((x) => x.track === p.name) as x, k (k)}
                  <div class="finding {x.severity}"><span class="dot"></span><span>{x.what}</span></div>
                {/each}
              {/if}
              {#if partClips(p.name).length}
                <h4>クリップの状態</h4>
                {#each partClips(p.name) as c (c.clip_id)}
                  <div class="clipstate">
                    {#if c.plan === "ahead"}<span class="badge ahead">計画が先に進んだ</span>{:else if c.plan === "in_sync"}<span class="badge">計画どおり</span>{/if}
                    {#if c.edited_bars?.length}<span class="badge edited"
                        >手で直した: {c.edited_bars.map(([a, b]) => (a === b ? `${a}` : `${a}〜${b}`)).join("・")} 小節</span
                      >{/if}
                    {#if c.locked_notes}<span class="badge">🔒 {c.locked_notes} 音</span>{/if}
                  </div>
                {/each}
              {/if}
            {/if}
          {/if}
          <div class="actions">
            <button class="btn sm" type="button" onclick={askAi} title="下のチャットの入力欄へ(選んだ所が対象として添わる)"
              >この所について AI に頼む ↓</button
            >
          </div>
          <p class="stage-note">画面で直す・メモを残す・この版から作り直すは、次の段階で使えるようになります。</p>
        {/if}
      </aside>
    </div>
  {/if}
</div>

<style>
  .design {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-2);
    background: var(--bg);
    font-size: var(--fs-md);
  }
  .design.dragging,
  .design.dragging :global(*) {
    user-select: none !important;
  }
  .empty-all {
    margin: auto;
    color: var(--text-dim);
  }
  .estimate {
    flex: none;
    display: flex;
    gap: var(--sp-3);
    align-items: center;
    flex-wrap: wrap;
    border: 1px dashed var(--border-strong);
    border-radius: var(--r-md);
    padding: 6px 12px;
    background: var(--bg-panel);
    font-size: var(--fs-sm);
  }
  .estimate span {
    color: var(--text-dim);
  }
  .main {
    flex: 1 1 0;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 300px;
    gap: var(--sp-2);
  }
  .lanes {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    user-select: none;
  }
  .lane {
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden;
    min-width: 0;
    flex: none;
    display: flex;
    flex-direction: column;
  }
  .lane.on {
    border-color: var(--accent-dim);
  }
  .lane.on > .main-head h2 {
    color: var(--accent);
  }
  .lanehead {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 5px 10px;
    background: var(--bg-lane);
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }
  .main-head {
    cursor: pointer;
  }
  .main-head:hover {
    background: var(--bg-lane-alt);
  }
  .lane.collapsed > .main-head {
    border-bottom: 0;
  }
  .song > .main-head {
    background: var(--bg-raised);
  }
  .lanehead h2 {
    margin: 0;
    font-size: var(--fs-md);
    font-weight: 600;
    white-space: nowrap;
  }
  .fold {
    background: transparent;
    border: 0;
    color: var(--text);
    width: 24px;
    height: 24px;
    margin-left: -4px;
    padding: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: var(--r-sm);
    flex: none;
  }
  .fold :global(svg) {
    transition: transform 0.12s;
  }
  .lane.collapsed .fold :global(svg) {
    transform: rotate(-90deg);
  }
  .pickall {
    border-radius: 10px;
    color: var(--text-dim);
    background: transparent;
    padding: 0 8px;
    font-size: var(--fs-xs);
    flex: none;
  }
  .pickall.on {
    color: var(--accent);
    border-color: var(--accent-dim);
  }
  .pickall.mini {
    padding: 0 6px;
  }
  .summary,
  .hint {
    color: var(--text-dim);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    min-width: 0;
  }
  .summary {
    flex: 1;
  }
  .tag {
    font-size: var(--fs-xs);
    border: 1px dashed var(--border-strong);
    border-radius: 9px;
    padding: 0 6px;
    color: var(--text-dim);
  }
  .qmark {
    background: transparent;
    padding: 0;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    border: 1px solid var(--border-strong);
    color: var(--text-dim);
    font-size: var(--fs-xs);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: help;
    flex: none;
  }
  .legend {
    margin-left: auto;
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .legend label {
    display: flex;
    gap: 4px;
    align-items: center;
    cursor: pointer;
  }
  /* 曲全体 */
  .songgrid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
    gap: 8px 16px;
    padding: 10px 12px 12px;
    cursor: pointer;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .field > span {
    color: var(--text-dim);
    font-size: var(--fs-xs);
  }
  .field small {
    color: var(--text-faint);
  }
  .field.wide {
    grid-column: 1 / -1;
  }
  .hint-row {
    color: var(--text-dim);
    font-size: var(--fs-sm);
  }
  .tags {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .word {
    font-size: var(--fs-xs);
    padding: 1px 8px;
    border-radius: 10px;
    border: 1px solid var(--border-strong);
    background: var(--bg-raised);
  }
  .word.must {
    border-color: var(--accent-dim);
  }
  .meter {
    height: 6px;
    border-radius: 3px;
    background: var(--bg-inset);
    overflow: hidden;
  }
  .meter i {
    display: block;
    height: 100%;
    background: var(--accent-dim);
  }
  .ends {
    display: flex;
    justify-content: space-between;
    color: var(--text-faint);
    font-size: 10px;
  }
  /* 区間ごとの計画の枠 */
  .timegroup {
    flex: 1 1 0;
    min-height: 200px;
    overflow-y: auto;
    overscroll-behavior: contain;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0 6px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--bg-inset);
    min-width: 0;
  }
  .gtop {
    position: sticky;
    top: 0;
    z-index: 3;
    background: var(--bg-inset);
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding-top: 4px;
    flex: none;
  }
  .grouphead {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 0 4px;
    font-size: var(--fs-xs);
    font-weight: 600;
    color: var(--text-dim);
  }
  .grouphead span {
    font-weight: 400;
    color: var(--text-faint);
  }
  .zoom {
    margin-left: auto;
    display: flex;
    gap: 2px;
  }
  .zoom .btn {
    font-weight: 400;
  }
  .lbody {
    display: grid;
    grid-template-columns: 140px minmax(0, 1fr);
  }
  .labels {
    border-right: 1px solid var(--border);
    background: var(--bg-panel);
    position: relative;
  }
  .hwin {
    overflow: hidden;
    position: relative;
  }
  .hcontent {
    position: relative;
    will-change: transform;
  }
  svg {
    display: block;
  }
  .lab1 {
    padding: 4px 6px 4px 10px;
    color: var(--text-dim);
    font-size: var(--fs-sm);
    height: 30px;
  }
  .sechead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 4px;
  }
  .sechead.on {
    background: var(--bg-raised);
  }
  .ruler {
    height: 30px;
  }
  .sec {
    position: absolute;
    top: 0;
    height: 30px;
    border: 0;
    border-left: 1px solid var(--border-strong);
    border-radius: 0;
    background: transparent;
    padding: 5px 6px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }
  .sec b {
    color: var(--text);
    font-weight: 600;
  }
  .sec.sel {
    background: var(--bg-raised);
    color: var(--accent);
  }
  .sec.sel b {
    color: var(--accent);
  }
  /* 盛り上がり */
  .cvlegend {
    position: absolute;
    left: 12px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }
  .cvlegend > div {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 15px;
    cursor: help;
  }
  .cvlegend hr {
    border: 0;
    border-top: 1px solid var(--border);
    width: 100%;
    margin: 2px 0;
  }
  .cvlegend .ln {
    width: 14px;
    border-top: 2px solid var(--accent);
  }
  .cvlegend .ln.dash {
    border-top: 2px dashed var(--text-dim);
  }
  .cvlegend .mk {
    width: 14px;
    height: 12px;
    border: 1px solid var(--border-strong);
    border-radius: 3px;
    background: var(--bg-raised);
    font-size: 9px;
    line-height: 10px;
    text-align: center;
  }
  .cvlegend .mk.warn {
    border-color: var(--warn);
    color: var(--warn);
  }
  .cvlegend .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--warn);
    margin: 0 4px 0 3px;
  }
  .cvlegend.nomeasure .m {
    opacity: 0.35;
  }
  .axisline {
    position: absolute;
    right: 30px;
    border-left: 1px solid var(--border);
  }
  .axis {
    position: absolute;
    right: 8px;
    font-size: 10px;
    color: var(--text-faint);
    transform: translateY(-50%);
    font-variant-numeric: tabular-nums;
  }
  .axis.edge {
    color: var(--text-dim);
  }
  .outside {
    fill: var(--bg-inset);
  }
  line.grid {
    stroke: var(--bg-lane-alt);
    stroke-dasharray: 2 4;
  }
  line.edge {
    stroke: var(--border-strong);
  }
  line.secline {
    stroke: var(--border);
  }
  path.area {
    fill: var(--accent);
    fill-opacity: 0.07;
  }
  path.area.on {
    fill-opacity: 0.18;
  }
  path.plan {
    stroke: var(--accent);
    stroke-width: 2;
    fill: none;
  }
  circle.pt {
    fill: var(--bg-panel);
    stroke: var(--accent);
    stroke-width: 1.5;
  }
  circle.pt.on {
    fill: var(--accent);
  }
  line.measured {
    stroke: var(--text-dim);
    stroke-width: 1.5;
    stroke-dasharray: 5 4;
    pointer-events: none;
  }
  circle.offdot {
    fill: var(--warn);
  }
  g.join rect {
    fill: var(--bg-raised);
    stroke: var(--border-strong);
  }
  g.join text {
    font-size: 10px;
    text-anchor: middle;
    fill: var(--text-dim);
  }
  g.join.step rect {
    stroke: var(--warn);
  }
  g.join.step text {
    fill: var(--warn);
  }
  /* 音域 */
  .bandlane {
    display: grid;
    grid-template-columns: 140px minmax(0, 1fr);
  }
  .partlist {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px;
    overflow-y: auto;
  }
  .partbtn {
    display: flex;
    gap: 6px;
    align-items: center;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    color: var(--text-dim);
    font-size: var(--fs-sm);
    padding: 3px 6px;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .partbtn:hover {
    background: var(--bg-raised);
    color: var(--text);
  }
  .partbtn.on {
    border-color: var(--accent-dim);
    background: var(--bg-raised);
    color: var(--text);
  }
  .chip {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    flex: none;
  }
  .emptyrow {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    padding: 6px 10px;
  }
  .octaves {
    position: absolute;
    left: 0;
    top: 0;
    width: 30px;
    pointer-events: none;
    background: linear-gradient(90deg, var(--bg-panel) 60%, transparent);
    z-index: 1;
  }
  .octave {
    position: absolute;
    left: 4px;
    font-size: 10px;
    color: var(--text-faint);
    transform: translateY(-50%);
  }
  rect.other {
    fill: var(--text);
    fill-opacity: 0.04;
    stroke: var(--text);
    stroke-opacity: 0.14;
    pointer-events: none;
  }
  rect.mine {
    fill: var(--c);
    fill-opacity: 0.26;
    stroke: var(--c);
    cursor: pointer;
  }
  rect.mine.on {
    fill-opacity: 0.42;
    stroke-width: 2;
  }
  rect.real {
    fill: none;
    stroke: var(--text);
    stroke-opacity: 0.7;
    stroke-dasharray: 3 3;
  }
  text.bandlabel {
    font-size: 10.5px;
    pointer-events: none;
  }
  line.hatch {
    stroke: var(--text);
    stroke-opacity: 0.25;
  }
  /* 役割 */
  .vscroll {
    overflow-y: auto;
    overflow-x: hidden;
  }
  .mlabel {
    display: flex;
    flex-direction: column;
    justify-content: center;
    width: 100%;
    border: 0;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    background: transparent;
    padding: 2px 10px;
    text-align: left;
    overflow: hidden;
  }
  .mlabel > * {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mlabel.on {
    background: var(--bg-raised);
  }
  .trackname {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: var(--fs-sm);
  }
  .mlabel small {
    color: var(--text-faint);
    font-size: var(--fs-xs);
    padding-left: 14px;
  }
  .lane.compact .mlabel small {
    display: none;
  }
  .mrow {
    position: relative;
    border-bottom: 1px solid var(--border);
  }
  .cell {
    position: absolute;
    top: 0;
    border: 0;
    border-left: 1px solid var(--border);
    border-radius: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: transparent;
    color: var(--text);
    padding: 0 4px;
    font-size: var(--fs-xs);
  }
  .cell:hover {
    background: var(--bg-raised);
  }
  .cell.sel {
    outline: 1px solid var(--accent);
    outline-offset: -1px;
    background: var(--bg-raised);
  }
  .cell.unplanned {
    color: var(--text-faint);
  }
  .steps {
    display: flex;
    gap: 2px;
  }
  .steps i {
    width: 4px;
    height: 10px;
    background: var(--bg-lane-alt);
    border-radius: 1px;
  }
  .steps i.on {
    background: var(--text-dim);
  }
  .cell:not(.unplanned) .steps i.on {
    background: var(--text);
  }
  .cell.sel .steps i.on {
    background: var(--accent);
  }
  .cell .lab {
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cell.diff::after {
    content: "";
    position: absolute;
    top: 5px;
    right: 5px;
    width: 5px;
    height: 5px;
    border-radius: 50%;
    background: var(--warn);
  }
  .cell .lock {
    position: absolute;
    left: 4px;
    top: 2px;
    font-size: 9px;
  }
  .vgrip {
    flex: none;
    height: 9px;
    cursor: row-resize;
    border-top: 1px solid var(--border);
    background: var(--bg-lane);
    position: relative;
    touch-action: none;
  }
  .vgrip::after {
    content: "";
    position: absolute;
    left: 50%;
    top: 3px;
    width: 36px;
    height: 2px;
    margin-left: -18px;
    border-radius: 1px;
    background: var(--border-strong);
  }
  .vgrip:hover::after {
    background: var(--accent);
  }
  .hbar {
    position: sticky;
    bottom: 0;
    z-index: 3;
    flex: none;
    overflow-x: auto;
    overflow-y: hidden;
    height: 20px;
    padding-bottom: 6px;
    margin-left: 141px;
    background: var(--bg-inset);
  }
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
  .empty,
  .stage-note {
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
  .finding .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--warn);
    margin-top: 6px;
    flex: none;
  }
  .finding.info .dot {
    background: var(--text-faint);
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
  @media (max-width: 980px) {
    .main {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
