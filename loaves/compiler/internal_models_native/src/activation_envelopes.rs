//! Canonical Crumb envelopes shared by the graph publisher and the selected compiler worker.

pub const DEMAND_SCHEMA: &str = "incan.engine.demand-page/2";
pub const PREPARED_SCHEMA: &str = "incan.engine.prepared-inputs/2";
pub const WORK_SCHEMA: &str = "incan.engine.ready-work/2";
pub const DEMAND_FAMILY: &str = "incan.engine.demand-page";
pub const PREPARED_FAMILY: &str = "incan.engine.prepared-inputs";
pub const WORK_FAMILY: &str = "incan.engine.ready-work";
