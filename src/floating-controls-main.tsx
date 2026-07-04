import React from "react";
import ReactDOM from "react-dom/client";
import FloatingControls from "./FloatingControls";
import "./App.css";
import { initializeI18n } from "./i18n";

async function render() {
  await initializeI18n();
  ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode><FloatingControls /></React.StrictMode>);
}
void render();
