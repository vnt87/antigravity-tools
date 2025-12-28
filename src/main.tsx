import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";

import App from './App';
import './i18n'; // Import i18n config
import "./App.css";

// Explicitly call Rust command to show window on startup
// Used with visible:false to solve startup black screen issue
invoke("show_main_window").catch(console.error);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />

  </React.StrictMode>,
);
