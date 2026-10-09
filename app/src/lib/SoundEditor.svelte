<script lang="ts">
  // 音色エディタ: インスペクターの「音色を作り込む」から開く広いパネル(ピアノロールと同じく、タイムラインの所を覆う)。
  // 共通の枠(見出し・作り込む前と今の聴き比べ・取り消し・AI に頼む・試しに鳴らす鍵盤・曲の再生)はここで、
  // 楽器ごとの中身(左・真ん中・右の列)は soundEditor/*.ts が組み立てる。試作(docs の音色エディタの試作 66 版)のまま
  import "./soundEditor/editor.css";
  import "./soundEditor/editors";
  import { onDestroy, onMount, untrack } from "svelte";
  import * as api from "./api";
  import { buildBars, formatPosition } from "./barMap";
  import { tr } from "./i18n.svelte";
  import { soundDesignStore, soundEditorStore } from "./selection.svelte";
  import { transportStore } from "./transport.svelte";
  import type { EntrySummary, Project } from "./types";
  import {
    afterPointerUp,
    cancelDrag,
    close,
    dragEnd,
    dragStart,
    ed,
    editorKindOf,
    endDrag,
    keyListeners,
    listen,
    mountBody,
    noteHistory,
    noteOff,
    noteOn,
    onProjectChanged,
    openTrack,
    redo,
    revertAll,
    setAB,
    state as seState,
    alignPaneCanvases,
    draw,
    syncPreview,
    ui,
    undo,
    view,
  } from "./soundEditor/core.svelte";
  import { drawSpectrum } from "./soundEditor/parts";
  import { setRoot } from "./soundEditor/dom";

  let { project, entries }: { project: Project; entries: EntrySummary[] } = $props();

  let root = $state<HTMLElement | undefined>();
  let bodyEl = $state<HTMLElement | undefined>();
  let helpOpen = $state(false);

  /** 作り込めるトラック(内蔵の音源のうち、エディタがある物) */
  const tracks = $derived(project.tracks.filter((t) => editorKindOf(t)));

  $effect(() => {
    setRoot(root ?? null);
    mountBody(bodyEl ?? null, root ?? null);
  });

  // 開くトラック(インスペクターの「音色を作り込む」・見出しのトラックの選択)
  $effect(() => {
    const id = soundEditorStore.trackId;
    if (!id || !bodyEl) return;
    untrack(() => {
      if (id !== seState.track || !view.open) openTrack(project, id);
    });
  });
  // 曲が変わった(AI・取り消し・ほかの画面): 音色の値を読み直す
  $effect(() => {
    const p = project;
    untrack(() => onProjectChanged(p));
  });
  // 履歴: このトラックの音色の変更(AI など)を取り消しの並びに積む
  $effect(() => {
    const e = entries;
    untrack(() => noteHistory(e));
  });
  // 選んでいる所はチャットに添える
  $effect(() => {
    soundEditorStore.sel = view.sel;
    soundEditorStore.trackName = view.open ? seState.trackName : "";
  });
  $effect(() => {
    void listen.solo;
    untrack(() => syncPreview());
  });

  // エディタを開いている間はインスペクターを隠す(つまみはエディタの右の列にある。真ん中の絵に横幅を譲る)。閉じたら同じトラックで戻す
  let hadInspector = false;
  onMount(() => {
    hadInspector = soundDesignStore.focus != null;
    soundDesignStore.focus = null;
  });
  function closeEditor() {
    const id = seState.track || soundEditorStore.trackId;
    close();
    soundEditorStore.trackId = null;
    soundEditorStore.sel = null;
    const t = project.tracks.find((x) => x.id === id);
    if (hadInspector && t) soundDesignStore.focus = { trackId: t.id, trackName: t.name };
  }

  function pickTrack(id: string) {
    soundEditorStore.trackId = id;
  }

  function askAi() {
    window.dispatchEvent(new CustomEvent("glaux:focus-chat"));
  }

  // ---- 試しに鳴らす鍵盤(C3〜C6。押している間鳴る。押したまま横へなぞると音を移る) ----
  const KEYS = Array.from({ length: 37 }, (_, i) => 48 + i);
  const BLACK = new Set([1, 3, 6, 8, 10]);
  const whiteX = (m: number) => KEYS.filter((k) => k < m && !BLACK.has(k % 12)).length * 22;
  let lit = $state<Record<number, boolean>>({});
  let down: number | null = null;
  const onKeyLight = (m: number, on: boolean) => {
    lit[m] = on;
  };
  keyListeners.add(onKeyLight);
  function kbdDown(e: PointerEvent) {
    const m = Number((e.target as HTMLElement).closest<HTMLElement>("[data-m]")?.dataset.m);
    if (!m) return;
    down = m;
    noteOn(m);
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function kbdMove(e: PointerEvent) {
    if (down == null) return;
    const t = document.elementFromPoint(e.clientX, e.clientY)?.closest<HTMLElement>("[data-m]");
    const m = t ? Number(t.dataset.m) : null;
    if (m && m !== down) {
      noteOff(down);
      down = m;
      noteOn(m);
    }
  }
  function kbdUp() {
    if (down != null) noteOff(down);
    down = null;
  }

  // ---- キー操作(テキスト入力・選択欄の中では無視) ----
  const KEYMAP: Record<string, number> = { KeyA: 60, KeyW: 61, KeyS: 62, KeyE: 63, KeyD: 64, KeyF: 65, KeyT: 66, KeyG: 67, KeyY: 68, KeyH: 69, KeyU: 70, KeyJ: 71, KeyK: 72 };
  function onKeydown(e: KeyboardEvent) {
    if (!view.open) return;
    const t = e.target as HTMLElement | null;
    if (t?.closest?.("input[type=text], input:not([type]), textarea, select, [contenteditable]")) return;
    // チャットなど、エディタの外で打っている文字は横取りしない
    if (t && t !== document.body && root && !root.contains(t)) return;
    const mod = e.ctrlKey || e.metaKey;
    // ドラッグ中は、そのドラッグをやめて元に戻す
    if (((mod && e.code === "KeyZ" && !e.shiftKey) || e.code === "Escape") && cancelDrag()) return stop(e);
    // ドラッグ中のやり直しは受け付けない
    if (seStateDragging() && mod && (e.code === "KeyZ" || e.code === "KeyY")) return stop(e);
    if (mod && e.code === "KeyZ") {
      stop(e);
      if (e.shiftKey) redo();
      else undo();
      return;
    }
    if (mod && e.code === "KeyY") {
      stop(e);
      redo();
      return;
    }
    if (e.code === "Escape") {
      stop(e);
      closeEditor();
      return;
    }
    // 選んだ物を消す(サンプラーの切れ目)
    if ((e.code === "Delete" || e.code === "Backspace") && ed()?.onDelete?.()) return stop(e);
    const m = KEYMAP[e.code];
    if (m && !mod && !e.altKey) {
      stop(e);
      if (!e.repeat) noteOn(m);
    }
  }
  function onKeyup(e: KeyboardEvent) {
    const m = KEYMAP[e.code];
    if (m) noteOff(m);
  }
  const stop = (e: Event) => {
    e.preventDefault();
    e.stopImmediatePropagation();
  };
  const seStateDragging = () => ui.drag != null;

  // ---- 曲の再生(アプリの再生と同じ。区間のループ・このトラックだけ) ----
  const transport = $derived(transportStore.state);
  const bars = $derived(buildBars(project, Math.max(transport.tick + 1, 1), 1, 2));
  const pos = $derived(formatPosition(bars, transport.tick, project.ppq));
  const sections = $derived([...(project.sections ?? [])].sort((a, b) => a.tick - b.tick));
  let secIdx = $state(-1);
  let loopOn = $state(true);
  function sectionRange(i: number): [number, number] | null {
    const s = sections[i];
    if (!s) return null;
    const end = sections[i + 1]?.tick ?? buildBars(project, s.tick + 1, 1, 0).at(-1)!.tick + 3840 * 8;
    return [s.tick, end];
  }
  async function applySection() {
    const r = sectionRange(secIdx);
    try {
      if (r && loopOn) await api.transportSetLoop(r[0], r[1]);
      else await api.transportClearLoop();
      if (r) await api.transportSeek(r[0]);
    } catch {
      // 再生できない環境
    }
  }
  async function togglePlay() {
    try {
      if (transport.playing) await api.transportPause();
      else await api.transportPlay();
    } catch {
      // 再生できない環境
    }
  }

  // ---- 鳴っている音(エンジンの出口の周波数。曲を流していれば曲全体) ----
  let specTimer = 0;
  let specBusy = false;
  onMount(() => {
    window.addEventListener("keydown", onKeydown, true);
    window.addEventListener("keyup", onKeyup, true);
    document.addEventListener("pointerdown", dragStart, true);
    document.addEventListener("pointerup", dragEnd, true);
    window.addEventListener("pointerup", afterPointerUp);
    document.addEventListener("pointercancel", endDrag, true);
    window.addEventListener("blur", endDrag);
    const onResize = () => {
      alignPaneCanvases();
      draw();
    };
    window.addEventListener("resize", onResize);
    const ro = new ResizeObserver(onResize);
    if (root) ro.observe(root);
    specTimer = window.setInterval(async () => {
      if (specBusy || !view.open) return;
      specBusy = true;
      try {
        const s = await api.transportSpectrum();
        drawSpectrum(s.bands, s.db);
      } catch {
        drawSpectrum([], null);
      } finally {
        specBusy = false;
      }
    }, 70);
    return () => {
      window.removeEventListener("keydown", onKeydown, true);
      window.removeEventListener("keyup", onKeyup, true);
      document.removeEventListener("pointerdown", dragStart, true);
      document.removeEventListener("pointerup", dragEnd, true);
      window.removeEventListener("pointerup", afterPointerUp);
      document.removeEventListener("pointercancel", endDrag, true);
      window.removeEventListener("blur", endDrag);
      window.removeEventListener("resize", onResize);
      ro.disconnect();
      clearInterval(specTimer);
    };
  });
  onDestroy(() => {
    keyListeners.delete(onKeyLight);
    close();
  });

  const helpRows = $derived.by((): [string, string][] => {
    void view.title;
    const rows: [string, string][] = [
      [
        tr("共通", "Common"),
        tr(
          "A / B で作り込む前と今を聴き比べ。Ctrl+Z で取り消し(このトラックの音色の変更だけ)。「AI に頼む」は選んでいる所を対象にしてチャットへ。鍵盤と A〜K のキーで鳴らせる",
          "A / B compares before and now. Ctrl+Z undoes (only this track's sound changes). “Ask AI” sends the selected part to the chat as the target. Play with the keyboard or the A–K keys",
        ),
      ],
    ];
    return view.open && seState.inst ? [...rows, ...ed().help()] : rows;
  });
</script>

<div class="sound-editor" class:ab-before={view.ab === "before"} bind:this={root} lang="ja">
  <div class="ed-head">
    <span class="ed-title">{tr("音色エディタ", "Sound editor")}</span>
    <select
      class="trackpick"
      title={tr("作り込むトラック(インスペクターで開いているトラック)", "Track to edit (the one open in the inspector)")}
      value={soundEditorStore.trackId}
      onchange={(e) => pickTrack((e.currentTarget as HTMLSelectElement).value)}
    >
      {#each tracks as t (t.id)}
        <option value={t.id}>{t.name}({editorKindOf(t)})</option>
      {/each}
    </select>
    <span class="sep"></span>
    <div class="seg" title={tr("作り込む前(このエディタを開いたとき)と今を、切り替えて聴く", "Switch between before (when this editor opened) and now")}>
      <button class:on={view.ab === "before"} onclick={() => setAB("before")}>{tr("A 作り込む前", "A Before")}</button><button
        class:on={view.ab === "now"}
        onclick={() => setAB("now")}>{tr("B 今", "B Now")}</button
      >
    </div>
    <button class="btn quiet" title={tr("開いたときの音に戻す(戻したことも履歴に残る)", "Back to the sound from when you opened it (kept in the history)")} onclick={revertAll}
      >{tr("作り込む前に戻す", "Revert to before")}</button
    >
    <span class="sep"></span>
    <button class="btn icon quiet" title={tr("取り消し(Ctrl+Z。このトラックの音色の変更)", "Undo (Ctrl+Z; this track's sound changes)")} disabled={!view.canUndo} onclick={undo}>↶</button>
    <button class="btn icon quiet" title={tr("やり直し(Ctrl+Y)", "Redo (Ctrl+Y)")} disabled={!view.canRedo} onclick={redo}>↷</button>
    <span class="sep"></span>
    <button class="btn ai" title={tr("選んでいる所(加工・編集した波形・つまみ・サンプルなど)を対象にして、下のチャットへ", "Send the selected part (an edit, a knob, a sample…) to the chat below as the target")} onclick={askAi}
      >✦ {tr("AI に頼む", "Ask AI")}</button
    >
    <span class="spacer"></span>
    <span class="hint">{view.sel ? tr(`選んでいる所: ${view.sel}`, `Selected: ${view.sel}`) : ""}</span>
    <button class="btn icon quiet" title={tr("使い方", "How to use")} onclick={(e) => ((helpOpen = !helpOpen), e.stopPropagation())}>?</button>
    <button class="btn icon quiet" title={tr("閉じる(Esc)", "Close (Esc)")} onclick={closeEditor}>×</button>
  </div>
  {#if helpOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="help open" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <b>{tr(`${view.title}の使い方`, `How to use ${view.title}`)}</b>
      <table>
        <tbody>
          {#each helpRows as [a, b] (a)}
            <tr><td>{a}</td><td>{b}</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
  {#if view.error}
    <div class="ed-body"><div class="col center"><div class="hint">{view.error}</div></div></div>
  {/if}
  <div class="ed-body" bind:this={bodyEl} hidden={!!view.error}></div>
  <div class="ed-foot">
    <div class="songbar" title={tr("曲を流しながら音を作る(アプリの再生と同じ。区間のループ中も A / B を切り替えられる)", "Shape the sound while the song plays (same as the app's playback; A / B works during a section loop)")}>
      <button class="btn icon" title={tr("曲を再生 / 止める(Space)", "Play / stop the song (Space)")} onclick={togglePlay}>{transport.playing ? "■" : "▶"}</button>
      <span class="pos">{pos}</span>
      <select title={tr("ループする区間", "Section to loop")} bind:value={secIdx} onchange={applySection}>
        <option value={-1}>{tr("区間を選ぶ", "Pick a section")}</option>
        {#each sections as s, i (s.tick)}
          <option value={i}>{s.name}</option>
        {/each}
      </select>
      <label><input type="checkbox" bind:checked={loopOn} onchange={applySection} /> {tr("ループ", "Loop")}</label>
      <label title={tr("ほかのトラックを止めて、このトラックだけを聴く(曲には残さない)", "Mute the other tracks and hear only this one (not saved to the song)")}
        ><input type="checkbox" bind:checked={listen.solo} /> {tr("このトラックだけ", "This track only")}</label
      >
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="kbd" onpointerdown={kbdDown} onpointermove={kbdMove} onpointerup={kbdUp} onpointercancel={kbdUp}>
      {#each KEYS as m (m)}
        {#if BLACK.has(m % 12)}
          <div class="b" class:on={lit[m]} data-m={m} style="left:{whiteX(m) - 7}px"></div>
        {:else}
          <div class="w" class:on={lit[m]} data-m={m}>{#if m % 12 === 0}<span>C{m / 12 - 1}</span>{/if}</div>
        {/if}
      {/each}
    </div>
    <div style="display: flex; flex-direction: column; gap: 2px">
      <label class="small dim" style="display: flex; gap: 4px; align-items: center"><input type="checkbox" bind:checked={listen.chord} /> {tr("和音で鳴らす", "Play a chord")}</label>
    </div>
    <span class="spacer"></span>
    {#if view.ab === "before"}
      <span class="abnote">{tr("A(作り込む前)を鳴らしています。調整するときは B に戻します", "Playing A (before). Switch back to B to edit")}</span>
    {/if}
    <span class="hint" title={tr("ドラッグ中は試聴だけ", "Dragging only previews")}>{tr("変更は 1 つずつ曲の履歴に残る", "Each change is kept in the song history")}</span>
  </div>
</div>

<svelte:window onclick={() => (helpOpen = false)} />
