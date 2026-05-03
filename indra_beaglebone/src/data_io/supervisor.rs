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

    // Ready to Drive — source: internal (scheduler compares clock vs ready_to_drive_time)
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
        evaluate_smart_charge().await;
        // evaluate_smart_export().await;       — TODO
        // evaluate_ev_drain_protection().await; — TODO
        // evaluate_off_peak_charging().await;   — TODO
        // evaluate_ready_to_drive().await;      — TODO
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
        // Operator has disabled the feature in the UI
        drop(settings);
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (disabled in settings)");
            sup.smart_charge_active = false;
        }
        return;
    }
    let soc_limit = settings.charge_soc_limit;
    drop(settings);

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let mode = *chademo.state();
    let soc = *chademo.soc() as f32;
    drop(chademo);

    if !matches!(mode, OperationMode::V2h) {
        // EV is not in V2H mode — smart charge only applies there
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (not in V2h, mode: {:?})", mode);
            sup.smart_charge_active = false;
        }
        return;
    }

    if soc >= soc_limit as f32 {
        // Battery already at or above target SoC
        let mut sup = SUPERVISORY.write().await;
        if sup.smart_charge_active {
            log::info!("Supervisor: smart_charge_active → false (SoC {:.1}% >= limit {}%)", soc, soc_limit);
            sup.smart_charge_active = false;
        }
        return;
    }

    // All conditions met — activate
    let mut sup = SUPERVISORY.write().await;
    if !sup.smart_charge_active {
        log::info!(
            "Supervisor: smart_charge_active → true (SoC {:.1}% < limit {}%)",
            soc, soc_limit
        );
        sup.smart_charge_active = true;
    }
}
