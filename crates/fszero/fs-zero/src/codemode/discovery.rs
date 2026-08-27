//! Progressive API discovery — generated from native `fs.*` catalog.

use super::api::{self, is_known_target};
use crate::core::{FSZeroSession, OpCode, visible_ack};

pub const SEARCH_REF: &str = "codemode/search";
pub const DESCRIBE_REF: &str = "codemode/describe";

/// Rank `fs.*` methods and recipes; payload at [`SEARCH_REF`]; 1-token ack.
pub fn discovery_search(session: &mut FSZeroSession, query: &str) -> String {
    session.record_internal_op();
    let body = api::search_all(query);
    session.recovery.put_key(SEARCH_REF, body.as_bytes());
    visible_ack(OpCode::Search, Some(session.op_count))
}

pub fn describe_signature(name: &str) -> String {
    api::describe(name)
}

/// Store signature at [`DESCRIBE_REF`]; 1-token ack (`X{n}` on hit, `X0` on unknown).
pub fn discovery_describe(session: &mut FSZeroSession, target: &str) -> String {
    session.record_internal_op();
    let target = target.trim();
    if !is_known_target(target) {
        let doc = describe_signature(target);
        session
            .recovery
            .put_key(super::runtime::ERROR_REF, doc.as_bytes());
        return "X0".to_string();
    }
    let doc = describe_signature(target);
    session.recovery.put_key(DESCRIBE_REF, doc.as_bytes());
    visible_ack(OpCode::Expand, Some(session.op_count))
}
