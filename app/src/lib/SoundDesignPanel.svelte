<script lang="ts">
  // 音作りビュー: トラック 1 本にフォーカスして音源のつまみ・エフェクトチェーン・
  // プリセットを操作するパネル(docs/HANDOFF.md §8-6 Phase 1)。
  // すべての編集は Command API(apply_edit)経由なので履歴に載り undo できる。
  import { open as pickFile } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { newClipId, newFxId } from "./ids";
  import { PHRASE_LEN, PHRASE_NAME, phraseNotes } from "./phrase";
  import { MASTER_FOCUS_ID, soundDesignStore } from "./selection.svelte";
  import type { EffectView, ParamView, PresetInfo, Project, Track, TrackParams } from "./types";

  let { project }: { project: Project } = $props();

  const track = $derived.by((): Track | null => {
    const focus = soundDesignStore.focus;
    if (!focus) return null;
    return project.tracks.find((t) => t.id === focus.trackId) ?? null;
  });

  // ---- CLAP プラグインのプリセット ----
  let clapPresetList = $state<api.ClapPreset[] | null>(null);
  let clapCategories = $state<{ name: string; count: number }[]>([]);
  let clapPresetTrack = $state<string | null>(null);
  let clapPresetBusy = $state(false);
  let clapPresetMsg = $state<string | null>(null);
  let clapCategory = $state("");
  let clapSearch = $state("");
  async function loadClapPresets(trackId: string, rescan = false) {
    clapPresetBusy = true;
    clapPresetMsg = null;
    try {
      const r = await api.clapPresets(trackId, rescan);
      clapPresetList = r.presets;
      clapCategories = r.categories;
      clapPresetTrack = trackId;
    } catch (e) {
      clapPresetList = [];
      clapPresetMsg = String(e);
    } finally {
      clapPresetBusy = false;
    }
  }
  $effect(() => {
    const t = track;
    if (t?.device?.type === "clap" && clapPresetTrack !== t.id && !clapPresetBusy) {
      clapCategory = "";
      clapSearch = "";
      loadClapPresets(t.id);
    }
  });
  const shownClapPresets = $derived.by(() => {
    const q = clapSearch.trim().toLowerCase();
    return (clapPresetList ?? []).filter(
      (p) =>
        (!clapCategory || p.category === clapCategory) &&
        (!q || p.name.toLowerCase().includes(q) || p.category.toLowerCase().includes(q)),
    );
  });
  const currentClapPreset = $derived(
    typeof track?.device?.params?.preset === "string" ? (track.device.params.preset as string) : null,
  );
  async function applyClapPreset(p: api.ClapPreset) {
    if (!track || clapPresetBusy) return;
    clapPresetBusy = true;
    clapPresetMsg = `「${p.name}」を読み込み中…`;
    try {
      await api.clapLoadPreset(track.id, p.id);
      clapPresetMsg = null;
    } catch (e) {
      clapPresetMsg = String(e);
    } finally {
      clapPresetBusy = false;
    }
  }

  /// マスターバスを開いている(エフェクトチェーンだけを扱う)
  const isMaster = $derived(soundDesignStore.focus?.trackId === MASTER_FOCUS_ID);
  /// 履歴ラベル用の対象名
  const targetName = $derived(isMaster ? "マスター" : (track?.name ?? ""));

  // トラックが消えたら閉じる
  $effect(() => {
    if (soundDesignStore.focus && !track && !isMaster) {
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
    const master = isMaster;
    void project; // 依存: どの編集でも現在値を取り直す
    if (!t && !master) {
      info = null;
      return;
    }
    (master ? api.getMasterParams() : api.getTrackParams(t!.id))
      .then((r) => {
        info = r;
        loadError = null;
      })
      .catch((e) => (loadError = String(e)));
  });

  function setLegato(name: "glide_ms" | "legato_ms", value: number) {
    const t = track;
    if (!t) return;
    const what = name === "glide_ms" ? "ポルタメントの滑る時間" : "レガートのつなぎ目";
    applyEdit([{ op: "set_param", track: t.id, path: `track/${name}`, value }], `${t.name} の${what}を ${value}ms に`);
  }

  function resetLegato() {
    const t = track;
    if (!t) return;
    const cmds = (["glide_ms", "legato_ms"] as const)
      .filter((n) => t[n] !== undefined)
      .map((n) => ({ op: "unset_param", track: t.id, path: `track/${n}` }));
    if (cmds.length > 0) applyEdit(cmds, `${t.name} のつなぎ方を既定に戻す`);
  }

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
    if (!t && !isMaster) return;
    let value: unknown = raw;
    if (p.range.kind === "float") value = Number(raw);
    if (p.range.kind === "int") value = Math.round(Number(raw));
    if (p.range.kind === "bool") value = Boolean(raw);
    applyEdit(
      [
        isMaster
          ? { op: "set_master_param", path: p.path, value }
          : { op: "set_param", track: t!.id, path: p.path, value },
      ],
      `${targetName} の ${p.display_name} を変更`,
    );
  }

  function fmtValue(p: ParamView): string {
    // CLAP プラグインのつまみはプラグイン自身の表示を優先
    if (p.current_text) return p.current_text;
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
    if (!t || name === "sampler" || t.device?.name === name) return;
    applyEdit(
      [{ op: "set_device", track: t.id, device: { type: "builtin", name } }],
      `${t.name} の音源を ${name} に変更`,
    );
  }

  /// WAV を選んでこのトラックの音源を sampler にする
  async function importSample() {
    const t = track;
    if (!t) return;
    const file = await pickFile({
      title: "サンプル(WAV / MP3 等)を読み込む",
      filters: [{ name: "音声(WAV / MP3 / FLAC / OGG / M4A)", extensions: ["wav", "mp3", "flac", "ogg", "m4a", "aac"] }],
    });
    if (typeof file !== "string") return;
    try {
      await api.importSample(t.id, file);
      presetMsg = "サンプルを設定しました(root にサンプルの実音を合わせてください)";
    } catch (e) {
      presetMsg = String(e);
    }
  }

  /// `name` は内蔵エフェクト名か "clap:<plugin_id>"(CLAP プラグイン)
  function addEffect(name: string) {
    const t = track;
    if ((!t && !isMaster) || !name) return;
    const clapId = name.startsWith("clap:") ? name.slice(5) : null;
    const effect = clapId
      ? { id: newFxId(), type: "clap", plugin_id: clapId }
      : { id: newFxId(), type: "builtin", name };
    const label = clapId ? (clapEffects.find((p) => p.id === clapId)?.name ?? clapId) : name;
    applyEdit(
      [isMaster ? { op: "add_master_effect", effect } : { op: "add_effect", track: t!.id, effect }],
      `${targetName} に ${label} を追加`,
    );
  }

  // ---- センド / バス ----

  const buses = $derived(project.tracks.filter((t) => t.kind === "bus"));
  /// バスを開いているとき: 送ってきているトラック
  const busSenders = $derived.by(() => {
    const t = track;
    if (!t || t.kind !== "bus") return [];
    return project.tracks.flatMap((src) =>
      (src.sends ?? [])
        .filter((s) => s.target === t.id)
        .map((s) => ({ name: src.name, level_db: s.level_db, pre: s.pre_fader ?? false })),
    );
  });

  function setSend(bus: Track, levelDb: number, preFader: boolean) {
    const t = track;
    if (!t) return;
    applyEdit(
      [{ op: "set_send", track: t.id, target: bus.id, level_db: levelDb, pre_fader: preFader }],
      `${t.name} から ${bus.name} へのセンドを ${levelDb.toFixed(1)} dB に`,
    );
  }

  function removeSend(bus: Track) {
    const t = track;
    if (!t) return;
    applyEdit([{ op: "set_send", track: t.id, target: bus.id }], `${t.name} から ${bus.name} へのセンドを外す`);
  }

  // ---- CLAP エフェクト ----

  /// インストール済みの CLAP エフェクト(音作りビューを開いたときに一度読む)
  let clapEffects = $state<api.ClapPluginInfo[]>([]);
  let clapEffectsLoaded = false;
  $effect(() => {
    if (soundDesignStore.focus && !clapEffectsLoaded) {
      clapEffectsLoaded = true;
      api
        .clapPlugins()
        .then((r) => (clapEffects = r.plugins.filter((p) => p.effect)))
        .catch(() => {});
    }
  });

  /// エフェクトの表示名(CLAP はプラグイン名)
  function fxLabel(fx: EffectView): string {
    if (fx.name !== "clap") return fx.name;
    return fx.plugin_name ?? fx.plugin_id ?? "CLAP";
  }

  function openFxGui(fx: EffectView) {
    api.clapOpenGui(null, fx.id).catch((e) => alert(String(e)));
  }

  // CLAP エフェクトのプリセット(開いているエフェクト 1 つ分)
  let fxPresetOpen = $state<string | null>(null);
  let fxPresetList = $state<api.ClapPreset[] | null>(null);
  let fxPresetMsg = $state<string | null>(null);
  let fxPresetBusy = $state(false);
  async function toggleFxPresets(fx: EffectView) {
    if (fxPresetOpen === fx.id) {
      fxPresetOpen = null;
      return;
    }
    fxPresetOpen = fx.id;
    fxPresetList = null;
    fxPresetMsg = null;
    try {
      const r = await api.clapPresets(null, false, fx.id);
      fxPresetList = r.presets;
      fxPresetMsg = r.current_preset ? `今: ${r.current_preset}` : null;
    } catch (e) {
      fxPresetList = [];
      fxPresetMsg = String(e);
    }
  }
  /// カテゴリごとにまとめた一覧(select の optgroup 用)
  const fxPresetGroups = $derived.by(() => {
    const groups = new Map<string, api.ClapPreset[]>();
    for (const p of (fxPresetList ?? []).slice(0, 1000)) {
      const g = groups.get(p.category) ?? [];
      g.push(p);
      groups.set(p.category, g);
    }
    return [...groups.entries()];
  });
  async function loadFxPreset(fx: EffectView, presetId: string) {
    if (!presetId || fxPresetBusy) return;
    fxPresetBusy = true;
    fxPresetMsg = "読み込み中…";
    try {
      const r = await api.clapLoadPreset(null, presetId, fx.id);
      fxPresetMsg = `今: ${r.preset}(Ctrl+Z で戻せます)`;
    } catch (e) {
      fxPresetMsg = String(e);
    } finally {
      fxPresetBusy = false;
    }
  }

  function removeEffect(id: string, name: string) {
    applyEdit([{ op: "remove_effect", id }], `${targetName} の ${name} を削除`);
  }

  function toggleBypass(id: string, name: string, bypass: boolean) {
    applyEdit(
      [{ op: "set_effect_bypass", id, bypass: !bypass }],
      `${targetName} の ${name} を${bypass ? "有効化" : "バイパス"}`,
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

  // ---- SoundFont ----

  let sfFiles = $state<string[]>([]);
  let sfFile = $state("");
  let sfPresets = $state<{ bank: number; preset: number; name: string }[]>([]);
  let sfSelected = $state(""); // "bank:preset"
  let sfMsg = $state<string | null>(null);

  async function refreshSoundfonts() {
    try {
      sfFiles = (await api.listSoundfonts()).files;
    } catch {
      sfFiles = [];
    }
  }

  $effect(() => {
    if (soundDesignStore.focus) refreshSoundfonts();
  });

  async function pickSfFile(file: string) {
    sfFile = file;
    sfPresets = [];
    sfSelected = "";
    if (!file) return;
    sfMsg = "プリセットを読み込み中…";
    try {
      sfPresets = (await api.listSoundfontPresets(file)).presets;
      sfMsg = null;
    } catch (e) {
      sfMsg = String(e);
    }
  }

  function applySoundfont() {
    const t = track;
    if (!t || !sfFile || !sfSelected) return;
    const [bank, preset] = sfSelected.split(":").map(Number);
    const name = sfPresets.find((p) => p.bank === bank && p.preset === preset)?.name ?? sfFile;
    applyEdit(
      [
        {
          op: "set_device",
          track: t.id,
          device: { type: "sf2", soundfont: sfFile, bank, preset },
        },
      ],
      `${t.name} の音源を「${name}」(SoundFont)に変更`,
    );
  }

  async function addSoundfontFile() {
    const file = await pickFile({
      title: "SoundFont(.sf2)をライブラリに追加",
      filters: [{ name: "SoundFont", extensions: ["sf2"] }],
    });
    if (typeof file !== "string") return;
    sfMsg = "コピー中…";
    try {
      const r = await api.addSoundfont(file);
      sfMsg = `追加しました: ${r.file}`;
      await refreshSoundfonts();
      pickSfFile(r.file);
    } catch (e) {
      sfMsg = String(e);
    }
  }

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

{#if track || isMaster}
  <div class="sd-panel">
    <div class="sd-head">
      <span class="sd-title">🎛 {isMaster ? "マスター" : track?.name}</span>
      <span class="sd-sub">{isMaster ? "マスターバスのエフェクト(全体に掛かる)" : "音作りビュー"}</span>
      <button class="sd-close" onclick={close} title="閉じる">✕</button>
    </div>

    {#if loadError}
      <div class="sd-error">{loadError}</div>
    {/if}

    <div class="sd-body">
      {#if track && track.kind === "bus"}
        <div class="sec">
          <div class="sec-title">🔀 バス</div>
          <div class="hint">
            各トラックの音作りビューの「センド」で、このバスへ送る量を決めます。
            ここに挿したエフェクト(リバーブ・ディレイ等)を複数のトラックで共有できます。
          </div>
          {#if busSenders.length > 0}
            <div class="hint">
              受けているトラック: {busSenders.map((s) => `${s.name}(${s.level_db.toFixed(1)} dB${s.pre ? "・フェーダー前" : ""})`).join("、")}
            </div>
          {:else}
            <div class="hint">まだどのトラックからも送られていません。</div>
          {/if}
        </div>
      {/if}
      {#if track && track.kind !== "bus"}
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
            value={track.device?.type === "clap" ? "clap" : (info?.device.name ?? track.device?.name ?? "subtractive")}
            onchange={(e) => setDevice((e.currentTarget as HTMLSelectElement).value)}
            title="切り替えるとパラメータは初期値に戻ります(Ctrl+Z 可)"
          >
            <option value="subtractive">subtractive(シンセ)</option>
            <option value="drum">drum(ドラム)</option>
            <option value="pluck">pluck(撥弦: ギター/ベース)</option>
            <option value="fm">fm(FM: エレピ/ベル/マレット)</option>
            <option value="wavetable">wavetable(ウェーブテーブル: うねり/母音)</option>
            <option value="sampler" disabled>sampler(下の読込ボタンから)</option>
            <option value="sf2" disabled>sf2(下の SoundFont から)</option>
            <option value="clap" disabled>CLAP プラグイン(トラックの音源メニューから)</option>
          </select>
          <button onclick={importSample} title="WAV をプロジェクトに取り込み、この音源を sampler にする">
            🎼 WAV
          </button>
          <select bind:value={selectedPreset} title="プリセット(全プロジェクト共通)">
            <option value="">プリセット…</option>
            {#each presets as p (p.name)}
              <option value={p.name}>{p.name}({p.instrument}{p.effects.length ? `+FX${p.effects.length}` : ""})</option>
            {/each}
          </select>
          <button disabled={!selectedPreset} onclick={() => applyPreset(selectedPreset)}>適用</button>
        </div>
        {#if track.device?.type !== "clap"}
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
        {/if}
        {#if presetMsg}<div class="hint">{presetMsg}</div>{/if}
      </div>

      <!-- SoundFont -->
      {#if track.device?.type !== "clap"}
      <div class="sec">
        <div class="sec-title">SoundFont(本物っぽい楽器一式)</div>
        <div class="row gap">
          <select
            class="grow"
            value={sfFile}
            onchange={(e) => pickSfFile((e.currentTarget as HTMLSelectElement).value)}
          >
            <option value="">.sf2 を選択…</option>
            {#each sfFiles as f (f)}
              <option value={f}>{f}</option>
            {/each}
          </select>
          <button onclick={addSoundfontFile} title=".sf2 をライブラリフォルダへコピーして追加">
            + 追加
          </button>
        </div>
        {#if sfPresets.length > 0}
          <div class="row gap">
            <select class="grow" bind:value={sfSelected}>
              <option value="">プリセットを選択…</option>
              {#each sfPresets as p (`${p.bank}:${p.preset}`)}
                <option value={`${p.bank}:${p.preset}`}>
                  {p.bank}:{String(p.preset).padStart(3, "0")} {p.name}
                </option>
              {/each}
            </select>
            <button disabled={!sfSelected} onclick={applySoundfont}>適用</button>
          </div>
        {/if}
        {#if sfMsg}<div class="hint">{sfMsg}</div>{/if}
        {#if sfFiles.length === 0}
          <div class="hint">
            まだ .sf2 がありません。FluidR3_GM などのフリー SoundFont をダウンロードして
            「+ 追加」から登録すると、ピアノ・ストリングス等の GM 音源一式が使えます。
          </div>
        {/if}
      </div>
      {/if}

      {/if}
      <!-- CLAP プラグイン: 音作りはプラグイン自身の画面で -->
      {#if track?.device?.type === "clap"}
        <div class="sec">
          <div class="sec-title">🔌 CLAP プラグイン</div>
          <div class="hint">{track.device.plugin_id}</div>
          <div class="row gap">
            <button onclick={() => api.clapOpenGui(track!.id).catch((e) => alert(String(e)))}>
              🖥 プラグインの画面を開く
            </button>
            <button
              onclick={() => api.clapSaveState(track!.id).catch((e) => alert(String(e)))}
              title="プラグインの今の設定をプロジェクトに保存する(画面を閉じたときや操作の後にも自動で保存されます)"
            >
              💾 設定を保存
            </button>
          </div>
          <div class="sec-title sub">
            プリセット{currentClapPreset ? `(今: ${currentClapPreset})` : ""}
            <button
              class="mini"
              disabled={clapPresetBusy}
              title="プリセットを探し直す(追加した後など)"
              onclick={() => track && loadClapPresets(track.id, true)}>🔄</button
            >
          </div>
          {#if clapPresetList === null}
            <div class="hint">プリセットを探しています…</div>
          {:else if clapPresetList.length === 0}
            <div class="hint">
              このプラグインはプリセットを Glaux に公開していません。プラグインの画面のプリセットメニューから選んでください
              (選んだ設定は自動でプロジェクトに保存され、Ctrl+Z で戻せます)。
            </div>
          {:else}
            <div class="row gap">
              <select bind:value={clapCategory} title="カテゴリ(フォルダ)">
                <option value="">すべて({clapPresetList.length})</option>
                {#each clapCategories as c (c.name)}
                  <option value={c.name}>{c.name || "(未分類)"}({c.count})</option>
                {/each}
              </select>
              <input class="grow" type="search" placeholder="名前で絞り込み" bind:value={clapSearch} />
            </div>
            <div class="preset-list">
              {#each shownClapPresets.slice(0, 500) as p (p.id)}
                <button
                  class="preset-item"
                  class:current={p.name === currentClapPreset}
                  disabled={clapPresetBusy}
                  title={`${p.collection}\n${p.category}${p.creators.length ? "\nby " + p.creators.join(", ") : ""}`}
                  onclick={() => applyClapPreset(p)}
                >
                  <span class="pi-name">{p.name}</span>
                  <span class="pi-cat">{p.category}</span>
                </button>
              {/each}
              {#if shownClapPresets.length > 500}
                <div class="hint">…ほか {shownClapPresets.length - 500} 件(絞り込んでください)</div>
              {/if}
            </div>
          {/if}
          {#if clapPresetMsg}<div class="hint">{clapPresetMsg}</div>{/if}
          <div class="hint">
            プリセットを選ぶと音色が丸ごと入れ替わります(Ctrl+Z で戻せます)。
            音色はプラグインの画面でも作れます。画面での変更は自動でプロジェクトに保存され、Ctrl+Z で戻せます。
            エフェクト・音量・パン・オートメーション(音量・パン・エフェクト)は Glaux 側でも使えます。
          </div>
        </div>
      {/if}

      <!-- 音源パラメータ -->
      {#if info && track && track.kind !== "bus" && track.device?.type !== "clap"}
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
      {/if}
      {#if info}
        <!-- エフェクトチェーン(トラック・マスター共通) -->
        <div class="sec">
          <div class="sec-title">エフェクト({info.effects.length})</div>
          {#each info.effects as fx (fx.id)}
            <div class="fx" class:bypassed={fx.bypass}>
              <div class="fx-head">
                <span class="fx-name" title={fx.plugin_id ?? ""}>
                  {#if fx.name === "clap"}<span class="clap-chip">CLAP</span>{/if}{fxLabel(fx)}
                </span>
                {#if fx.name === "clap" && !fx.missing}
                  <button
                    class="mini"
                    class:on={fxPresetOpen === fx.id}
                    onclick={() => toggleFxPresets(fx)}
                    title="プラグインのプリセットから選ぶ"
                  >
                    プリセット
                  </button>
                  <button class="mini" onclick={() => openFxGui(fx)} title="プラグインの画面を開く(変更は自動で保存され、Ctrl+Z で戻せます)">
                    画面
                  </button>
                {/if}
                <button
                  class="mini"
                  class:on={!fx.bypass}
                  onclick={() => toggleBypass(fx.id, fxLabel(fx), fx.bypass)}
                  title={fx.bypass ? "バイパス中(クリックで有効化)" : "有効(クリックでバイパス)"}
                >
                  {fx.bypass ? "OFF" : "ON"}
                </button>
                <button class="mini danger" onclick={() => removeEffect(fx.id, fxLabel(fx))} title="削除(Ctrl+Z 可)">
                  ✕
                </button>
              </div>
              {#if fxPresetOpen === fx.id}
                <div class="fx-presets">
                  {#if fxPresetList === null}
                    <div class="hint">プリセットを探しています…</div>
                  {:else if fxPresetList.length === 0}
                    <div class="hint">
                      このプラグインはプリセットを Glaux に公開していません(例: Surge XT Effects)。
                      「画面」を開き、プラグイン自身のプリセットメニューから選んでください。
                      選んだ設定は自動でプロジェクトに保存され、Ctrl+Z で戻せます。
                    </div>
                  {:else}
                    <select
                      class="grow"
                      disabled={fxPresetBusy}
                      onchange={(e) => loadFxPreset(fx, (e.currentTarget as HTMLSelectElement).value)}
                    >
                      <option value="">プリセットを選ぶ…({fxPresetList.length})</option>
                      {#each fxPresetGroups as [cat, list] (cat)}
                        <optgroup label={cat || "(未分類)"}>
                          {#each list as p (p.id)}
                            <option value={p.id}>{p.name}</option>
                          {/each}
                        </optgroup>
                      {/each}
                    </select>
                  {/if}
                  {#if fxPresetMsg}<div class="hint">{fxPresetMsg}</div>{/if}
                </div>
              {/if}
              {#if fx.missing}
                <div class="hint warn">
                  このプラグインはこの PC に見つかりません(音は素通し)。インストールすると元の設定で鳴ります。
                </div>
              {/if}
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
              {#if fx.name === "clap" && (fx.param_total ?? 0) > fx.params.length}
                <div class="hint">ほか {(fx.param_total ?? 0) - fx.params.length} 個のつまみはプラグインの画面か AI から操作できます。</div>
              {/if}
            </div>
          {/each}
          <div class="row gap">
            <select bind:value={addFxName} class="grow">
              <option value="">エフェクトを追加…</option>
              <optgroup label="内蔵">
                {#each info.available_effects as fx (fx.name)}
                  <option value={fx.name} title={fx.description}>{fx.name}</option>
                {/each}
              </optgroup>
              {#if clapEffects.length > 0}
                <optgroup label="CLAP プラグイン">
                  {#each clapEffects as p (p.id)}
                    <option value={`clap:${p.id}`} title={`${p.vendor} ${p.version}`}>{p.name}</option>
                  {/each}
                </optgroup>
              {/if}
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

      {#if track && track.kind !== "bus"}
        {#if track.kind === "midi"}
          <!-- レガート / ポルタメントのつなぎ方(奏法 T / P のノートに効く) -->
          <div class="sec">
            <div class="sec-title">⌒ つなぎ(レガート / ポルタメント)</div>
            <div class="param" title="ポルタメント(P)のノートが直前の音から滑る時間。ゆったりした弦は 250〜400、速いリードは 50〜80">
              <span class="p-name">滑る時間</span>
              <input
                type="range"
                min="10"
                max="1000"
                step="10"
                value={track.glide_ms ?? 150}
                onchange={(e) => setLegato("glide_ms", Number((e.currentTarget as HTMLInputElement).value))}
              />
              <span class="p-val">{track.glide_ms ?? 150}ms{track.glide_ms === undefined ? "(既定)" : ""}</span>
            </div>
            <div class="param" title="レガート(T)・ポルタメント(P)で前の音と入れ替わる長さ。長いほどふんわり重なる">
              <span class="p-name">つなぎ目</span>
              <input
                type="range"
                min="5"
                max="200"
                step="5"
                value={track.legato_ms ?? 30}
                onchange={(e) => setLegato("legato_ms", Number((e.currentTarget as HTMLInputElement).value))}
              />
              <span class="p-val">{track.legato_ms ?? 30}ms{track.legato_ms === undefined ? "(既定)" : ""}</span>
            </div>
            {#if track.glide_ms !== undefined || track.legato_ms !== undefined}
              <div class="row gap">
                <button class="mini" onclick={resetLegato}>既定に戻す</button>
              </div>
            {/if}
            <div class="hint">ピアノロールでノートに T(レガート)/ P(ポルタメント)を付けたときのつながり方です。1 音だけ滑る時間を変えるのはピアノロールで。</div>
          </div>
        {/if}

        <!-- センド(バスへ送る量) -->
        <div class="sec">
          <div class="sec-title">🔀 センド</div>
          {#if buses.length === 0}
            <div class="hint">
              バスがありません。タイムライン下の「+ 🔀 バス」でリバーブ付きのバスを作ると、
              複数のトラックで同じリバーブを共有できます。
            </div>
          {:else}
            {#each buses as bus (bus.id)}
              {@const snd = track.sends?.find((s) => s.target === bus.id)}
              <div class="param" title="このトラックの音をバスへ送る量。フェーダー後はトラックの音量に追従します">
                <span class="p-name">{bus.name}</span>
                <input
                  type="range"
                  min="-60"
                  max="6"
                  step="0.5"
                  value={snd?.level_db ?? -60}
                  onchange={(e) => setSend(bus, Number((e.currentTarget as HTMLInputElement).value), snd?.pre_fader ?? false)}
                />
                <span class="p-val">{snd ? `${snd.level_db.toFixed(1)}dB` : "送らない"}</span>
              </div>
              {#if snd}
                <div class="row gap send-opts">
                  <label class="mini-label">
                    <input
                      type="checkbox"
                      checked={snd.pre_fader ?? false}
                      onchange={(e) => setSend(bus, snd.level_db, (e.currentTarget as HTMLInputElement).checked)}
                    />
                    フェーダー前
                  </label>
                  <button class="mini" onclick={() => removeSend(bus)}>送らない</button>
                </div>
              {/if}
            {/each}
          {/if}
        </div>
      {/if}

      <div class="hint" class:hidden-hint={track?.device?.type === "clap"}>
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

  .fx-presets {
    margin: 4px 0 6px 0;
  }

  .fx-presets select {
    width: 100%;
  }

  .send-opts {
    margin: -2px 0 6px 0;
    padding-left: 4px;
  }

  .mini-label {
    font-size: 11px;
    color: var(--text-dim);
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .hint.warn {
    color: #e8a07c;
  }

  .clap-chip {
    font-size: 9px;
    padding: 0 4px;
    margin-right: 4px;
    border: 1px solid var(--accent-dim);
    border-radius: 3px;
    color: var(--accent);
    vertical-align: 1px;
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
  .sec-title.sub {
    margin-top: 8px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .preset-list {
    max-height: 220px;
    overflow-y: auto;
    margin-top: 4px;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .preset-item {
    display: flex;
    width: 100%;
    justify-content: space-between;
    gap: 8px;
    border: none;
    border-radius: 0;
    background: none;
    padding: 3px 8px;
    font-size: 11px;
    text-align: left;
    cursor: pointer;
  }

  .preset-item:hover {
    background: var(--bg-lane-alt);
  }

  .preset-item.current {
    color: var(--accent);
  }

  .pi-cat {
    color: var(--text-dim);
    font-size: 10px;
    white-space: nowrap;
  }

  .hidden-hint {
    display: none;
  }
</style>
