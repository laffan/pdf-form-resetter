import { defineConfig } from "vite";

// Tauri serves the built files from dist/ and, in development, from this dev
// server. The fixed port matches devUrl in src-tauri/tauri.conf.json.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    target: "esnext",
    emptyOutDir: true,
  },
});
