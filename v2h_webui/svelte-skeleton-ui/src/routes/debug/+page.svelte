<script lang="ts">
	import { afterUpdate } from 'svelte';

	interface LogEntry {
		ts: string;
		level: string;
		target: string;
		message: string;
	}

	// --- filter state ---
	let showError = true;
	let showWarn  = true;
	let showInfo  = true;
	let showDebug = true;
	let grepInclude = '';   // show only lines matching any term (comma-sep)
	let grepExclude = '';   // hide lines matching any term (comma-sep)
	let autoScroll = true;

	// --- data ---
	let entries: LogEntry[] = [];
	let pending: LogEntry[] = [];
	let flushTimer: ReturnType<typeof setTimeout> | null = null;

	// Batch incoming entries into the display array at most every 200 ms
	// so rapid debug bursts don't stall the UI.
	function scheduleFlush() {
		if (flushTimer) return;
		flushTimer = setTimeout(() => {
			flushTimer = null;
			if (pending.length === 0) return;
			entries = [...entries, ...pending].slice(-3000);
			pending = [];
		}, 200);
	}

	// --- derived / filtered ---
	const LEVELS = ['ERROR','WARN','INFO','DEBUG','TRACE'] as const;

	function levelVisible(level: string): boolean {
		if (level === 'ERROR') return showError;
		if (level === 'WARN')  return showWarn;
		if (level === 'INFO')  return showInfo;
		return showDebug;  // DEBUG / TRACE
	}

	function terms(raw: string): string[] {
		return raw.split(',').map(t => t.trim().toLowerCase()).filter(Boolean);
	}

	$: includeTerms = terms(grepInclude);
	$: excludeTerms = terms(grepExclude);

	$: filtered = entries.filter(e => {
		if (!levelVisible(e.level)) return false;
		const haystack = (e.target + ' ' + e.message).toLowerCase();
		if (includeTerms.length > 0 && !includeTerms.some(t => haystack.includes(t))) return false;
		if (excludeTerms.length > 0 &&  excludeTerms.some(t => haystack.includes(t))) return false;
		return true;
	});

	// --- auto-scroll ---
	let logEl: HTMLElement;
	afterUpdate(() => {
		if (autoScroll && logEl) logEl.scrollTop = logEl.scrollHeight;
	});

	// --- split message at first ' | ' ---
	function splitMsg(msg: string): [string, string] {
		const idx = msg.indexOf(' | ');
		if (idx === -1) return [msg, ''];
		return [msg.slice(0, idx), msg.slice(idx + 3)];
	}

	// --- module shortening: strip crate prefix ---
	function shortTarget(t: string): string {
		return t.replace(/^indra_beaglebone::/, '');
	}

	// --- level styling ---
	function levelClass(level: string): string {
		switch (level) {
			case 'ERROR': return 'text-red-400 font-bold';
			case 'WARN':  return 'text-amber-400';
			case 'INFO':  return 'text-sky-300';
			default:      return 'text-surface-400';
		}
	}
	function rowClass(level: string): string {
		if (level === 'ERROR') return 'bg-red-950/30';
		if (level === 'WARN')  return 'bg-amber-950/20';
		return '';
	}

	// --- WebSocket ---
	let wsConnected = false;
	let liveLog = false;
	let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	let socket: WebSocket;

	function sendLogControl(enable: boolean) {
		if (socket && socket.readyState === WebSocket.OPEN) {
			socket.send(JSON.stringify({ cmd: enable ? 'StartLogs' : 'StopLogs' }));
		}
	}

	function connect() {
		if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null; }
		if (typeof WebSocket === 'undefined') return;
		socket = new WebSocket('ws://192.168.10.101:5555');
		socket.addEventListener('open', () => {
			wsConnected = true;
			if (liveLog) sendLogControl(true); // re-subscribe after reconnect
		});
		socket.addEventListener('close',   () => { wsConnected = false; reconnectTimer = setTimeout(connect, 5000); });
		socket.addEventListener('error',   () => { wsConnected = false; });
		socket.addEventListener('message', (ev: MessageEvent) => {
			const msg = JSON.parse(ev.data);
			if (msg.Log) {
				pending.push(msg.Log);
				scheduleFlush();
			}
		});
	}
	connect();
</script>

<!-- status bar -->
<div class="flex flex-wrap items-center gap-3 px-4 py-2 bg-surface-800 border-b border-surface-600 text-sm sticky top-0 z-10">

	<!-- level toggles -->
	<label class="flex items-center gap-1 cursor-pointer select-none">
		<input type="checkbox" bind:checked={showError} class="w-3.5 h-3.5" />
		<span class="font-mono text-red-400">ERROR</span>
	</label>
	<label class="flex items-center gap-1 cursor-pointer select-none">
		<input type="checkbox" bind:checked={showWarn} class="w-3.5 h-3.5" />
		<span class="font-mono text-amber-400">WARN</span>
	</label>
	<label class="flex items-center gap-1 cursor-pointer select-none">
		<input type="checkbox" bind:checked={showInfo} class="w-3.5 h-3.5" />
		<span class="font-mono text-sky-300">INFO</span>
	</label>
	<label class="flex items-center gap-1 cursor-pointer select-none">
		<input type="checkbox" bind:checked={showDebug} class="w-3.5 h-3.5" />
		<span class="font-mono text-surface-400">DEBUG</span>
	</label>

	<div class="w-px h-5 bg-surface-500"></div>

	<!-- grep include -->
	<div class="flex items-center gap-1">
		<span class="text-surface-400 text-xs font-mono">grep</span>
		<input type="text" bind:value={grepInclude}
			placeholder="include (comma-sep)"
			class="input text-xs font-mono py-0.5 px-2 w-44 bg-surface-700" />
	</div>

	<!-- grep exclude -->
	<div class="flex items-center gap-1">
		<span class="text-surface-400 text-xs font-mono">grep&nbsp;-v</span>
		<input type="text" bind:value={grepExclude}
			placeholder="exclude (comma-sep)"
			class="input text-xs font-mono py-0.5 px-2 w-44 bg-surface-700" />
	</div>

	<div class="w-px h-5 bg-surface-500"></div>

	<label class="flex items-center gap-1 cursor-pointer select-none">
		<input type="checkbox" bind:checked={liveLog}
			on:change={() => sendLogControl(liveLog)}
			class="w-3.5 h-3.5" />
		<span class="text-xs font-semibold {liveLog ? 'text-green-400' : 'text-surface-200'}">Live Logs</span>
	</label>

	<div class="w-px h-5 bg-surface-500"></div>

	<label class="flex items-center gap-1 cursor-pointer select-none">
		<input type="checkbox" bind:checked={autoScroll} class="w-3.5 h-3.5" />
		<span class="text-xs {autoScroll ? 'text-green-400' : 'text-surface-200'}">Auto-scroll</span>
	</label>

	<button on:click={() => { entries = []; pending = []; }} class="btn btn-sm variant-ghost-surface text-xs py-0.5 px-2">
		Clear
	</button>

	<span class="text-xs text-surface-500 ml-auto">
		{#if !wsConnected}<span class="text-red-400 mr-2">● offline</span>{:else}<span class="text-green-400 mr-2">● live</span>{/if}
		{filtered.length} / {entries.length}
	</span>
</div>

<!-- log table -->
<div bind:this={logEl} class="overflow-y-auto font-mono text-xs" style="height: calc(100vh - 48px);">
	<table class="w-full border-collapse">
		<thead class="sticky top-0 bg-surface-800 text-surface-400 text-left">
			<tr>
				<th class="px-2 py-1 whitespace-nowrap w-28">Time</th>
				<th class="px-2 py-1 w-14">Level</th>
				<th class="px-2 py-1 w-56">Module</th>
				<th class="px-2 py-1 w-[36rem]">Message</th>
				<th class="px-2 py-1">Value</th>
			</tr>
		</thead>
		<tbody>
			{#each filtered as e (e.ts + e.message)}
				{@const [msg, val] = splitMsg(e.message)}
				<tr class="border-t border-surface-700/40 hover:bg-surface-700/30 {rowClass(e.level)}">
					<td class="px-2 py-0.5 text-surface-500 whitespace-nowrap">{e.ts}</td>
					<td class="px-2 py-0.5 whitespace-nowrap {levelClass(e.level)}">{e.level}</td>
					<td class="px-2 py-0.5 text-surface-400 truncate max-w-[14rem]" title={e.target}>{shortTarget(e.target)}</td>
					<td class="px-2 py-0.5 break-words max-w-xs">{msg}</td>
					<td class="px-2 py-0.5 text-cyan-700 dark:text-cyan-400 break-all">{val}</td>
				</tr>
			{/each}
		</tbody>
	</table>
</div>
