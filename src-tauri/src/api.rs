use crate::auth::CampusSession;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

const ROOT: &str = "https://ehall.seu.edu.cn/gsapp/sys/jzxxtjapp/hdyy";
const DEFAULT_REFERER: &str = "https://ehall.seu.edu.cn/gsapp/sys/jzxxtjapp/*default/index.do";

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn parse_json(text: &str) -> Option<Value> {
    let start = text.find(|character| character == '{' || character == '[')?;
    serde_json::from_str(&text[start..]).ok()
}

fn referer(session: &CampusSession) -> &str {
    if session.ehall_referer.is_empty() {
        DEFAULT_REFERER
    } else {
        &session.ehall_referer
    }
}

pub async fn list_lectures(session: &CampusSession) -> Result<Vec<Value>, String> {
    let response = session
        .client
        .post(format!("{ROOT}/queryActivityList.do?_={}", timestamp_ms()))
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .form(&[("pageIndex", "1"), ("pageSize", "100")])
        .send()
        .await
        .map_err(|error| format!("获取讲座列表失败：{error}"))?;
    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取讲座列表失败：{error}"))?;
    let data = parse_json(&body).ok_or_else(|| {
        if final_url.contains("vpn.seu.edu.cn") {
            "当前不在校园网，请连接 EasyConnect VPN 后重试".to_string()
        } else if final_url.to_ascii_lowercase().contains("login")
            || final_url.to_ascii_lowercase().contains("portal")
        {
            "登录会话已失效，请重新登录".to_string()
        } else {
            "讲座系统返回了无法识别的数据".to_string()
        }
    })?;
    data["datas"]
        .as_array()
        .cloned()
        .ok_or_else(|| "讲座列表响应缺少 datas 字段".to_string())
}

pub async fn list_my_bookings(session: &CampusSession) -> Result<Vec<Value>, String> {
    let response = session
        .client
        .post(format!(
            "{ROOT}/queryMyActivityList.do?_={}",
            timestamp_ms()
        ))
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .header("Referer", referer(session))
        .form(&[
            ("pageIndex", "1"),
            ("pageSize", "50"),
            ("sortField", ""),
            ("sortOrder", ""),
        ])
        .send()
        .await
        .map_err(|error| format!("获取已预约列表失败：{error}"))?;
    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取已预约列表失败：{error}"))?;
    let data = parse_json(&body).ok_or_else(|| {
        if final_url.contains("vpn.seu.edu.cn") {
            "当前不在校园网，请连接 EasyConnect VPN 后重试".to_string()
        } else {
            "已预约列表响应无法识别，请刷新或重新登录".to_string()
        }
    })?;
    Ok(data["datas"].as_array().cloned().unwrap_or_default())
}

pub async fn fetch_captcha(session: &CampusSession) -> Result<Vec<u8>, String> {
    let response = session
        .client
        .post(format!("{ROOT}/vcode.do?_={}", timestamp_ms()))
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header("Referer", referer(session))
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await
        .map_err(|error| format!("获取预约验证码失败：{error}"))?;
    let final_url = response.url().to_string();
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取预约验证码失败：{error}"))?;
    let data = parse_json(&body).ok_or_else(|| {
        if final_url.contains("vpn.seu.edu.cn") {
            "当前不在校园网，请连接 EasyConnect VPN 后重试".to_string()
        } else if final_url.to_ascii_lowercase().contains("login")
            || final_url.to_ascii_lowercase().contains("portal")
        {
            "登录会话已失效，请重新登录".to_string()
        } else {
            "验证码服务返回了无效数据".to_string()
        }
    })?;
    let encoded = data["result"]
        .as_str()
        .ok_or_else(|| "验证码响应中缺少图片".to_string())?;
    let raw_data = encoded
        .split_once(',')
        .map(|(_, payload)| payload)
        .unwrap_or(encoded);
    STANDARD
        .decode(raw_data)
        .map_err(|error| format!("验证码图片解码失败：{error}"))
}

pub async fn submit_reservation(
    session: &CampusSession,
    lecture_wid: &str,
    captcha_code: &str,
) -> Result<Value, String> {
    let payload = json!({ "HD_WID": lecture_wid, "vcode": captcha_code });
    let param_json =
        serde_json::to_string(&payload).map_err(|error| format!("构造预约数据失败：{error}"))?;
    let response = session
        .client
        .post(format!("{ROOT}/yySave.do"))
        .header("Accept", "application/json, text/javascript, */*; q=0.01")
        .header(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=UTF-8",
        )
        .header("Referer", referer(session))
        .header("X-Requested-With", "XMLHttpRequest")
        .form(&[("paramJson", param_json)])
        .send()
        .await
        .map_err(|error| format!("提交预约请求失败：{error}"))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("读取预约响应失败：{error}"))?;
    parse_json(&body).ok_or_else(|| "预约系统返回了无法识别的数据".to_string())
}

pub async fn is_booking_confirmed(
    session: &CampusSession,
    lecture_wid: &str,
) -> Result<bool, String> {
    for page in 1..=5 {
        let response = session
            .client
            .post(format!(
                "{ROOT}/queryMyActivityList.do?_={}",
                timestamp_ms()
            ))
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=UTF-8",
            )
            .header("Referer", referer(session))
            .form(&[
                ("pageIndex", page.to_string()),
                ("pageSize", "10".to_string()),
                ("sortField", String::new()),
                ("sortOrder", String::new()),
            ])
            .send()
            .await
            .map_err(|error| format!("核验预约状态失败：{error}"))?;
        let final_url = response.url().to_string();
        let body = response
            .text()
            .await
            .map_err(|error| format!("读取预约核验结果失败：{error}"))?;
        let data = parse_json(&body).ok_or_else(|| {
            if final_url.contains("vpn.seu.edu.cn") {
                "当前不在校园网，请连接 EasyConnect VPN 后重试".to_string()
            } else if final_url.to_ascii_lowercase().contains("login")
                || final_url.to_ascii_lowercase().contains("portal")
            {
                "登录会话已失效，请重新登录".to_string()
            } else {
                "预约核验响应无法识别".to_string()
            }
        })?;
        let bookings = data["datas"]
            .as_array()
            .ok_or_else(|| "预约核验响应缺少 datas 字段".to_string())?;
        if bookings.iter().any(|booking| {
            booking["HD_WID"].as_str() == Some(lecture_wid)
                || booking["WID"].as_str() == Some(lecture_wid)
        }) {
            return Ok(true);
        }
        if bookings.len() < 10 {
            break;
        }
    }
    Ok(false)
}
