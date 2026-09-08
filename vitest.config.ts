// Vitest config: jsdom + the Svelte compiler, so components can be smoke
// tested in isolation. $lib is aliased the same way SvelteKit does.
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [svelte()],
  resolve: {
    alias: {
      $lib: fileURLToPath(new URL("./src/lib", import.meta.url)),
    },
    // Use Svelte's client runtime under vitest (jsdom is browser-like).
    conditions: process.env.VITEST ? ["browser"] : undefined,
  },
  test: {
    environment: "jsdom",
    globals: true,
    include: ["src/**/*.test.ts"],
  },
});