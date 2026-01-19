mod base;
mod elyby;
mod microsoft;
mod offline;
mod telegram;

pub(crate) use base::AuthProvider;
pub use base::{AuthProviderConfig, AuthProviderType};

pub use elyby::ElyByAuthProvider;
pub use microsoft::MicrosoftAuthProvider;
pub use offline::OfflineAuthProvider;
pub use telegram::TGAuthProvider;
