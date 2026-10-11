<script lang="ts">
  // トラックの右クリック(見出しの ⋯)メニュー: 名前・色・並べ替え・複製・音声にする・プラグインの画面・削除、
  // すべてのトラックへの操作(まとめてミュート・ソロを外す・選ぶ・削除)
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";
  import type { Project } from "./types";
  import type { AllAction } from "./trackActions";
  import { tr } from "./i18n.svelte";

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
    onAll,
    deleteCount = 1,
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
    /// すべてのトラックへの操作
    onAll: (action: AllAction) => void;
    /// 削除で消すトラックの数(選んだトラックの中で開いたら、選んだ全部)
    deleteCount?: number;
  } = $props();

  const TRACK_COLORS = ["#25bdb1", "#5da2e8", "#b07ce8", "#e87ca8", "#e8a07c", "#e8d27c", "#7cc47c", null];

  const menuTrack = $derived(project.tracks[index]);
  const n = $derived(project.tracks.length);
  const anyMuted = $derived(project.tracks.some((t) => t.mute));
  const anyUnmuted = $derived(project.tracks.some((t) => !t.mute));
  const anySolo = $derived(project.tracks.some((t) => t.solo));
</script>

<TimelineMenu {x} {y} {onClose}>
  <button onclick={onRename}><Icon name="pencil" />{tr("名前を変更", "Rename")}</button>
  <div class="color-row" role="group" aria-label={tr("トラックの色", "Track color")}>
    <Icon name="palette" />
    {#each TRACK_COLORS as c (c)}
      <button
        class="color-chip"
        class:none={!c}
        style={c ? `background:${c}` : ""}
        title={c ? tr(`色: ${c}`, `Color: ${c}`) : tr("色を元に戻す", "Reset color")}
        aria-label={c ? tr(`色 ${c}`, `Color ${c}`) : tr("色を元に戻す", "Reset color")}
        onclick={() => onColor(c)}
      ></button>
    {/each}
  </div>
  <div class="menu-sep"></div>
  <button disabled={index === 0} onclick={() => onMove(index - 1)}><Icon name="arrow-up" />{tr("上へ移動", "Move up")}</button>
  <button disabled={index >= project.tracks.length - 1} onclick={() => onMove(index + 1)}
    ><Icon name="arrow-down" />{tr("下へ移動", "Move down")}</button
  >
  <div class="menu-note">
    {tr("見出しの左端をつかんでドラッグしても並べ替えられます", "You can also reorder by dragging the left edge of the header")}
  </div>
  <button onclick={onDuplicate}><Icon name="copy" />{tr("複製", "Duplicate")}</button>
  <button
    onclick={onBounce}
    disabled={bouncing || menuTrack?.kind === "bus"}
    title={tr(
      "エフェクト・音量・パン・送りの響きまで込みで音声に描き出し、直後に音声トラックとして置く(元はミュート)。CLAP の音源の曲をゲームで鳴らすとき・重いトラックを軽くするときに",
      "Render to audio including effects, volume, pan and sends, and place it as an audio track right below (the original is muted). Useful for playing CLAP instruments in games or lightening heavy tracks",
    )}
    ><Icon name="snowflake" />{tr("音声にする(フリーズ)", "Bounce to audio (freeze)")}</button
  >
  {#if menuTrack?.device?.type === "clap"}
    <button onclick={onOpenGui}><Icon name="app-window" />{tr("プラグインの画面を開く", "Open plugin window")}</button>
  {/if}
  <div class="menu-sep"></div>
  <div class="preset-title">{tr(`すべてのトラック(${n})`, `All tracks (${n})`)}</div>
  <button disabled={!anyUnmuted} onclick={() => onAll("mute")}
    ><Icon name="volume-x" />{tr("すべてミュート", "Mute all")}<span class="key">{tr("聴きたいものだけ M で戻す", "then un-mute with M")}</span></button
  >
  <button disabled={!anyMuted} onclick={() => onAll("unmute")}><Icon name="volume-2" />{tr("ミュートをすべて外す", "Unmute all")}</button>
  <button disabled={!anyMuted || !anyUnmuted} onclick={() => onAll("invert")}
    ><Icon name="repeat" />{tr("ミュートを入れ替える", "Invert mutes")}<span class="key">{tr("鳴っている ⇄ 消えている", "audible ⇄ muted")}</span></button
  >
  <button disabled={!anySolo} onclick={() => onAll("unsolo")}><Icon name="headphones" />{tr("ソロをすべて外す", "Clear all solos")}</button>
  <button onclick={() => onAll("select")}
    ><Icon name="list-checks" />{tr("すべて選ぶ", "Select all")}<span class="key">Ctrl+A</span></button
  >
  <button class="danger" onclick={() => onAll("delete")}
    ><Icon name="trash-2" />{tr(`すべて削除(${n})`, `Delete all (${n})`)}<span class="key">{tr("Ctrl+Z で戻せます", "Ctrl+Z to undo")}</span></button
  >
  <div class="menu-sep"></div>
  <button class="danger" onclick={onDelete}><Icon name="trash-2" />{deleteCount > 1 ? tr(`選んだ ${deleteCount} トラックを削除`, `Delete ${deleteCount} selected tracks`) : tr("削除", "Delete")}<span class="key"
      >{tr("Ctrl+Z で戻せます", "Ctrl+Z to undo")}</span
    ></button
  >
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
