import { nextTick } from "vue";

/** Preserve visible focus when one owned view inserts content in its workspace. */
export async function preserveVisibleFocus(
  component: HTMLElement | undefined,
  isCurrent: () => boolean,
): Promise<void> {
  const owner = component?.closest<HTMLElement>("[data-project-workspace]") ?? component;
  const target = document.activeElement;
  if (!isCurrent() || !(target instanceof HTMLElement) || !owner?.contains(target)) return;
  const before = target.getBoundingClientRect();
  if (
    before.width <= 0 ||
    before.height <= 0 ||
    before.bottom <= 0 ||
    before.top >= innerHeight ||
    before.right <= 0 ||
    before.left >= innerWidth
  )
    return;
  await nextTick();
  if (
    !isCurrent() ||
    !component?.isConnected ||
    !owner.contains(component) ||
    !owner.isConnected ||
    owner.ownerDocument !== document ||
    !owner.contains(target) ||
    !target.isConnected ||
    document.activeElement !== target
  )
    return;
  const bounds = target.getBoundingClientRect();
  if (bounds.top < 0 || bounds.bottom > innerHeight || bounds.left < 0 || bounds.right > innerWidth)
    target.scrollIntoView({ block: "nearest", inline: "nearest" });
}
