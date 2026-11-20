import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

import AssetCategoriesPage from "./page";

const get = vi.fn();
const post = vi.fn();
const patch = vi.fn();
const listAccounts = vi.fn();
vi.mock("@/lib/api", () => ({
  default: {
    get: (...a: unknown[]) => get(...a),
    post: (...a: unknown[]) => post(...a),
    patch: (...a: unknown[]) => patch(...a),
    delete: vi.fn(),
  },
  financeApi: { listAccounts: (...a: unknown[]) => listAccounts(...a) },
}));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/finance/assets/categories", useSearchParams: () => new URLSearchParams() }));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));

const ACCOUNTS = [
  { id: "a-fixed", code: "1500", name: "Equipment", category: "assets", is_active: true },
  { id: "a-accum", code: "1590", name: "Accumulated depreciation", category: "assets", is_active: true },
  { id: "a-dep", code: "6100", name: "Depreciation expense", category: "expenses", is_active: true },
];
const CATEGORY = {
  id: "c1", name: "IT Equipment", description: null, depreciation_method: "straight_line", useful_life_months: 36,
  salvage_value_pct: 0, gl_asset_account_id: "a-fixed", gl_depreciation_account_id: null, gl_accumulated_account_id: null,
};

function selectByOptionText(text: string, nth = 0) {
  return screen.getAllByRole("combobox").filter((s) =>
    Array.from((s as HTMLSelectElement).options).some((o) => o.text === text))[nth] as HTMLSelectElement;
}

describe("asset categories: GL account pickers", () => {
  beforeEach(() => {
    get.mockReset(); post.mockReset(); patch.mockReset(); listAccounts.mockReset();
    get.mockResolvedValue({ data: [CATEGORY] });
    listAccounts.mockResolvedValue({ data: ACCOUNTS });
    post.mockResolvedValue({ data: {} });
    patch.mockResolvedValue({ data: {} });
  });

  it("creates a category with the three GL accounts", async () => {
    render(<AssetCategoriesPage />);
    await waitFor(() => expect(listAccounts).toHaveBeenCalledWith({ active_only: true }));
    fireEvent.click(await screen.findByRole("button", { name: /New category/ }));
    fireEvent.change(screen.getByPlaceholderText(/Vehicles/), { target: { value: "Vehicles" } });
    fireEvent.change(screen.getByLabelText("Asset account"), { target: { value: "a-fixed" } });
    fireEvent.change(screen.getByLabelText("Depreciation expense account"), { target: { value: "a-dep" } });
    fireEvent.change(screen.getByLabelText("Accumulated depreciation account"), { target: { value: "a-accum" } });
    fireEvent.click(screen.getByRole("button", { name: "Create category" }));
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1));
    expect(post.mock.calls[0][0]).toBe("/finance/categories");
    expect(post.mock.calls[0][1]).toEqual(expect.objectContaining({
      name: "Vehicles",
      gl_asset_account_id: "a-fixed",
      gl_depreciation_account_id: "a-dep",
      gl_accumulated_account_id: "a-accum",
    }));
  });

  it("lists expense accounts first in the depreciation picker", async () => {
    render(<AssetCategoriesPage />);
    await waitFor(() => expect(listAccounts).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: /New category/ }));
    const dep = await waitFor(() => {
      const el = screen.getByLabelText("Depreciation expense account") as HTMLSelectElement;
      expect(el.options.length).toBe(4);
      return el;
    });
    expect(dep.options[1].text).toBe("6100 - Depreciation expense");
    expect(selectByOptionText("1500 - Equipment")).toBeTruthy();
  });

  it("edits an existing category's accounts", async () => {
    render(<AssetCategoriesPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit category" }))[0]);
    await waitFor(() => expect((screen.getByLabelText("Asset account") as HTMLSelectElement).value).toBe("a-fixed"));
    fireEvent.change(screen.getByLabelText("Depreciation expense account"), { target: { value: "a-dep" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(patch).toHaveBeenCalledTimes(1));
    expect(patch.mock.calls[0][0]).toBe("/finance/categories/c1");
    expect(patch.mock.calls[0][1]).toEqual(expect.objectContaining({
      gl_asset_account_id: "a-fixed", gl_depreciation_account_id: "a-dep", gl_accumulated_account_id: null,
    }));
  });

  it("clears an account by sending null (batch E: the API now clears it)", async () => {
    render(<AssetCategoriesPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: "Edit category" }))[0]);
    await waitFor(() => expect((screen.getByLabelText("Asset account") as HTMLSelectElement).value).toBe("a-fixed"));
    fireEvent.change(screen.getByLabelText("Asset account"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(patch).toHaveBeenCalledTimes(1));
    expect(patch.mock.calls[0][1]).toEqual(expect.objectContaining({ gl_asset_account_id: null }));
  });
});
