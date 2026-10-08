import "@fontsource-variable/nunito";
import { createRoot } from "react-dom/client";
import "../shared/base.css";
import "./panel.css";
import { App } from "./App";

createRoot(document.getElementById("root")!).render(<App />);
