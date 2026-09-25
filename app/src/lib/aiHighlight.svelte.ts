// 直前のチャットのターンで AI が変えたクリップ・ノート。タイムラインとピアノロールが縁取りに使う。
// 次の指示を送るまで(または「取り消す」まで)表示する。

export const aiHighlight = $state<{ clips: Set<string>; notes: Set<string> }>({
  clips: new Set(),
  notes: new Set(),
});

interface ClipChange {
  clip_id: string;
  added_ids?: string[];
  changed_ids?: string[];
}

/** turn_changes の結果から縁取りの対象を作る */
export function setAiHighlight(summary: { clips?: ClipChange[] }) {
  const clips = new Set<string>();
  const notes = new Set<string>();
  for (const c of summary.clips ?? []) {
    clips.add(c.clip_id);
    for (const id of [...(c.added_ids ?? []), ...(c.changed_ids ?? [])]) notes.add(id);
  }
  aiHighlight.clips = clips;
  aiHighlight.notes = notes;
}

export function clearAiHighlight() {
  aiHighlight.clips = new Set();
  aiHighlight.notes = new Set();
}
