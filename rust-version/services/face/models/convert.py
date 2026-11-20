#!/usr/bin/env python3
"""Convert the face-api.js 1.7.15 model weights (tfjs weight manifests) to ONNX graphs.

face-api.js does not ship graph models: each network is written by hand in TypeScript with tf ops and the
manifest only holds the weights. This script rebuilds the same three networks op for op as ONNX graphs
(NCHW) and fills them with the same weights, dequantized exactly as tfjs does it
(`uint8 * scale + min` in float64, stored as float32):

  ssd_mobilenetv1.onnx      src/ssdMobilenetv1/*.ts      input [1,3,512,512] in -1..1
                            outputs: boxes [5118,4] (decoded, relative ymin/xmin/ymax/xmax), scores [5118]
  face_landmark_68.onnx     src/faceFeatureExtractor + faceLandmarkNet   input [1,3,112,112] (x-mean)/255
                            output: landmarks [1,136] (the fc output, before postProcess)
  face_recognition.onnx     src/faceRecognitionNet/*.ts  input [1,3,150,150] (x-mean)/255
                            output: descriptor [1,128]

Pre-processing (pad to square, legacy bilinear resize, normalisation) and post-processing (NMS, landmark
un-padding, alignment, crops) stay in Rust (src/pipeline.rs), as face-api does them outside the networks.

Usage (repo venv with `pip install onnx numpy`):
  .venv/bin/python rust-version/services/face/models/convert.py <face-api package dir>/model <out dir>
"""
import json
import os
import sys

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

OPSET = 17


def load_weights(model_dir: str, name: str) -> dict:
    """tf.io.decodeWeights for a manifest: name -> np.ndarray (float32 / int32)."""
    with open(os.path.join(model_dir, f"{name}-weights_manifest.json")) as f:
        manifest = json.load(f)
    out = {}
    for group in manifest:
        data = b"".join(open(os.path.join(model_dir, p), "rb").read() for p in group["paths"])
        offset = 0
        for w in group["weights"]:
            shape = w["shape"]
            size = int(np.prod(shape)) if shape else 1
            q = w.get("quantization")
            if q:
                assert q["dtype"] == "uint8", q
                raw = np.frombuffer(data, dtype=np.uint8, count=size, offset=offset)
                offset += size
                vals = raw.astype(np.float64) * q["scale"] + q["min"]
                if w["dtype"] == "int32":
                    vals = np.floor(vals + 0.5).astype(np.int32)  # Math.round
                else:
                    vals = vals.astype(np.float32)
            else:
                dt = {"float32": np.float32, "int32": np.int32}[w["dtype"]]
                vals = np.frombuffer(data, dtype=dt, count=size, offset=offset).copy()
                offset += size * 4
            out[w["name"]] = vals.reshape(shape)
        assert offset == len(data), (name, offset, len(data))
    return out


class Graph:
    """A tiny NCHW graph builder that tracks static shapes (all inputs have a fixed size)."""

    def __init__(self, name: str):
        self.name = name
        self.nodes = []
        self.inits = []
        self.shapes = {}
        self.n = 0

    def uid(self, prefix: str) -> str:
        self.n += 1
        return f"{prefix}_{self.n}"

    def const(self, arr: np.ndarray, prefix: str = "c") -> str:
        name = self.uid(prefix)
        arr = np.asarray(arr)
        self.inits.append(numpy_helper.from_array(arr, name))
        self.shapes[name] = arr.shape
        return name

    def op(self, op_type: str, inputs: list, shape, **attrs) -> str:
        out = self.uid(op_type.lower())
        self.nodes.append(helper.make_node(op_type, inputs, [out], **attrs))
        self.shapes[out] = shape
        return out

    @staticmethod
    def same_pads(size: int, k: int, s: int) -> tuple:
        """TF 'same': out = ceil(in/s), extra padding goes after (bottom/right)."""
        out = -(-size // s)
        total = max((out - 1) * s + k - size, 0)
        return total // 2, total - total // 2, out

    def conv(self, x: str, w_hwio: np.ndarray, stride: int, padding: str, bias=None, group: int = 1) -> str:
        """tf.conv2d / tf.depthwiseConv2d (w given in TF layout, converted to OIHW here)."""
        n, c, h, wd = self.shapes[x]
        kh, kw = w_hwio.shape[0], w_hwio.shape[1]
        if group == 1:
            w = np.transpose(w_hwio, (3, 2, 0, 1))
            cout = w.shape[0]
        else:
            assert w_hwio.shape[3] == 1 and w_hwio.shape[2] == c
            w = np.transpose(w_hwio, (2, 3, 0, 1))
            cout = c
        if padding == "same":
            t, b, oh = self.same_pads(h, kh, stride)
            l, r, ow = self.same_pads(wd, kw, stride)
        else:
            t = b = l = r = 0
            oh = (h - kh) // stride + 1
            ow = (wd - kw) // stride + 1
        inputs = [x, self.const(w.astype(np.float32), "w")]
        if bias is not None:
            inputs.append(self.const(np.asarray(bias, dtype=np.float32), "b"))
        return self.op("Conv", inputs, (n, cout, oh, ow), kernel_shape=[kh, kw], strides=[stride, stride],
                       pads=[t, l, b, r], group=group)

    def add_channel(self, x: str, v: np.ndarray) -> str:
        """tf.add(x, v) with v of shape [C] broadcast over NHWC's last axis."""
        return self.op("Add", [x, self.const(np.asarray(v, np.float32).reshape(1, -1, 1, 1))], self.shapes[x])

    def mul_channel(self, x: str, v: np.ndarray) -> str:
        return self.op("Mul", [x, self.const(np.asarray(v, np.float32).reshape(1, -1, 1, 1))], self.shapes[x])

    def add(self, a: str, b: str) -> str:
        return self.op("Add", [a, b], self.shapes[a])

    def relu(self, x: str) -> str:
        return self.op("Relu", [x], self.shapes[x])

    def clip06(self, x: str) -> str:
        lo = self.const(np.array(0.0, np.float32))
        hi = self.const(np.array(6.0, np.float32))
        return self.op("Clip", [x, lo, hi], self.shapes[x])

    def pool(self, op_type: str, x: str, k: int, s: int) -> str:
        n, c, h, w = self.shapes[x]
        return self.op(op_type, [x], (n, c, (h - k) // s + 1, (w - k) // s + 1), kernel_shape=[k, k],
                       strides=[s, s])

    def concat(self, xs: list, axis: int) -> str:
        shape = list(self.shapes[xs[0]])
        shape[axis] = sum(self.shapes[x][axis] for x in xs)
        return self.op("Concat", xs, tuple(shape), axis=axis)

    def zeros(self, shape) -> str:
        return self.const(np.zeros(shape, np.float32), "zeros")

    def model(self, inp: str, in_shape, outputs: list) -> onnx.ModelProto:
        graph = helper.make_graph(
            self.nodes, self.name,
            [helper.make_tensor_value_info(inp, TensorProto.FLOAT, list(in_shape))],
            [helper.make_tensor_value_info(name, TensorProto.FLOAT, list(shape)) for name, shape in outputs],
            self.inits,
        )
        m = helper.make_model(graph, opset_imports=[helper.make_opsetid("", OPSET)],
                              producer_name="granary-face convert.py")
        m.ir_version = 8
        onnx.checker.check_model(m)
        return m


# ─── SSD MobileNetV1 (src/ssdMobilenetv1) ─────────────────────────────────────

BN_EPSILON = 0.0010000000474974513


def ssd_mobilenetv1(w: dict) -> onnx.ModelProto:
    g = Graph("ssd_mobilenetv1")
    g.shapes["input"] = (1, 3, 512, 512)

    def pointwise(x, prefix, idx, stride):
        out = g.conv(x, w[f"{prefix}/Conv2d_{idx}_pointwise/weights"], stride, "same")
        out = g.add_channel(out, w[f"{prefix}/Conv2d_{idx}_pointwise/convolution_bn_offset"])
        return g.clip06(out)

    def depthwise(x, idx, stride):
        p = f"MobilenetV1/Conv2d_{idx}_depthwise"
        out = g.conv(x, w[f"{p}/depthwise_weights"], stride, "same", group=g.shapes[x][1])
        c = g.shapes[out][1]
        bn = [g.const(np.asarray(w[f"{p}/BatchNorm/{k}"], np.float32).reshape(c))
              for k in ("gamma", "beta", "moving_mean", "moving_variance")]
        out = g.op("BatchNormalization", [out] + bn, g.shapes[out], epsilon=BN_EPSILON)
        return g.clip06(out)

    out = pointwise("input", "MobilenetV1", 0, 2)
    conv11 = None
    for idx in range(1, 14):
        stride = 2 if idx in (2, 4, 6, 12) else 1
        out = depthwise(out, idx, stride)
        out = pointwise(out, "MobilenetV1", idx, 1)
        if idx == 11:
            conv11 = out

    x = out
    conv0 = pointwise(x, "Prediction", 0, 1)
    conv1 = pointwise(conv0, "Prediction", 1, 2)
    conv2 = pointwise(conv1, "Prediction", 2, 1)
    conv3 = pointwise(conv2, "Prediction", 3, 2)
    conv4 = pointwise(conv3, "Prediction", 4, 1)
    conv5 = pointwise(conv4, "Prediction", 5, 2)
    conv6 = pointwise(conv5, "Prediction", 6, 1)
    conv7 = pointwise(conv6, "Prediction", 7, 2)

    def nhwc_rows(t, last):
        """tf.reshape(NHWC tensor, [-1, last]): transpose NCHW back to NHWC first."""
        n, c, h, wd = g.shapes[t]
        t = g.op("Transpose", [t], (n, h, wd, c), perm=[0, 2, 3, 1])
        rows = h * wd * c // last
        return g.op("Reshape", [t, g.const(np.array([rows, last], np.int64))], (rows, last))

    box_parts, class_parts = [], []
    for idx, feat in enumerate([conv11, x, conv1, conv3, conv5, conv7]):
        p = f"Prediction/BoxPredictor_{idx}"
        box = g.conv(feat, w[f"{p}/BoxEncodingPredictor/weights"], 1, "same",
                     bias=w[f"{p}/BoxEncodingPredictor/biases"])
        cls = g.conv(feat, w[f"{p}/ClassPredictor/weights"], 1, "same", bias=w[f"{p}/ClassPredictor/biases"])
        box_parts.append(nhwc_rows(box, 4))
        class_parts.append(nhwc_rows(cls, 3))
    box_pred = g.concat(box_parts, 0)        # [5118, 4]
    class_pred = g.concat(class_parts, 0)    # [5118, 3]
    num = g.shapes[box_pred][0]
    anchors = w["Output/extra_dim"].reshape(-1, 4)
    assert anchors.shape[0] == num, (anchors.shape, num)

    # outputLayer.decodeBoxesLayer, column by column as tf does it (float32)
    def col(t, i):
        return g.op("Gather", [t, g.const(np.array(i, np.int64))], (num,), axis=1)

    def bin_op(op_type, a, b):
        return g.op(op_type, [a, b], (num,))

    def scalar(v):
        return g.const(np.array(v, np.float32))

    a = [g.const(anchors[:, i].astype(np.float32), "anchor") for i in range(4)]
    size0 = bin_op("Sub", a[2], a[0])
    size1 = bin_op("Sub", a[3], a[1])
    center0 = bin_op("Add", a[0], bin_op("Div", size0, scalar(2.0)))
    center1 = bin_op("Add", a[1], bin_op("Div", size1, scalar(2.0)))
    v = [col(box_pred, i) for i in range(4)]
    div0 = bin_op("Div", bin_op("Mul", g.op("Exp", [bin_op("Div", v[2], scalar(5.0))], (num,)), size0), scalar(2.0))
    add0 = bin_op("Add", bin_op("Mul", bin_op("Div", v[0], scalar(10.0)), size0), center0)
    div1 = bin_op("Div", bin_op("Mul", g.op("Exp", [bin_op("Div", v[3], scalar(5.0))], (num,)), size1), scalar(2.0))
    add1 = bin_op("Add", bin_op("Mul", bin_op("Div", v[1], scalar(10.0)), size1), center1)
    cols = [bin_op("Sub", add0, div0), bin_op("Sub", add1, div1), bin_op("Add", add0, div0), bin_op("Add", add1, div1)]
    cols = [g.op("Unsqueeze", [c, g.const(np.array([1], np.int64))], (num, 1)) for c in cols]
    boxes = g.op("Concat", cols, (num, 4), axis=1)
    boxes = g.op("Identity", [boxes], (num, 4))
    g.nodes[-1].output[0] = "boxes"
    g.shapes["boxes"] = (num, 4)

    scores = g.op("Sigmoid", [col(class_pred, 1)], (num,))
    g.nodes[-1].output[0] = "scores"
    g.shapes["scores"] = (num,)
    return g.model("input", (1, 3, 512, 512), [("boxes", (num, 4)), ("scores", (num,))])


# ─── Face landmark 68 (src/faceFeatureExtractor + faceLandmarkNet) ────────────

def face_landmark_68(w: dict) -> onnx.ModelProto:
    g = Graph("face_landmark_68")
    g.shapes["input"] = (1, 3, 112, 112)

    def sep_conv(x, prefix, stride):
        """tf.separableConv2d(x, depthwise, pointwise, stride, 'same') + bias."""
        out = g.conv(x, w[f"{prefix}/depthwise_filter"], stride, "same", group=g.shapes[x][1])
        out = g.conv(out, w[f"{prefix}/pointwise_filter"], 1, "same")
        return g.add_channel(out, w[f"{prefix}/bias"])

    def dense_block4(x, prefix, first):
        if first:
            out1 = g.conv(x, w[f"{prefix}/conv0/filters"], 2, "same")
            out1 = g.relu(g.add_channel(out1, w[f"{prefix}/conv0/bias"]))
        else:
            out1 = g.relu(sep_conv(x, f"{prefix}/conv0", 2))
        out2 = sep_conv(out1, f"{prefix}/conv1", 1)
        in3 = g.relu(g.add(out1, out2))
        out3 = sep_conv(in3, f"{prefix}/conv2", 1)
        in4 = g.relu(g.add(out1, g.add(out2, out3)))
        out4 = sep_conv(in4, f"{prefix}/conv3", 1)
        return g.relu(g.add(out1, g.add(out2, g.add(out3, out4))))

    out = dense_block4("input", "dense0", True)
    out = dense_block4(out, "dense1", False)
    out = dense_block4(out, "dense2", False)
    out = dense_block4(out, "dense3", False)
    out = g.pool("AveragePool", out, 7, 2)
    n, c, h, wd = g.shapes[out]
    assert (h, wd) == (1, 1), g.shapes[out]
    # bottleneckFeatures.as2D(batch, -1): NHWC flattening; with H = W = 1 it equals the channel vector
    flat = g.op("Reshape", [out, g.const(np.array([1, c], np.int64))], (1, c))
    fc = g.op("MatMul", [flat, g.const(w["fc/weights"].astype(np.float32))], (1, 136))
    g.op("Add", [fc, g.const(w["fc/bias"].astype(np.float32))], (1, 136))
    g.nodes[-1].output[0] = "landmarks"
    return g.model("input", (1, 3, 112, 112), [("landmarks", (1, 136))])


# ─── Face recognition (src/faceRecognitionNet) ────────────────────────────────

def face_recognition(w: dict) -> onnx.ModelProto:
    g = Graph("face_recognition")
    g.shapes["input"] = (1, 3, 150, 150)

    def conv_layer(x, prefix, stride, relu, padding="same"):
        out = g.conv(x, w[f"{prefix}/conv/filters"], stride, padding)
        out = g.add_channel(out, w[f"{prefix}/conv/bias"])
        out = g.add_channel(g.mul_channel(out, w[f"{prefix}/scale/weights"]), w[f"{prefix}/scale/biases"])
        return g.relu(out) if relu else out

    def residual(x, prefix):
        out = conv_layer(x, f"{prefix}/conv1", 1, True)
        out = conv_layer(out, f"{prefix}/conv2", 1, False)
        return g.relu(g.add(out, x))

    def residual_down(x, prefix):
        out = conv_layer(x, f"{prefix}/conv1", 2, True, "valid")
        out = conv_layer(out, f"{prefix}/conv2", 1, False)
        pooled = g.pool("AveragePool", x, 2, 2)
        pn, pc, ph, pw = g.shapes[pooled]
        on, oc, oh, ow = g.shapes[out]
        if (ph, pw) != (oh, ow):
            # append one zero row (H) and then one zero column (W), as residualDown does
            out = g.concat([out, g.zeros((on, oc, 1, ow))], 2)
            on, oc, oh, ow = g.shapes[out]
            out = g.concat([out, g.zeros((on, oc, oh, 1))], 3)
        if pc != g.shapes[out][1]:
            pooled = g.concat([pooled, g.zeros((pn, pc, ph, pw))], 1)
        assert g.shapes[pooled] == g.shapes[out], (prefix, g.shapes[pooled], g.shapes[out])
        return g.relu(g.add(pooled, out))

    out = conv_layer("input", "conv32_down", 2, True, "valid")
    out = g.pool("MaxPool", out, 3, 2)
    for p in ("conv32_1", "conv32_2", "conv32_3"):
        out = residual(out, p)
    out = residual_down(out, "conv64_down")
    for p in ("conv64_1", "conv64_2", "conv64_3"):
        out = residual(out, p)
    out = residual_down(out, "conv128_down")
    for p in ("conv128_1", "conv128_2"):
        out = residual(out, p)
    out = residual_down(out, "conv256_down")
    for p in ("conv256_1", "conv256_2"):
        out = residual(out, p)
    out = residual_down(out, "conv256_down_out")
    n, c, h, wd = g.shapes[out]
    avg = g.op("ReduceMean", [out], (n, c), axes=[2, 3], keepdims=0)
    g.op("MatMul", [avg, g.const(w["fc"].astype(np.float32))], (1, 128))
    g.nodes[-1].output[0] = "descriptor"
    return g.model("input", (1, 3, 150, 150), [("descriptor", (1, 128))])


NETS = [
    ("ssd_mobilenetv1_model", "ssd_mobilenetv1.onnx", ssd_mobilenetv1),
    ("face_landmark_68_model", "face_landmark_68.onnx", face_landmark_68),
    ("face_recognition_model", "face_recognition.onnx", face_recognition),
]


def main() -> None:
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    model_dir, out_dir = sys.argv[1], sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    for manifest, filename, build in NETS:
        model = build(load_weights(model_dir, manifest))
        path = os.path.join(out_dir, filename)
        onnx.save(model, path)
        print(f"{path}: {os.path.getsize(path)} bytes, {len(model.graph.node)} nodes")


if __name__ == "__main__":
    main()
