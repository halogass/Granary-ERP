#!/usr/bin/env python3
"""Generate the Wiki's navigation from one tree: the sidebar, a breadcrumb and an "On this page" list at the top of
each page, and Previous / Next links at the bottom (the reading order follows the tree).

Run after adding, renaming or reordering a page: python3 scripts/wiki-nav.py
The generated blocks sit between <!-- nav:... --> markers, so the script can be re-run safely.
"""
import os
import re

WIKI = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "docs", "wiki")

# (title, page) for a page; (title, [children]) for a group. Order = reading order.
TREE = [
    ("Getting started", [("Installation", "Installation"), ("First Steps", "First-Steps"), ("FAQ", "FAQ")]),
    ("AI in Granary", [
        ("AI Across Granary", "AI-Across-Granary"),
        ("Nary AI Assistant", "Nary-AI-Assistant"),
        ("MCP for AI Clients", "MCP-for-AI-Clients"),
    ]),
    ("Using Granary", [
        ("Sell", [
            ("CRM", "CRM"), ("CRM Inboxes", "CRM-Inboxes"), ("Sales", "Sales"),
            ("Marketing", "Marketing"), ("Partners", "Partners"),
        ]),
        ("Approve and sign", [("Approvals and Signing", "Approvals-and-Signing")]),
        ("Money", [("Finance", "Finance"), ("Multi-Currency", "Multi-Currency")]),
        ("Buy and make", [
            ("Procurement", "Procurement"), ("Manufacturing and Inventory", "Manufacturing-and-Inventory"),
            ("Operations", "Operations"),
        ]),
        ("People", [("HR and Payroll", "HR-and-Payroll"), ("Face Clock-In", "Face-Clock-In")]),
        ("Work together", [
            ("Projects", "Projects"), ("R&D", "RnD"), ("Knowledge and Documents", "Knowledge-and-Documents"),
            ("Collaboration", "Collaboration"),
        ]),
    ]),
    ("Administration", [
        ("Configuration", "Configuration"), ("Administration", "Administration"),
        ("Backups and Upgrades", "Backups-and-Upgrades"), ("Troubleshooting", "Troubleshooting"),
    ]),
    ("Developers", [("Developer Guide", "Developer-Guide")]),
]
TOC_MIN_SECTIONS = 6  # pages with at least this many "## " sections get an "On this page" list


def walk(nodes, trail=()):
    """Yield (title, page, trail of group titles) in reading order."""
    for title, child in nodes:
        if isinstance(child, list):
            yield from walk(child, trail + (title,))
        else:
            yield title, child, trail


def sidebar():
    out = ["**[Granary](Home)**", ""]

    def emit(nodes, depth):
        for title, child in nodes:
            if isinstance(child, list):
                if depth == 0:
                    out.extend(["", f"**{title}**"])
                else:
                    out.append("  " * (depth - 1) + f"- *{title}*")
                emit(child, depth + 1)
            else:
                out.append("  " * max(depth - 1, 0) + f"- [{title}]({child})")

    emit(TREE, 0)
    return "\n".join(out).replace("\n\n\n", "\n\n").strip() + "\n"


def anchor(heading):
    """GitHub's heading anchor."""
    text = re.sub(r"[`*_]|\[([^\]]*)\]\([^)]*\)", r"\1", heading).strip().lower()
    return re.sub(r"[^a-z0-9 _-]", "", text).replace(" ", "-")


def strip_block(text, name):
    return re.sub(rf"\n?<!-- nav:{name} -->.*?<!-- /nav:{name} -->\n?", "\n", text, flags=re.S)


def main():
    pages = list(walk(TREE))
    missing = [p for _, p, _ in pages if not os.path.exists(os.path.join(WIKI, p + ".md"))]
    if missing:
        raise SystemExit(f"pages in the tree without a file: {missing}")
    listed = {p for _, p, _ in pages} | {"Home"}
    extra = [f[:-3] for f in os.listdir(WIKI) if f.endswith(".md") and not f.startswith("_") and f[:-3] not in listed]
    if extra:
        raise SystemExit(f"pages missing from the tree: {extra}")

    with open(os.path.join(WIKI, "_Sidebar.md"), "w") as fh:
        fh.write(sidebar())

    order = [("Granary", "Home", ())] + pages
    for i, (title, page, trail) in enumerate(order):
        path = os.path.join(WIKI, page + ".md")
        text = open(path).read()
        text = strip_block(strip_block(text, "top"), "bottom").strip("\n")
        lines = text.split("\n")
        # top block: after the H1 (or at the start)
        h1 = next((k for k, l in enumerate(lines) if l.startswith("# ")), None)
        top = []
        if page != "Home":
            crumbs = ["[Home](Home)", *trail, f"**{title}**"]
            top.append(" › ".join(crumbs))
        body = "\n".join(lines[(h1 + 1) if h1 is not None else 0:])
        sections = [m.group(1) for m in re.finditer(r"^## (.+)$", body, flags=re.M)]
        sections = [s for s in sections if s.strip().lower() != "related pages"]
        if len(sections) >= TOC_MIN_SECTIONS:
            if top:
                top.append("")
            top.append("**On this page:** " + " · ".join(f"[{s.strip()}](#{anchor(s)})" for s in sections))
        if top:
            block = "<!-- nav:top -->\n" + "\n".join(top) + "\n<!-- /nav:top -->"
            if h1 is not None:
                lines = lines[: h1 + 1] + ["", block] + lines[h1 + 1:]
            else:
                lines = [block, ""] + lines
        # bottom block: previous / up / next
        prev_ = order[i - 1] if i > 0 else None
        next_ = order[i + 1] if i + 1 < len(order) else None
        parts = []
        if prev_:
            parts.append(f"← Previous: [{prev_[0]}]({prev_[1]})")
        if next_:
            parts.append(f"Next: [{next_[0]}]({next_[1]}) →")
        bottom = "<!-- nav:bottom -->\n---\n\n" + " | ".join(parts) + "\n<!-- /nav:bottom -->"
        text = re.sub(r"\n{3,}", "\n\n", "\n".join(lines)).rstrip("\n") + "\n\n" + bottom + "\n"
        with open(path, "w") as fh:
            fh.write(text)
    print(f"sidebar + navigation written for {len(order)} pages")


if __name__ == "__main__":
    main()
