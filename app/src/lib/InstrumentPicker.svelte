<script lang="ts">
  // 音源ピッカー: 内蔵・音色のプリセット・SoundFont・CLAP・サンプル(WAV)を 1 か所から選ぶ。
  // トラックの見出しの音源名と、インスペクターの「変更」から開く(instrumentPickerStore)。
  // どの選び方も Command(set_device など)を通るので Ctrl+Z で戻せる。
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";
  import { BUILTIN_INSTRUMENTS, builtinDevice, deviceName } from "./instruments";
  import { keepInView } from "./menu";
  import { instrumentPickerStore } from "./selection.svelte";
  import { showError, showToast } from "./toast.svelte";
  import { tr } from "./i18n.svelte";
  import type { PresetInfo, Project } from "./types";

  let { project }: { project: Project } = $props();

  type Tab = "builtin" | "preset" | "sf2" | "clap" | "sample";
  const TABS: { key: Tab; readonly label: string; icon: IconName }[] = [
    { key: "builtin", get label() { return tr("内蔵", "Built-in"); }, icon: "audio-waveform" },
    { key: "preset", get label() { return tr("音色のプリセット", "Sound presets"); }, icon: "save" },
    { key: "sf2", get label() { return tr("SoundFont・SFZ", "SoundFont / SFZ"); }, icon: "library" },
    { key: "clap", label: "CLAP", icon: "plug" },
    { key: "sample", get label() { return tr("サンプル(WAV)", "Sample (WAV)"); }, icon: "file-audio" },
  ];

  const open = $derived(instrumentPickerStore.open);
  const track = $derived(open ? (project.tracks.find((t) => t.id === open.trackId) ?? null) : null);
  let tab = $state<Tab>("builtin");
  let query = $state("");

  let presets = $state<PresetInfo[] | null>(null);
  let sfFiles = $state<string[] | null>(null);
  let sfFile = $state("");
  let sfzFiles = $state<string[]>([]);
  let packs = $state<api.SfzPack[]>([]);
  // 取得中の音源(id と進み具合 0〜1)。取得は 1 つずつ
  let packBusy = $state<{ id: string; ratio: number } | null>(null);
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
    tab = open.tab ?? (type === "clap" ? "clap" : type === "sf2" || type === "sfz" ? "sf2" : type === "sampler" ? "sample" : "builtin");
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
      api
        .listSoundfonts()
        .then((r) => {
          sfFiles = r.files;
          sfzFiles = r.sfz ?? [];
          packs = r.packs ?? [];
        })
        .catch(() => (sfFiles = []));
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
      showError(tr("SoundFont を読めませんでした", "Couldn't read the SoundFont"), e);
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
      api.applyEdit(
        [{ op: "set_device", track: t.id, device: builtinDevice(name) }],
        tr(`${t.name} の音源を ${name} に変更`, `Change ${t.name} instrument to ${name}`),
      ),
      tr("音源を変えられませんでした", "Couldn't change the instrument"),
    );
  }

  function applyPreset(name: string) {
    const t = track;
    if (t) run(api.loadPreset(t.id, name), tr("プリセットを読み込めませんでした", "Couldn't load the preset"));
  }

  function setSf(bank: number, preset: number, name: string) {
    const t = track;
    if (!t) return;
    run(
      api.applyEdit(
        [{ op: "set_device", track: t.id, device: { type: "sf2", soundfont: sfFile, bank, preset } }],
        tr(`${t.name} の音源を「${name}」(SoundFont)に変更`, `Change ${t.name} instrument to "${name}" (SoundFont)`),
      ),
      tr("SoundFont にできませんでした", "Couldn't switch to the SoundFont"),
    );
  }

  function setSfz(instrument: string) {
    const t = track;
    if (!t) return;
    if (t.device?.type === "sfz" && t.device.instrument === instrument) return close();
    // カタログの音源なら推奨の調整つまみを付ける
    const cc = packs.find((p) => p.instruments.includes(instrument))?.cc;
    const device = cc && Object.keys(cc).length > 0 ? { type: "sfz", instrument, cc } : { type: "sfz", instrument };
    run(
      api.applyEdit(
        [{ op: "set_device", track: t.id, device }],
        tr(`${t.name} の音源を「${instrument}」(SFZ)に変更`, `Change ${t.name} instrument to "${instrument}" (SFZ)`),
      ),
      tr("SFZ にできませんでした", "Couldn't switch to the SFZ"),
    );
  }

  async function getPack(p: api.SfzPack) {
    if (packBusy) return;
    packBusy = { id: p.id, ratio: 0 };
    const un = await api
      .onSfzDownload((e) => {
        if (packBusy && e.id === packBusy.id && e.total > 0) packBusy = { id: e.id, ratio: e.got / e.total };
      })
      .catch(() => undefined);
    try {
      await api.downloadSfzPack(p.id);
      const r = await api.listSoundfonts();
      sfzFiles = r.sfz ?? [];
      packs = r.packs ?? [];
      showToast("ok", tr(`「${p.name}」を入れました。SFZ の楽器の一覧から選べます`, `Installed "${p.name}". Choose it from the SFZ instrument list`));
    } catch (e) {
      showError(tr(`「${p.name}」を取得できませんでした`, `Couldn't download "${p.name}"`), e);
    } finally {
      un?.();
      packBusy = null;
    }
  }

  async function addSf() {
    const file = await pickFile({ title: tr("SoundFont(.sf2)をライブラリに追加", "Add a SoundFont (.sf2) to the library"), filters: [{ name: "SoundFont", extensions: ["sf2"] }] });
    if (typeof file !== "string") return;
    try {
      const r = await api.addSoundfont(file);
      sfFiles = (await api.listSoundfonts()).files;
      pickSf(r.file);
    } catch (e) {
      showError(tr("SoundFont を追加できませんでした", "Couldn't add the SoundFont"), e);
    }
  }

  function setClap(p: api.ClapPluginInfo) {
    const t = track;
    if (!t) return;
    if (t.device?.type === "clap" && t.device.plugin_id === p.id) return close();
    run(
      api.applyEdit(
        [{ op: "set_device", track: t.id, device: { type: "clap", plugin_id: p.id } }],
        tr(`${t.name} の音源を ${p.name}(CLAP)に変更`, `Change ${t.name} instrument to ${p.name} (CLAP)`),
      ),
      tr("プラグインを読み込めませんでした", "Couldn't load the plugin"),
    );
  }

  async function importSample() {
    const t = track;
    if (!t) return;
    const file = await pickFile({
      title: tr("サンプル(WAV / MP3 等)を読み込む", "Load a sample (WAV, MP3, etc.)"),
      filters: [{ name: tr("音声(WAV / MP3 / FLAC / OGG / M4A)", "Audio (WAV / MP3 / FLAC / OGG / M4A)"), extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
    });
    if (typeof file !== "string") return;
    close();
    try {
      await api.importSample(t.id, file);
      showToast(
        "ok",
        tr(
          "サンプルを設定しました(インスペクターの「元の音程」をサンプルの実音に合わせてください)",
          "Sample set. Match the root note in the inspector to the sample's actual pitch",
        ),
      );
    } catch (e) {
      showError(tr("サンプルを読み込めませんでした", "Couldn't load the sample"), e);
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
  <div class="picker" role="dialog" aria-label={tr("音源を選ぶ", "Choose instrument")} use:keepInView style="left:{open.x}px;top:{open.y}px">
    <div class="pk-head">
      <Icon name="audio-waveform" />
      <b>{tr("音源を選ぶ", "Choose instrument")} — {track.name}</b>
      <input type="search" placeholder={tr("絞り込む", "Filter")} bind:value={query} />
      <button class="btn sm icon ghost" onclick={close} title={tr("閉じる(Esc)", "Close (Esc)")} aria-label={tr("閉じる", "Close")}><Icon name="x" /></button>
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
          <div class="note">{tr("切り替えるとつまみは初期値に戻ります(Ctrl+Z で戻せます)", "Switching resets the parameters to defaults (Ctrl+Z to undo)")}</div>
        {:else if tab === "preset"}
          {#if presets === null}
            <div class="note">{tr("読み込んでいます…", "Loading…")}</div>
          {:else if presets.length === 0}
            <div class="note">
              {tr(
                "まだありません。インスペクターの「音源」の保存ボタンで、今の音を保存できます。",
                "None yet. Save the current sound with the Save button under Instrument in the inspector.",
              )}
            </div>
          {:else}
            {#each presets.filter((p) => hit(p.name) || hit(p.instrument)) as p (p.name)}
              <button class="item" onclick={() => applyPreset(p.name)} title={p.description ?? ""}>
                <Icon name="save" size={20} />
                <span><b>{p.name}</b><small>{p.instrument}{p.effects.length ? ` + ${p.effects.join(tr("・", ", "))}` : ""}</small></span>
              </button>
            {/each}
            <div class="note">{tr("音源とエフェクトが丸ごと置き換わります(全プロジェクト共通)", "Replaces the instrument and all effects (shared by all projects)")}</div>
          {/if}
        {:else if tab === "sf2"}
          <div class="row">
            <select value={sfFile} onchange={(e) => pickSf((e.currentTarget as HTMLSelectElement).value)}>
              <option value="">{tr(".sf2 を選ぶ…", "Choose .sf2…")}</option>
              {#each sfFiles ?? [] as f (f)}<option value={f}>{f}</option>{/each}
            </select>
            <button class="btn sm" onclick={addSf} title={tr(".sf2 をライブラリフォルダへコピーして追加", "Copy a .sf2 into the library folder")}><Icon name="plus" />{tr("追加", "Add")}</button>
          </div>
          {#if sfBusy}
            <div class="note">{tr("プリセットを読み込んでいます…", "Loading presets…")}</div>
          {:else if sfFile}
            {#each sfPresets.filter((p) => hit(p.name)) as p (`${p.bank}:${p.preset}`)}
              <button class="item compact" onclick={() => setSf(p.bank, p.preset, p.name)}>
                <code>{p.bank}:{String(p.preset).padStart(3, "0")}</code><span><b>{p.name}</b></span>
              </button>
            {/each}
          {:else}
            {#if (sfFiles ?? []).length === 0}
              <div class="note">
                {tr(
                  "まだ .sf2 がありません。設定 → 表示 →「はじめの確認」の「GM 音源を取得」か、手持ちの .sf2 を「追加」から登録すると、ピアノ・ストリングス等の GM 音源一式が使えます。",
                  "No .sf2 yet. Use Get GM sounds in Settings → View → First-run check, or register your own .sf2 with Add, to get a full GM set (piano, strings, etc.).",
                )}
              </div>
            {/if}
            {#if sfzFiles.length > 0}
              <div class="note">{tr("SFZ の楽器", "SFZ instruments")}</div>
              {#each sfzFiles.filter((f) => hit(f)) as f (f)}
                <button class="item compact" class:sel={track.device?.type === "sfz" && track.device.instrument === f} onclick={() => setSfz(f)}>
                  <code>SFZ</code><span><b>{f.replace(/\.sfz$/i, "")}</b></span>
                </button>
              {/each}
            {/if}
            {#if packs.some((p) => !p.installed)}
              <div class="note">
                {tr(
                  "無料の音源(取得して使う。CC-BY の音源は、曲を公開するときに作者名を書いてください)",
                  "Free instruments (download to use; credit the author of CC-BY instruments when you publish a song)",
                )}
              </div>
              {#each packs.filter((p) => !p.installed && (hit(p.name) || hit(p.kind))) as p (p.id)}
                <div class="pack">
                  <span class="grow"><b>{p.name}</b><small>{tr(`${p.kind}・${p.author}・${p.license}・約 ${p.approx_mb} MB`, `${p.kind} · ${p.author} · ${p.license} · ~${p.approx_mb} MB`)}</small></span>
                  {#if packBusy?.id === p.id}
                    <span class="pct">{Math.round(packBusy.ratio * 100)}%</span>
                  {:else}
                    <button class="btn sm" disabled={packBusy !== null} onclick={() => getPack(p)} title={tr("sfzinstruments(GitHub)から取得して SFZ ライブラリに入れる", "Download from sfzinstruments (GitHub) into the SFZ library")}><Icon name="download" />{tr("取得", "Get")}</button>
                  {/if}
                </div>
              {/each}
            {/if}
          {/if}
        {:else if tab === "clap"}
          <div class="row">
            <span class="note grow">{tr("インストール済みの CLAP 音源", "Installed CLAP instruments")}</span>
            <button class="btn sm" disabled={clapBusy} onclick={() => loadClap(true)} title={tr("探し直す(インストールした後など)", "Rescan (e.g. after installing)")}
              ><Icon name="refresh-cw" />{tr("探し直す", "Rescan")}</button
            >
          </div>
          {#if clapList === null || clapBusy}
            <div class="note">{tr("探しています…", "Scanning…")}</div>
          {:else if clapList.length === 0}
            <div class="note">
              {tr(
                `見つかりません。Surge XT などの CLAP 版をインストールしてから「探し直す」を押してください(探す場所: ${clapDirs.join(" / ")})`,
                `None found. Install a CLAP version of e.g. Surge XT, then press Rescan (searched: ${clapDirs.join(" / ")})`,
              )}
            </div>
          {:else}
            {#each clapList.filter((p) => hit(p.name) || hit(p.vendor)) as p (p.id)}
              <button class="item" class:sel={track.device?.type === "clap" && track.device.plugin_id === p.id} onclick={() => setClap(p)} title={`${p.id}\n${p.path}`}>
                <Icon name="plug" size={20} />
                <span><b>{p.name}</b><small>{p.vendor} {p.version}</small></span>
              </button>
            {/each}
          {/if}
        {:else}
          <div class="note">
            {tr(
              "音声ファイル(WAV / MP3 など)を取り込み、音程を付けて鳴らすサンプラーにします(ワンショット・声ネタ向け)。",
              "Import an audio file (WAV, MP3, etc.) and play it pitched as a sampler (good for one-shots and vocal chops).",
            )}
          </div>
          <button class="btn" onclick={importSample}><Icon name="folder-open" />{tr("ファイルを選ぶ…", "Choose file…")}</button>
        {/if}
      </div>
    </div>
    <div class="pk-foot">{tr("今", "Current")}: {current}</div>
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

  .pack {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
  }

  .pack span {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .pack b {
    color: var(--text);
    font-size: var(--fs-md);
    font-weight: 600;
  }

  .pack small {
    font-size: var(--fs-xs);
    color: var(--text-dim);
  }

  .pack .pct {
    font-size: var(--fs-sm);
    color: var(--accent);
    font-variant-numeric: tabular-nums;
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
