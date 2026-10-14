mod api;
mod auth;
mod commands;
mod models;
mod ocr;

use commands::AppState;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ONNX_RUNTIME_RESOURCE: &str = "runtime/onnxruntime.dll";
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const ONNX_RUNTIME_RESOURCE: &str = "runtime/libonnxruntime.dylib";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const ONNX_RUNTIME_RESOURCE: &str = "runtime/libonnxruntime.so";
#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
compile_error!("ONNX Runtime is bundled for Windows x64, macOS arm64, and Linux x64 only");

use tauri::{path::BaseDirectory, Manager};

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let runtime_path = app
                .path()
                .resolve(ONNX_RUNTIME_RESOURCE, BaseDirectory::Resource)?;
            ort::init_from(runtime_path)
                .map_err(|error| std::io::Error::other(error.to_string()))?
                .commit();
            let model_path = app
                .path()
                .resolve("ocr/common_old.onnx", BaseDirectory::Resource)?;
            let charset_path = app
                .path()
                .resolve("ocr/charset_old.json", BaseDirectory::Resource)?;
            app.manage(AppState::new(model_path, charset_path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::login,
            commands::send_phone_code,
            commands::complete_phone_login,
            commands::list_lectures,
            commands::list_my_bookings,
            commands::get_captcha,
            commands::reserve_lecture,
            commands::start_booking,
            commands::stop_booking,
            commands::logout
        ])
        .run(tauri::generate_context!())
        .expect("启动 Fetch Lecture 桌面端失败");
}
