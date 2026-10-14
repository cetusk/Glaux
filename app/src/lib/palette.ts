// 画面の色は app.css の色の名前(CSS の変数)の 1 か所で決める。CSS では var(--名前) で使い、
// 絵(canvas)を描くコードは color("--名前") でここから読む(部品に色の値を直接書かない)。
// 透明度を変えた色は color("--名前", 0.3) のように、同じ色の名前から作る。
// テーマカラーを変えたら clearColorCache() で読み直す(applyTheme が呼ぶ)

const cache = new Map<string, string>();
// 場所ごとに名前の値を変えている所(音色エディタなど)は、その場所ごとに覚える
let scoped = new WeakMap<Element, Map<string, string>>();

/** 色の値(#rgb・#rrggbb・rgb()・rgba())を [r, g, b, a] にする。読めなければ null */
function parse(c: string): [number, number, number, number] | null {
  const hex = c.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const h = hex[1].length === 3 ? [...hex[1]].map((x) => x + x).join("") : hex[1];
    return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16), 1];
  }
  const fn = c.match(/^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/i);
  if (fn) return [Number(fn[1]), Number(fn[2]), Number(fn[3]), fn[4] == null ? 1 : Number(fn[4])];
  return null;
}

/** 色の名前の値。`alpha` を渡すと、その透明度の rgba にする。`scope` はその要素から見た値(音色エディタなど、
 *  場所ごとに名前の値を変えている所)。見つからなければ灰色 */
export function color(name: string, alpha?: number, scope?: Element | null): string {
  const key = `${name}|${alpha ?? ""}`;
  let memo = cache;
  if (scope) {
    memo = scoped.get(scope) ?? new Map();
    scoped.set(scope, memo);
  }
  const hit = memo.get(key);
  if (hit !== undefined) return hit;
  const raw = getComputedStyle(scope ?? document.documentElement).getPropertyValue(name).trim() || "#808080";
  let out = raw;
  if (alpha != null) {
    const p = parse(raw);
    if (p) out = `rgba(${p[0]}, ${p[1]}, ${p[2]}, ${alpha})`;
  }
  memo.set(key, out);
  return out;
}

/** 色の名前の "r, g, b"(`rgba(${rgbOf("--x")}, a)` のように、透明度を細かく変えながら描くとき) */
export function rgbOf(name: string, scope?: Element | null): string {
  const p = parse(color(name, undefined, scope));
  return p ? `${p[0]}, ${p[1]}, ${p[2]}` : "128, 128, 128";
}

/** テーマカラーを変えたときなどに、覚えた色を捨てて読み直させる */
export function clearColorCache(): void {
  cache.clear();
  scoped = new WeakMap();
}

/** トラックに付けられる色の候補(曲に保存する値なので、色の名前ではなく値で持つ。null = 色を外す) */
export const TRACK_COLORS: (string | null)[] = ["#25bdb1", "#5da2e8", "#b07ce8", "#e87ca8", "#e8a07c", "#e8d27c", "#7cc47c", null];
