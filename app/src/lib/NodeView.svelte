<script lang="ts">
  // エフェクトのノード表示: 入力(音源)→ カード → 出口 を 1 本の線でつなぐ(今の直列のチェーンと同じ)。
  // 上の帯 = 線でつないだカード(左から順に通る。位置は自動で並べる)、帯の下 = わき(線から外したカードを
  // 好きな所に置いておける。設定は残り、音は通らない)。カードを帯に落とすと並びに入り、帯の下に落とすと外れる。
  // すべての編集は Command(move_effect / set_effect_prop / add_effect など)を通るので Ctrl+Z で戻せる。
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import { fxColor, fxIcon, fxKind, fxName, FX_KIND_JA, FX_PRESET_MIME, insertIndexFor, moveIndexFor } from "./fx";
  import { deviceName } from "./instruments";
  import { focusNow, keepInView } from "./menu";
  import { fmtValue, fromPos, SLIDER_MAX, toPos } from "./params";
  import { MASTER_FOCUS_ID, viewStore } from "./selection.svelte";
  import { showError } from "./toast.svelte";
  import { newFxId } from "./ids";
  import type { EffectView, ParamView, Project, TrackParams } from "./types";

  let { project, targetId, onSavePreset }: {
    project: Project;
    targetId: string;
    /** カードをエフェクトのプリセットに保存する(棚を持つ親が受け持つ) */
    onSavePreset?: (fx: EffectView) => void;
  } = $props();

  const isMaster = $derived(targetId === MASTER_FOCUS_ID);
  const track = $derived(isMaster ? null : (project.tracks.find((t) => t.id === targetId) ?? null));
  const targetName = $derived(isMaster ? "マスター" : (track?.name ?? ""));

  // ---- つまみの一覧(インスペクターと同じ取り方。どの編集でも取り直す = AI の編集も反映) ----
  let info = $state<TrackParams | null>(null);
  $effect(() => {
    const id = targetId;
    void project;
    (id === MASTER_FOCUS_ID ? api.getMasterParams() : api.getTrackParams(id))
      .then((r) => {
        info = r;
        localPos = {};
      })
      .catch(() => (info = null));
  });

  let clapEffects = $state<api.ClapPluginInfo[]>([]);
  let clapLoaded = false;
  $effect(() => {
    if (clapLoaded) return;
    clapLoaded = true;
    api
      .clapPlugins()
      .then((r) => (clapEffects = r.plugins.filter((p) => p.effect)))
      .catch(() => {});
  });

  const all = $derived(info?.effects ?? []);
  const chain = $derived(all.filter((e) => !e.parked));
  const parked = $derived(all.filter((e) => e.parked));

  // ---- 並べ方 ----
  const IO_W = 120;
  const CARD_W = 196;
  const GAP = 44;
  const TOP = 36;
  /** 帯の高さ(これより下に落とすと外す) */
  const BAND_H = 210;
  const PORT_Y = 22;
  const chainX = (i: number) => 24 + IO_W + GAP + i * (CARD_W + GAP);

  /** わきのカードの位置(保存していなければ帯の下に並べる)。ドラッグの直後は保存が戻るまで手元の位置 */
  let localPos = $state<Record<string, [number, number]>>({});
  function parkedPos(e: EffectView, k: number): [number, number] {
    return localPos[e.id] ?? e.pos ?? [chainX(k), BAND_H + 40];
  }

  // ---- ドラッグ ----
  let drag = $state<{ id: string; wasParked: boolean; x: number; y: number; ox: number; oy: number; moved: boolean } | null>(null);
  let canvasEl = $state<HTMLDivElement | undefined>(undefined);

  /** 棚(エフェクトのプリセット)からドラッグして来ているときの、カードの左上の位置 */
  let ghost = $state<{ x: number; y: number } | null>(null);

  /** ドラッグ中のカードが帯の中なら、並びのどこに入るか(帯の下なら null) */
  const dropIndex = $derived.by((): number | null => {
    const d = drag?.moved ? drag : ghost;
    if (!d || d.y + 60 > BAND_H) return null;
    const cx = d.x + CARD_W / 2;
    return chain.filter((e) => e.id !== drag?.id).filter((_, i) => chainX(i) + CARD_W / 2 < cx).length;
  });

  function spacePoint(ev: DragEvent): { x: number; y: number } | null {
    if (!canvasEl) return null;
    const r = canvasEl.getBoundingClientRect();
    return {
      x: Math.max(0, ev.clientX - r.left + canvasEl.scrollLeft - CARD_W / 2),
      y: Math.max(0, ev.clientY - r.top + canvasEl.scrollTop - 16),
    };
  }
  function onPresetOver(ev: DragEvent) {
    if (!ev.dataTransfer?.types.includes(FX_PRESET_MIME)) return;
    ev.preventDefault();
    ev.dataTransfer.dropEffect = "copy";
    ghost = spacePoint(ev);
  }
  function onPresetLeave(ev: DragEvent) {
    if (!canvasEl?.contains(ev.relatedTarget as Node | null)) ghost = null;
  }
  async function onPresetDrop(ev: DragEvent) {
    const name = ev.dataTransfer?.getData(FX_PRESET_MIME);
    const idx = dropIndex;
    const pt = spacePoint(ev);
    ghost = null;
    if (!name || !pt) return;
    ev.preventDefault();
    try {
      if (idx === null) {
        await api.applyFxPreset(targetId, name, { parked: true, pos: [Math.round(pt.x), Math.round(Math.max(pt.y, BAND_H - 40))] });
      } else {
        await api.applyFxPreset(targetId, name, { index: insertIndexFor(all, idx) });
      }
    } catch (err) {
      showError("エフェクトを足せませんでした", err);
    }
  }

  /** 画面に出す並び(ドラッグ中は、つかんだカードを除いて、入る所を空ける) */
  const layout = $derived.by(() => {
    const d = drag;
    const rest = chain.filter((e) => !d || e.id !== d.id);
    const slots: (EffectView | null)[] = [...rest];
    if (dropIndex !== null) slots.splice(dropIndex, 0, null);
    return slots;
  });

  function posOf(e: EffectView): [number, number] {
    if (drag?.id === e.id) return [drag.x, drag.y];
    if (!e.parked) return [chainX(layout.indexOf(e)), TOP];
    return parkedPos(e, parked.indexOf(e));
  }

  function onCardDown(ev: PointerEvent, e: EffectView) {
    if (ev.button !== 0 || !canvasEl) return;
    const target = ev.target as HTMLElement;
    if (target.closest("button, input, select, textarea")) return;
    ev.preventDefault();
    const [x, y] = posOf(e);
    const rect = canvasEl.getBoundingClientRect();
    const ox = ev.clientX - rect.left + canvasEl.scrollLeft - x;
    const oy = ev.clientY - rect.top + canvasEl.scrollTop - y;
    drag = { id: e.id, wasParked: !!e.parked, x, y, ox, oy, moved: false };
    const move = (m: PointerEvent) => {
      if (!drag || !canvasEl) return;
      const r = canvasEl.getBoundingClientRect();
      const nx = Math.max(0, m.clientX - r.left + canvasEl.scrollLeft - drag.ox);
      const ny = Math.max(0, m.clientY - r.top + canvasEl.scrollTop - drag.oy);
      const moved = drag.moved || Math.abs(nx - x) + Math.abs(ny - y) > 4;
      drag = { ...drag, x: nx, y: ny, moved };
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const d = drag;
      const idx = dropIndex;
      drag = null;
      if (!d || !d.moved) return;
      drop(e, d, idx);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function drop(e: EffectView, d: { x: number; y: number; wasParked: boolean }, idx: number | null) {
    const name = fxName(e);
    const pos: [number, number] = [Math.round(d.x), Math.round(Math.max(d.y, BAND_H - 40))];
    if (idx === null) {
      // 帯の下: 外す(もう外してあれば置き場所だけ)
      localPos = { ...localPos, [e.id]: pos };
      const cmds: unknown[] = [];
      if (!d.wasParked) cmds.push({ op: "set_effect_prop", id: e.id, prop: "parked", value: true });
      cmds.push({ op: "set_effect_prop", id: e.id, prop: "pos", value: pos });
      edit(cmds, d.wasParked ? `${targetName} の ${name} を動かす` : `${targetName} の ${name} を線から外す`);
      return;
    }
    // 帯の中: 並びに入れる(戻す・並べ替え)
    const to = moveIndexFor(all, e.id, idx);
    const cmds: unknown[] = [];
    if (d.wasParked) {
      cmds.push({ op: "set_effect_prop", id: e.id, prop: "parked", value: false });
      cmds.push({ op: "set_effect_prop", id: e.id, prop: "pos", value: null });
    }
    if (to !== null) cmds.push({ op: "move_effect", id: e.id, to_index: to });
    if (cmds.length > 0) edit(cmds, d.wasParked ? `${targetName} の ${name} を線に戻す` : `${targetName} の ${name} を ${idx + 1} 番目へ`);
  }

  async function edit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (err) {
      showError("変更できませんでした", err);
    }
  }

  // ---- カードの操作 ----
  function toggleBypass(e: EffectView) {
    edit([{ op: "set_effect_bypass", id: e.id, bypass: !e.bypass }], `${targetName} の ${fxName(e)} を${e.bypass ? "有効に" : "バイパス"}`);
  }

  let renaming = $state<string | null>(null);
  function commitName(e: EffectView, v: string) {
    // Enter の後、入力欄が消えるときの blur でもう一度呼ばれるので 1 回だけ
    if (renaming !== e.id) return;
    renaming = null;
    const label = v.trim();
    if ((label || undefined) === e.label) return;
    edit([{ op: "set_effect_prop", id: e.id, prop: "label", value: label || null }], `${targetName} の ${fxName(e)} の名前を「${label || fxName({ ...e, label: undefined })}」に`);
  }

  let noting = $state<string | null>(null);
  function commitNote(e: EffectView, v: string) {
    if (noting !== e.id) return;
    noting = null;
    const note = v.trim();
    if ((note || undefined) === e.note) return;
    edit([{ op: "set_effect_prop", id: e.id, prop: "note", value: note || null }], `${targetName} の ${fxName(e)} のメモ`);
  }

  function park(e: EffectView, on: boolean) {
    menu = null;
    if (on) {
      edit([{ op: "set_effect_prop", id: e.id, prop: "parked", value: true }], `${targetName} の ${fxName(e)} を線から外す`);
    } else {
      // 並びの最後へ戻す
      const to = moveIndexFor(all, e.id, chain.length);
      const cmds: unknown[] = [
        { op: "set_effect_prop", id: e.id, prop: "parked", value: false },
        { op: "set_effect_prop", id: e.id, prop: "pos", value: null },
      ];
      if (to !== null) cmds.push({ op: "move_effect", id: e.id, to_index: to });
      edit(cmds, `${targetName} の ${fxName(e)} を線に戻す`);
    }
  }

  function remove(e: EffectView) {
    menu = null;
    edit([{ op: "remove_effect", id: e.id }], `${targetName} の ${fxName(e)} を削除`);
  }

  function commitParam(p: ParamView, raw: string | number | boolean) {
    let value: unknown = raw;
    if (p.range.kind === "float") value = Number(raw);
    if (p.range.kind === "int") value = Math.round(Number(raw));
    if (p.range.kind === "bool") value = Boolean(raw);
    edit(
      [isMaster ? { op: "set_master_param", path: p.path, value } : { op: "set_param", track: targetId, path: p.path, value }],
      `${targetName} の ${p.display_name} を変更`,
    );
  }
  let dragValues = $state<Record<string, number>>({});

  // ---- メニュー ----
  let menu = $state<{ fx: EffectView; x: number; y: number } | null>(null);
  let addMenu = $state<{ x: number; y: number } | null>(null);
  function at(ev: MouseEvent) {
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    return { x: r.left, y: r.bottom + 4 };
  }

  function addEffect(name: string) {
    addMenu = null;
    const clapId = name.startsWith("clap:") ? name.slice(5) : null;
    const effect = clapId ? { id: newFxId(), type: "clap", plugin_id: clapId } : { id: newFxId(), type: "builtin", name };
    const label = clapId ? (clapEffects.find((p) => p.id === clapId)?.name ?? clapId) : name;
    edit(
      [isMaster ? { op: "add_master_effect", effect } : { op: "add_effect", track: targetId, effect }],
      `${targetName} に ${label} を追加`,
    );
  }

  // ---- 入力・出口 ----
  const inputLabel = $derived(isMaster ? "全トラック" : track?.kind === "bus" ? "送られてきた音" : track?.kind === "audio" ? "音声" : deviceName(track?.device ?? null));
  const outX = $derived(chainX(layout.length) + (layout.length === 0 ? 0 : 0));
  const outputLabel = $derived.by(() => {
    if (isMaster) return "出力";
    if (!track) return "";
    const sends = (track.sends ?? []).length;
    return `音量 ${track.volume_db.toFixed(1)} dB${sends ? `・送り ${sends}` : ""}`;
  });

  // ---- 線(つないだカードの右の口 → 次の左の口) ----
  const wires = $derived.by(() => {
    const pts: [number, number][] = [[24 + IO_W, TOP + PORT_Y + 12]];
    layout.forEach((e, i) => {
      if (e) pts.push([chainX(i), TOP + PORT_Y], [chainX(i) + CARD_W, TOP + PORT_Y]);
      else pts.push([chainX(i), TOP + PORT_Y], [chainX(i), TOP + PORT_Y]); // 空けた所は素通し
    });
    pts.push([outX, TOP + PORT_Y + 12]);
    const paths: string[] = [];
    for (let i = 0; i + 1 < pts.length; i += 2) {
      const [a, b] = [pts[i], pts[i + 1]];
      const dx = Math.max(20, (b[0] - a[0]) * 0.5);
      paths.push(`M${a[0]} ${a[1]} C${a[0] + dx} ${a[1]}, ${b[0] - dx} ${b[1]}, ${b[0]} ${b[1]}`);
    }
    return paths;
  });

  const size = $derived.by(() => {
    let w = outX + IO_W + 60;
    let h = BAND_H + 260;
    parked.forEach((e, k) => {
      const [x, y] = parkedPos(e, k);
      w = Math.max(w, x + CARD_W + 60);
      h = Math.max(h, y + 240);
    });
    if (ghost) {
      w = Math.max(w, ghost.x + CARD_W + 60);
      h = Math.max(h, ghost.y + 240);
    }
    if (drag) {
      w = Math.max(w, drag.x + CARD_W + 60);
      h = Math.max(h, drag.y + 240);
    }
    return { w, h };
  });

  const shownParams = (e: EffectView) => e.params.slice(0, fxKind(e) === "clap" ? 2 : 3);
</script>

<div class="nodes" bind:this={canvasEl} ondragover={onPresetOver} ondragleave={onPresetLeave} ondrop={onPresetDrop} role="application" aria-label="エフェクトのノード表示">
  <div class="space" style="width:{size.w}px;height:{size.h}px">
    <div class="band" style="height:{BAND_H}px"></div>
    <div class="side-label" style="top:{BAND_H + 10}px">
      <Icon name="unplug" size={12} />わき(線から外したカードを置いておける。このトラックの中だけで、音は通らない)
    </div>
    {#if drag && drag.moved}
      <div class="drop-hint" style="left:{drag.x}px;top:{drag.y - 26}px">
        {dropIndex !== null ? (drag.wasParked ? "ここで離すと線に戻す" : "ここで離すと並べ替え") : drag.wasParked ? "わきに置く" : "ここで離すと線から外す(設定はそのまま)"}
      </div>
    {/if}
    {#if ghost}
      <div class="drop-ghost" style="left:{ghost.x}px;top:{ghost.y}px;width:{CARD_W}px">
        <Icon name="archive" size={13} />{dropIndex !== null ? `${dropIndex + 1} 番目に入れる` : "わきに置く"}
      </div>
    {/if}
    <svg class="wires" width={size.w} height={size.h}>
      {#each wires as d, i (i)}
        <path class="wire" {d} />
        <path class="wire flow" {d} />
      {/each}
    </svg>

    <div class="io" style="left:24px;top:{TOP}px;width:{IO_W}px">
      <span class="port out"></span>
      <Icon name={isMaster ? "merge" : "plug"} />
      <b>入力</b><small>{inputLabel}</small>
    </div>
    <button class="add" style="left:{outX - GAP / 2 - 12}px;top:{TOP + PORT_Y}px" onclick={(e) => (addMenu = at(e))} title="エフェクトを足す(並びの最後に)" aria-label="エフェクトを足す"
      ><Icon name="plus" size={14} /></button
    >
    <div class="io" style="left:{outX}px;top:{TOP}px;width:{IO_W}px">
      <span class="port in"></span>
      <Icon name="volume-2" />
      <b>出口</b><small>{outputLabel}</small>
    </div>

    {#each all as e (e.id)}
      {@const [x, y] = posOf(e)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="card"
        class:parked={e.parked}
        class:bypass={e.bypass && !e.parked}
        class:lifted={drag?.id === e.id}
        class:hl={viewStore.highlightFx === e.id}
        style="left:{x}px;top:{y}px;width:{CARD_W}px;--nc:{fxColor(e)}"
        onpointerdown={(ev) => onCardDown(ev, e)}
      >
        <span class="port in"></span><span class="port out"></span>
        <div class="bar">
          <span class="kind"><Icon name={fxIcon(e)} size={13} /></span>
          {#if renaming === e.id}
            <input
              class="name-input"
              value={e.label ?? ""}
              placeholder={fxName({ ...e, label: undefined })}
              use:focusNow
              onkeydown={(ev) => {
                if (ev.isComposing) return;
                if (ev.key === "Enter") commitName(e, ev.currentTarget.value);
                else if (ev.key === "Escape") renaming = null;
              }}
              onblur={(ev) => commitName(e, ev.currentTarget.value)}
            />
          {:else}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <b title={`${fxName(e)}(ダブルクリックで名前を変える)`} ondblclick={() => (renaming = e.id)}>{fxName(e)}</b>
          {/if}
          {#if e.parked}
            <span class="dim" title="線から外してある(音は通らない)"><Icon name="unplug" size={13} /></span>
          {:else}
            <button class="btn sm icon" class:on={!e.bypass} onclick={() => toggleBypass(e)} title={e.bypass ? "バイパス中(押すと有効)" : "有効(押すとバイパス)"} aria-label="有効 / バイパス"
              ><Icon name="power" /></button
            >
          {/if}
          <button class="btn sm icon ghost" onclick={(ev) => (menu = { fx: e, ...at(ev) })} title="その他" aria-label="その他"><Icon name="ellipsis" /></button>
        </div>
        <div class="body">
          <span class="kind-label">{fxKind(e) === "clap" ? "CLAP" : (FX_KIND_JA[fxKind(e)] ?? fxKind(e))}{#if e.label}<span class="dim"> · {fxName({ ...e, label: undefined })}</span>{/if}</span>
          {#if noting === e.id}
            <textarea
              class="note-input"
              rows="2"
              use:focusNow
              placeholder="メモ(なぜ取っておいたかなど)"
              value={e.note ?? ""}
              onkeydown={(ev) => {
                if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) commitNote(e, ev.currentTarget.value);
                else if (ev.key === "Escape") noting = null;
              }}
              onblur={(ev) => commitNote(e, ev.currentTarget.value)}
            ></textarea>
          {:else if e.note}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="note" ondblclick={() => (noting = e.id)} title="ダブルクリックで書き直す"><Icon name="pin" size={11} />{e.note}</div>
          {/if}
          {#each shownParams(e) as p (p.path)}
            <div class="pm" title={p.description}>
              <div class="top"><span>{p.display_name}</span><span>{fmtValue(p, dragValues[p.path])}</span></div>
              {#if p.range.kind === "float" || p.range.kind === "int"}
                <input
                  type="range"
                  min="0"
                  max={SLIDER_MAX}
                  value={toPos(p, Number(p.current))}
                  oninput={(ev) => (dragValues[p.path] = fromPos(p, Number(ev.currentTarget.value)))}
                  onchange={(ev) => {
                    delete dragValues[p.path];
                    commitParam(p, fromPos(p, Number(ev.currentTarget.value)));
                  }}
                  aria-label={p.display_name}
                />
              {:else if p.range.kind === "enum"}
                <select value={String(p.current)} onchange={(ev) => commitParam(p, ev.currentTarget.value)} aria-label={p.display_name}>
                  {#each p.range.choices as c (c)}<option value={c}>{c}</option>{/each}
                </select>
              {/if}
            </div>
          {/each}
          {#if e.params.length > shownParams(e).length}
            <span class="more">ほか {(e.param_total ?? e.params.length) - shownParams(e).length} 個(インスペクターで)</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
</div>

{#if menu || addMenu}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="menu-backdrop" role="presentation" onclick={() => ((menu = null), (addMenu = null))}></div>
{/if}
{#if menu}
  {@const e = menu.fx}
  <div class="menu" use:keepInView style="left:{menu.x}px;top:{menu.y}px">
    <button onclick={() => {
        renaming = e.id;
        menu = null;
      }}><Icon name="pencil" />名前を変える</button>
    <button onclick={() => {
        noting = e.id;
        menu = null;
      }}><Icon name="pin" />メモを書く</button>
    {#if e.parked}
      <button onclick={() => park(e, false)}><Icon name="arrow-up" />線に戻す(並びの最後へ)</button>
    {:else}
      <button onclick={() => park(e, true)}><Icon name="unplug" />線から外す(わきに置く)</button>
    {/if}
    {#if onSavePreset}
      <button onclick={() => {
          const fx = e;
          menu = null;
          onSavePreset(fx);
        }}><Icon name="archive" />エフェクトのプリセットに保存</button>
    {/if}
    {#if fxKind(e) === "clap" && !e.missing}
      <button onclick={() => {
          const id = e.id;
          menu = null;
          api.clapOpenGui(null, id).catch((err) => showError("プラグインの画面を開けませんでした", err));
        }}
        ><Icon name="app-window" />プラグインの画面を開く</button
      >
    {/if}
    <div class="menu-sep"></div>
    <button class="danger" onclick={() => remove(e)}><Icon name="trash-2" />削除<span class="key">Ctrl+Z で戻せます</span></button>
  </div>
{/if}
{#if addMenu && info}
  <div class="menu" use:keepInView style="left:{addMenu.x}px;top:{addMenu.y}px">
    <div class="menu-h">内蔵</div>
    {#each info.available_effects as fx (fx.name)}
      <button class="rich" onclick={() => addEffect(fx.name)}><span>{FX_KIND_JA[fx.name] ?? fx.name}<small>{fx.name} · {fx.description}</small></span></button>
    {/each}
    {#if clapEffects.length > 0}
      <div class="menu-sep"></div>
      <div class="menu-h">CLAP プラグイン</div>
      {#each clapEffects as p (p.id)}
        <button class="rich" onclick={() => addEffect(`clap:${p.id}`)}><Icon name="plug" /><span>{p.name}<small>{p.vendor} {p.version}</small></span></button>
      {/each}
    {/if}
  </div>
{/if}

<style>
  .nodes {
    position: absolute;
    inset: 0;
    overflow: auto;
    user-select: none;
    background:
      radial-gradient(circle at 1px 1px, #2b2b2b 1px, transparent 1.4px) 0 0 / 20px 20px,
      #121212;
  }

  .space {
    position: relative;
    min-width: 100%;
    min-height: 100%;
  }

  .band {
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    border-bottom: 1px dashed #333;
    pointer-events: none;
  }

  .side-label {
    position: absolute;
    left: 24px;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--fs-xs);
    color: var(--text-faint);
    pointer-events: none;
  }

  .drop-hint {
    position: absolute;
    z-index: 5;
    font-size: var(--fs-xs);
    color: var(--accent);
    background: rgba(10, 30, 28, 0.92);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-md);
    padding: 3px 8px;
    white-space: nowrap;
    pointer-events: none;
  }

  .drop-ghost {
    position: absolute;
    z-index: 5;
    height: 60px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    border: 1px dashed var(--accent);
    border-radius: 10px;
    background: rgba(10, 30, 28, 0.6);
    color: var(--accent);
    font-size: var(--fs-xs);
    pointer-events: none;
  }

  .wires {
    position: absolute;
    left: 0;
    top: 0;
    pointer-events: none;
  }

  .wire {
    fill: none;
    stroke: #6b6b6b;
    stroke-width: 2.5;
  }

  .wire.flow {
    stroke: #a0a0a0;
    stroke-dasharray: 2 10;
    stroke-linecap: round;
    animation: flow 1.2s linear infinite;
  }

  @keyframes flow {
    to {
      stroke-dashoffset: -24;
    }
  }

  .io {
    position: absolute;
    padding: 8px;
    border-radius: 10px;
    border: 1px solid var(--border-strong);
    background: #1a1a1a;
    text-align: center;
    color: var(--text-dim);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .io b {
    color: var(--text);
    font-size: var(--fs-sm);
  }

  .io small {
    font-size: 10px;
    line-height: 1.3;
  }

  .port {
    position: absolute;
    top: 16px;
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #2a2a2a;
    border: 2px solid #8a8a8a;
  }

  .io .port {
    top: 28px;
  }

  .port.in {
    left: -7px;
  }

  .port.out {
    right: -7px;
  }

  .add {
    position: absolute;
    width: 24px;
    height: 24px;
    padding: 0;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--bg-raised);
    color: var(--text-dim);
    z-index: 2;
  }

  .card {
    position: absolute;
    border-radius: 10px;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.45);
    transition:
      left 0.16s ease,
      top 0.16s ease;
    cursor: grab;
    z-index: 1;
  }

  .card.lifted {
    transition: none;
    z-index: 4;
    cursor: grabbing;
    border-color: var(--accent);
    box-shadow: 0 20px 36px rgba(0, 0, 0, 0.7);
  }

  .card.hl {
    border-color: var(--accent);
    box-shadow:
      0 0 0 1px var(--accent),
      0 6px 16px rgba(0, 0, 0, 0.45);
  }

  .card.parked {
    border-style: dashed;
    border-color: #5a5a5a;
    background: #181818;
    box-shadow: none;
  }

  .card.parked .bar {
    background: #1f1f1f;
  }

  .card.parked .port {
    border-color: #444;
    background: #151515;
  }

  .card.parked .body {
    opacity: 0.65;
  }

  .card.bypass .bar b {
    text-decoration: line-through;
    color: var(--text-dim);
  }

  .card.bypass .body {
    opacity: 0.45;
  }

  .bar {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 0 5px 0 9px;
    border-radius: 10px 10px 0 0;
    background: color-mix(in srgb, var(--nc) 28%, var(--bg-panel));
    border-bottom: 1px solid color-mix(in srgb, var(--nc) 50%, transparent);
  }

  .bar .kind {
    display: inline-flex;
    color: var(--nc);
  }

  .bar b {
    flex: 1;
    min-width: 0;
    font-size: var(--fs-md);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .name-input {
    flex: 1;
    min-width: 0;
    height: 22px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    padding: 0 5px;
  }

  .dim {
    color: var(--text-dim);
    display: inline-flex;
  }

  .body {
    padding: 6px 10px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .kind-label {
    font-size: 10px;
    color: var(--nc);
  }

  .kind-label .dim {
    display: inline;
  }

  .note {
    display: flex;
    gap: 4px;
    font-size: 10px;
    color: var(--text-dim);
    line-height: 1.4;
  }

  .note-input {
    width: 100%;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-xs);
    resize: vertical;
  }

  .pm {
    font-size: 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pm .top {
    display: flex;
    justify-content: space-between;
    color: var(--text-dim);
  }

  .pm .top span:last-child {
    font-family: var(--mono);
  }

  .pm input[type="range"] {
    width: 100%;
    height: 12px;
    margin: 0;
    accent-color: var(--nc);
  }

  .pm select {
    height: 20px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: 10px;
  }

  .more {
    font-size: 10px;
    color: var(--text-faint);
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .menu {
    position: fixed;
    z-index: 20;
    max-height: calc(100vh - 16px);
    overflow-y: auto;
    min-width: 200px;
    max-width: 360px;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 4px;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .menu button {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 0;
    background: none;
    text-align: left;
    padding: 6px 10px;
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
    --icon-size: 15px;
  }

  .menu button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .menu button > :global(.icon) {
    color: var(--text-dim);
  }

  .menu button.rich span {
    display: flex;
    flex-direction: column;
  }

  .menu small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .menu .danger {
    color: var(--danger-text);
  }

  .menu .key {
    margin-left: auto;
    padding-left: 20px;
    font-size: var(--fs-xs);
    color: var(--text-faint);
  }

  .menu-h {
    font-size: var(--fs-xs);
    color: var(--text-faint);
    padding: 4px 10px 2px;
  }

  .menu-sep {
    height: 1px;
    background: var(--border);
    margin: 3px 4px;
  }
</style>
