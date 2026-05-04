// GLOBAL VARIABLES OVERVIEW

// APP_CONFIG
// Defined in:     data_io/config.rs                                      Used in:  main.rs
//                                                                                  data_io/meter.rs
// Type:           Arc<AppConfig>
// Purpose:        Global configuration loaded from config.toml

// CHADEMO
// Defined in:     chademo/state.rs (duplicate in main.rs)                Used in:   api/mod.rs
// Type:           Arc<Mutex<Chademo>>
// Purpose:        Main charger hardware state

// METER
// Defined in:     data_io/meter.rs                                       Used in:  chademo/ev_connect.rs
// Type:           Arc<RwLock<Option<f32>>>                                         data_io/db.rs
// Purpose:        Current grid power reading (in watts) from meter                 data_io/meter.rs

// PREDATA
// Defined in:     pre_charger/mod.rs                                     Used in:  pre_charger/pre_thread.rs
// Type:           Arc<Mutex<PreCharger>>                                           chademo/ev_connect.rs
// Purpose:        Pre-charger state and data                                       pre_charger/can.rs



///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
/////////////////////////    MORE DETAILS ON GLOBALS    ///////////////////////////////////////////////////////////////////////////////////////////////////
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

// APP_CONFIG
// Defined in:     data_io/config.rs                                      Used in:  main.rs
//                                                                                  data_io/meter.rs
// Type:           Arc<AppConfig>
// Purpose:        Global configuration loaded from config.toml
// structure                        
// ---------    
//  <<TO DO>>


// CHADEMO
// Defined in:     chademo/state.rs (duplicate in main.rs)                Used in:   api/mod.rs
// Type:           Arc<Mutex<Chademo>>
// Purpose:        Main charger hardware state
// structure                        
// ---------    
//  <<TO DO>>




// METER
// Defined in:     data_io/meter.rs                                       Used in:  data_io/meter.rs(w)
// Type:           Arc<RwLock<Option<f32>>>                                         chademo/ev_connect.rs
// Purpose:        Current grid power reading (in watts) from meter                 data_io/db.rs
// structure                        
// ---------                        
//  Option(f32)                     data_io/meter.rs            Updates the value from modbus or MQTT (SET TO NONE IF NO METER READING AVAILABLE - OTHERWISE EXACTLY THE SAME AS CHADEMO_DATA.meter_kw ABOVE - ) 
//                                  data_io/db.rs               Overwrite the meter_kw field in the db row with this value (converted to kW) when adding a new row to the db
//                                  chademo/ev_connect.rs       Used to calculate setpoint amps.



// PREDATA
// Defined in:     pre_charger/mod.rs                                     Used in:  pre_charger/pre_thread.rs
// Type:           Arc<Mutex<PreCharger>>                                           chademo/ev_connect.rs
// Purpose:        Pre-charger state and data                                       pre_charger/can.rs
// structure                        
// ---------    
//  <<TO DO>>




















// Remove - doesnt need to be a global
// LAST_METER_UPDATE
// Defined in:     data_io/meter.rs
// Type:           Arc<Mutex<Instant>>
// Purpose:        Timestamp of the last successful meter update (used for staleness checking)




