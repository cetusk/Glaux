<script lang="ts">
  // 上の帯の「自分の曲 / 参考曲 / 重ねる」の切り替え(参考曲のトラックがあるときだけ出す)。
  // 切り替えは再生位置のまま約 10ms でなめらかに。聴き比べ・重ねるときは音量を自動でそろえる
  import Icon from "./Icon.svelte";
  import { tr } from "./i18n.svelte";
  import { referenceClipId, referenceStore, setReferenceMode } from "./reference.svelte";
  import type { ReferenceMode } from "./api";
  import type { Project } from "./types";

  let {
    project,
    projectVersion,
    range,
  }: {
    project: Project | null;
    projectVersion: number;
    /** 音量を測る範囲(ルーラーで選んだ区間。無ければ参考曲全体) */
    range: [number, number] | null;
  } = $props();

  const summary = $derived.by(() => {
    const id = referenceClipId(project);
    return id ? (referenceStore.summaries[id] ?? "") : "";
  });

  /** 音量をそろえた量の説明 */
  const gainText = $derived.by(() => {
    const m = referenceStore.match;
    if (!m) return "";
    if (m.song_lufs == null) return tr("曲がまだ鳴らないので、音量はそろえていません", "The song is silent, so loudness isn't matched");
    const parts: string[] = [];
    if (m.reference_gain_db < -0.05) parts.push(tr(`参考曲を ${m.reference_gain_db.toFixed(1)} dB`, `reference ${m.reference_gain_db.toFixed(1)} dB`));
    if (m.song_gain_db < -0.05) parts.push(tr(`自分の曲を ${m.song_gain_db.toFixed(1)} dB`, `your song ${m.song_gain_db.toFixed(1)} dB`));
    return parts.length
      ? tr(`音量をそろえています(${parts.join("・")})`, `Loudness matched (${parts.join(", ")})`)
      : tr("音量はほぼ同じです", "Loudness is about the same");
  });

  function pick(mode: ReferenceMode) {
    void setReferenceMode(mode, projectVersion, range);
  }

  const mine = $derived(referenceStore.mode === "off" || referenceStore.mode === "song");
</script>

<div class="ref-switch" role="group" aria-label={tr("参考曲との聴き比べ", "Compare with the reference")}>
  <button
    class="btn mine"
    class:on={mine}
    aria-pressed={mine}
    onclick={() => pick(referenceStore.mode === "off" ? "off" : "song")}
    title={tr(
      `自分の曲を聴く${gainText ? `。${gainText}` : ""}`,
      `Listen to your song${gainText ? `. ${gainText}` : ""}`,
    )}
    aria-label={tr("自分の曲", "My song")}><Icon name="music" /><span class="lbl">{tr("自分の曲", "Mine")}</span></button
  >
  <button
    class="btn ref"
    class:on={referenceStore.mode === "reference"}
    aria-pressed={referenceStore.mode === "reference"}
    onclick={() => pick(referenceStore.mode === "reference" ? "song" : "reference")}
    title={tr(
      `参考曲を聴く(再生位置のまま切り替わります。音量は自動でそろえます。もう一度押すと自分の曲に戻ります)${gainText ? `。${gainText}` : ""}${summary ? `\n参考曲: ${summary}` : ""}`,
      `Listen to the reference (switches at the playhead; loudness is matched automatically. Click again to return to your song)${gainText ? `. ${gainText}` : ""}${summary ? `\nReference: ${summary}` : ""}`,
    )}
    aria-label={tr("参考曲", "Reference")}><Icon name="disc-3" /><span class="lbl">{tr("参考曲", "Reference")}</span></button
  >
  <button
    class="btn ref both"
    class:on={referenceStore.mode === "both"}
    aria-pressed={referenceStore.mode === "both"}
    onclick={() => pick(referenceStore.mode === "both" ? "song" : "both")}
    title={tr(
      "重ねて聴く(参考曲を自分の曲と同じくらいの大きさで重ねる。打ち込みの位置・音程を合わせるときに)",
      "Overlay (plays the reference at about the same level as your song; for checking timing and pitch)",
    )}
    aria-label={tr("重ねる", "Overlay")}><Icon name="layers" /><span class="lbl">{tr("重ねる", "Overlay")}</span></button
  >
  {#if referenceStore.matching}
    <span class="busy" title={tr("音量を測っています…", "Measuring loudness…")}><Icon name="loader-circle" size={14} /></span>
  {/if}
</div>

<style>
  .ref-switch {
    display: flex;
    align-items: center;
  }

  .ref-switch .btn {
    height: 30px;
    padding: 0 8px;
    border-radius: 0;
    gap: 4px;
  }

  .ref-switch .btn:first-child {
    border-radius: var(--r-sm) 0 0 var(--r-sm);
  }

  .ref-switch .btn:nth-child(3) {
    border-radius: 0 var(--r-sm) var(--r-sm) 0;
  }

  .ref-switch .btn + .btn {
    margin-left: -1px;
  }

  .ref-switch .btn.ref.on {
    background: color-mix(in srgb, var(--reference) 18%, var(--bg-panel));
    border-color: var(--reference);
    color: var(--reference);
  }

  .lbl {
    font-size: var(--fs-xs);
  }

  /* 狭いときは文字をしまってアイコンだけ(吹き出しで中身が分かる。再生の表示窓の幅を空ける) */
  @media (max-width: 1240px) {
    .lbl {
      display: none;
    }
    .ref-switch .btn {
      padding: 0 6px;
    }
  }

  /* とても狭いときは「参考曲」だけを残す(押すたびに参考曲 ⇄ 自分の曲。重ねるは広い窓で) */
  @media (max-width: 900px) {
    .ref-switch .btn.mine,
    .ref-switch .btn.both {
      display: none;
    }
    .ref-switch .btn.ref:not(.both) {
      border-radius: var(--r-sm);
      margin-left: 0;
    }
  }

  .busy {
    display: inline-flex;
    margin-left: 6px;
    color: var(--text-dim);
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
