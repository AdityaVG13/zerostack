//! GraphZero MCP query tools: symbol, callers, deps, outline, context, hot,
//! changes, word, search — shared JSON for MCP and CLI.

mod budget;
mod delta;
pub(crate) mod frecency;
mod helpers;
mod page;
mod rg_ladder;
mod skeleton;
mod surfaces;
mod types;
mod worktree_keyword;

pub use page::{load_page, page_document, payload_if_kind, remember_session_cursor, spill_page};
pub use types::*;
pub use worktree_keyword::{keyword_surface, worktree_keyword_response};

pub struct QuerySurfaceRouter;
