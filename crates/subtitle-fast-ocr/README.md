# subtitle-fast-ocr

`subtitle-fast-ocr` defines the abstraction that turns luma-plane crops into recognised text. It supplies shared data
structures plus optional engines tailored for macOS and cross-platform ONNX Runtime builds.

## OCR flow at a glance

1. **Prepare the plane** – callers turn a `VideoFrame` into a compact `LumaPlane` buffer.
2. **Describe regions** – rectangular areas are collected as OCR regions, typically taken from the subtitle detector.
3. **Issue a request** – the `OcrEngine` trait receives the plane and regions, performs recognition, and returns text
   fragments with optional confidence values.

The trait also offers a warm-up hook so engines can preload models or allocate resources before the first recognition call.

## Engines

- `VisionOcrEngine` (macOS, behind `engine-vision`) uses Apple Vision.
- `OrtOcrEngine` (cross-platform, behind `engine-ort`) runs the PP-OCRv5 recognition model via ONNX Runtime.
- `NoopOcrEngine` returns empty results and is handy for pipeline or benchmarking tests.
- Additional engines can be integrated by implementing `OcrEngine` and wiring it into the caller's configuration.

## Feature flags

| Feature | Description |
| ------- | ----------- |
| `engine-vision` | Enables the Apple Vision OCR backend (macOS only). |
| `engine-ort` | Enables ONNX Runtime + PP-OCRv5 recognition (requires a local ORT build). |

With neither feature enabled the crate only exposes `NoopOcrEngine`, which is useful for pipeline testing without OCR.

## ORT backend notes

- The default model path is `models/ch_PP-OCRv5_rec_infer.onnx` and the default dictionary path is
  `models/ch_PP-OCRv5_rec_infer.txt` (extracted from model metadata).
- Build a static ONNX Runtime and point `ORT_LIB_LOCATION` at the resulting `MinSizeRel` output directory. The workspace
  `.cargo/config.toml` already sets a default path (`target/onnxruntime/build/MinSizeRel`).
- To reduce binary size, generate `models/ch_PP-OCRv5_rec_infer.config` with
  `target/onnxruntime/onnxruntime-1.22.0/tools/python/create_reduced_build_config.py` and rebuild ORT with
  `scripts/build_onnxruntime.sh --ops-config models/ch_PP-OCRv5_rec_infer.config`.
