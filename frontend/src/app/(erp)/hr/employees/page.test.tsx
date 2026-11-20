import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";

import EmployeesPage from "./page";

const createEmployee = vi.fn();
const updateEmployee = vi.fn();
const getEmployee = vi.fn();
vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  hrApi: {
    listEmployees: () => Promise.resolve({ data: [
      { id: "e1", employee_id: "EMP-1", full_name: "Ana Putri", first_name: "Ana", last_name: "Putri", email: "ana@example.com",
        employment_type: "full_time", status: "active" },
    ] }),
    listJobTitles: () => Promise.resolve({ data: [] }),
    createEmployee: (...a: unknown[]) => createEmployee(...a),
    updateEmployee: (...a: unknown[]) => updateEmployee(...a),
    getEmployee: (...a: unknown[]) => getEmployee(...a),
  },
  systemApi: { getCurrencySettings: () => Promise.resolve({ data: { functional: "IDR", active: ["IDR", "USD"] } }) },
}));
const perms = { held: [] as string[] };
vi.mock("@/hooks/usePermissions", () => ({
  usePermissions: () => ({
    loaded: true, isAdmin: false, isSuperadmin: false,
    holds: (c: string) => perms.held.includes(c), can: (c: string) => perms.held.includes(c),
  }),
}));
vi.mock("next-auth/react", () => ({ useSession: () => ({ data: { user: { id: "u1", email: "a@b.c" } } }) }));
vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn() }), usePathname: () => "/hr/employees", useSearchParams: () => new URLSearchParams() }));

async function openEdit() {
  fireEvent.click((await screen.findAllByRole("button", { name: "Show details" }))[0]);
  fireEvent.click((await screen.findAllByRole("button", { name: "Edit" }))[0]);
}

describe("HR > Employees: base salary", () => {
  beforeEach(() => {
    createEmployee.mockReset(); updateEmployee.mockReset(); getEmployee.mockReset();
    perms.held = ["hr:manage_employees", "hr:manage_compensation", "hr:view_compensation"];
  });

  it("adds an employee with a salary in the functional currency by default", async () => {
    createEmployee.mockResolvedValue({ data: { id: "e2" } });
    render(<EmployeesPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: /Add employee/ }))[0]);
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByPlaceholderText("e.g. Robert Smith"), { target: { value: "Budi Santoso" } });
    fireEvent.change(within(dialog).getByPlaceholderText("robert.smith@example.com"), { target: { value: "budi@example.com" } });
    fireEvent.change(within(dialog).getByLabelText(/Base salary/), { target: { value: "9500000" } });
    await waitFor(() => expect((within(dialog).getByLabelText("Salary currency") as HTMLSelectElement).value).toBe("IDR"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Add employee" }));
    await waitFor(() => expect(createEmployee).toHaveBeenCalledTimes(1));
    expect(createEmployee.mock.calls[0][0]).toEqual(expect.objectContaining({
      full_name: "Budi Santoso", base_salary: "9500000", salary_currency: "IDR",
    }));
  });

  it("edits the salary, loading the current one first", async () => {
    getEmployee.mockResolvedValue({ data: { id: "e1", base_salary: 8000000, salary_currency: "IDR" } });
    updateEmployee.mockResolvedValue({ data: {} });
    render(<EmployeesPage />);
    await openEdit();
    const dialog = await screen.findByRole("dialog");
    const salary = within(dialog).getByLabelText(/Base salary/) as HTMLInputElement;
    await waitFor(() => expect(salary.value).toBe("8000000"));
    fireEvent.change(salary, { target: { value: "8500000" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateEmployee).toHaveBeenCalledTimes(1));
    expect(updateEmployee.mock.calls[0][0]).toBe("e1");
    expect(updateEmployee.mock.calls[0][1]).toEqual(expect.objectContaining({ base_salary: "8500000" }));
    expect(updateEmployee.mock.calls[0][1]).not.toHaveProperty("salary_currency");
  });

  it("never shows or loads the salary without compensation rights", async () => {
    perms.held = ["hr:manage_employees"];
    updateEmployee.mockResolvedValue({ data: {} });
    render(<EmployeesPage />);
    fireEvent.click((await screen.findAllByRole("button", { name: /Add employee/ }))[0]);
    let dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText(/Base salary/)).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await openEdit();
    dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText(/Base salary/)).toBeNull();
    expect(getEmployee).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(updateEmployee).toHaveBeenCalledTimes(1));
    expect(updateEmployee.mock.calls[0][1]).not.toHaveProperty("base_salary");
  });
});
