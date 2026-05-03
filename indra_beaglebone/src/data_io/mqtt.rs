use crate::{error::IndraError, global_state::{ChargeParameters, OperationMode}, log_error, statics::ChademoTx};
use chrono::Timelike;
use std::sync::Arc;
use std::time::Instant;
use super::config::MqttConfig;
use super::supervisor::SUPERVISORY;
use tokio::time::{sleep, Duration};

static HANDLE_SMART_CHARGE_CHANGE_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct SmartChargeRunningGuard;
impl Drop for SmartChargeRunningGuard {
    fn drop(&mut self) {
        HANDLE_SMART_CHARGE_CHANGE_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}



pub async fn mqtt_task(mqtt_config: MqttConfig, mode_tx: ChademoTx) -> Result<(), IndraError> {
    use rumqttc::{AsyncClient, MqttOptions, QoS};

    log::info!("Starting thread: mqtt_task   | {}", tokio::task::id());
    if !mqtt_config.enabled {
        log::warn!("MQTT not enabled in config");
        return Ok(());
    }
    
	// Clone the values we need BEFORE they are moved into mqttoptions
    let client_id = mqtt_config.client_id.clone();
    let host = mqtt_config.host.clone();
    let port = mqtt_config.port;
    let mut mqttoptions = MqttOptions::new(client_id, host, port);
    mqttoptions.set_keep_alive(Duration::from_secs(5));
    let username = mqtt_config.username.clone();
    let password = mqtt_config.password.clone();
    mqttoptions.set_credentials(username, password);
    mqttoptions.set_transport(rumqttc::Transport::Tcp);
    mqttoptions.set_clean_session(true);
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);
    
    let mode_tx_for_eventloop = mode_tx.clone();
    let mqtt_config_clone = mqtt_config.clone();
    // Clone the client into the spawn so it can resubscribe after reconnections.
    let client_for_reconnect = client.clone();

    tokio::spawn(async move {
        use rumqttc::Event;
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                    // Fires on first connect and every reconnect. With clean_session(true)
                    // the broker discards subscriptions on disconnect, so we resubscribe here.
                    log::info!("MQTT: connected/reconnected — subscribing to topics");
                    if let Err(e) = client_for_reconnect
                        .subscribe(&mqtt_config_clone.sub, QoS::AtLeastOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to command topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", mqtt_config_clone.sub);
                    }
                    if let Err(e) = client_for_reconnect
                        .subscribe(&mqtt_config_clone.mqtt_smart_charge_topic, QoS::AtMostOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to smart_charge topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", mqtt_config_clone.mqtt_smart_charge_topic);
                    }
                    if let Err(e) = client_for_reconnect
                        .subscribe(&mqtt_config_clone.mqtt_ev_drain_protection_topic, QoS::AtMostOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to ev_drain_protection topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", mqtt_config_clone.mqtt_ev_drain_protection_topic);
                    }
                    if let Err(e) = client_for_reconnect
                        .subscribe(&mqtt_config_clone.mqtt_smart_export_topic, QoS::AtMostOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to smart_export topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", mqtt_config_clone.mqtt_smart_export_topic);
                    }
                }
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(msg))) => {
                    handle_publish(msg, &mqtt_config_clone, mode_tx_for_eventloop.clone()).await;
                }
                Ok(_) => {}
                Err(e) => {
                    // Connection dropped — rumqttc will reconnect automatically.
                    // Log it so disconnects are visible, then keep polling.
                    log::error!("MQTT: connection error | {e:?}");
                }
            }
        }
    });

    log::info!("MQTT: Spawn Staleness check");
    tokio::spawn(start_staleness_checker(mqtt_config.clone()));



    let interval = mqtt_config.interval;
    let publish_client = client.clone();           
    let publish_config = mqtt_config.clone(); 

    loop {
        sleep(Duration::from_secs(interval.into())).await;

        let snap = super::status::snapshot().await;
        let msg = match serde_json::to_string(&snap) {
            Ok(d) => d,
            Err(e) => {
                log::error!("MQTT Serialize | {e}");
                continue;
            }
        };
        let topic = publish_config.topic.clone();
        let msg_with_space = msg.replace(":", ": ");

        log::debug!("MQTT Publishing:  | {} = {msg_with_space}", &topic);

        // spawn to avoid latency spikes
        let client_send = publish_client.clone();
        tokio::task::spawn(async move {
            log_error!(
                "MQTT Publishing Chademo Data:",
                client_send
                    .publish(topic, QoS::AtLeastOnce, true, msg)
                    .await
                    .map_err(|e| IndraError::MqttSend(e))
            );
        });




    }
}

/// Extract a value from either a plain number ("1") or a JSON payload with a
/// dotted field path ("state.value").  Returns None and logs a warning on failure.
fn extract_value(payload: &str, field: &str) -> Option<f32> {
    use serde_json::Value;
    if let Ok(v) = payload.parse::<f32>() {
        log::debug!("MQTT: plain number | {:.2}", v);
        return Some(v);
    }
    if let Ok(json) = serde_json::from_str::<Value>(payload) {
        let mut cur = &json;
        for key in field.split('.') {
            cur = match cur.get(key) {
                Some(v) => v,
                None => {
                    log::warn!("MQTT: JSON path '{}' not found | payload: {}", field, payload);
                    return None;
                }
            };
        }
        if let Some(v) = cur.as_f64() {
            log::debug!("MQTT: JSON field '{}' | {:.2}", field, v);
            return Some(v as f32);
        }
        log::warn!("MQTT: JSON field '{}' is not a number | payload: {}", field, payload);
        return None;
    }
    log::warn!("MQTT: payload is not a number or JSON | {}", payload);
    None
}

/// Handle a single MQTT Publish message.
/// handle_smart_charge_change is spawned so the event loop is not blocked by its sleeps.
async fn handle_publish(msg: rumqttc::mqttbytes::v4::Publish, cfg: &MqttConfig, mode_tx: ChademoTx) {
    let payload = String::from_utf8_lossy(&msg.payload);
    let payload = payload.trim().replace(['\n', '\r'], "");
    log::debug!("MQTT: publish | topic: '{}' payload: '{}'", msg.topic, payload);

    if msg.topic == cfg.sub {
        log::warn!("MQTT: command topic not yet implemented | {}", msg.topic);
    }

    if msg.topic == cfg.mqtt_smart_charge_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_smart_charge_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_smart_charge_request(enabled, false);
            log::debug!("MQTT: smart_charge_request → {}", enabled);
            let tx = mode_tx.clone();
            tokio::spawn(async move {
                handle_smart_charge_change(enabled, &tx).await;
            });
        }
    }

    if msg.topic == cfg.mqtt_ev_drain_protection_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_ev_drain_protection_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_ev_drain_protection_request(enabled, false);
            log::debug!("MQTT: ev_drain_protection_request → {}", enabled);
        }
    }

    if msg.topic == cfg.mqtt_smart_export_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_smart_export_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_smart_export_request(enabled, false);
            log::debug!("MQTT: smart_export_request → {}", enabled);
        }
    }
}





// Handles smart charge activation from Octopus IOG slots
// Uses soft start/stop to avoid sudden 0 <-> 6.4kW reversal
async fn handle_smart_charge_change(enabled: bool, mode_tx: &ChademoTx) {
  
    if HANDLE_SMART_CHARGE_CHANGE_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        log::warn!("Smart Charge: Received but already running - ignoring new trigger");
        return;
    }
    let _guard = SmartChargeRunningGuard;

      // Use a static to remember the last known state
    static LAST_SMART_CHARGE: std::sync::atomic::AtomicBool = 
        std::sync::atomic::AtomicBool::new(false);

    let previous = LAST_SMART_CHARGE.load(std::sync::atomic::Ordering::Relaxed);

    // If the value hasn't changed → do nothing and return early
    if previous == enabled {
        log::debug!("Smart Charge: Received but value not changed - No Action taken | {}", enabled);
        return;
    }
    // Update the stored state
    LAST_SMART_CHARGE.store(enabled, std::sync::atomic::Ordering::Relaxed);

    let requested_soc: f32 = 80.0;
    let cheap_start_min: u16 = 23 * 60 + 30; // 23:30      /// BODGE - READ FROM SCHEDULER / TIMED CHARGE SETTINGS INSTEAD
    let cheap_end_min: u16 = 5 * 60 + 30;   // 05:30       /// BODGE - READ FROM SCHEDULER / TIMED CHARGE SETTINGS INSTEAD
    let now = chrono::Local::now();
    let current_min = (now.hour() * 60 + now.minute()) as u16;
    let in_cheap_window = if cheap_start_min > cheap_end_min {
        current_min >= cheap_start_min || current_min < cheap_end_min
    } else {
        current_min >= cheap_start_min && current_min < cheap_end_min
    };
    //Check(in_cheap_window) - Not checked to enable - This will be ignored as we wont be in V2h mode


    if enabled {

        // Soft start: 1A for 2 seconds
        let (current_mode, current_soc) = {
            let chademo = crate::chademo::state::CHADEMO.lock().await;
            (*chademo.state(), *chademo.soc() as f32)
        };
        log::debug!("Smart Charge->true = Checking current mode is V2h: {:?}", current_mode);
        
        //Checks - Not in V2h mode - ignore smart charge   
        if !matches!(current_mode, OperationMode::V2h) {
            log::debug!("Smart Charge->true - SKIPPED (current mode not V2h: {:?})", current_mode);
            return;
        }
        //Check -  SoC > Target  - ignore smart charge -> Immediate go to Idle
        if current_soc >= requested_soc {
            log::info!("Smart Charge->true - SoC > Request - GOING TO IDLE (SoC {:.1}% >= target {:.1}%)", current_soc, requested_soc);
            if let Err(e) = mode_tx.send(OperationMode::Idle).await {
                log::error!("MQTT: failed to send mode: {:?}", e);
            }
            log::info!("Smart Charge SWITCHED TO Idle");
            return;
        }
        //Check - Not in timed charge overnight - This will be ignored as we wont be in V2h mode


        //smart charge - start at 1A then after 2s move to 16A  
        log::info!("Smart Charge->true - soft start at 1A for 2s");
        let mut params = ChargeParameters::default();
        params.set_amps(1);
        params.set_soc_limit(requested_soc as u8);
        if let Err(e) = mode_tx.send(OperationMode::Charge(params)).await {
            log::error!("MQTT: failed to send mode: {:?}", e);
        }
        
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let current_mode = *crate::chademo::state::CHADEMO.lock().await.state();
        //2nd check incase someone has hit idle/off
        if !current_mode.is_charge() {
            log::warn!("Smart Charge->true - ABORTED(ramping to 16A) - no longer in Charge mode: {:?}", current_mode);
            return;
        }
        // Ramp up to full 16A
        log::info!("Smart Charge->true, soft start complete - ramping to 16A");
        let mut params = ChargeParameters::default();
        params.set_amps(16);
        params.set_soc_limit(requested_soc as u8);
        if let Err(e) = mode_tx.send(OperationMode::Charge(params)).await {
            log::error!("MQTT: failed to send mode: {:?}", e);
        }
        
        // TODO: Consider smoother ramp (e.g. 1A → 4A → 8A → 16A over time)

    } else {
        //smartcharge flag set to false - return to v2h mode 
        
        // Check - Overnight charging
        if in_cheap_window {
            log::info!("Smart Charge->false IGNORED (cheap-rate window active) - BODGE - READ FROM SCHEDULER / TIMED CHARGE SETTINGS INSTEAD");
            return;
        }
        
        // Soft stop: reduce to 1A for 2 seconds before returning to V2H
        let current_mode = *crate::chademo::state::CHADEMO.lock().await.state();
        log::debug!("Smart Charge->false = Checking current mode is Idle/Charge: {:?}", current_mode);
        if !matches!(current_mode, OperationMode::Idle | OperationMode::Charge(_)) {
            log::warn!("Smart Charge->false = IGNORED - not Idle/Charge: {:?}", current_mode);
            return;
        }
        log::info!("Smart Charge->false - soft stop at 1A for 2s");
        let mut params = ChargeParameters::default();
        params.set_amps(1);
        params.set_soc_limit(requested_soc as u8);
        if let Err(e) = mode_tx.send(OperationMode::Charge(params)).await {
            log::error!("MQTT: failed to send mode: {:?}", e);
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let current_mode = *crate::chademo::state::CHADEMO.lock().await.state();
        if !current_mode.is_charge() {
            log::warn!("Smart Charge->false - ABORTED(returning to V2H) - no longer in Charge mode: {:?}", current_mode);
            return;
        }
        log::info!("Smart Charge->false - soft stop complete - returning to V2H");
        if let Err(e) = mode_tx.send(OperationMode::V2h).await {
            log::error!("MQTT: failed to send mode: {:?}", e);
        }
        log::info!("Smart Charge SWITCHED TO V2H");

    }
}






















/// Returns true if `last` is older than `timeout_seconds`.
/// Never-received (None) returns false — no data yet is not the same as stale data.
fn is_stale(last: Option<Instant>, timeout_seconds: u64, label: &str) -> bool {
    match last {
        Some(t) => {
            let age = t.elapsed().as_secs();
            if age > timeout_seconds {
                log::warn!("MQTT staleness: {} STALE ({}s > {}s)", label, age, timeout_seconds);
                true
            } else {
                log::debug!("MQTT staleness: {} fresh ({}s)", label, age);
                false
            }
        }
        None => {
            log::debug!("MQTT staleness: {} never received data", label);
            false
        }
    }
}

async fn start_staleness_checker(mqtt_config: MqttConfig) {
    loop {
        sleep(Duration::from_secs(30)).await;

        let timeout = mqtt_config.mqtt_timeout_seconds;
        let snap = *SUPERVISORY.read().await;

        if is_stale(snap.smart_charge_request_update, timeout, "smart_charge_request") {
            SUPERVISORY.write().await.update_smart_charge_request(false, true);
        }

        if is_stale(snap.ev_drain_protection_request_update, timeout, "ev_drain_protection_request") {
            SUPERVISORY.write().await.update_ev_drain_protection_request(false, true);
        }

        if is_stale(snap.smart_export_request_update, timeout, "smart_export_request") {
            SUPERVISORY.write().await.update_smart_export_request(false, true);
        }
    }
}
