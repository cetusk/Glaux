<script lang="ts">
  // トラックの右クリック(見出しの ⋯)メニュー: 名前・色・並べ替え・複製・音声にする・プラグインの画面・削除
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";
  import type { Project } from "./types";

  let {
    project,
    x,
    y,
    index,
    bouncing,
    onRename,
    onColor,
    onMove,
    onDuplicate,
    onBounce,
    onOpenGui,
    onDelete,
    onClose,
  }: {
    project: Project;
    x: number;
    y: number;
    /// 実際の並びでの位置
    index: number;
    /// 音声に描き出している最中か
    bouncing: boolean;
    onRename: () => void;
    onColor: (color: string | null) => void;
    onMove: (toIndex: number) => void;
    onDuplicate: () => void;
    onBounce: () => void;
    onOpenGui: () => void;
    onDelete: () => void;
    onClose: () => void;
  } = $props();

  const TRACK_COLORS = ["#25bdb1", "#5da2e8", "#b07ce8", "#e87ca8", "#e8a07c", "#e8d27c", "#7cc47c", null];

  const menuTrack = $derived(project.tracks[index]);
</script>

<TimelineMenu {x} {y} {onClose}>
  <button onclick={onRename}><Icon name="pencil" />名前を変更</button>
  <div class="color-row" role="group" aria-label="トラックの色">
    <Icon name="palette" />
    {#each TRACK_COLORS as c (c)}
      <button
        class="color-chip"
        class:none={!c}
        style={c ? `background:${c}` : ""}
        title={c ? `色: ${c}` : "色を元に戻す"}
        aria-label={c ? `色 ${c}` : "色を元に戻す"}
        onclick={() => onColor(c)}
      ></button>
    {/each}
  </div>
  <div class="menu-sep"></div>
  <button disabled={index === 0} onclick={() => onMove(index - 1)}><Icon name="arrow-up" />上へ移動</button>
  <button disabled={index >= project.tracks.length - 1} onclick={() => onMove(index + 1)}
    ><Icon name="arrow-down" />下へ移動</button
  >
  <div class="menu-note">見出しの左端をつかんでドラッグしても並べ替えられます</div>
  <button onclick={onDuplicate}><Icon name="copy" />複製</button>
  <button
    onclick={onBounce}
    disabled={bouncing || menuTrack?.kind === "bus"}
    title="エフェクト・音量・パン・送りの響きまで込みで音声に描き出し、直後に音声トラックとして置く(元はミュート)。CLAP の音源の曲をゲームで鳴らすとき・重いトラックを軽くするときに"
    ><Icon name="snowflake" />音声にする(フリーズ)</button
  >
  {#if menuTrack?.device?.type === "clap"}
    <button onclick={onOpenGui}><Icon name="app-window" />プラグインの画面を開く</button>
  {/if}
  <div class="menu-sep"></div>
  <button class="danger" onclick={onDelete}><Icon name="trash-2" />削除<span class="key">Ctrl+Z で戻せます</span></button>
</TimelineMenu>

<style>
  .color-row {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    color: var(--text-dim);
    --icon-size: 15px;
  }

  .color-row :global(.icon) {
    margin-right: 5px;
  }

  .color-chip {
    width: 16px;
    height: 16px;
    padding: 0;
    border-radius: 50%;
    border: 1px solid var(--border);
  }

  .color-chip.none {
    background: repeating-linear-gradient(45deg, var(--bg), var(--bg) 3px, var(--border) 3px, var(--border) 5px);
  }
</style>
