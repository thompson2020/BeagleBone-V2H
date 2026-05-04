use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Debug, Clone, Copy, PartialEq)]
pub enum OperationMode {
    /// Bidirectional load matching
    V2h,
    /// Charge using settings from OperatorSettings
    Charge,
    /// Safe idle mode - plug unlocked
    #[default]
    Idle,
    /// Bad state
    Uninitalised,
    /// Shutdown all peripherals and safetly unlock plug
    Quit,
    /// Forced discharge mode — uses v2h_max_amps / v2h_soc_min from OperatorSettings
    Discharge,
}

impl OperationMode {
    pub fn is_discharge(&self) -> bool {
        matches!(self, Self::Discharge)
    }
    pub fn boost(&mut self) {
        use OperationMode::*;
        *self = match self {
            Quit => Quit,
            Uninitalised => Idle,
            Discharge | V2h | Idle => Charge,
            Charge => V2h,
        }
    }
    pub fn onoff(&mut self) {
        use OperationMode::*;
        *self = match self {
            Quit => Quit,
            Uninitalised => Idle,
            Discharge | V2h | Charge => Idle,
            Idle => V2h,
        }
    }
    pub fn idle(&mut self) {
        use OperationMode::*;
        *self = Idle;
    }
    pub fn is_idle(&self) -> bool {
        use OperationMode::*;
        matches!(self, Idle)
    }
    pub fn is_quit(&self) -> bool {
        use OperationMode::*;
        matches!(self, Quit)
    }
    pub fn is_uninitalised(&self) -> bool {
        use OperationMode::*;
        matches!(self, Uninitalised)
    }
    pub fn is_inactive(&self) -> bool {
        self.is_idle() || self.is_quit() || self.is_uninitalised()
    }

    pub fn is_charge(&self) -> bool {
        use OperationMode::*;
        matches!(self, Charge)
    }
    pub fn is_v2h(&self) -> bool {
        use OperationMode::*;
        matches!(self, V2h)
    }
}
