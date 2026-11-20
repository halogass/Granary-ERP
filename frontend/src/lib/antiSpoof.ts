/**
 * Passive liveness, part 2: the MiniFASNet anti-spoof score, run on the kiosk tablet with onnxruntime-web
 * (rust-version/services/face/models/antispoof/README.md).
 *
 * - The runtime (ort.wasm.min.mjs + its wasm) is served by our own app from /ort/ (copied out of the pinned
 *   onnxruntime-web package at build time, see package.json "build"); never a CDN.
 * - The two .onnx weight files are downloaded ONLY through the authenticated kiosk engine proxy, with the kiosk
 *   session token, exactly like the face-api weights (faceEngine.ts); nothing is in /public.
 * - Preprocessing follows the original generate_patches.py: the face box scaled about its centre (2.7 and 4.0),
 *   shifted inside the image, cropped inclusive, resized to 80 x 80 bilinear, BGR, 0..255, CHW.
 *   real = mean over the two models of softmax(logits)[1].
 */
import type * as OrtTypes from "onnxruntime-web";

import type { Pixels } from "@/lib/faceFlash";

type Ort = typeof OrtTypes;

export const ANTISPOOF_MODELS = [
  { file: "minifasnet_v2_2.7.onnx", scale: 2.7 },
  { file: "minifasnet_v1se_4.0.onnx", scale: 4.0 },
] as const;
export const INPUT_SIZE = 80;
/** Where the app serves the onnxruntime-web runtime files (copied from node_modules at build time). */
export const ORT_PATH = "/ort/";

export interface FaceBox { x: number; y: number; width: number; height: number }
/** An inclusive pixel rectangle. */
export interface CropRect { left: number; top: number; right: number; bottom: number }

/**
 * `_get_new_box` of generate_patches.py: scale = min(scale, (H-1)/h, (W-1)/w); the box scaled about its centre;
 * moved right/down when it pokes out left/top and left/up when it pokes out right/bottom; int() of each edge.
 */
export function cropBox(box: FaceBox, scale: number, width: number, height: number): CropRect {
  const s = Math.min(scale, (height - 1) / box.height, (width - 1) / box.width);
  const nw = box.width * s;
  const nh = box.height * s;
  const cx = box.x + box.width / 2;
  const cy = box.y + box.height / 2;
  let left = cx - nw / 2, top = cy - nh / 2, right = cx + nw / 2, bottom = cy + nh / 2;
  if (left < 0) { right -= left; left = 0; }
  if (top < 0) { bottom -= top; top = 0; }
  if (right > width - 1) { left -= right - width + 1; right = width - 1; }
  if (bottom > height - 1) { top -= bottom - height + 1; bottom = height - 1; }
  return { left: Math.trunc(left), top: Math.trunc(top), right: Math.trunc(right), bottom: Math.trunc(bottom) };
}

/**
 * Crop `rect` (inclusive) out of RGBA pixels and resize it to size x size the way cv2.resize(INTER_LINEAR) does
 * (half-pixel centres, edge clamp, rounded to 0..255), returned as a 1 x 3 x size x size float tensor in BGR order.
 */
export function cropToTensor(img: Pixels, rect: CropRect, size = INPUT_SIZE): Float32Array {
  const left = Math.max(0, Math.min(img.width - 1, rect.left));
  const top = Math.max(0, Math.min(img.height - 1, rect.top));
  const sw = Math.max(1, Math.min(img.width - 1, rect.right) - left + 1);
  const sh = Math.max(1, Math.min(img.height - 1, rect.bottom) - top + 1);
  const plane = size * size;
  const out = new Float32Array(3 * plane);
  const axis = (dst: number, src: number) => {
    let f = (dst + 0.5) * (src / size) - 0.5;
    let i = Math.floor(f);
    f -= i;
    if (i < 0) { i = 0; f = 0; }
    if (i >= src - 1) { i = src - 1; f = 0; }
    return { i, j: Math.min(i + 1, src - 1), f };
  };
  const xs = Array.from({ length: size }, (_, x) => axis(x, sw));
  for (let y = 0; y < size; y++) {
    const ay = axis(y, sh);
    const row0 = (top + ay.i) * img.width;
    const row1 = (top + ay.j) * img.width;
    for (let x = 0; x < size; x++) {
      const ax = xs[x];
      const p00 = (row0 + left + ax.i) * 4, p01 = (row0 + left + ax.j) * 4;
      const p10 = (row1 + left + ax.i) * 4, p11 = (row1 + left + ax.j) * 4;
      const w00 = (1 - ax.f) * (1 - ay.f), w01 = ax.f * (1 - ay.f), w10 = (1 - ax.f) * ay.f, w11 = ax.f * ay.f;
      const o = y * size + x;
      // RGBA channel c goes to tensor plane (2 - c): BGR.
      for (let c = 0; c < 3; c++) {
        const v = img.data[p00 + c] * w00 + img.data[p01 + c] * w01 + img.data[p10 + c] * w10 + img.data[p11 + c] * w11;
        out[(2 - c) * plane + o] = Math.min(255, Math.max(0, Math.round(v)));
      }
    }
  }
  return out;
}

export function softmax(logits: ArrayLike<number>): number[] {
  const arr = Array.from(logits);
  const m = Math.max(...arr);
  const ex = arr.map((v) => Math.exp(v - m));
  const sum = ex.reduce((s, v) => s + v, 0);
  return ex.map((v) => v / sum);
}

/** real = mean over the models of softmax(logits)[1]. */
export function realScore(logitsPerModel: ArrayLike<number>[]): number {
  if (!logitsPerModel.length) return 0;
  return logitsPerModel.reduce((s, l) => s + softmax(l)[1], 0) / logitsPerModel.length;
}

// ─── Runtime ──────────────────────────────────────────────────────────────────

export interface AntiSpoof {
  /** The real-face score of the face in `box` on these pixels (the neutral frame). */
  score(img: Pixels, box: FaceBox): Promise<number>;
  release(): Promise<void>;
}

async function importOrt(): Promise<Ort> {
  const base = `${window.location.origin}${ORT_PATH}`;
  const ort = (await import(/* webpackIgnore: true */ `${base}ort.wasm.min.mjs`)) as Ort;
  ort.env.wasm.wasmPaths = base;
  // One thread: no SharedArrayBuffer, so no cross-origin isolation headers are needed.
  ort.env.wasm.numThreads = 1;
  ort.env.wasm.proxy = false;
  return ort;
}

async function load(base: string, token: string): Promise<AntiSpoof> {
  const ort = await importOrt();
  const sessions = await Promise.all(ANTISPOOF_MODELS.map(async (m) => {
    const res = await fetch(`${base}/${m.file}`, { headers: { Authorization: `Bearer ${token}` }, cache: "no-store" });
    if (!res.ok) throw new Error(res.status === 401 ? "unauthorized" : `anti-spoof model unavailable (${res.status})`);
    const bytes = new Uint8Array(await res.arrayBuffer());
    return { scale: m.scale, session: await ort.InferenceSession.create(bytes, { executionProviders: ["wasm"] }) };
  }));
  return {
    async score(img, box) {
      const logits: Float32Array[] = [];
      for (const { scale, session } of sessions) {
        const data = cropToTensor(img, cropBox(box, scale, img.width, img.height));
        const out = await session.run({ [session.inputNames[0]]: new ort.Tensor("float32", data, [1, 3, INPUT_SIZE, INPUT_SIZE]) });
        logits.push(out[session.outputNames[0]].data as Float32Array);
      }
      return realScore(logits);
    },
    async release() {
      await Promise.all(sessions.map((s) => s.session.release().catch(() => {})));
    },
  };
}

let current: { key: string; promise: Promise<AntiSpoof> } | null = null;

/** Load (once per base) the two anti-spoof models with the kiosk session token. */
export function loadAntiSpoof(base: string, token: string): Promise<AntiSpoof> {
  if (!current || current.key !== base) {
    const promise = load(base, token);
    current = { key: base, promise };
    promise.catch(() => { if (current?.promise === promise) current = null; });
  }
  return current.promise;
}

/** Drop the models from memory (the kiosk was revoked). */
export async function unloadAntiSpoof(): Promise<void> {
  const loaded = current;
  current = null;
  if (!loaded) return;
  try {
    await (await loaded.promise).release();
  } catch {
    /* never loaded */
  }
}
