import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, act } from "@testing-library/react";
import { useState } from "react";

vi.mock("@/components/ui", async (orig) => {
  const mod = await orig<typeof import("@/components/ui")>();
  return { ...mod, toast: { success: vi.fn(), error: vi.fn() } };
});

import { toast } from "@/components/ui";
import { SaveBar, SettingsDraftProvider, useDraftSource, useLeaveGuard, useDraftSummary } from "./SettingsDraft";
import { SettingRow } from "./SettingRow";
import type { SettingDef } from "@/lib/settings-registry";

function Source({ id, save, discard, validate }: { id: string; save: () => Promise<void>; discard?: () => void; validate?: () => string | null }) {
  const [value, setValue] = useState("");
  const count = value ? value.split(",").length : 0;
  const { inSaveBar } = useDraftSource(id, { count, save, discard: () => { setValue(""); discard?.(); }, validate });
  return (
    <div>
      <input aria-label={`field ${id}`} value={value} onChange={(e) => setValue(e.target.value)} />
      {!inSaveBar && <button type="button">Own save</button>}
    </div>
  );
}

function Guarded() {
  const { total } = useDraftSummary();
  useLeaveGuard(total > 0);
  return <a href="/crm/deals">Deals</a>;
}

afterEach(() => vi.restoreAllMocks());

describe("SaveBar", () => {
  it("sums the changes of every form in the area and hides when clean", () => {
    render(
      <SettingsDraftProvider>
        <Source id="a" save={async () => {}} />
        <Source id="b" save={async () => {}} />
        <SaveBar areaLabel="People & payroll" />
      </SettingsDraftProvider>,
    );
    expect(screen.queryByRole("region", { name: "Unsaved changes" })).toBeNull();
    fireEvent.change(screen.getByLabelText("field a"), { target: { value: "x,y" } });
    fireEvent.change(screen.getByLabelText("field b"), { target: { value: "z" } });
    expect(screen.getByRole("region", { name: "Unsaved changes" })).toHaveTextContent("3 unsaved changes in People & payroll");
    expect(screen.queryByText("Own save")).toBeNull();
  });

  it("saves every dirty form, then says so", async () => {
    const saveA = vi.fn(async () => {});
    const saveB = vi.fn(async () => {});
    render(
      <SettingsDraftProvider>
        <Source id="a" save={saveA} />
        <Source id="b" save={saveB} />
        <SaveBar areaLabel="Money & tax" />
      </SettingsDraftProvider>,
    );
    fireEvent.change(screen.getByLabelText("field a"), { target: { value: "1" } });
    expect(screen.getByText("1 unsaved change in Money & tax")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveA).toHaveBeenCalledTimes(1));
    expect(saveB).not.toHaveBeenCalled();
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Settings saved"));
  });

  it("does not save while a form reports a problem", async () => {
    const save = vi.fn(async () => {});
    render(
      <SettingsDraftProvider>
        <Source id="a" save={save} validate={() => "Income tax brackets: Not valid JSON."} />
        <SaveBar areaLabel="People & payroll" />
      </SettingsDraftProvider>,
    );
    fireEvent.change(screen.getByLabelText("field a"), { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("Income tax brackets: Not valid JSON."));
    expect(save).not.toHaveBeenCalled();
  });

  it("shows the error and keeps the changes when saving fails", async () => {
    render(
      <SettingsDraftProvider>
        <Source id="a" save={async () => { throw new Error("boom"); }} />
        <SaveBar areaLabel="Operations" />
      </SettingsDraftProvider>,
    );
    fireEvent.change(screen.getByLabelText("field a"), { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(screen.getByText("1 unsaved change in Operations")).toBeInTheDocument();
  });

  it("discards every form", () => {
    const discard = vi.fn();
    render(
      <SettingsDraftProvider>
        <Source id="a" save={async () => {}} discard={discard} />
        <SaveBar areaLabel="Operations" />
      </SettingsDraftProvider>,
    );
    fireEvent.change(screen.getByLabelText("field a"), { target: { value: "1,2" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(discard).toHaveBeenCalled();
    expect(screen.queryByRole("region", { name: "Unsaved changes" })).toBeNull();
  });

  it("keeps a form's own Save button outside the provider", () => {
    render(<Source id="a" save={async () => {}} />);
    expect(screen.getByText("Own save")).toBeInTheDocument();
  });

  it("asks before following a link with unsaved changes", () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(
      <SettingsDraftProvider>
        <Source id="a" save={async () => {}} />
        <Guarded />
      </SettingsDraftProvider>,
    );
    const link = screen.getByRole("link", { name: "Deals" });
    let ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    act(() => { link.dispatchEvent(ev); });
    expect(confirm).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("field a"), { target: { value: "1" } });
    ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    act(() => { link.dispatchEvent(ev); });
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(ev.defaultPrevented).toBe(true);
  });
});

describe("SettingRow", () => {
  const def: SettingDef = { key: "nary.wa.complaint_pause_hours", label: "Pause a chat after a complaint", type: "number", default: "72", unit: "hours" };

  it("shows the key, an unsaved mark while edited and a changed mark against the default", () => {
    const onChange = vi.fn();
    const { rerender } = render(<SettingRow def={def} saved="72" draft={undefined} onChange={onChange} />);
    expect(screen.getByText("nary.wa.complaint_pause_hours")).toBeInTheDocument();
    expect(screen.queryByText("Unsaved")).toBeNull();
    expect(screen.queryByText(/changed \(default/)).toBeNull();
    fireEvent.change(screen.getByLabelText("Pause a chat after a complaint"), { target: { value: "48" } });
    expect(onChange).toHaveBeenCalledWith("48");
    rerender(<SettingRow def={def} saved="72" draft="48" onChange={onChange} />);
    expect(screen.getByText("Unsaved")).toBeInTheDocument();
    expect(screen.getByText("changed (default 72 hours)")).toBeInTheDocument();
  });

  it("edits a bracket table and falls back to raw JSON", () => {
    const brackets: SettingDef = { key: "payroll.income_tax", label: "Income tax brackets", type: "json", columns: ["Annual taxable income up to", "Rate %"] };
    const onChange = vi.fn();
    render(<SettingRow def={brackets} saved="[[60000000, 5], [null, 35]]" draft={undefined} onChange={onChange} />);
    expect(screen.getByRole("columnheader", { name: "Annual taxable income up to" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Rate %, row 1"), { target: { value: "6" } });
    expect(onChange).toHaveBeenLastCalledWith("[[60000000,6],[null,35]]");
    fireEvent.click(screen.getByRole("button", { name: /Add row/ }));
    expect(onChange).toHaveBeenLastCalledWith("[[60000000,5],[null,null],[null,35]]");
    fireEvent.click(screen.getByRole("button", { name: /Raw JSON/ }));
    expect(screen.getByLabelText("Income tax brackets (JSON)")).toHaveValue("[[60000000, 5], [null, 35]]");
  });

  it("flags invalid JSON", () => {
    const d: SettingDef = { key: "x.y", label: "Thing", type: "json" };
    render(<SettingRow def={d} saved="{}" draft="{oops" onChange={() => {}} error="Not valid JSON." />);
    expect(screen.getByText("Not valid JSON yet.")).toBeInTheDocument();
    expect(screen.getByText("Not valid JSON.")).toBeInTheDocument();
  });
});
