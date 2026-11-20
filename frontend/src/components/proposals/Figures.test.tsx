import { describe, it, expect, vi } from "vitest";
import { useState } from "react";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import FigureDataPanel from "./FigureDataPanel";
import FigureBlock from "./FigureBlock";
import ProposalEditor, { type Block } from "./ProposalEditor";
import {
  addCategory, addEdge, addNode, iconLabel, parseNumber, removeCategory, removeGroup, removeNode, setChartKind, setValue, sourceLine,
  type ChartSpec, type DiagramSpec, type FigureData, type FigureSpec,
} from "@/lib/proposalVisuals";

const chart: ChartSpec = {
  kind: "bar", title: "Users per site", language: "en", unit: "users", x_label: null, y_label: null,
  categories: ["Gate", "Yard"], series: [{ name: "Now", values: [4, 6] }], source: { type: "customer", note: "site survey" },
};
const diagram: DiagramSpec = {
  kind: "architecture", title: "System", language: "en", direction: "LR",
  groups: [{ id: "g1", label: "Plant", style: "site" }],
  nodes: [{ id: "n1", label: "Laptop", icon: "laptop", sku: null, group: "g1", count: 4 },
          { id: "n2", label: "App server", icon: "server", sku: "SRV-1", group: "g1", count: 1 }],
  edges: [{ from: "n1", to: "n2", kind: "poe", label: null }],
};

/** The panel as the editor uses it: it keeps the spec and reports every change. */
function Harness({ spec, onSpec }: { spec: FigureSpec; onSpec: (s: FigureSpec) => void }) {
  const [s, setS] = useState(spec);
  return <FigureDataPanel spec={s} editable onChange={(n) => { setS(n); onSpec(n); }} />;
}
const last = (fn: ReturnType<typeof vi.fn>) => fn.mock.calls.at(-1)![0];

describe("figure spec edits", () => {
  it("keeps one value per category when categories and series change", () => {
    let s = addCategory(chart);
    expect(s.categories).toEqual(["Gate", "Yard", ""]);
    expect(s.series![0].values).toEqual([4, 6, 0]);
    s = setValue(s, 2, 0, 9);
    s = removeCategory(s, 0);
    expect(s.categories).toEqual(["Yard", ""]);
    expect(s.series![0].values).toEqual([6, 9]);
  });

  it("removes a node's links and ungroups a removed group's nodes", () => {
    expect(removeNode(diagram, "n1").edges).toEqual([]);
    expect(removeGroup(diagram, "g1").nodes.every((n) => n.group === null)).toBe(true);
    const added = addNode(diagram);
    expect(added.nodes.at(-1)!.id).toBe("n3");
    expect(addEdge(diagram).edges.at(-1)).toMatchObject({ from: "n1", to: "n2" });
  });

  it("reads typed numbers in either decimal style and refuses words", () => {
    expect(parseNumber("1,5")).toBe(1.5);
    expect(parseNumber(" 12 ")).toBe(12);
    expect(parseNumber("12a")).toBeNull();
    expect(parseNumber("")).toBeNull();
  });

  it("gives a gantt a first task and drops stacking from kinds without it", () => {
    const g = setChartKind({ ...chart, stacked: true }, "gantt");
    expect(g.tasks).toHaveLength(1);
    expect(g.time_unit).toBe("week");
    expect(g.stacked).toBeUndefined();
  });

  it("prints a source line only for customer and analysis numbers", () => {
    expect(sourceLine(chart)).toBe("Source: site survey");
    expect(sourceLine({ ...chart, source: { type: "analysis", note: "pipeline, Q3", from: ["get_pipeline"] } })).toBe("Source: pipeline, Q3");
    expect(sourceLine({ ...chart, source: { type: "erp", ref: "proposal.phases" } })).toBeNull();
    expect(iconLabel("networkSwitch")).toBe("Network Switch");
    expect(iconLabel("router4g")).toBe("Router 4G");
  });
});

describe("figure data panels", () => {
  it("grid edits produce the right chart spec", async () => {
    const onSpec = vi.fn();
    render(<Harness spec={chart} onSpec={onSpec} />);
    const cell = screen.getByLabelText("Yard, Now");
    await userEvent.clear(cell);
    await userEvent.type(cell, "7,5");
    expect((last(onSpec) as ChartSpec).series![0].values).toEqual([4, 7.5]);
    await userEvent.click(screen.getByRole("button", { name: "Series" }));
    await userEvent.type(screen.getByLabelText("Series 2 name"), "Phase 2");
    await userEvent.click(screen.getByRole("button", { name: "Category" }));
    await userEvent.type(screen.getByLabelText("Category 3"), "Office");
    const s = last(onSpec) as ChartSpec;
    expect(s.categories).toEqual(["Gate", "Yard", "Office"]);
    expect(s.series).toEqual([{ name: "Now", values: [4, 7.5, 0] }, { name: "Phase 2", values: [0, 0, 0] }]);
    await userEvent.click(screen.getByRole("button", { name: "Remove category 1" }));
    expect((last(onSpec) as ChartSpec).series![1].values).toEqual([0, 0]);
  });

  it("edits gantt tasks and milestones", async () => {
    const onSpec = vi.fn();
    render(<Harness spec={{ ...chart, kind: "gantt", time_unit: "week", categories: [], series: [],
      tasks: [{ name: "Survey", start: 0, length: 1, group: null }] }} onSpec={onSpec} />);
    await userEvent.click(screen.getByRole("button", { name: "Task" }));
    await userEvent.type(screen.getByLabelText("Task 2"), "Install");
    const len = screen.getByLabelText("Task 2 length");
    await userEvent.clear(len);
    await userEvent.type(len, "3");
    await userEvent.click(screen.getByRole("button", { name: "Milestone" }));
    await userEvent.type(screen.getByLabelText("Milestone 1"), "Go live");
    const s = last(onSpec) as ChartSpec;
    expect(s.tasks).toEqual([{ name: "Survey", start: 0, length: 1, group: null }, { name: "Install", start: 1, length: 3, group: null }]);
    expect(s.milestones).toEqual([{ name: "Go live", at: 4 }]);
  });

  it("edits scatter points and combo series", async () => {
    const onSpec = vi.fn();
    const { unmount } = render(<Harness spec={{ ...chart, kind: "scatter", series: [{ name: "Sites", values: [], points: [{ x: 1, y: 2 }] }] }} onSpec={onSpec} />);
    await userEvent.click(screen.getByRole("button", { name: "Point" }));
    const y = screen.getByLabelText("Series 1 point 2 y");
    await userEvent.clear(y);
    await userEvent.type(y, "8");
    expect((last(onSpec) as ChartSpec).series![0].points).toEqual([{ x: 1, y: 2 }, { x: 0, y: 8, label: null }]);
    unmount();
    render(<Harness spec={{ ...chart, kind: "combo" }} onSpec={onSpec} />);
    await userEvent.selectOptions(screen.getByLabelText("Series 1 drawn as"), "line");
    await userEvent.selectOptions(screen.getByLabelText("Series 1 axis"), "right");
    expect((last(onSpec) as ChartSpec).series![0]).toMatchObject({ type: "line", axis: "right" });
  });

  it("edits diagram nodes with the icon picker and links between them", async () => {
    const onSpec = vi.fn();
    render(<Harness spec={diagram} onSpec={onSpec} />);
    await userEvent.selectOptions(screen.getByLabelText("Item 1 icon"), "database");
    expect((last(onSpec) as DiagramSpec).nodes[0].icon).toBe("database");
    expect(screen.getByLabelText("Item 1 SKU")).toHaveAttribute("placeholder", "optional");
    await userEvent.click(screen.getByRole("button", { name: "Item" }));
    await userEvent.type(screen.getByLabelText("Item 3 label"), "Router");
    await userEvent.click(screen.getByRole("button", { name: "Link" }));
    await userEvent.selectOptions(screen.getByLabelText("Link 2 to"), "n3");
    await userEvent.selectOptions(screen.getByLabelText("Link 2 kind"), "4g");
    let s = last(onSpec) as DiagramSpec;
    expect(s.edges[1]).toEqual({ from: "n1", to: "n3", kind: "4g", label: null });
    await userEvent.click(screen.getByRole("button", { name: "Remove item 1" }));
    s = last(onSpec) as DiagramSpec;
    expect(s.nodes.map((n) => n.id)).toEqual(["n2", "n3"]);
    expect(s.edges).toEqual([]);
  });

  it("shows a custom drawing as read only", () => {
    render(<FigureDataPanel spec={{ kind: "custom_svg", title: "x", svg: "<svg><script>alert(1)</script></svg>" }} editable onChange={vi.fn()} />);
    expect(screen.getByLabelText("Custom drawing")).toHaveTextContent(/ask Nary to change this figure/);
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});

describe("figure blocks in the proposal editor", () => {
  const data: FigureData = { spec: chart, caption: "Users", size: "full", svg_asset_id: "svg-1", png_asset_id: null, checked: { ok: false, notes: ["Labels overlap"] } };

  it("shows the stored drawing as an image, its source and warnings, never inline SVG", async () => {
    const load = vi.fn().mockResolvedValue(new Blob(["<svg/>"], { type: "image/svg+xml" }));
    globalThis.URL.createObjectURL ??= () => "blob:x";
    globalThis.URL.revokeObjectURL ??= () => undefined;
    const { container } = render(<FigureBlock blockId="b1" data={data} editable={false} loadAsset={load} onChange={vi.fn()} />);
    expect(load).toHaveBeenCalledWith("svg-1");
    expect(await screen.findByRole("img", { name: "Users" })).toBeInTheDocument();
    expect(screen.getByText("Source: site survey")).toBeInTheDocument();
    expect(screen.getByText("Labels overlap")).toBeInTheDocument();
    expect(container.querySelector("svg:not([aria-hidden])")).toBeNull();
  });

  it("previews as a data URL image, redraws and asks Nary about this figure", async () => {
    const onPreview = vi.fn().mockResolvedValue({ svg: "<svg xmlns='http://www.w3.org/2000/svg'><text>&lt;script&gt;</text></svg>", warnings: ["Long label cut"] });
    const onAsk = vi.fn().mockResolvedValue(undefined);
    const onChange = vi.fn();
    render(<FigureBlock blockId="b1" data={{ ...data, svg_asset_id: null, checked: null }} editable loadAsset={vi.fn()} onChange={onChange}
      onPreview={onPreview} onAsk={onAsk} />);
    await userEvent.click(screen.getByRole("button", { name: /Preview/ }));
    expect(onPreview).toHaveBeenCalledWith({ spec: chart, size: "full", block_id: "b1" });
    const img = await screen.findByRole("img", { name: "Users" });
    expect(img.getAttribute("src")).toMatch(/^data:image\/svg\+xml/);
    expect(screen.getByText("Long label cut")).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText("Figure size"), "half");
    expect(last(onChange).size).toBe("half");
    fireEvent.change(screen.getByLabelText("Ask Nary to change this figure"), { target: { value: "add a pilot week" } });
    await userEvent.click(screen.getByRole("button", { name: /Ask Nary/ }));
    expect(onAsk).toHaveBeenCalledWith("add a pilot week");
  });

  it("adds a chart or a diagram from the Add figure menu", async () => {
    const onChange = vi.fn();
    const blocks: Block[] = [{ section: "architecture", block_type: "text", data: { md: "x" } }];
    render(<ProposalEditor sections={[{ key: "architecture", title: "Solution Architecture" }]} blocks={blocks}
      assets={[]} language="en" editable loadAsset={async () => new Blob()} onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: "Add figure to Solution Architecture" }));
    await userEvent.click(screen.getByRole("button", { name: "Add Diagram to Solution Architecture" }));
    const added = last(onChange)[1] as Block;
    expect(added.block_type).toBe("diagram");
    expect((added.data as FigureData).spec).toMatchObject({ kind: "architecture", language: "en" });
    await userEvent.click(screen.getByRole("button", { name: "Add figure to Solution Architecture" }));
    expect(screen.getByRole("button", { name: "Add Chart to Solution Architecture" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add Image to Solution Architecture" })).toBeInTheDocument();
  });
});
