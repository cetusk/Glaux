<script lang="ts">
  // ルーラーの右クリック(拍子チップのクリック)で開くメニュー: その小節からの拍子・テンポの変更と、マーカーの追加・名前の変更・削除
  import * as api from "./api";
  import { defaultGrouping, type Bar } from "./barMap";
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";
  import { normalizeSigs, type RulerMenuState } from "./timelineOps";
  import type { Project } from "./types";

  let {
    project,
    barList,
    menu = $bindable(),
    markerName = $bindable(),
    onClose,
    onPutMarker,
    onRemoveMarker,
  }: {
    project: Project;
    barList: Bar[];
    menu: RulerMenuState;
    /// マーカーの名前の入力欄(閉じても残す)
    markerName: string;
    onClose: () => void;
    onPutMarker: (barIndex: number, name: string) => void;
    onRemoveMarker: (tick: number) => void;
  } = $props();

  const DENS = [1, 2, 4, 8, 16, 32];
  const MARKER_PRESETS = ["intro", "Aメロ", "Bメロ", "サビ", "間奏", "outro"];

  /// メニューを開いた小節にあるマーカー
  const markerHere = $derived((project.sections ?? []).find((m) => m.tick === barList[menu.barIndex]?.tick));

  // ---- 曲の途中のテンポ変更(小節の頭から) ----

  function applyTempo() {
    const m = menu;
    const bpm = Math.round(Number(m.bpm) * 100) / 100;
    if (!(bpm >= 20 && bpm <= 300)) return;
    const bar = barList[m.barIndex];
    // 同じ位置の変更を置き換え、直前と同じテンポになる変更は取り除く(先頭は必ず残す)
    const sorted = [...project.tempo_map.filter((e) => e.tick !== bar.tick), { tick: bar.tick, bpm }].sort(
      (a, b) => a.tick - b.tick,
    );
    const events: { tick: number; bpm: number }[] = [];
    for (const e of sorted) {
      if (events.length > 0 && events[events.length - 1].bpm === e.bpm) continue;
      events.push(e);
    }
    if (events.length === 0 || events[0].tick !== 0) events.unshift({ tick: 0, bpm: project.tempo_map[0]?.bpm ?? 120 });
    onClose();
    api
      .applyEdit([{ op: "set_tempo", events }], `${bar.index + 1} 小節目からテンポを ${bpm} に変更`)
      .catch(() => {});
  }

  function removeTempo() {
    const bar = barList[menu.barIndex];
    onClose();
    const events = project.tempo_map.filter((e) => e.tick !== bar.tick);
    api
      .applyEdit([{ op: "set_tempo", events }], `${bar.index + 1} 小節目のテンポ変更を削除`)
      .catch(() => {});
  }

  // ---- 拍子の変更 ----

  function applySig() {
    const m = menu;
    const num = Math.round(Number(m.num));
    const den = Number(m.den);
    if (!(num >= 1 && num <= 32) || !DENS.includes(den)) return;
    const bar = barList[m.barIndex];
    // まとまり("2+2+3")。和が分子と合わないか既定と同じなら持たない
    const parts = m.grouping
      .split(/[+, ]+/)
      .filter((x) => x !== "")
      .map(Number);
    const valid = parts.length > 0 && parts.every((x) => Number.isInteger(x) && x >= 1) && parts.reduce((a, b) => a + b, 0) === num;
    const grouping = valid && parts.join("+") !== defaultGrouping(num, den).join("+") ? parts : undefined;
    const events = normalizeSigs([
      ...project.time_sig_map.filter((e) => e.tick !== bar.tick),
      grouping ? { tick: bar.tick, num, den, grouping } : { tick: bar.tick, num, den },
    ]);
    onClose();
    api
      .applyEdit(
        [{ op: "set_time_sig", events }],
        `${bar.index + 1} 小節目から拍子を ${num}/${den}${grouping ? `(${grouping.join("+")})` : ""} に変更`,
      )
      .catch(() => {});
  }

  function removeSig() {
    const bar = barList[menu.barIndex];
    onClose();
    const events = normalizeSigs(project.time_sig_map.filter((e) => e.tick !== bar.tick));
    api
      .applyEdit([{ op: "set_time_sig", events }], `${bar.index + 1} 小節目の拍子変更を削除`)
      .catch(() => {});
  }
</script>

<TimelineMenu x={menu.x} y={menu.y} extraClass="sig-menu" {onClose}>
  <div class="preset-title">{menu.barIndex + 1} 小節目から拍子を変更</div>
  <div class="sig-form">
    <input
      class="sig-num"
      type="number"
      min="1"
      max="32"
      bind:value={menu.num}
      onkeydown={(e) => e.key === "Enter" && applySig()}
    />
    <span>/</span>
    <select bind:value={menu.den}>
      {#each DENS as d (d)}
        <option value={String(d)}>{d}</option>
      {/each}
    </select>
    <button class="sig-apply" onclick={applySig}>適用</button>
  </div>
  <div class="sig-form">
    <span class="sig-group-label">拍のまとまり</span>
    <input
      class="sig-group"
      type="text"
      placeholder={defaultGrouping(Number(menu.num) || 4, Number(menu.den) || 4).join("+")}
      bind:value={menu.grouping}
      onkeydown={(e) => e.key === "Enter" && applySig()}
      aria-label="拍のまとまり(例 2+2+3)"
    />
  </div>
  <div class="sig-presets">
    {#each ["4/4", "3/4", "6/8", "7/8", "5/4", "12/8", "7/8 3+2+2", "9/8 2+2+2+3"] as p (p)}
      <button
        class="sig-preset"
        onclick={() => {
          const [sig, g] = p.split(" ");
          const [n, d] = sig.split("/");
          menu.num = n;
          menu.den = d;
          menu.grouping = g ?? "";
          applySig();
        }}>{p}</button
      >
    {/each}
  </div>
  {#if menu.barIndex > 0 && project.time_sig_map.some((e) => e.tick === barList[menu.barIndex].tick)}
    <div class="menu-sep"></div>
    <button class="danger" onclick={removeSig}><Icon name="trash-2" />この拍子の変更を削除(前の拍子に戻す)</button>
  {/if}
  <div class="menu-note">ノートの位置は変わらず、この小節から先の小節線だけが変わります(Ctrl+Z で戻せます)</div>
  <div class="menu-sep"></div>
  <div class="preset-title">{menu.barIndex === 0 ? "曲の頭のテンポ" : `${menu.barIndex + 1} 小節目からテンポを変更`}</div>
  <div class="sig-form">
    <input
      class="sig-num tempo-num"
      type="number"
      min="20"
      max="300"
      step="0.5"
      bind:value={menu.bpm}
      onkeydown={(e) => e.key === "Enter" && applyTempo()}
      aria-label="テンポ(BPM)"
    />
    <span>BPM</span>
    <button class="sig-apply" onclick={applyTempo}>適用</button>
  </div>
  {#if menu.barIndex > 0 && project.tempo_map.some((e) => e.tick === barList[menu.barIndex].tick)}
    <button class="danger" onclick={removeTempo}><Icon name="trash-2" />このテンポの変更を削除(前のテンポに戻す)</button>
  {/if}
  <div class="menu-note">ノートは拍の位置のまま、この小節から先の速さが変わります</div>
  <div class="menu-sep"></div>
  <div class="preset-title">
    {markerHere ? `マーカー「${markerHere.name}」` : `${menu.barIndex + 1} 小節目にマーカーを置く`}
  </div>
  <div class="sig-form">
    <input
      class="marker-name"
      placeholder={markerHere ? "新しい名前" : "名前(例: サビ)"}
      bind:value={markerName}
      onkeydown={(e) => e.key === "Enter" && onPutMarker(menu.barIndex, markerName)}
    />
    <button class="sig-apply" onclick={() => onPutMarker(menu.barIndex, markerName)}>
      {markerHere ? "名前を変更" : "追加"}
    </button>
  </div>
  <div class="sig-presets">
    {#each MARKER_PRESETS as name (name)}
      <button class="sig-preset" onclick={() => onPutMarker(menu.barIndex, name)}>{name}</button>
    {/each}
  </div>
  {#if markerHere}
    <button class="danger" onclick={() => onRemoveMarker(markerHere!.tick)}><Icon name="trash-2" />このマーカーを削除</button>
  {/if}
</TimelineMenu>

<style>
  /* 箱(.track-menu.sig-menu)は TimelineMenu のもの。先祖の指定は元の 1 ファイルのときと同じ詳細度にするため残す */

  /* このメニューの削除は警告の色(トラックのメニューの削除より控えめ) */
  :global(.sig-menu) .danger {
    color: var(--warn);
  }

  .sig-form {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 6px;
  }

  .sig-num {
    width: 52px;
  }

  .sig-num.tempo-num {
    width: 64px;
  }

  .sig-group-label {
    font-size: 12px;
    opacity: 0.8;
    white-space: nowrap;
  }

  .sig-group {
    width: 96px;
    margin-left: auto;
  }

  .sig-apply {
    margin-left: auto;
  }

  .sig-presets {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    padding: 2px 6px;
  }

  :global(.track-menu) .sig-preset {
    padding: 2px 8px;
    border: 1px solid var(--border);
    font-size: 12px;
  }

  .marker-name {
    flex: 1;
    min-width: 0;
  }
</style>
