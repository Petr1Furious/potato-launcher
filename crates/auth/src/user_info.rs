use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct UserInfo {
    pub uuid: String,
    pub username: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct AccountData {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub user_info: UserInfo,
}
