//! Shared Crumb field contracts for ranked compiler work and completed read witnesses.

pub const KEY_TYPE: &str = "tuple<string,string,string,string,string,string>";
pub const REQUEST_TYPE: &str =
    "tuple<tuple<string,string,string,string,string,string>,i64,string,list<string>,list<i64>>";
