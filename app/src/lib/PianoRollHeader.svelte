<script lang="ts">
  // ピアノロールの見出しの行(ツールバー): 表示 / 道具 / スナップ / クオンタイズ / スウィング / 滑る時間と、
  // 操作のヘルプのポップアップ。値はピアノロールが持ち、ここは選ぶだけ
  import Icon from "./Icon.svelte";
  import { GLIDE_CHOICES, SNAP_OPTIONS, type ArtEntry } from "./pianoRollOps";
  import { pianoRollStore } from "./selection.svelte";
  import type { MidiClip, Project, Track } from "./types";

  let {
    project,
    clip,
    track,
    pane,
    isDrum,
    isFrettable,
    otherClips,
    availableArts,
    fretTuning,
    swingMsg,
    portaCount,
    glideValue,
    showVel = $bindable(),
    showKit = $bindable(),
    showFret = $bindable(),
    curveMode = $bindable(),
    snapTicks = $bindable(),
    swingGrid = $bindable(),
    onTuning,
    onOpenSplit,
    onSwap,
    onQuantize,
    onSwing,
    onGlide,
    onClose,
  }: {
    project: Project;
    clip: MidiClip;
    track: Track;
    pane: "main" | "second";
    isDrum: boolean;
    isFrettable: boolean;
    /// 分割ペインに開ける MIDI クリップ(自分以外)
    otherClips: { track: Track; clip: MidiClip }[];
    /// この楽器で効く奏法(ヘルプに出す)
    availableArts: ArtEntry[];
    fretTuning: "guitar" | "bass";
    swingMsg: string | null;
    /// 選択中のポルタメントのノートの数(あれば「滑る時間」を出す)
    portaCount: number;
    /// 選んだポルタメントの滑る時間(みな同じならその値、ばらばらなら "")
    glideValue: string;
    showVel: boolean;
    showKit: boolean;
    showFret: boolean;
    curveMode: boolean;
    snapTicks: number;
    swingGrid: number;
    onTuning: (tuning: "guitar" | "bass") => void;
    onOpenSplit: (clipId: string) => void;
    onSwap: () => void;
    onQuantize: (strength: number) => void;
    onSwing: (value: string) => void;
    onGlide: (value: string) => void;
    onClose: () => void;
  } = $props();

  /// 操作のヘルプ(以前は 1 行のヒント文に詰め込んでいた)
  let helpOpen = $state(false);
</script>

<!-- ツールバー: 表示 / 道具 / スナップ / スウィング。操作の説明は ? のヘルプに -->
<div class="head">
  <div class="head-left">
    {#if pianoRollStore.second}
      <span class="pane-tag">{pane === "main" ? "上" : "下"}</span>
    {/if}
    <span class="clip-name" title={clip.name}>{clip.name}</span>
    <span class="track-name">{track.name}</span>
    {#if clip.loop && clip.loop_len}
      <span class="loop-tag" title="ループのクリップ: ここで編集した範囲がクリップの長さまで繰り返し鳴ります"
        ><Icon name="infinity" size={14} />{(clip.loop_len / (project.ppq * 4)).toFixed(
          clip.loop_len % (project.ppq * 4) === 0 ? 0 : 2,
        )} 小節を繰り返し</span
      >
    {/if}
  </div>
  <div class="head-right">
    <span class="glabel">表示</span>
    <div class="seg">
      <button class="btn sm" class:on={showVel} onclick={() => (showVel = !showVel)} title="ベロシティ(音の強さ)の帯。縦棒を上下にドラッグで変更、選択中のノートはまとめて変わる"
        ><Icon name="chart-no-axes-column" />ベロシティ</button
      >
      {#if isDrum}
        <button class="btn sm" class:on={showKit} onclick={() => (showKit = !showKit)} title="ドラムキットの図(押すと挿入カーソルの位置に打ち込む)"
          ><Icon name="drum" />キット</button
        >
      {/if}
      {#if isFrettable}
        <button class="btn sm" class:on={showFret} onclick={() => (showFret = !showFret)} title="フレット盤(押すと挿入カーソルの位置に打ち込む)"
          ><Icon name="guitar" />フレット</button
        >
      {/if}
    </div>
    {#if isFrettable && showFret}
      <select
        class="tuning"
        value={fretTuning}
        onchange={(e) => onTuning((e.currentTarget as HTMLSelectElement).value as "guitar" | "bass")}
        title="フレット盤のチューニング"
      >
        <option value="guitar">ギター(6 弦)</option>
        <option value="bass">ベース(4 弦)</option>
      </select>
    {/if}
    {#if pane === "main"}
      <label class="sel-ic" title="別のクリップを下に開いて見比べ・コピペ(Ctrl+C → 下をクリック → Ctrl+V)">
        <Icon name="rows-2" size={14} />
        <select
          class="split"
          value=""
          onchange={(e) => {
            const v = (e.currentTarget as HTMLSelectElement).value;
            if (v) onOpenSplit(v);
            (e.currentTarget as HTMLSelectElement).value = "";
          }}
        >
          <option value="">分割して開く…</option>
          {#each otherClips as o (o.clip.id)}
            <option value={o.clip.id}>{o.track.name} / {o.clip.name}</option>
          {/each}
        </select>
      </label>
    {:else}
      <button class="btn sm" onclick={onSwap} title="上下のクリップを入れ替える"><Icon name="arrow-up-down" />入れ替え</button>
    {/if}
    <span class="sep"></span>
    <span class="glabel">道具</span>
    <button
      class="btn sm"
      class:on={curveMode}
      onclick={() => (curveMode = !curveMode)}
      title="ピッチカーブを手で描く: ノートの上をなぞると、その高さのずれ(1 行 = 半音)がカーブになる。右クリックでカーブを消す"
      ><Icon name="pencil-line" />カーブ</button
    >
    <span class="sep"></span>
    <label class="snap">
      スナップ
      <select bind:value={snapTicks}>
        {#each SNAP_OPTIONS as o (o.ticks)}
          <option value={o.ticks}>{o.label}</option>
        {/each}
      </select>
    </label>
    <label class="snap" title="ノートの開始位置をスナップの格子へ寄せる(選択中のノート、無ければクリップ全体)。長さは変えない。Q キーで 100%、Shift+Q で 50%">
      <select
        value=""
        aria-label="クオンタイズ"
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          if (el.value) onQuantize(Number(el.value));
          el.value = "";
        }}
      >
        <option value="">クオンタイズ…</option>
        <option value="1">格子にそろえる(100%)</option>
        <option value="0.75">75%(少し残す)</option>
        <option value="0.5">50%(人間味を残す)</option>
      </select>
    </label>
    <label class="snap" title="裏拍の音をハネさせる(選択中のノート、無ければクリップ全体)。表の音と長さは変えない。同じ設定なら何度掛けても同じ">
      スウィング
      <select bind:value={swingGrid} aria-label="スウィングの単位">
        <option value={480}>1/8</option>
        <option value={240}>1/16</option>
      </select>
      <select
        value=""
        aria-label="スウィングを掛ける"
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          onSwing(el.value);
          el.value = "";
        }}
      >
        <option value="">掛ける…</option>
        <option value="0.5">ストレート(50%)</option>
        <option value="0.58">軽め(58%)</option>
        <option value="0.62">中くらい(62%)</option>
        <option value="0.6667">3 連シャッフル(67%)</option>
        <option value="0.75">付点(75%)</option>
      </select>
    </label>
    {#if swingMsg}<span class="swing-msg">{swingMsg}</span>{/if}
    {#if portaCount > 0}
      <label class="snap" title="選んだポルタメント(P)のノートが直前の音から滑る時間。トラック全体の既定はインスペクターの「つなぎ」で">
        滑る時間
        <select value={glideValue} onchange={(e) => onGlide((e.currentTarget as HTMLSelectElement).value)}>
          {#if glideValue === ""}<option value="">(ばらばら)</option>{/if}
          <option value="0">トラックの設定</option>
          {#each GLIDE_CHOICES as ms (ms)}
            <option value={String(ms)}>{ms}ms</option>
          {/each}
          {#if glideValue !== "" && glideValue !== "0" && !GLIDE_CHOICES.includes(Number(glideValue))}
            <option value={glideValue}>{glideValue}ms</option>
          {/if}
        </select>
      </label>
    {/if}
    <span class="grow"></span>
    <button class="btn sm icon ghost" class:on={helpOpen} onclick={() => (helpOpen = !helpOpen)} title="操作のヘルプ" aria-label="操作のヘルプ"
      ><Icon name="circle-help" /></button
    >
    <button class="btn sm icon ghost" onclick={onClose} title={pane === "main" ? "閉じる(Esc)" : "この分割ペインを閉じる(Esc)"} aria-label="閉じる"
      ><Icon name="x" /></button
    >
  </div>
</div>
{#if helpOpen}
  <div class="help-pop" role="dialog" aria-label="ピアノロールの操作">
    <div class="help-h">
      <b>ピアノロールの操作</b>
      <button class="btn sm icon ghost" onclick={() => (helpOpen = false)} aria-label="閉じる"><Icon name="x" /></button>
    </div>
    <table>
      <tbody>
        <tr><td>追加</td><td>空きをダブルクリック(長さはスナップの幅)</td></tr>
        <tr><td>選ぶ</td><td>クリック / <kbd>Shift</kbd>+クリックで追加 / 空きをドラッグで囲む / <kbd>Ctrl</kbd>+<kbd>A</kbd></td></tr>
        <tr><td>動かす・長さ</td><td>ドラッグ / 右端をドラッグ / <kbd>Alt</kbd>+<kbd>←</kbd><kbd>→</kbd></td></tr>
        <tr><td>音の高さ</td><td><kbd>↑</kbd><kbd>↓</kbd>(<kbd>Shift</kbd> でオクターブ)</td></tr>
        <tr><td>消す</td><td><kbd>Delete</kbd> / 右クリック</td></tr>
        <tr><td>コピー</td><td><kbd>Ctrl</kbd>+<kbd>C</kbd> <kbd>X</kbd> <kbd>V</kbd>(マウスの位置へ。別のクリップにも)</td></tr>
        <tr><td>挿入カーソル</td><td>空きをクリック / <kbd>←</kbd><kbd>→</kbd>(キット・フレットの打ち込み先)</td></tr>
        <tr>
          <td>奏法</td>
          <td>{#each availableArts as a (a.key)}<span class="art"><kbd>{a.key}</kbd>{a.label}</span>{/each}(選択中のノートに。もう一度で外す)</td>
        </tr>
        <tr><td>固定</td><td><kbd>K</kbd>(選択中のノートを固定する。もう一度で外す。固定の音は AI が変えない。点線の縁と鍵の印)</td></tr>
        <tr><td>強さ</td><td>下の帯の縦棒を上下にドラッグ</td></tr>
        <tr><td>ズーム</td><td><kbd>Ctrl</kbd>+ホイール(横)/ <kbd>Shift</kbd>+ホイール(縦)</td></tr>
        <tr><td>閉じる</td><td><kbd>Esc</kbd>(選択を外してから)</td></tr>
      </tbody>
    </table>
  </div>
{/if}

<style>
  /* 分割時: 非アクティブなペインのヘッダを少し落として、キーがどちらに効くか示す(.overlay はピアノロールのもの) */
  :global(.overlay.inactive) .head {
    opacity: 0.6;
  }

  .pane-tag {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 3px;
    background: color-mix(in srgb, var(--accent) 25%, transparent);
    color: var(--accent);
  }

  .split {
    font-size: 11px;
    max-width: 150px;
  }

  .sel-ic {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-dim);
    flex-shrink: 0;
  }

  .glabel {
    font-size: 10px;
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .seg {
    display: inline-flex;
    gap: 2px;
    flex-shrink: 0;
  }

  .sep {
    width: 1px;
    height: 18px;
    background: var(--border);
    flex-shrink: 0;
  }

  .grow {
    flex: 1;
  }

  .help-pop {
    position: absolute;
    right: 12px;
    top: 44px;
    z-index: 30;
    width: 420px;
    max-width: calc(100% - 24px);
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    padding: 10px 14px 12px;
    font-size: var(--fs-sm);
  }

  .help-h {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
  }

  .help-pop table {
    border-collapse: collapse;
    width: 100%;
  }

  .help-pop td {
    padding: 3px 4px;
    vertical-align: top;
    line-height: 1.6;
  }

  .help-pop td:first-child {
    color: var(--text-dim);
    white-space: nowrap;
    width: 96px;
  }

  .art {
    margin-right: 8px;
    white-space: nowrap;
  }

  kbd {
    font-family: var(--mono);
    font-size: 10px;
    border: 1px solid var(--border-strong);
    border-bottom-width: 2px;
    border-radius: 3px;
    padding: 0 4px;
    margin-right: 2px;
    background: var(--bg-raised);
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 8px 0 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .head-left {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    max-width: 30%;
    white-space: nowrap;
    overflow: hidden;
  }

  .clip-name {
    font-weight: 600;
    font-size: var(--fs-md);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .track-name {
    color: var(--text-dim);
    font-size: 12px;
  }

  /* ヘッダーは 1 行に保つ(ボタン類は折り返さず、説明文だけ省略表示) */
  .loop-tag {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 11px;
    color: var(--accent);
  }

  .head-right {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
    white-space: nowrap;
  }

  .swing-msg {
    font-size: 11px;
    color: var(--accent);
    white-space: nowrap;
  }

  .head-right > button,
  .head-right > select,
  .head-right > .snap {
    flex-shrink: 0;
  }

  .snap {
    font-size: 12px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .snap select {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 4px;
  }

  .tuning {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 4px;
    font-size: 11px;
  }
</style>
