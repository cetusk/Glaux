<script lang="ts">
  import { folderName } from "./folderName";
  import Icon from "./Icon.svelte";
  import { open as pickFolder } from "@tauri-apps/plugin-dialog";
  import * as api from "./api";
  import { chatStatus } from "./aiStatus.svelte";
  import { tr } from "./i18n.svelte";
  import { newClipId, newTrackId } from "./ids";
  import { builtinDevice } from "./instruments";
  import { PHRASE_LEN, PHRASE_NAME, phraseNotes } from "./phrase";
  import { selectionStore, soundDesignStore } from "./selection.svelte";
  import type { RecentProject } from "./types";

  let { title }: { title: string } = $props();

  let openMenu = $state(false);
  let recent = $state<RecentProject[]>([]);
  let defaultDir = $state("");
  let parentDir = $state("");
  let newName = $state("");
  let busy = $state(false);
  let menuError = $state<string | null>(null);
  // 「フォルダを選択して開く」で曲をまとめたフォルダを選んだとき: 中の曲の一覧 / 曲が無いフォルダ
  let candidates = $state<{ path: string; title: string }[] | null>(null);
  let candidatesDir = $state("");
  let emptyDir = $state<string | null>(null);
  let nameInput: HTMLInputElement | undefined = $state();

  // 現在のプロジェクトの移動 / 名前変更
  let curParent = $state("");
  let curStem = $state("");
  let moveName = $state("");
  let moveParent = $state("");
  let curPath = $state("");

  // 作られるフォルダの下見(同じ名前があると -2 が付く・曲のフォルダの中には作れない、を先に見せる)
  let newPreview = $state<api.ProjectDirPreview | null>(null);
  let movePreview = $state<api.ProjectDirPreview | null>(null);
  $effect(() => {
    const name = newName.trim();
    const parent = soundLab ? soundLabDir() : parentDir || defaultDir;
    if (!openMenu || !name) {
      newPreview = null;
      return;
    }
    // 打ち直している間に前の下見の応答が後から届いても、古い名前の結果で上書きしない
    let stale = false;
    const t = setTimeout(() => {
      api
        .previewProjectDir(parent, name)
        .then((p) => {
          if (!stale) newPreview = p;
        })
        .catch(() => {
          if (!stale) newPreview = null;
        });
    }, 150);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  });
  $effect(() => {
    const name = moveName.trim() || title;
    const parent = moveParent;
    if (!openMenu || !moveDirty || !parent) {
      movePreview = null;
      return;
    }
    let stale = false;
    const t = setTimeout(() => {
      api
        .previewProjectDir(parent, name, curPath)
        .then((p) => {
          if (!stale) movePreview = p;
        })
        .catch(() => {
          if (!stale) movePreview = null;
        });
    }, 150);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  });

  function splitProjectPath(path: string) {
    const parts = path.split(/[\\/]/).filter((p) => p.length > 0);
    const folder = parts.pop() ?? "";
    const sep = path.includes("\\") ? "\\" : "/";
    const parent = path.slice(0, path.length - folder.length).replace(/[\\/]+$/, "") || sep;
    return { parent, stem: folder.replace(/\.glaux$/i, "") };
  }

  // ドロップダウンは画面基準(fixed)で出す。ヘッダーは横スクロールのため overflow が
  // 付いており、absolute だとヘッダーの枠で切り取られて裏に隠れる
  let titleBtn: HTMLButtonElement | undefined = $state();
  let menuPos = $state({ left: 0, top: 0 });

  async function toggle() {
    if (!openMenu && titleBtn) {
      const r = titleBtn.getBoundingClientRect();
      menuPos = { left: Math.max(8, r.left), top: r.bottom + 6 };
    }
    openMenu = !openMenu;
    menuError = null;
    candidates = null;
    emptyDir = null;
    if (openMenu) {
      try {
        const [r, info] = await Promise.all([api.listRecentProjects(), api.appInfo()]);
        recent = r.recent;
        defaultDir = r.default_dir;
        if (!parentDir) parentDir = r.default_dir;
        const cur = splitProjectPath(info.project_dir);
        curPath = info.project_dir;
        curParent = cur.parent;
        curStem = cur.stem;
        moveName = title;
        moveParent = cur.parent;
      } catch (e) {
        menuError = String(e);
      }
    }
  }

  async function browseMoveParent() {
    const dir = await pickFolder({ directory: true, title: tr("プロジェクトの移動先フォルダ", "Destination folder for the project") });
    if (typeof dir === "string") moveParent = dir;
  }

  const moveDirty = $derived(
    (moveName.trim() !== "" && moveName.trim() !== title) || moveParent !== curParent,
  );

  async function applyMove() {
    if (busy || !moveDirty || movePreview?.error) return;
    busy = true;
    menuError = null;
    try {
      await api.moveProject(
        moveParent !== curParent ? moveParent : null,
        moveName.trim() !== title ? moveName.trim() : null,
      );
      // 会話・履歴はフォルダごと移動するので継続。メニューを閉じるだけでよい
      openMenu = false;
    } catch (e) {
      menuError = String(e);
    } finally {
      busy = false;
    }
  }

  function afterSwitch() {
    // チャットパネルに会話表示のクリアを促す(プロジェクトごとに会話は別)
    chatStatus.epoch += 1;
    selectionStore.range = null;
    openMenu = false;
    busy = false;
    newName = "";
  }

  async function openPath(path: string) {
    if (busy) return;
    busy = true;
    menuError = null;
    try {
      await api.openProject(path);
      afterSwitch();
    } catch (e) {
      menuError = String(e);
      busy = false;
    }
  }

  /// 曲のフォルダ(○○.glaux)を選べばそれを開く。曲をまとめたフォルダ(ゲームの songs など)を選んだら、
  /// 中の曲が 1 つならそれを開き、複数なら一覧から選ぶ。曲が無ければそこに新しい曲を作れるようにする
  async function browseAndOpen() {
    const dir = await pickFolder({
      directory: true,
      title: tr(
        "曲のフォルダ(○○.glaux)か、曲をまとめたフォルダを選ぶ",
        "Choose a song folder (*.glaux) or a folder containing songs",
      ),
    });
    if (typeof dir !== "string") return;
    candidates = null;
    emptyDir = null;
    menuError = null;
    try {
      const r = await api.findProjects(dir);
      if (r.projects.length === 1) {
        await openPath(r.projects[0].path);
      } else if (r.projects.length > 1) {
        candidates = r.projects;
        candidatesDir = dir;
      } else {
        emptyDir = dir;
      }
    } catch (e) {
      menuError = String(e);
    }
  }

  /// 曲の無いフォルダを、新しい曲の作成先にする(既定の作業フォルダは変えない)
  function createHere() {
    if (!emptyDir) return;
    parentDir = emptyDir;
    soundLab = false;
    emptyDir = null;
    nameInput?.focus();
  }

  async function browseParentDir() {
    const dir = await pickFolder({
      directory: true,
      title: tr("作業フォルダ(新規プロジェクトの作成先)", "Working folder (where new projects are created)"),
    });
    if (typeof dir === "string") {
      parentDir = dir;
      // 次回起動後も同じ場所を使えるよう既定として保存する
      try {
        await api.setProjectsDir(dir);
        defaultDir = dir;
      } catch (e) {
        menuError = String(e);
      }
    }
  }

  let soundLab = $state(false);

  /// 作業フォルダ配下の SoundLab サブフォルダ(音作りプロジェクトの置き場)
  function soundLabDir(): string {
    const base = (parentDir || defaultDir).replace(/[\\/]+$/, "");
    const sep = base.includes("\\") ? "\\" : "/";
    return `${base}${sep}SoundLab`;
  }

  async function createNew() {
    if (busy || !newName.trim() || newPreview?.error) return;
    busy = true;
    menuError = null;
    try {
      if (soundLab) {
        // 音作りテンプレート: SoundLab/ 配下に作成 → トラック + 試聴フレーズ →
        // ループ ON → 音作りビューを開いた状態にする
        await api.createProject(soundLabDir(), newName);
        const trackId = newTrackId();
        await api.applyEdit(
          [
            {
              op: "add_track",
              track: {
                id: trackId,
                name: "Sound",
                kind: "midi",
                device: builtinDevice("subtractive"),
              },
            },
            {
              op: "add_clip",
              track: trackId,
              clip: {
                id: newClipId(),
                name: PHRASE_NAME,
                start: 0,
                length: PHRASE_LEN,
                kind: "midi",
                notes: phraseNotes(),
              },
            },
          ],
          tr("音作りテンプレートを作成", "Create sound design template"),
        );
        soundDesignStore.focus = { trackId, trackName: "Sound" };
        // ループ ON(オーディオデバイスが無い環境では黙って諦める)
        api.transportSetLoop(0, PHRASE_LEN).catch(() => {});
      } else {
        await api.createProject(parentDir, newName);
      }
      afterSwitch();
    } catch (e) {
      menuError = String(e);
      busy = false;
    }
  }

  function onNameKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      createNew();
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && openMenu && (openMenu = false)} />

<div class="menu-root">
  <button class="title-btn" bind:this={titleBtn} onclick={toggle} title={tr(`${title} — プロジェクトを切り替える`, `${title} — switch project`)}>
    <span class="title-text">{title}</span>
    <span class="chev"><Icon name="chevron-down" size={14} /></span>
  </button>

  {#if openMenu}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div class="backdrop" role="presentation" onclick={() => (openMenu = false)}></div>
    <div class="dropdown" style="left:{menuPos.left}px;top:{menuPos.top}px">
      {#if menuError}
        <div class="menu-error">{menuError}</div>
      {/if}

      <div class="section">
        <div class="section-title">{tr("新規プロジェクト", "New project")}</div>
        <div class="new-row">
          <input
            type="text"
            placeholder={tr("曲名", "Song title")}
            bind:this={nameInput}
            bind:value={newName}
            onkeydown={onNameKeydown}
            disabled={busy}
          />
          <button onclick={createNew} disabled={busy || !newName.trim() || !!newPreview?.error}>{tr("作成", "Create")}</button>
        </div>
        <div class="loc-row">
          <span class="loc-label">{tr("場所", "Location")}</span>
          <span class="loc-path" title={soundLab ? soundLabDir() : parentDir || defaultDir}
            >{soundLab ? soundLabDir() : parentDir || defaultDir}</span
          >
          <button
            class="loc-btn"
            onclick={browseParentDir}
            disabled={busy}
            title={tr("新しい曲を作る場所を選ぶ(次からもこの場所が既定になります)", "Choose where new songs are created (it becomes the default from now on)")}
            ><Icon name="folder-open" size={13} />{tr("変更…", "Change…")}</button
          >
        </div>
        {#if newName.trim() && newPreview}
          {#if newPreview.error}
            <div class="move-warn">{newPreview.error}</div>
          {:else if newPreview.renamed}
            <div class="move-warn">
              {tr(
                `同じ名前のフォルダ(${folderName(newName)}.glaux)があるので「${newPreview.folder}」になります。曲名か場所を変えると避けられます`,
                `A folder with the same name (${folderName(newName)}.glaux) exists, so it will be "${newPreview.folder}". Change the title or location to avoid this`,
              )}
            </div>
          {:else}
            <div class="move-note" title={tr(
                "曲名はそのまま。フォルダ名だけ、空白や使えない記号を _ にして付けます",
                "The title is kept as is; only the folder name replaces spaces and invalid characters with _",
              )}
            >
              {tr("フォルダ:", "Folder:")} {newPreview.folder}
            </div>
          {/if}
        {/if}
        <label
          class="lab-check"
          title={tr(
            "1 トラック + 試聴フレーズ + ループ ON + 音作りビューを開いた状態で作成(場所の中の SoundLab/ に置かれます)",
            "Creates 1 track + a preview phrase + loop on, with the sound design view open (placed in SoundLab/ under the location)",
          )}
        >
          <input type="checkbox" bind:checked={soundLab} disabled={busy} />
          {tr("音作り用テンプレートで作成", "Create from sound design template")}
        </label>
      </div>

      <div class="section">
        <button class="wide" onclick={browseAndOpen} disabled={busy}>
          {tr("フォルダを選択して開く…", "Open folder…")}
        </button>
        {#if candidates}
          <div class="found-title">
            {tr(`「${candidatesDir}」の中の曲(${candidates.length})`, `Songs in "${candidatesDir}" (${candidates.length})`)}
          </div>
          {#each candidates as c (c.path)}
            <button class="recent-item" disabled={busy} onclick={() => openPath(c.path)} title={c.path}>
              <span class="recent-title">{c.title}</span>
              <span class="recent-path">{c.path}</span>
            </button>
          {/each}
        {/if}
        {#if emptyDir}
          <div class="move-note">{tr(`「${emptyDir}」には Glaux の曲がありません。`, `"${emptyDir}" contains no Glaux songs.`)}</div>
          <button class="wide" onclick={createHere} disabled={busy}>{tr("このフォルダに新しい曲を作る", "Create a new song in this folder")}</button>
        {/if}
      </div>

      <div class="section">
        <div class="section-title">{tr("現在のプロジェクトの移動 / 曲名の変更", "Move current project / rename")}</div>
        <div class="new-row">
          <input
            type="text"
            placeholder={tr("曲名", "Song title")}
            bind:value={moveName}
            disabled={busy}
            title={tr("曲名(フォルダ名は曲名から自動で付けます)", "Song title (the folder name is derived from it automatically)")}
          />
          <button onclick={applyMove} disabled={busy || !moveDirty || !!movePreview?.error}>{tr("適用", "Apply")}</button>
        </div>
        <button class="loc" onclick={browseMoveParent} title={tr("クリックで移動先フォルダを選択", "Click to choose the destination folder")}>
          {tr("移動先:", "Move to:")} {moveParent}
        </button>
        {#if movePreview?.error}
          <div class="move-warn">{movePreview.error}</div>
        {:else if movePreview?.renamed}
          <div class="move-warn">
            {tr(
              `同じ名前のフォルダがあるので「${movePreview.folder}」になります`,
              `A folder with the same name exists, so it will be "${movePreview.folder}"`,
            )}
          </div>
        {/if}
        <div class="move-note">
          {tr("フォルダ:", "Folder:")} {movePreview?.folder && !movePreview.error
            ? movePreview.folder
            : moveName.trim() && moveName.trim() !== title
              ? `${folderName(moveName)}.glaux`
              : `${curStem}.glaux`}<br />
          {tr(
            "履歴・AI との会話ごとフォルダを移動します(元に戻すには再度移動)。",
            "Moves the folder together with its history and AI conversations (move again to undo).",
          )}
        </div>
      </div>

      {#if recent.length > 0}
        <div class="section">
          <div class="section-title">{tr("最近使ったプロジェクト", "Recent projects")}</div>
          {#each recent as r (r.path)}
            <button
              class="recent-item"
              class:current={r.current}
              disabled={busy || r.current || !r.exists}
              onclick={() => openPath(r.path)}
              title={r.exists ? r.path : tr(`見つかりません: ${r.path}`, `Not found: ${r.path}`)}
            >
              <span class="recent-title">
                {r.title}{r.current ? tr("(開いています)", " (open)") : ""}{r.exists ? "" : tr("(見つかりません)", " (not found)")}
              </span>
              <span class="recent-path">{r.path}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  /* ヘッダーの左が狭いときは、画面の切り替えより先に曲名を「…」で縮める */
  .menu-root {
    position: relative;
    min-width: 64px;
    flex: 0 1 auto;
  }

  .title-btn {
    max-width: 100%;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    font-size: 15px;
    border: none;
    background: none;
    padding: 2px 6px;
  }

  .title-btn:hover {
    color: var(--accent);
  }

  .title-text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chev {
    flex: none;
    font-size: 10px;
    color: var(--text-dim);
  }

  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 39;
  }

  .dropdown {
    position: fixed;
    z-index: 40;
    width: 380px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.5);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-height: 70vh;
    overflow-y: auto;
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

  .new-row {
    display: flex;
    gap: 6px;
  }

  .new-row input {
    flex: 1;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 5px 8px;
    font-size: 13px;
  }

  .new-row input:focus {
    outline: none;
    border-color: var(--accent-dim);
  }

  .lab-check {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    cursor: pointer;
    color: var(--text-dim);
  }

  .lab-check:hover {
    color: var(--text);
  }

  .loc {
    text-align: left;
    font-size: 11px;
    color: var(--text-dim);
    border: none;
    background: none;
    padding: 0 2px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .loc:hover {
    color: var(--accent);
  }

  .wide {
    width: 100%;
    text-align: center;
  }

  .found-title {
    font-size: 11px;
    color: var(--text-dim);
    word-break: break-all;
  }

  .recent-item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    width: 100%;
    text-align: left;
    padding: 6px 8px;
  }

  .recent-item.current {
    border-color: var(--accent-dim);
  }

  .recent-item:disabled {
    opacity: 0.55;
  }

  .recent-title {
    font-size: 13px;
    font-weight: 600;
  }

  .recent-path {
    font-size: 10px;
    color: var(--text-dim);
    font-family: Consolas, monospace;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .move-note {
    font-size: 10px;
    color: var(--text-dim);
  }

  .move-warn {
    font-size: 11px;
    color: var(--warn, #e0a030);
    line-height: 1.5;
    white-space: normal;
    overflow-wrap: anywhere;
    min-width: 0;
  }

  /* 新しい曲の場所: 名前・パス(長ければ省略)・変更ボタン */
  .loc-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    color: var(--text-dim);
    min-width: 0;
  }

  .loc-label {
    flex-shrink: 0;
  }

  .loc-path {
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .loc-btn {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    padding: 2px 8px;
  }

  .menu-error {
    background: var(--danger-bg);
    color: var(--danger-text);
    font-size: 12px;
    padding: 6px 8px;
    border-radius: 6px;
  }
</style>
