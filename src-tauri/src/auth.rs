use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::rngs::OsRng;
use reqwest::{Client, Response};
use rsa::{pkcs8::DecodePublicKey, Pkcs1v15Encrypt, RsaPublicKey};
use serde_json::{json, Value};
use std::time::Duration;

const AUTH_ROOT: &str = "https://auth.seu.edu.cn/auth/casback";
pub const SERVICE_URL: &str = "http://ehall.seu.edu.cn/gsapp/sys/jzxxtjapp/*default/index.do";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0 Safari/537.36";

#[derive(Clone)]
pub struct CampusSession {
    pub client: Client,
    pub ehall_referer: String,
}

#[derive(Clone)]
pub struct PendingLogin {
    pub client: Client,
    pub username: String,
    pub password: String,
    pub fingerprint: Option<String>,
}

pub enum LoginAttempt {
    Connected(CampusSession),
    NeedsPhoneCode(PendingLogin),
}

fn is_seu_vpn_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "vpn.seu.edu.cn" || host.ends_with(".vpn.seu.edu.cn")
}

pub fn reject_vpn_redirect(response: &Response, operation: &str) -> Result<(), String> {
    if !response.status().is_redirection() {
        return Ok(());
    }
    let Some(location) = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Ok(());
    };
    let Ok(target) = response.url().join(location) else {
        return Ok(());
    };
    let Some(host) = target.host_str() else {
        return Ok(());
    };
    let host = host.to_ascii_lowercase();
    if is_seu_vpn_host(&host) {
        Err(format!(
            "{operation}被学校服务重定向到 VPN 门户（{host}）；应用不会自动连接 VPN。请检查校园网认证或登录会话。"
        ))
    } else {
        Ok(())
    }
}

pub fn new_client() -> Result<Client, String> {
    Client::builder()
        .cookie_store(true)
        .timeout(Duration::from_secs(30))
        .no_proxy()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let host = attempt
                .url()
                .host_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if is_seu_vpn_host(&host) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| format!("创建学校认证会话失败：{error}"))
}

async fn get_public_key(client: &Client) -> Result<String, String> {
    let response = client
        .post(format!("{AUTH_ROOT}/getChiperKey"))
        .json(&json!({}))
        .send()
        .await
        .map_err(|error| format!("获取登录加密公钥失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("获取登录加密公钥失败：{error}"))?;
    let payload: Value = response
        .json()
        .await
        .map_err(|error| format!("认证服务返回的公钥数据无效：{error}"))?;
    payload["publicKey"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "认证服务没有返回 RSA 公钥".to_string())
}

fn encrypt(message: &str, encoded_key: &str) -> Result<String, String> {
    let mut normalized = encoded_key.replace('-', "+").replace('_', "/");
    while normalized.len() % 4 != 0 {
        normalized.push('=');
    }
    let der = STANDARD
        .decode(normalized)
        .map_err(|error| format!("解析认证公钥失败：{error}"))?;
    let public_key = RsaPublicKey::from_public_key_der(&der)
        .map_err(|error| format!("读取认证公钥失败：{error}"))?;
    let cipher_text = public_key
        .encrypt(&mut OsRng, Pkcs1v15Encrypt, message.as_bytes())
        .map_err(|error| format!("加密登录信息失败：{error}"))?;
    Ok(STANDARD.encode(cipher_text))
}

async fn submit_login(
    client: &Client,
    username: &str,
    password: &str,
    fingerprint: Option<&str>,
    phone_code: Option<&str>,
    public_key: &str,
) -> Result<Value, String> {
    let mut payload = json!({
        "captcha": "",
        "loginType": "account",
        "mobilePhoneNum": "",
        "mobileVerifyCode": "",
        "password": encrypt(password, public_key)?,
        "rememberMe": false,
        "service": SERVICE_URL,
        "username": username,
        "wxBinded": false
    });
    if let Some(fingerprint) = fingerprint.filter(|value| !value.is_empty()) {
        payload["fingerPrint"] = json!(fingerprint);
    }
    if let Some(code) = phone_code {
        payload["mobileVerifyCode"] = json!(encrypt(code, public_key)?);
    }

    let response = client
        .post(format!("{AUTH_ROOT}/casLogin"))
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("统一身份认证请求失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("统一身份认证失败：{error}"))?;
    response
        .json()
        .await
        .map_err(|error| format!("认证服务返回的数据无效：{error}"))
}

async fn connect_ehall(client: Client, result: &Value) -> Result<CampusSession, String> {
    let raw = result["redirectUrl"]
        .as_str()
        .ok_or_else(|| "登录成功，但认证服务没有提供讲座系统跳转地址".to_string())?;
    let raw = raw.replace('+', "%2B");
    let redirect_url = urlencoding::decode(&raw)
        .map_err(|error| format!("解析讲座系统跳转地址失败：{error}"))?
        .into_owned();
    let redirect_target = reqwest::Url::parse(&redirect_url)
        .map_err(|error| format!("解析讲座系统跳转地址失败：{error}"))?;
    if redirect_target
        .host_str()
        .map(is_seu_vpn_host)
        .unwrap_or(false)
    {
        return Err("统一身份认证返回了 VPN 门户地址；应用不会自动连接 VPN。".to_string());
    }
    let response = client
        .get(redirect_target)
        .send()
        .await
        .map_err(|error| format!("连接讲座系统失败：{error}"))?;
    reject_vpn_redirect(&response, "连接讲座系统")?;
    let response = response
        .error_for_status()
        .map_err(|error| format!("连接讲座系统失败：{error}"))?;
    Ok(CampusSession {
        client,
        ehall_referer: response.url().to_string(),
    })
}

pub async fn begin_login(
    username: String,
    password: String,
    fingerprint: Option<String>,
) -> Result<LoginAttempt, String> {
    let client = new_client()?;
    let public_key = get_public_key(&client).await?;
    let result = submit_login(
        &client,
        &username,
        &password,
        fingerprint.as_deref(),
        None,
        &public_key,
    )
    .await?;
    if result["success"].as_bool() != Some(true) {
        let message = result["info"].as_str().unwrap_or("登录失败").to_string();
        if message.contains("非可信设备") {
            return Ok(LoginAttempt::NeedsPhoneCode(PendingLogin {
                client,
                username,
                password,
                fingerprint,
            }));
        }
        return Err(message);
    }
    Ok(LoginAttempt::Connected(
        connect_ehall(client, &result).await?,
    ))
}

pub async fn send_phone_code(pending: &PendingLogin) -> Result<String, String> {
    let response = pending
        .client
        .post(format!("{AUTH_ROOT}/sendStage2Code"))
        .json(&json!({ "userId": pending.username }))
        .send()
        .await
        .map_err(|error| format!("发送手机验证码失败：{error}"))?
        .error_for_status()
        .map_err(|error| format!("发送手机验证码失败：{error}"))?;
    let result: Value = response
        .json()
        .await
        .map_err(|error| format!("验证码服务返回的数据无效：{error}"))?;
    if result["success"].as_bool() == Some(true) {
        Ok("短信验证码已发送".to_string())
    } else {
        Err(result["info"]
            .as_str()
            .unwrap_or("发送验证码失败")
            .to_string())
    }
}

pub async fn complete_phone_login(
    pending: &PendingLogin,
    phone_code: &str,
) -> Result<CampusSession, String> {
    let client = new_client()?;
    let public_key = get_public_key(&client).await?;
    let result = submit_login(
        &client,
        &pending.username,
        &pending.password,
        pending.fingerprint.as_deref(),
        Some(phone_code),
        &public_key,
    )
    .await?;
    if result["success"].as_bool() != Some(true) {
        return Err(result["info"]
            .as_str()
            .unwrap_or("短信验证登录失败")
            .to_string());
    }
    connect_ehall(client, &result).await
}
