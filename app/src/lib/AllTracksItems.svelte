<script lang="ts">
  // 「すべてのトラック」の操作の項目(まとめてミュート・外す・入れ替える・ソロを外す・選ぶ・削除)。
  // タイムラインのトラックのメニューとミキサーの右クリックで共通に使う
  import Icon from "./Icon.svelte";
  import type { Project } from "./types";
  import type { AllAction } from "./trackActions";
  import { tr } from "./i18n.svelte";

  let {
    project,
    onAll,
    canSelect = true,
  }: {
    project: Project;
    onAll: (action: AllAction) => void;
    /// 「すべて選ぶ」を出すか(トラックを選ぶ仕組みのある画面だけ)
    canSelect?: boolean;
  } = $props();

  const n = $derived(project.tracks.length);
  const anyMuted = $derived(project.tracks.some((t) => t.mute));
  const anyUnmuted = $derived(project.tracks.some((t) => !t.mute));
  const anySolo = $derived(project.tracks.some((t) => t.solo));
</script>

<div class="preset-title">{tr(`すべてのトラック(${n})`, `All tracks (${n})`)}</div>
<button disabled={!anyUnmuted} onclick={() => onAll("mute")}
  ><Icon name="volume-x" />{tr("すべてミュート", "Mute all")}<span class="key">{tr("聴きたいものだけ M で戻す", "then un-mute with M")}</span></button
>
<button disabled={!anyMuted} onclick={() => onAll("unmute")}><Icon name="volume-2" />{tr("ミュートをすべて外す", "Unmute all")}</button>
<button disabled={!anyMuted || !anyUnmuted} onclick={() => onAll("invert")}
  ><Icon name="repeat" />{tr("ミュートを入れ替える", "Invert mutes")}<span class="key">{tr("鳴っている ⇄ 消えている", "audible ⇄ muted")}</span></button
>
<button disabled={!anySolo} onclick={() => onAll("unsolo")}><Icon name="headphones" />{tr("ソロをすべて外す", "Clear all solos")}</button>
{#if canSelect}
  <button onclick={() => onAll("select")}><Icon name="list-checks" />{tr("すべて選ぶ", "Select all")}<span class="key">Ctrl+A</span></button>
{/if}
<button class="danger" disabled={n === 0} onclick={() => onAll("delete")}
  ><Icon name="trash-2" />{tr(`すべて削除(${n})`, `Delete all (${n})`)}<span class="key">{tr("Ctrl+Z で戻せます", "Ctrl+Z to undo")}</span></button
>
