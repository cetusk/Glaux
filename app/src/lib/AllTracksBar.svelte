<script lang="ts">
  // すべてのトラックをまとめて操作するボタン(トラックごとの M・S と同じ見た目)。
  // M = すべてミュート(全部ミュート中なら、すべて外す)/ S = ソロをすべて外す / ⇄ = ミュートを入れ替える / ごみ箱 = すべて削除。
  // タイムライン(トラックを追加の下)とミキサー(左端の列)で使う。右クリックのメニューと同じ操作
  import Icon from "./Icon.svelte";
  import type { Project } from "./types";
  import { setAllListen } from "./trackActions";
  import { tr } from "./i18n.svelte";

  let {
    project,
    vertical = false,
    onDeleteAll,
  }: {
    project: Project;
    /// 縦に並べる(ミキサーの列)
    vertical?: boolean;
    onDeleteAll: () => void;
  } = $props();

  const n = $derived(project.tracks.length);
  const allMuted = $derived(n > 0 && project.tracks.every((t) => t.mute));
  const anyMuted = $derived(project.tracks.some((t) => t.mute));
  const anySolo = $derived(project.tracks.some((t) => t.solo));
</script>

<div class="all-bar" class:vertical role="group" aria-label={tr("すべてのトラック", "All tracks")}>
  <span class="lbl" title={tr(`すべてのトラック(${n})をまとめて`, `All ${n} tracks at once`)}>{tr("すべて", "All")}</span>
  <button
    class="btn letter"
    class:m-on={allMuted}
    aria-pressed={allMuted}
    disabled={n === 0}
    onclick={() => setAllListen(project.tracks, allMuted ? "unmute" : "mute")}
    title={allMuted
      ? tr("ミュートをすべて外す", "Unmute all tracks")
      : tr("すべてミュート(聴きたいトラックだけ M で戻す)", "Mute all tracks (then un-mute the ones you want with M)")}
    aria-label={allMuted ? tr("ミュートをすべて外す", "Unmute all") : tr("すべてミュート", "Mute all")}>M</button
  >
  <button
    class="btn letter"
    class:s-on={anySolo}
    disabled={!anySolo}
    onclick={() => setAllListen(project.tracks, "unsolo")}
    title={tr("ソロをすべて外す", "Clear all solos")}
    aria-label={tr("ソロをすべて外す", "Clear all solos")}>S</button
  >
  <button
    class="btn letter"
    disabled={!anyMuted || allMuted}
    onclick={() => setAllListen(project.tracks, "invert")}
    title={tr("ミュートを入れ替える(鳴っている ⇄ 消えている)", "Invert mutes (audible ⇄ muted)")}
    aria-label={tr("ミュートを入れ替える", "Invert mutes")}><Icon name="repeat" size={12} /></button
  >
  <button
    class="btn letter danger"
    disabled={n === 0}
    onclick={onDeleteAll}
    title={tr(`すべてのトラック(${n})を削除(Ctrl+Z で戻せます)`, `Delete all ${n} tracks (Ctrl+Z to undo)`)}
    aria-label={tr("すべて削除", "Delete all")}><Icon name="trash-2" size={12} /></button
  >
</div>

<style>
  .all-bar {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .all-bar.vertical {
    flex-direction: column;
  }

  .lbl {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    white-space: nowrap;
  }

  .all-bar:not(.vertical) .lbl {
    margin-right: 2px;
  }

  .all-bar.vertical .lbl {
    margin-bottom: 2px;
  }

  .letter {
    width: 22px;
    height: 20px;
    padding: 0;
    font-size: 10px;
    font-weight: 700;
    border-radius: var(--r-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .letter.m-on {
    background: var(--mute);
    border-color: var(--mute);
    color: var(--text-on-color);
  }

  .letter.s-on {
    background: var(--solo);
    border-color: var(--solo);
    color: var(--text-on-color);
  }

  .danger:not(:disabled):hover {
    color: var(--danger);
    border-color: var(--danger);
  }
</style>
