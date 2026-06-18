import React from "react";
import ReactDOM from "react-dom/client";

import SettingsApp from "./SettingsApp";
import { Toaster } from "@/components/ui/sonner";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <SettingsApp />
    <Toaster />
  </React.StrictMode>
);
