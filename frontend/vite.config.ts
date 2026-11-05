import type { ClientRequest } from 'node:http';
import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

const backend = 'http://127.0.0.1:8080';

// The backend's CSRF check wants its own PUBLIC_URL as the Origin, not the dev server's.
const proxied = {
	target: backend,
	configure: (proxy: {
		on: (event: 'proxyReq', handler: (request: ClientRequest) => void) => void;
	}) => proxy.on('proxyReq', (request) => request.setHeader('origin', 'http://localhost:8080'))
};

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: {
		proxy: {
			'/api': proxied,
			'/oauth': proxied,
			'/mcp': proxied,
			'/.well-known': proxied
		}
	},
	test: {
		expect: { requireAssertions: true },
		environment: 'node',
		include: ['src/**/*.{test,spec}.{js,ts}']
	}
});
