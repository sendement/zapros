"""zapros' icon packs: each concept from each pack (by its own name for it),
what a pack lacks taken from the pack nearest in look; written white (the
app tints them) into zapros/assets/icons/<pack>/<concept>.svg, with
src/icons_data.rs listing them.

  python3 build_icons.py
"""

import json
import re
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
CACHE = HERE / "packs"
ZAPROS = Path.home() / "zapros"

# id, label, npm package, folder, file pattern, licence, borrow from (nearest look first)
PACKS = [
    ("lucide", "Lucide", "lucide-static@1.72.0", "icons", "{}.svg", "ISC", ["tabler", "iconoir"]),
    ("tabler", "Tabler", "@tabler/icons@3.51.0", "icons/outline", "{}.svg", "MIT", ["lucide", "iconoir"]),
    ("phosphor", "Phosphor", "@phosphor-icons/core@2.1.1", "assets/regular", "{}.svg", "MIT", ["lucide", "fluent"]),
    ("codicons", "Codicons", "@vscode/codicons@0.0.46-24", "src/icons", "{}.svg", "CC BY 4.0", ["fluent", "lucide"]),
    ("fluent", "Fluent", "@fluentui/svg-icons@1.1.345", "icons", "{}_20_regular.svg", "MIT", ["phosphor", "codicons"]),
    ("material", "Material Symbols", "@material-symbols/svg-400@0.49.0", "rounded", "{}.svg", "Apache 2.0", ["bootstrap", "tabler"]),
    ("iconoir", "Iconoir", "iconoir@7.12.2", "icons/regular", "{}.svg", "MIT", ["lucide", "tabler"]),
    ("bootstrap", "Bootstrap", "bootstrap-icons@1.13.2", "icons", "{}.svg", "MIT", ["material", "tabler"]),
]

# Concept (an Ic variant in snake case) → names to try, in any pack.
GENERIC = {
    "connection": "database server db",
    "schema": "folder-cog folder-settings folder",
    "folder": "folder",
    "table": "table table-2",
    "view": "eye visibility",
    "matview": "layers layers-2 layer stack",
    "function": "square-function math-function function sigma-function",
    "procedure": "settings-automation workflow gear-six gear settings",
    "package": "package package_2 box-seam box-iso box",
    "trigger": "zap bolt lightning flash",
    "sequence": "list-ordered list-numbers format_list_numbered numbered-list-left",
    "index": "bookmark bookmark-simple",
    "job": "alarm-clock alarm clock",
    "dictionary": "book-open menu_book open-book book",
    "type": "type text-t title text",
    "synonym": "link link-simple",
    "column": "columns-3 columns view_column view-columns-3 layout-three-columns",
    "key": "key-round key",
    "run": "play player-play play_arrow",
    "run_all": "fast-forward player-track-next fast_forward forward",
    "stop": "square player-stop stop",
    "explain": "search-code zoom-code query_stats magnifying-glass search",
    "format": "wand-sparkles wand magic-wand wand_stars magic",
    "commit": "check check-lg",
    "rollback": "undo-2 arrow-back-up arrow-counter-clockwise undo arrow-counterclockwise",
    "notebook": "notebook notebook-pen journal-code",
    "files": "folder-open folder2-open folder_open",
    "file": "file file-text file-earmark",
    "connections": "plug plug-2 power",
    "settings": "settings gear",
    "help": "circle-help help-circle question question-circle help",
    "outline": "list-tree tree-structure account_tree list-nested",
    "filter": "filter funnel filter_alt",
    "export": "download download-simple",
    "copy": "copy content_copy files",
    "grip": "grip-vertical gripper dots-six-vertical drag_indicator re_order_dots_vertical drag",
    "done": "circle-check check-circle check_circle checkmark_circle check-circle-solid",
}

# A pack's own name for a concept, where it differs.
OWN = {
    "codicons": {"done": "pass", "grip": "gripper", "function": "symbol-method", "procedure": "gear", "trigger": "symbol-event", "job": "watch", "type": "text-size",
                 "column": "symbol-field", "run_all": "run-all", "stop": "debug-stop", "rollback": "discard", "files": "files",
                 "file": "file", "settings": "settings-gear", "export": "export", "outline": "list-tree", "help": "question"},
    "fluent": {"copy": "copy", "grip": "re_order_dots_vertical", "done": "checkmark_circle", "function": "math_formula", "procedure": "flow", "package": "box", "trigger": "flash", "sequence": "text_number_list_ltr",
               "job": "clock_alarm", "dictionary": "book_open", "type": "text_t", "column": "column_triple", "run_all": "fast_forward",
               "commit": "checkmark", "rollback": "arrow_undo", "files": "folder_open", "file": "document", "connections": "plug_connected",
               "filter": "filter", "export": "arrow_download", "outline": "text_bullet_list_tree", "help": "question_circle",
               "explain": "search", "format": "wand"},
    "material": {"copy": "content_copy", "grip": "drag_indicator", "done": "check_circle", "files": "folder_open", "file": "draft", "connections": "power", "filter": "filter_alt", "notebook": "note_stack",
                 "format": "wand_stars", "function": "function", "help": "help", "outline": "account_tree"},
    "iconoir": {"grip": "drag", "done": "check-circle", "function": "sigma-function", "sequence": "numbered-list-left", "type": "text", "column": "view-columns-3", "notebook": "journal",
                "files": "multiple-pages", "file": "page", "connections": "ev-plug", "dictionary": "open-book", "outline": "list", "help": "help-circle",
                "procedure": "settings", "format": "magic-wand", "rollback": "undo", "export": "download"},
    "bootstrap": {"grip": "grip-vertical", "done": "check-circle", "files": "folder2-open", "file": "file-earmark-text", "outline": "diagram-3", "notebook": "journal-code", "filter": "funnel",
                  "help": "question-circle", "connections": "plug"},
    "tabler": {"done": "circle-check", "type": "typography", "function": "math-function", "outline": "list-tree", "help": "help-circle", "file": "file"},
    "phosphor": {"grip": "dots-six-vertical", "done": "check-circle", "function": "function", "outline": "tree-structure", "help": "question", "file": "file", "connections": "plug"},
    "lucide": {"done": "circle-check", "function": "square-function", "help": "circle-help"},
}


def get(url):
    with urllib.request.urlopen(url, timeout=30) as r:
        return r.read()


def files_of(pkg):
    cache = CACHE / (pkg.replace("/", "_") + ".json")
    if not cache.exists():
        cache.write_bytes(get(f"https://data.jsdelivr.com/v1/package/npm/{pkg}/flat"))
    return {f["name"].lstrip("/") for f in json.loads(cache.read_text())["files"]}


def white(svg: str) -> str:
    """All its ink white (tinted by the app), no size of its own."""
    s = svg
    for c in ["currentColor", "#000000", "#000", "#212121", "black"]:
        s = s.replace(f'"{c}"', '"#ffffff"')
    root = re.search(r"<svg[^>]*>", s).group(0)
    if "fill=" not in root and "stroke=" not in root:
        s = s.replace("<svg ", '<svg fill="#ffffff" ', 1)
    return s


def main():
    lists = {p[0]: files_of(p[2]) for p in PACKS}
    by_id = {p[0]: p for p in PACKS}

    def find(pid, concept):
        _, _, pkg, folder, pat, _, _ = by_id[pid]
        names = [OWN.get(pid, {}).get(concept)] + GENERIC[concept].split()
        for n in names:
            if n and f"{folder}/{pat.format(n)}" in lists[pid]:
                return (pid, n, f"https://cdn.jsdelivr.net/npm/{pkg}/{folder}/{pat.format(n)}")
        return None

    report = {}
    rows = []
    for pid, label, pkg, folder, pat, lic, borrow in PACKS:
        out = ZAPROS / "assets/icons" / pid
        out.mkdir(parents=True, exist_ok=True)
        for concept in GENERIC:
            hit = find(pid, concept)
            for b in borrow:
                if hit:
                    break
                hit = find(b, concept)
            if not hit:
                raise SystemExit(f"{pid}: nothing for {concept}")
            src, name, url = hit
            cache = CACHE / "raw" / src / f"{name}.svg"
            cache.parent.mkdir(parents=True, exist_ok=True)
            if not cache.exists():
                cache.write_bytes(get(url))
            (out / f"{concept}.svg").write_text(white(cache.read_text()))
            if src != pid:
                report.setdefault(pid, []).append(f"{concept} ← {src}:{name}")
            rows.append((pid, concept))
    for pid, b in report.items():
        print(pid, "borrows", ", ".join(b))

    # The Rust side: the packs and their files, by concept.
    concepts = list(GENERIC)
    lines = [
        "// Made by ~/icongen/build_icons.py — the icon packs' SVGs (white; tinted when drawn).",
        "",
        "/// (id, name, licence)",
        "pub const PACKS: &[(&str, &str, &str)] = &[",
    ]
    lines += [f'    ("{p[0]}", "{p[1]}", "{p[5]}"),' for p in PACKS]
    lines += ["];", "", f"pub const CONCEPTS: [&str; {len(concepts)}] = [" + ", ".join(f'"{c}"' for c in concepts) + "];", ""]
    lines += ["/// [pack][concept] → the SVG.", f"pub const SVG: [[&[u8]; {len(concepts)}]; {len(PACKS)}] = ["]
    for p in PACKS:
        lines.append("    [")
        for c in concepts:
            lines.append(f'        include_bytes!("../assets/icons/{p[0]}/{c}.svg"),')
        lines.append("    ],")
    lines.append("];")
    (ZAPROS / "src/icons_data.rs").write_text("\n".join(lines) + "\n")
    print(len(rows), "icons")


if __name__ == "__main__":
    main()
