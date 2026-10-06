<script lang="ts">
  import Icon from "./Icon.svelte";
  // 書き出しの画面。以前は 48kHz / 16bit / 曲全体 / プロジェクトの export/ 固定で、選べるものが無かった。
  import { open as pickDir, save as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { selectionStore } from "./selection.svelte";
  import { showError, showToast } from "./toast.svelte";
  import { plural, tr } from "./i18n.svelte";

  let {
    onClose,
    loop,
  }: {
    onClose: () => void;
    /** ループ区間 [開始 tick, 終了 tick](無ければ null) */
    loop: [number, number] | null;
  } = $props();

  let target = $state<"mix" | "stems" | "midi">("mix");
  let range = $state<"all" | "loop" | "selection">("all");
  let sampleRate = $state(48000);
  let bits = $state(16);
  let format = $state<"wav" | "flac">("wav");
  // FLAC は整数のみ(32bit 浮動小数は WAV だけ)
  $effect(() => {
    if (format === "flac" && bits === 32) bits = 24;
  });
  let noiseShaping = $state(true);
  let loudness = $state<string>("none");
  let path = $state<string | null>(null);
  let busy = $state(false);
  let result = $state<string | null>(null);

  // 対象・形式を変えたら保存先は既定に戻す(WAV / FLAC・フォルダ・.mid で選ぶものが違う)
  $effect(() => {
    void target;
    void format;
    path = null;
    result = null;
  });

  const LOUDNESS = $derived([
    { value: "none", label: tr("そのまま", "Off") },
    { value: "-14", label: tr("-14 LUFS(配信: YouTube・Spotify など)", "-14 LUFS (streaming: YouTube, Spotify, etc.)") },
    { value: "-16", label: tr("-16 LUFS(Apple Music・ポッドキャスト)", "-16 LUFS (Apple Music, podcasts)") },
    { value: "-23", label: tr("-23 LUFS(放送)", "-23 LUFS (broadcast)") },
  ]);

  async function choosePath() {
    try {
      const p =
        target === "stems"
          ? await pickDir({ directory: true, title: tr(`トラックごとの ${format.toUpperCase()} を書き出すフォルダ`, `Folder for per-track ${format.toUpperCase()} files`) })
          : target === "midi"
            ? await pickFile({ title: tr("書き出す MIDI ファイル", "Export MIDI file"), filters: [{ name: "MIDI", extensions: ["mid"] }] })
            : format === "flac"
              ? await pickFile({ title: tr("書き出す FLAC ファイル", "Export FLAC file"), filters: [{ name: "FLAC", extensions: ["flac"] }] })
              : await pickFile({ title: tr("書き出す WAV ファイル", "Export WAV file"), filters: [{ name: "WAV", extensions: ["wav"] }] });
      if (typeof p === "string") path = p;
    } catch (e) {
      showError(tr("保存先を選べませんでした", "Couldn't choose a destination"), e);
    }
  }

  async function run() {
    if (busy) return;
    if (target === "midi") {
      busy = true;
      result = null;
      try {
        const r = await api.exportMidi(path ?? undefined);
        result = tr(
          `MIDI に書き出しました(トラック ${r.tracks} 本): ${r.path}`,
          `Exported MIDI (${plural(r.tracks, "track")}): ${r.path}`,
        );
        showToast("ok", result);
      } catch (e) {
        showError(tr("書き出せませんでした", "Export failed"), e);
      } finally {
        busy = false;
      }
      return;
    }
    const request: api.ExportRequest = {
      path: path ?? undefined,
      sample_rate: sampleRate,
      bits,
      format,
      noise_shaping: bits === 16 ? noiseShaping : undefined,
      stems: target === "stems",
      loudness_lufs: target === "mix" && loudness !== "none" ? Number(loudness) : undefined,
    };
    if (range === "loop" && loop) {
      request.start_tick = loop[0];
      request.end_tick = loop[1];
    } else if (range === "selection" && selectionStore.range) {
      request.start_tick = selectionStore.range.startTick;
      request.end_tick = selectionStore.range.endTick;
    }
    busy = true;
    result = null;
    try {
      const r = await api.exportAudio(request);
      if (r.stems) {
        result = tr(`トラック ${r.stems.length} 本を書き出しました: ${r.folder}`, `Exported ${plural(r.stems.length, "track")}: ${r.folder}`);
      } else {
        const lufs = Number.isFinite(r.lufs) ? `${r.lufs} LUFS / ` : "";
        const gain = r.gain_db ? ` / ${tr("音量", "Gain")} ${r.gain_db > 0 ? "+" : ""}${r.gain_db} dB` : "";
        const lim = r.limiter_db ? ` / ${tr("リミッタ最大", "Limiter max")} -${r.limiter_db} dB` : "";
        const sp = r.streaming?.find((s) => s.service === "Spotify");
        const spot =
          sp && sp.gain_db < -0.5
            ? tr(`。Spotify では ${-sp.gain_db} dB 下げて再生される見込み`, `; Spotify will likely play it ${-sp.gain_db} dB quieter`)
            : "";
        result = tr(
          `書き出しました(${r.seconds?.toFixed(1)} 秒、${lufs}True Peak ${r.true_peak_db ?? r.peak_db} dBTP${gain}${lim}${spot}): ${r.path}`,
          `Exported (${r.seconds?.toFixed(1)} s, ${lufs}True Peak ${r.true_peak_db ?? r.peak_db} dBTP${gain}${lim}${spot}): ${r.path}`,
        );
      }
      showToast("ok", result);
    } catch (e) {
      showError(tr("書き出せませんでした", "Export failed"), e);
    } finally {
      busy = false;
    }
  }
</script>

<div class="backdrop" role="presentation" onclick={onClose}></div>
<div class="panel" role="dialog" aria-label={tr("書き出し", "Export")}>
  <div class="head">
    <h2><Icon name="download" />{tr("書き出し", "Export")}</h2>
    <button class="btn sm icon ghost" onclick={onClose} title={tr("閉じる(Esc)", "Close (Esc)")} aria-label={tr("閉じる", "Close")}><Icon name="x" /></button>
  </div>

  <div class="section">
    <div class="section-title">{tr("対象", "Target")}</div>
    <label class="row"><input type="radio" bind:group={target} value="mix" /> {tr("曲全体(ミックス)", "Whole song (mix)")}</label>
    <label class="row"
      ><input type="radio" bind:group={target} value="stems" />
      {tr("トラックごと(ステム。マスターのエフェクトは通さない)", "Stems (per track, without master effects)")}</label
    >
    <label class="row"
      ><input type="radio" bind:group={target} value="midi" />
      {tr(
        "MIDI ファイル(ノート・テンポ・拍子・マーカー。ループは展開)",
        "MIDI file (notes, tempo, time signature, markers; loops expanded)",
      )}</label
    >
  </div>

  {#if target !== "midi"}
    <div class="section">
      <div class="section-title">{tr("範囲", "Range")}</div>
      <label class="row"><input type="radio" bind:group={range} value="all" /> {tr("曲全体", "Whole song")}</label>
      <label class="row" class:off={!loop}
        ><input type="radio" bind:group={range} value="loop" disabled={!loop} /> {tr("ループ区間", "Loop range")}</label
      >
      <label class="row" class:off={!selectionStore.range}
        ><input type="radio" bind:group={range} value="selection" disabled={!selectionStore.range} />
        {tr("選んだ小節", "Selection")}{selectionStore.range
          ? tr(
              `(${selectionStore.range.startBar + 1}〜${selectionStore.range.endBar + 1} 小節)`,
              ` (bars ${selectionStore.range.startBar + 1}–${selectionStore.range.endBar + 1})`,
            )
          : tr("(ルーラーをドラッグして選ぶ)", " (drag on the ruler to select)")}</label
      >
    </div>

    <div class="section">
      <div class="section-title">{tr("形式", "Format")}</div>
      <div class="row">
        <select bind:value={format} aria-label={tr("ファイルの形式", "File format")}>
          <option value="wav">WAV</option>
          <option value="flac">{tr("FLAC(可逆圧縮。WAV の半分前後の大きさ)", "FLAC (lossless, about half the size of WAV)")}</option>
        </select>
        <select bind:value={sampleRate} aria-label={tr("サンプルレート", "Sample rate")}>
          <option value={48000}>48 kHz</option>
          <option value={44100}>{tr("44.1 kHz(CD・配信)", "44.1 kHz (CD, streaming)")}</option>
        </select>
        <select bind:value={bits} aria-label={tr("ビット数", "Bit depth")}>
          <option value={16}>16 bit</option>
          <option value={24}>24 bit</option>
          {#if format === "wav"}<option value={32}>{tr("32 bit 浮動小数", "32 bit float")}</option>{/if}
        </select>
      </div>
      {#if bits === 16}
        <label
          class="check"
          title={tr(
            "16bit に丸めるときの雑音を、耳が敏感な 2〜5kHz から聞こえにくい高域へ寄せる(聞こえ方で約 20dB 静か)。書き出した後でさらに加工・変換するなら外す",
            "Shifts 16-bit rounding noise from the sensitive 2–5 kHz range to less audible highs (about 20 dB quieter perceptually). Turn off if you'll process or convert the file further",
          )}
          ><input type="checkbox" bind:checked={noiseShaping} /> {tr("ノイズシェーピング", "Noise shaping")}</label
        >
      {/if}
    </div>

  {/if}

  {#if target === "mix"}
    <div class="section">
      <div class="section-title">{tr("音量", "Loudness")}</div>
      <select bind:value={loudness} aria-label={tr("音量の目標", "Loudness target")}>
        {#each LOUDNESS as l (l.value)}
          <option value={l.value}>{l.label}</option>
        {/each}
      </select>
      {#if loudness !== "none"}
        <div class="note">
          {tr(
            "曲全体の音量をこの値に合わせ、True Peak(サンプルの間の山も含むピーク)が -1 dBTP を超える所はリミッタで抑えます",
            "Normalizes the song to this loudness and limits anything above -1 dBTP true peak (including inter-sample peaks)",
          )}
        </div>
      {/if}
    </div>
  {/if}

  <div class="section">
    <div class="section-title">{tr("保存先", "Destination")}</div>
    <div class="row">
      <code class="path" title={path ?? ""}>{path ?? tr("プロジェクトの export フォルダ(日時付きの名前)", "Project export folder (timestamped name)")}</code>
      <button onclick={choosePath}>{tr("選ぶ…", "Choose…")}</button>
      {#if path}<button class="btn sm icon ghost" onclick={() => (path = null)} title={tr("既定に戻す", "Reset to default")} aria-label={tr("既定に戻す", "Reset to default")}
          ><Icon name="x" /></button
        >{/if}
    </div>
  </div>

  <button class="go" onclick={run} disabled={busy}>{busy ? tr("書き出し中…", "Exporting…") : tr("書き出す", "Export")}</button>
  {#if result}<div class="note result">{result}</div>{/if}
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 29;
    background: rgba(0, 0, 0, 0.45);
  }

  .panel {
    position: fixed;
    z-index: 30;
    top: 60px;
    right: 16px;
    width: 380px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: var(--fs-lg);
  }

  h2 :global(.icon) {
    color: var(--accent);
  }

  .section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .section-title {
    font-size: 11px;
    color: var(--text-dim);
    letter-spacing: 0.05em;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
  }

  .row.off {
    color: var(--text-dim);
  }

  .path {
    flex: 1;
    min-width: 0;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-dim);
  }

  .note {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }

  .result {
    word-break: break-all;
  }

  .go {
    padding: 6px 12px;
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .check {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    font-size: var(--fs-sm);
    color: var(--text-dim);
    cursor: pointer;
  }
</style>
