use crate::{error::IndraError, log_error, statics::ChademoTx};
use super::commands::Instruction;
use std::time::Instant;
use rumqttc::AsyncClient;
use super::config::MqttConfig;
use super::supervisor::SUPERVISORY;
use tokio::time::{sleep, Duration};

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
    let availability_topic = format!("{}/availability", mqtt_config.base_topic);
    mqttoptions.set_last_will(rumqttc::LastWill::new(
        &availability_topic, "offline", rumqttc::QoS::AtLeastOnce, true,
    ));
    let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);
    
    let mqtt_config_clone = mqtt_config.clone();
    // Clone the client into the spawn so it can resubscribe after reconnections.
    let client_for_reconnect = client.clone();
    let mode_tx_for_spawn = mode_tx.clone();

    tokio::spawn(async move {
        use rumqttc::Event;
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                    // Fires on first connect and every reconnect. With clean_session(true)
                    // the broker discards subscriptions on disconnect, so we resubscribe here.
                    log::info!("MQTT: connected/reconnected — subscribing to topics");
                    let avail = format!("{}/availability", mqtt_config_clone.base_topic);
                    if let Err(e) = client_for_reconnect
                        .publish(&avail, QoS::AtLeastOnce, true, "online")
                        .await
                    {
                        log::error!("MQTT: availability online publish | {e:?}");
                    }
                    // Spawn separately so the eventloop keeps polling while we publish 26 retained messages.
                    // Calling publish() here directly would deadlock once the 10-slot channel fills.
                    let disc_client = client_for_reconnect.clone();
                    let disc_cfg    = mqtt_config_clone.clone();
                    tokio::spawn(async move { publish_discovery(&disc_client, &disc_cfg).await; });
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
                    if let Err(e) = client_for_reconnect
                        .subscribe(&mqtt_config_clone.mqtt_smart_export_excess_solar_topic, QoS::AtMostOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to smart_export_excess_solar topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", mqtt_config_clone.mqtt_smart_export_excess_solar_topic);
                    }
                    let cmd_topic = format!("{}/command", mqtt_config_clone.base_topic);
                    if let Err(e) = client_for_reconnect
                        .subscribe(&cmd_topic, QoS::AtLeastOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to command topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", cmd_topic);
                    }
                }
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(msg))) => {
                    handle_publish(msg, &mqtt_config_clone, &mode_tx_for_spawn).await;
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



    let publish_client = client.clone();
    let publish_config = mqtt_config.clone();

    loop {
        sleep(Duration::from_secs(1)).await;

        let snap = super::status::snapshot().await;
        let msg = match serde_json::to_string(&snap) {
            Ok(d) => d,
            Err(e) => {
                log::error!("MQTT Serialize | {e}");
                continue;
            }
        };
        let topic = format!("{}/status", publish_config.base_topic);
        let msg_with_space = msg.replace(":", ": ");

        log::debug!("MQTT Publishing:  | {} = {msg_with_space}", &topic);

        // spawn to avoid latency spikes
        let client_send = publish_client.clone();
        tokio::task::spawn(async move {
            log_error!(
                "MQTT Publishing Chademo Data:",
                client_send
                    .publish(topic, QoS::AtLeastOnce, false, msg)
                    .await
                    .map_err(|e| IndraError::MqttSend(e))
            );
        });




    }
}

/// Extract a numeric value from a payload. Handles:
///   "1"                          — plain number string
///   true / false                 — bare JSON boolean (→ 1.0 / 0.0)
///   {"field": 1}                 — JSON object, dotted path
///   {"field": true}              — JSON object with boolean value
fn extract_value(payload: &str, field: &str) -> Option<f32> {
    use serde_json::Value;
    if let Ok(v) = payload.parse::<f32>() {
        return Some(v);
    }
    let json = match serde_json::from_str::<Value>(payload) {
        Ok(v) => v,
        Err(_) => {
            log::warn!("MQTT: payload is not a number or JSON | {}", payload);
            return None;
        }
    };
    // Bare boolean: true → 1.0, false → 0.0
    if let Some(b) = json.as_bool() {
        return Some(if b { 1.0 } else { 0.0 });
    }
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
        return Some(v as f32);
    }
    if let Some(b) = cur.as_bool() {
        return Some(if b { 1.0 } else { 0.0 });
    }
    log::warn!("MQTT: JSON field '{}' is not a number or bool | payload: {}", field, payload);
    None
}

/// Handle a single MQTT Publish message.
async fn handle_publish(msg: rumqttc::mqttbytes::v4::Publish, cfg: &MqttConfig, mode_tx: &ChademoTx) {
    let payload = String::from_utf8_lossy(&msg.payload);
    let payload = payload.trim().replace(['\n', '\r'], "");
    log::debug!("MQTT recv | topic: '{}' payload: '{}'", msg.topic, payload);

    let command_topic = format!("{}/command", cfg.base_topic);
    if msg.topic == command_topic {
        match serde_json::from_str::<Instruction>(&payload) {
            Ok(inst) => {
                if !super::commands::dispatch(inst.cmd, mode_tx).await {
                    log::warn!("MQTT: command not supported | {payload}");
                }
            }
            Err(e) => log::warn!("MQTT command: parse error | {e} | {payload}"),
        }
        return;
    }

    if msg.topic == cfg.mqtt_smart_charge_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_smart_charge_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_smart_charge_request(enabled, false);
            log::debug!("MQTT: smart_charge_request → {}", enabled);
        }
    }

    if msg.topic == cfg.mqtt_ev_drain_protection_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_ev_drain_protection_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_ev_drain_protection_request(enabled, false);
            log::info!("MQTT: ev_drain_protection_request → {}", enabled);
        }
    }

    if msg.topic == cfg.mqtt_smart_export_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_smart_export_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_smart_export_request(enabled, false);
            log::debug!("MQTT: smart_export_request → {}", enabled);
        }
    }

    if msg.topic == cfg.mqtt_smart_export_excess_solar_topic {
        if let Some(v) = extract_value(&payload, &cfg.mqtt_smart_export_excess_solar_field) {
            let enabled = v > 0.0;
            SUPERVISORY.write().await.update_smart_export_excess_solar_request(enabled, false);
            log::debug!("MQTT: smart_export_excess_solar_request → {}", enabled);
        }
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
                false
            }
        }
        None => false,
    }
}

async fn publish_one_discovery(client: &AsyncClient, topic: String, payload: serde_json::Value) {
    use rumqttc::QoS;
    let s = match serde_json::to_string(&payload) {
        Ok(v) => v,
        Err(e) => { log::error!("Discovery: serialize | {e}"); return; }
    };
    log::debug!("Discovery: {}", topic);
    if let Err(e) = client.publish(topic, QoS::AtLeastOnce, true, s).await {
        log::error!("Discovery: publish failed | {e:?}");
    }
}

async fn publish_discovery(client: &AsyncClient, cfg: &MqttConfig) {
    use serde_json::json;
    log::info!("MQTT: publishing HA Discovery config");

    let device = json!({
        "identifiers": ["BeagleBone-V2H"],
        "name": "V2H",
        "manufacturer": "Indra"
    });
    let state = format!("{}/status", cfg.base_topic);
    let avail = format!("{}/availability", cfg.base_topic);

    // Numeric sensors: (object_id, name, value_template, unit, device_class or "")
    let sensors: &[(&str, &str, &str, &str, &str)] = &[
        ("v2h_soc",                    "SoC",
         "{{ value_json.chademo.soc }}", "%", "battery"),
        ("v2h_dc_power_w",             "DC Power",
         "{{ ((value_json.pre.dc_output_volts | float(0)) * (value_json.pre.dc_output_amps | float(0))) | round(0) | int }}", "W", "power"),
        ("v2h_charger_ac_power_w",     "AC Power",
         "{{ value_json.meter.charger_w | default(0) | round(0) | int }}", "W", "power"),
        ("v2h_grid_power_w",           "Grid Power",
         "{{ value_json.meter.total_w | default(0) | round(0) | int }}", "W", "power"),
        ("v2h_grid_phase_power_w",     "Grid Phase Power",
         "{{ value_json.meter.phase_w | default(0) | round(0) | int }}", "W", "power"),
        ("v2h_efficiency_pct",         "Efficiency",
         "{{ value_json.meter.efficiency | default(0) | round(1) }}", "%", ""),
        ("v2h_pre_temp_c",             "PRE Temperature",
         "{{ value_json.pre.temp | round(1) }}", "°C", "temperature"),
        ("v2h_pre_fan_duty_pct",       "PRE Fan Duty",
         "{{ value_json.pre.fan_duty }}", "%", ""),
        ("v2h_charging_current_req_a", "Charging Current Request",
         "{{ value_json.chademo.x102.charging_current_request }}", "A", "current"),
        ("v2h_dc_output_volts",        "DC Output Volts",
         "{{ value_json.pre.dc_output_volts | round(1) }}", "V", "voltage"),
        ("v2h_dc_output_amps",         "DC Output Amps",
         "{{ value_json.pre.dc_output_amps | round(1) }}", "A", "current"),
    ];
    for &(id, name, tpl, unit, dc) in sensors {
        let mut payload = json!({
            "unique_id": id,
            "object_id": id,
            "name": name,
            "state_topic": state,
            "value_template": tpl,
            "unit_of_measurement": unit,
            "state_class": "measurement",
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        if !dc.is_empty() {
            payload["device_class"] = json!(dc);
        }
        publish_one_discovery(client, format!("homeassistant/sensor/{}/config", id), payload).await;
    }

    // Text sensors (no unit/device_class/state_class)
    let text_sensors: &[(&str, &str, &str)] = &[
        ("v2h_operation_mode", "Operation Mode", "{{ value_json.chademo.state }}"),
        ("v2h_pre_state",      "PRE State",      "{{ value_json.pre.state }}"),
        ("v2h_active_mode",    "Active Mode",
         "{% if value_json.supervisory.ready_to_drive_active %}Ready to Drive\
{% elif value_json.supervisory.off_peak_charging_active %}Off-Peak\
{% elif value_json.supervisory.smart_export_active %}Smart Export\
{% elif value_json.supervisory.smart_export_excess_solar_active %}Excess Solar\
{% elif value_json.supervisory.smart_charge_active %}Smart Charge\
{% elif value_json.supervisory.ev_drain_protection_active %}EV Drain Protection\
{% else %}Normal{% endif %}"),
    ];
    for &(id, name, tpl) in text_sensors {
        let payload = json!({
            "unique_id": id,
            "object_id": id,
            "name": name,
            "state_topic": state,
            "value_template": tpl,
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        publish_one_discovery(client, format!("homeassistant/sensor/{}/config", id), payload).await;
    }

    // Binary sensors: (object_id, name, supervisory field)
    let binary_sensors: &[(&str, &str, &str)] = &[
        ("v2h_smart_charge_request",        "Smart Charge Request",          "smart_charge_request"),
        ("v2h_smart_charge_active",         "Smart Charge Active",           "smart_charge_active"),
        ("v2h_ev_drain_protection_request", "EV Drain Protection Request",   "ev_drain_protection_request"),
        ("v2h_ev_drain_protection_active",  "EV Drain Protection Active",    "ev_drain_protection_active"),
        ("v2h_smart_export_request",        "Smart Export Request",          "smart_export_request"),
        ("v2h_smart_export_active",         "Smart Export Active",           "smart_export_active"),
        ("v2h_smart_export_solar_request",  "Smart Export Solar Request",    "smart_export_excess_solar_request"),
        ("v2h_smart_export_solar_active",   "Smart Export Solar Active",     "smart_export_excess_solar_active"),
        ("v2h_ready_to_drive_request",      "Ready to Drive Request",        "ready_to_drive_request"),
        ("v2h_ready_to_drive_active",       "Ready to Drive Active",         "ready_to_drive_active"),
        ("v2h_off_peak_charging_request",   "Off-Peak Charging Request",     "off_peak_charging_request"),
        ("v2h_off_peak_charging_active",    "Off-Peak Charging Active",      "off_peak_charging_active"),
    ];
    for &(id, name, field) in binary_sensors {
        let tpl = format!("{{{{value_json.supervisory.{} | lower}}}}", field);
        let payload = json!({
            "unique_id": id,
            "object_id": id,
            "name": name,
            "state_topic": state,
            "value_template": tpl,
            "payload_on": "true",
            "payload_off": "false",
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        publish_one_discovery(client, format!("homeassistant/binary_sensor/{}/config", id), payload).await;
    }

    // Mode select — allows HA to change mode via v2h/command
    let cmd_topic = format!("{}/command", cfg.base_topic);
    let mode_select = json!({
        "unique_id": "v2h_mode",
        "object_id": "v2h_mode",
        "name": "Mode",
        "state_topic": state,
        "value_template": "{{ value_json.chademo.state }}",
        "command_topic": cmd_topic,
        "command_template": "{\"cmd\":{\"SetMode\":\"{{ value }}\"}}",
        "options": ["Idle", "V2h", "Charge", "Discharge"],
        "availability_topic": avail,
        "payload_available": "online",
        "payload_not_available": "offline",
        "device": device,
    });
    publish_one_discovery(client, "homeassistant/select/v2h_mode/config".to_string(), mode_select).await;
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

        if is_stale(snap.smart_export_excess_solar_request_update, timeout, "smart_export_excess_solar_request") {
            SUPERVISORY.write().await.update_smart_export_excess_solar_request(false, true);
        }
    }
}
