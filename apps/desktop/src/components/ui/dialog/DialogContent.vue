<script setup lang="ts">
import type { DialogContentEmits, DialogContentProps } from "reka-ui";
import type { HTMLAttributes } from "vue";
import { computed, useAttrs } from "vue";
import { DialogContent, DialogOverlay, DialogPortal, useForwardPropsEmits } from "reka-ui";
import { cn, withoutClass, withoutUndefined } from "../../../lib/utils";
defineOptions({ inheritAttrs: false });

const props = defineProps<DialogContentProps & { class?: HTMLAttributes["class"] }>();
const emits = defineEmits<DialogContentEmits>();
const attrs = useAttrs();
const delegatedProps = computed(() => {
  return withoutUndefined(withoutClass(props));
});
const forwarded = useForwardPropsEmits(delegatedProps, emits);
const contentProps = computed(() =>
  withoutUndefined({
    ...forwarded.value,
    ...attrs,
  }),
);

// Native Linux WebKit can report Shift+Tab as key="Unidentified", code="Tab",
// which Reka does not wrap. Reka's wraps prevent scrolling to the newly
// focused control in a tall, magnified dialog.
function handleDialogTab(event: KeyboardEvent): void {
  if (
    event.code !== "Tab" ||
    (event.key !== "Tab" && event.key !== "Unidentified") ||
    event.altKey ||
    event.ctrlKey ||
    event.metaKey
  )
    return;

  const dialog = event.currentTarget as HTMLElement;
  let first: HTMLElement | undefined;
  let last: HTMLElement | undefined;
  for (const candidate of dialog.querySelectorAll<HTMLElement>(
    "a[href], button, input, select, textarea, [tabindex]",
  )) {
    if (
      candidate.tabIndex < 0 ||
      candidate.matches(":disabled") ||
      candidate.getClientRects().length === 0 ||
      getComputedStyle(candidate).visibility === "hidden" ||
      candidate.closest("[inert]")
    )
      continue;
    first ??= candidate;
    last = candidate;
  }
  if (!first || !last) return;
  if (event.shiftKey && event.target === first) {
    if (event.key === "Unidentified" && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    }
    requestAnimationFrame(() => {
      if (document.activeElement === last) last.scrollIntoView({ block: "nearest" });
    });
  } else if (!event.shiftKey && event.target === last) {
    requestAnimationFrame(() => {
      if (document.activeElement === first) first.scrollIntoView({ block: "nearest" });
    });
  }
}
</script>

<template>
  <DialogPortal>
    <DialogOverlay
      class="fixed inset-0 z-50 bg-canvas/80 backdrop-blur-xs data-[state=closed]:animate-out data-[state=closed]:fade-out data-[state=open]:animate-in data-[state=open]:fade-in motion-reduce:animate-none"
    />
    <DialogContent
      v-bind="contentProps"
      :class="
        cn(
          'fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100vh-2rem)] w-[calc(100%-2rem)] max-w-lg -translate-x-1/2 -translate-y-1/2 gap-4 overflow-y-auto rounded-lg border border-line bg-raised p-6 text-ink shadow-raised data-[state=closed]:animate-out data-[state=closed]:fade-out data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in data-[state=open]:zoom-in-95 motion-reduce:animate-none',
          props.class,
        )
      "
      @keydown="handleDialogTab"
    >
      <slot />
    </DialogContent>
  </DialogPortal>
</template>
