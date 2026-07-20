import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./App.css";
import { initializeI18n } from "./i18n";
import { TooltipLayer } from "./components/TooltipLayer";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
    <TooltipLayer />
  </React.StrictMode>
);

void initializeI18n();
