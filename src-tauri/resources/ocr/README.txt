ONNX OCR resource attribution
=============================

The OCR model and charset data in this directory are from ddddocr:
https://github.com/sml2h3/ddddocr

Model: common_old.onnx (the default OCR model used by ddddocr)
License: MIT; see LICENSE.ddddocr.
The Rust preprocessing and CTC decoding follow the public behavior documented
by the upstream project. The model is run locally through ONNX Runtime.
ONNX Runtime is provided by Microsoft under the MIT License; see LICENSE.onnxruntime.

Bundled ONNX Runtime: 1.27.1 (CPU).
Supported targets: Windows x64, macOS arm64, and Linux x64.
Runtime binaries are provided by Microsoft under the MIT License; see the
license and ThirdPartyNotices files shipped with the application.
