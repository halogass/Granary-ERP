# Passive anti-spoof models (MiniFASNet)

- Source: https://github.com/minivision-ai/Silent-Face-Anti-Spoofing (Apache License 2.0, Minivision),
  `resources/anti_spoof_models/2.7_80x80_MiniFASNetV2.pth` and `4_0_0_80x80_MiniFASNetV1SE.pth`, downloaded 6 Nov 2026
  with the Owner's approval.
- Converted to ONNX (opset 13) with torch 2.15 CPU (`torch.onnx.export`, weights loaded with weights_only=True);
  ONNX Runtime matches PyTorch to < 3e-6 on random inputs. Repo sample images: T1 real, F1/F2 fake.

| file | crop scale | sha256 |
|---|---|---|
| minifasnet_v2_2.7.onnx | 2.7 x the face box | 9d0a094c60caf5416ee974c9edd87d98518723d5b1b328b6531b6e0e30f766d2 |
| minifasnet_v1se_4.0.onnx | 4.0 x the face box | 48ab1443c1f2821b37346ab5b9832717e3e9ef93fedb590a17abb795776c098d |

Input `input`: N x 3 x 80 x 80 float32, **BGR**, values **0..255** (not divided by 255), CHW.
Output `logits`: N x 3. Apply softmax per model, add the two models' probabilities, divide by 2:
index 1 = real face, 0 and 2 = spoof. real_score = (p_v2[1] + p_v1se[1]) / 2.

Not generated at build time (unlike ../onnx): these files are committed here and copied into the face image.

Crop (from the original `generate_patches.py`): face box (x, y, w, h); scale = min(scale, (H-1)/h, (W-1)/w);
new box = box scaled about its centre; shift inside the image (left/top < 0 -> move right/down; right/bottom >
W-1/H-1 -> move left/up); crop [top..bottom] x [left..right] inclusive; resize to 80 x 80 (bilinear).
