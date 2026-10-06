<script lang="ts">
  // 「トラックを追加」のメニュー: MIDI・音声・バス・MIDI / MusicXML ファイルから
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";
  import { tr } from "./i18n.svelte";

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
    ><Icon name="piano" /><span
      >{tr("MIDI トラック", "MIDI track")}<small>{tr("ノートを打ち込む・AI に作らせる", "Enter notes or have the AI write them")}</small></span
    ></button
  >
  <button class="rich" onclick={() => onPick("audio")}
    ><Icon name="audio-lines" /><span
      >{tr("音声トラック", "Audio track")}<small
        >{tr("録音・音声ファイルを置く(空きをダブルクリック)", "Record or place audio files (double-click empty space)")}</small
      ></span
    ></button
  >
  <button class="rich" onclick={() => onPick("bus")}
    ><Icon name="merge" /><span
      >{tr("バス(リバーブ入り)", "Bus (with reverb)")}<small>{tr("複数のトラックから送って響きを共有する", "Share reverb by sending from multiple tracks")}</small></span
    ></button
  >
  <div class="menu-sep"></div>
  <button class="rich" onclick={() => onPick("midi-file")}
    ><Icon name="file-music" /><span
      >{tr("MIDI・MusicXML から…", "From MIDI / MusicXML…")}<small
        >{tr(
          "パートごとにトラックを足す。MusicXML は強弱・奏法・パート名も。空の曲ならテンポと拍子も",
          "Adds a track per part. MusicXML also brings dynamics, articulations and part names; tempo and time signature too if the song is empty",
        )}</small
      ></span
    ></button
  >
</TimelineMenu>

<style>
  /* 2 行の項目(見出し + 小さい説明)。アイコン(Icon の span)には掛けない */
  :global(.track-menu) button.rich span {
    display: flex;
    flex-direction: column;
  }
</style>
