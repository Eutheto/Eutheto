<script setup lang="ts">
// Chromium can emit several input events for one multiline insertion. Keep this
// controlled DOM value out of unrelated preview renders until the parent echoes
// the draft; reapplying the old value between those events corrupts the edit.
defineProps<{ modelValue: string }>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
function update(event: Event): void {
  emit("update:modelValue", (event.target as HTMLTextAreaElement).value);
}
</script>

<template>
  <textarea :value="modelValue" @input="update" />
</template>
