import { createApp } from "vue";
import { createPinia } from "pinia";
import { PiniaColada } from "@pinia/colada";

import App from "./App.vue";
import { createAppRouter } from "./router";
import "./styles.css";

createApp(App).use(createPinia()).use(PiniaColada).use(createAppRouter()).mount("#app");
