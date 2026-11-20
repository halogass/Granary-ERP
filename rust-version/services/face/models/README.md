# Face models for granary-face

The Rust service runs face-api.js 1.7.15's own three networks (SSD MobileNetV1 detector, 68-point
landmark net, 128-d recognition net) on ONNX Runtime. face-api.js ships no graph files: each network is
TypeScript code over tf ops plus a weight manifest. `convert.py` rebuilds the three networks op for op
as ONNX graphs and loads the same weights, dequantised exactly as tfjs does (`uint8 * scale + min` in
float64, stored as float32). Nothing here is committed: `onnx/` and `face-api/` are generated
(`.gitignore`).

## Reproduce (the repo venv)

```sh
# 1. the npm package (weights + the browser bundle GET /assets serves); any scratch folder
mkdir -p /tmp/faceapi && cd /tmp/faceapi
npm pack @vladmandic/face-api@1.7.15 && tar xzf vladmandic-face-api-1.7.15.tgz

# 2. the converter (onnx 1.23.2 + numpy; onnxruntime only for the check in step 4)
cd <repo>
.venv/bin/pip install onnx numpy onnxruntime
.venv/bin/python rust-version/services/face/models/convert.py /tmp/faceapi/package/model \
    rust-version/services/face/models/onnx

# 3. the browser assets for development (or set FACE_API_DIR to the package folder)
ln -s /tmp/faceapi/package rust-version/services/face/models/face-api
```

The conversion is deterministic. From face-api 1.7.15 it gives (SHA-256):

| file | bytes | sha256 |
|---|---|---|
| ssd_mobilenetv1.onnx | 22238632 | 631207b1b0a83b639820385924c6462ddf8c77a3dcad0415543393fc73341b14 |
| face_landmark_68.onnx | 1433824 | 75924e2f20fd34e7da4d5e4d9b610d6999505d23a4d63f143061b433b7913b09 |
| face_recognition.onnx | 22541312 | 3dfd1770afce3ee30c1928c9c49c625e60a8cb78775caea68f01503764ed106e |

## 4. Check the conversion against face-api.js

```sh
NODE_PATH=<node_modules with services/face/package.json installed> \
  node rust-version/compat-tests/services/face/net_dump.js /tmp/nets
.venv/bin/python rust-version/compat-tests/services/face/verify_nets.py /tmp/nets \
  rust-version/services/face/models/onnx
```

Fixed pseudo-random inputs (no photos) through tfjs-node and through ONNX Runtime; measured on
19 Oct 2026: SSD boxes 2.4e-6, scores 1.9e-7, landmarks 3.0e-7, descriptor 2.7e-7 max absolute
difference (float32 noise).

## Graphs

| model | input (NCHW float32) | outputs |
|---|---|---|
| ssd_mobilenetv1.onnx | `input` [1,3,512,512], `x / 127.5 - 1` | `boxes` [5118,4] decoded (ymin, xmin, ymax, xmax, relative), `scores` [5118] (sigmoid of class 1) |
| face_landmark_68.onnx | `input` [1,3,112,112], `(x - meanRgb) / 255` | `landmarks` [1,136] (fc output before postProcess) |
| face_recognition.onnx | `input` [1,3,150,150], `(x - meanRgb) / 255` | `descriptor` [1,128] |

TensorFlow's `same` padding (extra row/column after) is written as explicit pads; the recognition net's
residualDown zero row/column and channel padding are static concats. Everything around the networks
(image decoding, padToSquare, the legacy bilinear resize, NMS, crops, landmark post-processing, dlib
alignment) is in `src/pipeline.rs` and `src/decode.rs`, with the same float32 / float64 arithmetic as
TensorFlow and JavaScript.

## Runtime

- ONNX Runtime 1.31.0 is linked statically (ort 2.0.0-rc.14, `download-binaries`: the build fetches the
  prebuilt library from cdn.pyke.io). No onnxruntime shared library is needed at run time.
- JPEG decoding uses the system libjpeg-turbo TurboJPEG library (`libturbojpeg.so.0`, Debian/Ubuntu
  package `libturbojpeg0`) with the fast integer IDCT: on a reference data set this is bit-identical to
  what tfjs-node's `decodeImage` returns.
- Environment: `FACE_ONNX_DIR` (this `onnx/` folder) and `FACE_API_DIR` (the npm package folder with
  `dist/face-api.esm.js` and `model/`); see the `face-models` stage of `rust-version/Dockerfile`.
