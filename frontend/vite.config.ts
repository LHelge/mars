import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The orchestrator's API listener; API_PORT defaults to 7000 (README.md, "Configuration").
const apiTarget = process.env.VITE_API_TARGET ?? "http://localhost:7000";
const wsTarget = apiTarget.replace(/^http/, "ws");

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/api": { target: apiTarget, changeOrigin: true },
      "/ws": { target: wsTarget, ws: true },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
