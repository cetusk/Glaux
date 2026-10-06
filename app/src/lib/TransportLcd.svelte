<script lang="ts">
  // ヘッダーの表示窓: 位置・時間・録音中の入力レベル・テンポ・拍子・キー(推定)。
  // 押せるのはテンポと拍子だけ(▾ の付いた欄。先頭のテンポ・拍子をその場で書き換える)
  import * as api from "./api";
  import { buildBars, fmtBpm, formatPosition, formatSeconds, tickToSeconds } from "./barMap";
  import { harmonyStore } from "./harmony.svelte";
  import { tr } from "./i18n.svelte";
  import { transportStore } from "./transport.svelte";
  import type { Project } from "./types";

  let {
    project,
    contentEndTick,
    midiArmName,
    onError,
  }: {
    project: Project | null;
    /// コンテンツ終端を小節単位に切り上げた tick
    contentEndTick: number;
    /// MIDI 録音の先(アーム中のトラック名)
    midiArmName: string | null;
    onError: (message: string) => void;
  } = $props();

  const transport = $derived(transportStore.state);

  // 録音中の入力レベル(上がるときは即座に、下がるときはゆっくり)
  let recLevel = $state(-90);
  $effect(() => {
    const db = transport.input_peak_db;
    if (!transport.recording) {
      recLevel = -90;
      return;
    }
    recLevel = Math.max(db ?? -90, recLevel - 6);
  });

  // ---- テンポの編集(先頭のテンポの書き換え。途中の変更は保持) ----
  let editingBpm = $state(false);
  let bpmInput = $state("");

  function startBpmEdit() {
    bpmInput = String(bpm);
    editingBpm = true;
  }

  async function commitBpm() {
    editingBpm = false;
    if (!project) return;
    const v = Number(bpmInput);
    if (!Number.isFinite(v)) return;
    const clamped = Math.min(300, Math.max(20, v));
    if (Math.abs(clamped - bpm) < 1e-9) return;
    const events = project.tempo_map.map((e, i) =>
      i === 0 ? { ...e, bpm: clamped } : e,
    );
    try {
      await api.applyEdit(
        [{ op: "set_tempo", events }],
        `BPM を ${clamped} に変更`,
      );
    } catch (e) {
      onError(String(e));
    }
  }

  function onBpmKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commitBpm();
    } else if (e.key === "Escape") {
      editingBpm = false;
    }
  }

  // ---- 拍子の編集(先頭イベントの書き換え。途中の変更イベントは保持) ----

  let editingSig = $state(false);
  let sigNumInput = $state(4);
  let sigDenInput = $state(4);

  function startSigEdit() {
    if (!project) return;
    const first = project.time_sig_map[0] ?? { tick: 0, num: 4, den: 4 };
    sigNumInput = first.num;
    sigDenInput = first.den;
    editingSig = true;
  }

  async function commitSig() {
    editingSig = false;
    if (!project) return;
    const num = Math.min(32, Math.max(1, Math.round(Number(sigNumInput)) || 0));
    const den = Number(sigDenInput);
    if (num < 1 || ![1, 2, 4, 8, 16, 32].includes(den)) return;
    const cur = project.time_sig_map;
    const first = cur[0] ?? { tick: 0, num: 4, den: 4 };
    if (first.num === num && first.den === den) return;
    // 分子・分母を変えたら拍のまとまりは既定に戻す(和が合わなくなるため)
    const events =
      cur.length > 0
        ? cur.map((e, i) => (i === 0 ? { tick: e.tick, num, den } : e))
        : [{ tick: 0, num, den }];
    try {
      await api.applyEdit(
        [{ op: "set_time_sig", events }],
        `拍子を ${num}/${den} に変更`,
      );
    } catch (e) {
      onError(String(e));
    }
  }

  function onSigKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      commitSig();
    } else if (e.key === "Escape") {
      editingSig = false;
    }
  }

  // ---- 表示(位置・時間) ----
  const posBars = $derived.by(() => {
    if (!project) return [];
    return buildBars(project, Math.max(contentEndTick, transport.tick) + 1, 1, 2);
  });
  const posText = $derived(project ? formatPosition(posBars, transport.tick, project.ppq) : "1.1.1");
  const timeText = $derived(
    project ? formatSeconds(tickToSeconds(project.tempo_map, transport.tick, project.ppq)) : "0:00.0",
  );

  const bpm = $derived(project?.tempo_map[0]?.bpm ?? 120);
  const timeSig = $derived(
    project ? `${project.time_sig_map[0]?.num ?? 4}/${project.time_sig_map[0]?.den ?? 4}` : "4/4",
  );
  const hasSigChanges = $derived((project?.time_sig_map.length ?? 0) > 1);
</script>

<!-- 表示窓: 押せるのはテンポと拍子だけ(▾ の付いた欄) -->
<div class="lcd" class:recording={transport.recording}>
  <div class="f pos" title={tr("位置(小節.拍.16 分)", "Position (bar.beat.16th)")}>
    <span class="lbl">{tr("位置", "Position")}</span><span class="val">{posText}</span>
  </div>
  <div class="f time" title={tr("時間(分:秒)", "Time (min:sec)")}>
    <span class="lbl">{tr("時間", "Time")}</span><span class="val">{timeText}</span>
  </div>
  {#if transport.recording && !transport.midi_recording}
    <div class="f" title={tr(`入力レベル ${recLevel.toFixed(0)} dBFS(目安: -12〜-6dB)`, `Input level ${recLevel.toFixed(0)} dBFS (aim for -12 to -6 dB)`)}>
      <span class="lbl rec-lbl">{tr("録音中 · 入力", "Rec · Input")}</span>
      <span class="rec-meter"
        ><span
          class="rec-meter-fill"
          class:hot={recLevel > -3}
          style="width:{Math.max(0, Math.min(100, ((recLevel + 60) / 60) * 100))}%"
        ></span></span
      >
    </div>
  {:else if transport.midi_recording}
    <div class="f"><span class="lbl rec-lbl">{tr("MIDI 録音中", "MIDI rec")}</span><span class="val">{midiArmName ?? ""}</span></div>
  {/if}
  {#if editingBpm}
    <div class="f">
      <span class="lbl">{tr("テンポ", "Tempo")}</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="lcd-input"
        type="number"
        min="20"
        max="300"
        step="0.5"
        autofocus
        bind:value={bpmInput}
        onkeydown={onBpmKeydown}
        onblur={commitBpm}
      />
    </div>
  {:else}
    <button
      class="f edit"
      onclick={startBpmEdit}
      title={(project?.tempo_map.length ?? 0) > 1
        ? tr(
            "クリックで先頭のテンポ(BPM)を編集(曲の途中にテンポの変更あり。途中の変更はルーラーの右クリックで)",
            "Click to edit the starting tempo (BPM) (the song has tempo changes; right-click the ruler to edit those)",
          )
        : tr(
            "クリックでテンポ(BPM)を編集(曲の途中から変えるときはルーラーを右クリック)",
            "Click to edit the tempo (BPM) (right-click the ruler to change it mid-song)",
          )}
    >
      <span class="lbl">{tr("テンポ", "Tempo")}</span><span class="val">{fmtBpm(bpm)}{#if (project?.tempo_map.length ?? 0) > 1}*{/if}</span>
    </button>
  {/if}
  {#if editingSig}
    <div class="f">
      <span class="lbl">{tr("拍子", "Time sig")}</span>
      <span class="sig-edit">
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="lcd-input sig-input"
          type="number"
          min="1"
          max="32"
          autofocus
          bind:value={sigNumInput}
          onkeydown={onSigKeydown}
          onblur={(e) => {
            // 分母セレクトへの移動では確定しない
            const to = e.relatedTarget as HTMLElement | null;
            if (!to || !to.classList.contains("sig-den")) commitSig();
          }}
        />/<select class="sig-den" bind:value={sigDenInput} onkeydown={onSigKeydown} onchange={commitSig} onblur={commitSig}>
          {#each [2, 4, 8, 16] as d (d)}
            <option value={d}>{d}</option>
          {/each}
        </select>
      </span>
    </div>
  {:else}
    <button
      class="f edit"
      onclick={startSigEdit}
      title={hasSigChanges
        ? tr(
            "クリックで先頭の拍子を編集(曲の途中に拍子の変更あり。途中の変更はルーラーの右クリックで)",
            "Click to edit the starting time signature (the song has meter changes; right-click the ruler to edit those)",
          )
        : tr(
            "クリックで拍子を編集(曲の途中から変えるときはルーラーを右クリック)",
            "Click to edit the time signature (right-click the ruler to change it mid-song)",
          )}
    >
      <span class="lbl">{tr("拍子", "Time sig")}</span><span class="val">{timeSig}{#if hasSigChanges}*{/if}</span>
    </button>
  {/if}
  {#if harmonyStore.view?.key}
    <div
      class="f key"
      title={tr(
        `キー(ノートからの推定。確からしさ ${Math.round(harmonyStore.view.key.confidence * 100)}%)。ルーラーに小節ごとのコード`,
        `Key (estimated from notes, ${Math.round(harmonyStore.view.key.confidence * 100)}% confidence). Chords per bar are shown on the ruler`,
      )}
    >
      <span class="lbl">{tr("キー(推定)", "Key (est.)")}</span><span class="val">{harmonyStore.view.key.name}</span>
    </div>
  {/if}
</div>

<style>
  /* 表示窓 */
  .lcd {
    display: flex;
    align-items: stretch;
    height: 36px;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: var(--r-md);
    overflow: hidden;
  }

  .lcd .f {
    display: flex;
    flex-direction: column;
    justify-content: center;
    align-items: flex-start;
    padding: 0 10px;
    border: 0;
    border-left: 1px solid var(--border);
    border-radius: 0;
    background: none;
    color: var(--text);
    min-width: 0;
    height: auto;
  }

  .lcd .f:first-child {
    border-left: 0;
  }

  .lcd .lbl {
    font-size: 9px;
    line-height: 1;
    color: var(--text-faint);
    letter-spacing: 0.04em;
  }

  .lcd .val {
    font-family: var(--mono);
    font-size: 14px;
    line-height: 1.25;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .lcd .pos .val {
    color: var(--accent);
    font-size: 16px;
    min-width: 5.5ch;
  }

  .lcd.recording .pos .val {
    color: var(--danger);
  }

  /* とても狭い画面では時間をしまう(位置・テンポ・拍子を残す) */
  @media (max-width: 640px) {
    .lcd .time {
      display: none;
    }
  }

  .lcd .key .val {
    font-family: inherit;
    font-size: var(--fs-md);
  }

  .lcd .edit {
    cursor: pointer;
  }

  .lcd .edit:hover {
    background: var(--bg-raised);
    color: var(--text);
  }

  .lcd .edit .val::after {
    content: " ▾";
    font-size: 9px;
    color: var(--text-faint);
  }

  .lcd .rec-lbl {
    color: var(--danger);
  }

  .lcd-input {
    width: 64px;
    height: 20px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    padding: 0 4px;
    font-family: var(--mono);
    font-size: var(--fs-md);
  }

  .sig-edit {
    display: flex;
    align-items: center;
    gap: 2px;
    color: var(--text-dim);
  }

  .sig-input {
    width: 40px;
  }

  .sig-den {
    height: 20px;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--accent-dim);
    border-radius: var(--r-sm);
    font-size: var(--fs-md);
  }

  .rec-meter {
    display: inline-block;
    position: relative;
    width: 70px;
    height: 6px;
    margin-top: 4px;
    border-radius: 3px;
    background: var(--bg-raised);
    overflow: hidden;
  }

  .rec-meter-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--ok);
  }

  .rec-meter-fill.hot {
    background: var(--danger);
  }

  /* 狭い画面(ノート PC の 150% 表示など)ではキーを隠す(ヘッダーの「書き出し」の文字と同じ幅で) */
  @media (max-width: 1280px) {
    .lcd .key {
      display: none;
    }
  }
</style>
