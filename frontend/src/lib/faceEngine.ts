/**
 * The face engine (face-api.js + model weights), fetched ONLY after the device has authenticated.
 *
 * Nothing face-related ships with the frontend bundle or in /public: the library and weights are streamed
 * by the backend from the face sidecar, and every request carries the caller's token,
 *   - "guide" (logged-in employee): tiny detector + tiny landmarks, for on-screen guidance only;
 *   - "full"  (registered kiosk):   also SSD MobileNet, 68-point landmarks and the recognition network.
 * The library is imported from a blob: URL built from the authenticated download, and every weight shard is
 * downloaded with the token too (see loadNet).
 */

// The module is loaded at runtime from the backend, so there are no static types for it.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type FaceApi = any;

export type EngineMode = "guide" | "full";

interface Loaded {
  api: FaceApi;
  blobUrl: string;
}

let current: { key: string; promise: Promise<Loaded> } | null = null;

async function load(base: string, token: string, mode: EngineMode): Promise<Loaded> {
  const authFetch = async (url: string) => {
    const res = await fetch(url, { headers: { Authorization: `Bearer ${token}` }, cache: "no-store" });
    if (!res.ok) throw new Error(res.status === 401 ? "unauthorized" : `engine unavailable (${res.status})`);
    return res;
  };
  const source = (await (await authFetch(`${base}/face-api.esm.js`)).text()).replace(/\/\/# sourceMappingURL=.*$/m, "");
  const blobUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
  const api: FaceApi = await import(/* webpackIgnore: true */ blobUrl);

  // WebGL where available, else the CPU backend (never the WASM one, whose binaries are not served).
  try {
    if (!(await api.tf.setBackend("webgl"))) await api.tf.setBackend("cpu");
  } catch {
    await api.tf.setBackend("cpu");
  }
  await api.tf.ready();

  // Weights are downloaded here, with the token, and handed to each network, TensorFlow.js would
  // otherwise fetch the shards itself without the Authorization header. The bytes are kept (in memory only) so
  // the networks can be rebuilt on the CPU backend if WebGL stalls (fallbackToCpu).
  const sources: WeightSource[] = [];
  const loadNet = async (net: Net, name: string) => {
    const manifest: { paths: string[]; weights: unknown[] }[] = await (await authFetch(`${base}/${name}-weights_manifest.json`)).json();
    const shards = await Promise.all(manifest.flatMap((g) => g.paths).map(async (p) => (await authFetch(`${base}/${p}`)).arrayBuffer()));
    const joined = new Uint8Array(shards.reduce((n, b) => n + b.byteLength, 0));
    let offset = 0;
    for (const b of shards) { joined.set(new Uint8Array(b), offset); offset += b.byteLength; }
    const specs = manifest.flatMap((g) => g.weights);
    net.loadFromWeightMap(api.tf.io.decodeWeights(joined.buffer, specs));
    sources.push({ net, buffer: joined.buffer, specs });
  };
  await loadNet(api.nets.tinyFaceDetector, "tiny_face_detector_model");
  await loadNet(api.nets.faceLandmark68TinyNet, "face_landmark_68_tiny_model");
  if (mode === "full") {
    await loadNet(api.nets.ssdMobilenetv1, "ssd_mobilenetv1_model");
    await loadNet(api.nets.faceLandmark68Net, "face_landmark_68_model");
    await loadNet(api.nets.faceRecognitionNet, "face_recognition_model");
  }
  weightSources.set(api, sources);
  return { api, blobUrl };
}

interface Net { loadFromWeightMap: (m: unknown) => void; dispose?: () => void; isLoaded?: boolean }
interface WeightSource { net: Net; buffer: ArrayBuffer; specs: unknown[] }
const weightSources = new WeakMap<object, WeightSource[]>();

/** True when the engine's WebGL context is lost (false on the CPU backend or when it cannot be told). */
export function webglContextLost(api: FaceApi): boolean {
  try {
    const gl = api?.tf?.backend?.()?.gpgpu?.gl;
    return !!gl?.isContextLost?.();
  } catch {
    return false;
  }
}

/**
 * Move the engine to the CPU backend and rebuild every network from its kept weight bytes, never reading the
 * old weights back from the GPU (a stalled or lost WebGL context would hang or return zeros). Slower per tick, but
 * it finishes. Returns false when it could not switch (already on the CPU, or the switch failed).
 */
export async function fallbackToCpu(api: FaceApi): Promise<boolean> {
  if (engineBackend(api) === "cpu") return false;
  try {
    if (!(await api.tf.setBackend("cpu"))) return false;
    await api.tf.ready();
    for (const s of weightSources.get(api) ?? []) {
      try { s.net.dispose?.(); } catch { /* the old WebGL tensors may be gone already */ }
      s.net.loadFromWeightMap(api.tf.io.decodeWeights(s.buffer, s.specs));
    }
    warmed.delete(api);
    return true;
  } catch {
    return false;
  }
}

/** Load (once per base + mode) the engine with this token. */
export function loadFaceEngine(base: string, token: string, mode: EngineMode): Promise<FaceApi> {
  const key = `${base}|${mode}`;
  if (!current || current.key !== key) {
    const promise = load(base, token, mode);
    current = { key, promise };
    promise.catch(() => { if (current?.promise === promise) current = null; });
  }
  return current.promise.then((l) => l.api);
}

/** Drop the engine from memory (e.g. a kiosk was revoked): weights disposed, module released. */
export async function unloadFaceEngine(): Promise<void> {
  const loaded = current;
  current = null;
  if (!loaded) return;
  try {
    const { api, blobUrl } = await loaded.promise;
    for (const net of Object.values(api.nets ?? {}) as { isLoaded?: boolean; dispose?: () => void }[]) {
      if (net?.isLoaded) net.dispose?.();
    }
    URL.revokeObjectURL(blobUrl);
  } catch {
    /* never loaded */
  }
}

export interface DetectedFace {
  score: number;
  /** Pixel box on the input (x, y = top left). */
  box: { x: number; y: number; width: number; height: number };
  landmarks: [number, number][];
  descriptor?: Float32Array;
}

/** Fast pass for live guidance: tiny detector + tiny 68-point landmarks. */
export async function trackFaces(api: FaceApi, input: HTMLVideoElement | HTMLCanvasElement): Promise<DetectedFace[]> {
  const results = await api
    .detectAllFaces(input, new api.TinyFaceDetectorOptions({ inputSize: 224, scoreThreshold: 0.5 }))
    .withFaceLandmarks(true);
  return results.map(toFace);
}

const warmed = new WeakSet<object>();

/**
 * One detection on a blank frame, once per engine: WebGL compiles its shaders on the first call (often a second
 * or more on a phone), so the first real tick of a check is not eaten by it. Never counted against a step.
 */
export async function warmUpEngine(api: FaceApi): Promise<void> {
  if (!api || warmed.has(api)) return;
  warmed.add(api);
  await blankDetection(api);
}

/** One detection on a blank 224 x 224 frame (errors swallowed). */
export async function blankDetection(api: FaceApi): Promise<void> {
  try {
    const blank = document.createElement("canvas");
    blank.width = 224;
    blank.height = 224;
    blank.getContext("2d")?.fillRect(0, 0, 224, 224);
    await trackFaces(api, blank);
  } catch {
    /* a failed warm-up only costs the first tick */
  }
}

/** The TensorFlow.js backend the engine runs on, for the attempt log. */
export function engineBackend(api: FaceApi): "webgl" | "cpu" | "unknown" {
  try {
    const b = api?.tf?.getBackend?.();
    return b === "webgl" || b === "cpu" ? b : "unknown";
  } catch {
    return "unknown";
  }
}

/** Accurate pass on a key frame (kiosk): SSD detector, full landmarks, 128-d descriptor, as the server computes. */
export async function describeFaces(api: FaceApi, input: HTMLCanvasElement): Promise<DetectedFace[]> {
  const results = await api
    .detectAllFaces(input, new api.SsdMobilenetv1Options({ minConfidence: 0.5, maxResults: 5 }))
    .withFaceLandmarks()
    .withFaceDescriptors();
  return results.map(toFace);
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function toFace(r: any): DetectedFace {
  return {
    score: r.detection.score,
    box: { x: r.detection.box.x, y: r.detection.box.y, width: r.detection.box.width, height: r.detection.box.height },
    landmarks: r.landmarks.positions.map((p: { x: number; y: number }) => [p.x, p.y]),
    descriptor: r.descriptor,
  };
}
