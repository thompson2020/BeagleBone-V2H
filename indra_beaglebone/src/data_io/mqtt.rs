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

    // Numeric sensors: (id, name, value_template, unit, device_class or "")
    let sensors: &[(&str, &str, &str, &str, &str)] = &[
        ("beaglebone_v2h_soc",                    "SoC",
         "{{ value_json.chademo.soc }}", "%", "battery"),
        ("beaglebone_v2h_dc_power_w",             "DC Power",
         "{{ ((value_json.pre.dc_output_volts | float(0)) * (value_json.pre.dc_output_amps | float(0))) | round(0) | int }}", "W", "power"),
        ("beaglebone_v2h_charger_ac_power_w",     "AC Power",
         "{{ value_json.meter.charger_w | default(0) | round(0) | int }}", "W", "power"),
        ("beaglebone_v2h_grid_power_w",           "Grid Power",
         "{{ value_json.meter.total_w | default(0) | round(0) | int }}", "W", "power"),
        ("beaglebone_v2h_grid_phase_power_w",     "Grid Phase Power",
         "{{ value_json.meter.phase_w | default(0) | round(0) | int }}", "W", "power"),
        ("beaglebone_v2h_efficiency_pct",         "Efficiency",
         "{{ value_json.meter.efficiency | default(0) | round(1) }}", "%", ""),
        ("beaglebone_v2h_pre_temp_c",             "PRE Temperature",
         "{{ value_json.pre.temp | round(1) }}", "°C", "temperature"),
        ("beaglebone_v2h_pre_fan_duty_pct",       "PRE Fan Duty",
         "{{ value_json.pre.fan_duty }}", "%", ""),
        ("beaglebone_v2h_charging_current_req_a", "Charging Current Request",
         "{{ value_json.chademo.x102.charging_current_request }}", "A", "current"),
        ("beaglebone_v2h_dc_output_volts",        "DC Output Volts",
         "{{ value_json.pre.dc_output_volts | round(1) }}", "V", "voltage"),
        ("beaglebone_v2h_dc_output_amps",         "DC Output Amps",
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
        ("beaglebone_v2h_operation_mode", "Operation Mode", "{{ value_json.chademo.state }}"),
        ("beaglebone_v2h_pre_state",      "PRE State",      "{{ value_json.pre.state }}"),
        ("beaglebone_v2h_active_mode",    "Active Mode",
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

    // Binary sensors: (id, name, supervisory field)
    let binary_sensors: &[(&str, &str, &str)] = &[
        ("beaglebone_v2h_smart_charge_request",        "Smart Charge Request",          "smart_charge_request"),
        ("beaglebone_v2h_smart_charge_active",         "Smart Charge Active",           "smart_charge_active"),
        ("beaglebone_v2h_ev_drain_protection_request", "EV Drain Protection Request",   "ev_drain_protection_request"),
        ("beaglebone_v2h_ev_drain_protection_active",  "EV Drain Protection Active",    "ev_drain_protection_active"),
        ("beaglebone_v2h_smart_export_request",        "Smart Export Request",          "smart_export_request"),
        ("beaglebone_v2h_smart_export_active",         "Smart Export Active",           "smart_export_active"),
        ("beaglebone_v2h_smart_export_solar_request",  "Smart Export Solar Request",    "smart_export_excess_solar_request"),
        ("beaglebone_v2h_smart_export_solar_active",   "Smart Export Solar Active",     "smart_export_excess_solar_active"),
        ("beaglebone_v2h_ready_to_drive_request",      "Ready to Drive Request",        "ready_to_drive_request"),
        ("beaglebone_v2h_ready_to_drive_active",       "Ready to Drive Active",         "ready_to_drive_active"),
        ("beaglebone_v2h_off_peak_charging_request",   "Off-Peak Charging Request",     "off_peak_charging_request"),
        ("beaglebone_v2h_off_peak_charging_active",    "Off-Peak Charging Active",      "off_peak_charging_active"),
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
 
    let cmd_topic = format!("{}/command", cfg.base_topic);

    // Operator settings switches: read + write boolean settings
    // unique_id uses beaglebone_ prefix; object_id uses v2h_ so entity_id is switch.v2h_*
    let settings_switches: &[(&str, &str, &str, &str)] = &[
        // (unique_id, object_id, name, settings_field)
        ("beaglebone_v2h_ready_to_drive_enabled",      "v2h_ready_to_drive_enabled",      "Ready to Drive",      "ready_to_drive"),
        ("beaglebone_v2h_off_peak_charging_enabled",   "v2h_off_peak_charging_enabled",   "Off-Peak Charging",   "off_peak_charging"),
        ("beaglebone_v2h_smart_export_enabled",        "v2h_smart_export_enabled",        "Smart Export",        "smart_export"),
        ("beaglebone_v2h_smart_export_solar_enabled",  "v2h_smart_export_solar_enabled",  "Smart Export Solar",  "smart_export_excess_solar"),
        ("beaglebone_v2h_smart_charge_enabled",        "v2h_smart_charge_enabled",        "Smart Charge",        "smart_charge"),
        ("beaglebone_v2h_ev_drain_protection_enabled", "v2h_ev_drain_protection_enabled", "EV Drain Protection", "ev_drain_protection"),
        ("beaglebone_v2h_self_use",                    "v2h_self_use",                    "Self Use",            "self_use"),
        ("beaglebone_v2h_export_excess_solar",         "v2h_export_excess_solar",         "Export Excess Solar", "export_excess_solar"),
        ("beaglebone_v2h_charge_eco",                  "v2h_charge_eco",                  "Charge Eco",          "charge_eco"),
    ];
    for &(uid, oid, name, field) in settings_switches {
        let tpl     = format!("{{{{value_json.settings.{} | lower}}}}", field);
        let pay_on  = format!("{{\"cmd\":{{\"SetSetting\":{{\"{}\":true}}}}}}", field);
        let pay_off = format!("{{\"cmd\":{{\"SetSetting\":{{\"{}\":false}}}}}}", field);
        let payload = json!({
            "unique_id": uid,
            "object_id": oid,
            "name": name,
            "state_topic": state,
            "value_template": tpl,
            "state_on": "true",
            "state_off": "false",
            "command_topic": cmd_topic,
            "payload_on": pay_on,
            "payload_off": pay_off,
            "entity_category": "config",
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        publish_one_discovery(client, format!("homeassistant/switch/{}/config", uid), payload).await;
    }

    // Operator settings number entities: native HA slider + sends SetSetting on change
    // unique_id uses beaglebone_ prefix; object_id uses v2h_ so entity_id is number.v2h_*
    let settings_numbers: &[(&str, &str, &str, &str, i64, i64, i64, &str)] = &[
        // (unique_id, object_id, name, settings_field, min, max, step, unit)
        ("beaglebone_v2h_soc_min",              "v2h_soc_min",              "V2H SoC Min",          "v2h_soc_min",             10, 100,   1, "%"),
        ("beaglebone_v2h_soc_max",              "v2h_soc_max",              "V2H SoC Max",          "v2h_soc_max",             10, 100,   1, "%"),
        ("beaglebone_v2h_soc_max_boost",        "v2h_soc_max_boost",        "V2H SoC Max Boost",    "v2h_soc_max_boost",       10, 100,   1, "%"),
        ("beaglebone_v2h_max_amps",             "v2h_max_amps",             "V2H Max Amps",         "v2h_max_amps",             1,  16,   1, "A"),
        ("beaglebone_v2h_rtd_soc",              "v2h_rtd_soc",              "RTD SoC Target",       "ready_to_drive_soc",      10, 100,   1, "%"),
        ("beaglebone_v2h_smart_export_limit_w", "v2h_smart_export_limit_w", "Smart Export Limit",   "smart_export_limit_w",     0, 10000, 100, "W"),
        ("beaglebone_v2h_smart_export_soc_min", "v2h_smart_export_soc_min", "Smart Export SoC Min", "smart_export_soc_min",    10, 100,   1, "%"),
        ("beaglebone_v2h_charge_soc_limit",     "v2h_charge_soc_limit",     "Charge SoC Limit",     "charge_soc_limit",        10, 100,   1, "%"),
        ("beaglebone_v2h_charge_amps",          "v2h_charge_amps",          "Charge Amps",          "charge_amps",              1,  16,   1, "A"),
    ];
    for &(uid, oid, name, field, min, max, step, unit) in settings_numbers {
        let tpl     = format!("{{{{value_json.settings.{}}}}}", field);
        let cmd_tpl = format!("{{\"cmd\":{{\"SetSetting\":{{\"{}\":{}}}}}}}", field, "{{ value | int }}");
        let payload = json!({
            "unique_id": uid,
            "object_id": oid,
            "name": name,
            "state_topic": state,
            "value_template": tpl,
            "command_topic": cmd_topic,
            "command_template": cmd_tpl,
            "min": min,
            "max": max,
            "step": step,
            "unit_of_measurement": unit,
            "entity_category": "config",
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        publish_one_discovery(client, format!("homeassistant/number/{}/config", uid), payload).await;
    }

    // Operator settings text entities: editable text field + sends SetSetting on change
    // Used for HH:MM time fields (min/max = string length 5)
    let settings_times: &[(&str, &str, &str, &str)] = &[
        // (unique_id, object_id, name, settings_field)
        ("beaglebone_v2h_rtd_start_time", "v2h_rtd_start_time", "RTD Start Time", "ready_to_drive_start_time"),
        ("beaglebone_v2h_rtd_end_time",   "v2h_rtd_end_time",   "RTD End Time",   "ready_to_drive_end_time"),
        ("beaglebone_v2h_off_peak_start", "v2h_off_peak_start", "Off-Peak Start", "off_peak_start"),
        ("beaglebone_v2h_off_peak_end",   "v2h_off_peak_end",   "Off-Peak End",   "off_peak_end"),
    ];
    for &(uid, oid, name, field) in settings_times {
        let tpl     = format!("{{{{value_json.settings.{}}}}}", field);
        let cmd_tpl = format!("{{\"cmd\":{{\"SetSetting\":{{\"{}\":\"{}\"}}}}}}", field, "{{ value }}");
        let payload = json!({
            "unique_id": uid,
            "object_id": oid,
            "name": name,
            "state_topic": state,
            "value_template": tpl,
            "command_topic": cmd_topic,
            "command_template": cmd_tpl,
            "min": 5,
            "max": 5,
            "pattern": r"^([0-1][0-9]|2[0-3]):[0-5][0-9]$",
            "entity_category": "config",
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        publish_one_discovery(client, format!("homeassistant/text/{}/config", uid), payload).await;
    }

    // RTD day binary sensors: display-only, one per day (Mon=0 .. Sun=6)
    let day_names = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let day_labels = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    for (i, (day, label)) in day_names.iter().zip(day_labels.iter()).enumerate() {
        let uid = format!("beaglebone_v2h_rtd_day_{}", day);
        let oid = format!("v2h_rtd_day_{}", day);
        let tpl = format!("{{{{value_json.settings.ready_to_drive_days[{}] | lower}}}}", i);
        let payload = json!({
            "unique_id": uid,
            "object_id": oid,
            "name": format!("RTD {}", label),
            "state_topic": state,
            "value_template": tpl,
            "payload_on": "true",
            "payload_off": "false",
            "entity_category": "config",
            "availability_topic": avail,
            "payload_available": "online",
            "payload_not_available": "offline",
            "device": device.clone(),
        });
        publish_one_discovery(client, format!("homeassistant/binary_sensor/{}/config", uid), payload).await;
    }

    // Mode select — allows HA to change mode via v2h/command
    let mode_select = json!({
        "unique_id": "beaglebone_v2h_mode",
        "object_id": "beaglebone_v2h_mode",
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
