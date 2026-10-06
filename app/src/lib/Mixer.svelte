<script lang="ts">
  // ミキサー: 上 = 全トラックの縦の列(エフェクト・送り・パン・M/S・フェーダー・メーター。右にバスとマスター)、
  // 下 = 選んだ列のエフェクトのノード表示(NodeView)。上下の境目はドラッグで高さを変えられる。
  // 列のエフェクト名を押すと、下の同じカードが光る。すべての編集は Command を通る。
  import { untrack } from "svelte";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { effectiveLinks, fxColor, fxName, processingOrder, serialOrder } from "./fx";
  import { isEn, tr } from "./i18n.svelte";
  import { deviceIcon, deviceName } from "./instruments";
  import NodeView from "./NodeView.svelte";
  import FxPresetShelf from "./FxPresetShelf.svelte";
  import MonitorStrip from "./MonitorStrip.svelte";
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

  // ---- 処理の重さ(トラックごと。表示はなめらかに) ----
  let loads = $state<Record<string, number>>({});
  $effect(() => {
    void transportStore.seq;
    const lv = transportStore.state.levels;
    if (!transportStore.state.playing || !lv?.loads) {
      untrack(() => (loads = {}));
      return;
    }
    const prev = untrack(() => loads);
    const next: Record<string, number> = {};
    const ease = (id: string, v: number) => (prev[id] ?? v) * 0.6 + v * 0.4;
    project.tracks.forEach((t, i) => (next[t.id] = ease(t.id, lv.loads?.[i] ?? 0)));
    next[MASTER_FOCUS_ID] = ease(MASTER_FOCUS_ID, lv.master_load ?? 0);
    loads = next;
  });
  const LOAD_TITLE = $derived(
    tr(
      "処理の重さ(音の長さに対する処理時間の割合。音源の発音は鳴らした声の数で按分した目安)",
      "Processing load (processing time as a share of audio time; instrument voices are an estimate split by voice count)",
    ),
  );

  // ---- 編集 ----
  function edit(commands: unknown[], label: string) {
    api.applyEdit(commands, label).catch(() => {});
  }
  let dragVol = $state<Record<string, number>>({});
  function volumeCommand(t: Track | null, v: number): unknown {
    return t ? { op: "set_track_prop", id: t.id, prop: "volume_db", value: v } : { op: "set_master_volume", volume_db: v };
  }
  function setVolume(t: Track | null, v: number) {
    const key = t?.id ?? MASTER_FOCUS_ID;
    delete dragVol[key];
    edit([volumeCommand(t, v)], t
        ? tr(`${t.name} の音量を ${v.toFixed(1)} dB に`, `Set ${t.name} volume to ${v.toFixed(1)} dB`)
        : tr(`マスター音量を ${v.toFixed(1)} dB に`, `Set master volume to ${v.toFixed(1)} dB`));
  }
  /** ドラッグ中: 表示と音だけ変える(離したときに 1 回だけ確定する) */
  function dragVolume(t: Track | null, v: number) {
    dragVol[t?.id ?? MASTER_FOCUS_ID] = v;
    api.previewEdit([volumeCommand(t, v)]);
  }
  function setPan(t: Track, v: number) {
    edit([{ op: "set_track_prop", id: t.id, prop: "pan", value: v }], tr(`${t.name} のパンを ${v.toFixed(2)} に`, `Set ${t.name} pan to ${v.toFixed(2)}`));
  }
  function toggle(t: Track, prop: "mute" | "solo") {
    edit([{ op: "set_track_prop", id: t.id, prop, value: !t[prop] }], tr(
        `${t.name} の${prop === "mute" ? "ミュート" : "ソロ"}を${t[prop] ? "解除" : "オン"}`,
        `${prop === "mute" ? "Mute" : "Solo"} ${t[prop] ? "off" : "on"}: ${t.name}`,
      ));
  }
  let dragSend = $state<Record<string, number>>({});
  function setSend(t: Track, bus: Track, v: number) {
    delete dragSend[`${t.id}>${bus.id}`];
    const cur = t.sends?.find((s) => s.target === bus.id);
    if (v <= -60) {
      if (cur) edit([{ op: "set_send", track: t.id, target: bus.id }], tr(`${t.name} から ${bus.name} への送りを外す`, `Remove send from ${t.name} to ${bus.name}`));
      return;
    }
    edit(
      [{ op: "set_send", track: t.id, target: bus.id, level_db: v, pre_fader: cur?.pre_fader ?? false }],
      tr(`${t.name} から ${bus.name} への送りを ${v.toFixed(1)} dB に`, `Set send from ${t.name} to ${bus.name} to ${v.toFixed(1)} dB`),
    );
  }
  function openPicker(e: MouseEvent, t: Track) {
    e.stopPropagation();
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    instrumentPickerStore.open = { trackId: t.id, x: r.left, y: r.bottom + 4 };
  }
  const receivers = (bus: Track) =>
    project.tracks.filter((t) => t.output === bus.id || t.sends?.some((s) => s.target === bus.id));
  /** `from` の音が(出力・送りをたどって)`to` に届くか */
  function reaches(from: string, to: string): boolean {
    const seen = new Set<string>();
    const stack = [from];
    while (stack.length > 0) {
      const id = stack.pop()!;
      if (id === to) return true;
      if (seen.has(id)) continue;
      seen.add(id);
      const t = project.tracks.find((x) => x.id === id);
      if (!t) continue;
      if (t.output) stack.push(t.output);
      for (const s of t.sends ?? []) stack.push(s.target);
    }
    return false;
  }
  /** t の出力先・送り先にできるバス(自分と、輪になるものを除く) */
  const targetsFor = (t: Track) => buses.filter((b) => b.id !== t.id && !reaches(b.id, t.id));
  function setOutput(t: Track, target: string) {
    const bus = buses.find((b) => b.id === target);
    edit(
      [{ op: "set_track_prop", id: t.id, prop: "output", value: bus ? bus.id : null }],
      tr(`${t.name} の出力先を ${bus ? bus.name : "マスター"} に`, `Set ${t.name} output to ${bus ? bus.name : "Master"}`),
    );
  }
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
      trackName: selected === MASTER_FOCUS_ID ? tr("マスター", "Master") : (selectedTrack?.name ?? ""),
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
  <div class="s-sec"><span>{tr("エフェクト", "Effects")}</span><span>{c.list.length}</span></div>
  <div class="slots">
    {#each c.list as e (e.id)}
      <button
        class="slot"
        class:bypass={e.bypass}
        class:hl={viewStore.highlightFx === e.id}
        style="--nc:{fxColor(e)}"
        title={tr(`${fxName(e, clapNames)}(押すと下のカードが光る)`, `${fxName(e, clapNames)} (click to highlight its card below)`)}
        onclick={(ev) => {
          ev.stopPropagation();
          select(ownerId, e.id);
        }}><span class="cb"></span><span class="led"></span><span class="nm">{fxName(e, clapNames)}</span></button
      >
    {/each}
    {#if c.branched}
      <div
        class="folded"
        title={tr("分かれたり合流したりしている(並びは処理の順。つながりはノード表示で)", "Splits or merges (listed in processing order; see the node view for wiring)")}
      >
        <Icon name="split" size={11} />{tr("分岐あり", "Branched")}
      </div>
    {/if}
    {#if c.mute > 0}
      <div
        class="folded"
        title={tr("入力から出口まで線でたどれないカード(設定は残っていて、音は通らない)", "Cards not wired from input to output (settings kept, no sound passes)")}
      >
        <Icon name="unplug" size={11} />{tr(`鳴らない ${c.mute}`, `Silent ${c.mute}`)}
      </div>
    {/if}
  </div>
{/snippet}

{#snippet sendsOf(t: Track)}
  {#if targetsFor(t).length > 0 || (t.sends?.length ?? 0) > 0}
    <div class="s-sec"><span>{tr("送り", "Sends")}</span></div>
    <div class="sends">
      {#each buses.filter((b) => b.id !== t.id && (t.sends?.some((s) => s.target === b.id) || !reaches(b.id, t.id))) as b (b.id)}
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
            oninput={(e) => {
              const v = Number(e.currentTarget.value);
              dragSend[`${t.id}>${b.id}`] = v;
              const pre = t.sends?.find((s) => s.target === b.id)?.pre_fader ?? false;
              api.previewEdit([
                v <= -60
                  ? { op: "set_send", track: t.id, target: b.id }
                  : { op: "set_send", track: t.id, target: b.id, level_db: v, pre_fader: pre },
              ]);
            }}
            onchange={(e) => setSend(t, b, Number(e.currentTarget.value))}
            aria-label={tr(`${b.name} へ送る量`, `Send level to ${b.name}`)}
            title={tr(`${b.name} へ送る量(左端で送らない)`, `Send level to ${b.name} (far left = no send)`)}
          />
        </div>
      {/each}
    </div>
  {/if}
  {#if buses.length > 0 && (targetsFor(t).length > 0 || t.output)}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="out" role="presentation" onclick={(e) => e.stopPropagation()}>
      <span>{tr("出力", "Output")}</span>
      <select
        value={t.output ?? ""}
        onchange={(e) => setOutput(t, e.currentTarget.value)}
        aria-label={tr("出力先", "Output")}
        title={tr("出力先(バスにまとめるとグループになる)", "Output (route to a bus to group tracks)")}
      >
        <option value="">{tr("マスター", "Master")}</option>
        {#each buses.filter((b) => b.id === t.output || targetsFor(t).includes(b)) as b (b.id)}
          <option value={b.id}>{b.name}</option>
        {/each}
      </select>
    </div>
  {/if}
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
      oninput={(e) => dragVolume(t, Number(e.currentTarget.value))}
      onchange={(e) => setVolume(t, Number(e.currentTarget.value))}
      ondblclick={() => setVolume(t, 0)}
      aria-label={tr("音量", "Volume")}
      title={tr("音量(ダブルクリックで 0 dB)", "Volume (double-click for 0 dB)")}
    />
    <div class="meter" title={tr("レベル", "Level")}><i style="height:{100 - meterPct(meters[key] ?? -120)}%"></i></div>
  </div>
  <span class="s-db">{vol > 0 ? "+" : ""}{vol.toFixed(1)} dB</span>
{/snippet}

<div class="mixer" bind:this={rootEl} bind:clientHeight={rootH}>
  <div class="strips" style="height:{shownTop}px">
    {#each plain as t (t.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div class="strip" role="group" aria-label={t.name} class:sel={selected === t.id} style="--c:{t.color ?? '#555'}" onclick={() => select(t.id)}>
        <div class="s-top"></div>
        <div class="s-name">
          <Icon name={KIND_ICON[t.kind]} size={13} /><span title={t.name}>{t.name}</span>
          {#if (loads[t.id] ?? 0) >= 0.5}<small class="load" class:heavy={loads[t.id] >= 20} title={LOAD_TITLE}>{loads[t.id].toFixed(0)}%</small>{/if}
        </div>
        {#if t.kind === "midi"}
          <button class="s-dev" onclick={(e) => openPicker(e, t)} title={tr("音源を変える", "Change instrument")}
            ><Icon name={deviceIcon(t.device)} size={12} /><span>{deviceName(t.device, clapNames)}</span></button
          >
        {:else}
          <div class="s-dev plain">{tr("音声", "Audio")}</div>
        {/if}
        {@render slots(t.effects, t.fx_links, t.id)}
        {@render sendsOf(t)}
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
              oninput={(e) => api.previewEdit([{ op: "set_track_prop", id: t.id, prop: "pan", value: Number(e.currentTarget.value) }])}
              onchange={(e) => setPan(t, Number(e.currentTarget.value))}
              ondblclick={() => setPan(t, 0)}
              aria-label={tr("パン", "Pan")}
              title={tr("パン(ダブルクリックで中央)", "Pan (double-click to center)")}
            />
            <span>R</span>
          </div>
          <div class="ms">
            <button class="btn letter" class:m-on={t.mute} onclick={(e) => (e.stopPropagation(), toggle(t, "mute"))} title={tr("ミュート", "Mute")}>M</button>
            <button class="btn letter" class:s-on={t.solo} onclick={(e) => (e.stopPropagation(), toggle(t, "solo"))} title={tr("ソロ", "Solo")}>S</button>
          </div>
          {@render fader(t)}
        </div>
      </div>
    {/each}

    {#if buses.length > 0}<span class="grp-label">{tr("バス", "Buses")}</span>{/if}
    {#each buses as t (t.id)}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <div class="strip bus" role="group" aria-label={t.name} class:sel={selected === t.id} style="--c:{t.color ?? '#e8a07c'}" onclick={() => select(t.id)}>
        <div class="s-top"></div>
        <div class="s-name">
          <Icon name="merge" size={13} /><span title={t.name}>{t.name}</span>
          {#if (loads[t.id] ?? 0) >= 0.5}<small class="load" class:heavy={loads[t.id] >= 20} title={LOAD_TITLE}>{loads[t.id].toFixed(0)}%</small>{/if}
        </div>
        <div class="s-dev plain" title={tr("このバスへ送っているトラック", "Tracks sending to this bus")}>
          {tr("受けている", "From")}: {receivers(t).map((r) => r.name).join(tr("・", ", ")) || tr("なし", "none")}
        </div>
        {@render slots(t.effects, t.fx_links, t.id)}
        {@render sendsOf(t)}
        <div class="s-bottom">
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="pan" role="presentation" onclick={(e) => e.stopPropagation()}>
            <span>L</span>
            <input type="range" min="-1" max="1" step="0.02" value={t.pan} oninput={(e) => api.previewEdit([{ op: "set_track_prop", id: t.id, prop: "pan", value: Number(e.currentTarget.value) }])} onchange={(e) => setPan(t, Number(e.currentTarget.value))} ondblclick={() => setPan(t, 0)} aria-label={tr("パン", "Pan")} />
            <span>R</span>
          </div>
          <div class="ms">
            <button class="btn letter" class:m-on={t.mute} onclick={(e) => (e.stopPropagation(), toggle(t, "mute"))} title={tr("ミュート", "Mute")}>M</button>
            <button class="btn letter" class:s-on={t.solo} onclick={(e) => (e.stopPropagation(), toggle(t, "solo"))} title={tr("ソロ", "Solo")}>S</button>
          </div>
          {@render fader(t)}
        </div>
      </div>
    {/each}

    <span class="grp-label">{tr("出口", "Output")}</span>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <div class="strip master" role="group" aria-label={tr("マスター", "Master")} class:sel={selected === MASTER_FOCUS_ID} onclick={() => select(MASTER_FOCUS_ID)}>
      <div class="s-top" style="background:var(--accent)"></div>
      <div class="s-name">
        <Icon name="volume-2" size={13} /><span>{tr("マスター", "Master")}</span>
        {#if (loads[MASTER_FOCUS_ID] ?? 0) >= 0.5}<small class="load" class:heavy={loads[MASTER_FOCUS_ID] >= 20} title={LOAD_TITLE}
            >{loads[MASTER_FOCUS_ID].toFixed(0)}%</small
          >{/if}
      </div>
      <div class="s-dev plain">{tr("曲全体", "Whole song")}</div>
      {@render slots(project.master.effects, project.master.fx_links, MASTER_FOCUS_ID)}
      <div class="s-bottom">{@render fader(null)}</div>
    </div>
    <MonitorStrip />
  </div>

  <div class="split" role="separator" aria-orientation="horizontal" title={tr("ドラッグで高さを変える", "Drag to resize")} onpointerdown={startSplit}></div>

  <div class="detail">
    <div class="d-head">
      <span class="dot" style="background:{selected === MASTER_FOCUS_ID ? 'var(--accent)' : (selectedTrack?.color ?? '#777')}"></span>
      {#if isEn()}<span class="dim">Effects on</span>{/if}<b>{selected === MASTER_FOCUS_ID ? tr("マスター", "Master") : selectedTrack?.name}</b>{#if !isEn()}<span class="dim">のエフェクト</span>{/if}
      <span class="sp"></span>
      <span class="dim hint"
        >{tr(
          "口から線を引いてつなぐ・線をクリックで音量 / 間に足す / 切る・Ctrl+ドラッグでまとめて切る・名前はダブルクリック",
          "Drag from a port to wire · click a wire for gain / insert / cut · Ctrl+drag to cut several · double-click to rename",
        )}</span
      >
      <button
        class="btn sm"
        class:on={inspectorOpen}
        aria-pressed={inspectorOpen}
        onclick={toggleInspector}
        title={inspectorOpen ? tr("インスペクターを閉じる", "Close inspector") : tr("インスペクターで開く(つまみを全部見る)", "Open in inspector (see all parameters)")}
        ><Icon name="sliders-horizontal" />{tr("インスペクター", "Inspector")}</button
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

  .s-name .load {
    font-size: 10px;
    font-weight: 400;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }

  .s-name .load.heavy {
    color: #e0b050;
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

  .out {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 7px 0;
    font-size: 10px;
    color: var(--text-dim);
    flex-shrink: 0;
  }

  .out select {
    flex: 1;
    min-width: 0;
    font-size: 10px;
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
