<script lang="ts">
  // クリップの右クリックメニュー: ループ・テンポ追従・パートの分離・似た音・分割・複製・コピー・削除
  import Icon from "./Icon.svelte";
  import TimelineMenu from "./TimelineMenu.svelte";
  import { bpmAt, followBpm, type ClipMenuAction } from "./timelineOps";
  import type { Clip, Project } from "./types";

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
    <div class="menu-note">選択中のクリップ {selectedCount} 個が対象</div>
  {/if}
  {#if clip.kind === "midi"}
    {#if clip.loop && clip.loop_len}
      <button onclick={() => onAction("loop-off")}><Icon name="infinity" />ループを解除</button>
      <button onclick={() => onAction("expand")} title="繰り返しをノートに書き出して、1 回ずつ個別に編集できるようにする"
        ><Icon name="infinity" />繰り返しをノートに展開</button
      >
    {:else}
      <button onclick={() => onAction("loop-on")} title="今の長さを繰り返す。右端を伸ばすと、その分だけ繰り返し鳴る"
        ><Icon name="infinity" />ループにする</button
      >
    {/if}
    <div class="menu-sep"></div>
  {:else}
    {#if followBpm(clip) !== null}
      <button onclick={() => onAction("follow-off")} title={`今は ${followBpm(clip)} BPM の素材として伸縮中`}
        ><Icon name="move-horizontal" />テンポ追従を解除</button
      >
    {:else}
      <button
        class="rich"
        onclick={() => onAction("follow-on")}
        title="テンポを変えても拍がずれないよう、音程を保ったまま伸縮する"
        ><Icon name="move-horizontal" /><span
          >テンポに追従させる<small>{bpmAt(project, clip.start)} BPM で録った素材として</small></span
        ></button
      >
      <button class="rich" onclick={() => onAction("follow-detect")} disabled={detectingTempo}
        ><Icon name="move-horizontal" /><span>テンポに追従させる<small>素材の元のテンポを自動で検出(取り込んだ曲・ループ素材)</small></span
        ></button
      >
    {/if}
    <button class="rich" onclick={() => onAction("sep-builtin")} disabled={separating}
      ><Icon name="layers" /><span>パートに分ける: 打楽器 / 音程楽器<small>内蔵。すぐ終わる</small></span></button
    >
    <button class="rich" onclick={() => onAction("sep-demucs")} disabled={separating}
      ><Icon name="layers" /><span>パートに分ける: ボーカル / ドラム / ベース / その他<small>Demucs(要インストール)。数分かかる</small></span
      ></button
    >
    <button class="rich" onclick={() => onAction("match")} disabled={matching}
      ><Icon name="wand-sparkles" /><span
        >この音に似せたシンセのトラックを作る<small>subtractive / fm / wavetable とリバーブを自動で探す。約 30 秒。単音のサンプル向け</small></span
      ></button
    >
    <button class="rich" onclick={() => onAction("similar")}
      ><Icon name="search" /><span>この音に近い CLAP のプリセットを探す…<small>Surge XT など。読み込んでつまみも自動で詰められる</small></span
      ></button
    >
    <div class="menu-sep"></div>
  {/if}
  <button onclick={() => onAction("split")}><Icon name="scissors" />ここで分割({atLabel})</button>
  <button onclick={() => onAction("split-head")}><Icon name="scissors" />再生ヘッドで分割<span class="key">S</span></button>
  <div class="menu-sep"></div>
  <button onclick={() => onAction("dup")}><Icon name="copy" />複製(直後に並べる)<span class="key">Ctrl+D</span></button>
  <button onclick={() => onAction("copy")}><span class="ic-space"></span>コピー<span class="key">Ctrl+C</span></button>
  <button onclick={() => onAction("cut")}><span class="ic-space"></span>切り取り<span class="key">Ctrl+X</span></button>
  <div class="menu-sep"></div>
  <button class="danger" onclick={() => onAction("delete")}><Icon name="trash-2" />削除<span class="key">Delete</span></button>
  <div class="menu-note">貼り付け(Ctrl+V)は再生ヘッドの位置・元のトラックに置かれます</div>
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
