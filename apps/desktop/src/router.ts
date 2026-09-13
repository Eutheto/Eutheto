import { createRouter, createWebHashHistory } from "vue-router";

import ProjectHome from "./components/ProjectHome.vue";
import WelcomePage from "./components/WelcomePage.vue";
import ProjectWorkspace from "./components/ProjectWorkspace.vue";
import WorkforceSetupOverview from "./components/WorkforceSetupOverview.vue";
import HistoryPage from "./components/HistoryPage.vue";
import SettingsPage from "./components/SettingsPage.vue";
import AboutPage from "./components/AboutPage.vue";
import PortableWorkspace from "./components/PortableWorkspace.vue";
import RouteRecovery from "./components/RouteRecovery.vue";

export function createAppRouter() {
  return createRouter({
    history: createWebHashHistory(),
    routes: [
      { path: "/", name: "welcome", component: WelcomePage },
      {
        path: "/projects/new",
        name: "project-create",
        component: WelcomePage,
        props: { startCreate: true },
      },
      {
        path: "/projects/import",
        name: "project-import",
        component: PortableWorkspace,
        props: { mode: "import" },
      },
      { path: "/projects", name: "projects", component: ProjectHome },
      {
        path: "/project/:scenarioId",
        component: ProjectWorkspace,
        props: true,
        children: [
          { path: "", redirect: (to) => ({ name: "project-setup", params: to.params }) },
          { path: "setup", name: "project-setup", component: WorkforceSetupOverview },
          { path: "history", name: "project-history", component: HistoryPage },
          {
            path: "export",
            name: "project-export",
            component: PortableWorkspace,
            props: { mode: "export" },
          },
        ],
      },
      { path: "/settings", name: "settings", component: SettingsPage },
      {
        path: "/settings/backup-restore",
        name: "backup-restore",
        component: PortableWorkspace,
        props: { mode: "backup-restore" },
      },
      { path: "/about/licenses", name: "about", component: AboutPage },
      { path: "/:pathMatch(.*)*", name: "not-found", component: RouteRecovery },
    ],
  });
}
