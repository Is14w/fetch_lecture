use crate::{
    api,
    auth::{self, CampusSession, LoginAttempt, PendingLogin},
    models::{BookingReply, CaptchaReply, LoginReply},
    ocr::OcrEngine,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct LoginCredentials {
    username: String,
    password: String,
    fingerprint: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    session: Arc<Mutex<Option<CampusSession>>>,
    pending_login: Arc<Mutex<Option<PendingLogin>>>,
    credentials: Arc<Mutex<Option<LoginCredentials>>>,
    reauth_notify: Arc<Notify>,
    reauth_waiting: Arc<AtomicBool>,
    ocr: Arc<Mutex<Option<OcrEngine>>>,
    model_path: Arc<PathBuf>,
    charset_path: Arc<PathBuf>,
    booking_cancel: Arc<Mutex<Option<CancellationToken>>>,
}

impl AppState {
    pub fn new(model_path: PathBuf, charset_path: PathBuf) -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            pending_login: Arc::new(Mutex::new(None)),
            credentials: Arc::new(Mutex::new(None)),
            reauth_notify: Arc::new(Notify::new()),
            reauth_waiting: Arc::new(AtomicBool::new(false)),
            ocr: Arc::new(Mutex::new(None)),
            model_path: Arc::new(model_path),
            charset_path: Arc::new(charset_path),
            booking_cancel: Arc::new(Mutex::new(None)),
        }
    }

    async fn recognize(&self, image: &[u8]) -> Result<String, String> {
        let mut engine = self.ocr.lock().await;
        if engine.is_none() {
            *engine = Some(OcrEngine::new(&self.model_path, &self.charset_path)?);
        }
        engine
            .as_mut()
            .ok_or_else(|| "OCR 引擎尚未初始化".to_string())?
            .recognize(image)
    }

    async fn current_session(&self) -> Result<CampusSession, String> {
        self.session
            .lock()
            .await
            .clone()
            .ok_or_else(|| "请先登录统一身份认证".to_string())
    }
}

fn emit_log(app: &AppHandle, level: &str, message: impl Into<String>) {
    let message: String = message.into();
    let _ = app.emit("booking-log", json!({ "level": level, "message": message }));
}

async fn wait_until_or_cancel(cancel: &CancellationToken, target_ms: i64) -> bool {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    if target_ms <= now_ms {
        return !cancel.is_cancelled();
    }
    let wait = Duration::from_millis(target_ms.saturating_sub(now_ms) as u64);
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(wait) => true,
    }
}

fn numeric(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.parse::<i64>().ok())
}

fn lecture_is_full(lectures: &[Value], lecture_wid: &str) -> Option<bool> {
    let lecture = lectures.iter().find(|row| {
        ["WID", "HD_WID"].iter().any(|key| {
            let value = &row[*key];
            value.as_str() == Some(lecture_wid)
                || value.as_i64().map(|id| id.to_string()).as_deref() == Some(lecture_wid)
        })
    })?;
    let total = numeric(&lecture["HDZRS"])?;
    let booked = numeric(&lecture["YYRS"])?;
    Some(booked >= total)
}

async fn refresh_login_for_booking(
    app: &AppHandle,
    state: &AppState,
    cancel: &CancellationToken,
) -> bool {
    let Some(credentials) = state.credentials.lock().await.clone() else {
        emit_log(app, "error", "无法自动刷新会话：当前没有可用的登录凭据");
        return false;
    };
    emit_log(app, "info", "正在刷新学校登录会话");
    let attempt = tokio::select! {
        _ = cancel.cancelled() => return false,
        result = auth::begin_login(
            credentials.username,
            credentials.password,
            credentials.fingerprint,
        ) => result,
    };
    match attempt {
        Ok(LoginAttempt::Connected(session)) => {
            if cancel.is_cancelled() {
                return false;
            }
            *state.session.lock().await = Some(session);
            emit_log(app, "success", "学校登录会话已刷新");
            true
        }
        Ok(LoginAttempt::NeedsPhoneCode(pending)) => {
            if cancel.is_cancelled() {
                return false;
            }
            *state.pending_login.lock().await = Some(pending);
            state.reauth_waiting.store(true, Ordering::Release);
            let notified = state.reauth_notify.notified();
            let _ = app.emit(
                "reauth-required",
                json!({ "message": "会话刷新需要短信验证，请完成验证后继续预约。" }),
            );
            emit_log(app, "warning", "刷新会话需要短信验证，请在窗口中完成验证");
            let completed = tokio::select! {
                _ = cancel.cancelled() => false,
                _ = notified => true,
            };
            state.reauth_waiting.store(false, Ordering::Release);
            if completed {
                emit_log(app, "success", "短信验证完成，自动预约继续");
                true
            } else {
                *state.pending_login.lock().await = None;
                false
            }
        }
        Err(error) => {
            emit_log(app, "warning", format!("刷新登录会话失败：{error}"));
            false
        }
    }
}
async fn submit_with_code(
    session: &CampusSession,
    lecture_wid: &str,
    captcha_code: &str,
    verify_booking: bool,
) -> Result<BookingReply, String> {
    let result = api::submit_reservation(session, lecture_wid, captcha_code).await?;
    let success = result["success"].as_bool().unwrap_or(false);
    let verified = verify_booking
        && success
        && api::is_booking_confirmed(session, lecture_wid)
            .await
            .unwrap_or(false);
    Ok(BookingReply {
        code: result.get("code").cloned().unwrap_or(Value::Null),
        message: result["msg"]
            .as_str()
            .unwrap_or(if success {
                "预约请求已提交"
            } else {
                "预约未成功"
            })
            .to_string(),
        success,
        verified,
    })
}

async fn booking_attempt(state: &AppState, lecture_wid: &str) -> Result<BookingReply, String> {
    let session = state.current_session().await?;
    let image = api::fetch_captcha(&session).await?;
    let code = state.recognize(&image).await?;
    if code.trim().is_empty() {
        return Err("OCR 未识别出验证码，稍后重新获取".to_string());
    }
    submit_with_code(&session, lecture_wid, code.trim(), false).await
}

async fn run_booking(
    app: AppHandle,
    state: AppState,
    cancel: CancellationToken,
    lecture_wid: String,
    target_ms: i64,
    relogin_before_seconds: f64,
) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();
    let mut should_run = true;

    if target_ms > now_ms {
        if relogin_before_seconds > 0.0 {
            let relogin_at =
                target_ms.saturating_sub((relogin_before_seconds * 1000.0).round() as i64);
            emit_log(
                &app,
                "info",
                format!("将在开放前 {relogin_before_seconds:.1} 秒刷新登录会话"),
            );
            should_run = wait_until_or_cancel(&cancel, relogin_at).await;
            if should_run {
                let _ = refresh_login_for_booking(&app, &state, &cancel).await;
            }
        }
        if should_run {
            emit_log(&app, "info", "等待预约开放时间");
            should_run = wait_until_or_cancel(&cancel, target_ms).await;
        }
    }

    let mut attempt = 0u64;
    let mut last_seat_check = tokio::time::Instant::now();
    let mut check_seats_now = true;
    while should_run && !cancel.is_cancelled() {
        if check_seats_now || last_seat_check.elapsed() >= Duration::from_secs(5) {
            check_seats_now = false;
            last_seat_check = tokio::time::Instant::now();
            if let Ok(session) = state.current_session().await {
                match api::list_lectures(&session).await {
                    Ok(lectures) if lecture_is_full(&lectures, &lecture_wid) == Some(true) => {
                        emit_log(&app, "warning", "当前没有剩余名额，继续等待名额变化");
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
                        }
                    }
                    Err(error) if error.contains("会话已失效") => {
                        if !refresh_login_for_booking(&app, &state, &cancel).await {
                            break;
                        }
                        continue;
                    }
                    _ => {}
                }
            }
        }

        attempt += 1;
        let result = tokio::select! {
            _ = cancel.cancelled() => break,
            result = booking_attempt(&state, &lecture_wid) => result,
        };
        let mut wait = Duration::from_millis(650);
        match result {
            Ok(reply) => {
                emit_log(
                    &app,
                    if reply.success { "success" } else { "warning" },
                    format!("第 {attempt} 次：{}", reply.message),
                );

                let should_check_booking =
                    reply.success || attempt % 5 == 0 || reply.message.contains("已预约");
                let confirmed = reply.verified
                    || (should_check_booking
                        && match state.current_session().await {
                            Ok(session) => api::is_booking_confirmed(&session, &lecture_wid)
                                .await
                                .unwrap_or(false),
                            Err(_) => false,
                        });
                if confirmed {
                    emit_log(&app, "success", "预约已在已预约列表确认");
                    break;
                }
                if reply.success {
                    emit_log(&app, "warning", "服务器返回成功但列表暂未确认，将继续尝试");
                }

                if reply.message.contains("预约已结束") {
                    break;
                }
                if reply.message.contains("名额已满") || reply.message.contains("预约人数已满")
                {
                    wait = Duration::from_secs(2);
                } else if reply.message.contains("频繁") {
                    wait = Duration::from_secs(10);
                }
            }
            Err(error) if error.contains("会话已失效") => {
                emit_log(&app, "warning", "登录会话已失效，尝试自动重新登录");
                if !refresh_login_for_booking(&app, &state, &cancel).await {
                    emit_log(&app, "error", "自动重新登录失败，预约任务已停止");
                    break;
                }
                continue;
            }
            Err(error) => {
                let terminal = error.contains("请先登录")
                    || error.contains("ONNX")
                    || error.contains("验证码模型");
                emit_log(&app, if terminal { "error" } else { "warning" }, error);
                if terminal {
                    break;
                }
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(wait) => {}
        }
    }

    if cancel.is_cancelled() {
        state.reauth_waiting.store(false, Ordering::Release);
        emit_log(&app, "stopped", "自动预约已停止");
    }
    *state.booking_cancel.lock().await = None;
}

#[tauri::command]
pub async fn login(
    state: State<'_, AppState>,
    username: String,
    password: String,
    fingerprint: Option<String>,
) -> Result<LoginReply, String> {
    if username.trim().is_empty() || password.is_empty() {
        return Err("学号和密码不能为空".to_string());
    }
    let attempt =
        auth::begin_login(username.clone(), password.clone(), fingerprint.clone()).await?;
    *state.credentials.lock().await = Some(LoginCredentials {
        username,
        password,
        fingerprint,
    });
    match attempt {
        LoginAttempt::Connected(session) => {
            *state.session.lock().await = Some(session);
            *state.pending_login.lock().await = None;
            Ok(LoginReply {
                requires_phone_code: false,
                message: None,
            })
        }
        LoginAttempt::NeedsPhoneCode(pending) => {
            *state.pending_login.lock().await = Some(pending);
            Ok(LoginReply {
                requires_phone_code: true,
                message: Some("此设备需要短信验证码，请发送验证码后完成登录".to_string()),
            })
        }
    }
}

#[tauri::command]
pub async fn send_phone_code(state: State<'_, AppState>) -> Result<String, String> {
    let pending = state
        .pending_login
        .lock()
        .await
        .clone()
        .ok_or_else(|| "当前没有等待短信验证的登录".to_string())?;
    auth::send_phone_code(&pending).await
}

#[tauri::command]
pub async fn complete_phone_login(
    state: State<'_, AppState>,
    phone_code: String,
) -> Result<(), String> {
    if phone_code.trim().is_empty() {
        return Err("短信验证码不能为空".to_string());
    }
    let pending = state
        .pending_login
        .lock()
        .await
        .clone()
        .ok_or_else(|| "当前没有等待短信验证的登录".to_string())?;
    let session = auth::complete_phone_login(&pending, phone_code.trim()).await?;
    *state.session.lock().await = Some(session);
    *state.pending_login.lock().await = None;
    if state.reauth_waiting.swap(false, Ordering::AcqRel) {
        state.reauth_notify.notify_one();
    }
    Ok(())
}

#[tauri::command]
pub async fn list_lectures(state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let session = state.current_session().await?;
    api::list_lectures(&session).await
}

#[tauri::command]
pub async fn list_my_bookings(state: State<'_, AppState>) -> Result<Vec<Value>, String> {
    let session = state.current_session().await?;
    api::list_my_bookings(&session).await
}

#[tauri::command]
pub async fn get_captcha(state: State<'_, AppState>) -> Result<CaptchaReply, String> {
    let session = state.current_session().await?;
    let image = api::fetch_captcha(&session).await?;
    let (recognized_code, recognition_note) = match state.recognize(&image).await {
        Ok(code) => (code, None),
        Err(error) => (String::new(), Some(error)),
    };
    let mime_type = match image::guess_format(&image)
        .map_err(|error| format!("无法识别验证码图片格式：{error}"))?
    {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        _ => return Err("验证码图片格式暂不支持".to_string()),
    };
    Ok(CaptchaReply {
        image_data_url: format!("data:{mime_type};base64,{}", STANDARD.encode(image)),
        recognized_code,
        recognition_note,
    })
}

#[tauri::command]
pub async fn reserve_lecture(
    state: State<'_, AppState>,
    lecture_wid: String,
    captcha_code: String,
) -> Result<BookingReply, String> {
    if lecture_wid.trim().is_empty() || captcha_code.trim().is_empty() {
        return Err("讲座编号和验证码不能为空".to_string());
    }
    let session = state.current_session().await?;
    submit_with_code(&session, &lecture_wid, captcha_code.trim(), true).await
}

#[tauri::command]
pub async fn start_booking(
    app: AppHandle,
    state: State<'_, AppState>,
    lecture_wid: String,
    start_at_ms: i64,
    delay_seconds: f64,
    relogin_before_seconds: f64,
) -> Result<(), String> {
    if lecture_wid.trim().is_empty() {
        return Err("请先选择一场讲座".to_string());
    }
    state.current_session().await?;
    if !delay_seconds.is_finite() || !(-60.0..=60.0).contains(&delay_seconds) {
        return Err("预约时间微调需在 -60 到 60 秒之间".to_string());
    }
    if !relogin_before_seconds.is_finite() || !(0.0..=300.0).contains(&relogin_before_seconds) {
        return Err("提前登录时间需在 0 到 300 秒之间".to_string());
    }

    let target_ms = start_at_ms.saturating_add((delay_seconds * 1000.0).round() as i64);
    let cancel = CancellationToken::new();
    {
        let mut current = state.booking_cancel.lock().await;
        if current.is_some() {
            return Err("已有一个自动预约任务正在运行或正在停止".to_string());
        }
        *current = Some(cancel.clone());
    }

    let app = app.clone();
    let state = state.inner().clone();
    tokio::spawn(async move {
        run_booking(
            app,
            state,
            cancel,
            lecture_wid,
            target_ms,
            relogin_before_seconds,
        )
        .await;
    });
    Ok(())
}

#[tauri::command]
pub async fn stop_booking(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(token) = state.booking_cancel.lock().await.as_ref() {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(token) = state.booking_cancel.lock().await.as_ref() {
        token.cancel();
    }
    *state.session.lock().await = None;
    *state.pending_login.lock().await = None;
    *state.credentials.lock().await = None;
    state.reauth_waiting.store(false, Ordering::Release);
    Ok(())
}
