use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type Username = String;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct UserInfo {
    pub uuid: Uuid,
    pub username: Username,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct AccountData {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub user_info: UserInfo,
}
