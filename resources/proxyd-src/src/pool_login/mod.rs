//! What each pool's tool signs in with, its Login, read and written as a
//! document: the tokens, expiry and identity it holds, which of two logins
//! is newer, and how a renewal is applied to it. Shared by the app and the
//! remote daemon; nothing here reads or writes where a Login is kept, nor
//! contacts a server.
pub(crate) mod antigravity;
pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod grok;
