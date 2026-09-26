// 曲名 → 曲のフォルダ名(`.glaux` の前)。glaux-mcp の store::folder_name と同じ決まり(表示の下書き用。実際に付けるのはアプリ側)。
// 空白と使えない記号は _ に、先頭と末尾の _ . は落とす、Windows の予約名は末尾に _、空なら Untitled、80 文字まで
export function folderName(title: string): string {
  let out = "";
  for (const c of title.trim()) {
    const keep = (/[\p{L}\p{N}]/u.test(c) || "-_.()&+,'".includes(c)) && !/[\p{Cc}]/u.test(c);
    const ch = keep ? c : "_";
    if (ch === "_" && out.endsWith("_")) continue;
    out += ch;
  }
  let name = [...out.replace(/^[_.]+|[_.]+$/g, "")].slice(0, 80).join("").replace(/[_.]+$/, "");
  if (!name) return "Untitled";
  const base = name.toUpperCase().split(".")[0];
  if (/^(CON|PRN|AUX|NUL)$/.test(base) || /^(COM|LPT)\d$/.test(base)) name += "_";
  return name;
}
