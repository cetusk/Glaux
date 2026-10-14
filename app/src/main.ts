import { mount } from "svelte";
import App from "./App.svelte";
import "./app.css";
import { installTooltips } from "./lib/tooltip";

const app = mount(App, { target: document.getElementById("app")! });
// マウスを載せたときの説明を、標準の吹き出しより早く確実に出す
installTooltips();

export default app;
