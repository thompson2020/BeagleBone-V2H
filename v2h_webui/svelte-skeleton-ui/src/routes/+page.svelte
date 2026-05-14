<script lang="ts">
	import { RangeSlider } from '@skeletonlabs/skeleton';
	import { onMount } from 'svelte';
	import { writable } from 'svelte/store';
	import { operationalMode } from '$lib/stores';

	interface EventData {
		time: string;
		action: string;
	}

	interface OperatorSettings {
		v2h_soc_min:          number;
		v2h_soc_max:          number;
		v2h_soc_max_boost:    number;
		v2h_max_amps:         number;
		self_use:             boolean;
		export_excess_solar:  boolean;
		ev_drain_protection:  boolean;
		ready_to_drive:            boolean;
		ready_to_drive_end_time:   string;
		ready_to_drive_start_time: string;
		ready_to_drive_soc:        number;
		ready_to_drive_days:       boolean[];
		off_peak_charging:    boolean;
		off_peak_start:       string;
		off_peak_end:         string;
		smart_charge:         boolean;
		smart_export:                boolean;
		smart_export_limit_w:        number;
		smart_export_soc_min:        number;
		smart_export_excess_solar:   boolean;
		charge_soc_limit:            number;
		charge_amps:          number;
		charge_eco:           boolean;
	}

	interface X102Status {
		status_discharge_compatible:    boolean;
		status_normal_stop_request:     boolean;
		status_vehicle:                 boolean;
		status_charging_system:         boolean;
		status_vehicle_shifter_position:boolean;
		status_vehicle_charging:        boolean;
	}
	interface X102Faults {
		fault_battery_voltage_deviation: boolean;
		fault_high_battery_temperature:  boolean;
		fault_battery_current_deviation: boolean;
		fault_battery_undervoltage:      boolean;
		fault_battery_overvoltage:       boolean;
	}
	interface X109Status {
		status_charger_stop_control:         boolean;
		fault_charging_system_malfunction:   boolean;
		fault_battery_incompatibility:       boolean;
		status_vehicle_connector_lock:       boolean;
		fault_station_malfunction:           boolean;
		status_station:                      boolean;
	}
	interface ChademoConnectorGpioData {
		k_line: number; d1_ev_contactor: number; d2_ev_contactor: number; plug_lock: number;
	}
	interface EvseGpioData {
		estop: number; on_off_button: number; boost_button: number;
		c1_contactor: number; c2_contactor: number; pre_ac: number;
		master_contactor: number; pca_reset: number;
	}
	interface GpioData {
		chademo_connector: ChademoConnectorGpioData;
		evse_io: EvseGpioData;
	}
	interface ChademoData {
		state: string | object;
		soc: number;
		current_amps: number;
		x100: { minimum_charge_current: number; minimum_battery_voltage: number; maximum_battery_voltage: number; constant_of_charging_rate_indication: number; };
		x102: { control_protocol_number_ev: number; target_battery_voltage: number; charging_current_request: number; faults: X102Faults; status: X102Status; state_of_charge: number; };
		x200: { maximum_discharge_current: number; minimum_discharge_voltage: number; minimum_battery_discharge_level: number; max_remaining_capacity_for_charging: number; };
		x108: { available_output_current: number; avaible_output_voltage: number; welding_detection: number; threshold_voltage: number; };
		x109: { status: X109Status; output_voltage: number; output_current: number; remaining_charging_time_10s_bit: number; remaining_charging_time_1min_bit: number; };
		x208: { discharge_current: number; input_voltage: number; input_current: number; lower_threshold_voltage: number; };
		x209: { sequence: number; remaing_discharge_time: number; };
		gpio: GpioData;
	}
	interface PreData {
		state: string;
		temp: number;
		ac_amps: number;
		dc_output_volts: number;
		dc_output_amps: number;
		dc_output_volts_setpoint: number;
		dc_output_amps_setpoint: number;
		dc_bus_volts: number;
		enabled: boolean;
		fan_duty: number;
		status: [number, number];
	}
	interface MeterData {
		total_w: number | null;
		phase_w: number | null;
		charger_v: number | null;
		charger_a: number | null;
		charger_w: number | null;
		efficiency: number | null;
	}
	interface SupervisoryData {
		smart_charge_request: boolean;        smart_charge_active: boolean;
		ev_drain_protection_request: boolean; ev_drain_protection_active: boolean;
		smart_export_request: boolean;        smart_export_active: boolean;
		smart_export_excess_solar_request: boolean; smart_export_excess_solar_active: boolean;
		ready_to_drive_request: boolean;      ready_to_drive_active: boolean;
		off_peak_charging_request: boolean;   off_peak_charging_active: boolean;
	}
	interface RealTimeData {
		time?: string;
		chademo?: ChademoData;
		pre?: PreData;
		meter?: MeterData;
		supervisory?: SupervisoryData;
		settings?: OperatorSettings;
	}

	let socket: WebSocket;
	let wsConnected = false;
	let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	let ackError = false;
	let ackErrorTimer: ReturnType<typeof setTimeout> | null = null;

	let v2h_soc_min = 31;
	let v2h_soc_max = 90;
	let v2hBoostSocMax = 80;
	let selfUse = true;
	let exportExcessSolar = false;
	let evDrainProtection = false;
	let readyToDrive = false;
	let readyToDriveSoc = 90;
	let readyToDriveEndTime = '08:00';
	let readyToDriveStartTime = '--:--';
	let OffPeakCharging = true;
	let smartCharge = true;
	let smartExport = false;
	let smartExportLimit = 2500;
	let smartExportSocMin = 31;
	let showSmartExportOptions = false;
	let smartExportExcessSolar = false;
	let showSmartExportExcessSolarOptions = false;
	let showV2hSettings = false;
	let showChargeSettings = false;
	let showReadyToDriveOptions = false;
	let readyToDriveDays = [false, false, false, false, false, false, false];
	let showOffPeakOptions = false;
	let showSmartChargeOptions = false;
	let showEvDrainProtectionOptions = false;
	let offPeakStart = '00:30';
	let offPeakEnd = '04:30';
	let v2hMaxAmps = 16;
	let amps_value = 16;
	let soc_range_value = 90;
	let chargeEco = false;

	// Latest snapshot — used for settings apply
	let latest: RealTimeData | null = null;

	// Snapshot state for the main cards
	let snapshotMode = '';
	let snapshotSoc = 0;
	let snapshotDcKw = 0;
	let snapshotMeterW = 0;
	let snapshotPhaseW = 0;
	let snapshotChargerW = 0;
	let snapshotSmartChargeRequest = false;
	let snapshotSmartChargeActive = false;
	let snapshotEvDrainProtectionRequest = false;
	let snapshotEvDrainProtectionActive = false;
	let snapshotSmartExportRequest = false;
	let snapshotSmartExportActive = false;
	let snapshotSmartExportExcessSolarRequest = false;
	let snapshotSmartExportExcessSolarActive = false;
	let snapshotReadyToDriveRequest = false;
	let snapshotReadyToDriveActive = false;
	let snapshotOffPeakChargingRequest = false;
	let snapshotOffPeakChargingActive = false;

	function ledClass(request: boolean, active: boolean): string {
		if (active)  return 'inline-block w-2 h-2 rounded-full bg-success-500 ml-1.5 flex-shrink-0';
		if (request) return 'inline-block w-2 h-2 rounded-full ring-1 ring-success-500 ml-1.5 flex-shrink-0';
		return 'inline-block w-2 h-2 rounded-full bg-surface-400 ml-1.5 flex-shrink-0 opacity-30';
	}

	$: operationalMode.set(snapshotMode);

	const DC_BAR_MAX_W = 5600;
	$: dcBarPct   = Math.min(Math.abs(snapshotDcKw) / DC_BAR_MAX_W * 50, 50);
	$: dcBarStyle = snapshotDcKw >= 0 ? `bottom: 50%; height: ${dcBarPct}%` : `top: 50%; height: ${dcBarPct}%`;
	$: dcBarColor = snapshotDcKw > 50 ? 'bg-emerald-500' : snapshotDcKw < -50 ? 'bg-blue-400' : 'bg-surface-400';

	$: gridBarPct   = Math.min(Math.abs(snapshotMeterW) / DC_BAR_MAX_W * 50, 50);
	$: gridBarStyle = snapshotMeterW >= 0 ? `bottom: 50%; height: ${gridBarPct}%` : `top: 50%; height: ${gridBarPct}%`;
	$: gridBarColor = snapshotMeterW > 50 ? 'bg-amber-500' : snapshotMeterW < -50 ? 'bg-emerald-500' : 'bg-surface-400';

	$: phaseBarPct   = Math.min(Math.abs(snapshotPhaseW) / DC_BAR_MAX_W * 50, 50);
	$: phaseBarStyle = snapshotPhaseW >= 0 ? `bottom: 50%; height: ${phaseBarPct}%` : `top: 50%; height: ${phaseBarPct}%`;
	$: phaseBarColor = snapshotPhaseW > 50 ? 'bg-amber-500' : snapshotPhaseW < -50 ? 'bg-emerald-500' : 'bg-surface-400';

	$: acBarPct   = Math.min(Math.abs(snapshotChargerW) / DC_BAR_MAX_W * 50, 50);
	$: acBarStyle = snapshotChargerW >= 0 ? `bottom: 50%; height: ${acBarPct}%` : `top: 50%; height: ${acBarPct}%`;
	$: acBarColor = snapshotChargerW > 50 ? 'bg-emerald-500' : snapshotChargerW < -50 ? 'bg-blue-400' : 'bg-surface-400';

	$: activeModeLabel =
		snapshotReadyToDriveActive           ? 'Ready to Drive' :
		snapshotOffPeakChargingActive        ? 'Off-Peak Charging' :
		snapshotSmartExportActive            ? 'Smart Export' :
		snapshotSmartExportExcessSolarActive ? 'Smart Export Excess Solar' :
		snapshotSmartChargeActive            ? 'Smart Charge' :
		snapshotEvDrainProtectionActive      ? 'EV Drain Protection' :
		selfUse                              ? 'Self-Use' : '—';

	$: batteryFillW = Math.max(0, (snapshotSoc / 100) * 38);
	$: batteryColor = snapshotSoc > 50 ? '#22c55e' : snapshotSoc > 25 ? '#f59e0b' : '#ef4444';
	$: batteryArrowColor = snapshotDcKw > 0 ? '#10b981' : '#60a5fa';

	$: modePillClass =
		snapshotMode === 'V2h'    ? 'bg-emerald-600 text-white' :
		snapshotMode === 'Charge' ? 'bg-blue-600 text-white' :
		snapshotMode === 'Idle'   ? 'bg-white text-black border-2 border-black' :
		'border-2 border-surface-400 text-surface-400';

	let eventData = writable<EventData[]>([]);
	let realTimeData = writable<RealTimeData[]>([]);

	function sendSettings() {
		if (!socket || socket.readyState !== WebSocket.OPEN) return;
		socket.send(JSON.stringify({
			cmd: {
				SetSettings: {
					v2h_soc_min:          v2h_soc_min,
					v2h_soc_max:          v2h_soc_max,
					v2h_soc_max_boost:    v2hBoostSocMax,
					v2h_max_amps:         v2hMaxAmps,
					self_use:             selfUse,
					export_excess_solar:  exportExcessSolar,
					ev_drain_protection:  evDrainProtection,
					ready_to_drive:            readyToDrive,
					ready_to_drive_end_time:   readyToDriveEndTime,
					ready_to_drive_start_time: readyToDriveStartTime,
					ready_to_drive_soc:        readyToDriveSoc,
					ready_to_drive_days:       readyToDriveDays,
					off_peak_charging:    OffPeakCharging,
					off_peak_start:       offPeakStart,
					off_peak_end:         offPeakEnd,
					smart_charge:         smartCharge,
					smart_export:                smartExport,
					smart_export_limit_w:        smartExportLimit,
					smart_export_soc_min:        smartExportSocMin,
					smart_export_excess_solar:   smartExportExcessSolar,
					charge_soc_limit:            soc_range_value,
					charge_amps:          amps_value,
					charge_eco:           chargeEco,
				}
			}
		}));
	}

	function applySettings(s: OperatorSettings) {
		v2h_soc_min        = s.v2h_soc_min;
		v2h_soc_max        = s.v2h_soc_max;
		v2hBoostSocMax     = s.v2h_soc_max_boost;
		v2hMaxAmps         = s.v2h_max_amps;
		selfUse            = s.self_use;
		exportExcessSolar  = s.export_excess_solar;
		evDrainProtection  = s.ev_drain_protection;
		readyToDrive          = s.ready_to_drive;
		readyToDriveEndTime   = s.ready_to_drive_end_time;
		readyToDriveStartTime = s.ready_to_drive_start_time;
		readyToDriveSoc       = s.ready_to_drive_soc;
		readyToDriveDays      = s.ready_to_drive_days;
		OffPeakCharging    = s.off_peak_charging;
		offPeakStart       = s.off_peak_start;
		offPeakEnd         = s.off_peak_end;
		smartCharge        = s.smart_charge;
		smartExport             = s.smart_export;
		smartExportLimit        = s.smart_export_limit_w;
		smartExportSocMin       = s.smart_export_soc_min;
		smartExportExcessSolar  = s.smart_export_excess_solar;
		soc_range_value         = s.charge_soc_limit;
		amps_value         = s.charge_amps;
		chargeEco          = s.charge_eco;
	}

	function submitCharge(event: Event) {
		event.preventDefault();
		socket.send(JSON.stringify({ cmd: { SetMode: 'Charge' } }));
	}

	function sendModeChange(mode: string) {
		if (!socket || socket.readyState !== WebSocket.OPEN) return;
		let payload;
		if (mode === 'Discharge') {
			payload = { cmd: { SetMode: { Discharge: { soc_limit: soc_range_value, amps: amps_value } } } };
		} else {
			payload = { cmd: { SetMode: mode } };
		}
		socket.send(JSON.stringify(payload));
	}

	function connect() {
		if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null; }
		if (typeof WebSocket === 'undefined') return;

		socket = new WebSocket('ws://192.168.10.101:5555');

		socket.addEventListener('open', () => {
			wsConnected = true;
			socket.send(JSON.stringify({ cmd: 'GetEvents' }));
		});

		socket.addEventListener('message', (event: MessageEvent) => {
			const message = JSON.parse(event.data);
			if (message.ack === 'err') {
				ackError = true;
				if (ackErrorTimer) clearTimeout(ackErrorTimer);
				ackErrorTimer = setTimeout(() => { ackError = false; }, 4000);
			}
			if (message.Events) {
				eventData.set(message.Events);
			}
			if (message.Data) {
				const d: RealTimeData = message.Data;
				d.time = new Date().toLocaleTimeString();
				latest = d;
				realTimeData.update((items) => {
					items.unshift(d);
					if (items.length > 80) items.pop();
					return items;
				});
				const rawState = d.chademo?.state;
				snapshotMode = typeof rawState === 'object' && rawState !== null
					? Object.keys(rawState)[0]
					: (rawState ?? '');
				snapshotSoc    = d.chademo?.soc ?? 0;
				snapshotDcKw   = (d.pre?.dc_output_volts ?? 0) * (d.pre?.dc_output_amps ?? 0);
				snapshotMeterW = d.meter?.total_w ?? 0;
				snapshotPhaseW   = d.meter?.phase_w   ?? 0;
				snapshotChargerW = d.meter?.charger_w ?? 0;
				snapshotSmartChargeRequest              = d.supervisory?.smart_charge_request ?? false;
				snapshotSmartChargeActive               = d.supervisory?.smart_charge_active ?? false;
				snapshotEvDrainProtectionRequest        = d.supervisory?.ev_drain_protection_request ?? false;
				snapshotEvDrainProtectionActive         = d.supervisory?.ev_drain_protection_active ?? false;
				snapshotSmartExportRequest              = d.supervisory?.smart_export_request ?? false;
				snapshotSmartExportActive               = d.supervisory?.smart_export_active ?? false;
				snapshotSmartExportExcessSolarRequest   = d.supervisory?.smart_export_excess_solar_request ?? false;
				snapshotSmartExportExcessSolarActive    = d.supervisory?.smart_export_excess_solar_active ?? false;
				snapshotReadyToDriveRequest             = d.supervisory?.ready_to_drive_request ?? false;
				snapshotReadyToDriveActive              = d.supervisory?.ready_to_drive_active ?? false;
				snapshotOffPeakChargingRequest          = d.supervisory?.off_peak_charging_request ?? false;
				snapshotOffPeakChargingActive           = d.supervisory?.off_peak_charging_active ?? false;
				if (d.settings) applySettings(d.settings);
			}
		});

		socket.addEventListener('close', () => {
			wsConnected = false;
			latest = null;
			reconnectTimer = setTimeout(connect, 5000);
		});

		socket.addEventListener('error', () => {
			wsConnected = false;
		});
	}

	connect();
</script>

{#if !wsConnected}
<div class="fixed top-0 left-0 right-0 z-50 bg-error-500 text-white text-center py-3 px-4 font-semibold shadow-lg">
	E-Stop active or charger offline — twist E-Stop to release &nbsp;·&nbsp; <span class="font-normal opacity-80">Reconnecting…</span>
</div>
{/if}
{#if ackError}
<div class="fixed top-0 left-0 right-0 z-50 bg-warning-500 text-white text-center py-3 px-4 font-semibold shadow-lg">
	Command rejected by charger
</div>
{/if}

<br />
<div class="container mx-auto px-4 flex flex-wrap gap-2 justify-center" class:mt-12={!wsConnected}>

	<!-- Operational Mode Card -->
	<div class="card p-4 text-center w-72 transition-opacity {!wsConnected ? 'opacity-40' : ''}">
		<div class="text-2xl font-bold mb-1 inline-block rounded-lg px-4 py-1 {modePillClass}">{snapshotMode || '—'}</div>
		<div class="text-xs text-surface-400 mb-4">{activeModeLabel}</div>
		<div class="text-sm font-semibold text-surface-500 dark:text-surface-400 mb-1">Battery</div>
		<div class="text-2xl font-bold mb-1">{Math.round(snapshotSoc)}%</div>
		<div class="flex items-center justify-center gap-2 mb-3">
			<svg viewBox="0 0 50 24" class="w-20 h-10 text-surface-500 dark:text-surface-300">
				<defs><clipPath id="bat-clip"><rect x="3" y="5" width="38" height="14" rx="1.5"/></clipPath></defs>
				<rect x="1" y="3" width="42" height="18" rx="3" stroke="currentColor" stroke-width="2" fill="none"/>
				<rect x="43" y="9" width="6" height="6" rx="1" fill="currentColor"/>
				<rect x="3" y="5" width={batteryFillW} height="14" rx="1.5" fill={batteryColor}/>
				{#if snapshotDcKw > 0}
					<polygon points="{3 + batteryFillW + 1},{7} {3 + batteryFillW + 6},{12} {3 + batteryFillW + 1},{17}" fill={batteryArrowColor} clip-path="url(#bat-clip)"/>
				{:else if snapshotDcKw < 0}
					<polygon points="{3 + batteryFillW + 6},{7} {3 + batteryFillW + 1},{12} {3 + batteryFillW + 6},{17}" fill={batteryArrowColor} clip-path="url(#bat-clip)"/>
				{/if}
			</svg>
			<div class="flex flex-col gap-0.5 text-surface-400 leading-none" style="font-size:0.6rem;">
				<span>⊹ {latest?.pre?.temp != null ? Math.round(latest.pre.temp) : '—'}°C</span>
				<span>⟳ {latest?.pre?.fan_duty ?? '—'}%</span>
			</div>
		</div>
		<div class="grid grid-cols-4 gap-3 mt-3">
			<!-- titles row -->
			<div class="col-span-2 text-center text-sm font-semibold text-surface-500 dark:text-surface-400">Battery</div>
			<div class="text-center text-sm font-semibold text-surface-500 dark:text-surface-400">Grid</div>
			<div class="text-center text-sm font-semibold text-surface-500 dark:text-surface-400">Phase</div>
			<!-- subtitle row -->
			<div class="col-span-2 text-center text-xs text-surface-400">{snapshotDcKw > 0 ? 'Charging' : snapshotDcKw < 0 ? 'Discharging' : '—'}</div>
			<div class="text-center text-xs text-surface-400">{snapshotMeterW > 0 ? 'Import' : snapshotMeterW < 0 ? 'Export' : '—'}</div>
			<div class="text-center text-xs text-surface-400">{snapshotPhaseW > 0 ? 'Import' : snapshotPhaseW < 0 ? 'Export' : '—'}</div>
			<!-- bars row -->
			<div class="flex justify-center">
				<div class="relative w-6 h-28 bg-surface-300 rounded-full overflow-hidden">
					<div class="absolute left-0 w-full {dcBarColor}" style="{dcBarStyle}"></div>
					<div class="absolute left-0 w-full top-1/2 h-px bg-surface-500"></div>
				</div>
			</div>
			<div class="flex justify-center">
				<div class="relative w-6 h-28 bg-surface-300 rounded-full overflow-hidden">
					<div class="absolute left-0 w-full {acBarColor}" style="{acBarStyle}"></div>
					<div class="absolute left-0 w-full top-1/2 h-px bg-surface-500"></div>
				</div>
			</div>
			<div class="flex justify-center">
				<div class="relative w-6 h-28 bg-surface-300 rounded-full overflow-hidden">
					<div class="absolute left-0 w-full {gridBarColor}" style="{gridBarStyle}"></div>
					<div class="absolute left-0 w-full top-1/2 h-px bg-surface-500"></div>
				</div>
			</div>
			<div class="flex justify-center">
				<div class="relative w-6 h-28 bg-surface-300 rounded-full overflow-hidden">
					<div class="absolute left-0 w-full {phaseBarColor}" style="{phaseBarStyle}"></div>
					<div class="absolute left-0 w-full top-1/2 h-px bg-surface-500"></div>
				</div>
			</div>
			<!-- values row -->
			<div class="col-span-2 flex justify-around items-end">
				<div class="text-center"><div class="text-lg font-bold">{Math.abs(Math.round(snapshotDcKw))}</div><div class="text-xs text-surface-400">DC W</div></div>
				<div class="text-center"><div class="text-xs text-surface-400">Eff</div><div class="text-xs">{latest?.meter?.efficiency != null ? latest.meter.efficiency.toFixed(0) + '%' : '—'}</div></div>
				<div class="text-center"><div class="text-lg font-bold">{Math.abs(Math.round(snapshotChargerW))}</div><div class="text-xs text-surface-400">AC W</div></div>
			</div>
			<div class="text-center"><div class="text-lg font-bold">{Math.abs(Math.round(snapshotMeterW))}</div><div class="text-xs text-surface-400">W</div></div>
			<div class="text-center"><div class="text-lg font-bold">{Math.abs(Math.round(snapshotPhaseW))}</div><div class="text-xs text-surface-400">W</div></div>
		</div>
	</div>

	<!--Smart Self-Powered Card-->
	<div class="card p-4 w-72 transition-opacity {!wsConnected ? 'opacity-40 pointer-events-none' : ''}"  >
		<div class="flex justify-between mb-4">
			<button class="btn w-24 {snapshotMode === 'Idle' ? 'bg-white text-black border-2 border-black' : 'variant-filled'}" on:click={() => sendModeChange('Idle')}>Idle</button>
			<button class="btn w-24 {snapshotMode === 'V2h' ? 'bg-emerald-600 text-white' : 'variant-filled'}" on:click={() => sendModeChange('V2h')}>V2H</button>
		</div>

		<div class="text-sm mt-2">
			<div class="flex items-center">
				<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors" on:click={() => (showV2hSettings = !showV2hSettings)}>
					<span class="text-xs w-3 text-center">{showV2hSettings ? '▼' : '▶'}</span>
					<span>Settings</span>
				</button>
			</div>
			{#if showV2hSettings}
				<div class="mt-2 ml-4 flex flex-col gap-2">
					<RangeSlider name="soc-min" bind:value={v2h_soc_min} on:change={sendSettings} min={0} max={100} step={5} ticked>
						<div class="flex justify-between items-center"><div class="text-xs">SoC Min</div><div class="text-xs">{v2h_soc_min}%</div></div>
					</RangeSlider>
					<RangeSlider name="soc-max" bind:value={v2h_soc_max} on:change={sendSettings} min={0} max={100} step={5} ticked>
						<div class="flex justify-between items-center"><div class="text-xs">SoC Max (Solar)</div><div class="text-xs">{v2h_soc_max}%</div></div>
					</RangeSlider>
					<RangeSlider name="v2h-amps" bind:value={v2hMaxAmps} on:change={sendSettings} min={0} max={16} step={1} ticked>
						<div class="flex justify-between items-center"><div class="text-xs">Max Amps</div><div class="text-xs">{v2hMaxAmps}A</div></div>
					</RangeSlider>
				</div>
			{/if}
		</div>

		<div class="mt-4 flex flex-col gap-3">
			<div class="flex justify-between items-center text-sm">
				<span class="flex items-center gap-1" title="Discharge battery to offset house use">
					Self-Use <span class={ledClass(false, snapshotDcKw < -50)}></span>
				</span>
				<label class="relative inline-flex items-center cursor-pointer">
					<input type="checkbox" bind:checked={selfUse} class="sr-only peer" on:change={sendSettings} />
					<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
				</label>
			</div>

			<div class="flex justify-between items-center text-sm">
				<span class="flex items-center gap-1" title="Allow excess solar to be exported">
					Export Excess Solar <span class={ledClass(false, snapshotMeterW < -50)}></span>
				</span>
				<label class="relative inline-flex items-center cursor-pointer">
					<input type="checkbox" bind:checked={exportExcessSolar} class="sr-only peer" on:change={sendSettings} />
					<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
				</label>
			</div>

			<div class="mt-1 text-xs font-semibold text-surface-500 dark:text-surface-400">Time / Event Based Controls</div>

			<div class="text-sm">
				<div class="flex items-center justify-between">
					<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors mr-2" on:click={() => (showReadyToDriveOptions = !showReadyToDriveOptions)}>
						<span class="text-xs w-3 text-center">{showReadyToDriveOptions ? '▼' : '▶'}</span>
						<span>Ready to Drive</span>
						<span class={ledClass(snapshotReadyToDriveRequest, snapshotReadyToDriveActive)}></span>
					</button>
					<label class="relative inline-flex items-center cursor-pointer">
						<input type="checkbox" bind:checked={readyToDrive} class="sr-only peer" on:change={sendSettings} />
						<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
					</label>
				</div>
				{#if showReadyToDriveOptions}
					<div class="mt-2 ml-4 flex flex-col gap-2 text-surface-600 dark:text-surface-300">
						<div class="mt-1">
							<RangeSlider name="rtd-soc" bind:value={readyToDriveSoc} on:change={sendSettings} min={10} max={100} step={5} ticked>
								<div class="flex justify-between items-center"><div class="text-xs">Target SoC</div><div class="text-xs">{readyToDriveSoc}%</div></div>
							</RangeSlider>
						</div>
						<div class="flex justify-between items-center">
							<span>Start charge</span>
							<input type="time" value={readyToDriveStartTime} disabled class="input w-28 text-sm py-0.5 px-1 opacity-50 cursor-not-allowed" />
						</div>
						<div class="flex justify-between items-center">
							<span>Ready by</span>
							<input type="time" bind:value={readyToDriveEndTime} on:change={sendSettings} class="input w-28 text-sm py-0.5 px-1" />
						</div>
						<div class="flex items-center gap-2">
							{#each ['M','T','W','T','F','S','S'] as day, i}
								<label class="flex flex-col items-center cursor-pointer">
									<span class="text-xs mb-0.5">{day}</span>
									<input type="checkbox" bind:checked={readyToDriveDays[i]} class="w-4 h-4 accent-primary-500" on:change={sendSettings} />
								</label>
							{/each}
						</div>
					</div>
				{/if}
			</div>

			<div class="text-sm">
				<div class="flex items-center justify-between">
					<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors mr-2" on:click={() => (showOffPeakOptions = !showOffPeakOptions)}>
						<span class="text-xs w-3 text-center">{showOffPeakOptions ? '▼' : '▶'}</span>
						<span>Off-Peak Charging</span>
						<span class={ledClass(snapshotOffPeakChargingRequest, snapshotOffPeakChargingActive)}></span>
					</button>
					<label class="relative inline-flex items-center cursor-pointer">
						<input type="checkbox" bind:checked={OffPeakCharging} class="sr-only peer" on:change={sendSettings} />
						<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
					</label>
				</div>
				{#if showOffPeakOptions}
					<div class="mt-2 ml-4 flex flex-col gap-2 text-surface-600 dark:text-surface-300">
						<RangeSlider name="soc-max-boost-offpeak" bind:value={v2hBoostSocMax} on:change={sendSettings} min={0} max={100} step={5} ticked>
							<div class="flex justify-between items-center"><div class="text-xs">SoC Max (Off-Peak / Smart Charge)</div><div class="text-xs">{v2hBoostSocMax}%</div></div>
						</RangeSlider>
						<div class="flex justify-between items-center"><span>Start</span><input type="time" bind:value={offPeakStart} on:change={sendSettings} class="input w-28 text-sm py-0.5 px-1" /></div>
						<div class="flex justify-between items-center"><span>End</span><input type="time" bind:value={offPeakEnd} on:change={sendSettings} class="input w-28 text-sm py-0.5 px-1" /></div>
					</div>
				{/if}
			</div>

			<div class="mt-1 text-xs font-semibold text-surface-500 dark:text-surface-400">Smart</div>

			<div class="text-sm">
				<div class="flex items-center justify-between">
					<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors mr-2" on:click={() => (showSmartExportOptions = !showSmartExportOptions)}>
						<span class="text-xs w-3 text-center">{showSmartExportOptions ? '▼' : '▶'}</span>
						<span>Smart Export</span>
						<span class={ledClass(snapshotSmartExportRequest, snapshotSmartExportActive)}></span>
					</button>
					<label class="relative inline-flex items-center cursor-pointer">
						<input type="checkbox" bind:checked={smartExport} class="sr-only peer" on:change={sendSettings} />
						<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
					</label>
				</div>
				{#if showSmartExportOptions}
					<div class="mt-1 ml-4 text-xs text-surface-400">Export when prices are high</div>
					<div class="mt-2 ml-4 flex flex-col gap-2 text-surface-600 dark:text-surface-300">
						<RangeSlider name="smart-export-soc-min" bind:value={smartExportSocMin} on:change={sendSettings} min={0} max={100} step={5} ticked>
							<div class="flex justify-between items-center"><div class="text-xs">SoC Min (Smart Export)</div><div class="text-xs">{smartExportSocMin}%</div></div>
						</RangeSlider>
						<div class="flex justify-between items-center">
							<span>Export Limit</span>
							<div class="flex items-center gap-1">
								<input type="number" bind:value={smartExportLimit} on:change={sendSettings} min="0" max="10000" step="100" class="input w-20 text-right text-sm py-0.5 px-1" />
								<span class="text-xs">W</span>
							</div>
						</div>
					</div>
				{/if}
			</div>

			<div class="text-sm">
				<div class="flex items-center justify-between">
					<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors mr-2" on:click={() => (showSmartExportExcessSolarOptions = !showSmartExportExcessSolarOptions)}>
						<span class="text-xs w-3 text-center">{showSmartExportExcessSolarOptions ? '▼' : '▶'}</span>
						<span>Smart Export Excess Solar</span>
						<span class={ledClass(snapshotSmartExportExcessSolarRequest, snapshotSmartExportExcessSolarActive)}></span>
					</button>
					<label class="relative inline-flex items-center cursor-pointer">
						<input type="checkbox" bind:checked={smartExportExcessSolar} class="sr-only peer" on:change={sendSettings} />
						<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
					</label>
				</div>
				{#if showSmartExportExcessSolarOptions}
					<div class="mt-1 ml-4 text-xs text-surface-400">Exports when solar export rate &gt; overnight import rate</div>
				{/if}
			</div>

			<div class="text-sm">
				<div class="flex items-center justify-between">
					<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors mr-2" on:click={() => (showSmartChargeOptions = !showSmartChargeOptions)}>
						<span class="text-xs w-3 text-center">{showSmartChargeOptions ? '▼' : '▶'}</span>
						<span>Smart Charge</span>
						<span class={ledClass(snapshotSmartChargeRequest, snapshotSmartChargeActive)}></span>
					</button>
					<label class="relative inline-flex items-center cursor-pointer">
						<input type="checkbox" bind:checked={smartCharge} class="sr-only peer" on:change={sendSettings} />
						<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
					</label>
				</div>
				{#if showSmartChargeOptions}
					<div class="mt-1 ml-4 text-xs text-surface-400">Charge during cheap import slots</div>
					<div class="mt-2 ml-4 flex flex-col gap-2 text-surface-600 dark:text-surface-300">
						<RangeSlider name="soc-max-boost-smartcharge" bind:value={v2hBoostSocMax} on:change={sendSettings} min={0} max={100} step={5} ticked>
							<div class="flex justify-between items-center"><div class="text-xs">SoC Max (Off-Peak / Smart Charge)</div><div class="text-xs">{v2hBoostSocMax}%</div></div>
						</RangeSlider>
					</div>
				{/if}
			</div>

			<div class="text-sm">
				<div class="flex items-center justify-between">
					<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors mr-2" on:click={() => (showEvDrainProtectionOptions = !showEvDrainProtectionOptions)}>
						<span class="text-xs w-3 text-center">{showEvDrainProtectionOptions ? '▼' : '▶'}</span>
						<span>EV Drain Protection</span>
						<span class={ledClass(snapshotEvDrainProtectionRequest, snapshotEvDrainProtectionActive)}></span>
					</button>
					<label class="relative inline-flex items-center cursor-pointer">
						<input type="checkbox" bind:checked={evDrainProtection} class="sr-only peer" on:change={sendSettings} />
						<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
					</label>
				</div>
				{#if showEvDrainProtectionOptions}
					<div class="mt-1 ml-4 text-xs text-surface-400">Prevents discharge into EV charging load</div>
				{/if}
			</div>
		</div>
	</div>

	<!-- Charge Card -->
	<div class="card p-4 w-72 transition-opacity {!wsConnected ? 'opacity-40 pointer-events-none' : ''}"  >
		<div class="flex justify-end mb-4">
			<button class="btn w-24 {snapshotMode === 'Charge' ? 'bg-blue-600 text-white' : 'variant-filled'}" on:click={submitCharge}>Boost</button>
		</div>
		<div class="text-sm mt-2">
			<div class="flex items-center">
				<button type="button" class="flex items-center gap-1 hover:text-primary-500 transition-colors" on:click={() => (showChargeSettings = !showChargeSettings)}>
					<span class="text-xs w-3 text-center">{showChargeSettings ? '▼' : '▶'}</span>
					<span>Settings</span>
				</button>
			</div>
			{#if showChargeSettings}
				<div class="mt-2 ml-4 flex flex-col gap-2">
					<RangeSlider name="soc" id="range-slider-boost-soc" bind:value={soc_range_value} on:change={sendSettings} min={10} max={100} step={5} ticked>
						<div class="flex justify-between items-center"><div class="text-xs">SoC Max</div><div class="text-xs">{soc_range_value} / 100</div></div>
					</RangeSlider>
					<RangeSlider name="amps" id="range-slider-amps" bind:value={amps_value} on:change={sendSettings} max={16} step={1} ticked>
						<div class="flex justify-between items-center"><div class="text-xs">Max Amps</div><div class="text-xs">{amps_value} / 16</div></div>
					</RangeSlider>
				</div>
			{/if}
		</div>
		<div class="mt-3 flex justify-between items-center text-sm">
			<span title="Charge using solar power only" class="mr-2">Eco (Use Solar Only)</span>
			<label class="relative inline-flex items-center cursor-pointer">
				<input type="checkbox" bind:checked={chargeEco} class="sr-only peer" on:change={sendSettings} />
				<div class="w-9 h-5 bg-surface-300 rounded-full peer peer-checked:bg-primary-500 after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:rounded-full after:h-4 after:w-4 after:transition-all peer-checked:after:translate-x-4" />
			</label>
		</div>
	</div>

	<!-- Event table: display only. Edit/delete/add not yet implemented. -->
	{#if $eventData.length > 0}
	<div class="flex-auto card p-10 max-h-[60vh] overflow-y-auto space-y-4">
		<h2>Event Table</h2>
		<table id="eventsTable" class="table table-hover">
			<thead>
				<tr>
					<th>Time</th><th>Action</th>
				</tr>
			</thead>
			<tbody>
				{#each $eventData as event}
					<tr>
						<td>{event.time}</td>
						<td>{event.action}</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
	{/if}

</div>

<!-- Historical data table -->
<div class="container mx-auto px-4 mt-2">
	<div class="card p-4" style="max-height: 60vh; overflow-y: auto;">
		<div style="overflow-x: auto;">
			<table id="dataTable" style="white-space: nowrap; border-collapse: collapse; width: 100%;">
				<thead>
					<tr style="border-bottom: 1px solid var(--color-surface-600);">
						<th class="px-2 py-1 text-left text-xs font-semibold text-surface-400 align-bottom" rowspan="2">Time</th>
						<th class="px-2 py-1 text-left text-xs font-semibold text-surface-400 align-bottom" rowspan="2" style="border-left: 1px solid var(--color-surface-600);">State</th>
						<th class="px-2 py-1 text-left text-xs font-semibold text-primary-500" colspan="2" style="border-left: 1px solid var(--color-surface-600);">CHAdeMO</th>
						<th class="px-2 py-1 text-left text-xs font-semibold text-secondary-500" colspan="6" style="border-left: 1px solid var(--color-surface-600);">PRE Charger</th>
						<th class="px-2 py-1 text-left text-xs font-semibold text-tertiary-500" colspan="2" style="border-left: 1px solid var(--color-surface-600);">Meter</th>
						<th class="px-2 py-1 text-left text-xs font-semibold text-success-500" colspan="4" style="border-left: 1px solid var(--color-surface-600);">Supervisory</th>
					</tr>
					<tr style="border-bottom: 1px solid var(--color-surface-600);">
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400" style="border-left: 1px solid var(--color-surface-600);">SoC</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">Req A</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400" style="border-left: 1px solid var(--color-surface-600);">State</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">DC V SP</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">DC Out V</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">DC A SP</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">DC Out A</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">DC W</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400" style="border-left: 1px solid var(--color-surface-600);">Chgr W</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">Total W</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400" style="border-left: 1px solid var(--color-surface-600);">SE Req</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">SEES Req</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">SC Req</th>
						<th class="px-2 py-1 text-left text-xs font-normal text-surface-400">EDP Req</th>
					</tr>
				</thead>
				<tbody>
					{#each $realTimeData as rtd}
						<tr style="border-bottom: 1px solid var(--color-surface-700);">
							<td class="px-2 py-0.5 text-xs font-mono text-surface-400">{rtd.time}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{typeof rtd.chademo?.state === 'object' && rtd.chademo?.state !== null ? Object.keys(rtd.chademo.state)[0] : rtd.chademo?.state ?? '—'}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{Math.round(rtd.chademo?.soc ?? 0)}%</td>
							<td class="px-2 py-0.5 text-xs font-mono">{(rtd.chademo?.x102?.charging_current_request ?? 0).toFixed(1)} A</td>
							<td class="px-2 py-0.5 text-xs font-mono">{rtd.pre?.state ?? '—'}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{(rtd.pre?.dc_output_volts_setpoint ?? 0).toFixed(1)} V</td>
							<td class="px-2 py-0.5 text-xs font-mono">{(rtd.pre?.dc_output_volts ?? 0).toFixed(1)} V</td>
							<td class="px-2 py-0.5 text-xs font-mono">{(rtd.pre?.dc_output_amps_setpoint ?? 0).toFixed(1)} A</td>
							<td class="px-2 py-0.5 text-xs font-mono">{(rtd.pre?.dc_output_amps ?? 0).toFixed(1)} A</td>
							<td class="px-2 py-0.5 text-xs font-mono">{Math.round((rtd.pre?.dc_output_volts ?? 0) * (rtd.pre?.dc_output_amps ?? 0))} W</td>
							<td class="px-2 py-0.5 text-xs font-mono">{rtd.meter?.charger_w != null ? Math.round(rtd.meter.charger_w) + ' W' : '—'}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{Math.round(rtd.meter?.total_w ?? 0)} W</td>
							<td class="px-2 py-0.5 text-xs font-mono">{rtd.supervisory?.smart_export_request ? '✓' : '—'}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{rtd.supervisory?.smart_export_excess_solar_request ? '✓' : '—'}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{rtd.supervisory?.smart_charge_request ? '✓' : '—'}</td>
							<td class="px-2 py-0.5 text-xs font-mono">{rtd.supervisory?.ev_drain_protection_request ? '✓' : '—'}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	</div>
</div>

<style lang="postcss">
</style>
