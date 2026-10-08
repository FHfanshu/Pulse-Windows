import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  resolve: {
    alias: {
      "@locales": resolve(__dirname, "locales"),
      "@icons": resolve(__dirname, "assets/icons"),
    },
  },
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    target: "es2022",
    rollupOptions: {
      input: {
        panel: resolve(__dirname, "ui/panel.html"),
        settings: resolve(__dirname, "ui/settings.html"),
        chooser: resolve(__dirname, "ui/chooser.html"),
        recap: resolve(__dirname, "ui/recap.html"),
      },
    },
  },
});
