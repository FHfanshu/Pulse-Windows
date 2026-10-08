import { createRoot } from "react-dom/client";
import "../shared/base.css";
import "./settings.css";
import { Shell } from "./Shell";

createRoot(document.getElementById("root")!).render(<Shell />);
