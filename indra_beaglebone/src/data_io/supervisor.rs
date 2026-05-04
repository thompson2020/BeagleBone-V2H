use chrono::{Datelike, Timelike};
use lazy_static::lazy_static;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

/// All supervisory signals in one place.
///
/// `_request`  — the raw incoming signal (from MQTT or internal calculation).
/// `_active`   — set by the supervisor task when it decides to act on a request.
///               `ev_connect.rs` reads only `_active` flags and never evaluates conditions itself.
#[derive(Clone, Copy, Default)]
pub struct SupervisoryState {
    // Smart Charge — source: MQTT from HA (Octopus IOG cheap slot)
    pub smart_charge_request: bool,
    pub smart_charge_active: bool,
    pub smart_charge_request_update: Option<Instant>,

    // EV Drain Protection — source: MQTT from HA
    pub ev_drain_protection_request: bool,
    pub ev_drain_protection_active: bool,
    pub ev_drain_protection_request_update: Option<Instant>,

    // Smart Export — source: MQTT from HA (favourable export price)
    pub smart_export_request: bool,
    pub smart_export_active: bool,
    pub smart_export_request_update: Option<Instant>,

    // Smart Export Excess Solar — source: MQTT from HA (export rate > overnight import rate)
    pub smart_export_excess_solar_request: bool,
    pub smart_export_excess_solar_active: bool,
    pub smart_export_excess_solar_request_update: Option<Instant>,

    // Ready to Drive — source: internal (clock within calculated start → ready_to_drive_end_time+2h)
    pub ready_to_drive_request: bool,
    pub ready_to_drive_active: bool,

    // Off-Peak Charging — source: internal (clock within off_peak_start..off_peak_end window)
    pub off_peak_charging_request: bool,
    pub off_peak_charging_active: bool,
}

impl SupervisoryState {
    pub fn update_smart_charge_request(&mut self, enabled: bool, stale: bool) {
        self.smart_charge_request = enabled;
        if !stale {
            self.smart_charge_request_update = Some(Instant::now());
        }
    }
    pub fn update_ev_drain_protection_request(&mut self, enabled: bool, stale: bool) {
        self.ev_drain_protection_request = enabled;
        if !stale {
            self.ev_drain_protection_request_update = Some(Instant::now());
        }
    }
    pub fn update_smart_export_request(&mut self, enabled: bool, stale: bool) {
        self.smart_export_request = enabled;
        if !stale {
            self.smart_export_request_update = Some(Instant::now());
        }
    }
    pub fn update_smart_export_excess_solar_request(&mut self, enabled: bool, stale: bool) {
        self.smart_export_excess_solar_request = enabled;
        if !stale {
            self.smart_export_excess_solar_request_update = Some(Instant::now());
        }
    }
}

lazy_static! {
    pub static ref SUPERVISORY: Arc<RwLock<SupervisoryState>> =
        Arc::new(RwLock::new(SupervisoryState::default()));
}

/// The supervisor task — runs every second and decides which `_active` flags should be set.
///
/// This is the single place that answers "should X be happening right now?"
/// It reads `_request` flags, `OPERATOR_SETTINGS`, and current charger state,
/// then writes `_active` flags. `ev_connect.rs` only ever reads `_active`.
pub async fn supervisor_task() {
    log::info!("Starting thread: supervisor  | {}", tokio::task::id());
    loop {
        sleep(Duration::from_secs(1)).await;
        evaluate_ready_to_drive().await;     // highest priority — must run first
        evaluate_off_peak_charging().await;
        evaluate_smart_export().await;
        evaluate_smart_export_excess_solar().await;
        evaluate_smart_charge().await;
        evaluate_ev_drain_protection().await;
    }
}

/// Decides whether smart charge should be active this second.
///
/// Conditions (all must be true):
///   1. `smart_charge_request`          — HA says a cheap slot is active
///   2. `OPERATOR_SETTINGS.smart_charge` — operator has enabled the feature in the UI
///   3. Current mode is V2h             — only meaningful when the EV is in V2H mode
///   4. Current SoC < charge_soc_limit  — don't charge if already at the target
async fn evaluate_smart_charge() {
    use crate::data_io::operator_settings::OPERATOR_SETTINGS;
    use crate::global_state::OperationMode;

    if SUPERVISORY.read().await.ready_to_drive_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (ready_to_drive_active)");
            sup.smart_charge_active = false;
        }
        return;
    }

    if SUPERVISORY.read().await.off_peak_charging_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (off_peak_charging_active)");
            sup.smart_charge_active = false;
        }
        return;
    }

    let request = SUPERVISORY.read().await.smart_charge_request;

    // Short-circuit: no request → clear active and return without reading other globals
    if !request {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (no request)");
            sup.smart_charge_active = false;
        }
        return;
    }

    let settings = OPERATOR_SETTINGS.read().await;
    if !settings.smart_charge {
        drop(settings);
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (disabled in settings)");
            sup.smart_charge_active = false;
        }
        return;
    }
    drop(settings);

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let mode = *chademo.state();
    drop(chademo);

    if !matches!(mode, OperationMode::V2h) {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (not in V2h, mode: {:?})", mode);
            sup.smart_charge_active = false;
        }
        return;
    }

    // Export takes priority — do not charge during an active export slot
    let sup_snap = SUPERVISORY.read().await;
    let export_blocked = sup_snap.smart_export_active || sup_snap.smart_export_excess_solar_active;
    drop(sup_snap);
    if export_blocked {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (smart_export or smart_export_excess_solar active)");
            sup.smart_charge_active = false;
        }
        return;
    }

    // All conditions met — activate and stay active for the whole slot.
    // ev_connect reads soc_limit itself and returns 0A when the limit is reached,
    // so we do not clear active here when SoC is high.
    let mut sup = SUPERVISORY.write().await;
    if !sup.smart_charge_active {
        log::info!("Supervisor: smart_charge_active → true");
        sup.smart_charge_active = true;
    }
}

/// Decides whether smart export (discharge) should be active this second.
///
/// Conditions (all must be true):
///   1. `smart_export_request`          — HA says a favourable export slot is active
///   2. `OPERATOR_SETTINGS.smart_export` — operator has enabled the feature in the UI
///   3. Current mode is V2h
///
/// SoC vs v2h_soc_min is not checked here — ev_connect holds at 0A when the floor
/// is reached, keeping the session alive for the rest of the slot.
async fn evaluate_smart_export() {
    use crate::data_io::operator_settings::OPERATOR_SETTINGS;
    use crate::global_state::OperationMode;

    if SUPERVISORY.read().await.ready_to_drive_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_active {
            log::info!("Supervisor: smart_export_active → false (ready_to_drive_active)");
            sup.smart_export_active = false;
        }
        return;
    }

    if SUPERVISORY.read().await.off_peak_charging_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_active {
            log::info!("Supervisor: smart_export_active → false (off_peak_charging_active)");
            sup.smart_export_active = false;
        }
        return;
    }

    let request = SUPERVISORY.read().await.smart_export_request;

    if !request {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_active {
            log::info!("Supervisor: smart_export_active → false (no request)");
            sup.smart_export_active = false;
        }
        return;
    }

    let settings = OPERATOR_SETTINGS.read().await;
    if !settings.smart_export {
        drop(settings);
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_active {
            log::info!("Supervisor: smart_export_active → false (disabled in settings)");
            sup.smart_export_active = false;
        }
        return;
    }
    drop(settings);

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let mode = *chademo.state();
    drop(chademo);

    if !matches!(mode, OperationMode::V2h) {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_active {
            log::info!("Supervisor: smart_export_active → false (not in V2h, mode: {:?})", mode);
            sup.smart_export_active = false;
        }
        return;
    }

    let mut sup = SUPERVISORY.write().await;
    if !sup.smart_export_active {
        log::info!("Supervisor: smart_export_active → true");
        sup.smart_export_active = true;
    }
}

/// Decides whether smart export excess solar (discharge) should be active this second.
///
/// Same pattern as evaluate_smart_export. Active when export rate > overnight import rate.
/// Priority: below smart_export, above smart_charge and ev_drain_protection.
///
/// Conditions (all must be true):
///   1. `smart_export_excess_solar_request`           — HA says excess solar export is favourable
///   2. `OPERATOR_SETTINGS.smart_export_excess_solar` — operator has enabled the feature
///   3. Current mode is V2h
///   4. `ready_to_drive_active` and `off_peak_charging_active` are both false
async fn evaluate_smart_export_excess_solar() {
    use crate::data_io::operator_settings::OPERATOR_SETTINGS;
    use crate::global_state::OperationMode;

    if SUPERVISORY.read().await.ready_to_drive_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_excess_solar_active {
            log::info!("Supervisor: smart_export_excess_solar_active → false (ready_to_drive_active)");
            sup.smart_export_excess_solar_active = false;
        }
        return;
    }

    if SUPERVISORY.read().await.off_peak_charging_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_excess_solar_active {
            log::info!("Supervisor: smart_export_excess_solar_active → false (off_peak_charging_active)");
            sup.smart_export_excess_solar_active = false;
        }
        return;
    }

    let request = SUPERVISORY.read().await.smart_export_excess_solar_request;

    if !request {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_excess_solar_active {
            log::info!("Supervisor: smart_export_excess_solar_active → false (no request)");
            sup.smart_export_excess_solar_active = false;
        }
        return;
    }

    let settings = OPERATOR_SETTINGS.read().await;
    if !settings.smart_export_excess_solar {
        drop(settings);
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_excess_solar_active {
            log::info!("Supervisor: smart_export_excess_solar_active → false (disabled in settings)");
            sup.smart_export_excess_solar_active = false;
        }
        return;
    }
    drop(settings);

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let mode = *chademo.state();
    drop(chademo);

    if !matches!(mode, OperationMode::V2h) {
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_export_excess_solar_active {
            log::info!("Supervisor: smart_export_excess_solar_active → false (not in V2h, mode: {:?})", mode);
            sup.smart_export_excess_solar_active = false;
        }
        return;
    }

    let mut sup = SUPERVISORY.write().await;
    if !sup.smart_export_excess_solar_active {
        log::info!("Supervisor: smart_export_excess_solar_active → true");
        sup.smart_export_excess_solar_active = true;
    }
}

/// Decides whether EV drain protection should be active this second.
///
/// Holds the CHAdeMO current setpoint at 0A — neither charging nor discharging.
/// Only activates when smart export and smart charge are both inactive; those
/// take priority and this function runs after both so their flags are current.
///
/// Conditions (all must be true):
///   1. `ev_drain_protection_request`          — HA says drain protection is requested
///   2. `OPERATOR_SETTINGS.ev_drain_protection` — operator has enabled the feature
///   3. Current mode is V2h
///   4. `smart_export_active` is false
///   5. `smart_charge_active` is false
async fn evaluate_ev_drain_protection() {
    use crate::data_io::operator_settings::OPERATOR_SETTINGS;
    use crate::global_state::OperationMode;

    if SUPERVISORY.read().await.ready_to_drive_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.ev_drain_protection_active {
            log::info!("Supervisor: ev_drain_protection_active → false (ready_to_drive_active)");
            sup.ev_drain_protection_active = false;
        }
        return;
    }

    let request = SUPERVISORY.read().await.ev_drain_protection_request;

    if !request {
        let mut sup = SUPERVISORY.write().await;
        if sup.ev_drain_protection_active {
            log::info!("Supervisor: ev_drain_protection_active → false (no request)");
            sup.ev_drain_protection_active = false;
        }
        return;
    }

    let settings = OPERATOR_SETTINGS.read().await;
    if !settings.ev_drain_protection {
        drop(settings);
        let mut sup = SUPERVISORY.write().await;
        if sup.ev_drain_protection_active {
            log::info!("Supervisor: ev_drain_protection_active → false (disabled in settings)");
            sup.ev_drain_protection_active = false;
        }
        return;
    }
    drop(settings);

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let mode = *chademo.state();
    drop(chademo);

    if !matches!(mode, OperationMode::V2h) {
        let mut sup = SUPERVISORY.write().await;
        if sup.ev_drain_protection_active {
            log::info!("Supervisor: ev_drain_protection_active → false (not in V2h, mode: {:?})", mode);
            sup.ev_drain_protection_active = false;
        }
        return;
    }

    // Yield to higher-priority signals — export and charge are already evaluated this cycle
    let sup_snap = SUPERVISORY.read().await;
    let blocked = sup_snap.smart_export_active
        || sup_snap.smart_export_excess_solar_active
        || sup_snap.smart_charge_active;
    drop(sup_snap);

    if blocked {
        let mut sup = SUPERVISORY.write().await;
        if sup.ev_drain_protection_active {
            log::info!("Supervisor: ev_drain_protection_active → false (smart export/charge active)");
            sup.ev_drain_protection_active = false;
        }
        return;
    }

    let mut sup = SUPERVISORY.write().await;
    if !sup.ev_drain_protection_active {
        log::info!("Supervisor: ev_drain_protection_active → true");
        sup.ev_drain_protection_active = true;
    }
}

/// Decides whether off-peak charging should be active this second.
///
/// The request is generated internally from the clock — no external MQTT signal.
/// Parses `off_peak_start` / `off_peak_end` ("HH:MM") from operator settings and
/// handles windows that cross midnight (e.g. 23:30 → 05:30).
///
/// Priority: below ready_to_drive, above smart_export and smart_charge.
///
/// Conditions (all must be true):
///   1. `ready_to_drive_active` is false
///   2. Current time is within the off-peak window
///   3. `OPERATOR_SETTINGS.off_peak_charging` is enabled
///   4. Current mode is V2h
///
/// SoC vs charge_soc_limit is not checked here — ev_connect holds at 0A when the
/// limit is reached, keeping the session alive for the rest of the window.
async fn evaluate_off_peak_charging() {
    use crate::data_io::operator_settings::OPERATOR_SETTINGS;
    use crate::global_state::OperationMode;

    if SUPERVISORY.read().await.ready_to_drive_active {
        let mut sup = SUPERVISORY.write().await;
        if sup.off_peak_charging_active {
            log::info!("Supervisor: off_peak_charging_active → false (ready_to_drive_active)");
            sup.off_peak_charging_active = false;
        }
        return;
    }

    let settings = OPERATOR_SETTINGS.read().await;
    let enabled = settings.off_peak_charging;
    let start_str = settings.off_peak_start.clone();
    let end_str = settings.off_peak_end.clone();
    drop(settings);

    let parse_hhmm = |s: &str| -> Option<u32> {
        let mut it = s.splitn(2, ':');
        let h: u32 = it.next()?.parse().ok()?;
        let m: u32 = it.next()?.parse().ok()?;
        Some(h * 60 + m)
    };

    let (start_min, end_min) = match (parse_hhmm(&start_str), parse_hhmm(&end_str)) {
        (Some(s), Some(e)) => (s, e),
        _ => {
            log::warn!("Supervisor: off_peak_charging — invalid time format ({} / {})", start_str, end_str);
            let mut sup = SUPERVISORY.write().await;
            sup.off_peak_charging_request = false;
            sup.off_peak_charging_active = false;
            return;
        }
    };

    let now = chrono::Local::now();
    let current_min = now.hour() * 60 + now.minute();
    let in_window = if start_min > end_min {
        // Window crosses midnight (e.g. 23:30 → 05:30)
        current_min >= start_min || current_min < end_min
    } else {
        current_min >= start_min && current_min < end_min
    };

    // Update request flag from clock
    {
        let mut sup = SUPERVISORY.write().await;
        if sup.off_peak_charging_request != in_window {
            log::info!("Supervisor: off_peak_charging_request → {}", in_window);
            sup.off_peak_charging_request = in_window;
        }
    }

    if !in_window || !enabled {
        let mut sup = SUPERVISORY.write().await;
        if sup.off_peak_charging_active {
            log::info!("Supervisor: off_peak_charging_active → false ({})",
                if !in_window { "outside window" } else { "disabled in settings" });
            sup.off_peak_charging_active = false;
        }
        return;
    }

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let mode = *chademo.state();
    drop(chademo);

    if !matches!(mode, OperationMode::V2h) {
        let mut sup = SUPERVISORY.write().await;
        if sup.off_peak_charging_active {
            log::info!("Supervisor: off_peak_charging_active → false (not in V2h, mode: {:?})", mode);
            sup.off_peak_charging_active = false;
        }
        return;
    }

    let mut sup = SUPERVISORY.write().await;
    if !sup.off_peak_charging_active {
        log::info!("Supervisor: off_peak_charging_active → true");
        sup.off_peak_charging_active = true;
    }
}

/// Decides whether ready-to-drive charging should be active this second.
///
/// Calculates the start time backwards from the user's "ready by" end time:
///   start = end - ceil(kWh_needed / charge_rate_kw)
///
/// The active window runs from start to end+2h. During the 2h extension after
/// the end time, SoC is already at target so ev_connect holds at 0A — this
/// prevents the system reverting to normal V2H while the car is in use.
///
/// When active, all other supervisory modes are suppressed (they check this flag
/// and return early). This is the highest-priority mode.
async fn evaluate_ready_to_drive() {
    use crate::data_io::operator_settings::OPERATOR_SETTINGS;
    use crate::global_state::OperationMode;

    let settings = OPERATOR_SETTINGS.read().await;
    let enabled       = settings.ready_to_drive;
    let end_str       = settings.ready_to_drive_end_time.clone();
    let days          = settings.ready_to_drive_days;
    let target_soc    = settings.ready_to_drive_soc;
    let v2h_max_amps  = settings.v2h_max_amps;
    drop(settings);

    let parse_hhmm = |s: &str| -> Option<u32> {
        let mut it = s.splitn(2, ':');
        let h: u32 = it.next()?.parse().ok()?;
        let m: u32 = it.next()?.parse().ok()?;
        Some(h * 60 + m)
    };

    let end_min = match parse_hhmm(&end_str) {
        Some(e) => e,
        None => {
            log::warn!("Supervisor: ready_to_drive — invalid time format ({})", end_str);
            let mut sup = SUPERVISORY.write().await;
            sup.ready_to_drive_request = false;
            sup.ready_to_drive_active = false;
            drop(sup);
            OPERATOR_SETTINGS.write().await.ready_to_drive_start_time = "--:--".to_string();
            return;
        }
    };

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let current_soc = *chademo.soc() as f32;
    let mode = *chademo.state();
    drop(chademo);

    let charge_rate_kw = (v2h_max_amps as f32 * 400.0) / 1000.0;

    let kwh_needed = ((target_soc as f32 - current_soc).max(0.0) / 100.0) * 62.0;
    let hours_needed = if charge_rate_kw > 0.1 { kwh_needed / charge_rate_kw } else { 0.0 };

    // Start time in whole minutes, clamped to [0, 1440)
    let start_min_raw = end_min as i32 - (hours_needed * 60.0).ceil() as i32;
    let start_min = ((start_min_raw % 1440) + 1440) as u32 % 1440;

    let start_time_str = format!("{:02}:{:02}", start_min / 60, start_min % 60);
    OPERATOR_SETTINGS.write().await.ready_to_drive_start_time = start_time_str.clone();

    let now = chrono::Local::now();
    let current_min = now.hour() * 60 + now.minute();
    // num_days_from_monday: 0=Mon … 6=Sun, matching ready_to_drive_days [M,T,W,T,F,S,S]
    let weekday = now.weekday().num_days_from_monday() as usize;
    let today_selected    = days.get(weekday).copied().unwrap_or(false);
    let tomorrow_selected = days.get((weekday + 1) % 7).copied().unwrap_or(false);

    // Active window: start_min → end_min + 120 (with midnight crossover).
    // When the window crosses midnight the start portion is on the previous day,
    // so we check tomorrow's selection then, and today's selection in the morning portion.
    let end_plus_2h = (end_min + 120) % 1440;
    let in_window = if start_min <= end_plus_2h {
        // No midnight crossing — entire window on one day, check today
        today_selected && current_min >= start_min && current_min < end_plus_2h
    } else {
        // Window crosses midnight:
        //   evening portion (current >= start): the ready day is tomorrow
        //   morning portion (current < end+2h): the ready day is today
        (current_min >= start_min && tomorrow_selected)
            || (current_min < end_plus_2h && today_selected)
    };

    let request = in_window;
    {
        let mut sup = SUPERVISORY.write().await;
        if sup.ready_to_drive_request != request {
            log::info!("Supervisor: ready_to_drive_request → {} (start: {}, end: {}+2h, today: {}, tomorrow: {})",
                request, start_time_str, end_str, today_selected, tomorrow_selected);
            sup.ready_to_drive_request = request;
        }
    }

    if !request || !enabled {
        let mut sup = SUPERVISORY.write().await;
        if sup.ready_to_drive_active {
            log::info!("Supervisor: ready_to_drive_active → false ({})",
                if !request { "outside window" } else { "disabled in settings" });
            sup.ready_to_drive_active = false;
        }
        return;
    }

    if !matches!(mode, OperationMode::V2h) {
        let mut sup = SUPERVISORY.write().await;
        if sup.ready_to_drive_active {
            log::info!("Supervisor: ready_to_drive_active → false (not in V2h, mode: {:?})", mode);
            sup.ready_to_drive_active = false;
        }
        return;
    }

    let mut sup = SUPERVISORY.write().await;
    if !sup.ready_to_drive_active {
        log::info!("Supervisor: ready_to_drive_active → true (start: {}, end: {}+2h)",
            start_time_str, end_str);
        sup.ready_to_drive_active = true;
    }
}
