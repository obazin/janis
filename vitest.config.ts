import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

// Unit test harness — run locally via `pnpm test:unit`, which together with
// `pnpm check`, `cargo test` and `cargo clippy` is the project's quality gate
// (Janis has no CI yet; the gates are local). The svelte plugin compiles
// `.svelte` and `.svelte.ts` rune files so stores/util logic that uses
// `$state` can be imported directly. `resolve.conditions: ['browser']` makes
// `svelte` resolve its client runtime, which is what lets `$state` run
// outside a component.
export default defineConfig({
    plugins: [svelte()],
    resolve: {
        alias: {
            $lib: fileURLToPath(new URL('./src/lib', import.meta.url)),
        },
        conditions: ['browser'],
    },
    test: {
        environment: 'node',
        include: ['src/**/*.{test,spec}.ts'],
    },
});
