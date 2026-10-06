<script lang="ts">
  // クリップの右クリックメニュー: ループ・テンポ追従・パートの分離・似た音・分割・複製・コピー・削除
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";
  import { bpmAt, followBpm, type ClipMenuAction } from "./timelineOps";
  import type { Clip, Project } from "./types";
  import { tr } from "./i18n.svelte";

  let {
    project,
    x,
    y,
    clip,
    atLabel,
    selectedCount,
    detectingTempo,
    separating,
    matching,
    onAction,
    onClose,
  }: {
    project: Project;
    x: number;
    y: number;
    /// 右クリックしたクリップ
    clip: Clip;
    /// 「ここで分割」の位置の表示(◯ 小節 ◯ 拍)
    atLabel: string;
    selectedCount: number;
    /// 時間のかかる処理の最中か(その項目を押せなくする)
    detectingTempo: boolean;
    separating: boolean;
    matching: boolean;
    onAction: (action: ClipMenuAction) => void;
    onClose: () => void;
  } = $props();
</script>

<TimelineMenu {x} {y} extraClass="clip-menu" {onClose}>
  {#if selectedCount > 1}
    <div class="menu-note">{tr(`選択中のクリップ ${selectedCount} 個が対象`, `Applies to ${selectedCount} selected clips`)}</div>
  {/if}
  {#if clip.kind === "midi"}
    {#if clip.loop && clip.loop_len}
      <button onclick={() => onAction("loop-off")}><Icon name="infinity" />{tr("ループを解除", "Turn off loop")}</button>
      <button onclick={() => onAction("expand")} title={tr(
          "繰り返しをノートに書き出して、1 回ずつ個別に編集できるようにする",
          "Write the repeats out as notes so each pass can be edited separately",
        )}
        ><Icon name="infinity" />{tr("繰り返しをノートに展開", "Expand loop to notes")}</button
      >
    {:else}
      <button onclick={() => onAction("loop-on")} title={tr(
          "今の長さを繰り返す。右端を伸ばすと、その分だけ繰り返し鳴る",
          "Repeat the current length. Drag the right edge to extend the repeats",
        )}
        ><Icon name="infinity" />{tr("ループにする", "Loop")}</button
      >
    {/if}
    <div class="menu-sep"></div>
  {:else}
    {#if followBpm(clip) !== null}
      <button onclick={() => onAction("follow-off")} title={tr(`今は ${followBpm(clip)} BPM の素材として伸縮中`, `Currently stretched as ${followBpm(clip)} BPM material`)}
        ><Icon name="move-horizontal" />{tr("テンポ追従を解除", "Turn off tempo follow")}</button
      >
    {:else}
      <button
        class="rich"
        onclick={() => onAction("follow-on")}
        title={tr(
          "テンポを変えても拍がずれないよう、音程を保ったまま伸縮する",
          "Time-stretch without changing pitch so beats stay aligned when the tempo changes",
        )}
        ><Icon name="move-horizontal" /><span
          >{tr("テンポに追従させる", "Follow tempo")}<small
            >{tr(`${bpmAt(project, clip.start)} BPM で録った素材として`, `As material recorded at ${bpmAt(project, clip.start)} BPM`)}</small
          ></span
        ></button
      >
      <button class="rich" onclick={() => onAction("follow-detect")} disabled={detectingTempo}
        ><Icon name="move-horizontal" /><span
          >{tr("テンポに追従させる", "Follow tempo")}<small
            >{tr(
              "素材の元のテンポを自動で検出(取り込んだ曲・ループ素材)",
              "Detect the original tempo automatically (imported songs, loops)",
            )}</small
          ></span
        ></button
      >
    {/if}
    <button class="rich" onclick={() => onAction("sep-builtin")} disabled={separating}
      ><Icon name="layers" /><span
        >{tr("パートに分ける: 打楽器 / 音程楽器", "Separate: drums / pitched")}<small>{tr("内蔵。すぐ終わる", "Built-in. Quick")}</small></span
      ></button
    >
    <button class="rich" onclick={() => onAction("sep-demucs")} disabled={separating}
      ><Icon name="layers" /><span
        >{tr("パートに分ける: ボーカル / ドラム / ベース / その他", "Separate: vocals / drums / bass / other")}<small
          >{tr("Demucs(要インストール)。数分かかる", "Demucs (install required). Takes a few minutes")}</small
        ></span
      ></button
    >
    <button class="rich" onclick={() => onAction("match")} disabled={matching}
      ><Icon name="wand-sparkles" /><span
        >{tr("この音に似せたシンセのトラックを作る", "Create a synth track matching this sound")}<small
          >{tr(
            "subtractive / fm / wavetable とリバーブを自動で探す。約 30 秒。単音のサンプル向け",
            "Searches subtractive / fm / wavetable and reverb automatically. About 30 s. For single-note samples",
          )}</small
        ></span
      ></button
    >
    <button class="rich" onclick={() => onAction("similar")}
      ><Icon name="search" /><span
        >{tr("この音に近い CLAP のプリセットを探す…", "Find similar CLAP presets…")}<small
          >{tr("Surge XT など。読み込んでつまみも自動で詰められる", "Surge XT etc. Load one and fine-tune its knobs automatically")}</small
        ></span
      ></button
    >
    <div class="menu-sep"></div>
  {/if}
  <button onclick={() => onAction("split")}><Icon name="scissors" />{tr(`ここで分割(${atLabel})`, `Split here (${atLabel})`)}</button>
  <button onclick={() => onAction("split-head")}><Icon name="scissors" />{tr("再生ヘッドで分割", "Split at playhead")}<span class="key">S</span></button>
  <div class="menu-sep"></div>
  <button onclick={() => onAction("dup")}><Icon name="copy" />{tr("複製(直後に並べる)", "Duplicate (place after)")}<span class="key">Ctrl+D</span></button>
  <button onclick={() => onAction("copy")}><span class="ic-space"></span>{tr("コピー", "Copy")}<span class="key">Ctrl+C</span></button>
  <button onclick={() => onAction("cut")}><span class="ic-space"></span>{tr("切り取り", "Cut")}<span class="key">Ctrl+X</span></button>
  <div class="menu-sep"></div>
  <button class="danger" onclick={() => onAction("delete")}><Icon name="trash-2" />{tr("削除", "Delete")}<span class="key">Delete</span></button>
  <div class="menu-note">
    {tr(
      "貼り付け(Ctrl+V)は再生ヘッドの位置・元のトラックに置かれます",
      "Paste (Ctrl+V) places clips at the playhead on their original tracks",
    )}
  </div>
</TimelineMenu>

<style>
  /* 2 行の項目(見出し + 小さい説明)。アイコン(Icon の span)には掛けない */
  :global(.track-menu) button.rich span {
    display: flex;
    flex-direction: column;
  }

  /* このメニューの削除は警告の色(トラックのメニューの削除より控えめ)。箱(.clip-menu)は TimelineMenu のもの */
  :global(.clip-menu) .danger {
    color: var(--warn);
  }
</style>
