import React from "react";
import ReactDOM from "react-dom/client";
import TrayMenu from "./TrayMenu";
import { syncThemeFromStorage } from "./lib/theme";
import "./App.css";
import { initializeI18n } from "./i18n";
import { TooltipLayer } from "./components/TooltipLayer";

syncThemeFromStorage();

async function renderTray() {
  await initializeI18n();
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <TrayMenu />
      <TooltipLayer />
    </React.StrictMode>
  );
}

void renderTray();
