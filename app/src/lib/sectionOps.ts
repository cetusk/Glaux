// 区間(セクション)のマーカーの編集(タイムラインの区間の帯とルーラーのメニューで共用)
import * as api from "./api";
import { tr } from "./i18n.svelte";
import type { Marker } from "./timelineOps";
import type { Project } from "./types";

/// 曲の区間を tick の順に並べて置き換える(1 件の履歴)
export function commitSections(list: Marker[], label: string) {
  const sorted = [...list].sort((a, b) => a.tick - b.tick);
  api.applyEdit([{ op: "set_sections", sections: sorted }], label).catch(() => {});
}

/// 曲の区間の写し(書き換えて commitSections に渡す)
export function markersOf(project: Project): Marker[] {
  return (project.sections ?? []).map((m) => ({ ...m }));
}

/// `tick` のマーカーを消す。消したら true
export function removeMarker(project: Project, tick: number): boolean {
  const list = markersOf(project);
  const hit = list.find((m) => m.tick === tick);
  if (!hit) return false;
  commitSections(
    list.filter((m) => m.tick !== tick),
    tr(`マーカー「${hit.name}」を削除`, `Delete marker "${hit.name}"`),
  );
  return true;
}
