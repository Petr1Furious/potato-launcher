use std::sync::Arc;

use gpui::{Context, EventEmitter};
use launcher_bridge::AccountView;

#[derive(Clone, Default)]
pub struct AccountEntries {
    pub accounts: Vec<AccountView>,
}

#[derive(Clone)]
pub struct AccountsUpdatedEvent;

impl EventEmitter<AccountsUpdatedEvent> for AccountEntries {}

impl AccountEntries {
    pub fn replace(&mut self, accounts: Arc<[AccountView]>, cx: &mut Context<Self>) {
        self.accounts = accounts.iter().cloned().collect();
        cx.emit(AccountsUpdatedEvent);
        cx.notify();
    }
}
