import { defineStore } from "pinia";
import { ref } from "vue";

export const useWorkspaceStore = defineStore("workspace", () => {
  const selectedProjectId = ref<string | null>(null);
  return { selectedProjectId };
});
