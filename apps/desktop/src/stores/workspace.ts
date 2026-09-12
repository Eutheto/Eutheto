import { defineStore } from "pinia";
import { ref, shallowRef } from "vue";
import type { ProjectListItemV1 } from "../api/generated";

export interface ProjectDeletionReview {
  readonly project: ProjectListItemV1;
  readonly exportOutcome:
    | { readonly kind: "none" }
    | { readonly kind: "saved"; readonly artifactName: string }
    | { readonly kind: "cancelled" }
    | { readonly kind: "unconfirmed" };
}

export const useWorkspaceStore = defineStore("workspace", () => {
  const selectedProjectId = ref<string | null>(null);
  // Volatile presentation intent, never deletion authority or a persisted draft.
  const deletionReview = shallowRef<ProjectDeletionReview | null>(null);
  return { selectedProjectId, deletionReview };
});
