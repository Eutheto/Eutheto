<script setup lang="ts">
import { computed, onScopeDispose, ref } from "vue";
import { onBeforeRouteLeave, onBeforeRouteUpdate } from "vue-router";
import { messages } from "../messages";
import type { ProjectHomeController } from "../project-home";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "./ui/dialog";

const props = defineProps<{
  readonly home: ProjectHomeController;
  readonly dirty: boolean;
  readonly pending: boolean;
  readonly discard: () => void;
}>();

const open = ref(false);
const stayButton = ref<HTMLButtonElement | null>(null);
const waiting = computed(() => props.pending && props.home.state.operation?.settled !== true);
const canCancel = computed(() => waiting.value && props.home.state.operation?.cancel != null);
let resolveNavigation: ((leave: boolean) => void) | undefined;
let returnFocus: HTMLElement | null = null;
let leaving = false;

function decide(leave: boolean): void {
  if (leave && waiting.value && !canCancel.value) return;
  leaving = leave;
  if (leave) {
    if (waiting.value) void props.home.cancelOperation();
    props.discard();
  }
  open.value = false;
  const resolve = resolveNavigation;
  resolveNavigation = undefined;
  resolve?.(leave);
}

function beforeLeave(): boolean | Promise<boolean> {
  resolveNavigation?.(false);
  resolveNavigation = undefined;
  if (!props.dirty && !waiting.value) {
    leaving = true;
    open.value = false;
    props.discard();
    return true;
  }
  // A second navigation replaces the first decision, without stranding its promise.
  if (!open.value)
    returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  leaving = false;
  open.value = true;
  return new Promise<boolean>((resolve) => {
    resolveNavigation = resolve;
  });
}

function focusStay(event: Event): void {
  event.preventDefault();
  stayButton.value?.focus();
}

function restoreFocus(event: Event): void {
  event.preventDefault();
  // Successful route transitions receive focus from the shell, not the old view.
  if (!leaving && returnFocus?.isConnected) returnFocus.focus();
}

onBeforeRouteLeave(beforeLeave);
onBeforeRouteUpdate(beforeLeave);
onScopeDispose(() => resolveNavigation?.(false));
</script>

<template>
  <Dialog
    :open="open"
    @update:open="
      (value) => {
        if (!value) decide(false);
      }
    "
  >
    <DialogContent @open-auto-focus="focusStay" @close-auto-focus="restoreFocus">
      <DialogTitle>{{ messages.navigation.leaveTitle }}</DialogTitle>
      <DialogDescription>
        {{
          waiting
            ? canCancel
              ? messages.navigation.pendingDescription
              : messages.navigation.waitingDescription
            : dirty
              ? messages.navigation.dirtyDescription
              : messages.navigation.settledDescription
        }}
      </DialogDescription>
      <p v-if="waiting && dirty">{{ messages.navigation.dirtyDescription }}</p>
      <div class="dialog-actions">
        <button ref="stayButton" type="button" class="button-secondary" @click="decide(false)">
          {{ messages.navigation.stay }}
        </button>
        <button type="button" :disabled="waiting && !canCancel" @click="decide(true)">
          {{
            waiting
              ? messages.navigation.cancelAndLeave
              : dirty
                ? messages.navigation.discardAndLeave
                : messages.navigation.leave
          }}
        </button>
      </div>
    </DialogContent>
  </Dialog>
</template>
