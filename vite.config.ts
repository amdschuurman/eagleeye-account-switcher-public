import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { realpathSync } from "node:fs";

// Tauri expects a fixed port and no screen clearing (src-tauri devUrl = http://localhost:1420).
// node_modules may be a symlink to the Linux disk (WSL dev), so serving from its real path is allowed.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, fs: { allow: [".", realpathSync("node_modules")] }, watch: { ignored: ["**/src-tauri/**"] } },
  envPrefix: ["VITE_", "TAURI_"],
  build: { target: "es2021", outDir: "dist", assetsInlineLimit: 0 },
});
