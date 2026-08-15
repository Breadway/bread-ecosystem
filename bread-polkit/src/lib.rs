//! Non-GTK PolicyKit helper logic for `bread-polkit`.
//!
//! The binary (`bread-polkit`) registers as a session authentication
//! agent and shows a themed password prompt. This library is the
//! transport / identity / session parsing that can be unit-tested
//! without a display.

pub mod helper;
pub mod identity;
pub mod session;
