<script lang="ts">
  // ミキサー: 上 = 全トラックの縦の列(エフェクト・送り・パン・M/S・フェーダー・メーター。右にバスとマスター)、
  // 下 = 選んだ列のエフェクトのノード表示(NodeView)。上下の境目はドラッグで高さを変えられる。
  // 列のエフェクト名を押すと、下の同じカードが光る。すべての編集は Command を通る。
  import { untrack } from "svelte";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { effectiveLinks, fxColor, fxName, processingOrder, serialOrder } from "./fx";
  import { deviceIcon, deviceName } from "./instruments";
  import NodeView from "./NodeView.svelte";
  import FxPresetShelf from "./FxPresetShelf.svelte";
  import { instrumentPickerStore, MASTER_FOCUS_ID, soundDesignStore, viewStore } from "./selection.svelte";
  import { requestFastPolling, transportStore } from "./transport.svelte";
  import type { EffectView, FxLink, Project, ProjectEffect, Track } from "./types";

  let { project }: { project: Project } = $props();

  const KIND_ICON = { midi: "piano", audio: "audio-lines", bus: "merge" } as const;

  const plain = $derived(project.tracks.filter((t) => t.kind !== "bus"));
  const buses = $derived(project.tracks.filter((t) => t.kind === "bus"));

  /** 選んでいる列(無ければ最初のトラック) */
  const selected = $derived.by(() => {
    const id = viewStore.mixerTrack;
    if (id === MASTER_FOCUS_ID) return id;
    if (id && project.tracks.some((t) => t.id === id)) return id;
    return project.tracks[0]?.id ?? MASTER_FOCUS_ID;
  });
  const selectedTrack = $derived(project.tracks.find((t) => t.id === selected) ?? null);

  function select(id: string, fx: string | null = null) {
    viewStore.mixerTrack = id;
    viewStore.highlightFx = fx;
  }

  // CLAP の名前(列のエフェクト名・音源名)
  let clapNames = $state(new Map<string, string>());
  $effect(() => {
    api
      .clapPlugins()
      .then((r) => (clapNames = new Map(r.plugins.map((p) => [p.id, p.name]))))
      .catch(() => {});
  });

  // ---- メーター(問い合わせは App。ここは読むだけ。下がるときはゆっくり) ----
  $effect(() => requestFastPolling());
  let meters = $state<Record<string, number>>({});
  $effect(() => {
    void transportStore.seq;
    const lv = transportStore.state.levels;
    const next: Record<string, number> = {};
    // 前の値は読むだけ(依存にすると自分の書き込みで回り続ける)
    const prev = untrack(() => meters);
    const fall = (id: string, db: number) => Math.max(db, (prev[id] ?? -120) - 2.5);
    project.tracks.forEach((t, i) => (next[t.id] = fall(t.id, lv?.tracks[i] ?? -120)));
    next[MASTER_FOCUS_ID] = fall(MASTER_FOCUS_ID, lv?.master ?? -120);
    meters = next;
  });
  const meterPct = (db: number) => Math.max(0, Math.min(100, ((db + 60) / 66) * 100));

  // ---- 編集 ----
  function edit(commands: unknown[], label: string) {
    api.applyEdit(commands, label).catch(() => {});
  }
  let dragVol = $state<Record<string, number>>({});
  function setVolume(t: Track | null, v: number) {
    const key = t?.id ?? MASTER_FOCUS_ID;
    delete dragVol[key];
    if (!t) edit([{ op: "set_master_volume", volume_db: v }], `マスター音量を ${v.toFixed(1)} dB に`);
    else edit([{ op: "set_track_prop", id: t.id, prop: "volume_db", value: v }], `${t.name} の音量を ${v.toFixed(1)} dB に`);
  }
  function setPan(t: Track, v: number) {
    edit([{ op: "set_track_prop", id: t.id, prop: "pan", value: v }], `${t.name} のパンを ${v.toFixed(2)} に`);
  }
  function toggle(t: Track, prop: "mute" | "solo") {
    edit([{ op: "set_track_prop", id: t.id, prop, value: !t[prop] }], `${t.name} の${prop === "mute" ? "ミュート" : "ソロ"}を${t[prop] ? "解除" : "オン"}`);
  }
  let dragSend = $state<Record<string, number>>({});
  function setSend(t: Track, bus: Track, v: number) {
    delete dragSend[`${t.id}>${bus.id}`];
    const cur = t.sends?.find((s) => s.target === bus.id);
    if (v <= -60) {
      if (cur) edit([{ op: "set_send", track: t.id, target: bus.id }], `${t.name} から ${bus.name} への送りを外す`);
      return;
    }
    edit(
      [{ op: "set_send", track: t.id, target: bus.id, level_db: v, pre_fader: cur?.pre_fader ?? false }],
      `${t.name} から ${bus.name} への送りを ${v.toFixed(1)} dB に`,
    );
  }
  function openPicker(e: MouseEvent, t: Track) {
    e.stopPropagation();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    instrumentPickerStore.open = { trackId: t.id, x: r.left, y: r.bottom + 4 };
  }
  const receivers = (bus: Track) => project.tracks.filter((t) => t.sends?.some((s) => s.target === bus.id));
  /** 列に出すエフェクト: 鳴るものを処理の順に。分岐・合流があるか、鳴らないものの数も */
  function chainOf(effects: ProjectEffect[], fxLinks: FxLink[] | null | undefined) {
    const links = effectiveLinks(effects, fxLinks);
    const order = processingOrder(effects, links);
    const byId = new Map(effects.map((e) => [e.id, e]));
    const branched = serialOrder(links) === null && order.length > 0;
    return { list: order.map((id) => byId.get(id)!).filter(Boolean), mute: effects.length - order.length, branched };
  }

  // ---- 上下の高さ(境目のドラッグ、localStorage に保存) ----
  function loadTop(): number {
    try {
      const v = Number(localStorage.getItem("glaux.mixerTop"));
      return Number.isFinite(v) && v >= 200 ? v : 320;
    } catch {
      return 320;
    }
  }
  let topH = $state(loadTop());
  let rootEl = $state<HTMLDivElement | undefined>(undefined);
  let rootH = $state(800);
  /** 実際の上の段の高さ(画面が低いときは、下のノード表示に 220px は残す) */
  const shownTop = $derived(Math.min(topH, Math.max(300, rootH - 250)));
  function startSplit(e: PointerEvent) {
    const y0 = e.clientY;
    const h0 = shownTop;
    const max = (rootEl?.clientHeight ?? 800) - 160;
    const move = (m: PointerEvent) => (topH = Math.round(Math.min(max, Math.max(300, h0 + m.clientY - y0))));
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      try {
        localStorage.setItem("glaux.mixerTop", String(topH));
      } catch {
        // 保存できなくても動作には関係しない
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // インスペクター: このボタンで開き、もう一度押すと閉じる(別の列を開いているときは、この列に切り替える)
  const inspectorOpen = $derived(soundDesignStore.focus?.trackId === selected);
  function toggleInspector() {
    if (inspectorOpen) {
      soundDesignStore.focus = null;
      return;
    }
    soundDesignStore.focus = {
      trackId: selected,
      trackName: selected === MASTER_FOCUS_ID ? "マスター" : (selectedTrack?.name ?? ""),
    };
  }

  // エフェクトのプリセットの棚(下の段の右端)
  let shelf = $state<FxPresetShelf | undefined>(undefined);
  function savePreset(fx: EffectView) {
    shelf?.saveFrom(selected, fx);
  }
</script>

{#snippet slots(effects: ProjectEffect[], fxLinks: FxLink[] | null | undefined, ownerId: string)}
  {@const c = chainOf(effects, fxLinks)}
  <div class="s-sec"><span>エフェクト</span><span>{c.list.length}</span></div>
  <div class="slots">
    {#each c.list as e (e.id)}
      <button
        class="slot"
        class:bypass={e.bypass}
        class:hl={viewStore.highlightFx === e.id}
        style="--nc:{fxColor(e)}"
        title={`${fxName(e, clapNames)}(押すと下のカードが光る)`}
        onclick={(ev) => {
          ev.stopPropagation();
          select(ownerId, e.id);
        }}><span class="cb"></span><span class="led"></span><span class="nm">{fxName(e, clapNames)}</span></button
      >
    {/each}
    {#if c.branched}
      <div class="folded" title="分かれたり合流したりしている(並びは処理の順。つながりはノード表示で)"><Icon name="split" size={11} />分岐あり</div>
    {/if}
    {#if c.mute > 0}
      <div class="folded" title="入力から出口まで線でたどれないカード(設定は残っていて、音は通らない)"><Icon name="unplug" size={11} />鳴らない {c.mute}</div>
    {/if}
  </div>
{/snippet}

{#snippet fader(t: Track | null)}
  {@const key = t?.id ?? MASTER_FOCUS_ID}
  {@const vol = dragVol[key] ?? (t ? t.volume_db : project.master.volume_db)}
  <div class="fader-wrap">
    <input
      class="vfader"
      type="range"
      min="-40"
      max="6"
      step="0.5"
      value={t ? t.volume_db : project.master.volume_db}
      oninput={(e) => (dragVol[key] = Number(e.currentTarget.value))}
      onchange={(e) => setVolume(t, Number(e.currentTarget.value))}
      ondblclick={() => setVolume(t, 0)}
      aria-label="音量"
      title="音量(ダブルクリックで 0 dB)"
    />
    <div class="meter" title="レベル"><i style="height:{100 - meterPct(meters[key] ?? -120)}%"></i></div>
  </div>
  <span class="s-db">{vol > 0 ? "+" : ""}{vol.toFixed(1)} dB</span>
{/snippet}

<div class="mixer" bind:this={rootEl} bind:clientHeight={rootH}>
  <div class="strips" style="height:{shownTop}px">
    {#each plain as t (t.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div class="strip" role="group" aria-label={t.name} class:sel={selected === t.id} style="--c:{t.color ?? '#555'}" onclick={() => select(t.id)}>
        <div class="s-top"></div>
        <div class="s-name"><Icon name={KIND_ICON[t.kind]} size={13} /><span title={t.name}>{t.name}</span></div>
        {#if t.kind === "midi"}
          <button class="s-dev" onclick={(e) => openPicker(e, t)} title="音源を変える"
            ><Icon name={deviceIcon(t.device)} size={12} /><span>{deviceName(t.device, clapNames)}</span></button
          >
        {:else}
          <div class="s-dev plain">音声</div>
        {/if}
        {@render slots(t.effects, t.fx_links, t.id)}
        {#if buses.length > 0}
          <div class="s-sec"><span>送り</span></div>
          <div class="sends">
            {#each buses as b (b.id)}
              {@const snd = t.sends?.find((s) => s.target === b.id)}
              {@const shown = dragSend[`${t.id}>${b.id}`] ?? snd?.level_db}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div class="send" role="presentation" class:none={!snd} onclick={(e) => e.stopPropagation()}>
                <span class="sn" title={b.name}><i style="background:{b.color ?? '#888'}"></i>{b.name}</span>
                <span class="sv">{shown !== undefined ? shown.toFixed(0) : "—"}</span>
                <input
                  type="range"
                  min="-60"
                  max="6"
                  step="0.5"
                  value={snd?.level_db ?? -60}
                  oninput={(e) => (dragSend[`${t.id}>${b.id}`] = Number(e.currentTarget.value))}
                  onchange={(e) => setSend(t, b, Number(e.currentTarget.value))}
                  aria-label={`${b.name} へ送る量`}
                  title={`${b.name} へ送る量(左端で送らない)`}
                />
              </div>
            {/each}
          </div>
        {/if}
        <div class="s-bottom">
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="pan" role="presentation" onclick={(e) => e.stopPropagation()}>
            <span>L</span>
            <input
              type="range"
              min="-1"
              max="1"
              step="0.02"
              value={t.pan}
              onchange={(e) => setPan(t, Number(e.currentTarget.value))}
              ondblclick={() => setPan(t, 0)}
              aria-label="パン"
              title="パン(ダブルクリックで中央)"
            />
            <span>R</span>
          </div>
          <div class="ms">
            <button class="btn letter" class:m-on={t.mute} onclick={(e) => (e.stopPropagation(), toggle(t, "mute"))} title="ミュート">M</button>
            <button class="btn letter" class:s-on={t.solo} onclick={(e) => (e.stopPropagation(), toggle(t, "solo"))} title="ソロ">S</button>
          </div>
          {@render fader(t)}
        </div>
      </div>
    {/each}

    {#if buses.length > 0}<span class="grp-label">バス</span>{/if}
    {#each buses as t (t.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div class="strip bus" role="group" aria-label={t.name} class:sel={selected === t.id} style="--c:{t.color ?? '#e8a07c'}" onclick={() => select(t.id)}>
        <div class="s-top"></div>
        <div class="s-name"><Icon name="merge" size={13} /><span title={t.name}>{t.name}</span></div>
        <div class="s-dev plain" title="このバスへ送っているトラック">受けている: {receivers(t).map((r) => r.name).join("・") || "なし"}</div>
        {@render slots(t.effects, t.fx_links, t.id)}
        <div class="s-bottom">
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="pan" role="presentation" onclick={(e) => e.stopPropagation()}>
            <span>L</span>
            <input type="range" min="-1" max="1" step="0.02" value={t.pan} onchange={(e) => setPan(t, Number(e.currentTarget.value))} ondblclick={() => setPan(t, 0)} aria-label="パン" />
            <span>R</span>
          </div>
          <div class="ms">
            <button class="btn letter" class:m-on={t.mute} onclick={(e) => (e.stopPropagation(), toggle(t, "mute"))} title="ミュート">M</button>
            <button class="btn letter" class:s-on={t.solo} onclick={(e) => (e.stopPropagation(), toggle(t, "solo"))} title="ソロ">S</button>
          </div>
          {@render fader(t)}
        </div>
      </div>
    {/each}

    <span class="grp-label">出口</span>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <div class="strip master" role="group" aria-label="マスター" class:sel={selected === MASTER_FOCUS_ID} onclick={() => select(MASTER_FOCUS_ID)}>
      <div class="s-top" style="background:var(--accent)"></div>
      <div class="s-name"><Icon name="volume-2" size={13} /><span>マスター</span></div>
      <div class="s-dev plain">曲全体</div>
      {@render slots(project.master.effects, project.master.fx_links, MASTER_FOCUS_ID)}
      <div class="s-bottom">{@render fader(null)}</div>
    </div>
  </div>

  <div class="split" role="separator" aria-orientation="horizontal" title="ドラッグで高さを変える" onpointerdown={startSplit}></div>

  <div class="detail">
    <div class="d-head">
      <span class="dot" style="background:{selected === MASTER_FOCUS_ID ? 'var(--accent)' : (selectedTrack?.color ?? '#777')}"></span>
      <b>{selected === MASTER_FOCUS_ID ? "マスター" : selectedTrack?.name}</b><span class="dim">のエフェクト</span>
      <span class="sp"></span>
      <span class="dim hint">口から線を引いてつなぐ・線をクリックで音量 / 切る・Ctrl+ドラッグでまとめて切る・名前はダブルクリック</span>
      <button
        class="btn sm"
        class:on={inspectorOpen}
        aria-pressed={inspectorOpen}
        onclick={toggleInspector}
        title={inspectorOpen ? "インスペクターを閉じる" : "インスペクターで開く(つまみを全部見る)"}><Icon name="sliders-horizontal" />インスペクター</button
      >
    </div>
    <div class="d-body">
      <div class="canvas">
        {#key selected}
          <NodeView {project} targetId={selected} onSavePreset={savePreset} />
        {/key}
      </div>
      <FxPresetShelf bind:this={shelf} {project} targetId={selected} />
    </div>
  </div>
</div>

<style>
  .mixer {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }

  .strips {
    flex-shrink: 0;
    display: flex;
    gap: 6px;
    padding: 8px 10px;
    overflow: auto hidden;
  }

  .grp-label {
    writing-mode: vertical-rl;
    font-size: 10px;
    color: var(--text-faint);
    padding: 4px 1px;
    letter-spacing: 0.2em;
    flex-shrink: 0;
  }

  .strip {
    width: 128px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    /* 上の段が低くて入りきらないときは、重ねずに列の中でスクロール */
    overflow: hidden auto;
    scrollbar-width: thin;
    cursor: pointer;
    min-height: 0;
  }

  .strip.sel {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent) inset;
  }

  .strip.bus {
    background: #1d1a17;
  }

  .strip.master {
    background: #16201f;
  }

  .s-top {
    height: 4px;
    flex-shrink: 0;
    background: var(--c);
  }

  .s-name {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 5px 7px 2px 8px;
    font-weight: 700;
    font-size: var(--fs-md);
    color: var(--text);
  }

  .s-name :global(.icon) {
    color: var(--text-dim);
  }

  .s-name span {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .s-dev {
    margin: 2px 6px 2px;
    height: 20px;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 5px;
    border-radius: var(--r-sm);
    background: var(--bg-raised);
    border: 1px solid var(--border);
    font-size: var(--fs-xs);
    color: var(--text-dim);
    overflow: hidden;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .s-dev span {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .s-dev.plain {
    background: none;
    border-color: transparent;
    text-overflow: ellipsis;
  }

  .s-sec {
    display: flex;
    justify-content: space-between;
    font-size: 9px;
    color: var(--text-faint);
    padding: 4px 8px 2px;
    flex-shrink: 0;
  }

  .slots {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 5px;
    overflow-y: auto;
    /* 上の段が低くても 2 つは見える */
    min-height: 44px;
    flex-shrink: 1;
  }

  .slot {
    height: 20px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 4px 0 2px;
    border-radius: 3px;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    font-size: var(--fs-xs);
    text-align: left;
  }

  .slot .cb {
    width: 3px;
    height: 12px;
    border-radius: 2px;
    background: var(--nc);
    flex-shrink: 0;
  }

  .slot .led {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ok);
    flex-shrink: 0;
  }

  .slot.bypass .led {
    background: #444;
  }

  .slot.bypass .nm {
    color: var(--text-faint);
    text-decoration: line-through;
  }

  .slot.hl {
    border-color: var(--accent);
  }

  .slot .nm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .folded {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 10px;
    color: var(--text-faint);
    padding: 1px 2px;
  }

  .sends {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 0 7px;
    flex-shrink: 0;
  }

  .send {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 0 4px;
    font-size: 10px;
    color: var(--text-dim);
  }

  .send.none {
    opacity: 0.55;
  }

  .send .sn {
    display: flex;
    align-items: center;
    gap: 4px;
    overflow: hidden;
    white-space: nowrap;
  }

  .send .sn i {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .send .sv {
    font-family: var(--mono);
  }

  .send input {
    grid-column: 1 / -1;
    width: 100%;
    height: 10px;
    margin: 0;
    accent-color: var(--text-dim);
  }

  /* 残りの高さはフェーダーに回す(上の段の高さに合わせて伸び縮み) */
  .s-bottom {
    flex: 1 0 auto;
    /* パン・M/S・フェーダーの最小 48px・dB 表示が入る高さ。これより低くしない(はみ出して送りに重なっていた) */
    min-height: 136px;
    padding: 6px 6px 5px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 5px;
    border-top: 1px solid var(--border);
  }

  .pan {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 9px;
    color: var(--text-faint);
  }

  .pan input {
    width: 76px;
    height: 10px;
    margin: 0;
    accent-color: var(--text-dim);
  }

  .ms {
    display: flex;
    gap: 3px;
  }

  .letter {
    width: 22px;
    height: 20px;
    padding: 0;
    font-size: 10px;
    font-weight: 700;
    border-radius: var(--r-sm);
  }

  .letter.m-on {
    background: var(--mute);
    border-color: var(--mute);
    color: #1a1a1a;
  }

  .letter.s-on {
    background: var(--solo);
    border-color: var(--solo);
    color: #1a1a1a;
  }

  .fader-wrap {
    display: flex;
    gap: 8px;
    align-items: stretch;
    flex: 1 1 0;
    min-height: 48px;
    max-height: 170px;
  }

  .vfader {
    writing-mode: vertical-lr;
    direction: rtl;
    width: 20px;
    height: auto;
    min-height: 0;
    margin: 0;
    accent-color: #d8d8d8;
  }

  /* 色は枠の全高に対して固定し、鳴っていない上の部分を <i> で覆う */
  .meter {
    width: 8px;
    background: linear-gradient(0deg, var(--ok) 0 70%, var(--solo) 88%, var(--danger));
    border: 1px solid var(--border);
    border-radius: 2px;
    position: relative;
    overflow: hidden;
  }

  .meter i {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    background: var(--bg-inset);
    transition: height 0.08s linear;
  }

  .s-db {
    font-family: var(--mono);
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .split {
    height: 6px;
    flex-shrink: 0;
    background: var(--border);
    cursor: row-resize;
    touch-action: none;
  }

  .split:hover {
    background: var(--accent-dim);
  }

  .detail {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .d-head {
    height: 38px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    font-size: var(--fs-sm);
  }

  .d-head b {
    font-size: var(--fs-md);
  }

  .d-head .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }

  .d-head .dim {
    color: var(--text-dim);
  }

  .d-head .sp {
    flex: 1;
  }

  .d-head .hint {
    font-size: var(--fs-xs);
  }

  .d-body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .canvas {
    flex: 1;
    min-width: 0;
    position: relative;
  }
</style>
