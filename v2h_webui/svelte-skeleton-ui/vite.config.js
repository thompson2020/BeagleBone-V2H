import { purgeCss } from 'vite-plugin-tailwind-purgecss';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
	ssr: {
		noExternal: ['@floating-ui/dom', 'svelte-range-slider-pips'],
	},
	plugins: [
		sveltekit(),
		purgeCss(),
		VitePWA({
			registerType: 'autoUpdate',
			injectRegister: 'auto',
			manifest: {
				name: 'V2H Charger',
				short_name: 'V2H',
				description: 'V2H EV Charger Control',
				theme_color: '#10b981',
				background_color: '#1a1a2e',
				display: 'standalone',
				scope: '/',
				start_url: '/',
				icons: [
					{ src: '/pwa-192.png', sizes: '192x192', type: 'image/png' },
					{ src: '/pwa-512.png', sizes: '512x512', type: 'image/png', purpose: 'any maskable' },
				],
			},
			workbox: {
				globPatterns: ['**/*.{js,css,html,png,svg,ico}'],
				cleanupOutdatedCaches: true,
				navigateFallback: '/',
			},
			devOptions: { enabled: true, type: 'module' },
		}),
	]
});
