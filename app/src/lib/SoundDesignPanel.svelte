<script lang="ts">
  // 音作りビュー: トラック 1 本にフォーカスして音源のつまみ・エフェクトチェーン・
  // プリセットを操作するパネル(docs/HANDOFF.md §8-6 Phase 1)。
  // すべての編集は Command API(apply_edit)経由なので履歴に載り undo できる。
  import * as api from "./api";
  import { newClipId, newFxId } from "./ids";
  import { PHRASE_LEN, PHRASE_NAME, phraseNotes } from "./phrase";
  import { soundDesignStore } from "./selection.svelte";
  import type { ParamView, PresetInfo, Project, Track, TrackParams } from "./types";

  let { project }: { project: Project } = $props();

  const track = $derived.by((): Track | null => {
    const focus = soundDesignStore.focus;
    if (!focus) return null;
    return project.tracks.find((t) => t.id === focus.trackId) ?? null;
  });

  // トラックが消えたら閉じる
  $effect(() => {
    if (soundDesignStore.focus && !track) {
      soundDesignStore.focus = null;
    }
  });

  function close() {
    soundDesignStore.focus = null;
  }

  // ---- spec + 現在値の取得(プロジェクトが変わるたびに再取得 = AI の編集も反映) ----

  let info = $state<TrackParams | null>(null);
  let loadError = $state<string | null>(null);

  $effect(() => {
    const t = track;
    void project; // 依存: どの編集でも現在値を取り直す
    if (!t) {
      info = null;
      return;
    }
    api
      .getTrackParams(t.id)
      .then((r) => {
        info = r;
        loadError = null;
      })
      .catch((e) => (loadError = String(e)));
  });

  async function applyEdit(commands: unknown[], label: string) {
    try {
      await api.applyEdit(commands, label);
    } catch (e) {
      loadError = String(e);
    }
  }

  // ---- パラメータ編集 ----

  function commitParam(p: ParamView, raw: string | number | boolean) {
    const t = track;
    if (!t) return;
    let value: unknown = raw;
    if (p.range.kind === "float") value = Number(raw);
    if (p.range.kind === "int") value = Math.round(Number(raw));
    if (p.range.kind === "bool") value = Boolean(raw);
    applyEdit(
      [{ op: "set_param", track: t.id, path: p.path, value }],
      `${t.name} の ${p.display_name} を変更`,
    );
  }

  function fmtValue(p: ParamView): string {
    const v = p.current;
    if (typeof v === "number") {
      const digits = p.range.kind === "int" ? 0 : Math.abs(v) >= 100 ? 0 : 2;
      return `${v.toFixed(digits)}${p.unit ?? ""}`;
    }
    return `${v}`;
  }

  function sliderStep(p: ParamView): number {
    if (p.range.kind === "int") return 1;
    if (p.range.kind !== "float") return 1;
    return (p.range.max - p.range.min) / 200;
  }

  // ---- 音源・エフェクト ----

  function setDevice(name: string) {
    const t = track;
    if (!t || t.device?.name === name) return;
    applyEdit(
      [{ op: "set_device", track: t.id, device: { type: "builtin", name } }],
      `${t.name} の音源を ${name} に変更`,
    );
  }

  function addEffect(name: string) {
    const t = track;
    if (!t || !name) return;
    applyEdit(
      [
        {
          op: "add_effect",
          track: t.id,
          effect: { id: newFxId(), type: "builtin", name },
        },
      ],
      `${t.name} に ${name} を追加`,
    );
  }

  function removeEffect(id: string, name: string) {
    const t = track;
    if (!t) return;
    applyEdit([{ op: "remove_effect", id }], `${t.name} の ${name} を削除`);
  }

  function toggleBypass(id: string, name: string, bypass: boolean) {
    const t = track;
    if (!t) return;
    applyEdit(
      [{ op: "set_effect_bypass", id, bypass: !bypass }],
      `${t.name} の ${name} を${bypass ? "有効化" : "バイパス"}`,
    );
  }

  let addFxName = $state("");

  // ---- プリセット ----

  let presets = $state<PresetInfo[]>([]);
  let presetName = $state("");
  let presetMsg = $state<string | null>(null);

  async function refreshPresets() {
    try {
      presets = (await api.listPresets()).presets;
    } catch {
      presets = [];
    }
  }

  $effect(() => {
    if (soundDesignStore.focus) refreshPresets();
  });

  async function applyPreset(name: string) {
    const t = track;
    if (!t || !name) return;
    try {
      await api.loadPreset(t.id, name);
      presetMsg = `適用しました: ${name}`;
    } catch (e) {
      presetMsg = String(e);
    }
  }

  async function savePreset() {
    const t = track;
    const name = presetName.trim();
    if (!t || !name) return;
    try {
      await api.savePreset(t.id, name);
      presetMsg = `保存しました: ${name}`;
      presetName = "";
      refreshPresets();
    } catch (e) {
      presetMsg = String(e);
    }
  }

  let selectedPreset = $state("");

  // ---- 試聴(ソロ・試聴フレーズ) ----

  function toggleSolo() {
    const t = track;
    if (!t) return;
    applyEdit(
      [{ op: "set_track_prop", id: t.id, prop: "solo", value: !t.solo }],
      `${t.name} のソロを${t.solo ? "解除" : "オン"}`,
    );
  }

  const phraseClip = $derived(
    track?.clips.find((c) => c.kind === "midi" && c.name === PHRASE_NAME) ?? null,
  );

  function insertPhrase() {
    const t = track;
    if (!t || phraseClip) return;
    // 既存クリップの後ろに置く(なければ先頭)
    const start = t.clips.reduce((end, c) => Math.max(end, c.start + c.length), 0);
    applyEdit(
      [
        {
          op: "add_clip",
          track: t.id,
          clip: {
            id: newClipId(),
            name: PHRASE_NAME,
            start,
            length: PHRASE_LEN,
            kind: "midi",
            notes: phraseNotes(),
          },
        },
      ],
      `${t.name} に試聴フレーズを挿入`,
    );
  }

  function removePhrase() {
    const t = track;
    const c = phraseClip;
    if (!t || !c) return;
    applyEdit([{ op: "remove_clip", id: c.id }], `${t.name} の試聴フレーズを削除`);
  }
</script>

{#if track}
  <div class="sd-panel">
    <div class="sd-head">
      <span class="sd-title">🎛 {track.name}</span>
      <span class="sd-sub">音作りビュー</span>
      <button class="sd-close" onclick={close} title="閉じる">✕</button>
    </div>

    {#if loadError}
      <div class="sd-error">{loadError}</div>
    {/if}

    <div class="sd-body">
      <!-- 試聴 -->
      <div class="sec">
        <div class="sec-title">試聴</div>
        <div class="row gap">
          <button class:on={track.solo} onclick={toggleSolo} title="このトラックだけ聴く / ミックスの中で聴く を切り替え">
            {track.solo ? "🎧 ソロ中" : "🎧 ソロ"}
          </button>
          {#if phraseClip}
            <button onclick={removePhrase}>試聴フレーズを削除</button>
          {:else}
            <button onclick={insertPhrase} title="ロングトーン → 刻み → 分散和音 → オクターブ上の 4 小節を末尾に挿入">
              試聴フレーズを挿入
            </button>
          {/if}
        </div>
        <div class="hint">再生(Space)+ ループ(L)を回しながら調整するのがおすすめです。</div>
      </div>

      <!-- 音源 + プリセット -->
      <div class="sec">
        <div class="sec-title">音源</div>
        <div class="row gap">
          <select
            value={info?.device.name ?? track.device?.name ?? "subtractive"}
            onchange={(e) => setDevice((e.currentTarget as HTMLSelectElement).value)}
            title="切り替えるとパラメータは初期値に戻ります(Ctrl+Z 可)"
          >
            <option value="subtractive">subtractive(シンセ)</option>
            <option value="drum">drum(ドラム)</option>
            <option value="pluck">pluck(撥弦: ギター/ベース)</option>
          </select>
          <select bind:value={selectedPreset} title="プリセット(全プロジェクト共通)">
            <option value="">プリセット…</option>
            {#each presets as p (p.name)}
              <option value={p.name}>{p.name}({p.instrument}{p.effects.length ? `+FX${p.effects.length}` : ""})</option>
            {/each}
          </select>
          <button disabled={!selectedPreset} onclick={() => applyPreset(selectedPreset)}>適用</button>
        </div>
        <div class="row gap">
          <input
            class="grow"
            type="text"
            placeholder="今の音をプリセット保存(名前)"
            bind:value={presetName}
            onkeydown={(e) => {
              if (e.key === "Enter" && !e.isComposing) {
                e.preventDefault();
                savePreset();
              }
            }}
          />
          <button disabled={!presetName.trim()} onclick={savePreset}>保存</button>
        </div>
        {#if presetMsg}<div class="hint">{presetMsg}</div>{/if}
      </div>

      <!-- 音源パラメータ -->
      {#if info}
        <div class="sec">
          <div class="sec-title">
            パラメータ({info.device.name}{info.device.is_default_fallback ? " *未設定" : ""})
          </div>
          {#each info.params as p (p.path)}
            <div class="param" title={p.description}>
              <span class="p-name">{p.display_name}</span>
              {#if p.range.kind === "float" || p.range.kind === "int"}
                <input
                  type="range"
                  min={p.range.min}
                  max={p.range.max}
                  step={sliderStep(p)}
                  value={Number(p.current)}
                  onchange={(e) => commitParam(p, (e.currentTarget as HTMLInputElement).value)}
                />
                <span class="p-val">{fmtValue(p)}</span>
              {:else if p.range.kind === "bool"}
                <input
                  type="checkbox"
                  checked={Boolean(p.current)}
                  onchange={(e) => commitParam(p, (e.currentTarget as HTMLInputElement).checked)}
                />
                <span class="p-val"></span>
              {:else}
                <select
                  value={String(p.current)}
                  onchange={(e) => commitParam(p, (e.currentTarget as HTMLSelectElement).value)}
                >
                  {#each p.range.choices as c (c)}
                    <option value={c}>{c}</option>
                  {/each}
                </select>
                <span class="p-val"></span>
              {/if}
            </div>
          {/each}
        </div>

        <!-- エフェクトチェーン -->
        <div class="sec">
          <div class="sec-title">エフェクト({info.effects.length})</div>
          {#each info.effects as fx (fx.id)}
            <div class="fx" class:bypassed={fx.bypass}>
              <div class="fx-head">
                <span class="fx-name">{fx.name}</span>
                <button
                  class="mini"
                  class:on={!fx.bypass}
                  onclick={() => toggleBypass(fx.id, fx.name, fx.bypass)}
                  title={fx.bypass ? "バイパス中(クリックで有効化)" : "有効(クリックでバイパス)"}
                >
                  {fx.bypass ? "OFF" : "ON"}
                </button>
                <button class="mini danger" onclick={() => removeEffect(fx.id, fx.name)} title="削除(Ctrl+Z 可)">
                  ✕
                </button>
              </div>
              {#each fx.params as p (p.path)}
                <div class="param" title={p.description}>
                  <span class="p-name">{p.display_name}</span>
                  {#if p.range.kind === "float" || p.range.kind === "int"}
                    <input
                      type="range"
                      min={p.range.min}
                      max={p.range.max}
                      step={sliderStep(p)}
                      value={Number(p.current)}
                      onchange={(e) => commitParam(p, (e.currentTarget as HTMLInputElement).value)}
                    />
                    <span class="p-val">{fmtValue(p)}</span>
                  {:else if p.range.kind === "bool"}
                    <input
                      type="checkbox"
                      checked={Boolean(p.current)}
                      onchange={(e) => commitParam(p, (e.currentTarget as HTMLInputElement).checked)}
                    />
                    <span class="p-val"></span>
                  {:else}
                    <select
                      value={String(p.current)}
                      onchange={(e) => commitParam(p, (e.currentTarget as HTMLSelectElement).value)}
                    >
                      {#each p.range.choices as c (c)}
                        <option value={c}>{c}</option>
                      {/each}
                    </select>
                    <span class="p-val"></span>
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
          <div class="row gap">
            <select bind:value={addFxName} class="grow">
              <option value="">エフェクトを追加…</option>
              {#each info.available_effects as fx (fx.name)}
                <option value={fx.name} title={fx.description}>{fx.name}</option>
              {/each}
            </select>
            <button
              disabled={!addFxName}
              onclick={() => {
                addEffect(addFxName);
                addFxName = "";
              }}
            >
              追加
            </button>
          </div>
        </div>
      {/if}

      <div class="hint">
        つまみで大枠を作り、細かい狙いは AI へ(「もっと太く」「刺さる高域を抑えて」)。
        良い音ができたらプリセット保存を。すべて Ctrl+Z で戻せます。
      </div>
    </div>
  </div>
{/if}

<style>
  .sd-panel {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 380px;
    z-index: 6;
    background: var(--bg-panel);
    border-left: 1px solid var(--border);
    box-shadow: -8px 0 24px rgba(0, 0, 0, 0.35);
    display: flex;
    flex-direction: column;
  }

  .sd-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }

  .sd-title {
    font-weight: 600;
    font-size: 14px;
  }

  .sd-sub {
    font-size: 11px;
    color: var(--text-dim);
  }

  .sd-close {
    margin-left: auto;
    border: none;
    background: none;
    color: var(--text-dim);
  }

  .sd-body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .sec {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .sec-title {
    font-size: 11px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .row {
    display: flex;
    align-items: center;
  }

  .gap {
    gap: 6px;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .row input[type="text"],
  .row select {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 4px 6px;
    font-size: 12px;
    min-width: 0;
  }

  button.on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .param {
    display: grid;
    grid-template-columns: 92px 1fr 64px;
    align-items: center;
    gap: 6px;
    font-size: 11px;
  }

  .p-name {
    color: var(--text-dim);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .param input[type="range"] {
    width: 100%;
    accent-color: var(--accent);
    height: 14px;
  }

  .param select {
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 4px;
    font-size: 11px;
  }

  .p-val {
    text-align: right;
    font-variant-numeric: tabular-nums;
    color: var(--text-dim);
    font-size: 10px;
  }

  .fx {
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 6px 8px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .fx.bypassed {
    opacity: 0.55;
  }

  .fx-head {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .fx-name {
    font-size: 12px;
    font-weight: 600;
    flex: 1;
  }

  .mini {
    font-size: 9px;
    padding: 1px 7px;
  }

  .mini.on {
    border-color: var(--accent-dim);
    color: var(--accent);
  }

  .mini.danger:hover {
    background: #5c2b33;
    color: #ffb4c0;
  }

  .hint {
    font-size: 10px;
    color: var(--text-dim);
    line-height: 1.6;
  }

  .sd-error {
    background: #5c2b33;
    color: #ffb4c0;
    font-size: 11px;
    padding: 5px 12px;
  }
</style>
