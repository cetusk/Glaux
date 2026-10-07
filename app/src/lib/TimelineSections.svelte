<script lang="ts">
  // タイムラインの上の区間(セクション)の帯: クリックでその区間を範囲選択、ドラッグで移動、
  // ダブルクリックで名前を変更、右クリックで削除
  import { designStore } from "./design.svelte";
  import { tr } from "./i18n.svelte";
  import { barAtTick, type Bar } from "./barMap";
  import { focusSelect, planText, type Marker } from "./timelineOps";
  import { commitSections, markersOf, removeMarker } from "./sectionOps";
  import type { Project } from "./types";

  let {
    project,
    barList,
    pxPerTick,
    totalPx,
    onSelect,
  }: {
    project: Project;
    barList: Bar[];
    pxPerTick: number;
    totalPx: number;
    /// 区間(小節の番号 a〜b)を範囲選択にする
    onSelect: (a: number, b: number) => void;
  } = $props();

  /// 名前を変更中のマーカーの tick
  let renamingMarker = $state<number | null>(null);

  function renameMarker(tick: number, name: string) {
    // Enter の後に入力欄が消えてフォーカスが外れても、Escape で取り消した後でも、2 回目は確定しない
    if (renamingMarker !== tick) return;
    renamingMarker = null;
    const list = markersOf(project);
    const hit = list.find((m) => m.tick === tick);
    const n = name.trim();
    if (!hit || !n || n === hit.name) return;
    const old = hit.name;
    hit.name = n;
    commitSections(list, tr(`マーカーの名前を「${old}」から「${n}」に変更`, `Rename marker "${old}" to "${n}"`));
  }

  /// マーカーのドラッグ(移動)。動かさずに離したら、その区間を範囲選択にする
  let markerDrag = $state<{ tick: number; x0: number; to: number; moved: boolean } | null>(null);

  function laneBar(e: PointerEvent): number {
    const lane = (e.currentTarget as HTMLElement).parentElement!;
    const x = e.clientX - lane.getBoundingClientRect().left;
    return barAtTick(barList, Math.max(0, x / pxPerTick)).index;
  }

  function onMarkerDown(e: PointerEvent, m: Marker) {
    if (e.button !== 0 || renamingMarker !== null) return;
    e.stopPropagation();
    markerDrag = { tick: m.tick, x0: e.clientX, to: m.tick, moved: false };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function onMarkerMove(e: PointerEvent) {
    const d = markerDrag;
    if (!d) return;
    if (!d.moved && Math.abs(e.clientX - d.x0) < 4) return;
    d.moved = true;
    d.to = barList[laneBar(e)].tick;
  }

  function onMarkerUp(e: PointerEvent, m: Marker, next: number | undefined) {
    const d = markerDrag;
    markerDrag = null;
    if (!d) return;
    if (!d.moved) {
      // クリック: この区間(次のマーカーの手前まで)を範囲選択 = AI への指示の対象
      const a = barAtTick(barList, m.tick).index;
      const endTick = next ?? project.tracks.reduce((mx, t) => Math.max(mx, ...t.clips.map((c) => c.start + c.length)), m.tick + 1);
      const b = barAtTick(barList, Math.max(m.tick, endTick - 1)).index;
      onSelect(a, b);
      return;
    }
    if (d.to === m.tick) return;
    const list = markersOf(project);
    if (list.some((x) => x.tick === d.to)) return; // 別のマーカーと重なる所には置かない
    const hit = list.find((x) => x.tick === m.tick);
    if (!hit) return;
    hit.tick = d.to;
    const toBar = barAtTick(barList, d.to).index + 1;
    commitSections(list, tr(`マーカー「${m.name}」を ${toBar} 小節目へ移動`, `Move marker "${m.name}" to bar ${toBar}`));
  }
</script>

{#if project.sections && project.sections.length > 0}
  <div class="section-row">
    <div class="track-head section-head"></div>
    <div class="lane" style="width:{totalPx}px">
      {#each project.sections as sec, i (sec.tick)}
        {@const next = project.sections?.[i + 1]?.tick}
        {@const at = markerDrag?.tick === sec.tick ? markerDrag.to : sec.tick}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="section-band"
          class:dragging={markerDrag?.tick === sec.tick && markerDrag.moved}
          style="left:{at * pxPerTick}px;{next !== undefined && at === sec.tick
            ? `width:${(next - sec.tick) * pxPerTick}px`
            : at === sec.tick
              ? `right:0`
              : `width:120px`}"
          title={tr(
            `${sec.name}(${barAtTick(barList, sec.tick).index + 1} 小節目〜)${planText((sec.id && designStore.sectionDesign[sec.id]) || {})}\nクリックでこの区間を選択 / ドラッグで移動 / ダブルクリックで名前を変更 / 右クリックで削除`,
            `${sec.name} (from bar ${barAtTick(barList, sec.tick).index + 1})${planText((sec.id && designStore.sectionDesign[sec.id]) || {})}\nClick to select this section / drag to move / double-click to rename / right-click to delete`,
          )}
          onpointerdown={(e) => onMarkerDown(e, sec)}
          onpointermove={onMarkerMove}
          onpointerup={(e) => onMarkerUp(e, sec, next)}
          ondblclick={() => (renamingMarker = sec.tick)}
          oncontextmenu={(e) => {
            e.preventDefault();
            removeMarker(project, sec.tick);
          }}
        >
          {#if renamingMarker === sec.tick}
            <input
              class="marker-input"
              value={sec.name}
              use:focusSelect
              onpointerdown={(e) => e.stopPropagation()}
              onkeydown={(e) => {
                if (e.key === "Enter" && !e.isComposing) renameMarker(sec.tick, e.currentTarget.value);
                else if (e.key === "Escape") renamingMarker = null;
              }}
              onblur={(e) => renameMarker(sec.tick, e.currentTarget.value)}
            />
          {:else}
            {sec.name}
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<style>
  /* セクション行は、縦にスクロールしても上端に残す(ルーラーの top はこの行の高さ。Timeline で指定) */
  .section-row {
    display: flex;
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: 0;
    z-index: 7;
  }

  /* 見出しの欄(トラックの見出しと同じ幅・横にスクロールしても左に残す) */
  .section-head {
    width: var(--head-w, 200px);
    flex-shrink: 0;
    padding: 0;
    height: 18px;
    background: var(--bg-panel);
    border-right: 1px solid var(--border);
    position: sticky;
    left: 0;
    z-index: 5;
  }

  .lane {
    height: 18px;
    position: relative;
    background: var(--bg-panel);
  }

  .section-band {
    position: absolute;
    top: 2px;
    bottom: 2px;
    padding: 0 6px;
    font-size: 10px;
    line-height: 14px;
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    border-left: 2px solid var(--accent);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: grab;
    user-select: none;
  }

  .section-band.dragging {
    cursor: grabbing;
    opacity: 0.8;
    z-index: 1;
  }

  .marker-input {
    width: 100%;
    font: inherit;
    font-size: 10px;
    padding: 0 2px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
  }
</style>
