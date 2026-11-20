import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { THEME_KEY, THEME_SCRIPT, useTheme, type ThemePref } from "./theme";

let osDark = false;
const listeners: (() => void)[] = [];
function mockMatchMedia() {
  window.matchMedia = vi.fn().mockImplementation((q: string) => ({
    matches: q.includes("dark") ? osDark : false,
    media: q,
    addEventListener: (_: string, fn: () => void) => listeners.push(fn),
    removeEventListener: () => undefined,
  })) as unknown as typeof window.matchMedia;
}

function Probe() {
  const { theme, resolved, setTheme } = useTheme();
  return (
    <>
      <output data-testid="t">{theme}/{resolved}</output>
      {(["system", "light", "dark"] as ThemePref[]).map((p) => <button key={p} onClick={() => setTheme(p)}>{p}</button>)}
    </>
  );
}

beforeEach(() => {
  localStorage.clear();
  document.documentElement.classList.remove("dark");
  osDark = false;
  listeners.length = 0;
  mockMatchMedia();
});

describe("theme", () => {
  it("the pre-paint script follows the OS, then the stored choice", () => {
    osDark = true;
    new Function(THEME_SCRIPT)();
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    localStorage.setItem(THEME_KEY, "light");
    new Function(THEME_SCRIPT)();
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("switching stores the choice and sets class=dark; system clears it", () => {
    render(<Probe />);
    expect(screen.getByTestId("t")).toHaveTextContent("system/light");
    fireEvent.click(screen.getByText("dark"));
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(screen.getByTestId("t")).toHaveTextContent("dark/dark");

    fireEvent.click(screen.getByText("system"));
    expect(localStorage.getItem(THEME_KEY)).toBeNull();
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("on system, follows an OS change", () => {
    render(<Probe />);
    osDark = true;
    act(() => listeners.forEach((fn) => fn()));
    expect(screen.getByTestId("t")).toHaveTextContent("system/dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });
});
