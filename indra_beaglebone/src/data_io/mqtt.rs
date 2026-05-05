use crate::{error::IndraError, log_error};
use std::time::Instant;
use super::config::MqttConfig;
use super::supervisor::SUPERVISORY;
use tokio::time::{sleep, Duration};

pub async fn mqtt_task(mqtt_config: MqttConfig) -> Result<(), IndraError> {
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
                    if let Err(e) = client_for_reconnect
                        .subscribe(&mqtt_config_clone.mqtt_smart_export_excess_solar_topic, QoS::AtMostOnce)
                        .await
                    {
                        log::error!("MQTT: failed to subscribe to smart_export_excess_solar topic | {e:?}");
                    } else {
                        log::info!("MQTT: subscribed | {}", mqtt_config_clone.mqtt_smart_export_excess_solar_topic);
                    }
                }
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(msg))) => {
                    handle_publish(msg, &mqtt_config_clone).await;
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
async fn handle_publish(msg: rumqttc::mqttbytes::v4::Publish, cfg: &MqttConfig) {
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
