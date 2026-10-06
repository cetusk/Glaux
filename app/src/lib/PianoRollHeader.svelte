<script lang="ts">
  // ピアノロールの見出しの行(ツールバー): 表示 / 道具 / スナップ / クオンタイズ / スウィング / 滑る時間と、
  // 操作のヘルプのポップアップ。値はピアノロールが持ち、ここは選ぶだけ
  import Icon from "./Icon.svelte";
  import { tr } from "./i18n.svelte";
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
      <span class="pane-tag">{pane === "main" ? tr("上", "Top") : tr("下", "Bottom")}</span>
    {/if}
    <span class="clip-name" title={clip.name}>{clip.name}</span>
    <span class="track-name">{track.name}</span>
    {#if clip.loop && clip.loop_len}
      <span class="loop-tag" title={tr("ループのクリップ: ここで編集した範囲がクリップの長さまで繰り返し鳴ります", "Loop clip: the range edited here repeats up to the clip length")}
        ><Icon name="infinity" size={14} />{(clip.loop_len / (project.ppq * 4)).toFixed(
          clip.loop_len % (project.ppq * 4) === 0 ? 0 : 2,
        )}{tr(" 小節を繰り返し", " bars looped")}</span
      >
    {/if}
  </div>
  <div class="head-right">
    <!-- 道具は幅が足りなければ横にスクロールする(ヘルプと閉じるは右端に固定して、いつも見えるように) -->
    <div class="tools">
    <span class="glabel">{tr("表示", "View")}</span>
    <div class="seg">
      <button class="btn sm" class:on={showVel} onclick={() => (showVel = !showVel)} title={tr("ベロシティ(音の強さ)の帯。縦棒を上下にドラッグで変更、選択中のノートはまとめて変わる", "Velocity (note strength) lane. Drag bars up/down to change; selected notes change together")}
        ><Icon name="chart-no-axes-column" />{tr("ベロシティ", "Velocity")}</button
      >
      {#if isDrum}
        <button class="btn sm" class:on={showKit} onclick={() => (showKit = !showKit)} title={tr("ドラムキットの図(押すと挿入カーソルの位置に打ち込む)", "Drum kit map (click to enter a hit at the insert cursor)")}
          ><Icon name="drum" />{tr("キット", "Kit")}</button
        >
      {/if}
      {#if isFrettable}
        <button class="btn sm" class:on={showFret} onclick={() => (showFret = !showFret)} title={tr("フレット盤(押すと挿入カーソルの位置に打ち込む)", "Fretboard (click to enter a note at the insert cursor)")}
          ><Icon name="guitar" />{tr("フレット", "Fretboard")}</button
        >
      {/if}
    </div>
    {#if isFrettable && showFret}
      <select
        class="tuning"
        value={fretTuning}
        onchange={(e) => onTuning((e.currentTarget as HTMLSelectElement).value as "guitar" | "bass")}
        title={tr("フレット盤のチューニング", "Fretboard tuning")}
      >
        <option value="guitar">{tr("ギター(6 弦)", "Guitar (6-string)")}</option>
        <option value="bass">{tr("ベース(4 弦)", "Bass (4-string)")}</option>
      </select>
    {/if}
    {#if pane === "main"}
      <label class="sel-ic" title={tr("別のクリップを下に開いて見比べ・コピペ(Ctrl+C → 下をクリック → Ctrl+V)", "Open another clip below to compare and copy/paste (Ctrl+C → click below → Ctrl+V)")}>
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
          <option value="">{tr("分割して開く…", "Open split…")}</option>
          {#each otherClips as o (o.clip.id)}
            <option value={o.clip.id}>{o.track.name} / {o.clip.name}</option>
          {/each}
        </select>
      </label>
    {:else}
      <button class="btn sm" onclick={onSwap} title={tr("上下のクリップを入れ替える", "Swap the top and bottom clips")}><Icon name="arrow-up-down" />{tr("入れ替え", "Swap")}</button>
    {/if}
    <span class="sep"></span>
    <span class="glabel">{tr("道具", "Tools")}</span>
    <button
      class="btn sm"
      class:on={curveMode}
      onclick={() => (curveMode = !curveMode)}
      title={tr("ピッチカーブを手で描く: ノートの上をなぞると、その高さのずれ(1 行 = 半音)がカーブになる。右クリックでカーブを消す", "Draw a pitch curve by hand: trace over a note and the pitch offset (1 row = 1 semitone) becomes a curve. Right-click to clear it")}
      ><Icon name="pencil-line" />{tr("カーブ", "Curve")}</button
    >
    <span class="sep"></span>
    <label class="snap">
      {tr("スナップ", "Snap")}
      <select bind:value={snapTicks}>
        {#each SNAP_OPTIONS as o (o.ticks)}
          <option value={o.ticks}>{o.label}</option>
        {/each}
      </select>
    </label>
    <label class="snap" title={tr("ノートの開始位置をスナップの格子へ寄せる(選択中のノート、無ければクリップ全体)。長さは変えない。Q キーで 100%、Shift+Q で 50%", "Move note starts to the snap grid (selected notes, or the whole clip). Lengths are kept. Q for 100%, Shift+Q for 50%")}>
      <select
        value=""
        aria-label={tr("クオンタイズ", "Quantize")}
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          if (el.value) onQuantize(Number(el.value));
          el.value = "";
        }}
      >
        <option value="">{tr("クオンタイズ…", "Quantize…")}</option>
        <option value="1">{tr("格子にそろえる(100%)", "Snap to grid (100%)")}</option>
        <option value="0.75">{tr("75%(少し残す)", "75% (keep a little)")}</option>
        <option value="0.5">{tr("50%(人間味を残す)", "50% (keep the human feel)")}</option>
      </select>
    </label>
    <label class="snap" title={tr("裏拍の音をハネさせる(選択中のノート、無ければクリップ全体)。表の音と長さは変えない。同じ設定なら何度掛けても同じ", "Swing the off-beat notes (selected notes, or the whole clip). On-beat notes and lengths are kept. Applying the same setting again gives the same result")}>
      {tr("スウィング", "Swing")}
      <select bind:value={swingGrid} aria-label={tr("スウィングの単位", "Swing unit")}>
        <option value={480}>1/8</option>
        <option value={240}>1/16</option>
      </select>
      <select
        value=""
        aria-label={tr("スウィングを掛ける", "Apply swing")}
        onchange={(e) => {
          const el = e.currentTarget as HTMLSelectElement;
          onSwing(el.value);
          el.value = "";
        }}
      >
        <option value="">{tr("掛ける…", "Apply…")}</option>
        <option value="0.5">{tr("ストレート(50%)", "Straight (50%)")}</option>
        <option value="0.58">{tr("軽め(58%)", "Light (58%)")}</option>
        <option value="0.62">{tr("中くらい(62%)", "Medium (62%)")}</option>
        <option value="0.6667">{tr("3 連シャッフル(67%)", "Triplet shuffle (67%)")}</option>
        <option value="0.75">{tr("付点(75%)", "Dotted (75%)")}</option>
      </select>
    </label>
    {#if swingMsg}<span class="swing-msg">{swingMsg}</span>{/if}
    {#if portaCount > 0}
      <label class="snap" title={tr("選んだポルタメント(P)のノートが直前の音から滑る時間。トラック全体の既定はインスペクターの「つなぎ」で", "How long the selected portamento (P) notes glide from the previous note. The track default is set in the Inspector's Glide")}>
        {tr("滑る時間", "Glide time")}
        <select value={glideValue} onchange={(e) => onGlide((e.currentTarget as HTMLSelectElement).value)}>
          {#if glideValue === ""}<option value="">{tr("(ばらばら)", "(mixed)")}</option>{/if}
          <option value="0">{tr("トラックの設定", "Track setting")}</option>
          {#each GLIDE_CHOICES as ms (ms)}
            <option value={String(ms)}>{ms}ms</option>
          {/each}
          {#if glideValue !== "" && glideValue !== "0" && !GLIDE_CHOICES.includes(Number(glideValue))}
            <option value={glideValue}>{glideValue}ms</option>
          {/if}
        </select>
      </label>
    {/if}
    </div>
    <div class="head-end">
    <button class="btn sm icon ghost" class:on={helpOpen} onclick={() => (helpOpen = !helpOpen)} title={tr("操作のヘルプ", "Controls help")} aria-label={tr("操作のヘルプ", "Controls help")}
      ><Icon name="circle-help" /></button
    >
    <button class="btn sm icon ghost" onclick={onClose} title={pane === "main" ? tr("閉じる(Esc)", "Close (Esc)") : tr("この分割ペインを閉じる(Esc)", "Close this split pane (Esc)")} aria-label={tr("閉じる", "Close")}
      ><Icon name="x" /></button
    >
    </div>
  </div>
</div>
{#if helpOpen}
  <div class="help-pop" role="dialog" aria-label={tr("ピアノロールの操作", "Piano roll controls")}>
    <div class="help-h">
      <b>{tr("ピアノロールの操作", "Piano roll controls")}</b>
      <button class="btn sm icon ghost" onclick={() => (helpOpen = false)} aria-label={tr("閉じる", "Close")}><Icon name="x" /></button>
    </div>
    <table>
      <tbody>
        <tr><td>{tr("追加", "Add")}</td><td>{tr("空きをダブルクリック(長さはスナップの幅)", "Double-click an empty spot (length = snap width)")}</td></tr>
        <tr><td>{tr("選ぶ", "Select")}</td><td>{tr("クリック / ", "Click / ")}<kbd>Shift</kbd>{tr("+クリックで追加 / 空きをドラッグで囲む / ", "+click to add / drag on empty space to box-select / ")}<kbd>Ctrl</kbd>+<kbd>A</kbd></td></tr>
        <tr><td>{tr("動かす・長さ", "Move / length")}</td><td>{tr("ドラッグ / 右端をドラッグ(Alt を押しながらでスナップなし)/ ", "Drag / drag the right edge (hold Alt for no snap) / ")}<kbd>Alt</kbd>+<kbd>←</kbd><kbd>→</kbd></td></tr>
        <tr><td>{tr("音の高さ", "Pitch")}</td><td><kbd>↑</kbd><kbd>↓</kbd>{tr("(", " (")}<kbd>Shift</kbd>{tr(" でオクターブ)", " for an octave)")}</td></tr>
        <tr><td>{tr("消す", "Delete")}</td><td><kbd>Delete</kbd>{tr(" / 右クリック", " / right-click")}</td></tr>
        <tr><td>{tr("コピー", "Copy")}</td><td><kbd>Ctrl</kbd>+<kbd>C</kbd> <kbd>X</kbd> <kbd>V</kbd>{tr("(マウスの位置へ。別のクリップにも)", " (pastes at the mouse, also into another clip)")}</td></tr>
        <tr><td>{tr("挿入カーソル", "Insert cursor")}</td><td>{tr("空きをクリック / ", "Click an empty spot / ")}<kbd>←</kbd><kbd>→</kbd>{tr("(キット・フレットの打ち込み先)", " (where the kit and fretboard enter notes)")}</td></tr>
        <tr>
          <td>{tr("奏法", "Articulation")}</td>
          <td>{#each availableArts as a (a.key)}<span class="art"><kbd>{a.key}</kbd>{a.label}</span>{/each}{tr("(選択中のノートに。もう一度で外す)", " (on selected notes; press again to remove)")}</td>
        </tr>
        <tr><td>{tr("固定", "Lock")}</td><td><kbd>K</kbd>{tr("(選択中のノートを固定する。もう一度で外す。固定の音は AI が変えない。点線の縁と鍵の印)", " (locks the selected notes; press again to unlock. AI won't change locked notes. Shown with a dotted outline and a lock mark)")}</td></tr>
        <tr><td>{tr("強さ", "Velocity")}</td><td>{tr("下の帯の縦棒を上下にドラッグ", "Drag the bars in the lower lane up/down")}</td></tr>
        <tr><td>{tr("ズーム", "Zoom")}</td><td><kbd>Ctrl</kbd>{tr("+ホイール(横)/ ", "+wheel (horizontal) / ")}<kbd>Shift</kbd>{tr("+ホイール(縦)", "+wheel (vertical)")}</td></tr>
        <tr><td>{tr("閉じる", "Close")}</td><td><kbd>Esc</kbd>{tr("(選択を外してから)", " (after clearing the selection)")}</td></tr>
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
  /* 道具の並び: 縮んで、はみ出す分は横にスクロール(細いスクロールバー) */
  .tools {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: thin;
  }
  /* ヘルプと閉じるは縮めない */
  .head-end {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    margin-left: auto;
  }

  .swing-msg {
    font-size: 11px;
    color: var(--accent);
    white-space: nowrap;
  }

  /* 道具は縮めずに並べ、入りきらない分は横にスクロール */
  .tools > * {
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
