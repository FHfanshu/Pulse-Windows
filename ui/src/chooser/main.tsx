import { createRoot } from "react-dom/client";
import "../shared/base.css";
import "../settings/settings.css";
import "./chooser.css";
import { Chooser } from "./Chooser";

createRoot(document.getElementById("root")!).render(<Chooser />);
