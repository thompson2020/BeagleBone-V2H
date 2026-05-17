use crate::{
    chademo::state::{BOOSTPIN, ESTOPPIN, MASTERCONTACTOR, ONOFFPIN, RESETPCAPIN},
    global_state::OperationMode,
    pre_charger::PreCharger,
};
use super::{meter::MeterState, operator_settings::OperatorSettings, supervisor::SupervisoryState};
use chademo_v2::{X100, X101, X102, X108, X109, X200, X201, X208, X209};
use serde::Serialize;
use sysfs_gpio::Pin;

/// Signals on the CHAdeMO connector itself (as defined in CHAdeMO 2.0 / IEC 61851-23).
#[derive(Serialize, Debug)]
pub struct ChademoConnectorGpio {
    pub k_line:          u8, // Charge sequence signal (input -- low = EV requests sequence)
    pub d1_ev_contactor: u8, // D1: +HV EV-side relay (output -- high = commanded closed)
    pub d2_ev_contactor: u8, // D2: -HV EV-side relay (output -- high = commanded closed)
    pub plug_lock:       u8, // Plug-lock solenoid (output -- high = locked)
}

/// EVSE internal I/O -- buttons, safety, and power-path contactors.
#[derive(Serialize, Debug)]
pub struct EvseGpio {
    pub estop:            u8, // Emergency stop (input -- low = active/pressed)
    pub on_off_button:    u8, // Front-panel On/Off (input -- low = pressed)
    pub boost_button:     u8, // Front-panel Boost (input -- low = pressed)
    pub c1_contactor:     u8, // C1 AC contactor (output)
    pub c2_contactor:     u8, // C2 AC contactor (output)
    pub pre_ac:           u8, // PRE AC input contactor (output)
    pub master_contactor: u8, // Master DC contactor (output)
    pub pca_reset:        u8, // PCA9552 LED driver reset (output -- init high)
}

#[derive(Serialize, Debug)]
pub struct GpioSnapshot {
    pub chademo_connector: ChademoConnectorGpio,
    pub evse_io:           EvseGpio,
}

#[derive(Serialize, Debug)]
pub struct ChademoSnapshot {
    pub state:        OperationMode,
    pub soc:          u8,
    pub current_amps: i16,
    // EV -> EVSE (received from vehicle)
    pub x100: X100,
    pub x101: X101,
    pub x102: X102,
    pub x200: X200,
    pub x201: X201,
    // EVSE -> EV (sent to vehicle)
    pub x108: X108,
    pub x109: X109,
    pub x208: X208,
    pub x209: X209,
    pub gpio: GpioSnapshot,
}

#[derive(Serialize, Debug)]
pub struct ChargerSnapshot {
    pub chademo:     ChademoSnapshot,
    pub pre:         PreCharger,
    pub meter:       MeterState,
    pub supervisory: SupervisoryState,
    pub settings:    OperatorSettings,
}

pub async fn snapshot() -> ChargerSnapshot {
    let read_pin = |num: u64| Pin::new(num).get_value().unwrap_or(255);

    let chademo = crate::chademo::state::CHADEMO.lock().await;
    let pins = chademo.pins();
    let gpio = GpioSnapshot {
        chademo_connector: ChademoConnectorGpio {
            k_line:          pins.k.get_value().unwrap_or(255),
            d1_ev_contactor: pins.d1.get_value().unwrap_or(255),
            d2_ev_contactor: pins.d2.get_value().unwrap_or(255),
            plug_lock:       pins.pluglock.get_value().unwrap_or(255),
        },
        evse_io: EvseGpio {
            estop:            read_pin(ESTOPPIN),
            on_off_button:    read_pin(ONOFFPIN),
            boost_button:     read_pin(BOOSTPIN),
            c1_contactor:     pins.c1.get_value().unwrap_or(255),
            c2_contactor:     pins.c2.get_value().unwrap_or(255),
            pre_ac:           pins.pre_ac.get_value().unwrap_or(255),
            master_contactor: read_pin(MASTERCONTACTOR),
            pca_reset:        read_pin(RESETPCAPIN),
        },
    };
    let chademo_snap = ChademoSnapshot {
        state:        *chademo.state(),
        soc:          *chademo.soc(),
        current_amps: *chademo.output_amps(),
        x100: chademo.x100,
        x101: chademo.x101,
        x102: chademo.x102,
        x200: chademo.x200,
        x201: chademo.x201,
        x108: chademo.x108,
        x109: chademo.x109,
        x208: chademo.x208,
        x209: chademo.x209,
        gpio,
    };
    drop(chademo);

    let pre         = *crate::pre_charger::PREDATA.lock().await;
    let meter       = *super::meter::METER.read().await;
    let supervisory = *super::supervisor::SUPERVISORY.read().await;
    let settings    = super::operator_settings::OPERATOR_SETTINGS.read().await.clone();

    ChargerSnapshot { chademo: chademo_snap, pre, meter, supervisory, settings }
}
