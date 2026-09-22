<script lang="ts">
  import * as api from "./api";
  import AutomationLaneRow from "./AutomationLaneRow.svelte";
  import { barAtTick, barsEndTick, buildBars } from "./barMap";
  import ClipPreview from "./ClipPreview.svelte";
  import { newTrackId } from "./ids";
  import { pianoRollStore, selectionStore, soundDesignStore } from "./selection.svelte";
  import type { Clip, PresetInfo, Project, Track } from "./types";

  let {
    project,
    playheadTick = 0,
    playing = false,
    onSeek,
  }: {
    project: Project;
    playheadTick?: number;
    playing?: boolean;
    onSeek?: (tick: number) => void;
  } = $props();

  // 4/4 の 1 小節 = 96px となる密度。小節の実幅は拍子に応じて変わる(7/8 は狭い)
  const PX_PER_WHOLE = 96;
  const pxPerTick = $derived(PX_PER_WHOLE / (project.ppq * 4));

  const endTick = $derived.by(() => {
    let end = 0;
    for (const t of project.tracks) {
      for (const c of t.clips) {
        end = Math.max(end, c.start + c.length);
      }
    }
    return end;
  });

  // 拍子イベントを考慮した小節列(グリッド・ルーラー・範囲選択の共通ソース)
  const barList = $derived(buildBars(project, endTick));
  const totalPx = $derived(barsEndTick(barList) * pxPerTick);

  function clipStyle(clip: Clip): string {
    const left = clip.start * pxPerTick;
    const width = Math.max(clip.length * pxPerTick, 8);
    return `left:${left}px;width:${width}px`;
  }

  function volumeText(t: Track): string {
    const v = t.volume_db;
    return `${v > 0 ? "+" : ""}${v.toFixed(1)} dB`;
  }

  const HEAD_W = 200;
  const playheadPx = $derived(HEAD_W + playheadTick * pxPerTick);

  // ---- 再生中の自動スクロール(再生ヘッドが見える範囲を追いかける) ----

  let root: HTMLDivElement | undefined = $state();

  let lastFollowPx: number | null = null;

  $effect(() => {
    const px = playheadPx; // 依存として追跡
    void playing;
    // 位置が変わったときだけ追従(停止中のシークにも反応し、手動スクロールは邪魔しない)
    if (px === lastFollowPx) return;
    const first = lastFollowPx === null;
    lastFollowPx = px;
    if (first) return;
    const scroller = root?.parentElement;
    if (!scroller) return;
    const view = scroller.clientWidth;
    const left = scroller.scrollLeft;
    // 右端に近づいたら(またはヘッドが画面外なら)ページ送り
    if (px > left + view - 80 || px < left + HEAD_W) {
      scroller.scrollLeft = Math.max(0, px - HEAD_W - 80);
    }
  });

  // ---- ルーラー: クリックでシーク、ドラッグで小節範囲を選択(マスク) ----

  let dragStart: { x: number; bar: number } | null = null;
  let dragging = false;

  function barAt(e: PointerEvent): number {
    const lane = e.currentTarget as HTMLElement;
    const x = e.clientX - lane.getBoundingClientRect().left;
    return barAtTick(barList, Math.max(0, x / pxPerTick)).index;
  }

  function setRange(a: number, b: number) {
    const startBar = Math.min(a, b);
    const endBar = Math.max(a, b);
    const s = barList[Math.min(startBar, barList.length - 1)];
    const e = barList[Math.min(endBar, barList.length - 1)];
    selectionStore.range = {
      startBar,
      endBar,
      startTick: s.tick,
      endTick: e.tick + e.len,
    };
  }

  function onRulerDown(e: PointerEvent) {
    dragStart = { x: e.clientX, bar: barAt(e) };
    dragging = false;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onRulerMove(e: PointerEvent) {
    if (!dragStart) return;
    if (!dragging && Math.abs(e.clientX - dragStart.x) > 4) {
      dragging = true;
    }
    if (dragging) {
      setRange(dragStart.bar, barAt(e));
    }
  }

  function onRulerUp(e: PointerEvent) {
    if (!dragStart) return;
    if (!dragging) {
      // クリック = シーク + 選択解除
      selectionStore.range = null;
      const lane = e.currentTarget as HTMLElement;
      const x = e.clientX - lane.getBoundingClientRect().left;
      onSeek?.(Math.max(0, x / pxPerTick));
    }
    dragStart = null;
    dragging = false;
  }

  const selection = $derived(selectionStore.range);

  // ---- トラック操作(Command API 経由、author: human) ----

  function setVolume(t: Track, e: Event) {
    const v = Number((e.currentTarget as HTMLInputElement).value);
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "volume_db", value: v }],
        `${t.name} の音量を ${v.toFixed(1)} dB に変更`,
      )
      .catch(() => {});
  }

  function toggleMute(t: Track) {
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "mute", value: !t.mute }],
        `${t.name} を${t.mute ? "ミュート解除" : "ミュート"}`,
      )
      .catch(() => {});
  }

  function openPianoRoll(track: Track, clip: Clip, e: MouseEvent) {
    if (clip.kind !== "midi") return;
    pianoRollStore.focus = {
      clipId: clip.id,
      clipName: clip.name,
      trackId: track.id,
      trackName: track.name,
      anchorTick: Math.max(0, (e.offsetX ?? 0) / pxPerTick),
    };
  }

  // ---- オートメーションレーンの開閉 ----

  let autoLanes = $state<Record<string, "volume_db" | "pan">>({});

  function toggleAutoLane(trackId: string) {
    if (autoLanes[trackId]) {
      const next = { ...autoLanes };
      delete next[trackId];
      autoLanes = next;
    } else {
      autoLanes = { ...autoLanes, [trackId]: "volume_db" };
    }
  }

  function hasVolumeLane(t: Track): boolean {
    return t.automation.some(
      (l) => l.target === "track/volume_db" && l.points.length > 0,
    );
  }

  // ---- トラックの右クリックメニュー(移動・削除) ----

  let trackMenu = $state<{ trackId: string; index: number; x: number; y: number } | null>(null);

  function openTrackMenu(e: MouseEvent, track: Track, index: number) {
    e.preventDefault();
    trackMenu = { trackId: track.id, index, x: e.clientX, y: e.clientY };
  }

  function moveTrack(toIndex: number) {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu || toIndex < 0 || toIndex >= project.tracks.length) return;
    const track = project.tracks[menu.index];
    api
      .applyEdit(
        [{ op: "move_track", id: menu.trackId, to_index: toIndex }],
        `${track?.name ?? "トラック"} を${toIndex < menu.index ? "上" : "下"}へ移動`,
      )
      .catch(() => {});
  }

  function deleteTrack() {
    const menu = trackMenu;
    trackMenu = null;
    if (!menu) return;
    const track = project.tracks[menu.index];
    api
      .applyEdit(
        [{ op: "remove_track", id: menu.trackId }],
        `${track?.name ?? "トラック"} を削除`,
      )
      .catch(() => {});
  }

  // ---- 音源(デバイス)の選択メニュー ----

  const INSTRUMENTS = [
    { name: "subtractive", label: "🎹 subtractive", desc: "シンセ全般(リード・ベース・パッド)" },
    { name: "drum", label: "🥁 drum", desc: "ドラムシンセ(GM 配置、キット UI 対応)" },
  ];

  let deviceMenu = $state<{ track: Track; x: number; y: number } | null>(null);
  let presets = $state<PresetInfo[]>([]);
  let presetName = $state("");
  let presetMsg = $state<string | null>(null);

  function openDeviceMenu(e: MouseEvent, track: Track) {
    e.stopPropagation();
    presetMsg = null;
    presetName = "";
    deviceMenu = { track, x: e.clientX, y: e.clientY };
    api
      .listPresets()
      .then((r) => (presets = r.presets))
      .catch(() => (presets = []));
  }

  function setDevice(name: string) {
    const menu = deviceMenu;
    deviceMenu = null;
    if (!menu) return;
    if (menu.track.device?.name === name) return; // 変更なし
    api
      .applyEdit(
        [
          {
            op: "set_device",
            track: menu.track.id,
            device: { type: "builtin", name },
          },
        ],
        `${menu.track.name} の音源を ${name} に変更`,
      )
      .catch(() => {});
  }

  function applyPreset(name: string) {
    const menu = deviceMenu;
    deviceMenu = null;
    if (!menu) return;
    api.loadPreset(menu.track.id, name).catch((e) => {
      console.error(e);
    });
  }

  async function saveCurrentPreset() {
    const menu = deviceMenu;
    const name = presetName.trim();
    if (!menu || !name) return;
    try {
      await api.savePreset(menu.track.id, name);
      presetMsg = `保存しました: ${name}`;
      presetName = "";
      const r = await api.listPresets();
      presets = r.presets;
    } catch (e) {
      presetMsg = String(e);
    }
  }

  function addTrack() {
    const id = newTrackId();
    api
      .applyEdit(
        [
          {
            op: "add_track",
            track: { id, name: `トラック ${project.tracks.length + 1}`, kind: "midi" },
          },
        ],
        "トラックを追加",
      )
      .catch(() => {});
  }

  function toggleSolo(t: Track) {
    api
      .applyEdit(
        [{ op: "set_track_prop", id: t.id, prop: "solo", value: !t.solo }],
        `${t.name} のソロを${t.solo ? "解除" : "オン"}`,
      )
      .catch(() => {});
  }
</script>

<div class="timeline" bind:this={root}>
  <!-- 再生ヘッド -->
  <div class="playhead" style="left:{playheadPx}px"></div>

  <!-- 選択中の小節範囲(チャット指示のマスク) -->
  {#if selection}
    <div
      class="selection-overlay"
      style="left:{HEAD_W + selection.startTick * pxPerTick}px;width:{(selection.endTick -
        selection.startTick) *
        pxPerTick}px"
    ></div>
  {/if}

  <!-- 小節ルーラー(クリックでシーク、ドラッグで範囲選択) -->
  <div class="ruler-row">
    <div class="track-head ruler-head"></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="lane seekable"
      style="width:{totalPx}px"
      onpointerdown={onRulerDown}
      onpointermove={onRulerMove}
      onpointerup={onRulerUp}
    >
      {#each barList as bar (bar.index)}
        <div class="bar-mark" style="left:{bar.tick * pxPerTick}px">
          {bar.index + 1}{#if bar.sigChange}<span class="sig-chip">{bar.num}/{bar.den}</span>{/if}
        </div>
      {/each}
    </div>
  </div>

  {#if project.tracks.length === 0}
    <div class="empty">
      トラックがありません。Claude に「トラックを追加して」と頼んでみてください。
    </div>
  {/if}

  {#each project.tracks as track, ti (track.id)}
    <div class="track-row" class:alt={ti % 2 === 1}>
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="track-head" oncontextmenu={(e) => openTrackMenu(e, track, ti)}>
        <div class="head-row">
          <div class="track-name" style={track.color ? `color:${track.color}` : ""}>
            {track.name}
          </div>
          <button class="ms" class:mute-on={track.mute} onclick={() => toggleMute(track)} title="ミュート">
            M
          </button>
          <button class="ms" class:solo-on={track.solo} onclick={() => toggleSolo(track)} title="ソロ">
            S
          </button>
          <button
            class="ms"
            class:auto-on={autoLanes[track.id] !== undefined}
            onclick={() => toggleAutoLane(track.id)}
            title="オートメーションレーンを開閉"
          >
            〜
          </button>
          <button
            class="ms"
            class:auto-on={soundDesignStore.focus?.trackId === track.id}
            onclick={() =>
              (soundDesignStore.focus =
                soundDesignStore.focus?.trackId === track.id
                  ? null
                  : { trackId: track.id, trackName: track.name })}
            title="音作りビューを開閉(つまみ・エフェクト・プリセット)"
          >
            🎛
          </button>
        </div>
        <div class="head-row">
          <input
            class="vol"
            type="range"
            min="-40"
            max="6"
            step="0.5"
            value={track.volume_db}
            disabled={hasVolumeLane(track)}
            title={hasVolumeLane(track)
              ? "音量オートメーション使用中(フェーダーより優先されます)"
              : ""}
            onchange={(e) => setVolume(track, e)}
          />
          <span class="db">{volumeText(track)}</span>
        </div>
        <div class="track-meta">
          <span class="kind {track.kind}">{track.kind}</span>
          <button
            class="dev"
            onclick={(e) => openDeviceMenu(e, track)}
            title={track.device?.name
              ? "クリックで音源を変更"
              : "音源未設定(既定の subtractive で発音)。クリックで選択"}
          >
            🎹 {track.device?.name ?? "subtractive*"} ▾
          </button>
          <code>{track.id}</code>
        </div>
      </div>
      <div class="lane" style="width:{totalPx}px">
        {#each barList as bar (bar.index)}
          <div class="grid-line" style="left:{bar.tick * pxPerTick}px"></div>
        {/each}
        {#each track.clips as clip (clip.id)}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="clip {clip.kind}"
            style={clipStyle(clip)}
            title={`${clip.name} (${clip.id})${clip.kind === "midi" ? " — ダブルクリックでピアノロール" : ""}`}
            ondblclick={(e) => openPianoRoll(track, clip, e)}
          >
            <span class="clip-name">{clip.name}</span>
            {#if clip.kind === "midi"}
              <ClipPreview {clip} widthPx={clip.length * pxPerTick} />
            {/if}
          </div>
        {/each}
      </div>
    </div>
    {#if autoLanes[track.id]}
      <AutomationLaneRow
        {track}
        target={autoLanes[track.id]}
        {pxPerTick}
        {totalPx}
        onTarget={(t) => (autoLanes = { ...autoLanes, [track.id]: t })}
        onClose={() => toggleAutoLane(track.id)}
      />
    {/if}
  {/each}

  {#if deviceMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={() => (deviceMenu = null)} oncontextmenu={(e) => { e.preventDefault(); deviceMenu = null; }}></div>
    <div class="track-menu" style="left:{deviceMenu.x}px;top:{deviceMenu.y}px">
      {#each INSTRUMENTS as inst (inst.name)}
        <button
          class:active-dev={(deviceMenu.track.device?.name ?? "subtractive") === inst.name}
          onclick={() => setDevice(inst.name)}
        >
          <span class="dev-label">
            {inst.label}{(deviceMenu.track.device?.name ?? "subtractive") === inst.name ? " ✓" : ""}
          </span>
          <span class="dev-desc">{inst.desc}</span>
        </button>
      {/each}
      <div class="menu-note">切り替えると音源パラメータは初期値に戻ります(Ctrl+Z で取り消せます)。細かい音作りは AI に依頼してください。</div>

      <div class="menu-sep"></div>
      <div class="preset-title">プリセット(全プロジェクト共通)</div>
      {#if presets.length === 0}
        <div class="menu-note">まだありません。良い音ができたら下の欄で保存できます。</div>
      {:else}
        {#each presets as p (p.name)}
          <button
            onclick={() => applyPreset(p.name)}
            title={`${p.description ?? ""}\n音源: ${p.instrument}${p.effects.length ? " / FX: " + p.effects.join(" → ") : ""}\n適用すると音源とエフェクトが置き換わります(Ctrl+Z 可)`}
          >
            <span class="dev-label">🎨 {p.name}</span>
            <span class="dev-desc">{p.instrument}{p.effects.length ? ` + ${p.effects.join(", ")}` : ""}</span>
          </button>
        {/each}
      {/if}
      <div class="preset-save">
        <input
          type="text"
          placeholder="今の音を保存(名前)"
          bind:value={presetName}
          onkeydown={(e) => {
            if (e.key === "Enter" && !e.isComposing) {
              e.preventDefault();
              saveCurrentPreset();
            }
          }}
          onclick={(e) => e.stopPropagation()}
        />
        <button class="save-btn" disabled={!presetName.trim()} onclick={saveCurrentPreset}>保存</button>
      </div>
      {#if presetMsg}
        <div class="menu-note">{presetMsg}</div>
      {/if}
    </div>
  {/if}

  {#if trackMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="menu-backdrop" onclick={() => (trackMenu = null)} oncontextmenu={(e) => { e.preventDefault(); trackMenu = null; }}></div>
    <div class="track-menu" style="left:{trackMenu.x}px;top:{trackMenu.y}px">
      <button disabled={trackMenu.index === 0} onclick={() => moveTrack(trackMenu!.index - 1)}>
        ↑ 上へ移動
      </button>
      <button
        disabled={trackMenu.index >= project.tracks.length - 1}
        onclick={() => moveTrack(trackMenu!.index + 1)}
      >
        ↓ 下へ移動
      </button>
      <div class="menu-sep"></div>
      <button class="danger" onclick={deleteTrack} title="Ctrl+Z で元に戻せます">
        🗑 トラックを削除
      </button>
    </div>
  {/if}

  <div class="add-track-row">
    <button class="add-track" onclick={addTrack} title="MIDI トラックを追加(音源は後から AI に頼むか自動で subtractive)">
      + トラックを追加
    </button>
  </div>
</div>

<style>
  .timeline {
    min-width: max-content;
    position: relative;
  }

  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--accent);
    box-shadow: 0 0 4px var(--accent);
    z-index: 3;
    pointer-events: none;
  }

  .selection-overlay {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    border-left: 1px solid var(--accent-dim);
    border-right: 1px solid var(--accent-dim);
    z-index: 1;
    pointer-events: none;
  }

  .lane.seekable {
    cursor: pointer;
  }

  .bar-mark {
    pointer-events: none;
  }

  .ruler-row,
  .track-row {
    display: flex;
    border-bottom: 1px solid var(--border);
  }

  .track-head {
    width: 200px;
    flex-shrink: 0;
    padding: 6px 10px;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    position: sticky;
    left: 0;
    z-index: 2;
  }

  .head-row {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .ms {
    padding: 0 6px;
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
    border-radius: 3px;
  }

  .ms.mute-on {
    background: #6b4030;
    border-color: #8a5a44;
    color: #ffab7a;
  }

  .ms.solo-on {
    background: #6b6130;
    border-color: #8a7d44;
    color: var(--accent);
  }

  .ms.auto-on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .vol {
    flex: 1;
    min-width: 0;
    accent-color: var(--accent);
    height: 14px;
  }

  .db {
    font-size: 10px;
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
    width: 52px;
    text-align: right;
  }

  .ruler-head {
    padding: 0;
    height: 24px;
  }

  .ruler-row .lane {
    height: 24px;
    position: relative;
    background: var(--bg-panel);
  }

  .bar-mark {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid var(--border);
    padding-left: 4px;
    font-size: 11px;
    color: var(--text-dim);
    line-height: 24px;
    white-space: nowrap;
  }

  .sig-chip {
    margin-left: 4px;
    padding: 0 4px;
    border-radius: 3px;
    font-size: 9px;
    background: color-mix(in srgb, var(--accent) 25%, transparent);
    color: var(--accent);
  }

  .track-row {
    height: 72px;
  }

  .track-row .lane {
    position: relative;
    background: var(--bg-lane);
  }

  .track-row.alt .lane {
    background: var(--bg-lane-alt);
  }

  .grid-line {
    position: absolute;
    top: 0;
    bottom: 0;
    border-left: 1px solid rgba(255, 255, 255, 0.05);
  }

  .track-name {
    flex: 1;
    min-width: 0;
    font-weight: 600;
    font-size: 13px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .track-meta {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 10px;
    color: var(--text-dim);
    margin-top: 2px;
    opacity: 0.8;
  }

  .dev {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    border: none;
    background: none;
    padding: 0;
    font-size: 10px;
    color: var(--text-dim);
    cursor: pointer;
  }

  .dev:hover {
    color: var(--accent);
  }

  .dev-label {
    display: block;
    font-size: 12px;
    font-weight: 600;
  }

  .dev-desc {
    display: block;
    font-size: 10px;
    color: var(--text-dim);
  }

  .track-menu button.active-dev {
    border: 1px solid var(--accent-dim);
  }

  .menu-note {
    font-size: 9px;
    color: var(--text-dim);
    padding: 4px 10px 2px;
    max-width: 230px;
    line-height: 1.5;
  }

  .preset-title {
    font-size: 10px;
    color: var(--text-dim);
    padding: 2px 10px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .preset-save {
    display: flex;
    gap: 4px;
    padding: 4px 8px 2px;
  }

  .preset-save input {
    flex: 1;
    min-width: 0;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 3px 6px;
    font-size: 11px;
  }

  .preset-save input:focus {
    outline: none;
    border-color: var(--accent-dim);
  }

  .preset-save .save-btn {
    font-size: 11px;
    padding: 2px 8px;
  }

  .kind {
    padding: 0 5px;
    border-radius: 3px;
    font-size: 10px;
    text-transform: uppercase;
  }

  .kind.midi {
    background: color-mix(in srgb, var(--clip-midi) 30%, transparent);
    color: var(--clip-midi);
  }

  .kind.audio {
    background: color-mix(in srgb, var(--clip-audio) 30%, transparent);
    color: var(--clip-audio);
  }

  .clip {
    position: absolute;
    top: 6px;
    bottom: 6px;
    border-radius: 5px;
    overflow: hidden;
    border: 1px solid rgba(255, 255, 255, 0.25);
    padding: 2px 6px;
  }

  .clip.midi {
    background: color-mix(in srgb, var(--clip-midi) 45%, var(--bg));
  }

  .clip.audio {
    background: color-mix(in srgb, var(--clip-audio) 45%, var(--bg));
  }

  .clip-name {
    font-size: 11px;
    white-space: nowrap;
    position: relative;
    z-index: 1;
  }

  .empty {
    padding: 40px;
    color: var(--text-dim);
  }

  .menu-backdrop {
    position: fixed;
    inset: 0;
    z-index: 19;
  }

  .track-menu {
    position: fixed;
    z-index: 20;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    padding: 5px;
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 160px;
  }

  .track-menu button {
    text-align: left;
    border: none;
    background: none;
    padding: 6px 10px;
    border-radius: 5px;
  }

  .track-menu button:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 15%, transparent);
  }

  .track-menu button.danger:hover {
    background: #5c2b33;
    color: #ffb4c0;
  }

  .menu-sep {
    height: 1px;
    background: var(--border);
    margin: 2px 4px;
  }

  .add-track-row {
    padding: 8px 10px;
    position: sticky;
    left: 0;
    width: 200px;
  }

  .add-track {
    width: 100%;
    color: var(--text-dim);
    border-style: dashed;
  }

  .add-track:hover {
    color: var(--accent);
  }
</style>
