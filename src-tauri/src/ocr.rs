use image::GenericImageView;
use ndarray::{Array4, IxDyn};
use ort::{session::Session, value::TensorRef};
use std::path::Path;

pub struct OcrEngine {
    session: Session,
    charset: Vec<String>,
}

impl OcrEngine {
    pub fn new(model_path: &Path, charset_path: &Path) -> Result<Self, String> {
        let charset: Vec<String> = serde_json::from_slice(
            &std::fs::read(charset_path)
                .map_err(|error| format!("读取 OCR 字符集失败：{error}"))?,
        )
        .map_err(|error| format!("解析 OCR 字符集失败：{error}"))?;
        let session = Session::builder()
            .map_err(|error| format!("创建 ONNX Runtime 会话失败：{error}"))?
            .commit_from_file(model_path)
            .map_err(|error| format!("加载验证码 ONNX 模型失败：{error}"))?;

        if charset.is_empty() || session.inputs().is_empty() || session.outputs().is_empty() {
            return Err("验证码模型缺少输入、输出或字符集".to_string());
        }
        Ok(Self { session, charset })
    }

    pub fn recognize(&mut self, image_bytes: &[u8]) -> Result<String, String> {
        let decoded = image::load_from_memory(image_bytes)
            .map_err(|error| format!("无法读取验证码图片：{error}"))?;
        let (width, height) = decoded.dimensions();
        if width == 0 || height == 0 {
            return Err("验证码图片尺寸无效".to_string());
        }

        let resized_width = ((u64::from(width) * 64) / u64::from(height)).max(1) as u32;
        let resized = decoded
            .resize_exact(resized_width, 64, image::imageops::FilterType::Lanczos3)
            .to_luma8();
        let pixels = resized
            .as_raw()
            .iter()
            .map(|pixel| *pixel as f32 / 255.0)
            .collect::<Vec<_>>();
        let input = Array4::from_shape_vec((1, 1, 64, resized_width as usize), pixels)
            .map_err(|error| format!("准备验证码图片失败：{error}"))?;

        let input_name = self.session.inputs()[0].name().to_string();
        let input_tensor = TensorRef::from_array_view(input.view())
            .map_err(|error| format!("创建 OCR 输入张量失败：{error}"))?;
        let outputs = self
            .session
            .run(ort::inputs![input_name.as_str() => input_tensor])
            .map_err(|error| format!("验证码 ONNX 推理失败：{error}"))?;
        let output = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|error| format!("读取 OCR 输出失败：{error}"))?;

        let shape = output.shape();
        let (steps, classes, layout) = match shape.len() {
            2 => (shape[0], shape[1], 0),
            3 if shape[1] == 1 => (shape[0], shape[2], 1),
            3 if shape[0] == 1 => (shape[1], shape[2], 2),
            _ => return Err(format!("不支持的 OCR 输出形状：{shape:?}")),
        };

        let mut decoded = String::new();
        let mut previous_index = None;
        for step in 0..steps {
            let mut best_index = 0usize;
            let mut best_score = f32::NEG_INFINITY;
            for class in 0..classes {
                let score = match layout {
                    0 => output[IxDyn(&[step, class])],
                    1 => output[IxDyn(&[step, 0, class])],
                    _ => output[IxDyn(&[0, step, class])],
                };
                if score > best_score {
                    best_score = score;
                    best_index = class;
                }
            }
            if best_index != 0 && previous_index != Some(best_index) {
                if let Some(character) = self.charset.get(best_index) {
                    decoded.push_str(character);
                }
            }
            previous_index = Some(best_index);
        }
        Ok(decoded)
    }
}
