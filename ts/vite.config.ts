import { defineConfig } from "vite";

export default defineConfig({
  clearScreen: false,
  server: {
    port: 8111,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
