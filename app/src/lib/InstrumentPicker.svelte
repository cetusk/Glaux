<script lang="ts">
  // 音源ピッカー: 内蔵・音色のプリセット・SoundFont・CLAP・サンプル(WAV)を 1 か所から選ぶ。
  // トラックの見出しの音源名と、インスペクターの「変更」から開く(instrumentPickerStore)。
  // どの選び方も Command(set_device など)を通るので Ctrl+Z で戻せる。
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";
  import { BUILTIN_INSTRUMENTS, deviceName } from "./instruments";
  import { keepInView } from "./menu";
  import { instrumentPickerStore } from "./selection.svelte";
  import { showError, showToast } from "./toast.svelte";
  import type { PresetInfo, Project } from "./types";

  let { project }: { project: Project } = $props();

  type Tab = "builtin" | "preset" | "sf2" | "clap" | "sample";
  const TABS: { key: Tab; label: string; icon: IconName }[] = [
    { key: "builtin", label: "内蔵", icon: "audio-waveform" },
    { key: "preset", label: "音色のプリセット", icon: "save" },
    { key: "sf2", label: "SoundFont", icon: "library" },
    { key: "clap", label: "CLAP", icon: "plug" },
    { key: "sample", label: "サンプル(WAV)", icon: "file-audio" },
  ];

  const open = $derived(instrumentPickerStore.open);
  const track = $derived(open ? (project.tracks.find((t) => t.id === open.trackId) ?? null) : null);
  let tab = $state<Tab>("builtin");
  let query = $state("");

  let presets = $state<PresetInfo[] | null>(null);
  let sfFiles = $state<string[] | null>(null);
  let sfFile = $state("");
  let sfPresets = $state<{ bank: number; preset: number; name: string }[]>([]);
  let sfBusy = $state(false);
  let clapList = $state<api.ClapPluginInfo[] | null>(null);
  let clapDirs = $state<string[]>([]);
  let clapBusy = $state(false);

  // 開いたら今の音源の種類のタブにする
  let openedFor = "";
  $effect(() => {
    const t = track;
    if (!t || !open) return;
    const key = `${t.id}:${open.x}:${open.y}`;
    if (key === openedFor) return;
    openedFor = key;
    query = "";
    const type = t.device?.type;
    tab = open.tab ?? (type === "clap" ? "clap" : type === "sf2" ? "sf2" : type === "sampler" ? "sample" : "builtin");
    if (type === "sf2") {
      const d = t.device as { soundfont?: string };
      if (d.soundfont) pickSf(d.soundfont);
    }
  });

  $effect(() => {
    if (!open) return;
    if (tab === "preset" && presets === null)
      api.listPresets().then((r) => (presets = r.presets)).catch(() => (presets = []));
    if (tab === "sf2" && sfFiles === null)
      api.listSoundfonts().then((r) => (sfFiles = r.files)).catch(() => (sfFiles = []));
    if (tab === "clap" && clapList === null) loadClap(false);
  });

  async function loadClap(rescan: boolean) {
    clapBusy = true;
    try {
      const r = await api.clapPlugins(rescan);
      clapList = r.plugins.filter((p) => p.instrument);
      clapDirs = r.dirs;
    } catch {
      clapList = [];
    } finally {
      clapBusy = false;
    }
  }

  async function pickSf(file: string) {
    sfFile = file;
    sfPresets = [];
    if (!file) return;
    sfBusy = true;
    try {
      sfPresets = (await api.listSoundfontPresets(file)).presets;
    } catch (e) {
      showError("SoundFont を読めませんでした", e);
    } finally {
      sfBusy = false;
    }
  }

  function close() {
    instrumentPickerStore.open = null;
  }

  async function run(p: Promise<unknown>, what: string) {
    close();
    try {
      await p;
    } catch (e) {
      showError(what, e);
    }
  }

  function setBuiltin(name: string) {
    const t = track;
    if (!t) return;
    if (t.device?.type !== "clap" && t.device?.type !== "sf2" && t.device?.name === name) return close();
    run(
      api.applyEdit([{ op: "set_device", track: t.id, device: { type: "builtin", name } }], `${t.name} の音源を ${name} に変更`),
      "音源を変えられませんでした",
    );
  }

  function applyPreset(name: string) {
    const t = track;
    if (t) run(api.loadPreset(t.id, name), "プリセットを読み込めませんでした");
  }

  function setSf(bank: number, preset: number, name: string) {
    const t = track;
    if (!t) return;
    run(
      api.applyEdit(
        [{ op: "set_device", track: t.id, device: { type: "sf2", soundfont: sfFile, bank, preset } }],
        `${t.name} の音源を「${name}」(SoundFont)に変更`,
      ),
      "SoundFont にできませんでした",
    );
  }

  async function addSf() {
    const file = await pickFile({ title: "SoundFont(.sf2)をライブラリに追加", filters: [{ name: "SoundFont", extensions: ["sf2"] }] });
    if (typeof file !== "string") return;
    try {
      const r = await api.addSoundfont(file);
      sfFiles = (await api.listSoundfonts()).files;
      pickSf(r.file);
    } catch (e) {
      showError("SoundFont を追加できませんでした", e);
    }
  }

  function setClap(p: api.ClapPluginInfo) {
    const t = track;
    if (!t) return;
    if (t.device?.type === "clap" && t.device.plugin_id === p.id) return close();
    run(
      api.applyEdit([{ op: "set_device", track: t.id, device: { type: "clap", plugin_id: p.id } }], `${t.name} の音源を ${p.name}(CLAP)に変更`),
      "プラグインを読み込めませんでした",
    );
  }

  async function importSample() {
    const t = track;
    if (!t) return;
    const file = await pickFile({
      title: "サンプル(WAV / MP3 等)を読み込む",
      filters: [{ name: "音声(WAV / MP3 / FLAC / OGG / M4A)", extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
    });
    if (typeof file !== "string") return;
    close();
    try {
      await api.importSample(t.id, file);
      showToast("ok", "サンプルを設定しました(インスペクターの「元の音程」をサンプルの実音に合わせてください)");
    } catch (e) {
      showError("サンプルを読み込めませんでした", e);
    }
  }

  const q = $derived(query.trim().toLowerCase());
  const hit = (s: string) => !q || s.toLowerCase().includes(q);
  const current = $derived(track ? deviceName(track.device) : "");

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape" && open) {
      e.preventDefault();
      e.stopImmediatePropagation();
      close();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if open && track}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="backdrop" role="presentation" onclick={close} oncontextmenu={(e) => { e.preventDefault(); close(); }}></div>
  <div class="picker" role="dialog" aria-label="音源を選ぶ" use:keepInView style="left:{open.x}px;top:{open.y}px">
    <div class="pk-head">
      <Icon name="audio-waveform" />
      <b>音源を選ぶ — {track.name}</b>
      <input type="search" placeholder="絞り込む" bind:value={query} />
      <button class="btn sm icon ghost" onclick={close} title="閉じる(Esc)" aria-label="閉じる"><Icon name="x" /></button>
    </div>
    <div class="pk-body">
      <div class="tabs" role="tablist">
        {#each TABS as t (t.key)}
          <button class="tab" class:sel={tab === t.key} role="tab" aria-selected={tab === t.key} onclick={() => (tab = t.key)}>
            <Icon name={t.icon} />{t.label}
          </button>
        {/each}
      </div>
      <div class="list">
        {#if tab === "builtin"}
          {#each BUILTIN_INSTRUMENTS.filter((b) => hit(b.name) || hit(b.desc)) as b (b.name)}
            <button class="item" class:sel={track.device?.type !== "clap" && track.device?.type !== "sf2" && track.device?.type !== "sampler" && (track.device?.name ?? "subtractive") === b.name} onclick={() => setBuiltin(b.name)}>
              <Icon name={b.icon} size={20} />
              <span><b>{b.name}</b><small>{b.desc}</small></span>
            </button>
          {/each}
          <div class="note">切り替えるとつまみは初期値に戻ります(Ctrl+Z で戻せます)</div>
        {:else if tab === "preset"}
          {#if presets === null}
            <div class="note">読み込んでいます…</div>
          {:else if presets.length === 0}
            <div class="note">まだありません。インスペクターの「音源」の保存ボタンで、今の音を保存できます。</div>
          {:else}
            {#each presets.filter((p) => hit(p.name) || hit(p.instrument)) as p (p.name)}
              <button class="item" onclick={() => applyPreset(p.name)} title={p.description ?? ""}>
                <Icon name="save" size={20} />
                <span><b>{p.name}</b><small>{p.instrument}{p.effects.length ? ` + ${p.effects.join("・")}` : ""}</small></span>
              </button>
            {/each}
            <div class="note">音源とエフェクトが丸ごと置き換わります(全プロジェクト共通)</div>
          {/if}
        {:else if tab === "sf2"}
          <div class="row">
            <select value={sfFile} onchange={(e) => pickSf((e.currentTarget as HTMLSelectElement).value)}>
              <option value="">.sf2 を選ぶ…</option>
              {#each sfFiles ?? [] as f (f)}<option value={f}>{f}</option>{/each}
            </select>
            <button class="btn sm" onclick={addSf} title=".sf2 をライブラリフォルダへコピーして追加"><Icon name="plus" />追加</button>
          </div>
          {#if sfBusy}
            <div class="note">プリセットを読み込んでいます…</div>
          {:else if sfFile}
            {#each sfPresets.filter((p) => hit(p.name)) as p (`${p.bank}:${p.preset}`)}
              <button class="item compact" onclick={() => setSf(p.bank, p.preset, p.name)}>
                <code>{p.bank}:{String(p.preset).padStart(3, "0")}</code><span><b>{p.name}</b></span>
              </button>
            {/each}
          {:else if (sfFiles ?? []).length === 0}
            <div class="note">まだ .sf2 がありません。設定 → 表示 →「はじめの確認」の「GM 音源を取得」か、手持ちの .sf2 を「追加」から登録すると、ピアノ・ストリングス等の GM 音源一式が使えます。</div>
          {/if}
        {:else if tab === "clap"}
          <div class="row">
            <span class="note grow">インストール済みの CLAP 音源</span>
            <button class="btn sm" disabled={clapBusy} onclick={() => loadClap(true)} title="探し直す(インストールした後など)"><Icon name="refresh-cw" />探し直す</button>
          </div>
          {#if clapList === null || clapBusy}
            <div class="note">探しています…</div>
          {:else if clapList.length === 0}
            <div class="note">見つかりません。Surge XT などの CLAP 版をインストールしてから「探し直す」を押してください(探す場所: {clapDirs.join(" / ")})</div>
          {:else}
            {#each clapList.filter((p) => hit(p.name) || hit(p.vendor)) as p (p.id)}
              <button class="item" class:sel={track.device?.type === "clap" && track.device.plugin_id === p.id} onclick={() => setClap(p)} title={`${p.id}\n${p.path}`}>
                <Icon name="plug" size={20} />
                <span><b>{p.name}</b><small>{p.vendor} {p.version}</small></span>
              </button>
            {/each}
          {/if}
        {:else}
          <div class="note">音声ファイル(WAV / MP3 など)を取り込み、音程を付けて鳴らすサンプラーにします(ワンショット・声ネタ向け)。</div>
          <button class="btn" onclick={importSample}><Icon name="folder-open" />ファイルを選ぶ…</button>
        {/if}
      </div>
    </div>
    <div class="pk-foot">今: {current}</div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 39;
  }

  .picker {
    position: fixed;
    z-index: 40;
    width: 560px;
    max-width: calc(100vw - 16px);
    height: 420px;
    max-height: calc(100vh - 16px);
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border: 1px solid var(--border-strong);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-pop);
    overflow: hidden;
  }

  .pk-head {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--accent);
  }

  .pk-head b {
    flex: 1;
    color: var(--text);
    font-size: var(--fs-md);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  input[type="search"],
  select {
    height: 26px;
    background: var(--bg-inset);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: var(--fs-sm);
    padding: 0 6px;
  }

  input[type="search"] {
    width: 170px;
  }

  .pk-body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .tabs {
    width: 150px;
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 0;
    background: none;
    text-align: left;
    padding: 6px 8px;
    border-radius: var(--r-sm);
    color: var(--text-dim);
    font-size: var(--fs-md);
  }

  .tab:hover:not(:disabled) {
    background: var(--bg-raised);
    color: var(--text);
    border: 0;
  }

  .tab.sel {
    background: color-mix(in srgb, var(--accent) 14%, transparent);
    color: var(--accent);
  }

  .list {
    flex: 1;
    overflow-y: auto;
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    border: 1px solid transparent;
    background: none;
    text-align: left;
    padding: 6px 10px;
    border-radius: var(--r-md);
    color: var(--text-dim);
  }

  .item:hover:not(:disabled) {
    background: var(--bg-raised);
    border-color: transparent;
    color: var(--text);
  }

  .item.sel {
    border-color: var(--accent-dim);
    background: color-mix(in srgb, var(--accent) 12%, transparent);
    color: var(--accent);
  }

  .item span {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .item b {
    color: var(--text);
    font-size: var(--fs-md);
    font-weight: 600;
  }

  .item small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .item.compact {
    padding: 3px 10px;
  }

  .item code {
    color: var(--text-faint);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px 6px;
  }

  .row select {
    flex: 1;
    min-width: 0;
  }

  .grow {
    flex: 1;
  }

  .note {
    font-size: var(--fs-xs);
    color: var(--text-dim);
    line-height: 1.5;
    padding: 4px 6px;
  }

  .pk-foot {
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }
</style>
