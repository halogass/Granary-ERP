import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";

const push = vi.fn();
vi.mock("next/navigation", () => ({ useRouter: () => ({ push }) }));

import { DataTable, type ColumnDef } from "./DataTable";

interface Row { id: string; name: string; amount: number }
const rows: Row[] = [
  { id: "a", name: "Beta", amount: 20 },
  { id: "b", name: "Alpha", amount: 300 },
];
const columns: ColumnDef<Row>[] = [
  { accessorKey: "name", header: "Name", cell: (c) => <span>{c.getValue<string>()} <button type="button">Edit</button></span> },
  { accessorKey: "amount", header: "Amount", meta: { numeric: true } },
];

describe("DataTable", () => {
  it("unfolds a row's details under it on click, one at a time", () => {
    render(<DataTable columns={columns} data={rows} getRowId={(r) => r.id} renderDetails={(r) => <p>Details of {r.name}</p>} />);
    fireEvent.click(screen.getByText("Beta"));
    const details = screen.getByText("Details of Beta");
    // the details row sits right after the clicked row
    const detailRow = details.closest("tr")!;
    expect(detailRow.previousElementSibling?.getAttribute("data-row-id")).toBe("a");
    expect(screen.getAllByRole("button", { name: "Hide details" })).toHaveLength(1);

    fireEvent.click(screen.getByText("Alpha"));
    expect(screen.queryByText("Details of Beta")).toBeNull();
    expect(screen.getByText("Details of Alpha")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Alpha"));
    expect(screen.queryByText("Details of Alpha")).toBeNull();
  });

  it("does not toggle when a button inside the row is clicked", () => {
    render(<DataTable columns={columns} data={rows} getRowId={(r) => r.id} renderDetails={(r) => <p>Details of {r.name}</p>} />);
    fireEvent.click(within(screen.getByText("Beta").closest("tr")!).getByRole("button", { name: "Edit" }));
    expect(screen.queryByText("Details of Beta")).toBeNull();
  });

  it("opens the chevron by keyboard with aria-expanded", () => {
    render(<DataTable columns={columns} data={rows} getRowId={(r) => r.id} renderDetails={() => <p>Open</p>} />);
    const [first] = screen.getAllByRole("button", { name: "Show details" });
    fireEvent.click(first);
    expect(screen.getByRole("button", { name: "Hide details" })).toHaveAttribute("aria-expanded", "true");
  });

  it("sorts by a header and right-aligns numeric columns", () => {
    render(<DataTable columns={columns} data={rows} getRowId={(r) => r.id} />);
    const cell = screen.getByText("300").closest("td")!;
    expect(cell.className).toContain("text-right");
    fireEvent.click(screen.getByRole("button", { name: /Name/ }));
    const names = screen.getAllByRole("row").slice(1).map((r) => r.querySelector("td")?.textContent);
    expect(names[0]).toContain("Alpha");
    expect(screen.getByRole("columnheader", { name: /Name/ })).toHaveAttribute("aria-sort", "ascending");
  });

  it("navigates to rowHref on click and Enter", () => {
    render(<DataTable columns={columns} data={rows} getRowId={(r) => r.id} rowHref={(r) => `/x/${r.id}`} />);
    fireEvent.click(screen.getByText("Beta"));
    expect(push).toHaveBeenCalledWith("/x/a");
    const row = screen.getByText("Alpha").closest("tr")!;
    fireEvent.keyDown(row, { key: "Enter" });
    expect(push).toHaveBeenCalledWith("/x/b");
  });

  it("shows the empty state, and skeleton rows while loading", () => {
    const { rerender } = render(<DataTable columns={columns} data={[]} empty={{ title: "No invoices yet" }} />);
    expect(screen.getByText("No invoices yet")).toBeInTheDocument();
    rerender(<DataTable columns={columns} data={[]} loading />);
    expect(screen.queryByText("No invoices yet")).toBeNull();
    expect(screen.getByRole("table")).toHaveAttribute("aria-busy", "true");
  });
});

describe("DataTable on phones (card mode)", () => {
  interface Deal { id: string; name: string; client: string; status: string; amount: number; owner: string }
  const deals: Deal[] = [
    { id: "d1", name: "Office laptops", client: "Acme Ltd", status: "open", amount: 500, owner: "Diana" },
    { id: "d2", name: "Laptop upgrade", client: "Globex Corp", status: "won", amount: 90, owner: "" },
  ];
  const onDelete = vi.fn();
  const onToggle = vi.fn();
  const dealColumns: ColumnDef<Deal>[] = [
    { id: "select", enableSorting: false, header: () => <input type="checkbox" aria-label="Select every deal" />,
      cell: (c) => <input type="checkbox" aria-label={`Select ${c.row.original.name}`} onChange={() => onToggle(c.row.original.id)} /> },
    { accessorKey: "name", header: "Deal" },
    { accessorKey: "client", header: "Client", meta: { hideOnMobile: true } },
    { accessorKey: "status", header: "Status", cell: (c) => <span data-testid="pill">{c.getValue<string>()}</span> },
    { accessorKey: "amount", header: "Amount", meta: { numeric: true, hideOnMobile: true } },
    { accessorKey: "owner", header: "Owner", meta: { hideOnMobile: true }, cell: (c) => c.getValue<string>() || null },
    { id: "actions", header: "", enableSorting: false,
      cell: (c) => <button type="button" onClick={() => onDelete(c.row.original.id)}>Delete</button> },
  ];

  let original: typeof window.matchMedia;
  beforeEach(() => {
    original = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      matches: query.includes("max-width"), media: query, onchange: null,
      addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
  });
  afterEach(() => { window.matchMedia = original; });

  const cardOf = (text: string) => screen.getByText(text).closest("li")!;

  it("renders a card per row with every column, hideOnMobile ones included", () => {
    render(<DataTable label="Deals" columns={dealColumns} data={deals} getRowId={(r) => r.id} />);
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.getByRole("list", { name: "Deals" }).children).toHaveLength(2);
    const card = within(cardOf("Office laptops"));
    for (const label of ["Client", "Amount", "Owner"]) expect(card.getByText(label).tagName).toBe("DT");
    expect(card.getByText("Acme Ltd").tagName).toBe("DD");
    expect(card.getByText("500")).toBeInTheDocument();
    expect(card.getByText("Diana")).toBeInTheDocument();
    expect(card.getByTestId("pill")).toHaveTextContent("open");
    // the status and the title are not repeated as label: value pairs
    expect(card.queryByText("Status")).toBeNull();
    expect(card.queryByText("Deal")).toBeNull();
  });

  it("shows row actions as buttons and selection checkboxes that do not open the row", () => {
    const rowClick = vi.fn();
    render(<DataTable columns={dealColumns} data={deals} getRowId={(r) => r.id} onRowClick={rowClick} />);
    const card = within(cardOf("Office laptops"));
    fireEvent.click(card.getByRole("button", { name: "Delete" }));
    expect(onDelete).toHaveBeenCalledWith("d1");
    fireEvent.click(card.getByRole("checkbox", { name: "Select Office laptops" }));
    expect(onToggle).toHaveBeenCalledWith("d1");
    expect(rowClick).not.toHaveBeenCalled();
    expect(screen.getByRole("checkbox", { name: "Select every deal" })).toBeInTheDocument();
    fireEvent.click(screen.getByText("Office laptops"));
    expect(rowClick).toHaveBeenCalledTimes(1);
  });

  it("unfolds the same details on tap or with the details button", () => {
    render(<DataTable columns={dealColumns} data={deals} getRowId={(r) => r.id} renderDetails={(r) => <p>Details of {r.name}</p>} />);
    fireEvent.click(screen.getByText("Office laptops"));
    expect(screen.getByText("Details of Office laptops")).toBeInTheDocument();
    expect(within(cardOf("Office laptops")).getByRole("button", { name: "Hide details" })).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(within(cardOf("Laptop upgrade")).getByRole("button", { name: "Show details" }));
    expect(screen.queryByText("Details of Office laptops")).toBeNull();
    expect(screen.getByText("Details of Laptop upgrade")).toBeInTheDocument();
  });

  it("sorts with the Sort by select", () => {
    render(<DataTable columns={dealColumns} data={deals} getRowId={(r) => r.id} />);
    const select = screen.getByRole("combobox", { name: "Sort by" });
    const values = Array.from((select as HTMLSelectElement).options).map((o) => o.value);
    expect(values).toContain("amount:desc");
    expect(values).not.toContain("actions:asc");
    expect(values).not.toContain("select:asc");
    fireEvent.change(select, { target: { value: "name:asc" } });
    const order = () => screen.getAllByRole("listitem").map((li) => li.getAttribute("data-row-id"));
    expect(order()).toEqual(["d2", "d1"]);
    fireEvent.change(select, { target: { value: "amount:desc" } });
    expect(order()).toEqual(["d1", "d2"]);
  });

  it("navigates to rowHref on tap, and shows the empty state", () => {
    const { rerender } = render(<DataTable columns={dealColumns} data={deals} getRowId={(r) => r.id} rowHref={(r) => `/deals/${r.id}`} />);
    fireEvent.click(screen.getByText("Laptop upgrade"));
    expect(push).toHaveBeenCalledWith("/deals/d2");
    rerender(<DataTable columns={dealColumns} data={[]} empty={{ title: "No deals yet" }} />);
    expect(screen.getByText("No deals yet")).toBeInTheDocument();
  });
});
