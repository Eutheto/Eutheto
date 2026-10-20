import { createRouter, createWebHashHistory } from "vue-router";

import ProjectHome from "./components/ProjectHome.vue";
import WelcomePage from "./components/WelcomePage.vue";
import ProjectWorkspace from "./components/ProjectWorkspace.vue";
import WorkforceSetupOverview from "./components/WorkforceSetupOverview.vue";
import PeopleSetup from "./components/PeopleSetup.vue";
import WorkSetup from "./components/WorkSetup.vue";
import ScorePolicySetup from "./components/ScorePolicySetup.vue";
import EligibilitySetup from "./components/EligibilitySetup.vue";
import AvailabilitySetup from "./components/AvailabilitySetup.vue";
import RuleSetup from "./components/RuleSetup.vue";
import ValidationSetup from "./components/ValidationSetup.vue";
import PeopleCsvImport from "./components/PeopleCsvImport.vue";
import AssignmentCsvImport from "./components/AssignmentCsvImport.vue";
import EligibilityCsvImport from "./components/EligibilityCsvImport.vue";
import EligibilityMatrixImport from "./components/EligibilityMatrixImport.vue";
import ShiftCsvImport from "./components/ShiftCsvImport.vue";
import AvailabilityCsvImport from "./components/AvailabilityCsvImport.vue";
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
          { path: "people", name: "project-people", component: PeopleSetup },
          { path: "people/import", name: "project-people-import", component: PeopleCsvImport },
          {
            path: "assignments/import",
            name: "project-assignments-import",
            component: AssignmentCsvImport,
          },
          { path: "work", name: "project-work", component: WorkSetup },
          { path: "work/import", name: "project-work-import", component: ShiftCsvImport },
          { path: "score-policy", name: "project-score-policy", component: ScorePolicySetup },
          { path: "eligibility", name: "project-eligibility", component: EligibilitySetup },
          {
            path: "eligibility/import",
            name: "project-eligibility-import",
            component: EligibilityCsvImport,
          },
          {
            path: "eligibility/paste",
            name: "project-eligibility-matrix-import",
            component: EligibilityMatrixImport,
          },
          { path: "availability", name: "project-availability", component: AvailabilitySetup },
          {
            path: "availability/import",
            name: "project-availability-import",
            component: AvailabilityCsvImport,
          },
          { path: "rules", name: "project-rules", component: RuleSetup },
          { path: "validation", name: "project-validation", component: ValidationSetup },
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
