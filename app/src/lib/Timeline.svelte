<script lang="ts">
  import * as api from "./api";
  import ClipPreview from "./ClipPreview.svelte";
  import { newTrackId } from "./ids";
  import { pianoRollStore, selectionStore } from "./selection.svelte";
  import type { Clip, Project, Track } from "./types";

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

  // 1 小節 = ppq * 4 * num / den tick(先頭の拍子で近似。拍子変更対応は後回し)
  const ticksPerBar = $derived.by(() => {
    const sig = project.time_sig_map[0] ?? { num: 4, den: 4 };
    return (project.ppq * 4 * sig.num) / sig.den;
  });

  const PX_PER_BAR = 96;
  const pxPerTick = $derived(PX_PER_BAR / ticksPerBar);

  const endTick = $derived.by(() => {
    let end = 0;
    for (const t of project.tracks) {
      for (const c of t.clips) {
        end = Math.max(end, c.start + c.length);
      }
    }
    return end;
  });

  const bars = $derived(Math.max(16, Math.ceil(endTick / ticksPerBar) + 2));
  const totalPx = $derived(bars * PX_PER_BAR);

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
    return Math.max(0, Math.floor(x / PX_PER_BAR));
  }

  function setRange(a: number, b: number) {
    const startBar = Math.min(a, b);
    const endBar = Math.max(a, b);
    selectionStore.range = {
      startBar,
      endBar,
      startTick: startBar * ticksPerBar,
      endTick: (endBar + 1) * ticksPerBar,
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
      style="left:{HEAD_W + selection.startBar * PX_PER_BAR}px;width:{(selection.endBar -
        selection.startBar +
        1) *
        PX_PER_BAR}px"
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
      {#each Array(bars) as _, i}
        <div class="bar-mark" style="left:{i * PX_PER_BAR}px">{i + 1}</div>
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
      <div class="track-head">
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
        </div>
        <div class="head-row">
          <input
            class="vol"
            type="range"
            min="-40"
            max="6"
            step="0.5"
            value={track.volume_db}
            onchange={(e) => setVolume(track, e)}
          />
          <span class="db">{volumeText(track)}</span>
        </div>
        <div class="track-meta">
          <span class="kind {track.kind}">{track.kind}</span>
          <span class="dev" title={track.device?.name ? "音源" : "音源未設定(既定の subtractive で発音)"}>
            🎹 {track.device?.name ?? "subtractive*"}
          </span>
          <code>{track.id}</code>
        </div>
      </div>
      <div class="lane" style="width:{totalPx}px">
        {#each Array(bars) as _, i}
          <div class="grid-line" style="left:{i * PX_PER_BAR}px"></div>
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
  {/each}

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
