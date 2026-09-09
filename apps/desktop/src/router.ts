import { createRouter, createWebHashHistory } from "vue-router";

import ProjectHome from "./components/ProjectHome.vue";

export function createAppRouter() {
  return createRouter({
    history: createWebHashHistory(),
    routes: [
      { path: "/", name: "projects", component: ProjectHome },
      { path: "/:pathMatch(.*)*", redirect: { name: "projects" } },
    ],
  });
}
