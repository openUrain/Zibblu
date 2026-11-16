#[cfg(feature = "client")]
pub mod assistant;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::*;

#[cfg(all(windows, feature = "client"))]
pub mod dpapi;

#[cfg(feature = "private-chat")]
pub mod private_chat;

mod protocol;
pub use protocol::*;
