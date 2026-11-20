import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";

vi.mock("next/navigation", () => ({
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

import { replaceUrlParams, useUrlParam } from "./useUrlParam";

describe("useUrlParam", () => {
  let push: ReturnType<typeof vi.spyOn>;
  let replace: ReturnType<typeof vi.spyOn>;
  let back: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    window.history.replaceState(null, "", "/crm/email");
    push = vi.spyOn(window.history, "pushState");
    replace = vi.spyOn(window.history, "replaceState");
    back = vi.spyOn(window.history, "back").mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  it("opening pushes a history entry, switching replaces it, closing goes back", () => {
    const { result, rerender } = renderHook(() => useUrlParam("thread"));
    expect(result.current[0]).toBeNull();

    act(() => result.current[1]("a"));
    expect(push).toHaveBeenCalledTimes(1);
    expect(window.location.search).toBe("?thread=a");
    rerender();
    expect(result.current[0]).toBe("a");

    act(() => result.current[1]("b"));
    expect(replace).toHaveBeenCalledTimes(1);
    expect(window.location.search).toBe("?thread=b");

    act(() => result.current[1](null));
    expect(back).toHaveBeenCalledTimes(1);
  });

  it("closing a deep-linked value drops the parameter in place", () => {
    window.history.replaceState(null, "", "/crm/email?thread=x&box=sent");
    replace.mockClear();
    const { result } = renderHook(() => useUrlParam("thread"));
    expect(result.current[0]).toBe("x");
    act(() => result.current[1](null));
    expect(back).not.toHaveBeenCalled();
    expect(window.location.search).toBe("?box=sent");
  });

  it("replaceUrlParams changes several parameters in one entry", () => {
    window.history.replaceState(null, "", "/finance/bank?bank=1&stmt=2");
    replaceUrlParams({ bank: "3", stmt: null });
    expect(window.location.search).toBe("?bank=3");
    expect(push).not.toHaveBeenCalled();
  });
});
