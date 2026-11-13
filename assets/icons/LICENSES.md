# Icon packs

The SVGs under this folder come from these free icon sets (recoloured white
so the app can tint them; otherwise unchanged). Each keeps its own licence.

| Folder | Set | Licence | Source |
|---|---|---|---|
| lucide | Lucide 1.72.0 | ISC | https://lucide.dev |
| tabler | Tabler Icons 3.51.0 | MIT | https://tabler.io/icons |
| phosphor | Phosphor Icons 2.1.1 | MIT | https://phosphoricons.com |
| codicons | VS Code Codicons 0.0.46 | CC BY 4.0 (© Microsoft) | https://github.com/microsoft/vscode-codicons |
| fluent | Fluent UI System Icons 1.1.345 | MIT (© Microsoft) | https://github.com/microsoft/fluentui-system-icons |
| material | Material Symbols 0.49.0 | Apache 2.0 (© Google) | https://fonts.google.com/icons |
| iconoir | Iconoir 7.12.2 | MIT | https://iconoir.com |
| bootstrap | Bootstrap Icons 1.13.2 | MIT | https://icons.getbootstrap.com |

Where a set has no icon for something, one from the set nearest in look
stands in: iconoir/matview is Lucide's "layers"; bootstrap/function and
bootstrap/sequence are Material Symbols' "function" and
"format_list_numbered".

Made by `build_icons.py` (picks, recolours, writes `src/icons_data.rs`).
