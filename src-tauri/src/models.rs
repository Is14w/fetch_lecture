use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginReply {
    pub requires_phone_code: bool,
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptchaReply {
    pub image_data_url: String,
    pub recognized_code: String,
    pub recognition_note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookingReply {
    pub code: Value,
    pub message: String,
    pub success: bool,
    pub verified: bool,
}
