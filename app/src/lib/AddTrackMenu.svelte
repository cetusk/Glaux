<script lang="ts">
  // 「トラックを追加」のメニュー: MIDI・音声・バス・MIDI / MusicXML ファイルから
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";

  let {
    x,
    y,
    onPick,
    onClose,
  }: {
    x: number;
    y: number;
    onPick: (kind: "midi" | "audio" | "bus" | "midi-file") => void;
    onClose: () => void;
  } = $props();
</script>

<TimelineMenu {x} {y} {onClose}>
  <button class="rich" onclick={() => onPick("midi")}
    ><Icon name="piano" /><span>MIDI トラック<small>ノートを打ち込む・AI に作らせる</small></span></button
  >
  <button class="rich" onclick={() => onPick("audio")}
    ><Icon name="audio-lines" /><span>音声トラック<small>録音・音声ファイルを置く(空きをダブルクリック)</small></span></button
  >
  <button class="rich" onclick={() => onPick("bus")}
    ><Icon name="merge" /><span>バス(リバーブ入り)<small>複数のトラックから送って響きを共有する</small></span></button
  >
  <div class="menu-sep"></div>
  <button class="rich" onclick={() => onPick("midi-file")}
    ><Icon name="file-music" /><span>MIDI・MusicXML から…<small>パートごとにトラックを足す。MusicXML は強弱・奏法・パート名も。空の曲ならテンポと拍子も</small></span></button
  >
</TimelineMenu>

<style>
  /* 2 行の項目(見出し + 小さい説明)。アイコン(Icon の span)には掛けない */
  :global(.track-menu) button.rich span {
    display: flex;
    flex-direction: column;
  }
</style>
