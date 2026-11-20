import { describe, expect, it } from "vitest";

import { ANTISPOOF_MODELS, cropBox, cropToTensor, realScore, softmax } from "@/lib/antiSpoof";

function image(width: number, height: number, px: (x: number, y: number) => [number, number, number]) {
  const data = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const [r, g, b] = px(x, y);
      const i = (y * width + x) * 4;
      data[i] = r; data[i + 1] = g; data[i + 2] = b; data[i + 3] = 255;
    }
  }
  return { data, width, height };
}

describe("cropBox (generate_patches.py _get_new_box)", () => {
  it("scales the box about its centre", () => {
    expect(cropBox({ x: 300, y: 200, width: 40, height: 40 }, 2.7, 640, 480)).toEqual({ left: 266, top: 166, right: 374, bottom: 274 });
  });

  it("caps the scale at the image size and moves the box right/down inside it", () => {
    // scale min(4, 99/50) = 1.98 -> 99 px box centred at 35 -> starts at -14.5 -> shifted to 0..99.
    expect(cropBox({ x: 10, y: 10, width: 50, height: 50 }, 4.0, 100, 100)).toEqual({ left: 0, top: 0, right: 99, bottom: 99 });
  });

  it("moves the box left when it pokes out on the right", () => {
    expect(cropBox({ x: 170, y: 150, width: 20, height: 20 }, 2.7, 200, 200)).toEqual({ left: 145, top: 133, right: 199, bottom: 187 });
  });

  it("truncates each edge like int()", () => {
    expect(cropBox({ x: 100.7, y: 50.2, width: 10, height: 10 }, 2.7, 400, 400)).toEqual({ left: 92, top: 41, right: 119, bottom: 68 });
  });

  it("uses the two README crop scales", () => {
    expect(ANTISPOOF_MODELS.map((m) => m.scale)).toEqual([2.7, 4.0]);
  });
});

describe("cropToTensor", () => {
  it("is CHW in BGR order with raw 0..255 values", () => {
    const t = cropToTensor(image(80, 80, () => [10, 20, 255]), { left: 0, top: 0, right: 79, bottom: 79 });
    expect(t).toHaveLength(3 * 80 * 80);
    const plane = 80 * 80;
    expect(t[0]).toBe(255);            // B
    expect(t[plane]).toBe(20);         // G
    expect(t[2 * plane]).toBe(10);     // R
    expect(t[plane - 1]).toBe(255);
    expect(t[3 * plane - 1]).toBe(10);
  });

  it("crops the inclusive rectangle (same size: pixels copied as they are)", () => {
    const t = cropToTensor(image(10, 10, (x, y) => [x, y, 0]), { left: 2, top: 3, right: 5, bottom: 6 }, 4);
    const plane = 16;
    for (let y = 0; y < 4; y++) {
      for (let x = 0; x < 4; x++) {
        expect(t[2 * plane + y * 4 + x]).toBe(2 + x);   // R
        expect(t[plane + y * 4 + x]).toBe(3 + y);       // G
      }
    }
  });

  it("downscales bilinearly with half-pixel centres like cv2.resize", () => {
    // 8 -> 4 wide: each output pixel sits between source pixels 2x and 2x+1: (20x + 20x + 10) / 2.
    const t = cropToTensor(image(8, 8, (x) => [10 * x, 0, 0]), { left: 0, top: 0, right: 7, bottom: 7 }, 4);
    expect(Array.from(t.slice(2 * 16, 2 * 16 + 4))).toEqual([5, 25, 45, 65]);
  });
});

describe("real score", () => {
  it("is the mean of softmax[1] over the two models", () => {
    expect(softmax([0, 0, 0]).reduce((a, b) => a + b, 0)).toBeCloseTo(1, 10);
    expect(realScore([[0, 0, 0], [0, Math.log(3), 0]])).toBeCloseTo((1 / 3 + 3 / 5) / 2, 10);
    expect(realScore([[0, 50, 0], [0, 50, 0]])).toBeCloseTo(1, 10);
  });
});
