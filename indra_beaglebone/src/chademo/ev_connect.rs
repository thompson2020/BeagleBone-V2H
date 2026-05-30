use crate::{
    async_timeout_loop, async_timeout_result,
    chademo::{
        can::*,
        state::{Chademo, *}, //ChargerState
    },
    data_io::panel::LedCommand,
    error::IndraError,
    global_state::OperationMode,
    log_error,
    meter::METER,
    pre_charger::{
        fans::Fan,
        pre_thread::{self},
        pwm::Pwm,
        PreCharger, PreCommand, BB_PWM_CHIP, BB_PWM_NUMBER, PREDATA,
    },
    statics::{self, *},
    timeout_condition, MAX_AMPS, MAX_SOC, METER_BIAS, MIN_SOC,
};
use chademo_v2::{X109Status, X108};
//use log::warn;
use std::{sync::Arc, time::Duration};
use sysfs_gpio::Pin;
use tokio::{
    sync::Mutex,
    time::{sleep, timeout, Instant},
};
use tokio_socketcan::{CANFrame, CANSocket};

const DUMMYMODE: bool = false;
const KP: f32 = 0.45;
const EFF: f32 = 0.9;

pub async fn ev100ms(led_tx: LedTx, mode_rx: ChademoRx) -> Result<(), IndraError> {
    log::info!("Starting thread: EV ");

    // let operational_mode = OPERATIONAL_MODE.clone();
    let mut chademo = Chademo::new();
    let mut last_logo = crate::data_io::panel::State::Off; // sentinel -- forces first send
    let t100ms = Duration::from_millis(100);
    let predata = PREDATA.clone();
    let (pre_tx, pre_rx) = statics::pre_channel();
    let pre_rx = mutex(pre_rx);
    let mode_rx = mutex(mode_rx);
    use tokio::task::JoinHandle;
    let mut handles: Vec<JoinHandle<Result<(), IndraError>>> = Vec::new(); // Store spawned task handles
    loop {
        for handle in handles.drain(..) {
            log::info!("Aborting Pre thread  | {}", handle.id());
            handle.abort(); // Abort the previous tasks
        }
        // Reset electrical state immediately so the UI snapshot shows clean zeros while idle.
        // SoC is deliberately kept -- it's the last known EV battery level.
        *PREDATA.lock().await = PreCharger::default();
        chademo.update_amps(0i16);
        chademo.x102.charging_current_request = 0;
        reset_gpio_state(&mut chademo);
        chademo.set_state(OperationMode::Idle);
        update_panel_leds(&led_tx, &chademo, &mut last_logo).await;
        update_chademo_mutex(&chademo).await;
        {
            // fan fudge
            let pwm = Pwm::new(BB_PWM_CHIP, BB_PWM_NUMBER, 1000).unwrap(); // number depends on chip, etc.
            Fan::new(pwm).update(10.0);
            // fan.update(10.0);
        }
        let mut can = tokio_socketcan::CANSocket::open(&"can1").map_err(|_| IndraError::Error)?;
        {
            if let Some(state) = mode_rx.clone().lock().await.recv().await {
                chademo.set_state(state);
                log::info!("EV received new mode: {:?}", state);
                update_panel_leds(&led_tx, &chademo, &mut last_logo).await;
                update_chademo_mutex(&chademo).await;
                if matches!(state, OperationMode::Quit) {
                    return Ok(());
                }
                if !(state.is_v2h() || state.is_charge()) {
                    continue;
                }
            }
        }

        if DUMMYMODE {
            log::info!("            Entering charge loop!");
            let _ = match charge_mode(&mut chademo, &mut can, &pre_tx, &led_tx, mode_rx.clone(), &mut last_logo)
                .await
            {
                Ok(reason) => reason,
                Err(e) => {
                    log::error!("Bailed out of main charge (dummy) | {e:?}");
                    log_error!("LED error", led_tx.send(LedCommand::Logo(crate::data_io::panel::State::Error)).await);
                    last_logo = crate::data_io::panel::State::Error;
                    OperationMode::Idle
                }
            };
            continue;
        }
        log::info!("Operational Mode: | {:?}", chademo.state());
        // Spawn new task
        let handle = tokio::spawn(pre_thread::init(pre_rx.clone()));
        log::info!("Spawned new Pre thread  | {}", handle.id());
        handles.push(handle);
        chademo.charge_stop();
        chademo.pins().pre_ac.set_value(1).unwrap();
        if let Err(e) = init_pre(&predata, t100ms, &pre_tx).await {
            log::error!("Pre init failed - | {e:?}");
            log_error!("LED error", led_tx.send(LedCommand::Logo(crate::data_io::panel::State::Error)).await);
            last_logo = crate::data_io::panel::State::Error;
            chademo.set_state(OperationMode::Idle);
            reset_gpio_state(&mut chademo);
            update_chademo_mutex(&chademo).await;
            continue;
        };
        chademo.x109.status = X109Status::from(0x20);
        assert!(!chademo.x109.status.status_station);
        assert!(!chademo.x109.status.status_vehicle_connector_lock);
        log::debug!("Raise D1");
        log_error!("Setting D1 high", chademo.pins().d1.set_value(1));

        log::info!("Check can frames & Wait for K line");
        if let Err(e) = k_line(&mut can, &mut chademo).await {
            log::error!("K line init failed - is car connected? | {e:?}");
            log_error!("LED error", led_tx.send(LedCommand::Logo(crate::data_io::panel::State::Error)).await);
            last_logo = crate::data_io::panel::State::Error;
            chademo.set_state(OperationMode::Idle);
            reset_gpio_state(&mut chademo);
            update_chademo_mutex(&chademo).await;
            continue;
        };

        chademo.plug_lock(true).expect("Plug lock failed");
        assert!(!chademo.x109.status.status_station);
        assert!(chademo.x109.status.status_vehicle_connector_lock);
        // update_chademo_mutex(&chademo).await;
        // chademo.precharge();
        // TODO: Insulation tests skipped -- requires external SPI ADC for contactor differential
        // voltage measurement and welding detection. BeagleBone internal ADC is disabled
        // (disable_uboot_overlay_adc=1 in uEnv.txt). An external SPI ADC was planned --
        // spidev = "0.5.2" is commented out in Cargo.toml. Chip model unknown (no schematic
        // available); physically trace PCB to identify IC and SPI pins before implementing.
        // Once chip is identified: uncomment spidev in Cargo.toml, write a driver wrapper,
        // implement isolation resistance check here before raising D2, and add welding
        // detection in shutdown() after contactors are commanded open.
        log::info!("insulation tests skipped !!!");
        chademo.pins().d2.set_value(1).unwrap();

        // update_chademo_mutex(&chademo).await;
        log::info!("when voltage match - raise D2");
        chademo.charge_start();
        if let Err(e) = precharge(&mut can, &mut chademo, &pre_tx, &predata).await {
            log::error!("precharge & contactor init failed - should be catastropic and hang | {e:?}");
            log_error!("LED error", led_tx.send(LedCommand::Logo(crate::data_io::panel::State::Error)).await);
            last_logo = crate::data_io::panel::State::Error;
            chademo.set_state(OperationMode::Idle);
            reset_gpio_state(&mut chademo);
            update_chademo_mutex(&chademo).await;
            continue;
        }
        log::info!("precharge left");
        chademo.x109.status = X109Status::from(0x05);
        assert!(chademo.x109.status.status_vehicle_connector_lock);
        assert!(chademo.x109.status.status_station);
        // Reflect the 1A PRE init setpoint in x109/x208 so the EV sees a valid current
        // request via CAN and sets status_vehicle_charging = true to start the session.
        chademo.update_amps(1i16);
        update_panel_leds(&led_tx, &chademo, &mut last_logo).await;
        update_chademo_mutex(&chademo).await;

        log::info!("            Entering charge loop!");
        let exit_reason =
            match charge_mode(&mut chademo, &mut can, &pre_tx, &led_tx, mode_rx.clone(), &mut last_logo).await {
                Ok(reason) => reason,
                Err(e) => {
                    log::error!("Bailed out of main charge | {e:?}");
                    log_error!("LED error", led_tx.send(LedCommand::Logo(crate::data_io::panel::State::Error)).await);
                    last_logo = crate::data_io::panel::State::Error;
                    OperationMode::Idle
                }
            };

        // end charge ========================================================
        //
        // TODO: Review CHAdeMO shutdown procedure -- OBD2 fault codes may be thrown by EV.
        // Suspected causes:
        //   1. PRE is shut down before the CAN handshake completes -- EV may see DC voltage
        //      drop before the protocol reaches its terminal state, triggering a fault.
        //   2. x109 status 0x25 (set after contactors open) has status_station (bit 0) set
        //      alongside the stop flag (bit 5) -- may be contradictory per CHAdeMO spec.
        //   3. x109.output_voltage is never updated during shutdown -- EV receives a stale
        //      voltage value and cross-checks it against its own measurement.
        //   4. charging_stop_control_release() is commented out below -- 0x24 (bit 5 set)
        //      may already cover this, but worth confirming against spec.
        // To diagnose: capture the OBD2 DTC code from the car after a session and match
        // against CHAdeMO fault table. ERROR log below will flag x102 fault bits if set.

        log::warn!("End of init fn 'end charge' with exit reason | {exit_reason:?}");
        update_chademo_mutex(&chademo).await;
        chademo.x109.status = X109Status::from(0x24);
        // chademo.charging_stop_control_release();
        log_error!("Shutdown pre", pre_tx.send(PreCommand::Shutdown).await);
        shutdown(&mut chademo, &mut can).await;
        log::warn!("Charge/discharge mode ended");
        update_chademo_mutex(&chademo).await;
        if matches!(exit_reason, OperationMode::Quit) {
            return Ok(());
        }
        drop(can);
        //loops back to idle
    }
}

async fn shutdown(chademo: &mut Chademo, can: &mut CANSocket) {
    let mut contactors = true;
    loop {
        log::debug!(" | {}", chademo.x102.status);
        match timeout(Duration::from_millis(200), recv_send(can, chademo, false)).await {
            Ok(Ok(_)) => (),
            Ok(Err(e)) => {
                match e {
                    crate::error::IndraError::Error =>
                        log::error!("EV fault active on closure | faults=[{}]", chademo.x102.faults()),
                    _ =>
                        log::error!("CAN error on closure | {:?}", e),
                }
                if !contactors {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                log::warn!("CAN timed out on closure | {:?}", e);
                if !contactors {
                    break;
                }
            }
        };

        if matches!(chademo.pins().k.get_value(), Ok(0)) {
            log::info!("Awaiting K line release");
            continue; // back to top of loop -- wait for K line to drop before opening contactors and cutting power
            
        };
        log::info!("K line released");
        if contactors {
            log::info!("Contactors opening");
            if chademo.pins().c1.set_value(0).is_ok() {
                //print!("\x07"); // Bell Sound
                if chademo.pins().c2.set_value(0).is_ok() {
                    //print!("\x07"); // Bell Sound
                    log::warn!("                                       !!!!CONTACTORS OPEN!!!!");

                    contactors = false;
                    chademo.x109.status = X109Status::from(0x25);
                    let x102status: u8 = chademo.x102.status.into();
                    if chademo.x102.fault() {
                        log::error!("EV reported fault on shutdown | status=0x{:02x} fault_byte=0x{:02x} faults=[{}] -- check car for DTC codes", x102status, chademo.x102.fault_byte(), chademo.x102.faults());
                    }
                    continue;
                }
            }
        };
        if !chademo.x109.status.status_vehicle_connector_lock {
            break;
        }
        {
            if PREDATA.clone().lock().await.get_dc_output_volts() < 10.0 {
                chademo.x109.status = X109Status::from(0x20); // make this an enum
                                                              // if !chademo.x102.status.status_vehicle {
            } //     break;
              // }
            log_error!("Pluglock disable", chademo.plug_lock(false));
        }
    }
}
fn reset_gpio_state(chademo: &mut Chademo) {
    log_error!("Exit charge: c2", chademo.pins().c2.set_value(0));
    log_error!("Exit charge: c1", chademo.pins().c1.set_value(0));
    log_error!("Exit charge: d2", chademo.pins().d2.set_value(0));
    log_error!("Exit charge: d1", chademo.pins().d1.set_value(0));
    log_error!("Exit charge: Pre AC", chademo.pins().pre_ac.set_value(0));
    log_error!(
        "Exit charge: pluglock",
        chademo.pins().pluglock.set_value(0)
    );
    chademo.x109.status = X109Status::from(0x24);
}

async fn charge_mode(
    chademo: &mut Chademo,
    can: &mut CANSocket,
    pre_tx: &tokio::sync::mpsc::Sender<PreCommand>,
    led_tx: &tokio::sync::mpsc::Sender<LedCommand>,
    mode_rx: Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<OperationMode>>>,
    last_logo: &mut crate::data_io::panel::State,
) -> Result<OperationMode, IndraError> {
    let mut mode_rx = mode_rx.lock().await;
    let mut last_soc = *chademo.soc();
    let mut last_volts = 0.0;
    let mut last_amps = PREDATA.lock().await.get_dc_setpoint_amps();
    let mut last_meter = 0.01;
    let mut counter = 0;
    let mut last_faults = chademo_v2::X102Faults::default();
    let mut pre_diverge_since:  Option<Instant> = None;
    let mut pre_diverge_logged: bool            = false;

    // PID state -- shared across all meter-based modes.
    let mut dv: f32 = 0.0;
    let mut inner_tick: u32 = 0;

    use crate::global_state::OperationMode::*;

    let exit_reason = loop {
        if DUMMYMODE {
            sleep(Duration::from_millis(100)).await
        } else {
            recv_send(can, chademo, false).await?;
            let current_faults = chademo.x102.faults();
            if current_faults != last_faults {
                let x102status: u8 = chademo.x102.status.into();
                log::warn!(
                    "x102 faults changed | status=0x{:02x} faults_byte=0x{:02x} faults=[{}] SoC={}% charging={}",
                    x102status, chademo.x102.fault_byte(), current_faults, chademo.soc(),
                    chademo.x102.status.status_vehicle_charging
                );
                last_faults = current_faults;
            }
            if !chademo.status_vehicle_charging() {
                let x102status: u8 = chademo.x102.status.into();
                let state = *chademo.state();
                log::warn!(
                    "EV cleared charge permission (5.0) | mode={:?} SoC={}% status=0x{:02x} faults=[{}] last_amps={:.1}A",
                    state, chademo.soc(), x102status, chademo.x102.faults(), last_amps
                );
                break if state.is_quit() { state } else { Idle };
            }
        };

        if counter > 10 || counter == 0 {
            let x102status: u8 = chademo.x102.status.into();
            let x109status: u8 = chademo.x109.status.into();

            log::debug!(
                "102s:{:02x}, 109s:{:02x}, Soc:{}% Req:{}A",
                x102status,
                x109status,
                chademo.soc(),
                chademo.x102.charging_current_request
            );
            counter = 0;
            update_chademo_mutex(&*chademo).await; // keep UI current even when amps/SoC unchanged
        }
        counter += 1;
        inner_tick += 1;
        {
            // listen for incomming mode changes
            if let Ok(op) = mode_rx.try_recv() {
                update_panel_leds(&led_tx, &chademo, last_logo).await;
                log::info!("New CHAdeMO mode received | {op:?}");

                chademo.set_state(op);
                update_chademo_mutex(chademo).await;
            }
            // update_panel_leds(&led_tx, &chademo).await
        }

        let op = chademo.state();

        let charging_current_request = match *op {
            V2h => v2h_requested_amps(chademo, &mut dv, last_amps, &mut last_meter, &mut inner_tick).await,
            Discharge => {
                let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
                let max_amps = settings.v2h_max_amps.min(crate::MAX_AMPS);
                let soc_min = settings.v2h_soc_min.max(crate::MIN_SOC);
                drop(settings);
                if *chademo.soc() <= soc_min {
                    log::info!("Discharge SoC floor hit ({}%), stopping", soc_min);
                    chademo.request_stop_charge();
                    continue;
                }
                -(max_amps as f32).min(chademo.requested_discharging_amps())
            }
            Charge => {
                let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
                let amps = settings.charge_amps.min(crate::MAX_AMPS);
                let soc_limit = settings.charge_soc_limit.min(crate::MAX_SOC);
                let eco = settings.charge_eco;
                drop(settings);
                if soc_limit <= *chademo.soc() {
                    log::info!("Charge SoC limit hit ({}%), stopping", soc_limit);
                    chademo.request_stop_charge();
                    continue;
                }
                if eco {
                    let lower = 0.0_f32;
                    let upper = amps as f32;
                    dv = dv.clamp(lower, upper);
                    pid_step(&mut dv, last_amps, &mut last_meter, &mut inner_tick, lower, upper, 0.0, "eco").await
                } else {
                    (amps as f32).min(chademo.requested_charging_amps())
                }
            }
            Quit | Idle => {
                chademo.request_stop_charge();
                continue;
            }
            _ => continue,
        };

        {
            let pre = PREDATA.lock().await;
            chademo.x109.output_voltage = pre.get_dc_output_volts();
            let pre_sp      = pre.get_dc_setpoint_amps();
            let pre_act     = pre.get_dc_output_amps();
            let pre_enabled = pre.enabled();
            drop(pre);
            if (pre_sp - pre_act).abs() > 0.2 {
                let since = pre_diverge_since.get_or_insert_with(Instant::now);
                if !pre_diverge_logged && since.elapsed().as_secs() >= 10 {
                    log::info!("PRE amps divergence >10s | SP={:.1}A actual={:.1}A diff={:.1}A enabled={}",
                        pre_sp, pre_act, (pre_sp - pre_act).abs(), pre_enabled);
                    pre_diverge_logged = true;
                }
            } else {
                if pre_diverge_logged {
                    log::info!("PRE amps divergence resolved | SP={:.1}A actual={:.1}A",
                        pre_sp, pre_act);
                }
                pre_diverge_since  = None;
                pre_diverge_logged = false;
            }
        }

        if &last_volts != chademo.target_voltage() {
            last_volts = *chademo.target_voltage();
            log_error!(
                "",
                pre_tx.send(PreCommand::DcVoltsSetpoint(last_volts)).await
            );
        }

        // Slew-rate limit: cap the per-tick change to protect the PRE on direction reversals.
        // 2 A/100 ms = 20 A/s  ->  full -16 A->+16 A reversal takes ~1.6 s.
        const SLEW: f32 = 2.0;
        let charging_current_request =
            last_amps + (charging_current_request - last_amps).clamp(-SLEW, SLEW);

        if last_amps != charging_current_request {
            last_amps = charging_current_request;
            // Keep x109.output_current / x208.discharge_current in sync with the PRE
            // setpoint so the EV always sees the correct current request via CAN.
            chademo.update_amps(charging_current_request as i16);
            log_error!(
                "",
                pre_tx
                    .send(PreCommand::DcAmpsSetpoint(charging_current_request))
                    .await
            );

            update_chademo_mutex(&*chademo).await;
            update_panel_leds(&led_tx, &chademo, last_logo).await
        }
        if &last_soc != chademo.soc() {
            last_soc = *chademo.soc();
            update_chademo_mutex(&*chademo).await;
            update_panel_leds(&led_tx, &chademo, last_logo).await
        }
    };
    Ok(exit_reason)
}

// Outer+inner PI loop shared by all meter-based modes.
//
// Outer loop fires once per new meter reading -- converts the watt imbalance into a DC amp
// target (DV). meter_offset shifts the balance point (used by smart-export to settle at a
// non-zero export level). Efficiency correction: meter > 0 divides by EFF, meter < 0 multiplies.
//
// Inner loop fires every 1 s (10 × 100 ms ticks): SP = last_SP + KP × (DV − PV).
// Accumulates toward DV using actual measured DC amps, giving zero steady-state error.
// Returns the new setpoint, or last_amps unchanged if the inner loop has not fired this tick.
async fn pid_step(
    dv: &mut f32,
    last_amps: f32,
    last_meter: &mut f32,
    inner_tick: &mut u32,
    lower: f32,
    upper: f32,
    meter_offset: f32,
    label: &str,
) -> f32 {
    let pre = PREDATA.lock().await;
    let pv = pre.get_dc_output_amps();
    let dc_v = pre.get_dc_output_volts().max(10.0);
    drop(pre);

    if let Some(meter_raw) = METER.read().await.total_w {
        let meter = meter_raw + METER_BIAS + meter_offset;
        if meter != *last_meter {
            *last_meter = meter;
            let raw_error = meter / dc_v;
            let adj_error = if raw_error >= 0.0 { raw_error / EFF } else { raw_error * EFF };
            *dv = (pv - adj_error).clamp(lower, upper);
            log::info!("Outer loop ({label}) | meter={:.1}W offset={:.1}W PV={:.3}A adj_err={:.3}A DV={:.3}A",
                meter_raw, meter_offset, pv, adj_error, *dv);
        }
    }

    if *inner_tick >= 10 {
        *inner_tick = 0;
        let error = *dv - pv;
        let proportional = KP * error;
        let sp = (last_amps + proportional).clamp(lower, upper);
        log::debug!("Inner loop ({label}) | PV={:.3}A DV={:.3}A err={:.3}A prop={:.3}A SP={:.3}A",
            pv, *dv, error, proportional, sp);
        sp
    } else {
        last_amps
    }
}

// Computes the requested amps for V2h mode, dispatching across supervisor sub-modes.
// Non-PID branches reset dv and inner_tick so the PID re-evaluates cleanly on re-entry.
async fn v2h_requested_amps(
    chademo: &Chademo,
    dv: &mut f32,
    last_amps: f32,
    last_meter: &mut f32,
    inner_tick: &mut u32,
) -> f32 {
    let sup = crate::data_io::supervisor::SUPERVISORY.read().await;
    let ready_to_drive_active = sup.ready_to_drive_active;
    let smart_export_active = sup.smart_export_active;
    let smart_export_excess_solar_active = sup.smart_export_excess_solar_active;
    let smart_charge_active = sup.smart_charge_active;
    let off_peak_charging_active = sup.off_peak_charging_active;
    let ev_drain_protection_active = sup.ev_drain_protection_active;
    drop(sup);

    // Priority: ready-to-drive > off-peak > smart-export > smart-charge > drain-protection > V2h default
    if ready_to_drive_active {
        *dv = 0.0;
        *inner_tick = 9;
        let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
        let amps = settings.v2h_max_amps.min(crate::MAX_AMPS);
        let soc_target = settings.ready_to_drive_soc.min(crate::MAX_SOC);
        drop(settings);
        if soc_target <= *chademo.soc() {
            if last_amps > 0.5 {
                log::info!("Ready-to-drive SoC target reached ({}% >= {}%), holding at 0A", chademo.soc(), soc_target);
            }
            0.0
        } else { (amps as f32).min(chademo.requested_charging_amps()) }
    } else if off_peak_charging_active {
        *dv = 0.0;
        *inner_tick = 9;
        let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
        let amps = settings.v2h_max_amps.min(crate::MAX_AMPS);
        let soc_limit = settings.v2h_soc_max_boost.min(crate::MAX_SOC);
        drop(settings);
        let soc = *chademo.soc();
        // 1% deadband: stop at soc_limit, only restart below soc_limit-1.
        // last_amps == 0.0 means we were already stopped -- stay stopped until below deadband.
        if soc >= soc_limit || (last_amps == 0.0 && soc >= soc_limit.saturating_sub(1)) {
            if last_amps > 0.5 {
                log::info!("Off-peak SoC limit reached ({}% >= {}%), stopping charge", soc, soc_limit);
            }
            0.0
        } else { (amps as f32).min(chademo.requested_charging_amps()) }
    } else if smart_export_active {
        let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
        let soc_min = settings.smart_export_soc_min.max(crate::MIN_SOC);
        let soc_max = settings.v2h_soc_max.min(crate::MAX_SOC);
        let max_amps = settings.v2h_max_amps.min(crate::MAX_AMPS);
        let export_limit_w = (settings.smart_export_limit_w as f32).max(0.0);
        drop(settings);
        let soc = *chademo.soc();
        if soc <= soc_min {
            if last_amps < -0.5 {
                log::info!("Smart export SoC floor hit ({}% <= {}%), pausing discharge", soc, soc_min);
            }
            *dv = 0.0;
            *inner_tick = 9;
            0.0
        } else {
            let lower = -(max_amps as f32);
            // Allow charging (positive amps) to absorb excess solar -- only prevent charging
            // once the car reaches its SoC ceiling.
            let upper = if soc >= soc_max { 0.0 } else { max_amps as f32 };
            if soc >= soc_max && last_amps > 0.5 {
                log::info!("Smart export SoC ceiling hit ({}% >= {}%), holding discharge-only", soc, soc_max);
            }
            *dv = dv.clamp(lower, upper);
            let meter_snap = METER.read().await;
            let phase_correction = match (meter_snap.phase_w, meter_snap.total_w) {
                (Some(phase), Some(total)) => phase - total,
                _ => {
                    log::warn!("Smart export: phase_w unavailable, falling back to total_w for export limit");
                    0.0
                }
            };
            drop(meter_snap);
            pid_step(dv, last_amps, last_meter, inner_tick, lower, upper, export_limit_w + phase_correction, "export").await
        }
    } else if smart_charge_active {
        *dv = 0.0;
        *inner_tick = 9;
        let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
        let amps = settings.v2h_max_amps.min(crate::MAX_AMPS);
        let soc_limit = settings.v2h_soc_max_boost.min(crate::MAX_SOC);
        drop(settings);
        if soc_limit <= *chademo.soc() {
            if last_amps > 0.5 {
                log::info!("Smart charge SoC limit reached ({}% >= {}%), stopping charge", chademo.soc(), soc_limit);
            }
            0.0
        } else { (amps as f32).min(chademo.requested_charging_amps()) }
    } else if ev_drain_protection_active {
        *dv = 0.0;
        *inner_tick = 9;
        0.0
    } else {
        let settings = crate::data_io::operator_settings::OPERATOR_SETTINGS.read().await;
        let self_use = settings.self_use;
        let export_excess_solar = settings.export_excess_solar;
        let soc_min = settings.v2h_soc_min.max(crate::MIN_SOC);
        let soc_max = settings.v2h_soc_max.min(crate::MAX_SOC);
        let max_amps = settings.v2h_max_amps.min(crate::MAX_AMPS);
        drop(settings);
        let soc_at_min = *chademo.soc() <= soc_min;
        let soc_at_max = *chademo.soc() >= soc_max;

        // Permitted range for this tick -- derived from SoC position and operator flags
        let upper = if export_excess_solar || smart_export_excess_solar_active || soc_at_max { 0.0 } else { max_amps as f32 };
        let lower = if self_use && !soc_at_min { -(max_amps as f32) } else { 0.0 };
        *dv = dv.clamp(lower, upper);
        pid_step(dv, last_amps, last_meter, inner_tick, lower, upper, 0.0, "V2h").await
    }
}

async fn init_pre(
    predata: &std::sync::Arc<tokio::sync::Mutex<crate::pre_charger::PreCharger>>,
    t100ms: Duration,
    pre_tx: &tokio::sync::mpsc::Sender<PreCommand>,
) -> Result<(), IndraError> {
    log::info!("Initalise PRE");
    let mut c = false;
    let mut counter = 0;
    while !c {
        if counter > 20 {
            log::error!("Initalise PRE stage 1 timed out after | {counter}s");
            return Err(IndraError::Timeout);
        }
        sleep(Duration::from_millis(1000)).await;
        counter += 1;
        let pre = predata.lock().await;
        c = pre.get_state().is_online()
    }
    log::info!("Pre stage 1");
    log_error!("", pre_tx.send(PreCommand::DcAmpsSetpoint(1.0)).await);
    sleep(t100ms).await;
    log_error!("", pre_tx.send(PreCommand::DcVoltsSetpoint(370.0)).await);
    sleep(t100ms).await;

    c = false;
    counter = 0;
    while !c {
        if counter > 5 {
            log::error!("Initalise PRE stage 2 timed out after | {counter}s");
            return Err(IndraError::Timeout);
        }
        sleep(Duration::from_millis(1000)).await;
        counter += 1;
        let pre = predata.lock().await;
        if pre.get_dc_setpoint_volts() as u16 == 370 && pre.get_dc_setpoint_amps() as u16 == 1 {
            c = pre.volts_equal();
        };
    }

    log::info!("Pre stage 2");
    Ok(())
}

async fn k_line(can: &mut CANSocket, chademo: &mut Chademo) -> Result<(), IndraError> {
    let mut counter = 100u8; //10 seconds
    sleep(Duration::from_millis(100)).await;
    while counter != 0 {
        //100ms loop
        // log::debug!("K-loop {counter}");
        recv_send(can, chademo, false).await?;

        // log::info!("{}", chademo.x102.status);
        if chademo.k_line_check()? {
            log::debug!("K && 102.5.0 ok");
            let x102status: u8 = chademo.x102.status.into();
            let x109status: u8 = chademo.x109.status.into();

            log::debug!(
                "102s:{:02x}, 109s:{:02x}, Soc:{}%",
                x102status,
                x109status,
                chademo.soc()
            );
            return Ok(());
        };

        counter -= 1
    }
    Err(IndraError::Timeout)
}

async fn precharge(
    can: &mut CANSocket,
    chademo: &mut Chademo,
    pre_tx: &tokio::sync::mpsc::Sender<PreCommand>,
    predata: &Arc<Mutex<PreCharger>>,
) -> Result<(), IndraError> {
    let mut old_soc = 255;
    let mut counter = 100u8;
    while counter != 0 {
        counter -= 1;
        recv_send(can, chademo, true).await?;
        log::debug!("Counter | {counter}");
        let x102status: u8 = chademo.x102.status.into();
        let x109status: u8 = chademo.x109.status.into();

        log::info!(
            "102s:{:02x}, 109s:{:02x}, Soc:{}%",
            x102status,
            x109status,
            chademo.soc()
        );

        if (10..=100).contains(chademo.soc()) {
            if &old_soc != chademo.soc() {
                old_soc = *chademo.soc();
                log_error!(
                    format!("SoC at  | {}", chademo.soc()),
                    pre_tx
                        .send(PreCommand::DcVoltsSetpoint(chademo.soc_to_voltage()))
                        .await
                );
            }
        }
        if old_soc <= 100 {
            let predata = predata.lock().await;
            chademo.x109.output_voltage = predata.get_dc_output_volts();

            log::trace!("precharge x102: {:?}", chademo.x102);
            log::trace!("precharge x109: {:?}", chademo.x109);
            if chademo.x102.contactors_closed() {
                if predata.volts_equal() {
                    if chademo.x102.car_ready() {
                        log::trace!("precharge chademo state: {:?}", chademo);
                        return chademo.close_contactors();
                    } else {
                        log::warn!("x102.5.0 low");
                    }
                } else {
                    log::warn!("Pre volts not equal");
                }
            } else {
                log::warn!("x102.5.3 high");
            }
        }

        // if x102.5.3
    }
    Err(IndraError::Timeout)
}

async fn update_chademo_mutex(chademo: &Chademo) {
    *crate::chademo::state::CHADEMO.lock().await = *chademo;
}

async fn update_panel_leds(led_tx: &LedTx, chademo: &Chademo, last_logo: &mut crate::data_io::panel::State) {
    use crate::data_io::panel::State;

    // Priority: EV fault > meter stale > operating mode
    let meter = *crate::data_io::meter::METER.read().await;
    let meter_stale = meter.total_w.is_none() && meter.last_total_update.is_some();

    let logo = if chademo.fault() {
        State::Error
    } else if meter_stale {
        State::MeterStale
    } else {
        State::from(chademo.state())
    };
    if logo != *last_logo {
        log_error!("Update LED Logo", led_tx.send(LedCommand::Logo(logo)).await);
        *last_logo = logo;
    }

    // Bars
    let bar_cmd = match chademo.state() {
        OperationMode::Idle | OperationMode::Quit => LedCommand::SocBar(0),
        _ => {
            let amps_pct = ((chademo.output_amps().abs() as u32)
                .min(MAX_AMPS as u32) * 100
                / MAX_AMPS as u32) as u8;
            let neg = chademo.output_amps().is_negative();
            log_error!("Update LED EnergyBar", led_tx.send(LedCommand::EnergyBar(amps_pct, neg)).await);
            LedCommand::SocBar(*chademo.soc())
        }
    };
    log_error!("Update LED SocBar", led_tx.send(bar_cmd).await);
}

#[cfg(test)]
mod test {
    use chademo_v2::{X102, X109};

    use super::*;

    #[test]
    fn test_x109() {
        // let mut chademo = Chademo::new();
        let mut x109 = X109::new(3, true);
        println!("{:02x}", Into::<u8>::into(x109.status));  //test code
        assert!(!x109.status.status_vehicle_connector_lock);
        assert!(!x109.status.status_station);
        x109.status = 0x24.into();
        println!("{:02x}", Into::<u8>::into(x109.status));  //test code
        assert!(x109.status.status_vehicle_connector_lock);
        x109.status = 0x05.into();
        println!("{:02x}", Into::<u8>::into(x109.status)); //test code
        assert!(x109.status.status_vehicle_connector_lock);
        assert!(x109.status.status_station);
    }

    #[test]
    fn test1() {
        let frame = CANFrame::new(
            0x102,
            [0x2, 0x9A, 0x01, 0x00, 0x0, 0xC8, 0x56, 0x00].as_slice(),
            false,
            false,
        )
        .unwrap();
        let x102: X102 = X102::from(&frame);
        //         02 9A 01 00 00 C8 56 00    <x102>
        // 100ms
        // ControlProtocolNumberEV: 2-
        // TargetBatteryVoltage: 410V
        // ChargingCurrentRequest: 0A
        // FaultBatteryVoltageDeviation: Normal
        // FaultHighBatteryTemperature: Normal
        // FaultBatteryCurrentDeviation: Normal
        // FaultBatteryUndervoltage: Normal
        // FaultBatteryOvervoltage: Normal
        // StatusNormalStopRequest: No request
        // StatusVehicle: EV contactor open or welding detection finished
        // StatusChargingSystem: Normal
        // StatusVehicleShifterPosition: Parked
        // StatusVehicleCharging: Disabled
        // ChargingRate: 86%
        assert_eq!(x102.control_protocol_number_ev, 2);
        assert_eq!(x102.target_battery_voltage, 410.0);
        assert_eq!(x102.charging_current_request, 0);
        assert_eq!(x102.fault(), false);
        assert_eq!(x102.status.status_vehicle, true); // EV contactors open
        assert_eq!(x102.status.status_vehicle_charging, false); // No commanded charge
    }

    #[test]
    fn test2() {
        let frame = CANFrame::new(
            0x109,
            [0x2, 0x9A, 0x01, 0x00, 0x0, 0xC0, 0x56, 0x00].as_slice(),
            false,
            false,
        )
        .unwrap();
        let x102: X102 = X102::from(&frame);

        //         02 9A 01 00 00 C0 56 00    <x102>
        // 100ms
        // ControlProtocolNumberEV: 2-
        // TargetBatteryVoltage: 410V
        // ChargingCurrentRequest: 0A
        // FaultBatteryVoltageDeviation: Normal
        // FaultHighBatteryTemperature: Normal
        // FaultBatteryCurrentDeviation: Normal
        // FaultBatteryUndervoltage: Normal
        // FaultBatteryOvervoltage: Normal
        // StatusNormalStopRequest: No request
        // StatusVehicle: EV contactor closed or during welding detection
        // StatusChargingSystem: Normal
        // StatusVehicleShifterPosition: Parked
        // StatusVehicleCharging: Disabled
        // ChargingRate: 86%
        // Charging_close_unknown1: Enabled
        // Charging_close_unknown2: Enabled

        assert_eq!(x102.control_protocol_number_ev, 2);
        assert_eq!(x102.target_battery_voltage, 410.0);
        assert_eq!(x102.charging_current_request, 0);
        assert_eq!(x102.fault(), false);
        assert_eq!(x102.status.status_vehicle, false); // EV contactors closed
        assert_eq!(x102.status.status_vehicle_charging, false); // No commanded charge
    }

    #[test]
    fn test3() {
        let frame = CANFrame::new(
            0x109,
            [0x2, 0x9A, 0x01, 0x00, 0x0, 0xC1, 0x56, 0x00].as_slice(),
            false,
            false,
        )
        .unwrap();
        let x102: X102 = X102::from(&frame);

        //  02 9A 01 0E 00 C1 56 00    <x102>
        // 100ms
        // ControlProtocolNumberEV: 2-
        // TargetBatteryVoltage: 410V
        // ChargingCurrentRequest: 14A
        // FaultBatteryVoltageDeviation: Normal
        // FaultHighBatteryTemperature: Normal
        // FaultBatteryCurrentDeviation: Normal
        // FaultBatteryUndervoltage: Normal
        // FaultBatteryOvervoltage: Normal
        // StatusNormalStopRequest: No request
        // StatusVehicle: EV contactor closed or during welding detection
        // StatusChargingSystem: Normal
        // StatusVehicleShifterPosition: Parked
        // StatusVehicleCharging: Enabled
        // ChargingRate: 86%
        // Charging_close_unknown1: Enabled
        // Charging_close_unknown2: Enabled

        assert_eq!(x102.control_protocol_number_ev, 2);
        assert_eq!(x102.target_battery_voltage, 410.0);
        assert_eq!(x102.charging_current_request, 0);
        assert_eq!(x102.fault(), false);
        assert_eq!(x102.status.status_vehicle, false); // EV contactors closed
        assert_eq!(x102.status.status_vehicle_charging, true); // Charge commanded
    }
}
