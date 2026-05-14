<script lang="ts">
	import '../app.postcss';
	import { AppShell, AppBar, LightSwitch } from '@skeletonlabs/skeleton';
	import { operationalMode } from '$lib/stores';
	import { onMount, onDestroy } from 'svelte';
	import { page } from '$app/stores';
	// Floating UI for Popups
	import { computePosition, autoUpdate, flip, shift, offset, arrow } from '@floating-ui/dom';
	import { storePopup } from '@skeletonlabs/skeleton';
	storePopup.set({ computePosition, autoUpdate, flip, shift, offset, arrow });
</script>

<!-- App Shell -->

<AppShell>
	<svelte:fragment slot="header">
		<!-- App Bar -->
		<AppBar>
			<svelte:fragment slot="lead">
				<strong class="text-xl uppercase">V2H</strong>
			</svelte:fragment>
			<svelte:fragment slot="trail">
				{#if $page.url.pathname !== '/'}
					<a class="btn btn-sm variant-ghost-surface" href="/" rel="noreferrer"> Home </a>
				{/if}
				{#if $page.url.pathname !== '/data'}
					<a class="btn btn-sm variant-ghost-surface" href="/data" rel="noreferrer"> Data </a>
				{/if}
				{#if $page.url.pathname !== '/debug'}
					<a class="btn btn-sm variant-ghost-surface" href="/debug" rel="noreferrer"> Debug </a>
				{/if}
				<LightSwitch />
			</svelte:fragment>
		</AppBar>
	</svelte:fragment>
	<!-- Page Route Content -->
	<slot />
</AppShell>
