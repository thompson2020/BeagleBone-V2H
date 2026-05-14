<script lang="ts">

	// ── TypeScript interfaces ──────────────────────────────────────────────────
	interface X102Status {
		status_discharge_compatible:     boolean;
		status_normal_stop_request:      boolean;
		status_vehicle:                  boolean;
		status_charging_system:          boolean;
		status_vehicle_shifter_position: boolean;
		status_vehicle_charging:         boolean;
	}
	interface X102Faults {
		fault_battery_voltage_deviation: boolean;
		fault_high_battery_temperature:  boolean;
		fault_battery_current_deviation: boolean;
		fault_battery_undervoltage:      boolean;
		fault_battery_overvoltage:       boolean;
	}
	interface X109Status {
		status_charger_stop_control:       boolean;
		fault_charging_system_malfunction: boolean;
		fault_battery_incompatibility:     boolean;
		status_vehicle_connector_lock:     boolean;
		fault_station_malfunction:         boolean;
		status_station:                    boolean;
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
		x101: { max_charging_time_10s_bit: number; max_charging_time_1min_bit: number; estimated_charging_time: number; rated_battery_capacity: number; };
		x102: { control_protocol_number_ev: number; target_battery_voltage: number; charging_current_request: number; faults: X102Faults; status: X102Status; state_of_charge: number; };
		x200: { maximum_discharge_current: number; minimum_discharge_voltage: number; minimum_battery_discharge_level: number; max_remaining_capacity_for_charging: number; };
		x201: { sequence: number; approx_discharge_time: number; available_energy: number; };
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
		smart_charge_request: boolean;              smart_charge_active: boolean;
		ev_drain_protection_request: boolean;       ev_drain_protection_active: boolean;
		smart_export_request: boolean;              smart_export_active: boolean;
		smart_export_excess_solar_request: boolean; smart_export_excess_solar_active: boolean;
		ready_to_drive_request: boolean;            ready_to_drive_active: boolean;
		off_peak_charging_request: boolean;         off_peak_charging_active: boolean;
	}
	interface RealTimeData {
		chademo?: ChademoData;
		pre?: PreData;
		meter?: MeterData;
		supervisory?: SupervisoryData;
	}

	// ── State ─────────────────────────────────────────────────────────────────
	let latest: RealTimeData | null = null;

	// WS
	let socket: WebSocket;
	let wsConnected = false;
	let reconnectTimer: ReturnType<typeof setTimeout> | null = null;

	// ── Helper dot functions ───────────────────────────────────────────────────
	function pinDot(val: number | undefined, activeHigh: boolean = true): string {
		if (val === undefined || val === 255) return 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
		const on = activeHigh ? val === 1 : val === 0;
		return on ? 'inline-block w-3 h-3 rounded-full bg-success-500'
		          : 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
	}
	function estopDot(val: number | undefined): string {
		if (val === undefined || val === 255) return 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
		return val === 0 ? 'inline-block w-3 h-3 rounded-full bg-error-500'
		                 : 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
	}
	function faultDot(fault: boolean | undefined): string {
		return fault ? 'inline-block w-3 h-3 rounded-full bg-error-500'
		             : 'inline-block w-3 h-3 rounded-full bg-success-500';
	}
	function okDot(ok: boolean | undefined): string {
		return ok ? 'inline-block w-3 h-3 rounded-full bg-success-500'
		          : 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
	}
	function faultActiveDot(active: boolean | undefined): string {
		if (active === undefined || active === null) return 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
		return active ? 'inline-block w-3 h-3 rounded-full bg-error-500'
		              : 'inline-block w-3 h-3 rounded-full bg-surface-400 opacity-40';
	}
	function supLed(request: boolean, active: boolean): string {
		if (active)  return 'inline-block w-2.5 h-2.5 rounded-full bg-success-500';
		if (request) return 'inline-block w-2.5 h-2.5 rounded-full ring-1 ring-success-500';
		return 'inline-block w-2.5 h-2.5 rounded-full bg-surface-400 opacity-30';
	}

	// ── WebSocket ─────────────────────────────────────────────────────────────
	function connect() {
		if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null; }
		if (typeof WebSocket === 'undefined') return;
		socket = new WebSocket('ws://192.168.10.101:5555');
		socket.addEventListener('open', () => { wsConnected = true; });
		socket.addEventListener('close', () => {
			wsConnected = false;
			latest = null;
			reconnectTimer = setTimeout(connect, 5000);
		});
		socket.addEventListener('error', () => { wsConnected = false; });
		socket.addEventListener('message', (ev: MessageEvent) => {
			const msg = JSON.parse(ev.data);
			if (msg.Data) { latest = msg.Data; }
		});
	}
	connect();
</script>

<div class="overflow-x-auto">
<div class="w-fit mx-auto px-2 py-4 space-y-6">

<!-- ═══════════════════════════════════════════════════════════════════════════
     CHAdeMO section
═══════════════════════════════════════════════════════════════════════════ -->
<div>
	<div class="text-xs font-bold uppercase tracking-widest text-primary-500 mb-3 flex items-center gap-2">
		<span>CHAdeMO CAN Bus</span>
		<span class="flex-1 h-px bg-primary-500 opacity-30"></span>
		<span class="text-surface-400 font-normal normal-case tracking-normal">
			State: <span class="font-mono">{latest?.chademo ? (typeof latest.chademo.state === 'object' ? Object.keys(latest.chademo.state)[0] : latest.chademo.state) : '—'}</span>
			&nbsp;·&nbsp; SoC: <span class="font-mono">{latest?.chademo?.soc ?? '—'}%</span>
		</span>
	</div>

	<div class="flex gap-3 items-start">

		<!-- cols 1+2: 2×2 block (x100/x108 top, x101/CHAdeMO IO bottom) ─────── -->
		<div class="flex flex-col gap-3">

		<div class="flex gap-3">
		<div class="card p-4 w-64">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-success-500">x100 — Charge Request</div>
				<div class="text-xs text-surface-400">EV → EVSE</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x100}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Min charge current</span>    <span class="font-mono">{x.minimum_charge_current} A</span>
				<span class="text-surface-400 whitespace-nowrap">Min battery voltage</span>   <span class="font-mono">{x.minimum_battery_voltage.toFixed(0)} V</span>
				<span class="text-surface-400 whitespace-nowrap">Max battery voltage</span>   <span class="font-mono">{x.maximum_battery_voltage.toFixed(0)} V</span>
				<span class="text-surface-400 whitespace-nowrap">Charge rate indicator</span> <span class="font-mono">{x.constant_of_charging_rate_indication}%</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<div class="card p-4 w-64">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-secondary-500">x108 — EVSE Capabilities</div>
				<div class="text-xs text-surface-400">EVSE → EV</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x108}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Available current</span>  <span class="font-mono">{x.available_output_current} A</span>
				<span class="text-surface-400 whitespace-nowrap">Available voltage</span>  <span class="font-mono">{x.avaible_output_voltage} V</span>
				<span class="text-surface-400 whitespace-nowrap">Threshold voltage</span>  <span class="font-mono">{x.threshold_voltage} V</span>
				<span class="text-surface-400 whitespace-nowrap">Welding detection</span>  <span class="font-mono">{x.welding_detection}</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>
		</div>

		<div class="border-t border-surface-600 opacity-50"></div>

		<div class="flex gap-3">
		<div class="card p-4 w-64 self-start">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-success-500">x101 — Charge Time</div>
				<div class="text-xs text-surface-400">EV → EVSE</div>
			</div>
			{#if latest?.chademo?.x101}
			{@const x = latest.chademo.x101}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Battery capacity</span>      <span class="font-mono">{(x.rated_battery_capacity / 10).toFixed(1)} kWh</span>
				<span class="text-surface-400 whitespace-nowrap">EV est. time remain</span>   <span class="font-mono">{x.estimated_charging_time} min</span>
				<span class="text-surface-400 whitespace-nowrap">Max charge time (10s)</span> <span class="font-mono">{x.max_charging_time_10s_bit}</span>
				<span class="text-surface-400 whitespace-nowrap">Max charge time (1min)</span><span class="font-mono">{x.max_charging_time_1min_bit}</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<div class="card p-4 w-64">
			<div class="text-xs font-semibold mb-3 text-surface-400">CHAdeMO IEC 61851-23 IO</div>
			{#if latest?.chademo?.gpio}
			{@const c = latest.chademo.gpio.chademo_connector}
			<div class="flex flex-col gap-1.5 text-xs">
				<div class="text-xs font-medium text-success-500 mb-0.5 uppercase tracking-wide">Input</div>
				<div class="flex justify-between items-center gap-4">
					<span class="text-surface-400 whitespace-nowrap">K-line</span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{c.k_line === 255 ? '?' : c.k_line === 0 ? 'seq request' : 'idle'}</span>
						<span class={pinDot(c.k_line, false)}></span>
					</div>
				</div>
				<div class="text-xs font-medium text-secondary-500 mt-1 mb-0.5 uppercase tracking-wide">Outputs</div>
				<div class="flex justify-between items-center gap-4">
					<span class="text-surface-400 whitespace-nowrap">D1 EV contactor <span class="text-surface-500">(+HV)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{c.d1_ev_contactor === 255 ? '?' : c.d1_ev_contactor === 1 ? 'CLOSE' : 'open'}</span>
						<span class={pinDot(c.d1_ev_contactor)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-4">
					<span class="text-surface-400 whitespace-nowrap">D2 EV contactor <span class="text-surface-500">(-HV)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{c.d2_ev_contactor === 255 ? '?' : c.d2_ev_contactor === 1 ? 'CLOSE' : 'open'}</span>
						<span class={pinDot(c.d2_ev_contactor)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-4">
					<span class="text-surface-400 whitespace-nowrap">Plug lock solenoid</span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{c.plug_lock === 255 ? '?' : c.plug_lock === 1 ? 'LOCK' : 'open'}</span>
						<span class={pinDot(c.plug_lock)}></span>
					</div>
				</div>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>
		</div>

		</div>

		<!-- col 4: x102 ────────────────────────────────────────────────────── -->
		<div class="card p-4 w-64">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-success-500">x102 — EV Status</div>
				<div class="text-xs text-surface-400">EV → EVSE</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x102}
			<div class="grid grid-cols-2 gap-x-3 gap-y-1 text-xs mb-3">
				<span class="text-surface-400">Protocol #</span>     <span class="font-mono">{x.control_protocol_number_ev}</span>
				<span class="text-surface-400">Target voltage</span> <span class="font-mono">{x.target_battery_voltage.toFixed(0)} V</span>
				<span class="text-surface-400">Charge request</span> <span class="font-mono">{x.charging_current_request} A</span>
				<span class="text-surface-400">SoC</span>            <span class="font-mono">{x.state_of_charge}%</span>
			</div>
			<div class="text-xs font-medium text-surface-400 mb-1">Status</div>
			<div class="flex flex-col gap-0.5 text-xs mb-3">
				<div class="flex justify-between items-center"><span class="text-surface-400">Charging enabled</span>    <span class={okDot(x.status.status_vehicle_charging)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">EV contactors closed</span><span class={okDot(!x.status.status_vehicle)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">V2H compatible</span>      <span class={okDot(x.status.status_discharge_compatible)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">No stop request</span>     <span class={okDot(!x.status.status_normal_stop_request)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Charging system OK</span>  <span class={okDot(!x.status.status_charging_system)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">In Park</span>             <span class={okDot(!x.status.status_vehicle_shifter_position)}></span></div>
			</div>
			<div class="text-xs font-medium text-surface-400 mb-1">Faults</div>
			<div class="flex flex-col gap-0.5 text-xs">
				<div class="flex justify-between items-center"><span class="text-surface-400">Battery overvoltage</span>     <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.faults.fault_battery_overvoltage ? '1' : '0'}</span><span class={faultActiveDot(x.faults.fault_battery_overvoltage)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Battery undervoltage</span>    <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.faults.fault_battery_undervoltage ? '1' : '0'}</span><span class={faultActiveDot(x.faults.fault_battery_undervoltage)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Current deviation</span>       <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.faults.fault_battery_current_deviation ? '1' : '0'}</span><span class={faultActiveDot(x.faults.fault_battery_current_deviation)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">High temperature</span>        <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.faults.fault_high_battery_temperature ? '1' : '0'}</span><span class={faultActiveDot(x.faults.fault_high_battery_temperature)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Voltage deviation</span>       <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.faults.fault_battery_voltage_deviation ? '1' : '0'}</span><span class={faultActiveDot(x.faults.fault_battery_voltage_deviation)}></span></div></div>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<!-- col 5: x109 ────────────────────────────────────────────────────── -->
		<div class="card p-4 w-64">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-secondary-500">x109 — EVSE Status</div>
				<div class="text-xs text-surface-400">EVSE → EV</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x109}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs mb-3">
				<span class="text-surface-400 whitespace-nowrap">Output voltage</span>     <span class="font-mono">{x.output_voltage.toFixed(0)} V</span>
				<span class="text-surface-400 whitespace-nowrap">Output current</span>     <span class="font-mono">{x.output_current} A</span>
				<span class="text-surface-400 whitespace-nowrap">Time remain (10s)</span>  <span class="font-mono">{x.remaining_charging_time_10s_bit}</span>
				<span class="text-surface-400 whitespace-nowrap">Time remain (1min)</span> <span class="font-mono">{x.remaining_charging_time_1min_bit}</span>
			</div>
			<div class="text-xs font-medium text-surface-400 mb-1">Status</div>
			<div class="flex flex-col gap-0.5 text-xs">
				<div class="flex justify-between items-center"><span class="text-surface-400">Charging/Discharging</span>    <span class={okDot(x.status.status_station)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Starting/Stopping Charge</span> <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.status.status_charger_stop_control ? '1' : '0'}</span><span class={okDot(x.status.status_charger_stop_control)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Output energised</span>        <span class={okDot(x.status.status_vehicle_connector_lock)}></span></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">System fault</span>             <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.status.fault_charging_system_malfunction ? '1' : '0'}</span><span class={faultActiveDot(x.status.fault_charging_system_malfunction)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Battery incompatible</span>     <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.status.fault_battery_incompatibility ? '1' : '0'}</span><span class={faultActiveDot(x.status.fault_battery_incompatibility)}></span></div></div>
				<div class="flex justify-between items-center"><span class="text-surface-400">Charger error</span>            <div class="flex items-center gap-1.5"><span class="font-mono text-surface-500">{x.status.fault_station_malfunction ? '1' : '0'}</span><span class={faultActiveDot(x.status.fault_station_malfunction)}></span></div></div>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<!-- col 6: x200 + x201 ─────────────────────────────────────────────── -->
		<div class="flex flex-col gap-3 w-64">

		<div class="card p-4">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-success-500">x200 — Discharge Limits</div>
				<div class="text-xs text-surface-400">EV → EVSE</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x200}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Max discharge current</span> <span class="font-mono">{x.maximum_discharge_current} A</span>
				<span class="text-surface-400 whitespace-nowrap">Min discharge voltage</span> <span class="font-mono">{x.minimum_discharge_voltage} V</span>
				<span class="text-surface-400 whitespace-nowrap">Min discharge level</span>   <span class="font-mono">{x.minimum_battery_discharge_level}%</span>
				<span class="text-surface-400 whitespace-nowrap">Max charge capacity</span>   <span class="font-mono">{x.max_remaining_capacity_for_charging}%</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<div class="card p-4">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-success-500">x201 — V2H Status</div>
				<div class="text-xs text-surface-400">EV → EVSE</div>
			</div>
			{#if latest?.chademo?.x201}
			{@const x = latest.chademo.x201}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Sequence</span>           <span class="font-mono">{x.sequence}</span>
				<span class="text-surface-400 whitespace-nowrap">Discharge time est.</span><span class="font-mono">{x.approx_discharge_time === 0 ? '—' : x.approx_discharge_time + ' min'}</span>
				<span class="text-surface-400 whitespace-nowrap">Available energy</span>   <span class="font-mono">{x.available_energy === 0 ? '—' : (x.available_energy / 10).toFixed(1) + ' kWh'}</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		</div>

		<!-- col 7: x208 + x209 ─────────────────────────────────────────────── -->
		<div class="flex flex-col gap-3 w-64">

		<div class="card p-4">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-secondary-500">x208 — Discharge Control</div>
				<div class="text-xs text-surface-400">EVSE → EV</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x208}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Discharge current</span>   <span class="font-mono">{x.discharge_current} A</span>
				<span class="text-surface-400 whitespace-nowrap">Input voltage limit</span> <span class="font-mono">{x.input_voltage} V</span>
				<span class="text-surface-400 whitespace-nowrap">Input current limit</span> <span class="font-mono">{x.input_current} A</span>
				<span class="text-surface-400 whitespace-nowrap">Lower threshold V</span>   <span class="font-mono">{x.lower_threshold_voltage} V</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<div class="card p-4">
			<div class="flex justify-between items-baseline mb-3">
				<div class="text-xs font-semibold text-secondary-500">x209 — Discharge Sequence</div>
				<div class="text-xs text-surface-400">EVSE → EV</div>
			</div>
			{#if latest?.chademo}
			{@const x = latest.chademo.x209}
			<div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">Sequence number</span>
				<div>
					<span class="font-mono">{x.sequence}</span>
					<div class="mt-0.5 text-xs">
						{#if x.sequence === 0}
							<span class="text-surface-400">Standby</span>
						{:else if x.sequence === 1}
							<span class="text-surface-400">Initial contact</span>
						{:else if x.sequence === 2}
							<!-- TODO: confirm label for sequence 2 once discharge sequence stepping is implemented -->
						{:else if x.sequence === 3}
							<span class="text-warning-400">Insulation test</span>
						{:else if x.sequence === 4}
							<span class="text-warning-400">Pre-charge / volt match</span>
						{:else if x.sequence === 5}
							<span class="text-success-400">Active discharge</span>
						{:else if x.sequence === 6}
							<span class="text-warning-400">Welding detection</span>
						{:else if x.sequence === 7}
							<span class="text-surface-400">Stop / end session</span>
						{:else}
							<span class="text-surface-500">Unknown ({x.sequence})</span>
						{/if}
					</div>
				</div>
				<span class="text-surface-400 whitespace-nowrap">Remaining discharge time</span> <span class="font-mono">{x.remaing_discharge_time}</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		</div>

	</div>
</div>

<!-- ═══════════════════════════════════════════════════════════════════════════
     PRE / EVSE IO / Meter / Supervisory row
═══════════════════════════════════════════════════════════════════════════ -->
<div>
	<div class="flex flex-wrap gap-3 mb-3 text-xs font-bold uppercase tracking-widest">
		<div class="w-64 flex items-center gap-2">
			<span class="text-secondary-500 whitespace-nowrap">PRE Charger</span>
			<span class="flex-1 h-px bg-secondary-500 opacity-30"></span>
		</div>
		<div class="w-64 flex items-center gap-2">
			<span class="text-warning-500 whitespace-nowrap">EVSE I/O</span>
			<span class="flex-1 h-px bg-warning-500 opacity-30"></span>
		</div>
		<div class="w-64 flex items-center gap-2">
			<span class="text-tertiary-500 whitespace-nowrap">Grid Meter</span>
			<span class="flex-1 h-px bg-tertiary-500 opacity-30"></span>
		</div>
		<div class="w-64 flex items-center gap-2">
			<span class="text-success-500 whitespace-nowrap">Supervisory States</span>
			<span class="flex-1 h-px bg-success-500 opacity-30"></span>
		</div>
	</div>
	<div class="flex flex-wrap gap-3">

		<!-- PRE state & measurements ─────────────────────────────────────────── -->
		<div class="card p-4 w-64">
			<div class="text-xs font-semibold mb-3 text-secondary-500">PRE State & Measurements</div>
			{#if latest?.pre}
			{@const p = latest.pre}
			<div class="text-xs font-medium text-success-500 mb-1 uppercase tracking-wide">Inputs</div>
			<div class="grid grid-cols-2 gap-x-3 gap-y-1 text-xs mb-3">
				<span class="text-surface-400 whitespace-nowrap">State</span>         <span class="font-mono">{p.state}</span>
				<span class="text-surface-400 whitespace-nowrap">Temperature</span>   <span class="font-mono">{p.temp.toFixed(1)} °C</span>
				<span class="text-surface-400 whitespace-nowrap">AC input</span>      <span class="font-mono">{p.ac_amps.toFixed(1)} A</span>
				<span class="text-surface-400 whitespace-nowrap">DC bus</span>        <span class="font-mono">{p.dc_bus_volts.toFixed(1)} V</span>
				<span class="text-surface-400 whitespace-nowrap">Fan duty</span>      <span class="font-mono">{p.fan_duty}%</span>
				<span class="text-surface-400 whitespace-nowrap">Enabled</span>       <span class="font-mono">{p.enabled ? 'yes' : 'no'}</span>
				<span class="text-surface-400 whitespace-nowrap">Status bytes</span>
				<div>
					<span class="font-mono">0x{p.status[0].toString(16).padStart(2,'0')} 0x{p.status[1].toString(16).padStart(2,'0')}</span>
					<div class="mt-0.5">
						{#if p.status[0] === 0 && p.status[1] === 0}
							<span class="text-surface-400 text-xs">Idle / disabled</span>
						{:else if p.status[0] === 1 && p.status[1] === 0}
							<span class="text-success-400 text-xs">OK — enabled</span>
						{:else if p.status[0] === 1 && p.status[1] === 4}
							<span class="text-success-400 text-xs">Active — discharging</span>
						{:else}
							<span class="text-error-400 text-xs">Fault — unexpected state</span>
						{/if}
					</div>
				</div>
				<span class="text-surface-400 whitespace-nowrap">DC voltage</span> <span class="font-mono">{p.dc_output_volts.toFixed(1)} V</span>
				<span class="text-surface-400 whitespace-nowrap">DC current</span> <span class="font-mono">{p.dc_output_amps.toFixed(1)} A</span>
				<span class="text-surface-400 whitespace-nowrap">DC power</span>
				<span class="font-mono font-bold {(p.dc_output_volts * p.dc_output_amps) > 50 ? 'text-emerald-400' : (p.dc_output_volts * p.dc_output_amps) < -50 ? 'text-blue-400' : ''}">
					{Math.round(p.dc_output_volts * p.dc_output_amps)} W
				</span>
			</div>
			<div class="text-xs font-medium text-secondary-500 mb-1 uppercase tracking-wide">Outputs</div>
			<div class="grid grid-cols-2 gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400 whitespace-nowrap">DC voltage SP</span> <span class="font-mono">{p.dc_output_volts_setpoint.toFixed(1)} V</span>
				<span class="text-surface-400 whitespace-nowrap">DC current SP</span> <span class="font-mono">{p.dc_output_amps_setpoint.toFixed(1)} A</span>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<!-- EVSE I/O ────────────────────────────────────────────────────────── -->
		<div class="card p-4 w-64">
			<div class="text-xs font-semibold mb-3 text-warning-500">EVSE Internal I/O</div>
			{#if latest?.chademo?.gpio}
			{@const io = latest.chademo.gpio.evse_io}
			<div class="text-xs font-medium text-success-500 mb-1 uppercase tracking-wide">Inputs</div>
			<div class="flex flex-col gap-1.5 text-xs mb-3">
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">E-Stop <span class="text-surface-500">(P8_11)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.estop === 255 ? '?' : io.estop === 0 ? 'ACTIVE' : 'OK'}</span>
						<span class={estopDot(io.estop)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">On/Off button <span class="text-surface-500">(P9_23)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.on_off_button === 255 ? '?' : io.on_off_button === 0 ? 'PRESSED' : 'open'}</span>
						<span class={pinDot(io.on_off_button, false)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">Boost button <span class="text-surface-500">(P9_25)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.boost_button === 255 ? '?' : io.boost_button === 0 ? 'PRESSED' : 'open'}</span>
						<span class={pinDot(io.boost_button, false)}></span>
					</div>
				</div>
			</div>
			<div class="text-xs font-medium text-secondary-500 mb-1 uppercase tracking-wide">Outputs</div>
			<div class="flex flex-col gap-1.5 text-xs">
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">C1 AC contactor <span class="text-surface-500">(P8_30)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.c1_contactor === 255 ? '?' : io.c1_contactor === 1 ? 'CLOSE' : 'open'}</span>
						<span class={pinDot(io.c1_contactor)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">C2 AC contactor <span class="text-surface-500">(P8_32)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.c2_contactor === 255 ? '?' : io.c2_contactor === 1 ? 'CLOSE' : 'open'}</span>
						<span class={pinDot(io.c2_contactor)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">PRE AC contactor <span class="text-surface-500">(P8_28)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.pre_ac === 255 ? '?' : io.pre_ac === 1 ? 'CLOSE' : 'open'}</span>
						<span class={pinDot(io.pre_ac)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">Master contactor <span class="text-surface-500">(P8_12)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.master_contactor === 255 ? '?' : io.master_contactor === 1 ? 'CLOSE' : 'open'}</span>
						<span class={pinDot(io.master_contactor)}></span>
					</div>
				</div>
				<div class="flex justify-between items-center gap-2">
					<span class="text-surface-400 whitespace-nowrap">PCA9552 reset <span class="text-surface-500">(P8_31)</span></span>
					<div class="flex items-center gap-1.5">
						<span class="font-mono">{io.pca_reset === 255 ? '?' : io.pca_reset === 1 ? 'normal' : 'RESET'}</span>
						<span class={pinDot(io.pca_reset)}></span>
					</div>
				</div>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<!-- Meter ──────────────────────────────────────────────────────────────── -->
		<div class="card p-4 w-64">
			<div class="text-xs font-semibold mb-3 text-tertiary-500">Grid Meter</div>
			{#if latest?.meter}
			{@const m = latest.meter}
			<div class="grid grid-cols-2 gap-x-3 gap-y-1 text-xs">
				<span class="text-surface-400">Grid</span>      <span class="font-mono">{m.total_w != null ? Math.round(m.total_w) + ' W' : '—'}</span>
				<span class="text-surface-400">Phase</span>     <span class="font-mono">{m.phase_w != null ? Math.round(m.phase_w) + ' W' : '—'}</span>
				{#if m.charger_v != null}
				<span class="text-surface-400 mt-2 col-span-2 text-xs font-semibold text-surface-300">Charger sub-meter</span>
				<span class="text-surface-400">Voltage</span>   <span class="font-mono">{m.charger_v.toFixed(1)} V</span>
				<span class="text-surface-400">Current</span>   <span class="font-mono">{m.charger_a?.toFixed(3)} A</span>
				<span class="text-surface-400">Power</span>     <span class="font-mono">{m.charger_w != null ? Math.round(m.charger_w) + ' W' : '—'}</span>
				<span class="text-surface-400">Efficiency</span><span class="font-mono">{m.efficiency != null ? m.efficiency.toFixed(1) + '%' : '—'}</span>
				{/if}
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

		<!-- Supervisory ─────────────────────────────────────────────────────── -->
		<div class="card p-4 w-64">
			<div class="text-xs font-semibold mb-3 text-success-500">Supervisory States</div>
			{#if latest?.supervisory}
			{@const s = latest.supervisory}
			<div class="text-xs">
				<div class="text-xs font-medium text-warning-500 mb-1 uppercase tracking-wide">Time / Event</div>
				<div class="grid grid-cols-[1fr_auto_auto] gap-x-3 gap-y-1.5 items-center mb-3">
					<span class="text-surface-400">Ready to Drive</span>
					<span class="text-surface-500 text-right">Req <span class={supLed(s.ready_to_drive_request, false)}></span></span>
					<span class="text-surface-500 text-right">Act <span class={supLed(false, s.ready_to_drive_active)}></span></span>

					<span class="text-surface-400">Off-Peak Charging</span>
					<span class="text-surface-500 text-right">Req <span class={supLed(s.off_peak_charging_request, false)}></span></span>
					<span class="text-surface-500 text-right">Act <span class={supLed(false, s.off_peak_charging_active)}></span></span>
				</div>
				<div class="text-xs font-medium text-tertiary-500 mb-1 uppercase tracking-wide">Smart (MQTT Requests)</div>
				<div class="grid grid-cols-[1fr_auto_auto] gap-x-3 gap-y-1.5 items-center">
					<span class="text-surface-400">Smart Export</span>
					<span class="text-surface-500 text-right">Req <span class={supLed(s.smart_export_request, false)}></span></span>
					<span class="text-surface-500 text-right">Act <span class={supLed(false, s.smart_export_active)}></span></span>

					<span class="text-surface-400">Smart Export Solar</span>
					<span class="text-surface-500 text-right">Req <span class={supLed(s.smart_export_excess_solar_request, false)}></span></span>
					<span class="text-surface-500 text-right">Act <span class={supLed(false, s.smart_export_excess_solar_active)}></span></span>

					<span class="text-surface-400">Smart Charge</span>
					<span class="text-surface-500 text-right">Req <span class={supLed(s.smart_charge_request, false)}></span></span>
					<span class="text-surface-500 text-right">Act <span class={supLed(false, s.smart_charge_active)}></span></span>

					<span class="text-surface-400">EV Drain Protection</span>
					<span class="text-surface-500 text-right">Req <span class={supLed(s.ev_drain_protection_request, false)}></span></span>
					<span class="text-surface-500 text-right">Act <span class={supLed(false, s.ev_drain_protection_active)}></span></span>
				</div>
			</div>
			{:else}
			<div class="text-xs text-surface-400">No data</div>
			{/if}
		</div>

	</div>
</div>
</div>
</div>
