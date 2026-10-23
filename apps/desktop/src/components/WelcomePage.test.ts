import { PiniaColada } from "@pinia/colada";
import { createPinia } from "pinia";
import { expect, it, onTestFinished, vi } from "vitest";
import { createSSRApp, defineComponent, h } from "vue";
import { renderToString } from "vue/server-renderer";
import { createMemoryHistory, createRouter, RouterView } from "vue-router";
import { createProjectHomeController } from "../project-home";
import { fakeApi } from "../testing/project-home";
import WelcomePage from "./WelcomePage.vue";

it("renders empty required planning dates with explicit shared entry guidance", async () => {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path: "/new",
        component: defineComponent({
          setup() {
            const home = createProjectHomeController(fakeApi());
            onTestFinished(() => home.dispose());
            return () => h(WelcomePage, { home, startCreate: true });
          },
        }),
      },
      { path: "/", name: "welcome", component: { template: "<p>Welcome</p>" } },
    ],
  });
  const app = createSSRApp(RouterView);
  app.use(createPinia()).use(PiniaColada).use(router);
  onTestFinished(() => {
    vi.unstubAllGlobals();
  });
  await router.push("/new");
  const html = await renderToString(app);
  for (const id of ["first-date", "last-date"]) {
    const input = html.match(new RegExp(`<input[^>]*id="${id}"[^>]*>`, "u"))?.[0];
    expect(input).toContain('type="date"');
    expect(input).toContain('value=""');
    expect(input).toContain("required");
    expect(input).toContain('aria-describedby="planning-dates-help"');
  }
  expect(html).toContain("Enter both dates explicitly; no dates are selected by default.");
});
