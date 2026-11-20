import { describe, it, expect } from "vitest";
import { currencyOptions } from "./useCurrencySettings";

describe("currencyOptions", () => {
  it("offers the active currencies, not a fixed USD/EUR pair", () => {
    expect(currencyOptions(["SGD", "JPY"]).map((o) => o.value)).toEqual(["SGD", "JPY"]);
  });

  it("keeps a current value that is no longer active", () => {
    expect(currencyOptions(["SGD"], "EUR").map((o) => o.value)).toEqual(["EUR", "SGD"]);
    expect(currencyOptions(["SGD"], "SGD").map((o) => o.value)).toEqual(["SGD"]);
  });
});
