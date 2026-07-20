import React from "react";
import ReactDOM from "react-dom/client";
import FloatingWidget from "./FloatingWidget";
import "./App.css";
import { initializeI18n } from "./i18n";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <FloatingWidget />
  </React.StrictMode>
);

void initializeI18n();
