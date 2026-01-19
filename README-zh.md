# subtitle-fast (中文)

subtitle-fast 是一个 Rust 工作区，用异步流水线把 H.264 视频转换成字幕文件。深度细节参见各 crate 文档：[`subtitle-fast`](crates/subtitle-fast/README.md)、[`subtitle-fast-decoder`](crates/subtitle-fast-decoder/README.md)、[`subtitle-fast-validator`](crates/subtitle-fast-validator/README.md)、[`subtitle-fast-comparator`](crates/subtitle-fast-comparator/README.md)、[`subtitle-fast-ocr`](crates/subtitle-fast-ocr/README.md)。

## 快速开始

- 前置依赖：Rust 稳定版；对应平台的原生组件（FFmpeg 库用于 `backend-ffmpeg`，macOS 自带 VideoToolbox，Windows 需 D3D11/DXVA 驱动，Media Foundation 作为回退，Apple Vision 框架用于 `ocr-vision`）。
- 直接运行（显式开启后端与 OCR）：

```bash
cargo run --release --features backend-ffmpeg,ocr-ort \
  -- --output subtitles.srt path/to/video.mp4
```

- macOS 示例（VideoToolbox + Vision）：

```bash
cargo run --release --features backend-videotoolbox,ocr-vision \
  -- --output subtitles.srt path/to/video.mp4
```

## 后端与特性

- 解码：`backend-ffmpeg`（通用）、`backend-videotoolbox`（macOS 硬解）、`backend-dxva`（Windows D3D11/DXVA 硬解）、`backend-mft`（Windows 回退）、`backend-all`（全部后端）、`mock`（始终可用，`--backend mock`）。
- OCR：`ocr-vision` 启用 Apple Vision（macOS）；`ocr-ort` 启用 ONNX Runtime + PP-OCRv5（全平台）；`ocr-all` 同时启用两者；未启用时可用 noop 引擎做流水线/性能测试。
- 检测：`detector-vision`（macOS）。非 macOS 时关闭该特性。

默认特性为空（`default = []`），未开启解码或 OCR 特性时将使用 mock/Noop 以便测试。CLI 会按优先级选择首个已编译的解码后端（CI 先 mock；macOS 先 VideoToolbox 后 FFmpeg；Windows 先 DXVA 再 MFT 再 FFmpeg；其他平台 FFmpeg），失败则自动回退，并在下游变慢时保持背压。

## 配置

优先级：CLI 参数 > `--config <path>` > `./config.toml` > 平台配置目录（如 `~/.config/subtitle-fast/config.toml`）。可从 `config.toml.example` 拷贝：

```toml
[detection]
samples_per_second = 7
target = 230
delta = 12
# comparator = "bitset-cover"
# roi = { x = 0.0, y = 0.75, width = 1.0, height = 0.25 } # 0-1 归一化；留空或零尺寸即全屏

[decoder]
# backend = "dxva"
# channel_capacity = 32

[ocr]
# backend = "auto" # auto | vision | ort | noop
```

常用覆盖：`--detector-target`、`--detector-delta`、`--roi x,y,width,height`、`--backend`、`--ocr-backend`。ROI 归一化到 0-1，省略或设为零尺寸时默认全屏检测。

## 流水线概览

1. 选择解码器并输出 Y 平面帧。
2. 抽样并评分字幕区域。
3. 跨帧比较区域以判断字幕起止。
4. 对确认区域做 OCR，生成 `.srt`。

每步都用异步流传递数据，背压自动传递给解码端。

## 调试与测试

- 快速冒烟：`cargo run --release -- --backend mock --output subtitles.srt path/to/video.mp4`
- 解码集成测试需示例视频与对应特性，例如：

```bash
SUBFAST_TEST_ASSET=/path/to/video.mp4 \
cargo test -p subtitle-fast-decoder --features backend-ffmpeg
```

## 性能快照

- 在 Mac mini M4 上，`cargo run --release --features backend-videotoolbox,ocr-vision` 处理一段 2h01m 的 1080p H.264（High，yuv420p，29.97 fps，约 5.0 Mbps 视频 + AAC 48 kHz 立体声 约 255 kb/s，总码率约 5.26 Mbps）约耗时 1 分 40 秒，全程触发约 3,622 次 OCR 请求。
