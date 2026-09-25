<script lang="ts">
  // 線のアイコン(Lucide)。色は文字色(currentColor)、大きさは size か CSS の --icon-size。
  // 1 つの意味に 1 つのアイコン(例: 再生ループ = repeat、クリップのループ = infinity、
  // 直前の取り消し = undo-2、選んだ編集の打ち消し = rotate-ccw)。新しく使うときは既存の意味と重ねない。
  import { ICONS, type IconName } from "./icons";

  let { name, size, fill = false }: { name: IconName; size?: number; fill?: boolean } = $props();
</script>

<span class="icon" style={size ? `--icon-size:${size}px` : undefined} aria-hidden="true"
  >{@html `<svg viewBox="0 0 24 24" fill="${fill ? "currentColor" : "none"}" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name]}</svg>`}</span
>

<style>
  .icon {
    display: inline-flex;
    width: var(--icon-size, 16px);
    height: var(--icon-size, 16px);
    flex-shrink: 0;
    vertical-align: -3px;
  }

  .icon :global(svg) {
    width: 100%;
    height: 100%;
  }
</style>
